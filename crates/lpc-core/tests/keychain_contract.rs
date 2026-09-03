//! Keychain red lines from AGENTS.md, enforced as assertions.
//!
//! Feishu rotates refresh tokens: using one invalidates it immediately. On
//! 2026-07-22 a whole-hive registry import replayed an old snapshot over
//! accounts that were still healthy, the official CLI refreshed with the now
//! dead token, the server answered 20064 / invalid_grant, and the CLI deleted
//! those credentials. A cascade of revocations from one "restore".
//!
//! Hence three rules, all about `HKCU\Software\LarkCli\keychain`:
//!
//! 1. Never replay a whole hive — restore one value at a time.
//! 2. Export a `.reg` snapshot before any write.
//! 3. Never delete or recreate the key itself; single values only.

mod common;

use std::path::PathBuf;

/// Rust sources that could reach the registry.
const SCANNED_SOURCE_DIRS: &[&str] = &[
    "crates/lpc-core/src",
    "crates/lpcctl/src",
    "crates/lpc-shim/src",
    "apps/desktop/src-tauri/src",
];

/// Operator and incident-response scripts.
const SCANNED_SCRIPT_DIRS: &[&str] = &["scripts", "data"];

/// Registry key that holds official CLI credentials, lowercased for matching.
const KEYCHAIN_KEY: &str = r"larkcli\keychain";

/// Commands that replay an entire exported hive in one shot.
const WHOLE_HIVE_IMPORT: &[&str] = &[
    "reg import",
    "reg.exe import",
    "reg restore",
    "reg.exe restore",
    "regedit /s",
    "regedit.exe /s",
    "regedit /i",
    "import-registryfile",
];

/// Anything that mutates the keychain and therefore needs a prior snapshot.
const REGISTRY_WRITES: &[&str] = &[
    "set-itemproperty",
    "new-itemproperty",
    "remove-itemproperty",
    "reg add",
    "reg.exe add",
    "reg delete",
    "reg.exe delete",
];

/// Ways to capture a `.reg` snapshot before writing.
const SNAPSHOT_EXPORTS: &[&str] = &["reg export", "reg.exe export", "backup_keychain_registry"];

#[test]
fn keychain_is_never_restored_by_whole_hive_import() {
    let root = common::repo_root();
    let mut violations = Vec::new();

    for path in scanned_files(&root) {
        let relative = common::relative(&path, &root);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        for (offset, line) in text.lines().enumerate() {
            let lowered = line.to_ascii_lowercase();
            for pattern in WHOLE_HIVE_IMPORT {
                if lowered.contains(pattern) {
                    violations.push(format!("  {relative}:{} — `{pattern}`", offset + 1));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "whole-hive registry replay found:\n{}\n\n\
         Replaying a snapshot overwrites healthy accounts with already-rotated \
         refresh tokens, which Feishu rejects with 20064 and the official CLI \
         answers by deleting the credential (see AGENTS.md and \
         docs/TOKEN-EXPIRY-INVESTIGATION-2026-07-22.md). \
         Restore one named value at a time instead.",
        violations.join("\n")
    );
}

#[test]
fn keychain_writes_are_preceded_by_a_snapshot_export() {
    let root = common::repo_root();
    let mut violations = Vec::new();

    for path in scanned_script_files(&root) {
        let relative = common::relative(&path, &root);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        if !text.to_ascii_lowercase().contains(KEYCHAIN_KEY) {
            continue;
        }

        let Some(first_write) = first_line_matching(&text, REGISTRY_WRITES) else {
            continue;
        };
        let first_export = first_line_matching(&text, SNAPSHOT_EXPORTS);

        match first_export {
            Some(export) if export < first_write => {}
            Some(export) => violations.push(format!(
                "  {relative}: writes the keychain at line {first_write} but only \
                 exports a snapshot later, at line {export}"
            )),
            None => violations.push(format!(
                "  {relative}:{first_write} writes the keychain with no `.reg` snapshot anywhere"
            )),
        }
    }

    assert!(
        violations.is_empty(),
        "keychain writes without a preceding snapshot:\n{}\n\n\
         AGENTS.md requires exporting a `.reg` snapshot to \
         %USERPROFILE%\\Documents\\LarkProfileConsoleBackups\\keychain\\ before \
         touching the key, so a bad write can always be undone.",
        violations.join("\n")
    );
}

#[test]
fn keychain_key_itself_is_never_deleted_or_recreated() {
    let root = common::repo_root();
    let mut violations = Vec::new();

    for path in scanned_files(&root) {
        let relative = common::relative(&path, &root);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        for (offset, line) in text.lines().enumerate() {
            let lowered = line.to_ascii_lowercase();
            if !lowered.contains(KEYCHAIN_KEY) {
                continue;
            }
            // Single-value operations end in `-itemproperty` and stay allowed.
            let key_level = lowered
                .replace("remove-itemproperty", "")
                .replace("new-itemproperty", "");
            for pattern in ["remove-item", "new-item"] {
                if key_level.contains(pattern) {
                    violations.push(format!("  {relative}:{} — `{pattern}`", offset + 1));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "key-level delete/create on the credential key:\n{}\n\n\
         Wiping the key before a restore is how the hive ended up empty on \
         2026-07-17 (docs/KEYCHAIN-DURABILITY.md). Operate on single values \
         with Remove-ItemProperty / Set-ItemProperty.",
        violations.join("\n")
    );
}

#[test]
fn shim_never_fails_open_when_the_keychain_lock_is_busy() {
    let root = common::repo_root();
    let shim = std::fs::read_to_string(root.join("crates/lpc-shim/src/main.rs"))
        .expect("read lpc shim source");

    assert!(
        shim.contains(".ok_or(LpcError::CliKeychainBusy)?"),
        "the shim must reject a command when the shared keychain lock times out"
    );
    assert!(
        !shim.contains("仍将执行命令"),
        "the shim must never continue without the shared keychain lock"
    );
}

#[test]
fn desktop_blocks_msix_before_touching_autostart_or_credentials() {
    let root = common::repo_root();
    let desktop = std::fs::read_to_string(root.join("apps/desktop/src-tauri/src/main.rs"))
        .expect("read desktop source");
    let guard = desktop
        .find("enforce_msix_shim_policy()")
        .expect("desktop must enforce the MSIX credential policy");
    let autostart = desktop
        .find("ensure_installed_autostart(app.handle())")
        .expect("desktop startup marker");
    let backup = desktop
        .find("run_credential_backup(&paths, \"startup\")")
        .expect("credential backup marker");

    assert!(
        guard < autostart && guard < backup,
        "MSIX guard must run before startup writes"
    );
}

#[test]
fn shim_blocks_a_shadow_registry_before_running_the_official_cli() {
    let root = common::repo_root();
    let shim = std::fs::read_to_string(root.join("crates/lpc-shim/src/main.rs"))
        .expect("read lpc shim source");
    let view_guard = shim
        .find("enforce_host_keychain_view(&paths)?")
        .expect("shim must verify the host registry view");
    let managed_launch = shim
        .find("Command::new(&managed)")
        .expect("official CLI launch marker");
    assert!(
        view_guard < managed_launch,
        "registry view guard must run before the official CLI"
    );
}

#[test]
fn desktop_establishes_host_view_before_backing_up_or_inspecting_credentials() {
    let root = common::repo_root();
    let desktop = std::fs::read_to_string(root.join("apps/desktop/src-tauri/src/main.rs"))
        .expect("read desktop source");
    let guard = desktop
        .find("ensure_host_keychain_view(&paths)")
        .expect("desktop must establish the host registry view");
    let backup = desktop
        .find("run_credential_backup(&paths, \"startup\")")
        .expect("credential backup marker");
    let inspect = desktop
        .find("lpc_core::inspect_keychain()")
        .expect("credential inspection marker");
    assert!(
        guard < backup && guard < inspect,
        "host view must be established before credential access"
    );
}

fn scanned_files(root: &std::path::Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = SCANNED_SOURCE_DIRS
        .iter()
        .flat_map(|directory| common::collect_files(&root.join(directory), "rs"))
        .collect();
    files.extend(scanned_script_files(root));
    files
}

fn scanned_script_files(root: &std::path::Path) -> Vec<PathBuf> {
    SCANNED_SCRIPT_DIRS
        .iter()
        .flat_map(|directory| common::collect_files(&root.join(directory), "ps1"))
        .collect()
}

/// 1-based line number of the first line containing any of `patterns`.
fn first_line_matching(text: &str, patterns: &[&str]) -> Option<usize> {
    text.lines().enumerate().find_map(|(offset, line)| {
        let lowered = line.to_ascii_lowercase();
        patterns
            .iter()
            .any(|pattern| lowered.contains(pattern))
            .then_some(offset + 1)
    })
}
