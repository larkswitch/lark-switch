//! Executes a managed CLI command in the unpackaged desktop process' registry view.
//!
//! Sandboxed callers can share LPC's files while Windows redirects HKCU. They must
//! never run the official CLI against that shadow keychain. The desktop owns a
//! local-only named pipe and launches the already-installed shim as its child, so
//! routing, locking, management guards, and audit logging remain on the normal path.

use crate::error::{LpcError, Result};
use crate::paths::AppPaths;
use serde::{Deserialize, Serialize};

const PROTOCOL_VERSION: u32 = 3;
#[cfg(any(windows, test))]
const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;
#[cfg(windows)]
const MAX_STDIN_BYTES: usize = 16 * 1024 * 1024;

#[cfg(windows)]
fn host_bridge_child_creation_flags() -> u32 {
    windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
}

#[cfg(windows)]
fn configure_host_bridge_child(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(host_bridge_child_creation_flags());
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HostBridgeRequest {
    version: u32,
    target: HostBridgeTarget,
    args: Vec<String>,
    stdin_utf8: Option<String>,
    current_dir: std::path::PathBuf,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
enum HostBridgeTarget {
    OfficialCli,
    Control,
}

/// An environment flag alone is not authority: require the actual parent to be
/// the installed desktop, launched in the Task Scheduler bootstrap mode.
#[cfg(windows)]
pub(crate) fn is_host_bridge_child() -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let Some(expected_pid) = std::env::var("LPC_HOST_EXECUTOR_PID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
    else {
        return false;
    };
    let mut system = System::new();
    let current = Pid::from_u32(std::process::id());
    let expected = Pid::from_u32(expected_pid);
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[current, expected]),
        ProcessRefreshKind::new()
            .with_exe(UpdateKind::Always)
            .with_cmd(UpdateKind::Always),
    );
    let actual_parent = system.process(current).and_then(|process| process.parent());
    let Some(parent) = system.process(expected) else {
        return false;
    };
    let installed =
        crate::expected_installed_desktop_exe().and_then(|path| path.canonicalize().ok());
    let parent_exe = parent.exe().and_then(|path| path.canonicalize().ok());
    actual_parent == Some(expected)
        && installed.is_some()
        && parent_exe == installed
        && parent.cmd().iter().any(|arg| arg == "--host-bootstrap")
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
    execute_target(paths, args, HostBridgeTarget::OfficialCli)
}

pub fn execute_control_via_host_bridge(
    paths: &AppPaths,
    args: &[std::ffi::OsString],
) -> Result<HostBridgeResponse> {
    execute_target(paths, args, HostBridgeTarget::Control)
}

fn execute_target(
    paths: &AppPaths,
    args: &[std::ffi::OsString],
    target: HostBridgeTarget,
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
    #[cfg(windows)]
    let stdin_utf8 = read_requested_stdin(&args)?;
    #[cfg(not(windows))]
    let stdin_utf8 = None;
    let request = HostBridgeRequest {
        version: PROTOCOL_VERSION,
        target,
        args,
        stdin_utf8,
        current_dir: std::env::current_dir()?,
    };
    match platform::execute(paths, request.clone()) {
        Err(LpcError::HostBridgeUnavailable(_)) if target == HostBridgeTarget::Control => {
            crate::run_host_bootstrap_task()?;
            for _ in 0..60 {
                std::thread::sleep(std::time::Duration::from_millis(250));
                // Reuse the captured stdin; a connection retry must not read it twice.
                match platform::execute(paths, request.clone()) {
                    Err(LpcError::HostBridgeUnavailable(_)) => continue,
                    result => return result,
                }
            }
            Err(LpcError::HostBridgeUnavailable(
                "the scheduled desktop host did not become available".into(),
            ))
        }
        result => result,
    }
}

#[cfg(any(windows, test))]
fn requests_stdin(args: &[String]) -> bool {
    // Git's helper protocol carries protocol/host/path on stdin implicitly.
    // The host bridge must preserve it even without a '-' or '*-stdin' flag.
    args.windows(2)
        .any(|pair| pair[0] == "apps" && pair[1] == "git-credential-helper")
        && !args.iter().any(|arg| arg == "--help" || arg == "-h")
        || args.iter().any(|arg| {
            arg == "-"
                || arg.ends_with("-stdin")
                || arg.strip_prefix('-').is_some_and(|arg| arg.ends_with("=-"))
        })
}

#[cfg(windows)]
fn read_requested_stdin(args: &[String]) -> Result<Option<String>> {
    use std::io::Read;

    if !requests_stdin(args) {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .take((MAX_STDIN_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_STDIN_BYTES {
        return Err(LpcError::HostBridgeUnavailable(format!(
            "stdin exceeded the host bridge limit of {MAX_STDIN_BYTES} bytes"
        )));
    }
    String::from_utf8(bytes).map(Some).map_err(|_| {
        LpcError::HostBridgeUnavailable("host bridge stdin must be valid UTF-8".into())
    })
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

        let shim = paths.bin_dir().join(match request.target {
            HostBridgeTarget::OfficialCli => "lark-cli.exe",
            HostBridgeTarget::Control => "lpcctl.exe",
        });
        let mut command = Command::new(&shim);
        configure_host_bridge_child(&mut command);
        command
            .args(&request.args)
            .current_dir(&request.current_dir)
            .env("LPC_HOST_EXECUTOR_PID", std::process::id().to_string())
            .env("LPC_HOME", paths.root())
            .stdin(if request.stdin_utf8.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let response = match run_child(command, request.stdin_utf8) {
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
        tracing::info!(target = ?request.target, exit_code = response.exit_code, "host CLI bridge request completed");
        // lpc-allow-raw-write: framed bytes go to an ephemeral named pipe, not persistent state.
        pipe.write_all(&encode_frame(&response)?)?;
        pipe.flush()?;
        Ok(())
    }

    fn run_child(
        mut command: Command,
        stdin_utf8: Option<String>,
    ) -> std::io::Result<std::process::Output> {
        let mut child = command.spawn()?;
        if let Some(stdin_utf8) = stdin_utf8 {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| std::io::Error::other("host bridge child stdin was not piped"))?;
            stdin.write_all(stdin_utf8.as_bytes())?;
        }
        child.wait_with_output()
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

    #[cfg(windows)]
    #[test]
    fn environment_hint_cannot_authorize_an_unrelated_process() {
        let old = std::env::var_os("LPC_HOST_EXECUTOR_PID");
        std::env::set_var("LPC_HOST_EXECUTOR_PID", std::process::id().to_string());
        assert!(!is_host_bridge_child());
        match old {
            Some(value) => std::env::set_var("LPC_HOST_EXECUTOR_PID", value),
            None => std::env::remove_var("LPC_HOST_EXECUTOR_PID"),
        }
    }

    #[test]
    fn framed_protocol_round_trips_unicode_arguments() {
        let request = HostBridgeRequest {
            version: PROTOCOL_VERSION,
            target: HostBridgeTarget::Control,
            args: vec!["--lpc-account".into(), "道庸".into(), "whoami".into()],
            stdin_utf8: Some("{\"中文\":true}\n".into()),
            current_dir: std::path::PathBuf::from("workspace"),
        };
        let frame = encode_frame(&request).unwrap();
        let decoded: HostBridgeRequest = decode_frame(&mut frame.as_slice()).unwrap();
        assert_eq!(decoded.version, PROTOCOL_VERSION);
        assert_eq!(decoded.args, request.args);
        assert_eq!(decoded.stdin_utf8, request.stdin_utf8);
        assert_eq!(decoded.target, request.target);
        assert_eq!(decoded.current_dir, request.current_dir);
    }

    #[test]
    fn stdin_is_captured_for_explicit_arguments_and_git_helper_protocol() {
        assert!(requests_stdin(&["--cells=-".into()]));
        assert!(requests_stdin(&["--cells".into(), "-".into()]));
        assert!(requests_stdin(&["--app-secret-stdin".into()]));
        assert!(!requests_stdin(&["--cells=@payload.json".into()]));
        assert!(!requests_stdin(&[
            "whoami".into(),
            "--as".into(),
            "user".into()
        ]));
        for operation in ["get", "store", "erase"] {
            assert!(requests_stdin(&[
                "--lpc-account".into(),
                "selected-account".into(),
                "apps".into(),
                "git-credential-helper".into(),
                "--app-id".into(),
                "app_fixture".into(),
                operation.into(),
            ]));
        }
        assert!(!requests_stdin(&[
            "apps".into(),
            "git-credential-helper".into(),
            "--help".into()
        ]));
        assert!(!requests_stdin(&[
            "apps".into(),
            "+git-credential-init".into(),
            "--app-id".into(),
            "app_fixture".into()
        ]));
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
