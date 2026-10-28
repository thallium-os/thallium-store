//! Store settings with typed defaults.
//!
//! `settings.get` always returns every key, so the settings hubs can show a
//! current value on each row before the user has touched anything. The file
//! holds whatever the user changed; everything else comes from `Default`.
//! Keys this build does not know (written by a newer or older build) are
//! carried through untouched.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::Path;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct Settings {
    pub first_run_done: bool,

    // Updates
    pub auto_check: bool,
    pub check_frequency: Frequency,
    pub notify_updates: bool,
    pub auto_install: AutoInstall,
    /// Follow the network's metered flag from system Settings; the store
    /// keeps no metered state of its own.
    pub respect_metered: bool,

    // Sources
    pub sources: SourceToggles,
    pub show_proprietary: bool,
    pub prerelease: Prerelease,

    // Interface
    pub default_tab: Tab,
    pub source_badges: bool,

    // Installs & storage
    pub flatpak_scope: Scope,
    pub menu_shortcuts: bool,
    pub remove_unused_deps: bool,
    pub remove_unused_runtimes: bool,
    pub keep_downloads: bool,
    /// 0 = no limit.
    pub cache_limit_mb: u64,

    // Downloads
    pub parallel_connections: bool,
    pub max_concurrent_downloads: u8,
    /// 0 = no limit.
    pub bandwidth_limit_kbps: u64,
    pub retries: u8,
    pub timeout_secs: u16,

    // Content & safety
    pub age_rating: AgeRating,
    pub warn_unverified: bool,
    pub review_permissions: bool,

    // Maintenance
    pub debug_logging: bool,

    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            first_run_done: false,
            auto_check: true,
            check_frequency: Frequency::Daily,
            notify_updates: true,
            auto_install: AutoInstall::default(),
            respect_metered: true,
            sources: SourceToggles::default(),
            show_proprietary: true,
            prerelease: Prerelease::default(),
            default_tab: Tab::Discover,
            source_badges: true,
            flatpak_scope: Scope::User,
            menu_shortcuts: true,
            remove_unused_deps: true,
            remove_unused_runtimes: true,
            keep_downloads: false,
            cache_limit_mb: 5000,
            parallel_connections: true,
            max_concurrent_downloads: 2,
            bandwidth_limit_kbps: 0,
            retries: 3,
            timeout_secs: 30,
            age_rating: AgeRating::Any,
            warn_unverified: true,
            review_permissions: true,
            debug_logging: false,
            extra: Map::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Frequency {
    Daily,
    Weekly,
}

/// What a source may install without asking. `Security` only means something
/// where the source marks security updates, which today is APT.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoMode {
    Off,
    Security,
    All,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct AutoInstall {
    pub apt: AutoMode,
    pub flathub: AutoMode,
    pub github: AutoMode,
    pub appimage: AutoMode,
}

impl Default for AutoInstall {
    fn default() -> Self {
        Self {
            apt: AutoMode::Security,
            flathub: AutoMode::Off,
            github: AutoMode::Off,
            appimage: AutoMode::Off,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct SourceToggles {
    pub apt: bool,
    pub flathub: bool,
    pub github: bool,
    pub appimage: bool,
    pub aur: bool,
}

impl Default for SourceToggles {
    fn default() -> Self {
        Self {
            apt: true,
            flathub: true,
            github: true,
            appimage: true,
            aur: false,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default)]
pub struct Prerelease {
    pub apt_backports: bool,
    pub flathub_beta: bool,
    pub github: bool,
    pub appimage: bool,
    pub aur_git: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tab {
    Discover,
    Apps,
    Updates,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    User,
    System,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgeRating {
    Any,
    Teen,
    Child,
}

/// The file layered over the defaults. A missing or unreadable file, or one
/// whose values no longer fit the types, falls back to the defaults rather
/// than failing every request; the file itself is left alone until the next
/// `settings.set`.
pub fn load(path: &Path) -> Settings {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Settings::default();
    };
    match serde_json::from_str(&text) {
        Ok(settings) => settings,
        Err(err) => {
            tracing::warn!("ignoring unreadable {}: {err}", path.display());
            Settings::default()
        }
    }
}

/// Merge `patch` into the current settings and save. Nested objects merge
/// key by key, so `{"auto_install": {"apt": "all"}}` leaves the other
/// sources as they were. A value of the wrong type is rejected and nothing
/// is written.
pub fn apply(path: &Path, patch: &Value) -> Result<Settings> {
    let mut merged = serde_json::to_value(load(path))?;
    merge(&mut merged, patch);
    let settings: Settings =
        serde_json::from_value(merged).context("setting has the wrong type")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&settings)?)?;
    Ok(settings)
}

fn merge(target: &mut Value, patch: &Value) {
    match (target, patch) {
        (Value::Object(target), Value::Object(patch)) => {
            for (key, value) in patch {
                match target.get_mut(key) {
                    Some(existing) if existing.is_object() && value.is_object() => {
                        merge(existing, value)
                    }
                    _ => {
                        target.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (target, patch) => *target = patch.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "thallium-store-settings-{name}-{}.json",
            std::process::id()
        ))
    }

    #[test]
    fn missing_file_gives_every_default() {
        let value = serde_json::to_value(load(Path::new("/nonexistent/settings.json"))).unwrap();
        assert_eq!(value["auto_check"], json!(true));
        assert_eq!(value["auto_install"]["apt"], json!("security"));
        assert_eq!(value["sources"]["aur"], json!(false));
        assert_eq!(value["max_concurrent_downloads"], json!(2));
    }

    #[test]
    fn nested_patch_keeps_siblings_and_unknown_keys() {
        let path = temp("nested");
        std::fs::write(&path, r#"{"first_run_done": true, "from_newer_build": 7}"#).unwrap();
        let settings = apply(&path, &json!({"auto_install": {"flathub": "all"}})).unwrap();
        assert_eq!(settings.auto_install.flathub, AutoMode::All);
        assert_eq!(settings.auto_install.apt, AutoMode::Security);
        assert!(settings.first_run_done);
        assert_eq!(settings.extra["from_newer_build"], json!(7));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn wrong_type_is_rejected_and_not_written() {
        let path = temp("wrong");
        std::fs::write(&path, r#"{"retries": 5}"#).unwrap();
        assert!(apply(&path, &json!({"retries": "lots"})).is_err());
        assert_eq!(load(&path).retries, 5);
        std::fs::remove_file(&path).ok();
    }
}
