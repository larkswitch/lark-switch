import { copy } from './copy';

export function groupScopes(scopes: string[]): Record<string, string[]> {
  return [...scopes].sort().reduce<Record<string, string[]>>((groups, scope) => {
    const module = scope.split(':')[0] || 'other';
    (groups[module] ??= []).push(scope);
    return groups;
  }, {});
}

export function formatTime(value?: string | null): string {
  if (!value) return copy.common.neverChecked;
  return new Intl.DateTimeFormat('zh-CN', { dateStyle: 'short', timeStyle: 'short' }).format(new Date(value));
}

export function maskId(value: string): string {
  if (value.length <= 10) return value;
  return `${value.slice(0, 5)}…${value.slice(-4)}`;
}

export function normalizeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
