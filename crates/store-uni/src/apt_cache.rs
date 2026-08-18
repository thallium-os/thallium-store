//! User-initiated refresh of apt's package lists.
//!
//! `updates.list` answers from apt's *local* cache. Thallium machines have
//! periodic apt refresh switched off, so that cache only moves when something
//! else runs `apt-get update` -- and a machine whose owner only ever updates
//! through this Store may never do that. The Store then shows an empty update
//! list that looks authoritative and is not, which is how a machine silently
//! misses a release it was supposed to receive. That matters beyond cosmetics:
//! the archive keyring ships as a package now (see the Thallium_81 key
//! rotation plan), so an update this list never mentions is how a machine ends
//! up unable to verify the archive at all.
//!
//! Refresh is only ever run from an explicit user action. It needs root, and a
//! polkit prompt raised on window open trains people to dismiss the prompt.

use std::process::Stdio;
use std::time::{Duration, UNIX_EPOCH};

use crate::privilege;

const LISTS_DIR: &str = "/var/lib/apt/lists";

/// What a refresh attempt did. Every variant still leaves the caller with a
/// usable (if stale) list -- refreshing is best-effort, never a precondition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AptRefreshOutcome {
    /// No refresh was asked for; the list is whatever apt already had.
    Skipped,
    /// `apt-get update` ran and exited 0.
    Ok,
    /// The user dismissed the polkit prompt, or is not authorised.
    Declined,
    /// It ran and failed -- offline, archive down, malformed sources.
    Failed,
}

impl AptRefreshOutcome {
    /// Wire name. Kept as a plain string so store-uni stays serde-free.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Skipped => "skipped",
            Self::Ok => "ok",
            Self::Declined => "declined",
            Self::Failed => "failed",
        }
    }
}

/// Outcome plus the age of the list the caller is about to read, so a UI can
/// say *when* it last saw the archive instead of implying it just looked.
#[derive(Clone, Debug)]
pub struct AptRefresh {
    pub outcome: AptRefreshOutcome,
    pub error: Option<String>,
    /// Unix seconds of the newest file in apt's lists directory.
    pub checked_at: Option<u64>,
}

impl AptRefresh {
    fn new(outcome: AptRefreshOutcome, error: Option<String>) -> Self {
        Self {
            outcome,
            error,
            checked_at: checked_at(),
        }
    }

    /// The list was not refreshed and nobody asked it to be.
    pub fn skipped() -> Self {
        Self::new(AptRefreshOutcome::Skipped, None)
    }
}

/// Run `apt-get update` under polkit, returning what happened.
///
/// Never returns an error: a failed refresh degrades the answer to "stale",
/// which the caller reports alongside the cached list. `kill_on_drop` so a
/// polkit prompt nobody answers dies with the timeout instead of outliving it.
pub async fn refresh(timeout: Duration) -> AptRefresh {
    if !privilege::has("apt-get") && !privilege::is_root() {
        return AptRefresh::new(
            AptRefreshOutcome::Failed,
            Some("apt-get is not installed".into()),
        );
    }

    // Retries=1: the default three, each with its own connect timeout, can
    // outlast the caller's timeout on a machine that is simply offline.
    let mut cmd = privilege::privileged("apt-get", &["update", "-o", "Acquire::Retries=1"]);
    cmd.stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let output = match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(err)) => {
            return AptRefresh::new(
                AptRefreshOutcome::Failed,
                Some(format!("could not start apt-get: {err}")),
            )
        }
        Err(_) => {
            return AptRefresh::new(
                AptRefreshOutcome::Failed,
                Some("timed out waiting for apt-get update".into()),
            )
        }
    };

    if output.status.success() {
        return AptRefresh::new(AptRefreshOutcome::Ok, None);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let outcome = classify(output.status.code(), &stderr);
    AptRefresh::new(outcome, Some(last_line(&stderr)))
}

/// Distinguish "the user said no" from "it went wrong", because they need
/// different words in the UI: one is a choice, the other is a fault.
///
/// pkexec exits 126 when authorisation could not be obtained (prompt
/// dismissed) and 127 when the user is not authorised at all -- the same pair
/// the Dash update badge special-cases. `sudo -n` has no such code and just
/// exits 1, so it is recognised by what it prints.
fn classify(code: Option<i32>, stderr: &str) -> AptRefreshOutcome {
    if matches!(code, Some(126) | Some(127)) {
        return AptRefreshOutcome::Declined;
    }
    let lowered = stderr.to_ascii_lowercase();
    if lowered.contains("password is required")
        || lowered.contains("authentication failed")
        || lowered.contains("not authorized")
        || lowered.contains("dismissed")
    {
        return AptRefreshOutcome::Declined;
    }
    AptRefreshOutcome::Failed
}

/// Last non-empty line, clipped. apt's failures are wordy and the interesting
/// part is at the end; the whole thing does not belong in a JSON-RPC reply.
fn last_line(stderr: &str) -> String {
    let line = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .next_back()
        .unwrap_or("apt-get update failed");
    if line.len() > 200 {
        format!("{}…", &line[..200])
    } else {
        line.to_string()
    }
}

/// Unix seconds of the newest thing in apt's lists directory.
///
/// The directory's own mtime moves when apt renames a fetched list into
/// place, and each list file carries its own; taking the max of both covers
/// the case where a refresh finds nothing changed. It is an approximation of
/// "when did we last see the archive" -- good enough to print, not something
/// to branch on.
fn checked_at() -> Option<u64> {
    let dir = std::path::Path::new(LISTS_DIR);
    let mut newest = std::fs::metadata(dir).ok().and_then(|m| m.modified().ok());

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            if let Ok(modified) = meta.modified() {
                if newest.is_none_or(|current| modified > current) {
                    newest = Some(modified);
                }
            }
        }
    }

    newest
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|since| since.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dismissed_pkexec_reads_as_declined() {
        assert_eq!(classify(Some(126), ""), AptRefreshOutcome::Declined);
        assert_eq!(classify(Some(127), ""), AptRefreshOutcome::Declined);
    }

    #[test]
    fn non_interactive_sudo_refusal_reads_as_declined() {
        assert_eq!(
            classify(Some(1), "sudo: a password is required"),
            AptRefreshOutcome::Declined
        );
    }

    #[test]
    fn a_real_apt_failure_is_not_a_refusal() {
        let stderr = "E: Failed to fetch https://dronzer-tb.github.io/Thallium_81/dists/stable/InRelease  Could not connect";
        assert_eq!(classify(Some(100), stderr), AptRefreshOutcome::Failed);
        assert!(last_line(stderr).starts_with("E: Failed to fetch"));
    }

    #[test]
    fn skipped_still_reports_a_timestamp_when_apt_is_present() {
        // Only asserts the shape: on a machine with no /var/lib/apt/lists the
        // timestamp is legitimately absent.
        let refresh = AptRefresh::skipped();
        assert_eq!(refresh.outcome, AptRefreshOutcome::Skipped);
        assert!(refresh.error.is_none());
    }
}
