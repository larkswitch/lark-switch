// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import assert from 'node:assert/strict';
// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import test from 'node:test';
// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import { readFileSync } from 'node:fs';
// @ts-expect-error Direct Node execution requires the exact TypeScript module specifier.
import * as authPresentation from './auth-presentation.ts';

const {
  authBytesToObjectUrl,
  formatRemainingTime,
  isAuthorizationExpired,
  isDuplicateAccountError,
  maskAppId,
  nextAuthorizationMode,
} = authPresentation;

test('formats remaining OAuth time', () => {
  assert.equal(formatRemainingTime(125), '02:05');
  assert.equal(formatRemainingTime(-1), '00:00');
});

test('expires authorization actions when no time remains', () => {
  assert.equal(isAuthorizationExpired(0), true);
  assert.equal(isAuthorizationExpired(1), false);
});

test('maps standard OAuth tab navigation keys', () => {
  assert.equal(nextAuthorizationMode('qr', 'ArrowRight'), 'browser');
  assert.equal(nextAuthorizationMode('browser', 'ArrowRight'), 'qr');
  assert.equal(nextAuthorizationMode('qr', 'ArrowLeft'), 'browser');
  assert.equal(nextAuthorizationMode('browser', 'ArrowLeft'), 'qr');
  assert.equal(nextAuthorizationMode('browser', 'Home'), 'qr');
  assert.equal(nextAuthorizationMode('qr', 'End'), 'browser');
  assert.equal(nextAuthorizationMode('qr', 'Tab'), null);
});

test('recognizes only the stable duplicate account error', () => {
  assert.equal(isDuplicateAccountError('[LPC_ACCOUNT_ALREADY_EXISTS] account exists'), true);
  assert.equal(isDuplicateAccountError('[LPC_AUTH_FLOW_NOT_FOUND] missing'), false);
});

test('masks App IDs without embedding a real identifier', () => {
  assert.equal(maskAppId('cli_1234567890'), 'cli_…7890');
});

test('creates a PNG Blob URL from authorization bytes', async () => {
  const originalCreateObjectURL = URL.createObjectURL;
  let captured: Blob | undefined;
  URL.createObjectURL = (blob) => {
    captured = blob as Blob;
    return 'blob:oauth-qr';
  };

  try {
    assert.equal(authBytesToObjectUrl([137, 80, 78, 71]), 'blob:oauth-qr');
    if (!captured) throw new Error('Expected the QR Blob to be created');
    assert.equal(captured.type, 'image/png');
    assert.deepEqual([...new Uint8Array(await captured.arrayBuffer())], [137, 80, 78, 71]);
  } finally {
    URL.createObjectURL = originalCreateObjectURL;
  }
});

test('explains whether OAuth is waiting on the user or checking the server', () => {
  const presentation = authPresentation as typeof authPresentation & {
    authorizationStatusText?: (phase: 'waiting' | 'checking') => string;
  };

  assert.equal(
    presentation.authorizationStatusText?.('waiting'),
    '等待目标账号在飞书/Lark 中确认授权。',
  );
  assert.equal(
    presentation.authorizationStatusText?.('checking'),
    '正在向飞书/Lark 核验授权结果，请勿重复发起。',
  );
});

test('authorization completes automatically without batches or a manual confirmation button', () => {
  const appSource = readFileSync(new URL('./App.tsx', import.meta.url), 'utf8');
  const copySource = readFileSync(new URL('./copy.ts', import.meta.url), 'utf8');
  // Batch vocabulary must stay out of both the flow and the strings it renders.
  const visible = `${appSource}\n${copySource}`;

  assert.doesNotMatch(appSource, /progress\.next/);
  assert.doesNotMatch(visible, /上一批|下一批|当前批次|本批能力/);
  assert.doesNotMatch(visible, /我已完成授权/);
  assert.match(appSource, /void pollAuthorization\(auth\.flowId\)/);
  assert.match(copySource, /一次性申请默认核心权限/);
  assert.doesNotMatch(visible, /使用 App 当前全部权限/);
});
