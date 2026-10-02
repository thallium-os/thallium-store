use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    System,
    Flathub,
    Github,
    Appimage,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustLevel {
    Sandboxed,
    SystemAccess,
    Verified,
    Unverified,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AppVariant {
    pub id: String,
    pub source: SourceKind,
    pub package_id: String,
    pub version: Option<String>,
    pub download_size: Option<u64>,
    pub installed_size: Option<u64>,
    pub architecture: Option<String>,
    pub repository: Option<String>,
    pub install_location: Option<String>,
    pub trust: TrustLevel,
    pub verified: bool,
    pub command_preview: String,
    pub ranking_reasons: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CanonicalApp {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub description: Option<String>,
    pub developer: Option<String>,
    pub homepage: Option<String>,
    pub license: Option<String>,
    pub repository: Option<String>,
    pub rating: Option<f32>,
    /// GitHub repository that is itself a fork. Search ranks forks below the
    /// upstream they copy: a fork with a handful of stars is almost never what
    /// someone typing the project's name is looking for.
    #[serde(default)]
    pub fork: bool,
    #[serde(default)]
    pub tags: Vec<String>,
    pub icon: Option<String>,
    pub screenshots: Vec<String>,
    pub last_updated: Option<DateTime<Utc>>,
    pub installed: bool,
    pub launch_desktop_id: Option<String>,
    pub variants: Vec<AppVariant>,
    pub recommended_variant_id: Option<String>,
    pub merge_confidence: f32,
    pub merge_evidence: Vec<String>,
    /// Source-language breakdown (GitHub linguist), percent-sorted descending.
    /// Empty when the app has no discoverable repository.
    #[serde(default)]
    pub languages: Vec<LanguageStat>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LanguageStat {
    pub name: String,
    pub percent: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProviderStatus {
    pub source: SourceKind,
    pub state: String,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SearchParams {
    pub query: String,
    pub sources: Option<Vec<SourceKind>>,
    pub limit: Option<usize>,
    pub protocol_version: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub results: Vec<CanonicalApp>,
    pub providers: Vec<ProviderStatus>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DiscoverCollection {
    pub title: String,
    pub subtitle: String,
    pub apps: Vec<CanonicalApp>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OperationAction {
    Install,
    Update,
    Reinstall,
    Remove,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Pending,
    Resolving,
    Downloading,
    AwaitingAuthentication,
    Installing,
    Finalizing,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EnqueueOperation {
    pub app_id: String,
    pub variant_id: String,
    pub action: OperationAction,
    pub app_name: Option<String>,
    pub package_id: Option<String>,
    pub source: Option<SourceKind>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Operation {
    pub id: String,
    pub app_id: String,
    pub app_name: String,
    pub variant_id: String,
    pub source: SourceKind,
    pub action: OperationAction,
    /// The package the operation acts on, kept so a failed operation can be
    /// retried from its own record. Optional because rows written before this
    /// field existed deserialize with it absent.
    #[serde(default)]
    pub package_id: Option<String>,
    pub state: OperationState,
    pub percent: u8,
    pub message: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OperationLog {
    pub operation_id: String,
    pub timestamp: DateTime<Utc>,
    pub level: String,
    pub message: String,
}
