use lpc_core::{
    execute_via_host_bridge, inspect_host_keychain_view, is_running_in_msix_package,
    resolve_execution_override, strip_leading_lpc_flags, try_acquire_cli_keychain_lock, AppPaths,
    KeychainViewKind, LpcError, RoutingGate, StateStore,
};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("[{}] {}", error.stable_code(), error);
            error.process_exit_code()
        }
    };
    std::process::exit(code);
}

fn run() -> lpc_core::Result<i32> {
    let raw_args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if raw_args.len() == 1 && raw_args[0].as_os_str() == std::ffi::OsStr::new("--lpc-shim-version")
    {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }
    let parsed = strip_leading_lpc_flags(&raw_args)?;
    if let Some(reason) = guarded_management_command(&parsed.forwarded) {
        eprintln!(
            "[LPC_MANAGEMENT_GUARDED] {reason}. Use the desktop UI or `larkswitch` / `lpcctl`; set LPC_ALLOW_DIRECT_MANAGEMENT=1 only for recovery."
        );
        return Ok(64);
    }

    let override_selector = resolve_execution_override(
        parsed.account_override.as_deref(),
        ["LARKSWITCH_ACCOUNT", "LPC_ACCOUNT"]
            .into_iter()
            .find_map(|key| {
                std::env::var_os(key).filter(|value| !value.to_string_lossy().trim().is_empty())
            })
            .as_deref(),
    );

    let paths = AppPaths::discover()?;
    let keychain_view = inspect_host_keychain_view(&paths);
    if is_running_in_msix_package() || keychain_view.kind == KeychainViewKind::Mismatch {
        let mut bridge_args = Vec::with_capacity(parsed.forwarded.len() + 2);
        if let Some(selector) = override_selector.as_deref() {
            bridge_args.push(std::ffi::OsString::from("--lpc-account"));
            bridge_args.push(std::ffi::OsString::from(selector));
        }
        bridge_args.extend(parsed.forwarded.iter().cloned());
        let response = execute_via_host_bridge(&paths, &bridge_args)?;
        print!("{}", response.stdout);
        eprint!("{}", response.stderr);
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        return Ok(response.exit_code);
    }
    lpc_core::enforce_host_keychain_view(&paths)?;
    // Once, on the way in: this is the hot path for every `lark-cli` call, and
    // a shim that refused to run because a log file would not open would take
    // the whole CLI down with it.
    let _ = lpc_core::init_file_logging(&paths);
    let store = StateStore::new(paths.clone());
    let gate = RoutingGate::new(paths.clone());
    let (route, lease) =
        gate.snapshot_for_execution_with_override(&store, override_selector.as_deref())?;
    // Who ran under which identity. The arguments are deliberately absent: they
    // carry user data, and the audit question is about the account, not the command.
    tracing::info!(
        account_id = %route.account.id,
        generation = route.generation,
        "shim routed command"
    );

    let current = std::env::current_exe()?.canonicalize()?;
    let managed = route.managed_cli_path.canonicalize()?;
    if current == managed {
        return Err(LpcError::RuntimeRecursion);
    }

    let keychain_lock = try_acquire_cli_keychain_lock(&paths, Duration::from_secs(30))?
        .ok_or(LpcError::CliKeychainBusy)?;

    let status = Command::new(&managed)
        .args(&parsed.forwarded)
        .env("LARKSUITE_CLI_CONFIG_DIR", &route.account.config_dir)
        .env("LPC_ACTIVE_ACCOUNT_ID", route.account.id.to_string())
        .env("LPC_ACTIVE_APP_ID", &route.app.app_id)
        .env("LPC_ROUTE_GENERATION", route.generation.to_string())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;

    drop(keychain_lock);

    lease.release()?;
    Ok(exit_code(status))
}

fn guarded_management_command(args: &[std::ffi::OsString]) -> Option<&'static str> {
    if std::env::var_os("LPC_ALLOW_DIRECT_MANAGEMENT").as_deref() == Some(std::ffi::OsStr::new("1"))
    {
        return None;
    }
    let words: Vec<String> = args
        .iter()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .collect();
    let first = words.first().map(String::as_str).unwrap_or("");
    let second = words.get(1).map(String::as_str).unwrap_or("");
    match (first, second) {
        ("profile", "add" | "use" | "remove" | "rename") => {
            Some("direct profile mutation would invalidate account isolation")
        }
        ("config", "init" | "bind") => {
            Some("direct config mutation would overwrite the isolated account configuration")
        }
        ("auth", "login" | "logout") => Some(
            "direct login/logout would bypass identity verification and account metadata updates",
        ),
        _ => None,
    }
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}
