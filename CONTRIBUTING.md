# Contributing

1. Open an issue describing the user-visible result and failure/rollback path.
2. Keep official CLI interaction behind `OfficialCli`; never parse output in UI code.
3. Never add a token/App Secret field to product models.
4. Every routing, scope, filesystem, or PATH change needs failure-path tests.
5. Run `./scripts/verify.sh` (or `scripts/verify.ps1`) before opening a PR.
6. Changes to account isolation or OAuth must pass the real-account manual gate before release.
7. Public name is `larkswitch`. `lpcctl` is a compatibility alias. Do not use official `--profile` to mean a person.

