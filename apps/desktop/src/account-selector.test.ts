// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import assert from 'node:assert/strict';
// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import test from 'node:test';
// @ts-expect-error Direct Node execution requires the exact TypeScript module specifier.
import { ACCOUNT_COMMAND_PLACEHOLDER, accountCommand, accountSelector, validateAccountAlias } from './account-selector.ts';
import type { AccountRecord } from './types';

function account(partial: Pick<AccountRecord, 'id' | 'displayName'> & Partial<AccountRecord>): AccountRecord {
  return {
    appRef: 'app',
    userOpenId: 'ou_test',
    configDir: 'x',
    credentialOrigin: 'managed',
    health: 'ready',
    effectiveScopes: [],
    createdAt: '2020-01-01T00:00:00Z',
    updatedAt: '2020-01-01T00:00:00Z',
    alias: null,
    ...partial,
  };
}

test('prefers alias selector when an alias is set', () => {
  const named = account({ id: '11111111-1111-1111-1111-111111111111', displayName: 'Alice', alias: 'work' });
  const other = account({ id: '22222222-2222-2222-2222-222222222222', displayName: 'Bob' });
  assert.equal(accountSelector(named, [named, other]), 'alias:work');
});

test('uses the bare display name when it is unique and there is no alias', () => {
  const alice = account({ id: '11111111-1111-1111-1111-111111111111', displayName: 'Alice' });
  const bob = account({ id: '22222222-2222-2222-2222-222222222222', displayName: 'Bob' });
  assert.equal(accountSelector(alice, [alice, bob]), 'Alice');
});

test('falls back to id when the display name is ambiguous', () => {
  const first = account({ id: '11111111-1111-1111-1111-111111111111', displayName: 'Alice' });
  const second = account({ id: '22222222-2222-2222-2222-222222222222', displayName: 'Alice' });
  assert.equal(accountSelector(first, [first, second]), 'id:11111111-1111-1111-1111-111111111111');
  assert.equal(accountSelector(second, [first, second]), 'id:22222222-2222-2222-2222-222222222222');
});

test('alias wins even when the display name would be unique', () => {
  const named = account({ id: '11111111-1111-1111-1111-111111111111', displayName: 'Alice', alias: 'desk' });
  const other = account({ id: '22222222-2222-2222-2222-222222222222', displayName: 'Bob' });
  assert.equal(accountSelector(named, [named, other]), 'alias:desk');
});

test('formats a full lark-cli command with a visible placeholder', () => {
  const named = account({ id: '11111111-1111-1111-1111-111111111111', displayName: 'Alice', alias: 'work' });
  assert.equal(
    accountCommand(named, [named]),
    `lark-cli --account alias:work ${ACCOUNT_COMMAND_PLACEHOLDER}`,
  );
});

test('mirrors backend alias rules before save', () => {
  assert.equal(validateAccountAlias('').ok, false);
  assert.equal(validateAccountAlias('   ').ok, false);
  assert.deepEqual(validateAccountAlias('  ok  '), { ok: true, alias: 'ok' });
  assert.equal(validateAccountAlias('id:x').ok, false);
  assert.equal(validateAccountAlias('ALIAS:x').ok, false);
  assert.equal(validateAccountAlias('app:x').ok, false);
  assert.equal(validateAccountAlias('a/b').ok, false);
  assert.equal(validateAccountAlias('a\\b').ok, false);
  assert.equal(validateAccountAlias('a\nb').ok, false);
  assert.equal(validateAccountAlias('x'.repeat(64)).ok, true);
  assert.equal(validateAccountAlias('x'.repeat(65)).ok, false);
  assert.equal(validateAccountAlias('中'.repeat(21)).ok, true);
  assert.equal(validateAccountAlias('中'.repeat(22)).ok, false);
});
