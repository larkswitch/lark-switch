//! Bind the shared LPC data root to the host Windows registry view.
//!
//! Packaged or sandboxed agent processes can share `LPC_HOME` while HKCU is
//! redirected to a private hive. Comparing their keychain slot count with the
//! host count then creates a false "credential cliff", and running the official
//! CLI rotates or deletes tokens in the wrong hive. A random marker stored once
//! in both places makes that split observable without reading credential data.

#[cfg(windows)]
use crate::atomic::write_json_atomic;
use crate::error::{LpcError, Result};
use crate::paths::AppPaths;
#[cfg(windows)]
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::fs;
#[cfg(any(windows, test))]
use uuid::Uuid;

#[cfg(windows)]
const MARKER_VERSION: u32 = 1;
#[cfg(windows)]
const REGISTRY_KEY: &str = r"Software\LarkProfileConsole\HostKeychainView";
#[cfg(windows)]
const REGISTRY_VALUE: &str = "Marker";

// A registry overlay may inherit the marker while shadowing individual token
// values. Marker equality is necessary, but never proof of host execution.
#[cfg(windows)]
static BOOTSTRAPPED_HOST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(windows)]
fn trusted_host_execution() -> bool {
    BOOTSTRAPPED_HOST.load(std::sync::atomic::Ordering::Acquire)
        || crate::host_bridge::is_host_bridge_child()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeychainViewKind {
    Unsupported,
    Uninitialized,
    Host,
    Mismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeychainViewStatus {
    pub kind: KeychainViewKind,
    pub detail: String,
}

#[cfg(windows)]
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedMarker {
    version: u32,
    marker: Uuid,
}

/// Normal desktop entry point: verification only. Marker creation and repair
/// belong exclusively to the Task Scheduler bootstrap path below, so a
/// shadow-first launch cannot bless its private registry as the host view.
pub fn ensure_host_keychain_view(paths: &AppPaths) -> Result<KeychainViewStatus> {
    let status = ensure_platform(paths)?;
    match status.kind {
        KeychainViewKind::Mismatch => Err(LpcError::KeychainViewMismatch),
        KeychainViewKind::Uninitialized => Err(LpcError::KeychainViewUninitialized),
        KeychainViewKind::Unsupported | KeychainViewKind::Host => Ok(status),
    }
}

/// Reconcile the marker from a process launched by the on-demand host task.
/// Normal desktop/shim entry points must keep using `ensure_host_keychain_view`;
/// only Task Scheduler provides the independent host registry view required to
/// safely finish a missing side after a shadow-first installation.
pub fn bootstrap_host_keychain_view(paths: &AppPaths) -> Result<KeychainViewStatus> {
    let status = bootstrap_platform(paths)?;
    match status.kind {
        KeychainViewKind::Mismatch => Err(LpcError::KeychainViewMismatch),
        KeychainViewKind::Uninitialized => Err(LpcError::KeychainViewUninitialized),
        KeychainViewKind::Unsupported | KeychainViewKind::Host => Ok(status),
    }
}

pub fn inspect_host_keychain_view(paths: &AppPaths) -> KeychainViewStatus {
    inspect_platform(paths).unwrap_or_else(|error| KeychainViewStatus {
        kind: KeychainViewKind::Mismatch,
        detail: format!("Could not verify the host registry view: {error}"),
    })
}

pub fn enforce_host_keychain_view(paths: &AppPaths) -> Result<()> {
    match inspect_host_keychain_view(paths).kind {
        KeychainViewKind::Unsupported | KeychainViewKind::Host => Ok(()),
        KeychainViewKind::Uninitialized => Err(LpcError::KeychainViewUninitialized),
        KeychainViewKind::Mismatch => Err(LpcError::KeychainViewMismatch),
    }
}

#[cfg(windows)]
fn read_disk_marker(paths: &AppPaths) -> Result<Option<Uuid>> {
    let path = paths.host_keychain_view_file();
    if !path.is_file() {
        return Ok(None);
    }
    let persisted: PersistedMarker = serde_json::from_str(&fs::read_to_string(path)?)?;
    if persisted.version != MARKER_VERSION {
        return Err(LpcError::Integrity(format!(
            "unsupported host keychain view marker version {}",
            persisted.version
        )));
    }
    Ok(Some(persisted.marker))
}

#[cfg(windows)]
fn write_disk_marker(paths: &AppPaths, marker: Uuid) -> Result<()> {
    write_json_atomic(
        &paths.host_keychain_view_file(),
        &PersistedMarker {
            version: MARKER_VERSION,
            marker,
        },
    )
}

#[cfg(any(windows, test))]
fn classify(disk: Option<Uuid>, registry: Option<Uuid>) -> KeychainViewStatus {
    match (disk, registry) {
        (None, None) => KeychainViewStatus {
            kind: KeychainViewKind::Uninitialized,
            detail: "Host registry view marker has not been initialized by the desktop app.".into(),
        },
        (Some(_), None) | (None, Some(_)) => KeychainViewStatus {
            kind: KeychainViewKind::Mismatch,
            detail: "Shared LPC data and the current Windows registry view have different marker presence. This process is likely sandboxed or virtualized.".into(),
        },
        (Some(disk), Some(registry)) if disk == registry => KeychainViewStatus {
            kind: KeychainViewKind::Host,
            detail: "Current process is bound to the host Windows registry view.".into(),
        },
        (Some(_), Some(_)) => KeychainViewStatus {
            kind: KeychainViewKind::Mismatch,
            detail: "Shared LPC data and the current Windows registry view have different markers. This process is using a shadow keychain.".into(),
        },
    }
}

#[cfg(windows)]
fn read_registry_marker() -> Result<Option<Uuid>> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = match hkcu.open_subkey_with_flags(REGISTRY_KEY, KEY_READ) {
        Ok(key) => key,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    match key.get_value::<String, _>(REGISTRY_VALUE) {
        Ok(value) => Uuid::parse_str(&value)
            .map(Some)
            .map_err(|error| LpcError::Integrity(format!("invalid host registry marker: {error}"))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(windows)]
fn write_registry_marker(marker: Uuid) -> Result<()> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::RegKey;

    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey_with_flags(REGISTRY_KEY, KEY_READ | KEY_WRITE)?;
    key.set_value(REGISTRY_VALUE, &marker.to_string())?;
    Ok(())
}

#[cfg(windows)]
fn inspect_platform(paths: &AppPaths) -> Result<KeychainViewStatus> {
    Ok(require_host_execution(
        classify(read_disk_marker(paths)?, read_registry_marker()?),
        trusted_host_execution(),
    ))
}

#[cfg(any(windows, test))]
fn require_host_execution(mut status: KeychainViewStatus, trusted: bool) -> KeychainViewStatus {
    if status.kind == KeychainViewKind::Host && !trusted {
        status.kind = KeychainViewKind::Mismatch;
        status.detail = "Registry marker matches, but copy-on-write token isolation cannot be excluded. Execute through the scheduled desktop host.".into();
    }
    status
}

#[cfg(not(windows))]
fn inspect_platform(_paths: &AppPaths) -> Result<KeychainViewStatus> {
    Ok(KeychainViewStatus {
        kind: KeychainViewKind::Unsupported,
        detail: "Windows registry view checks are not applicable on this platform.".into(),
    })
}

#[cfg(windows)]
fn ensure_platform(paths: &AppPaths) -> Result<KeychainViewStatus> {
    inspect_platform(paths)
}

#[cfg(windows)]
fn bootstrap_platform(paths: &AppPaths) -> Result<KeychainViewStatus> {
    // A command-line flag is not authority. An agent can inherit an isolated
    // HKCU view even without package identity, and marker values can be copied.
    // Only the real Schedule service may launch a host that repairs markers or
    // refreshes credentials. In particular, Start-Process --host-bootstrap from
    // an agent must fail before any marker or keychain operation.
    if !launched_by_task_scheduler() {
        return Err(LpcError::KeychainViewMismatch);
    }
    let disk = read_disk_marker(paths)?;
    let registry = read_registry_marker()?;
    let marker = match (disk, registry) {
        (None, None) => {
            let marker = Uuid::new_v4();
            write_registry_marker(marker)?;
            write_disk_marker(paths, marker)?;
            marker
        }
        (Some(marker), None) => {
            write_registry_marker(marker)?;
            marker
        }
        (None, Some(marker)) => {
            write_disk_marker(paths, marker)?;
            marker
        }
        (Some(disk), Some(registry)) if disk == registry => disk,
        (Some(_), Some(_)) => return Err(LpcError::KeychainViewMismatch),
    };
    BOOTSTRAPPED_HOST.store(true, std::sync::atomic::Ordering::Release);
    Ok(classify(Some(marker), Some(marker)))
}

#[cfg(any(windows, test))]
fn scheduler_parent_matches(parent: Option<u32>, scheduler_pid: u32) -> bool {
    scheduler_pid != 0 && parent == Some(scheduler_pid)
}

#[cfg(windows)]
fn launched_by_task_scheduler() -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    use windows_sys::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx, SC_MANAGER_CONNECT,
        SC_STATUS_PROCESS_INFO, SERVICE_QUERY_STATUS, SERVICE_STATUS_PROCESS,
    };
    let current = Pid::from_u32(std::process::id());
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[current]),
        ProcessRefreshKind::new(),
    );
    let parent = system
        .process(current)
        .and_then(|process| process.parent())
        .map(|pid| pid.as_u32());
    // Query the service manager, rather than trusting a process name or an
    // environment variable that an ordinary caller could supply.
    let scheduler_pid = unsafe {
        let manager = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if manager.is_null() {
            return false;
        }
        let name: Vec<u16> = "Schedule\0".encode_utf16().collect();
        let service = OpenServiceW(manager, name.as_ptr(), SERVICE_QUERY_STATUS);
        if service.is_null() {
            CloseServiceHandle(manager);
            return false;
        }
        let mut status: SERVICE_STATUS_PROCESS = std::mem::zeroed();
        let mut needed = 0;
        let ok = QueryServiceStatusEx(
            service,
            SC_STATUS_PROCESS_INFO,
            &mut status as *mut _ as *mut u8,
            std::mem::size_of_val(&status) as u32,
            &mut needed,
        );
        CloseServiceHandle(service);
        CloseServiceHandle(manager);
        if ok == 0 {
            return false;
        }
        status.dwProcessId
    };
    scheduler_parent_matches(parent, scheduler_pid)
}

#[cfg(not(windows))]
fn ensure_platform(paths: &AppPaths) -> Result<KeychainViewStatus> {
    inspect_platform(paths)
}

#[cfg(not(windows))]
fn bootstrap_platform(paths: &AppPaths) -> Result<KeychainViewStatus> {
    inspect_platform(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_requires_the_actual_scheduler_parent() {
        assert!(scheduler_parent_matches(Some(2700), 2700));
        assert!(!scheduler_parent_matches(Some(1234), 2700));
        assert!(!scheduler_parent_matches(None, 2700));
        assert!(!scheduler_parent_matches(Some(0), 0));
    }

    #[test]
    fn inherited_marker_is_not_proof_of_host_execution() {
        let marker = Uuid::new_v4();
        let matched = classify(Some(marker), Some(marker));
        assert_eq!(
            require_host_execution(matched.clone(), false).kind,
            KeychainViewKind::Mismatch
        );
        assert_eq!(
            require_host_execution(matched, true).kind,
            KeychainViewKind::Host
        );
        assert_eq!(
            require_host_execution(classify(Some(marker), None), true).kind,
            KeychainViewKind::Mismatch
        );
    }

    #[test]
    fn identical_markers_identify_the_host_view() {
        let marker = Uuid::new_v4();
        assert_eq!(
            classify(Some(marker), Some(marker)).kind,
            KeychainViewKind::Host
        );
    }

    #[test]
    fn missing_or_different_registry_marker_is_a_shadow_view() {
        let marker = Uuid::new_v4();
        assert_eq!(
            classify(Some(marker), None).kind,
            KeychainViewKind::Mismatch
        );
        assert_eq!(
            classify(Some(marker), Some(Uuid::new_v4())).kind,
            KeychainViewKind::Mismatch
        );
    }

    #[test]
    fn two_missing_markers_are_uninitialized_not_a_false_host_match() {
        assert_eq!(classify(None, None).kind, KeychainViewKind::Uninitialized);
    }
}
