// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import assert from 'node:assert/strict';
// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import { existsSync, readFileSync } from 'node:fs';
// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import test from 'node:test';

const appSource = readFileSync(new URL('./App.tsx', import.meta.url), 'utf8');
const stylesSource = readFileSync(new URL('./styles.css', import.meta.url), 'utf8');
const mainSource = readFileSync(new URL('../src-tauri/src/main.rs', import.meta.url), 'utf8');
const iconUrl = new URL('./assets/app-icon.svg', import.meta.url);

test('sidebar uses the canonical product icon without the placeholder letter or token footnote', () => {
  assert.equal(existsSync(iconUrl), true, 'the canonical SVG asset must exist');
  assert.match(appSource, /import appIcon from ['"]\.\/assets\/app-icon\.svg['"]/);
  assert.match(appSource, /<img\b[^>]*className="brand-icon"[^>]*src=\{appIcon\}[^>]*alt=""[^>]*aria-hidden="true"/);
  assert.match(appSource, /<div className="brand-title">larkswitch<\/div>/);
  assert.doesNotMatch(appSource, />\s*L\s*</);
  assert.doesNotMatch(appSource, /brand-mark|sidebar-foot|Token 由官方 CLI 与系统钥匙串管理/);
  assert.doesNotMatch(appSource, /Lark Profile Console/);
  assert.doesNotMatch(stylesSource, /\.brand-mark\b|\.sidebar-foot\b/);
});

test('tray reuses the bundled default window icon', () => {
  assert.match(mainSource, /\.icon\(\s*app\s*\.default_window_icon\(\)\s*\.cloned\(\)/s);
});
