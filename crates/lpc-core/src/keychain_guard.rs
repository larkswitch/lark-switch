//! Official CLI credential-store durability helpers (Windows keychain registry).
//!
//! LPC never parses token semantics. This module only:
//! - counts registry value slots under the official CLI keychain path,
//! - snapshots the registry branch to a `.reg` file for disaster recovery,
//! - reports empty/missing keychain as a diagnostic fail.
//!
//! Snapshots are written **in-process via winreg** (not `reg.exe`). Spawning
//! `reg export` from a `windows_subsystem = "windows"` GUI process fails with
//! Win32 0x800700E8 / ERROR_NO_DATA ("pipe is being closed") because console
//! stdio pipes are torn down. Keep this path free of child `reg` processes.

use crate::error::{LpcError, Result};
use chrono::Utc;
use directories::UserDirs;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_RETAINED_KEYCHAIN_BACKUPS: usize = 240;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeychainStatus {
    pub platform_supported: bool,
    pub key_exists: bool,
    pub entry_count: usize,
    pub empty: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeychainBackupReport {
    pub created_at: chrono::DateTime<Utc>,
    pub reason: String,
    pub destination: PathBuf,
    pub entry_count: usize,
    pub skipped: bool,
    pub message: String,
}

pub fn default_keychain_backup_dir() -> Result<PathBuf> {
    let user_dirs = UserDirs::new()
        .ok_or_else(|| LpcError::Internal("cannot resolve user profile directory".into()))?;
    Ok(user_dirs
        .document_dir()
        .unwrap_or_else(|| user_dirs.home_dir())
        .join("LarkProfileConsoleBackups")
        .join("keychain"))
}

/// Inspect the official CLI credential slot count (no secret values are logged).
pub fn inspect_keychain() -> KeychainStatus {
    #[cfg(windows)]
    let status = inspect_keychain_windows();
    #[cfg(not(windows))]
    let status = KeychainStatus {
        platform_supported: false,
        key_exists: false,
        entry_count: 0,
        empty: false,
        detail: "Official CLI keychain durability checks are Windows-only in this build.".into(),
    };
    // The slot count over time is the curve nobody could plot during the
    // 2026-07-22 cascade: it shows whether credentials went one at a time or
    // all at once, which tells apart per-account revocation from a wipe.
    tracing::info!(
        entry_count = status.entry_count,
        key_exists = status.key_exists,
        "keychain slots observed"
    );
    status
}

/// Snapshot the official CLI keychain registry branch. Best-effort API surface.
pub fn backup_keychain_registry(reason: &str) -> Result<KeychainBackupReport> {
    let dir = default_keychain_backup_dir()?;
    backup_keychain_registry_to(&dir, reason)
}

/// Export a keychain snapshot when the newest `.reg` in the backup dir is older than `max_age`.
pub fn ensure_keychain_snapshot_if_stale(
    max_age: std::time::Duration,
) -> Result<Option<KeychainBackupReport>> {
    let dir = default_keychain_backup_dir()?;
    if should_export_keychain_snapshot(&dir, max_age)? {
        backup_keychain_registry_to(&dir, "hourly").map(Some)
    } else {
        Ok(None)
    }
}

fn should_export_keychain_snapshot(dir: &Path, max_age: std::time::Duration) -> Result<bool> {
    if !dir.exists() {
        return Ok(true);
    }
    Ok(match latest_keychain_backup_modified(dir)? {
        Some(modified) => modified
            .elapsed()
            .map(|elapsed| elapsed > max_age)
            .unwrap_or(true),
        None => true,
    })
}

fn latest_keychain_backup_modified(dir: &Path) -> Result<Option<std::time::SystemTime>> {
    let mut latest: Option<std::time::SystemTime> = None;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("reg"))
        {
            let modified = entry.metadata()?.modified().ok();
            if let Some(modified) = modified {
                latest = Some(match latest {
                    Some(current) if current >= modified => current,
                    _ => modified,
                });
            }
        }
    }
    Ok(latest)
}

pub fn backup_keychain_registry_to(dir: &Path, reason: &str) -> Result<KeychainBackupReport> {
    #[cfg(windows)]
    let report = backup_keychain_registry_windows(dir, reason)?;
    #[cfg(not(windows))]
    let report = {
        let _ = dir;
        KeychainBackupReport {
            created_at: Utc::now(),
            reason: reason.to_owned(),
            destination: PathBuf::new(),
            entry_count: 0,
            skipped: true,
            message: "keychain registry backup is Windows-only".into(),
        }
    };
    // A snapshot that was skipped is the one you reach for during recovery and
    // find missing, so the skip has to be as visible as the success.
    tracing::info!(
        entry_count = report.entry_count,
        skipped = report.skipped,
        reason,
        "keychain snapshot finished"
    );
    Ok(report)
}

#[cfg(windows)]
fn inspect_keychain_windows() -> KeychainStatus {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    match hkcu.open_subkey(r"Software\LarkCli\keychain\lark-cli") {
        Ok(key) => {
            let entry_count = key.enum_values().filter_map(|item| item.ok()).count();
            let empty = entry_count == 0;
            KeychainStatus {
                platform_supported: true,
                key_exists: true,
                entry_count,
                empty,
                detail: if empty {
                    "Official CLI keychain registry key exists but has ZERO values. \
                     All user tokens and app secrets appear wiped. Restore from \
                     Documents\\LarkProfileConsoleBackups\\keychain or re-authorize."
                        .into()
                } else {
                    format!(
                        "Official CLI keychain has {entry_count} registry value slot(s) \
                         (names only counted; secrets not logged)."
                    )
                },
            }
        }
        Err(_) => KeychainStatus {
            platform_supported: true,
            key_exists: false,
            entry_count: 0,
            empty: true,
            detail: "Official CLI keychain registry path is missing \
                     (HKCU\\Software\\LarkCli\\keychain\\lark-cli)."
                .into(),
        },
    }
}

#[cfg(windows)]
fn backup_keychain_registry_windows(dir: &Path, reason: &str) -> Result<KeychainBackupReport> {
    fs::create_dir_all(dir)?;
    let status = inspect_keychain_windows();
    let created_at = Utc::now();
    let safe_reason = sanitize_reason(reason);
    let destination = dir.join(format!(
        "{}-{}.reg",
        created_at.format("%Y%m%d-%H%M%S"),
        safe_reason
    ));

    if !status.key_exists {
        return Ok(KeychainBackupReport {
            created_at,
            reason: reason.to_owned(),
            destination,
            entry_count: 0,
            skipped: true,
            message: status.detail,
        });
    }

    // In-process export avoids spawning `reg.exe` from a GUI subsystem process
    // (Win32 0x800700E8 / ERROR_NO_DATA when stdio pipes close).
    let body = export_keychain_reg_file()?;
    // UTF-16 LE with BOM matches `reg export` default encoding on modern Windows.
    let mut bytes = Vec::with_capacity(2 + body.len() * 2);
    bytes.extend_from_slice(&[0xFF, 0xFE]);
    for unit in body.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    crate::atomic::write_bytes_atomic(&destination, &bytes)?;

    prune_old_keychain_backups(dir)?;

    Ok(KeychainBackupReport {
        created_at,
        reason: reason.to_owned(),
        destination,
        entry_count: status.entry_count,
        skipped: false,
        message: if status.empty {
            "Exported EMPTY keychain (for forensics). Prefer restoring an earlier non-empty .reg."
                .into()
        } else {
            format!(
                "Exported official CLI keychain ({} value slots).",
                status.entry_count
            )
        },
    })
}

#[cfg(windows)]
fn export_keychain_reg_file() -> Result<String> {
    use winreg::enums::*;
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let root = hkcu
        .open_subkey(r"Software\LarkCli\keychain")
        .map_err(|error| {
            LpcError::Internal(format!("open LarkCli keychain root failed: {error}"))
        })?;

    let mut out = String::from("Windows Registry Editor Version 5.00\r\n\r\n");
    append_reg_key(
        &mut out,
        r"HKEY_CURRENT_USER\Software\LarkCli\keychain",
        &root,
    )?;

    // Primary credential leaf used by official CLI.
    if let Ok(leaf) = root.open_subkey("lark-cli") {
        append_reg_key(
            &mut out,
            r"HKEY_CURRENT_USER\Software\LarkCli\keychain\lark-cli",
            &leaf,
        )?;
    }

    // Any other subkeys under keychain (future-proof).
    for sub in root.enum_keys().filter_map(|item| item.ok()) {
        if sub.eq_ignore_ascii_case("lark-cli") {
            continue;
        }
        if let Ok(child) = root.open_subkey(&sub) {
            let path = format!(r"HKEY_CURRENT_USER\Software\LarkCli\keychain\{sub}");
            append_reg_key(&mut out, &path, &child)?;
        }
    }

    Ok(out)
}

#[cfg(windows)]
fn append_reg_key(out: &mut String, path: &str, key: &winreg::RegKey) -> Result<()> {
    use winreg::enums::*;

    out.push('[');
    out.push_str(path);
    out.push_str("]\r\n");

    for (name, value) in key.enum_values().filter_map(|item| item.ok()) {
        let quoted_name = if name.is_empty() {
            "@".to_owned()
        } else {
            format!("\"{}\"", escape_reg_token(&name))
        };
        let rendered = match value.vtype {
            REG_SZ | REG_EXPAND_SZ => {
                let text = utf16le_c_string(&value.bytes);
                format!("{quoted_name}=\"{}\"", escape_reg_token(&text))
            }
            REG_DWORD => {
                let dword = u32::from_le_bytes([
                    value.bytes.first().copied().unwrap_or(0),
                    value.bytes.get(1).copied().unwrap_or(0),
                    value.bytes.get(2).copied().unwrap_or(0),
                    value.bytes.get(3).copied().unwrap_or(0),
                ]);
                format!("{quoted_name}=dword:{dword:08x}")
            }
            REG_QWORD => {
                let hex = hex_csv(&value.bytes);
                format!("{quoted_name}=hex(b):{hex}")
            }
            REG_BINARY => {
                let hex = hex_csv(&value.bytes);
                format!("{quoted_name}=hex:{hex}")
            }
            REG_MULTI_SZ => {
                let hex = hex_csv(&value.bytes);
                format!("{quoted_name}=hex(7):{hex}")
            }
            other => {
                let hex = hex_csv(&value.bytes);
                format!("{quoted_name}=hex({:x}):{hex}", other as u32)
            }
        };
        out.push_str(&rendered);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    Ok(())
}

#[cfg(windows)]
fn utf16le_c_string(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    let mut units = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        units.push(u16::from_le_bytes([chunk[0], chunk[1]]));
    }
    if let Some(end) = units.iter().position(|&unit| unit == 0) {
        units.truncate(end);
    }
    String::from_utf16_lossy(&units)
}

#[cfg(windows)]
fn hex_csv(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(windows)]
fn escape_reg_token(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
}

fn prune_old_keychain_backups(dir: &Path) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    let mut files: Vec<_> = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("reg"))
        })
        .collect();
    files.sort_by_key(|path| {
        std::cmp::Reverse(
            path.file_name()
                .map(|name| name.to_os_string())
                .unwrap_or_default(),
        )
    });
    for path in files.into_iter().skip(MAX_RETAINED_KEYCHAIN_BACKUPS) {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

fn sanitize_reason(reason: &str) -> String {
    let sanitized = reason
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned();
    if sanitized.is_empty() {
        "keychain".into()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_reason_strips_unsafe_chars() {
        assert_eq!(
            sanitize_reason("Before Full Restore!"),
            "before-full-restore"
        );
    }

    #[test]
    fn inspect_does_not_panic() {
        let status = inspect_keychain();
        assert!(!status.detail.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn backup_writes_reg_without_spawning_reg_exe() {
        let dir = tempfile::tempdir().unwrap();
        let report = backup_keychain_registry_to(dir.path(), "unit-test").unwrap();
        if report.skipped {
            return;
        }
        assert!(report.destination.is_file());
        let bytes = fs::read(&report.destination).unwrap();
        // UTF-16 LE BOM
        assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let text = String::from_utf16_lossy(&units);
        assert!(text.contains("Windows Registry Editor Version 5.00"));
        assert!(text.contains(r"HKEY_CURRENT_USER\Software\LarkCli\keychain"));
    }
}
