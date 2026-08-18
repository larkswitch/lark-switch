use crate::atomic::write_json_atomic;
use crate::error::{LpcError, Result};
use crate::model::{AccountRecord, AppRecord, ExecutionLeaseRecord};
use crate::paths::AppPaths;
use crate::store::StateStore;
use chrono::Utc;
use fs2::FileExt;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessesToUpdate, System};
use uuid::Uuid;

pub const CLI_KEYCHAIN_LOCK_NAME: &str = "cli-keychain";

/// Every critical section under the routing gate is local file I/O measured in
/// milliseconds, so this is roughly three orders of magnitude of headroom. It
/// exists to bound a wedged holder, not to arbitrate normal contention.
pub const ROUTING_GATE_TIMEOUT: Duration = Duration::from_secs(30);
const GATE_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Exclusive lock over the shared lark-cli Windows keychain registry hive.
/// All managed lark-cli child processes must hold this for their lifetime.
pub struct CliKeychainGuard {
    file: Option<File>,
}

impl CliKeychainGuard {
    #[cfg(test)]
    pub(crate) fn noop_for_tests() -> Self {
        Self { file: None }
    }
}

impl Drop for CliKeychainGuard {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = FileExt::unlock(&file);
        }
    }
}

pub fn cli_keychain_lock_path(paths: &AppPaths) -> PathBuf {
    paths
        .locks_dir()
        .join(format!("{CLI_KEYCHAIN_LOCK_NAME}.lock"))
}

/// Tries to acquire the shared keychain lock, polling until `timeout` elapses.
/// Returns `Ok(None)` when another holder keeps the lock (busy, not an I/O error).
pub fn try_acquire_cli_keychain_lock(
    paths: &AppPaths,
    timeout: Duration,
) -> Result<Option<CliKeychainGuard>> {
    fs::create_dir_all(paths.locks_dir())?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(cli_keychain_lock_path(paths))?;
    let deadline = Instant::now() + timeout;
    loop {
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => return Ok(Some(CliKeychainGuard { file: Some(file) })),
            Err(error) if error.kind() == fs2::lock_contended_error().kind() => {
                if Instant::now() >= deadline {
                    return Ok(None);
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// Keeps the keychain lock until `pid` exits. Used when the caller retains the
/// spawned `Child` handle and waits on it elsewhere.
pub fn release_cli_keychain_lock_when_process_exits(pid: u32, guard: CliKeychainGuard) {
    if guard.file.is_none() {
        return;
    }
    thread::spawn(move || {
        wait_for_process_exit(pid);
        drop(guard);
    });
}

fn wait_for_process_exit(pid: u32) {
    let pid = Pid::from_u32(pid);
    let mut system = System::new();
    loop {
        system.refresh_processes(ProcessesToUpdate::Some(&[pid]));
        if system.process(pid).is_none() {
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
}

#[derive(Debug, Clone)]
pub struct RoutingGate {
    paths: AppPaths,
}

pub struct RoutingGuard {
    file: File,
}

impl Drop for RoutingGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// A process-lifetime singleton lock backed by an OS advisory file lock.
///
/// Holding this guard means the current process is the sole owner of the named
/// lock; a second process that tries to acquire the same lock observes
/// contention and gets `Ok(None)`. Used to stop two desktop instances (e.g. an
/// installed build and a dev build) from racing on the same data root and
/// clobbering each other's catalog.
pub struct SingletonLock {
    _file: File,
}

impl RoutingGate {
    /// Tries to take a named singleton lock without blocking. Returns
    /// `Ok(Some(guard))` when acquired, `Ok(None)` when another live process
    /// already holds it, and `Err(..)` only on real I/O failures.
    pub fn try_acquire_singleton(&self, name: &str) -> Result<Option<SingletonLock>> {
        fs::create_dir_all(self.paths.locks_dir())?;
        let path = self.paths.locks_dir().join(format!("{name}.lock"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Some(SingletonLock { _file: file })),
            Err(error) if error.kind() == fs2::lock_contended_error().kind() => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RouteSnapshot {
    pub account: AccountRecord,
    pub app: AppRecord,
    pub managed_cli_path: PathBuf,
    pub generation: u64,
}

#[derive(Debug)]
pub struct ExecutionLease {
    path: PathBuf,
    pub record: ExecutionLeaseRecord,
    released: bool,
}

impl RoutingGate {
    pub fn new(paths: AppPaths) -> Self {
        Self { paths }
    }

    pub fn lock(&self) -> Result<RoutingGuard> {
        self.lock_with_timeout(ROUTING_GATE_TIMEOUT)
    }

    /// The gate is only ever held across local file writes, so waiting past a
    /// generous bound means the holder is stuck rather than busy. Waiting
    /// forever, as this used to, turns one wedged process into a machine where
    /// every `lark-cli` call hangs and nothing says why — the shim takes this
    /// gate on every single invocation.
    ///
    /// An OS advisory lock is released when its holder exits, so this cannot
    /// fire for a crashed process; only for a live one that stopped making
    /// progress.
    pub fn lock_with_timeout(&self, timeout: Duration) -> Result<RoutingGuard> {
        fs::create_dir_all(self.paths.locks_dir())?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.paths.routing_gate_file())?;

        let deadline = Instant::now() + timeout;
        loop {
            match FileExt::try_lock_exclusive(&file) {
                Ok(()) => return Ok(RoutingGuard { file }),
                Err(error) if error.kind() == fs2::lock_contended_error().kind() => {
                    if Instant::now() >= deadline {
                        return Err(LpcError::RoutingGateBusy(timeout));
                    }
                    thread::sleep(GATE_POLL_INTERVAL);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// Atomically snapshots the account used by a new command and creates its
    /// lease under the same gate. A tray switch can happen after this returns;
    /// this command continues with its immutable account/config snapshot while
    /// future commands observe the new account.
    ///
    /// When `account_override` is set, the strict selector is resolved under the
    /// same lock and active-state is only read (never written).
    pub fn snapshot_for_execution(
        &self,
        store: &StateStore,
    ) -> Result<(RouteSnapshot, ExecutionLease)> {
        self.snapshot_for_execution_with_override(store, None)
    }

    pub fn snapshot_for_execution_with_override(
        &self,
        store: &StateStore,
        account_override: Option<&str>,
    ) -> Result<(RouteSnapshot, ExecutionLease)> {
        let _guard = self.lock()?;
        self.cleanup_orphans_locked()?;
        let state = store.load_state()?;
        let catalog = store.load_catalog()?;
        let (account, app) =
            if let Some(raw) = account_override.map(str::trim).filter(|v| !v.is_empty()) {
                let selector = crate::selector::parse_selector(raw)?;
                let (account, app) = crate::selector::resolve_account(&catalog, &selector, None)?;
                (account.clone(), app.clone())
            } else {
                let account_id = state.active_account_id.ok_or(LpcError::NoActiveAccount)?;
                let account = catalog
                    .accounts
                    .iter()
                    .find(|account| account.id == account_id)
                    .cloned()
                    .ok_or_else(|| LpcError::AccountNotFound(account_id.to_string()))?;
                let app = catalog
                    .apps
                    .iter()
                    .find(|app| app.id == account.app_ref)
                    .cloned()
                    .ok_or_else(|| LpcError::AppNotFound(account.app_ref.to_string()))?;
                (account, app)
            };
        let managed_cli_path = state
            .managed_cli_path
            .clone()
            .ok_or_else(|| LpcError::RuntimeMissing(self.paths.runtime_dir()))?;
        if !managed_cli_path.is_file() {
            return Err(LpcError::RuntimeMissing(managed_cli_path));
        }
        let lease = ExecutionLease::create(&self.paths, &account, &app)?;
        Ok((
            RouteSnapshot {
                account,
                app,
                managed_cli_path,
                generation: state.generation,
            },
            lease,
        ))
    }

    pub fn switch_account(&self, store: &StateStore, account_id: Uuid) -> Result<()> {
        let _guard = self.lock()?;
        self.cleanup_orphans_locked()?;
        store.switch_active_account(account_id)?;
        Ok(())
    }

    pub fn running_counts(&self) -> Result<HashMap<Uuid, usize>> {
        let mut counts = HashMap::new();
        for record in self.running_leases()? {
            *counts.entry(record.account_id).or_insert(0) += 1;
        }
        Ok(counts)
    }

    pub fn running_leases(&self) -> Result<Vec<ExecutionLeaseRecord>> {
        let _guard = self.lock()?;
        self.cleanup_orphans_locked()?;
        let mut records = self.read_lease_records_locked()?;
        records.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.pid.cmp(&right.pid))
        });
        Ok(records)
    }

    pub fn running_for_account(&self, account_id: Uuid) -> Result<usize> {
        Ok(self
            .running_counts()?
            .get(&account_id)
            .copied()
            .unwrap_or(0))
    }

    /// Acquires the routing gate and returns it only when the account has no
    /// live execution lease. Callers keep the returned guard through a
    /// destructive operation so no new command can start for any account while
    /// metadata/config is being removed.
    pub fn lock_account_idle(&self, account_id: Uuid) -> Result<RoutingGuard> {
        let guard = self.lock()?;
        self.cleanup_orphans_locked()?;
        let running = self
            .read_lease_records_locked()?
            .into_iter()
            .filter(|record| record.account_id == account_id)
            .count();
        if running > 0 {
            drop(guard);
            return Err(LpcError::AccountBusy {
                account_id: account_id.to_string(),
                running,
            });
        }
        Ok(guard)
    }

    fn read_lease_records_locked(&self) -> Result<Vec<ExecutionLeaseRecord>> {
        let mut records = Vec::new();
        fs::create_dir_all(self.paths.leases_dir())?;
        for entry in fs::read_dir(self.paths.leases_dir())? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let bytes = match fs::read(entry.path()) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            if let Ok(record) = serde_json::from_slice::<ExecutionLeaseRecord>(&bytes) {
                records.push(record);
            }
        }
        Ok(records)
    }

    fn cleanup_orphans_locked(&self) -> Result<()> {
        fs::create_dir_all(self.paths.leases_dir())?;
        let mut entries = Vec::new();
        for entry in fs::read_dir(self.paths.leases_dir())? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let record = fs::read(entry.path())
                .ok()
                .and_then(|bytes| serde_json::from_slice::<ExecutionLeaseRecord>(&bytes).ok());
            entries.push((entry.path(), record));
        }

        let pids = entries
            .iter()
            .filter_map(|(_, record)| record.as_ref())
            .map(|record| Pid::from_u32(record.pid))
            .collect::<Vec<_>>();
        let mut system = System::new();
        if !pids.is_empty() {
            system.refresh_processes(ProcessesToUpdate::Some(&pids));
        }

        for (path, record) in entries {
            let remove = match record {
                None => true,
                Some(record) => {
                    let pid = Pid::from_u32(record.pid);
                    match system.process(pid) {
                        None => true,
                        Some(_) if record.process_started_at == 0 => false,
                        Some(process) => process.start_time() != record.process_started_at,
                    }
                }
            };
            if remove {
                let _ = fs::remove_file(path);
            }
        }
        Ok(())
    }
}

impl ExecutionLease {
    fn create(paths: &AppPaths, account: &AccountRecord, app: &AppRecord) -> Result<Self> {
        fs::create_dir_all(paths.leases_dir())?;
        let id = Uuid::new_v4();
        let pid = std::process::id();
        let process_pid = Pid::from_u32(pid);
        let mut system = System::new();
        system.refresh_processes(ProcessesToUpdate::Some(&[process_pid]));
        let process_started_at = system
            .process(process_pid)
            .map(|process| process.start_time())
            .unwrap_or(0);
        let record = ExecutionLeaseRecord {
            id,
            pid,
            process_started_at,
            account_id: account.id,
            app_id: app.app_id.clone(),
            created_at: Utc::now(),
        };
        let path = paths.leases_dir().join(format!("{id}.json"));
        write_json_atomic(&path, &record)?;
        Ok(Self {
            path,
            record,
            released: false,
        })
    }

    pub fn release(mut self) -> Result<()> {
        if !self.released {
            match fs::remove_file(&self.path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            self.released = true;
        }
        Ok(())
    }
}

impl Drop for ExecutionLease {
    fn drop(&mut self) {
        if !self.released {
            let _ = fs::remove_file(&self.path);
            self.released = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_keychain_lock_is_exclusive_and_releases_on_drop() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("lpc"));

        let first = try_acquire_cli_keychain_lock(&paths, Duration::from_secs(1))
            .unwrap()
            .expect("first acquisition should succeed");
        let second = try_acquire_cli_keychain_lock(&paths, Duration::from_millis(100)).unwrap();
        assert!(second.is_none(), "second acquisition must time out as busy");

        drop(first);
        let third = try_acquire_cli_keychain_lock(&paths, Duration::from_secs(1))
            .unwrap()
            .expect("lock should be reusable after drop");
        drop(third);
    }

    #[test]
    fn cli_keychain_lock_contention_times_out_on_second_thread() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("lpc"));
        let _first = try_acquire_cli_keychain_lock(&paths, Duration::from_secs(1))
            .unwrap()
            .expect("first acquisition should succeed");

        let paths_for_thread = paths.clone();
        let handle = thread::spawn(move || {
            let started = Instant::now();
            let second =
                try_acquire_cli_keychain_lock(&paths_for_thread, Duration::from_millis(250))
                    .unwrap();
            (second.is_none(), started.elapsed())
        });

        let (busy, elapsed) = handle.join().expect("thread should finish");
        assert!(busy, "contended acquisition must return busy");
        assert!(
            elapsed >= Duration::from_millis(200),
            "contended acquisition should wait until timeout, got {elapsed:?}"
        );
    }

    #[test]
    fn singleton_lock_is_exclusive_and_releases_on_drop() {
        let temp = tempfile::tempdir().unwrap();
        let gate = RoutingGate::new(AppPaths::new(temp.path().join("lpc")));

        let first = gate.try_acquire_singleton("desktop-instance").unwrap();
        assert!(first.is_some(), "first acquisition should succeed");

        // A second acquisition while the first is held must observe contention.
        let second = gate.try_acquire_singleton("desktop-instance").unwrap();
        assert!(second.is_none(), "second acquisition must be blocked");

        // Releasing the first lets a later acquisition succeed again.
        drop(first);
        let third = gate.try_acquire_singleton("desktop-instance").unwrap();
        assert!(third.is_some(), "lock should be reusable after drop");
    }
}
