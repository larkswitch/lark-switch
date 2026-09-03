use clap::{Parser, Subcommand, ValueEnum};
use lpc_core::{
    assert_summary_safe, default_backup_root, default_official_config_dirs, default_policy,
    diagnostics::run_diagnostics_with, list_backups, parse_selector, resolve_account,
    restore_from_backup, restore_latest, run_credential_backup, search_accounts, summarize_account,
    summarize_views, AccountHealth, AccountService, AppPaths, AuthCoordinator, Brand,
    HealthRefreshOutcome, LpcError, OfficialCli, PathTakeover, RedactionLevel, RuntimeManager,
    SearchFilter, SecretString, ShimInstallOptions, StateStore,
};
use std::io::{self, BufRead, IsTerminal};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use uuid::Uuid;

#[derive(Parser)]
#[command(
    name = "larkswitch",
    version,
    about = "Unofficial identity layer for official lark-cli. Switch people, not apps.",
    after_help = "lpcctl is a compatibility alias for larkswitch."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize local state, install the official CLI and the product shim.
    ///
    /// PATH and the global npm command route are always managed so every
    /// `lark-cli` invocation crosses the account router and keychain lock.
    Setup {
        #[arg(long, default_value = lpc_core::SUPPORTED_CLI_VERSION)]
        cli_version: String,
        #[arg(long)]
        path_takeover: bool,
        /// Deprecated compatibility flag; secure routing can no longer be disabled.
        #[arg(long, hide = true)]
        no_path_takeover: bool,
        #[arg(long)]
        shim: Option<PathBuf>,
    },
    /// Import accounts from an existing official lark-cli config (default ~/.lark-cli).
    Import {
        #[arg(long)]
        config_dir: Option<PathBuf>,
        #[arg(long)]
        label: Option<String>,
    },
    Runtime {
        #[command(subcommand)]
        command: RuntimeCommand,
    },
    App {
        #[command(subcommand)]
        command: AppCommand,
    },
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
    Path {
        #[command(subcommand)]
        command: PathCommand,
    },
    Snapshot,
    /// List live lark-cli commands currently holding routing leases.
    Ps,
    /// Run local self-checks.
    Doctor {
        /// Strip machine-identifying detail (user names inside paths) so the
        /// report is safe to paste into a ticket or a chat.
        #[arg(long)]
        share: bool,
    },
    /// Create a manual credential/config backup snapshot.
    Backup,
    /// List or restore backup snapshots.
    Restore {
        /// List available snapshots without restoring.
        #[arg(long)]
        list: bool,
        /// Restore a specific snapshot by directory id.
        #[arg(long)]
        snapshot: Option<String>,
        /// Restore the newest snapshot (default when no other flag is given).
        #[arg(long)]
        latest: bool,
    },
}

#[derive(Subcommand)]
enum RuntimeCommand {
    Install {
        #[arg(default_value = lpc_core::SUPPORTED_CLI_VERSION)]
        version: String,
    },
    Rollback,
    List,
}

#[derive(Subcommand)]
enum AppCommand {
    Import {
        #[arg(long)]
        label: String,
        #[arg(long)]
        app_id: String,
        #[arg(long, value_enum, default_value_t = BrandArg::Feishu)]
        brand: BrandArg,
        /// Read App Secret from stdin instead of a hidden terminal prompt.
        #[arg(long)]
        secret_stdin: bool,
    },
    ImportConfig {
        #[arg(long)]
        label: String,
        #[arg(long)]
        config_dir: PathBuf,
    },
    Create {
        #[arg(long)]
        label: String,
        #[arg(long, value_enum, default_value_t = BrandArg::Feishu)]
        brand: BrandArg,
    },
    List,
    RefreshScopes {
        app: Uuid,
    },
    PolicyAll {
        app: Uuid,
    },
    PolicySet {
        app: Uuid,
        #[arg(long, value_delimiter = ',')]
        scopes: Vec<String>,
    },
    Remove {
        app: Uuid,
    },
}

#[derive(Subcommand)]
enum AccountCommand {
    Login {
        app: Uuid,
    },
    Reauthorize {
        account: Uuid,
    },
    DiscoverConfigs,
    ImportConfig {
        #[arg(long)]
        label: String,
        #[arg(long)]
        config_dir: PathBuf,
    },
    /// Compact account list for humans and AI agents.
    List {
        #[arg(long)]
        with_scopes: bool,
    },
    /// Loose query over name/alias/app label/app id/tenant.
    Search {
        #[arg(short, long)]
        q: Option<String>,
        #[arg(long)]
        app: Option<String>,
        #[arg(long, value_enum)]
        health: Option<HealthArg>,
        #[arg(long)]
        scope: Option<String>,
        #[arg(long)]
        with_scopes: bool,
    },
    /// Strict unique resolve using the same selector rules as --account / --lpc-account.
    Resolve {
        selector: String,
        #[arg(long)]
        app: Option<String>,
        #[arg(long)]
        with_scopes: bool,
    },
    Alias {
        #[command(subcommand)]
        command: AliasCommand,
    },
    Switch {
        account: Uuid,
    },
    Check {
        account: Uuid,
    },
    Remove {
        account: Uuid,
    },
}

#[derive(Subcommand)]
enum AliasCommand {
    Set { account: Uuid, alias: String },
    Clear { account: Uuid },
}

#[derive(Subcommand)]
enum PathCommand {
    Install,
    /// Reinstall the managed shim/forwarders and restore PATH takeover.
    Repair {
        /// Compatibility flag; Windows repair now always closes this bypass.
        #[arg(long)]
        takeover_npm: bool,
    },
    Remove,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum BrandArg {
    Feishu,
    Lark,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum HealthArg {
    Unknown,
    Ready,
    Refreshable,
    ReauthRequired,
    TemporaryFailure,
    CliFailure,
}

impl From<BrandArg> for Brand {
    fn from(value: BrandArg) -> Self {
        match value {
            BrandArg::Feishu => Brand::Feishu,
            BrandArg::Lark => Brand::Lark,
        }
    }
}

impl From<HealthArg> for AccountHealth {
    fn from(value: HealthArg) -> Self {
        match value {
            HealthArg::Unknown => AccountHealth::Unknown,
            HealthArg::Ready => AccountHealth::Ready,
            HealthArg::Refreshable => AccountHealth::Refreshable,
            HealthArg::ReauthRequired => AccountHealth::ReauthRequired,
            HealthArg::TemporaryFailure => AccountHealth::TemporaryFailure,
            HealthArg::CliFailure => AccountHealth::CliFailure,
        }
    }
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("[{}] {}", error.stable_code(), error);
        std::process::exit(error.process_exit_code());
    }
}

fn run(cli: Cli) -> lpc_core::Result<()> {
    let paths = AppPaths::discover()?;
    // After path discovery, because the log lives under LPC_HOME, and ignoring
    // the result on purpose: a read-only or full disk must not turn a working
    // command into a failed one. Replaces the previous stderr subscriber —
    // only one global subscriber can win, and this one survives the process.
    let _ = lpc_core::init_file_logging(&paths);
    let store = StateStore::new(paths.clone());
    match cli.command {
        Command::Setup {
            cli_version,
            path_takeover,
            no_path_takeover,
            shim,
        } => {
            store.initialize()?;
            let runtime = RuntimeManager::new(store.clone())?;
            let executable = runtime.install(&cli_version)?;
            let shim_source = shim.or_else(find_sibling_shim).ok_or_else(|| {
                lpc_core::LpcError::Internal(
                    "cannot locate the lark-cli shim; pass --shim <path>".into(),
                )
            })?;
            install_shim(&shim_source, &paths, ShimInstallOptions::default())?;
            let _legacy_flags = (path_takeover, no_path_takeover);
            store.set_path_takeover_enabled(true)?;
            print_json(&PathTakeover::new(paths).install()?)?;
            println!("Managed official CLI: {}", executable.display());
        }
        Command::Import { config_dir, label } => {
            store.initialize()?;
            let service = account_service(store.clone())?;
            print_json(&import_official_accounts(&service, config_dir, label)?)?;
        }
        Command::Runtime { command } => {
            store.initialize()?;
            let manager = RuntimeManager::new(store.clone())?;
            match command {
                RuntimeCommand::Install { version } => {
                    println!("{}", manager.install(&version)?.display())
                }
                RuntimeCommand::Rollback => println!("{}", manager.rollback()?.display()),
                RuntimeCommand::List => print_json(&manager.installed_versions()?)?,
            }
        }
        Command::App { command } => {
            store.initialize()?;
            let service = account_service(store.clone())?;
            match command {
                AppCommand::Import {
                    label,
                    app_id,
                    brand,
                    secret_stdin,
                } => {
                    let secret = read_secret(secret_stdin)?;
                    print_json(&service.import_existing_app(
                        &label,
                        &app_id,
                        secret,
                        brand.into(),
                    )?)?;
                }
                AppCommand::ImportConfig { label, config_dir } => {
                    print_json(&service.import_official_config(&label, &config_dir)?)?
                }
                AppCommand::Create { label, brand } => {
                    let id = Uuid::new_v4();
                    let staging = store.paths().staging_dir().join(format!("create-app-{id}"));
                    std::fs::create_dir_all(&staging)?;
                    let cli = managed_cli(&store)?;
                    let args = [
                        "config",
                        "init",
                        "--new",
                        "--brand",
                        Brand::from(brand).as_cli_value(),
                    ];
                    let code = cli.run_interactive(Some(&staging), args)?;
                    if code != 0 {
                        return Err(lpc_core::LpcError::CliFailed {
                            code,
                            message: "official app creation flow failed".into(),
                        });
                    }
                    let app = service.import_official_config(&label, &staging)?;
                    let _ = std::fs::remove_dir_all(staging);
                    print_json(&app)?;
                }
                AppCommand::List => print_json(&store.load_catalog()?.apps)?,
                AppCommand::RefreshScopes { app } => {
                    print_json(&service.refresh_app_boundary(app)?)?
                }
                AppCommand::PolicyAll { app } => {
                    let refreshed = service.refresh_app_boundary(app)?;
                    let policy = default_policy(&refreshed.available_scopes);
                    print_json(&service.set_app_policy(app, policy)?)?;
                }
                AppCommand::PolicySet { app, scopes } => {
                    print_json(&service.set_app_policy(app, scopes.into_iter().collect())?)?;
                }
                AppCommand::Remove { app } => {
                    service.remove_app_metadata(app)?;
                    println!("removed app metadata {app}");
                }
            }
        }
        Command::Account { command } => {
            store.initialize()?;
            let service = account_service(store.clone())?;
            match command {
                AccountCommand::Login { app } => {
                    drive_auth(AuthCoordinator::new(service)?, AuthMode::New(app))?
                }
                AccountCommand::Reauthorize { account } => drive_auth(
                    AuthCoordinator::new(service)?,
                    AuthMode::Reauthorize(account),
                )?,
                AccountCommand::DiscoverConfigs => {
                    let candidates = default_official_config_dirs()
                        .into_iter()
                        .filter_map(|config_dir| {
                            match service.inspect_existing_account_config(&config_dir) {
                                Ok(candidate) => Some(candidate),
                                Err(error) => {
                                    eprintln!(
                                        "skipped unverifiable config {}: {error}",
                                        config_dir.display()
                                    );
                                    None
                                }
                            }
                        })
                        .collect::<Vec<_>>();
                    print_json(&candidates)?;
                }
                AccountCommand::ImportConfig { label, config_dir } => {
                    print_json(&service.import_existing_account_config(&label, &config_dir)?)?;
                }
                AccountCommand::List { with_scopes } => {
                    let counts = lpc_core::RoutingGate::new(paths).running_counts()?;
                    let views = store.snapshot(&counts)?.accounts;
                    let summaries = summarize_views(&views, with_scopes);
                    for summary in &summaries {
                        assert_summary_safe(summary)?;
                    }
                    print_compact(&summaries)?;
                }
                AccountCommand::Search {
                    q,
                    app,
                    health,
                    scope,
                    with_scopes,
                } => {
                    let counts = lpc_core::RoutingGate::new(paths).running_counts()?;
                    let catalog = store.load_catalog()?;
                    let state = store.load_state()?;
                    let summaries = search_accounts(
                        &catalog,
                        &state,
                        &counts,
                        &SearchFilter {
                            query: q,
                            app,
                            health: health.map(Into::into),
                            scope,
                        },
                        with_scopes,
                    )?;
                    for summary in &summaries {
                        assert_summary_safe(summary)?;
                    }
                    print_compact(&summaries)?;
                }
                AccountCommand::Resolve {
                    selector,
                    app,
                    with_scopes,
                } => {
                    let counts = lpc_core::RoutingGate::new(paths).running_counts()?;
                    let catalog = store.load_catalog()?;
                    let state = store.load_state()?;
                    let parsed = parse_selector(&selector)?;
                    let (account, app_record) = resolve_account(&catalog, &parsed, app.as_deref())?;
                    let summary = summarize_account(
                        account,
                        app_record,
                        state.active_account_id == Some(account.id),
                        counts.get(&account.id).copied().unwrap_or(0),
                        with_scopes,
                    );
                    assert_summary_safe(&summary)?;
                    print_compact(&summary)?;
                }
                AccountCommand::Alias { command } => match command {
                    AliasCommand::Set { account, alias } => {
                        let updated = service.set_account_alias(account, &alias)?;
                        let catalog = store.load_catalog()?;
                        let state = store.load_state()?;
                        let app = catalog
                            .apps
                            .iter()
                            .find(|item| item.id == updated.app_ref)
                            .ok_or_else(|| {
                                lpc_core::LpcError::AppNotFound(updated.app_ref.to_string())
                            })?;
                        let summary = summarize_account(
                            &updated,
                            app,
                            state.active_account_id == Some(updated.id),
                            0,
                            false,
                        );
                        assert_summary_safe(&summary)?;
                        print_compact(&summary)?;
                    }
                    AliasCommand::Clear { account } => {
                        let updated = service.clear_account_alias(account)?;
                        let catalog = store.load_catalog()?;
                        let state = store.load_state()?;
                        let app = catalog
                            .apps
                            .iter()
                            .find(|item| item.id == updated.app_ref)
                            .ok_or_else(|| {
                                lpc_core::LpcError::AppNotFound(updated.app_ref.to_string())
                            })?;
                        let summary = summarize_account(
                            &updated,
                            app,
                            state.active_account_id == Some(updated.id),
                            0,
                            false,
                        );
                        assert_summary_safe(&summary)?;
                        print_compact(&summary)?;
                    }
                },
                AccountCommand::Switch { account } => {
                    service.switch_account(account)?;
                    println!("future lark-cli commands will use account {account}");
                }
                AccountCommand::Check { account } => {
                    match service.refresh_account_health_with(account, false)? {
                        HealthRefreshOutcome::Updated(record) => print_json(&record)?,
                        HealthRefreshOutcome::SkippedBusy(_) => {
                            match service.refresh_account_health_with(account, false)? {
                                HealthRefreshOutcome::Updated(record) => print_json(&record)?,
                                HealthRefreshOutcome::SkippedBusy(_) => {
                                    return Err(LpcError::CliKeychainBusy);
                                }
                            }
                        }
                    }
                }
                AccountCommand::Remove { account } => {
                    service.remove_account(account)?;
                    println!("removed account {account}");
                }
            }
        }
        Command::Path { command } => {
            store.initialize()?;
            let takeover = PathTakeover::new(paths.clone());
            match command {
                PathCommand::Install => {
                    let shim_source = find_sibling_shim().ok_or_else(|| {
                        lpc_core::LpcError::Internal(
                            "cannot locate the lark-cli shim beside larkswitch/lpcctl".into(),
                        )
                    })?;
                    install_shim(&shim_source, &paths, ShimInstallOptions::default())?;
                    store.set_path_takeover_enabled(true)?;
                    print_json(&takeover.install()?)?;
                }
                PathCommand::Repair { takeover_npm } => {
                    let shim_source = find_sibling_shim().ok_or_else(|| {
                        lpc_core::LpcError::Internal(
                            "cannot locate the lark-cli shim beside larkswitch/lpcctl".into(),
                        )
                    })?;
                    let shim = install_shim(
                        &shim_source,
                        &paths,
                        ShimInstallOptions { takeover_npm: true },
                    )?;
                    store.set_path_takeover_enabled(true)?;
                    let path = takeover.install()?;
                    print_json(&serde_json::json!({
                        "shim": shim,
                        "path": path,
                        "takeoverNpm": true,
                        "takeoverRequested": takeover_npm,
                    }))?;
                }
                PathCommand::Remove => {
                    store.set_path_takeover_enabled(false)?;
                    print_json(&takeover.uninstall()?)?;
                }
            }
        }
        Command::Snapshot => {
            store.initialize()?;
            let counts = lpc_core::RoutingGate::new(paths).running_counts()?;
            print_json(&store.snapshot(&counts)?)?;
        }
        Command::Ps => {
            store.initialize()?;
            let leases = lpc_core::RoutingGate::new(paths.clone()).running_leases()?;
            let catalog = store.load_catalog()?;
            let rows = leases
                .into_iter()
                .map(|lease| {
                    let account = catalog
                        .accounts
                        .iter()
                        .find(|account| account.id == lease.account_id);
                    serde_json::json!({
                        "leaseId": lease.id,
                        "pid": lease.pid,
                        "accountId": lease.account_id,
                        "accountName": account.map(|account| account.display_name.as_str()),
                        "accountAlias": account.and_then(|account| account.alias.as_deref()),
                        "appId": lease.app_id,
                        "startedAt": lease.created_at,
                    })
                })
                .collect::<Vec<_>>();
            print_json(&rows)?;
        }
        Command::Doctor { share } => {
            store.initialize()?;
            let level = if share {
                RedactionLevel::Outbound
            } else {
                RedactionLevel::Local
            };
            print_json(&run_diagnostics_with(&store, level)?)?;
        }
        Command::Backup => {
            print_json(&run_credential_backup(&paths, "manual")?)?;
        }
        Command::Restore {
            list,
            snapshot,
            latest,
        } => {
            let _ = latest;
            let backup_root = default_backup_root()?;
            if list {
                print_json(&list_backups(&backup_root)?)?;
            } else if let Some(id) = snapshot {
                let snapshots = list_backups(&backup_root)?;
                let selected = snapshots.iter().find(|item| item.id == id).ok_or_else(|| {
                    LpcError::Internal(format!("backup snapshot not found: {id}"))
                })?;
                print_json(&restore_from_backup(&paths, selected)?)?;
            } else {
                print_json(&restore_latest(&paths, &backup_root)?)?;
            }
        }
    }
    Ok(())
}

enum AuthMode {
    New(Uuid),
    Reauthorize(Uuid),
}

fn drive_auth(mut coordinator: AuthCoordinator, mode: AuthMode) -> lpc_core::Result<()> {
    let mut start = match mode {
        AuthMode::New(app) => coordinator.begin_new_account(app)?,
        AuthMode::Reauthorize(account) => coordinator.begin_reauthorization(account)?,
    };
    loop {
        println!(
            "Open this official authorization page:\n{}",
            start.verification_url
        );
        let _ = open::that(&start.verification_url);
        println!(
            "Authorize in browser, then press Enter. Ctrl+C cancels without persisting a device code."
        );
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;

        let flow_id = start.flow_id;
        loop {
            println!("Checking authorization status...");
            let progress = match coordinator.complete_current_batch(flow_id) {
                Ok(progress) => progress,
                Err(LpcError::AuthFlowExpired) => {
                    eprintln!(
                        "Authorization flow expired. Run the command again to start a new flow."
                    );
                    return Err(LpcError::AuthFlowExpired);
                }
                Err(error) => return Err(error),
            };
            if progress.complete {
                print_json(&progress.account)?;
                return Ok(());
            }
            if let Some(next) = progress.next {
                // Legacy multi-batch path (single-auth model leaves next = None).
                start = next;
                break;
            }
            println!("Waiting for authorization...");
            thread::sleep(Duration::from_millis(500));
        }
    }
}

fn managed_cli(store: &StateStore) -> lpc_core::Result<OfficialCli> {
    let state = store.load_state()?;
    let path = state
        .managed_cli_path
        .ok_or_else(|| lpc_core::LpcError::RuntimeMissing(store.paths().runtime_dir()))?;
    Ok(OfficialCli::new(path))
}

fn account_service(store: StateStore) -> lpc_core::Result<AccountService> {
    Ok(AccountService::new(store.clone(), managed_cli(&store)?))
}

fn read_secret(from_stdin: bool) -> lpc_core::Result<SecretString> {
    let value = if from_stdin || !io::stdin().is_terminal() {
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;
        line.trim_end_matches(['\r', '\n']).to_owned()
    } else {
        rpassword::prompt_password("App Secret (not stored by Lark Profile Console): ")?
    };
    if value.trim().is_empty() {
        return Err(lpc_core::LpcError::UnsafeConfig(
            "App Secret is empty".into(),
        ));
    }
    Ok(SecretString::new(value))
}

fn find_sibling_shim() -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?;
    let directory = current.parent()?;
    #[cfg(windows)]
    let candidates = [
        directory.join("lark-cli.exe"),
        directory.join("lpc-shim.exe"),
    ];
    #[cfg(not(windows))]
    let candidates = [directory.join("lark-cli"), directory.join("lpc-shim")];
    candidates.into_iter().find(|path| path.is_file())
}

fn install_shim(
    source: &Path,
    paths: &AppPaths,
    options: ShimInstallOptions,
) -> lpc_core::Result<PathBuf> {
    lpc_core::install_managed_shim_with(source, paths, options)
}

fn import_official_accounts(
    service: &AccountService,
    config_dir: Option<PathBuf>,
    label: Option<String>,
) -> lpc_core::Result<Vec<lpc_core::ExistingAccountImport>> {
    let dirs = if let Some(config_dir) = config_dir {
        vec![config_dir]
    } else {
        default_official_config_dirs()
    };
    if dirs.is_empty() {
        return Err(lpc_core::LpcError::Internal(
            "no official lark-cli config found; pass --config-dir or run official `lark-cli auth login` first".into(),
        ));
    }

    let mut imported = Vec::new();
    for dir in dirs {
        let candidate = match service.inspect_existing_account_config(&dir) {
            Ok(candidate) => candidate,
            Err(error) => {
                eprintln!("skipped unverifiable config {}: {error}", dir.display());
                continue;
            }
        };
        let app_label = label
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                if candidate.display_name.trim().is_empty() {
                    format!("imported-{}", &candidate.app_id)
                } else {
                    candidate.display_name.clone()
                }
            });
        imported.push(service.import_existing_account_config(&app_label, &dir)?);
    }
    if imported.is_empty() {
        return Err(lpc_core::LpcError::Internal(
            "no official lark-cli config could be imported".into(),
        ));
    }
    Ok(imported)
}

fn print_json<T: serde::Serialize>(value: &T) -> lpc_core::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn print_compact<T: serde::Serialize>(value: &T) -> lpc_core::Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{install_shim, Cli, ShimInstallOptions};
    use clap::Parser;
    use lpc_core::AppPaths;

    #[test]
    fn ps_is_a_real_control_plane_command() {
        assert!(Cli::try_parse_from(["lpcctl", "ps"]).is_ok());
    }

    #[test]
    fn setup_keeps_legacy_path_flags_parseable() {
        let cli = Cli::try_parse_from(["larkswitch", "setup"]).unwrap();
        match cli.command {
            super::Command::Setup {
                path_takeover,
                no_path_takeover,
                ..
            } => {
                assert!(!path_takeover);
                assert!(!no_path_takeover);
            }
            _ => panic!("expected setup"),
        }
    }

    #[test]
    fn import_is_a_real_control_plane_command() {
        assert!(Cli::try_parse_from(["larkswitch", "import"]).is_ok());
        assert!(Cli::try_parse_from(["lpcctl", "import"]).is_ok());
    }

    #[test]
    fn path_repair_accepts_the_legacy_takeover_npm_flag() {
        let cli = Cli::try_parse_from(["larkswitch", "path", "repair"]).unwrap();
        match cli.command {
            super::Command::Path {
                command: super::PathCommand::Repair { takeover_npm },
            } => assert!(!takeover_npm),
            _ => panic!("expected path repair"),
        }
        let cli = Cli::try_parse_from(["larkswitch", "path", "repair", "--takeover-npm"]).unwrap();
        match cli.command {
            super::Command::Path {
                command: super::PathCommand::Repair { takeover_npm },
            } => assert!(takeover_npm),
            _ => panic!("expected path repair"),
        }
    }

    #[test]
    fn shim_install_covers_explicit_windows_command_names() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("home"));
        let source = temp.path().join("lark-cli.exe");
        std::fs::write(&source, b"shim").unwrap();

        install_shim(&source, &paths, ShimInstallOptions::default()).unwrap();

        #[cfg(windows)]
        {
            let cmd = std::fs::read_to_string(paths.bin_dir().join("lark-cli.cmd")).unwrap();
            let ps1 = std::fs::read_to_string(paths.bin_dir().join("lark-cli.ps1")).unwrap();
            assert!(cmd.contains("%~dp0lark-cli.exe"));
            assert!(ps1.contains("$PSScriptRoot"));
            assert!(!ps1.contains("exit $LASTEXITCODE"));
        }
    }
}
