use crate::error::{LpcError, Result};
use crate::paths::AppPaths;
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(any(not(windows), test))]
const BLOCK_START: &str = "# >>> Lark Profile Console >>>";
#[cfg(any(not(windows), test))]
const BLOCK_END: &str = "# <<< Lark Profile Console <<<";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathTakeoverReport {
    pub changed: bool,
    pub bin_dir: PathBuf,
    pub touched_files: Vec<PathBuf>,
    pub message: String,
}

/// Whether the data root this process resolved matches the one persisted for
/// the machine (Windows `HKCU\Environment\LPC_HOME`).
#[derive(Debug, Clone)]
pub enum DataRootConsistency {
    /// Safe to proceed: no persistent root, or it matches the effective root.
    Consistent,
    /// The effective root and the persistent root disagree. Proceeding would
    /// risk opening (and creating an empty) catalog in the wrong place while the
    /// real profiles live elsewhere, so startup should stop and tell the user.
    Mismatch {
        effective: PathBuf,
        persistent: PathBuf,
    },
}

/// Environment escape hatch for intentional multi-root setups (e.g. a developer
/// deliberately running against an isolated `LPC_HOME`).
const ALLOW_MISMATCH_ENV: &str = "LPC_ALLOW_HOME_MISMATCH";

/// Compares the effective data root against the machine's persistent `LPC_HOME`.
///
/// This is the startup guard against the "my profiles vanished" failure where a
/// second launch context (dev build, packaged sandbox, or a moved data folder)
/// resolves a *different* root than the one that actually holds the user's data.
/// Instead of silently creating an empty catalog there, the caller can stop and
/// surface both paths. Returns `Consistent` when the override env is set.
pub fn check_data_root_consistency(effective_root: &Path) -> DataRootConsistency {
    if std::env::var_os(ALLOW_MISMATCH_ENV).is_some() {
        return DataRootConsistency::Consistent;
    }
    #[cfg(windows)]
    {
        match windows_user_lpc_home() {
            Ok(Some(persistent)) => {
                if normalize_path(&persistent) == normalize_path(effective_root) {
                    DataRootConsistency::Consistent
                } else {
                    DataRootConsistency::Mismatch {
                        effective: effective_root.to_path_buf(),
                        persistent,
                    }
                }
            }
            // No persistent root recorded yet (first run) — nothing to disagree with.
            Ok(None) => DataRootConsistency::Consistent,
            // If the registry can't be read, fail open rather than block startup.
            Err(_) => DataRootConsistency::Consistent,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = effective_root;
        DataRootConsistency::Consistent
    }
}

#[derive(Debug, Clone)]
pub struct PathTakeover {
    paths: AppPaths,
}

impl PathTakeover {
    pub fn new(paths: AppPaths) -> Self {
        Self { paths }
    }

    pub fn install(&self) -> Result<PathTakeoverReport> {
        let bin = self.paths.bin_dir();
        fs::create_dir_all(&bin)?;
        #[cfg(windows)]
        {
            install_windows_path(self.paths.root(), &bin)
        }
        #[cfg(not(windows))]
        {
            install_shell_path(&bin)
        }
    }

    pub fn uninstall(&self) -> Result<PathTakeoverReport> {
        let bin = self.paths.bin_dir();
        #[cfg(windows)]
        {
            uninstall_windows_path(self.paths.root(), &bin)
        }
        #[cfg(not(windows))]
        {
            uninstall_shell_path(&bin)
        }
    }
}

#[cfg(windows)]
pub(crate) fn windows_user_path() -> Result<OsString> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let environment = hkcu
        .open_subkey_with_flags("Environment", KEY_READ)
        .map_err(|error| LpcError::PathTakeover(error.to_string()))?;
    let current: String = environment.get_value("Path").unwrap_or_default();
    Ok(current.into())
}

#[cfg(windows)]
pub(crate) fn windows_user_lpc_home() -> Result<Option<PathBuf>> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let environment = hkcu
        .open_subkey_with_flags("Environment", KEY_READ)
        .map_err(|error| LpcError::PathTakeover(error.to_string()))?;
    let current: String = environment.get_value("LPC_HOME").unwrap_or_default();
    Ok((!current.trim().is_empty()).then(|| PathBuf::from(current)))
}

#[cfg(windows)]
fn install_windows_path(root: &Path, bin: &Path) -> Result<PathTakeoverReport> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let environment = hkcu
        .open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
        .map_err(|error| LpcError::PathTakeover(error.to_string()))?;
    let current: String = environment.get_value("Path").unwrap_or_default();
    let (updated, path_changed) = prepend_path_entry(&current, bin, ';')?;
    if path_changed {
        environment
            .set_value("Path", &updated)
            .map_err(|error| LpcError::PathTakeover(error.to_string()))?;
    }
    let configured_home: String = environment.get_value("LPC_HOME").unwrap_or_default();
    let home_changed = normalize_os_path(OsStr::new(&configured_home)) != normalize_path(root);
    if home_changed {
        let root = root
            .to_str()
            .ok_or_else(|| LpcError::PathTakeover("LPC_HOME is not valid Unicode".into()))?;
        environment
            .set_value("LPC_HOME", &root)
            .map_err(|error| LpcError::PathTakeover(error.to_string()))?;
    }
    let changed = path_changed || home_changed;
    if changed {
        broadcast_environment_change();
    }
    Ok(PathTakeoverReport {
        changed,
        bin_dir: bin.to_path_buf(),
        touched_files: Vec::new(),
        message: if changed {
            "User PATH and LPC_HOME updated. Open a new terminal to use the managed lark-cli."
                .into()
        } else {
            "Managed lark-cli and LPC_HOME are already configured for this data root.".into()
        },
    })
}

#[cfg(windows)]
fn uninstall_windows_path(root: &Path, bin: &Path) -> Result<PathTakeoverReport> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let environment = hkcu
        .open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)
        .map_err(|error| LpcError::PathTakeover(error.to_string()))?;
    let current: String = environment.get_value("Path").unwrap_or_default();
    let (updated, path_changed) = remove_path_entry(&current, bin, ';')?;
    if path_changed {
        environment
            .set_value("Path", &updated)
            .map_err(|error| LpcError::PathTakeover(error.to_string()))?;
    }
    let configured_home: String = environment.get_value("LPC_HOME").unwrap_or_default();
    let home_removed = normalize_os_path(OsStr::new(&configured_home)) == normalize_path(root);
    if home_removed {
        environment
            .delete_value("LPC_HOME")
            .map_err(|error| LpcError::PathTakeover(error.to_string()))?;
    }
    let changed = path_changed || home_removed;
    if changed {
        broadcast_environment_change();
    }
    Ok(PathTakeoverReport {
        changed,
        bin_dir: bin.to_path_buf(),
        touched_files: Vec::new(),
        message: if changed {
            "Removed only this Lark Profile Console PATH entry and matching LPC_HOME.".into()
        } else {
            "This Lark Profile Console PATH entry and LPC_HOME were not present.".into()
        },
    })
}

/// Shows a blocking native message box so a startup abort is visible even in the
/// windowed subsystem where stderr is hidden. Falls back to stderr off Windows.
pub fn show_blocking_message(title: &str, body: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            MessageBoxW, MB_ICONERROR, MB_OK, MB_SYSTEMMODAL,
        };
        let to_wide = |value: &str| {
            OsStr::new(value)
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<u16>>()
        };
        let body_w = to_wide(body);
        let title_w = to_wide(title);
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                body_w.as_ptr(),
                title_w.as_ptr(),
                MB_OK | MB_ICONERROR | MB_SYSTEMMODAL,
            );
        }
    }
    #[cfg(not(windows))]
    {
        eprintln!("{title}: {body}");
    }
}

#[cfg(windows)]
fn broadcast_environment_change() {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };
    let environment: Vec<u16> = OsStr::new("Environment")
        .encode_wide()
        .chain(Some(0))
        .collect();
    let mut result = 0_usize;
    unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            environment.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            5000,
            &mut result,
        );
    }
}

#[cfg(not(windows))]
fn install_shell_path(bin: &Path) -> Result<PathTakeoverReport> {
    let files = shell_startup_files()?;
    let block = managed_shell_block(bin)?;
    let mut changed = false;
    let mut touched = Vec::new();
    for file in files {
        let current = fs::read_to_string(&file).unwrap_or_default();
        let cleaned = remove_managed_block(&current);
        let next = if cleaned.trim().is_empty() {
            format!("{block}\n")
        } else {
            format!("{}\n\n{block}\n", cleaned.trim_end())
        };
        if next != current {
            if let Some(parent) = file.parent() {
                fs::create_dir_all(parent)?;
            }
            crate::atomic::write_bytes_atomic(&file, next.as_bytes())?;
            changed = true;
            touched.push(file);
        }
    }
    Ok(PathTakeoverReport {
        changed,
        bin_dir: bin.to_path_buf(),
        touched_files: touched,
        message: if changed {
            "Shell startup files updated. Open a new terminal to use the managed lark-cli.".into()
        } else {
            "Shell PATH takeover is already installed.".into()
        },
    })
}

#[cfg(not(windows))]
fn uninstall_shell_path(bin: &Path) -> Result<PathTakeoverReport> {
    let files = shell_startup_files()?;
    let mut changed = false;
    let mut touched = Vec::new();
    for file in files {
        if !file.exists() {
            continue;
        }
        let current = fs::read_to_string(&file)?;
        let cleaned = remove_managed_block(&current);
        let next = if cleaned.trim().is_empty() {
            String::new()
        } else {
            format!("{}\n", cleaned.trim_end())
        };
        if next != current {
            crate::atomic::write_bytes_atomic(&file, next.as_bytes())?;
            changed = true;
            touched.push(file);
        }
    }
    Ok(PathTakeoverReport {
        changed,
        bin_dir: bin.to_path_buf(),
        touched_files: touched,
        message: if changed {
            "Removed only the managed shell block; unrelated shell settings were preserved.".into()
        } else {
            "No managed shell block was present.".into()
        },
    })
}

#[cfg(not(windows))]
fn shell_startup_files() -> Result<Vec<PathBuf>> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| LpcError::PathTakeover("HOME is not set".into()))?;
    let shell = std::env::var("SHELL").unwrap_or_default();
    let names: &[&str] = if shell.ends_with("bash") {
        &[".bash_profile", ".bashrc"]
    } else {
        // zsh is the macOS default. Unknown shells receive the zsh-compatible
        // files rather than modifying every possible startup file.
        &[".zprofile", ".zshrc"]
    };
    Ok(names.iter().map(|name| home.join(name)).collect())
}

#[cfg(not(windows))]
fn managed_shell_block(bin: &Path) -> Result<String> {
    let text = bin
        .to_str()
        .ok_or_else(|| LpcError::PathTakeover("bin path is not valid UTF-8".into()))?;
    if text.chars().any(|ch| matches!(ch, '\n' | '\r' | '"')) {
        return Err(LpcError::PathTakeover(
            "bin path cannot be safely quoted".into(),
        ));
    }
    Ok(format!(
        "{BLOCK_START}\nexport PATH=\"{text}:$PATH\"\n{BLOCK_END}"
    ))
}

#[cfg(any(not(windows), test))]
fn remove_managed_block(input: &str) -> String {
    let mut output = Vec::new();
    let mut inside = false;
    for line in input.lines() {
        if line.trim() == BLOCK_START {
            inside = true;
            continue;
        }
        if inside && line.trim() == BLOCK_END {
            inside = false;
            continue;
        }
        if !inside {
            output.push(line);
        }
    }
    output.join("\n").trim_end().to_owned()
}

pub fn prepend_path_entry(current: &str, entry: &Path, separator: char) -> Result<(String, bool)> {
    let mut parts = split_path(current, separator);
    let normalized_entry = normalize_path(entry);
    let before = parts.clone();
    parts.retain(|part| normalize_os_path(part) != normalized_entry);
    parts.insert(0, entry.as_os_str().to_owned());
    let changed = parts != before;
    Ok((join_path(&parts, separator)?, changed))
}

pub fn remove_path_entry(current: &str, entry: &Path, separator: char) -> Result<(String, bool)> {
    let mut parts = split_path(current, separator);
    let before = parts.clone();
    let normalized_entry = normalize_path(entry);
    parts.retain(|part| normalize_os_path(part) != normalized_entry);
    let changed = parts != before;
    Ok((join_path(&parts, separator)?, changed))
}

fn split_path(current: &str, separator: char) -> Vec<OsString> {
    current
        .split(separator)
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(OsString::from)
        .collect()
}

fn join_path(parts: &[OsString], separator: char) -> Result<String> {
    let mut output = String::new();
    for (index, part) in parts.iter().enumerate() {
        let part = part
            .to_str()
            .ok_or_else(|| LpcError::PathTakeover("PATH contains non-Unicode data".into()))?;
        if index > 0 {
            output.push(separator);
        }
        output.push_str(part);
    }
    Ok(output)
}

fn normalize_path(path: &Path) -> String {
    normalize_os_path(path.as_os_str())
}

fn normalize_os_path(path: &OsStr) -> String {
    let value = path
        .to_string_lossy()
        .trim_end_matches(['/', '\\'])
        .to_owned();
    #[cfg(windows)]
    {
        value.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_prepend_is_idempotent_and_moves_entry_first() {
        let entry = Path::new("C:\\LPC\\bin");
        let (first, changed) = prepend_path_entry("C:\\A;C:\\LPC\\bin;C:\\B", entry, ';').unwrap();
        assert!(changed);
        assert_eq!(first, "C:\\LPC\\bin;C:\\A;C:\\B");
        let (second, changed) = prepend_path_entry(&first, entry, ';').unwrap();
        assert!(!changed);
        assert_eq!(second, first);
    }

    #[test]
    fn managed_block_removal_preserves_user_content() {
        let source = format!("before\n{BLOCK_START}\nexport PATH=x\n{BLOCK_END}\nafter\n");
        assert_eq!(remove_managed_block(&source), "before\nafter");
    }
}
