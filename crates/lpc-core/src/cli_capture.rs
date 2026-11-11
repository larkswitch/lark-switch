//! A caller deadline is not permission to interrupt a credential writer.
use crate::cli::ProcessOutput;
use crate::error::{LpcError, Result};
use crate::locking::CliKeychainGuard;
use std::process::Child;
use std::thread::JoinHandle;
use std::time::Duration;
use wait_timeout::ChildExt;

fn join_reader(reader: JoinHandle<Vec<u8>>) -> Result<Vec<u8>> {
    reader
        .join()
        .map_err(|_| LpcError::Internal("CLI output reader panicked".into()))
}

pub(crate) fn wait_capture(
    mut child: Child,
    stdout: JoinHandle<Vec<u8>>,
    stderr: JoinHandle<Vec<u8>>,
    _keychain_guard: CliKeychainGuard,
    credential_capable: bool,
    timeout: Duration,
) -> Result<ProcessOutput> {
    let status = match child.wait_timeout(timeout)? {
        Some(status) => status,
        None if credential_capable => {
            let pid = child.id();
            tracing::warn!(
                pid,
                "CLI deadline reached; waiting for credential writer to finish"
            );
            // Keep synchronous ownership: even a short-lived lpcctl must not exit
            // and release its keychain lock before the writer has finished.
            let status = child.wait()?;
            tracing::info!(
                pid,
                exit_code = status.code(),
                "credential writer finished after deadline"
            );
            status
        }
        None => {
            // No account config: version/help/QR utility cannot refresh user tokens.
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout.join();
            let _ = stderr.join();
            return Err(LpcError::CliTimeout(timeout.as_secs()));
        }
    };
    // Return the real result, including late diagnostics, for the normal parser.
    // Never log stdout: it may contain credentials.
    Ok(ProcessOutput {
        status,
        stdout: join_reader(stdout)?,
        stderr: join_reader(stderr)?,
    })
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::thread;

    #[test]
    fn caller_cannot_return_or_release_guard_before_delayed_writer_finishes() {
        let temp = tempfile::tempdir().unwrap();
        let mut command = Command::new("powershell.exe");
        command.current_dir(temp.path()).args([
            "-NoProfile", "-NonInteractive", "-Command",
            "Start-Sleep -Milliseconds 1200; [IO.File]::WriteAllText('saved.txt', 'new credential fixture'); [Console]::WriteLine('secret stdout fixture'); exit 7",
        ]).stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
        let mut child = command.spawn().unwrap();
        let read = |mut pipe: Box<dyn Read + Send>| {
            thread::spawn(move || {
                let mut bytes = Vec::new();
                pipe.read_to_end(&mut bytes).unwrap();
                bytes
            })
        };
        let stdout = read(Box::new(child.stdout.take().unwrap()));
        let stderr = read(Box::new(child.stderr.take().unwrap()));
        let paths = crate::paths::AppPaths::new(temp.path());
        let guard = crate::locking::try_acquire_shared_cli_keychain_lock(&paths, Duration::ZERO)
            .unwrap()
            .unwrap();
        let caller = thread::spawn(move || {
            wait_capture(
                child,
                stdout,
                stderr,
                guard,
                true,
                Duration::from_millis(50),
            )
        });
        thread::sleep(Duration::from_millis(150));
        assert!(!caller.is_finished());
        assert!(
            crate::locking::try_acquire_cli_keychain_lock(&paths, Duration::ZERO)
                .unwrap()
                .is_none()
        );
        let output = caller.join().unwrap().unwrap();
        assert_eq!(output.status.code(), Some(7));
        assert!(String::from_utf8_lossy(&output.stdout).contains("secret stdout fixture"));
        assert_eq!(
            std::fs::read_to_string(temp.path().join("saved.txt")).unwrap(),
            "new credential fixture"
        );
        assert!(
            crate::locking::try_acquire_cli_keychain_lock(&paths, Duration::ZERO)
                .unwrap()
                .is_some()
        );
    }
}
