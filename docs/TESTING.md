# Testing and Release Gates

## Deterministic local suite

`./scripts/verify.sh` runs:

- formatting,
- Clippy with warnings denied for core/shim/control CLI,
- unit and integration tests,
- frontend TypeScript/Vite production build.

Coverage includes:

- atomic JSON replacement,
- live boundary enforcement,
- deterministic batching,
- official config sanitization,
- secret redaction,
- PATH merge/removal,
- runtime asset/checksum parsing,
- execution leases,
- switching while a command retains the old snapshot,
- account/App snapshot joining.

## Static contract tests

`cargo test -p lpc-core` also runs four text-level contracts. They never execute product code: they read repository sources, scripts and documents as text and assert the hard rules from `AGENTS.md`. The CI matrix runs them on all three platforms.

| Test | What it guards |
| --- | --- |
| `tests/deploy_contract.rs` | The release desktop binary must embed the current `apps/desktop/dist/assets` bundle, and the packaged `lark-cli` sidecar must report the same version as the desktop. This prevents both the devUrl blank-screen incident and a stale sidecar silently downgrading the managed shim during startup self-repair. Desktop packaging must build sidecars first and then use `npx tauri build --no-bundle`; never ship a plain `cargo build --release` desktop binary. |
| `tests/atomic_write_contract.rs` | Control-plane state may only reach disk through `lpc_core::atomic`; raw `fs::write` / `fs::copy` / `File::create` are rejected outside `atomic.rs`. |
| `tests/keychain_contract.rs` | No whole-hive registry replay, a `.reg` snapshot before any keychain write, and never deleting or recreating the credential key itself. |
| `tests/docs_contract.rs` | Documentation may not drift from the implementation: every documented `lpcctl` subcommand and flag must exist in the clap definition, load-bearing identifiers must appear on both the document and the code side, and retired or dangerous restore advice must stay deleted. |

The deploy contract skips when no release binary is present; set `LPC_REQUIRE_DEPLOY_ARTIFACT=1` in the release pipeline to turn that skip into a failure. A write that lands in a scratch directory and is published by a single later rename is legitimate and opts out with a `// lpc-allow-raw-write: <reason>` marker on the line or the line above it.

## Real official CLI smoke

The ignored `official_cli_smoke` test downloads the official release, verifies checksum, executes compatibility checks, and proves two independent configuration directories with the same App ID are selected through `LARKSUITE_CLI_CONFIG_DIR`. It needs no credentials and runs in CI on Windows and macOS.

## Real OAuth E2E

OAuth cannot be responsibly replaced by a fake service for the final gate. `scripts/real-oauth-e2e.sh` uses a disposable real App and two real users. A human completes official browser consent.

Required assertions:

1. Import/create App through official CLI.
2. Add user A and user B under the same App.
3. Restart desktop; both still pass `whoami` and a real read-only API.
4. Start a long command as A, switch to B, verify old process remains A and new process is B.
5. Let access tokens expire; verify each account refreshes independently.
6. Revoke A; B remains usable.
7. Add a second App and account; switch across Apps.
8. Use a large policy requiring multiple serial batches; final missing set is empty.
9. Select the wrong user during reauthorization; expected account metadata is not silently reassigned.
10. Remove A; B and shared App configuration remain usable.

## Cross-platform matrix

- Windows 11 x64, clean user, no existing CLI.
- Windows 10/11 x64, existing npm/global CLI.
- macOS Intel.
- macOS Apple Silicon.
- zsh and bash startup-file takeover.
- New terminal PATH route and uninstall restoration.

## Release decision

A release is blocked by any of:

- secret scanner finding product-state/log/diagnostic credentials,
- identity mismatch between account metadata, `whoami`, and a real API,
- command identity changing after startup,
- any requested scope outside live boundary,
- final missing scopes non-empty,
- updater activating an incompatible runtime,
- uninstall deleting unrelated PATH/shell configuration,
- unsigned/unnotarized public production artifacts when signing is required.
