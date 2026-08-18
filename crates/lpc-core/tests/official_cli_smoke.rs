//! Real integration smoke against an official larksuite/cli release.
//!
//! Run with:
//! `cargo test -p lpc-core --test official_cli_smoke -- --ignored --nocapture`
//! It performs network I/O and stores only a random, non-working test secret
//! through the official CLI keychain adapter. No real credential is needed.

use lpc_core::{AppPaths, OfficialCli, RuntimeManager, SecretString, StateStore};
use uuid::Uuid;

#[test]
#[ignore = "requires an installed official CLI executable"]
fn official_cli_qrcode_is_a_clean_png() {
    let executable = std::env::var_os("LPC_TEST_OFFICIAL_CLI")
        .expect("LPC_TEST_OFFICIAL_CLI must point to the managed official CLI");
    let temp = tempfile::tempdir().unwrap();
    let cli = OfficialCli::new(executable);

    let png = cli
        .render_qrcode_png(temp.path(), "https://example.com/device")
        .unwrap();

    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert!(!std::fs::read_dir(temp.path()).unwrap().any(|entry| {
        entry
            .ok()
            .and_then(|entry| entry.path().extension().map(|value| value == "png"))
            .unwrap_or(false)
    }));
}

#[test]
#[ignore = "downloads and executes the official CLI release"]
fn official_cli_binary_and_config_directory_isolation_are_real() {
    let temp = tempfile::tempdir().unwrap();
    let store = StateStore::new(AppPaths::new(temp.path().join("lpc")));
    store.initialize().unwrap();
    let runtime = RuntimeManager::new(store.clone()).unwrap();
    let executable = runtime.install(lpc_core::SUPPORTED_CLI_VERSION).unwrap();
    let cli = OfficialCli::new(executable);
    cli.compatibility_check().unwrap();

    // The same App ID is deliberately configured in two independent official
    // CLI config roots. `profile add` would reject the duplicate inside one
    // root, but the environment override isolates them as required by LPC.
    let app_id = format!("cli_lpc_fixture_{}", Uuid::new_v4().simple());
    for name in ["one", "two"] {
        let dir = temp.path().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let secret = SecretString::new("not-a-real-app-secret");
        let output = cli
            .run_capture(
                Some(&dir),
                [
                    "profile",
                    "add",
                    "--name",
                    name,
                    "--app-id",
                    &app_id,
                    "--app-secret-stdin",
                    "--brand",
                    "feishu",
                ],
                Some(&secret),
                std::time::Duration::from_secs(30),
            )
            .unwrap();
        assert!(
            output.status.success(),
            "profile add stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = cli
            .run_capture(
                Some(&dir),
                ["profile", "list"],
                None,
                std::time::Duration::from_secs(20),
            )
            .unwrap();
        assert!(output.status.success());
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(text.contains(name));
        assert!(text.contains(&app_id));
    }
}
