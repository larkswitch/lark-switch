use crate::atomic::{write_bytes_atomic, write_json_atomic};
use crate::cli::{IdentityStatus, OfficialCli, SecretString, WhoAmI};
use crate::error::{LpcError, Result};
use crate::locking::RoutingGate;
use crate::model::{AccountHealth, AccountRecord, AppRecord, Brand, Catalog, CredentialOrigin};
use crate::scope_policy::{clamp_policy, default_policy, validate_policy_selection};
use crate::store::StateStore;
use chrono::{DateTime, Duration, Utc};
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum HealthRefreshOutcome {
    Updated(AccountRecord),
    SkippedBusy(AccountRecord),
}

impl HealthRefreshOutcome {
    pub fn account(self) -> AccountRecord {
        match self {
            Self::Updated(account) | Self::SkippedBusy(account) => account,
        }
    }

    pub fn skipped_busy(&self) -> bool {
        matches!(self, Self::SkippedBusy(_))
    }
}

#[derive(Debug, Clone)]
pub struct ImportedApp {
    pub app: AppRecord,
    pub config_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExistingCliCandidate {
    pub config_dir: PathBuf,
    pub app_id: String,
    pub brand: Brand,
    pub display_name: String,
    pub user_open_id: String,
    pub health: AccountHealth,
    pub already_imported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExistingAccountImport {
    pub app: AppRecord,
    pub account: AccountRecord,
    pub already_imported: bool,
}

#[derive(Debug, Clone)]
struct MigrationSourceIdentity {
    app_id: String,
    brand: Brand,
    user_open_id: String,
    display_name: String,
}

#[derive(Debug, Clone)]
struct PreparedExistingAccount {
    config_dir: PathBuf,
    identity: MigrationSourceIdentity,
    sanitized_base: Value,
    isolated_account: Value,
    available_scopes: BTreeSet<String>,
    effective_scopes: BTreeSet<String>,
    display_name: String,
    health: AccountHealth,
}

fn reject_duplicate_new_account(catalog: &Catalog, app_ref: Uuid, open_id: &str) -> Result<()> {
    if let Some(account) = catalog
        .accounts
        .iter()
        .find(|account| account.app_ref == app_ref && account.user_open_id == open_id)
    {
        return Err(LpcError::AccountAlreadyExists {
            account_id: account.id.to_string(),
        });
    }
    Ok(())
}

pub fn default_official_config_dirs() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(value) = std::env::var_os("LARKSUITE_CLI_CONFIG_DIR") {
        candidates.push(PathBuf::from(value));
    }
    if let Some(base) = BaseDirs::new() {
        candidates.push(base.home_dir().join(".lark-cli"));
    }

    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter(|path| path.join("config.json").is_file())
        .filter_map(|path| path.canonicalize().ok().or(Some(path)))
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

#[derive(Debug, Clone)]
pub struct AccountService {
    store: StateStore,
    gate: RoutingGate,
    cli: OfficialCli,
}

impl AccountService {
    pub fn new(store: StateStore, cli: OfficialCli) -> Self {
        let gate = RoutingGate::new(store.paths().clone());
        Self { store, gate, cli }
    }

    pub fn store(&self) -> &StateStore {
        &self.store
    }

    pub fn cli(&self) -> &OfficialCli {
        &self.cli
    }

    pub fn discover_existing_account_config(
        &self,
        config_dir: &Path,
    ) -> Result<ExistingCliCandidate> {
        let config_dir = config_dir.canonicalize()?;
        let source: Value = serde_json::from_slice(&fs::read(config_dir.join("config.json"))?)?;
        let identity = migration_source_identity(&source)?;
        let catalog = self.store.load_catalog()?;
        let already_imported = catalog
            .apps
            .iter()
            .find(|app| app.app_id == identity.app_id)
            .is_some_and(|app| {
                catalog.accounts.iter().any(|account| {
                    account.app_ref == app.id && account.user_open_id == identity.user_open_id
                })
            });
        Ok(ExistingCliCandidate {
            config_dir,
            app_id: identity.app_id,
            brand: identity.brand,
            display_name: identity.display_name,
            user_open_id: identity.user_open_id,
            health: AccountHealth::Unknown,
            already_imported,
        })
    }

    pub fn inspect_existing_account_config(
        &self,
        config_dir: &Path,
    ) -> Result<ExistingCliCandidate> {
        let prepared = self.prepare_existing_account(config_dir)?;
        let catalog = self.store.load_catalog()?;
        let already_imported = catalog
            .apps
            .iter()
            .find(|app| app.app_id == prepared.identity.app_id)
            .is_some_and(|app| {
                catalog.accounts.iter().any(|account| {
                    account.app_ref == app.id
                        && account.user_open_id == prepared.identity.user_open_id
                })
            });
        Ok(ExistingCliCandidate {
            config_dir: prepared.config_dir,
            app_id: prepared.identity.app_id,
            brand: prepared.identity.brand,
            display_name: prepared.display_name,
            user_open_id: prepared.identity.user_open_id,
            health: prepared.health,
            already_imported,
        })
    }

    pub fn import_existing_account_config(
        &self,
        label: &str,
        config_dir: &Path,
    ) -> Result<ExistingAccountImport> {
        let prepared = self.prepare_existing_account(config_dir)?;
        self.commit_existing_account(label, prepared)
    }

    fn prepare_existing_account(&self, config_dir: &Path) -> Result<PreparedExistingAccount> {
        self.store.initialize()?;
        let config_dir = config_dir.canonicalize()?;

        // Read the migration source exactly once. Every official CLI command is
        // then run against an LPC-owned snapshot so verification/refresh cannot
        // mutate the user's original CLI config.
        let source: Value = serde_json::from_slice(&fs::read(config_dir.join("config.json"))?)?;
        let identity = migration_source_identity(&source)?;
        let sanitized_base = sanitize_official_config(&source)?;
        let isolated_account = isolate_official_account_config(&source, &identity.user_open_id)?;
        let staging = self
            .store
            .paths()
            .staging_dir()
            .join(format!("existing-account-{}", Uuid::new_v4()));
        fs::create_dir_all(&staging)?;
        let result = (|| {
            write_json_atomic(&staging.join("config.json"), &isolated_account)?;
            let scopes = self.cli.scopes(&staging)?.value;
            if scopes.app_id != identity.app_id || scopes.token_type != "user" {
                return Err(LpcError::UnsafeConfig(
                    "existing CLI scope identity does not match its config".into(),
                ));
            }

            let whoami = self.cli.whoami(&staging)?.value;
            let delegated = verified_delegated_user(&whoami)?;
            if whoami.app_id != identity.app_id || delegated.open_id != identity.user_open_id {
                return Err(LpcError::AuthIdentityMismatch {
                    expected: identity.user_open_id.clone(),
                    actual: delegated.open_id,
                });
            }

            let status = self.cli.status(&staging, true)?.value;
            let user = &status.identities.user;
            if !user.available
                || (status.verified != Some(true) && user.verified != Some(true))
                || (!user.open_id.is_empty() && user.open_id != identity.user_open_id)
                || (!status.app_id.is_empty() && status.app_id != identity.app_id)
            {
                return Err(LpcError::CliFailed {
                    code: 1,
                    message: if user.message.is_empty() {
                        "existing CLI user identity could not be verified".into()
                    } else {
                        user.message.clone()
                    },
                });
            }

            Ok(PreparedExistingAccount {
                config_dir,
                identity: identity.clone(),
                sanitized_base,
                isolated_account,
                available_scopes: scopes.user_scopes,
                effective_scopes: user.scope.clone(),
                display_name: if delegated.user_name.is_empty() {
                    identity.display_name.clone()
                } else {
                    delegated.user_name
                },
                health: account_health_from_status(
                    user.status.as_str(),
                    user.token_status.as_str(),
                ),
            })
        })();
        let _ = fs::remove_dir_all(&staging);
        result
    }

    fn commit_existing_account(
        &self,
        label: &str,
        prepared: PreparedExistingAccount,
    ) -> Result<ExistingAccountImport> {
        let _guard = self.gate.lock()?;
        let mut catalog = self.store.load_catalog()?;
        let original_catalog = catalog.clone();
        let original_state = self.store.load_state()?;
        let now = Utc::now();

        let existing_app = catalog
            .apps
            .iter()
            .find(|app| app.app_id == prepared.identity.app_id)
            .cloned();
        let app_created = existing_app.is_none();
        let app = if let Some(mut app) = existing_app {
            // A second account under the same App must not rename every existing
            // account card. The user-supplied label is only for a new App.
            app.brand = prepared.identity.brand;
            app.available_scopes = prepared.available_scopes.clone();
            // Preserve the user's stable subset; only drop scopes no longer allowed.
            app.policy_scopes = clamp_policy(&app.policy_scopes, &app.available_scopes);
            if app.policy_scopes.is_empty() {
                app.policy_scopes = default_policy(&app.available_scopes);
            }
            app.scopes_observed_at = Some(now);
            app.updated_at = now;
            app
        } else {
            let id = Uuid::new_v4();
            AppRecord {
                id,
                app_id: prepared.identity.app_id.clone(),
                label: label.to_owned(),
                brand: prepared.identity.brand,
                base_config_path: self.store.paths().app_base_config(id),
                available_scopes: prepared.available_scopes.clone(),
                policy_scopes: default_policy(&prepared.available_scopes),
                scopes_observed_at: Some(now),
                created_at: now,
                updated_at: now,
            }
        };

        let existing_account = catalog
            .accounts
            .iter()
            .find(|account| {
                account.app_ref == app.id && account.user_open_id == prepared.identity.user_open_id
            })
            .cloned();
        let already_imported = existing_account.is_some();
        let account_created = existing_account.is_none();
        let account = if let Some(mut account) = existing_account {
            account.display_name = prepared.display_name;
            account.credential_origin = CredentialOrigin::ExternalShared;
            account.health = prepared.health;
            account.effective_scopes = prepared.effective_scopes;
            account.last_verified_at = Some(now);
            account.updated_at = now;
            account
        } else {
            let id = Uuid::new_v4();
            AccountRecord {
                id,
                app_ref: app.id,
                user_open_id: prepared.identity.user_open_id,
                display_name: prepared.display_name,
                alias: None,
                tenant_label: None,
                config_dir: self.store.paths().account_config_dir(id),
                credential_origin: CredentialOrigin::ExternalShared,
                health: prepared.health,
                effective_scopes: prepared.effective_scopes,
                last_verified_at: Some(now),
                created_at: now,
                updated_at: now,
            }
        };

        let original_app_config = read_optional_file(&app.base_config_path)?;
        let account_config_path = account.config_dir.join("config.json");
        let original_account_config = read_optional_file(&account_config_path)?;

        let result = (|| {
            write_json_atomic(&app.base_config_path, &prepared.sanitized_base)?;
            fs::create_dir_all(&account.config_dir)?;
            write_json_atomic(&account_config_path, &prepared.isolated_account)?;

            if let Some(existing) = catalog.apps.iter_mut().find(|item| item.id == app.id) {
                *existing = app.clone();
            } else {
                catalog.apps.push(app.clone());
            }
            if let Some(existing) = catalog
                .accounts
                .iter_mut()
                .find(|item| item.id == account.id)
            {
                *existing = account.clone();
            } else {
                catalog.accounts.push(account.clone());
            }
            self.store.save_catalog(&catalog)?;

            if original_state.active_account_id.is_none() {
                let mut state = original_state.clone();
                state.active_account_id = Some(account.id);
                state.generation = state.generation.saturating_add(1);
                state.updated_at = Utc::now();
                self.store.save_state(&state)?;
            }
            Ok(ExistingAccountImport {
                app: app.clone(),
                account: account.clone(),
                already_imported,
            })
        })();

        match result {
            Ok(imported) => Ok(imported),
            Err(import_error) => {
                // Restore configs/delete new directories only after catalog
                // rollback succeeds. If metadata rollback itself fails, the
                // new files are deliberately retained so the committed catalog
                // cannot be left with dangling paths.
                match self.store.save_catalog(&original_catalog) {
                    Ok(()) => {
                        let state_rollback = self.store.save_state(&original_state);
                        let app_rollback =
                            restore_optional_file(&app.base_config_path, &original_app_config);
                        let account_rollback =
                            restore_optional_file(&account_config_path, &original_account_config);
                        if account_created {
                            let _ = fs::remove_dir_all(&account.config_dir);
                        }
                        if app_created {
                            if let Some(parent) = app.base_config_path.parent() {
                                let _ = fs::remove_dir_all(parent);
                            }
                        }
                        if let Err(rollback_error) = state_rollback
                            .and(app_rollback)
                            .and(account_rollback)
                        {
                            return Err(LpcError::Internal(format!(
                                "migration failed: {import_error}; rollback was incomplete: {rollback_error}"
                            )));
                        }
                        Err(import_error)
                    }
                    Err(rollback_error) => Err(LpcError::Internal(format!(
                        "migration failed: {import_error}; catalog rollback failed, new configs were retained: {rollback_error}"
                    ))),
                }
            }
        }
    }

    pub fn import_existing_app(
        &self,
        label: &str,
        app_id: &str,
        app_secret: SecretString,
        brand: Brand,
    ) -> Result<AppRecord> {
        self.store.initialize()?;
        if self
            .store
            .load_catalog()?
            .apps
            .iter()
            .any(|app| app.app_id == app_id)
        {
            // `config init` would overwrite the shared appsecret:<appId>
            // keychain value before we can compare it. Treat credential repair
            // as a separate explicit operation instead of silently mutating all
            // users attached to the App.
            return Err(LpcError::AppAlreadyExists(app_id.to_owned()));
        }
        let staging = self
            .store
            .paths()
            .staging_dir()
            .join(format!("app-import-{}", Uuid::new_v4()));
        fs::create_dir_all(&staging)?;
        let result = (|| {
            self.cli
                .config_init_existing(&staging, app_id, &app_secret, brand)?;
            self.import_official_config(label, &staging)
        })();
        let _ = fs::remove_dir_all(&staging);
        result
    }

    /// Imports a configuration created by the official CLI. The App Secret
    /// itself is never read; only an official keychain reference is accepted.
    pub fn import_official_config(&self, label: &str, config_dir: &Path) -> Result<AppRecord> {
        self.store.initialize()?;
        let source = config_dir.join("config.json");
        let root: Value = serde_json::from_slice(&fs::read(&source)?)?;
        let sanitized = sanitize_official_config(&root)?;
        let app_id = sanitized["apps"][0]["appId"]
            .as_str()
            .ok_or_else(|| LpcError::UnsafeConfig("missing appId".into()))?
            .to_owned();
        let brand = match sanitized["apps"][0]["brand"].as_str().unwrap_or("feishu") {
            "lark" => Brand::Lark,
            _ => Brand::Feishu,
        };

        let scopes = self.cli.scopes(config_dir)?.value;
        if scopes.app_id != app_id {
            return Err(LpcError::UnsafeConfig(format!(
                "auth scopes returned appId {}, expected {}",
                scopes.app_id, app_id
            )));
        }
        if scopes.token_type != "user" {
            return Err(LpcError::UnsafeConfig(format!(
                "auth scopes tokenType must be user, received {}",
                scopes.token_type
            )));
        }

        let _guard = self.gate.lock()?;
        let mut catalog = self.store.load_catalog()?;
        if let Some(existing) = catalog.apps.iter_mut().find(|app| app.app_id == app_id) {
            existing.label = label.to_owned();
            existing.brand = brand;
            existing.available_scopes = scopes.user_scopes.clone();
            existing.policy_scopes =
                clamp_policy(&existing.policy_scopes, &existing.available_scopes);
            if existing.policy_scopes.is_empty() {
                existing.policy_scopes = default_policy(&existing.available_scopes);
            }
            existing.scopes_observed_at = Some(Utc::now());
            existing.updated_at = Utc::now();
            write_json_atomic(&existing.base_config_path, &sanitized)?;
            let result = existing.clone();
            self.store.save_catalog(&catalog)?;
            return Ok(result);
        }

        let id = Uuid::new_v4();
        let base_config_path = self.store.paths().app_base_config(id);
        write_json_atomic(&base_config_path, &sanitized)?;
        let now = Utc::now();
        let app = AppRecord {
            id,
            app_id,
            label: label.to_owned(),
            brand,
            base_config_path,
            available_scopes: scopes.user_scopes.clone(),
            policy_scopes: default_policy(&scopes.user_scopes),
            scopes_observed_at: Some(now),
            created_at: now,
            updated_at: now,
        };
        catalog.apps.push(app.clone());
        self.store.save_catalog(&catalog)?;
        Ok(app)
    }

    pub fn refresh_app_boundary(&self, app_ref: Uuid) -> Result<AppRecord> {
        // Load path and identity without holding the routing gate across CLI I/O.
        let catalog = self.store.load_catalog()?;
        let app = catalog
            .apps
            .iter()
            .find(|app| app.id == app_ref)
            .ok_or_else(|| LpcError::AppNotFound(app_ref.to_string()))?;
        let expected_app_id = app.app_id.clone();
        let config_dir = app
            .base_config_path
            .parent()
            .ok_or_else(|| LpcError::Internal("base config has no parent".into()))?
            .to_path_buf();
        let scopes = self.cli.scopes(&config_dir)?.value;
        if scopes.app_id != expected_app_id || scopes.token_type != "user" {
            return Err(LpcError::UnsafeConfig(
                "scope boundary identity mismatch".into(),
            ));
        }

        let _guard = self.gate.lock()?;
        let mut catalog = self.store.load_catalog()?;
        let app = catalog
            .apps
            .iter_mut()
            .find(|app| app.id == app_ref)
            .ok_or_else(|| LpcError::AppNotFound(app_ref.to_string()))?;
        if app.app_id != scopes.app_id {
            return Err(LpcError::UnsafeConfig(
                "scope boundary identity mismatch".into(),
            ));
        }
        app.available_scopes = scopes.user_scopes;
        // Clamp only — do not expand a deliberate user subset back to full default.
        app.policy_scopes = clamp_policy(&app.policy_scopes, &app.available_scopes);
        if app.policy_scopes.is_empty() {
            app.policy_scopes = default_policy(&app.available_scopes);
        }
        app.scopes_observed_at = Some(Utc::now());
        app.updated_at = Utc::now();
        let result = app.clone();
        self.store.save_catalog(&catalog)?;
        Ok(result)
    }

    pub fn set_app_policy(&self, app_ref: Uuid, selected: BTreeSet<String>) -> Result<AppRecord> {
        validate_policy_selection(&selected)?;
        let _guard = self.gate.lock()?;
        let mut catalog = self.store.load_catalog()?;
        let app = catalog
            .apps
            .iter_mut()
            .find(|app| app.id == app_ref)
            .ok_or_else(|| LpcError::AppNotFound(app_ref.to_string()))?;
        let outside: Vec<String> = selected
            .difference(&app.available_scopes)
            .cloned()
            .collect();
        if !outside.is_empty() {
            return Err(LpcError::ScopeOutOfBoundary(outside));
        }
        app.policy_scopes = selected;
        app.updated_at = Utc::now();
        let result = app.clone();
        self.store.save_catalog(&catalog)?;
        Ok(result)
    }

    pub fn prepare_account_config(&self, app_ref: Uuid, destination: &Path) -> Result<AppRecord> {
        let catalog = self.store.load_catalog()?;
        let app = catalog
            .apps
            .iter()
            .find(|app| app.id == app_ref)
            .cloned()
            .ok_or_else(|| LpcError::AppNotFound(app_ref.to_string()))?;
        let base: Value = serde_json::from_slice(&fs::read(&app.base_config_path)?)?;
        let clean = sanitize_official_config(&base)?;
        fs::create_dir_all(destination)?;
        write_json_atomic(&destination.join("config.json"), &clean)?;
        Ok(app)
    }

    pub fn register_new_account_from_config(
        &self,
        app_ref: Uuid,
        config_dir: &Path,
    ) -> Result<AccountRecord> {
        let whoami = self.cli.whoami(config_dir)?.value;
        let delegated = verified_delegated_user(&whoami)?;
        let status = self.cli.status(config_dir, true)?.value;
        if status.identities.user.verified == Some(false) || !status.identities.user.available {
            return Err(LpcError::CliFailed {
                code: 1,
                message: status.identities.user.message,
            });
        }
        let effective_scopes = status.identities.user.scope.clone();

        let source: Value = serde_json::from_slice(&fs::read(config_dir.join("config.json"))?)?;
        let isolated = isolate_official_account_config(&source, &delegated.open_id)?;

        let _guard = self.gate.lock()?;
        let mut catalog = self.store.load_catalog()?;
        let app_id = catalog
            .apps
            .iter()
            .find(|app| app.id == app_ref)
            .map(|app| app.app_id.clone())
            .ok_or_else(|| LpcError::AppNotFound(app_ref.to_string()))?;
        if app_id != whoami.app_id {
            return Err(LpcError::UnsafeConfig(format!(
                "whoami appId {} does not match app {}",
                whoami.app_id, app_id
            )));
        }
        if (!status.app_id.is_empty() && status.app_id != app_id)
            || (!status.identities.user.open_id.is_empty()
                && status.identities.user.open_id != delegated.open_id)
        {
            return Err(LpcError::AuthIdentityMismatch {
                expected: delegated.open_id,
                actual: status.identities.user.open_id,
            });
        }
        reject_duplicate_new_account(&catalog, app_ref, &delegated.open_id)?;

        let account_id = Uuid::new_v4();
        let final_dir = self.store.paths().account_config_dir(account_id);
        fs::create_dir_all(&final_dir)?;
        // Persist only approved official metadata for this verified user.
        // OAuth cache/device files and unexpected token fields are never promoted.
        write_json_atomic(&final_dir.join("config.json"), &isolated)?;
        let now = Utc::now();
        let account = AccountRecord {
            id: account_id,
            app_ref,
            user_open_id: delegated.open_id,
            display_name: if delegated.user_name.is_empty() {
                "Unnamed user".to_owned()
            } else {
                delegated.user_name
            },
            alias: None,
            tenant_label: None,
            config_dir: final_dir,
            credential_origin: CredentialOrigin::Managed,
            health: AccountHealth::Ready,
            effective_scopes,
            last_verified_at: Some(now),
            created_at: now,
            updated_at: now,
        };
        catalog.accounts.push(account.clone());
        self.store.save_catalog(&catalog)?;
        let state = self.store.load_state()?;
        if state.active_account_id.is_none() {
            self.store.switch_active_account(account.id)?;
        }
        Ok(account)
    }

    pub fn refresh_account_health(&self, account_id: Uuid) -> Result<AccountRecord> {
        Ok(self
            .refresh_account_health_with(account_id, false)?
            .account())
    }

    pub fn refresh_account_health_with(
        &self,
        account_id: Uuid,
        force_verify: bool,
    ) -> Result<HealthRefreshOutcome> {
        // Resolve config_dir without holding the routing gate across CLI I/O.
        let catalog = self.store.load_catalog()?;
        let config_dir = catalog
            .accounts
            .iter()
            .find(|account| account.id == account_id)
            .map(|account| account.config_dir.clone())
            .ok_or_else(|| LpcError::AccountNotFound(account_id.to_string()))?;
        let now = Utc::now();
        let status = if force_verify {
            self.cli.status(&config_dir, true)
        } else {
            match self.cli.status(&config_dir, false) {
                Ok(quick) => {
                    let user = &quick.value.identities.user;
                    let token_status = non_empty_field(&user.token_status);
                    let expires_at = non_empty_field(&user.expires_at);
                    if needs_verify(token_status, expires_at, now) {
                        self.cli.status(&config_dir, true)
                    } else {
                        Ok(quick)
                    }
                }
                Err(error) => Err(error),
            }
        };

        if status.as_ref().err().is_some_and(should_skip_health_update) {
            // A business command holds the shared CLI keychain lock. Skip this
            // round entirely: health and last_verified_at stay untouched.
            let account = self
                .store
                .load_catalog()?
                .accounts
                .into_iter()
                .find(|account| account.id == account_id)
                .ok_or_else(|| LpcError::AccountNotFound(account_id.to_string()))?;
            return Ok(HealthRefreshOutcome::SkippedBusy(account));
        }

        let _guard = self.gate.lock()?;
        let mut catalog = self.store.load_catalog()?;
        let account = catalog
            .accounts
            .iter_mut()
            .find(|account| account.id == account_id)
            .ok_or_else(|| LpcError::AccountNotFound(account_id.to_string()))?;
        match status {
            Ok(result) => {
                let user = result.value.identities.user;
                account.health = map_identity_status_to_health(&user);
                account.effective_scopes = user.scope;
                account.last_verified_at = Some(Utc::now());
            }
            Err(_) => account.health = AccountHealth::CliFailure,
        }
        account.updated_at = Utc::now();
        let result = account.clone();
        self.store.save_catalog(&catalog)?;
        Ok(HealthRefreshOutcome::Updated(result))
    }

    pub fn switch_account(&self, account_id: Uuid) -> Result<()> {
        self.gate.switch_account(&self.store, account_id)
    }

    pub fn set_account_alias(&self, account_id: Uuid, alias: &str) -> Result<AccountRecord> {
        let alias = crate::selector::validate_alias(alias)?;
        let _guard = self.gate.lock()?;
        let mut catalog = self.store.load_catalog()?;
        if let Some(other) = catalog.accounts.iter().find(|account| {
            account.id != account_id && account.alias.as_deref() == Some(alias.as_str())
        }) {
            return Err(LpcError::AccountSelectorInvalid(format!(
                "alias '{alias}' is already assigned to account {}",
                other.id
            )));
        }
        let account = catalog
            .accounts
            .iter_mut()
            .find(|account| account.id == account_id)
            .ok_or_else(|| LpcError::AccountNotFound(account_id.to_string()))?;
        account.alias = Some(alias);
        account.updated_at = Utc::now();
        let cloned = account.clone();
        self.store.save_catalog(&catalog)?;
        Ok(cloned)
    }

    pub fn clear_account_alias(&self, account_id: Uuid) -> Result<AccountRecord> {
        let _guard = self.gate.lock()?;
        let mut catalog = self.store.load_catalog()?;
        let account = catalog
            .accounts
            .iter_mut()
            .find(|account| account.id == account_id)
            .ok_or_else(|| LpcError::AccountNotFound(account_id.to_string()))?;
        account.alias = None;
        account.updated_at = Utc::now();
        let cloned = account.clone();
        self.store.save_catalog(&catalog)?;
        Ok(cloned)
    }

    pub fn remove_account(&self, account_id: Uuid) -> Result<()> {
        let _guard = self.gate.lock_account_idle(account_id)?;
        let mut catalog = self.store.load_catalog()?;
        let index = catalog
            .accounts
            .iter()
            .position(|account| account.id == account_id)
            .ok_or_else(|| LpcError::AccountNotFound(account_id.to_string()))?;
        let account = catalog.accounts[index].clone();
        if account.credential_origin == CredentialOrigin::Managed {
            // Managed OAuth accounts are logged out through the official CLI.
            // Imported configs share the original CLI's keychain credential,
            // so removing their LPC metadata must leave that login untouched.
            let _ = self.cli.logout(&account.config_dir);
        }
        catalog.accounts.remove(index);
        self.store.save_catalog(&catalog)?;
        let _ = fs::remove_dir_all(&account.config_dir);

        let mut state = self.store.load_state()?;
        if state.active_account_id == Some(account_id) {
            state.active_account_id = catalog.accounts.first().map(|item| item.id);
            state.generation = state.generation.saturating_add(1);
            state.updated_at = Utc::now();
            self.store.save_state(&state)?;
        }
        Ok(())
    }

    pub fn remove_app_metadata(&self, app_ref: Uuid) -> Result<()> {
        let _guard = self.gate.lock()?;
        let mut catalog = self.store.load_catalog()?;
        if catalog
            .accounts
            .iter()
            .any(|account| account.app_ref == app_ref)
        {
            return Err(LpcError::Internal(
                "remove every account attached to the app before removing app metadata".into(),
            ));
        }
        let index = catalog
            .apps
            .iter()
            .position(|app| app.id == app_ref)
            .ok_or_else(|| LpcError::AppNotFound(app_ref.to_string()))?;
        let app = catalog.apps.remove(index);
        self.store.save_catalog(&catalog)?;
        if let Some(parent) = app.base_config_path.parent() {
            let _ = fs::remove_dir_all(parent);
        }
        // Deliberately do not manipulate the OS keychain. The official CLI does
        // not expose a safe "remove app secret only when unused" operation.
        Ok(())
    }
}

fn verified_delegated_user(whoami: &WhoAmI) -> Result<crate::cli::DelegatedUser> {
    if whoami.identity != "user" || !whoami.available {
        return Err(LpcError::InvalidCliOutput(format!(
            "whoami did not resolve an available user identity: {}",
            whoami.hint
        )));
    }
    let delegated = whoami
        .on_behalf_of
        .clone()
        .ok_or_else(|| LpcError::InvalidCliOutput("whoami missing onBehalfOf".into()))?;
    if delegated.open_id.is_empty() {
        return Err(LpcError::InvalidCliOutput(
            "whoami returned an empty openId".into(),
        ));
    }
    Ok(delegated)
}

fn account_health_from_status(status: &str, token_status: &str) -> AccountHealth {
    match (status, token_status) {
        ("ready", "valid") | ("ready", "") => AccountHealth::Ready,
        ("needs_refresh", _) => AccountHealth::Refreshable,
        ("missing", _) | ("not_configured", _) => AccountHealth::ReauthRequired,
        ("verify_failed", _) => AccountHealth::TemporaryFailure,
        _ => AccountHealth::Unknown,
    }
}

fn read_optional_file(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn restore_optional_file(path: &Path, original: &Option<Vec<u8>>) -> Result<()> {
    match original {
        Some(bytes) => write_bytes_atomic(path, bytes),
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        },
    }
}

fn migration_source_identity(root: &Value) -> Result<MigrationSourceIdentity> {
    let sanitized = sanitize_official_config(root)?;
    let app = root
        .get("apps")
        .and_then(Value::as_array)
        .and_then(|apps| apps.first())
        .and_then(Value::as_object)
        .ok_or_else(|| LpcError::UnsafeConfig("config app must be an object".into()))?;
    let users = app
        .get("users")
        .and_then(Value::as_array)
        .ok_or_else(|| LpcError::UnsafeConfig("config app users must be an array".into()))?;
    if users.len() != 1 {
        return Err(LpcError::UnsafeConfig(format!(
            "existing config migration requires exactly one user, found {}",
            users.len()
        )));
    }
    let user = users[0]
        .as_object()
        .ok_or_else(|| LpcError::UnsafeConfig("config user must be an object".into()))?;
    let user_open_id = user
        .get("userOpenId")
        .or_else(|| user.get("openId"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| LpcError::UnsafeConfig("config user open ID is missing".into()))?
        .to_owned();
    let display_name = user
        .get("userName")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("Unnamed user")
        .to_owned();
    let app_id = sanitized["apps"][0]["appId"]
        .as_str()
        .ok_or_else(|| LpcError::UnsafeConfig("missing appId".into()))?
        .to_owned();
    let brand = match sanitized["apps"][0]["brand"].as_str().unwrap_or("feishu") {
        "lark" => Brand::Lark,
        _ => Brand::Feishu,
    };
    Ok(MigrationSourceIdentity {
        app_id,
        brand,
        user_open_id,
        display_name,
    })
}

pub fn isolate_official_account_config(root: &Value, expected_open_id: &str) -> Result<Value> {
    let identity = migration_source_identity(root)?;
    if identity.user_open_id != expected_open_id {
        return Err(LpcError::AuthIdentityMismatch {
            expected: expected_open_id.to_owned(),
            actual: identity.user_open_id,
        });
    }
    let mut isolated = sanitize_official_config(root)?;
    let mut user = Map::new();
    user.insert(
        "userOpenId".into(),
        Value::String(expected_open_id.to_owned()),
    );
    user.insert("userName".into(), Value::String(identity.display_name));
    isolated["apps"][0]["users"] = Value::Array(vec![Value::Object(user)]);
    Ok(isolated)
}

pub fn sanitize_official_config(root: &Value) -> Result<Value> {
    let object = root
        .as_object()
        .ok_or_else(|| LpcError::UnsafeConfig("config root must be an object".into()))?;
    let apps = object
        .get("apps")
        .and_then(Value::as_array)
        .ok_or_else(|| LpcError::UnsafeConfig("config.apps must be an array".into()))?;
    if apps.len() != 1 {
        return Err(LpcError::UnsafeConfig(format!(
            "isolated config must contain exactly one app, found {}",
            apps.len()
        )));
    }
    let app = apps[0]
        .as_object()
        .ok_or_else(|| LpcError::UnsafeConfig("config app must be an object".into()))?;
    let app_id = app
        .get("appId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| LpcError::UnsafeConfig("appId is missing".into()))?;
    let secret = app.get("appSecret").ok_or_else(|| {
        LpcError::UnsafeConfig(
            "App Secret must be an official keychain reference; plaintext config is rejected"
                .into(),
        )
    })?;
    let expected_key = format!("appsecret:{app_id}");
    let secret = normalize_official_keychain_reference(secret, &expected_key).ok_or_else(|| {
        LpcError::UnsafeConfig(format!(
            "App Secret is not a recognized official keychain reference for {expected_key}"
        ))
    })?;

    let mut sanitized_app = Map::new();
    sanitized_app.insert("name".into(), Value::String("lpc".into()));
    sanitized_app.insert("appId".into(), Value::String(app_id.to_owned()));
    sanitized_app.insert("appSecret".into(), secret);
    sanitized_app.insert(
        "brand".into(),
        Value::String(
            app.get("brand")
                .and_then(Value::as_str)
                .unwrap_or("feishu")
                .to_owned(),
        ),
    );
    copy_optional_string(app, &mut sanitized_app, "lang")?;
    copy_optional_string(app, &mut sanitized_app, "defaultAs")?;
    copy_optional_bool(app, &mut sanitized_app, "strictMode")?;
    sanitized_app.insert("users".into(), Value::Array(Vec::new()));

    let mut result = Map::new();
    copy_optional_bool(object, &mut result, "strictMode")?;
    result.insert("currentApp".into(), Value::String("lpc".into()));
    result.insert(
        "apps".into(),
        Value::Array(vec![Value::Object(sanitized_app)]),
    );
    Ok(Value::Object(result))
}

fn copy_optional_string(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
) -> Result<()> {
    if let Some(value) = source.get(key) {
        let value = value
            .as_str()
            .ok_or_else(|| LpcError::UnsafeConfig(format!("config {key} must be a string")))?;
        target.insert(key.to_owned(), Value::String(value.to_owned()));
    }
    Ok(())
}

fn copy_optional_bool(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    key: &str,
) -> Result<()> {
    if let Some(value) = source.get(key) {
        let value = value
            .as_bool()
            .ok_or_else(|| LpcError::UnsafeConfig(format!("config {key} must be a boolean")))?;
        target.insert(key.to_owned(), Value::Bool(value));
    }
    Ok(())
}

fn normalize_official_keychain_reference(value: &Value, expected_key: &str) -> Option<Value> {
    let valid = match value {
        Value::String(text) => text.eq_ignore_ascii_case(&format!("keychain:{expected_key}")),
        Value::Object(map) if map.len() == 2 => {
            map.get("source")
                .and_then(Value::as_str)
                .is_some_and(|source| source.eq_ignore_ascii_case("keychain"))
                && map
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| id.eq_ignore_ascii_case(expected_key))
        }
        _ => false,
    };
    valid.then(|| {
        serde_json::json!({
            "source": "keychain",
            "id": expected_key,
        })
    })
}

impl AccountService {
    pub fn commit_reauthorization_from_config(
        &self,
        account_id: Uuid,
        config_dir: &Path,
        expected_open_id: &str,
    ) -> Result<AccountRecord> {
        let whoami = self.cli.whoami(config_dir)?.value;
        let delegated = verified_delegated_user(&whoami)?;
        if delegated.open_id != expected_open_id {
            return Err(LpcError::AuthIdentityMismatch {
                expected: expected_open_id.to_owned(),
                actual: delegated.open_id,
            });
        }
        let status = self.cli.status(config_dir, true)?.value;
        if !status.identities.user.available {
            return Err(LpcError::CliFailed {
                code: 1,
                message: status.identities.user.message,
            });
        }
        let source: Value = serde_json::from_slice(&fs::read(config_dir.join("config.json"))?)?;
        let isolated = isolate_official_account_config(&source, expected_open_id)?;

        let _guard = self.gate.lock()?;
        let mut catalog = self.store.load_catalog()?;
        let account = catalog
            .accounts
            .iter_mut()
            .find(|account| account.id == account_id)
            .ok_or_else(|| LpcError::AccountNotFound(account_id.to_string()))?;
        if account.user_open_id != expected_open_id {
            return Err(LpcError::AuthIdentityMismatch {
                expected: account.user_open_id.clone(),
                actual: expected_open_id.to_owned(),
            });
        }
        write_json_atomic(&account.config_dir.join("config.json"), &isolated)?;
        account.display_name = if delegated.user_name.is_empty() {
            account.display_name.clone()
        } else {
            delegated.user_name
        };
        account.health = AccountHealth::Ready;
        account.effective_scopes = status.identities.user.scope;
        account.last_verified_at = Some(Utc::now());
        account.updated_at = Utc::now();
        let result = account.clone();
        self.store.save_catalog(&catalog)?;
        Ok(result)
    }

    pub fn account_and_app(&self, account_id: Uuid) -> Result<(AccountRecord, AppRecord)> {
        let catalog = self.store.load_catalog()?;
        let account = catalog
            .accounts
            .iter()
            .find(|account| account.id == account_id)
            .cloned()
            .ok_or_else(|| LpcError::AccountNotFound(account_id.to_string()))?;
        let app = catalog
            .apps
            .iter()
            .find(|app| app.id == account.app_ref)
            .cloned()
            .ok_or_else(|| LpcError::AppNotFound(account.app_ref.to_string()))?;
        Ok((account, app))
    }

    pub fn app(&self, app_ref: Uuid) -> Result<AppRecord> {
        self.store
            .load_catalog()?
            .apps
            .into_iter()
            .find(|app| app.id == app_ref)
            .ok_or_else(|| LpcError::AppNotFound(app_ref.to_string()))
    }
}

const VERIFY_LEAD_TIME: Duration = Duration::minutes(20);

fn non_empty_field(value: &str) -> Option<&str> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn parse_expires_at(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|expires| expires.with_timezone(&Utc))
}

/// Decide whether scheduled health checks should call `auth status --verify`.
pub fn needs_verify(
    token_status: Option<&str>,
    expires_at: Option<&str>,
    now: DateTime<Utc>,
) -> bool {
    let Some(token_status) = token_status else {
        return true;
    };
    if token_status != "ready" && token_status != "valid" {
        return true;
    }
    let Some(expires_at) = expires_at else {
        return true;
    };
    let Some(expires) = parse_expires_at(expires_at) else {
        return true;
    };
    expires.signed_duration_since(now) <= VERIFY_LEAD_TIME
}

/// Scheduled health refresh must not mark an account unhealthy when the shared
/// CLI keychain lock is merely held by a concurrent business command.
fn should_skip_health_update(error: &LpcError) -> bool {
    matches!(error, LpcError::CliKeychainBusy)
}

fn map_identity_status_to_health(user: &IdentityStatus) -> AccountHealth {
    match (user.available, user.status.as_str()) {
        (true, "ready") => AccountHealth::Ready,
        (true, "needs_refresh") => AccountHealth::Refreshable,
        (false, "missing") | (false, "not_configured") => AccountHealth::ReauthRequired,
        (false, "verify_failed") => AccountHealth::TemporaryFailure,
        _ => AccountHealth::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Catalog;
    use crate::paths::AppPaths;
    use chrono::TimeZone;
    use serde_json::json;

    #[test]
    fn needs_verify_false_when_token_fresh() {
        let now = Utc.with_ymd_and_hms(2026, 7, 22, 12, 0, 0).unwrap();
        let expires = "2026-07-22T13:00:00+00:00";
        assert!(!needs_verify(Some("ready"), Some(expires), now));
        assert!(!needs_verify(Some("valid"), Some(expires), now));
    }

    #[test]
    fn needs_verify_true_when_within_twenty_minutes() {
        let now = Utc.with_ymd_and_hms(2026, 7, 22, 12, 50, 0).unwrap();
        let expires = "2026-07-22T13:00:00+00:00";
        assert!(needs_verify(Some("ready"), Some(expires), now));
    }

    #[test]
    fn needs_verify_true_at_exactly_twenty_minutes() {
        let now = Utc.with_ymd_and_hms(2026, 7, 22, 12, 40, 0).unwrap();
        let expires = "2026-07-22T13:00:00+00:00";
        assert!(needs_verify(Some("ready"), Some(expires), now));
    }

    #[test]
    fn needs_verify_false_just_beyond_twenty_minutes() {
        let now = Utc.with_ymd_and_hms(2026, 7, 22, 12, 39, 59).unwrap();
        let expires = "2026-07-22T13:00:00+00:00";
        assert!(!needs_verify(Some("ready"), Some(expires), now));
    }

    #[test]
    fn needs_verify_true_on_parse_failure() {
        let now = Utc.with_ymd_and_hms(2026, 7, 22, 12, 0, 0).unwrap();
        assert!(needs_verify(Some("ready"), Some("not-a-date"), now));
    }

    #[test]
    fn needs_verify_true_on_needs_refresh_token_status() {
        let now = Utc.with_ymd_and_hms(2026, 7, 22, 12, 0, 0).unwrap();
        let expires = "2026-07-22T13:00:00+00:00";
        assert!(needs_verify(Some("needs_refresh"), Some(expires), now));
    }

    #[test]
    fn keychain_busy_skips_health_update_while_other_errors_do_not() {
        // `OfficialCli` is a concrete type spawning real child processes and
        // `CliKeychainBusy` is produced by the in-process lock (a no-op under
        // cfg(test)), so the full refresh path cannot surface it in unit
        // tests. The skip decision is therefore tested as a pure function;
        // `refresh_account_health_with` returns SkippedBusy (no catalog write)
        // when it says skip.
        assert!(should_skip_health_update(&LpcError::CliKeychainBusy));
        assert!(!should_skip_health_update(&LpcError::CliTimeout(60)));
        assert!(!should_skip_health_update(&LpcError::CliFailed {
            code: 1,
            message: "boom".into(),
        }));
        assert!(!should_skip_health_update(&LpcError::Internal("x".into())));
    }

    #[test]
    fn health_refresh_outcome_distinguishes_skip_from_update() {
        let account = AccountRecord {
            id: Uuid::new_v4(),
            app_ref: Uuid::new_v4(),
            user_open_id: "ou_test".into(),
            display_name: "Test".into(),
            alias: None,
            tenant_label: None,
            config_dir: PathBuf::from("test"),
            credential_origin: CredentialOrigin::Managed,
            health: AccountHealth::ReauthRequired,
            effective_scopes: BTreeSet::new(),
            last_verified_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let skipped = HealthRefreshOutcome::SkippedBusy(account.clone());
        assert!(skipped.skipped_busy());
        assert_eq!(skipped.account().id, account.id);
        let updated = HealthRefreshOutcome::Updated(account.clone());
        assert!(!updated.skipped_busy());
        assert_eq!(updated.account().display_name, "Test");
    }

    #[test]
    fn needs_verify_true_when_fields_missing() {
        let now = Utc.with_ymd_and_hms(2026, 7, 22, 12, 0, 0).unwrap();
        let expires = "2026-07-22T13:00:00+00:00";
        assert!(needs_verify(None, Some(expires), now));
        assert!(needs_verify(Some("ready"), None, now));
    }

    #[test]
    fn duplicate_new_account_is_rejected_without_catalog_mutation() {
        let app_ref = Uuid::new_v4();
        let account_id = Uuid::new_v4();
        let mut catalog = Catalog::default();
        catalog.accounts.push(AccountRecord {
            id: account_id,
            app_ref,
            user_open_id: "ou_existing".into(),
            display_name: "Existing User".into(),
            alias: None,
            tenant_label: None,
            config_dir: PathBuf::from("existing"),
            credential_origin: CredentialOrigin::Managed,
            health: AccountHealth::Ready,
            effective_scopes: BTreeSet::new(),
            last_verified_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        });
        let before = serde_json::to_vec(&catalog).unwrap();

        let error = reject_duplicate_new_account(&catalog, app_ref, "ou_existing").unwrap_err();

        assert_eq!(error.stable_code(), "LPC_ACCOUNT_ALREADY_EXISTS");
        assert!(error.to_string().contains(&account_id.to_string()));
        assert_eq!(serde_json::to_vec(&catalog).unwrap(), before);
    }

    #[test]
    fn rejects_plaintext_app_secret() {
        let value = json!({
            "apps": [{"appId":"cli_a", "appSecret":"secret", "brand":"feishu", "users":[]}]
        });
        assert!(sanitize_official_config(&value).is_err());
    }

    #[test]
    fn rejects_keychain_reference_with_embedded_plaintext() {
        let value = json!({
            "apps": [{
                "appId":"cli_a",
                "appSecret":{
                    "source":"keychain",
                    "id":"appsecret:cli_a",
                    "value":"plaintext-must-not-survive"
                },
                "brand":"feishu",
                "users":[]
            }]
        });

        assert!(sanitize_official_config(&value).is_err());
    }

    #[test]
    fn rejects_nested_values_in_optional_metadata() {
        let value = json!({
            "strictMode":{"refreshToken":"plaintext-must-not-survive"},
            "apps": [{
                "appId":"cli_a",
                "appSecret":{"source":"keychain", "id":"appsecret:cli_a"},
                "brand":"feishu",
                "lang":{"accessToken":"plaintext-must-not-survive"},
                "users":[]
            }]
        });

        assert!(sanitize_official_config(&value).is_err());
    }

    #[test]
    fn sanitizes_users_without_reading_secret_value() {
        let value = json!({
            "currentApp":"anything",
            "apps": [{
                "name":"anything",
                "appId":"cli_a",
                "appSecret":{"source":"keychain", "id":"appsecret:cli_a"},
                "brand":"feishu",
                "users":[{"userOpenId":"ou_x", "userName":"x"}]
            }]
        });
        let sanitized = sanitize_official_config(&value).unwrap();
        assert_eq!(sanitized["currentApp"], "lpc");
        assert_eq!(sanitized["apps"][0]["users"], json!([]));
    }

    #[test]
    fn accepts_string_encoded_official_keychain_reference() {
        let value = json!({
            "apps": [{
                "appId":"cli_a",
                "appSecret":"keychain:appsecret:cli_a",
                "brand":"feishu",
                "users":[]
            }]
        });
        assert!(sanitize_official_config(&value).is_ok());
    }

    fn migration_config(users: Value) -> Value {
        json!({
            "apps": [{
                "appId":"cli_a",
                "appSecret":{"source":"keychain", "id":"appsecret:cli_a"},
                "brand":"feishu",
                "lang":"zh_cn",
                "users": users
            }]
        })
    }

    #[test]
    fn existing_account_discovery_does_not_invoke_official_cli() {
        let temp = tempfile::tempdir().unwrap();
        let config_dir = temp.path().join("source");
        fs::create_dir_all(&config_dir).unwrap();
        write_json_atomic(
            &config_dir.join("config.json"),
            &migration_config(json!([{
                "userOpenId": "ou_discovered",
                "userName": "Discovered User"
            }])),
        )
        .unwrap();
        let store = StateStore::new(AppPaths::new(temp.path().join("lpc")));
        store.initialize().unwrap();
        let service = AccountService::new(store, OfficialCli::new("must-not-run"));

        let candidate = service
            .discover_existing_account_config(&config_dir)
            .unwrap();

        assert_eq!(candidate.user_open_id, "ou_discovered");
        assert_eq!(candidate.health, AccountHealth::Unknown);
    }

    #[test]
    fn migration_requires_exactly_one_app_and_one_user() {
        let no_apps = json!({"apps": []});
        let two_apps = json!({
            "apps": [
                {"appId":"cli_a", "appSecret":{"source":"keychain", "id":"appsecret:cli_a"}, "users":[]},
                {"appId":"cli_b", "appSecret":{"source":"keychain", "id":"appsecret:cli_b"}, "users":[]}
            ]
        });
        let no_users = migration_config(json!([]));
        let two_users = migration_config(json!([
            {"userOpenId":"ou_a", "userName":"A"},
            {"userOpenId":"ou_b", "userName":"B"}
        ]));

        assert!(isolate_official_account_config(&no_apps, "ou_a").is_err());
        assert!(isolate_official_account_config(&two_apps, "ou_a").is_err());
        assert!(isolate_official_account_config(&no_users, "ou_a").is_err());
        assert!(isolate_official_account_config(&two_users, "ou_a").is_err());
    }

    #[test]
    fn isolates_only_verified_user_without_mutating_source() {
        let source = migration_config(json!([{
            "userOpenId":"ou_verified",
            "userName":"Alice",
            "accessToken":"must-not-be-copied",
            "refreshToken":"must-not-be-copied"
        }]));
        let before = source.clone();

        let isolated = isolate_official_account_config(&source, "ou_verified").unwrap();

        assert_eq!(source, before);
        assert_eq!(isolated["currentApp"], "lpc");
        assert_eq!(isolated["apps"][0]["users"].as_array().unwrap().len(), 1);
        assert_eq!(isolated["apps"][0]["users"][0]["userOpenId"], "ou_verified");
        assert_eq!(isolated["apps"][0]["users"][0]["userName"], "Alice");
        let serialized = serde_json::to_string(&isolated)
            .unwrap()
            .to_ascii_lowercase();
        assert!(!serialized.contains("accesstoken"));
        assert!(!serialized.contains("refreshtoken"));
        assert!(!serialized.contains("must-not-be-copied"));
    }

    #[test]
    fn candidate_dto_has_no_credential_fields() {
        let candidate = ExistingCliCandidate {
            config_dir: PathBuf::from("safe-config-dir"),
            app_id: "cli_a".into(),
            brand: Brand::Feishu,
            display_name: "Alice".into(),
            user_open_id: "ou_verified".into(),
            health: AccountHealth::Ready,
            already_imported: false,
        };

        let serialized = serde_json::to_string(&candidate)
            .unwrap()
            .to_ascii_lowercase();
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("accesstoken"));
        assert!(!serialized.contains("refreshtoken"));
        assert!(!serialized.contains("devicecode"));
    }

    #[test]
    fn existing_account_commit_is_idempotent_repairs_copy_and_preserves_app_label() {
        let temp = tempfile::tempdir().unwrap();
        let store = StateStore::new(AppPaths::new(temp.path().join("lpc")));
        store.initialize().unwrap();
        let service = AccountService::new(store.clone(), OfficialCli::new("unused-in-test"));
        let source = migration_config(json!([{
            "userOpenId":"ou_verified",
            "userName":"Alice"
        }]));
        let identity = migration_source_identity(&source).unwrap();
        let prepared = PreparedExistingAccount {
            config_dir: temp.path().join("source"),
            identity,
            sanitized_base: sanitize_official_config(&source).unwrap(),
            isolated_account: isolate_official_account_config(&source, "ou_verified").unwrap(),
            available_scopes: ["docs:read".to_owned()].into_iter().collect(),
            effective_scopes: ["docs:read".to_owned()].into_iter().collect(),
            display_name: "Alice".into(),
            health: AccountHealth::Ready,
        };

        let first = service
            .commit_existing_account("本机飞书", prepared.clone())
            .unwrap();
        assert!(!first.already_imported);
        assert_eq!(
            first.account.credential_origin,
            CredentialOrigin::ExternalShared
        );
        fs::remove_file(first.account.config_dir.join("config.json")).unwrap();

        let mut repaired = prepared;
        repaired.display_name = "Alice (verified)".into();
        let second = service
            .commit_existing_account("不应覆盖已有名称", repaired)
            .unwrap();

        assert!(second.already_imported);
        assert_eq!(second.app.id, first.app.id);
        assert_eq!(second.app.label, "本机飞书");
        assert_eq!(second.account.id, first.account.id);
        assert_eq!(second.account.display_name, "Alice (verified)");
        assert_eq!(
            second.account.credential_origin,
            CredentialOrigin::ExternalShared
        );
        assert!(second.account.config_dir.join("config.json").is_file());
        let catalog = store.load_catalog().unwrap();
        assert_eq!(catalog.apps.len(), 1);
        assert_eq!(catalog.accounts.len(), 1);
    }
}
