use crate::atomic::write_json_atomic;
use crate::error::{LpcError, Result};
use crate::locking::RoutingGate;
use crate::model::{AccountView, ActiveState, Catalog, ControlPlaneSnapshot};
use crate::paths::AppPaths;
use chrono::Utc;
use semver::Version;
use std::fs;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct StateStore {
    paths: AppPaths,
}

impl StateStore {
    pub fn new(paths: AppPaths) -> Self {
        Self { paths }
    }

    pub fn paths(&self) -> &AppPaths {
        &self.paths
    }

    /// Prepares the on-disk layout and runs startup migrations.
    ///
    /// Every catalog mutation here happens *inside the routing gate* and reloads
    /// the catalog after taking the lock. Historically this ran unlocked, so a
    /// second process (a different app version, or a concurrent `lpcctl`) could
    /// have its freshly written accounts clobbered by a stale in-memory copy —
    /// a direct cause of "my profiles disappeared". Callers must never hold the
    /// routing gate when calling `initialize`.
    pub fn initialize(&self) -> Result<()> {
        self.paths.ensure_layout()?;
        if !self.paths.catalog_file().exists() {
            write_json_atomic(&self.paths.catalog_file(), &Catalog::default())?;
        }
        if !self.paths.active_state_file().exists() {
            write_json_atomic(&self.paths.active_state_file(), &ActiveState::default())?;
        }

        let gate = RoutingGate::new(self.paths.clone());
        let _guard = gate.lock()?;
        // Reload under the lock so we never persist a copy that predates a
        // concurrent writer.
        let mut catalog = self.load_catalog()?;
        let mut changed = crate::scope_policy::normalize_catalog(&mut catalog);
        changed |= migrate_recommended_cli_version(&mut catalog.settings.recommended_cli_version);
        if changed {
            self.save_catalog(&catalog)?;
        }
        drop(_guard);

        // Soft-heal a stale managed CLI path when an allowlisted binary is still on disk.
        let _ = crate::runtime::recover_missing_managed_cli(self);

        self.report_consistency();
        Ok(())
    }

    /// Compares the catalog against the directories on disk and records the
    /// verdict. Never repairs and never fails the caller: see the reasoning in
    /// `crate::consistency`.
    fn report_consistency(&self) {
        let Ok(report) = crate::consistency::check_consistency(self) else {
            return;
        };
        if report.is_consistent() {
            tracing::debug!("catalog consistent with disk");
            return;
        }
        tracing::error!(
            orphan_account_dirs = report.orphan_account_dirs.len(),
            accounts_missing_config = report.accounts_missing_config.len(),
            orphan_app_dirs = report.orphan_app_dirs.len(),
            apps_missing_config = report.apps_missing_config.len(),
            "catalog does not match the directories on disk"
        );
    }

    pub fn load_catalog(&self) -> Result<Catalog> {
        let path = self.paths.catalog_file();
        if !path.exists() {
            return Err(LpcError::NotInitialized);
        }
        let bytes = fs::read(path)?;
        let catalog: Catalog = serde_json::from_slice(&bytes)?;
        if catalog.schema_version != crate::SCHEMA_VERSION {
            return Err(schema_mismatch(
                "catalog",
                catalog.schema_version,
                crate::SCHEMA_VERSION,
            ));
        }
        Ok(catalog)
    }

    pub fn save_catalog(&self, catalog: &Catalog) -> Result<()> {
        write_json_atomic(&self.paths.catalog_file(), catalog)?;
        // Every catalog write funnels through here, so the sequence of these
        // records is the answer to "when did seven accounts become two"
        // (2026-07-17), which nothing on disk could answer at the time.
        tracing::info!(
            apps = catalog.apps.len(),
            accounts = catalog.accounts.len(),
            "catalog saved"
        );
        Ok(())
    }

    pub fn path_takeover_enabled(&self) -> Result<bool> {
        Ok(self.load_catalog()?.settings.path_takeover_enabled)
    }

    pub fn set_path_takeover_enabled(&self, enabled: bool) -> Result<()> {
        let gate = RoutingGate::new(self.paths.clone());
        let _guard = gate.lock()?;
        let mut catalog = self.load_catalog()?;
        if catalog.settings.path_takeover_enabled == enabled {
            return Ok(());
        }
        catalog.settings.path_takeover_enabled = enabled;
        self.save_catalog(&catalog)
    }

    pub fn load_state(&self) -> Result<ActiveState> {
        let path = self.paths.active_state_file();
        if !path.exists() {
            return Err(LpcError::NotInitialized);
        }
        let bytes = fs::read(path)?;
        let state: ActiveState = serde_json::from_slice(&bytes)?;
        if state.schema_version != crate::SCHEMA_VERSION {
            return Err(schema_mismatch(
                "active-state",
                state.schema_version,
                crate::SCHEMA_VERSION,
            ));
        }
        Ok(state)
    }

    pub fn save_state(&self, state: &ActiveState) -> Result<()> {
        write_json_atomic(&self.paths.active_state_file(), state)?;
        // Which identity the shim hands to the next `lark-cli` call, and the
        // generation that stamps it. Without this, a command run under the
        // wrong account leaves no trace that the route had moved.
        let active = state.active_account_id.map(|id| id.to_string());
        tracing::info!(
            active_account_id = active.as_deref().unwrap_or("none"),
            generation = state.generation,
            "active route saved"
        );
        Ok(())
    }

    pub fn switch_active_account(&self, account_id: Uuid) -> Result<ActiveState> {
        let catalog = self.load_catalog()?;
        if !catalog
            .accounts
            .iter()
            .any(|account| account.id == account_id)
        {
            return Err(LpcError::AccountNotFound(account_id.to_string()));
        }
        let mut state = self.load_state()?;
        state.active_account_id = Some(account_id);
        state.generation = state.generation.saturating_add(1);
        state.updated_at = Utc::now();
        self.save_state(&state)?;
        Ok(state)
    }

    pub fn snapshot(
        &self,
        running: &std::collections::HashMap<Uuid, usize>,
    ) -> Result<ControlPlaneSnapshot> {
        let catalog = self.load_catalog()?;
        let state = self.load_state()?;
        let mut accounts = Vec::with_capacity(catalog.accounts.len());
        for account in &catalog.accounts {
            let app = catalog
                .apps
                .iter()
                .find(|app| app.id == account.app_ref)
                .ok_or_else(|| LpcError::AppNotFound(account.app_ref.to_string()))?
                .clone();
            accounts.push(AccountView {
                account: account.clone(),
                app,
                active: state.active_account_id == Some(account.id),
                running_commands: running.get(&account.id).copied().unwrap_or(0),
            });
        }
        Ok(ControlPlaneSnapshot {
            state,
            settings: catalog.settings.clone(),
            accounts,
            apps: catalog.apps,
        })
    }
}

/// Builds a clear, fail-closed error for a schema version we cannot handle.
///
/// The caller returns this *without touching the file*, so a catalog written by
/// a newer app version is preserved intact instead of being overwritten by an
/// older build (a version-mismatch data-loss path).
fn schema_mismatch(kind: &str, found: u32, supported: u32) -> LpcError {
    if found > supported {
        LpcError::Integrity(format!(
            "{kind} schema {found} is newer than this build supports ({supported}). \
             Update Lark Profile Console to the latest version. \
             The file was left untouched so no data is lost."
        ))
    } else {
        LpcError::Integrity(format!(
            "{kind} schema {found} is older than expected ({supported}) and cannot be read. \
             The file was left untouched."
        ))
    }
}

/// Migrates the stored `recommended_cli_version` hint toward this build's
/// supported version, but only ever *upgrades* it. An older app version must not
/// downgrade the hint a newer version wrote, otherwise two co-installed versions
/// ping-pong the field and rewrite the catalog on every launch. Returns whether
/// the value changed.
fn migrate_recommended_cli_version(stored: &mut String) -> bool {
    let current = crate::SUPPORTED_CLI_VERSION;
    if stored == current {
        return false;
    }
    let should_update = match (Version::parse(stored), Version::parse(current)) {
        (Ok(existing), Ok(supported)) => existing < supported,
        // An unparseable stored value is corrupt; normalize it to the known-good one.
        _ => true,
    };
    if should_update {
        *stored = current.to_owned();
    }
    should_update
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atomic::write_json_atomic;
    use crate::model::{AppRecord, Brand, Catalog};
    use chrono::Utc;
    use std::collections::BTreeSet;

    #[test]
    fn initialize_preserves_existing_advanced_policy() {
        let temp = tempfile::tempdir().unwrap();
        let store = StateStore::new(AppPaths::new(temp.path().join("lpc")));
        store.initialize().unwrap();
        let now = Utc::now();
        let advanced = ["directory:employee:read".to_owned()]
            .into_iter()
            .collect::<BTreeSet<_>>();
        let mut catalog = Catalog::default();
        catalog.apps.push(AppRecord {
            id: Uuid::new_v4(),
            app_id: "cli_fixture".into(),
            label: "fixture".into(),
            brand: Brand::Feishu,
            base_config_path: temp.path().join("config.json"),
            available_scopes: [
                "cardkit:card:read".to_owned(),
                "directory:employee:read".to_owned(),
            ]
            .into_iter()
            .collect::<BTreeSet<_>>(),
            policy_scopes: advanced.clone(),
            scopes_observed_at: Some(now),
            created_at: now,
            updated_at: now,
        });
        write_json_atomic(&store.paths().catalog_file(), &catalog).unwrap();

        store.initialize().unwrap();

        let migrated = store.load_catalog().unwrap();
        assert_eq!(migrated.apps[0].policy_scopes, advanced);
    }

    #[test]
    fn initialize_seeds_default_when_policy_is_empty() {
        let temp = tempfile::tempdir().unwrap();
        let store = StateStore::new(AppPaths::new(temp.path().join("lpc")));
        store.initialize().unwrap();
        let now = Utc::now();
        let mut catalog = Catalog::default();
        catalog.apps.push(AppRecord {
            id: Uuid::new_v4(),
            app_id: "cli_fixture".into(),
            label: "fixture".into(),
            brand: Brand::Feishu,
            base_config_path: temp.path().join("config.json"),
            available_scopes: [
                "cardkit:card:read".to_owned(),
                "directory:employee:read".to_owned(),
            ]
            .into_iter()
            .collect::<BTreeSet<_>>(),
            policy_scopes: BTreeSet::new(),
            scopes_observed_at: Some(now),
            created_at: now,
            updated_at: now,
        });
        write_json_atomic(&store.paths().catalog_file(), &catalog).unwrap();

        store.initialize().unwrap();

        let migrated = store.load_catalog().unwrap();
        assert_eq!(
            migrated.apps[0].policy_scopes,
            ["cardkit:card:read".to_owned()].into_iter().collect()
        );
    }

    #[test]
    fn initialize_preserves_user_core_policy_subset() {
        let temp = tempfile::tempdir().unwrap();
        let store = StateStore::new(AppPaths::new(temp.path().join("lpc")));
        store.initialize().unwrap();
        let now = Utc::now();
        let user_subset = ["cardkit:card:read".to_owned(), "docs:doc".to_owned()]
            .into_iter()
            .collect::<BTreeSet<_>>();
        let mut catalog = Catalog::default();
        catalog.apps.push(AppRecord {
            id: Uuid::new_v4(),
            app_id: "cli_fixture".into(),
            label: "fixture".into(),
            brand: Brand::Feishu,
            base_config_path: temp.path().join("config.json"),
            available_scopes: [
                "cardkit:card:read".to_owned(),
                "contact:user:search".to_owned(),
                "docs:doc".to_owned(),
            ]
            .into_iter()
            .collect::<BTreeSet<_>>(),
            policy_scopes: user_subset.clone(),
            scopes_observed_at: Some(now),
            created_at: now,
            updated_at: now,
        });
        write_json_atomic(&store.paths().catalog_file(), &catalog).unwrap();

        store.initialize().unwrap();

        let migrated = store.load_catalog().unwrap();
        assert_eq!(migrated.apps[0].policy_scopes, user_subset);
    }

    #[test]
    fn initialize_migrates_recommended_cli_version() {
        let temp = tempfile::tempdir().unwrap();
        let store = StateStore::new(AppPaths::new(temp.path().join("lpc")));
        store.initialize().unwrap();
        let mut catalog = store.load_catalog().unwrap();
        catalog.settings.recommended_cli_version = "1.0.68".into();
        write_json_atomic(&store.paths().catalog_file(), &catalog).unwrap();

        store.initialize().unwrap();

        let migrated = store.load_catalog().unwrap();
        assert_eq!(
            migrated.settings.recommended_cli_version,
            crate::SUPPORTED_CLI_VERSION
        );
    }

    #[test]
    fn recommended_cli_version_migration_only_upgrades() {
        // Older stored value upgrades to the current build's version.
        let mut older = "1.0.68".to_owned();
        assert!(migrate_recommended_cli_version(&mut older));
        assert_eq!(older, crate::SUPPORTED_CLI_VERSION);

        // A newer stored value (written by a future build) is NOT downgraded.
        let mut newer = "9.9.9".to_owned();
        assert!(!migrate_recommended_cli_version(&mut newer));
        assert_eq!(newer, "9.9.9");

        // Equal is a no-op.
        let mut same = crate::SUPPORTED_CLI_VERSION.to_owned();
        assert!(!migrate_recommended_cli_version(&mut same));
    }

    #[test]
    fn load_catalog_rejects_missing_accounts_instead_of_emptying() {
        let temp = tempfile::tempdir().unwrap();
        let store = StateStore::new(AppPaths::new(temp.path().join("lpc")));
        store.initialize().unwrap();
        // A syntactically valid catalog that is missing the `accounts` key must
        // NOT be silently accepted as "zero accounts"; it must fail loudly and
        // leave the file untouched.
        let foreign = r#"{ "schemaVersion": 1, "apps": [], "settings": {} }"#;
        write_json_atomic(&store.paths().catalog_file(), &()).unwrap();
        std::fs::write(store.paths().catalog_file(), foreign).unwrap();

        let result = store.load_catalog();
        assert!(result.is_err(), "missing accounts should fail to load");
    }

    #[test]
    fn load_catalog_rejects_newer_schema_without_touching_file() {
        let temp = tempfile::tempdir().unwrap();
        let store = StateStore::new(AppPaths::new(temp.path().join("lpc")));
        store.initialize().unwrap();
        // `settings` is omitted so serde uses its default; the point of this test
        // is the schema-version gate, not settings parsing.
        let newer = r#"{ "schemaVersion": 999, "apps": [], "accounts": [] }"#;
        std::fs::write(store.paths().catalog_file(), newer).unwrap();

        let err = store.load_catalog().unwrap_err();
        assert_eq!(err.stable_code(), "LPC_INTEGRITY_FAILED");
        // The file must be preserved byte-for-byte (fail-closed).
        let after = std::fs::read_to_string(store.paths().catalog_file()).unwrap();
        assert!(after.contains("\"schemaVersion\": 999"));
    }

    #[test]
    fn initialize_is_idempotent_and_keeps_accounts() {
        let temp = tempfile::tempdir().unwrap();
        let store = StateStore::new(AppPaths::new(temp.path().join("lpc")));
        store.initialize().unwrap();
        let now = Utc::now();
        let mut catalog = Catalog::default();
        let app_id = Uuid::new_v4();
        catalog.apps.push(AppRecord {
            id: app_id,
            app_id: "cli_fixture".into(),
            label: "fixture".into(),
            brand: Brand::Feishu,
            base_config_path: temp.path().join("config.json"),
            available_scopes: ["cardkit:card:read".to_owned()]
                .into_iter()
                .collect::<BTreeSet<_>>(),
            policy_scopes: ["cardkit:card:read".to_owned()]
                .into_iter()
                .collect::<BTreeSet<_>>(),
            scopes_observed_at: Some(now),
            created_at: now,
            updated_at: now,
        });
        write_json_atomic(&store.paths().catalog_file(), &catalog).unwrap();

        // Two more initialize passes must never drop the account/app.
        store.initialize().unwrap();
        store.initialize().unwrap();

        let after = store.load_catalog().unwrap();
        assert_eq!(after.apps.len(), 1);
        assert_eq!(after.apps[0].app_id, "cli_fixture");
    }
}
