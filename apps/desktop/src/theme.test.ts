// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import assert from 'node:assert/strict';
// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import test from 'node:test';
// @ts-expect-error Direct Node execution requires the exact TypeScript module specifier.
import { getThemeMode, initTheme, resolvedTheme, setThemeMode } from './theme.ts';

type ThemeMode = 'system' | 'light' | 'dark';

class MemoryStorage {
  #map = new Map<string, string>();

  getItem(key: string): string | null {
    return this.#map.has(key) ? this.#map.get(key)! : null;
  }

  setItem(key: string, value: string): void {
    this.#map.set(key, String(value));
  }

  removeItem(key: string): void {
    this.#map.delete(key);
  }

  clear(): void {
    this.#map.clear();
  }
}

function stubBody() {
  const attrs = new Map<string, string>();
  const body = {
    setAttribute(name: string, value: string) {
      attrs.set(name, value);
    },
    removeAttribute(name: string) {
      attrs.delete(name);
    },
    getAttribute(name: string) {
      return attrs.has(name) ? attrs.get(name)! : null;
    },
    hasAttribute(name: string) {
      return attrs.has(name);
    },
  };
  Object.defineProperty(globalThis, 'document', {
    configurable: true,
    value: { body },
  });
  return body;
}

function stubMatchMedia(matches: boolean) {
  const listeners = new Set<(event: { matches: boolean }) => void>();
  const media = {
    matches,
    media: '(prefers-color-scheme: dark)',
    addEventListener(_type: string, listener: (event: { matches: boolean }) => void) {
      listeners.add(listener);
    },
    removeEventListener(_type: string, listener: (event: { matches: boolean }) => void) {
      listeners.delete(listener);
    },
    dispatch(next: boolean) {
      media.matches = next;
      for (const listener of listeners) {
        listener({ matches: next });
      }
    },
  };
  Object.defineProperty(globalThis, 'matchMedia', {
    configurable: true,
    value: () => media,
  });
  return media;
}

function installBrowserStubs(options?: { stored?: string | null; prefersDark?: boolean }) {
  const storage = new MemoryStorage();
  if (options?.stored != null) {
    storage.setItem('larkswitch.theme', options.stored);
  }
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: storage,
  });
  const body = stubBody();
  const media = stubMatchMedia(options?.prefersDark ?? false);
  return { storage, body, media };
}

test('getThemeMode defaults to system when storage is empty', () => {
  installBrowserStubs();
  assert.equal(getThemeMode(), 'system');
});

test('getThemeMode reads persisted light and dark values', () => {
  installBrowserStubs({ stored: 'dark' });
  assert.equal(getThemeMode(), 'dark');

  installBrowserStubs({ stored: 'light' });
  assert.equal(getThemeMode(), 'light');

  installBrowserStubs({ stored: 'system' });
  assert.equal(getThemeMode(), 'system');
});

test('getThemeMode falls back to system for illegal stored values', () => {
  for (const stored of ['', 'Dark', 'nope', 'auto', '1']) {
    installBrowserStubs({ stored });
    assert.equal(getThemeMode(), 'system', `expected ${JSON.stringify(stored)} to fall back`);
  }
});

test('resolvedTheme follows the system preference only in system mode', () => {
  installBrowserStubs({ stored: 'system', prefersDark: true });
  assert.equal(resolvedTheme(), 'dark');

  installBrowserStubs({ stored: 'system', prefersDark: false });
  assert.equal(resolvedTheme(), 'light');

  installBrowserStubs({ stored: 'light', prefersDark: true });
  assert.equal(resolvedTheme(), 'light');

  installBrowserStubs({ stored: 'dark', prefersDark: false });
  assert.equal(resolvedTheme(), 'dark');
});

test('setThemeMode persists and applies immediately', () => {
  const { storage, body } = installBrowserStubs({ stored: 'system', prefersDark: false });

  setThemeMode('dark');
  assert.equal(storage.getItem('larkswitch.theme'), 'dark');
  assert.equal(body.getAttribute('theme-mode'), 'dark');
  assert.equal(resolvedTheme(), 'dark');

  setThemeMode('light');
  assert.equal(storage.getItem('larkswitch.theme'), 'light');
  assert.equal(body.hasAttribute('theme-mode'), false);
  assert.equal(resolvedTheme(), 'light');
});

test('initTheme in system mode tracks prefers-color-scheme until overridden', () => {
  const { body, media } = installBrowserStubs({ prefersDark: false });

  initTheme();
  assert.equal(body.hasAttribute('theme-mode'), false);

  media.dispatch(true);
  assert.equal(body.getAttribute('theme-mode'), 'dark');

  media.dispatch(false);
  assert.equal(body.hasAttribute('theme-mode'), false);

  setThemeMode('light' as ThemeMode);
  media.dispatch(true);
  assert.equal(body.hasAttribute('theme-mode'), false, 'manual light must ignore system changes');
});
