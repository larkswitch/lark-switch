#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use lpc_core::show_blocking_message;
use lpc_core::{
    check_data_root_consistency, default_official_config_dirs, diagnostics::run_diagnostics,
    ensure_keychain_snapshot_if_stale, force_verify_for_health, observe_keychain_slots,
    pin_user_run_autostart, run_credential_backup, start_host_bridge, AccountHealth,
    AccountService, AppCreationCoordinator, AppCreationProgress, AppCreationStart, AppPaths,
    AuthCoordinator, AuthFlowStart, AuthProgress, Brand, ControlPlaneSnapshot, DataRootConsistency,
    DiagnosticReport, ExistingAccountImport, ExistingCliCandidate, HealthRefreshOutcome,
    KeychainWatchKind, OfficialCli, PathTakeover, PathTakeoverReport, RoutingGate, RuntimeManager,
    SecretString, SingletonLock, StateStore,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use uuid::Uuid;

struct ServiceBundle {
    account: AccountService,
    /// Separate lock so long OAuth CLI work does not block account/tray paths
    /// that only need `account` or `RoutingGate`.
    auth: Arc<Mutex<AuthCoordinator>>,
    app_creation: AppCreationCoordinator,
    app_creation_begins: usize,
    runtime_change_active: bool,
}

struct DesktopState {
    paths: AppPaths,
    store: StateStore,
    services: Mutex<ServiceBundle>,
    /// Held for the whole process lifetime so a second desktop instance cannot
    /// race on the same data root. Never read; dropped on exit to release.
    _instance_lock: SingletonLock,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportAppRequest {
    label: String,
    app_id: String,
    app_secret: String,
    brand: Brand,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetPolicyRequest {
    app_ref: Uuid,
    scopes: BTreeSet<String>,
}

fn managed_cli(store: &StateStore) -> Result<OfficialCli, String> {
    let state = store.load_state().map_err(error_text)?;
    let path = state
        .managed_cli_path
        .ok_or_else(|| "Managed official lark-cli is not installed".to_owned())?;
    Ok(OfficialCli::new(path))
}

#[tauri::command]
fn snapshot(state: State<'_, DesktopState>) -> Result<ControlPlaneSnapshot, String> {
    let counts = lpc_core::RoutingGate::new(state.paths.clone())
        .running_counts()
        .map_err(error_text)?;
    state.store.snapshot(&counts).map_err(error_text)
}

#[tauri::command]
fn switch_account(
    app: AppHandle,
    state: State<'_, DesktopState>,
    account_id: Uuid,
) -> Result<ControlPlaneSnapshot, String> {
    state
        .services
        .lock()
        .map_err(|_| "Control service lock poisoned".to_owned())?
        .account
        .switch_account(account_id)
        .map_err(error_text)?;
    rebuild_tray(&app).map_err(|error| error.to_string())?;
    snapshot(state)
}

#[tauri::command]
fn install_runtime(
    app: AppHandle,
    state: State<'_, DesktopState>,
    version: String,
) -> Result<PathBuf, String> {
    begin_runtime_change(&state)?;
    let next_services = (|| {
        let manager = RuntimeManager::new(state.store.clone()).map_err(error_text)?;
        let result = manager.install(&version).map_err(error_text)?;
        if let Some(source) = sibling_shim() {
            let _ = install_shim(&source, &state.paths);
        }
        let account = AccountService::new(state.store.clone(), OfficialCli::new(result.clone()));
        let auth = Arc::new(Mutex::new(
            AuthCoordinator::new(account.clone()).map_err(error_text)?,
        ));
        let app_creation = AppCreationCoordinator::new(account.clone());
        Ok((
            result,
            ServiceBundle {
                account,
                auth,
                app_creation,
                app_creation_begins: 0,
                runtime_change_active: false,
            },
        ))
    })();
    let (result, next_services) = match next_services {
        Ok(value) => value,
        Err(error) => {
            clear_runtime_change(&state);
            return Err(error);
        }
    };
    *state
        .services
        .lock()
        .map_err(|_| "Control service lock poisoned".to_owned())? = next_services;
    let _ = app;
    Ok(result)
}

#[tauri::command]
fn rollback_runtime(app: AppHandle, state: State<'_, DesktopState>) -> Result<PathBuf, String> {
    begin_runtime_change(&state)?;
    let next_services = (|| {
        let manager = RuntimeManager::new(state.store.clone()).map_err(error_text)?;
        let result = manager.rollback().map_err(error_text)?;
        let account = AccountService::new(state.store.clone(), OfficialCli::new(result.clone()));
        let auth = Arc::new(Mutex::new(
            AuthCoordinator::new(account.clone()).map_err(error_text)?,
        ));
        let app_creation = AppCreationCoordinator::new(account.clone());
        Ok((
            result,
            ServiceBundle {
                account,
                auth,
                app_creation,
                app_creation_begins: 0,
                runtime_change_active: false,
            },
        ))
    })();
    let (result, next_services) = match next_services {
        Ok(value) => value,
        Err(error) => {
            clear_runtime_change(&state);
            return Err(error);
        }
    };
    *state
        .services
        .lock()
        .map_err(|_| "Control service lock poisoned".to_owned())? = next_services;
    let _ = app;
    Ok(result)
}

fn begin_runtime_change(state: &DesktopState) -> Result<(), String> {
    let mut services = state
        .services
        .lock()
        .map_err(|_| "Control service lock poisoned".to_owned())?;
    if services.runtime_change_active {
        return Err("A CLI runtime change is already in progress".into());
    }
    let auth_active = services
        .auth
        .lock()
        .map_err(|_| "Auth service lock poisoned".to_owned())?
        .active_flow_count();
    if auth_active > 0
        || services.app_creation.active_flow_count() > 0
        || services.app_creation_begins > 0
    {
        return Err(
            "Finish or cancel active OAuth and App creation flows before changing the CLI runtime"
                .into(),
        );
    }
    services.runtime_change_active = true;
    Ok(())
}

fn clear_runtime_change(state: &DesktopState) {
    if let Ok(mut services) = state.services.lock() {
        services.runtime_change_active = false;
    }
}

#[tauri::command]
fn install_path_takeover(state: State<'_, DesktopState>) -> Result<PathTakeoverReport, String> {
    let source = sibling_shim().ok_or_else(|| {
        "The packaged lark-cli shim is missing beside the desktop executable".to_owned()
    })?;
    install_shim(&source, &state.paths)?;
    state
        .store
        .set_path_takeover_enabled(true)
        .map_err(error_text)?;
    PathTakeover::new(state.paths.clone())
        .install()
        .map_err(error_text)
}

#[tauri::command]
fn remove_path_takeover(state: State<'_, DesktopState>) -> Result<PathTakeoverReport, String> {
    state
        .store
        .set_path_takeover_enabled(false)
        .map_err(error_text)?;
    PathTakeover::new(state.paths.clone())
        .uninstall()
        .map_err(error_text)
}

fn is_cargo_target_build_exe(path: &Path) -> bool {
    let parts: Vec<_> = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => {
                Some(value.to_string_lossy().to_ascii_lowercase())
            }
            _ => None,
        })
        .collect();
    parts.iter().enumerate().any(|(index, part)| {
        part == "target"
            && parts
                .get(index + 1..index + 3)
                .into_iter()
                .flatten()
                .any(|next| *next == "debug" || *next == "release")
    })
}

fn is_packaged_app_virtualized_exe(path: &Path) -> bool {
    let parts: Vec<_> = path
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => {
                Some(value.to_string_lossy().to_ascii_lowercase())
            }
            _ => None,
        })
        .collect();
    parts
        .windows(4)
        .any(|window| window[0] == "packages" && window[2] == "localcache" && window[3] == "local")
}

fn ensure_autostart_exe_is_install(path: &Path) -> Result<(), String> {
    if is_cargo_target_build_exe(path) {
        return Err(
            "Autostart can only register the installed application executable, not a cargo target build"
                .to_owned(),
        );
    }
    if is_packaged_app_virtualized_exe(path) {
        return Err(
            "Autostart cannot register a packaged-app virtualized copy; launch the installed application first"
                .to_owned(),
        );
    }
    Ok(())
}

fn ensure_installed_autostart(app: &AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    // Development and packaged-agent virtualized copies must never replace the real install entry.
    if ensure_autostart_exe_is_install(&exe).is_err() {
        return Ok(());
    }
    app.autolaunch()
        .enable()
        .map_err(|error| error.to_string())?;
    if let Err(error) = pin_user_run_autostart(&exe, &["--hidden"]) {
        tracing::error!(%error, "failed to pin HKCU Run autostart to the installed exe");
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeIdentity {
    version: String,
    exe_path: PathBuf,
}

#[tauri::command]
fn runtime_identity() -> Result<RuntimeIdentity, String> {
    Ok(RuntimeIdentity {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        exe_path: std::env::current_exe().map_err(|error| error.to_string())?,
    })
}

#[tauri::command]
fn autostart_status(app: AppHandle) -> Result<bool, String> {
    app.autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    let manager = app.autolaunch();
    if enabled {
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        ensure_autostart_exe_is_install(&exe)?;
        manager.enable().map_err(|error| error.to_string())?;
        if let Err(error) = pin_user_run_autostart(&exe, &["--hidden"]) {
            tracing::error!(%error, "failed to pin HKCU Run autostart after enabling");
        }
    } else {
        manager.disable().map_err(|error| error.to_string())?;
    }
    manager.is_enabled().map_err(|error| error.to_string())
}

#[tauri::command]
fn import_existing_app(
    state: State<'_, DesktopState>,
    request: ImportAppRequest,
) -> Result<lpc_core::AppRecord, String> {
    let secret = SecretString::new(request.app_secret);
    state
        .services
        .lock()
        .map_err(|_| "Control service lock poisoned".to_owned())?
        .account
        .import_existing_app(&request.label, &request.app_id, secret, request.brand)
        .map_err(error_text)
}

#[tauri::command]
fn discover_existing_configs(
    state: State<'_, DesktopState>,
) -> Result<Vec<ExistingCliCandidate>, String> {
    let account = state
        .services
        .lock()
        .map_err(|_| "Control service lock poisoned".to_owned())?
        .account
        .clone();
    default_official_config_dirs()
        .into_iter()
        .map(|config_dir| {
            account
                .discover_existing_account_config(&config_dir)
                .map_err(|error| format!("{}: {}", config_dir.display(), error_text(error)))
        })
        .collect()
}

#[tauri::command]
async fn inspect_existing_config(
    state: State<'_, DesktopState>,
    config_dir: PathBuf,
) -> Result<ExistingCliCandidate, String> {
    let account = state
        .services
        .lock()
        .map_err(|_| "Control service lock poisoned".to_owned())?
        .account
        .clone();
    tauri::async_runtime::spawn_blocking(move || {
        account.inspect_existing_account_config(&config_dir)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(error_text)
}

#[tauri::command]
async fn import_existing_account_config(
    app: AppHandle,
    state: State<'_, DesktopState>,
    label: String,
    config_dir: PathBuf,
) -> Result<ExistingAccountImport, String> {
    let account = state
        .services
        .lock()
        .map_err(|_| "Control service lock poisoned".to_owned())?
        .account
        .clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        account.import_existing_account_config(&label, &config_dir)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(error_text)?;
    rebuild_tray(&app).map_err(|error| error.to_string())?;
    Ok(result)
}

#[tauri::command]
fn refresh_app_scopes(
    state: State<'_, DesktopState>,
    app_ref: Uuid,
) -> Result<lpc_core::AppRecord, String> {
    state
        .services
        .lock()
        .map_err(|_| "Control service lock poisoned".to_owned())?
        .account
        .refresh_app_boundary(app_ref)
        .map_err(error_text)
}

#[tauri::command]
fn set_app_policy(
    state: State<'_, DesktopState>,
    request: SetPolicyRequest,
) -> Result<lpc_core::AppRecord, String> {
    state
        .services
        .lock()
        .map_err(|_| "Control service lock poisoned".to_owned())?
        .account
        .set_app_policy(request.app_ref, request.scopes)
        .map_err(error_text)
}

#[tauri::command]
fn scope_catalog(
    state: State<'_, DesktopState>,
    app_ref: Uuid,
) -> Result<Vec<lpc_core::ScopeInfo>, String> {
    let _services = state
        .services
        .lock()
        .map_err(|_| "Control service lock poisoned".to_owned())?;
    let catalog = state.store.load_catalog().map_err(error_text)?;
    let app = catalog
        .apps
        .iter()
        .find(|app| app.id == app_ref)
        .ok_or_else(|| error_text(lpc_core::LpcError::AppNotFound(app_ref.to_string())))?;
    Ok(lpc_core::scope_catalog(&app.available_scopes))
}

#[tauri::command]
fn set_account_alias(
    state: State<'_, DesktopState>,
    account_id: Uuid,
    alias: String,
) -> Result<lpc_core::AccountRecord, String> {
    state
        .services
        .lock()
        .map_err(|_| "Control service lock poisoned".to_owned())?
        .account
        .set_account_alias(account_id, &alias)
        .map_err(error_text)
}

#[tauri::command]
fn clear_account_alias(
    state: State<'_, DesktopState>,
    account_id: Uuid,
) -> Result<lpc_core::AccountRecord, String> {
    state
        .services
        .lock()
        .map_err(|_| "Control service lock poisoned".to_owned())?
        .account
        .clear_account_alias(account_id)
        .map_err(error_text)
}

fn lock_auth(
    auth: &Arc<Mutex<AuthCoordinator>>,
) -> Result<std::sync::MutexGuard<'_, AuthCoordinator>, String> {
    auth.lock()
        .map_err(|_| "Auth service lock poisoned".to_owned())
}

#[tauri::command]
async fn begin_account_login(
    state: State<'_, DesktopState>,
    app_ref: Uuid,
) -> Result<AuthFlowStart, String> {
    let auth = {
        let services = state
            .services
            .lock()
            .map_err(|_| "Control service lock poisoned".to_owned())?;
        if services.runtime_change_active {
            return Err("Wait for the CLI runtime change to finish before starting OAuth".into());
        }
        services.auth.clone()
    };
    tauri::async_runtime::spawn_blocking(move || {
        lock_auth(&auth)?
            .begin_new_account(app_ref)
            .map_err(error_text)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn begin_reauthorization(
    state: State<'_, DesktopState>,
    account_id: Uuid,
) -> Result<AuthFlowStart, String> {
    let auth = {
        let services = state
            .services
            .lock()
            .map_err(|_| "Control service lock poisoned".to_owned())?;
        if services.runtime_change_active {
            return Err("Wait for the CLI runtime change to finish before starting OAuth".into());
        }
        services.auth.clone()
    };
    tauri::async_runtime::spawn_blocking(move || {
        lock_auth(&auth)?
            .begin_reauthorization(account_id)
            .map_err(error_text)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn render_authorization_qr(
    state: State<'_, DesktopState>,
    flow_id: Uuid,
) -> Result<Vec<u8>, String> {
    let auth = state
        .services
        .lock()
        .map_err(|_| "Control service lock poisoned".to_owned())?
        .auth
        .clone();
    let png = lock_auth(&auth)?.render_qr(flow_id).map_err(error_text)?;
    Ok(png)
}

#[tauri::command]
async fn complete_authorization(
    app: AppHandle,
    state: State<'_, DesktopState>,
    flow_id: Uuid,
) -> Result<AuthProgress, String> {
    let auth = state
        .services
        .lock()
        .map_err(|_| "Control service lock poisoned".to_owned())?
        .auth
        .clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        lock_auth(&auth)?
            .complete_current_batch(flow_id)
            .map_err(error_text)
    })
    .await
    .map_err(|error| error.to_string())??;
    if result.complete {
        rebuild_tray(&app).map_err(|error| error.to_string())?;
    }
    Ok(result)
}

#[tauri::command]
fn cancel_authorization(state: State<'_, DesktopState>, flow_id: Uuid) -> Result<(), String> {
    let auth = state
        .services
        .lock()
        .map_err(|_| "Control service lock poisoned".to_owned())?
        .auth
        .clone();
    lock_auth(&auth)?.cancel(flow_id).map_err(error_text)?;
    Ok(())
}

#[tauri::command]
fn check_account(
    state: State<'_, DesktopState>,
    account_id: Uuid,
) -> Result<lpc_core::AccountRecord, String> {
    let service = state
        .services
        .lock()
        .map_err(|_| "Control service lock poisoned".to_owned())?
        .account
        .clone();
    match service
        .refresh_account_health_with(account_id, false)
        .map_err(error_text)?
    {
        HealthRefreshOutcome::Updated(account) => Ok(account),
        HealthRefreshOutcome::SkippedBusy(_) => {
            match service
                .refresh_account_health_with(account_id, false)
                .map_err(error_text)?
            {
                HealthRefreshOutcome::Updated(account) => Ok(account),
                HealthRefreshOutcome::SkippedBusy(_) => {
                    Err("钥匙串正被其他命令占用，这次没有改体检结果。稍后再试。".into())
                }
            }
        }
    }
}

#[tauri::command]
fn remove_account(
    app: AppHandle,
    state: State<'_, DesktopState>,
    account_id: Uuid,
) -> Result<(), String> {
    state
        .services
        .lock()
        .map_err(|_| "Control service lock poisoned".to_owned())?
        .account
        .remove_account(account_id)
        .map_err(error_text)?;
    rebuild_tray(&app).map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
fn diagnose(state: State<'_, DesktopState>) -> Result<DiagnosticReport, String> {
    run_diagnostics(&state.store).map_err(error_text)
}

#[tauri::command]
async fn begin_official_app_creation(
    state: State<'_, DesktopState>,
    label: String,
    brand: Brand,
) -> Result<AppCreationStart, String> {
    let coordinator = {
        let mut services = state
            .services
            .lock()
            .map_err(|_| "Control service lock poisoned".to_owned())?;
        if services.runtime_change_active {
            return Err("Wait for the CLI runtime change to finish before creating an App".into());
        }
        services.app_creation_begins = services.app_creation_begins.saturating_add(1);
        services.app_creation.clone()
    };
    let result = tauri::async_runtime::spawn_blocking(move || coordinator.begin(&label, brand))
        .await
        .map_err(|error| error.to_string());
    let mut services = state
        .services
        .lock()
        .map_err(|_| "Control service lock poisoned".to_owned())?;
    services.app_creation_begins = services.app_creation_begins.saturating_sub(1);
    drop(services);
    result?.map_err(error_text)
}

#[tauri::command]
async fn poll_official_app_creation(
    app: AppHandle,
    state: State<'_, DesktopState>,
    flow_id: Uuid,
) -> Result<AppCreationProgress, String> {
    let coordinator = {
        state
            .services
            .lock()
            .map_err(|_| "Control service lock poisoned".to_owned())?
            .app_creation
            .clone()
    };
    let progress = tauri::async_runtime::spawn_blocking(move || coordinator.poll(flow_id))
        .await
        .map_err(|error| error.to_string())?
        .map_err(error_text)?;
    if progress.complete {
        rebuild_tray(&app).map_err(|error| error.to_string())?;
    }
    Ok(progress)
}

#[tauri::command]
async fn cancel_official_app_creation(
    state: State<'_, DesktopState>,
    flow_id: Uuid,
) -> Result<(), String> {
    let coordinator = {
        state
            .services
            .lock()
            .map_err(|_| "Control service lock poisoned".to_owned())?
            .app_creation
            .clone()
    };
    tauri::async_runtime::spawn_blocking(move || coordinator.cancel(flow_id))
        .await
        .map_err(|error| error.to_string())?
        .map_err(error_text)
}

fn error_text(error: lpc_core::LpcError) -> String {
    format!("[{}] {}", error.stable_code(), error)
}

fn rebuild_tray(app: &AppHandle) -> tauri::Result<()> {
    let state = app.state::<DesktopState>();
    let catalog = match state.store.load_catalog() {
        Ok(value) => value,
        Err(_) => return Ok(()),
    };
    let active = state
        .store
        .load_state()
        .ok()
        .and_then(|value| value.active_account_id);
    let mut builder = MenuBuilder::new(app);
    if catalog.accounts.is_empty() {
        builder = builder.item(
            &MenuItemBuilder::with_id("no-accounts", "暂无账号")
                .enabled(false)
                .build(app)?,
        );
    } else {
        for account in &catalog.accounts {
            let app_record = catalog.apps.iter().find(|item| item.id == account.app_ref);
            let marker = if active == Some(account.id) {
                "●"
            } else {
                "○"
            };
            let label = format!(
                "{marker} {} · {}",
                account.display_name,
                app_record
                    .map(|item| item.label.as_str())
                    .unwrap_or("Unknown App")
            );
            builder = builder.item(
                &MenuItemBuilder::with_id(format!("account:{}", account.id), label).build(app)?,
            );
        }
    }
    builder = builder.separator();
    builder = builder.item(&MenuItemBuilder::with_id("open", "打开控制台").build(app)?);
    builder = builder.item(&MenuItemBuilder::with_id("quit", "退出").build(app)?);
    let menu = builder.build()?;

    if let Some(tray) = app.tray_by_id("main") {
        tray.set_menu(Some(menu))?;
    }
    Ok(())
}

fn sibling_shim() -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?;
    let dir = current.parent()?;
    #[cfg(windows)]
    let candidates = [dir.join("lark-cli.exe"), dir.join("lark-cli")];
    #[cfg(not(windows))]
    let candidates = [dir.join("lark-cli"), dir.join("lark-cli.exe")];
    candidates.into_iter().find(|path| path.is_file())
}

fn install_shim(source: &std::path::Path, paths: &AppPaths) -> Result<PathBuf, String> {
    lpc_core::install_managed_shim(source, paths).map_err(|error| error.to_string())
}

fn validate_packaged_shim(source: &std::path::Path) -> Result<(), String> {
    let output = std::process::Command::new(source)
        .arg("--lpc-shim-version")
        .output()
        .map_err(|error| format!("cannot inspect packaged lark-cli shim: {error}"))?;
    let reported = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let expected = env!("CARGO_PKG_VERSION");
    if !output.status.success() || reported != expected {
        return Err(format!(
            "packaged lark-cli shim version mismatch: desktop={expected}, shim={} (exit={})",
            if reported.is_empty() {
                "unknown"
            } else {
                &reported
            },
            output.status
        ));
    }
    Ok(())
}

fn repair_managed_route(store: &StateStore) -> Result<(), String> {
    let paths = store.paths();
    let source = sibling_shim().ok_or_else(|| {
        "The packaged lark-cli shim is missing beside the desktop executable".to_owned()
    })?;
    validate_packaged_shim(&source)?;
    install_shim(&source, paths)?;
    store.set_path_takeover_enabled(true).map_err(error_text)?;
    PathTakeover::new(paths.clone())
        .install()
        .map_err(error_text)?;
    Ok(())
}

#[derive(Default)]
struct HealthRoundStats {
    ready: usize,
    refreshable: usize,
    reauth_required: usize,
    cli_failure: usize,
    temporary_failure: usize,
    unknown: usize,
}

impl HealthRoundStats {
    fn record(&mut self, health: &AccountHealth) {
        match health {
            AccountHealth::Ready => self.ready += 1,
            AccountHealth::Refreshable => self.refreshable += 1,
            AccountHealth::ReauthRequired => self.reauth_required += 1,
            AccountHealth::CliFailure => self.cli_failure += 1,
            AccountHealth::TemporaryFailure => self.temporary_failure += 1,
            AccountHealth::Unknown => self.unknown += 1,
        }
    }

    fn failed(&self) -> usize {
        self.reauth_required + self.cli_failure + self.temporary_failure + self.unknown
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthDegradedPayload {
    account_name: String,
    health: AccountHealth,
    detail: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct KeychainCliffPayload {
    from: usize,
    to: usize,
    detail: String,
}

fn account_display_name(account: &lpc_core::AccountRecord) -> String {
    account
        .alias
        .clone()
        .unwrap_or_else(|| account.display_name.clone())
}

fn record_scheduled_health(
    app: &AppHandle,
    stats: &mut HealthRoundStats,
    before: &lpc_core::AccountRecord,
    updated: lpc_core::AccountRecord,
) {
    stats.record(&updated.health);
    if health_is_stable(&before.health) && health_is_critical(&updated.health) {
        let account_name = account_display_name(before);
        let detail = format!(
            "从 {} 变为 {}",
            health_label(&before.health),
            health_label(&updated.health)
        );
        tracing::error!(
            account = %account_name,
            account_id = %before.id,
            before = health_label(&before.health),
            after = health_label(&updated.health),
            "account health degraded during scheduled check"
        );
        let _ = app.emit(
            "lpc://health-degraded",
            HealthDegradedPayload {
                account_name,
                health: updated.health,
                detail,
            },
        );
    }
}

fn record_scheduled_health_error(
    app: &AppHandle,
    stats: &mut HealthRoundStats,
    before: &lpc_core::AccountRecord,
    error: lpc_core::LpcError,
) {
    stats.cli_failure += 1;
    if health_is_stable(&before.health) {
        let account_name = account_display_name(before);
        let detail = error.to_string();
        tracing::error!(
            account = %account_name,
            account_id = %before.id,
            before = health_label(&before.health),
            %error,
            "scheduled account health refresh failed"
        );
        let _ = app.emit(
            "lpc://health-degraded",
            HealthDegradedPayload {
                account_name,
                health: AccountHealth::CliFailure,
                detail,
            },
        );
    }
}

fn health_is_stable(health: &AccountHealth) -> bool {
    matches!(health, AccountHealth::Ready | AccountHealth::Refreshable)
}

fn health_is_critical(health: &AccountHealth) -> bool {
    matches!(
        health,
        AccountHealth::ReauthRequired | AccountHealth::CliFailure
    )
}

fn health_label(health: &AccountHealth) -> &'static str {
    match health {
        AccountHealth::Ready => "ready",
        AccountHealth::Refreshable => "refreshable",
        AccountHealth::ReauthRequired => "reauth_required",
        AccountHealth::CliFailure => "cli_failure",
        AccountHealth::TemporaryFailure => "temporary_failure",
        AccountHealth::Unknown => "unknown",
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            if let Err(error) = lpc_core::enforce_msix_shim_policy() {
                show_blocking_message(
                    "larkswitch — 已阻止影子凭据环境",
                    "当前进程带有 MSIX/AppX 包身份，会把飞书凭据写入隔离的影子注册表。\n\n请从开始菜单的 larkswitch 或 Windows Terminal 启动安装版。",
                );
                return Err(std::io::Error::other(error.to_string()).into());
            }
            ensure_installed_autostart(app.handle()).map_err(std::io::Error::other)?;
            let paths =
                AppPaths::discover().map_err(|error| std::io::Error::other(error.to_string()))?;
            // The app already had a subscriber, writing to a stderr that a
            // `windows_subsystem = "windows"` process does not have; every
            // record below was going nowhere. This replaces it rather than
            // adding a second one, and a failure here is not worth refusing to
            // start over.
            let _ = lpc_core::init_file_logging(&paths);

            // Data-safety guard #1: refuse to run against an ambiguous data root.
            // If the machine's persistent LPC_HOME points somewhere other than the
            // root we resolved, the real profiles likely live there; opening this
            // root would create an empty catalog and look like "everything is gone".
            // Stop before initialize/backup/repair so nothing is created or rebound.
            if let DataRootConsistency::Mismatch {
                effective,
                persistent,
            } = check_data_root_consistency(paths.root())
            {
                let body = format!(
                    "larkswitch 检测到数据目录不一致，为避免误建空档/覆盖数据已停止启动。\n\n\
                     本次要打开的目录:\n{}\n\n系统记录的目录:\n{}\n\n\
                     请从同一入口(安装版快捷方式)启动，或设置环境变量 LPC_HOME 指向正确目录后重试。\n\
                     确认要用当前目录可设置 LPC_ALLOW_HOME_MISMATCH=1 跳过本检查。",
                    effective.display(),
                    persistent.display()
                );
                tracing::error!(effective = %effective.display(), persistent = %persistent.display(), "data root mismatch; refusing to start");
                show_blocking_message("larkswitch — 数据目录不一致", &body);
                std::process::exit(3);
            }

            // Data-safety guard #2: single instance per data root. Two desktop
            // builds (e.g. installed + dev) sharing one LPC_HOME used to race on
            // the catalog and drop accounts; hold an OS lock for the lifetime.
            let instance_lock = match RoutingGate::new(paths.clone())
                .try_acquire_singleton("desktop-instance")
            {
                Ok(Some(lock)) => lock,
                Ok(None) => {
                    tracing::warn!("another larkswitch instance owns this data root; exiting");
                    show_blocking_message(
                        "larkswitch",
                        "larkswitch 已在运行（同一数据目录只允许一个实例）。请使用已打开的窗口。",
                    );
                    std::process::exit(0);
                }
                Err(error) => return Err(std::io::Error::other(error.to_string()).into()),
            };

            if let Err(error) = lpc_core::ensure_host_keychain_view(&paths) {
                tracing::error!(%error, "host keychain view verification failed");
                show_blocking_message(
                    "larkswitch — 已阻止影子凭据环境",
                    "当前进程看到的 Windows 注册表与宿主凭据视图不一致。为避免刷新或删除错误副本，larkswitch 已在接触凭据前停止。\n\n请从 Windows Terminal 或开始菜单启动安装版。",
                );
                return Err(std::io::Error::other(error.to_string()).into());
            }

            if let Err(error) = run_credential_backup(&paths, "startup") {
                tracing::warn!(%error, "credential backup failed during startup");
            }
            // Explicit second pass: file backup also triggers keychain export, but
            // make wipe detection loud at boot even if file backup is skipped.
            match lpc_core::inspect_keychain() {
                status if status.platform_supported && status.empty => {
                    tracing::error!(
                        entry_count = status.entry_count,
                        "official CLI keychain is EMPTY — restore from Documents\\LarkProfileConsoleBackups\\keychain or re-authorize"
                    );
                }
                status if status.platform_supported => {
                    tracing::info!(
                        entry_count = status.entry_count,
                        "official CLI keychain credential slots present"
                    );
                }
                _ => {}
            }
            if let Err(error) = lpc_core::backup_keychain_registry("desktop-startup") {
                tracing::warn!(%error, "keychain registry backup failed during startup");
            }
            let store = StateStore::new(paths.clone());
            store
                .initialize()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            let startup_keychain = lpc_core::inspect_keychain();
            let startup_watch = store.load_catalog().ok().and_then(|catalog| {
                observe_keychain_slots(
                    store.paths(),
                    &startup_keychain,
                    catalog.accounts.len(),
                    true,
                )
                .ok()
            });
            if let Some(event) = &startup_watch {
                if let Some(detail) = event.cliff_message() {
                    tracing::error!(detail = %detail, "keychain slot cliff at desktop startup");
                }
            }
            if let Err(error) = repair_managed_route(&store) {
                tracing::warn!(%error, "managed route self-repair failed during startup");
            }
            start_host_bridge(paths.clone())
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            let cli = managed_cli(&store)
                .unwrap_or_else(|_| OfficialCli::new(paths.runtime_dir().join("missing-lark-cli")));
            let account_service = AccountService::new(store.clone(), cli);
            let auth = Arc::new(Mutex::new(
                AuthCoordinator::new(account_service.clone())
                    .map_err(|error| std::io::Error::other(error.to_string()))?,
            ));
            let app_creation = AppCreationCoordinator::new(account_service.clone());
            let backup_paths = paths.clone();
            app.manage(DesktopState {
                paths,
                store,
                services: Mutex::new(ServiceBundle {
                    account: account_service,
                    auth,
                    app_creation,
                    app_creation_begins: 0,
                    runtime_change_active: false,
                }),
                _instance_lock: instance_lock,
            });

            let menu = MenuBuilder::new(app)
                .item(&MenuItemBuilder::with_id("open", "打开控制台").build(app)?)
                .item(&MenuItemBuilder::with_id("quit", "退出").build(app)?)
                .build()?;
            TrayIconBuilder::with_id("main")
                .icon(
                    app.default_window_icon()
                        .cloned()
                        .ok_or_else(|| std::io::Error::other("Bundled app icon is missing"))?,
                )
                .menu(&menu)
                .tooltip("larkswitch")
                .on_menu_event(|app, event| {
                    let id = event.id().as_ref();
                    if id == "quit" {
                        app.exit(0);
                    } else if id == "open" {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    } else if let Some(value) = id.strip_prefix("account:") {
                        if let Ok(account_id) = Uuid::parse_str(value) {
                            // RoutingGate uses its own file lock; do not wait on the
                            // services mutex held during long OAuth CLI polls.
                            let state = app.state::<DesktopState>();
                            let gate = lpc_core::RoutingGate::new(state.paths.clone());
                            let _ = gate.switch_account(&state.store, account_id);
                            let _ = rebuild_tray(app);
                        }
                    }
                })
                .build(app)?;
            rebuild_tray(app.handle())?;

            // Window starts with tauri.conf visible=false so --hidden never flashes.
            // Only explicit show for interactive launches; tray "打开控制台" still shows later.
            if !std::env::args().any(|argument| argument == "--hidden") {
                if let Some(window) = app.get_webview_window("main") {
                    window.show()?;
                }
            }

            if let Some(event) = &startup_watch {
                if let KeychainWatchKind::Cliff { from, to } = event.kind {
                    let _ = app.emit(
                        "lpc://keychain-cliff",
                        KeychainCliffPayload {
                            from,
                            to,
                            detail: event.cliff_message().unwrap_or_default(),
                        },
                    );
                }
            }

            let health_app = app.handle().clone();
            std::thread::spawn(move || loop {
                let state = health_app.state::<DesktopState>();
                if let Err(error) = repair_managed_route(&state.store) {
                    tracing::error!(%error, "scheduled managed route repair failed");
                }
                let service = match state.services.try_lock() {
                    Ok(services) => services.account.clone(),
                    Err(_) => {
                        std::thread::sleep(std::time::Duration::from_secs(15 * 60));
                        continue;
                    }
                };
                if let Err(error) = ensure_keychain_snapshot_if_stale(std::time::Duration::from_secs(
                    60 * 60,
                )) {
                    tracing::warn!(%error, "hourly keychain snapshot check failed");
                }
                let accounts = state
                    .store
                    .load_catalog()
                    .map(|catalog| catalog.accounts)
                    .unwrap_or_default();
                let keychain = lpc_core::inspect_keychain();
                let watch = observe_keychain_slots(
                    &state.paths,
                    &keychain,
                    accounts.len(),
                    true,
                )
                .ok();
                if let Some(event) = &watch {
                    if event.should_skip_scheduled_verify() {
                        if let KeychainWatchKind::Cliff { from, to } = event.kind {
                            tracing::error!(
                                from,
                                to,
                                "skipping scheduled health verify after keychain slot cliff"
                            );
                            let _ = health_app.emit(
                                "lpc://keychain-cliff",
                                KeychainCliffPayload {
                                    from,
                                    to,
                                    detail: event.cliff_message().unwrap_or_default(),
                                },
                            );
                        }
                        std::thread::sleep(std::time::Duration::from_secs(15 * 60));
                        continue;
                    }
                }
                let force_reauth = watch
                    .as_ref()
                    .is_some_and(|event| event.should_force_reauth_verify());
                let mut stats = HealthRoundStats::default();
                let mut skipped = Vec::new();
                for account in &accounts {
                    let force = force_verify_for_health(force_reauth, &account.health);
                    match service.refresh_account_health_with(account.id, force) {
                        Ok(HealthRefreshOutcome::SkippedBusy(_)) => skipped.push(account.clone()),
                        Ok(HealthRefreshOutcome::Updated(updated)) => {
                            record_scheduled_health(&health_app, &mut stats, account, updated);
                        }
                        Err(error) => {
                            record_scheduled_health_error(&health_app, &mut stats, account, error);
                        }
                    }
                }
                for account in skipped {
                    let force = force_verify_for_health(force_reauth, &account.health);
                    match service.refresh_account_health_with(account.id, force) {
                        Ok(HealthRefreshOutcome::SkippedBusy(_)) => {
                            tracing::warn!(
                                account = %account_display_name(&account),
                                account_id = %account.id,
                                "scheduled health check skipped twice (keychain busy); leaving last_verified_at unchanged"
                            );
                            stats.record(&account.health);
                        }
                        Ok(HealthRefreshOutcome::Updated(updated)) => {
                            record_scheduled_health(&health_app, &mut stats, &account, updated);
                        }
                        Err(error) => {
                            record_scheduled_health_error(&health_app, &mut stats, &account, error);
                        }
                    }
                }
                let _ = rebuild_tray(&health_app);
                let _ = health_app.emit("lpc://health-updated", ());
                tracing::info!(
                    ready = stats.ready,
                    refreshable = stats.refreshable,
                    failed = stats.failed(),
                    reauth_required = stats.reauth_required,
                    cli_failure = stats.cli_failure,
                    temporary_failure = stats.temporary_failure,
                    unknown = stats.unknown,
                    "scheduled health check round complete"
                );
                std::thread::sleep(std::time::Duration::from_secs(15 * 60));
            });
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(6 * 60 * 60));
                if let Err(error) = run_credential_backup(&backup_paths, "scheduled") {
                    tracing::warn!(%error, "scheduled credential backup failed");
                }
                // Keepalive: light user-identity probes refresh access tokens when
                // possible. This does NOT survive keychain wipe; backups do.
                let status = lpc_core::inspect_keychain();
                if status.platform_supported && status.empty {
                    tracing::error!(
                        "scheduled check: official CLI keychain EMPTY — user tokens wiped"
                    );
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            snapshot,
            switch_account,
            install_runtime,
            rollback_runtime,
            install_path_takeover,
            remove_path_takeover,
            autostart_status,
            set_autostart,
            import_existing_app,
            discover_existing_configs,
            inspect_existing_config,
            import_existing_account_config,
            refresh_app_scopes,
            set_app_policy,
            scope_catalog,
            set_account_alias,
            clear_account_alias,
            begin_account_login,
            begin_reauthorization,
            render_authorization_qr,
            complete_authorization,
            cancel_authorization,
            check_account,
            remove_account,
            diagnose,
            runtime_identity,
            begin_official_app_creation,
            poll_official_app_creation,
            cancel_official_app_creation,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Lark Profile Console");
}

#[cfg(test)]
mod tests {
    use super::{
        ensure_autostart_exe_is_install, install_shim, is_cargo_target_build_exe,
        is_packaged_app_virtualized_exe,
    };
    use lpc_core::AppPaths;
    use std::path::Path;

    #[test]
    fn detects_workspace_target_builds_and_spares_install_paths() {
        assert!(is_cargo_target_build_exe(Path::new(
            r"D:\repo\target\release\lark-profile-console.exe"
        )));
        assert!(is_cargo_target_build_exe(Path::new(
            r"D:\repo\target\x86_64-pc-windows-msvc\release\lark-profile-console.exe"
        )));
        assert!(is_cargo_target_build_exe(Path::new(
            "/home/dev/project/target/debug/lark-profile-console"
        )));
        assert!(!is_cargo_target_build_exe(Path::new(
            r"C:\Users\me\AppData\Local\Lark Profile Console\lark-profile-console.exe"
        )));
        assert!(!is_cargo_target_build_exe(Path::new(
            r"C:\Program Files\Lark Profile Console\lark-profile-console.exe"
        )));
    }

    #[test]
    fn rejects_packaged_app_virtualized_copies_for_autostart() {
        let virtualized = Path::new(
            r"C:\Users\me\AppData\Local\Packages\OpenAI.CodexBeta_123\LocalCache\Local\Lark Profile Console\lark-profile-console.exe",
        );
        assert!(is_packaged_app_virtualized_exe(virtualized));
        assert!(ensure_autostart_exe_is_install(virtualized).is_err());
        assert!(!is_packaged_app_virtualized_exe(Path::new(
            r"C:\Users\me\AppData\Local\Lark Profile Console\lark-profile-console.exe"
        )));
    }

    #[test]
    fn shim_install_is_idempotent_and_updates_changed_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("home"));
        let source = temp.path().join("source-shim");
        std::fs::write(&source, b"first").unwrap();

        let destination = install_shim(&source, &paths).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"first");
        assert_eq!(install_shim(&source, &paths).unwrap(), destination);

        std::fs::write(&source, b"second").unwrap();
        install_shim(&source, &paths).unwrap();
        assert_eq!(std::fs::read(destination).unwrap(), b"second");

        #[cfg(windows)]
        {
            let cmd = std::fs::read_to_string(paths.bin_dir().join("lark-cli.cmd")).unwrap();
            let ps1 = std::fs::read_to_string(paths.bin_dir().join("lark-cli.ps1")).unwrap();
            assert!(cmd.contains("%~dp0lark-cli.exe"));
            assert!(ps1.contains("$PSScriptRoot"));
            assert!(ps1.contains("lark-cli.exe"));
            assert!(!ps1.contains("exit $LASTEXITCODE"));
        }
    }
}
