# Desktop startup recovery (0.2.14)

Antivirus cleanup left an orphaned WebView browser holding the desktop profile.
Further launches created invisible, unresponsive windows before startup guards
or logging ran. The legacy Task Scheduler entries also failed with 0x80070057;
registering a new task identity succeeded on the affected machine.

The desktop now disables automatic WebView creation. It initializes logging,
hands normal Windows launches to the scheduled host, and acquires the data-root
singleton before creating the window. A second launch requests that the running
host unminimize and show its existing window. Hidden launches remain hidden.

Only the exclusive desktop owner may stop leftover msedgewebview2 processes,
and only when their user-data-dir exactly matches this application's WebView
profile. It does this before creating its own WebView. No profile files are
deleted. Window initialization has a 30-second timeout with a visible error;
credential operations have not started at that point. Task Scheduler calls have
a 20-second timeout and failures are logged and shown rather than swallowed.

The replacement on-demand tasks are `LarkSwitch Desktop Host v2` and
`LarkSwitch Desktop Host Visible v2`. They run as the interactive user without
elevation, including on battery power, with no automatic execution timeout.
Normal shortcuts carry no bootstrap argument. The scheduled-host credential
view checks remain in place; this change does not classify or suppress antivirus
alerts.

The bootstrap flag alone no longer grants host authority. Before reading or
repairing the host marker, the desktop verifies that its actual parent PID is
the Schedule service PID reported by Windows Service Control Manager. Launching
the bootstrap flag directly from an agent now fails closed. During recovery,
a direct agent launch had refreshed tokens in an isolated registry view; the
host then retried stale copies. Eleven missing host values were restored one
at a time from the verified newer encrypted copies, preserving the remaining
host account. All twelve user identities subsequently verified as available.

For a local repair, build sidecars, then package the desktop with
`npx tauri build --no-bundle --config src-tauri/tauri.sidecars.conf.json` from
`apps/desktop`. Run the core deployment contract with
`LPC_REQUIRE_DEPLOY_ARTIFACT=1`, then run `scripts/repair-desktop-install.ps1`.
The repair validates embedded assets, backs up existing executables, replaces
the desktop and both sidecar locations, verifies hashes, restores the normal
Start menu shortcut, and starts the application. It does not restore credentials
or change antivirus settings.
