# Architecture

## Why isolated configuration directories

Official profiles are App profiles. They cannot model multiple users under one App ID. The official CLI, however, exposes a supported configuration-directory override. Lark Profile Console therefore launches the official binary with:

```text
LARKSUITE_CLI_CONFIG_DIR=<account.config_dir>
```

Each account config contains one App and one selected user. App Secret and UAT values remain in the official CLI's OS keychain.

## Filesystem

```text
LPC_HOME/
├─ apps/<app-uuid>/config.json          sanitized App base config, users=[]
├─ accounts/<account-uuid>/config.json  official metadata for one App/User
├─ data/catalog.json                    non-sensitive App/account/policy metadata
├─ data/active-state.json               active account + managed CLI pointer
├─ runtime/versions/<version>/lark-cli  immutable official binary
├─ bin/lark-cli                         product Shim
├─ locks/routing.lock
├─ locks/runtime.lock
├─ locks/executions/<lease>.json
└─ staging/                             ephemeral OAuth/config flows
```

Every JSON write uses a same-directory temporary file, flush/fsync, then atomic replacement.

## Routing linearization

Command startup and account switching share one routing gate.

### Command startup

1. Lock routing gate.
2. Optionally resolve a one-shot account override (`--account` / `--lpc-account` / `LARKSWITCH_ACCOUNT` / `LPC_ACCOUNT`) with the same strict selector rules as `larkswitch account resolve`. Override paths only read active-state; they never write `active-state.json` or bump generation.
3. Otherwise read the active account from state.
4. Validate managed official binary.
5. Create execution lease containing only PID, process start time, account ID, App ID, and timestamp.
6. Unlock.
7. Spawn official CLI with captured config directory and inherited stdio/cwd. Child env sets `LPC_ACTIVE_ACCOUNT_ID` / `LPC_ACTIVE_APP_ID` for this invocation and inherits `LPC_ACCOUNT` if present.
8. Return exact official exit code and remove lease.

Shim argv rules: only a leading run of `--account` / `--lpc-account` (`VALUE` or `=VALUE`) is consumed. Official `--profile`, `--as`, mid-argv identity flags, and everything after `--` are forwarded unchanged. Unknown leading `--lpc-*` or missing values exit `64` with `LPC_ACCOUNT_SELECTOR_INVALID`.

### Switch

1. Lock routing gate.
2. Validate target account.
3. Atomically update active account/generation.
4. Unlock.

This deliberately permits switching while an old command runs. The old command has already captured its route; a later command observes the new generation. One-shot overrides never participate in this persistent switch path.

## Windows host execution bridge

Some IDE/agent launch contexts can share `LPC_HOME` while seeing a different
HKCU/keychain view. The shim compares a marker stored in both locations before
touching credentials. A mismatch no longer makes the CLI unusable and is never
allowed to fall through to the official binary in that process.

The unpackaged desktop app starts a local Windows named-pipe server only after it
has proved that its disk and registry markers match. On a mismatch (or an MSIX
package identity), the caller forwards argv to that pipe. The desktop launches
the installed shim as its child, so selector parsing, management-command guards,
routing leases, the shared keychain lock, and redacted audit logging all run in
the verified host view. Tokens and keychain values never cross the pipe.

The pipe rejects remote clients and inherits the desktop user's Windows ACL. It
does not use localhost TCP, does not expose a token service, and does not copy a
credential into the caller's registry view. If the desktop is not running, the
shim fails closed with `LPC_HOST_BRIDGE_UNAVAILABLE`.

## App and account lifecycle

The sanitized App base config is accepted only when `appSecret` is an explicit official keychain reference tied to `appsecret:<App ID>`. Plaintext secret configs are rejected.

To add a user, each OAuth batch starts from a fresh base config with `users=[]`. This prevents an incorrect browser login from causing official CLI cleanup of the expected user's previous token through config user replacement.

A successful batch copies only the official `config.json` into a canonical staging directory. Device codes and requested-scope caches never enter persistent state.

## OAuth atomicity limit

The product cannot roll back an official keychain token without reading or copying it, which it intentionally refuses to do. During multi-batch reauthorization, an early successful batch may update the account token before a later batch fails. The account is therefore verified after every batch and never reported complete until the final Scope difference is empty. A failed multi-batch reauthorization may require retrying the authorization plan.

## Permission policy

```text
boundary  = live auth scopes userScopes
policy    = user-selected stable subset of boundary
actual    = token scopes from auth status
remaining = policy - actual
```

The planner groups scopes by the first segment only for presentation/batching. It does not infer implication from names such as `readonly` or `read`.

The initial budgets are conservative and configurable, not represented as platform limits. Before a verification URL is shown, recognizable count/combination failures can shrink the current batch. Every completed batch is reconciled against actual token scopes.

## Runtime management

- Asset name follows the official GoReleaser convention.
- Download official `checksums.txt` and archive over HTTPS.
- Verify SHA-256 before extraction.
- Reject archive path traversal.
- Validate `--version` and required command help surfaces.
- Install into an immutable version directory.
- Atomically update active runtime pointer under routing gate.
- Existing commands continue using their captured old binary path.

## PATH takeover

Setup and the desktop app keep the managed shim first on PATH. They also replace a global npm package `lark-cli.exe` with the managed shim so it cannot bypass routing or the shared keychain lock. `larkswitch path remove` is reserved for explicit uninstall; a running desktop app restores the secure route.

- Windows: prepend one exact user PATH entry in `HKCU\\Environment`; broadcast `WM_SETTINGCHANGE`; uninstall removes only that exact entry.
- macOS / Linux: write a marked block to the current Shell's login and interactive startup files; uninstall removes only the marked block.
- No system PATH mutation and no administrator permission.
