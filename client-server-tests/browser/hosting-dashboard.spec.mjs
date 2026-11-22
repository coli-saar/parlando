import { fileURLToPath } from 'node:url';
import { expect, test } from '@playwright/test';

const assets = new URL('../../rust-server/src/', import.meta.url);
const files = {
  '/admin': ['deployment/dashboard.html', 'text/html'],
  '/admin/dashboard.js': ['deployment/dashboard.js', 'text/javascript'],
  '/admin/dashboard.css': ['deployment/dashboard.css', 'text/css'],
  '/admin/assets/palette.css': ['../brand/palette.css', 'text/css'],
  '/admin/assets/parlando-wordmark.svg': ['../brand/parlando-wordmark.svg', 'image/svg+xml'],
  '/admin/assets/parlando-mark.svg': ['../brand/parlando-mark.svg', 'image/svg+xml'],
};

// Resolves hosting assets to their production files.
function serveDashboardFile(route, path) {
  const file = files[path];
  if (!file) return undefined;
  const [name, contentType] = file;
  return route.fulfill({ path: fileURLToPath(new URL(name, assets)), contentType });
}

test('plain admin offers setup until an administrator is created', async ({ page }) => {
  const credentials = { username: 'local-admin', password: 'local-admin-password' };
  const rejectedPassword = 'short';
  const setupRequests = [];
  const loginRequests = [];
  let signedIn = false;
  let setupAvailable = true;
  await page.route('http://hosting.test/**', async route => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const asset = serveDashboardFile(route, path);
    if (asset) return asset;
    if (path === '/api/hosting/auth/session') {
      if (!signedIn) return route.fulfill({ status: 401 });
      return route.fulfill({ json: { id: 1, username: credentials.username, role: 'operator', csrf_token: 'test-csrf' } });
    }
    if (path === '/api/hosting/auth/setup' && request.method() === 'GET') {
      return route.fulfill({ json: { available: setupAvailable } });
    }
    if (path === '/api/hosting/auth/setup' || path === '/api/hosting/auth/login') {
      const record = { method: request.method(), bearer: request.headers().authorization, body: request.postDataJSON() };
      if (path.endsWith('/setup')) {
        setupRequests.push(record);
        if (setupRequests.length === 1) return route.fulfill({ status: 400, body: 'Choose a longer password' });
        setupAvailable = false;
        return route.fulfill({ status: 201 });
      }
      loginRequests.push(record);
      signedIn = true;
      return route.fulfill({ json: {} });
    }
    if (path === '/api/hosting/dashboard') {
      return route.fulfill({ json: { games: [] } });
    }
    if (path === '/api/hosting/accounts') return route.fulfill({ json: { accounts: [] } });
    if (path === '/api/hosting/audit') return route.fulfill({ json: { events: [] } });
    await route.fulfill({ status: 404 });
  });

  await page.goto('http://hosting.test/admin');
  const wordmark = page.getByRole('img', { name: 'Parlando', exact: true });
  await expect(wordmark).toHaveJSProperty('complete', true);
  await expect(wordmark).not.toHaveJSProperty('naturalWidth', 0);
  await expect(page.getByRole('heading', { name: 'Create your administrator account' })).toBeVisible();
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Create your administrator account' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Sign in' })).toBeHidden();
  const form = page.locator('#setup-form');
  await form.getByLabel('Username').fill(credentials.username);
  await form.getByLabel('Password', { exact: true }).fill(rejectedPassword);
  await form.getByLabel('Confirm password').fill('mismatch');
  const submit = form.getByRole('button', { name: 'Create administrator' });
  await submit.click();
  await expect(page.getByRole('status')).toHaveText('Passwords do not match.');
  expect(setupRequests).toHaveLength(0);
  await form.getByLabel('Confirm password').fill(rejectedPassword);
  await submit.click();
  await expect(page.getByRole('status')).toHaveText('Choose a longer password');
  expect(loginRequests).toHaveLength(0);
  await form.getByLabel('Username').fill(credentials.username);
  await form.getByLabel('Password', { exact: true }).fill(credentials.password);
  await form.getByLabel('Confirm password').fill(credentials.password);
  await submit.click();
  await expect(page.getByRole('heading', { name: 'Games', exact: true })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Users', exact: true })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Create your administrator account' })).toBeHidden();
  await expect(page.getByRole('status')).toHaveText('Administrator account created.');
  expect(setupRequests).toEqual([
    { method: 'POST', bearer: undefined, body: { username: credentials.username, password: rejectedPassword } },
    { method: 'POST', bearer: undefined, body: credentials },
  ]);
  expect(loginRequests).toEqual([{ method: 'POST', bearer: undefined, body: credentials }]);
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Games', exact: true })).toBeVisible();
  signedIn = false;
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
  const login = page.locator('#login');
  await login.getByLabel('Username').fill(credentials.username);
  await login.getByLabel('Password').fill(credentials.password);
  await login.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { name: 'Games', exact: true })).toBeVisible();
});

test('setup switches to sign-in when another visitor creates the administrator first', async ({ page }) => {
  let setupAvailable = true;
  await page.route('http://hosting.test/**', async route => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const asset = serveDashboardFile(route, path);
    if (asset) return asset;
    if (path === '/api/hosting/auth/setup') {
      if (request.method() === 'GET') {
        return route.fulfill({ json: { available: setupAvailable } });
      }
      setupAvailable = false;
      return route.fulfill({ status: 409, body: 'The dashboard already has an account.' });
    }
    await route.fulfill({ status: 401 });
  });
  await page.goto('http://hosting.test/admin');
  const form = page.locator('#setup-form');
  await form.getByLabel('Username').fill('administrator');
  await form.getByLabel('Password', { exact: true }).fill('local-admin-password');
  await form.getByLabel('Confirm password').fill('local-admin-password');
  await form.getByRole('button', { name: 'Create administrator' }).click();
  await expect(page.getByRole('status')).toHaveText('The dashboard already has an account.');
  await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Create your administrator account' })).toBeHidden();
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
});

test('sign-in resumes a local game dashboard address and ignores external destinations', async ({ page }) => {
  let signedIn = false;
  await page.route('http://hosting.test/**', async route => {
    const path = new URL(route.request().url()).pathname;
    const asset = serveDashboardFile(route, path);
    if (asset) return asset;
    if (path === '/api/hosting/auth/setup') return route.fulfill({ json: { available: false } });
    if (path === '/api/hosting/auth/session') {
      if (!signedIn) return route.fulfill({ status: 401 });
      return route.fulfill({ json: { id: 1, username: 'researcher', role: 'experimenter', csrf_token: 'test-csrf' } });
    }
    if (path === '/api/hosting/auth/login') {
      signedIn = true;
      return route.fulfill({ json: {} });
    }
    if (path === '/api/hosting/dashboard') return route.fulfill({ json: { games: [] } });
    if (path === '/example' && signedIn) return route.fulfill({ body: 'Game dashboard destination' });
    await route.fulfill({ status: 404 });
  });

  await page.goto('http://hosting.test/admin?next=/example');
  const login = page.locator('#login');
  await login.getByLabel('Username').fill('researcher');
  await login.getByLabel('Password').fill('researcher-password');
  await login.getByRole('button', { name: 'Sign in' }).click();
  await expect(page).toHaveURL('http://hosting.test/example');

  await page.goto('http://hosting.test/admin?next=%2F%2Fexample.com');
  await expect(page.getByRole('heading', { name: 'Games', exact: true })).toBeVisible();
  await expect(page).toHaveURL('http://hosting.test/admin?next=%2F%2Fexample.com');
});

test('Games deploys and updates packages, manages activation, and opens the game dashboard', async ({ page }) => {
  const id = 'example';
  const name = 'Example Game';
  const first = { id, name, version: '1.0.0', revision: 'a'.repeat(64), active: false };
  const second = { ...first, version: '2.0.0', revision: 'b'.repeat(64) };
  const zip = Buffer.from([0x50, 0x4b, 0x03, 0x04, 0, 255]);
  const uploads = [];
  const lifecycle = [];
  let game;
  await page.route('http://hosting.test/**', async route => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const asset = serveDashboardFile(route, path);
    if (asset) return asset;
    if (path === '/api/hosting/auth/session') {
      return route.fulfill({ json: { id: 1, username: 'researcher', role: 'experimenter', csrf_token: 'test-csrf' } });
    }
    if (path === '/api/hosting/dashboard') {
      return route.fulfill({ json: { games: game ? [game] : [] } });
    }
    const csrf = request.headers()['x-csrf-token'];
    if ((path === '/api/hosting/packages' || path === '/api/hosting/packages/example') && request.method() === 'PUT') {
      uploads.push({ path, csrf, contentType: request.headers()['content-type'], body: request.postDataBuffer() });
      if (uploads.length === 1) return route.fulfill({ status: 400, body: 'Invalid game ZIP' });
      game = path.endsWith('/example') ? second : first;
      return route.fulfill({ json: { ...game, game: id } });
    }
    if (path.startsWith('/api/hosting/games/example/') && request.method() === 'POST') {
      lifecycle.push({ path, csrf });
      game = { ...game, active: path.endsWith('/activate') };
      return route.fulfill({ json: { active: game.active } });
    }
    if (path === '/example') return route.fulfill({ body: 'Game dashboard destination' });
    await route.fulfill({ status: 404 });
  });

  await page.goto('http://hosting.test/admin');
  await expect(page.getByText('No games have been deployed.')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Users' })).toBeHidden();
  await page.getByText('Upload a game or agent', { exact: true }).click();
  const input = page.getByLabel('Build ZIP');
  const upload = page.getByRole('button', { name: 'Upload package' });
  const notice = page.getByRole('status');
  await expect(input).toHaveAttribute('accept', '.zip');
  await input.setInputFiles({ name: 'game.yaml', mimeType: 'text/yaml', buffer: Buffer.from('name: test') });
  await upload.click();
  await expect(notice).toHaveText('Choose a game or agent ZIP file.');
  expect(uploads).toHaveLength(0);

  const file = { name: 'example.zip', mimeType: 'application/octet-stream', buffer: zip };
  await input.setInputFiles(file);
  await upload.click();
  await expect(notice).toHaveText('Invalid game ZIP');
  await expect(input).toHaveValue(/example\.zip$/);
  await upload.click();
  await expect(notice).toHaveText('Deployed Example Game 1.0.0 (aaaaaaaaaaaa).');

  await page.getByRole('button', { name: 'Update example' }).click();
  await expect(input).toBeFocused();
  await expect(page.getByText('Updating Example Game (example).')).toBeVisible();
  await input.setInputFiles(file);
  await upload.click();
  await expect(page.getByRole('cell', { name: '2.0.0' })).toBeVisible();
  expect(uploads).toEqual([
    '/api/hosting/packages',
    '/api/hosting/packages',
    '/api/hosting/packages/example',
  ].map(path => ({ path, csrf: 'test-csrf', contentType: 'application/zip', body: zip })));

  await page.getByRole('button', { name: 'Activate example' }).click();
  await expect(page.getByRole('cell', { name: 'Active', exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Deactivate example' }).click();
  await expect(page.getByRole('cell', { name: 'Inactive', exact: true })).toBeVisible();
  expect(lifecycle).toEqual([
    { path: '/api/hosting/games/example/activate', csrf: 'test-csrf' },
    { path: '/api/hosting/games/example/deactivate', csrf: 'test-csrf' },
  ]);

  await page.getByRole('link', { name: 'example', exact: true }).click();
  await expect(page).toHaveURL('http://hosting.test/example');
});

test('Users loads accounts on entry and audit history on demand', async ({ page }) => {
  const accountRequests = [];
  const auditRequests = [];
  const accountCreations = [];
  const accounts = [{ id: 2, username: 'researcher', role: 'experimenter', disabled: false }];
  await page.route('http://hosting.test/**', async route => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const asset = serveDashboardFile(route, path);
    if (asset) return asset;
    if (path === '/api/hosting/auth/session') {
      return route.fulfill({ json: { id: 1, username: 'administrator', role: 'operator', csrf_token: 'test-csrf' } });
    }
    if (path === '/api/hosting/dashboard') {
      return route.fulfill({ json: { games: [] } });
    }
    if (path === '/api/hosting/accounts') {
      accountRequests.push(request.method());
      if (request.method() === 'POST') {
        const { username, password, role } = request.postDataJSON();
        accountCreations.push({ username, password, role, csrf: request.headers()['x-csrf-token'] });
        accounts.push({ id: 3, username, role, disabled: false });
        return route.fulfill({ status: 201 });
      }
      return route.fulfill({ json: { accounts } });
    }
    if (path === '/api/hosting/audit') {
      auditRequests.push(request.method());
      return route.fulfill({ json: { events: [
        { created_at: 1, actor: 'host-operator-token', operation: 'create_account', target: 'researcher', outcome: 'success' },
      ] } });
    }
    await route.fulfill({ status: 404 });
  });

  await page.goto('http://hosting.test/admin');
  await expect(page.getByRole('heading', { name: 'Games', exact: true })).toBeVisible();
  expect(accountRequests).toHaveLength(0);
  expect(auditRequests).toHaveLength(0);
  await page.getByRole('link', { name: 'Users', exact: true }).click();
  await expect(page).toHaveURL('http://hosting.test/admin#users');
  await expect(page.getByRole('heading', { name: 'Users', exact: true })).toBeVisible();
  await expect(page.locator('#account-rows').getByText('researcher', { exact: true })).toBeVisible();
  await expect(page.locator('#games')).toBeHidden();
  expect(accountRequests).toEqual(['GET']);
  expect(auditRequests).toHaveLength(0);
  await page.getByText('Create an account', { exact: true }).click();
  const form = page.locator('#account-form');
  await form.getByLabel('Username').fill('second-researcher');
  await form.getByLabel('Initial password').fill('second-researcher-password');
  await form.getByRole('button', { name: 'Create account' }).click();
  await expect(page.locator('#account-rows').getByText('second-researcher', { exact: true })).toBeVisible();
  expect(accountCreations).toEqual([
    { username: 'second-researcher', password: 'second-researcher-password', role: 'experimenter', csrf: 'test-csrf' },
  ]);
  expect(auditRequests).toHaveLength(0);
  await page.getByText('Audit history', { exact: true }).click();
  await expect(page.locator('#audit-rows').getByText('create_account', { exact: true })).toBeVisible();
  expect(auditRequests).toEqual(['GET']);
});
