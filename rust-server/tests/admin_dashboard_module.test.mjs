import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

test('dashboard entry module has valid ECMAScript module syntax', () => {
  const source = readFileSync(
    new URL('../src/app/admin_dashboard.js', import.meta.url),
    'utf8',
  );
  const result = spawnSync(process.execPath, ['--input-type=module', '--check'], {
    encoding: 'utf8',
    input: source,
  });
  assert.equal(result.status, 0, result.stderr);
});
