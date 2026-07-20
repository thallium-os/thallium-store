use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::Stdio;
use store_core::{OperationAction, OperationState, SourceKind};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};

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
    ) -> mpsc::Receiver<Result<UniProgress, String>> {
        let (tx, rx) = mpsc::channel(16);
        let fake = self.fake;
        tokio::spawn(async move {
            if fake {
                run_fake(action, &app_name, tx).await;
            } else {
                run_real_json_events(action, &package_id, source, tx).await;
            }
        });
        rx
    }
}

async fn run_fake(
    action: OperationAction,
    app_name: &str,
    tx: mpsc::Sender<Result<UniProgress, String>>,
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
        sleep(Duration::from_millis(450)).await;
        let _ = tx
            .send(Ok(UniProgress {
                state,
                percent,
                message: format!("{verb} {app_name}: {message}"),
            }))
            .await;
    }
}

async fn run_real_json_events(
    action: OperationAction,
    package_id: &str,
    source: SourceKind,
    tx: mpsc::Sender<Result<UniProgress, String>>,
) {
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

    let mut child = match Command::new(uni_binary())
        .arg(action_arg)
        .arg(package_id)
        .arg("--source")
        .arg(source_arg)
        .arg("--json-events")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            let _ = tx.send(Err(format!("failed to start UNI: {err}"))).await;
            return;
        }
    };

    if let Some(stdout) = child.stdout.take() {
        let tx_stdout = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                match parse_uni_event(&line) {
                    Ok(progress) => {
                        let _ = tx_stdout.send(Ok(progress)).await;
                    }
                    Err(err) => {
                        let _ = tx_stdout
                            .send(Err(format!("malformed UNI JSON event: {err}: {line}")))
                            .await;
                    }
                }
            }
        });
    }

    if let Some(stderr) = child.stderr.take() {
        let tx_stderr = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    let _ = tx_stderr
                        .send(Ok(UniProgress {
                            state: OperationState::Installing,
                            percent: 50,
                            message: line,
                        }))
                        .await;
                }
            }
        });
    }

    match child.wait().await {
        Ok(status) if status.success() => {}
        Ok(status) => {
            let _ = tx
                .send(Err(format!("UNI exited with status {status}")))
                .await;
        }
        Err(err) => {
            let _ = tx.send(Err(format!("UNI wait failed: {err}"))).await;
        }
    }
}

fn uni_binary() -> String {
    std::env::var("THALLIUM_STORE_UNI").unwrap_or_else(|_| "uni".to_string())
}

fn parse_uni_event(line: &str) -> Result<UniProgress> {
    let value: Value = serde_json::from_str(line)?;
    let event = value.get("event").and_then(Value::as_str).unwrap_or("");
    let state_text = value.get("state").and_then(Value::as_str).unwrap_or(event);
    let percent = value
        .get("progress")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(100) as u8;
    let message = value
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or(event)
        .to_string();

    let state = match state_text {
        "queued" => OperationState::Pending,
        "succeeded" | "success" | "done" => OperationState::Succeeded,
        "failed" | "error" => OperationState::Failed,
        "downloading" | "download_progress" => OperationState::Downloading,
        "installing" => OperationState::Installing,
        "finalizing" => OperationState::Finalizing,
        "awaiting_authentication" => OperationState::AwaitingAuthentication,
        _ if event == "succeeded" => OperationState::Succeeded,
        _ if event == "failed" => OperationState::Failed,
        _ if percent < 20 => OperationState::Resolving,
        _ if percent < 60 => OperationState::Downloading,
        _ if percent < 90 => OperationState::Installing,
        _ => OperationState::Finalizing,
    };

    Ok(UniProgress {
        state,
        percent,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_uni_json_event() {
        let progress = parse_uni_event(
            r#"{"event":"succeeded","state":"succeeded","progress":100,"message":"done"}"#,
        )
        .unwrap();

        assert_eq!(progress.state, OperationState::Succeeded);
        assert_eq!(progress.percent, 100);
        assert_eq!(progress.message, "done");
    }
}
