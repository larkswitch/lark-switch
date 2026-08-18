use crate::error::Result;
use crate::locking::RoutingGate;
#[cfg(windows)]
use crate::path_takeover::{windows_user_lpc_home, windows_user_path};
use crate::redact::{redact_with, RedactionLevel};
use crate::store::StateStore;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticCheck {
    pub id: String,
    pub status: DiagnosticStatus,
    pub summary: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticReport {
    pub generated_at: DateTime<Utc>,
    pub checks: Vec<DiagnosticCheck>,
}

pub fn run_diagnostics(store: &StateStore) -> Result<DiagnosticReport> {
    run_diagnostics_with(store, RedactionLevel::Local)
}

/// Use [`RedactionLevel::Outbound`] when the report is going somewhere other
/// than the machine it describes — a ticket, a chat, an upload.
pub fn run_diagnostics_with(store: &StateStore, level: RedactionLevel) -> Result<DiagnosticReport> {
    let mut checks = Vec::new();
    let state = store.load_state()?;
    let catalog = store.load_catalog()?;
    let gate = RoutingGate::new(store.paths().clone());
    let counts = gate.running_counts()?;

    checks.push(DiagnosticCheck {
        id: "data_layout".into(),
        status: if store.paths().root().is_dir() {
            DiagnosticStatus::Pass
        } else {
            DiagnosticStatus::Fail
        },
        summary: "Local data directory".into(),
        detail: store.paths().root().display().to_string(),
    });

    #[cfg(windows)]
    checks.push(match windows_user_lpc_home() {
        Ok(configured) => lpc_home_route_check(store.paths().root(), configured.as_deref(), None),
        Err(error) => lpc_home_route_check(
            store.paths().root(),
            None,
            Some(&format!("Could not read persistent LPC_HOME: {error}")),
        ),
    });

    match &state.managed_cli_path {
        Some(path) if path.is_file() => checks.push(DiagnosticCheck {
            id: "runtime".into(),
            status: DiagnosticStatus::Pass,
            summary: "Managed official lark-cli is present".into(),
            detail: format!(
                "version={}, path={}",
                state.managed_cli_version.as_deref().unwrap_or("unknown"),
                path.display()
            ),
        }),
        Some(path) => checks.push(DiagnosticCheck {
            id: "runtime".into(),
            status: DiagnosticStatus::Fail,
            summary: "Managed official lark-cli is missing".into(),
            detail: path.display().to_string(),
        }),
        None => checks.push(DiagnosticCheck {
            id: "runtime".into(),
            status: DiagnosticStatus::Fail,
            summary: "Managed official lark-cli is not installed".into(),
            detail: "Install the recommended runtime from the desktop console.".into(),
        }),
    }

    let expected = shim_name(store.paths().bin_dir());
    checks.push(path_route_check(&expected));

    let keychain = crate::keychain_guard::inspect_keychain();
    let account_count = catalog.accounts.len();
    let watch = crate::keychain_watch::observe_keychain_slots(
        store.paths(),
        &keychain,
        account_count,
        false,
    )
    .ok();
    checks.push(keychain_slot_check(
        &keychain,
        watch.as_ref(),
        account_count,
    ));

    checks.push(autostart_target_check());

    checks.push(catalog_consistency_check(store));

    for account in &catalog.accounts {
        let running = counts.get(&account.id).copied().unwrap_or(0);
        let config = account.config_dir.join("config.json");
        checks.push(DiagnosticCheck {
            id: format!("account_{}", account.id),
            status: if config.is_file() {
                DiagnosticStatus::Pass
            } else {
                DiagnosticStatus::Fail
            },
            summary: format!("Account {}", account.display_name),
            detail: format!(
                "config={}, runningCommands={}, health={:?}",
                config.display(),
                running,
                account.health
            ),
        });
    }

    // Summaries interpolate account labels, so they need the same treatment as
    // details; redacting only the detail left half the report unscreened.
    for check in &mut checks {
        check.summary = redact_with(level, &check.summary);
        check.detail = redact_with(level, &check.detail);
    }
    Ok(DiagnosticReport {
        generated_at: Utc::now(),
        checks,
    })
}

fn keychain_slot_check(
    keychain: &crate::keychain_guard::KeychainStatus,
    watch: Option<&crate::keychain_watch::KeychainWatchEvent>,
    account_count: usize,
) -> DiagnosticCheck {
    let expected = crate::keychain_watch::expected_keychain_slots(account_count);
    if !keychain.platform_supported {
        return DiagnosticCheck {
            id: "official_cli_keychain".into(),
            status: DiagnosticStatus::Warn,
            summary: "Official CLI keychain durability checks are Windows-only".into(),
            detail: keychain.detail.clone(),
        };
    }
    if keychain.empty {
        return DiagnosticCheck {
            id: "official_cli_keychain".into(),
            status: DiagnosticStatus::Fail,
            summary: "Official CLI keychain is EMPTY (tokens wiped)".into(),
            detail: keychain.detail.clone(),
        };
    }
    if let Some(event) = watch {
        if let crate::keychain_watch::KeychainWatchKind::Cliff { from, to } = event.kind {
            return DiagnosticCheck {
                id: "official_cli_keychain".into(),
                status: DiagnosticStatus::Fail,
                summary: format!("Official CLI keychain dropped from {from} to {to} slots"),
                detail: event
                    .cliff_message()
                    .unwrap_or_else(|| keychain.detail.clone()),
            };
        }
        if event.deficit {
            return DiagnosticCheck {
                id: "official_cli_keychain".into(),
                status: DiagnosticStatus::Fail,
                summary: format!(
                    "Official CLI keychain has {} slot(s); expected at least {} for {} account(s)",
                    keychain.entry_count, expected, account_count
                ),
                detail: format!(
                    "{}. Missing slots usually mean user tokens were deleted. \
                     Dry-run restore-lark-keychain.ps1; never whole-hive import.",
                    keychain.detail
                ),
            };
        }
    }
    DiagnosticCheck {
        id: "official_cli_keychain".into(),
        status: DiagnosticStatus::Pass,
        summary: format!(
            "Official CLI keychain has {} credential slot(s)",
            keychain.entry_count
        ),
        detail: keychain.detail.clone(),
    }
}

fn autostart_target_check() -> DiagnosticCheck {
    let expected = crate::autostart::expected_installed_desktop_exe();
    match crate::autostart::list_desktop_run_entries() {
        Ok(entries) => {
            if crate::autostart::autostart_uses_cargo_target(&entries) {
                return DiagnosticCheck {
                    id: "autostart_target".into(),
                    status: DiagnosticStatus::Fail,
                    summary: "Autostart points at a cargo target build".into(),
                    detail: format_autostart_entries(&entries),
                };
            }
            if let Some(expected) = expected.as_deref() {
                if entries.is_empty() {
                    return DiagnosticCheck {
                        id: "autostart_target".into(),
                        status: DiagnosticStatus::Warn,
                        summary: "No autostart Run entry for the desktop app".into(),
                        detail: format!("expected {}", expected.display()),
                    };
                }
                if crate::autostart::autostart_points_at_install(&entries, expected) {
                    return DiagnosticCheck {
                        id: "autostart_target".into(),
                        status: DiagnosticStatus::Pass,
                        summary: "Autostart launches the installed desktop app".into(),
                        detail: format_autostart_entries(&entries),
                    };
                }
                return DiagnosticCheck {
                    id: "autostart_target".into(),
                    status: DiagnosticStatus::Fail,
                    summary: "Autostart does not launch the installed desktop app".into(),
                    detail: format!(
                        "expected {}\n{}",
                        expected.display(),
                        format_autostart_entries(&entries)
                    ),
                };
            }
            DiagnosticCheck {
                id: "autostart_target".into(),
                status: DiagnosticStatus::Warn,
                summary: "Could not resolve the installed desktop path".into(),
                detail: format_autostart_entries(&entries),
            }
        }
        Err(error) => DiagnosticCheck {
            id: "autostart_target".into(),
            status: DiagnosticStatus::Warn,
            summary: "Autostart target unknown".into(),
            detail: error.to_string(),
        },
    }
}

fn format_autostart_entries(entries: &[crate::autostart::AutostartRunEntry]) -> String {
    if entries.is_empty() {
        return "no matching HKCU Run values".into();
    }
    entries
        .iter()
        .map(|entry| {
            format!(
                "{} => {}",
                entry.value_name,
                entry
                    .exe_path
                    .as_ref()
                    .map_or_else(|| entry.command.clone(), |path| path.display().to_string())
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// A config directory nobody claims means the catalog lost an entry. That is
/// the 2026-07-17 signature, and it stayed invisible because nothing compared
/// the index against the disk.
fn catalog_consistency_check(store: &StateStore) -> DiagnosticCheck {
    match crate::consistency::check_consistency(store) {
        Ok(report) if report.is_consistent() => DiagnosticCheck {
            id: "catalog_consistency".into(),
            status: DiagnosticStatus::Pass,
            summary: "Catalog matches disk".into(),
            detail: report.summary(),
        },
        Ok(report) => DiagnosticCheck {
            id: "catalog_consistency".into(),
            status: DiagnosticStatus::Fail,
            summary: "Catalog does not match disk".into(),
            detail: format!(
                "{} — do not delete anything; restore from \
                 %USERPROFILE%\\Documents\\LarkProfileConsoleBackups if entries are missing.",
                report.summary()
            ),
        },
        Err(error) => DiagnosticCheck {
            id: "catalog_consistency".into(),
            status: DiagnosticStatus::Warn,
            summary: "Catalog consistency unknown".into(),
            detail: error.to_string(),
        },
    }
}

fn path_route_check(expected: &Path) -> DiagnosticCheck {
    let current = resolve_command_candidates("lark-cli");
    #[cfg(windows)]
    {
        match windows_user_path() {
            Ok(path) => {
                let pathext = std::env::var_os("PATHEXT");
                let configured =
                    resolve_windows_command_candidates_from(&path, "lark-cli", pathext.as_deref());
                windows_path_route_check(expected, &configured, &current, None)
            }
            Err(error) => windows_path_route_check(
                expected,
                &[],
                &current,
                Some(&format!("Could not read the persistent user PATH: {error}")),
            ),
        }
    }
    #[cfg(not(windows))]
    {
        let route_ok = route_matches(&current, expected);
        DiagnosticCheck {
            id: "path_route".into(),
            status: if route_ok {
                DiagnosticStatus::Pass
            } else if current.is_empty() {
                DiagnosticStatus::Fail
            } else {
                DiagnosticStatus::Warn
            },
            summary: if route_ok {
                "Terminal routes lark-cli through Lark Profile Console".into()
            } else {
                "Terminal does not currently resolve the managed shim first".into()
            },
            detail: format_candidates(&current),
        }
    }
}

#[cfg(any(windows, test))]
fn lpc_home_route_check(
    expected: &Path,
    configured: Option<&Path>,
    configured_error: Option<&str>,
) -> DiagnosticCheck {
    let matches =
        configured_error.is_none() && configured.is_some_and(|path| same_path(path, expected));
    let status = if matches {
        DiagnosticStatus::Pass
    } else if configured.is_some() {
        DiagnosticStatus::Fail
    } else {
        DiagnosticStatus::Warn
    };
    let detail = configured_error.map(str::to_owned).unwrap_or_else(|| {
        format!(
            "configuredLpcHome={}; expectedLpcHome={}",
            configured
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "unset".into()),
            expected.display()
        )
    });
    DiagnosticCheck {
        id: "lpc_home_route".into(),
        status,
        summary: if matches {
            "Persistent LPC_HOME selects this data root".into()
        } else if configured.is_some() {
            "Persistent LPC_HOME selects a different data root".into()
        } else {
            "Persistent LPC_HOME is not configured".into()
        },
        detail,
    }
}

fn route_matches(candidates: &[PathBuf], expected: &Path) -> bool {
    candidates
        .first()
        .is_some_and(|path| same_path(path, expected))
}

#[cfg(any(windows, test))]
fn explicit_windows_routes_match(candidates: &[PathBuf], expected: &Path) -> bool {
    let Some(expected_dir) = expected.parent() else {
        return false;
    };
    ["cmd", "ps1"].into_iter().all(|extension| {
        candidates
            .iter()
            .find(|path| {
                path.extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case(extension))
            })
            .and_then(|path| path.parent())
            .is_some_and(|parent| same_path(parent, expected_dir))
    })
}

fn format_candidates(candidates: &[PathBuf]) -> String {
    if candidates.is_empty() {
        return "none".into();
    }
    candidates
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(any(windows, test))]
fn windows_path_route_check(
    expected: &Path,
    configured: &[PathBuf],
    current: &[PathBuf],
    configured_error: Option<&str>,
) -> DiagnosticCheck {
    let configured_base_ok = configured_error.is_none() && route_matches(configured, expected);
    let configured_explicit_ok = explicit_windows_routes_match(configured, expected);
    let configured_ok = configured_base_ok && configured_explicit_ok;
    let current_base_ok = route_matches(current, expected);
    let current_explicit_ok = explicit_windows_routes_match(current, expected);
    let current_ok = current_base_ok && current_explicit_ok;
    let (status, summary) = if configured_ok {
        (
            DiagnosticStatus::Pass,
            if current_ok {
                "Terminal routes lark-cli through Lark Profile Console"
            } else {
                "PATH takeover is configured for newly launched terminals"
            },
        )
    } else if configured_base_ok && !configured_explicit_ok {
        (
            DiagnosticStatus::Warn,
            "Some explicit Windows command names bypass Lark Profile Console",
        )
    } else if current_base_ok || !configured.is_empty() {
        (
            DiagnosticStatus::Warn,
            "Persistent user PATH does not resolve the managed shim first",
        )
    } else {
        (
            DiagnosticStatus::Fail,
            "Persistent user PATH does not contain a working managed shim",
        )
    };

    let mut detail = format!(
        "configuredUserPath={}; currentProcessPath={}",
        format_candidates(configured),
        format_candidates(current)
    );
    if configured_ok && !current_ok {
        detail.push_str(
            "; the desktop app inherited an older PATH; existing processes keep their inherited PATH, while newly launched terminals use the configured route",
        );
    }
    if let Some(error) = configured_error {
        detail.push_str("; ");
        detail.push_str(error);
    }

    DiagnosticCheck {
        id: "path_route".into(),
        status,
        summary: summary.into(),
        detail,
    }
}

fn shim_name(bin: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        bin.join("lark-cli.exe")
    }
    #[cfg(not(windows))]
    {
        bin.join("lark-cli")
    }
}

fn resolve_command_candidates(name: &str) -> Vec<PathBuf> {
    let Some(path) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    #[cfg(windows)]
    {
        let pathext = std::env::var_os("PATHEXT");
        resolve_windows_command_candidates_from(&path, name, pathext.as_deref())
    }
    #[cfg(not(windows))]
    {
        resolve_command_candidates_from(&path, name, &[""])
    }
}

#[cfg(any(windows, test))]
fn resolve_windows_command_candidates_from(
    path: &OsStr,
    name: &str,
    pathext: Option<&OsStr>,
) -> Vec<PathBuf> {
    const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

    let raw_extensions = pathext
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsStr::new(DEFAULT_PATHEXT));
    let mut extensions = Vec::new();
    let mut seen = HashSet::new();
    for extension in raw_extensions.to_string_lossy().split(';') {
        let extension = extension
            .trim()
            .trim_start_matches('.')
            .to_ascii_lowercase();
        if !extension.is_empty() && seen.insert(extension.clone()) {
            extensions.push(extension);
        }
    }
    if extensions.is_empty() {
        extensions.extend(["com", "exe", "bat", "cmd"].map(str::to_owned));
        seen.extend(extensions.iter().cloned());
    }
    if seen.insert("ps1".into()) {
        extensions.push("ps1".into());
    }
    extensions.push(String::new());

    resolve_command_candidates_from(path, name, &extensions)
}

fn resolve_command_candidates_from<S: AsRef<str>>(
    path: &OsStr,
    name: &str,
    extensions: &[S],
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    for directory in std::env::split_paths(path) {
        for extension in extensions {
            let extension = extension.as_ref();
            let candidate = if extension.is_empty() {
                directory.join(name)
            } else {
                directory.join(format!("{name}.{extension}"))
            };
            if !candidate.is_file() {
                continue;
            }
            let key = candidate
                .canonicalize()
                .unwrap_or_else(|_| candidate.clone());
            if seen.insert(key) {
                candidates.push(candidate);
            }
        }
    }
    candidates
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discovers_windows_wrappers_and_extensionless_commands() {
        let temp = tempfile::tempdir().unwrap();
        let npm = temp.path().join("npm");
        fs::create_dir_all(&npm).unwrap();
        let cmd = npm.join("lark-cli.cmd");
        let ps1 = npm.join("lark-cli.ps1");
        let extensionless = npm.join("lark-cli");
        fs::write(&cmd, b"").unwrap();
        fs::write(&ps1, b"").unwrap();
        fs::write(&extensionless, b"").unwrap();

        let path = std::env::join_paths([&npm]).unwrap();
        let candidates =
            resolve_command_candidates_from(&path, "lark-cli", &["exe", "cmd", "bat", "ps1", ""]);

        assert_eq!(candidates, vec![cmd, ps1, extensionless]);
    }

    #[test]
    fn preserves_path_priority_and_deduplicates_directories() {
        let temp = tempfile::tempdir().unwrap();
        let lpc = temp.path().join("lpc");
        let npm = temp.path().join("npm");
        fs::create_dir_all(&lpc).unwrap();
        fs::create_dir_all(&npm).unwrap();
        let shim = lpc.join("lark-cli.exe");
        let wrapper = npm.join("lark-cli.cmd");
        fs::write(&shim, b"").unwrap();
        fs::write(&wrapper, b"").unwrap();

        let path = std::env::join_paths([&lpc, &npm, &lpc]).unwrap();
        let candidates =
            resolve_command_candidates_from(&path, "lark-cli", &["exe", "cmd", "bat", "ps1", ""]);

        assert_eq!(candidates, vec![shim, wrapper]);
    }

    #[test]
    fn follows_pathext_within_each_path_directory_and_keeps_diagnostic_fallbacks() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();

        let first_cmd = first.join("lark-cli.cmd");
        let first_exe = first.join("lark-cli.exe");
        let first_bat = first.join("lark-cli.bat");
        let first_ps1 = first.join("lark-cli.ps1");
        let first_extensionless = first.join("lark-cli");
        let second_cmd = second.join("lark-cli.cmd");
        for candidate in [
            &first_cmd,
            &first_exe,
            &first_bat,
            &first_ps1,
            &first_extensionless,
            &second_cmd,
        ] {
            fs::write(candidate, b"").unwrap();
        }

        let path = std::env::join_paths([&first, &second]).unwrap();
        let candidates = resolve_windows_command_candidates_from(
            &path,
            "lark-cli",
            Some(OsStr::new(".CMD;.EXE;.BAT")),
        );

        assert_eq!(
            candidates,
            vec![
                first_cmd,
                first_exe,
                first_bat,
                first_ps1,
                first_extensionless,
                second_cmd,
            ]
        );
    }

    #[test]
    fn windows_diagnostics_prefer_persistent_path_over_stale_process_path() {
        let expected = PathBuf::from("C:\\LPC\\bin\\lark-cli.exe");
        let configured = vec![
            expected.clone(),
            PathBuf::from("C:\\LPC\\bin\\lark-cli.cmd"),
            PathBuf::from("C:\\LPC\\bin\\lark-cli.ps1"),
        ];

        let check = windows_path_route_check(&expected, &configured, &[], None);

        assert_eq!(check.status, DiagnosticStatus::Pass);
        assert_eq!(
            check.summary,
            "PATH takeover is configured for newly launched terminals"
        );
        assert!(check.detail.contains("desktop app inherited an older PATH"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_diagnostics_warn_when_explicit_cmd_and_ps1_bypass_the_managed_bin() {
        let expected = PathBuf::from("C:\\LPC\\bin\\lark-cli.exe");
        let configured = vec![
            expected.clone(),
            PathBuf::from("C:\\Users\\me\\AppData\\Roaming\\npm\\lark-cli.cmd"),
            PathBuf::from("C:\\Users\\me\\AppData\\Roaming\\npm\\lark-cli.ps1"),
        ];

        let check = windows_path_route_check(&expected, &configured, &configured, None);

        assert_eq!(check.status, DiagnosticStatus::Warn);
        assert!(check.summary.contains("explicit Windows command names"));
    }

    #[test]
    fn windows_diagnostics_warn_when_only_current_process_has_the_route() {
        let expected = PathBuf::from("C:\\LPC\\bin\\lark-cli.exe");

        let check = windows_path_route_check(&expected, &[], &[expected.clone()], None);

        assert_eq!(check.status, DiagnosticStatus::Warn);
        assert_eq!(
            check.summary,
            "Persistent user PATH does not resolve the managed shim first"
        );
    }

    #[test]
    fn lpc_home_diagnostics_fail_for_a_different_persistent_root() {
        let expected = Path::new("C:\\LPC\\canonical");
        let configured = Path::new("C:\\LPC\\stale");

        let check = lpc_home_route_check(expected, Some(configured), None);

        assert_eq!(check.status, DiagnosticStatus::Fail);
        assert_eq!(
            check.summary,
            "Persistent LPC_HOME selects a different data root"
        );
    }

    fn sample_keychain(count: usize, empty: bool) -> crate::keychain_guard::KeychainStatus {
        crate::keychain_guard::KeychainStatus {
            platform_supported: true,
            key_exists: count > 0,
            entry_count: count,
            empty,
            detail: format!("{count} credential slots"),
        }
    }

    #[test]
    fn keychain_cliff_fifteen_to_four_fails_even_when_not_empty() {
        let keychain = sample_keychain(4, false);
        let event = crate::keychain_watch::classify_keychain_delta(Some(15), 4, 13);
        let check = keychain_slot_check(&keychain, Some(&event), 13);
        assert_eq!(check.status, DiagnosticStatus::Fail);
        assert_eq!(check.id, "official_cli_keychain");
        assert!(check.summary.contains("15"));
        assert!(check.summary.contains("4"));
        assert!(check.detail.contains("不要整表导入注册表"));
        assert!(!check.summary.to_ascii_lowercase().contains("empty"));
    }

    #[test]
    fn keychain_empty_keeps_the_empty_copy_even_after_a_cliff() {
        let keychain = sample_keychain(0, true);
        let event = crate::keychain_watch::classify_keychain_delta(Some(15), 0, 13);
        let check = keychain_slot_check(&keychain, Some(&event), 13);
        assert_eq!(check.status, DiagnosticStatus::Fail);
        assert!(check.summary.contains("EMPTY"));
        assert!(!check.summary.contains("dropped from"));
    }

    #[test]
    fn keychain_rise_does_not_fail_when_slots_match_accounts() {
        let keychain = sample_keychain(15, false);
        let event = crate::keychain_watch::classify_keychain_delta(Some(4), 15, 13);
        assert!(event.should_force_reauth_verify());
        let check = keychain_slot_check(&keychain, Some(&event), 13);
        assert_eq!(check.status, DiagnosticStatus::Pass);
    }

    #[cfg(windows)]
    #[test]
    fn autostart_cargo_target_fails_the_diagnostic() {
        let expected =
            Path::new(r"C:\Users\me\AppData\Local\Lark Profile Console\lark-profile-console.exe");
        let cargo = crate::autostart::AutostartRunEntry {
            value_name: crate::autostart::AUTOSTART_VALUE_NAME.into(),
            command: r#""D:\repo\target\release\lark-profile-console.exe" --hidden"#.into(),
            exe_path: Some(PathBuf::from(
                r"D:\repo\target\release\lark-profile-console.exe",
            )),
        };
        assert!(crate::autostart::autostart_uses_cargo_target(&[
            cargo.clone()
        ]));
        assert!(!crate::autostart::autostart_points_at_install(
            &[cargo],
            expected
        ));
    }
}
