pub mod registry;

mod backends;
mod privilege;

pub use backends::StagePermits;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use store_core::{OperationAction, OperationState, SourceKind};
use tokio::process::Command;
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
}

impl UniAdapter {
    pub fn new(fake: bool) -> Self {
        Self { fake }
    }

    pub async fn health(&self) -> Result<String> {
        if self.fake {
            return Ok("fake-uni enabled".to_string());
        }
        let output = Command::new(uni_binary()).arg("version").output().await?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !stdout.is_empty() {
            return Ok(stdout);
        }
        if output.status.success() {
            Ok("uni available".to_string())
        } else {
            Ok(format!("uni version failed: {}", output.status))
        }
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
            }))
            .await;
    }
}

fn uni_binary() -> String {
    std::env::var("THALLIUM_STORE_UNI").unwrap_or_else(|_| "uni".to_string())
}
