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

test('progress loads during initialization and when its tab opens', () => {
  const source = readFileSync(
    new URL('../src/app/admin_dashboard.js', import.meta.url),
    'utf8',
  );
  assert.match(source, /Promise\.all\(\[loadSessions\(\), loadProgress\(\), loadLoad\(\)\]\)/);
  assert.match(source, /state\.activeTab === 'progress'\) loadProgress\(\)/);
  assert.match(source, /state\.sessionProgress\?\.sessions \|\| \[\]/);
  assert.match(source, /session\.purpose === 'testing'/);
  assert.match(source, /setInterval\([\s\S]*state\.activeTab === 'progress' \? loadProgress\(\)/);
  assert.match(source, /if \(signature === state\.sessionProgressSignature\) return false;/);
  assert.match(source, /if \(initialLoad\) sessionEndChart\.innerHTML = '<div class="empty">Loading…<\/div>';/);
  assert.doesNotMatch(source, /async function loadProgress\(\)[\s\S]*?state\.sessionProgress = null;[\s\S]*?\/\/ Refreshes the linked Prolific study/);
  assert.match(source, /data-progress-session/);
  assert.match(source, /state\.activeTab = 'sessions';[\s\S]*selectSession/);
  assert.match(source, /<button class="progress-session-row/);
  assert.doesNotMatch(source, /class="progress-session-link"/);
  assert.doesNotMatch(source, /presentation\.label\}\$\{session\.purpose === 'testing'/);
  assert.match(source, /session-wait-detail[^`]*Unsuccessful wait <strong>/);
  assert.doesNotMatch(source, /<dt>Unsuccessful wait<\/dt>/);
  assert.doesNotMatch(source, /Prolific places:/);
  assert.match(source, /session\.lifecycle === 'ended' \? sessionEndPresentation\(session\)/);
  assert.match(source, /namedStatusLabel\(terminal\.status, terminal\.label, true\)/);
  assert.match(source, /class="progress-session-date"[^\n]*fmtDate\(session\.created_at\)/);
  assert.match(source, /class="progress-session-time"[^\n]*fmtClockTime\(session\.created_at\)/);
});

test('dashboard utility classes cannot override the hidden attribute', () => {
  const css = readFileSync(
    new URL('../src/app/admin_dashboard.css', import.meta.url),
    'utf8',
  );
  assert.match(css, /\[hidden\]\s*\{\s*display:\s*none\s*!important;/);
  assert.match(css, /\.planned-session-target::after\s*\{[^}]*border:\s*2px solid/);
  assert.match(css, /\.session-end-segment\.overflow\s*\{[^}]*height:\s*18px/);
  assert.match(css, /\.planned-session-target \+ \.session-end-segment\.overflow\s*\{[^}]*margin-left:\s*6px/);
  assert.match(css, /\.progress-session-log\s*\{[^}]*column-gap:\s*20px;[^}]*grid-template-columns:\s*12px max-content max-content minmax\(180px, 1fr\) auto/);
  assert.match(css, /\.progress-session-row\s*\{[^}]*column-gap:\s*20px;[^}]*grid-column:\s*1 \/ -1;[^}]*grid-template-columns:\s*subgrid/);
  assert.match(css, /\.progress-session-row strong\s*\{[^}]*color:\s*var\(--text\)/);
  assert.match(css, /\.progress-header\s*\{[^}]*max-width:\s*900px/);
  assert.match(css, /--session-completed:\s*#2f9e62/);
  assert.match(css, /\.status-dot\.completed\s*\{\s*background:\s*var\(--session-completed\)/);
  assert.match(css, /\.status-dot\.connection-lost\s*\{\s*background:\s*var\(--session-connection-lost\)/);
  assert.match(css, /\.status-dot\.no-partner\s*\{\s*background:\s*var\(--session-no-partner\)/);
  assert.match(css, /\.status-dot\.technical-failure\s*\{\s*background:\s*var\(--session-technical-failure\)/);
});

test('Prolific target refresh is a labeled icon aligned with the chart', () => {
  const html = readFileSync(
    new URL('../src/app/admin_dashboard.html', import.meta.url),
    'utf8',
  );
  assert.match(html, /id="refreshProgress"[^>]*aria-label="Refresh session target from Prolific"/);
  assert.match(html, /id="refreshProgress"[^>]*data-quick-tooltip="Refresh session target from Prolific"/);
  assert.match(html, /<use href="#icon-refresh"><\/use>/);
  assert.doesNotMatch(html, />Refresh Prolific<\/button>/);
});
