use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use store_catalog::CatalogManager;
use store_core::{
    AppVariant, CanonicalApp, EnqueueOperation, Operation, OperationLog, OperationState,
    SearchParams, SourceKind, TrustLevel,
};
use store_db::StoreDb;
use store_uni::UniAdapter;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::process::Command;
use tokio::sync::{Mutex, Semaphore};
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    catalog: CatalogManager,
    db: Arc<Mutex<StoreDb>>,
    uni: UniAdapter,
    fake_uni: bool,
    recent_apps: Arc<Mutex<HashMap<String, CanonicalApp>>>,
    active: Arc<Semaphore>,
    system_mutation: Arc<Semaphore>,
    flatpak_mutation: Arc<Semaphore>,
    github_mutation: Arc<Semaphore>,
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
    let state = AppState {
        catalog: CatalogManager::new(),
        db: Arc::new(Mutex::new(StoreDb::open(&db_path)?)),
        uni: UniAdapter::new(fake_uni),
        fake_uni,
        recent_apps: Arc::new(Mutex::new(HashMap::new())),
        active: Arc::new(Semaphore::new(4)),
        system_mutation: Arc::new(Semaphore::new(1)),
        flatpak_mutation: Arc::new(Semaphore::new(1)),
        github_mutation: Arc::new(Semaphore::new(2)),
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
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await? {
        let request: RpcRequest = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(err) => {
                write_response(
                    &writer,
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
        let result = handle_request(request, state.clone(), writer.clone()).await;
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
        write_response(&writer, response).await?;
    }

    Ok(())
}

async fn handle_request(
    request: RpcRequest,
    state: AppState,
    writer: Arc<Mutex<OwnedWriteHalf>>,
) -> Result<Value> {
    match request.method.as_str() {
        "system.hello" => Ok(json!({
            "name": "Thallium Store Backend",
            "protocolVersion": 1,
            "fakeUni": state.fake_uni
        })),
        "system.health" => Ok(json!({
            "status": "ready",
            "fakeUni": state.fake_uni,
            "mode": if state.fake_uni { "fake" } else { "real-json" },
            "uni": state.uni.health().await.unwrap_or_else(|err| err.to_string())
        })),
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
        "updates.list" => Ok(json!({ "items": [] })),
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
        "operations.cancel" => Ok(json!({
            "accepted": false,
            "message": "Cancellation is available for queued jobs in the scheduler contract; active fake UNI jobs finish quickly in this MVP."
        })),
        "operations.retry" => Err(anyhow::anyhow!("retry is not implemented in the MVP slice")),
        "apps.launch" => Err(anyhow::anyhow!(
            "launch requires trusted desktop entry discovery; not enabled in fake MVP mode"
        )),
        "operations.enqueue" => enqueue_operation(request.params, state, writer).await,
        _ => Err(anyhow::anyhow!("unknown method {}", request.method)),
    }
}

async fn enqueue_operation(
    params: Value,
    state: AppState,
    writer: Arc<Mutex<OwnedWriteHalf>>,
) -> Result<Value> {
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

    let operation_for_task = operation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        run_operation(state, operation_for_task, variant.package_id, writer).await;
    });

    Ok(serde_json::to_value(operation)?)
}

async fn run_operation(
    state: AppState,
    mut operation: Operation,
    package_id: String,
    writer: Arc<Mutex<OwnedWriteHalf>>,
) {
    let _active = state.active.clone().acquire_owned().await.ok();
    let _source_permit = match operation.source {
        SourceKind::System => state.system_mutation.clone().acquire_owned().await.ok(),
        SourceKind::Flathub => state.flatpak_mutation.clone().acquire_owned().await.ok(),
        SourceKind::Github => state.github_mutation.clone().acquire_owned().await.ok(),
        SourceKind::Appimage => state.github_mutation.clone().acquire_owned().await.ok(),
    };

    let mut rx = state.uni.run(
        operation.action,
        operation.app_name.clone(),
        package_id,
        operation.source,
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
        let _ = write_notification(&writer, "event.operationProgress", &operation).await;
    }
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
    items.extend(read_uni_installed().await);
    items.extend(read_flatpak_installed().await);
    items.extend(read_known_dpkg_installed().await);
    dedupe_installed(items)
}

async fn read_uni_installed() -> Vec<InstalledItem> {
    let output = match Command::new(uni_binary())
        .args(["installed", "--json"])
        .output()
        .await
    {
        Ok(output) => output,
        Err(_) => return Vec::new(),
    };

    if output.status.success() {
        if let Ok(value) = serde_json::from_slice::<Value>(&output.stdout) {
            if let Some(items) = value.get("items").and_then(Value::as_array) {
                return items
                    .iter()
                    .filter_map(|item| {
                        let name = item.get("name").and_then(Value::as_str)?;
                        let id = item.get("id").and_then(Value::as_str).unwrap_or(name);
                        let source = item.get("source").and_then(Value::as_str).unwrap_or("uni");
                        Some(InstalledItem {
                            id: format!("uni:{source}:{id}"),
                            name: name.to_string(),
                            source: source.to_string(),
                            version: item
                                .get("version")
                                .and_then(Value::as_str)
                                .map(ToString::to_string),
                            detail: id.to_string(),
                            managed_by_uni: item
                                .get("managedByUni")
                                .and_then(Value::as_bool)
                                .unwrap_or(true),
                        })
                    })
                    .collect();
            }
        }
    }

    let fallback = match Command::new(uni_binary()).arg("list").output().await {
        Ok(output) => output,
        Err(_) => return Vec::new(),
    };

    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&fallback.stdout),
        String::from_utf8_lossy(&fallback.stderr)
    );

    text.lines()
        .filter_map(parse_uni_list_line)
        .collect::<Vec<InstalledItem>>()
}

fn parse_uni_list_line(line: &str) -> Option<InstalledItem> {
    let clean = strip_ansi(line).trim().to_string();
    if clean.is_empty()
        || clean.contains("Packages installed via uni")
        || clean.contains("────")
        || !clean.contains('[')
        || !clean.contains(']')
    {
        return None;
    }

    let (name_part, rest) = clean.split_once('[')?;
    let (source_part, detail_part) = rest.split_once(']')?;
    let name = name_part.trim().to_string();
    let source = source_part.trim().to_string();
    let detail = detail_part.trim().to_string();

    if name.is_empty() || source.is_empty() {
        return None;
    }

    Some(InstalledItem {
        id: format!("uni:{source}:{name}"),
        name,
        source,
        version: None,
        detail,
        managed_by_uni: true,
    })
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

async fn read_known_dpkg_installed() -> Vec<InstalledItem> {
    let known_packages = ["gimp", "obs-studio", "heroic", "zed"];
    let output = match Command::new("dpkg-query")
        .arg("-W")
        .arg("-f=${Package}\t${Version}\t${binary:Summary}\n")
        .args(known_packages)
        .output()
        .await
    {
        Ok(output) => output,
        Err(_) => return Vec::new(),
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let columns = line.split('\t').collect::<Vec<_>>();
            let package = columns.first()?.trim();
            if package.is_empty() {
                return None;
            }
            let version = columns
                .get(1)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let summary = columns.get(2).map(|s| s.trim()).unwrap_or("system package");
            Some(InstalledItem {
                id: format!("dpkg:{package}"),
                name: package.to_string(),
                source: "system".to_string(),
                version,
                detail: summary.to_string(),
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

fn strip_ansi(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            output.push(ch);
        }
    }

    output
}

fn uni_binary() -> String {
    std::env::var("THALLIUM_STORE_UNI").unwrap_or_else(|_| "uni".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_uni_list_line_with_ansi() {
        let line = "\u{1b}[32m  heroic                           [dpkg      ]  heroic\u{1b}[0m";
        let item = parse_uni_list_line(line).expect("line should parse");

        assert_eq!(item.name, "heroic");
        assert_eq!(item.source, "dpkg");
        assert_eq!(item.detail, "heroic");
        assert!(item.managed_by_uni);
    }

    #[test]
    fn ignores_uni_header_lines() {
        assert!(parse_uni_list_line("Packages installed via uni").is_none());
        assert!(parse_uni_list_line("────────────────────────────────────────").is_none());
    }
}
