import type { AccountRecord } from './types';

export const MAX_ALIAS_BYTES = 64;
export const ACCOUNT_COMMAND_PLACEHOLDER = '<你的命令>';

export type AliasInvalidReason = 'empty' | 'too_long' | 'bad_chars' | 'bad_prefix';

export type AliasValidation =
  | { ok: true; alias: string }
  | { ok: false; reason: AliasInvalidReason };

export function aliasUtf8ByteLength(value: string): number {
  return new TextEncoder().encode(value).length;
}

export function validateAccountAlias(raw: string): AliasValidation {
  const alias = raw.trim();
  if (alias.length === 0) {
    return { ok: false, reason: 'empty' };
  }
  if (aliasUtf8ByteLength(alias) > MAX_ALIAS_BYTES) {
    return { ok: false, reason: 'too_long' };
  }
  if (alias.includes('/') || alias.includes('\\') || alias.includes('\0') || /[\p{Cc}\p{Cf}]/u.test(alias)) {
    return { ok: false, reason: 'bad_chars' };
  }
  const lower = alias.toLowerCase();
  if (lower.startsWith('id:') || lower.startsWith('alias:') || lower.startsWith('app:')) {
    return { ok: false, reason: 'bad_prefix' };
  }
  return { ok: true, alias };
}

export function accountSelector(account: AccountRecord, allAccounts: AccountRecord[]): string {
  const alias = account.alias?.trim();
  if (alias) {
    return `alias:${alias}`;
  }
  const unique = allAccounts.filter((item) => item.displayName === account.displayName).length === 1;
  if (unique) {
    return account.displayName;
  }
  return `id:${account.id}`;
}

export function accountCommand(account: AccountRecord, allAccounts: AccountRecord[]): string {
  return `lark-cli --account ${accountSelector(account, allAccounts)} ${ACCOUNT_COMMAND_PLACEHOLDER}`;
}
