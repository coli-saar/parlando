import assert from 'node:assert/strict';
import test from 'node:test';

import {
  SelectionRequests,
  absoluteParticipantUrl,
  adminRequestOptions,
  experimentRuntimeApi,
  participantUrlCanOpen,
} from '../src/app/admin_dashboard_api.mjs';

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

test('participant URL copying preserves the server-owned Prolific template', () => {
  const template = 'https://public.example/e/pilot/?PROLIFIC_PID={{%PROLIFIC_PID%}}&STUDY_ID={{%STUDY_ID%}}&SESSION_ID={{%SESSION_ID%}}';
  assert.equal(absoluteParticipantUrl('https://admin.internal', template), template);
  assert.equal(
    absoluteParticipantUrl('https://admin.internal', '/e/pilot/'),
    'https://admin.internal/e/pilot/',
  );
});

test('provider-backed participant templates cannot be opened directly', () => {
  assert.equal(participantUrlCanOpen({ participant_url: { kind: 'prolific' } }), false);
  assert.equal(participantUrlCanOpen({ participant_url: { kind: 'local' } }), true);
  assert.equal(participantUrlCanOpen({ participant_url: { kind: 'direct' } }), true);
});

test('repeated polling shares its pending read and invalidation discards late data and errors', async t => {
  const requests = new SelectionRequests();
  const fetch = t.mock.method(globalThis, 'fetch');
  let finish;
  fetch.mock.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  const received = [];
  const read = () => requests.json('sessions', {}, '/first/sessions', data => received.push(data.room));
  const pending = read();
  assert.equal(pending, read());
  assert.equal(fetch.mock.callCount(), 1);
  requests.reset();
  fetch.mock.mockImplementationOnce(async () => Response.json({ room: 'selected' }));
  await requests.json('sessions', {}, '/second/sessions', data => received.push(data.room));
  finish(new Response('old failure', { status: 503 }));
  await pending;
  assert.deepEqual(received, ['selected']);
});
