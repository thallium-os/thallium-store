//! Native reader/writer for UNI's install registry.
//!
//! UNI records everything it installs in `~/.local/share/uni/installed.json`
//! as `{ "<name>": { "backend": "<source>", "id": "<package id>" } }`. The
//! bash tool mutates this file through python heredocs under `flock`; this
//! module replaces that path with plain Rust so the backend no longer forks
//! python just to learn what is installed. Writes are atomic (temp file +
//! rename), which is corruption-safe for the single-writer-per-user case.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RegistryEntry {
    pub backend: String,
    pub id: String,
}

/// Name -> entry, matching UNI's on-disk shape.
pub type Registry = BTreeMap<String, RegistryEntry>;

/// Location of `installed.json`, overridable with `UNI_REGISTRY` for tests.
pub fn registry_path() -> PathBuf {
    if let Some(explicit) = std::env::var_os("UNI_REGISTRY") {
        return PathBuf::from(explicit);
    }
    data_home().join("uni/installed.json")
}

/// Load the registry, tolerating a missing or empty file as an empty map.
pub async fn load() -> Result<Registry> {
    let path = registry_path();
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Registry::new()),
        Err(err) => return Err(err).context("reading UNI registry"),
    };
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(Registry::new());
    }
    serde_json::from_slice(&bytes).context("parsing UNI registry")
}

/// Record an install (insert or overwrite by name).
pub async fn add(name: &str, backend: &str, id: &str) -> Result<()> {
    let mut registry = load().await?;
    registry.insert(
        name.to_string(),
        RegistryEntry {
            backend: backend.to_string(),
            id: id.to_string(),
        },
    );
    write(&registry).await
}

/// Drop an entry by name. Missing entries are a no-op.
pub async fn remove(name: &str) -> Result<()> {
    let mut registry = load().await?;
    if registry.remove(name).is_some() {
        write(&registry).await?;
    }
    Ok(())
}

async fn write(registry: &Registry) -> Result<()> {
    let path = registry_path();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let body = serde_json::to_vec_pretty(registry)?;
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, &body).await?;
    tokio::fs::rename(&tmp, &path)
        .await
        .context("committing UNI registry")
}

fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").unwrap_or_else(|| ".".into());
            PathBuf::from(home).join(".local/share")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn add_remove_roundtrip() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("uni-reg-{nonce}"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("installed.json");
        std::env::set_var("UNI_REGISTRY", &path);

        assert!(load().await.unwrap().is_empty());
        add("Zed", "dpkg", "zed").await.unwrap();
        let reg = load().await.unwrap();
        assert_eq!(reg["Zed"].backend, "dpkg");
        assert_eq!(reg["Zed"].id, "zed");

        remove("Zed").await.unwrap();
        assert!(load().await.unwrap().is_empty());

        std::env::remove_var("UNI_REGISTRY");
        std::fs::remove_dir_all(&dir).ok();
    }
}
