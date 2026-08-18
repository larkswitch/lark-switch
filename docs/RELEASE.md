# Release Process

## Versioning

- Application and crates use the same semver.
- The recommended official CLI version is explicit in `lpc-core::SUPPORTED_CLI_VERSION` and default settings.
- Updating it requires real official CLI smoke and the full manual E2E refresh gate.

## Build

1. Run deterministic verification.
2. Run official CLI smoke on Windows and macOS.
3. Build `lpcctl` and the `lark-cli` Shim for the target triple.
4. `scripts/prepare-sidecars.py` copies target-suffixed sidecars and creates a Tauri config overlay.
5. Build Tauri bundle with that overlay.
6. Run malware/secret scanning on source artifacts and diagnostic fixtures.
7. Sign Windows artifacts; sign and notarize macOS artifacts.
8. Run install/upgrade/uninstall smoke on clean VMs.

## Signing inputs

The project owner must provision:

- Windows code-signing certificate or trusted signing service credentials.
- Apple Developer ID Application certificate.
- Apple notarization account/key and team ID.
- Tauri updater signing private key if application auto-update is enabled.

CI contains no default private key and therefore cannot honestly produce a trusted public production installer until these secrets are configured.

## Rollout

- Alpha: maintainers, real two-user/same-App test.
- Beta: at least five machines covering existing/no CLI and both mac architectures.
- GA: seven consecutive days without identity routing, credential, or unrecoverable runtime-update incident.
