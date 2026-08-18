use crate::error::{LpcError, Result};
use crate::locking::RoutingGate;
use crate::store::StateStore;
use chrono::Utc;
use fs2::FileExt;
use reqwest::blocking::Client;
use semver::Version;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ReleaseAsset {
    pub version: Version,
    pub os: String,
    pub arch: String,
    pub archive_name: String,
    pub executable_name: String,
    pub download_url: String,
    pub checksums_url: String,
}

impl ReleaseAsset {
    pub fn for_current_platform(version: &str) -> Result<Self> {
        let os = match std::env::consts::OS {
            "macos" => "darwin",
            "windows" => "windows",
            "linux" => "linux",
            other => {
                return Err(LpcError::RuntimeIncompatible(format!(
                    "unsupported operating system {other}"
                )))
            }
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            "riscv64" => "riscv64",
            other => {
                return Err(LpcError::RuntimeIncompatible(format!(
                    "unsupported architecture {other}"
                )))
            }
        };
        Self::new(version, os, arch)
    }

    pub fn new(version: &str, os: &str, arch: &str) -> Result<Self> {
        let version = Version::parse(version.trim_start_matches('v'))?;
        let extension = if os == "windows" { "zip" } else { "tar.gz" };
        let archive_name = format!("lark-cli-{version}-{os}-{arch}.{extension}");
        let base = format!("https://github.com/larksuite/cli/releases/download/v{version}");
        Ok(Self {
            version,
            os: os.to_owned(),
            arch: arch.to_owned(),
            executable_name: if os == "windows" {
                "lark-cli.exe".to_owned()
            } else {
                "lark-cli".to_owned()
            },
            download_url: format!("{base}/{archive_name}"),
            checksums_url: format!("{base}/checksums.txt"),
            archive_name,
        })
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeManager {
    store: StateStore,
    client: Client,
}

impl RuntimeManager {
    pub fn new(store: StateStore) -> Result<Self> {
        let client = Client::builder()
            .user_agent("LarkProfileConsole/0.1")
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(180))
            .https_only(true)
            .build()?;
        Ok(Self { store, client })
    }

    pub fn install(&self, version: &str) -> Result<PathBuf> {
        self.store.initialize()?;
        let normalized = version.trim_start_matches('v');
        if !crate::SUPPORTED_CLI_VERSIONS.contains(&normalized) {
            return Err(LpcError::RuntimeIncompatible(format!(
                "version {normalized} is not in the tested allowlist {:?}",
                crate::SUPPORTED_CLI_VERSIONS
            )));
        }
        let asset = ReleaseAsset::for_current_platform(normalized)?;
        let paths = self.store.paths();
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(paths.runtime_gate_file())?;
        FileExt::lock_exclusive(&lock_file)?;
        let result = self.install_locked(&asset);
        let _ = FileExt::unlock(&lock_file);
        result
    }

    fn install_locked(&self, asset: &ReleaseAsset) -> Result<PathBuf> {
        let version = asset.version.to_string();
        let rollback_version = self
            .store
            .load_state()?
            .managed_cli_version
            .filter(|current| current != &version);
        let final_dir = self.store.paths().runtime_version_dir(&version);
        let final_executable = final_dir.join(&asset.executable_name);
        if final_executable.is_file() {
            let cli = crate::cli::OfficialCli::new(&final_executable);
            cli.compatibility_check()?;
            self.activate(&version, &final_executable)?;
            self.prune_other_versions(&version, rollback_version.as_deref())?;
            return Ok(final_executable);
        }

        let temp = tempfile::Builder::new()
            .prefix("lpc-runtime-")
            .tempdir_in(self.store.paths().runtime_dir())?;
        let archive_path = temp.path().join(&asset.archive_name);
        let checksum_text = self.download_text(&asset.checksums_url)?;
        let expected = checksum_for(&checksum_text, &asset.archive_name)?;
        self.download_file(&asset.download_url, &archive_path)?;
        verify_sha256(&archive_path, &expected)?;

        let extract_dir = temp.path().join("extract");
        fs::create_dir_all(&extract_dir)?;
        extract_archive(asset, &archive_path, &extract_dir)?;
        let extracted = find_executable(&extract_dir, &asset.executable_name)?;
        fs::create_dir_all(&final_dir)?;
        let staged_executable = final_dir.join(format!(".{}.staged", &asset.executable_name));
        // lpc-allow-raw-write: staged sibling file, promoted by the atomic rename below
        fs::copy(&extracted, &staged_executable)?;
        set_executable_permissions(&staged_executable)?;

        let cli = crate::cli::OfficialCli::new(&staged_executable);
        let observed = cli.version()?;
        if observed != asset.version {
            let _ = fs::remove_file(&staged_executable);
            return Err(LpcError::RuntimeIncompatible(format!(
                "downloaded binary reports {observed}, expected {}",
                asset.version
            )));
        }
        cli.compatibility_check()?;
        // lpc-allow-raw-write: this rename promotes the version-verified staged binary
        fs::rename(&staged_executable, &final_executable)?;
        self.activate(&version, &final_executable)?;
        self.prune_other_versions(&version, rollback_version.as_deref())?;
        Ok(final_executable)
    }

    fn prune_other_versions(&self, keep: &str, rollback: Option<&str>) -> Result<()> {
        let versions_dir = self.store.paths().runtime_versions_dir();
        if !versions_dir.exists() {
            return Ok(());
        }
        let mut installed = Vec::new();
        for entry in fs::read_dir(&versions_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Some(version) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if Version::parse(&version).is_ok() {
                installed.push((version, entry.path()));
            }
        }
        let rollback_to_keep = rollback.map(str::to_owned).or_else(|| {
            installed
                .iter()
                .filter(|(version, _)| version != keep)
                .max_by_key(|(version, _)| Version::parse(version).ok())
                .map(|(version, _)| version.clone())
        });
        for (version, path) in installed {
            if version != keep && rollback_to_keep.as_deref() != Some(version.as_str()) {
                fs::remove_dir_all(path)?;
            }
        }
        Ok(())
    }

    pub fn activate(&self, version: &str, executable: &Path) -> Result<()> {
        activate_managed_cli(&self.store, version, executable)
    }

    pub fn installed_versions(&self) -> Result<Vec<String>> {
        let mut versions = Vec::new();
        let dir = self.store.paths().runtime_versions_dir();
        if !dir.exists() {
            return Ok(versions);
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    if Version::parse(name).is_ok() {
                        versions.push(name.to_owned());
                    }
                }
            }
        }
        versions.sort_by(|a, b| Version::parse(b).ok().cmp(&Version::parse(a).ok()));
        Ok(versions)
    }

    pub fn rollback(&self) -> Result<PathBuf> {
        let state = self.store.load_state()?;
        let current = state.managed_cli_version.unwrap_or_default();
        let candidate = self
            .installed_versions()?
            .into_iter()
            .find(|version| version != &current)
            .ok_or_else(|| {
                LpcError::RuntimeIncompatible("no previous runtime is installed".into())
            })?;
        let asset = ReleaseAsset::for_current_platform(&candidate)?;
        let executable = self
            .store
            .paths()
            .runtime_version_dir(&candidate)
            .join(asset.executable_name);
        crate::cli::OfficialCli::new(&executable).compatibility_check()?;
        self.activate(&candidate, &executable)?;
        Ok(executable)
    }

    fn download_text(&self, url: &str) -> Result<String> {
        let response = self.client.get(url).send()?.error_for_status()?;
        Ok(response.text()?)
    }

    fn download_file(&self, url: &str, destination: &Path) -> Result<()> {
        let mut response = self.client.get(url).send()?.error_for_status()?;
        // lpc-allow-raw-write: download target is inside a TempDir and checksum-verified afterwards
        let mut file = File::create(destination)?;
        io::copy(&mut response, &mut file)?;
        file.sync_all()?;
        Ok(())
    }
}

fn activate_managed_cli(store: &StateStore, version: &str, executable: &Path) -> Result<()> {
    if !executable.is_file() {
        return Err(LpcError::RuntimeMissing(executable.to_path_buf()));
    }
    let gate = RoutingGate::new(store.paths().clone());
    let _guard = gate.lock()?;
    let mut state = store.load_state()?;
    let previous = state.managed_cli_version.clone();
    state.managed_cli_path = Some(executable.to_path_buf());
    state.managed_cli_version = Some(version.to_owned());
    state.generation = state.generation.saturating_add(1);
    state.updated_at = Utc::now();
    store.save_state(&state)?;
    // Both versions, because a self-repair or rollback that silently moved the
    // runtime under the user is otherwise indistinguishable from a fresh install.
    tracing::info!(
        version,
        previous_version = previous.as_deref().unwrap_or("none"),
        "managed cli activated"
    );
    Ok(())
}

/// If `managed_cli_path` points at a missing file, re-point state at the newest
/// installed allowlisted `lark-cli` under `runtime/versions/*`. Best-effort.
pub fn recover_missing_managed_cli(store: &StateStore) -> Result<bool> {
    let state = match store.load_state() {
        Ok(state) => state,
        Err(LpcError::NotInitialized) => return Ok(false),
        Err(error) => return Err(error),
    };
    let needs_recovery = matches!(&state.managed_cli_path, Some(path) if !path.is_file());
    if !needs_recovery {
        return Ok(false);
    }

    let Some((version, executable)) = find_newest_installed_supported_cli(store)? else {
        return Ok(false);
    };
    activate_managed_cli(store, &version, &executable)?;
    Ok(true)
}

fn find_newest_installed_supported_cli(store: &StateStore) -> Result<Option<(String, PathBuf)>> {
    let versions_dir = store.paths().runtime_versions_dir();
    if !versions_dir.exists() {
        return Ok(None);
    }

    let mut candidates: Vec<(Version, String, PathBuf)> = Vec::new();
    for entry in fs::read_dir(&versions_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(version_name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !crate::SUPPORTED_CLI_VERSIONS.contains(&version_name.as_str()) {
            continue;
        }
        let Ok(version) = Version::parse(&version_name) else {
            continue;
        };
        let dir = entry.path();
        let executable = ["lark-cli.exe", "lark-cli"]
            .into_iter()
            .map(|name| dir.join(name))
            .find(|path| path.is_file());
        if let Some(executable) = executable {
            candidates.push((version, version_name, executable));
        }
    }

    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(candidates
        .into_iter()
        .next()
        .map(|(_, version, executable)| (version, executable)))
}

pub fn checksum_for(checksums: &str, filename: &str) -> Result<String> {
    for line in checksums.lines() {
        let mut parts = line.split_whitespace();
        if let (Some(hash), Some(name)) = (parts.next(), parts.next()) {
            if name.trim_start_matches('*') == filename {
                if hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Ok(hash.to_ascii_lowercase());
                }
                return Err(LpcError::Integrity(format!(
                    "invalid SHA-256 for {filename}"
                )));
            }
        }
    }
    Err(LpcError::Integrity(format!(
        "checksums.txt has no entry for {filename}"
    )))
}

pub fn verify_sha256(path: &Path, expected: &str) -> Result<()> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = hex::encode(hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(LpcError::Integrity(format!(
            "SHA-256 mismatch for {}: expected {expected}, got {actual}",
            path.display()
        )));
    }
    Ok(())
}

fn extract_archive(asset: &ReleaseAsset, archive: &Path, destination: &Path) -> Result<()> {
    if asset.os == "windows" {
        let file = File::open(archive)?;
        let mut zip = zip::ZipArchive::new(file)
            .map_err(|error| LpcError::Integrity(format!("invalid zip archive: {error}")))?;
        for index in 0..zip.len() {
            let mut entry = zip
                .by_index(index)
                .map_err(|error| LpcError::Integrity(format!("invalid zip entry: {error}")))?;
            let enclosed = entry
                .enclosed_name()
                .ok_or_else(|| LpcError::Integrity("zip path traversal detected".into()))?
                .to_path_buf();
            let output = destination.join(enclosed);
            if entry.is_dir() {
                fs::create_dir_all(&output)?;
            } else {
                if let Some(parent) = output.parent() {
                    fs::create_dir_all(parent)?;
                }
                // lpc-allow-raw-write: extraction target is inside a TempDir, never a state path
                let mut file = File::create(output)?;
                io::copy(&mut entry, &mut file)?;
            }
        }
    } else {
        let file = File::open(archive)?;
        let decoder = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_path_buf();
            if path.is_absolute()
                || path
                    .components()
                    .any(|component| matches!(component, Component::ParentDir))
            {
                return Err(LpcError::Integrity("tar path traversal detected".into()));
            }
            entry.unpack_in(destination)?;
        }
    }
    Ok(())
}

fn find_executable(root: &Path, filename: &str) -> Result<PathBuf> {
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|error| LpcError::Internal(error.to_string()))?;
        if entry.file_type().is_file() && entry.file_name() == filename {
            return Ok(entry.path().to_path_buf());
        }
    }
    Err(LpcError::Integrity(format!(
        "archive does not contain {filename}"
    )))
}

#[cfg(unix)]
fn set_executable_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_asset_names_match_official_goreleaser_template() {
        let windows = ReleaseAsset::new("1.0.68", "windows", "amd64").unwrap();
        assert_eq!(windows.archive_name, "lark-cli-1.0.68-windows-amd64.zip");
        let mac = ReleaseAsset::new("1.0.68", "darwin", "arm64").unwrap();
        assert_eq!(mac.archive_name, "lark-cli-1.0.68-darwin-arm64.tar.gz");
    }

    #[test]
    fn parses_checksums() {
        let text = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  file.zip\n";
        assert_eq!(checksum_for(text, "file.zip").unwrap(), "a".repeat(64));
    }

    #[test]
    fn installing_a_runtime_keeps_one_rollback_version_and_prunes_older_versions() {
        let temp = tempfile::tempdir().unwrap();
        let paths = crate::paths::AppPaths::new(temp.path());
        paths.ensure_layout().unwrap();
        let older = paths.runtime_version_dir("1.0.68");
        let rollback = paths.runtime_version_dir("1.0.71");
        let current = paths.runtime_version_dir("1.0.86");
        let unrelated = paths.runtime_versions_dir().join("download-cache");
        fs::create_dir_all(&older).unwrap();
        fs::create_dir_all(&rollback).unwrap();
        fs::create_dir_all(&current).unwrap();
        fs::create_dir_all(&unrelated).unwrap();

        let manager = RuntimeManager::new(crate::store::StateStore::new(paths.clone())).unwrap();
        manager
            .prune_other_versions("1.0.86", Some("1.0.71"))
            .unwrap();

        assert!(!older.exists());
        assert!(rollback.exists());
        assert!(current.exists());
        assert!(unrelated.exists());
    }

    #[test]
    fn reinstalling_the_active_runtime_keeps_the_existing_rollback_version() {
        let temp = tempfile::tempdir().unwrap();
        let paths = crate::paths::AppPaths::new(temp.path());
        paths.ensure_layout().unwrap();
        let rollback = paths.runtime_version_dir("1.0.68");
        let current = paths.runtime_version_dir("1.0.86");
        fs::create_dir_all(&rollback).unwrap();
        fs::create_dir_all(&current).unwrap();

        let manager = RuntimeManager::new(crate::store::StateStore::new(paths)).unwrap();
        manager.prune_other_versions("1.0.86", None).unwrap();

        assert!(rollback.exists());
        assert!(current.exists());
    }

    #[test]
    fn recover_missing_managed_cli_reactivates_newest_supported() {
        let temp = tempfile::tempdir().unwrap();
        let paths = crate::paths::AppPaths::new(temp.path().join("lpc"));
        let store = crate::store::StateStore::new(paths.clone());
        store.initialize().unwrap();

        let old = paths.runtime_version_dir("1.0.68");
        let current = paths.runtime_version_dir("1.0.71");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&current).unwrap();
        let old_exe = old.join(if cfg!(windows) {
            "lark-cli.exe"
        } else {
            "lark-cli"
        });
        let new_exe = current.join(if cfg!(windows) {
            "lark-cli.exe"
        } else {
            "lark-cli"
        });
        fs::write(&old_exe, b"old").unwrap();
        fs::write(&new_exe, b"new").unwrap();

        let mut state = store.load_state().unwrap();
        state.managed_cli_path = Some(temp.path().join("missing-lark-cli"));
        state.managed_cli_version = Some("1.0.71".into());
        store.save_state(&state).unwrap();

        assert!(recover_missing_managed_cli(&store).unwrap());
        let recovered = store.load_state().unwrap();
        assert_eq!(recovered.managed_cli_version.as_deref(), Some("1.0.71"));
        assert_eq!(
            recovered.managed_cli_path.as_deref(),
            Some(new_exe.as_path())
        );
        assert!(!recover_missing_managed_cli(&store).unwrap());
    }
}
