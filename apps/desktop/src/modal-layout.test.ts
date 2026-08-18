// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import assert from 'node:assert/strict';
// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import { readFileSync } from 'node:fs';
// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import test from 'node:test';

const appSource = [
  readFileSync(new URL('./App.tsx', import.meta.url), 'utf8'),
  readFileSync(new URL('./modals/ImportAppModal.tsx', import.meta.url), 'utf8'),
  readFileSync(new URL('./modals/OfficialAppCreationModal.tsx', import.meta.url), 'utf8'),
  readFileSync(new URL('./modals/ImportExistingAccountModal.tsx', import.meta.url), 'utf8'),
].join('\n');
const stylesSource = readFileSync(new URL('./styles.css', import.meta.url), 'utf8');

function count(source: string, pattern: RegExp): number {
  return [...source.matchAll(pattern)].length;
}

function cssRule(selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = stylesSource.match(new RegExp(`${escaped}\\s*\\{([^}]*)\\}`));
  assert.ok(match, `Expected CSS rule for ${selector}`);
  return match[1];
}

test('all five workflow modals expose shared shell classes and reachable footer actions', () => {
  assert.equal(count(appSource, /<Modal\b/g), 5, 'expected exactly five workflow modals');
  assert.equal(count(appSource, /footer=\{null\}/g), 0, 'workflow actions must not be embedded in footerless bodies');
  assert.equal(count(appSource, /className="lpc-modal"/g), 5, 'every workflow modal needs the shared outer class');
  assert.equal(
    count(appSource, /modalContentClass="lpc-modal-content"/g),
    5,
    'Semi Modal needs the shared content class on every workflow modal',
  );
});

test('modal CSS establishes a viewport-safe body scroll boundary and 4pt spacing rhythm', () => {
  assert.match(cssRule('.lpc-modal .semi-modal'), /max-width:\s*calc\(100vw - 32px\)/);
  assert.match(cssRule('.lpc-modal .semi-modal'), /margin:\s*32px auto/);

  const content = cssRule('.lpc-modal-content');
  assert.match(content, /max-height:\s*calc\(100vh - 64px\)/);

  const body = cssRule('.lpc-modal-content .semi-modal-body');
  assert.match(body, /min-height:\s*0/);
  assert.match(body, /overflow-y:\s*auto/);
  assert.match(body, /overscroll-behavior:\s*contain/);

  const footer = cssRule('.lpc-modal-content .semi-modal-footer');
  assert.match(footer, /flex:\s*none/);

  const modalTokens = cssRule('.lpc-modal');
  assert.match(modalTokens, /--modal-space-label-control:\s*8px/);
  assert.match(modalTokens, /--modal-space-field:\s*16px/);
  assert.match(modalTokens, /--modal-space-group:\s*24px/);

  assert.doesNotMatch(cssRule('.form-stack'), /margin-top/);
  assert.ok(count(appSource, /className="modal-stack"/g) >= 3, 'add/import workflows should share modal-stack');
});

test('the App selector keeps detailed options but renders a single-line selected identity', () => {
  assert.match(appSource, /renderSelectedItem=\{/);
  assert.match(appSource, />授权 App</);
  assert.match(appSource, /'aria-label':\s*'授权 App'/);

  assert.match(cssRule('.app-option-label'), /flex-direction:\s*column/);
  const selectedLabel = cssRule('.app-selected-label');
  assert.match(selectedLabel, /white-space:\s*nowrap/);
  assert.match(selectedLabel, /overflow:\s*hidden/);
  assert.match(cssRule('.app-selected-secondary'), /text-overflow:\s*ellipsis/);
});

test('brand selectors are named and migration candidates have dedicated selection and wrap styles', () => {
  assert.match(appSource, /'aria-label':\s*'创建 App 品牌'/);
  assert.match(appSource, /'aria-label':\s*'导入 App 品牌'/);
  assert.match(appSource, /id="create-app-brand-label"/);
  assert.match(appSource, /aria-labelledby="create-app-brand-label"/);
  assert.match(appSource, /id="import-app-brand-label"/);
  assert.match(appSource, /aria-labelledby="import-app-brand-label"/);
  assert.match(appSource, /migration-candidate-card-selected/);
  assert.match(cssRule('.migration-candidate-card .card-title-row'), /flex-wrap:\s*wrap/);
  assert.match(cssRule('.migration-candidate-card-selected'), /border-color:/);
});
