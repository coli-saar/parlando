import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';

const assets = new URL('../../rust-server/src/app/', import.meta.url);

const files = {
  '/admin/': ['admin_dashboard.html', 'text/html'],
  '/admin/assets/admin-dashboard.js': ['admin_dashboard.js', 'text/javascript'],
  '/admin/assets/admin-dashboard-format.js': ['admin_dashboard_format.mjs', 'text/javascript'],
  '/admin/assets/admin-dashboard-api.js': ['admin_dashboard_api.mjs', 'text/javascript'],
  '/admin/assets/admin-dashboard-state.js': ['admin_dashboard_state.js', 'text/javascript'],
  '/admin/assets/communication-config.js': ['communication_config.mjs', 'text/javascript'],
  '/admin/assets/admin-dashboard.css': ['admin_dashboard.css', 'text/css']
};

/** Builds a minimal catalogue entry for the real dashboard selection controls. */
function experiment(id) {
  return { experiment_id: id, game_version: '1.0', config_revision: 1, status: 'inactive', created_at: '2026-01-01T00:00:00Z' };
}

/** Creates a durable session with an identifying name for both the list and detail. */
function session(id, name) {
  return { session_id: id, dialogue_id: name, lifecycle: 'running', purpose: 'research', created_at: '2026-01-01T00:00:00Z', event_count: 0 };
}

/** Delays a specified request until the test has selected another dashboard view. */
function pendingResponse() {
  let release;
  let started;
  const blocked = new Promise(resolve => { release = resolve; });
  const requested = new Promise(resolve => { started = resolve; });
  return { release, requested, async wait() { started(); await blocked; } };
}

test('dashboard keeps late experiment lists and session details out of the selected view', async ({ page }) => {
  const slowList = pendingResponse();
  const slowDetail = pendingResponse();
  const slowEvents = pendingResponse();
  const sessions = [session(2, 'New first room'), session(3, 'Selected room')];
  const experiments = [experiment('first'), experiment('second')];
  await page.route('http://dashboard.test/**', async route => {
    const path = new URL(route.request().url()).pathname;
    if (files[path]) {
      const [name, contentType] = files[path];
      await route.fulfill({ path: fileURLToPath(new URL(name, assets)), contentType });
    } else if (path === '/api/admin/experiments') {
      await route.fulfill({ json: { experiments, game: { name: 'Test', version: '1.0' } } });
    } else if (path.endsWith('/config')) {
      const entry = experiments.find(item => path.includes('/' + item.experiment_id + '/'));
      await route.fulfill({ json: { experiment: { ...entry, config: {} } } });
    } else if (path.endsWith('/first/sessions')) {
      await slowList.wait();
      await route.fulfill({ json: { sessions: [session(1, 'Stale experiment')] } });
    } else if (path.endsWith('/second/sessions')) {
      await route.fulfill({ json: { sessions } });
    } else if (path.endsWith('/sessions/2')) {
      await slowDetail.wait();
      await route.fulfill({ json: { session: sessions[0], event_bundles: [{ title: 'Selected room event', first_index: 1, kind: 'action' }] } });
    } else if (path.endsWith('/sessions/3/events')) {
      await slowEvents.wait();
      await route.fulfill({ json: { event_bundles: [{ title: 'Stale event', first_index: 1, kind: 'action' }] } });
    } else if (path.endsWith('/sessions/3')) {
      await route.fulfill({ json: { session: sessions[1] } });
    } else if (path.endsWith('/progress')) {
      await route.fulfill({ json: { progress: { sessions, total_sessions: 2 } } });
    } else if (path.endsWith('/load')) {
      await route.fulfill({ status: 503 });
    } else {
      await route.fulfill({ json: {} });
    }
  });
  // Hold polling intervals while exercising the same real list/detail loaders they invoke.
  await page.clock.install({ time: new Date('2026-01-01T12:00:00Z') });
  await page.clock.pauseAt(new Date('2026-01-01T12:00:01Z'));
  await page.goto('http://dashboard.test/admin/');
  await slowList.requested;
  await page.locator('[data-experiment="second"]').click();
  await slowDetail.requested;
  await page.locator('[data-session="3"]').click();
  await expect(page.locator('#summary')).toContainText('Selected room');
  const listDone = page.waitForResponse(r => r.url().includes('/first/sessions?'));
  const detailDone = page.waitForResponse(r => r.url().endsWith('/sessions/2'));
  slowList.release();
  slowDetail.release();
  await Promise.all([(await listDone).finished(), (await detailDone).finished()]);
  await page.locator('[data-tab="progress"]').click();
  await expect(page.getByRole('button', { name: 'Open Selected room' })).toBeVisible();
  await page.locator('[data-tab="sessions"]').click();
  await expect(page.locator('#summary')).toContainText('Selected room');
  await expect(page.locator('#sessionList')).not.toContainText('Stale experiment');
  await page.clock.runFor(1_600);
  await slowEvents.requested;
  await page.locator('[data-session="2"]').click();
  await expect(page.locator('#timeline')).toContainText('Selected room event');
  const eventsDone = page.waitForResponse(r => r.url().includes('/sessions/3/events?'));
  slowEvents.release();
  await (await eventsDone).finished();
  await page.locator('[data-tab="progress"]').click();
  await page.locator('[data-tab="sessions"]').click();
  await expect(page.locator('#timeline')).toContainText('Selected room event');
  await expect(page.locator('#timeline')).not.toContainText('Stale event');
});

test('deletion blocks unfinished sessions, honors cancellation and removes deleted rows after confirmation', async ({ page }) => {
  const entry = experiment('study');
  const room = { ...session(7, 'shared-room'), lifecycle: 'ended' };
  let unfinished = true;
  let deletions = 0;
  await page.route('http://dashboard.test/**', async route => {
    const path = new URL(route.request().url()).pathname;
    if (files[path]) {
      const [name, contentType] = files[path];
      return route.fulfill({ path: fileURLToPath(new URL(name, assets)), contentType });
    }
    let data = {};
    if (path === '/api/admin/experiments') data = { experiments: [entry], game: { name: 'Fixture', version: '1.0' } };
    else if (path.endsWith('/config')) data = { experiment: { ...entry, config: {} } };
    else if (path.endsWith('/sessions')) data = { sessions: deletions ? [] : [room] };
    else if (path.endsWith('/sessions/7')) data = { session: room, participants: [{ research_id: 'human-a', role: 'A', participant_kind: 'human' }] };
    else if (path.endsWith('/progress')) data = { progress: { sessions: deletions ? [] : [room], total_sessions: deletions ? 0 : 1 } };
    else if (path.endsWith('/load')) return route.fulfill({ status: 503 });
    else if (path.endsWith('/participants/human-a/deletion')) {
      if (route.request().method() === 'POST') {
        deletions += 1;
        data = { deleted: true };
      } else {
        data = { preview: { has_non_terminal_session: unfinished, session_ids: ['shared-room'], other_participant_ids: ['human-b'] } };
      }
    }
    await route.fulfill({ json: data });
  });
  await page.goto('http://dashboard.test/admin/');
  const remove = page.getByRole('button', { name: 'Delete participant data' });
  /** Capture the actual prompt at the button and deliver the operator's response. */
  async function respond(accept) {
    const pending = page.waitForEvent('dialog');
    const click = remove.click();
    const dialog = await pending;
    const message = dialog.message();
    if (accept) await dialog.accept();
    else await dialog.dismiss();
    await click;
    return message;
  }
  expect(await respond(true)).toContain('blocked');
  expect(deletions).toBe(0);
  unfinished = false;
  expect(await respond(false)).toContain('human-b');
  expect(deletions).toBe(0);
  expect(await respond(true)).toContain('shared-room');
  await expect(page.locator('[data-session="7"]')).toHaveCount(0);
  await expect(remove).toBeHidden();
  expect(deletions).toBe(1);
});

test('typed communication disables speech settings and preserves their values when saved', async ({ page }) => {
  const entry = { ...experiment('study'), prolific_setup_url: 'http://dashboard.test/study/?PROLIFIC_PID={{%PROLIFIC_PID%}}' };
  let config = {
    communication: { mode: 'typed' },
    voice: { sample_rate_hz: 24000, frame_duration_ms: 20, jitter_buffer_ms: 120, post_completion_seconds: 20 },
    transcription: { enabled: true, language: 'de', model: 'enhanced', provider: 'speechmatics' },
    speechmatics: { realtime_url: 'wss://speech.example/v2', max_delay: 2, end_of_utterance_silence_trigger: 1.2 },
    tts: { enabled: true, voice_id: 'voice-1', model: 'eleven_flash_v2_5', provider: 'elevenlabs', output_format: 'pcm_24000' },
    capacity: { max_transcription_streams: 4 }
  };
  await page.route('http://dashboard.test/**', async route => {
    const path = new URL(route.request().url()).pathname;
    if (files[path]) {
      const [name, contentType] = files[path];
      return route.fulfill({ path: fileURLToPath(new URL(name, assets)), contentType });
    }
    if (path === '/api/admin/experiments') {
      return route.fulfill({ json: { experiments: [entry], game: { name: 'Fixture', version: '1.0' } } });
    }
    if (path.endsWith('/config')) {
      if (route.request().method() === 'POST') {
        config = route.request().postDataJSON().config;
        entry.config_revision++;
      }
      return route.fulfill({ json: { experiment: { ...entry, config } } });
    }
    return route.fulfill({ json: {} });
  });
  await page.goto('http://dashboard.test/admin/');
  await page.getByRole('button', { name: 'Configuration', exact: true }).click();

  const mode = page.locator('[data-config-path="communication.mode"]');
  const speechSettings = page.getByRole('group', { name: 'Speech settings' });
  const speechControls = speechSettings.locator('input, select');
  const voiceId = speechSettings.locator('[data-config-path="tts.voice_id"]');
  await expect(mode.locator('option')).toHaveText(['Typed', 'Speech']);
  await expect(mode).toHaveValue('typed');
  await expect(speechSettings).toBeVisible();
  await expect(speechSettings).toHaveCSS('opacity', '0.6');
  await expect(speechControls.first()).toBeDisabled();
  for (const control of await speechControls.all()) await expect(control).toBeDisabled();
  await expect(voiceId).toHaveValue('voice-1');

  await mode.selectOption('speech');
  for (const control of await speechControls.all()) await expect(control).toBeEnabled();
  await voiceId.fill('voice-2');
  await mode.selectOption('typed');
  await expect(voiceId).toBeDisabled();
  const save = page.waitForResponse(response => response.url().endsWith('/config') && response.request().method() === 'POST');
  await page.getByRole('button', { name: 'Save new revision' }).click();
  await (await save).finished();
  expect(config).toMatchObject({
    communication: { mode: 'typed' },
    voice: { jitter_buffer_ms: 120 },
    transcription: { enabled: true, language: 'de' },
    speechmatics: { realtime_url: 'wss://speech.example/v2' },
    tts: { enabled: true, voice_id: 'voice-2' },
    capacity: { max_transcription_streams: 4 }
  });
  expect(config.voice).not.toHaveProperty('enabled');
  await expect(page.locator('#configRevision')).toHaveText('Revision 2');
  await mode.selectOption('speech');
  await expect(voiceId).toBeEnabled();
  await expect(voiceId).toHaveValue('voice-2');
});

test('shared hosting creates and copies pins with one communication editor', async ({ page }) => {
  const hostingAssets = new URL('../../rust-server/src/deployment/', import.meta.url);
  const revision = 'a'.repeat(64);
  const defaults = {
    communication: { mode: 'typed' },
    voice: { sample_rate_hz: 24000, frame_duration_ms: 20, jitter_buffer_ms: 100, post_completion_seconds: 20 },
    transcription: { enabled: false, language: 'en', model: 'enhanced', provider: 'speechmatics' },
    speechmatics: { realtime_url: 'wss://speech.example/v2', max_delay: 2, end_of_utterance_silence_trigger: 1.2 },
    tts: { enabled: false, base_url: 'wss://tts.example', model: 'eleven_flash_v2_5', voice_id: '', provider: 'elevenlabs', output_format: 'pcm_24000' },
    capacity: { max_transcription_streams: 32 }
  };
  const copiedConfig = structuredClone(defaults);
  copiedConfig.communication.mode = 'speech';
  copiedConfig.voice.jitter_buffer_ms = 140;
  copiedConfig.transcription.language = 'de';
  copiedConfig.tts.model = 'custom_elevenlabs_model';
  copiedConfig.tts.output_format = 'dormant_custom_format';
  copiedConfig.capacity.max_active_sessions = 17;
  copiedConfig.game = { trial: 2 };
  const experiments = [{ id: 'existing', game: 'game', revision, config: copiedConfig, status: 'inactive' }];
  await page.route('http://hosting.test/**', async route => {
    const { pathname } = new URL(route.request().url());
    if (pathname === '/admin' || pathname === '/admin/dashboard.js' || pathname === '/admin/dashboard.css') {
      const [name, contentType] = {
        '/admin': ['dashboard.html', 'text/html'],
        '/admin/dashboard.js': ['dashboard.js', 'text/javascript'],
        '/admin/dashboard.css': ['dashboard.css', 'text/css'],
      }[pathname];
      return route.fulfill({ path: fileURLToPath(new URL(name, hostingAssets)), contentType });
    }
    if (pathname === '/admin/communication-config.js') {
      return route.fulfill({ path: fileURLToPath(new URL('communication_config.mjs', assets)), contentType: 'text/javascript' });
    }
    if (pathname === '/api/hosting/auth/session') {
      return route.fulfill({ json: { username: 'researcher', role: 'member' } });
    }
    if (pathname === '/api/hosting/dashboard') {
      return route.fulfill({ json: { games: [{ id: 'game', revisions: [{ revision, version: '1.0' }] }], experiments, config_defaults: defaults } });
    }
    if (route.request().method() === 'PUT') {
      experiments.push({ id: pathname.split('/').at(-1), status: 'inactive', ...route.request().postDataJSON() });
    }
    return route.fulfill({ json: {} });
  });
  await page.goto('http://hosting.test/admin');
  await page.locator('#game-rows').getByRole('button', { name: 'Create experiment' }).click();

  const form = page.locator('#experiment-form');
  const mode = form.locator('[data-config-path="communication.mode"]');
  const speech = form.getByRole('group', { name: 'Speech settings' });
  const buffer = speech.locator('[data-config-path="voice.jitter_buffer_ms"]');
  const other = form.getByRole('textbox', { name: 'Other experiment settings (JSON)' });
  await expect(mode.locator('option')).toHaveText(['Typed', 'Speech']);
  await expect(mode).toHaveValue('typed');
  await expect(speech).toHaveCSS('opacity', '0.6');
  await expect(buffer).toBeDisabled();
  await mode.selectOption('speech');
  await expect(buffer).toBeEnabled();
  await buffer.fill('130');
  await form.getByRole('textbox', { name: 'Experiment ID' }).fill('fresh');
  await other.fill('{"game":{"trial":1}}');
  await form.getByRole('button', { name: 'Create experiment' }).click();
  await expect(page.locator('#experiment-title')).toHaveText('fresh');
  expect(experiments.at(-1).config).toMatchObject({
    communication: { mode: 'speech' },
    voice: { jitter_buffer_ms: 130 },
    game: { trial: 1 },
  });

  await page.getByRole('row').filter({ hasText: 'existing' }).getByRole('button', { name: 'Manage' }).click();
  await page.getByRole('button', { name: 'Copy configuration to new experiment' }).click();
  await expect(mode).toHaveValue('speech');
  await expect(buffer).toHaveValue('140');
  const sampleRate = speech.locator('[data-config-path="voice.sample_rate_hz"]');
  await expect(sampleRate).toHaveValue('24000');
  await expect(sampleRate).not.toBeEditable();
  await expect(speech.locator('[data-config-path="tts.output_format"]')).toHaveValue('dormant_custom_format');
  expect(JSON.parse(await other.inputValue())).toEqual({
    capacity: { max_active_sessions: 17 },
    game: { trial: 2 },
  });
  await mode.selectOption('typed');
  await expect(buffer).toBeDisabled();
  await expect(other).toBeEditable();
  await form.getByRole('textbox', { name: 'Experiment ID' }).fill('copy');
  await form.getByRole('button', { name: 'Create experiment' }).click();
  await expect(page.locator('#experiment-title')).toHaveText('copy');
  expect(experiments.at(-1).config).toMatchObject({ ...copiedConfig, communication: { mode: 'typed' } });
});
