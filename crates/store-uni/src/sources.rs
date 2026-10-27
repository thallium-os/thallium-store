//! Download sources and their mirrors.
//!
//! Each source the store installs from (Flathub, the Debian archive, GitHub
//! releases) has a default server plus mirrors the user adds. The live choice
//! for Flathub and Debian is read back from the tool that owns it (the user
//! flatpak remote URL, the apt source lines) so the UI never disagrees with
//! what will actually download; GitHub has no such tool, so its choice is the
//! URL prefix stored in `sources.json`. Mirrored GitHub assets are only used
//! when GitHub publishes a SHA-256 for them, and the download is checked
//! against it.

use crate::privilege;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::process::Command;

const PROBE_BYTES: u64 = 2 * 1024 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
/// A fixed public release asset (~10 MB) to time GitHub downloads with.
const GITHUB_PROBE_ASSET: &str =
    "https://github.com/neovim/neovim/releases/download/v0.10.0/nvim-linux64.tar.gz";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceId {
    Flathub,
    System,
    Github,
}

impl SourceId {
    pub const ALL: [SourceId; 3] = [SourceId::Flathub, SourceId::System, SourceId::Github];

    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "flathub" => Ok(Self::Flathub),
            "system" => Ok(Self::System),
            "github" => Ok(Self::Github),
            _ => bail!("unknown source: {s}"),
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Flathub => "flathub",
            Self::System => "system",
            Self::Github => "github",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Flathub => "Flathub",
            Self::System => "System packages (Debian)",
            Self::Github => "GitHub releases",
        }
    }

    /// (id, name, url). GitHub's url is a prefix put in front of the asset URL.
    fn builtin(self) -> &'static [(&'static str, &'static str, &'static str)] {
        match self {
            Self::Flathub => &[
                (
                    "default",
                    "Flathub (Fastly)",
                    "https://dl.flathub.org/repo/",
                ),
                (
                    "sjtug",
                    "SJTUG mirror (China)",
                    "https://mirror.sjtu.edu.cn/flathub/",
                ),
            ],
            Self::System => &[
                (
                    "default",
                    "Debian CDN (Fastly)",
                    "http://deb.debian.org/debian/",
                ),
                (
                    "cloudfront",
                    "Debian CDN (CloudFront)",
                    "http://cloudfront.debian.net/debian/",
                ),
            ],
            Self::Github => &[("default", "GitHub direct", "")],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Mirror {
    id: String,
    name: String,
    url: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct SourceState {
    #[serde(default)]
    active: Option<String>,
    #[serde(default)]
    mirrors: Vec<Mirror>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct Config {
    #[serde(default)]
    flathub: SourceState,
    #[serde(default)]
    system: SourceState,
    #[serde(default)]
    github: SourceState,
}

impl Config {
    fn state(&mut self, source: SourceId) -> &mut SourceState {
        match source {
            SourceId::Flathub => &mut self.flathub,
            SourceId::System => &mut self.system,
            SourceId::Github => &mut self.github,
        }
    }
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

fn config_path() -> PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".config"))
        .join("thallium-store/sources.json")
}

fn load() -> Config {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(config: &Config) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(config)?)?;
    Ok(())
}

fn norm(url: &str) -> &str {
    url.trim_end_matches('/')
}

/// Mirror URLs reach a root `sed` and flatpak config, so only plain
/// http(s) URLs made of unreserved characters are accepted.
fn validate_url(url: &str) -> Result<()> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .context("mirror URL must start with http:// or https://")?;
    if rest.is_empty()
        || !rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._~:/".contains(c))
    {
        bail!("mirror URL may only contain letters, digits and - . _ ~ : /");
    }
    Ok(())
}

/// Every server known for `source`: built-ins, then the user's mirrors.
fn entries(source: SourceId, config: &mut Config) -> Vec<(Mirror, bool)> {
    let mut out: Vec<(Mirror, bool)> = source
        .builtin()
        .iter()
        .map(|(id, name, url)| {
            (
                Mirror {
                    id: id.to_string(),
                    name: name.to_string(),
                    url: url.to_string(),
                },
                true,
            )
        })
        .collect();
    out.extend(
        config
            .state(source)
            .mirrors
            .iter()
            .cloned()
            .map(|m| (m, false)),
    );
    out
}

/// The server `source` downloads from right now.
async fn current_url(source: SourceId, config: &mut Config) -> Option<String> {
    match source {
        SourceId::Flathub => flathub_url().await,
        SourceId::System => debian_url(),
        SourceId::Github => {
            let active = config
                .state(source)
                .active
                .clone()
                .unwrap_or_else(|| "default".into());
            entries(source, config)
                .into_iter()
                .find(|(m, _)| m.id == active)
                .map(|(m, _)| m.url)
                .or(Some(String::new()))
        }
    }
}

pub async fn list() -> Result<Value> {
    let mut config = load();
    let mut sources = Vec::new();
    for source in SourceId::ALL {
        let current = current_url(source, &mut config).await;
        let mut mirrors: Vec<Value> = Vec::new();
        let mut active = None;
        for (m, builtin) in entries(source, &mut config) {
            if active.is_none() && current.as_deref().map(norm) == Some(norm(&m.url)) {
                active = Some(m.id.clone());
            }
            mirrors.push(json!({ "id": m.id, "name": m.name, "url": m.url, "builtin": builtin }));
        }
        // Set by hand outside the store: show it rather than pretend.
        if active.is_none() {
            if let Some(url) = current.filter(|u| !u.is_empty()) {
                mirrors.push(json!({ "id": "detected", "name": "Current setting", "url": url, "builtin": true }));
                active = Some("detected".into());
            }
        }
        let mut entry = json!({
            "id": source.key(),
            "name": source.name(),
            "active": active,
            "mirrors": mirrors,
            "needsAdmin": source == SourceId::System,
        });
        if source == SourceId::Flathub {
            entry["parallel"] = json!(flathub_parallel());
        }
        sources.push(entry);
    }
    Ok(json!({ "sources": sources }))
}

pub async fn add_mirror(source: SourceId, name: &str, url: &str) -> Result<Value> {
    let name = name.trim();
    let url = url.trim();
    if name.is_empty() {
        bail!("mirror needs a name");
    }
    validate_url(url)?;
    let mut config = load();
    if entries(source, &mut config)
        .iter()
        .any(|(m, _)| norm(&m.url) == norm(url))
    {
        bail!("that mirror is already listed");
    }
    let url = format!("{}/", norm(url));
    let state = config.state(source);
    let n = state.mirrors.len() + 1;
    let id = (n..)
        .map(|i| format!("custom-{i}"))
        .find(|id| !state.mirrors.iter().any(|m| &m.id == id))
        .unwrap();
    state.mirrors.push(Mirror {
        id,
        name: name.to_string(),
        url,
    });
    save(&config)?;
    list().await
}

pub async fn remove_mirror(source: SourceId, id: &str) -> Result<Value> {
    let mut config = load();
    let current = current_url(source, &mut config).await;
    let state = config.state(source);
    let Some(pos) = state.mirrors.iter().position(|m| m.id == id) else {
        bail!("only mirrors you added can be removed");
    };
    if current.as_deref().map(norm) == Some(norm(&state.mirrors[pos].url)) {
        bail!("switch to another server before removing this one");
    }
    state.mirrors.remove(pos);
    save(&config)?;
    list().await
}

pub async fn set_active(source: SourceId, id: &str) -> Result<Value> {
    let mut config = load();
    let mirror = entries(source, &mut config)
        .into_iter()
        .find(|(m, _)| m.id == id)
        .map(|(m, _)| m)
        .with_context(|| format!("no server {id} for {}", source.key()))?;
    match source {
        SourceId::Flathub => {
            run(Command::new("flatpak").args([
                "remote-modify",
                "--user",
                &format!("--url={}", mirror.url),
                "flathub",
            ]))
            .await?
        }
        SourceId::System => {
            let current = debian_url().context("no Debian archive line found in apt sources")?;
            if norm(&current) != norm(&mirror.url) {
                switch_debian(&current, &mirror.url).await?;
            }
        }
        SourceId::Github => {
            config.state(source).active = Some(mirror.id);
            save(&config)?;
        }
    }
    list().await
}

async fn run(cmd: &mut Command) -> Result<()> {
    let out = cmd.kill_on_drop(true).output().await?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// Download up to PROBE_BYTES of a known file from every server of `source`
/// at once and report the throughput each one managed before the timeout.
pub async fn probe(source: SourceId) -> Result<Value> {
    let mut config = load();
    let mut set = tokio::task::JoinSet::new();
    for (m, _) in entries(source, &mut config) {
        let url = probe_url(source, &m.url);
        set.spawn(async move { (m.id, time_download(&url).await) });
    }
    let mut results = Vec::new();
    while let Some(Ok((id, outcome))) = set.join_next().await {
        results.push(match outcome {
            Ok((bytes, secs)) => json!({
                "id": id, "ok": bytes > 0,
                "bytesPerSec": (bytes as f64 / secs.max(0.001)) as u64,
            }),
            Err(err) => json!({ "id": id, "ok": false, "error": err.to_string() }),
        });
    }
    Ok(json!({ "source": source.key(), "results": results }))
}

fn probe_url(source: SourceId, base: &str) -> String {
    let base = norm(base);
    match source {
        SourceId::Flathub => format!(
            "{base}/appstream/{}/appstream.xml.gz",
            std::env::consts::ARCH
        ),
        SourceId::System => format!(
            "{base}/dists/{}/main/Contents-{}.gz",
            codename(),
            deb_arch()
        ),
        SourceId::Github if base.is_empty() => GITHUB_PROBE_ASSET.to_string(),
        SourceId::Github => format!("{base}/{GITHUB_PROBE_ASSET}"),
    }
}

async fn time_download(url: &str) -> Result<(u64, f64)> {
    let client = reqwest::Client::builder()
        .user_agent("thallium-store")
        .build()?;
    let start = Instant::now();
    let deadline = start + PROBE_TIMEOUT;
    let mut resp = tokio::time::timeout(
        PROBE_TIMEOUT,
        client
            .get(url)
            .header(
                reqwest::header::RANGE,
                format!("bytes=0-{}", PROBE_BYTES - 1),
            )
            .send(),
    )
    .await
    .context("no response")??
    .error_for_status()?;
    let mut bytes = 0u64;
    while bytes < PROBE_BYTES {
        match tokio::time::timeout_at(deadline.into(), resp.chunk()).await {
            Ok(Ok(Some(chunk))) => bytes += chunk.len() as u64,
            Ok(Ok(None)) | Err(_) => break,
            Ok(Err(err)) => return Err(err.into()),
        }
    }
    Ok((bytes, start.elapsed().as_secs_f64()))
}

fn os_release(key: &str) -> Option<String> {
    std::fs::read_to_string("/etc/os-release")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix(key)?
                .strip_prefix('=')
                .map(|v| v.trim_matches('"').to_string())
        })
}

fn codename() -> String {
    os_release("VERSION_CODENAME")
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "stable".into())
}

fn deb_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "arm" => "armhf",
        _ => "amd64",
    }
}

// --- Flathub ---------------------------------------------------------------

async fn flathub_url() -> Option<String> {
    let out = Command::new("flatpak")
        .args(["remotes", "--user", "--columns=name,url"])
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|line| {
            let mut cols = line.split_whitespace();
            (cols.next()? == "flathub").then(|| cols.next().map(str::to_string))?
        })
}

fn flatpak_repo_config() -> PathBuf {
    std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".local/share"))
        .join("flatpak/repo/config")
}

/// libostree fetches up to 8 objects at once but multiplexes them over one
/// HTTP/2 connection; `http2=false` makes them 8 separate connections, which
/// is what helps on a congested route.
fn flathub_parallel() -> bool {
    std::fs::read_to_string(flatpak_repo_config())
        .map(|text| {
            remote_section(&text, "flathub")
                .iter()
                .any(|l| l.trim() == "http2=false")
        })
        .unwrap_or(false)
}

fn remote_section<'a>(text: &'a str, remote: &str) -> Vec<&'a str> {
    let header = format!("[remote \"{remote}\"]");
    text.lines()
        .skip_while(|l| l.trim() != header)
        .skip(1)
        .take_while(|l| !l.trim_start().starts_with('['))
        .collect()
}

pub async fn set_flathub_parallel(enabled: bool) -> Result<Value> {
    let path = flatpak_repo_config();
    let text = std::fs::read_to_string(&path).context("no per-user flatpak installation")?;
    std::fs::write(&path, with_http2(&text, "flathub", enabled)?)?;
    list().await
}

fn with_http2(text: &str, remote: &str, parallel: bool) -> Result<String> {
    let header = format!("[remote \"{remote}\"]");
    let mut out = String::new();
    let mut inside = false;
    let mut found = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            inside = trimmed == header;
            if inside {
                found = true;
                out.push_str(line);
                out.push('\n');
                if parallel {
                    out.push_str("http2=false\n");
                }
                continue;
            }
        }
        if inside && trimmed.starts_with("http2=") {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !found {
        bail!("flatpak remote {remote} is not configured for this user");
    }
    Ok(out)
}

// --- Debian archive ----------------------------------------------------------

fn apt_files() -> Vec<PathBuf> {
    let mut files = vec![PathBuf::from("/etc/apt/sources.list")];
    if let Ok(dir) = std::fs::read_dir("/etc/apt/sources.list.d") {
        let mut more: Vec<PathBuf> = dir
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                matches!(
                    p.extension().and_then(|e| e.to_str()),
                    Some("list" | "sources")
                )
            })
            .collect();
        more.sort();
        files.extend(more);
    }
    files
}

fn debian_url() -> Option<String> {
    apt_files().iter().find_map(|path| {
        let text = std::fs::read_to_string(path).ok()?;
        debian_url_in(
            &text,
            path.extension().and_then(|e| e.to_str()) == Some("sources"),
        )
    })
}

/// The main Debian archive is the `…/debian` URL serving a non-security suite.
fn is_debian_archive(url: &str, suite: &str) -> bool {
    norm(url).ends_with("/debian") && !suite.contains("security")
}

fn debian_url_in(text: &str, deb822: bool) -> Option<String> {
    if deb822 {
        return text.split("\n\n").find_map(|block| {
            let field = |name: &str| {
                block
                    .lines()
                    .find_map(|l| l.strip_prefix(name).map(str::trim))
            };
            let suite = field("Suites:")?;
            field("URIs:")?
                .split_whitespace()
                .find(|url| is_debian_archive(url, suite))
                .map(str::to_string)
        });
    }
    text.lines().find_map(|line| {
        let mut tokens = line.split_whitespace();
        if !matches!(tokens.next()?, "deb" | "deb-src") {
            return None;
        }
        let mut url = tokens.next()?;
        if url.starts_with('[') {
            while !url.ends_with(']') {
                url = tokens.next()?;
            }
            url = tokens.next()?;
        }
        is_debian_archive(url, tokens.next()?).then(|| url.to_string())
    })
}

/// Replace the Debian archive URL in every apt source file that uses it,
/// through one privileged `sed` (the polkit prompt appears once).
async fn switch_debian(old: &str, new: &str) -> Result<()> {
    validate_url(new)?;
    validate_url(old).context("current Debian URL has unexpected characters")?;
    let files: Vec<String> = apt_files()
        .iter()
        .filter(|p| {
            std::fs::read_to_string(p)
                .map(|t| t.contains(norm(old)))
                .unwrap_or(false)
        })
        .map(|p| p.display().to_string())
        .collect();
    if files.is_empty() {
        bail!("no apt source file mentions {old}");
    }
    let expr = format!(
        "s#([[:space:]]){}/?([[:space:]]|$)#\\1{}/\\2#g",
        norm(old).replace('.', "\\."),
        norm(new)
    );
    let mut args = vec!["-i", "-E", expr.as_str()];
    args.extend(files.iter().map(String::as_str));
    run(&mut privilege::privileged("sed", &args)).await
}

// --- GitHub ------------------------------------------------------------------

/// The URL prefix chosen for GitHub release assets, if not direct.
pub fn github_prefix() -> Option<String> {
    let mut config = load();
    let active = config.github.active.clone()?;
    entries(SourceId::Github, &mut config)
        .into_iter()
        .find(|(m, _)| m.id == active)
        .map(|(m, _)| m.url)
        .filter(|u| !u.is_empty())
}

/// Lower-case hex SHA-256 of `path`.
pub async fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect())
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_debian_archive_in_one_line_format() {
        let text = "deb http://security.debian.org/debian-security/ trixie-security main\n\
                    deb [signed-by=/k.gpg] http://cloudfront.debian.net/debian trixie main\n";
        assert_eq!(
            debian_url_in(text, false).as_deref(),
            Some("http://cloudfront.debian.net/debian")
        );
    }

    #[test]
    fn finds_debian_archive_in_deb822() {
        let text = "Types: deb\nURIs: http://deb.debian.org/debian-security\nSuites: trixie-security\n\n\
                    Types: deb\nURIs: http://deb.debian.org/debian\nSuites: trixie trixie-updates\n";
        assert_eq!(
            debian_url_in(text, true).as_deref(),
            Some("http://deb.debian.org/debian")
        );
    }

    #[test]
    fn ignores_third_party_repos() {
        assert_eq!(
            debian_url_in("deb https://apt.thallium81.dev/ stable main\n", false),
            None
        );
    }

    #[test]
    fn rejects_urls_that_could_escape_sed() {
        assert!(validate_url("https://mirror.example.org/debian/").is_ok());
        assert!(validate_url("https://x.org/a#b").is_err());
        assert!(validate_url("https://x.org/a&b").is_err());
        assert!(validate_url("ftp://x.org/").is_err());
    }

    #[test]
    fn toggles_http2_inside_the_flathub_section_only() {
        let text = "[core]\nrepo_version=1\n\n[remote \"flathub\"]\nurl=https://dl.flathub.org/repo/\n\n[remote \"other\"]\nhttp2=false\n";
        let on = with_http2(text, "flathub", true).unwrap();
        assert!(remote_section(&on, "flathub").contains(&"http2=false"));
        let off = with_http2(&on, "flathub", false).unwrap();
        assert!(!remote_section(&off, "flathub").contains(&"http2=false"));
        assert!(remote_section(&off, "other").contains(&"http2=false"));
    }
}
