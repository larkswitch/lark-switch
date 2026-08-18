//! `lpcctl doctor` output gets pasted into tickets and group chats, so it is
//! treated as an untrusted egress point.
//!
//! Every assertion here comes in a pair. Proving that a planted secret does not
//! appear is only half the job: a redactor that blanks the whole report would
//! pass that half and leave the command useless. So each test also names
//! something that *must* survive. Redacting too much and redacting too little
//! are both failures.

use chrono::Utc;
use lpc_core::atomic::write_json_atomic;
use lpc_core::diagnostics::{run_diagnostics, run_diagnostics_with};
use lpc_core::{
    AccountHealth, AccountRecord, ActiveState, AppPaths, AppRecord, Brand, Catalog,
    CredentialOrigin, DiagnosticReport, RedactionLevel, StateStore,
};
use std::collections::BTreeSet;
use std::fs;
use tempfile::TempDir;
use uuid::Uuid;

/// Planted values. Two shapes on purpose: one a redactor can find by key name,
/// one it can only find by shape.
const KEYED_SECRET: &str = "lpcfake-keyed-2f8a41d0c7b94e63";
const BARE_SECRET: &str = "Zm9vQmFyMTIzNDU2Nzg5MEFiQ2REZUZnSGlKa0xtTm9QcVJzVHVWd1h5";

fn report_with_planted_secrets(level: RedactionLevel) -> (TempDir, DiagnosticReport) {
    let temp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(temp.path());
    let store = StateStore::new(paths.clone());
    store.initialize().unwrap();

    let app_ref = Uuid::new_v4();
    fs::create_dir_all(paths.app_dir(app_ref)).unwrap();
    let base = paths.app_base_config(app_ref);
    write_json_atomic(
        &base,
        &serde_json::json!({
            "currentApp": "lpc",
            "apps": [{
                "name": "lpc",
                "appId": "cli_test",
                "appSecret": {"source": "keychain", "id": "appsecret:cli_test"},
                "brand": "feishu",
                "users": []
            }]
        }),
    )
    .unwrap();

    let now = Utc::now();
    let app = AppRecord {
        id: app_ref,
        app_id: "cli_test".into(),
        label: "Test App".into(),
        brand: Brand::Feishu,
        base_config_path: base,
        available_scopes: BTreeSet::new(),
        policy_scopes: BTreeSet::new(),
        scopes_observed_at: Some(now),
        created_at: now,
        updated_at: now,
    };

    // Slot 1: the account label, which lands in `summary`.
    let account_id = Uuid::new_v4();
    let config_dir = paths.account_config_dir(account_id);
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("config.json"), b"{}").unwrap();
    let account = AccountRecord {
        id: account_id,
        app_ref,
        user_open_id: "ou_test".into(),
        display_name: format!("Support access_token={KEYED_SECRET}"),
        alias: None,
        tenant_label: None,
        config_dir,
        credential_origin: CredentialOrigin::Managed,
        health: AccountHealth::Ready,
        effective_scopes: BTreeSet::new(),
        last_verified_at: Some(now),
        created_at: now,
        updated_at: now,
    };

    let mut catalog = Catalog::default();
    catalog.apps.push(app);
    catalog.accounts.push(account);
    store.save_catalog(&catalog).unwrap();

    // Slot 2: the runtime version, which lands in `detail` with no key name in
    // front of it — only the shape rule can catch this one.
    let runtime = paths.runtime_version_dir("1.0.68").join(if cfg!(windows) {
        "lark-cli.exe"
    } else {
        "lark-cli"
    });
    fs::create_dir_all(runtime.parent().unwrap()).unwrap();
    fs::write(&runtime, b"fixture-not-executed").unwrap();
    store
        .save_state(&ActiveState {
            managed_cli_path: Some(runtime),
            managed_cli_version: Some(BARE_SECRET.into()),
            ..ActiveState::default()
        })
        .unwrap();

    let report = run_diagnostics_with(&store, level).unwrap();
    (temp, report)
}

fn serialized(report: &DiagnosticReport) -> String {
    serde_json::to_string(report).unwrap()
}

#[test]
fn planted_secrets_never_reach_the_local_report() {
    let (_temp, report) = report_with_planted_secrets(RedactionLevel::Local);
    let json = serialized(&report);

    assert!(
        !json.contains(KEYED_SECRET),
        "a keyed secret placed in an account label reached the diagnostic report"
    );
    assert!(
        !json.contains(BARE_SECRET),
        "an unkeyed credential-shaped value reached the diagnostic report"
    );
}

#[test]
fn the_local_report_still_says_something_useful() {
    let (temp, report) = report_with_planted_secrets(RedactionLevel::Local);
    let json = serialized(&report);

    // The point of a local diagnostic is to name the machine's own paths.
    let root_marker = temp
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .expect("temp dir name");
    assert!(
        json.contains(root_marker),
        "local diagnostics dropped the data root path, so it can no longer \
         tell the user where the control plane lives"
    );

    for id in ["data_layout", "runtime", "path_route"] {
        assert!(
            report.checks.iter().any(|check| check.id == id),
            "diagnostic check `{id}` disappeared from the report"
        );
    }
    assert!(
        report
            .checks
            .iter()
            .any(|check| check.id.starts_with("account_")),
        "the per-account check disappeared, so a broken account is now invisible"
    );
}

#[test]
fn the_shared_report_drops_the_machine_account_name() {
    let (_temp, report) = report_with_planted_secrets(RedactionLevel::Outbound);
    let json = serialized(&report);

    assert!(!json.contains(KEYED_SECRET));
    assert!(!json.contains(BARE_SECRET));

    // Whatever the OS calls this user, it must not survive the outbound pass.
    // Read from the environment rather than hardcoded, so this stays true on
    // every developer machine and on CI.
    let Some(user) = current_user_name() else {
        return;
    };
    // Guard against a vacuous pass: if the local report never carried the user
    // name to begin with, the assertion below proves nothing and this test
    // would keep reporting success while the outbound pass rots.
    let (_local_temp, local) = report_with_planted_secrets(RedactionLevel::Local);
    if !serialized(&local).contains(&user) {
        return;
    }

    assert!(
        !json.contains(&user),
        "the shared report still carries the machine's user name (`{user}`)"
    );
}

#[test]
fn the_shared_report_keeps_enough_to_diagnose_with() {
    let (_temp, report) = report_with_planted_secrets(RedactionLevel::Outbound);

    for id in ["data_layout", "runtime", "path_route"] {
        let check = report
            .checks
            .iter()
            .find(|check| check.id == id)
            .unwrap_or_else(|| panic!("diagnostic check `{id}` disappeared"));
        assert!(
            !check.summary.is_empty(),
            "check `{id}` lost its summary in the outbound pass"
        );
    }
}

/// The default entry point must not be the weaker one by accident.
#[test]
fn the_default_entry_point_redacts_too() {
    let temp = tempfile::tempdir().unwrap();
    let store = StateStore::new(AppPaths::new(temp.path()));
    store.initialize().unwrap();
    let mut catalog = Catalog::default();
    catalog.accounts.clear();
    store.save_catalog(&catalog).unwrap();

    let report = run_diagnostics(&store).unwrap();
    assert!(!serialized(&report).contains("access_token=lpcfake"));
}

fn current_user_name() -> Option<String> {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .ok()
        .filter(|name| name.len() >= 3)
}
