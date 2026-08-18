use crate::{atomic, AppPaths, Result};
#[cfg(windows)]
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[cfg(windows)]
const CMD_FORWARDER: &[u8] = b"@echo off\r\n\"%~dp0lark-cli.exe\" %*\r\nexit /b %ERRORLEVEL%\r\n";
#[cfg(windows)]
const POWERSHELL_FORWARDER: &[u8] = b"& (Join-Path $PSScriptRoot 'lark-cli.exe') @args\r\n";

/// Installs the managed shim and, on Windows, the explicit command-name
/// forwarders that prevent callers of `lark-cli.cmd` or `lark-cli.ps1` from
/// falling through to an independently installed npm CLI.
///
/// Replacing the global npm `lark-cli.exe` is opt-in via
/// [`ShimInstallOptions::takeover_npm`].
pub fn install_managed_shim(source: &Path, paths: &AppPaths) -> Result<PathBuf> {
    install_managed_shim_with(source, paths, ShimInstallOptions::default())
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ShimInstallOptions {
    /// Replace `%APPDATA%\npm\node_modules\@larksuite\cli\bin\lark-cli.exe`
    /// with the managed shim. Off by default: open-source installs must not
    /// rewrite a third-party package unless the user asks.
    pub takeover_npm: bool,
}

pub fn install_managed_shim_with(
    source: &Path,
    paths: &AppPaths,
    options: ShimInstallOptions,
) -> Result<PathBuf> {
    #[cfg(windows)]
    let destination = paths.bin_dir().join("lark-cli.exe");
    #[cfg(not(windows))]
    let destination = paths.bin_dir().join("lark-cli");

    let shim = std::fs::read(source)?;
    write_if_changed(&destination, &shim)?;

    #[cfg(windows)]
    {
        write_if_changed(&paths.bin_dir().join("lark-cli.cmd"), CMD_FORWARDER)?;
        write_if_changed(&paths.bin_dir().join("lark-cli.ps1"), POWERSHELL_FORWARDER)?;
        if options.takeover_npm {
            repair_global_npm_binary(&destination, paths)?;
        }
    }

    #[cfg(unix)]
    {
        let _ = options;
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&destination)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&destination, permissions)?;
    }

    Ok(destination)
}

fn write_if_changed(path: &Path, bytes: &[u8]) -> Result<()> {
    if std::fs::read(path).is_ok_and(|current| current == bytes) {
        return Ok(());
    }
    atomic::write_bytes_atomic(path, bytes)
}

#[cfg(windows)]
fn repair_global_npm_binary(shim: &Path, paths: &AppPaths) -> Result<()> {
    let Some(npm_root) = windows_npm_root(paths) else {
        return Ok(());
    };
    let direct_binary = npm_root
        .join("node_modules")
        .join("@larksuite")
        .join("cli")
        .join("bin")
        .join("lark-cli.exe");
    if !direct_binary.is_file() {
        return Ok(());
    }

    let shim_bytes = std::fs::read(shim)?;
    let direct_bytes = std::fs::read(&direct_binary)?;
    if direct_bytes == shim_bytes {
        return Ok(());
    }

    let digest = hex::encode(Sha256::digest(&direct_bytes));
    let backup = paths
        .runtime_dir()
        .join("legacy-cli-backups")
        .join(format!("npm-lark-cli-{}.exe", &digest[..16]));
    write_if_changed(&backup, &direct_bytes)?;
    write_if_changed(&direct_binary, &shim_bytes)?;
    tracing::warn!(
        target = %direct_binary.display(),
        backup = %backup.display(),
        "direct npm lark-cli route replaced with managed shim"
    );
    Ok(())
}

#[cfg(windows)]
fn windows_npm_root(paths: &AppPaths) -> Option<PathBuf> {
    if let Some(prefix) = std::env::var_os("NPM_CONFIG_PREFIX").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(prefix));
    }

    let discovered = AppPaths::discover().ok()?;
    if paths.root() != discovered.root() {
        return None;
    }
    std::env::var_os("APPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|root| root.join("npm"))
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::{install_managed_shim, install_managed_shim_with, ShimInstallOptions};
    #[cfg(windows)]
    use crate::AppPaths;
    #[cfg(windows)]
    use std::sync::{Mutex, OnceLock};

    #[cfg(windows)]
    fn environment_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[cfg(windows)]
    #[test]
    fn install_replaces_the_global_npm_package_binary_with_the_managed_shim() {
        let _environment = environment_lock();
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("lpc-home"));
        let source = temp.path().join("lark-cli.exe");
        std::fs::write(&source, b"managed-shim").unwrap();

        let npm_root = temp.path().join("npm");
        let direct_binary = npm_root
            .join("node_modules")
            .join("@larksuite")
            .join("cli")
            .join("bin")
            .join("lark-cli.exe");
        std::fs::create_dir_all(direct_binary.parent().unwrap()).unwrap();
        std::fs::write(&direct_binary, b"legacy-official-cli").unwrap();

        let original_prefix = std::env::var_os("NPM_CONFIG_PREFIX");
        std::env::set_var("NPM_CONFIG_PREFIX", &npm_root);
        let result =
            install_managed_shim_with(&source, &paths, ShimInstallOptions { takeover_npm: true });
        match original_prefix {
            Some(value) => std::env::set_var("NPM_CONFIG_PREFIX", value),
            None => std::env::remove_var("NPM_CONFIG_PREFIX"),
        }
        result.unwrap();

        assert_eq!(std::fs::read(&direct_binary).unwrap(), b"managed-shim");
        let backup_dir = paths.runtime_dir().join("legacy-cli-backups");
        let backups = std::fs::read_dir(backup_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read(&backups[0]).unwrap(), b"legacy-official-cli");
    }

    #[cfg(windows)]
    #[test]
    fn default_install_does_not_replace_the_global_npm_package_binary() {
        let _environment = environment_lock();
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("lpc-home"));
        let source = temp.path().join("lark-cli.exe");
        std::fs::write(&source, b"managed-shim").unwrap();

        let npm_root = temp.path().join("npm");
        let direct_binary = npm_root
            .join("node_modules")
            .join("@larksuite")
            .join("cli")
            .join("bin")
            .join("lark-cli.exe");
        std::fs::create_dir_all(direct_binary.parent().unwrap()).unwrap();
        std::fs::write(&direct_binary, b"legacy-official-cli").unwrap();

        let original_prefix = std::env::var_os("NPM_CONFIG_PREFIX");
        std::env::set_var("NPM_CONFIG_PREFIX", &npm_root);
        let result = install_managed_shim(&source, &paths);
        match original_prefix {
            Some(value) => std::env::set_var("NPM_CONFIG_PREFIX", value),
            None => std::env::remove_var("NPM_CONFIG_PREFIX"),
        }
        result.unwrap();

        assert_eq!(
            std::fs::read(&direct_binary).unwrap(),
            b"legacy-official-cli"
        );
        assert!(!paths.runtime_dir().join("legacy-cli-backups").exists());
    }
}
