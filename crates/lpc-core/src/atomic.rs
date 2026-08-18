use crate::error::Result;
use serde::Serialize;
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    write_bytes_atomic(path, &bytes)
}

pub fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temp = temp_path(path);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);

    atomic_replace(&temp, path)?;
    sync_parent(parent);
    Ok(())
}

fn temp_path(path: &Path) -> PathBuf {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("state");
    path.with_file_name(format!(".{name}.{}.tmp", Uuid::new_v4()))
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(source, target)
}

/// Replacement on Windows fails transiently whenever anything else still holds
/// a handle on the target: the search indexer, an antivirus scanner, a backup
/// agent, or simply the previous replacement of the same path still being torn
/// down. Every one of those means "try again", not "this write is impossible",
/// so surfacing them to the caller would turn routine contention into a lost
/// control-plane write.
#[cfg(windows)]
fn atomic_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    const RETRY_DELAYS_MS: &[u64] = &[1, 5, 15, 40, 100, 250];

    let mut attempt = 0usize;
    loop {
        match replace_once(source, target) {
            Ok(()) => return Ok(()),
            Err(error) if attempt < RETRY_DELAYS_MS.len() && is_transient_replace_error(&error) => {
                std::thread::sleep(std::time::Duration::from_millis(RETRY_DELAYS_MS[attempt]));
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(windows)]
fn is_transient_replace_error(error: &std::io::Error) -> bool {
    // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION,
    // ERROR_UNABLE_TO_REMOVE_REPLACED. The last one leaves both files under
    // their original names, so retrying is safe.
    const TRANSIENT: &[i32] = &[5, 32, 33, 1175];
    error
        .raw_os_error()
        .is_some_and(|code| TRANSIENT.contains(&code))
}

#[cfg(windows)]
fn replace_once(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

    if !target.exists() {
        return fs::rename(source, target);
    }
    let source_w: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target_w: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let ok = unsafe {
        ReplaceFileW(
            target_w.as_ptr(),
            source_w.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        Err(std::io::Error::from_raw_os_error(
            unsafe { GetLastError() } as i32
        ))
    } else {
        Ok(())
    }
}

fn sync_parent(parent: &Path) {
    #[cfg(unix)]
    {
        if let Ok(file) = File::open(parent) {
            let _ = file.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = parent;
}
