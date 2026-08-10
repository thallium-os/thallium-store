//! AppImage backend.
//!
//! Installs a `.AppImage` from a direct URL (the catalog's appimage source):
//! download, mark executable, drop a desktop entry, and record it in the
//! registry. Removal deletes the file, desktop entry and registry row.

use super::download::download;
use super::{emit, fail, ProgressSender};
use crate::registry;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use store_core::{OperationAction, OperationState};
use tokio::sync::Semaphore;
use tokio::time::{sleep, Duration};
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
    match action {
        OperationAction::Remove => {
            let _mutation_permit = mutation.clone().acquire_owned().await.ok();
            match remove(app_name).await {
                Ok(()) => {
                    emit(
                        tx,
                        OperationState::Succeeded,
                        100,
                        format!("Removed {app_name}"),
                    )
                    .await
                }
                Err(err) => fail(tx, format!("removing {app_name} failed: {err}")).await,
            }
        }
        _ => {
            let dest = appimage_dir().join(format!("{}.AppImage", sanitize(app_name)));
            if let Err(err) =
                install(app_name, package_id, &dest, tx, token, network, mutation).await
            {
                if token.is_cancelled() {
                    emit(
                        tx,
                        OperationState::Cancelled,
                        0,
                        format!("Cancelled installing {app_name}"),
                    )
                    .await;
                } else {
                    fail(tx, format!("installing {app_name} failed: {err}")).await;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn install(
    app_name: &str,
    url: &str,
    dest: &Path,
    tx: &ProgressSender,
    token: &CancellationToken,
    network: &Arc<Semaphore>,
    mutation: &Arc<Semaphore>,
) -> anyhow::Result<()> {
    emit(
        tx,
        OperationState::Resolving,
        2,
        format!("Fetching {app_name}"),
    )
    .await;
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let done = Arc::new(AtomicU64::new(0));
    let total = Arc::new(AtomicU64::new(0));
    let ticker = spawn_progress_ticker(tx.clone(), Arc::clone(&done), Arc::clone(&total), 0, 90);

    // Download stage: bounded by the shared network permit only, so several
    // appimage/github downloads can run concurrently.
    let network_permit = network.clone().acquire_owned().await.ok();
    let result = download(url, dest, &done, &total, token).await;
    ticker.abort();
    drop(network_permit);
    result?;

    // Install stage: fs + registry write, serialized per source.
    let _mutation_permit = mutation.clone().acquire_owned().await.ok();
    finalize(app_name, dest, tx).await
}

/// Mark executable, write a desktop entry and register. Shared with the github
/// backend when it downloads an `.AppImage` asset.
pub(super) async fn finalize(
    app_name: &str,
    dest: &Path,
    tx: &ProgressSender,
) -> anyhow::Result<()> {
    emit(
        tx,
        OperationState::Finalizing,
        92,
        "Registering application",
    )
    .await;

    let mut perms = tokio::fs::metadata(dest).await?.permissions();
    perms.set_mode(0o755);
    tokio::fs::set_permissions(dest, perms).await?;

    write_desktop_entry(app_name, dest).await?;
    registry::add(app_name, "appimage", &dest.to_string_lossy()).await?;

    emit(
        tx,
        OperationState::Succeeded,
        100,
        format!("{app_name} ready"),
    )
    .await;
    Ok(())
}

async fn remove(app_name: &str) -> anyhow::Result<()> {
    let registry = registry::load().await?;
    if let Some(entry) = registry.get(app_name) {
        let _ = tokio::fs::remove_file(&entry.id).await;
    }
    let desktop = desktop_dir().join(format!("{}.desktop", sanitize(app_name)));
    let _ = tokio::fs::remove_file(&desktop).await;
    registry::remove(app_name).await
}

async fn write_desktop_entry(app_name: &str, exec: &Path) -> anyhow::Result<()> {
    let dir = desktop_dir();
    tokio::fs::create_dir_all(&dir).await?;
    let body = format!(
        "[Desktop Entry]\nName={app_name}\nExec={}\nIcon=application-x-executable\nType=Application\nCategories=Utility;\nComment=Installed via Thallium Store\n",
        exec.display()
    );
    tokio::fs::write(dir.join(format!("{}.desktop", sanitize(app_name))), body).await?;
    Ok(())
}

/// Emit Downloading events every 200ms from the shared byte counters, mapping
/// bytes into the [base, base+span] percentage band.
fn spawn_progress_ticker(
    tx: ProgressSender,
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    base: u8,
    span: u8,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut last_done = u64::MAX;
        let mut last_percent = u8::MAX;
        loop {
            sleep(Duration::from_millis(200)).await;
            let total = total.load(Ordering::Relaxed);
            let done = done.load(Ordering::Relaxed);
            let percent = match (done.min(total) * span as u64).checked_div(total) {
                Some(scaled) => base + scaled as u8,
                None => base,
            };
            let mib = done as f64 / (1024.0 * 1024.0);
            if done == last_done && percent == last_percent {
                continue;
            }
            last_done = done;
            last_percent = percent;
            emit(
                &tx,
                OperationState::Downloading,
                percent,
                format!("Downloaded {mib:.1} MiB"),
            )
            .await;
        }
    })
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").unwrap_or_else(|| ".".into());
            PathBuf::from(home).join(".local/share")
        })
}

fn appimage_dir() -> PathBuf {
    data_home().join("uni/appimages")
}

fn desktop_dir() -> PathBuf {
    data_home().join("applications")
}

pub(super) fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").unwrap_or_else(|| ".".into());
            PathBuf::from(home).join(".cache")
        })
        .join("uni")
}

pub(super) fn spawn_ticker(
    tx: ProgressSender,
    done: Arc<AtomicU64>,
    total: Arc<AtomicU64>,
    base: u8,
    span: u8,
) -> tokio::task::JoinHandle<()> {
    spawn_progress_ticker(tx, done, total, base, span)
}
