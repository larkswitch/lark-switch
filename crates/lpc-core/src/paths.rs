use crate::error::{LpcError, Result};
use directories::ProjectDirs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppPaths {
    root: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<Self> {
        if let Some(value) = std::env::var_os("LPC_HOME") {
            return Ok(Self::new(value));
        }
        let dirs = ProjectDirs::from("dev", "LarkProfileConsole", "Lark Profile Console")
            .ok_or_else(|| LpcError::Internal("cannot resolve user data directory".into()))?;
        Ok(Self::new(dirs.data_local_dir().to_path_buf()))
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn data_dir(&self) -> PathBuf {
        self.root.join("data")
    }

    pub fn catalog_file(&self) -> PathBuf {
        self.data_dir().join("catalog.json")
    }

    pub fn active_state_file(&self) -> PathBuf {
        self.data_dir().join("active-state.json")
    }

    pub fn keychain_watch_file(&self) -> PathBuf {
        self.data_dir().join("keychain-watch.json")
    }

    pub fn apps_dir(&self) -> PathBuf {
        self.root.join("apps")
    }

    pub fn app_dir(&self, app_id: uuid::Uuid) -> PathBuf {
        self.apps_dir().join(app_id.to_string())
    }

    pub fn app_base_config(&self, app_id: uuid::Uuid) -> PathBuf {
        self.app_dir(app_id).join("config.json")
    }

    pub fn accounts_dir(&self) -> PathBuf {
        self.root.join("accounts")
    }

    pub fn account_config_dir(&self, account_id: uuid::Uuid) -> PathBuf {
        self.accounts_dir().join(account_id.to_string())
    }

    pub fn staging_dir(&self) -> PathBuf {
        self.root.join("staging")
    }

    pub fn locks_dir(&self) -> PathBuf {
        self.root.join("locks")
    }

    pub fn routing_gate_file(&self) -> PathBuf {
        self.locks_dir().join("routing.lock")
    }

    pub fn runtime_gate_file(&self) -> PathBuf {
        self.locks_dir().join("runtime.lock")
    }

    pub fn leases_dir(&self) -> PathBuf {
        self.locks_dir().join("executions")
    }

    pub fn runtime_dir(&self) -> PathBuf {
        self.root.join("runtime")
    }

    pub fn runtime_versions_dir(&self) -> PathBuf {
        self.runtime_dir().join("versions")
    }

    pub fn runtime_version_dir(&self, version: &str) -> PathBuf {
        self.runtime_versions_dir().join(version)
    }

    pub fn bin_dir(&self) -> PathBuf {
        self.root.join("bin")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn ensure_layout(&self) -> Result<()> {
        for path in [
            self.root.clone(),
            self.data_dir(),
            self.apps_dir(),
            self.accounts_dir(),
            self.staging_dir(),
            self.locks_dir(),
            self.leases_dir(),
            self.runtime_versions_dir(),
            self.bin_dir(),
            self.logs_dir(),
        ] {
            std::fs::create_dir_all(path)?;
        }
        Ok(())
    }
}
