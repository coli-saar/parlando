// Formats one stored or runtime timestamp for the administrator's locale.
export function fmtTime(value) {
  if (!value) return '-';
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString(undefined, {
    day: '2-digit', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit', second: '2-digit'
  });
}

// Formats a game-clock coordinate as compact minutes, seconds, and milliseconds.
export function fmtGameTime(value) {
  if (!Number.isFinite(Number(value))) return '-';
  const milliseconds = Math.round(Number(value));
  const sign = milliseconds < 0 ? '−' : '';
  const absolute = Math.abs(milliseconds);
  const minutes = Math.floor(absolute / 60000);
  const seconds = Math.floor((absolute % 60000) / 1000);
  const remainder = absolute % 1000;
  return `${sign}${minutes}:${String(seconds).padStart(2, '0')}.${String(remainder).padStart(3, '0')}`;
}

// Escapes arbitrary server data before inserting it into HTML templates.
export function escapeHtml(value) {
  return String(value ?? '').replace(/[&<>"']/g, ch => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[ch]));
}

// Returns the lifecycle status exposed directly by the server model.
export function experimentDisplayStatus(experiment) {
  return experiment.status;
}

// Converts a machine status into a compact human-readable label.
export function statusText(status) {
  const value = String(status || 'unknown').replaceAll('-', ' ').replaceAll('_', ' ');
  return value.charAt(0).toUpperCase() + value.slice(1);
}

// Renders a colored status mark with caller-supplied plain-language text.
export function namedStatusLabel(status, text, compact = false) {
  const normalized = String(status || 'unknown').toLowerCase();
  const style = normalized.replaceAll('_', '-');
  return `<span class="status-label"><span class="status-dot ${escapeHtml(style)}" aria-hidden="true"></span>${compact ? `<span class="visually-hidden">${escapeHtml(text)}</span>` : escapeHtml(text)}</span>`;
}

// Renders a colored status mark together with its machine-status label.
export function statusLabel(status, compact = false) {
  const normalized = String(status || 'unknown').toLowerCase();
  return namedStatusLabel(normalized, statusText(normalized), compact);
}

// Formats a date without adding the time when catalogue density matters.
export function fmtDate(value) {
  if (!value) return '—';
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleDateString(undefined, {
    day: '2-digit', month: 'short', year: 'numeric'
  });
}

// Formats only the clock portion of a timestamp for aligned tabular displays.
export function fmtClockTime(value) {
  if (!value) return '—';
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleTimeString(undefined, {
    hour: '2-digit', minute: '2-digit', second: '2-digit'
  });
}

// Formats an operational timestamp as an age without treating it as research activity.
export function formatAge(timestampMs) {
  if (!timestampMs) return 'never';
  const seconds = Math.max(0, Math.round((Date.now() - timestampMs) / 1000));
  if (seconds < 60) return `${seconds}s ago`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  return `${Math.floor(seconds / 3600)}h ago`;
}

// Formats an elapsed millisecond count without implying a configured reward.
export function formatDuration(milliseconds) {
  const totalSeconds = Math.floor(milliseconds / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return minutes ? `${minutes}m ${seconds}s` : `${seconds}s`;
}

// Formats byte quantities for storage and throughput labels.
export function formatBytes(bytes) {
  if (!Number.isFinite(Number(bytes))) return '-';
  const value = Number(bytes);
  if (value >= 1073741824) return `${(value / 1073741824).toFixed(1)} GB`;
  if (value >= 1048576) return `${(value / 1048576).toFixed(1)} MB`;
  if (value >= 1024) return `${(value / 1024).toFixed(1)} KB`;
  return `${value} B`;
}

// Shortens a commit identifier for compact build metadata.
export function shortSha(value) {
  if (!value) return '-';
  return String(value).slice(0, 12);
}

// Orders complete server-provided terminal-cause counts for chart rendering.
export function sessionEndCauseRows(endCauses) {
  return Object.entries(endCauses || {})
    .map(([type, count]) => ({ type, count: Number(count) }))
    .filter(row => Number.isFinite(row.count) && row.count > 0)
    .sort((left, right) => right.count - left.count || left.type.localeCompare(right.type));
}

// Allocates research-session counts between the planned target and visible overflow.
export function plannedProgressLayout(progress) {
  const planned = Number(progress?.planned_sessions);
  if (!Number.isInteger(planned) || planned <= 0) return null;
  const total = Math.max(0, Number(progress?.total_sessions) || 0);
  const ended = Math.max(0, Number(progress?.ended_sessions) || 0);
  const completed = Math.max(0, Number(progress?.end_causes?.game_completed) || 0);
  const active = Math.max(0, total - ended);
  const failures = sessionEndCauseRows(progress?.end_causes)
    .filter(row => row.type !== 'game_completed');
  const insideCompleted = Math.min(completed, planned);
  const insideActive = Math.min(active, Math.max(0, planned - insideCompleted));
  const remaining = Math.max(0, planned - insideCompleted - insideActive);
  const overflow = [
    ...(completed > insideCompleted ? [{ type: 'game_completed', count: completed - insideCompleted }] : []),
    ...(active > insideActive ? [{ type: 'active', count: active - insideActive }] : []),
    ...failures,
  ];
  return {
    planned,
    total,
    ended,
    completed,
    active,
    failures,
    insideCompleted,
    insideActive,
    remaining,
    overflow,
    overflowTotal: overflow.reduce((sum, row) => sum + row.count, 0),
  };
}

// Identifies recorded speech; agent authorship alone does not imply audio output.
export function isSpeechBundle(bundle) {
  return bundle.kind === 'transcript' || (bundle.kind === 'conversation' && bundle.origin === 'agent' && bundle.spoken === true);
}

// Uses one chronological layout; token mode only expands recorded speech entries.
export function sessionTimeline(bundles, view = 'utterances', renderBundle = timelineBundleText) {
  if (!bundles.length) return '<div class="empty">No action or message events recorded yet.</div>';
  const entries = bundles.flatMap(bundle => {
    const expand = view === 'tokens' && isSpeechBundle(bundle);
    if (expand && bundle.tokens?.length) return bundle.tokens.map((token, index) => ({ time: token.start_ms, token, bundle, index }));
    return [{ time: bundle.utterance_timing?.start_ms ?? bundle.game_time_ms, bundle, index: 0, unavailable: expand }];
  });
  entries.sort((a, b) => (a.time ?? Number.MAX_SAFE_INTEGER) - (b.time ?? Number.MAX_SAFE_INTEGER)
    || a.bundle.first_index - b.bundle.first_index || a.index - b.index);
  const rows = [];
  for (const entry of entries) {
    const message = ['transcript', 'conversation'].includes(entry.bundle.kind);
    const previous = rows.at(-1);
    if (message && previous?.message && previous.time === entry.time) previous.entries.push(entry);
    else rows.push({ time: entry.time, message, entries: [entry] });
  }
  return rows.map(row => `<div class="session-timeline-row">
    <span class="game-time" title="Game time">${row.time == null ? '—' : escapeHtml(fmtGameTime(row.time))}</span>
    <div class="session-timeline-content">${row.entries.map(entry => `<div class="session-timeline-entry">${roleBadge(entry.bundle.role)}<div class="timeline-entry-content">${row.message ? conversationBubble(entry) : renderBundle(entry.bundle)}</div></div>`).join('')}</div>
  </div>`).join('');
}

// Provides escaped event text for consumers without the dashboard's full card renderer.
function timelineBundleText(bundle) {
  return escapeHtml(bundle.text || bundle.title || '');
}

// Renders tokens and whole messages with the same edge-aligned speaker bubble.
function conversationBubble({ token, bundle, unavailable }) {
  const timing = token || bundle.utterance_timing;
  const duration = timing?.end_ms != null && timing?.start_ms != null ? timing.end_ms - timing.start_ms : null;
  const parent = `Utterance #${bundle.first_index}: ${bundle.text || ''}`;
  const title = token ? `${token.kind} · ${fmtGameTime(token.start_ms)}–${fmtGameTime(token.end_ms)} · ${duration} ms${token.confidence == null ? '' : ` · confidence ${token.confidence}`} · ${parent}` : parent;
  const end = duration == null ? '' : `→ ${escapeHtml(fmtGameTime(timing.end_ms))} · ${duration} ms`;
  return `<article class="speech-bubble speaker-${bundle.role === 'B' ? 'b' : 'a'}${token ? ' speech-token' : ''}${token?.kind === 'punctuation' ? ' punctuation' : ''}"${token ? ' tabindex="0"' : ''} title="${escapeHtml(title)}" aria-label="${escapeHtml(title)}">
    <div>${escapeHtml(token ? token.text : bundle.text || '')}</div>
    <div class="muted small">${end}${unavailable && end ? ' · ' : ''}${token?.kind === 'punctuation' ? ' · punctuation' : ''}${unavailable ? 'Token timings unavailable' : ''}</div>
  </article>`;
}

// Uses the same participant colors in the timeline gutter and participant cards.
export function roleBadge(role) {
  const normalized = role === 'A' || role === 'B' ? role : '';
  if (!normalized) return '<span class="role-badge role-system">SYS</span>';
  return `<span class="role-badge role-${normalized.toLowerCase()}">${escapeHtml(normalized)}</span>`;
}
