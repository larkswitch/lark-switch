// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import assert from 'node:assert/strict';
// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import { readFileSync } from 'node:fs';
// @ts-expect-error Node's built-in runner provides this module; the browser app has no Node types.
import test from 'node:test';

const modalSource = readFileSync(new URL('./modals/OfficialAppCreationModal.tsx', import.meta.url), 'utf8');

test('App creation polling serializes every modal close path', () => {
  assert.match(modalSource, /const \[polling, setPolling\] = useState\(false\);/);
  assert.match(
    modalSource,
    /const close = async \(\) => \{\s*if \(pollInFlight\.current\) return;\s*pollInFlight\.current = true;/,
    'close must atomically acquire the same lock before cancellation can yield',
  );
  assert.match(
    modalSource,
    /pollInFlight\.current = true;\s*setPolling\(true\);/,
    'poll start must synchronously lock closing and expose render state',
  );
  assert.match(
    modalSource,
    /catch \(cancelError\) \{\s*Toast\.error\(normalizeError\(cancelError\)\);\s*setBusy\(false\);\s*return;\s*\} finally \{\s*pollInFlight\.current = false;\s*\}\s*reset\(\);\s*props\.onClose\(\);/,
    'all cancel outcomes must release the lock, while failure retains the flow and modal',
  );
  assert.match(
    modalSource,
    /finally \{\s*pollInFlight\.current = false;\s*if \(active\) setPolling\(false\);\s*\}/,
    'poll cleanup must unlock mounted UI without updating an inactive effect',
  );

  assert.equal(
    [...modalSource.matchAll(/<Button disabled=\{busy \|\| polling\} onClick=\{\(\) => void close\(\)\}>/g)].length,
    2,
    'both footer cancel variants must be disabled while polling',
  );
  assert.match(modalSource, /closable=\{!busy && !polling\}/);
  assert.match(modalSource, /closeOnEsc=\{!busy && !polling\}/);
  assert.match(modalSource, /maskClosable=\{!busy && !polling\}/);
});
