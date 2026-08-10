//! GitHub Releases backend.
//!
//! Resolves the latest release, scores its assets to pick the best Linux
//! artifact (port of UNI's score_asset), downloads it with the native
//! segmented downloader, then installs by type: `.deb` via apt-get (root),
//! `.AppImage` via the appimage finalizer. Other asset types are rejected.

use super::appimage;
use super::download::download;
use super::{emit, fail, ProgressSender, StagePermits};
use crate::{privilege::privileged, registry};
use serde_json::Value;
use std::process::Stdio;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use store_core::{OperationAction, OperationState};
use tokio_util::sync::CancellationToken;

pub async fn run(
    action: OperationAction,
    app_name: &str,
    package_id: &str,
    tx: &ProgressSender,
    token: &CancellationToken,
    permits: &StagePermits,
) {
    if matches!(action, OperationAction::Remove) {
        let _mutation_permit = permits.github_mutation.clone().acquire_owned().await.ok();
        match registry::remove(app_name).await {
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
        return;
    }

    if let Err(err) = install(action, app_name, package_id, tx, token, permits).await {
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

async fn install(
    _action: OperationAction,
    app_name: &str,
    package_id: &str,
    tx: &ProgressSender,
    token: &CancellationToken,
    permits: &StagePermits,
) -> anyhow::Result<()> {
    let (owner, repo) = package_id
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("github package id must be owner/repo, got {package_id}"))?;

    emit(tx, OperationState::Resolving, 3, "Querying GitHub release").await;
    let client = reqwest::Client::builder()
        .user_agent("thallium-store")
        .build()?;
    let mut request = client.get(format!(
        "https://api.github.com/repos/{owner}/{repo}/releases/latest"
    ));
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        request = request.bearer_auth(token);
    }
    let release: Value = request.send().await?.error_for_status()?.json().await?;

    let (url, name) = pick_best_asset(&release).ok_or_else(|| {
        anyhow::anyhow!("no suitable Linux asset in latest {owner}/{repo} release")
    })?;

    let cache = appimage::cache_dir();
    tokio::fs::create_dir_all(&cache).await?;
    let dest = cache.join(&name);

    let done = Arc::new(AtomicU64::new(0));
    let total = Arc::new(AtomicU64::new(0));
    let ticker = appimage::spawn_ticker(tx.clone(), Arc::clone(&done), Arc::clone(&total), 5, 80);
    // Download stage: shared network permit only, so this can run alongside
    // other sources' downloads (and other github releases up to the limit).
    let network_permit = permits.network.clone().acquire_owned().await.ok();
    let result = download(&url, &dest, &done, &total, token).await;
    ticker.abort();
    drop(network_permit);
    result?;

    // Install stage: the resulting asset type decides which mutation permit
    // applies. A `.deb` shells out to apt-get, which touches the dpkg lock
    // just like the System backend, so it must serialize on `system_mutation`
    // (never `github_mutation`) to stay strictly serial system-wide.
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".appimage") {
        let _mutation_permit = permits.github_mutation.clone().acquire_owned().await.ok();
        appimage::finalize(app_name, &dest, tx).await
    } else if lower.ends_with(".deb") {
        let _mutation_permit = permits.system_mutation.clone().acquire_owned().await.ok();
        install_deb(app_name, repo, &dest, tx, token).await
    } else {
        anyhow::bail!("unsupported asset type: {name}")
    }
}

async fn install_deb(
    app_name: &str,
    repo: &str,
    dest: &std::path::Path,
    tx: &ProgressSender,
    token: &CancellationToken,
) -> anyhow::Result<()> {
    emit(
        tx,
        OperationState::Installing,
        88,
        format!("Installing {app_name}"),
    )
    .await;
    let path = dest.to_string_lossy().to_string();
    let mut command = privileged("/usr/bin/apt-get", &["install", "-y", &path]);
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::select! {
        output = command.output() => output?,
        _ = token.cancelled() => anyhow::bail!("cancelled"),
    };
    if !output.status.success() {
        // pkexec reserves 127 for a dismissed/failed authentication request.
        // apt itself maps package/script failures to 100, so this distinction
        // gives the user something actionable instead of a mysterious code.
        if output.status.code() == Some(127) {
            anyhow::bail!(
                "administrator authorization was cancelled, or no polkit authentication agent is available"
            );
        }
        let detail = command_diagnostic(&output.stdout, &output.stderr);
        if detail.is_empty() {
            anyhow::bail!("apt-get exited with {}", output.status);
        }
        anyhow::bail!("apt-get exited with {}: {detail}", output.status);
    }
    registry::add(app_name, "dpkg", repo).await?;
    emit(
        tx,
        OperationState::Succeeded,
        100,
        format!("{app_name} ready"),
    )
    .await;
    Ok(())
}

fn command_diagnostic(stdout: &[u8], stderr: &[u8]) -> String {
    let mut lines = String::from_utf8_lossy(stderr)
        .lines()
        .chain(String::from_utf8_lossy(stdout).lines())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if lines.len() > 4 {
        lines.drain(..lines.len() - 4);
    }
    lines.join(" | ")
}

/// Pick the highest-scoring asset URL/name, rejecting clearly wrong artifacts.
/// Scoring ported from UNI's score_asset.
fn pick_best_asset(release: &Value) -> Option<(String, String)> {
    let assets = release.get("assets")?.as_array()?;
    assets
        .iter()
        .filter_map(|asset| {
            let name = asset.get("name")?.as_str()?.to_string();
            let url = asset.get("browser_download_url")?.as_str()?.to_string();
            Some((score_asset(&name), url, name))
        })
        .filter(|(score, _, _)| *score > -100)
        .max_by_key(|(score, _, _)| *score)
        .map(|(_, url, name)| (url, name))
}

fn arch_patterns() -> &'static [&'static str] {
    match std::env::consts::ARCH {
        "x86_64" => &["x86_64", "amd64", "x64"],
        "aarch64" => &["aarch64", "arm64"],
        "arm" => &["armv7l", "armhf", "arm"],
        _ => &[],
    }
}

fn score_asset(name: &str) -> i32 {
    let name = name.to_ascii_lowercase();
    let mut score = 0i32;

    let ours = arch_patterns();
    if ours.iter().any(|pat| name.contains(pat)) {
        score += 50;
    }
    for other in [
        "x86_64", "amd64", "x64", "aarch64", "arm64", "armv7l", "armhf", "i386", "i686",
    ] {
        if !ours.contains(&other) && name.contains(other) {
            score -= 200;
        }
    }

    // Matched on the suffix, not anywhere in the name: sidecars are named after
    // the artifact they describe, so `App-linux-x86_64.AppImage.zsync` contains
    // `.appimage` and used to score as high as the AppImage it points at.
    for (suffix, delta) in [
        (".deb", 40),
        (".appimage", 30),
        (".tar.gz", 10),
        (".tar.xz", 10),
        (".zip", 5),
        (".rpm", -100),
        (".sha256", -500),
        (".sha512", -500),
        (".asc", -500),
        (".sig", -500),
        (".zsync", -500),
        (".blockmap", -500),
        (".exe", -300),
        (".dmg", -300),
    ] {
        if name.ends_with(suffix) {
            score += delta;
        }
    }

    for (needle, delta) in [
        ("linux", 20),
        ("source", -100),
        ("debug", -100),
        ("windows", -300),
        ("darwin", -300),
        ("macos", -300),
    ] {
        if name.contains(needle) {
            score += delta;
        }
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_reject_windows_and_checksums() {
        assert!(score_asset("app-linux-x86_64.AppImage") > score_asset("app-windows.exe"));
        assert!(score_asset("app.sha256") < -100);
        assert!(score_asset("app.deb") > 0);
    }

    #[test]
    fn scores_reject_sidecars_named_after_their_artifact() {
        assert!(score_asset("PolyMC-Linux-x86_64-7.1.AppImage.zsync") < -100);
        assert!(score_asset("app-linux-x86_64.AppImage.sha256") < -100);
        assert!(
            score_asset("PolyMC-Linux-x86_64-7.1.AppImage")
                > score_asset("PolyMC-Linux-x86_64-7.1.AppImage.zsync")
        );
    }

    #[test]
    fn picks_the_appimage_over_its_zsync() {
        let release = serde_json::json!({
            "assets": [
                {"name": "PolyMC-Linux-x86_64-7.1.AppImage", "browser_download_url": "u1"},
                {"name": "PolyMC-Linux-x86_64-7.1.AppImage.zsync", "browser_download_url": "u2"}
            ]
        });
        let (url, name) = pick_best_asset(&release).unwrap();
        assert_eq!(url, "u1");
        assert!(name.ends_with(".AppImage"));
    }

    #[test]
    fn picks_best_asset_from_release() {
        let release = serde_json::json!({
            "assets": [
                {"name": "app.sha256", "browser_download_url": "u1"},
                {"name": "app-linux-x86_64.AppImage", "browser_download_url": "u2"},
                {"name": "app-windows.exe", "browser_download_url": "u3"}
            ]
        });
        let (url, name) = pick_best_asset(&release).unwrap();
        assert_eq!(url, "u2");
        assert!(name.ends_with(".AppImage"));
    }

    #[test]
    fn command_diagnostic_keeps_the_useful_tail() {
        let stdout = b"line one\nline two\nline three\n";
        let stderr = b"error one\nerror two\n";
        let detail = command_diagnostic(stdout, stderr);
        assert_eq!(detail, "error two | line one | line two | line three");
    }
}
