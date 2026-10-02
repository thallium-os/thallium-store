//! Minimal privilege escalation for package operations.
//!
//! apt/dpkg mutations need root; flatpak `--user` and the github/appimage
//! backends do not. When already root we exec directly, otherwise prefer
//! polkit's `pkexec` (GUI auth prompt), falling back to non-interactive
//! `sudo -n` so a daemon never blocks on a TTY password prompt.

use tokio::process::Command;

/// Build a command that runs `program` with root privilege when required.
pub fn privileged(program: &str, args: &[&str]) -> Command {
    if is_root() {
        let mut cmd = Command::new(program);
        cmd.args(args);
        cmd
    } else if has("pkexec") {
        let mut cmd = Command::new("pkexec");
        cmd.arg(program).args(args);
        cmd
    } else {
        let mut cmd = Command::new("sudo");
        cmd.arg("-n").arg(program).args(args);
        cmd
    }
}

/// Effective uid == 0, read from /proc so we avoid a libc dependency.
pub(crate) fn is_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with("Uid:"))
                .and_then(|line| line.split_whitespace().nth(2).map(str::to_string))
        })
        .map(|effective| effective == "0")
        .unwrap_or(false)
}

/// Whether `bin` resolves on PATH.
pub(crate) fn has(bin: &str) -> bool {
    store_core::host::on_path(bin)
}
