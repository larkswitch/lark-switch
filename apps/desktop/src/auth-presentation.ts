export function authBytesToObjectUrl(bytes: number[]): string {
  return URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type: 'image/png' }));
}

export function formatRemainingTime(seconds: number): string {
  const safe = Math.max(0, Math.floor(seconds));
  return `${String(Math.floor(safe / 60)).padStart(2, '0')}:${String(safe % 60).padStart(2, '0')}`;
}

export function isAuthorizationExpired(remainingSeconds: number): boolean {
  return remainingSeconds <= 0;
}

export type AuthorizationMode = 'qr' | 'browser';
export type AuthorizationPollPhase = 'waiting' | 'checking';

export function authorizationStatusText(phase: AuthorizationPollPhase): string {
  return phase === 'checking'
    ? '正在向飞书/Lark 核验授权结果，请勿重复发起。'
    : '等待目标账号在飞书/Lark 中确认授权。';
}

export function nextAuthorizationMode(
  current: AuthorizationMode,
  key: string,
): AuthorizationMode | null {
  if (key === 'Home') return 'qr';
  if (key === 'End') return 'browser';
  if (key === 'ArrowLeft' || key === 'ArrowRight') {
    return current === 'qr' ? 'browser' : 'qr';
  }
  return null;
}

export function isDuplicateAccountError(error: unknown): boolean {
  return String(error).includes('[LPC_ACCOUNT_ALREADY_EXISTS]');
}

export function maskAppId(value: string): string {
  return value.length <= 8 ? value : `${value.slice(0, 4)}…${value.slice(-4)}`;
}
