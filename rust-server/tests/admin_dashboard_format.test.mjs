import assert from 'node:assert/strict';
import test from 'node:test';

import {
  escapeHtml,
  fmtGameTime,
  formatBytes,
  formatDuration,
  namedStatusLabel,
  shortSha,
  statusText,
  sessionEndCauseRows,
} from '../src/app/admin_dashboard_format.mjs';

test('dashboard formatters preserve stable presentation contracts', () => {
  assert.equal(fmtGameTime(61_234), '1:01.234');
  assert.equal(formatDuration(61_000), '1m 1s');
  assert.equal(formatBytes(1_048_576), '1.0 MB');
  assert.equal(shortSha('1234567890abcdef'), '1234567890ab');
  assert.equal(statusText('technical_failure'), 'Technical failure');
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
