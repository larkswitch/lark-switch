//! Deployment contract for the desktop binary (AGENTS.md build rules).
//!
//! `cargo build --release` produces a desktop executable that still points at
//! `devUrl` (`http://localhost:1420`), because the `custom-protocol` feature
//! that switches Tauri to embedded assets is injected by the Tauri CLI rather
//! than declared in `Cargo.toml`. Such a binary launches and then shows
//! ERR_CONNECTION_REFUSED once deployed. That is the 2026-07-22 incident.
//!
//! Checking that the binary does *not* contain the devUrl literal cannot detect
//! this: `http://localhost:1420` comes from the embedded `tauri.conf.json` and
//! is present in good and bad binaries alike. The usable signal is the opposite
//! one — Tauri stores embedded resource keys as plain-text paths and only
//! brotli-compresses the contents, so a correctly packaged binary contains the
//! hashed asset file names verbatim and a `cargo`-built one contains none.

mod common;

/// Set to any value to turn "no release binary present" from skip into failure.
/// Intended for the release workflow, so the check cannot silently pass.
const REQUIRE_ARTIFACT_ENV: &str = "LPC_REQUIRE_DEPLOY_ARTIFACT";

#[test]
fn release_desktop_binary_embeds_current_frontend_bundle() {
    let root = common::repo_root();
    let binary = root.join("target").join("release").join(binary_file_name());

    if !binary.exists() {
        assert!(
            std::env::var_os(REQUIRE_ARTIFACT_ENV).is_none(),
            "{REQUIRE_ARTIFACT_ENV} is set but {} does not exist. Build it with \
             `npx tauri build --no-bundle` from apps/desktop.",
            common::relative(&binary, &root)
        );
        eprintln!(
            "skip: {} not built; run `npx tauri build --no-bundle` in apps/desktop to check it",
            common::relative(&binary, &root)
        );
        return;
    }

    let assets_dir = root.join("apps/desktop/dist/assets");
    let bundle_names = bundle_file_names(&assets_dir);
    assert!(
        !bundle_names.is_empty(),
        "no .js/.css found under {}. The frontend bundle must be built \
         (`pnpm run build` in apps/desktop) before this contract means anything; \
         an empty dist also produces a blank-screen desktop binary.",
        common::relative(&assets_dir, &root)
    );

    let bytes = std::fs::read(&binary).expect("read desktop release binary");
    let missing: Vec<&String> = bundle_names
        .iter()
        .filter(|name| !contains(&bytes, name.as_bytes()))
        .collect();

    assert!(
        missing.is_empty(),
        "{} does not embed the current frontend bundle; missing {:?}.\n\
         This is the signature of a binary produced by `cargo build --release`, \
         which falls back to devUrl and shows ERR_CONNECTION_REFUSED after deployment.\n\
         Rebuild with `npx tauri build --no-bundle` from apps/desktop (AGENTS.md).",
        common::relative(&binary, &root),
        missing
    );
}

/// The contract above assumes Tauri embeds `apps/desktop/dist`. Fail loudly if
/// that wiring is ever changed, instead of silently checking the wrong folder.
#[test]
fn tauri_config_still_embeds_the_scanned_dist_directory() {
    let root = common::repo_root();
    let config_path = root.join("apps/desktop/src-tauri/tauri.conf.json");
    let config = std::fs::read_to_string(&config_path).expect("read tauri.conf.json");

    assert!(
        config.contains("\"frontendDist\": \"../dist\""),
        "{} no longer sets frontendDist to \"../dist\"; \
         update deploy_contract.rs to scan the new bundle directory.",
        common::relative(&config_path, &root)
    );
}

fn binary_file_name() -> &'static str {
    if cfg!(windows) {
        "lark-profile-console.exe"
    } else {
        "lark-profile-console"
    }
}

/// Hashed bundle file names, e.g. `index-BaydJH4i.js`. Vite regenerates the
/// hash on every content change, so a stale binary fails the containment check.
fn bundle_file_names(assets_dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = ["js", "css"]
        .iter()
        .flat_map(|extension| common::collect_files(assets_dir, extension))
        .filter_map(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .collect();
    names.sort();
    names
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
