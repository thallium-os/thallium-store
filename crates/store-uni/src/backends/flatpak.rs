//! Flatpak backend.
//!
//! flatpak installs into the per-user installation (no root needed) and
//! prints a table of the refs it will fetch, then one progress line per ref
//! (`Installing 2/4… ███ 63%  6.6 MB/s  00:12`). We parse both into byte
//! counts per ref; unparseable lines become log events.

use super::{emit, emit_transfer, fail, ProgressSender};
use crate::registry;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use store_core::{OperationAction, OperationState, Transfer, TransferPart};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
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
    emit(
        tx,
        OperationState::Resolving,
        2,
        format!("Preparing flatpak for {app_name}"),
    )
    .await;

    // flatpak fetches and deploys in one invocation too (and serializes itself
    // internally via its own repo lock); hold both permits for the whole call
    // to keep the boundary consistent with apt while still letting other
    // sources overlap.
    let _network_permit = network.clone().acquire_owned().await.ok();
    let _mutation_permit = mutation.clone().acquire_owned().await.ok();

    // flatpak_search reports the merged (system+user) remote list, but we
    // install into the per-user installation. Ensure the flathub remote exists
    // there so installs never fail with "No remote refs found for 'flathub'".
    if !matches!(action, OperationAction::Remove) {
        ensure_flathub_remote().await;
    }

    // New installs go to the per-user installation, but removes/updates must
    // target wherever the ref actually lives — apps installed system-wide
    // (by the distro or a manual `flatpak install --system`) are invisible
    // to `--user` and used to fail with "No installed refs found".
    let scope = match action {
        OperationAction::Remove | OperationAction::Update => installed_scope(package_id).await,
        _ => "--user",
    };
    let args: Vec<&str> = match action {
        OperationAction::Install => vec!["install", "-y", "--user", "flathub", package_id],
        OperationAction::Reinstall => {
            vec![
                "install",
                "-y",
                "--user",
                "--reinstall",
                "flathub",
                package_id,
            ]
        }
        OperationAction::Update => vec!["update", "-y", scope, package_id],
        OperationAction::Remove => vec!["uninstall", "-y", scope, package_id],
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

    // The ref table and the progress lines can arrive on either stream.
    let transfer = Arc::new(Mutex::new(Vec::<TransferPart>::new()));
    if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        let transfer = Arc::clone(&transfer);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                stream_line(&tx, &transfer, &line).await;
            }
        });
    }

    if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        let transfer = Arc::clone(&transfer);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                stream_line(&tx, &transfer, &line).await;
            }
        });
    }

    tokio::select! {
        status = child.wait() => match status {
            Ok(status) if status.success() => {
                record(action, app_name, package_id).await;
                emit(tx, OperationState::Succeeded, 100, format!("{app_name} ready")).await;
            }
            Ok(status) => fail(tx, format!("flatpak exited with {status}")).await,
            Err(err) => fail(tx, format!("flatpak wait failed: {err}")).await,
        },
        _ = token.cancelled() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            emit(tx, OperationState::Cancelled, 0, format!("Cancelled flatpak for {app_name}")).await;
        }
    }
}

async fn stream_line(tx: &ProgressSender, parts: &Mutex<Vec<TransferPart>>, line: &str) {
    if line.trim().is_empty() {
        return;
    }
    if let Some(part) = parse_ref_row(line) {
        parts.lock().unwrap_or_else(|e| e.into_inner()).push(part);
        emit(tx, OperationState::Resolving, 2, line.trim()).await;
        return;
    }
    let Some(percent) = parse_percent(line) else {
        emit(tx, OperationState::Installing, 50, line.trim()).await;
        return;
    };
    let transfer = parse_position(line).and_then(|(index, bytes_per_sec)| {
        let mut parts = parts.lock().unwrap_or_else(|e| e.into_inner());
        apply_progress(&mut parts, index, percent, bytes_per_sec)
    });
    // Weighted by bytes across every ref when the table gave sizes; a bare
    // per-ref percentage otherwise.
    let overall = match &transfer {
        Some(t) => (t.bytes_done * 100 / t.bytes_total) as u8,
        None => percent,
    };
    let state = if overall >= 95 {
        OperationState::Finalizing
    } else {
        OperationState::Downloading
    };
    match transfer {
        Some(t) => emit_transfer(tx, state, overall, line.trim(), t).await,
        None => emit(tx, state, overall, line.trim()).await,
    }
}

/// A row of the table flatpak prints before fetching:
/// `1.\t   \tmd.obsidian.Obsidian\tstable\ti\tflathub\t< 236.8 MB`.
/// Uninstall rows carry no size column and come back with `bytes_total` 0.
fn parse_ref_row(line: &str) -> Option<TransferPart> {
    let fields: Vec<&str> = line
        .split('\t')
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .collect();
    let number = fields.first()?.strip_suffix('.')?;
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) || fields.len() < 4 {
        return None;
    }
    let size = fields
        .last()
        .filter(|f| f.starts_with('<'))
        .and_then(|f| parse_size(f.trim_start_matches('<').trim_end_matches("(partial)")))
        .unwrap_or(0);
    Some(TransferPart {
        name: fields[1].to_string(),
        bytes_done: 0,
        bytes_total: size,
    })
}

/// The `N/M` position (as a 0-based index) and the speed from a progress line.
fn parse_position(line: &str) -> Option<(usize, Option<u64>)> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let index = words.iter().find_map(|w| {
        let w = w.trim_end_matches('…').trim_end_matches("...");
        let (n, m) = w.split_once('/')?;
        let n: usize = n.parse().ok()?;
        m.parse::<usize>().ok()?;
        n.checked_sub(1)
    })?;
    let speed = words.windows(2).find_map(|pair| {
        let unit = pair[1].strip_suffix("/s")?;
        parse_size(&format!("{} {unit}", pair[0]))
    });
    Some((index, speed))
}

/// Mark refs before `index` complete and `index` at `percent`; None when the
/// table gave no sizes, so there is nothing to weigh bytes against.
fn apply_progress(
    parts: &mut [TransferPart],
    index: usize,
    percent: u8,
    bytes_per_sec: Option<u64>,
) -> Option<Transfer> {
    let bytes_total: u64 = parts.iter().map(|p| p.bytes_total).sum();
    if bytes_total == 0 || index >= parts.len() {
        return None;
    }
    for (i, part) in parts.iter_mut().enumerate() {
        if i < index {
            part.bytes_done = part.bytes_total;
        } else if i == index {
            part.bytes_done = part.bytes_total * u64::from(percent.min(100)) / 100;
        }
    }
    Some(Transfer {
        bytes_done: parts.iter().map(|p| p.bytes_done).sum(),
        bytes_total,
        bytes_per_sec: bytes_per_sec.unwrap_or(0),
        parts: parts.to_vec(),
    })
}

/// `236.8 MB`, `4.8 kB`, `512 bytes`: flatpak prints GLib's decimal sizes.
fn parse_size(text: &str) -> Option<u64> {
    let mut words = text.split_whitespace();
    let number: f64 = words.next()?.parse().ok()?;
    let scale = match words.next().unwrap_or("B") {
        "B" | "bytes" | "byte" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((number * scale).round() as u64)
}

/// Which installation holds this ref: `--user` when present there (or on any
/// probe failure, matching the old behaviour), `--system` otherwise.
async fn installed_scope(package_id: &str) -> &'static str {
    let user = Command::new("flatpak")
        .args(["info", "--user", package_id])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(true);
    if user {
        "--user"
    } else {
        "--system"
    }
}

async fn ensure_flathub_remote() {
    let _ = Command::new("flatpak")
        .args([
            "remote-add",
            "--user",
            "--if-not-exists",
            "flathub",
            "https://dl.flathub.org/repo/flathub.flatpakrepo",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
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

    // Lines below are verbatim from a real Obsidian install, NBSPs included.
    const ROW_LOCALE: &str =
        "1.\t   \tmd.obsidian.Obsidian.Locale\tstable\ti\tflathub\t< 4.8\u{a0}MB (partial)";
    const ROW_APP: &str = "2.\t   \tmd.obsidian.Obsidian\tstable\ti\tflathub\t< 236.8\u{a0}MB";
    const PROGRESS: &str = "Installing 2/2… █████████████▌        68%  6.6\u{a0}MB/s  00:10";

    #[test]
    fn parses_ref_table_rows() {
        let part = parse_ref_row(ROW_LOCALE).unwrap();
        assert_eq!(part.name, "md.obsidian.Obsidian.Locale");
        assert_eq!(part.bytes_total, 4_800_000);
        assert_eq!(parse_ref_row(ROW_APP).unwrap().bytes_total, 236_800_000);
        let uninstall = parse_ref_row("1.\t   \tmd.obsidian.Obsidian\tstable\tr").unwrap();
        assert_eq!(uninstall.bytes_total, 0);
        assert!(parse_ref_row("[1] /media, /mnt, /run/media, home,").is_none());
        assert!(parse_ref_row("Installing 1/2…").is_none());
    }

    #[test]
    fn parses_position_and_speed() {
        assert_eq!(parse_position(PROGRESS), Some((1, Some(6_600_000))));
        assert_eq!(parse_position("Installing 1/2…"), Some((0, None)));
        assert_eq!(parse_position("Looking for matches…"), None);
    }

    #[test]
    fn weighs_progress_by_bytes_across_refs() {
        let mut parts = vec![
            parse_ref_row(ROW_LOCALE).unwrap(),
            parse_ref_row(ROW_APP).unwrap(),
        ];
        let t = apply_progress(&mut parts, 1, 68, Some(6_600_000)).unwrap();
        assert_eq!(t.parts[0].bytes_done, 4_800_000);
        assert_eq!(t.parts[1].bytes_done, 161_024_000);
        assert_eq!(t.bytes_done, 165_824_000);
        assert_eq!(t.bytes_total, 241_600_000);
        assert_eq!(t.bytes_per_sec, 6_600_000);
    }

    #[test]
    fn no_transfer_without_sizes() {
        let mut parts = vec![parse_ref_row("1.\t   \tmd.obsidian.Obsidian\tstable\tr").unwrap()];
        assert!(apply_progress(&mut parts, 0, 50, None).is_none());
        assert!(apply_progress(&mut [], 0, 50, None).is_none());
    }
}
