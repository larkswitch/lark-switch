//! Startup work must run before WebView creation, including duplicate launches.
use lpc_core::{atomic::write_json_atomic, AppPaths, RoutingGate, SingletonLock};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::Manager;
use uuid::Uuid;

#[derive(Serialize, Deserialize)]
struct Activation {
    id: Uuid,
    requested_at: u64,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn request_activation(paths: &AppPaths) -> lpc_core::Result<()> {
    write_json_atomic(
        &paths.root().join("desktop-activation.json"),
        &Activation {
            id: Uuid::new_v4(),
            requested_at: now(),
        },
    )
}

/// A file lock, rather than a PID file, distinguishes a live owner from a crash.
pub fn activate_if_running(paths: &AppPaths, hidden: bool) -> lpc_core::Result<bool> {
    if RoutingGate::new(paths.clone())
        .try_acquire_singleton("desktop-instance")?
        .is_some()
    {
        return Ok(false);
    }
    if !hidden {
        request_activation(paths)?;
    }
    Ok(true)
}

pub fn listen_for_activation(app: tauri::AppHandle, paths: AppPaths) {
    std::thread::spawn(move || {
        let mut last = None;
        loop {
            if let Ok(bytes) = std::fs::read(paths.root().join("desktop-activation.json")) {
                if let Ok(request) = serde_json::from_slice::<Activation>(&bytes) {
                    if last != Some(request.id) && now().saturating_sub(request.requested_at) < 60 {
                        last = Some(request.id);
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.unminimize();
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    });
}

#[cfg(any(windows, test))]
fn is_our_webview(name: &str, args: &[std::ffi::OsString], profile: &Path) -> bool {
    name.eq_ignore_ascii_case("msedgewebview2.exe")
        && args.iter().any(|arg| {
            arg.to_str()
                .and_then(|arg| arg.strip_prefix("--user-data-dir="))
                .is_some_and(|value| Path::new(value.trim_matches('"')).eq(profile))
        })
}

/// Only called with the desktop singleton held and before creating any WebView.
/// This profile belongs to this application; never kill other WebView clients.
pub fn clean_orphan_webviews(profile: &Path, _owner: &SingletonLock) {
    #[cfg(windows)]
    {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            ProcessRefreshKind::new().with_cmd(UpdateKind::Always),
        );
        let mut stopped = 0;
        for process in system.processes().values() {
            if is_our_webview(&process.name().to_string_lossy(), process.cmd(), profile)
                && process.kill()
            {
                stopped += 1;
            }
        }
        if stopped > 0 {
            tracing::warn!(stopped, "orphan desktop webviews stopped before startup");
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    #[cfg(not(windows))]
    let _ = profile;
}

/// A stuck WebView must report an error instead of retaining an invisible host.
/// Arm this only around window creation, before starting credential operations.
pub fn watch_window_creation() -> std::sync::mpsc::Sender<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        if matches!(
            rx.recv_timeout(Duration::from_secs(30)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ) {
            tracing::error!("desktop WebView initialization timed out");
            lpc_core::show_blocking_message("larkswitch — 启动超时", "界面组件未能在 30 秒内启动。请检查安全软件的拦截记录，关闭此提示后重新打开 larkswitch。");
            std::process::exit(1);
        }
    });
    tx
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_live_owner_receives_activation_and_hidden_launch_does_not_show() {
        let dir = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(dir.path().to_owned());
        assert!(!activate_if_running(&paths, false).unwrap());
        let owner = RoutingGate::new(paths.clone())
            .try_acquire_singleton("desktop-instance")
            .unwrap()
            .unwrap();
        assert!(activate_if_running(&paths, true).unwrap());
        assert!(!paths.root().join("desktop-activation.json").exists());
        assert!(activate_if_running(&paths, false).unwrap());
        let request: Activation = serde_json::from_slice(
            &std::fs::read(paths.root().join("desktop-activation.json")).unwrap(),
        )
        .unwrap();
        assert!(now().saturating_sub(request.requested_at) < 5);
        drop(owner);
        assert!(!activate_if_running(&paths, false).unwrap());
    }
    #[test]
    fn cleanup_excludes_other_apps_and_similar_profile_names() {
        let profile = Path::new("C:/Local/dev.larkswitch.desktop/EBWebView");
        let args = vec!["--user-data-dir=C:/Local/dev.larkswitch.desktop/EBWebView".into()];
        assert!(is_our_webview("msedgewebview2.exe", &args, profile));
        assert!(!is_our_webview("other.exe", &args, profile));
        assert!(!is_our_webview(
            "msedgewebview2.exe",
            &["--user-data-dir=C:/Local/dev.larkswitch.desktop/EBWebView-other".into()],
            profile
        ));
        assert!(!is_our_webview(
            "msedgewebview2.exe",
            &["--user-data-dir=C:/Local/OtherApp/EBWebView".into()],
            profile
        ));
    }
}
