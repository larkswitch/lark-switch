# Product Definition — V1

## Positioning

Lark Profile Console is not merely a GUI for CLI flags. It is the local identity-routing layer between users/agents and the official Lark CLI.

## Domain model

- **App**: App ID, brand, official keychain App Secret reference, live permission boundary, and stable permission policy.
- **Account**: `(App, User Open ID)` plus an isolated official CLI configuration directory.
- **Active account**: the default route used when the next Shim process starts.
- **Execution snapshot**: immutable account/config/runtime selected when a command starts.

One App can have N users. One user can also appear under multiple Apps as distinct accounts.

## Non-negotiable V1 outcomes

1. Existing App import and official App creation.
2. One App with at least two independently usable real users.
3. Multiple Apps in one installation.
4. Windows and macOS installers.
5. Optional PATH takeover with exact-entry/managed-block removal.
6. Tray account switching.
7. Running commands retain old account; future invocations use new account.
8. Official CLI owns all token/keychain work.
9. Live permission boundary and stable App policy.
10. Strict serial OAuth batches with actual-Scope verification.
11. Official CLI install, integrity verification, activation, and rollback.
12. Diagnostics that never dump credentials.

## Primary flows

### First setup

1. Desktop opens System Status.
2. Install recommended official CLI.
3. Install the product Shim and make it the only command-name route to the official CLI.
4. Reopen the desktop after the runtime is first installed.

### Add App

- Existing App: input App ID / App Secret, pass secret by stdin to official `config init`, verify App, read live userScopes, store only sanitized keychain reference.
- New App: launch official `config init --new` in a system terminal and import the resulting sanitized configuration.

### Add account

1. Refresh live App boundary.
2. Compute stable target policy.
3. Create blank isolated batch config using only the App keychain reference.
4. Start one official device flow.
5. Open opaque official verification URL in system browser.
6. Complete flow, verify identity and actual Scope.
7. Continue the next batch only after recomputing the remaining set.
8. Promote only `config.json`; never persist device-code caches.

### Switch

- Acquire routing gate.
- Atomically update active account and generation.
- Release gate.
- Running leases are not interrupted and continue on their immutable snapshot.

### Remove account

- Refuse while that account has active leases.
- Run official logout only in that account's isolated configuration.
- Delete account metadata/config.
- Never call native `profile remove`, because the App Secret may be shared by other users under the same App.

## Scope policy UX

The default view shows App and policy counts. Raw scopes are an advanced view grouped by prefix only for readability. Prefixes do not imply permission inheritance.

New App permissions are never silently selected. “Use all current App permissions” is an explicit action.

## Success metrics

- 0 cross-account command identity changes after process start.
- 0 scope request outside live App boundary.
- 0 successful UI completion while target Scope difference is non-empty.
- 0 product-state/log/diagnostic secret findings.
- 100% runtime activation rollback on compatibility failure.
- First real account usable without Node.js/npm on the target machine.
