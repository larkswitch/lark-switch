// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import assert from 'node:assert/strict';
// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import { readFileSync } from 'node:fs';
// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import test from 'node:test';

const source = readFileSync(new URL('./App.tsx', import.meta.url), 'utf8');
const settingsSource = readFileSync(new URL('./pages/SettingsPage.tsx', import.meta.url), 'utf8');
const copySource = readFileSync(new URL('./copy.ts', import.meta.url), 'utf8');
const rustSource = readFileSync(new URL('../src-tauri/src/main.rs', import.meta.url), 'utf8');
const tauriConf = JSON.parse(
  readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8'),
);
const packageJson = JSON.parse(
  readFileSync(new URL('../package.json', import.meta.url), 'utf8'),
);
const workspaceCargo = readFileSync(new URL('../../../Cargo.toml', import.meta.url), 'utf8');

test('settings exposes autostart while desktop enforces background refresh lifecycle', () => {
  // User-visible strings live in copy.ts; the page wires them to the autostart switch.
  assert.match(copySource, /title: '设置'/);
  assert.match(copySource, /autostart: '开机自动启动'/);
  assert.match(copySource, /每 15 分钟校验并按需刷新全部/);
  assert.match(settingsSource, /copy\.settings\.title/);
  assert.match(settingsSource, /aria-label=\{copy\.settings\.autostart\}/);
  assert.match(settingsSource, /api\.setAutostart\(enabled\)/);
  assert.match(copySource, /安全路由已强制开启/);
  assert.match(settingsSource, /aria-label=\{copy\.settings\.pathTakeover\}[\s\S]*?checked[\s\S]*?disabled/);
  assert.doesNotMatch(settingsSource, /api\.removePathTakeover\(\)/);
  assert.match(settingsSource, /setThemeMode\(/);
  assert.match(source, /<SettingsPage data=\{snapshot\} onReload=\{reload\} \/>/);
  assert.match(settingsSource, /api\.runtimeIdentity\(\)/);
  assert.match(rustSource, /ensure_installed_autostart\(/);
  assert.match(rustSource, /pin_user_run_autostart\(/);
  assert.match(rustSource, /refresh_account_health_with\(/);
  assert.match(rustSource, /HealthRefreshOutcome::SkippedBusy/);
  assert.match(rustSource, /should_skip_scheduled_verify/);
  assert.match(rustSource, /should_force_reauth_verify/);
  assert.match(rustSource, /for account in skipped/);
  assert.match(rustSource, /skipped twice \(keychain busy\)/);
  assert.match(source, /lpc:\/\/keychain-cliff/);
  assert.match(copySource, /钥匙串槽位从/);
  assert.match(settingsSource, /copy\.settings\.running/);
  assert.match(rustSource, /repair_managed_route\(&store\)/);
  assert.match(source, /if \(page === 'system'\) \{\s*void refreshDiagnostics\(\);/);
  assert.match(rustSource, /Some\(vec!\["--hidden"\]\)/);
  assert.match(rustSource, /api\.prevent_close\(\)/);
});

test('desktop and Rust artifacts share one explicit product version', () => {
  const workspaceVersion = workspaceCargo.match(
    /\[workspace\.package\][\s\S]*?version = "([^"]+)"/,
  )?.[1];
  assert.equal(packageJson.version, workspaceVersion);
  assert.equal(tauriConf.version, workspaceVersion);
});

test('NSIS install stays searchable and refuses cargo-target autostart registration', () => {
  assert.equal(tauriConf.productName, 'larkswitch');
  assert.equal(tauriConf.bundle?.publisher, 'larkswitch');
  assert.equal(tauriConf.bundle?.windows?.nsis?.installMode, 'currentUser');
  assert.match(rustSource, /fn is_cargo_target_build_exe/);
  assert.match(rustSource, /fn is_packaged_app_virtualized_exe/);
  assert.match(
    rustSource,
    /Autostart can only register the installed application executable, not a cargo target build/,
  );
  assert.match(
    rustSource,
    /Autostart cannot register a packaged-app virtualized copy/,
  );
  assert.match(rustSource, /argument == "--hidden"/);
});

test('hidden autostart never relies on late hide after visible=true', () => {
  assert.equal(tauriConf.app?.windows?.[0]?.visible, false);
  assert.match(
    rustSource,
    /if !std::env::args\(\)\.any\(\|argument\| argument == "--hidden"\)/,
  );
  assert.match(rustSource, /window\.show\(\)\?;/);
  assert.doesNotMatch(
    rustSource,
    /if std::env::args\(\)\.any\(\|argument\| argument == "--hidden"\) \{\s*if let Some\(window\) = app\.get_webview_window\("main"\) \{\s*window\.hide\(\)\?;/,
  );
  assert.match(rustSource, /id == "open"[\s\S]*?window\.show\(\)/);
});
