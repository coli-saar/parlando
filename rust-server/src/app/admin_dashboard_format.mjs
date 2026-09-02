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
