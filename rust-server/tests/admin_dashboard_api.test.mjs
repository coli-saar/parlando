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

test('refresh after deletion replaces a stale list and preserves unrelated pending reads', async t => {
  const requests = new SelectionRequests();
  const replies = [];
  t.mock.method(globalThis, 'fetch', () => new Promise(resolve => replies.push(resolve)));
  const seen = [];
  const read = key => requests.json(key, {}, '/' + key, data => seen.push(data));
  const old = read('sessions');
  const config = read('config');
  requests.invalidate('sessions');
  const fresh = read('sessions');
  assert.equal(config, read('config'));
  replies[2](Response.json('fresh'));
  await fresh;
  replies[0](Response.json('obsolete'));
  replies[1](Response.json('configuration'));
  await Promise.all([old, config]);
  assert.deepEqual(seen, ['fresh', 'configuration']);
});

test('read-only session search coalesces identical suffixes and drops stale results', async t => {
  const requests = new SelectionRequests();
  const replies = [];
  t.mock.method(globalThis, 'fetch', () => new Promise(resolve => replies.push(resolve)));
  const received = [];
  const search = suffix => requests.json('sessions', {}, '/sessions/search', data => received.push(data), {
    method: 'POST',
    body: JSON.stringify({ prolific_pid_suffix: suffix }),
  }, true);
  const first = search('123');
  assert.equal(search('123'), first);
  const second = search('456');
  assert.equal(replies.length, 2);
  replies[1](Response.json('current'));
  replies[0](Response.json('obsolete'));
  await Promise.all([first, second]);
  assert.deepEqual(received, ['current']);
});
