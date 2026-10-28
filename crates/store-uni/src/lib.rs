pub mod apt_cache;
pub mod registry;
pub mod sources;

mod backends;
mod privilege;

pub use backends::appimage_local::{inspect as inspect_appimage, Inspection as AppImageInspection};
pub use backends::StagePermits;
pub use privilege::privileged;

use serde::{Deserialize, Serialize};
use store_core::{OperationAction, OperationState, SourceKind, Transfer, TransferPart};
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct UniAdapter {
    fake: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct UniProgress {
    pub state: OperationState,
    pub percent: u8,
    pub message: String,
    #[serde(default)]
    pub transfer: Option<Transfer>,
}

/// Native backend/privilege readiness reported by `system.health`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeReadiness {
    pub apt: bool,
    pub flatpak: bool,
    pub privilege: bool,
}

fn readiness_from(apt: bool, flatpak: bool, root: bool, escalator: bool) -> NativeReadiness {
    NativeReadiness {
        apt,
        flatpak,
        privilege: root || escalator,
    }
}

impl UniAdapter {
    pub fn new(fake: bool) -> Self {
        Self { fake }
    }

    /// Presence of native backend tooling and privilege escalation, scanned
    /// from PATH / /proc — no subprocess spawned, no bash `uni` involved.
    pub fn native_readiness(&self) -> NativeReadiness {
        readiness_from(
            privilege::has("apt-get"),
            privilege::has("flatpak"),
            privilege::is_root(),
            privilege::has("pkexec") || privilege::has("sudo"),
        )
    }

    pub fn command_preview(
        action: OperationAction,
        package_id: &str,
        source: SourceKind,
    ) -> Vec<String> {
        let source_arg = match source {
            SourceKind::System => "apt",
            SourceKind::Flathub => "flatpak",
            SourceKind::Github => "github",
            SourceKind::Appimage => "appimage",
        };
        let action_arg = match action {
            OperationAction::Install | OperationAction::Reinstall => "install",
            OperationAction::Update => "update",
            OperationAction::Remove => "remove",
        };

        match action {
            OperationAction::Update => vec![
                "uni".to_string(),
                action_arg.to_string(),
                package_id.to_string(),
                "--source".to_string(),
                source_arg.to_string(),
            ],
            _ => vec![
                "uni".to_string(),
                action_arg.to_string(),
                package_id.to_string(),
                "--source".to_string(),
                source_arg.to_string(),
            ],
        }
    }

    pub fn run(
        &self,
        action: OperationAction,
        app_name: String,
        package_id: String,
        source: SourceKind,
        token: CancellationToken,
        permits: StagePermits,
    ) -> mpsc::Receiver<Result<UniProgress, String>> {
        let (tx, rx) = mpsc::channel(16);
        let fake = self.fake;
        tokio::spawn(async move {
            if fake {
                run_fake(action, &app_name, tx, token).await;
            } else {
                backends::run(action, app_name, package_id, source, tx, token, permits).await;
            }
        });
        rx
    }
}

async fn run_fake(
    action: OperationAction,
    app_name: &str,
    tx: mpsc::Sender<Result<UniProgress, String>>,
    token: CancellationToken,
) {
    let verb = match action {
        OperationAction::Install => "Installing",
        OperationAction::Update => "Updating",
        OperationAction::Reinstall => "Reinstalling",
        OperationAction::Remove => "Removing",
    };
    let events = [
        (OperationState::Resolving, 5, "Resolving metadata"),
        (OperationState::Downloading, 35, "Downloading package data"),
        (OperationState::Installing, 70, "Applying package changes"),
        (OperationState::Finalizing, 90, "Refreshing installed state"),
        (OperationState::Succeeded, 100, "Completed"),
    ];

    for (state, percent, message) in events {
        tokio::select! {
            _ = sleep(Duration::from_millis(450)) => {}
            _ = token.cancelled() => {
                let _ = tx
                    .send(Ok(UniProgress {
                        state: OperationState::Cancelled,
                        percent,
                        message: format!("Cancelled {app_name}"),
                        transfer: None,
                    }))
                    .await;
                return;
            }
        }
        let _ = tx
            .send(Ok(UniProgress {
                state,
                percent,
                message: format!("{verb} {app_name}: {message}"),
                transfer: None,
            }))
            .await;
        if state == OperationState::Downloading
            && action != OperationAction::Remove
            && !fake_transfer(app_name, &tx, &token).await
        {
            return;
        }
    }
}

/// A Flatpak-shaped download (runtime, GL extension, app) at a steady
/// 40 MB/s, so fake mode exercises byte progress and the per-part ring.
/// Returns false when cancelled.
async fn fake_transfer(
    app_name: &str,
    tx: &mpsc::Sender<Result<UniProgress, String>>,
    token: &CancellationToken,
) -> bool {
    const SPEED: u64 = 40_000_000;
    const TICK_MS: u64 = 400;
    let sizes = [
        ("org.freedesktop.Platform", 210_000_000),
        ("org.freedesktop.Platform.GL.default", 140_000_000),
        (app_name, 137_000_000),
    ];
    let total: u64 = sizes.iter().map(|(_, size)| size).sum();
    let mut done = 0;
    while done < total {
        tokio::select! {
            _ = sleep(Duration::from_millis(TICK_MS)) => {}
            _ = token.cancelled() => {
                let _ = tx
                    .send(Ok(UniProgress {
                        state: OperationState::Cancelled,
                        percent: 0,
                        message: format!("Cancelled {app_name}"),
                        transfer: None,
                    }))
                    .await;
                return false;
            }
        }
        done = (done + SPEED * TICK_MS / 1000).min(total);
        let mut before = 0;
        let parts = sizes
            .iter()
            .map(|(name, size)| {
                let part_done = done.saturating_sub(before).min(*size);
                before += size;
                TransferPart {
                    name: name.to_string(),
                    bytes_done: part_done,
                    bytes_total: *size,
                }
            })
            .collect();
        let _ = tx
            .send(Ok(UniProgress {
                state: OperationState::Downloading,
                percent: (35 + done * 35 / total) as u8,
                message: format!("Downloading {app_name}"),
                transfer: Some(Transfer {
                    bytes_done: done,
                    bytes_total: total,
                    bytes_per_sec: SPEED,
                    parts,
                }),
            }))
            .await;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_from_requires_root_or_escalator_for_privilege() {
        assert_eq!(
            readiness_from(true, true, false, false),
            NativeReadiness {
                apt: true,
                flatpak: true,
                privilege: false
            }
        );
        assert!(readiness_from(false, false, true, false).privilege);
        assert!(readiness_from(false, false, false, true).privilege);
    }

    #[test]
    fn native_readiness_serializes_camel_case() {
        let readiness = NativeReadiness {
            apt: true,
            flatpak: false,
            privilege: true,
        };
        let value = serde_json::to_value(&readiness).unwrap();
        assert_eq!(value["apt"], serde_json::json!(true));
        assert_eq!(value["flatpak"], serde_json::json!(false));
        assert_eq!(value["privilege"], serde_json::json!(true));
    }
}
