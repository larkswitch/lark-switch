//! Runs the real shim through the real bridge with the host check failing, in a
//! private data root. Every shim then wants to forward again, which once started
//! thousands of processes. It stops before routing, so it never reaches a credential.
#![cfg(windows)]

mod common;

use lpc_core::{start_host_bridge, AppPaths};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

const COMMANDS: usize = 20;
/// Each command may hold its outer shim plus one shim per allowed hop.
const MAX_PROCESSES: usize = COMMANDS * 4;

fn running_copies(system: &mut System, shim: &std::path::Path) -> Vec<sysinfo::Pid> {
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        ProcessRefreshKind::new().with_exe(UpdateKind::Always),
    );
    system
        .processes()
        .iter()
        .filter(|(_, process)| process.exe().is_some_and(|exe| same_path(exe, shim)))
        .map(|(pid, _)| *pid)
        .collect()
}

/// Path spellings differ (`\?\`, short names), but the temp dir name is unique.
fn same_path(left: &std::path::Path, right: &std::path::Path) -> bool {
    let unique = right
        .ancestors()
        .find(|dir| {
            dir.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".tmp"))
        })
        .and_then(|dir| dir.file_name())
        .map(|name| name.to_string_lossy().to_lowercase());
    let left = left.to_string_lossy().to_lowercase();
    left.ends_with("lark-cli.exe") && unique.is_some_and(|unique| left.contains(&unique))
}

fn stop_all(system: &mut System, shim: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let running = running_copies(system, shim);
        if running.is_empty() {
            return;
        }
        for pid in running {
            if let Some(process) = system.process(pid) {
                process.kill();
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_failing_host_check_cannot_multiply_processes() {
    let release_shim = common::repo_root().join("target/release/lark-cli.exe");
    if !release_shim.exists() {
        assert!(
            std::env::var_os("LPC_REQUIRE_DEPLOY_ARTIFACT").is_none(),
            "the release shim is required"
        );
        eprintln!("skip: release shim not built");
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("lpc");
    let paths = AppPaths::new(&root);
    paths.ensure_layout().unwrap();
    let shim = paths.bin_dir().join("lark-cli.exe");
    std::fs::copy(&release_shim, &shim).unwrap();
    // No real registry view can match this marker, so every shim forwards.
    std::fs::write(
        paths.host_keychain_view_file(),
        r#"{"version":1,"marker":"00000000-0000-4000-8000-000000000001"}"#,
    )
    .unwrap();
    // Windows stops this test's whole process tree at this size, so even a
    // regression cannot run away on the machine.
    std::env::set_var("LPC_HOST_PROCESS_LIMIT", "100");
    start_host_bridge(paths.clone()).unwrap();

    let commands: Vec<_> = (0..COMMANDS)
        .map(|_| {
            Command::new(&shim)
                .args(["auth", "status", "--json"])
                .env("LPC_HOME", &root)
                .env_remove("LPC_HOST_EXECUTOR_PID")
                .env_remove("LPC_HOST_BRIDGE_HOPS")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    let waiters: Vec<_> = commands
        .into_iter()
        .map(|child| std::thread::spawn(move || child.wait_with_output().unwrap()))
        .collect();

    let mut system = System::new();
    assert!(
        !running_copies(&mut system, &shim).is_empty(),
        "the process count must see the commands it just started"
    );
    let started = Instant::now();
    let mut peak = 0;
    while !waiters.iter().all(|waiter| waiter.is_finished()) {
        peak = peak.max(running_copies(&mut system, &shim).len());
        if peak > MAX_PROCESSES || started.elapsed() > Duration::from_secs(60) {
            stop_all(&mut system, &shim);
            panic!(
                "forwarding storm: peak {peak} processes after {:?}",
                started.elapsed()
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    for waiter in waiters {
        let output = waiter.join().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(70), "{stderr}");
        assert!(stderr.contains("LPC_HOST_BRIDGE_LOOP"), "{stderr}");
        assert!(stderr.contains("LPC_HOST_CHECK"), "{stderr}");
    }
    std::thread::sleep(Duration::from_secs(1));
    assert!(running_copies(&mut system, &shim).is_empty());
    eprintln!("peak {peak} processes, finished in {:?}", started.elapsed());
}
