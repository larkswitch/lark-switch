//! Strict LPC account selectors, compact account views, and shim argv parsing.
//!
//! Official `lark-cli --profile` is never interpreted here. Public one-shot
//! identity uses `--account` / `LARKSWITCH_ACCOUNT`; `--lpc-account` /
//! `LPC_ACCOUNT` remain compatibility aliases.

use crate::error::{LpcError, Result};
use crate::model::{
    AccountHealth, AccountRecord, AccountView, ActiveState, AppRecord, Catalog, CredentialOrigin,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use uuid::Uuid;

const MAX_ALIAS_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSelector {
    /// Optional app constraint: exact `appId`, or exact unique app `label`.
    pub app: Option<String>,
    pub identity: IdentitySelector,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentitySelector {
    Id(Uuid),
    Alias(String),
    /// Bare token: full UUID, then exact alias, then exact unique displayName.
    Bare(String),
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccountCandidateRef {
    pub id: Uuid,
    pub alias: Option<String>,
    pub display_name: String,
    pub app_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    pub id: Uuid,
    pub alias: Option<String>,
    pub display_name: String,
    pub app_id: String,
    pub app_label: String,
    pub tenant_label: Option<String>,
    pub health: AccountHealth,
    pub credential_origin: CredentialOrigin,
    pub active: bool,
    pub running_commands: usize,
    pub last_verified_at: Option<DateTime<Utc>>,
    pub scope_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_scopes: Option<BTreeSet<String>>,
}

#[derive(Debug, Clone)]
pub struct SearchFilter {
    pub query: Option<String>,
    pub app: Option<String>,
    pub health: Option<AccountHealth>,
    pub scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LpcArgv {
    pub account_override: Option<String>,
    pub forwarded: Vec<OsString>,
}

pub fn validate_alias(raw: &str) -> Result<String> {
    let alias = raw.trim();
    if alias.is_empty() {
        return Err(LpcError::AccountSelectorInvalid(
            "alias must be non-empty".into(),
        ));
    }
    if alias.len() > MAX_ALIAS_LEN {
        return Err(LpcError::AccountSelectorInvalid(format!(
            "alias exceeds {MAX_ALIAS_LEN} characters"
        )));
    }
    if alias.contains('/') || alias.contains('\\') || alias.contains('\0') {
        return Err(LpcError::AccountSelectorInvalid(
            "alias must not contain '/', '\\\\', or NUL".into(),
        ));
    }
    if alias.contains(char::is_control) {
        return Err(LpcError::AccountSelectorInvalid(
            "alias must not contain control characters".into(),
        ));
    }
    let lower = alias.to_ascii_lowercase();
    if lower.starts_with("id:") || lower.starts_with("alias:") || lower.starts_with("app:") {
        return Err(LpcError::AccountSelectorInvalid(
            "alias must not start with id:, alias:, or app:".into(),
        ));
    }
    Ok(alias.to_owned())
}

/// Parse a strict selector string.
///
/// Supported forms:
/// - `id:<uuid>`
/// - `alias:<alias>`
/// - bare `<uuid|alias|displayName>`
/// - `app:<appIdOrUniqueLabel>/<identity>` where identity is one of the above
pub fn parse_selector(raw: &str) -> Result<ParsedSelector> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(LpcError::AccountSelectorInvalid(
            "selector must be non-empty".into(),
        ));
    }
    if let Some(rest) = raw.strip_prefix("app:") {
        let (app_key, identity_raw) = split_app_scoped(rest)?;
        return Ok(ParsedSelector {
            app: Some(app_key),
            identity: parse_identity(identity_raw)?,
        });
    }
    Ok(ParsedSelector {
        app: None,
        identity: parse_identity(raw)?,
    })
}

fn split_app_scoped(rest: &str) -> Result<(String, &str)> {
    let Some((app_key, identity_raw)) = rest.split_once('/') else {
        return Err(LpcError::AccountSelectorInvalid(
            "app-scoped selector requires app:<appIdOrLabel>/<identity>".into(),
        ));
    };
    if app_key.trim().is_empty() || identity_raw.trim().is_empty() {
        return Err(LpcError::AccountSelectorInvalid(
            "app-scoped selector requires non-empty app key and identity".into(),
        ));
    }
    Ok((app_key.trim().to_owned(), identity_raw.trim()))
}

fn parse_identity(raw: &str) -> Result<IdentitySelector> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(LpcError::AccountSelectorInvalid(
            "identity selector must be non-empty".into(),
        ));
    }
    if let Some(value) = raw.strip_prefix("id:") {
        let value = value.trim();
        let id = Uuid::parse_str(value).map_err(|_| {
            LpcError::AccountSelectorInvalid(format!("id selector is not a valid UUID: {value}"))
        })?;
        return Ok(IdentitySelector::Id(id));
    }
    if let Some(value) = raw.strip_prefix("alias:") {
        let alias = validate_alias(value)?;
        return Ok(IdentitySelector::Alias(alias));
    }
    if raw.contains('/') {
        return Err(LpcError::AccountSelectorInvalid(
            "bare selectors must not contain '/'; use app:<app>/<identity>".into(),
        ));
    }
    Ok(IdentitySelector::Bare(raw.to_owned()))
}

pub fn resolve_account<'a>(
    catalog: &'a Catalog,
    selector: &ParsedSelector,
    extra_app: Option<&str>,
) -> Result<(&'a AccountRecord, &'a AppRecord)> {
    let app_key = selector.app.as_deref().or(extra_app);
    let scoped = filter_by_app(catalog, app_key)?;
    let matches = match &selector.identity {
        IdentitySelector::Id(id) => scoped
            .into_iter()
            .filter(|(account, _)| account.id == *id)
            .collect::<Vec<_>>(),
        IdentitySelector::Alias(alias) => scoped
            .into_iter()
            .filter(|(account, _)| account.alias.as_deref() == Some(alias.as_str()))
            .collect::<Vec<_>>(),
        IdentitySelector::Bare(token) => resolve_bare(&scoped, token),
    };
    finish_unique(selector_display(selector, extra_app), matches)
}

fn selector_display(selector: &ParsedSelector, extra_app: Option<&str>) -> String {
    let mut parts = Vec::new();
    if let Some(app) = selector.app.as_deref().or(extra_app) {
        parts.push(format!("app:{app}"));
    }
    match &selector.identity {
        IdentitySelector::Id(id) => parts.push(format!("id:{id}")),
        IdentitySelector::Alias(alias) => parts.push(format!("alias:{alias}")),
        IdentitySelector::Bare(token) => parts.push(token.clone()),
    }
    parts.join("/")
}

fn filter_by_app<'a>(
    catalog: &'a Catalog,
    app_key: Option<&str>,
) -> Result<Vec<(&'a AccountRecord, &'a AppRecord)>> {
    let Some(app_key) = app_key.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(catalog
            .accounts
            .iter()
            .filter_map(|account| {
                catalog
                    .apps
                    .iter()
                    .find(|app| app.id == account.app_ref)
                    .map(|app| (account, app))
            })
            .collect());
    };

    let apps: Vec<&AppRecord> = catalog
        .apps
        .iter()
        .filter(|app| app.app_id == app_key || app.label == app_key)
        .collect();
    if apps.is_empty() {
        return Err(LpcError::AppNotFound(app_key.to_owned()));
    }
    if apps.len() > 1 {
        // Prefer exact appId when label collisions exist.
        let by_id: Vec<&AppRecord> = apps
            .iter()
            .copied()
            .filter(|app| app.app_id == app_key)
            .collect();
        if by_id.len() == 1 {
            let app = by_id[0];
            return Ok(catalog
                .accounts
                .iter()
                .filter(|account| account.app_ref == app.id)
                .map(|account| (account, app))
                .collect());
        }
        return Err(LpcError::AccountSelectorInvalid(format!(
            "app key '{app_key}' matches multiple apps; use exact appId"
        )));
    }
    let app = apps[0];
    Ok(catalog
        .accounts
        .iter()
        .filter(|account| account.app_ref == app.id)
        .map(|account| (account, app))
        .collect())
}

fn resolve_bare<'a>(
    scoped: &[(&'a AccountRecord, &'a AppRecord)],
    token: &str,
) -> Vec<(&'a AccountRecord, &'a AppRecord)> {
    if let Ok(id) = Uuid::parse_str(token) {
        return scoped
            .iter()
            .copied()
            .filter(|(account, _)| account.id == id)
            .collect();
    }
    let by_alias: Vec<_> = scoped
        .iter()
        .copied()
        .filter(|(account, _)| account.alias.as_deref() == Some(token))
        .collect();
    if !by_alias.is_empty() {
        return by_alias;
    }
    scoped
        .iter()
        .copied()
        .filter(|(account, _)| account.display_name == token)
        .collect()
}

fn finish_unique<'a>(
    selector: String,
    matches: Vec<(&'a AccountRecord, &'a AppRecord)>,
) -> Result<(&'a AccountRecord, &'a AppRecord)> {
    match matches.len() {
        0 => Err(LpcError::AccountNotFound(selector)),
        1 => Ok(matches[0]),
        _ => {
            let candidates = matches
                .iter()
                .map(|(account, app)| AccountCandidateRef {
                    id: account.id,
                    alias: account.alias.clone(),
                    display_name: account.display_name.clone(),
                    app_id: app.app_id.clone(),
                })
                .collect::<Vec<_>>();
            let encoded = serde_json::to_string(&candidates).unwrap_or_else(|_| "[]".into());
            Err(LpcError::AccountAmbiguous {
                selector,
                candidates: encoded,
            })
        }
    }
}

pub fn summarize_account(
    account: &AccountRecord,
    app: &AppRecord,
    active: bool,
    running_commands: usize,
    with_scopes: bool,
) -> AccountSummary {
    AccountSummary {
        id: account.id,
        alias: account.alias.clone(),
        display_name: account.display_name.clone(),
        app_id: app.app_id.clone(),
        app_label: app.label.clone(),
        tenant_label: account.tenant_label.clone(),
        health: account.health.clone(),
        credential_origin: account.credential_origin,
        active,
        running_commands,
        last_verified_at: account.last_verified_at,
        scope_count: account.effective_scopes.len(),
        effective_scopes: with_scopes.then(|| account.effective_scopes.clone()),
    }
}

pub fn summarize_views(views: &[AccountView], with_scopes: bool) -> Vec<AccountSummary> {
    views
        .iter()
        .map(|view| {
            summarize_account(
                &view.account,
                &view.app,
                view.active,
                view.running_commands,
                with_scopes,
            )
        })
        .collect()
}

pub fn search_accounts(
    catalog: &Catalog,
    state: &ActiveState,
    running: &HashMap<Uuid, usize>,
    filter: &SearchFilter,
    with_scopes: bool,
) -> Result<Vec<AccountSummary>> {
    let app_scoped = filter_by_app(catalog, filter.app.as_deref())?;
    let query = filter
        .query
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase());

    let mut out = Vec::new();
    for (account, app) in app_scoped {
        if let Some(health) = &filter.health {
            if &account.health != health {
                continue;
            }
        }
        if let Some(scope) = filter.scope.as_deref() {
            if !account.effective_scopes.iter().any(|item| item == scope) {
                continue;
            }
        }
        if let Some(query) = &query {
            let haystacks = [
                account.display_name.as_str(),
                account.alias.as_deref().unwrap_or(""),
                app.label.as_str(),
                app.app_id.as_str(),
                account.tenant_label.as_deref().unwrap_or(""),
            ];
            let matched = haystacks
                .iter()
                .any(|value| value.to_ascii_lowercase().contains(query));
            if !matched {
                continue;
            }
        }
        out.push(summarize_account(
            account,
            app,
            state.active_account_id == Some(account.id),
            running.get(&account.id).copied().unwrap_or(0),
            with_scopes,
        ));
    }
    out.sort_by(|left, right| {
        left.display_name
            .cmp(&right.display_name)
            .then(left.id.cmp(&right.id))
    });
    Ok(out)
}

pub fn assert_summary_safe(summary: &AccountSummary) -> Result<()> {
    let encoded = serde_json::to_string(summary)?;
    let lower = encoded.to_ascii_lowercase();
    for needle in [
        "secret",
        "token",
        "devicecode",
        "device_code",
        "keychain",
        "configdir",
        "config_dir",
    ] {
        if lower.contains(needle) {
            return Err(LpcError::Internal(format!(
                "account summary leaked sensitive marker '{needle}'"
            )));
        }
    }
    Ok(())
}

/// Strip only a leading run of product-owned identity flags.
///
/// Consumes `--account` / `--lpc-account` (`=` or following value) and stops at
/// the first other token. `--` is never consumed. Official `--profile`, `--as`,
/// mid-argv identity flags, and unknown leading `--lpc-*` stay as documented:
/// unknown `--lpc-*` is an error; everything else is forwarded unchanged.
pub fn strip_leading_lpc_flags(args: &[OsString]) -> Result<LpcArgv> {
    let mut index = 0;
    let mut account_override = None;
    while index < args.len() {
        let arg = args[index].to_string_lossy();
        if arg == "--" {
            break;
        }
        if let Some(consumed) = take_account_override(args, index, &mut account_override)? {
            index += consumed;
            continue;
        }
        if arg.starts_with("--lpc-") {
            return Err(LpcError::AccountSelectorInvalid(format!(
                "unknown LPC flag '{arg}'"
            )));
        }
        break;
    }
    Ok(LpcArgv {
        account_override,
        forwarded: args[index..].to_vec(),
    })
}

fn take_account_override(
    args: &[OsString],
    index: usize,
    account_override: &mut Option<String>,
) -> Result<Option<usize>> {
    let arg = args[index].to_string_lossy();
    let (flag, inline) = if let Some(value) = arg.strip_prefix("--account=") {
        ("--account", Some(value.to_owned()))
    } else if let Some(value) = arg.strip_prefix("--lpc-account=") {
        ("--lpc-account", Some(value.to_owned()))
    } else if arg == "--account" || arg == "--lpc-account" {
        (arg.as_ref(), None)
    } else {
        return Ok(None);
    };

    let (value, consumed) = if let Some(value) = inline {
        (value, 1usize)
    } else {
        let Some(raw) = args.get(index + 1) else {
            return Err(LpcError::AccountSelectorInvalid(format!(
                "{flag} requires a value"
            )));
        };
        let value = raw.to_string_lossy();
        if value.is_empty() || value.starts_with('-') {
            return Err(LpcError::AccountSelectorInvalid(format!(
                "{flag} requires a non-flag value"
            )));
        }
        (value.into_owned(), 2usize)
    };

    if value.is_empty() {
        return Err(LpcError::AccountSelectorInvalid(format!(
            "{flag} value must be non-empty"
        )));
    }
    if account_override.replace(value).is_some() {
        return Err(LpcError::AccountSelectorInvalid(
            "duplicate --account / --lpc-account flag".into(),
        ));
    }
    Ok(Some(consumed))
}

pub fn resolve_execution_override(flag: Option<&str>, env_value: Option<&OsStr>) -> Option<String> {
    if let Some(flag) = flag.map(str::trim).filter(|value| !value.is_empty()) {
        return Some(flag.to_owned());
    }
    env_value
        .map(|value| value.to_string_lossy().into_owned())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Brand, Catalog};
    use chrono::Utc;
    use std::path::PathBuf;

    fn sample_catalog() -> Catalog {
        let now = Utc::now();
        let app_a = Uuid::new_v4();
        let app_b = Uuid::new_v4();
        let mut catalog = Catalog::default();
        catalog.apps.push(AppRecord {
            id: app_a,
            app_id: "cli_a".into(),
            label: "Alpha".into(),
            brand: Brand::Feishu,
            base_config_path: PathBuf::from("a"),
            available_scopes: BTreeSet::new(),
            policy_scopes: BTreeSet::new(),
            scopes_observed_at: None,
            created_at: now,
            updated_at: now,
        });
        catalog.apps.push(AppRecord {
            id: app_b,
            app_id: "cli_b".into(),
            label: "Beta".into(),
            brand: Brand::Feishu,
            base_config_path: PathBuf::from("b"),
            available_scopes: BTreeSet::new(),
            policy_scopes: BTreeSet::new(),
            scopes_observed_at: None,
            created_at: now,
            updated_at: now,
        });
        let id1 = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let id2 = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
        let id3 = Uuid::parse_str("33333333-3333-3333-3333-333333333333").unwrap();
        catalog.accounts.push(AccountRecord {
            id: id1,
            app_ref: app_a,
            user_open_id: "ou_1".into(),
            display_name: "Alice".into(),
            alias: Some("alice".into()),
            tenant_label: Some("TenantA".into()),
            config_dir: PathBuf::from("secret-should-not-leak"),
            credential_origin: CredentialOrigin::Managed,
            health: AccountHealth::Ready,
            effective_scopes: ["docs:read".into()].into_iter().collect(),
            last_verified_at: Some(now),
            created_at: now,
            updated_at: now,
        });
        catalog.accounts.push(AccountRecord {
            id: id2,
            app_ref: app_a,
            user_open_id: "ou_2".into(),
            display_name: "Bob".into(),
            alias: Some("中文别名".into()),
            tenant_label: None,
            config_dir: PathBuf::from("also-secret"),
            credential_origin: CredentialOrigin::Managed,
            health: AccountHealth::Ready,
            effective_scopes: BTreeSet::new(),
            last_verified_at: None,
            created_at: now,
            updated_at: now,
        });
        catalog.accounts.push(AccountRecord {
            id: id3,
            app_ref: app_b,
            user_open_id: "ou_3".into(),
            display_name: "Alice".into(),
            alias: None,
            tenant_label: None,
            config_dir: PathBuf::from("nope"),
            credential_origin: CredentialOrigin::Managed,
            health: AccountHealth::TemporaryFailure,
            effective_scopes: ["im:message".into()].into_iter().collect(),
            last_verified_at: None,
            created_at: now,
            updated_at: now,
        });
        catalog
    }

    #[test]
    fn resolves_uuid_alias_name_and_app_scope() {
        let catalog = sample_catalog();
        let (account, _) = resolve_account(
            &catalog,
            &parse_selector("id:11111111-1111-1111-1111-111111111111").unwrap(),
            None,
        )
        .unwrap();
        assert_eq!(account.display_name, "Alice");

        let (account, _) =
            resolve_account(&catalog, &parse_selector("alias:中文别名").unwrap(), None).unwrap();
        assert_eq!(account.user_open_id, "ou_2");

        let (account, _) =
            resolve_account(&catalog, &parse_selector("app:cli_b/Alice").unwrap(), None).unwrap();
        assert_eq!(account.user_open_id, "ou_3");
    }

    #[test]
    fn duplicate_display_name_is_ambiguous_without_app() {
        let catalog = sample_catalog();
        let error = resolve_account(&catalog, &parse_selector("Alice").unwrap(), None).unwrap_err();
        assert_eq!(error.stable_code(), "LPC_ACCOUNT_AMBIGUOUS");
        let text = error.to_string().to_ascii_lowercase();
        assert!(text.contains("11111111-1111-1111-1111-111111111111"));
        assert!(!text.contains("secret"));
        assert!(!text.contains("config"));
    }

    #[test]
    fn not_found_and_invalid_selectors() {
        let catalog = sample_catalog();
        let missing =
            resolve_account(&catalog, &parse_selector("alias:missing").unwrap(), None).unwrap_err();
        assert_eq!(missing.stable_code(), "LPC_ACCOUNT_NOT_FOUND");
        assert_eq!(
            parse_selector("").unwrap_err().stable_code(),
            "LPC_ACCOUNT_SELECTOR_INVALID"
        );
        assert_eq!(
            parse_selector("id:not-a-uuid").unwrap_err().stable_code(),
            "LPC_ACCOUNT_SELECTOR_INVALID"
        );
    }

    #[test]
    fn old_catalog_without_alias_deserializes() {
        let json = r#"{"schemaVersion":1,"apps":[],"accounts":[{"id":"11111111-1111-1111-1111-111111111111","appRef":"11111111-1111-1111-1111-111111111111","userOpenId":"ou","displayName":"Old","tenantLabel":null,"configDir":"x","credentialOrigin":"managed","health":"ready","effectiveScopes":[],"lastVerifiedAt":null,"createdAt":"2020-01-01T00:00:00Z","updatedAt":"2020-01-01T00:00:00Z"}],"settings":{"pathTakeoverEnabled":true,"recommendedCliVersion":"1.0.68","scopeBatchMaxCount":30,"scopeBatchMaxEncodedBytes":1800}}"#;
        let catalog: Catalog = serde_json::from_str(json).unwrap();
        assert_eq!(catalog.accounts[0].alias, None);
    }

    #[test]
    fn alias_rules_reject_bad_values() {
        assert!(validate_alias("").is_err());
        assert!(validate_alias("id:x").is_err());
        assert!(validate_alias("a/b").is_err());
        assert_eq!(validate_alias("  ok  ").unwrap(), "ok");
    }

    #[test]
    fn strip_leading_flags_supports_forms_and_passthrough() {
        let args = [
            OsString::from("--lpc-account"),
            OsString::from("alice"),
            OsString::from("--profile"),
            OsString::from("official"),
            OsString::from("whoami"),
        ];
        let parsed = strip_leading_lpc_flags(&args).unwrap();
        assert_eq!(parsed.account_override.as_deref(), Some("alice"));
        assert_eq!(
            parsed.forwarded,
            vec![
                OsString::from("--profile"),
                OsString::from("official"),
                OsString::from("whoami")
            ]
        );

        let equals = [
            OsString::from("--lpc-account=中文别名"),
            OsString::from("x"),
        ];
        let parsed = strip_leading_lpc_flags(&equals).unwrap();
        assert_eq!(parsed.account_override.as_deref(), Some("中文别名"));

        let mid = [
            OsString::from("whoami"),
            OsString::from("--lpc-account"),
            OsString::from("alice"),
        ];
        let parsed = strip_leading_lpc_flags(&mid).unwrap();
        assert!(parsed.account_override.is_none());
        assert_eq!(parsed.forwarded.len(), 3);

        let after_dd = [
            OsString::from("--"),
            OsString::from("--lpc-account"),
            OsString::from("alice"),
        ];
        let parsed = strip_leading_lpc_flags(&after_dd).unwrap();
        assert!(parsed.account_override.is_none());
        assert_eq!(parsed.forwarded[0], OsString::from("--"));

        assert_eq!(
            strip_leading_lpc_flags(&[OsString::from("--lpc-foo")])
                .unwrap_err()
                .stable_code(),
            "LPC_ACCOUNT_SELECTOR_INVALID"
        );

        let public = [
            OsString::from("--account"),
            OsString::from("alice"),
            OsString::from("whoami"),
        ];
        let parsed = strip_leading_lpc_flags(&public).unwrap();
        assert_eq!(parsed.account_override.as_deref(), Some("alice"));
        assert_eq!(parsed.forwarded, vec![OsString::from("whoami")]);

        let mixed = [
            OsString::from("--account=bob"),
            OsString::from("--lpc-account"),
            OsString::from("alice"),
        ];
        assert_eq!(
            strip_leading_lpc_flags(&mixed).unwrap_err().stable_code(),
            "LPC_ACCOUNT_SELECTOR_INVALID"
        );
    }

    #[test]
    fn override_priority_flag_over_env() {
        assert_eq!(
            resolve_execution_override(Some("flag"), Some(OsStr::new("env"))).as_deref(),
            Some("flag")
        );
        assert_eq!(
            resolve_execution_override(None, Some(OsStr::new("env"))).as_deref(),
            Some("env")
        );
        assert_eq!(resolve_execution_override(None, None), None);
    }

    #[test]
    fn compact_summary_omits_scopes_and_secrets() {
        let catalog = sample_catalog();
        let account = &catalog.accounts[0];
        let app = &catalog.apps[0];
        let summary = summarize_account(account, app, true, 2, false);
        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("\"scopeCount\":1"));
        assert!(!json.contains("effectiveScopes"));
        assert!(!json.to_ascii_lowercase().contains("config"));
        assert_summary_safe(&summary).unwrap();
        let with_scopes = summarize_account(account, app, true, 2, true);
        assert!(serde_json::to_string(&with_scopes)
            .unwrap()
            .contains("docs:read"));
    }

    #[test]
    fn search_is_loose() {
        let catalog = sample_catalog();
        let state = ActiveState {
            active_account_id: Some(catalog.accounts[0].id),
            ..ActiveState::default()
        };
        let results = search_accounts(
            &catalog,
            &state,
            &HashMap::new(),
            &SearchFilter {
                query: Some("alpha".into()),
                app: None,
                health: None,
                scope: None,
            },
            false,
        )
        .unwrap();
        assert_eq!(results.len(), 2);
    }
}
