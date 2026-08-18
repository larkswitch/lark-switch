# Security Model

## Credential ownership

The official `lark-cli` is the only component allowed to create, refresh, revoke, read, or write:

- App Secret values in the OS keychain.
- User access tokens.
- Refresh tokens.

Lark Profile Console stores only App IDs, user Open IDs, display metadata, scope names, keychain references already present in official config, and health timestamps.

## Existing App import

An App Secret must enter somewhere to configure the official CLI. In the desktop V1 it exists transiently in:

1. password input memory in WebView,
2. Tauri IPC message memory,
3. Rust `SecretString`,
4. stdin pipe to official `lark-cli config init --app-secret-stdin`.

It is never written to product files or included in argv. Rust copies are zeroized where ownership allows. The WebView field is cleared immediately after success. Users requiring a smaller UI trust boundary can use `lpcctl app import --secret-stdin` from a pipe.

## Device code

Device code is returned by the official CLI and retained only in the in-memory OAuth coordinator. UI DTOs receive the opaque verification URL but not the device code. A desktop restart or flow expiry discards it and starts a fresh flow.

The official CLI currently accepts completion through a `--device-code` flag; therefore the code can be briefly visible to same-user process inspection while the completion process runs. Product logs never record the argv. Removing this exposure requires an upstream stdin/API completion interface.

## Logging

Production code must not log:

- command argv,
- OAuth URLs,
- App Secret,
- device/user code,
- access/refresh token,
- file contents passed to the CLI.

Redaction covers common structured and header forms. Diagnostic output contains paths, IDs, versions, counts, health states, and redacted error text only.

## Supply chain

- HTTPS-only official release URL.
- Official release checksums.
- Streaming SHA-256 verification.
- Archive path traversal rejection.
- Required command compatibility checks before activation.
- Version directories are immutable after activation.
- Repository CI pins Rust toolchain and lockfiles.
- Desktop signing keys are CI secrets, never repository files.

## Local attack boundary

This is a per-user desktop tool. A process already running as the same OS user can inspect that user's files/processes and invoke the official CLI; Lark Profile Console does not claim protection against a fully compromised user session. It does prevent accidental cross-account routing and avoids creating a new network token vault or localhost credential service.

## Reporting

Do not include raw config files, auth logs, OAuth URLs, or diagnostic bundles in public issues until they have been reviewed for secrets. See repository `SECURITY.md` for the disclosure channel placeholder that maintainers must configure before public launch.
