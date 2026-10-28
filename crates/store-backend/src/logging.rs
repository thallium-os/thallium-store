//! Tracing setup: stderr plus a log file, with a level that can change at
//! runtime so the debug toggle in Settings takes effect without a restart.

use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, reload, EnvFilter, Registry};

static HANDLE: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();
static DEBUG: OnceLock<Mutex<bool>> = OnceLock::new();

/// Rotate the log once it passes this size so it cannot grow without bound.
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

pub fn log_dir() -> PathBuf {
    crate::data_home().join("thallium-store/logs")
}

fn level_for(debug: bool) -> &'static str {
    if debug {
        "debug"
    } else {
        "info"
    }
}

fn open_log_file() -> Option<File> {
    let dir = log_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("backend.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = std::fs::rename(&path, dir.join("backend.log.1"));
    }
    OpenOptions::new().create(true).append(true).open(path).ok()
}

pub fn init(debug: bool) {
    let (filter, handle) = reload::Layer::new(EnvFilter::new(level_for(debug)));
    let file_layer = open_log_file().map(|file| {
        fmt::layer()
            .with_target(false)
            .with_ansi(false)
            .with_writer(Mutex::new(file))
    });
    let _ = Registry::default()
        .with(filter)
        .with(fmt::layer().with_target(false))
        .with(file_layer)
        .try_init();
    let _ = HANDLE.set(handle);
    let _ = DEBUG.set(Mutex::new(debug));
}

pub fn set_debug(debug: bool) {
    if let Some(handle) = HANDLE.get() {
        let _ = handle.reload(EnvFilter::new(level_for(debug)));
    }
    if let Some(flag) = DEBUG.get() {
        *flag.lock().unwrap_or_else(|e| e.into_inner()) = debug;
    }
}

pub fn is_debug() -> bool {
    DEBUG
        .get()
        .map(|flag| *flag.lock().unwrap_or_else(|e| e.into_inner()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_maps_to_level() {
        assert_eq!(level_for(true), "debug");
        assert_eq!(level_for(false), "info");
    }
}
