//! AUR backend: builds and installs through paru or yay.
//!
//! The helper runs as the invoking user (makepkg refuses root) and is told to
//! escalate its own pacman step with pkexec, with every review/diff prompt
//! pre-answered so it never waits on stdin. Removal needs no helper: an AUR
//! package is an ordinary pacman package once installed.

use super::pacman::{check_package_id, pacman_args, record, run_streamed};
use super::{emit, fail, ProgressSender};
use crate::privilege::{is_root, privileged};
use std::sync::Arc;
use store_core::host::{aur_helper, AurHelper, NO_AUR_HELPER};
use store_core::{OperationAction, OperationState};
use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

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

    // The helper installs into the pacman db, so it shares the system slot.
    let _network_permit = network.clone().acquire_owned().await.ok();
    let _mutation_permit = mutation.clone().acquire_owned().await.ok();

    if action == OperationAction::Remove {
        let args = pacman_args(action, package_id);
        let label = format!("pacman {}", args.join(" "));
        emit(
            tx,
            OperationState::Resolving,
            2,
            format!("Removing AUR package {package_id} with pacman"),
        )
        .await;
        if run_streamed(privileged("pacman", &args), &label, app_name, tx, token).await {
            record(action, app_name, package_id, "aur").await;
            emit(
                tx,
                OperationState::Succeeded,
                100,
                format!("{app_name} removed"),
            )
            .await;
        }
        return;
    }

    let Some(helper) = aur_helper() else {
        fail(tx, NO_AUR_HELPER).await;
        return;
    };
    if is_root() {
        fail(
            tx,
            format!(
                "{} cannot build AUR packages as root; run Thallium Store as your user",
                helper.binary()
            ),
        )
        .await;
        return;
    }

    let args = helper_args(helper, action, package_id);
    let label = format!("{} {}", helper.binary(), args.join(" "));
    emit(
        tx,
        OperationState::Resolving,
        2,
        format!(
            "Building {package_id} from the AUR with {}: {label}",
            helper.binary()
        ),
    )
    .await;
    let mut cmd = Command::new(helper.binary());
    cmd.args(&args);
    if run_streamed(cmd, &label, app_name, tx, token).await {
        record(action, app_name, package_id, "aur").await;
        emit(
            tx,
            OperationState::Succeeded,
            100,
            format!("{app_name} ready (built with {})", helper.binary()),
        )
        .await;
    }
}

/// Non-interactive install/upgrade arguments for `helper`.
///
/// paru: `--skipreview` skips the PKGBUILD review pager. yay: `--answerdiff
/// None --answerclean None` pre-answers its diff and clean-build menus. Both
/// take `--sudo <cmd>` for the binary they escalate pacman with, and forward
/// `--noconfirm`/`--needed` to pacman.
pub(super) fn helper_args(
    helper: AurHelper,
    action: OperationAction,
    package_id: &str,
) -> Vec<&str> {
    let mut args = vec!["-S", "--noconfirm"];
    if action != OperationAction::Reinstall {
        args.push("--needed");
    }
    match helper {
        AurHelper::Paru => args.push("--skipreview"),
        AurHelper::Yay => args.extend(["--answerdiff", "None", "--answerclean", "None"]),
    }
    args.extend(["--sudo", "pkexec", package_id]);
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paru_install_is_non_interactive_and_escalates_with_pkexec() {
        assert_eq!(
            helper_args(AurHelper::Paru, OperationAction::Install, "paru-bin"),
            [
                "-S",
                "--noconfirm",
                "--needed",
                "--skipreview",
                "--sudo",
                "pkexec",
                "paru-bin"
            ]
        );
    }

    #[test]
    fn yay_install_answers_every_menu() {
        assert_eq!(
            helper_args(
                AurHelper::Yay,
                OperationAction::Install,
                "visual-studio-code-bin"
            ),
            [
                "-S",
                "--noconfirm",
                "--needed",
                "--answerdiff",
                "None",
                "--answerclean",
                "None",
                "--sudo",
                "pkexec",
                "visual-studio-code-bin"
            ]
        );
    }

    #[test]
    fn reinstall_drops_needed() {
        assert!(
            !helper_args(AurHelper::Paru, OperationAction::Reinstall, "x").contains(&"--needed")
        );
    }
}
