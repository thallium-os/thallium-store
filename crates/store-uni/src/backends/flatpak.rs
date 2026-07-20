//! Flatpak backend.
//!
//! flatpak installs into the per-user installation (no root needed) and
//! prints human progress lines that carry a trailing `NN%`. We parse that for
//! a real download/install percentage; unparseable lines become log events.

use super::{emit, fail, ProgressSender};
use crate::registry;
use std::process::Stdio;
use store_core::{OperationAction, OperationState};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

pub async fn run(action: OperationAction, app_name: &str, package_id: &str, tx: &ProgressSender) {
    emit(tx, OperationState::Resolving, 2, format!("Preparing flatpak for {app_name}")).await;

    let args: Vec<&str> = match action {
        OperationAction::Install => vec!["install", "-y", "--user", "flathub", package_id],
        OperationAction::Reinstall => {
            vec!["install", "-y", "--user", "--reinstall", "flathub", package_id]
        }
        OperationAction::Update => vec!["update", "-y", "--user", package_id],
        OperationAction::Remove => vec!["uninstall", "-y", "--user", package_id],
    };

    let mut cmd = Command::new("flatpak");
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            fail(tx, format!("failed to start flatpak: {err}")).await;
            return;
        }
    };

    if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                stream_line(&tx, &line).await;
            }
        });
    }

    if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                stream_line(&tx, &line).await;
            }
        });
    }

    match child.wait().await {
        Ok(status) if status.success() => {
            record(action, app_name, package_id).await;
            emit(tx, OperationState::Succeeded, 100, format!("{app_name} ready")).await;
        }
        Ok(status) => fail(tx, format!("flatpak exited with {status}")).await,
        Err(err) => fail(tx, format!("flatpak wait failed: {err}")).await,
    }
}

async fn stream_line(tx: &ProgressSender, line: &str) {
    if line.trim().is_empty() {
        return;
    }
    match parse_percent(line) {
        Some(percent) => {
            let state = if percent >= 95 {
                OperationState::Finalizing
            } else {
                OperationState::Downloading
            };
            emit(tx, state, percent, line.trim()).await;
        }
        None => emit(tx, OperationState::Installing, 50, line.trim()).await,
    }
}

async fn record(action: OperationAction, app_name: &str, package_id: &str) {
    let result = match action {
        OperationAction::Remove => registry::remove(app_name).await,
        _ => registry::add(app_name, "flatpak", package_id).await,
    };
    if let Err(err) = result {
        tracing::warn!("updating UNI registry after flatpak op failed: {err:#}");
    }
}

/// Extract a trailing/inline `NN%` token from a flatpak progress line.
fn parse_percent(line: &str) -> Option<u8> {
    let idx = line.find('%')?;
    let digits: String = line[..idx]
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    digits.parse::<u8>().ok().map(|p| p.min(100))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_percent() {
        assert_eq!(parse_percent("Installing 1/1... 73%"), Some(73));
        assert_eq!(parse_percent("[####] 100% done"), Some(100));
        assert_eq!(parse_percent("Resolving dependencies"), None);
    }
}
