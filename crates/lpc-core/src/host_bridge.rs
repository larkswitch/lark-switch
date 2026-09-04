//! Executes a managed CLI command in the unpackaged desktop process' registry view.
//!
//! Sandboxed callers can share LPC's files while Windows redirects HKCU. They must
//! never run the official CLI against that shadow keychain. The desktop owns a
//! local-only named pipe and launches the already-installed shim as its child, so
//! routing, locking, management guards, and audit logging remain on the normal path.

use crate::error::{LpcError, Result};
use crate::paths::AppPaths;
use serde::{Deserialize, Serialize};

const PROTOCOL_VERSION: u32 = 1;
#[cfg(any(windows, test))]
const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;

#[cfg(windows)]
fn host_bridge_child_creation_flags() -> u32 {
    windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
}

#[cfg(windows)]
fn configure_host_bridge_child(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(host_bridge_child_creation_flags());
}

#[derive(Debug, Serialize, Deserialize)]
struct HostBridgeRequest {
    version: u32,
    args: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HostBridgeResponse {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Starts the host executor owned by the unpackaged desktop process.
pub fn start_host_bridge(paths: AppPaths) -> Result<()> {
    platform::start(paths)
}

/// Runs one command through the desktop-owned host executor.
pub fn execute_via_host_bridge(
    paths: &AppPaths,
    args: &[std::ffi::OsString],
) -> Result<HostBridgeResponse> {
    let args = args
        .iter()
        .map(|value| {
            value.to_str().map(str::to_owned).ok_or_else(|| {
                LpcError::HostBridgeUnavailable(
                    "a command argument is not valid Unicode and cannot cross the host bridge"
                        .into(),
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;
    platform::execute(
        paths,
        HostBridgeRequest {
            version: PROTOCOL_VERSION,
            args,
        },
    )
}

#[cfg(any(windows, test))]
fn pipe_name(paths: &AppPaths) -> String {
    use sha2::{Digest, Sha256};
    let normalized = paths
        .root()
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    let digest = Sha256::digest(normalized.as_bytes());
    format!(
        r"\\.\pipe\larkswitch-host-exec-v1-{}",
        hex::encode(&digest[..16])
    )
}

#[cfg(any(windows, test))]
fn encode_frame<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let body = serde_json::to_vec(value)?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(LpcError::HostBridgeUnavailable(
            "host bridge message exceeded the size limit".into(),
        ));
    }
    let mut frame = Vec::with_capacity(body.len() + 4);
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(&body);
    Ok(frame)
}

#[cfg(any(windows, test))]
fn decode_frame<T: for<'de> Deserialize<'de>>(reader: &mut impl std::io::Read) -> Result<T> {
    let mut length = [0_u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length > MAX_FRAME_BYTES {
        return Err(LpcError::HostBridgeUnavailable(
            "host bridge message exceeded the size limit".into(),
        ));
    }
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body)?;
    Ok(serde_json::from_slice(&body)?)
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::process::{Command, Stdio};
    use std::ptr;
    use windows_sys::Win32::Foundation::{
        ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FlushFileBuffers, FILE_ATTRIBUTE_NORMAL, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, WaitNamedPipeW,
        PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
        PIPE_WAIT,
    };

    const CONNECT_TIMEOUT_MS: u32 = 5_000;

    pub(super) fn start(paths: AppPaths) -> Result<()> {
        // Create the first instance synchronously so desktop startup cannot claim
        // the bridge is ready when the pipe name or platform setup is invalid.
        let first = create_pipe(&paths)?;
        std::thread::Builder::new()
            .name("larkswitch-host-bridge".into())
            .spawn(move || serve(paths, first))
            .map_err(|error| LpcError::HostBridgeUnavailable(error.to_string()))?;
        Ok(())
    }

    fn serve(paths: AppPaths, mut pipe: File) {
        tracing::info!("host CLI bridge started");
        loop {
            if let Err(error) = serve_one(&paths, &mut pipe) {
                tracing::warn!(%error, "host CLI bridge request failed");
            }
            unsafe {
                let _ = FlushFileBuffers(pipe.as_raw_handle());
                let _ = DisconnectNamedPipe(pipe.as_raw_handle());
            }
            match create_pipe(&paths) {
                Ok(next) => pipe = next,
                Err(error) => {
                    tracing::error!(%error, "host CLI bridge stopped");
                    return;
                }
            }
        }
    }

    fn serve_one(paths: &AppPaths, pipe: &mut File) -> Result<()> {
        let connected = unsafe { ConnectNamedPipe(pipe.as_raw_handle(), ptr::null_mut()) };
        if connected == 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32) {
                return Err(error.into());
            }
        }

        let request: HostBridgeRequest = decode_frame(pipe)?;
        if request.version != PROTOCOL_VERSION {
            return Err(LpcError::HostBridgeUnavailable(format!(
                "unsupported host bridge protocol {}",
                request.version
            )));
        }

        let shim = paths.bin_dir().join("lark-cli.exe");
        let mut command = Command::new(&shim);
        configure_host_bridge_child(&mut command);
        let response = match command
            .args(&request.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
        {
            Ok(output) => HostBridgeResponse {
                exit_code: output.status.code().unwrap_or(1),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            },
            Err(error) => HostBridgeResponse {
                exit_code: 70,
                stdout: String::new(),
                stderr: format!("[LPC_HOST_BRIDGE_FAILED] {error}\n"),
            },
        };
        // lpc-allow-raw-write: framed bytes go to an ephemeral named pipe, not persistent state.
        pipe.write_all(&encode_frame(&response)?)?;
        pipe.flush()?;
        Ok(())
    }

    fn create_pipe(paths: &AppPaths) -> Result<File> {
        let name = wide(&pipe_name(paths));
        let handle = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                ptr::null(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(LpcError::HostBridgeUnavailable(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        Ok(unsafe { File::from_raw_handle(handle as _) })
    }

    pub(super) fn execute(
        paths: &AppPaths,
        request: HostBridgeRequest,
    ) -> Result<HostBridgeResponse> {
        let name = wide(&pipe_name(paths));
        if unsafe { WaitNamedPipeW(name.as_ptr(), CONNECT_TIMEOUT_MS) } == 0 {
            return Err(LpcError::HostBridgeUnavailable(format!(
                "the larkswitch desktop host is not reachable: {}",
                std::io::Error::last_os_error()
            )));
        }
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(LpcError::HostBridgeUnavailable(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        let mut pipe = unsafe { File::from_raw_handle(handle as _) };
        // lpc-allow-raw-write: framed bytes go to an ephemeral named pipe, not persistent state.
        pipe.write_all(&encode_frame(&request)?)?;
        pipe.flush()?;
        decode_frame(&mut pipe)
    }

    fn wide(value: &str) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub(super) fn start(_paths: AppPaths) -> Result<()> {
        Ok(())
    }

    pub(super) fn execute(
        _paths: &AppPaths,
        _request: HostBridgeRequest,
    ) -> Result<HostBridgeResponse> {
        Err(LpcError::HostBridgeUnavailable(
            "the host bridge is only required on Windows".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framed_protocol_round_trips_unicode_arguments() {
        let request = HostBridgeRequest {
            version: PROTOCOL_VERSION,
            args: vec!["--lpc-account".into(), "道庸".into(), "whoami".into()],
        };
        let frame = encode_frame(&request).unwrap();
        let decoded: HostBridgeRequest = decode_frame(&mut frame.as_slice()).unwrap();
        assert_eq!(decoded.version, PROTOCOL_VERSION);
        assert_eq!(decoded.args, request.args);
    }

    #[test]
    fn pipe_name_is_stable_per_data_root() {
        let paths = AppPaths::new(r"C:\Users\Example\LPC");
        assert_eq!(pipe_name(&paths), pipe_name(&paths));
        assert_ne!(pipe_name(&paths), pipe_name(&AppPaths::new(r"C:\Other")));
    }

    #[cfg(windows)]
    #[test]
    fn host_bridge_cli_child_uses_platform_creation_flags() {
        assert_eq!(
            host_bridge_child_creation_flags(),
            windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
        );
    }
}
