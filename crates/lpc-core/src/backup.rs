use crate::account::default_official_config_dirs;
use crate::atomic::write_bytes_atomic;
use crate::error::{LpcError, Result};
use crate::paths::AppPaths;
use chrono::{DateTime, NaiveDateTime, Utc};
use directories::UserDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tracing;
use uuid::Uuid;

/// 滚动保留上限：只留最近 N 份完整快照，防止备份目录无限膨胀。
const MAX_RETAINED_BACKUPS: usize = 10;

const TOKEN_RESTORE_WARNING: &str =
    "登录凭据(token)在系统凭据库(Windows: HKCU\\Software\\LarkCli\\keychain)。\
     文件级 LPC 备份不含 token；另有 Documents\\LarkProfileConsoleBackups\\keychain\\*.reg 注册表快照。\
     恢复文件备份后若仍掉线，优先从 keychain .reg 恢复，仍失败再重新授权。";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSourceReport {
    pub label: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub copied_files: usize,
    pub skipped_files: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupReport {
    pub created_at: DateTime<Utc>,
    pub reason: String,
    pub destination: PathBuf,
    pub sources: Vec<BackupSourceReport>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSnapshot {
    pub id: String,
    pub path: PathBuf,
    pub created_at: Option<DateTime<Utc>>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReport {
    pub restored_from: PathBuf,
    pub restored_files: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

pub fn default_backup_root() -> Result<PathBuf> {
    let user_dirs = UserDirs::new()
        .ok_or_else(|| LpcError::Internal("cannot resolve user profile directory".into()))?;
    Ok(user_dirs
        .document_dir()
        .unwrap_or_else(|| user_dirs.home_dir())
        .join("LarkProfileConsoleBackups"))
}

pub fn run_credential_backup(paths: &AppPaths, reason: &str) -> Result<BackupReport> {
    let root = default_backup_root()?;
    run_credential_backup_to(paths, &root, reason)
}

pub fn run_credential_backup_to(
    paths: &AppPaths,
    backup_root: &Path,
    reason: &str,
) -> Result<BackupReport> {
    run_credential_backup_to_with_prune(paths, backup_root, reason, true)
}

fn run_credential_backup_to_with_prune(
    paths: &AppPaths,
    backup_root: &Path,
    reason: &str,
    prune: bool,
) -> Result<BackupReport> {
    fs::create_dir_all(backup_root)?;
    // 清掉上次崩溃遗留的半成品临时目录，避免占盘且干扰滚动统计。
    cleanup_stale_temp_dirs(backup_root)?;

    let created_at = Utc::now();
    let destination = backup_root.join(format!(
        "{}-{}",
        created_at.format("%Y%m%d-%H%M%S"),
        sanitize_reason(reason)
    ));
    let temp_destination = backup_root.join(format!(".tmp-{}", Uuid::new_v4()));
    fs::create_dir_all(&temp_destination)?;

    let mut source_specs = vec![("lpc-control-plane".to_owned(), paths.root().to_path_buf())];
    for (index, config_dir) in default_official_config_dirs().into_iter().enumerate() {
        source_specs.push((format!("official-lark-cli-{}", index + 1), config_dir));
    }

    let mut sources = Vec::new();
    for (label, source) in source_specs {
        if !source.exists() {
            continue;
        }
        let target = temp_destination.join(&label);
        sources.push(copy_source(&label, &source, &target));
    }

    let report = BackupReport {
        created_at,
        reason: reason.to_owned(),
        destination: destination.clone(),
        sources,
    };
    // lpc-allow-raw-write: scratch snapshot directory, published by the rename below
    fs::write(
        temp_destination.join("manifest.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;

    // 全部拷完再 rename：外部观察者只会看到完整快照或旧快照，不会看到半成品。
    // lpc-allow-raw-write: this rename is the atomic publish step for the whole snapshot
    if let Err(error) = fs::rename(&temp_destination, &destination) {
        let _ = fs::remove_dir_all(&temp_destination);
        return Err(error.into());
    }

    if prune {
        prune_old_backups(backup_root)?;
    }

    // Parallel durability track: official CLI keychain registry (Windows).
    // File snapshots above never contain UATs; this export does (still DPAPI-bound).
    if let Err(error) = crate::keychain_guard::backup_keychain_registry(reason) {
        tracing::warn!(%error, "official CLI keychain registry backup failed");
    }

    // A snapshot that copied nothing still leaves a directory behind and looks
    // like protection. The two counts are what tell those apart afterwards.
    let copied_files: usize = report.sources.iter().map(|item| item.copied_files).sum();
    let skipped_files: usize = report.sources.iter().map(|item| item.skipped_files).sum();
    tracing::info!(
        copied_files,
        skipped_files,
        reason,
        "credential backup completed"
    );

    Ok(report)
}

pub fn list_backups(backup_root: &Path) -> Result<Vec<BackupSnapshot>> {
    if !backup_root.exists() {
        return Ok(Vec::new());
    }

    let mut snapshots = Vec::new();
    for entry in fs::read_dir(backup_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        if !is_backup_snapshot_name(&id) {
            continue;
        }
        let path = entry.path();
        let (mut created_at, mut reason) = parse_snapshot_name(&id);
        if let Ok(bytes) = fs::read(path.join("manifest.json")) {
            if let Ok(manifest) = serde_json::from_slice::<BackupReport>(&bytes) {
                created_at = Some(manifest.created_at);
                reason = Some(manifest.reason);
            }
        }
        snapshots.push(BackupSnapshot {
            id,
            path,
            created_at,
            reason,
        });
    }

    snapshots.sort_by(|left, right| right.id.cmp(&left.id));
    Ok(snapshots)
}

pub fn restore_latest(paths: &AppPaths, backup_root: &Path) -> Result<RestoreReport> {
    let snapshots = list_backups(backup_root)?;
    let snapshot = snapshots.into_iter().next().ok_or_else(|| {
        LpcError::Integrity(format!(
            "no backup snapshots found under {}",
            backup_root.display()
        ))
    })?;
    restore_from_backup(paths, &snapshot)
}

pub fn restore_from_backup(paths: &AppPaths, snapshot: &BackupSnapshot) -> Result<RestoreReport> {
    let backup_root = snapshot
        .path
        .parent()
        .ok_or_else(|| LpcError::Integrity("backup snapshot has no parent directory".into()))?;
    let _pre_restore =
        run_credential_backup_to_with_prune(paths, backup_root, "pre-restore", false)?;

    let source_root = snapshot.path.join("lpc-control-plane");
    if !source_root.is_dir() {
        return Err(LpcError::Integrity(format!(
            "backup snapshot {} does not contain lpc-control-plane data",
            snapshot.id
        )));
    }

    let mut report = RestoreReport {
        restored_from: snapshot.path.clone(),
        restored_files: Vec::new(),
        warnings: vec![TOKEN_RESTORE_WARNING.to_owned()],
    };
    restore_subtree_atomic(&source_root, paths.root(), &mut report)?;
    Ok(report)
}

fn restore_subtree_atomic(
    source_root: &Path,
    target_root: &Path,
    report: &mut RestoreReport,
) -> Result<()> {
    for entry in walkdir::WalkDir::new(source_root)
        .min_depth(1)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        let source_path = entry.path();
        let relative = source_path
            .strip_prefix(source_root)
            .map_err(|error| LpcError::Internal(error.to_string()))?;
        let target_path = target_root.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target_path)?;
            continue;
        }
        let bytes = fs::read(source_path)?;
        write_bytes_atomic(&target_path, &bytes)?;
        report.restored_files.push(target_path);
    }
    Ok(())
}

fn cleanup_stale_temp_dirs(backup_root: &Path) -> Result<()> {
    let entries = match fs::read_dir(backup_root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".tmp-") && entry.file_type()?.is_dir() {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    Ok(())
}

fn prune_old_backups(backup_root: &Path) -> Result<()> {
    let mut ids = Vec::new();
    for entry in fs::read_dir(backup_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        if is_backup_snapshot_name(&id) {
            ids.push(id);
        }
    }
    ids.sort();
    while ids.len() > MAX_RETAINED_BACKUPS {
        let oldest = ids.remove(0);
        fs::remove_dir_all(backup_root.join(oldest))?;
    }
    Ok(())
}

fn is_backup_snapshot_name(name: &str) -> bool {
    if name.len() < 17 {
        return false;
    }
    let bytes = name.as_bytes();
    bytes[8] == b'-'
        && bytes.get(15) == Some(&b'-')
        && name[..8].chars().all(|ch| ch.is_ascii_digit())
        && name[9..15].chars().all(|ch| ch.is_ascii_digit())
}

fn parse_snapshot_name(name: &str) -> (Option<DateTime<Utc>>, Option<String>) {
    if !is_backup_snapshot_name(name) {
        return (None, None);
    }
    let created_at = NaiveDateTime::parse_from_str(&name[..15], "%Y%m%d-%H%M%S")
        .ok()
        .map(|value| value.and_utc());
    let reason = name.get(16..).map(str::to_owned);
    (created_at, reason)
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
        "backup".to_owned()
    } else {
        sanitized
    }
}

fn copy_source(label: &str, source: &Path, destination: &Path) -> BackupSourceReport {
    let mut report = BackupSourceReport {
        label: label.to_owned(),
        source: source.to_path_buf(),
        destination: destination.to_path_buf(),
        copied_files: 0,
        skipped_files: 0,
        errors: Vec::new(),
    };
    copy_dir_contents(source, destination, &mut report);
    report
}

fn copy_dir_contents(source: &Path, destination: &Path, report: &mut BackupSourceReport) {
    if let Err(error) = fs::create_dir_all(destination) {
        report.skipped_files += 1;
        report
            .errors
            .push(format!("create {}: {error}", destination.display()));
        return;
    }

    let entries = match fs::read_dir(source) {
        Ok(entries) => entries,
        Err(error) => {
            report.skipped_files += 1;
            report
                .errors
                .push(format!("read {}: {error}", source.display()));
            return;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                report.skipped_files += 1;
                report.errors.push(format!("read entry: {error}"));
                continue;
            }
        };
        let path = entry.path();
        let target = destination.join(entry.file_name());
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                report.skipped_files += 1;
                report
                    .errors
                    .push(format!("stat {}: {error}", path.display()));
                continue;
            }
        };
        if file_type.is_dir() {
            copy_dir_contents(&path, &target, report);
        } else if file_type.is_file() {
            // lpc-allow-raw-write: copies into the scratch snapshot directory, never a live path
            match fs::copy(&path, &target) {
                Ok(_) => report.copied_files += 1,
                Err(error) => {
                    report.skipped_files += 1;
                    report
                        .errors
                        .push(format!("copy {}: {error}", path.display()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_fake_snapshot(backup_root: &Path, id: &str) {
        let dir = backup_root.join(id);
        fs::create_dir_all(dir.join("lpc-control-plane").join("data")).unwrap();
        fs::write(
            dir.join("lpc-control-plane")
                .join("data")
                .join("catalog.json"),
            "{}",
        )
        .unwrap();
        let manifest = BackupReport {
            created_at: Utc::now(),
            reason: "fake".into(),
            destination: dir.clone(),
            sources: Vec::new(),
        };
        fs::write(
            dir.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn backup_copies_lpc_files_and_writes_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("lpc");
        fs::create_dir_all(home.join("data")).unwrap();
        fs::write(home.join("data").join("catalog.json"), "{}").unwrap();
        let paths = AppPaths::new(&home);
        let backup_root = temp.path().join("backups");

        let report = run_credential_backup_to(&paths, &backup_root, "Startup Backup").unwrap();

        assert!(report.destination.is_dir());
        assert!(report.destination.join("manifest.json").is_file());
        assert!(report
            .destination
            .join("lpc-control-plane")
            .join("data")
            .join("catalog.json")
            .is_file());
        assert_eq!(report.sources[0].copied_files, 1);
        assert!(!backup_root.read_dir().unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tmp-")));
    }

    #[test]
    fn rolling_retention_keeps_at_most_max_backups() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("lpc");
        fs::create_dir_all(home.join("data")).unwrap();
        fs::write(
            home.join("data").join("catalog.json"),
            r#"{"schemaVersion":1}"#,
        )
        .unwrap();
        let paths = AppPaths::new(&home);
        let backup_root = temp.path().join("backups");
        fs::create_dir_all(&backup_root).unwrap();

        for index in 0..12 {
            write_fake_snapshot(&backup_root, &format!("20240101-{index:06}-fake-{index}"));
        }

        run_credential_backup_to(&paths, &backup_root, "retention-test").unwrap();

        let remaining = fs::read_dir(&backup_root)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter(|entry| is_backup_snapshot_name(&entry.file_name().to_string_lossy()))
            .count();
        assert!(remaining <= MAX_RETAINED_BACKUPS);
    }

    #[test]
    fn restore_latest_round_trip_restores_catalog_and_warns_about_tokens() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("lpc");
        fs::create_dir_all(home.join("data")).unwrap();
        let catalog = r#"{"schemaVersion":1,"apps":[],"accounts":[]}"#;
        fs::write(home.join("data").join("catalog.json"), catalog).unwrap();
        let paths = AppPaths::new(&home);
        let backup_root = temp.path().join("backups");

        run_credential_backup_to(&paths, &backup_root, "round-trip").unwrap();
        fs::write(home.join("data").join("catalog.json"), "tampered").unwrap();

        let report = restore_latest(&paths, &backup_root).unwrap();
        let restored = fs::read_to_string(home.join("data").join("catalog.json")).unwrap();
        assert_eq!(restored, catalog);
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("token")));
    }
}
