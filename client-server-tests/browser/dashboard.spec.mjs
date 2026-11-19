import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';

const assets = new URL('../../rust-server/src/app/', import.meta.url);

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
  const files = {
    '/admin/': ['admin_dashboard.html', 'text/html'],
    '/admin/assets/admin-dashboard.js': ['admin_dashboard.js', 'text/javascript'],
    '/admin/assets/admin-dashboard-format.js': ['admin_dashboard_format.mjs', 'text/javascript'],
    '/admin/assets/admin-dashboard-api.js': ['admin_dashboard_api.mjs', 'text/javascript'],
    '/admin/assets/admin-dashboard-state.js': ['admin_dashboard_state.js', 'text/javascript'],
    '/admin/assets/admin-dashboard.css': ['admin_dashboard.css', 'text/css']
  };
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
