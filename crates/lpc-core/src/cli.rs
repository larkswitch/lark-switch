use crate::error::{LpcError, Result};
#[cfg(not(test))]
use crate::locking::try_acquire_cli_keychain_lock;
use crate::locking::{release_cli_keychain_lock_when_process_exits, CliKeychainGuard};
use crate::model::Brand;
#[cfg(not(test))]
use crate::paths::AppPaths;
use crate::redact::redact_text;
use semver::Version;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::Duration;
use uuid::Uuid;
use wait_timeout::ChildExt;
use zeroize::Zeroize;

#[derive(Default)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Debug, Clone)]
pub struct OfficialCli {
    executable: PathBuf,
    timeout: Duration,
}

#[derive(Debug)]
pub struct ProcessOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegatedUser {
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub open_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WhoAmI {
    #[serde(default)]
    pub profile: String,
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub brand: String,
    #[serde(default)]
    pub default_as: String,
    #[serde(default)]
    pub identity: String,
    #[serde(default)]
    pub identity_source: String,
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub token_status: String,
    pub on_behalf_of: Option<DelegatedUser>,
    #[serde(default)]
    pub hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthScopes {
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub brand: String,
    #[serde(default)]
    pub token_type: String,
    #[serde(default)]
    pub user_scopes: BTreeSet<String>,
    #[serde(default)]
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct IdentityStatus {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub available: bool,
    pub verified: Option<bool>,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub hint: String,
    #[serde(default)]
    pub open_id: String,
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub token_status: String,
    #[serde(default, deserialize_with = "deserialize_scope_set")]
    pub scope: BTreeSet<String>,
    #[serde(default)]
    pub expires_at: String,
    #[serde(default)]
    pub refresh_expires_at: String,
}

fn deserialize_scope_set<'de, D>(deserializer: D) -> std::result::Result<BTreeSet<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    let mut result = BTreeSet::new();
    match value {
        None | Some(Value::Null) => {}
        Some(Value::String(text)) => {
            result.extend(
                text.split_whitespace()
                    .filter(|v| !v.is_empty())
                    .map(str::to_owned),
            );
        }
        Some(Value::Array(values)) => {
            for value in values {
                if let Some(text) = value.as_str() {
                    result.insert(text.to_owned());
                }
            }
        }
        Some(other) => {
            return Err(serde::de::Error::custom(format!(
                "invalid scope value: {other}"
            )));
        }
    }
    Ok(result)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct IdentityDiagnostics {
    #[serde(default)]
    pub user: IdentityStatus,
    #[serde(default)]
    pub bot: IdentityStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub brand: String,
    #[serde(default)]
    pub identity: String,
    pub verified: Option<bool>,
    #[serde(default)]
    pub identities: IdentityDiagnostics,
}

impl AuthStatus {
    pub fn effective_user_scopes(&self) -> &BTreeSet<String> {
        &self.identities.user.scope
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DeviceAuthorization {
    #[serde(alias = "verification_uri_complete")]
    pub verification_url: String,
    pub device_code: String,
    #[serde(default)]
    pub expires_in: u64,
}

#[derive(Debug, Clone)]
pub struct CliJson<T> {
    pub value: T,
    pub stderr: String,
}

impl OfficialCli {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            timeout: Duration::from_secs(60),
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }

    pub fn version(&self) -> Result<Version> {
        let output = self.run_capture(None, ["--version"], None, Duration::from_secs(15))?;
        ensure_success(&output)?;
        let text = String::from_utf8_lossy(&output.stdout);
        let version = text
            .split_whitespace()
            .find(|part| part.chars().next().is_some_and(|c| c.is_ascii_digit()))
            .ok_or_else(|| {
                LpcError::InvalidCliOutput(format!("unrecognized version output: {text}"))
            })?;
        Ok(Version::parse(version.trim_start_matches('v'))?)
    }

    pub fn compatibility_check(&self) -> Result<()> {
        let version = self.version()?;
        let observed = version.to_string();
        if !crate::SUPPORTED_CLI_VERSIONS.contains(&observed.as_str()) {
            return Err(LpcError::RuntimeIncompatible(format!(
                "lark-cli {version} is not in the tested allowlist {:?}",
                crate::SUPPORTED_CLI_VERSIONS
            )));
        }
        for args in [
            vec!["profile", "--help"],
            vec!["auth", "scopes", "--help"],
            vec!["auth", "status", "--help"],
            vec!["auth", "login", "--help"],
            vec!["whoami", "--help"],
        ] {
            let output = self.run_capture(None, args, None, Duration::from_secs(15))?;
            ensure_success(&output)?;
        }
        Ok(())
    }

    pub fn config_init_existing(
        &self,
        config_dir: &Path,
        app_id: &str,
        app_secret: &SecretString,
        brand: Brand,
    ) -> Result<()> {
        if app_secret.is_empty() {
            return Err(LpcError::UnsafeConfig("App Secret is empty".into()));
        }
        let args = vec![
            "config".to_owned(),
            "init".to_owned(),
            "--app-id".to_owned(),
            app_id.to_owned(),
            "--app-secret-stdin".to_owned(),
            "--brand".to_owned(),
            brand.as_cli_value().to_owned(),
        ];
        let output = self.run_capture(Some(config_dir), args, Some(app_secret), self.timeout)?;
        ensure_success(&output)
    }

    pub fn scopes(&self, config_dir: &Path) -> Result<CliJson<AuthScopes>> {
        self.run_json(config_dir, ["auth", "scopes", "--json"])
    }

    pub fn status(&self, config_dir: &Path, verify: bool) -> Result<CliJson<AuthStatus>> {
        let mut args = vec!["auth", "status", "--json"];
        if verify {
            args.push("--verify");
        }
        self.run_json(config_dir, args)
    }

    pub fn whoami(&self, config_dir: &Path) -> Result<CliJson<WhoAmI>> {
        self.run_json(config_dir, ["whoami"])
    }

    pub fn begin_login(
        &self,
        config_dir: &Path,
        scopes: &BTreeSet<String>,
    ) -> Result<CliJson<DeviceAuthorization>> {
        if scopes.is_empty() {
            return Err(LpcError::ScopeOutOfBoundary(Vec::new()));
        }
        let joined = scopes.iter().cloned().collect::<Vec<_>>().join(" ");
        self.run_json_owned(
            config_dir,
            vec![
                "auth".into(),
                "login".into(),
                "--no-wait".into(),
                "--json".into(),
                "--scope".into(),
                joined,
            ],
            Duration::from_secs(45),
        )
    }

    pub fn complete_login(
        &self,
        config_dir: &Path,
        device_code: &SecretString,
    ) -> Result<CliJson<Value>> {
        self.complete_login_with_timeout(config_dir, device_code, Duration::from_secs(660))
    }

    pub fn complete_login_with_timeout(
        &self,
        config_dir: &Path,
        device_code: &SecretString,
        timeout: Duration,
    ) -> Result<CliJson<Value>> {
        let args = vec![
            "auth".to_owned(),
            "login".to_owned(),
            "--device-code".to_owned(),
            String::from_utf8_lossy(device_code.expose_bytes()).into_owned(),
            "--json".to_owned(),
        ];
        self.run_json_owned(config_dir, args, timeout)
    }

    pub fn render_qrcode_png(&self, working_dir: &Path, verification_url: &str) -> Result<Vec<u8>> {
        fs::create_dir_all(working_dir)?;
        let name = format!("oauth-{}.png", Uuid::new_v4());
        let path = working_dir.join(&name);
        let output = match self.run_capture_internal(
            None,
            Some(working_dir),
            [
                "auth",
                "qrcode",
                verification_url,
                "--output",
                name.as_str(),
                "--size",
                "256",
            ],
            None,
            Duration::from_secs(15),
        ) {
            Ok(output) => output,
            Err(error) => {
                let _ = fs::remove_file(&path);
                return Err(error);
            }
        };
        if let Err(error) = ensure_success(&output) {
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        read_validated_png_and_remove(&path)
    }

    pub fn logout(&self, config_dir: &Path) -> Result<()> {
        let output = self.run_capture(
            Some(config_dir),
            ["auth", "logout", "--json"],
            None,
            self.timeout,
        )?;
        ensure_success(&output)
    }

    pub fn run_interactive<I, S>(&self, config_dir: Option<&Path>, args: I) -> Result<i32>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let _lock = acquire_core_cli_keychain_lock()?;
        let mut command = Command::new(&self.executable);
        command.args(args);
        if let Some(config_dir) = config_dir {
            command.env("LARKSUITE_CLI_CONFIG_DIR", config_dir);
        }
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let status = command.status()?;
        Ok(status.code().unwrap_or(1))
    }

    fn new_app_creation_command(&self, config_dir: &Path, brand: Brand) -> Command {
        let mut command = Command::new(&self.executable);
        command.args(["config", "init", "--new", "--brand", brand.as_cli_value()]);
        command.env("LARKSUITE_CLI_CONFIG_DIR", config_dir);
        command
    }

    pub fn spawn_new_app_creation(&self, config_dir: &Path, brand: Brand) -> Result<Child> {
        if !self.executable.is_file() {
            return Err(LpcError::RuntimeMissing(self.executable.clone()));
        }
        fs::create_dir_all(config_dir)?;
        let lock = acquire_core_cli_keychain_lock()?;
        let mut command = self.new_app_creation_command(config_dir, brand);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_background_command(&mut command);
        let child = command.spawn()?;
        release_cli_keychain_lock_when_process_exits(child.id(), lock);
        Ok(child)
    }

    fn run_json<T, I, S>(&self, config_dir: &Path, args: I) -> Result<CliJson<T>>
    where
        T: DeserializeOwned,
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.run_capture(Some(config_dir), args, None, self.timeout)?;
        parse_json_output(output)
    }

    fn run_json_owned<T: DeserializeOwned>(
        &self,
        config_dir: &Path,
        args: Vec<String>,
        timeout: Duration,
    ) -> Result<CliJson<T>> {
        let output = self.run_capture(Some(config_dir), args, None, timeout)?;
        parse_json_output(output)
    }

    pub fn run_capture<I, S>(
        &self,
        config_dir: Option<&Path>,
        args: I,
        stdin_secret: Option<&SecretString>,
        timeout: Duration,
    ) -> Result<ProcessOutput>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_capture_internal(config_dir, None, args, stdin_secret, timeout)
    }

    fn run_capture_internal<I, S>(
        &self,
        config_dir: Option<&Path>,
        current_dir: Option<&Path>,
        args: I,
        stdin_secret: Option<&SecretString>,
        timeout: Duration,
    ) -> Result<ProcessOutput>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        if !self.executable.is_file() {
            return Err(LpcError::RuntimeMissing(self.executable.clone()));
        }
        let _lock = acquire_core_cli_keychain_lock()?;
        let mut command = Command::new(&self.executable);
        command.args(args);
        if let Some(current_dir) = current_dir {
            command.current_dir(current_dir);
        }
        if let Some(config_dir) = config_dir {
            fs::create_dir_all(config_dir)?;
            command.env("LARKSUITE_CLI_CONFIG_DIR", config_dir);
        }
        command.stdin(if stdin_secret.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        configure_background_command(&mut command);

        let mut child = command.spawn()?;
        if let (Some(secret), Some(mut stdin)) = (stdin_secret, child.stdin.take()) {
            stdin.write_all(secret.expose_bytes())?;
            stdin.write_all(b"\n")?;
            stdin.flush()?;
        }

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| LpcError::Internal("stdout pipe unavailable".into()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| LpcError::Internal("stderr pipe unavailable".into()))?;
        let stdout_thread = thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.read_to_end(&mut bytes);
            bytes
        });
        let stderr_thread = thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes);
            bytes
        });

        let status = match child.wait_timeout(timeout)? {
            Some(status) => status,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(LpcError::CliTimeout(timeout.as_secs()));
            }
        };
        let stdout = stdout_thread
            .join()
            .map_err(|_| LpcError::Internal("stdout reader panicked".into()))?;
        let stderr = stderr_thread
            .join()
            .map_err(|_| LpcError::Internal("stderr reader panicked".into()))?;
        Ok(ProcessOutput {
            status,
            stdout,
            stderr,
        })
    }
}

fn acquire_core_cli_keychain_lock() -> Result<CliKeychainGuard> {
    #[cfg(test)]
    {
        // In-crate tests use ephemeral stores while discover() may point at the
        // developer's persistent data root; real lock behavior is tested in locking.rs.
        Ok(CliKeychainGuard::noop_for_tests())
    }

    #[cfg(not(test))]
    {
        let paths = AppPaths::discover()?;
        try_acquire_cli_keychain_lock(&paths, Duration::from_secs(5))?
            .ok_or(LpcError::CliKeychainBusy)
    }
}

fn background_creation_flags() -> u32 {
    #[cfg(windows)]
    {
        windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
    }

    #[cfg(not(windows))]
    {
        0
    }
}

fn configure_background_command(command: &mut Command) {
    let creation_flags = background_creation_flags();

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(creation_flags);
    }

    #[cfg(not(windows))]
    let _ = (command, creation_flags);
}

fn read_validated_png_and_remove(path: &Path) -> Result<Vec<u8>> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

    let read_result = fs::read(path);
    let remove_result = fs::remove_file(path);
    let bytes = read_result?;
    remove_result?;
    if !bytes.starts_with(PNG_SIGNATURE) {
        return Err(LpcError::InvalidCliOutput(
            "QR output is not a PNG image".into(),
        ));
    }
    Ok(bytes)
}

fn parse_json_output<T: DeserializeOwned>(output: ProcessOutput) -> Result<CliJson<T>> {
    ensure_success(&output)?;
    let value = serde_json::from_slice::<T>(&output.stdout).map_err(|error| {
        let sample = redact_text(&String::from_utf8_lossy(&output.stdout));
        LpcError::InvalidCliOutput(format!("{error}; stdout={sample}"))
    })?;
    Ok(CliJson {
        value,
        stderr: redact_text(&String::from_utf8_lossy(&output.stderr)),
    })
}

fn ensure_success(output: &ProcessOutput) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let code = output.status.code().unwrap_or(1);
    let stderr = redact_text(&String::from_utf8_lossy(&output.stderr));
    let stdout = redact_text(&String::from_utf8_lossy(&output.stdout));
    let message = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    Err(LpcError::CliFailed {
        code,
        message: message.trim().to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_string_and_array_scopes() {
        let one: IdentityStatus = serde_json::from_str(r#"{"scope":"a b c"}"#).unwrap();
        assert_eq!(one.scope.len(), 3);
        let two: IdentityStatus = serde_json::from_str(r#"{"scope":["a","b"]}"#).unwrap();
        assert_eq!(two.scope.len(), 2);
    }

    #[test]
    fn reads_valid_png_and_removes_temporary_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("oauth.png");
        let expected = b"\x89PNG\r\n\x1a\nfixture".to_vec();
        std::fs::write(&path, &expected).unwrap();

        assert_eq!(read_validated_png_and_remove(&path).unwrap(), expected);
        assert!(!path.exists());
    }

    #[test]
    fn rejects_non_png_and_still_removes_temporary_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("oauth.png");
        std::fs::write(&path, b"not a png").unwrap();

        assert!(read_validated_png_and_remove(&path).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn background_cli_child_uses_platform_creation_flags() {
        #[cfg(windows)]
        assert_eq!(
            background_creation_flags(),
            windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
        );

        #[cfg(not(windows))]
        assert_eq!(background_creation_flags(), 0);
    }

    #[test]
    fn app_creation_uses_direct_managed_cli_argv_and_keeps_label_outside_process_args() {
        let managed_cli = PathBuf::from(r"C:\managed runtime\lark-cli.exe");
        let staging = PathBuf::from(r"C:\lpc data\staging\flow");
        let display_label = "Team App &|%^' with spaces";
        let cli = OfficialCli::new(&managed_cli);

        let command = cli.new_app_creation_command(&staging, Brand::Lark);
        let args = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(command.get_program(), managed_cli.as_os_str());
        assert_eq!(args, ["config", "init", "--new", "--brand", "lark"]);
        assert!(!args.iter().any(|arg| arg.contains(display_label)));
        assert!(!command
            .get_program()
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("cmd.exe"));
        assert!(!command
            .get_program()
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains("powershell"));
    }
}
