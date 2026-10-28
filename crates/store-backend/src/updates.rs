//! Background update checker and the `updates.status` / `updates.checkNow`
//! RPCs.
//!
//! The checker only reads: it never refreshes apt's lists (that needs a
//! polkit prompt, which must stay a user action). It records when it last
//! looked next to the database, tells the user once per check if anything is
//! waiting, and queues updates only for sources set to install automatically.

use crate::settings::{self, AutoInstall, AutoMode, Frequency};
use crate::{AppState, UpdateItem};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

/// How often the loop wakes to see whether a check is due.
const TICK: Duration = Duration::from_secs(600);
/// Settle time after launch before the first check.
const STARTUP_DELAY: Duration = Duration::from_secs(60);

#[derive(Debug, Default, Deserialize, Serialize)]
struct CheckState {
    last_checked: Option<DateTime<Utc>>,
    count: usize,
}

fn state_path() -> PathBuf {
    crate::data_home().join("thallium-store/updates-state.json")
}

fn load_state() -> CheckState {
    std::fs::read_to_string(state_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_state(state: &CheckState) {
    let path = state_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(state) {
        if let Err(err) = std::fs::write(&path, text) {
            tracing::warn!("could not save update state: {err}");
        }
    }
}

pub fn interval(frequency: Frequency) -> ChronoDuration {
    match frequency {
        Frequency::Daily => ChronoDuration::days(1),
        Frequency::Weekly => ChronoDuration::weeks(1),
    }
}

pub fn is_due(last: Option<DateTime<Utc>>, now: DateTime<Utc>, frequency: Frequency) -> bool {
    last.is_none_or(|last| now - last >= interval(frequency))
}

/// Whether any connected device reports a metered connection. Input is
/// `nmcli -t -f GENERAL.METERED,GENERAL.STATE device show`: blocks of
/// `KEY:value` lines separated by blank lines. `yes` and `yes (guessed)`
/// both count; `unknown` does not, since guessing would block updates on
/// every machine NetworkManager has no opinion about.
pub fn parse_metered(out: &str) -> bool {
    out.split("\n\n").any(|block| {
        let value = |key: &str| {
            block
                .lines()
                .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix(':')))
                .map(str::trim)
        };
        value("GENERAL.STATE").is_some_and(|s| s.starts_with("100"))
            && value("GENERAL.METERED").is_some_and(|m| m.starts_with("yes"))
    })
}

pub async fn is_metered() -> bool {
    let mut cmd = Command::new("nmcli");
    cmd.args([
        "-t",
        "-f",
        "GENERAL.METERED,GENERAL.STATE",
        "device",
        "show",
    ])
    .kill_on_drop(true);
    match tokio::time::timeout(Duration::from_secs(5), cmd.output()).await {
        Ok(Ok(output)) if output.status.success() => {
            parse_metered(&String::from_utf8_lossy(&output.stdout))
        }
        _ => false,
    }
}

/// Updates the settings allow to install without asking.
pub fn select_auto_installs<'a>(
    items: &'a [UpdateItem],
    auto: &AutoInstall,
) -> Vec<&'a UpdateItem> {
    items
        .iter()
        .filter(|item| {
            let mode = match item.source.as_str() {
                "apt" => auto.apt,
                "flatpak" => auto.flathub,
                "github" => auto.github,
                _ => AutoMode::Off,
            };
            match mode {
                AutoMode::Off => false,
                AutoMode::All => true,
                AutoMode::Security => item.security,
            }
        })
        .collect()
}

/// Enqueue request for one update, shaped like a direct `operations.enqueue`.
fn enqueue_request(item: &UpdateItem) -> Option<Value> {
    let (source, package_id) = match item.source.as_str() {
        "apt" => ("system", item.name.clone()),
        "flatpak" => ("flathub", item.id.strip_prefix("flatpak:")?.to_string()),
        "github" => ("github", item.id.strip_prefix("uni:github:")?.to_string()),
        _ => return None,
    };
    Some(json!({
        "app_id": item.id, "variant_id": "auto", "action": "update",
        "app_name": item.name, "package_id": package_id, "source": source,
    }))
}

fn summary(count: usize) -> String {
    match count {
        1 => "1 update is available".to_string(),
        n => format!("{n} updates are available"),
    }
}

async fn notify(count: usize) {
    let mut cmd = Command::new("notify-send");
    cmd.args([
        "-a",
        "Thallium Store",
        "-i",
        "software-update-available",
        "Updates available",
    ])
    .arg(summary(count))
    .kill_on_drop(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), cmd.status()).await;
}

async fn busy_apps(state: &AppState) -> Vec<String> {
    use store_core::OperationState::*;
    state
        .db
        .lock()
        .await
        .list_operations()
        .unwrap_or_default()
        .into_iter()
        .filter(|op| !matches!(op.state, Succeeded | Failed | Cancelled))
        .map(|op| op.app_id)
        .collect()
}

/// One look at what is upgradable. `background` additionally notifies and
/// installs per the settings; a manual check only records and reports.
async fn run_check(state: &AppState, background: bool) -> CheckState {
    let report = crate::updates_list(false).await;
    let result = CheckState {
        last_checked: Some(Utc::now()),
        count: report.items.len(),
    };
    save_state(&result);

    if background && result.count > 0 {
        let settings = settings::load(&crate::settings_path());
        if settings.notify_updates {
            notify(result.count).await;
        }
        let metered = settings.respect_metered && is_metered().await;
        if metered {
            tracing::info!("metered connection: not installing updates automatically");
        } else {
            let busy = busy_apps(state).await;
            for item in select_auto_installs(&report.items, &settings.auto_install) {
                if busy.contains(&item.id) {
                    continue;
                }
                if let Some(request) = enqueue_request(item) {
                    if let Err(err) = crate::enqueue_operation(request, state.clone()).await {
                        tracing::warn!("could not queue update for {}: {err:#}", item.name);
                    }
                }
            }
        }
    }
    result
}

async fn status_value(state: CheckState) -> Value {
    json!({
        "lastChecked": state.last_checked.map(|t| t.to_rfc3339()),
        "count": state.count,
        "metered": is_metered().await,
    })
}

pub async fn status() -> Value {
    status_value(load_state()).await
}

pub async fn check_now(state: &AppState) -> Value {
    status_value(run_check(state, false).await).await
}

/// Long-lived task: check on the configured cadence and keep the download
/// cache inside its limits. Does nothing in fake mode, which must not touch
/// the network or the system.
pub fn spawn(state: AppState) {
    if state.fake_uni {
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(STARTUP_DELAY).await;
        loop {
            let settings = settings::load(&crate::settings_path());
            crate::storage::apply_policy(
                &crate::storage::download_dir(),
                settings.keep_downloads,
                settings.cache_limit_mb,
            );
            if settings.auto_check
                && is_due(
                    load_state().last_checked,
                    Utc::now(),
                    settings.check_frequency,
                )
            {
                run_check(&state, true).await;
            }
            tokio::time::sleep(TICK).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(source: &str, id: &str, security: bool) -> UpdateItem {
        UpdateItem {
            id: id.into(),
            name: id.rsplit(':').next().unwrap().into(),
            source: source.into(),
            current_version: None,
            available_version: None,
            managed_by_uni: false,
            security,
        }
    }

    #[test]
    fn metered_parsing() {
        let wifi_metered = "GENERAL.METERED:yes (guessed)\nGENERAL.STATE:100 (connected)\n\nGENERAL.METERED:no\nGENERAL.STATE:100 (connected)\n";
        assert!(parse_metered(wifi_metered));
        let unmetered = "GENERAL.METERED:no (guessed)\nGENERAL.STATE:100 (connected)\n\nGENERAL.METERED:unknown\nGENERAL.STATE:30 (disconnected)\n";
        assert!(!parse_metered(unmetered));
        // A metered device that is not connected does not count.
        assert!(!parse_metered(
            "GENERAL.METERED:yes\nGENERAL.STATE:30 (disconnected)\n"
        ));
        assert!(!parse_metered(""));
    }

    #[test]
    fn due_follows_frequency() {
        let now = Utc::now();
        assert!(is_due(None, now, Frequency::Daily));
        assert!(!is_due(
            Some(now - ChronoDuration::hours(23)),
            now,
            Frequency::Daily
        ));
        assert!(is_due(
            Some(now - ChronoDuration::hours(25)),
            now,
            Frequency::Daily
        ));
        assert!(!is_due(
            Some(now - ChronoDuration::days(3)),
            now,
            Frequency::Weekly
        ));
        assert!(is_due(
            Some(now - ChronoDuration::days(8)),
            now,
            Frequency::Weekly
        ));
    }

    #[test]
    fn auto_install_selection_honors_modes() {
        let items = vec![
            item("apt", "apt:openssl", true),
            item("apt", "apt:vim", false),
            item("flatpak", "flatpak:org.x.App", false),
            item("github", "uni:github:o/r", false),
        ];
        let mut auto = AutoInstall::default();
        let picked: Vec<_> = select_auto_installs(&items, &auto)
            .iter()
            .map(|i| i.id.clone())
            .collect();
        assert_eq!(picked, ["apt:openssl"]);
        auto.apt = AutoMode::All;
        auto.flathub = AutoMode::All;
        assert_eq!(select_auto_installs(&items, &auto).len(), 3);
        auto.apt = AutoMode::Off;
        auto.flathub = AutoMode::Off;
        assert!(select_auto_installs(&items, &auto).is_empty());
    }

    #[test]
    fn enqueue_requests_name_source_and_package() {
        let req = enqueue_request(&item("flatpak", "flatpak:org.x.App", false)).unwrap();
        assert_eq!(req["source"], "flathub");
        assert_eq!(req["package_id"], "org.x.App");
        assert_eq!(req["action"], "update");
        let req = enqueue_request(&item("github", "uni:github:o/r", false)).unwrap();
        assert_eq!(req["package_id"], "o/r");
        assert_eq!(
            enqueue_request(&item("apt", "apt:vim", false)).unwrap()["source"],
            "system"
        );
    }

    #[test]
    fn notification_text() {
        assert_eq!(summary(1), "1 update is available");
        assert_eq!(summary(4), "4 updates are available");
    }
}
