//! Native install backends.
//!
//! Each backend spawns the underlying package tool directly (apt-get,
//! flatpak, ...) and streams real progress as [`UniProgress`] events, then
//! records the result in the UNI registry. This replaces shelling out to the
//! `uni` bash script (which buffered all output and emitted fake 50%/90%
//! milestones only after the child had already exited).

mod appimage;
pub mod appimage_local;
mod apt;
mod download;
mod flatpak;
mod github;

use crate::UniProgress;
use std::sync::Arc;
use store_core::{OperationAction, OperationState, SourceKind};
use tokio::sync::mpsc::Sender;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

pub type ProgressSender = Sender<Result<UniProgress, String>>;

/// Scheduler-owned concurrency permits threaded down into backends so each one
/// can bound its own download stage (shared `network` slot, parallel across all
/// sources) separately from its install stage (per-source `*_mutation` slot,
/// serial for system/flatpak). See individual backends for where each stage
/// boundary actually falls.
#[derive(Clone)]
pub struct StagePermits {
    pub network: Arc<Semaphore>,
    pub system_mutation: Arc<Semaphore>,
    pub flatpak_mutation: Arc<Semaphore>,
    pub github_mutation: Arc<Semaphore>,
}

/// Dispatch an operation to the backend that owns `source`.
pub async fn run(
    action: OperationAction,
    app_name: String,
    package_id: String,
    source: SourceKind,
    tx: ProgressSender,
    token: CancellationToken,
    permits: StagePermits,
) {
    match source {
        SourceKind::System => {
            apt::run(
                action,
                &app_name,
                &package_id,
                &tx,
                &token,
                &permits.network,
                &permits.system_mutation,
            )
            .await
        }
        SourceKind::Flathub => {
            flatpak::run(
                action,
                &app_name,
                &package_id,
                &tx,
                &token,
                &permits.network,
                &permits.flatpak_mutation,
            )
            .await
        }
        SourceKind::Github => {
            github::run(action, &app_name, &package_id, &tx, &token, &permits).await
        }
        SourceKind::Appimage => {
            appimage::run(
                action,
                &app_name,
                &package_id,
                &tx,
                &token,
                &permits.network,
                &permits.github_mutation,
            )
            .await
        }
    }
}

/// Emit a progress event, ignoring a closed receiver.
pub(crate) async fn emit(
    tx: &ProgressSender,
    state: OperationState,
    percent: u8,
    message: impl Into<String>,
) {
    let _ = tx
        .send(Ok(UniProgress {
            state,
            percent,
            message: message.into(),
        }))
        .await;
}

/// Emit a terminal failure event.
pub(crate) async fn fail(tx: &ProgressSender, message: impl Into<String>) {
    let _ = tx.send(Err(message.into())).await;
}
