//! Inspect and pin the per-user Windows Run key for the installed desktop exe.
//!
//! Tauri autostart writes this key from `current_exe()`. A later launch from a
//! cargo target is refused, so the Run key can stay pointed at an older copy.
//! Pinning rewrites it to the installed path when the installed exe starts.

#[cfg(windows)]
use crate::error::LpcError;
use crate::error::Result;
use std::path::{Path, PathBuf};

pub const DESKTOP_EXE_FILE_NAME: &str = "lark-profile-console.exe";
pub const AUTOSTART_VALUE_NAME: &str = "Lark Profile Console";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

pub fn is_cargo_target_build_exe(path: &Path) -> bool {
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

pub fn is_packaged_app_virtualized_exe(path: &Path) -> bool {
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

pub fn expected_installed_desktop_exe() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(
        PathBuf::from(local)
            .join("Lark Profile Console")
            .join(DESKTOP_EXE_FILE_NAME),
    )
}

pub fn exe_from_run_command(command: &str) -> Option<PathBuf> {
    let command = command.trim();
    if command.is_empty() {
        return None;
    }
    if let Some(rest) = command.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some(PathBuf::from(&rest[..end]));
    }
    command.split_whitespace().next().map(PathBuf::from)
}

pub fn run_command_for_exe(exe: &Path, extra_args: &[&str]) -> String {
    let mut command = format!("\"{}\"", exe.display());
    for arg in extra_args {
        command.push(' ');
        command.push_str(arg);
    }
    command
}

pub fn command_targets_desktop(command: &str) -> bool {
    exe_from_run_command(command)
        .and_then(|path| {
            path.file_name().map(|name| {
                name.to_string_lossy()
                    .eq_ignore_ascii_case(DESKTOP_EXE_FILE_NAME)
            })
        })
        .unwrap_or(false)
}

fn same_exe(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutostartRunEntry {
    pub value_name: String,
    pub command: String,
    pub exe_path: Option<PathBuf>,
}

#[cfg(windows)]
pub fn list_desktop_run_entries() -> Result<Vec<AutostartRunEntry>> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = match hkcu.open_subkey(RUN_KEY) {
        Ok(key) => key,
        Err(_) => return Ok(Vec::new()),
    };
    let mut entries = Vec::new();
    for name in key
        .enum_values()
        .filter_map(|item| item.ok().map(|(name, _)| name))
    {
        let command: String = match key.get_value(&name) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if !command_targets_desktop(&command) && name != AUTOSTART_VALUE_NAME {
            continue;
        }
        entries.push(AutostartRunEntry {
            exe_path: exe_from_run_command(&command),
            value_name: name,
            command,
        });
    }
    Ok(entries)
}

#[cfg(not(windows))]
pub fn list_desktop_run_entries() -> Result<Vec<AutostartRunEntry>> {
    Ok(Vec::new())
}

/// Rewrite every LPC Run value so it launches `exe` with `extra_args`.
/// Creates the canonical value when none exists.
#[cfg(windows)]
pub fn pin_user_run_autostart(exe: &Path, extra_args: &[&str]) -> Result<()> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_ALL_ACCESS};
    use winreg::RegKey;

    if is_cargo_target_build_exe(exe) || is_packaged_app_virtualized_exe(exe) {
        return Err(LpcError::Internal(
            "refusing to pin autostart to a cargo target or virtualized exe".into(),
        ));
    }
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey_with_flags(RUN_KEY, KEY_ALL_ACCESS)?;
    let command = run_command_for_exe(exe, extra_args);
    let mut names: Vec<String> = list_desktop_run_entries()?
        .into_iter()
        .map(|entry| entry.value_name)
        .collect();
    if !names.iter().any(|name| name == AUTOSTART_VALUE_NAME) {
        names.push(AUTOSTART_VALUE_NAME.to_owned());
    }
    for name in names {
        key.set_value(&name, &command)?;
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn pin_user_run_autostart(exe: &Path, extra_args: &[&str]) -> Result<()> {
    let _ = (exe, extra_args);
    Ok(())
}

pub fn autostart_points_at_install(entries: &[AutostartRunEntry], expected: &Path) -> bool {
    entries.iter().any(|entry| {
        entry
            .exe_path
            .as_deref()
            .is_some_and(|path| same_exe(path, expected) && !is_cargo_target_build_exe(path))
    })
}

pub fn autostart_uses_cargo_target(entries: &[AutostartRunEntry]) -> bool {
    entries.iter().any(|entry| {
        entry
            .exe_path
            .as_deref()
            .is_some_and(is_cargo_target_build_exe)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_and_bare_run_commands() {
        assert_eq!(
            exe_from_run_command(
                r#""C:\Users\me\AppData\Local\Lark Profile Console\lark-profile-console.exe" --hidden"#
            ),
            Some(PathBuf::from(
                r"C:\Users\me\AppData\Local\Lark Profile Console\lark-profile-console.exe"
            ))
        );
        assert_eq!(
            exe_from_run_command(r"C:\app\lark-profile-console.exe --hidden"),
            Some(PathBuf::from(r"C:\app\lark-profile-console.exe"))
        );
    }

    #[test]
    fn detects_desktop_command_by_file_name() {
        assert!(command_targets_desktop(
            r#""C:\Local\Lark Profile Console\lark-profile-console.exe" --hidden"#
        ));
        assert!(!command_targets_desktop(r#""C:\Other\foo.exe""#));
    }

    #[test]
    fn cargo_target_and_install_paths() {
        assert!(is_cargo_target_build_exe(Path::new(
            r"D:\repo\target\release\lark-profile-console.exe"
        )));
        assert!(is_cargo_target_build_exe(Path::new(
            r"D:\repo\target\x86_64-pc-windows-msvc\release\lark-profile-console.exe"
        )));
        assert!(!is_cargo_target_build_exe(Path::new(
            r"C:\Users\me\AppData\Local\Lark Profile Console\lark-profile-console.exe"
        )));
        assert!(is_packaged_app_virtualized_exe(Path::new(
            r"C:\Users\me\AppData\Local\Packages\OpenAI.CodexBeta_123\LocalCache\Local\Lark Profile Console\lark-profile-console.exe"
        )));
    }

    #[test]
    fn install_match_rejects_cargo_target() {
        let expected =
            Path::new(r"C:\Users\me\AppData\Local\Lark Profile Console\lark-profile-console.exe");
        let cargo = AutostartRunEntry {
            value_name: AUTOSTART_VALUE_NAME.into(),
            command: r#""D:\repo\target\release\lark-profile-console.exe" --hidden"#.into(),
            exe_path: Some(PathBuf::from(
                r"D:\repo\target\release\lark-profile-console.exe",
            )),
        };
        assert!(autostart_uses_cargo_target(&[cargo.clone()]));
        assert!(!autostart_points_at_install(&[cargo], expected));
        let installed = AutostartRunEntry {
            value_name: AUTOSTART_VALUE_NAME.into(),
            command: run_command_for_exe(expected, &["--hidden"]),
            exe_path: Some(expected.to_path_buf()),
        };
        assert!(autostart_points_at_install(&[installed], expected));
    }

    #[test]
    fn pin_refuses_cargo_target_and_virtualized_paths() {
        let cargo = Path::new(r"D:\repo\target\release\lark-profile-console.exe");
        let virtualized = Path::new(
            r"C:\Users\me\AppData\Local\Packages\OpenAI.CodexBeta_123\LocalCache\Local\Lark Profile Console\lark-profile-console.exe",
        );
        let cargo_err = pin_user_run_autostart(cargo, &["--hidden"]);
        let virt_err = pin_user_run_autostart(virtualized, &["--hidden"]);
        #[cfg(windows)]
        {
            assert!(cargo_err.is_err());
            assert!(virt_err.is_err());
        }
        #[cfg(not(windows))]
        {
            assert!(cargo_err.is_ok());
            assert!(virt_err.is_ok());
        }
    }
}
