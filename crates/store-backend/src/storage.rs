//! Download cache accounting and pruning.
//!
//! The cache is `$XDG_CACHE_HOME/uni` (the native downloader and the AppImage
//! backend both write there). Automatic pruning leaves recently modified
//! files alone: a transfer in progress touches its file continuously, so age
//! is the only signal that a file is not being written.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Automatic pruning never touches files younger than this.
pub const AUTO_PRUNE_MIN_AGE: Duration = Duration::from_secs(600);

pub fn download_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into())).join(".cache")
        })
        .join("uni")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheFile {
    pub path: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
}

pub fn scan(dir: &Path) -> Vec<CacheFile> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push(entry.path());
            } else if kind.is_file() {
                files.push(CacheFile {
                    path: entry.path(),
                    size: meta.len(),
                    modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                });
            }
        }
    }
    files
}

/// Files to delete so the total drops to `limit_bytes` or less: oldest first,
/// skipping anything modified within `min_age` of `now`.
pub fn plan_prune(
    files: &[CacheFile],
    limit_bytes: u64,
    min_age: Duration,
    now: SystemTime,
) -> Vec<CacheFile> {
    let mut total: u64 = files.iter().map(|f| f.size).sum();
    let mut candidates: Vec<&CacheFile> = files
        .iter()
        .filter(|f| now.duration_since(f.modified).unwrap_or_default() >= min_age)
        .collect();
    candidates.sort_by_key(|f| f.modified);
    let mut doomed = Vec::new();
    for file in candidates {
        if total <= limit_bytes {
            break;
        }
        total -= file.size;
        doomed.push(file.clone());
    }
    doomed
}

fn delete(files: &[CacheFile]) -> u64 {
    let mut freed = 0;
    for file in files {
        if std::fs::remove_file(&file.path).is_ok() {
            freed += file.size;
        }
    }
    freed
}

/// Remove every file not modified within `min_age`; returns bytes freed.
pub fn clear(dir: &Path, min_age: Duration) -> u64 {
    let files = scan(dir);
    let freed = delete(&plan_prune(&files, 0, min_age, SystemTime::now()));
    remove_empty_dirs(dir);
    freed
}

/// Apply the `keep_downloads` / `cache_limit_mb` settings; returns bytes freed.
pub fn apply_policy(dir: &Path, keep_downloads: bool, cache_limit_mb: u64) -> u64 {
    let limit = if !keep_downloads {
        0
    } else if cache_limit_mb == 0 {
        return 0;
    } else {
        cache_limit_mb.saturating_mul(1024 * 1024)
    };
    let files = scan(dir);
    let freed = delete(&plan_prune(
        &files,
        limit,
        AUTO_PRUNE_MIN_AGE,
        SystemTime::now(),
    ));
    if freed > 0 {
        remove_empty_dirs(dir);
    }
    freed
}

fn remove_empty_dirs(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            remove_empty_dirs(&entry.path());
            let _ = std::fs::remove_dir(entry.path());
        }
    }
}

pub fn info() -> Value {
    let dir = download_dir();
    let files = scan(&dir);
    let mut value = crate::icon_cache_info();
    value["downloadBytes"] = json!(files.iter().map(|f| f.size).sum::<u64>());
    value["downloadCount"] = json!(files.len());
    value["downloadDir"] = json!(dir.to_string_lossy());
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: u64, age_secs: u64, now: SystemTime) -> CacheFile {
        CacheFile {
            path: name.into(),
            size,
            modified: now - Duration::from_secs(age_secs),
        }
    }

    #[test]
    fn prunes_oldest_first_until_under_limit() {
        let now = SystemTime::now();
        let files = vec![
            file("new", 100, 1000, now),
            file("old", 100, 5000, now),
            file("mid", 100, 3000, now),
        ];
        let doomed = plan_prune(&files, 150, Duration::from_secs(600), now);
        let names: Vec<_> = doomed.iter().map(|f| f.path.to_str().unwrap()).collect();
        assert_eq!(names, ["old", "mid"]);
    }

    #[test]
    fn recent_files_are_never_pruned() {
        let now = SystemTime::now();
        let files = vec![file("active", 500, 5, now), file("old", 10, 9000, now)];
        let doomed = plan_prune(&files, 0, Duration::from_secs(600), now);
        assert_eq!(doomed.len(), 1);
        assert_eq!(doomed[0].path.to_str(), Some("old"));
    }

    #[test]
    fn under_limit_prunes_nothing() {
        let now = SystemTime::now();
        let files = vec![file("a", 10, 9000, now)];
        assert!(plan_prune(&files, 10, Duration::ZERO, now).is_empty());
    }

    #[test]
    fn clear_removes_files_and_reports_bytes() {
        let dir = std::env::temp_dir().join(format!("ts-cache-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.bin"), [0u8; 64]).unwrap();
        std::fs::write(dir.join("sub/b.bin"), [0u8; 36]).unwrap();
        assert_eq!(clear(&dir, Duration::ZERO), 100);
        assert!(scan(&dir).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unlimited_cache_with_keep_is_untouched() {
        assert_eq!(apply_policy(Path::new("/nonexistent"), true, 0), 0);
    }
}
