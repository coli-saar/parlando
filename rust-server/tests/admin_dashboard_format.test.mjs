import assert from 'node:assert/strict';
import test from 'node:test';

import {
  escapeHtml,
  fmtClockTime,
  fmtGameTime,
  formatBytes,
  formatDuration,
  namedStatusLabel,
  plannedProgressLayout,
  shortSha,
  statusText,
  sessionEndCauseRows,
} from '../src/app/admin_dashboard_format.mjs';

test('dashboard formatters preserve stable presentation contracts', () => {
  assert.match(fmtClockTime('2026-09-28T17:06:03Z'), /\d{2}:\d{2}:\d{2}/);
  assert.equal(fmtGameTime(61_234), '1:01.234');
  assert.equal(formatDuration(61_000), '1m 1s');
  assert.equal(formatBytes(1_048_576), '1.0 MB');
  assert.equal(shortSha('1234567890abcdef'), '1234567890ab');
  assert.equal(statusText('technical_failure'), 'Technical failure');
});

test('planned progress keeps failed sessions outside the target', () => {
  assert.deepEqual(
    plannedProgressLayout({
      planned_sessions: 10,
      total_sessions: 9,
      ended_sessions: 7,
      end_causes: { game_completed: 4, technical_failure: 2, partner_unavailable: 1 },
    }),
    {
      planned: 10,
      total: 9,
      ended: 7,
      completed: 4,
      active: 2,
      failures: [
        { type: 'technical_failure', count: 2 },
        { type: 'partner_unavailable', count: 1 },
      ],
      insideCompleted: 4,
      insideActive: 2,
      remaining: 4,
      overflow: [
        { type: 'technical_failure', count: 2 },
        { type: 'partner_unavailable', count: 1 },
      ],
      overflowTotal: 3,
    },
  );
});

test('planned progress exposes completed and active sessions beyond 100 percent', () => {
  const completedOverflow = plannedProgressLayout({
    planned_sessions: 3,
    total_sessions: 7,
    ended_sessions: 6,
    end_causes: { game_completed: 5, participant_left: 1 },
  });
  assert.equal(completedOverflow.remaining, 0);
  assert.deepEqual(completedOverflow.overflow, [
    { type: 'game_completed', count: 2 },
    { type: 'active', count: 1 },
    { type: 'participant_left', count: 1 },
  ]);
  assert.equal(completedOverflow.overflowTotal, 4);
});

test('planned progress omits the target for absent or invalid plans', () => {
  assert.equal(plannedProgressLayout({ total_sessions: 4 }), null);
  assert.equal(plannedProgressLayout({ planned_sessions: 0, total_sessions: 4 }), null);
  assert.equal(plannedProgressLayout({ planned_sessions: 2.5, total_sessions: 4 }), null);
});

test('dashboard HTML helpers escape server-owned content', () => {
  assert.equal(escapeHtml('<script>"x" & y</script>'), '&lt;script&gt;&quot;x&quot; &amp; y&lt;/script&gt;');
  assert.equal(
    namedStatusLabel('active', '<unsafe>'),
    '<span class="status-label"><span class="status-dot active" aria-hidden="true"></span>&lt;unsafe&gt;</span>',
  );
});

test('session end cause rows order complete server-provided counts', () => {
  assert.deepEqual(
    sessionEndCauseRows({ partner_unavailable: 1, game_completed: 2, ignored: 0 }),
    [
      { type: 'game_completed', count: 2 },
      { type: 'partner_unavailable', count: 1 },
    ],
  );
});
