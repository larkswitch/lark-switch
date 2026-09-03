//! Bind the shared LPC data root to the host Windows registry view.
//!
//! Packaged or sandboxed agent processes can share `LPC_HOME` while HKCU is
//! redirected to a private hive. Comparing their keychain slot count with the
//! host count then creates a false "credential cliff", and running the official
//! CLI rotates or deletes tokens in the wrong hive. A random marker stored once
//! in both places makes that split observable without reading credential data.

use crate::atomic::write_json_atomic;
use crate::error::{LpcError, Result};
use crate::paths::AppPaths;
use serde::{Deserialize, Serialize};
use std::fs;
use uuid::Uuid;

const MARKER_VERSION: u32 = 1;
#[cfg(windows)]
const REGISTRY_KEY: &str = r"Software\LarkProfileConsole\HostKeychainView";
#[cfg(windows)]
const REGISTRY_VALUE: &str = "Marker";

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

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedMarker {
    version: u32,
    marker: Uuid,
}

/// Called only by the unpackaged desktop owner before any credential work.
/// Never repairs one side from the other when both do not already exist: a
/// sandbox must not be able to bless its private registry as the host view.
pub fn ensure_host_keychain_view(paths: &AppPaths) -> Result<KeychainViewStatus> {
    let status = ensure_platform(paths)?;
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

fn write_disk_marker(paths: &AppPaths, marker: Uuid) -> Result<()> {
    write_json_atomic(
        &paths.host_keychain_view_file(),
        &PersistedMarker {
            version: MARKER_VERSION,
            marker,
        },
    )
}

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
    Ok(classify(read_disk_marker(paths)?, read_registry_marker()?))
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
    let disk = read_disk_marker(paths)?;
    let registry = read_registry_marker()?;
    match (disk, registry) {
        (None, None) => {
            let marker = Uuid::new_v4();
            write_registry_marker(marker)?;
            write_disk_marker(paths, marker)?;
            Ok(classify(Some(marker), Some(marker)))
        }
        (None, Some(marker)) => {
            // A crash can occur after the registry write and before publishing
            // the atomic file. Only the desktop owner is allowed to finish it.
            write_disk_marker(paths, marker)?;
            Ok(classify(Some(marker), Some(marker)))
        }
        (disk, registry) => Ok(classify(disk, registry)),
    }
}

#[cfg(not(windows))]
fn ensure_platform(paths: &AppPaths) -> Result<KeychainViewStatus> {
    inspect_platform(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

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
