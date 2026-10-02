//! pacman backend: the `system` source on Arch hosts.
//!
//! pacman has no machine-readable progress channel, so its output is relayed
//! line by line as log events. Mutations run under pkexec (or sudo -n) via
//! [`privileged`], strictly serially behind the caller's system mutation
//! permit, and a failure reports the exit code plus pacman's last stderr
//! lines so the user sees why, not just that, it failed.

use super::{emit, fail, ProgressSender};
use crate::{privilege::privileged, registry};
use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use store_core::{OperationAction, OperationState};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

/// How many trailing stderr lines a failure message carries.
const STDERR_TAIL: usize = 4;

pub async fn run(
    action: OperationAction,
    app_name: &str,
    package_id: &str,
    tx: &ProgressSender,
    token: &CancellationToken,
    network: &Arc<Semaphore>,
    mutation: &Arc<Semaphore>,
) {
    if let Err(message) = check_package_id(package_id) {
        fail(tx, message).await;
        return;
    }
    emit(
        tx,
        OperationState::Resolving,
        2,
        format!("Preparing pacman for {app_name}"),
    )
    .await;

    // Like apt, pacman downloads and installs in one call under one db lock.
    let _network_permit = network.clone().acquire_owned().await.ok();
    let _mutation_permit = mutation.clone().acquire_owned().await.ok();

    let args = pacman_args(action, package_id);
    let label = format!("pacman {}", args.join(" "));
    let cmd = privileged("pacman", &args);
    if run_streamed(cmd, &label, app_name, tx, token).await {
        record(action, app_name, package_id, "system").await;
        emit(
            tx,
            OperationState::Succeeded,
            100,
            format!("{app_name} ready"),
        )
        .await;
    }
}

/// pacman arguments for `action`. Never `-y`: refreshing the sync db and
/// then installing one package is a partial upgrade.
pub(super) fn pacman_args(action: OperationAction, package_id: &str) -> Vec<&str> {
    match action {
        OperationAction::Install | OperationAction::Update => {
            vec!["-S", "--noconfirm", "--needed", package_id]
        }
        OperationAction::Reinstall => vec!["-S", "--noconfirm", package_id],
        OperationAction::Remove => vec!["-Rns", "--noconfirm", package_id],
    }
}

/// A package id that starts with `-` would be read as an option.
pub(super) fn check_package_id(package_id: &str) -> Result<(), String> {
    if package_id.is_empty() || package_id.starts_with('-') {
        Err(format!("refusing invalid package name {package_id:?}"))
    } else {
        Ok(())
    }
}

/// Spawn `cmd`, relay its output as progress, and return whether it exited 0.
/// Every failure (spawn, wait, non-zero exit) is reported through `tx` with
/// the command, exit status and last stderr lines; cancellation kills the
/// child and reports Cancelled.
pub(super) async fn run_streamed(
    mut cmd: Command,
    label: &str,
    app_name: &str,
    tx: &ProgressSender,
    token: &CancellationToken,
) -> bool {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            fail(tx, format!("failed to start `{label}`: {err}")).await;
            return false;
        }
    };

    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        readers.push(tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    emit(&tx, OperationState::Installing, 50, line).await;
                }
            }
        }));
    }
    let tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL)));
    if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        let tail = tail.clone();
        readers.push(tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                {
                    let mut tail = tail.lock().unwrap();
                    if tail.len() == STDERR_TAIL {
                        tail.pop_front();
                    }
                    tail.push_back(line.trim().to_string());
                }
                emit(&tx, OperationState::Installing, 50, line).await;
            }
        }));
    }

    tokio::select! {
        status = child.wait() => {
            // Drain the pipes so the stderr tail is complete before reporting.
            for reader in readers {
                let _ = reader.await;
            }
            match status {
                Ok(status) if status.success() => true,
                Ok(status) => {
                    let tail: Vec<String> = tail.lock().unwrap().iter().cloned().collect();
                    fail(tx, failure_message(label, status.code(), &tail)).await;
                    false
                }
                Err(err) => {
                    fail(tx, format!("waiting for `{label}` failed: {err}")).await;
                    false
                }
            }
        }
        _ = token.cancelled() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            emit(tx, OperationState::Cancelled, 0, format!("Cancelled {label} for {app_name}")).await;
            false
        }
    }
}

/// `` `pacman -S ...` exited with code 1: error: target not found: foo ``.
/// pkexec's own exit codes (126 dismissed, 127 not authorized) are named,
/// since its stderr is often empty.
fn failure_message(label: &str, code: Option<i32>, stderr_tail: &[String]) -> String {
    let status = match code {
        Some(code) => format!("exited with code {code}"),
        None => "was killed by a signal".to_string(),
    };
    let mut message = format!("`{label}` {status}");
    match code {
        Some(126) if stderr_tail.is_empty() => {
            message.push_str(": authentication was dismissed");
        }
        Some(127) if stderr_tail.is_empty() => {
            message.push_str(": not authorized (polkit) or command not found");
        }
        _ => {}
    }
    if !stderr_tail.is_empty() {
        message.push_str(": ");
        message.push_str(&stderr_tail.join(" | "));
    }
    message
}

pub(super) async fn record(
    action: OperationAction,
    app_name: &str,
    package_id: &str,
    backend: &str,
) {
    let result = match action {
        OperationAction::Remove => registry::remove(app_name).await,
        _ => registry::add(app_name, backend, package_id).await,
    };
    if let Err(err) = result {
        tracing::warn!("updating UNI registry after {backend} op failed: {err:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pacman_args_never_refresh_the_sync_db() {
        assert_eq!(
            pacman_args(OperationAction::Install, "firefox"),
            ["-S", "--noconfirm", "--needed", "firefox"]
        );
        assert_eq!(
            pacman_args(OperationAction::Remove, "firefox"),
            ["-Rns", "--noconfirm", "firefox"]
        );
        for action in [
            OperationAction::Install,
            OperationAction::Update,
            OperationAction::Reinstall,
        ] {
            assert!(!pacman_args(action, "x").iter().any(|a| a.contains('y')));
        }
    }

    #[test]
    fn option_like_package_ids_are_refused() {
        assert!(check_package_id("--overwrite=*").is_err());
        assert!(check_package_id("").is_err());
        assert!(check_package_id("python-requests").is_ok());
    }

    #[test]
    fn failure_message_names_command_code_and_stderr() {
        let tail = vec!["error: target not found: nope".to_string()];
        assert_eq!(
            failure_message("pacman -S --noconfirm nope", Some(1), &tail),
            "`pacman -S --noconfirm nope` exited with code 1: error: target not found: nope"
        );
        assert_eq!(
            failure_message("pacman -Rns x", Some(126), &[]),
            "`pacman -Rns x` exited with code 126: authentication was dismissed"
        );
        assert_eq!(
            failure_message("yay -S x", None, &[]),
            "`yay -S x` was killed by a signal"
        );
    }
}
