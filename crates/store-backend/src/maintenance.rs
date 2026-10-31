//! Maintenance RPCs: repair, held apps, repositories, app export/import and
//! the flatpak permission preview.
//!
//! Parsing is kept in pure functions; everything that spawns a tool is a thin
//! wrapper around them. Anything that needs root goes through
//! `store_uni::privileged`, the same helper the install backends use.

use crate::{AppState, InstalledItem};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

const REPAIR_TIMEOUT: Duration = Duration::from_secs(600);
const QUERY_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_IMPORT_BYTES: u64 = 5 * 1024 * 1024;

/// Run a command to completion, returning success and its combined output
/// lines. A command that cannot start or times out is a failure with the
/// reason as its only line, never an RPC error: the UI shows these lines.
async fn run_lines(mut cmd: Command, timeout: Duration) -> (bool, Vec<String>) {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(output)) => {
            let mut lines: Vec<String> = Vec::new();
            for stream in [&output.stdout, &output.stderr] {
                lines.extend(
                    String::from_utf8_lossy(stream)
                        .lines()
                        .map(str::trim_end)
                        .filter(|l| !l.is_empty())
                        .map(str::to_string),
                );
            }
            (output.status.success(), lines)
        }
        Ok(Err(err)) => (false, vec![format!("could not start: {err}")]),
        Err(_) => (false, vec!["timed out".to_string()]),
    }
}

fn simulated() -> Value {
    json!({ "ok": true, "lines": ["Simulated: fake mode makes no changes."] })
}

pub async fn repair_flatpak(fake: bool) -> Result<Value> {
    if fake {
        return Ok(simulated());
    }
    let mut cmd = Command::new("flatpak");
    cmd.args(["repair", "--user"]);
    let (ok, lines) = run_lines(cmd, REPAIR_TIMEOUT).await;
    Ok(json!({ "ok": ok, "lines": lines }))
}

/// Finish an interrupted dpkg run, then let apt repair broken dependencies.
pub async fn fix_apt(fake: bool) -> Result<Value> {
    if fake {
        return Ok(simulated());
    }
    let mut all = Vec::new();
    let steps: [&[&str]; 2] = [
        &["dpkg", "--configure", "-a"],
        &["apt-get", "-y", "install", "-f"],
    ];
    for step in steps {
        all.push(format!("$ {}", step.join(" ")));
        let (ok, lines) =
            run_lines(store_uni::privileged(step[0], &step[1..]), REPAIR_TIMEOUT).await;
        all.extend(lines);
        if !ok {
            return Ok(json!({ "ok": false, "lines": all }));
        }
    }
    Ok(json!({ "ok": true, "lines": all }))
}

pub async fn refresh_lists(fake: bool) -> Result<Value> {
    let ok = if fake {
        true
    } else {
        let result = store_uni::apt_cache::refresh(Duration::from_secs(120)).await;
        result.outcome == store_uni::apt_cache::AptRefreshOutcome::Ok
    };
    Ok(json!({ "ok": ok, "ageSeconds": store_uni::apt_cache::lists_age_seconds() }))
}

pub fn lists_age() -> Value {
    json!({ "ageSeconds": store_uni::apt_cache::lists_age_seconds() })
}

// ---- holds -------------------------------------------------------------

/// Package names and flatpak mask patterns are passed as argv, never through
/// a shell, but a leading dash would still be read as an option.
fn valid_package_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && id.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-' | '_' | ':' | '*' | '/')
        })
}

fn parse_showhold(out: &str) -> Vec<String> {
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// `dpkg-query -W -f='${Package}\t${Version}\n'` rows into (name, version).
fn parse_versions(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|line| {
            let (name, version) = line.split_once('\t')?;
            Some((name.trim().to_string(), version.trim().to_string()))
        })
        .collect()
}

async fn query(program: &str, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null()).kill_on_drop(true);
    let output = tokio::time::timeout(QUERY_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

pub async fn holds_list() -> Result<Value> {
    let mut items = Vec::new();
    if let Some(out) = query("apt-mark", &["showhold"]).await {
        let names = parse_showhold(&out);
        let mut args = vec!["-W", "-f=${Package}\t${Version}\n"];
        args.extend(names.iter().map(String::as_str));
        let versions = if names.is_empty() {
            Vec::new()
        } else {
            parse_versions(&query("dpkg-query", &args).await.unwrap_or_default())
        };
        for name in names {
            let version = versions
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.clone());
            items.push(
                json!({ "name": name, "source": "apt", "package_id": name, "version": version }),
            );
        }
    }
    if let Some(out) = query("flatpak", &["mask", "--user"]).await {
        for pattern in parse_showhold(&out) {
            items.push(json!({ "name": pattern, "source": "flatpak", "package_id": pattern, "version": null }));
        }
    }
    Ok(json!({ "items": items }))
}

pub async fn holds_set(params: &Value, fake: bool) -> Result<Value> {
    let source = crate::str_param(params, "source")?;
    let package_id = params
        .get("packageId")
        .and_then(Value::as_str)
        .context("missing packageId")?;
    let held = params
        .get("held")
        .and_then(Value::as_bool)
        .context("missing held")?;
    if !valid_package_id(package_id) {
        bail!("invalid package id");
    }
    if fake {
        return Ok(json!({ "ok": true }));
    }
    let (ok, lines) = match source {
        "apt" | "system" => {
            let verb = if held { "hold" } else { "unhold" };
            run_lines(
                store_uni::privileged("apt-mark", &[verb, package_id]),
                QUERY_TIMEOUT,
            )
            .await
        }
        "flatpak" | "flathub" => {
            let mut cmd = Command::new("flatpak");
            cmd.args(["mask", "--user"]);
            if !held {
                cmd.arg("--remove");
            }
            cmd.arg(package_id);
            run_lines(cmd, QUERY_TIMEOUT).await
        }
        other => bail!("holds are not supported for {other}"),
    };
    if !ok {
        bail!("{}", lines.join("; "));
    }
    Ok(json!({ "ok": true }))
}

// ---- remotes -----------------------------------------------------------

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct AptRemote {
    pub uri: String,
    pub suites: Vec<String>,
    pub components: Vec<String>,
    pub file: String,
    pub signed: bool,
    /// Added through the store, so the store may remove it again.
    pub custom: bool,
}

fn words(value: &str) -> Vec<String> {
    value.split_whitespace().map(str::to_string).collect()
}

/// Parse a deb822 `.sources` file. One entry per URI; stanzas with
/// `Enabled: no` or only `deb-src` types are skipped.
pub fn parse_deb822(text: &str, file: &str) -> Vec<AptRemote> {
    let mut remotes = Vec::new();
    let mut stanza: Vec<(String, String)> = Vec::new();
    let flush = |stanza: &mut Vec<(String, String)>, remotes: &mut Vec<AptRemote>| {
        let get = |key: &str| {
            stanza
                .iter()
                .rev()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v.as_str())
        };
        let enabled = get("Enabled").is_none_or(|v| !v.eq_ignore_ascii_case("no"));
        let has_deb = get("Types").is_none_or(|v| words(v).iter().any(|t| t == "deb"));
        if enabled && has_deb {
            let signed = get("Signed-By").is_some_and(|v| !v.trim().is_empty());
            for uri in words(get("URIs").unwrap_or("")) {
                remotes.push(AptRemote {
                    uri,
                    suites: words(get("Suites").unwrap_or("")),
                    components: words(get("Components").unwrap_or("")),
                    file: file.to_string(),
                    signed,
                    custom: custom_apt_name(file).is_some(),
                });
            }
        }
        stanza.clear();
    };
    for line in text.lines() {
        if line.trim().is_empty() {
            flush(&mut stanza, &mut remotes);
        } else if line.starts_with('#') {
        } else if line.starts_with([' ', '\t']) {
            if let Some((_, value)) = stanza.last_mut() {
                value.push(' ');
                value.push_str(line.trim());
            }
        } else if let Some((key, value)) = line.split_once(':') {
            stanza.push((key.trim().to_string(), value.trim().to_string()));
        }
    }
    flush(&mut stanza, &mut remotes);
    remotes
}

/// Parse a one-line-style `sources.list` / `.list` file.
pub fn parse_one_line(text: &str, file: &str) -> Vec<AptRemote> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let rest = line
                .strip_prefix("deb ")
                .or_else(|| line.strip_prefix("deb\t"))?;
            let mut rest = rest.trim_start();
            let mut signed = false;
            if let Some(stripped) = rest.strip_prefix('[') {
                let (opts, after) = stripped.split_once(']')?;
                signed = opts.contains("signed-by");
                rest = after.trim_start();
            }
            let mut parts = rest.split_whitespace();
            let uri = parts.next()?.to_string();
            let suite = parts.next()?.to_string();
            Some(AptRemote {
                uri,
                suites: vec![suite],
                components: parts.map(str::to_string).collect(),
                file: file.to_string(),
                signed,
                custom: custom_apt_name(file).is_some(),
            })
        })
        .collect()
}

fn apt_remotes() -> Vec<AptRemote> {
    let mut files = vec![PathBuf::from("/etc/apt/sources.list")];
    if let Ok(dir) = std::fs::read_dir("/etc/apt/sources.list.d") {
        let mut listed: Vec<PathBuf> = dir.flatten().map(|e| e.path()).collect();
        listed.sort();
        files.extend(listed);
    }
    let mut remotes = Vec::new();
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let name = path.to_string_lossy();
        match path.extension().and_then(|e| e.to_str()) {
            Some("sources") => remotes.extend(parse_deb822(&text, &name)),
            Some("list") => remotes.extend(parse_one_line(&text, &name)),
            _ => {}
        }
    }
    remotes
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct FlatpakRemote {
    pub name: String,
    pub url: String,
    pub scope: String,
}

/// Rows of `flatpak remotes --columns=name,url,options`.
pub fn parse_flatpak_remotes(out: &str) -> Vec<FlatpakRemote> {
    out.lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split('\t').collect();
            let name = cols.first()?.trim();
            if name.is_empty() {
                return None;
            }
            let options = cols.get(2).copied().unwrap_or("");
            let scope = if options.split(',').any(|o| o.trim() == "user") {
                "user"
            } else {
                "system"
            };
            Some(FlatpakRemote {
                name: name.to_string(),
                url: cols.get(1).copied().unwrap_or("").trim().to_string(),
                scope: scope.to_string(),
            })
        })
        .collect()
}

pub async fn remotes_list() -> Result<Value> {
    let flatpak = query("flatpak", &["remotes", "--columns=name,url,options"])
        .await
        .map(|out| parse_flatpak_remotes(&out))
        .unwrap_or_default();
    Ok(json!({ "apt": apt_remotes(), "flatpak": flatpak }))
}

fn valid_remote_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

fn valid_remote_url(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://"))
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

pub async fn remotes_add_flatpak(params: &Value, fake: bool) -> Result<Value> {
    let name = crate::str_param(params, "name")?;
    let url = crate::str_param(params, "url")?;
    if !valid_remote_name(name) {
        bail!("invalid remote name");
    }
    if !valid_remote_url(url) {
        bail!("remote URL must be http(s)");
    }
    if fake {
        return Ok(json!({ "ok": true }));
    }
    let mut cmd = Command::new("flatpak");
    cmd.args(["remote-add", "--user", "--if-not-exists", name, url]);
    let (ok, lines) = run_lines(cmd, QUERY_TIMEOUT * 3).await;
    if !ok {
        bail!("{}", lines.join("; "));
    }
    Ok(json!({ "ok": true }))
}

pub async fn remotes_remove_flatpak(params: &Value, fake: bool) -> Result<Value> {
    let name = crate::str_param(params, "name")?;
    let scope = crate::str_param(params, "scope")?;
    if !valid_remote_name(name) {
        bail!("invalid remote name");
    }
    let flag = match scope {
        "user" => "--user",
        "system" => "--system",
        _ => bail!("scope must be user or system"),
    };
    if fake {
        return Ok(json!({ "ok": true }));
    }
    let mut cmd = Command::new("flatpak");
    cmd.args(["remote-delete", flag, "--force", name]);
    let (ok, lines) = run_lines(cmd, QUERY_TIMEOUT * 3).await;
    if !ok {
        bail!("{}", lines.join("; "));
    }
    Ok(json!({ "ok": true }))
}

// ---- custom APT repositories -------------------------------------------

const APT_SOURCES_DIR: &str = "/etc/apt/sources.list.d";
const APT_KEYRINGS_DIR: &str = "/etc/apt/keyrings";
const CUSTOM_APT_PREFIX: &str = "thallium-custom-";
const MAX_KEY_BYTES: usize = 256 * 1024;

/// The name part of a sources file the store wrote, `None` for anything else.
pub fn custom_apt_name(file: &str) -> Option<&str> {
    let name = file
        .strip_prefix(APT_SOURCES_DIR)?
        .strip_prefix('/')?
        .strip_prefix(CUSTOM_APT_PREFIX)?
        .strip_suffix(".sources")?;
    valid_remote_name(name).then_some(name)
}

/// One deb822 stanza. Every value is validated before it gets here, so no
/// field can smuggle in a newline or a second stanza.
pub fn custom_apt_stanza(uri: &str, suites: &[&str], components: &[&str], key: Option<&str>) -> String {
    let mut text = format!("Types: deb\nURIs: {uri}\nSuites: {}\n", suites.join(" "));
    if !components.is_empty() {
        text.push_str(&format!("Components: {}\n", components.join(" ")));
    }
    if let Some(key) = key {
        text.push_str(&format!("Signed-By: {key}\n"));
    }
    text
}

fn apt_words(value: &str) -> Result<Vec<&str>> {
    let list: Vec<&str> = value.split_whitespace().collect();
    if list.iter().any(|w| {
        !w.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '/' | '+'))
    }) {
        bail!("suites and components are plain words like `stable` or `main contrib`");
    }
    Ok(list)
}

async fn fetch_key(url: &str) -> Result<Vec<u8>> {
    if !url.starts_with("https://") {
        bail!("signing key URL must be https");
    }
    let bytes = reqwest::Client::new()
        .get(url)
        .timeout(QUERY_TIMEOUT)
        .send()
        .await
        .context("fetching signing key")?
        .error_for_status()
        .context("fetching signing key")?
        .bytes()
        .await?;
    if bytes.len() > MAX_KEY_BYTES {
        bail!("signing key is larger than 256 KiB");
    }
    Ok(bytes.to_vec())
}

/// Write a deb822 source (and its key) under the `thallium-custom-` prefix,
/// then refresh only that source. A source apt cannot read is rolled back so
/// a typo never leaves every later `apt update` failing.
pub async fn remotes_add_apt(params: &Value, fake: bool) -> Result<Value> {
    let name = crate::str_param(params, "name")?;
    let uri = crate::str_param(params, "uri")?.trim();
    let suites = apt_words(crate::str_param(params, "suites")?)?;
    let components = apt_words(params.get("components").and_then(Value::as_str).unwrap_or(""))?;
    let key_url = params.get("keyUrl").and_then(Value::as_str).unwrap_or("").trim();
    if !valid_remote_name(name) {
        bail!("invalid repository name");
    }
    if !valid_remote_url(uri) {
        bail!("repository URL must be http(s)");
    }
    if suites.is_empty() {
        bail!("give at least one suite, like `stable`");
    }
    // A suite ending in `/` is a flat repository, which takes no components.
    if suites.iter().any(|s| s.ends_with('/')) != components.is_empty() {
        bail!("a flat suite (ending in /) takes no components; any other suite needs them");
    }
    let key = if key_url.is_empty() { None } else { Some(fetch_key(key_url).await?) };
    if fake {
        return Ok(json!({ "ok": true }));
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let staging = std::env::temp_dir().join(format!("thallium-apt-{}-{nanos}", std::process::id()));
    std::fs::create_dir(&staging)?;
    let result = add_apt_staged(&staging, name, uri, &suites, &components, key.as_deref()).await;
    let _ = std::fs::remove_dir_all(&staging);
    result
}

async fn add_apt_staged(
    staging: &Path,
    name: &str,
    uri: &str,
    suites: &[&str],
    components: &[&str],
    key: Option<&[u8]>,
) -> Result<Value> {
    let armored = key.is_some_and(|k| k.starts_with(b"-----BEGIN PGP"));
    let key_dest = key
        .map(|_| format!("{APT_KEYRINGS_DIR}/{CUSTOM_APT_PREFIX}{name}.{}", if armored { "asc" } else { "gpg" }));
    let list_dest = format!("{APT_SOURCES_DIR}/{CUSTOM_APT_PREFIX}{name}.sources");
    if Path::new(&list_dest).exists() {
        bail!("a repository named {name} already exists");
    }
    let list_tmp = staging.join("list");
    std::fs::write(&list_tmp, custom_apt_stanza(uri, suites, components, key_dest.as_deref()))?;
    let key_tmp = staging.join("key");
    if let Some(key) = key {
        std::fs::write(&key_tmp, key)?;
    }

    // One root prompt for the whole thing. Paths ride in as positional
    // arguments, never spliced into the script.
    const SCRIPT: &str = r#"set -eu
list_tmp=$1 list_dest=$2 key_tmp=$3 key_dest=$4
if [ -n "$key_dest" ]; then install -D -m 0644 "$key_tmp" "$key_dest"; fi
install -m 0644 "$list_tmp" "$list_dest"
out=$(apt-get update -o Dir::Etc::sourcelist="$list_dest" -o Dir::Etc::sourceparts=- \
      -o APT::Get::List-Cleanup=0 2>&1) || true
printf '%s\n' "$out"
if printf '%s\n' "$out" | grep -q -E '^(E:|W: (GPG|The repository|Failed))'; then
  rm -f "$list_dest"
  if [ -n "$key_dest" ]; then rm -f "$key_dest"; fi
  exit 3
fi
"#;
    let key_dest_arg = key_dest.clone().unwrap_or_default();
    let args = [
        "-c",
        SCRIPT,
        "sh",
        list_tmp.to_str().context("temp path")?,
        &list_dest,
        key_tmp.to_str().context("temp path")?,
        &key_dest_arg,
    ];
    let (ok, lines) = run_lines(store_uni::privileged("sh", &args), REPAIR_TIMEOUT).await;
    if !ok {
        let reason = lines
            .iter()
            .rev()
            .find(|l| l.starts_with("E:") || l.starts_with("W:"))
            .cloned()
            .unwrap_or_else(|| lines.join("; "));
        bail!("repository not added: {reason}");
    }
    Ok(json!({ "ok": true }))
}

/// Remove a repository the store added, with its key. Anything else in
/// sources.list.d belongs to the system or another tool and is refused.
pub async fn remotes_remove_apt(params: &Value, fake: bool) -> Result<Value> {
    let file = crate::str_param(params, "file")?;
    let Some(name) = custom_apt_name(file) else {
        bail!("only repositories added in the store can be removed here");
    };
    if fake {
        return Ok(json!({ "ok": true }));
    }
    let asc = format!("{APT_KEYRINGS_DIR}/{CUSTOM_APT_PREFIX}{name}.asc");
    let gpg = format!("{APT_KEYRINGS_DIR}/{CUSTOM_APT_PREFIX}{name}.gpg");
    let (ok, lines) = run_lines(
        store_uni::privileged("rm", &["-f", file, &asc, &gpg]),
        QUERY_TIMEOUT * 3,
    )
    .await;
    if !ok {
        bail!("{}", lines.join("; "));
    }
    Ok(json!({ "ok": true }))
}

// ---- local .deb ----------------------------------------------------------

/// Control fields of a `.deb` on disk. Read with `dpkg-deb -f`, which only
/// parses the archive: no maintainer script runs until the person installs.
pub async fn deb_inspect(params: &Value) -> Result<Value> {
    let path = Path::new(crate::str_param(params, "path")?);
    if !path.is_absolute() || path.extension().and_then(|e| e.to_str()) != Some("deb") {
        bail!("not a .deb file");
    }
    let size = std::fs::metadata(path).context("reading the .deb")?.len();
    let fields = [
        "Package", "Version", "Architecture", "Maintainer", "Description",
        "Depends", "Installed-Size", "Homepage",
    ];
    let mut cmd = Command::new("dpkg-deb");
    cmd.arg("-f").arg(path).args(fields);
    let (ok, lines) = run_lines(cmd, QUERY_TIMEOUT).await;
    if !ok {
        bail!("not a readable .deb: {}", lines.join("; "));
    }
    let control = parse_control(&lines.join("\n"));
    let field = |k: &str| control.get(k).cloned().unwrap_or_default();
    let package = field("Package");
    if package.is_empty() {
        bail!("not a readable .deb: no Package field");
    }
    let installed = query("dpkg-query", &["-W", "-f=${db:Status-Abbrev}\t${Version}", &package])
        .await
        .and_then(|out| {
            let (status, version) = out.trim().split_once('\t')?;
            status.starts_with("ii").then(|| version.to_string())
        });
    let description = field("Description");
    let (summary, _) = description.split_once('\n').unwrap_or((&description, ""));
    Ok(json!({ "deb": {
        "kind": "deb",
        "path": path.to_string_lossy(),
        "file_name": path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(),
        "name": package,
        "version": field("Version"),
        "architecture": field("Architecture"),
        "maintainer": field("Maintainer"),
        "summary": summary.trim(),
        "depends": field("Depends"),
        "homepage": field("Homepage"),
        "size": size,
        "installed_size_kb": field("Installed-Size").parse::<u64>().ok(),
        "installed_version": installed,
        "already_installed": installed.is_some(),
    }}))
}

/// `Key: value` control text, continuation lines folded into the value.
pub fn parse_control(text: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut last: Option<String> = None;
    for line in text.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(key) = &last {
                let value: &mut String = map.entry(key.clone()).or_default();
                value.push('\n');
                value.push_str(line.trim());
            }
        } else if let Some((key, value)) = line.split_once(':') {
            map.insert(key.trim().to_string(), value.trim().to_string());
            last = Some(key.trim().to_string());
        }
    }
    map
}

const FLATHUB_BETA_URL: &str = "https://flathub.org/beta-repo/flathub-beta.flatpakrepo";

/// Add or remove the user-scope flathub-beta remote.
pub async fn set_flathub_beta(enabled: bool) {
    let mut cmd = Command::new("flatpak");
    if enabled {
        cmd.args([
            "remote-add",
            "--user",
            "--if-not-exists",
            "flathub-beta",
            FLATHUB_BETA_URL,
        ]);
    } else {
        cmd.args(["remote-delete", "--user", "--force", "flathub-beta"]);
    }
    let (ok, lines) = run_lines(cmd, QUERY_TIMEOUT * 3).await;
    if !ok {
        tracing::warn!("flathub-beta remote change failed: {}", lines.join("; "));
    }
}

// ---- flatpak permissions ----------------------------------------------

fn permission_label(group: &str, token: &str) -> Option<String> {
    let label = match (group, token) {
        ("shared", "network") => "Network access",
        ("shared", "ipc") => "Shared memory with other apps",
        ("sockets", "x11") | ("sockets", "fallback-x11") => "Display (X11)",
        ("sockets", "wayland") => "Display (Wayland)",
        ("sockets", "pulseaudio") => "Audio playback and recording",
        ("sockets", "session-bus") => "Full session bus access",
        ("sockets", "system-bus") => "Full system bus access",
        ("sockets", "ssh-auth") => "SSH agent",
        ("sockets", "pcsc") => "Smart cards",
        ("sockets", "cups") => "Printing",
        ("devices", "all") => "All devices",
        ("devices", "dri") => "GPU acceleration",
        ("devices", "kvm") => "Virtualization",
        ("devices", "shm") => "Shared memory devices",
        ("filesystems", "host") => "All files",
        ("filesystems", "host-os") => "System files (read-only)",
        ("filesystems", "host-etc") => "System configuration",
        ("filesystems", "home") => "Home folder",
        _ => return None,
    };
    Some(label.to_string())
}

/// Human-readable permission lines from the `[Context]` and bus-policy
/// sections of `flatpak remote-info --show-metadata`. Negated entries
/// (`!network`) mean "not granted" and are skipped; unknown tokens are shown
/// as written so nothing the app asks for is hidden.
pub fn parse_permissions(metadata: &str) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    let mut section = String::new();
    for line in metadata.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match (section.as_str(), key) {
            ("Context", "shared" | "sockets" | "devices") => {
                for token in value
                    .split(';')
                    .filter(|t| !t.is_empty() && !t.starts_with('!'))
                {
                    items.push(
                        permission_label(key, token).unwrap_or_else(|| format!("{key}: {token}")),
                    );
                }
            }
            ("Context", "filesystems") => {
                for token in value
                    .split(';')
                    .filter(|t| !t.is_empty() && !t.starts_with('!'))
                {
                    let (path, mode) = match token.rsplit_once(':') {
                        Some((p, m @ ("ro" | "rw" | "create"))) => (p, m),
                        _ => (token, "rw"),
                    };
                    let base = permission_label("filesystems", path)
                        .unwrap_or_else(|| format!("Files: {path}"));
                    items.push(if mode == "ro" {
                        format!("{base} (read-only)")
                    } else {
                        base
                    });
                }
            }
            ("Session Bus Policy" | "System Bus Policy", name)
                if value == "talk" || value == "own" =>
            {
                let bus = if section.starts_with("Session") {
                    "session"
                } else {
                    "system"
                };
                items.push(format!(
                    "{} on the {bus} bus: {name}",
                    if value == "own" { "Owns" } else { "Talks to" }
                ));
            }
            _ => {}
        }
    }
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| seen.insert(item.clone()));
    items
}

pub async fn flatpak_permissions(params: &Value, fake: bool) -> Result<Value> {
    let app_ref = crate::str_param(params, "ref")?;
    if !valid_package_id(app_ref) {
        bail!("invalid ref");
    }
    if fake {
        return Ok(json!({ "items": [] }));
    }
    let remote = params
        .get("remote")
        .and_then(Value::as_str)
        .unwrap_or("flathub");
    if !valid_remote_name(remote) {
        bail!("invalid remote");
    }
    let out = query(
        "flatpak",
        &["remote-info", "--show-metadata", remote, app_ref],
    )
    .await
    .unwrap_or_default();
    Ok(json!({ "items": parse_permissions(&out) }))
}

// ---- export / import ---------------------------------------------------

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ExportedApp {
    pub app_id: String,
    pub name: String,
    pub source: String,
    pub package_id: String,
}

/// Installed items that can be reinstalled from their source. AppImages are
/// left out: their registry id is a local file path.
pub fn export_items(items: &[InstalledItem]) -> Vec<ExportedApp> {
    items
        .iter()
        .filter(|item| item.source != "appimage")
        .map(|item| ExportedApp {
            app_id: item.id.clone(),
            name: item.name.clone(),
            source: item.source.clone(),
            package_id: item
                .id
                .strip_prefix("flatpak:")
                .unwrap_or(&item.detail)
                .to_string(),
        })
        .collect()
}

fn downloads_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| ".".into())).join("Downloads")
}

pub async fn apps_export() -> Result<Value> {
    let exported = export_items(&crate::installed_list().await);
    let dir = downloads_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!(
        "thallium-apps-{}.json",
        chrono::Local::now().format("%Y-%m-%d")
    ));
    std::fs::write(&path, serde_json::to_string_pretty(&exported)?)?;
    Ok(json!({ "path": path.to_string_lossy(), "count": exported.len() }))
}

pub fn parse_import(text: &str) -> Result<Vec<ExportedApp>> {
    let items: Vec<ExportedApp> = serde_json::from_str(text).context("not a Thallium app list")?;
    Ok(items
        .into_iter()
        .filter(|i| !i.name.is_empty() && valid_package_id(&i.package_id))
        .collect())
}

fn read_import(path: &str) -> Result<Vec<ExportedApp>> {
    let path = Path::new(path);
    if std::fs::metadata(path)?.len() > MAX_IMPORT_BYTES {
        bail!("file is too large");
    }
    parse_import(&std::fs::read_to_string(path)?)
}

/// The store source an exported entry installs from, if reinstalling is
/// supported.
pub fn import_source(source: &str) -> Option<&'static str> {
    match source {
        "flatpak" | "flathub" => Some("flathub"),
        "apt" | "system" => Some("system"),
        "github" => Some("github"),
        _ => None,
    }
}

pub async fn import_preview(params: &Value) -> Result<Value> {
    let items = read_import(crate::str_param(params, "path")?)?;
    let installed = crate::installed_package_ids().await;
    let items: Vec<Value> = items
        .iter()
        .map(|item| {
            json!({
                "app_id": item.app_id, "name": item.name, "source": item.source,
                "package_id": item.package_id, "installed": installed.contains(&item.package_id),
            })
        })
        .collect();
    Ok(json!({ "items": items }))
}

pub async fn import_apply(params: &Value, state: &AppState) -> Result<Value> {
    let items = read_import(crate::str_param(params, "path")?)?;
    let installed = crate::installed_package_ids().await;
    let mut queued = 0;
    for item in items {
        let Some(source) = import_source(&item.source) else {
            continue;
        };
        if installed.contains(&item.package_id) {
            continue;
        }
        let request = json!({
            "app_id": item.app_id, "variant_id": "import", "action": "install",
            "app_name": item.name, "package_id": item.package_id, "source": source,
        });
        match crate::enqueue_operation(request, state.clone()).await {
            Ok(_) => queued += 1,
            Err(err) => tracing::warn!("import: could not queue {}: {err:#}", item.name),
        }
    }
    Ok(json!({ "queued": queued }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_apt_names_only_match_store_files() {
        assert_eq!(custom_apt_name("/etc/apt/sources.list.d/thallium-custom-vscode.sources"), Some("vscode"));
        assert_eq!(custom_apt_name("/etc/apt/sources.list.d/debian.sources"), None);
        assert_eq!(custom_apt_name("/etc/apt/sources.list.d/thallium-custom-a b.sources"), None);
        assert_eq!(custom_apt_name("/etc/apt/sources.list.d/thallium-custom-x.list"), None);
    }

    #[test]
    fn custom_apt_stanza_is_deb822() {
        let text = custom_apt_stanza("https://ex.org/apt", &["stable"], &["main"], Some("/etc/apt/keyrings/k.asc"));
        assert_eq!(text, "Types: deb\nURIs: https://ex.org/apt\nSuites: stable\nComponents: main\nSigned-By: /etc/apt/keyrings/k.asc\n");
        let flat = custom_apt_stanza("https://ex.org/apt", &["./"], &[], None);
        assert_eq!(flat, "Types: deb\nURIs: https://ex.org/apt\nSuites: ./\n");
        assert!(apt_words("main\nTypes: deb-src").is_err());
        assert!(apt_words("main;rm").is_err());
    }

    #[test]
    fn control_folds_continuations() {
        let map = parse_control("Package: zed\nDescription: Editor\n fast one\nVersion: 1.0");
        assert_eq!(map["Package"], "zed");
        assert_eq!(map["Description"], "Editor\nfast one");
        assert_eq!(map["Version"], "1.0");
    }

    #[test]
    fn deb822_one_entry_per_uri_and_skips_disabled() {
        let text = "\
Types: deb
URIs: https://deb.debian.org/debian https://mirror.example/debian
Suites: trixie trixie-updates
Components: main contrib
Signed-By: /usr/share/keyrings/debian.gpg

Types: deb
URIs: https://off.example/
Suites: x
Enabled: no

Types: deb-src
URIs: https://src.example/
Suites: x
";
        let remotes = parse_deb822(text, "/etc/apt/sources.list.d/debian.sources");
        assert_eq!(remotes.len(), 2);
        assert_eq!(remotes[0].uri, "https://deb.debian.org/debian");
        assert_eq!(remotes[0].suites, ["trixie", "trixie-updates"]);
        assert_eq!(remotes[0].components, ["main", "contrib"]);
        assert!(remotes[1].signed);
    }

    #[test]
    fn deb822_continuation_lines_join() {
        let text = "Types: deb\nURIs: https://a.example/\nSuites: s\nComponents: main\n contrib\n";
        let remotes = parse_deb822(text, "f");
        assert_eq!(remotes[0].components, ["main", "contrib"]);
        assert!(!remotes[0].signed);
    }

    #[test]
    fn one_line_format_with_options() {
        let text = "# comment\ndeb [arch=amd64 signed-by=/k.gpg] https://a.example/apt stable main extra\ndeb-src https://a.example/apt stable main\ndeb http://b.example/ bookworm main\n";
        let remotes = parse_one_line(text, "f");
        assert_eq!(remotes.len(), 2);
        assert!(remotes[0].signed);
        assert_eq!(remotes[0].components, ["main", "extra"]);
        assert_eq!(remotes[1].suites, ["bookworm"]);
        assert!(!remotes[1].signed);
    }

    #[test]
    fn flatpak_remote_rows() {
        let out = "flathub\thttps://dl.flathub.org/repo/\tsystem\nflathub\thttps://dl.flathub.org/repo/\tuser\nbeta\thttps://x/\tuser,disabled\n";
        let remotes = parse_flatpak_remotes(out);
        assert_eq!(remotes.len(), 3);
        assert_eq!(remotes[0].scope, "system");
        assert_eq!(remotes[1].scope, "user");
        assert_eq!(remotes[2].scope, "user");
    }

    #[test]
    fn permissions_are_readable_and_skip_negations() {
        let meta = "[Application]\nname=org.x.App\n\n[Context]\nshared=network;ipc;\nsockets=wayland;!x11;pulseaudio;\ndevices=dri;\nfilesystems=xdg-download:ro;home;\n\n[Session Bus Policy]\norg.freedesktop.Notifications=talk\norg.x.Other=see\n";
        let items = parse_permissions(meta);
        assert!(items.contains(&"Network access".to_string()));
        assert!(items.contains(&"Display (Wayland)".to_string()));
        assert!(!items.iter().any(|i| i.contains("X11")));
        assert!(items.contains(&"Files: xdg-download (read-only)".to_string()));
        assert!(items.contains(&"Home folder".to_string()));
        assert!(items
            .contains(&"Talks to on the session bus: org.freedesktop.Notifications".to_string()));
        assert!(!items.iter().any(|i| i.contains("org.x.Other")));
    }

    #[test]
    fn hold_parsing_and_validation() {
        assert_eq!(parse_showhold("a\n\n b \n"), ["a", "b"]);
        assert_eq!(
            parse_versions("a\t1.0\nbad\n"),
            [("a".to_string(), "1.0".to_string())]
        );
        assert!(valid_package_id("libfoo2+dev"));
        assert!(valid_package_id("org.gnome.*"));
        assert!(!valid_package_id("-rf"));
        assert!(!valid_package_id("a b"));
        assert!(!valid_package_id(""));
    }

    fn installed(id: &str, name: &str, source: &str, detail: &str) -> InstalledItem {
        InstalledItem {
            id: id.into(),
            name: name.into(),
            source: source.into(),
            version: None,
            detail: detail.into(),
            managed_by_uni: false,
        }
    }

    #[test]
    fn export_maps_package_ids_and_drops_appimages() {
        let items = vec![
            installed("flatpak:org.x.App", "X", "flatpak", "flathub"),
            installed("uni:apt:htop", "htop", "apt", "htop"),
            installed(
                "uni:appimage:/h/a.AppImage",
                "A",
                "appimage",
                "/h/a.AppImage",
            ),
        ];
        let out = export_items(&items);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].package_id, "org.x.App");
        assert_eq!(out[1].package_id, "htop");
    }

    #[test]
    fn import_round_trip_and_rejects_bad_ids() {
        let items = vec![ExportedApp {
            app_id: "flatpak:org.x.App".into(),
            name: "X".into(),
            source: "flatpak".into(),
            package_id: "org.x.App".into(),
        }];
        let json = serde_json::to_string(&items).unwrap();
        assert_eq!(parse_import(&json).unwrap(), items);
        let bad = r#"[{"app_id":"a","name":"n","source":"apt","package_id":"--purge"}]"#;
        assert!(parse_import(bad).unwrap().is_empty());
        assert!(parse_import("{}").is_err());
        assert_eq!(import_source("flatpak"), Some("flathub"));
        assert_eq!(import_source("appimage"), None);
    }

    #[test]
    fn remote_validation() {
        assert!(valid_remote_name("flathub-beta"));
        assert!(!valid_remote_name("a b"));
        assert!(valid_remote_url("https://x.example/repo.flatpakrepo"));
        assert!(!valid_remote_url("file:///etc"));
        assert!(!valid_remote_url("https://x y"));
    }
}
