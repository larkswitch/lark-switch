//! Core control plane for Lark Profile Console.
//!
//! Security boundary: this crate never reads user access tokens, refresh tokens,
//! or App Secret values from the operating-system keychain. All credential
//! operations are delegated to the official `lark-cli` binary.

pub mod account;
pub mod app_creation;
pub mod atomic;
pub mod auth_flow;
pub mod autostart;
pub mod backup;
pub mod cli;
pub mod consistency;
pub mod diagnostics;
pub mod error;
pub mod host_bridge;
pub mod keychain_guard;
pub mod keychain_view;
pub mod keychain_watch;
pub mod locking;
pub mod logging;
pub mod model;
pub mod msix;
pub mod path_takeover;
pub mod paths;
pub mod redact;
pub mod runtime;
pub mod scope;
pub mod scope_policy;
pub mod selector;
pub mod shim;
pub mod store;

pub use account::{
    default_official_config_dirs, AccountService, ExistingAccountImport, ExistingCliCandidate,
    HealthRefreshOutcome, ImportedApp,
};
pub use app_creation::{AppCreationCoordinator, AppCreationProgress, AppCreationStart};
pub use auth_flow::{AuthCoordinator, AuthFlowStart, AuthProgress};
pub use autostart::{
    autostart_points_at_install, autostart_uses_cargo_target, expected_installed_desktop_exe,
    is_cargo_target_build_exe, is_packaged_app_virtualized_exe, list_desktop_run_entries,
    pin_user_run_autostart, AUTOSTART_VALUE_NAME, DESKTOP_EXE_FILE_NAME,
};
pub use backup::{
    default_backup_root, list_backups, restore_from_backup, restore_latest, run_credential_backup,
    BackupReport, BackupSnapshot, BackupSourceReport, RestoreReport,
};
pub use cli::{AuthScopes, AuthStatus, CliJson, OfficialCli, SecretString, WhoAmI};
pub use consistency::{check_consistency, ConsistencyReport};
pub use diagnostics::{DiagnosticCheck, DiagnosticReport, DiagnosticStatus};
pub use error::{LpcError, Result};
pub use host_bridge::{execute_via_host_bridge, start_host_bridge, HostBridgeResponse};
pub use keychain_guard::{
    backup_keychain_registry, default_keychain_backup_dir, ensure_keychain_snapshot_if_stale,
    inspect_keychain, KeychainBackupReport, KeychainStatus,
};
pub use keychain_view::{
    enforce_host_keychain_view, ensure_host_keychain_view, inspect_host_keychain_view,
    KeychainViewKind, KeychainViewStatus,
};
pub use keychain_watch::{
    classify_keychain_delta, expected_keychain_slots, force_verify_for_health, is_mass_cliff,
    observe_keychain_slots, KeychainWatchEvent, KeychainWatchKind,
};
pub use locking::{
    cli_keychain_lock_path, try_acquire_cli_keychain_lock, CliKeychainGuard, ExecutionLease,
    RouteSnapshot, RoutingGate, SingletonLock, CLI_KEYCHAIN_LOCK_NAME,
};
pub use logging::init_file_logging;
pub use model::*;
pub use msix::{enforce_msix_shim_policy, is_running_in_msix_package};
pub use path_takeover::{
    check_data_root_consistency, show_blocking_message, DataRootConsistency, PathTakeover,
    PathTakeoverReport,
};
pub use paths::AppPaths;
pub use redact::{contains_likely_secret, redact_text, redact_with, RedactionLevel};
pub use runtime::{ReleaseAsset, RuntimeManager};
pub use scope::{AuthorizationPlan, ScopeBatch, ScopePlanner};
pub use scope_policy::{
    core_scope_allowlist, default_policy, exclusion_reason, scope_catalog, ScopeInfo,
    MAX_SINGLE_AUTH_SCOPES,
};
pub use selector::{
    assert_summary_safe, parse_selector, resolve_account, resolve_execution_override,
    search_accounts, strip_leading_lpc_flags, summarize_account, summarize_views, validate_alias,
    AccountCandidateRef, AccountSummary, IdentitySelector, LpcArgv, ParsedSelector, SearchFilter,
};
pub use shim::{install_managed_shim, install_managed_shim_with, ShimInstallOptions};
pub use store::StateStore;

pub const SUPPORTED_CLI_VERSION: &str = "1.0.86";
/// Recommended plus previously shipped versions still accepted at runtime.
pub const SUPPORTED_CLI_VERSIONS: &[&str] = &["1.0.86", "1.0.71", "1.0.68"];
pub const SCHEMA_VERSION: u32 = 1;
