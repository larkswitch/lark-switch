use chrono::Utc;
use lpc_core::{
    atomic::write_json_atomic, AccountHealth, AccountRecord, ActiveState, AppPaths, AppRecord,
    Brand, Catalog, CredentialOrigin, RoutingGate, StateStore,
};
use std::collections::BTreeSet;
use std::fs;
use tempfile::TempDir;
use uuid::Uuid;

/// A gate held by a live process used to block every later caller forever.
/// Since the shim takes this gate on every `lark-cli` invocation, one wedged
/// process meant a machine where nothing ran and nothing explained why.
#[test]
fn a_held_routing_gate_gives_up_instead_of_waiting_forever() {
    let temp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(temp.path());
    let store = StateStore::new(paths.clone());
    store.initialize().unwrap();

    let gate = RoutingGate::new(paths);
    let _held = gate.lock().expect("first acquisition");

    let started = std::time::Instant::now();
    let outcome = gate.lock_with_timeout(std::time::Duration::from_millis(200));
    let waited = started.elapsed();

    let Err(error) = outcome else {
        panic!("a gate already held must not be handed out twice");
    };
    assert_eq!(error.stable_code(), "LPC_ROUTING_GATE_BUSY");
    assert!(
        waited >= std::time::Duration::from_millis(150),
        "gave up before the timeout, so it was not really waiting: {waited:?}"
    );
    assert!(
        waited < std::time::Duration::from_secs(5),
        "waited far past the timeout: {waited:?}"
    );
}

#[test]
fn the_routing_gate_is_reusable_once_released() {
    let temp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(temp.path());
    StateStore::new(paths.clone()).initialize().unwrap();

    let gate = RoutingGate::new(paths);
    let held = gate.lock().unwrap();
    drop(held);
    gate.lock_with_timeout(std::time::Duration::from_millis(200))
        .expect("the gate must be acquirable again after the holder drops it");
}

fn fixture() -> (TempDir, StateStore, Uuid, Uuid) {
    let temp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(temp.path());
    let store = StateStore::new(paths.clone());
    store.initialize().unwrap();

    let app_ref = Uuid::new_v4();
    let app_id = "cli_test".to_owned();
    fs::create_dir_all(paths.app_dir(app_ref)).unwrap();
    let base = paths.app_base_config(app_ref);
    write_json_atomic(
        &base,
        &serde_json::json!({
            "currentApp": "lpc",
            "apps": [{
                "name": "lpc",
                "appId": app_id,
                "appSecret": {"source":"keychain", "id":"appsecret:cli_test"},
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
        available_scopes: ["docs:read".to_owned()].into_iter().collect(),
        policy_scopes: ["docs:read".to_owned()].into_iter().collect(),
        scopes_observed_at: Some(now),
        created_at: now,
        updated_at: now,
    };
    let account_a = Uuid::new_v4();
    let account_b = Uuid::new_v4();
    let accounts = [account_a, account_b]
        .into_iter()
        .enumerate()
        .map(|(index, id)| {
            let dir = paths.account_config_dir(id);
            fs::create_dir_all(&dir).unwrap();
            fs::copy(&app.base_config_path, dir.join("config.json")).unwrap();
            AccountRecord {
                id,
                app_ref,
                user_open_id: format!("ou_{index}"),
                display_name: format!("User {index}"),
                alias: None,
                tenant_label: None,
                config_dir: dir,
                credential_origin: CredentialOrigin::Managed,
                health: AccountHealth::Ready,
                effective_scopes: BTreeSet::new(),
                last_verified_at: Some(now),
                created_at: now,
                updated_at: now,
            }
        })
        .collect::<Vec<_>>();
    let mut catalog = Catalog::default();
    catalog.apps.push(app);
    catalog.accounts = accounts;
    store.save_catalog(&catalog).unwrap();

    let runtime = paths.runtime_version_dir("1.0.68").join(if cfg!(windows) {
        "lark-cli.exe"
    } else {
        "lark-cli"
    });
    fs::create_dir_all(runtime.parent().unwrap()).unwrap();
    fs::write(&runtime, b"fixture-not-executed").unwrap();
    let state = ActiveState {
        active_account_id: Some(account_a),
        managed_cli_path: Some(runtime),
        managed_cli_version: Some("1.0.68".into()),
        ..ActiveState::default()
    };
    store.save_state(&state).unwrap();
    (temp, store, account_a, account_b)
}

#[test]
fn a_running_command_keeps_its_snapshot_while_future_commands_switch() {
    let (_temp, store, account_a, account_b) = fixture();
    let gate = RoutingGate::new(store.paths().clone());

    let (old_route, old_lease) = gate.snapshot_for_execution(&store).unwrap();
    assert_eq!(old_route.account.id, account_a);
    assert_eq!(gate.running_for_account(account_a).unwrap(), 1);

    // This is the product's explicit semantics: a normal switch is allowed
    // while a previous command is running. The old command already owns an
    // immutable config-dir snapshot; only the next invocation sees B.
    gate.switch_account(&store, account_b).unwrap();
    let (new_route, new_lease) = gate.snapshot_for_execution(&store).unwrap();
    assert_eq!(new_route.account.id, account_b);
    assert_eq!(old_route.account.id, account_a);

    old_lease.release().unwrap();
    new_lease.release().unwrap();
    assert!(gate.running_counts().unwrap().is_empty());
}

#[test]
fn state_files_remain_valid_json_after_repeated_atomic_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.json");
    for generation in 0..100_u64 {
        let state = ActiveState {
            generation,
            ..ActiveState::default()
        };
        write_json_atomic(&path, &state).unwrap();
        let observed: ActiveState = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(observed.generation, generation);
    }
    let leftovers = fs::read_dir(temp.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
        .count();
    assert_eq!(leftovers, 0);
}

#[test]
fn snapshot_joins_accounts_with_apps_and_running_counts() {
    let (_temp, store, account_a, _) = fixture();
    let gate = RoutingGate::new(store.paths().clone());
    let (_route, lease) = gate.snapshot_for_execution(&store).unwrap();
    let counts = gate.running_counts().unwrap();
    let snapshot = store.snapshot(&counts).unwrap();
    assert_eq!(snapshot.accounts.len(), 2);
    assert_eq!(
        snapshot
            .accounts
            .iter()
            .find(|item| item.account.id == account_a)
            .unwrap()
            .running_commands,
        1
    );
    lease.release().unwrap();
}

#[test]
fn override_route_does_not_mutate_active_state_bytes_or_generation() {
    let (_temp, store, account_a, account_b) = fixture();
    let gate = RoutingGate::new(store.paths().clone());
    let before = std::fs::read(store.paths().active_state_file()).unwrap();
    let before_state = store.load_state().unwrap();
    assert_eq!(before_state.active_account_id, Some(account_a));

    let selector = format!("id:{account_b}");
    let (route, lease) = gate
        .snapshot_for_execution_with_override(&store, Some(&selector))
        .unwrap();
    assert_eq!(route.account.id, account_b);
    assert_eq!(route.generation, before_state.generation);

    let after = std::fs::read(store.paths().active_state_file()).unwrap();
    let after_state = store.load_state().unwrap();
    assert_eq!(before, after);
    assert_eq!(after_state.active_account_id, Some(account_a));
    assert_eq!(after_state.generation, before_state.generation);
    lease.release().unwrap();
}

#[test]
fn concurrent_leases_block_delete_while_busy() {
    let (_temp, store, account_a, account_b) = fixture();
    let gate = RoutingGate::new(store.paths().clone());
    let (route_a, lease_a) = gate.snapshot_for_execution(&store).unwrap();
    let (route_b, lease_b) = gate
        .snapshot_for_execution_with_override(&store, Some(&format!("id:{account_b}")))
        .unwrap();
    assert_eq!(route_a.account.id, account_a);
    assert_eq!(route_b.account.id, account_b);
    assert_eq!(gate.running_for_account(account_a).unwrap(), 1);
    assert_eq!(gate.running_for_account(account_b).unwrap(), 1);

    let busy = match gate.lock_account_idle(account_a) {
        Err(error) => error,
        Ok(_) => panic!("expected account busy while lease is held"),
    };
    assert_eq!(busy.stable_code(), "LPC_ACCOUNT_BUSY");

    lease_a.release().unwrap();
    let _guard = gate.lock_account_idle(account_a).unwrap();
    lease_b.release().unwrap();
}
