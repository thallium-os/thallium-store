//! Bakes the package version into the binary.
//!
//! `system.health` used to report `CARGO_PKG_VERSION`, which is the bare
//! workspace version and never moves -- so Settings showed "0.1.10" on every
//! build ever made, including ones with completely different content. The
//! version people actually have is the Debian one, and `scripts/deb-version`
//! owns how it is built: the Cargo version plus the commit's UTC timestamp and
//! short SHA, so a new commit changes the version by construction.
//!
//! This computes the same string so the binary reports what apt installed.
//! `THALLIUM_STORE_VERSION` wins when set -- `scripts/build-deb` exports the
//! exact string it is about to stamp into the control file, so the two can
//! never disagree.

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=THALLIUM_STORE_VERSION");
    // The stamp is derived from HEAD, so it has to be recomputed when HEAD
    // moves. Without this a rebuild after a commit keeps the old SHA.
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for path in ["HEAD", "logs/HEAD"] {
        let watched = repo.join(".git").join(path);
        if watched.exists() {
            println!("cargo:rerun-if-changed={}", watched.display());
        }
    }

    let version = std::env::var("THALLIUM_STORE_VERSION")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| git_stamp(&repo))
        .unwrap_or_else(|| {
            std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "unknown".to_string())
        });

    println!("cargo:rustc-env=THALLIUM_STORE_VERSION={version}");
}

/// `<cargo version>+<commit UTC timestamp>.g<short sha>`, matching
/// `scripts/deb-version`. Returns None outside a git checkout (a source
/// tarball, say), where the bare Cargo version is the only truth available.
fn git_stamp(repo: &Path) -> Option<String> {
    let base = std::env::var("CARGO_PKG_VERSION").ok()?;
    let git = |args: &[&str]| -> Option<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("TZ", "UTC")
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
        if value.is_empty() {
            None
        } else {
            Some(value)
        }
    };

    let stamp = git(&[
        "log",
        "-1",
        "--date=format-local:%Y%m%d%H%M%S",
        "--format=%cd",
    ])?;
    let sha = git(&["rev-parse", "--short=7", "HEAD"])?;
    Some(format!("{base}+{stamp}.g{sha}"))
}
