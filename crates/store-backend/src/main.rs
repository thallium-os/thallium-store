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
use store_uni::apt_cache::{self, AptRefresh};
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

fn settings_path() -> PathBuf {
    let config = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".config")
        });
    config.join("thallium-store/settings.json")
}

fn load_settings() -> Value {
    std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({}))
}

fn save_settings(settings: &Value) -> Result<()> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(settings)?)?;
    Ok(())
}

fn icon_cache_info() -> Value {
    let dir = store_catalog::icon_cache_dir();
    let mut bytes: u64 = 0;
    let mut count: u64 = 0;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    bytes += meta.len();
                    count += 1;
                }
            }
        }
    }
    json!({ "iconBytes": bytes, "iconCount": count })
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

    // Simulation is opt-in. It used to be the default, so a backend started
    // without the wrapper script -- by hand, by a launcher, by anything that
    // did not export the variable -- silently faked every install: progress
    // bars that move, an operation that "downloads" without touching the
    // network, and nothing on disk at the end of it.
    let fake_uni = std::env::var("THALLIUM_STORE_FAKE_UNI").unwrap_or_else(|_| "0".to_string()) != "0";
    let db_path = data_home().join("thallium-store/store.db");
    let (progress_tx, _) = broadcast::channel(256);
    let db = StoreDb::open(&db_path)?;
    // Anything still mid-flight belongs to a process that is already gone.
    match db.reap_orphaned_operations() {
        Ok(0) => {}
        Ok(n) => tracing::info!("cancelled {n} operation(s) left running by a previous backend"),
        Err(err) => tracing::warn!("could not reap orphaned operations: {err}"),
    }
    let state = AppState {
        catalog: CatalogManager::new(),
        db: Arc::new(Mutex::new(db)),
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

    // Warm the catalog index in the background so searches are served from
    // memory; live providers cover the brief window before it's ready.
    let warm_catalog = state.catalog.clone();
    tokio::spawn(async move { warm_catalog.warm().await });

    let listener = UnixListener::bind(&socket_path)?;
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;
    tracing::info!("listening on {}", socket_path.display());

    // Take the socket file with us on the way out. The UI owns this process and
    // kills it when its window closes, which otherwise leaves a socket nobody
    // is listening on -- and the next launch spends its startup connecting to
    // that corpse and printing "connection refused" over the splash screen.
    {
        let socket_path = socket_path.clone();
        tokio::spawn(async move {
            let mut term = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(sig) => sig,
                Err(err) => {
                    tracing::warn!("cannot watch for SIGTERM: {err}");
                    return;
                }
            };
            tokio::select! {
                _ = term.recv() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
            let _ = std::fs::remove_file(&socket_path);
            std::process::exit(0);
        });
    }

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
                let _ = write_notification(&forward_writer, "event.operationProgress", &operation)
                    .await;
            }
        }
        loop {
            match progress_rx.recv().await {
                Ok(operation) => {
                    let _ =
                        write_notification(&forward_writer, "event.operationProgress", &operation)
                            .await;
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
        "settings.get" => Ok(json!({ "settings": load_settings() })),
        "settings.set" => {
            let mut settings = load_settings();
            if let (Some(target), Some(patch)) =
                (settings.as_object_mut(), request.params.as_object())
            {
                for (key, value) in patch {
                    target.insert(key.clone(), value.clone());
                }
            }
            save_settings(&settings)?;
            Ok(json!({ "settings": settings }))
        }
        "system.cacheInfo" => Ok(icon_cache_info()),
        "system.clearIconCache" => {
            let dir = store_catalog::icon_cache_dir();
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
            Ok(icon_cache_info())
        }
        "system.health" => {
            let readiness = state.uni.native_readiness();
            Ok(json!({
                "status": "ready",
                // The UI used to print a hardcoded version string, which had
                // drifted five releases behind the package it shipped in.
                "version": env!("CARGO_PKG_VERSION"),
                "fakeUni": state.fake_uni,
                "mode": if state.fake_uni { "fake" } else { "real-json" },
                "apt": readiness.apt,
                "flatpak": readiness.flatpak,
                "privilege": readiness.privilege
            }))
        }
        "catalog.search" => {
            let params: SearchParams = serde_json::from_value(request.params)?;
            let mut response = state.catalog.search(params).await;
            let mut cache = state.recent_apps.lock().await;
            for app in &response.results {
                cache.insert(app.id.clone(), app.clone());
            }
            for app in &mut response.results {
                store_catalog::rewrite_icon(app);
            }
            Ok(serde_json::to_value(response)?)
        }
        "catalog.discover" => {
            let mut collections = state.catalog.discover();
            let mut cache = state.recent_apps.lock().await;
            for collection in &collections {
                for app in &collection.apps {
                    cache.insert(app.id.clone(), app.clone());
                }
            }
            for collection in &mut collections {
                for app in &mut collection.apps {
                    store_catalog::rewrite_icon(app);
                }
            }
            Ok(json!({ "collections": collections }))
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
                let mut app = state.catalog.enrich_details(app).await;
                store_catalog::rewrite_icon(&mut app);
                Ok(serde_json::to_value(app)?)
            } else {
                Ok(Value::Null)
            }
        }
        "installed.list" => Ok(json!({ "items": installed_list().await })),
        "updates.list" => {
            // Refresh only when the caller says a person asked for it: the
            // refresh needs root, and a polkit prompt on every automatic poll
            // is a prompt people learn to dismiss.
            let refresh = request
                .params
                .get("refresh")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(serde_json::to_value(updates_list(refresh).await)?)
        }
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
                    // The lookup is bound to its own statement on purpose. In
                    // edition 2021 a temporary in an `if let` scrutinee lives
                    // until the end of the whole `if let`, so locking the db
                    // again inside the body waited on a guard the same task was
                    // still holding: pressing Cancel deadlocked the db mutex and
                    // every later operations.list, enqueue and dismiss hung
                    // behind it for the life of the process.
                    let existing = state.db.lock().await.get_operation(id)?;
                    if let Some(mut operation) = existing {
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
        // Re-run a finished operation from its own stored record. The old row
        // is deleted rather than left behind: a retry that piles a second row
        // on top of the first turns Activity into a list of every attempt ever
        // made, which is exactly what it looked like before this existed.
        "operations.retry" => {
            let id = request
                .params
                .get("operationId")
                .and_then(Value::as_str)
                .context("missing operationId")?;
            let previous = state
                .db
                .lock()
                .await
                .get_operation(id)?
                .context("unknown operation")?;
            let terminal = matches!(
                previous.state,
                OperationState::Succeeded | OperationState::Failed | OperationState::Cancelled
            );
            if !terminal {
                anyhow::bail!("operation is still running");
            }
            let package_id = previous
                .package_id
                .clone()
                .context("operation predates package_id and cannot be retried")?;
            let params = json!({
                "app_id": previous.app_id,
                "variant_id": previous.variant_id,
                "action": previous.action,
                "app_name": previous.app_name,
                "package_id": package_id,
                "source": previous.source,
            });
            let queued = enqueue_operation(params, state.clone()).await?;
            state.db.lock().await.delete_operation(id)?;
            Ok(queued)
        }
        // Remove one finished row from Activity. A failure is the only record
        // of itself, so it has to be dismissed deliberately -- but it does have
        // to be dismissible, or the rail accretes every attempt forever.
        "operations.dismiss" => {
            let id = request
                .params
                .get("operationId")
                .and_then(Value::as_str)
                .context("missing operationId")?;
            let dismissed = state.db.lock().await.delete_operation(id)?;
            Ok(json!({ "dismissed": dismissed }))
        }
        // Bulk version of the same. `states` defaults to failures alone,
        // because "clear all errors" is the thing people actually want and
        // sweeping succeeded rows away with it is not.
        "operations.clear" => {
            let states = match request.params.get("states") {
                Some(value) => serde_json::from_value::<Vec<OperationState>>(value.clone())
                    .context("states must be operation state names")?,
                None => vec![OperationState::Failed],
            };
            let removed = state.db.lock().await.delete_operations_in_states(&states)?;
            Ok(json!({ "removed": removed }))
        }
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
        package_id: Some(variant.package_id.clone()),
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
    let cancelled = token.clone();
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
                // A backend notices cancellation only at its next chunk
                // boundary, and its progress ticker keeps firing until then.
                // Those events used to overwrite the Cancelled row that
                // operations.cancel had just written, so the row flipped back
                // to "downloading" for as long as the transfer took to give up.
                let terminal = matches!(
                    progress.state,
                    OperationState::Succeeded
                        | OperationState::Failed
                        | OperationState::Cancelled
                );
                if cancelled.is_cancelled() && !terminal {
                    continue;
                }
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

    // A backend that exits without a terminal event would leave a row nothing
    // can ever dismiss or retry, because both refuse anything still running.
    // Nothing is driving it any more once we are here, so say so.
    let settled = matches!(
        operation.state,
        OperationState::Succeeded | OperationState::Failed | OperationState::Cancelled
    );
    if !settled {
        operation.state = if cancelled.is_cancelled() {
            OperationState::Cancelled
        } else {
            OperationState::Failed
        };
        operation.message = if cancelled.is_cancelled() {
            format!("Cancelled {}", operation.app_name)
        } else {
            format!("{} ended without reporting a result", operation.app_name)
        };
        operation.updated_at = Utc::now();
        let _ = state.db.lock().await.upsert_operation(&operation);
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
    let mut refusals = 0;
    let mut swept = false;

    for _ in 0..60 {
        match UnixStream::connect(socket_path).await {
            Ok(stream) => return Ok(stream),
            Err(err)
                if matches!(
                    err.kind(),
                    ErrorKind::NotFound | ErrorKind::ConnectionRefused | ErrorKind::WouldBlock
                ) =>
            {
                // ConnectionRefused on a unix socket means the file is there and
                // nobody is behind it: the daemon was killed without cleaning up,
                // which is exactly what happens when the UI exits and takes its
                // child with it. Retrying that forever just spells the corpse's
                // error onto the splash screen. Unlink it once -- after a few
                // refusals, so a daemon binding this instant is not disturbed --
                // and let the next attempt find the socket the UI's backend
                // process is about to create.
                if err.kind() == ErrorKind::ConnectionRefused {
                    refusals += 1;
                    if refusals >= 3 && !swept {
                        swept = true;
                        let _ = tokio::fs::remove_file(socket_path).await;
                    }
                }
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
/// Generous next to `UPDATES_TIMEOUT`, because most of it is a human reading a
/// polkit prompt, not apt working.
const APT_REFRESH_TIMEOUT: Duration = Duration::from_secs(120);

/// `updates.list`'s reply. `items` stays where it was so older UIs keep
/// working; the apt fields let a newer one distinguish "nothing to update"
/// from "could not look".
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdatesReport {
    items: Vec<UpdateItem>,
    /// One of `skipped` / `ok` / `declined` / `failed`.
    apt_refresh: &'static str,
    apt_error: Option<String>,
    /// Unix seconds; when apt's lists were last written.
    apt_checked_at: Option<u64>,
}

/// Discover available updates from apt, flatpak, and GitHub-tracked
/// installs concurrently, mirroring `CatalogManager::search`'s per-provider
/// timeout fan-out so one slow/misbehaving source can't stall the others.
async fn updates_list(refresh: bool) -> UpdatesReport {
    // Sequenced, not joined: reading apt's cache before refreshing it would
    // answer from exactly the stale data the refresh exists to replace.
    let apt_refresh: AptRefresh = if refresh {
        apt_cache::refresh(APT_REFRESH_TIMEOUT).await
    } else {
        AptRefresh::skipped()
    };

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

    UpdatesReport {
        items,
        apt_refresh: apt_refresh.outcome.as_str(),
        apt_error: apt_refresh.error,
        apt_checked_at: apt_refresh.checked_at,
    }
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

    let semaphore = Arc::new(Semaphore::new(GITHUB_UPDATE_CONCURRENCY));
    let mut tasks = Vec::new();
    for (name, repo_id) in github_entries {
        let semaphore = semaphore.clone();
        tasks.push(tokio::spawn(async move {
            let _permit = semaphore.acquire_owned().await.ok();
            tokio::time::timeout(UPDATES_TIMEOUT, fetch_github_latest(&name, &repo_id))
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

async fn fetch_github_latest(name: &str, repo_id: &str) -> Option<UpdateItem> {
    let (owner, repo) = repo_id.split_once('/')?;
    // Shared cache + rate-limit breaker: one update sweep is one request per
    // installed GitHub app, which is most of an anonymous hourly budget.
    let release = store_catalog::github_json(format!(
        "https://api.github.com/repos/{owner}/{repo}/releases/latest"
    ))
    .await
    .ok()?;
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
