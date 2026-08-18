export type ThemeMode = 'system' | 'light' | 'dark';

const STORAGE_KEY = 'larkswitch.theme';
const DARK_MEDIA = '(prefers-color-scheme: dark)';

let attachedMedia: MediaQueryList | null = null;

function parseThemeMode(value: string | null | undefined): ThemeMode {
  if (value === 'system' || value === 'light' || value === 'dark') {
    return value;
  }
  return 'system';
}

function readStorage(): string | null {
  try {
    return globalThis.localStorage?.getItem(STORAGE_KEY) ?? null;
  } catch {
    return null;
  }
}

function writeStorage(mode: ThemeMode): void {
  try {
    globalThis.localStorage?.setItem(STORAGE_KEY, mode);
  } catch {
    // Quota / privacy mode: keep the in-memory apply path working.
  }
}

function systemPrefersDark(): boolean {
  try {
    return Boolean(globalThis.matchMedia?.(DARK_MEDIA).matches);
  } catch {
    return false;
  }
}

function applyToBody(theme: 'light' | 'dark'): void {
  const body = globalThis.document?.body;
  if (!body) {
    return;
  }
  if (theme === 'dark') {
    body.setAttribute('theme-mode', 'dark');
  } else {
    body.removeAttribute('theme-mode');
  }
}

function onSystemPreferenceChange(event: MediaQueryListEvent): void {
  if (getThemeMode() !== 'system') {
    return;
  }
  applyToBody(event.matches ? 'dark' : 'light');
}

function setSystemListener(enabled: boolean): void {
  const media = globalThis.matchMedia?.(DARK_MEDIA) ?? null;

  if (attachedMedia && (!enabled || attachedMedia !== media)) {
    attachedMedia.removeEventListener('change', onSystemPreferenceChange);
    attachedMedia = null;
  }

  if (enabled && media && attachedMedia !== media) {
    media.addEventListener('change', onSystemPreferenceChange);
    attachedMedia = media;
  }
}

function applyCurrent(): void {
  applyToBody(resolvedTheme());
  setSystemListener(getThemeMode() === 'system');
}

export function getThemeMode(): ThemeMode {
  return parseThemeMode(readStorage());
}

export function setThemeMode(mode: ThemeMode): void {
  const next = parseThemeMode(mode);
  writeStorage(next);
  applyCurrent();
}

export function resolvedTheme(): 'light' | 'dark' {
  const mode = getThemeMode();
  if (mode === 'light' || mode === 'dark') {
    return mode;
  }
  return systemPrefersDark() ? 'dark' : 'light';
}

export function initTheme(): void {
  applyCurrent();
}
