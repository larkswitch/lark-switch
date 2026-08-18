//! Persist the last observed official-CLI keychain slot count and classify
//! changes. LPC never reads secret values; this is a count-only tripwire.
//!
//! A drop from 15 slots to 4 is not "empty", so `inspect_keychain().empty`
//! stays false and used to stay silent. That is the 2026-08-13 signature:
//! the account list survived, the tokens did not.

use crate::atomic::write_json_atomic;
use crate::error::Result;
use crate::keychain_guard::KeychainStatus;
use crate::paths::AppPaths;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;

/// Two App Secret slots plus one user-token slot per catalog account.
pub fn expected_keychain_slots(account_count: usize) -> usize {
    account_count.saturating_add(2)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeychainWatchKind {
    FirstSight,
    Unchanged,
    Rose { from: usize, to: usize },
    /// Mass deletion relative to the last observation. Not the same as a
    /// single official-CLI revocation of one dead refresh token.
    Cliff { from: usize, to: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeychainWatchEvent {
    pub kind: KeychainWatchKind,
    pub current: usize,
    pub previous: Option<usize>,
    pub expected: usize,
    pub deficit: bool,
}

impl KeychainWatchEvent {
    pub fn should_skip_scheduled_verify(&self) -> bool {
        matches!(self.kind, KeychainWatchKind::Cliff { .. }) || self.current == 0
    }

    pub fn should_force_reauth_verify(&self) -> bool {
        matches!(self.kind, KeychainWatchKind::Rose { .. })
    }

    pub fn cliff_message(&self) -> Option<String> {
        match self.kind {
            KeychainWatchKind::Cliff { from, to } => Some(format!(
                "钥匙串槽位从 {from} 降到 {to}，账号名单还在。不要整表导入注册表。用 restore-lark-keychain.ps1 干跑对照。"
            )),
            _ if self.current == 0 => Some(
                "Official CLI keychain is EMPTY (tokens wiped). Restore from \
                 Documents\\LarkProfileConsoleBackups\\keychain or re-authorize."
                    .into(),
            ),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedWatch {
    entry_count: usize,
    observed_at: DateTime<Utc>,
}

pub fn classify_keychain_delta(
    previous: Option<usize>,
    current: usize,
    account_count: usize,
) -> KeychainWatchEvent {
    let expected = expected_keychain_slots(account_count);
    let deficit = current < expected;
    let kind = match previous {
        None => KeychainWatchKind::FirstSight,
        Some(from) if current == from => KeychainWatchKind::Unchanged,
        Some(from) if current > from => KeychainWatchKind::Rose { from, to: current },
        Some(from) if is_mass_cliff(from, current) => KeychainWatchKind::Cliff { from, to: current },
        Some(_) => KeychainWatchKind::Unchanged,
    };
    KeychainWatchEvent {
        kind,
        current,
        previous,
        expected,
        deficit,
    }
}

/// A one-slot drop is usually the official CLI deleting one revoked refresh
/// token. A drop of three or more, a halving, or a wipe is the 08-13 pattern.
pub fn is_mass_cliff(previous: usize, current: usize) -> bool {
    if previous == 0 || current >= previous {
        return false;
    }
    if current == 0 {
        return true;
    }
    let drop = previous - current;
    drop >= 3 || current * 2 < previous
}

pub fn load_keychain_watch(paths: &AppPaths) -> Result<Option<usize>> {
    let path = paths.keychain_watch_file();
    if !path.is_file() {
        return Ok(None);
    }
    let text = fs::read_to_string(path)?;
    let persisted: PersistedWatch = serde_json::from_str(&text)?;
    Ok(Some(persisted.entry_count))
}

pub fn save_keychain_watch(paths: &AppPaths, entry_count: usize) -> Result<()> {
    let persisted = PersistedWatch {
        entry_count,
        observed_at: Utc::now(),
    };
    write_json_atomic(&paths.keychain_watch_file(), &persisted)
}

/// Classify against the last persisted count. `persist` should be true for the
/// desktop process that owns the watch; `lpcctl doctor` reads without writing
/// so it cannot swallow a cliff the desktop has not seen yet.
pub fn observe_keychain_slots(
    paths: &AppPaths,
    status: &KeychainStatus,
    account_count: usize,
    persist: bool,
) -> Result<KeychainWatchEvent> {
    let previous = load_keychain_watch(paths)?;
    let event = classify_keychain_delta(previous, status.entry_count, account_count);
    if persist && status.platform_supported {
        save_keychain_watch(paths, status.entry_count)?;
    }
    match event.kind {
        KeychainWatchKind::Cliff { from, to } => {
            tracing::error!(from, to, expected = event.expected, "keychain slot cliff");
        }
        KeychainWatchKind::Rose { from, to } => {
            tracing::info!(from, to, "keychain slots recovered");
        }
        _ => {}
    }
    Ok(event)
}

pub fn force_verify_for_health(force_reauth_round: bool, health: &crate::AccountHealth) -> bool {
    force_reauth_round && matches!(health, crate::AccountHealth::ReauthRequired)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::AppPaths;

    #[test]
    fn expected_slots_are_accounts_plus_two_secrets() {
        assert_eq!(expected_keychain_slots(13), 15);
        assert_eq!(expected_keychain_slots(0), 2);
    }

    #[test]
    fn fifteen_to_four_is_a_mass_cliff() {
        let event = classify_keychain_delta(Some(15), 4, 13);
        assert_eq!(event.kind, KeychainWatchKind::Cliff { from: 15, to: 4 });
        assert!(event.deficit);
        assert!(event.should_skip_scheduled_verify());
        assert!(!event.should_force_reauth_verify());
        assert!(event.cliff_message().unwrap().contains("从 15 降到 4"));
    }

    #[test]
    fn empty_after_nonempty_is_a_cliff() {
        let event = classify_keychain_delta(Some(15), 0, 13);
        assert_eq!(event.kind, KeychainWatchKind::Cliff { from: 15, to: 0 });
        assert!(event.should_skip_scheduled_verify());
    }

    #[test]
    fn single_slot_drop_is_not_a_mass_cliff() {
        let event = classify_keychain_delta(Some(15), 14, 13);
        assert_eq!(event.kind, KeychainWatchKind::Unchanged);
        assert!(event.deficit);
        assert!(!event.should_skip_scheduled_verify());
    }

    #[test]
    fn rise_after_restore_forces_reauth_verify() {
        let event = classify_keychain_delta(Some(4), 15, 13);
        assert_eq!(event.kind, KeychainWatchKind::Rose { from: 4, to: 15 });
        assert!(event.should_force_reauth_verify());
        assert!(!event.should_skip_scheduled_verify());
        assert!(!event.deficit);
    }

    #[test]
    fn first_sight_of_a_short_keychain_is_not_a_cliff() {
        let event = classify_keychain_delta(None, 4, 13);
        assert_eq!(event.kind, KeychainWatchKind::FirstSight);
        assert!(event.deficit);
        assert!(!event.should_skip_scheduled_verify());
    }

    #[test]
    fn persist_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path());
        paths.ensure_layout().unwrap();
        assert_eq!(load_keychain_watch(&paths).unwrap(), None);
        save_keychain_watch(&paths, 15).unwrap();
        assert_eq!(load_keychain_watch(&paths).unwrap(), Some(15));
    }

    fn sample_status(count: usize) -> crate::keychain_guard::KeychainStatus {
        crate::keychain_guard::KeychainStatus {
            platform_supported: true,
            key_exists: count > 0,
            entry_count: count,
            empty: count == 0,
            detail: format!("{count} slots"),
        }
    }

    #[test]
    fn doctor_observe_does_not_persist_a_cliff() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path());
        paths.ensure_layout().unwrap();
        save_keychain_watch(&paths, 15).unwrap();
        let event = observe_keychain_slots(&paths, &sample_status(4), 13, false).unwrap();
        assert_eq!(event.kind, KeychainWatchKind::Cliff { from: 15, to: 4 });
        assert_eq!(load_keychain_watch(&paths).unwrap(), Some(15));
    }

    #[test]
    fn desktop_observe_persists_after_classifying_the_cliff() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path());
        paths.ensure_layout().unwrap();
        save_keychain_watch(&paths, 15).unwrap();
        let event = observe_keychain_slots(&paths, &sample_status(4), 13, true).unwrap();
        assert_eq!(event.kind, KeychainWatchKind::Cliff { from: 15, to: 4 });
        assert_eq!(load_keychain_watch(&paths).unwrap(), Some(4));
    }

    #[test]
    fn force_verify_only_reauth_on_rise() {
        use crate::AccountHealth;
        assert!(force_verify_for_health(true, &AccountHealth::ReauthRequired));
        assert!(!force_verify_for_health(true, &AccountHealth::Ready));
        assert!(!force_verify_for_health(
            false,
            &AccountHealth::ReauthRequired
        ));
    }
}
