//! Inspection of an AppImage that is already on disk -- the file a browser
//! just downloaded, opened from the file manager.
//!
//! Nothing here executes the AppImage. Its own runtime is untrusted code by
//! definition (running it is the thing the user has not agreed to yet), so
//! the metadata is read the long way round: the ELF section table gives the
//! signature and update-info blobs and the offset of the appended squashfs,
//! and `unsquashfs` pulls the desktop entry and icon out of that image.

use super::appimage::{cache_dir, sanitize};
use serde::Serialize;
use std::ffi::CString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Clone, Debug, Default, Serialize)]
pub struct Inspection {
    pub path: String,
    pub file_name: String,
    pub size: u64,
    pub sha256: String,
    /// "type2" (ELF + squashfs), "type1" (ISO 9660) or "unknown".
    pub format: String,
    pub architecture: Option<String>,
    pub executable: bool,
    pub name: String,
    pub version: Option<String>,
    pub summary: Option<String>,
    pub categories: Vec<String>,
    pub exec: Option<String>,
    /// Extracted icon, as a file path the UI can load directly.
    pub icon: Option<String>,
    /// Publisher signature embedded in the image. Presence only: the key is
    /// not checked against anything, so this says "signed", not "trusted".
    pub signed: bool,
    pub update_info: Option<String>,
    /// Where the browser fetched it from (user.xdg.origin.url), if recorded.
    pub origin_url: Option<String>,
    /// Same name already integrated through the store.
    pub already_installed: bool,
    /// Why metadata is thin, when it is.
    pub warnings: Vec<String>,
}

struct Elf {
    machine: u16,
    sections: Vec<(String, u64, u64)>,
    squashfs_offset: u64,
}

pub async fn inspect(path: &Path) -> anyhow::Result<Inspection> {
    let path = tokio::fs::canonicalize(path).await?;
    let meta = tokio::fs::metadata(&path).await?;
    anyhow::ensure!(meta.is_file(), "not a regular file");
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut out = Inspection {
        path: path.to_string_lossy().into_owned(),
        file_name: file_name.clone(),
        size: meta.len(),
        executable: meta.permissions().mode() & 0o111 != 0,
        format: "unknown".into(),
        name: guess_name(&file_name),
        ..Default::default()
    };

    let head = read_range(&path, 0, 64).await?;
    let is_elf = head.len() >= 12 && &head[0..4] == b"\x7fELF";
    let magic = if is_elf && &head[8..10] == b"AI" {
        head[10]
    } else {
        0
    };
    out.origin_url = origin_url(&path);
    out.sha256 = sha256(&path).await.unwrap_or_default();

    // The "AI\x02" magic is optional in practice: electron-builder leaves it
    // out on purpose, so the appended squashfs is what identifies a type-2
    // image, and the magic only confirms it.
    let elf = if is_elf && magic != 1 {
        read_elf(&path).await
    } else {
        Err(anyhow::anyhow!("not an ELF image"))
    };
    if let Ok(elf) = &elf {
        let sb = read_range(&path, elf.squashfs_offset, 4)
            .await
            .unwrap_or_default();
        if sb == b"hsqs" || magic == 2 {
            out.format = "type2".into();
        }
    }
    if magic == 1 {
        out.format = "type1".into();
    }

    if out.format == "type2" {
        match elf {
            Ok(elf) => {
                out.architecture = match elf.machine {
                    0x3E => Some("x86_64".into()),
                    0xB7 => Some("aarch64".into()),
                    0x03 => Some("i386".into()),
                    0x28 => Some("armhf".into()),
                    _ => None,
                };
                for (name, offset, size) in &elf.sections {
                    match name.as_str() {
                        ".sha256_sig" => out.signed = section_nonempty(&path, *offset, *size).await,
                        ".upd_info" => {
                            let raw = read_range(&path, *offset, (*size).min(4096) as usize)
                                .await
                                .unwrap_or_default();
                            let text = String::from_utf8_lossy(&raw)
                                .trim_end_matches('\0')
                                .trim()
                                .to_string();
                            if !text.is_empty() {
                                out.update_info = Some(text);
                            }
                        }
                        _ => {}
                    }
                }
                if let Err(err) = extract_payload(&path, elf.squashfs_offset, &mut out).await {
                    out.warnings
                        .push(format!("could not read the app's metadata: {err}"));
                }
            }
            Err(err) => out.warnings.push(format!("unreadable ELF header: {err}")),
        }
    } else if out.format == "type1" {
        out.warnings
            .push("legacy type-1 AppImage: metadata cannot be read without running it".into());
    } else {
        out.warnings
            .push("file does not carry the AppImage magic".into());
    }

    let registry = crate::registry::load().await.unwrap_or_default();
    out.already_installed = registry.values().any(|entry| {
        entry.backend == "appimage"
            && entry
                .id
                .ends_with(&format!("/{}.AppImage", sanitize(&out.name)))
    });
    Ok(out)
}

/// `Beeper-4.3.123-x86_64.AppImage` -> `Beeper`. Only a fallback for when the
/// image carries no desktop entry.
fn guess_name(file_name: &str) -> String {
    let stem = file_name
        .strip_suffix(".AppImage")
        .or_else(|| file_name.strip_suffix(".appimage"))
        .unwrap_or(file_name);
    let mut parts = stem.split(['-', '_']);
    let first = parts.next().unwrap_or(stem);
    let mut name = first.to_string();
    for part in parts {
        // Stop at the first token that looks like a version or an arch.
        if part.chars().next().is_some_and(|c| c.is_ascii_digit())
            || matches!(
                part,
                "x86_64" | "x86" | "amd64" | "aarch64" | "arm64" | "linux" | "Linux"
            )
        {
            break;
        }
        name.push(' ');
        name.push_str(part);
    }
    name
}

async fn read_range(path: &Path, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let mut file = tokio::fs::File::open(path).await?;
    file.seek(std::io::SeekFrom::Start(offset)).await?;
    let mut buf = vec![0u8; len];
    let mut filled = 0;
    while filled < len {
        let n = file.read(&mut buf[filled..]).await?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    buf.truncate(filled);
    Ok(buf)
}

async fn section_nonempty(path: &Path, offset: u64, size: u64) -> bool {
    match read_range(path, offset, size.min(4096) as usize).await {
        Ok(bytes) => bytes.iter().any(|b| *b != 0),
        Err(_) => false,
    }
}

/// ELF64 little-endian section table: enough to name the AppImage sections
/// and find where the runtime ends and the squashfs begins.
async fn read_elf(path: &Path) -> anyhow::Result<Elf> {
    let header = read_range(path, 0, 64).await?;
    anyhow::ensure!(header.len() == 64, "short ELF header");
    anyhow::ensure!(
        header[4] == 2 && header[5] == 1,
        "not a 64-bit little-endian ELF"
    );
    let u16_at = |b: &[u8], at: usize| u16::from_le_bytes([b[at], b[at + 1]]);
    let u64_at = |b: &[u8], at: usize| {
        let mut a = [0u8; 8];
        a.copy_from_slice(&b[at..at + 8]);
        u64::from_le_bytes(a)
    };
    let machine = u16_at(&header, 18);
    let shoff = u64_at(&header, 40);
    let shentsize = u16_at(&header, 58) as u64;
    let shnum = u16_at(&header, 60) as u64;
    let shstrndx = u16_at(&header, 62) as usize;
    anyhow::ensure!(
        shentsize == 64 && shnum > 0 && shnum < 4096,
        "implausible section table"
    );

    let table = read_range(path, shoff, (shentsize * shnum) as usize).await?;
    anyhow::ensure!(
        table.len() as u64 == shentsize * shnum,
        "short section table"
    );
    let entry = |i: usize| -> (u32, u64, u64) {
        let e = &table[i * 64..(i + 1) * 64];
        (
            u32::from_le_bytes([e[0], e[1], e[2], e[3]]),
            u64_at(e, 24),
            u64_at(e, 32),
        )
    };
    anyhow::ensure!(shstrndx < shnum as usize, "bad shstrndx");
    let (_, str_off, str_size) = entry(shstrndx);
    let strtab = read_range(path, str_off, str_size.min(1 << 20) as usize).await?;
    let name_at = |start: usize| -> String {
        strtab
            .get(start..)
            .and_then(|s| s.iter().position(|b| *b == 0).map(|end| &s[..end]))
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .unwrap_or_default()
    };
    let sections = (0..shnum as usize)
        .map(|i| {
            let (name_off, off, size) = entry(i);
            (name_at(name_off as usize), off, size)
        })
        .collect();
    Ok(Elf {
        machine,
        sections,
        squashfs_offset: shoff + shentsize * shnum,
    })
}

/// Pull the desktop entry and icon out of the squashfs without mounting or
/// running anything. Extracted files land under the store's cache, keyed by
/// content hash so a re-open of the same file is free.
async fn extract_payload(path: &Path, offset: u64, out: &mut Inspection) -> anyhow::Result<()> {
    let key = if out.sha256.len() >= 16 {
        &out.sha256[..16]
    } else {
        "unknown"
    };
    let dir = cache_dir().join("inspect").join(key);
    if !dir.join(".done").exists() {
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await?;
        let status = Command::new("unsquashfs")
            .args(["-o", &offset.to_string(), "-d"])
            .arg(&dir)
            .args(["-f", "-n", "-q", "-no-xattrs"])
            .arg(path)
            .args([
                "*.desktop",
                ".DirIcon",
                "*.png",
                "*.svg",
                "usr/share/metainfo/*.xml",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map_err(|e| anyhow::anyhow!("unsquashfs is not available ({e})"))?;
        anyhow::ensure!(status.success(), "unsquashfs could not read the image");
        tokio::fs::write(dir.join(".done"), b"").await?;
    }

    let mut entries = tokio::fs::read_dir(&dir).await?;
    let mut desktop: Option<PathBuf> = None;
    let mut root_icons: Vec<PathBuf> = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let p = entry.path();
        match p.extension().and_then(|e| e.to_str()) {
            Some("desktop") if desktop.is_none() => desktop = Some(p),
            Some("png") | Some("svg") => root_icons.push(p),
            _ => {}
        }
    }

    let mut icon_name: Option<String> = None;
    if let Some(desktop) = desktop {
        let text = tokio::fs::read_to_string(&desktop).await?;
        let mut in_entry = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_entry = line == "[Desktop Entry]";
                continue;
            }
            if !in_entry {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            match k.trim() {
                "Name" => out.name = v.trim().to_string(),
                "Comment" => out.summary = Some(v.trim().to_string()),
                "Exec" => out.exec = Some(v.trim().to_string()),
                "Icon" => icon_name = Some(v.trim().to_string()),
                "Categories" => {
                    out.categories = v
                        .split(';')
                        .map(str::trim)
                        .filter(|c| !c.is_empty())
                        .map(String::from)
                        .collect()
                }
                "X-AppImage-Version" => out.version = Some(v.trim().to_string()),
                _ => {}
            }
        }
    } else {
        out.warnings
            .push("no desktop entry inside the image".into());
    }

    // .DirIcon and the root-level icon are normally symlinks into
    // usr/share/icons, which the first pass did not pull out; follow the link
    // and fetch just that one file. Order: .DirIcon, the icon the desktop
    // entry names, anything else at the root.
    let mut candidates = vec![dir.join(".DirIcon")];
    if let Some(name) = &icon_name {
        candidates.extend(
            root_icons
                .iter()
                .filter(|p| p.file_stem().and_then(|s| s.to_str()) == Some(name.as_str()))
                .cloned(),
        );
    }
    candidates.extend(root_icons);
    let mut icon = None;
    for candidate in candidates {
        if let Some(found) = materialize(path, offset, &dir, &candidate).await {
            icon = Some(found);
            break;
        }
    }
    out.icon = icon.map(|p| p.to_string_lossy().into_owned());
    Ok(())
}

/// Turn an extracted entry into a real file: a plain file is returned as is,
/// a symlink is followed inside the image and its target extracted on demand.
async fn materialize(image: &Path, offset: u64, dir: &Path, entry: &Path) -> Option<PathBuf> {
    let meta = tokio::fs::symlink_metadata(entry).await.ok()?;
    if meta.is_file() {
        return Some(entry.to_path_buf());
    }
    if !meta.file_type().is_symlink() {
        return None;
    }
    let target = tokio::fs::read_link(entry).await.ok()?;
    let rel = target.strip_prefix("/").unwrap_or(&target).to_path_buf();
    let full = dir.join(&rel);
    if !full.is_file() {
        let _ = Command::new("unsquashfs")
            .args(["-o", &offset.to_string(), "-d"])
            .arg(dir)
            .args(["-f", "-n", "-q", "-no-xattrs"])
            .arg(image)
            .arg(&rel)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
    }
    full.is_file().then_some(full)
}

async fn sha256(path: &Path) -> anyhow::Result<String> {
    let output = Command::new("sha256sum").arg(path).output().await?;
    anyhow::ensure!(output.status.success(), "sha256sum failed");
    Ok(String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_string())
}

/// Browsers stamp downloads with the source URL as an extended attribute.
fn origin_url(path: &Path) -> Option<String> {
    let c_path = CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    let attr = CString::new("user.xdg.origin.url").ok()?;
    let mut buf = vec![0u8; 2048];
    // SAFETY: both strings are valid NUL-terminated C strings and the buffer
    // length passed matches its allocation.
    let n = unsafe {
        libc::getxattr(
            c_path.as_ptr(),
            attr.as_ptr(),
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len(),
        )
    };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    let url = String::from_utf8(buf).ok()?.trim().to_string();
    (!url.is_empty()).then_some(url)
}

#[cfg(test)]
mod tests {
    use super::guess_name;

    #[test]
    fn name_from_file_name() {
        assert_eq!(guess_name("Beeper-4.3.123-x86_64.AppImage"), "Beeper");
        assert_eq!(
            guess_name("Zen_Browser-linux-x86_64.AppImage"),
            "Zen Browser"
        );
        assert_eq!(guess_name("obsidian.AppImage"), "obsidian");
    }
}
