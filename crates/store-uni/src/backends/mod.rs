//! Native install backends.
//!
//! Each backend spawns the underlying package tool directly (apt-get,
//! flatpak, ...) and streams real progress as [`UniProgress`] events, then
//! records the result in the UNI registry. This replaces shelling out to the
//! `uni` bash script (which buffered all output and emitted fake 50%/90%
//! milestones only after the child had already exited).

mod apt;
mod appimage;
mod download;
mod flatpak;
mod github;

use crate::UniProgress;
use store_core::{OperationAction, OperationState, SourceKind};
use tokio::sync::mpsc::Sender;

pub type ProgressSender = Sender<Result<UniProgress, String>>;

/// Dispatch an operation to the backend that owns `source`.
pub async fn run(
    action: OperationAction,
    app_name: String,
    package_id: String,
    source: SourceKind,
    tx: ProgressSender,
) {
    match source {
        SourceKind::System => apt::run(action, &app_name, &package_id, &tx).await,
        SourceKind::Flathub => flatpak::run(action, &app_name, &package_id, &tx).await,
        SourceKind::Github => github::run(action, &app_name, &package_id, &tx).await,
        SourceKind::Appimage => appimage::run(action, &app_name, &package_id, &tx).await,
    }
}

/// Emit a progress event, ignoring a closed receiver.
pub(crate) async fn emit(tx: &ProgressSender, state: OperationState, percent: u8, message: impl Into<String>) {
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
