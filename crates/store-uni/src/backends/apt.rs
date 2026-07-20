//! APT backend: real progress via `APT::Status-Fd`.
//!
//! apt-get emits machine-readable `dlstatus:`/`pmstatus:` lines when told to
//! write status to a file descriptor. We route that to stderr and parse it
//! for true download/install percentages, while stdout is captured as log
//! lines. apt mutates system state under the dpkg lock, so the backend runs
//! it strictly serially (enforced by the caller's per-source semaphore) and
//! with root privilege.

use super::{emit, fail, ProgressSender};
use crate::{privilege::privileged, registry};
use std::process::Stdio;
use std::sync::Arc;
use store_core::{OperationAction, OperationState};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    action: OperationAction,
    app_name: &str,
    package_id: &str,
    tx: &ProgressSender,
    token: &CancellationToken,
    network: &Arc<Semaphore>,
    mutation: &Arc<Semaphore>,
) {
    emit(tx, OperationState::Resolving, 2, format!("Preparing apt for {app_name}")).await;

    // apt-get streams both dlstatus (download) and pmstatus (install) from a
    // single invocation under one dpkg lock, so download/install can't be
    // split into separate child processes without a --download-only pre-fetch.
    // Hold both the network slot and the (always-1) system mutation permit for
    // the whole call: apt stays strictly serial while other sources still
    // download/install in parallel via their own permits.
    let _network_permit = network.clone().acquire_owned().await.ok();
    let _mutation_permit = mutation.clone().acquire_owned().await.ok();

    let action_args: Vec<&str> = match action {
        OperationAction::Install => vec!["install", package_id],
        OperationAction::Reinstall => vec!["install", "--reinstall", package_id],
        OperationAction::Update => vec!["install", "--only-upgrade", package_id],
        OperationAction::Remove => vec!["remove", package_id],
    };

    let mut args: Vec<&str> = vec![
        "-y",
        "-o",
        "APT::Status-Fd=2",
        "-o",
        "Dpkg::Progress-Fancy=0",
    ];
    args.extend(action_args);

    let mut cmd = privileged("apt-get", &args);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            fail(tx, format!("failed to start apt-get: {err}")).await;
            return;
        }
    };

    if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if !line.trim().is_empty() {
                    emit(&tx, OperationState::Installing, 50, line).await;
                }
            }
        });
    }

    if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(progress) = parse_status(&line) {
                    emit(&tx, progress.0, progress.1, progress.2).await;
                } else if !line.trim().is_empty() {
                    emit(&tx, OperationState::Installing, 50, line).await;
                }
            }
        });
    }

    tokio::select! {
        status = child.wait() => match status {
            Ok(status) if status.success() => {
                record(action, app_name, package_id).await;
                emit(tx, OperationState::Succeeded, 100, format!("{app_name} ready")).await;
            }
            Ok(status) => fail(tx, format!("apt-get exited with {status}")).await,
            Err(err) => fail(tx, format!("apt-get wait failed: {err}")).await,
        },
        _ = token.cancelled() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            emit(tx, OperationState::Cancelled, 0, format!("Cancelled apt-get for {app_name}")).await;
        }
    }
}

async fn record(action: OperationAction, app_name: &str, package_id: &str) {
    let result = match action {
        OperationAction::Remove => registry::remove(app_name).await,
        _ => registry::add(app_name, "apt", package_id).await,
    };
    if let Err(err) = result {
        tracing::warn!("updating UNI registry after apt op failed: {err:#}");
    }
}

/// Parse an `APT::Status-Fd` line into (state, percent, message).
///
/// Format: `dlstatus:<id>:<percent>:<desc>` or `pmstatus:<pkg>:<percent>:<desc>`
/// where percent is a float 0..=100.
fn parse_status(line: &str) -> Option<(OperationState, u8, String)> {
    let mut parts = line.splitn(4, ':');
    let kind = parts.next()?;
    let _id = parts.next()?;
    let percent: f32 = parts.next()?.trim().parse().ok()?;
    let message = parts.next().unwrap_or("").trim().to_string();

    let state = match kind {
        "dlstatus" => OperationState::Downloading,
        "pmstatus" => OperationState::Installing,
        _ => return None,
    };

    Some((state, percent.round().clamp(0.0, 100.0) as u8, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_download_and_install_status() {
        let (state, pct, msg) = parse_status("dlstatus:1:42.5:Retrieving zed").unwrap();
        assert_eq!(state, OperationState::Downloading);
        assert_eq!(pct, 43);
        assert_eq!(msg, "Retrieving zed");

        let (state, pct, _) = parse_status("pmstatus:zed:100:Unpacking").unwrap();
        assert_eq!(state, OperationState::Installing);
        assert_eq!(pct, 100);
    }

    #[test]
    fn ignores_non_status_lines() {
        assert!(parse_status("Reading package lists...").is_none());
        assert!(parse_status("dlstatus:1:notanumber:oops").is_none());
    }
}
