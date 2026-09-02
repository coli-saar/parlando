import assert from 'node:assert/strict';
import test from 'node:test';

import { adminRequestOptions, experimentRuntimeApi } from '../src/app/admin_dashboard_api.mjs';

test('runtime URLs encode experiment identifiers and preserve feature paths', () => {
  assert.equal(
    experimentRuntimeApi({ experiment_id: 'pilot one' }, '/sessions/4'),
    '/api/admin/runtime/pilot%20one/sessions/4',
  );
});

test('administrator mutations carry CSRF and JSON headers', () => {
  const options = adminRequestOptions(
    { csrfToken: 'token-123' },
    { method: 'POST', body: '{}' },
  );
  assert.equal(options.headers.get('X-CSRF-Token'), 'token-123');
  assert.equal(options.headers.get('Content-Type'), 'application/json');
  assert.equal(adminRequestOptions({ csrfToken: 'ignored' }).headers.has('X-CSRF-Token'), false);
});
