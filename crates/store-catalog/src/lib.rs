use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;
use store_core::{
    rank_variants, AppVariant, CanonicalApp, ProviderStatus, SearchParams, SearchResponse,
    SourceKind, TrustLevel,
};
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

        if wanted.contains(&SourceKind::Flathub) {
            match flatpak_search(&query).await {
                Ok(mut apps) => {
                    providers.push(ProviderStatus {
                        source: SourceKind::Flathub,
                        state: "ready".to_string(),
                        message: Some("Flatpak read-only search completed".to_string()),
                    });
                    results.append(&mut apps);
                }
                Err(err) => {
                    let message = err.to_string();
                    let state = if message.contains("Flathub remote is not configured") {
                        "misconfigured"
                    } else {
                        "failed"
                    };
                    providers.push(ProviderStatus {
                        source: SourceKind::Flathub,
                        state: state.to_string(),
                        message: Some(message),
                    });
                }
            }
        }

        if wanted.contains(&SourceKind::System) {
            match apt_search(&query).await {
                Ok(mut apps) => {
                    providers.push(ProviderStatus {
                        source: SourceKind::System,
                        state: "ready".to_string(),
                        message: Some("APT read-only search completed".to_string()),
                    });
                    results.append(&mut apps);
                }
                Err(err) => providers.push(ProviderStatus {
                    source: SourceKind::System,
                    state: "failed".to_string(),
                    message: Some(err.to_string()),
                }),
            }
        }

        if wanted.contains(&SourceKind::Github) {
            match uni_search(&query, &wanted).await {
                Ok((mut apps, uni_providers)) => {
                    for provider in uni_providers
                        .into_iter()
                        .filter(|provider| provider.source == SourceKind::Github)
                    {
                        push_provider(&mut providers, provider);
                    }
                    results.append(&mut apps);
                }
                Err(err) => push_provider(
                    &mut providers,
                    ProviderStatus {
                        source: SourceKind::Github,
                        state: "failed".to_string(),
                        message: Some(err.to_string()),
                    },
                ),
            }
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
            if let Ok(value) = uni_info(&variant.package_id, variant.source).await {
                apply_info(&mut app, &mut variant, value);
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

async fn uni_search(
    query: &str,
    wanted: &[SourceKind],
) -> Result<(Vec<CanonicalApp>, Vec<ProviderStatus>)> {
    if query.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    let output = Command::new(uni_binary())
        .args(["search", query, "--json"])
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }

    let value: Value = serde_json::from_slice(&output.stdout)?;
    let empty = Vec::new();
    let provider_values = value
        .get("providers")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let providers = provider_values
        .iter()
        .filter_map(|provider| {
            let source = source_from_uni(provider.get("source")?.as_str()?)?;
            if !wanted.contains(&source) {
                return None;
            }
            Some(ProviderStatus {
                source,
                state: provider
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("ready")
                    .to_string(),
                message: provider
                    .get("message")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
            })
        })
        .collect::<Vec<_>>();

    let empty = Vec::new();
    let result_values = value
        .get("results")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    let apps = result_values
        .iter()
        .filter_map(|item| app_from_uni_result(item, wanted))
        .collect::<Vec<_>>();

    Ok((apps, providers))
}

fn app_from_uni_result(item: &Value, wanted: &[SourceKind]) -> Option<CanonicalApp> {
    let source = source_from_uni(item.get("source")?.as_str()?)?;
    if !wanted.contains(&source) {
        return None;
    }
    let package_id = item
        .get("packageId")
        .or_else(|| item.get("package_id"))
        .and_then(Value::as_str)
        .or_else(|| item.get("id").and_then(Value::as_str))?;
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(package_id);
    let summary = item
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("No summary provided");
    let trust = match item.get("trust").and_then(Value::as_str).unwrap_or("") {
        "sandboxed" => TrustLevel::Sandboxed,
        "verified" => TrustLevel::Verified,
        "system_access" => TrustLevel::SystemAccess,
        _ if source == SourceKind::System => TrustLevel::SystemAccess,
        _ if source == SourceKind::Flathub => TrustLevel::Sandboxed,
        _ => TrustLevel::Unverified,
    };
    let verified = item
        .get("verified")
        .and_then(Value::as_bool)
        .unwrap_or(source == SourceKind::Flathub || source == SourceKind::System);
    let mut app = single_variant_app(
        &format!("{}:{package_id}", source_key(source)),
        name,
        summary,
        source,
        package_id,
        trust,
        verified,
    );
    app.homepage = item
        .get("homepage")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    app.repository = (source == SourceKind::Github)
        .then(|| package_id.to_string())
        .or_else(|| {
            item.get("repository")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
    app.rating = item
        .get("stars")
        .and_then(Value::as_f64)
        .map(|stars| stars as f32);
    app.tags = default_tags(source, name, summary);
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

async fn uni_info(package_id: &str, source: SourceKind) -> Result<Value> {
    let output = Command::new(uni_binary())
        .args(["info", package_id, "--source", source_arg(source), "--json"])
        .output()
        .await?;
    if !output.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn apply_info(app: &mut CanonicalApp, variant: &mut AppVariant, value: Value) {
    if app.description.is_none() {
        app.description = value
            .get("description")
            .and_then(Value::as_str)
            .or_else(|| value.get("summary").and_then(Value::as_str))
            .map(ToString::to_string);
    }
    if app.homepage.is_none() {
        app.homepage = value
            .get("homepage")
            .and_then(Value::as_str)
            .map(ToString::to_string);
    }
    if app.license.is_none() {
        app.license = value
            .get("license")
            .and_then(Value::as_str)
            .or_else(|| {
                value
                    .get("metadata")
                    .and_then(|metadata| metadata.get("License"))
                    .and_then(Value::as_str)
            })
            .map(ToString::to_string);
    }
    if variant.version.is_none() {
        variant.version = value
            .get("version")
            .and_then(Value::as_str)
            .map(ToString::to_string);
    }
    if variant.installed_size.is_none() {
        variant.installed_size = value
            .get("metadata")
            .and_then(|metadata| {
                metadata
                    .get("Installed")
                    .or_else(|| metadata.get("Installed-Size"))
            })
            .and_then(Value::as_str)
            .and_then(parse_size);
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

fn source_from_uni(source: &str) -> Option<SourceKind> {
    match source {
        "apt" | "dpkg" | "system" => Some(SourceKind::System),
        "flatpak" | "flathub" => Some(SourceKind::Flathub),
        "github" | "gh" => Some(SourceKind::Github),
        "appimage" => Some(SourceKind::Appimage),
        _ => None,
    }
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

fn uni_binary() -> String {
    std::env::var("THALLIUM_STORE_UNI").unwrap_or_else(|_| "uni".to_string())
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
