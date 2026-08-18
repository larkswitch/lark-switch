use crate::account::AccountService;
use crate::error::{LpcError, Result};
use crate::model::{AppRecord, Brand};
use crate::redact::redact_text;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::process::{Child, ExitStatus};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use uuid::Uuid;

const VERIFICATION_URL_TIMEOUT: Duration = Duration::from_secs(45);
const READER_POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_CAPTURED_OUTPUT_BYTES: usize = 64 * 1024;

static HTTPS_URL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"https://[^\s\x00-\x1f\x7f<>"']+"#)
        .expect("app creation HTTPS URL regex must compile")
});
static DIAGNOSTIC_URL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"https?://[^\s\x00-\x1f\x7f<>"']+"#)
        .expect("app creation diagnostic URL regex must compile")
});
static CREDENTIAL_REMAINDER: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(app[\s_-]*secret|client[\s_-]*secret|token|credential|password)[^\r\n]*")
        .expect("app creation credential regex must compile")
});

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppCreationStart {
    pub flow_id: Uuid,
    pub verification_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppCreationProgress {
    pub complete: bool,
    pub app: Option<AppRecord>,
}

pub(crate) fn extract_verification_url(text: &str) -> Option<String> {
    HTTPS_URL.find(text).map(|value| value.as_str().to_owned())
}

fn redact_app_creation_diagnostics(text: &str) -> String {
    let redacted = redact_text(text);
    let redacted = DIAGNOSTIC_URL.replace_all(&redacted, "[REDACTED URL]");
    let redacted = CREDENTIAL_REMAINDER.replace_all(&redacted, "$1 [REDACTED]");
    redacted.chars().take(2048).collect()
}

#[derive(Clone)]
pub struct AppCreationCoordinator {
    shared: Arc<AppCreationShared>,
}

struct AppCreationShared {
    service: AccountService,
    flows: Mutex<HashMap<Uuid, ActiveAppCreation>>,
    in_flight_operations: AtomicUsize,
}

struct ActiveAppCreation {
    child: Child,
    stdout_reader: Option<JoinHandle<Vec<u8>>>,
    stderr_reader: Option<JoinHandle<Vec<u8>>>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    label: String,
    staging: std::path::PathBuf,
}

impl AppCreationCoordinator {
    pub fn new(service: AccountService) -> Self {
        Self {
            shared: Arc::new(AppCreationShared {
                service,
                flows: Mutex::new(HashMap::new()),
                in_flight_operations: AtomicUsize::new(0),
            }),
        }
    }

    pub fn begin(&self, label: &str, brand: Brand) -> Result<AppCreationStart> {
        if label.trim().is_empty() {
            return Err(LpcError::UnsafeConfig(
                "App display name must not be empty".into(),
            ));
        }
        let _starting = ActiveOperation::new(&self.shared.in_flight_operations);
        let flow_id = Uuid::new_v4();
        let staging = self
            .shared
            .service
            .store()
            .paths()
            .staging_dir()
            .join(format!("app-creation-{flow_id}"));
        fs::create_dir_all(&staging)?;

        let mut child = match self
            .shared
            .service
            .cli()
            .spawn_new_app_creation(&staging, brand)
        {
            Ok(child) => child,
            Err(error) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(error);
            }
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                terminate_child_best_effort(&mut child);
                let _ = fs::remove_dir_all(&staging);
                return Err(LpcError::Internal(
                    "official CLI stdout pipe is unavailable".into(),
                ));
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                terminate_child_best_effort(&mut child);
                let _ = fs::remove_dir_all(&staging);
                return Err(LpcError::Internal(
                    "official CLI stderr pipe is unavailable".into(),
                ));
            }
        };
        let stdout_reader = spawn_stream_reader(stdout, None);
        let (stderr_tx, stderr_rx) = mpsc::channel();
        let stderr_reader = spawn_stream_reader(stderr, Some(stderr_tx));
        let mut flow = ActiveAppCreation {
            child,
            stdout_reader: Some(stdout_reader),
            stderr_reader: Some(stderr_reader),
            stdout: Vec::new(),
            stderr: Vec::new(),
            label: label.to_owned(),
            staging,
        };

        let deadline = Instant::now() + VERIFICATION_URL_TIMEOUT;
        let mut streamed_stderr = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                terminate_child_best_effort(&mut flow.child);
                let _ = flow.finish_readers();
                return Err(LpcError::CliTimeout(VERIFICATION_URL_TIMEOUT.as_secs()));
            }

            match stderr_rx.recv_timeout(remaining.min(READER_POLL_INTERVAL)) {
                Ok(chunk) => {
                    append_bounded(&mut streamed_stderr, &chunk);
                    if let Some(url) =
                        extract_verification_url(&String::from_utf8_lossy(&streamed_stderr))
                    {
                        self.insert_flow(flow_id, flow)?;
                        return Ok(AppCreationStart {
                            flow_id,
                            verification_url: url,
                        });
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let mut status = flow.child.try_wait()?;
                    if status.is_none() {
                        terminate_child(&mut flow.child)?;
                        status = flow.child.try_wait()?;
                    }
                    flow.finish_readers()?;
                    if let Some(url) =
                        extract_verification_url(&String::from_utf8_lossy(&flow.stderr))
                    {
                        self.insert_flow(flow_id, flow)?;
                        return Ok(AppCreationStart {
                            flow_id,
                            verification_url: url,
                        });
                    }
                    return Err(missing_url_error(status, &flow.stdout, &flow.stderr));
                }
            }

            if let Some(status) = flow.child.try_wait()? {
                flow.finish_readers()?;
                if let Some(url) = extract_verification_url(&String::from_utf8_lossy(&flow.stderr))
                {
                    self.insert_flow(flow_id, flow)?;
                    return Ok(AppCreationStart {
                        flow_id,
                        verification_url: url,
                    });
                }
                return Err(missing_url_error(Some(status), &flow.stdout, &flow.stderr));
            }
        }
    }

    pub fn poll(&self, flow_id: Uuid) -> Result<AppCreationProgress> {
        let mut flows = self
            .shared
            .flows
            .lock()
            .map_err(|_| LpcError::Internal("App creation coordinator lock poisoned".into()))?;
        let status = flows
            .get_mut(&flow_id)
            .ok_or_else(|| app_creation_flow_not_found(flow_id))?
            .child
            .try_wait()?;
        let Some(status) = status else {
            return Ok(AppCreationProgress {
                complete: false,
                app: None,
            });
        };
        let _finalizing = ActiveOperation::new(&self.shared.in_flight_operations);
        let mut flow = flows
            .remove(&flow_id)
            .ok_or_else(|| app_creation_flow_not_found(flow_id))?;
        drop(flows);

        flow.finish_readers()?;
        if !status.success() {
            return Err(cli_failure(status, &flow.stdout, &flow.stderr));
        }
        let app = self
            .shared
            .service
            .import_official_config(&flow.label, &flow.staging)
            .map_err(sanitize_app_creation_error)?;
        Ok(AppCreationProgress {
            complete: true,
            app: Some(app),
        })
    }

    pub fn cancel(&self, flow_id: Uuid) -> Result<()> {
        let mut flows = self
            .shared
            .flows
            .lock()
            .map_err(|_| LpcError::Internal("App creation coordinator lock poisoned".into()))?;
        let _canceling = ActiveOperation::new(&self.shared.in_flight_operations);
        let mut flow = flows
            .remove(&flow_id)
            .ok_or_else(|| app_creation_flow_not_found(flow_id))?;
        drop(flows);
        terminate_child(&mut flow.child)?;
        flow.finish_readers()?;
        Ok(())
    }

    pub fn active_flow_count(&self) -> usize {
        let active = self
            .shared
            .flows
            .lock()
            .map(|flows| flows.len())
            .unwrap_or(1);
        let in_flight = self.shared.in_flight_operations.load(Ordering::Acquire);
        in_flight.saturating_add(active)
    }

    fn insert_flow(&self, flow_id: Uuid, flow: ActiveAppCreation) -> Result<()> {
        self.shared
            .flows
            .lock()
            .map_err(|_| LpcError::Internal("App creation coordinator lock poisoned".into()))?
            .insert(flow_id, flow);
        Ok(())
    }
}

impl AppCreationShared {
    fn cleanup_all(&mut self) {
        let flows = self
            .flows
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        flows.clear();
    }
}

impl Drop for AppCreationShared {
    fn drop(&mut self) {
        self.cleanup_all();
    }
}

impl ActiveAppCreation {
    fn finish_readers(&mut self) -> Result<()> {
        if let Some(reader) = self.stdout_reader.take() {
            self.stdout = reader
                .join()
                .map_err(|_| LpcError::Internal("official CLI stdout reader panicked".into()))?;
        }
        if let Some(reader) = self.stderr_reader.take() {
            self.stderr = reader
                .join()
                .map_err(|_| LpcError::Internal("official CLI stderr reader panicked".into()))?;
        }
        Ok(())
    }
}

impl Drop for ActiveAppCreation {
    fn drop(&mut self) {
        terminate_child_best_effort(&mut self.child);
        let _ = self.finish_readers();
        let _ = fs::remove_dir_all(&self.staging);
    }
}

struct ActiveOperation<'a>(&'a AtomicUsize);

impl<'a> ActiveOperation<'a> {
    fn new(count: &'a AtomicUsize) -> Self {
        count.fetch_add(1, Ordering::AcqRel);
        Self(count)
    }
}

impl Drop for ActiveOperation<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn spawn_stream_reader<R>(
    mut reader: R,
    chunks: Option<mpsc::Sender<Vec<u8>>>,
) -> JoinHandle<Vec<u8>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut captured = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let bytes = &chunk[..count];
                    append_bounded(&mut captured, bytes);
                    if let Some(sender) = &chunks {
                        let _ = sender.send(bytes.to_vec());
                    }
                }
            }
        }
        captured
    })
}

fn append_bounded(buffer: &mut Vec<u8>, bytes: &[u8]) {
    if bytes.len() >= MAX_CAPTURED_OUTPUT_BYTES {
        buffer.clear();
        buffer.extend_from_slice(&bytes[bytes.len() - MAX_CAPTURED_OUTPUT_BYTES..]);
        return;
    }
    let overflow = buffer
        .len()
        .saturating_add(bytes.len())
        .saturating_sub(MAX_CAPTURED_OUTPUT_BYTES);
    if overflow > 0 {
        buffer.drain(..overflow);
    }
    buffer.extend_from_slice(bytes);
}

fn terminate_child(child: &mut Child) -> Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    if let Err(error) = child.kill() {
        if child.try_wait()?.is_none() {
            return Err(error.into());
        }
        return Ok(());
    }
    child.wait()?;
    Ok(())
}

fn terminate_child_best_effort(child: &mut Child) {
    let _ = terminate_child(child);
}

fn app_creation_flow_not_found(flow_id: Uuid) -> LpcError {
    LpcError::Internal(format!("App creation flow {flow_id} is not active"))
}

fn missing_url_error(status: Option<ExitStatus>, stdout: &[u8], stderr: &[u8]) -> LpcError {
    let code = status.and_then(|value| value.code()).unwrap_or(1);
    let diagnostics = redacted_diagnostics(stdout, stderr);
    let message = if diagnostics.is_empty() {
        "official CLI did not provide an HTTPS verification URL".into()
    } else {
        format!("official CLI did not provide an HTTPS verification URL: {diagnostics}")
    };
    LpcError::CliFailed { code, message }
}

fn cli_failure(status: ExitStatus, stdout: &[u8], stderr: &[u8]) -> LpcError {
    let diagnostics = redacted_diagnostics(stdout, stderr);
    LpcError::CliFailed {
        code: status.code().unwrap_or(1),
        message: if diagnostics.is_empty() {
            "official App creation failed".into()
        } else {
            diagnostics
        },
    }
}

fn sanitize_app_creation_error(error: LpcError) -> LpcError {
    match error {
        LpcError::CliFailed { code, message } => LpcError::CliFailed {
            code,
            message: redact_app_creation_diagnostics(&message),
        },
        LpcError::InvalidCliOutput(message) => {
            LpcError::InvalidCliOutput(redact_app_creation_diagnostics(&message))
        }
        LpcError::UnsafeConfig(message) => {
            LpcError::UnsafeConfig(redact_app_creation_diagnostics(&message))
        }
        LpcError::Internal(message) => {
            LpcError::Internal(redact_app_creation_diagnostics(&message))
        }
        error => error,
    }
}

fn redacted_diagnostics(stdout: &[u8], stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let stdout = String::from_utf8_lossy(stdout);
    let source = if stderr.trim().is_empty() {
        stdout.as_ref()
    } else {
        stderr.as_ref()
    };
    redact_app_creation_diagnostics(source.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AccountService, AppPaths, Brand, OfficialCli, StateStore};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::thread;
    use std::time::Duration;
    use uuid::Uuid;

    #[test]
    fn extracts_only_the_https_verification_url_from_streamed_output() {
        let text = "QR noise\n  https://open.example/device?code=a-b_c\nhttps://ignored.example/second\nwaiting";

        assert_eq!(
            extract_verification_url(text),
            Some("https://open.example/device?code=a-b_c".into())
        );
        assert_eq!(
            extract_verification_url("http://open.example/device https-not-a-url"),
            None
        );
    }

    #[test]
    fn app_creation_dto_contains_no_secret_or_shell_command() {
        let dto = AppCreationStart {
            flow_id: Uuid::new_v4(),
            verification_url: "https://example.test/device".into(),
        };

        let json = serde_json::to_string(&dto).unwrap().to_ascii_lowercase();
        assert!(!json.contains("secret"));
        assert!(!json.contains("command"));
        assert!(!json.contains("argv"));
    }

    #[test]
    fn app_creation_failure_diagnostics_hide_urls_and_credentials() {
        let output = "open https://example.test/device?code=opaque http://example.test/fallback\nApp Secret: fixture-visible\napp_secret=fixture-value";

        let redacted = redact_app_creation_diagnostics(output);

        assert!(!redacted.contains("https://"));
        assert!(!redacted.contains("http://"));
        assert!(!redacted.contains("opaque"));
        assert!(!redacted.contains("fixture-visible"));
        assert!(!redacted.contains("fixture-value"));
    }

    #[test]
    fn coordinator_begin_then_cancel_stops_the_flow_and_removes_staging() {
        let temp = tempfile::tempdir().unwrap();
        let cli_path = write_fake_cli(temp.path(), FakeCliMode::WaitForCancel);
        let coordinator = test_coordinator(&temp, &cli_path);

        let start = coordinator
            .begin("Workspace App &|%^'", Brand::Feishu)
            .unwrap();
        let staging = temp
            .path()
            .join("lpc")
            .join("staging")
            .join(format!("app-creation-{}", start.flow_id));

        assert_eq!(
            start.verification_url,
            "https://example.test/device?code=opaque"
        );
        assert_eq!(coordinator.active_flow_count(), 1);
        assert!(staging.is_dir());

        coordinator.cancel(start.flow_id).unwrap();

        assert_eq!(coordinator.active_flow_count(), 0);
        assert!(!staging.exists());
    }

    #[test]
    fn coordinator_poll_imports_successful_config_and_cleans_staging() {
        let temp = tempfile::tempdir().unwrap();
        let cli_path = write_fake_cli(temp.path(), FakeCliMode::Complete);
        let coordinator = test_coordinator(&temp, &cli_path);
        let start = coordinator.begin("Workspace App", Brand::Feishu).unwrap();
        let staging = temp
            .path()
            .join("lpc")
            .join("staging")
            .join(format!("app-creation-{}", start.flow_id));

        let progress = (0..100)
            .find_map(|_| {
                let progress = coordinator.poll(start.flow_id).unwrap();
                if progress.complete {
                    Some(progress)
                } else {
                    thread::sleep(Duration::from_millis(10));
                    None
                }
            })
            .expect("fake CLI should complete");

        assert_eq!(progress.app.unwrap().label, "Workspace App");
        assert_eq!(coordinator.active_flow_count(), 0);
        assert!(!staging.exists());
    }

    #[test]
    fn coordinator_counts_flow_as_active_while_success_is_being_imported() {
        let temp = tempfile::tempdir().unwrap();
        let cli_path = write_fake_cli(temp.path(), FakeCliMode::CompleteWithBlockedScopes);
        let coordinator = test_coordinator(&temp, &cli_path);
        let start = coordinator.begin("Workspace App", Brand::Feishu).unwrap();
        let staging = temp
            .path()
            .join("lpc")
            .join("staging")
            .join(format!("app-creation-{}", start.flow_id));
        let poller = {
            let coordinator = coordinator.clone();
            thread::spawn(move || loop {
                let progress = coordinator.poll(start.flow_id).unwrap();
                if progress.complete {
                    break progress;
                }
                thread::sleep(Duration::from_millis(10));
            })
        };
        let scopes_started = staging.join("scopes-started");
        for _ in 0..200 {
            if scopes_started.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        let active_during_import = coordinator.active_flow_count();
        fs::write(staging.join("allow-scopes"), b"continue").unwrap();
        let progress = poller.join().unwrap();

        assert!(progress.complete);
        assert_eq!(active_during_import, 1);
        assert_eq!(coordinator.active_flow_count(), 0);
    }

    #[test]
    fn coordinator_sanitizes_user_visible_scopes_import_failure() {
        let temp = tempfile::tempdir().unwrap();
        let cli_path = write_fake_cli(temp.path(), FakeCliMode::CompleteWithScopesFailure);
        let coordinator = test_coordinator(&temp, &cli_path);
        let start = coordinator.begin("Workspace App", Brand::Feishu).unwrap();

        let error = (0..100)
            .find_map(|_| match coordinator.poll(start.flow_id) {
                Ok(progress) if !progress.complete => {
                    thread::sleep(Duration::from_millis(10));
                    None
                }
                Ok(_) => panic!("scopes import failure must not complete the flow"),
                Err(error) => Some(error),
            })
            .expect("fake CLI scopes import should fail");

        assert_eq!(error.stable_code(), "LPC_CLI_FAILED");
        assert!(matches!(error, LpcError::CliFailed { code: 23, .. }));
        let user_visible = format!("[{}] {error}", error.stable_code());
        for sensitive in [
            "https://",
            "http://",
            "scope-visible",
            "url-visible",
            "import-secret-visible",
            "client-secret-visible",
            "access-token-visible",
        ] {
            assert!(
                !user_visible.contains(sensitive),
                "user-visible App creation error leaked {sensitive}: {user_visible}"
            );
        }
    }

    fn test_coordinator(temp: &tempfile::TempDir, cli_path: &Path) -> AppCreationCoordinator {
        let store = StateStore::new(AppPaths::new(temp.path().join("lpc")));
        store.initialize().unwrap();
        AppCreationCoordinator::new(AccountService::new(store, OfficialCli::new(cli_path)))
    }

    #[derive(Clone, Copy)]
    enum FakeCliMode {
        WaitForCancel,
        Complete,
        CompleteWithBlockedScopes,
        CompleteWithScopesFailure,
    }

    #[cfg(windows)]
    fn write_fake_cli(root: &Path, mode: FakeCliMode) -> PathBuf {
        let path = root.join("fake-lark-cli.cmd");
        let script = match mode {
            FakeCliMode::WaitForCancel => {
                r#"@echo off
>&2 echo https://example.test/device?code=opaque
:wait
goto wait
"#
            }
            FakeCliMode::Complete
            | FakeCliMode::CompleteWithBlockedScopes
            | FakeCliMode::CompleteWithScopesFailure => {
                let block_scopes = matches!(mode, FakeCliMode::CompleteWithBlockedScopes);
                if block_scopes {
                    return write_windows_blocked_scopes_cli(&path);
                }
                if matches!(mode, FakeCliMode::CompleteWithScopesFailure) {
                    return write_windows_scopes_failure_cli(&path);
                }
                r#"@echo off
if "%~1"=="config" (
  >"%LARKSUITE_CLI_CONFIG_DIR%\config.json" echo {"currentApp":"lpc","apps":[{"name":"lpc","appId":"cli_fixture","appSecret":{"source":"keychain","id":"appsecret:cli_fixture"},"brand":"feishu","users":[]}]}
  >&2 echo https://example.test/device?code=opaque
  exit /b 0
)
if "%~1"=="auth" (
  echo {"appId":"cli_fixture","brand":"feishu","tokenType":"user","userScopes":["im:read"],"count":1}
  exit /b 0
)
exit /b 1
"#
            }
        };
        fs::write(&path, script).unwrap();
        path
    }

    #[cfg(windows)]
    fn write_windows_blocked_scopes_cli(path: &Path) -> PathBuf {
        let script = r#"@echo off
if "%~1"=="config" (
  >"%LARKSUITE_CLI_CONFIG_DIR%\config.json" echo {"currentApp":"lpc","apps":[{"name":"lpc","appId":"cli_fixture","appSecret":{"source":"keychain","id":"appsecret:cli_fixture"},"brand":"feishu","users":[]}]}
  >&2 echo https://example.test/device?code=opaque
  exit /b 0
)
if "%~1"=="auth" (
  >"%LARKSUITE_CLI_CONFIG_DIR%\scopes-started" echo started
  :wait_scopes
  if not exist "%LARKSUITE_CLI_CONFIG_DIR%\allow-scopes" goto wait_scopes
  echo {"appId":"cli_fixture","brand":"feishu","tokenType":"user","userScopes":["im:read"],"count":1}
  exit /b 0
)
exit /b 1
"#;
        fs::write(path, script).unwrap();
        path.to_path_buf()
    }

    #[cfg(windows)]
    fn write_windows_scopes_failure_cli(path: &Path) -> PathBuf {
        let script = r#"@echo off
if "%~1"=="config" (
  >"%LARKSUITE_CLI_CONFIG_DIR%\config.json" echo {"currentApp":"lpc","apps":[{"name":"lpc","appId":"cli_fixture","appSecret":{"source":"keychain","id":"appsecret:cli_fixture"},"brand":"feishu","users":[]}]}
  >&2 echo https://example.test/device?code=opaque
  exit /b 0
)
if "%~1"=="auth" (
  >&2 echo scopes failed at https://auth.example.test/verify?code=scope-visible
  >&2 echo fallback http://auth.example.test/fallback?token=url-visible
  >&2 echo App Secret: import-secret-visible
  >&2 echo client-secret=client-secret-visible
  >&2 echo access_token=access-token-visible
  exit /b 23
)
exit /b 1
"#;
        fs::write(path, script).unwrap();
        path.to_path_buf()
    }

    #[cfg(unix)]
    fn write_fake_cli(root: &Path, mode: FakeCliMode) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = root.join("fake-lark-cli.sh");
        let script = match mode {
            FakeCliMode::WaitForCancel => {
                r#"#!/bin/sh
echo 'https://example.test/device?code=opaque' >&2
while :; do :; done
"#
            }
            FakeCliMode::Complete
            | FakeCliMode::CompleteWithBlockedScopes
            | FakeCliMode::CompleteWithScopesFailure => {
                let block_scopes = matches!(mode, FakeCliMode::CompleteWithBlockedScopes);
                if block_scopes {
                    return write_unix_blocked_scopes_cli(&path);
                }
                if matches!(mode, FakeCliMode::CompleteWithScopesFailure) {
                    return write_unix_scopes_failure_cli(&path);
                }
                r#"#!/bin/sh
if [ "$1" = "config" ]; then
  printf '%s\n' '{"currentApp":"lpc","apps":[{"name":"lpc","appId":"cli_fixture","appSecret":{"source":"keychain","id":"appsecret:cli_fixture"},"brand":"feishu","users":[]}]}' > "$LARKSUITE_CLI_CONFIG_DIR/config.json"
  echo 'https://example.test/device?code=opaque' >&2
  exit 0
fi
if [ "$1" = "auth" ]; then
  printf '%s\n' '{"appId":"cli_fixture","brand":"feishu","tokenType":"user","userScopes":["im:read"],"count":1}'
  exit 0
fi
exit 1
"#
            }
        };
        fs::write(&path, script).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    #[cfg(unix)]
    fn write_unix_blocked_scopes_cli(path: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let script = r#"#!/bin/sh
if [ "$1" = "config" ]; then
  printf '%s\n' '{"currentApp":"lpc","apps":[{"name":"lpc","appId":"cli_fixture","appSecret":{"source":"keychain","id":"appsecret:cli_fixture"},"brand":"feishu","users":[]}]}' > "$LARKSUITE_CLI_CONFIG_DIR/config.json"
  echo 'https://example.test/device?code=opaque' >&2
  exit 0
fi
if [ "$1" = "auth" ]; then
  printf '%s\n' started > "$LARKSUITE_CLI_CONFIG_DIR/scopes-started"
  while [ ! -f "$LARKSUITE_CLI_CONFIG_DIR/allow-scopes" ]; do :; done
  printf '%s\n' '{"appId":"cli_fixture","brand":"feishu","tokenType":"user","userScopes":["im:read"],"count":1}'
  exit 0
fi
exit 1
"#;
        fs::write(path, script).unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).unwrap();
        path.to_path_buf()
    }

    #[cfg(unix)]
    fn write_unix_scopes_failure_cli(path: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let script = r#"#!/bin/sh
if [ "$1" = "config" ]; then
  printf '%s\n' '{"currentApp":"lpc","apps":[{"name":"lpc","appId":"cli_fixture","appSecret":{"source":"keychain","id":"appsecret:cli_fixture"},"brand":"feishu","users":[]}]}' > "$LARKSUITE_CLI_CONFIG_DIR/config.json"
  echo 'https://example.test/device?code=opaque' >&2
  exit 0
fi
if [ "$1" = "auth" ]; then
  echo 'scopes failed at https://auth.example.test/verify?code=scope-visible' >&2
  echo 'fallback http://auth.example.test/fallback?token=url-visible' >&2
  echo 'App Secret: import-secret-visible' >&2
  echo 'client-secret=client-secret-visible' >&2
  echo 'access_token=access-token-visible' >&2
  exit 23
fi
exit 1
"#;
        fs::write(path, script).unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).unwrap();
        path.to_path_buf()
    }
}
