use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Brand {
    #[default]
    Feishu,
    Lark,
}

impl Brand {
    pub fn as_cli_value(self) -> &'static str {
        match self {
            Self::Feishu => "feishu",
            Self::Lark => "lark",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountHealth {
    Unknown,
    Ready,
    Refreshable,
    ReauthRequired,
    TemporaryFailure,
    CliFailure,
}

impl Default for AccountHealth {
    fn default() -> Self {
        Self::Unknown
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CredentialOrigin {
    #[default]
    Managed,
    ExternalShared,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRecord {
    pub id: Uuid,
    pub app_id: String,
    pub label: String,
    pub brand: Brand,
    /// Sanitized official CLI config. It may contain a keychain reference, never
    /// an App Secret value.
    pub base_config_path: PathBuf,
    /// Live `userScopes` last read from `lark-cli auth scopes --json`.
    #[serde(default)]
    pub available_scopes: BTreeSet<String>,
    /// Stable policy chosen by the user. New CLI recommendations do not alter it.
    #[serde(default)]
    pub policy_scopes: BTreeSet<String>,
    pub scopes_observed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRecord {
    pub id: Uuid,
    pub app_ref: Uuid,
    pub user_open_id: String,
    pub display_name: String,
    /// Optional globally unique human alias for strict selectors / automation.
    #[serde(default)]
    pub alias: Option<String>,
    pub tenant_label: Option<String>,
    pub config_dir: PathBuf,
    #[serde(default)]
    pub credential_origin: CredentialOrigin,
    #[serde(default)]
    pub health: AccountHealth,
    pub effective_scopes: BTreeSet<String>,
    pub last_verified_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default)]
    pub path_takeover_enabled: bool,
    pub recommended_cli_version: String,
    pub scope_batch_max_count: usize,
    pub scope_batch_max_encoded_bytes: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            path_takeover_enabled: false,
            recommended_cli_version: crate::SUPPORTED_CLI_VERSION.to_owned(),
            // Conservative, configurable planning budgets. These are not stated
            // as Feishu platform limits; server errors can trigger replanning.
            scope_batch_max_count: 30,
            scope_batch_max_encoded_bytes: 1800,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub schema_version: u32,
    // `apps` and `accounts` are intentionally NOT `#[serde(default)]`. Every
    // catalog LPC writes always serializes both keys, so a file that is missing
    // one is corrupt or foreign. Failing to deserialize (loud) is safer than
    // silently treating it as an empty list and then persisting that emptiness,
    // which is one of the ways user profiles were lost.
    pub apps: Vec<AppRecord>,
    pub accounts: Vec<AccountRecord>,
    #[serde(default)]
    pub settings: Settings,
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            schema_version: crate::SCHEMA_VERSION,
            apps: Vec::new(),
            accounts: Vec::new(),
            settings: Settings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveState {
    pub schema_version: u32,
    pub active_account_id: Option<Uuid>,
    pub managed_cli_path: Option<PathBuf>,
    pub managed_cli_version: Option<String>,
    pub generation: u64,
    pub updated_at: DateTime<Utc>,
}

impl Default for ActiveState {
    fn default() -> Self {
        Self {
            schema_version: crate::SCHEMA_VERSION,
            active_account_id: None,
            managed_cli_path: None,
            managed_cli_version: None,
            generation: 0,
            updated_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionLeaseRecord {
    pub id: Uuid,
    pub pid: u32,
    pub process_started_at: u64,
    pub account_id: Uuid,
    pub app_id: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountView {
    pub account: AccountRecord,
    pub app: AppRecord,
    pub active: bool,
    pub running_commands: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlPlaneSnapshot {
    pub state: ActiveState,
    pub settings: Settings,
    pub accounts: Vec<AccountView>,
    pub apps: Vec<AppRecord>,
}
