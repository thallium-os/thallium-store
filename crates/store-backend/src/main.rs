use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use store_catalog::CatalogManager;
use store_core::{
    AppVariant, CanonicalApp, EnqueueOperation, Operation, OperationLog, OperationState,
    SearchParams, SourceKind, TrustLevel,
};
use store_db::StoreDb;
use store_uni::{StagePermits, UniAdapter};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::process::Command;
use tokio::sync::{broadcast, Mutex, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    catalog: CatalogManager,
    db: Arc<Mutex<StoreDb>>,
    uni: UniAdapter,
    fake_uni: bool,
    recent_apps: Arc<Mutex<HashMap<String, CanonicalApp>>>,
    active: Arc<Semaphore>,
    permits: StagePermits,
    /// Broadcasts `event.operationProgress` payloads to every connected
    /// client, not just the one that enqueued the op, so reconnects don't
    /// lose the stream.
    progress_tx: broadcast::Sender<Operation>,
    /// Cancellation tokens for currently running operations, keyed by
    /// operation id, so `operations.cancel` can signal a running backend.
    running_ops: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

#[derive(Debug, Deserialize)]
struct RpcRequest {
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct RpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstalledItem {
    id: String,
    name: String,
    source: String,
    version: Option<String>,
    detail: String,
    managed_by_uni: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateItem {
    id: String,
    name: String,
    source: String,
    current_version: Option<String>,
    available_version: Option<String>,
    managed_by_uni: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    let args: Vec<String> = std::env::args().collect();
    let socket_path = socket_path();

    if args.get(1).map(String::as_str) == Some("--request") {
        let method = args.get(2).context("missing method")?;
        let params = args.get(3).map(String::as_str).unwrap_or("{}");
        return client_request(&socket_path, method, params).await;
    }

    serve(socket_path).await
}

async fn serve(socket_path: PathBuf) -> Result<()> {
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }

    if socket_path.exists() {
        match UnixStream::connect(&socket_path).await {
            Ok(_) => {
                tracing::info!(
                    "backend already available on {}; exiting helper instance",
                    socket_path.display()
                );
                return Ok(());
            }
            Err(err)
                if matches!(
                    err.kind(),
                    ErrorKind::ConnectionRefused
                        | ErrorKind::NotFound
                        | ErrorKind::AddrNotAvailable
                ) =>
            {
                tokio::fs::remove_file(&socket_path).await?;
            }
            Err(err) => return Err(err).context("existing backend socket is not usable"),
        }
    }

    let fake_uni =
        std::env::var("THALLIUM_STORE_FAKE_UNI").unwrap_or_else(|_| "1".to_string()) != "0";
    let db_path = data_home().join("thallium-store/store.db");
    let (progress_tx, _) = broadcast::channel(256);
    let state = AppState {
        catalog: CatalogManager::new(),
        db: Arc::new(Mutex::new(StoreDb::open(&db_path)?)),
        uni: UniAdapter::new(fake_uni),
        fake_uni,
        recent_apps: Arc::new(Mutex::new(HashMap::new())),
        active: Arc::new(Semaphore::new(4)),
        // network bounds the parallel download stage across all sources;
        // *_mutation bounds each source's install stage (system MUST stay 1).
        permits: StagePermits {
            network: Arc::new(Semaphore::new(4)),
            system_mutation: Arc::new(Semaphore::new(1)),
            flatpak_mutation: Arc::new(Semaphore::new(1)),
            github_mutation: Arc::new(Semaphore::new(2)),
        },
        progress_tx,
        running_ops: Arc::new(Mutex::new(HashMap::new())),
    };

    let listener = UnixListener::bind(&socket_path)?;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
    tracing::info!("listening on {}", socket_path.display());

    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_connection(stream, state).await {
                tracing::warn!("connection failed: {err:#}");
            }
        });
    }
}

async fn handle_connection(stream: UnixStream, state: AppState) -> Result<()> {
    let (reader, writer) = stream.into_split();
    let writer = Arc::new(Mutex::new(writer));

    // Replay current operation state so a client that just (re)connected isn't
    // blind to progress made before it connected, then keep forwarding every
    // future event.operationProgress from the shared broadcast channel. This
    // decouples delivery from the connection that enqueued the op.
    let mut progress_rx = state.progress_tx.subscribe();
    let forward_writer = writer.clone();
    let replay_db = state.db.clone();
    let forward_task = tokio::spawn(async move {
        let replayed = replay_db.lock().await.list_operations();
        if let Ok(operations) = replayed {
            for operation in operations {
                let _ = write_notification(&forward_writer, "event.operationProgress", &operation).await;
            }
        }
        loop {
            match progress_rx.recv().await {
                Ok(operation) => {
                    let _ = write_notification(&forward_writer, "event.operationProgress", &operation).await;
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });

    let result = read_requests(reader, &writer, &state).await;
    forward_task.abort();
    result
}

async fn read_requests(
    reader: tokio::net::unix::OwnedReadHalf,
    writer: &Arc<Mutex<OwnedWriteHalf>>,
    state: &AppState,
) -> Result<()> {
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        let request: RpcRequest = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(err) => {
                write_response(
                    writer,
                    RpcResponse {
                        jsonrpc: "2.0",
                        id: None,
                        result: None,
                        error: Some(RpcError {
                            code: -32700,
                            message: err.to_string(),
                        }),
                    },
                )
                .await?;
                continue;
            }
        };

        let id = request.id.clone();
        let result = handle_request(request, state.clone()).await;
        let response = match result {
            Ok(result) => RpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(result),
                error: None,
            },
            Err(err) => RpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(RpcError {
                    code: -32000,
                    message: err.to_string(),
                }),
            },
        };
        write_response(writer, response).await?;
    }

    Ok(())
}

async fn handle_request(request: RpcRequest, state: AppState) -> Result<Value> {
    match request.method.as_str() {
        "system.hello" => Ok(json!({
            "name": "Thallium Store Backend",
            "protocolVersion": 1,
            "fakeUni": state.fake_uni
        })),
        "system.health" => {
            let readiness = state.uni.native_readiness();
            Ok(json!({
                "status": "ready",
                "fakeUni": state.fake_uni,
                "mode": if state.fake_uni { "fake" } else { "real-json" },
                "apt": readiness.apt,
                "flatpak": readiness.flatpak,
                "privilege": readiness.privilege
            }))
        }
        "catalog.search" => {
            let params: SearchParams = serde_json::from_value(request.params)?;
            let response = state.catalog.search(params).await;
            let mut cache = state.recent_apps.lock().await;
            for app in &response.results {
                cache.insert(app.id.clone(), app.clone());
            }
            Ok(serde_json::to_value(response)?)
        }
        "catalog.appDetails" => {
            let id = request
                .params
                .get("id")
                .and_then(Value::as_str)
                .context("missing id")?;
            let cached = state.recent_apps.lock().await.get(id).cloned();
            let app = cached.or_else(|| state.catalog.app_details(id));
            if let Some(app) = app {
                Ok(serde_json::to_value(
                    state.catalog.enrich_details(app).await,
                )?)
            } else {
                Ok(Value::Null)
            }
        }
        "installed.list" => Ok(json!({ "items": installed_list().await })),
        "updates.list" => Ok(json!({ "items": updates_list().await })),
        "operations.list" => {
            let operations = state.db.lock().await.list_operations()?;
            Ok(json!({ "items": operations }))
        }
        "operations.logs" => {
            let id = request
                .params
                .get("operationId")
                .and_then(Value::as_str)
                .context("missing operationId")?;
            let logs = state.db.lock().await.logs(id)?;
            Ok(json!({ "items": logs }))
        }
        "operations.cancel" => {
            let id = request
                .params
                .get("operationId")
                .and_then(Value::as_str)
                .context("missing operationId")?;
            let token = state.running_ops.lock().await.get(id).cloned();
            match token {
                Some(token) => {
                    token.cancel();
                    if let Some(mut operation) = state.db.lock().await.get_operation(id)? {
                        operation.state = OperationState::Cancelled;
                        operation.message = "Cancellation requested".to_string();
                        operation.updated_at = Utc::now();
                        state.db.lock().await.upsert_operation(&operation)?;
                        let _ = state.progress_tx.send(operation);
                    }
                    Ok(json!({ "accepted": true }))
                }
                None => Ok(json!({
                    "accepted": false,
                    "message": "operation is not currently running"
                })),
            }
        }
        "operations.retry" => Err(anyhow::anyhow!("retry is not implemented in the MVP slice")),
        "apps.launch" => Err(anyhow::anyhow!(
            "launch requires trusted desktop entry discovery; not enabled in fake MVP mode"
        )),
        "operations.enqueue" => enqueue_operation(request.params, state).await,
        _ => Err(anyhow::anyhow!("unknown method {}", request.method)),
    }
}

async fn enqueue_operation(params: Value, state: AppState) -> Result<Value> {
    let request: EnqueueOperation = serde_json::from_value(params)?;
    let cached_app = state
        .recent_apps
        .lock()
        .await
        .get(&request.app_id)
        .cloned()
        .or_else(|| state.catalog.app_details(&request.app_id));

    let (app_id, app_name, variant, command_preview) = if let Some(app) = cached_app {
        let variant = app
            .variants
            .iter()
            .find(|variant| variant.id == request.variant_id)
            .cloned()
            .context("variant not found")?;
        (
            app.id.clone(),
            app.name.clone(),
            variant.clone(),
            variant.command_preview.clone(),
        )
    } else {
        let source = request
            .source
            .context("app not found in recent catalog; source is required for direct enqueue")?;
        let package_id = request.package_id.clone().context(
            "app not found in recent catalog; package_id is required for direct enqueue",
        )?;
        let app_name = request
            .app_name
            .clone()
            .unwrap_or_else(|| package_id.clone());
        let command_preview =
            UniAdapter::command_preview(request.action, &package_id, source).join(" ");
        let trust = match source {
            SourceKind::System => TrustLevel::SystemAccess,
            SourceKind::Flathub => TrustLevel::Sandboxed,
            SourceKind::Github | SourceKind::Appimage => TrustLevel::Unverified,
        };
        let variant = AppVariant {
            id: request.variant_id.clone(),
            source,
            package_id,
            version: None,
            download_size: None,
            installed_size: None,
            architecture: None,
            repository: None,
            install_location: None,
            trust,
            verified: false,
            command_preview: command_preview.clone(),
            ranking_reasons: Vec::new(),
        };
        (request.app_id.clone(), app_name, variant, command_preview)
    };

    let now = Utc::now();
    let operation = Operation {
        id: Uuid::now_v7().to_string(),
        app_id,
        app_name,
        variant_id: variant.id.clone(),
        source: variant.source,
        action: request.action,
        state: OperationState::Pending,
        percent: 0,
        message: "Queued".to_string(),
        created_at: now,
        updated_at: now,
    };
    state.db.lock().await.upsert_operation(&operation)?;
    state.db.lock().await.add_log(&OperationLog {
        operation_id: operation.id.clone(),
        timestamp: now,
        level: "info".to_string(),
        message: command_preview,
    })?;

    let token = CancellationToken::new();
    state
        .running_ops
        .lock()
        .await
        .insert(operation.id.clone(), token.clone());

    let operation_for_task = operation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        run_operation(state, operation_for_task, variant.package_id, token).await;
    });

    Ok(serde_json::to_value(operation)?)
}

async fn run_operation(
    state: AppState,
    mut operation: Operation,
    package_id: String,
    token: CancellationToken,
) {
    let _active = state.active.clone().acquire_owned().await.ok();

    // Stage-split scheduler boundary: each backend acquires `permits.network`
    // around its download-heavy portion and the correct `permits.*_mutation`
    // around its install-heavy portion (see store-uni::backends), so downloads
    // across ops/sources overlap while installs stay serial per source
    // (system_mutation permit=1, never relaxed). We just hand the whole
    // permit set + cancellation token down.
    let mut rx = state.uni.run(
        operation.action,
        operation.app_name.clone(),
        package_id,
        operation.source,
        token,
        state.permits.clone(),
    );

    while let Some(event) = rx.recv().await {
        match event {
            Ok(progress) => {
                operation.state = progress.state;
                operation.percent = progress.percent;
                operation.message = progress.message;
            }
            Err(message) => {
                operation.state = OperationState::Failed;
                operation.percent = 100;
                operation.message = message;
            }
        }
        operation.updated_at = Utc::now();
        let _ = state.db.lock().await.upsert_operation(&operation);
        let _ = state.db.lock().await.add_log(&OperationLog {
            operation_id: operation.id.clone(),
            timestamp: operation.updated_at,
            level: if operation.state == OperationState::Failed {
                "error".to_string()
            } else {
                "info".to_string()
            },
            message: operation.message.clone(),
        });
        let _ = state.progress_tx.send(operation.clone());
    }

    state.running_ops.lock().await.remove(&operation.id);
}

async fn write_response(writer: &Arc<Mutex<OwnedWriteHalf>>, response: RpcResponse) -> Result<()> {
    let mut writer = writer.lock().await;
    writer
        .write_all(serde_json::to_string(&response)?.as_bytes())
        .await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

async fn write_notification<T: Serialize>(
    writer: &Arc<Mutex<OwnedWriteHalf>>,
    method: &str,
    params: &T,
) -> Result<()> {
    let mut writer = writer.lock().await;
    let payload = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });
    writer
        .write_all(serde_json::to_string(&payload)?.as_bytes())
        .await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

async fn client_request(socket_path: &Path, method: &str, params: &str) -> Result<()> {
    let mut stream = connect_with_retry(socket_path).await?;
    let params: Value = serde_json::from_str(params)?;
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    stream
        .write_all(serde_json::to_string(&request)?.as_bytes())
        .await?;
    stream.write_all(b"\n").await?;

    let mut lines = BufReader::new(stream).lines();
    while let Some(line) = lines.next_line().await? {
        let value: Value = serde_json::from_str(&line)?;
        if value.get("id").and_then(Value::as_i64) == Some(1) {
            println!("{line}");
            return Ok(());
        }
    }

    anyhow::bail!("backend closed connection without a response")
}

async fn connect_with_retry(socket_path: &Path) -> Result<UnixStream> {
    let mut last_error = None;

    for _ in 0..30 {
        match UnixStream::connect(socket_path).await {
            Ok(stream) => return Ok(stream),
            Err(err)
                if matches!(
                    err.kind(),
                    ErrorKind::NotFound | ErrorKind::ConnectionRefused | ErrorKind::WouldBlock
                ) =>
            {
                last_error = Some(err);
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            Err(err) => return Err(err).context("failed to connect to backend socket"),
        }
    }

    Err(last_error
        .unwrap_or_else(|| std::io::Error::new(ErrorKind::TimedOut, "backend socket not ready"))
        .into())
}

fn socket_path() -> PathBuf {
    std::env::var_os("THALLIUM_STORE_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| runtime_dir().join("thallium-store/backend.sock"))
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME").unwrap_or_else(|| ".".into());
            PathBuf::from(home).join(".local/share")
        })
}

async fn installed_list() -> Vec<InstalledItem> {
    let mut items = Vec::new();
    items.extend(read_registry_installed().await);
    items.extend(read_flatpak_installed().await);
    dedupe_installed(items)
}

async fn read_registry_installed() -> Vec<InstalledItem> {
    let registry = match store_uni::registry::load().await {
        Ok(registry) => registry,
        Err(err) => {
            tracing::warn!("reading UNI registry failed: {err:#}");
            return Vec::new();
        }
    };

    registry
        .into_iter()
        .map(|(name, entry)| InstalledItem {
            id: format!("uni:{}:{}", entry.backend, entry.id),
            name,
            source: entry.backend,
            version: None,
            detail: entry.id,
            managed_by_uni: true,
        })
        .collect()
}

async fn read_flatpak_installed() -> Vec<InstalledItem> {
    let output = match Command::new("flatpak")
        .args(["list", "--app", "--columns=application,name,version,origin"])
        .output()
        .await
    {
        Ok(output) if output.status.success() => output,
        _ => return Vec::new(),
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let columns = line.split('\t').collect::<Vec<_>>();
            let app_id = columns.first()?.trim();
            if app_id.is_empty() {
                return None;
            }
            let name = columns.get(1).map(|s| s.trim()).unwrap_or(app_id);
            let version = columns
                .get(2)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let origin = columns.get(3).map(|s| s.trim()).unwrap_or("flatpak");
            Some(InstalledItem {
                id: format!("flatpak:{app_id}"),
                name: name.to_string(),
                source: "flatpak".to_string(),
                version,
                detail: origin.to_string(),
                managed_by_uni: false,
            })
        })
        .collect()
}

fn dedupe_installed(items: Vec<InstalledItem>) -> Vec<InstalledItem> {
    let mut deduped: Vec<InstalledItem> = Vec::new();
    for item in items {
        if !deduped.iter().any(|existing| {
            existing.id == item.id || existing.name.eq_ignore_ascii_case(&item.name)
        }) {
            deduped.push(item);
        }
    }
    deduped.sort_by_key(|item| item.name.to_lowercase());
    deduped
}

const UPDATES_TIMEOUT: Duration = Duration::from_secs(8);
const GITHUB_UPDATE_CONCURRENCY: usize = 4;

/// Discover available updates from apt, flatpak, and GitHub-tracked
/// installs concurrently, mirroring `CatalogManager::search`'s per-provider
/// timeout fan-out so one slow/misbehaving source can't stall the others.
async fn updates_list() -> Vec<UpdateItem> {
    let (apt, flatpak, github) = tokio::join!(
        async {
            tokio::time::timeout(UPDATES_TIMEOUT, read_apt_updates())
                .await
                .unwrap_or_default()
        },
        async {
            tokio::time::timeout(UPDATES_TIMEOUT, read_flatpak_updates())
                .await
                .unwrap_or_default()
        },
        async {
            tokio::time::timeout(UPDATES_TIMEOUT, read_github_updates())
                .await
                .unwrap_or_default()
        },
    );

    let mut items = Vec::new();
    items.extend(apt);
    items.extend(flatpak);
    items.extend(github);
    items.sort_by_key(|item| item.name.to_lowercase());
    items
}

/// Read-only `apt list --upgradable` — no dpkg lock, no root, no mutation.
async fn read_apt_updates() -> Vec<UpdateItem> {
    let output = match Command::new("apt")
        .args(["list", "--upgradable"])
        .output()
        .await
    {
        Ok(output) if output.status.success() => output,
        _ => return Vec::new(),
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_apt_upgradable_line)
        .collect()
}

/// Parses one line of `apt list --upgradable` output, e.g.
/// `firefox/jammy-updates 118.0-0ubuntu1 amd64 [upgradable from: 117.0-0ubuntu1]`.
fn parse_apt_upgradable_line(line: &str) -> Option<UpdateItem> {
    if !line.contains("[upgradable from:") {
        return None;
    }

    let mut parts = line.split_whitespace();
    let pkg = parts.next()?.split('/').next()?.trim();
    if pkg.is_empty() {
        return None;
    }
    let available_version = parts.next()?.to_string();
    let _arch = parts.next();
    let remainder = parts.collect::<Vec<_>>().join(" ");
    let current_version = remainder
        .rsplit("from:")
        .next()
        .map(|s| s.trim_end_matches(']').trim().to_string())
        .filter(|s| !s.is_empty());

    Some(UpdateItem {
        id: format!("apt:{pkg}"),
        name: pkg.to_string(),
        source: "apt".to_string(),
        current_version,
        available_version: Some(available_version),
        managed_by_uni: false,
    })
}

/// Read-only `flatpak remote-ls --updates` — no repo mutation.
async fn read_flatpak_updates() -> Vec<UpdateItem> {
    let output = match Command::new("flatpak")
        .args([
            "remote-ls",
            "--updates",
            "--user",
            "--columns=application,name,version",
        ])
        .output()
        .await
    {
        Ok(output) if output.status.success() => output,
        _ => return Vec::new(),
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(parse_flatpak_update_line)
        .collect()
}

fn parse_flatpak_update_line(line: &str) -> Option<UpdateItem> {
    let columns = line.split('\t').collect::<Vec<_>>();
    let app_id = columns.first()?.trim();
    if app_id.is_empty() {
        return None;
    }
    let name = columns.get(1).map(|s| s.trim()).unwrap_or(app_id);
    // `remote-ls` only reports the remote (available) version; the
    // currently-installed version isn't in this table, so it's left unset
    // rather than shelling out to a second `flatpak list` per row.
    let available_version = columns
        .get(2)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    Some(UpdateItem {
        id: format!("flatpak:{app_id}"),
        name: name.to_string(),
        source: "flatpak".to_string(),
        current_version: None,
        available_version,
        managed_by_uni: false,
    })
}

/// For every UNI-registry entry installed via GitHub, ask the GitHub API for
/// the latest release tag. The registry only records `{backend, id}` (no
/// installed version), so this is always the documented best-effort case:
/// `currentVersion` is null and `availableVersion` is the latest tag.
/// Concurrency is capped (not unbounded `tokio::join!`) to avoid hammering
/// the API when a user has many GitHub-sourced installs.
async fn read_github_updates() -> Vec<UpdateItem> {
    let registry = match store_uni::registry::load().await {
        Ok(registry) => registry,
        Err(err) => {
            tracing::warn!("reading UNI registry for updates failed: {err:#}");
            return Vec::new();
        }
    };

    let github_entries: Vec<(String, String)> = registry
        .into_iter()
        .filter(|(_, entry)| entry.backend == "github")
        .map(|(name, entry)| (name, entry.id))
        .collect();

    if github_entries.is_empty() {
        return Vec::new();
    }

    let client = match reqwest::Client::builder()
        .user_agent("thallium-store")
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            tracing::warn!("building GitHub client for updates failed: {err:#}");
            return Vec::new();
        }
    };

    let semaphore = Arc::new(Semaphore::new(GITHUB_UPDATE_CONCURRENCY));
    let mut tasks = Vec::new();
    for (name, repo_id) in github_entries {
        let client = client.clone();
        let semaphore = semaphore.clone();
        tasks.push(tokio::spawn(async move {
            let _permit = semaphore.acquire_owned().await.ok();
            tokio::time::timeout(UPDATES_TIMEOUT, fetch_github_latest(&client, &name, &repo_id))
                .await
                .ok()
                .flatten()
        }));
    }

    let mut items = Vec::new();
    for task in tasks {
        if let Ok(Some(item)) = task.await {
            items.push(item);
        }
    }
    items
}

async fn fetch_github_latest(
    client: &reqwest::Client,
    name: &str,
    repo_id: &str,
) -> Option<UpdateItem> {
    let (owner, repo) = repo_id.split_once('/')?;
    let mut request = client.get(format!(
        "https://api.github.com/repos/{owner}/{repo}/releases/latest"
    ));
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        request = request.bearer_auth(token);
    }
    let release: Value = request.send().await.ok()?.error_for_status().ok()?.json().await.ok()?;
    let tag_name = release.get("tag_name").and_then(Value::as_str)?.to_string();

    Some(UpdateItem {
        id: format!("uni:github:{repo_id}"),
        name: name.to_string(),
        source: "github".to_string(),
        current_version: None,
        available_version: Some(tag_name),
        managed_by_uni: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_apt_line_extracts_current_and_available_versions() {
        let line = "firefox/jammy-updates 118.0+build1-0ubuntu0.22.04.1 amd64 [upgradable from: 117.0+build2-0ubuntu0.22.04.1]";
        let item = parse_apt_upgradable_line(line).expect("parses upgradable line");
        assert_eq!(item.id, "apt:firefox");
        assert_eq!(item.name, "firefox");
        assert_eq!(item.source, "apt");
        assert_eq!(
            item.available_version.as_deref(),
            Some("118.0+build1-0ubuntu0.22.04.1")
        );
        assert_eq!(
            item.current_version.as_deref(),
            Some("117.0+build2-0ubuntu0.22.04.1")
        );
        assert!(!item.managed_by_uni);
    }

    #[test]
    fn parse_apt_line_ignores_non_upgrade_output() {
        assert!(parse_apt_upgradable_line("Listing... Done").is_none());
        assert!(parse_apt_upgradable_line("").is_none());
    }

    #[test]
    fn parse_flatpak_update_line_reads_columns() {
        let item = parse_flatpak_update_line("org.gimp.GIMP\tGIMP\t2.10.36")
            .expect("parses flatpak update line");
        assert_eq!(item.id, "flatpak:org.gimp.GIMP");
        assert_eq!(item.name, "GIMP");
        assert_eq!(item.available_version.as_deref(), Some("2.10.36"));
        assert!(item.current_version.is_none());
    }

    #[test]
    fn dedupe_collapses_case_insensitive_names_and_sorts() {
        let items = vec![
            InstalledItem {
                id: "flatpak:org.zed.Zed".into(),
                name: "Zed".into(),
                source: "flatpak".into(),
                version: None,
                detail: "flathub".into(),
                managed_by_uni: false,
            },
            InstalledItem {
                id: "uni:dpkg:zed".into(),
                name: "zed".into(),
                source: "dpkg".into(),
                version: None,
                detail: "zed".into(),
                managed_by_uni: true,
            },
            InstalledItem {
                id: "flatpak:org.gimp.GIMP".into(),
                name: "GIMP".into(),
                source: "flatpak".into(),
                version: None,
                detail: "flathub".into(),
                managed_by_uni: false,
            },
        ];

        let out = dedupe_installed(items);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].name, "GIMP");
        assert_eq!(out[1].name, "Zed");
    }
}
