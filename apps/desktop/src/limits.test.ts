// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import assert from 'node:assert/strict';
// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import { readFileSync } from 'node:fs';
// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import test from 'node:test';
// @ts-expect-error Direct Node execution requires the exact TypeScript module specifier.
import { MAX_SINGLE_AUTH_SCOPES } from './limits.ts';

const rustSource = readFileSync(
  new URL('../../../crates/lpc-core/src/scope_policy.rs', import.meta.url),
  'utf8',
);
const limitsSource = readFileSync(new URL('./limits.ts', import.meta.url), 'utf8');
const drawerSource = readFileSync(
  new URL('./components/ScopePolicyDrawer.tsx', import.meta.url),
  'utf8',
);

test('desktop and Rust share the Feishu single-auth scope ceiling', () => {
  const rustValue = rustSource.match(
    /pub const MAX_SINGLE_AUTH_SCOPES:\s*usize\s*=\s*(\d+)\s*;/,
  )?.[1];
  assert.equal(rustValue, '250');
  assert.equal(MAX_SINGLE_AUTH_SCOPES, 250);
  assert.equal(Number(rustValue), MAX_SINGLE_AUTH_SCOPES);
  assert.match(limitsSource, /export const MAX_SINGLE_AUTH_SCOPES = 250;/);
  assert.match(drawerSource, /import \{ MAX_SINGLE_AUTH_SCOPES \} from '\.\.\/limits'/);
  assert.doesNotMatch(drawerSource, /scopeBatchMaxCount/);
});
