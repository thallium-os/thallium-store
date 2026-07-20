use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;
use store_core::{
    rank_variants, AppVariant, CanonicalApp, ProviderStatus, SearchParams, SearchResponse,
    SourceKind, TrustLevel,
};
use std::time::Duration;
use tokio::process::Command;

#[derive(Clone)]
pub struct CatalogManager {
    bundled: Vec<CanonicalApp>,
}

impl CatalogManager {
    pub fn new() -> Self {
        Self {
            bundled: bundled_catalog(),
        }
    }

    pub async fn search(&self, params: SearchParams) -> SearchResponse {
        let limit = params.limit.unwrap_or(30);
        let query = params.query.trim().to_ascii_lowercase();
        let wanted = params.sources.unwrap_or_else(|| {
            vec![
                SourceKind::System,
                SourceKind::Flathub,
                SourceKind::Github,
                SourceKind::Appimage,
            ]
        });

        let mut results = Vec::new();
        let mut providers = Vec::new();

        for app in self.bundled.iter().filter(|app| matches_query(app, &query)) {
            if app.variants.iter().any(|v| wanted.contains(&v.source)) {
                let mut app = app.clone();
                app.variants.retain(|v| wanted.contains(&v.source));
                app.recommended_variant_id = rank_variants(&mut app.variants);
                results.push(app);
            }
        }

        // Fan out the three subprocess-backed providers concurrently; each is
        // wrapped in a timeout so one slow source cannot stall the whole search.
        let want_flathub = wanted.contains(&SourceKind::Flathub);
        let want_system = wanted.contains(&SourceKind::System);
        let want_github = wanted.contains(&SourceKind::Github);

        let (flathub, system, github) = tokio::join!(
            async {
                if want_flathub {
                    flathub_provider(&query).await
                } else {
                    (Vec::new(), Vec::new())
                }
            },
            async {
                if want_system {
                    system_provider(&query).await
                } else {
                    (Vec::new(), Vec::new())
                }
            },
            async {
                if want_github {
                    github_provider(&query).await
                } else {
                    (Vec::new(), Vec::new())
                }
            },
        );

        for (mut apps, mut provs) in [flathub, system, github] {
            results.append(&mut apps);
            providers.append(&mut provs);
        }

        if wanted.contains(&SourceKind::Appimage) {
            let mut apps = appimage_manifest_search(&query);
            push_provider(
                &mut providers,
                ProviderStatus {
                    source: SourceKind::Appimage,
                    state: "ready".to_string(),
                    message: Some("Curated AppImage manifest loaded".to_string()),
                },
            );
            results.append(&mut apps);
        }

        dedupe(&mut results);
        sort_results(&mut results, &query);
        results.truncate(limit);

        SearchResponse {
            query: params.query,
            results,
            providers,
        }
    }

    pub fn app_details(&self, id: &str) -> Option<CanonicalApp> {
        self.bundled.iter().find(|app| app.id == id).cloned()
    }

    pub async fn enrich_details(&self, mut app: CanonicalApp) -> CanonicalApp {
        let variants = std::mem::take(&mut app.variants);
        let mut enriched_variants = Vec::with_capacity(variants.len());
        for mut variant in variants {
            let info = match variant.source {
                SourceKind::Github => github_info(&variant.package_id).await,
                SourceKind::Flathub => flathub_info(&variant.package_id).await,
                SourceKind::System => apt_info(&variant.package_id).await,
                SourceKind::Appimage => None,
            };
            if let Some(info) = info {
                apply_info(&mut app, &mut variant, info);
            }
            enriched_variants.push(variant);
        }
        app.variants = enriched_variants;
        app
    }
}

impl Default for CatalogManager {
    fn default() -> Self {
        Self::new()
    }
}

fn matches_query(app: &CanonicalApp, query: &str) -> bool {
    query.is_empty()
        || app.id.contains(query)
        || app.name.to_ascii_lowercase().contains(query)
        || app.summary.to_ascii_lowercase().contains(query)
        || app
            .tags
            .iter()
            .any(|tag| tag.to_ascii_lowercase().contains(query))
        || app
            .variants
            .iter()
            .any(|v| v.package_id.to_ascii_lowercase().contains(query))
}

fn push_provider(providers: &mut Vec<ProviderStatus>, provider: ProviderStatus) {
    if let Some(existing) = providers
        .iter_mut()
        .find(|existing| existing.source == provider.source)
    {
        if existing.state != "ready" {
            *existing = provider;
        }
    } else {
        providers.push(provider);
    }
}

async fn flatpak_search(query: &str) -> Result<Vec<CanonicalApp>> {
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let remotes = Command::new("flatpak")
        .args(["remotes", "--columns=name"])
        .output()
        .await?;
    if !remotes.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&remotes.stderr).trim());
    }
    let has_flathub = String::from_utf8_lossy(&remotes.stdout)
        .lines()
        .any(|line| line.trim() == "flathub");
    if !has_flathub {
        anyhow::bail!("Flathub remote is not configured. Add it with: flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo");
    }

    let output = Command::new("flatpak")
        .args(["search", "--columns=application,name,description", query])
        .output()
        .await?;

    if !output.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 2 || parts[0].trim().is_empty() {
                return None;
            }
            let app_id = parts[0].trim();
            let name = parts.get(1).copied().unwrap_or(app_id).trim();
            let summary = parts.get(2).copied().unwrap_or("").trim();
            Some(single_variant_app(
                &format!("flathub:{app_id}"),
                name,
                summary,
                SourceKind::Flathub,
                app_id,
                TrustLevel::Sandboxed,
                true,
            ))
        })
        .collect())
}

async fn apt_search(query: &str) -> Result<Vec<CanonicalApp>> {
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let output = Command::new("apt-cache")
        .args(["search", query])
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .take(20)
        .filter_map(|line| {
            let (pkg, summary) = line.split_once(" - ")?;
            if pkg.contains("-dev") || pkg.starts_with("lib") {
                return None;
            }
            Some(single_variant_app(
                &format!("system:{pkg}"),
                pkg,
                summary,
                SourceKind::System,
                pkg,
                TrustLevel::SystemAccess,
                true,
            ))
        })
        .collect())
}

const SEARCH_TIMEOUT: Duration = Duration::from_secs(8);

async fn flathub_provider(query: &str) -> (Vec<CanonicalApp>, Vec<ProviderStatus>) {
    match tokio::time::timeout(SEARCH_TIMEOUT, flatpak_search(query)).await {
        Ok(Ok(apps)) => (
            apps,
            vec![ProviderStatus {
                source: SourceKind::Flathub,
                state: "ready".to_string(),
                message: Some("Flatpak read-only search completed".to_string()),
            }],
        ),
        Ok(Err(err)) => {
            let message = err.to_string();
            let state = if message.contains("Flathub remote is not configured") {
                "misconfigured"
            } else {
                "failed"
            };
            (
                Vec::new(),
                vec![ProviderStatus {
                    source: SourceKind::Flathub,
                    state: state.to_string(),
                    message: Some(message),
                }],
            )
        }
        Err(_) => (
            Vec::new(),
            vec![ProviderStatus {
                source: SourceKind::Flathub,
                state: "failed".to_string(),
                message: Some("Flatpak search timed out".to_string()),
            }],
        ),
    }
}

async fn system_provider(query: &str) -> (Vec<CanonicalApp>, Vec<ProviderStatus>) {
    match tokio::time::timeout(SEARCH_TIMEOUT, apt_search(query)).await {
        Ok(Ok(apps)) => (
            apps,
            vec![ProviderStatus {
                source: SourceKind::System,
                state: "ready".to_string(),
                message: Some("APT read-only search completed".to_string()),
            }],
        ),
        Ok(Err(err)) => (
            Vec::new(),
            vec![ProviderStatus {
                source: SourceKind::System,
                state: "failed".to_string(),
                message: Some(err.to_string()),
            }],
        ),
        Err(_) => (
            Vec::new(),
            vec![ProviderStatus {
                source: SourceKind::System,
                state: "failed".to_string(),
                message: Some("APT search timed out".to_string()),
            }],
        ),
    }
}

async fn github_provider(query: &str) -> (Vec<CanonicalApp>, Vec<ProviderStatus>) {
    match tokio::time::timeout(SEARCH_TIMEOUT, github_search(query)).await {
        Ok(Ok(apps)) => (
            apps,
            vec![ProviderStatus {
                source: SourceKind::Github,
                state: "ready".to_string(),
                message: Some("GitHub repository search completed".to_string()),
            }],
        ),
        Ok(Err(err)) => (
            Vec::new(),
            vec![ProviderStatus {
                source: SourceKind::Github,
                state: "failed".to_string(),
                message: Some(err.to_string()),
            }],
        ),
        Err(_) => (
            Vec::new(),
            vec![ProviderStatus {
                source: SourceKind::Github,
                state: "failed".to_string(),
                message: Some("GitHub search timed out".to_string()),
            }],
        ),
    }
}

/// Build a reqwest client with the standard GitHub UA, and attach a bearer
/// token from `GITHUB_TOKEN` when present (raises the unauthenticated rate
/// limit; absence just means lower-throughput anonymous access).
fn github_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent("thallium-store")
        .build()?)
}

fn github_request(client: &reqwest::Client, url: String) -> reqwest::RequestBuilder {
    let mut request = client.get(url);
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        request = request.bearer_auth(token);
    }
    request
}

async fn github_search(query: &str) -> Result<Vec<CanonicalApp>> {
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let client = github_client()?;
    let request = github_request(
        &client,
        "https://api.github.com/search/repositories".to_string(),
    )
    .query(&[("q", query), ("per_page", "15")]);
    let value: Value = request.send().await?.error_for_status()?.json().await?;

    let empty = Vec::new();
    let items = value.get("items").and_then(Value::as_array).unwrap_or(&empty);
    Ok(items.iter().filter_map(github_repo_to_app).collect())
}

fn github_repo_to_app(item: &Value) -> Option<CanonicalApp> {
    let package_id = item.get("full_name").and_then(Value::as_str)?;
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(package_id);
    let summary = item
        .get("description")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("No summary provided");

    let mut app = single_variant_app(
        &format!("github:{package_id}"),
        name,
        summary,
        SourceKind::Github,
        package_id,
        TrustLevel::Unverified,
        false,
    );
    app.homepage = item
        .get("homepage")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            item.get("html_url")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
    app.repository = Some(package_id.to_string());
    app.rating = item
        .get("stargazers_count")
        .and_then(Value::as_f64)
        .map(|stars| stars as f32);

    let mut tags = default_tags(SourceKind::Github, name, summary);
    if let Some(topics) = item.get("topics").and_then(Value::as_array) {
        for topic in topics.iter().filter_map(Value::as_str) {
            if !tags.iter().any(|existing| existing == topic) {
                tags.push(topic.to_string());
            }
        }
    }
    app.tags = tags;

    if let Some(variant) = app.variants.first_mut() {
        variant.repository = app.repository.clone();
    }
    Some(app)
}

#[derive(Debug, Deserialize)]
struct AppImageManifestEntry {
    id: String,
    name: String,
    summary: String,
    repository: String,
    asset_pattern: String,
    verified: bool,
    homepage: Option<String>,
    license: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

fn appimage_manifest_search(query: &str) -> Vec<CanonicalApp> {
    let entries: Vec<AppImageManifestEntry> =
        serde_json::from_str(include_str!("../../../data/appimages.json")).unwrap_or_default();
    entries
        .into_iter()
        .filter(|entry| {
            query.is_empty()
                || entry.id.to_ascii_lowercase().contains(query)
                || entry.name.to_ascii_lowercase().contains(query)
                || entry.summary.to_ascii_lowercase().contains(query)
                || entry.repository.to_ascii_lowercase().contains(query)
                || entry
                    .tags
                    .iter()
                    .any(|tag| tag.to_ascii_lowercase().contains(query))
        })
        .map(|entry| {
            let package_id = format!("github:{}#{}", entry.repository, entry.asset_pattern);
            let mut app = single_variant_app(
                &format!("appimage:{}", entry.id),
                &entry.name,
                &entry.summary,
                SourceKind::Appimage,
                &package_id,
                TrustLevel::Unverified,
                entry.verified,
            );
            app.repository = Some(entry.repository.clone());
            app.homepage = entry.homepage;
            app.license = entry.license;
            app.tags = entry.tags;
            if let Some(variant) = app.variants.first_mut() {
                variant.repository = Some(entry.repository);
                variant.install_location = Some("$HOME/.local/share/uni/appimages".to_string());
            }
            app
        })
        .collect()
}

/// Native, best-effort enrichment fields fed into `apply_info`. Any field
/// left `None` is simply skipped by the caller — a failed lookup never
/// aborts enrichment for the rest of an app's variants.
#[derive(Default)]
struct PackageInfo {
    description: Option<String>,
    homepage: Option<String>,
    license: Option<String>,
    version: Option<String>,
    installed_size: Option<u64>,
}

/// `GET /repos/{owner}/{repo}` for description/homepage/license, plus a
/// best-effort `GET /repos/{owner}/{repo}/releases/latest` for the version
/// tag (skipped on 404 / no releases — repos without releases still enrich).
async fn github_info(package_id: &str) -> Option<PackageInfo> {
    let (owner, repo) = package_id.split_once('/')?;
    let client = github_client().ok()?;

    let repo_json: Value = github_request(
        &client,
        format!("https://api.github.com/repos/{owner}/{repo}"),
    )
    .send()
    .await
    .ok()?
    .error_for_status()
    .ok()?
    .json()
    .await
    .ok()?;

    let mut info = PackageInfo {
        description: repo_json
            .get("description")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string),
        homepage: repo_json
            .get("homepage")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .or_else(|| {
                repo_json
                    .get("html_url")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            }),
        license: repo_json
            .get("license")
            .and_then(|license| license.get("spdx_id"))
            .and_then(Value::as_str)
            .filter(|spdx| *spdx != "NOASSERTION")
            .map(ToString::to_string),
        ..Default::default()
    };

    if let Ok(response) = github_request(
        &client,
        format!("https://api.github.com/repos/{owner}/{repo}/releases/latest"),
    )
    .send()
    .await
    {
        if let Ok(response) = response.error_for_status() {
            if let Ok(release_json) = response.json::<Value>().await {
                info.version = release_json
                    .get("tag_name")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
            }
        }
    }

    Some(info)
}

/// `flatpak info --user <app_id>`, falling back to `remote-info --user
/// flathub` when the app isn't installed locally. Best-effort text parse.
async fn flathub_info(package_id: &str) -> Option<PackageInfo> {
    if let Some(info) = run_flatpak_info(&["info", "--user", package_id]).await {
        return Some(info);
    }
    run_flatpak_info(&["remote-info", "--user", "flathub", package_id]).await
}

async fn run_flatpak_info(args: &[&str]) -> Option<PackageInfo> {
    let output = Command::new("flatpak").args(args).output().await.ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_flatpak_info(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_flatpak_info(text: &str) -> PackageInfo {
    let mut info = PackageInfo::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key.trim().to_ascii_lowercase().as_str() {
            "version" => info.version = Some(value.to_string()),
            "license" => info.license = Some(value.to_string()),
            "homepage" => info.homepage = Some(value.to_string()),
            "installed" => info.installed_size = parse_size(value),
            _ => {}
        }
    }
    info
}

/// `apt-cache show <pkg>` — read-only, never touches the dpkg lock. Only the
/// first stanza (the newest available version) is parsed.
async fn apt_info(package_id: &str) -> Option<PackageInfo> {
    let output = Command::new("apt-cache")
        .args(["show", package_id])
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(parse_apt_cache_show(&String::from_utf8_lossy(&output.stdout)))
}

fn parse_apt_cache_show(text: &str) -> PackageInfo {
    let mut info = PackageInfo::default();
    for line in text.lines() {
        if line.trim().is_empty() {
            break; // end of the first (newest) stanza
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            continue; // continuation of a multi-line field (e.g. description body)
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key.trim().to_ascii_lowercase().as_str() {
            "version" if info.version.is_none() => info.version = Some(value.to_string()),
            "homepage" if info.homepage.is_none() => info.homepage = Some(value.to_string()),
            "description" | "description-en" if info.description.is_none() => {
                info.description = Some(value.to_string())
            }
            "installed-size" if info.installed_size.is_none() => {
                info.installed_size = value.parse::<u64>().ok().map(|kb| kb * 1024)
            }
            _ => {}
        }
    }
    info
}

fn apply_info(app: &mut CanonicalApp, variant: &mut AppVariant, info: PackageInfo) {
    if app.description.is_none() {
        app.description = info.description;
    }
    if app.homepage.is_none() {
        app.homepage = info.homepage;
    }
    if app.license.is_none() {
        app.license = info.license;
    }
    if variant.version.is_none() {
        variant.version = info.version;
    }
    if variant.installed_size.is_none() {
        variant.installed_size = info.installed_size;
    }
    if app.tags.is_empty() {
        app.tags = default_tags(variant.source, &app.name, &app.summary);
    }
}

fn dedupe(apps: &mut Vec<CanonicalApp>) {
    let mut merged: Vec<CanonicalApp> = Vec::new();
    for mut app in apps.drain(..) {
        if let Some(existing) = merged.iter_mut().find(|candidate| {
            candidate.name.eq_ignore_ascii_case(&app.name)
                || candidate.variants.iter().any(|left| {
                    app.variants
                        .iter()
                        .any(|right| left.package_id == right.package_id)
                })
        }) {
            existing.variants.append(&mut app.variants);
            existing.merge_confidence = existing.merge_confidence.max(app.merge_confidence);
            existing
                .merge_evidence
                .push("name/package match".to_string());
            dedupe_variants(&mut existing.variants);
            existing.recommended_variant_id = rank_variants(&mut existing.variants);
        } else {
            dedupe_variants(&mut app.variants);
            app.recommended_variant_id = rank_variants(&mut app.variants);
            merged.push(app);
        }
    }
    *apps = merged;
}

fn dedupe_variants(variants: &mut Vec<AppVariant>) {
    let mut unique = Vec::new();
    for variant in variants.drain(..) {
        if !unique
            .iter()
            .any(|existing: &AppVariant| existing.id == variant.id)
        {
            unique.push(variant);
        }
    }
    *variants = unique;
}

fn sort_results(apps: &mut [CanonicalApp], query: &str) {
    apps.sort_by_key(|app| result_score(app, query));
}

fn result_score(app: &CanonicalApp, query: &str) -> u16 {
    let name = app.name.to_ascii_lowercase();
    let id = app.id.to_ascii_lowercase();
    let summary = app.summary.to_ascii_lowercase();
    let package_match = app
        .variants
        .iter()
        .any(|variant| variant.package_id.to_ascii_lowercase() == query);
    let package_starts = app
        .variants
        .iter()
        .any(|variant| variant.package_id.to_ascii_lowercase().starts_with(query));
    let package_contains = app
        .variants
        .iter()
        .any(|variant| variant.package_id.to_ascii_lowercase().contains(query));

    let text_score = if query.is_empty() {
        20
    } else if name == query || id == query || package_match {
        0
    } else if name.starts_with(query) || package_starts {
        5
    } else if name.contains(query) || package_contains {
        20
    } else if app.tags.iter().any(|tag| tag.to_ascii_lowercase() == query) {
        18
    } else if summary.contains(query) {
        40
    } else {
        60
    };

    let source_score = app
        .variants
        .first()
        .map(|variant| match variant.source {
            SourceKind::Flathub => 0,
            SourceKind::System => 4,
            SourceKind::Appimage => 8,
            SourceKind::Github => 12,
        })
        .unwrap_or(20);

    text_score + source_score
}

fn source_arg(source: SourceKind) -> &'static str {
    match source {
        SourceKind::System => "apt",
        SourceKind::Flathub => "flatpak",
        SourceKind::Github => "github",
        SourceKind::Appimage => "appimage",
    }
}

fn source_key(source: SourceKind) -> &'static str {
    match source {
        SourceKind::System => "system",
        SourceKind::Flathub => "flathub",
        SourceKind::Github => "github",
        SourceKind::Appimage => "appimage",
    }
}

fn install_location(source: SourceKind) -> &'static str {
    match source {
        SourceKind::System => "System package database (/usr, managed by APT/dpkg)",
        SourceKind::Flathub => "User Flatpak installation (Flathub remote)",
        SourceKind::Github => "UNI-selected release asset location",
        SourceKind::Appimage => "$HOME/.local/share/uni/appimages",
    }
}

fn default_tags(source: SourceKind, name: &str, summary: &str) -> Vec<String> {
    let mut tags = vec![source_key(source).to_string()];
    let text = format!(
        "{} {}",
        name.to_ascii_lowercase(),
        summary.to_ascii_lowercase()
    );
    for (needle, tag) in [
        ("browser", "browser"),
        ("video", "video"),
        ("media", "media"),
        ("audio", "audio"),
        ("image", "graphics"),
        ("game", "games"),
        ("editor", "developer"),
        ("terminal", "terminal"),
        ("music", "music"),
    ] {
        if text.contains(needle) && !tags.iter().any(|existing| existing == tag) {
            tags.push(tag.to_string());
        }
    }
    tags
}

fn parse_size(value: &str) -> Option<u64> {
    let normalized = value.trim().replace('\u{a0}', " ");
    let mut parts = normalized.split_whitespace();
    let number = parts.next()?.replace(',', ".").parse::<f64>().ok()?;
    let unit = parts.next().unwrap_or("KB").to_ascii_lowercase();
    let multiplier = if unit.starts_with("gb") || unit.starts_with("gib") {
        1024.0 * 1024.0 * 1024.0
    } else if unit.starts_with("mb") || unit.starts_with("mib") {
        1024.0 * 1024.0
    } else if unit.starts_with("kb") || unit.starts_with("kib") {
        1024.0
    } else {
        1.0
    };
    Some((number * multiplier) as u64)
}

fn single_variant_app(
    id: &str,
    name: &str,
    summary: &str,
    source: SourceKind,
    package_id: &str,
    trust: TrustLevel,
    verified: bool,
) -> CanonicalApp {
    let source_arg = source_arg(source);
    let variant = AppVariant {
        id: format!("{source_arg}:{package_id}"),
        source,
        package_id: package_id.to_string(),
        version: None,
        download_size: None,
        installed_size: None,
        architecture: None,
        repository: None,
        install_location: Some(install_location(source).to_string()),
        trust,
        verified,
        command_preview: format!("uni install {package_id} --source {source_arg}"),
        ranking_reasons: Vec::new(),
    };

    CanonicalApp {
        id: id.to_string(),
        name: name.to_string(),
        summary: if summary.is_empty() {
            "No summary provided".to_string()
        } else {
            summary.to_string()
        },
        description: None,
        developer: None,
        homepage: None,
        license: None,
        repository: None,
        rating: None,
        tags: default_tags(source, name, summary),
        icon: None,
        screenshots: Vec::new(),
        last_updated: None,
        installed: false,
        launch_desktop_id: None,
        variants: vec![variant],
        recommended_variant_id: None,
        merge_confidence: 0.7,
        merge_evidence: vec!["provider result".to_string()],
    }
}

fn bundled_catalog() -> Vec<CanonicalApp> {
    let mut gimp = single_variant_app(
        "org.gimp.GIMP",
        "GIMP",
        "Create and edit images",
        SourceKind::Flathub,
        "org.gimp.GIMP",
        TrustLevel::Sandboxed,
        true,
    );
    gimp.description = Some(
        "GNU Image Manipulation Program for photo retouching, composition, and image authoring."
            .to_string(),
    );
    gimp.developer = Some("The GIMP Team".to_string());
    gimp.homepage = Some("https://www.gimp.org".to_string());
    gimp.license = Some("GPL-3.0-or-later".to_string());
    gimp.screenshots = vec![
        "https://dl.flathub.org/media/org/gimp/GIMP/stable/1280x720/org.gimp.GIMP.png".to_string(),
    ];
    gimp.variants.push(AppVariant {
        id: "apt:gimp".to_string(),
        source: SourceKind::System,
        package_id: "gimp".to_string(),
        version: None,
        download_size: None,
        installed_size: None,
        architecture: None,
        repository: None,
        install_location: Some(install_location(SourceKind::System).to_string()),
        trust: TrustLevel::SystemAccess,
        verified: true,
        command_preview: "uni install gimp --source apt".to_string(),
        ranking_reasons: Vec::new(),
    });

    let mut zed = single_variant_app(
        "dev.zed.Zed",
        "Zed",
        "Fast collaborative code editor",
        SourceKind::Github,
        "zed-industries/zed",
        TrustLevel::Verified,
        true,
    );
    zed.description = Some("A high-performance editor distributed by the Zed project through curated release metadata.".to_string());
    zed.developer = Some("Zed Industries".to_string());
    zed.homepage = Some("https://zed.dev".to_string());
    zed.license = Some("GPL-3.0-or-later".to_string());
    zed.repository = Some("zed-industries/zed".to_string());
    zed.tags = vec![
        "developer".to_string(),
        "editor".to_string(),
        "github".to_string(),
    ];
    if let Some(variant) = zed.variants.first_mut() {
        variant.repository = zed.repository.clone();
    }

    let mut obs = single_variant_app(
        "com.obsproject.Studio",
        "OBS Studio",
        "Record and stream video",
        SourceKind::Flathub,
        "com.obsproject.Studio",
        TrustLevel::Sandboxed,
        true,
    );
    obs.variants.push(AppVariant {
        id: "apt:obs-studio".to_string(),
        source: SourceKind::System,
        package_id: "obs-studio".to_string(),
        version: None,
        download_size: None,
        installed_size: None,
        architecture: None,
        repository: None,
        install_location: Some(install_location(SourceKind::System).to_string()),
        trust: TrustLevel::SystemAccess,
        verified: true,
        command_preview: "uni install obs-studio --source apt".to_string(),
        ranking_reasons: Vec::new(),
    });

    vec![gimp, zed, obs]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_repo_to_app_maps_search_result() {
        let item = serde_json::json!({
            "full_name": "zed-industries/zed",
            "name": "zed",
            "description": "A high-performance, multiplayer code editor",
            "html_url": "https://github.com/zed-industries/zed",
            "homepage": "https://zed.dev",
            "stargazers_count": 42000,
            "topics": ["editor", "rust"],
        });

        let app = github_repo_to_app(&item).expect("mapping should succeed");
        assert_eq!(app.id, "github:zed-industries/zed");
        assert_eq!(app.name, "zed");
        assert_eq!(app.summary, "A high-performance, multiplayer code editor");
        assert_eq!(app.homepage.as_deref(), Some("https://zed.dev"));
        assert_eq!(app.repository.as_deref(), Some("zed-industries/zed"));
        assert_eq!(app.rating, Some(42000.0));
        assert!(app.tags.contains(&"editor".to_string()));
        assert!(app.tags.contains(&"rust".to_string()));

        let variant = app.variants.first().expect("single variant");
        assert_eq!(variant.source, SourceKind::Github);
        assert_eq!(variant.package_id, "zed-industries/zed");
        assert_eq!(variant.repository.as_deref(), Some("zed-industries/zed"));
    }

    #[test]
    fn github_repo_to_app_falls_back_to_html_url_and_default_summary() {
        let item = serde_json::json!({
            "full_name": "octocat/hello-world",
        });

        let app = github_repo_to_app(&item).expect("mapping should succeed");
        assert_eq!(app.name, "octocat/hello-world");
        assert_eq!(app.summary, "No summary provided");
        assert_eq!(app.homepage, None);
        assert_eq!(app.rating, None);
    }

    #[test]
    fn github_repo_to_app_rejects_missing_full_name() {
        let item = serde_json::json!({ "name": "hello-world" });
        assert!(github_repo_to_app(&item).is_none());
    }

    #[test]
    fn parse_flatpak_info_extracts_known_fields() {
        let text = "\
          ID: org.gimp.GIMP
        Branch: stable
       Version: 2.10.36
       License: GPL-3.0-or-later
     Installed: 350.2 MB
";
        let info = parse_flatpak_info(text);
        assert_eq!(info.version.as_deref(), Some("2.10.36"));
        assert_eq!(info.license.as_deref(), Some("GPL-3.0-or-later"));
        assert_eq!(info.installed_size, Some((350.2 * 1024.0 * 1024.0) as u64));
        assert_eq!(info.homepage, None);
    }

    #[test]
    fn parse_apt_cache_show_parses_first_stanza_only() {
        let text = "\
Package: gimp
Version: 2.10.34-1
Installed-Size: 12345
Homepage: https://www.gimp.org
Description-en: GNU Image Manipulation Program
 A longer wrapped description line.

Package: gimp
Version: 2.10.30-1
Installed-Size: 9999
";
        let info = parse_apt_cache_show(text);
        assert_eq!(info.version.as_deref(), Some("2.10.34-1"));
        assert_eq!(info.homepage.as_deref(), Some("https://www.gimp.org"));
        assert_eq!(
            info.description.as_deref(),
            Some("GNU Image Manipulation Program")
        );
        assert_eq!(info.installed_size, Some(12345 * 1024));
    }

    #[test]
    fn apply_info_only_fills_unset_fields() {
        let mut app = single_variant_app(
            "test:app",
            "App",
            "summary",
            SourceKind::System,
            "app",
            TrustLevel::SystemAccess,
            true,
        );
        app.license = Some("Existing".to_string());
        let mut variant = app.variants.remove(0);

        apply_info(
            &mut app,
            &mut variant,
            PackageInfo {
                description: Some("New description".to_string()),
                homepage: Some("https://example.com".to_string()),
                license: Some("Should be ignored".to_string()),
                version: Some("1.0.0".to_string()),
                installed_size: Some(1024),
            },
        );

        assert_eq!(app.description.as_deref(), Some("New description"));
        assert_eq!(app.homepage.as_deref(), Some("https://example.com"));
        assert_eq!(app.license.as_deref(), Some("Existing"));
        assert_eq!(variant.version.as_deref(), Some("1.0.0"));
        assert_eq!(variant.installed_size, Some(1024));
    }
}
