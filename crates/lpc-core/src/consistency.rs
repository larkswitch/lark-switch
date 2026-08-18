//! Cross-check between the catalog and what is actually on disk.
//!
//! On 2026-07-17 the catalog came back with fewer accounts than it went in
//! with. The isolated config directories were all still there — only the index
//! that pointed at them had shrunk. Nothing compared the two, so the loss went
//! unnoticed until someone tried to use a missing account.
//!
//! Orphaned directories are therefore the signal worth watching: a config
//! directory that no catalog entry claims means the index lost an entry, not
//! that a directory appeared from nowhere.
//!
//! This reports; it does not repair, and it does not abort. Silently recreating
//! an entry would invent an account with no credentials and paper over the
//! event this exists to catch. Aborting every command would be worse still —
//! the binaries needed to diagnose and fix the damage are the same ones that
//! would refuse to start, locking the user out of their own recovery path. So
//! the finding goes to the log, to `lpcctl doctor`, and to the desktop
//! diagnostics panel, and a human decides.

use crate::error::Result;
use crate::store::StateStore;
use std::path::Path;
use uuid::Uuid;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsistencyReport {
    /// Config directories no catalog entry claims. The catalog lost entries.
    pub orphan_account_dirs: Vec<String>,
    /// Accounts whose recorded config directory is gone. The data lost entries.
    pub accounts_missing_config: Vec<Uuid>,
    pub orphan_app_dirs: Vec<String>,
    pub apps_missing_config: Vec<Uuid>,
}

impl ConsistencyReport {
    pub fn is_consistent(&self) -> bool {
        self.orphan_account_dirs.is_empty()
            && self.accounts_missing_config.is_empty()
            && self.orphan_app_dirs.is_empty()
            && self.apps_missing_config.is_empty()
    }

    /// One line, safe to put in a diagnostic summary.
    pub fn summary(&self) -> String {
        if self.is_consistent() {
            return "Catalog matches the directories on disk.".into();
        }
        format!(
            "orphanAccountDirs={}, accountsMissingConfig={}, orphanAppDirs={}, appsMissingConfig={}",
            self.orphan_account_dirs.len(),
            self.accounts_missing_config.len(),
            self.orphan_app_dirs.len(),
            self.apps_missing_config.len()
        )
    }
}

pub fn check_consistency(store: &StateStore) -> Result<ConsistencyReport> {
    let catalog = store.load_catalog()?;
    let paths = store.paths();

    let known_accounts: Vec<Uuid> = catalog.accounts.iter().map(|account| account.id).collect();
    let known_apps: Vec<Uuid> = catalog.apps.iter().map(|app| app.id).collect();

    let report = ConsistencyReport {
        orphan_account_dirs: orphan_dirs(&paths.accounts_dir(), &known_accounts),
        accounts_missing_config: catalog
            .accounts
            .iter()
            .filter(|account| !account.config_dir.join("config.json").is_file())
            .map(|account| account.id)
            .collect(),
        orphan_app_dirs: orphan_dirs(&paths.apps_dir(), &known_apps),
        apps_missing_config: catalog
            .apps
            .iter()
            .filter(|app| !app.base_config_path.is_file())
            .map(|app| app.id)
            .collect(),
    };
    Ok(report)
}

/// Directory names under `root` that parse as UUIDs but are absent from
/// `known`. Anything that is not a UUID directory belongs to something else and
/// is none of this check's business.
fn orphan_dirs(root: &Path, known: &[Uuid]) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut orphans: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .filter(|name| match Uuid::parse_str(name) {
            Ok(id) => !known.contains(&id),
            Err(_) => false,
        })
        .collect();
    orphans.sort();
    orphans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AccountHealth, AccountRecord, AppRecord, Brand, Catalog, CredentialOrigin};
    use crate::paths::AppPaths;
    use chrono::Utc;
    use std::collections::BTreeSet;
    use std::fs;
    use tempfile::TempDir;

    fn store_with_one_account() -> (TempDir, StateStore, Uuid, Uuid) {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path());
        let store = StateStore::new(paths.clone());
        store.initialize().unwrap();

        let now = Utc::now();
        let app_id = Uuid::new_v4();
        fs::create_dir_all(paths.app_dir(app_id)).unwrap();
        let base = paths.app_base_config(app_id);
        fs::write(&base, b"{}").unwrap();

        let account_id = Uuid::new_v4();
        let config_dir = paths.account_config_dir(account_id);
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(config_dir.join("config.json"), b"{}").unwrap();

        let mut catalog = Catalog::default();
        catalog.apps.push(AppRecord {
            id: app_id,
            app_id: "cli_test".into(),
            label: "Test App".into(),
            brand: Brand::Feishu,
            base_config_path: base,
            available_scopes: BTreeSet::new(),
            policy_scopes: BTreeSet::new(),
            scopes_observed_at: Some(now),
            created_at: now,
            updated_at: now,
        });
        catalog.accounts.push(AccountRecord {
            id: account_id,
            app_ref: app_id,
            user_open_id: "ou_test".into(),
            display_name: "Test".into(),
            alias: None,
            tenant_label: None,
            config_dir,
            credential_origin: CredentialOrigin::Managed,
            health: AccountHealth::Ready,
            effective_scopes: BTreeSet::new(),
            last_verified_at: Some(now),
            created_at: now,
            updated_at: now,
        });
        store.save_catalog(&catalog).unwrap();
        (temp, store, app_id, account_id)
    }

    #[test]
    fn a_matching_layout_reports_nothing() {
        let (_temp, store, _app, _account) = store_with_one_account();
        let report = check_consistency(&store).unwrap();
        assert!(report.is_consistent(), "unexpected findings: {report:?}");
    }

    #[test]
    fn an_account_dropped_from_the_catalog_shows_up_as_an_orphan_directory() {
        let (_temp, store, _app, account) = store_with_one_account();

        // Exactly the 2026-07-17 shape: the directory survives, the index entry
        // does not.
        let mut catalog = store.load_catalog().unwrap();
        catalog.accounts.clear();
        store.save_catalog(&catalog).unwrap();

        let report = check_consistency(&store).unwrap();
        assert_eq!(report.orphan_account_dirs, vec![account.to_string()]);
        assert!(report.accounts_missing_config.is_empty());
        assert!(!report.is_consistent());
    }

    #[test]
    fn an_account_whose_config_vanished_is_reported_separately() {
        let (_temp, store, _app, account) = store_with_one_account();
        let catalog = store.load_catalog().unwrap();
        fs::remove_dir_all(&catalog.accounts[0].config_dir).unwrap();

        let report = check_consistency(&store).unwrap();
        assert_eq!(report.accounts_missing_config, vec![account]);
        assert!(
            report.orphan_account_dirs.is_empty(),
            "a removed directory must not also count as an orphan"
        );
    }

    #[test]
    fn an_app_dropped_from_the_catalog_is_caught_too() {
        let (_temp, store, app, _account) = store_with_one_account();
        let mut catalog = store.load_catalog().unwrap();
        catalog.apps.clear();
        store.save_catalog(&catalog).unwrap();

        let report = check_consistency(&store).unwrap();
        assert_eq!(report.orphan_app_dirs, vec![app.to_string()]);
    }

    #[test]
    fn unrelated_directories_are_left_alone() {
        let (_temp, store, _app, _account) = store_with_one_account();
        fs::create_dir_all(store.paths().accounts_dir().join("not-a-uuid")).unwrap();

        let report = check_consistency(&store).unwrap();
        assert!(
            report.is_consistent(),
            "a directory that is not a UUID is somebody else's business: {report:?}"
        );
    }
}
