import { state } from './admin-dashboard-state.js';
import { escapeHtml, experimentDisplayStatus, fmtDate, fmtGameTime, fmtTime, formatAge, formatBytes, formatDuration, namedStatusLabel, shortSha, statusLabel, statusText } from './admin-dashboard-format.js';
import { adminFetch, experimentRuntimeApi } from './admin-dashboard-api.js';
const experimentList = document.getElementById('experimentList');
const menuButton = document.getElementById('menuButton');
const experimentHeader = document.getElementById('experimentHeader');
const experimentStats = document.getElementById('experimentStats');
const experimentWorkspaceHeader = document.getElementById('experimentWorkspaceHeader');
const emptyExperimentWorkspace = document.getElementById('emptyExperimentWorkspace');
const experimentWorkspaceTabs = document.getElementById('experimentWorkspaceTabs');
const sessionList = document.getElementById('sessionList');
const sessionDetail = document.getElementById('sessionDetail');
const summary = document.getElementById('summary');
const timeline = document.getElementById('timeline');
const liveStatus = document.getElementById('liveStatus');
const reproducibility = document.getElementById('reproducibility');
const gameName = document.getElementById('gameName');
const gameVersion = document.getElementById('gameVersion');
const gameGit = document.getElementById('gameGit');
const gameBuild = document.getElementById('gameBuild');
const experimentLabel = document.getElementById('experimentLabel');
const experimentStatusFilter = document.getElementById('experimentStatusFilter');
const sessionLifecycleFilter = document.getElementById('sessionLifecycleFilter');
const showHousekeeping = document.getElementById('showHousekeeping');
const showLogs = document.getElementById('showLogs');
const configForm = document.getElementById('configForm');
const configRevision = document.getElementById('configRevision');
const revisionHistory = document.getElementById('revisionHistory');
const saveConfiguration = document.getElementById('saveConfiguration');
const institutionInput = document.getElementById('institutionInput');
const adminAllowedIpRangesInput = document.getElementById('adminAllowedIpRangesInput');
const speechmaticsRealtimeUrlInput = document.getElementById('speechmaticsRealtimeUrlInput');
const ttsBaseUrlInput = document.getElementById('ttsBaseUrlInput');
const prolificApiBaseUrlInput = document.getElementById('prolificApiBaseUrlInput');
const loadOverview = document.getElementById('loadOverview');
const capacityList = document.getElementById('capacityList');
const trafficMetrics = document.getElementById('trafficMetrics');
const loadUpdated = document.getElementById('loadUpdated');
const loadChart = document.getElementById('loadChart');
const livenessTable = document.getElementById('livenessTable');
const privacyContent = document.getElementById('privacyContent');
const notesEditor = document.getElementById('notesEditor');
const notesPreview = document.getElementById('notesPreview');
const notesSaveState = document.getElementById('notesSaveState');
const createExperimentDialog = document.getElementById('createExperimentDialog');
const quickTooltip = document.getElementById('quickTooltip');
const scopeButtons = Array.from(document.querySelectorAll('[data-scope]'));
const scopePanels = Array.from(document.querySelectorAll('[data-scope-panel]'));
const tabButtons = Array.from(document.querySelectorAll('.tab'));
const tabPanels = {
  sessions: document.getElementById('sessionsPanel'),
  notes: document.getElementById('notesPanel'),
  export: document.getElementById('exportPanel'),
  details: document.getElementById('detailsPanel'),
  privacy: document.getElementById('privacyPanel')
};

// Converts a native title into dashboard tooltip content before the browser's delayed popup.
function quickTooltipTarget(element) {
  const target = element?.closest?.('[title], [data-quick-tooltip]');
  if (!target) return null;
  if (target.hasAttribute('title')) {
    target.dataset.quickTooltip = target.getAttribute('title');
    target.removeAttribute('title');
  }
  return target.dataset.quickTooltip ? target : null;
}

// Positions the shared tooltip next to its trigger without leaving the viewport.
function positionQuickTooltip(target) {
  const margin = 8;
  const gap = 6;
  const trigger = target.getBoundingClientRect();
  const tooltip = quickTooltip.getBoundingClientRect();
  const left = Math.min(
    window.innerWidth - tooltip.width - margin,
    Math.max(margin, trigger.left + (trigger.width - tooltip.width) / 2)
  );
  let top = trigger.bottom + gap;
  if (top + tooltip.height > window.innerHeight - margin) top = trigger.top - tooltip.height - gap;
  quickTooltip.style.left = `${Math.round(left)}px`;
  quickTooltip.style.top = `${Math.max(margin, Math.round(top))}px`;
}

// Shows one delegated tooltip after a short, consistent hover or focus delay.
function showQuickTooltip(target) {
  clearTimeout(state.quickTooltipTimer);
  state.quickTooltipTarget = target;
  state.quickTooltipTimer = setTimeout(() => {
    if (!target.isConnected || state.quickTooltipTarget !== target) return;
    quickTooltip.textContent = target.dataset.quickTooltip;
    quickTooltip.hidden = false;
    positionQuickTooltip(target);
  }, 90);
}

// Hides the delegated tooltip and cancels any pending appearance.
function hideQuickTooltip() {
  clearTimeout(state.quickTooltipTimer);
  state.quickTooltipTimer = null;
  state.quickTooltipTarget = null;
  quickTooltip.hidden = true;
}

// Highlights common GitHub-flavored Markdown source tokens without interpreting raw HTML.
function highlightMarkdownSource(source) {
  const inline = value => {
    const pattern = /(`[^`\n]*`|\[[^\]\n]+\]\([^)\n]+\)|\*\*[^*\n]+\*\*|__[^_\n]+__|~~[^~\n]+~~|[*_][^*_\n]+[*_])/g;
    let result = '';
    let offset = 0;
    for (const match of value.matchAll(pattern)) {
      result += escapeHtml(value.slice(offset, match.index));
      const token = match[0];
      const kind = token.startsWith('`') ? 'code' : token.startsWith('[') ? 'link' : 'emphasis';
      result += `<span class="markdown-token-${kind}">${escapeHtml(token)}</span>`;
      offset = match.index + token.length;
    }
    return result + escapeHtml(value.slice(offset));
  };
  let fenced = false;
  return source.split('\n').map(line => {
    if (/^\s*```/.test(line)) {
      fenced = !fenced;
      return `<span class="markdown-token-marker">${escapeHtml(line)}</span>`;
    }
    if (fenced) return `<span class="markdown-token-code">${escapeHtml(line)}</span>`;
    const prefix = line.match(/^(\s{0,3}(?:#{1,6}\s+|>\s+|[-+*]\s+(?:\[[ xX]\]\s+)?|\d+\.\s+))/);
    if (!prefix) return inline(line);
    const heading = prefix[0].includes('#');
    return `<span class="markdown-token-marker">${escapeHtml(prefix[0])}</span><span class="${heading ? 'markdown-token-heading' : ''}">${inline(line.slice(prefix[0].length))}</span>`;
  }).join('\n');
}

// Builds a local source editor with line numbers, Markdown highlighting, and indentation.
function renderSourceEditor(container, id, value, label, onChange) {
  container.innerHTML = `<div class="source-editor markdown-source-editor"><pre class="source-editor-lines" aria-hidden="true">1</pre><div class="source-editor-stage"><pre class="source-editor-highlight" aria-hidden="true"></pre><textarea id="${id}" aria-label="${escapeHtml(label)}" spellcheck="true"></textarea></div></div>`;
  const input = container.querySelector('textarea');
  const lines = container.querySelector('.source-editor-lines');
  const highlight = container.querySelector('.source-editor-highlight');
  input.value = value;
  const synchronize = () => {
    lines.textContent = Array.from({ length: input.value.split('\n').length }, (_, index) => index + 1).join('\n');
    highlight.innerHTML = highlightMarkdownSource(input.value) || ' ';
    highlight.scrollTop = input.scrollTop;
    highlight.scrollLeft = input.scrollLeft;
    lines.scrollTop = input.scrollTop;
  };
  input.addEventListener('input', () => { synchronize(); onChange(input.value); });
  input.addEventListener('scroll', synchronize);
  input.addEventListener('keydown', event => {
    if (event.key !== 'Tab') return;
    event.preventDefault();
    const start = input.selectionStart;
    const end = input.selectionEnd;
    input.setRangeText('  ', start, end, 'end');
    input.dispatchEvent(new Event('input'));
  });
  input.synchronizeSourceEditor = synchronize;
  synchronize();
}

// Upgrades a YAML textarea in place while preserving its stable form identifier and fallback.
function upgradeYamlEditor() {
  const textarea = document.getElementById('gameYaml');
  if (!textarea) return;
  const holder = document.createElement('div');
  holder.className = 'source-editor';
  holder.innerHTML = `<pre class="source-editor-lines" aria-hidden="true">1</pre>`;
  const lines = holder.firstElementChild;
  textarea.parentNode.insertBefore(holder, textarea);
  holder.appendChild(textarea);
  const updateLines = () => { lines.textContent = Array.from({ length: textarea.value.split('\n').length }, (_, index) => index + 1).join('\n'); };
  textarea.addEventListener('input', updateLines);
  textarea.addEventListener('keydown', event => {
    if (event.key !== 'Tab') return;
    event.preventDefault();
    textarea.setRangeText('  ', textarea.selectionStart, textarea.selectionEnd, 'end');
    textarea.dispatchEvent(new Event('input'));
  });
  updateLines();
}

// Renders conservative Markdown from escaped source without allowing active HTML or URLs.
function safeMarkdown(source) {
  const safeLink = (_match, label, url) => {
    try {
      const parsed = new URL(url, window.location.origin);
      if (!['http:', 'https:', 'mailto:'].includes(parsed.protocol)) return escapeHtml(label);
      return `<a href="${escapeHtml(parsed.href)}" rel="noopener noreferrer" target="_blank">${escapeHtml(label)}</a>`;
    } catch (_) { return escapeHtml(label); }
  };
  const inline = value => value
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
    .replace(/__([^_]+)__/g, '<strong>$1</strong>')
    .replace(/~~([^~]+)~~/g, '<del>$1</del>')
    .replace(/\[([^\]]+)\]\(([^)]+)\)/g, safeLink);
  return escapeHtml(source).split(/\n{2,}/).map(block => {
    if (/^```/.test(block) && /```$/.test(block)) return `<pre><code>${block.replace(/^```[^\n]*\n?/, '').replace(/\n?```$/, '')}</code></pre>`;
    const lines = block.split('\n');
    if (lines.every(line => /^[-+*]\s+/.test(line))) return `<ul>${lines.map(line => {
      const item = line.replace(/^[-+*]\s+/, '');
      const task = item.match(/^\[([ xX])\]\s+(.*)$/);
      return task ? `<li><input type="checkbox" disabled ${task[1].toLowerCase() === 'x' ? 'checked' : ''}> ${inline(task[2])}</li>` : `<li>${inline(item)}</li>`;
    }).join('')}</ul>`;
    if (lines.every(line => /^\d+\.\s+/.test(line))) return `<ol>${lines.map(line => `<li>${inline(line.replace(/^\d+\.\s+/, ''))}</li>`).join('')}</ol>`;
    if (lines.every(line => /^&gt;\s?/.test(line))) return `<blockquote>${lines.map(line => inline(line.replace(/^&gt;\s?/, ''))).join('<br>')}</blockquote>`;
    const linked = inline(block);
    if (linked.startsWith('### ')) return `<h3>${linked.slice(4)}</h3>`;
    if (linked.startsWith('## ')) return `<h2>${linked.slice(3)}</h2>`;
    if (linked.startsWith('# ')) return `<h1>${linked.slice(2)}</h1>`;
    return `<p>${linked.replace(/\n/g, '<br>')}</p>`;
  }).join('') || '<p class="muted">No notes yet.</p>';
}

// Applies the selected Notes mode without rebuilding the editor or disturbing focus.
function setNotesMode(mode) {
  state.notesMode = mode;
  notesPreview.hidden = mode !== 'preview';
  notesEditor.hidden = mode !== 'write';
  document.getElementById('notesWriteMode').setAttribute('aria-pressed', String(mode === 'write'));
  document.getElementById('notesPreviewMode').setAttribute('aria-pressed', String(mode === 'preview'));
  if (mode === 'preview') notesPreview.innerHTML = safeMarkdown(state.notesSource);
}

// Synchronizes Notes with selection changes while preserving the active editor across polling.
function renderNotes() {
  const selectionChanged = state.notesExperimentId !== state.experiment?.experiment_id;
  if (selectionChanged) {
    state.notesExperimentId = state.experiment?.experiment_id || null;
    state.notesSource = state.experiment?.notes || '';
    state.notesDirty = false;
  }
  let input = document.getElementById('experimentNotesSource');
  if (selectionChanged || !input) {
    renderSourceEditor(notesEditor, 'experimentNotesSource', state.notesSource, 'Experiment notes in Markdown', value => {
      state.notesSource = value;
      state.notesDirty = value !== (state.experiment?.notes || '');
      notesSaveState.textContent = state.notesDirty ? 'Unsaved changes' : 'Saved';
      if (state.notesMode === 'preview') notesPreview.innerHTML = safeMarkdown(value);
    });
  } else if (!state.notesDirty && document.activeElement !== input && input.value !== (state.experiment?.notes || '')) {
    state.notesSource = state.experiment?.notes || '';
    input.value = state.notesSource;
    input.synchronizeSourceEditor();
  }
  notesPreview.innerHTML = safeMarkdown(state.notesSource);
  setNotesMode(state.notesMode);
  notesSaveState.textContent = state.notesDirty ? 'Unsaved changes' : 'Saved';
}

// Renders one symbol from the dashboard's bundled SVG icon sprite.
function icon(name, className = '') {
  return `<svg class="icon ${escapeHtml(className)}" aria-hidden="true"><use href="#icon-${escapeHtml(name)}"></use></svg>`;
}

// Closes every custom status menu except an optional control being opened.
function closeStatusFilters(except = null) {
  document.querySelectorAll('[data-status-filter]').forEach(control => {
    if (control === except) return;
    control.querySelector('.status-filter-menu').hidden = true;
    control.querySelector('.status-filter-trigger').setAttribute('aria-expanded', 'false');
  });
}

// Applies one custom status option and notifies the existing filtering logic.
function selectStatusFilterOption(control, option, notify = true) {
  const input = control.querySelector('input[type="hidden"]');
  const current = control.querySelector('.status-filter-current');
  input.value = option.dataset.value;
  current.replaceChildren(...Array.from(option.childNodes, node => node.cloneNode(true)));
  control.querySelectorAll('[role="option"]').forEach(candidate => {
    candidate.setAttribute('aria-selected', String(candidate === option));
  });
  if (notify) input.dispatchEvent(new Event('change', { bubbles: true }));
}

// Gives one status filter button, menu, and options their interaction behavior.
function initializeStatusFilter(control) {
  const input = control.querySelector('input[type="hidden"]');
  const trigger = control.querySelector('.status-filter-trigger');
  const menu = control.querySelector('.status-filter-menu');
  const options = Array.from(menu.querySelectorAll('[role="option"]'));
  const selected = options.find(option => option.dataset.value === input.value) || options[0];
  selectStatusFilterOption(control, selected, false);
  trigger.addEventListener('click', event => {
    event.stopPropagation();
    const opening = menu.hidden;
    closeStatusFilters(opening ? control : null);
    menu.hidden = !opening;
    trigger.setAttribute('aria-expanded', String(opening));
  });
  options.forEach(option => {
    option.addEventListener('click', () => {
      selectStatusFilterOption(control, option);
      menu.hidden = true;
      trigger.setAttribute('aria-expanded', 'false');
      trigger.focus();
    });
  });
  trigger.addEventListener('keydown', event => {
    if (!['ArrowDown', 'ArrowUp'].includes(event.key)) return;
    event.preventDefault();
    closeStatusFilters(control);
    menu.hidden = false;
    trigger.setAttribute('aria-expanded', 'true');
    const target = options.find(option => option.getAttribute('aria-selected') === 'true') || options[0];
    target.focus();
  });
  menu.addEventListener('keydown', event => {
    if (event.key === 'Escape') {
      menu.hidden = true;
      trigger.setAttribute('aria-expanded', 'false');
      trigger.focus();
      return;
    }
    if (!['ArrowDown', 'ArrowUp'].includes(event.key)) return;
    event.preventDefault();
    const index = options.indexOf(document.activeElement);
    const offset = event.key === 'ArrowDown' ? 1 : -1;
    options[(index + offset + options.length) % options.length].focus();
  });
}

// Returns the runtime-only liveness row corresponding to one durable session.
function sessionLiveness(sessionId) {
  return (state.load?.sessions || []).find(row => row.experiment_id === state.experiment?.experiment_id && row.session_id === sessionId) || null;
}

// Renders health in the same dot-and-label style as experiment lifecycle status.
function livenessBadge(health, detail = '') {
  const normalized = ['live', 'delayed', 'stale', 'disconnected', 'waiting', 'ended', 'server', 'unavailable'].includes(health) ? health : 'unavailable';
  const title = detail ? ` title="${escapeHtml(detail)}"` : '';
  const label = normalized === 'disconnected' ? 'Connection lost' : normalized === 'stale' ? 'Connection unresponsive' : statusText(normalized);
  return `<span${title}>${namedStatusLabel(normalized, label)}</span>`;
}

// Renders one current-versus-limit capacity gauge with escalating color only near its ceiling.
function capacityGauge(label, current, limit, note = '') {
  const ratio = limit > 0 ? current / limit : 0;
  const percent = Math.max(0, Math.min(100, ratio * 100));
  const level = ratio >= 1 ? 'full' : ratio >= 0.8 ? 'high' : '';
  return `<div>
    <div class="capacity-head"><strong>${escapeHtml(label)}</strong><span>${escapeHtml(current)} / ${escapeHtml(limit)}</span></div>
    <div class="gauge" role="meter" aria-label="${escapeHtml(label)}" aria-valuemin="0" aria-valuemax="${escapeHtml(limit)}" aria-valuenow="${escapeHtml(current)}"><span class="${level}" style="width:${percent.toFixed(1)}%"></span></div>
    ${note ? `<div class="muted small">${escapeHtml(note)}</div>` : ''}
  </div>`;
}

// Calculates a per-second rate over up to the most recent minute of cumulative samples.
function recentRate(counter) {
  const history = state.load?.history || [];
  if (history.length < 2) return 0;
  const last = history.at(-1);
  const earliestMs = last.sampled_at_ms - 60000;
  const first = history.find(sample => sample.sampled_at_ms >= earliestMs) || history[0];
  const seconds = Math.max(1, (last.sampled_at_ms - first.sampled_at_ms) / 1000);
  return Math.max(0, ((last.counters[counter] || 0) - (first.counters[counter] || 0)) / seconds);
}

// Converts chronological samples into one SVG polyline.
function chartPoints(samples, valueFor, maximum) {
  if (!samples.length) return '';
  const width = 760;
  const height = 190;
  return samples.map((sample, index) => {
    const x = 32 + (samples.length === 1 ? 0 : index / (samples.length - 1)) * width;
    const y = 12 + height - (valueFor(sample) / maximum) * height;
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  }).join(' ');
}

// Draws the bounded rolling history without requiring a charting dependency.
function renderLoadChart() {
  const history = state.load?.history || [];
  if (!history.length) {
    loadChart.innerHTML = '<div class="empty">The first five-second sample has not arrived yet.</div>';
    return;
  }
  const maximum = Math.max(1, ...history.flatMap(sample => [
    sample.capacity.active_reserved_sessions,
    sample.connections.game_connections,
    sample.connections.audio_connections
  ]));
  const start = fmtTime(history[0].sampled_at);
  const end = fmtTime(history.at(-1).sampled_at);
  loadChart.innerHTML = `<svg viewBox="0 0 824 230" role="img" aria-labelledby="loadChartTitle loadChartDescription">
    <title id="loadChartTitle">Rolling server load</title>
    <desc id="loadChartDescription">Reserved sessions, game sockets, and audio sockets over the retained hour.</desc>
    <line class="chart-grid" x1="32" y1="12" x2="32" y2="202"></line>
    <line class="chart-grid" x1="32" y1="202" x2="792" y2="202"></line>
    <line class="chart-grid" x1="32" y1="107" x2="792" y2="107"></line>
    <text class="chart-axis" x="4" y="16">${maximum}</text><text class="chart-axis" x="17" y="205">0</text>
    <text class="chart-axis" x="32" y="222">${escapeHtml(start)}</text><text class="chart-axis" x="792" y="222" text-anchor="end">${escapeHtml(end)}</text>
    <polyline class="chart-active" points="${chartPoints(history, sample => sample.capacity.active_reserved_sessions, maximum)}"></polyline>
    <polyline class="chart-game" points="${chartPoints(history, sample => sample.connections.game_connections, maximum)}"></polyline>
    <polyline class="chart-audio" points="${chartPoints(history, sample => sample.connections.audio_connections, maximum)}"></polyline>
  </svg>`;
}

// Renders the current capacity, throughput, transport health, and storage footprint.
function renderLoad() {
  const load = state.load;
  const current = load?.current;
  if (!current) return;
  document.getElementById('operationsScopeNote').textContent = 'Telemetry aggregates every experiment runtime currently hosted by this game process.';
  const capacity = current.capacity;
  const connections = current.connections;
  const counters = current.counters;
  const averageLatency = counters.requests_total ? counters.request_duration_ms_total / counters.requests_total : 0;
  loadOverview.innerHTML = `
    <div class="metric-card"><div class="label">Reserved sessions</div><div class="metric">${capacity.active_reserved_sessions}</div><div class="muted small">${capacity.waiting_sessions} waiting · ${capacity.completed_retained_sessions} completed retained · ${capacity.active_agents} agents</div></div>
    <div class="metric-card"><div class="label">Game liveness</div><div class="metric">${connections.game_live}/${connections.game_connections}</div><div class="muted small">${connections.game_delayed} delayed · ${connections.game_stale} stale</div></div>
    <div class="metric-card"><div class="label">HTTP work</div><div class="metric">${counters.requests_in_flight}</div><div class="muted small">in flight · ${averageLatency.toFixed(1)} ms mean</div></div>
    <div class="metric-card"><div class="label">SQLite footprint</div><div class="metric">${formatBytes(current.storage?.database_total_bytes || current.storage?.database_bytes || 0)}</div><div class="muted small">${formatBytes(current.storage?.available_bytes)} filesystem free</div></div>`;
  capacityList.innerHTML = [
    capacityGauge('Active/retained sessions', capacity.active_reserved_sessions, capacity.active_session_limit, `${capacity.completed_retained_sessions} completed sessions remain during cleanup grace`),
    capacityGauge('Waiting sessions', capacity.waiting_sessions, capacity.waiting_session_limit),
    capacityGauge('Unattached participants', capacity.unattached_participants, capacity.unattached_participant_limit),
    capacityGauge('ASR streams reserved', capacity.transcription_streams_reserved, capacity.transcription_stream_limit),
    current.storage ? capacityGauge('Filesystem used', current.storage.total_bytes - current.storage.available_bytes, current.storage.total_bytes, `${formatBytes(current.storage.available_bytes)} free; admission reserve ${formatBytes(capacity.storage_reserve_bytes)} · WAL ${formatBytes(current.storage.wal_bytes)}`) : '<div class="muted small">Storage capacity is unavailable for the in-memory database.</div>'
  ].join('');
  trafficMetrics.innerHTML = `
    <div><div class="label">HTTP requests/s</div><div class="value">${recentRate('requests_total').toFixed(1)}</div></div>
    <div><div class="label">Game messages/s</div><div class="value">${recentRate('game_messages_total').toFixed(1)}</div></div>
    <div><div class="label">Heartbeats/s</div><div class="value">${recentRate('heartbeats_total').toFixed(1)}</div></div>
    <div><div class="label">Audio frames/s</div><div class="value">${recentRate('audio_frames_total').toFixed(1)}</div></div>
    <div><div class="label">Actions accepted/s</div><div class="value">${recentRate('actions_accepted_total').toFixed(1)}</div></div>
    <div><div class="label">Chats accepted/s</div><div class="value">${recentRate('chat_messages_accepted_total').toFixed(1)}</div></div>
    <div><div class="label">Input rejections</div><div class="value">${counters.actions_rejected_total + counters.chat_messages_rejected_total + counters.transport_messages_rejected_total}</div><div class="muted small">${current.pending_rejections} awaiting aggregate flush</div></div>
    <div><div class="label">HTTP errors</div><div class="value">${counters.request_client_errors_total} / ${counters.request_server_errors_total}</div><div class="muted small">4xx / 5xx</div></div>
    <div><div class="label">Peak HTTP latency</div><div class="value">${counters.request_duration_ms_max} ms</div></div>
    <div><div class="label">ASR backpressure</div><div class="value">${counters.asr_backpressure_total}</div></div>
    <div><div class="label">Audio drops</div><div class="value">${counters.audio_frames_dropped_total}</div></div>
    <div><div class="label">Reconnect replacements</div><div class="value">${counters.reconnections_total}</div></div>
    <div><div class="label">TTS</div><div class="value">${counters.tts_in_flight} active</div><div class="muted small">${counters.tts_messages_total} total · ${counters.tts_failures_total} failed</div></div>`;
  loadUpdated.textContent = `Sampled ${fmtTime(current.sampled_at)} · process uptime ${Math.floor(counters.uptime_seconds / 60)} min`;
  renderLoadChart();
  renderLivenessTable();
}

// Clears game-wide telemetry when the process endpoint cannot be read.
function renderLoadUnavailable() {
  document.getElementById('operationsScopeNote').textContent = 'Game-process telemetry is temporarily unavailable.';
  loadOverview.innerHTML = '<div class="empty">Runtime telemetry could not be loaded.</div>';
  capacityList.innerHTML = '';
  trafficMetrics.innerHTML = '';
  loadChart.innerHTML = '<div class="empty">No runtime samples are available.</div>';
  livenessTable.innerHTML = '<div class="empty">Runtime liveness is unavailable.</div>';
  loadUpdated.textContent = 'Unavailable';
}

// Renders every in-memory session with transport ages and its next cleanup deadline.
function renderLivenessTable() {
  const sessions = state.load?.sessions || [];
  if (!sessions.length) {
    livenessTable.innerHTML = '<div class="empty">No sessions currently occupy runtime memory.</div>';
    return;
  }
  livenessTable.innerHTML = `<table class="load-table"><thead><tr><th>Experiment / session</th><th>Liveness</th><th>Participants</th><th>Meaningful activity</th><th>Next lifecycle deadline</th></tr></thead><tbody>${sessions.map(session => `
    <tr class="load-session" data-experiment="${escapeHtml(session.experiment_id)}" data-session="${session.session_id}">
      <td><strong>${escapeHtml(session.experiment_id)} · #${session.session_id}</strong><div class="muted small">${escapeHtml(session.lifecycle)}</div></td>
      <td>${livenessBadge(session.health)}</td>
      <td>${session.participants.map(participant => participant.source === 'agent' ? `<div class="participant-liveness">${escapeHtml(participant.role)} ${livenessBadge('server', 'Trusted server-side agent')}</div>` : `<div class="participant-liveness">${escapeHtml(participant.role)} ${livenessBadge(participant.game_health, participant.game ? `Last game message ${formatAge(participant.game.last_message_at_ms)}` : 'No current game socket')}<div class="muted small">game ${participant.game ? formatAge(participant.game.last_message_at_ms) : 'disconnected'} · audio ${participant.audio ? formatAge(participant.audio.last_message_at_ms) : 'disconnected'}</div></div>`).join('')}</td>
      <td>${escapeHtml(formatAge(new Date(session.meaningful_activity_at).getTime()))}</td>
      <td>${session.lifecycle_deadline_at ? `${escapeHtml(fmtTime(session.lifecycle_deadline_at))}<div class="muted small">${escapeHtml(session.deadline_reason)}</div>` : '—'}</td>
    </tr>`).join('')}</tbody></table>`;
  livenessTable.querySelectorAll('[data-session]').forEach(row => {
    row.addEventListener('click', async () => {
      state.activeTab = 'sessions';
      renderTabs();
      showScope('experiments');
      if (state.experiment?.experiment_id !== row.dataset.experiment) {
        await selectExperiment(row.dataset.experiment);
      }
      await selectSession(Number(row.dataset.session));
    });
  });
}

// Fetches operational telemetry aggregated across the game process.
async function loadLoad() {
  const response = await adminFetch(state, '/api/admin/load');
  if (!response.ok) {
    state.load = null;
    renderLoadUnavailable();
    renderExperimentHeader();
    renderSessions();
    return;
  }
  state.load = await response.json();
  renderLoad();
  renderExperimentHeader();
  renderSessions();
  if (state.selectedSession) renderSummary(state.selectedSession, state.selectedParticipants);
}

// Returns mode-specific readiness without treating optional Prolific setup as globally required.
function launchReadiness(experiment, needsProlific) {
  if (experiment.configuration_valid !== true) {
    return { ready: false, issues: [experiment.configuration_error || 'The experiment configuration is invalid.'], destination: 'configuration' };
  }
  const issues = needsProlific ? (experiment.runnable_issues || []) : (experiment.local_testing_issues || []);
  const gameSettingIssue = issues.some(issue => /Game Settings|API key is required/i.test(issue));
  return { ready: issues.length === 0, issues, destination: gameSettingIssue ? 'game' : 'configuration' };
}

// Renders one icon-only readiness badge whose tooltip carries the mode diagnostics.
function launchReadinessBadge(readiness, modeLabel) {
  const title = readiness.ready ? `${modeLabel} is ready` : `${modeLabel} is not ready: ${readiness.issues.join(' ')}`;
  if (readiness.ready) return `<span class="capability-badge ready" title="${escapeHtml(title)}" aria-label="${escapeHtml(title)}">${icon('check')}</span>`;
  return `<button class="capability-badge blocked" type="button" data-fix-readiness="${escapeHtml(readiness.destination)}" title="${escapeHtml(title)}" aria-label="${escapeHtml(title)}">${icon('alert')}</button>`;
}

// Renders the filtered experiment catalogue using only durable server metadata.
function renderExperiment() {
  const experiment = state.experiment;
  const requestedStatus = experimentStatusFilter.value;
  const visibleExperiments = state.experiments.filter(item => requestedStatus ? experimentDisplayStatus(item) === requestedStatus : experimentDisplayStatus(item) !== 'archived');
  if (!state.experiments.length) {
    experimentList.innerHTML = '<div class="empty">No experiments yet.</div>';
    return;
  }
  if (!visibleExperiments.length) {
    experimentList.innerHTML = '<div class="empty">No experiments match this status.</div>';
    return;
  }
  experimentList.innerHTML = visibleExperiments.map(item => {
    const status = experimentDisplayStatus(item);
    return `
    <button class="experiment ${experiment?.experiment_id === item.experiment_id ? 'active' : ''} ${status === 'archived' ? 'archived' : ''}" data-experiment="${escapeHtml(item.experiment_id)}" type="button">
      <span class="experiment-main">
        <span class="experiment-name">${statusLabel(status, true)}<strong>${item.pinned ? '★ ' : ''}${escapeHtml(item.experiment_id)}</strong></span>
        <span class="experiment-meta muted small">
          <time>${escapeHtml(fmtDate(item.created_at))}</time>
        </span>
      </span>
      <span class="experiment-count small" title="${item.session_count} sessions">${icon('messages', 'count-icon')}${item.session_count}</span>
    </button>
  `}).join('');
  experimentList.querySelectorAll('[data-experiment]').forEach(button => {
    button.addEventListener('click', () => selectExperiment(button.dataset.experiment));
  });
}

// Renders experiment identity, lifecycle metadata, and actions without duplicating session totals.
function renderExperimentHeader() {
  const experiment = state.experiment;
  if (!experiment) {
    experimentHeader.innerHTML = '<h1>Unavailable</h1>';
    experimentStats.innerHTML = '';
    return;
  }
  const openLaunchMenu = experimentStats.querySelector('.launch-menu[open]');
  const restoreLaunchMenu = openLaunchMenu?.dataset.experimentId === experiment.experiment_id
    && openLaunchMenu?.dataset.experimentStatus === experiment.status;
  const status = experimentDisplayStatus(experiment);
  experimentHeader.innerHTML = `
    <h1>${escapeHtml(experiment.experiment_id)}</h1>
    <span class="workspace-facts small">
      ${statusLabel(status)}
      <span>Configuration revision ${escapeHtml(experiment.config_revision)}</span>
      <span>Created ${escapeHtml(fmtDate(experiment.created_at))}</span>
    </span>
  `;
  experimentStats.innerHTML = experimentActionsMarkup(experiment);
  if (restoreLaunchMenu) experimentStats.querySelector('.launch-menu')?.setAttribute('open', '');
  experimentStats.querySelectorAll('[data-lifecycle]').forEach(button => {
    button.addEventListener('click', () => updateExperimentStatus(button.dataset.lifecycle, button.dataset.verifyProlific === 'true', button.dataset.participantUrlKind));
  });
  experimentStats.querySelectorAll('[data-copy-participant-url]').forEach(button => {
    button.addEventListener('click', async () => {
      await copyParticipantUrl(experiment, button.dataset.copyParticipantUrl, button);
    });
  });
  experimentStats.querySelectorAll('[data-open-participant-url]').forEach(button => {
    button.addEventListener('click', () => openParticipantUrl(experiment));
  });
  experimentStats.querySelectorAll('[data-fix-readiness]').forEach(button => {
    button.addEventListener('click', () => openReadinessDestination(button.dataset.fixReadiness));
  });
  document.getElementById('cloneButton')?.addEventListener('click', cloneSelectedExperiment);
  document.getElementById('archiveButton')?.addEventListener('click', experiment.status === 'archived' ? () => updateExperimentStatus('inactive') : archiveSelectedExperiment);
}

// Returns whether the selected experiment recruits participants through Prolific.
function prolificIsEnabled(experiment) {
  const selectedConfig = state.experiment?.experiment_id === experiment.experiment_id ? state.configValue : null;
  return Boolean((selectedConfig?.recruitment?.prolific || experiment.config?.recruitment?.prolific)?.enabled);
}

// Builds the literal URL template copied into a Prolific study before intake starts.
function prolificSetupUrl(experiment) {
  const url = new URL(`/e/${encodeURIComponent(experiment.experiment_id)}/`, window.location.origin);
  return `${url.origin}${url.pathname}?PROLIFIC_PID={{%PROLIFIC_PID%}}&STUDY_ID={{%STUDY_ID%}}&SESSION_ID={{%SESSION_ID%}}`;
}

// Creates one opaque synthetic identifier for a single Local Preview invitation.
function localPreviewId() {
  return `TEST${crypto.randomUUID().replaceAll('-', '').toUpperCase()}`;
}

// Builds a participant URL, minting a fresh identity for each Local Preview action.
function participantPageHref(experiment) {
  const url = new URL(`/e/${encodeURIComponent(experiment.experiment_id)}/`, window.location.origin);
  const selectedConfig = state.experiment?.experiment_id === experiment.experiment_id ? state.configValue : null;
  const prolific = selectedConfig?.recruitment?.prolific || experiment.config?.recruitment?.prolific;
  const runningUrl = experiment.participant_url;
  if (!runningUrl || runningUrl.kind === 'direct' || !prolific?.enabled) return `${url.pathname}${url.search}`;
  if (runningUrl.kind === 'prolific') {
    return `${url.pathname}?PROLIFIC_PID={{%PROLIFIC_PID%}}&STUDY_ID={{%STUDY_ID%}}&SESSION_ID={{%SESSION_ID%}}`;
  }
  url.searchParams.set('PROLIFIC_PID', localPreviewId());
  url.searchParams.set('STUDY_ID', prolific.study_id);
  url.searchParams.set('SESSION_ID', localPreviewId());
  return `${url.pathname}${url.search}`;
}

// Opens one newly minted Local Preview invitation without retaining it for later launches.
function openParticipantUrl(experiment) {
  window.open(participantPageHref(experiment), '_blank', 'noopener');
}

// Copies an absolute participant URL while preserving Prolific's literal placeholder syntax.
async function copyParticipantUrl(experiment, _launch, button) {
  const value = `${window.location.origin}${participantPageHref(experiment)}`;
  try {
    await navigator.clipboard.writeText(value);
  } catch (_error) {
    window.prompt('Copy participant URL', value);
    return;
  }
  const original = button.innerHTML;
  button.textContent = 'Copied';
  setTimeout(() => { button.innerHTML = original; }, 1400);
}

// Opens the settings surface most likely to resolve a blocked launch mode.
function openReadinessDestination(destination) {
  if (destination === 'game') {
    showScope('game');
    return;
  }
  state.activeTab = 'details';
  showScope('experiments');
  renderTabs();
}

// Shows a dedicated creation prompt instead of experiment controls when the catalogue is empty.
function renderExperimentWorkspaceState() {
  const hasExperiment = Boolean(state.experiment);
  experimentWorkspaceHeader.hidden = !hasExperiment;
  emptyExperimentWorkspace.hidden = hasExperiment;
  experimentWorkspaceTabs.hidden = !hasExperiment;
  if (hasExperiment) {
    renderTabs();
    return;
  }
  Object.values(tabPanels).forEach(panel => { panel.hidden = true; });
}

// Renders one launch mode with an icon-only readiness badge and a mode-specific action.
function launchModeMarkup(experiment, label, lifecycle, needsProlific) {
  const readiness = launchReadiness(experiment, needsProlific);
  const verify = needsProlific && lifecycle === 'testing' ? ' data-verify-prolific="true"' : '';
  const urlKind = needsProlific ? 'prolific' : lifecycle === 'testing' ? 'local' : 'direct';
  return `<span class="launch-mode"><strong>${escapeHtml(label)}</strong>${launchReadinessBadge(readiness, label)}<button class="${lifecycle === 'active' ? 'primary' : 'secondary'}" data-lifecycle="${lifecycle}" data-participant-url-kind="${urlKind}"${verify} type="button" ${readiness.ready ? '' : 'disabled'}>${icon(lifecycle === 'active' ? 'power' : 'flask')}Start</button></span>`;
}

// Returns actions for inactive, testing, active, completed, and archived experiments.
function experimentActionsMarkup(experiment) {
  const prolific = prolificIsEnabled(experiment);
  if (experiment.status === 'inactive') return `
    <span class="running-actions"><button class="secondary" id="cloneButton" type="button">${icon('copy')}Clone</button><button class="secondary" id="archiveButton" type="button">${icon('archive')}Archive</button></span>
    <details class="launch-menu" data-experiment-id="${escapeHtml(experiment.experiment_id)}" data-experiment-status="${escapeHtml(experiment.status)}"><summary class="primary">${icon('play')}Start experiment</summary><div class="launch-menu-panel"><div class="launch-actions" aria-label="Experiment launch modes">
      ${launchModeMarkup(experiment, 'Local preview', 'testing', false)}
      ${prolific ? launchModeMarkup(experiment, 'Test through Prolific', 'testing', true) : ''}
      ${launchModeMarkup(experiment, 'Official intake', 'active', prolific)}
    </div></div></details>`;
  if (experiment.status === 'testing') {
    const openControl = experiment.participant_url?.kind === 'local'
      ? `<button class="secondary" data-open-participant-url type="button">${icon('external')}Open</button>`
      : `<a class="secondary" href="${escapeHtml(participantPageHref(experiment))}" target="_blank" rel="noopener">${icon('external')}Open</a>`;
    return `<span class="running-actions">
      ${openControl}
      <button class="secondary" data-copy-participant-url="running" type="button">${icon('copy')}Copy URL</button>
      <button class="secondary" data-lifecycle="inactive" type="button">${icon('power')}Stop</button>
    </span>`;
  }
  if (experiment.status === 'active') return `<span class="running-actions">
    <a class="secondary" href="${escapeHtml(participantPageHref(experiment))}" target="_blank" rel="noopener">${icon('external')}Open</a>
    <button class="secondary" data-copy-participant-url="running" type="button">${icon('copy')}Copy URL</button>
    <button class="secondary" data-lifecycle="inactive" type="button">${icon('power')}Pause intake</button>
    <button class="primary" data-lifecycle="completed" type="button">${icon('check')}Complete</button>
  </span>`;
  if (experiment.status === 'archived') return `<button class="secondary" id="archiveButton" type="button">${icon('restore')}Unarchive</button>`;
  if (experiment.status === 'completed') return `<button class="secondary" id="archiveButton" type="button">${icon('archive')}Archive</button>`;
  return '';
}

// Renders the game and Parlando build identity shared by every experiment.
function renderExperimentDetails() {
  const experiment = state.experiment;
  const manifest = state.versionManifest || {};
  const warnings = manifest.warnings || [];
  const gameManifest = state.game?.build_manifest || manifest.game || {};
  gameName.textContent = state.game?.name || displayGameName(gameManifest);
  document.title = state.game ? `${gameName.textContent} · Parlando` : 'Parlando Experimenter Dashboard';
  gameVersion.textContent = `Version ${state.game?.version || gameManifest.version || '—'}`;
  gameGit.textContent = `Git ${shortSha(gameManifest.git_sha)}`;
  gameBuild.textContent = `Built ${fmtTime(gameManifest.build_time)}`;
  experimentLabel.textContent = experiment ? `Experiment ${experiment.experiment_id}` : 'Experiment unavailable';
  reproducibility.innerHTML = `
    <dl class="build-facts">
      <div><dt>GAME VERSION</dt><dd>${escapeHtml(gameManifest.version || state.game?.version || '—')}</dd></div>
      <div><dt>GAME GIT</dt><dd><code>${escapeHtml(shortSha(gameManifest.git_sha))}</code></dd></div>
      <div><dt>GAME BUILD TIME</dt><dd>${escapeHtml(fmtTime(gameManifest.build_time))}</dd></div>
      <div><dt>PARLANDO SERVER VERSION</dt><dd>${escapeHtml(manifest.server?.version || '—')}</dd></div>
      <div><dt>PARLANDO CLIENT VERSION</dt><dd>${escapeHtml(gameManifest.client?.version || '—')}</dd></div>
      <div><dt>PARLANDO CLIENT PACKAGE</dt><dd>${escapeHtml(gameManifest.client?.package_version || '—')}</dd></div>
    </dl>
    ${warnings.length ? warnings.map(warning => `<div class="warning">${escapeHtml(warning.message || warning.dependency || JSON.stringify(warning))}</div>`).join('') : '<p class="muted small">No local development dependency warnings recorded.</p>'}
  `;
}

// Derives a readable fallback when no explicit game display name is available.
function displayGameName(game) {
  const name = game?.display_name || game?.name || 'Game';
  return String(name).replace(/^parlando-/, '').replace(/-/g, ' ');
}

// Describes one terminal cause without claiming more than Parlando observed.
function sessionEndPresentation(session) {
  const cause = session.session_end?.cause || {};
  const actor = cause.actor || cause.disconnected_role;
  const subject = actor || 'Participant';
  if (cause.type === 'game_completed') return { status: 'completed', label: 'Completed', detail: '' };
  if (cause.type === 'participant_left') return { status: 'explicit-left', label: `${subject} chose to leave`, detail: 'The participant used Parlando’s leave action.' };
  if (cause.type === 'reconnect_timed_out') return { status: 'connection-lost', label: `${subject} connection lost`, detail: 'The browser connection ended and the participant did not reconnect. Parlando cannot distinguish a closed tab from a network interruption.' };
  if (cause.type === 'partner_unavailable') return { status: 'no-partner', label: 'No partner found', detail: '' };
  if (cause.type === 'idle_timed_out') return { status: 'inactivity-timeout', label: 'Inactivity timeout', detail: '' };
  if (cause.type === 'lifetime_timed_out') return { status: 'duration-limit', label: 'Maximum duration reached', detail: '' };
  if (cause.type === 'technical_failure') return { status: 'technical-failure', label: 'Technical failure', detail: '' };
  return { status: 'unavailable', label: 'End reason unavailable', detail: '' };
}

// Renders a terminal-cause badge with precise evidence boundaries in its tooltip.
function sessionEndBadge(session) {
  const presentation = sessionEndPresentation(session);
  const title = presentation.detail ? ` title="${escapeHtml(presentation.detail)}"` : '';
  return `<span${title}>${namedStatusLabel(presentation.status, presentation.label)}</span>`;
}

// Derives the factual session label used in the compact session catalogue.
function factualSessionStatus(session) {
  if (session.lifecycle === 'ended') return sessionEndPresentation(session).label;
  return statusText(session.lifecycle);
}

// Returns the terminal participant outcomes attached to one durable session.
function sessionOutcomes(session) {
  return Object.values(session.session_end?.participant_results || {}).map(result => result?.outcome).filter(Boolean);
}

// Returns whether a terminal session ended because no playable game formed.
function sessionGameDidNotStart(session) {
  return sessionOutcomes(session).includes('partner_unavailable');
}

// Formats exact unsuccessful waiting time from durable server timestamps.
function unsuccessfulWait(session) {
  if (!sessionGameDidNotStart(session) || !session.waiting_started_at) return '';
  const end = new Date(session.ended_at || session.waiting_deadline_at).getTime();
  const start = new Date(session.waiting_started_at).getTime();
  if (!Number.isFinite(end - start)) return '';
  return ` · waited ${formatDuration(Math.max(0, end - start))}`;
}

// Renders the selected experiment's durable sessions with lifecycle dots and compact event counts.
function renderSessions() {
  if (!state.sessions.length) {
    sessionList.innerHTML = '<div class="empty">No sessions in this experiment yet.</div>';
    return;
  }
  sessionList.innerHTML = state.sessions.map(session => `
    <button class="session ${state.selected === session.session_id ? 'active' : ''}" data-session="${session.session_id}">
      <span class="session-main">
        <span class="session-name">${statusLabel(session.lifecycle, true)}<strong>${escapeHtml(session.dialogue_id || `Session #${session.session_id}`)}</strong></span>
        <span class="muted small">${escapeHtml(factualSessionStatus(session))} · #${session.session_id} · ${escapeHtml(fmtTime(session.created_at))}${escapeHtml(unsuccessfulWait(session))}${session.purpose === 'testing' ? ` · <span class="purpose-marker"><span class="status-dot testing" aria-hidden="true"></span>Testing</span>` : ''}</span>
      </span>
      <span class="session-count small" title="${session.event_count} events">${icon('list-tree', 'count-icon')}${session.event_count}</span>
    </button>
  `).join('');
  sessionList.querySelectorAll('.session').forEach(button => {
    button.addEventListener('click', () => selectSession(Number(button.dataset.session)));
  });
}

// Loads recent durable sessions using the selected lifecycle filter.
async function loadSessions() {
  if (!state.experiment) return;
  const params = new URLSearchParams({ limit: '80' });
  if (sessionLifecycleFilter.value) params.set('lifecycle', sessionLifecycleFilter.value);
  const response = await adminFetch(state, `${runtimeApi('sessions')}?${params}`);
  if (!response.ok) throw new Error(await response.text());
  const data = await response.json();
  state.sessions = data.sessions || [];
  if (state.selected && !state.sessions.some(session => session.session_id === state.selected)) {
    state.selected = null;
    state.selectedSession = null;
    state.selectedParticipants = [];
    sessionDetail.hidden = true;
  } else if (state.selectedSession) {
    const refreshed = state.sessions.find(session => session.session_id === state.selected);
    if (refreshed) Object.assign(state.selectedSession, refreshed);
  }
  renderSessions();
  if (state.selectedSession) renderSummary(state.selectedSession, state.selectedParticipants);
  if (!state.selected && state.sessions[0]) selectSession(state.sessions[0].session_id);
}

// Refreshes game identity, catalogue metadata, shared settings, and the selected configuration.
async function loadExperiment() {
  const response = await adminFetch(state, '/api/admin/experiments');
  if (!response.ok) throw new Error(await response.text());
  const data = await response.json();
  state.experiments = data.experiments || [];
  state.game = data.game || null;
  state.versionManifest = data.version_manifest || null;
  state.gameSettings = data.game_settings || null;
  state.gameProviderSecrets = data.game_provider_secrets || [];
  state.csrfToken = data.csrf_token || null;
  state.configLoadFailed = false;
  state.configLoadError = null;
  const selectedId = state.experiment?.experiment_id || state.experiments.find(item => ['active', 'testing'].includes(item.status))?.experiment_id;
  state.experiment = state.experiments.find(item => item.experiment_id === selectedId) || state.experiments[0] || null;
  if (!state.experiment) {
    state.selected = null;
    state.selectedSession = null;
    state.selectedParticipants = [];
    state.sessions = [];
    state.events = [];
    state.eventBundles = [];
    state.load = null;
    state.privacy = null;
    state.activationIssues = [];
    state.loadedConfigKey = null;
    state.configValue = null;
    sessionDetail.hidden = true;
    sessionList.innerHTML = '<div class="empty">No experiment selected.</div>';
  }
  renderExperiment();
  renderExperimentHeader();
  renderExperimentWorkspaceState();
  renderExperimentDetails();
  renderNotes();
  if (!state.gameSettingsDirty) synchronizeGameSettingsForm();
  const configKey = state.experiment ? `${state.experiment.experiment_id}:${state.experiment.config_revision}` : null;
  if (configKey && configKey !== state.loadedConfigKey) await loadConfigurationSafely();
  updateConfigurationEditability();
}

// Changes the selected experiment's durable lifecycle.
async function updateExperimentStatus(status, verifyProlific = false, participantUrlKind = null) {
  const statusChanged = state.experiment?.status !== status;
  const buttons = [...experimentStats.querySelectorAll('button')];
  buttons.forEach(button => { button.disabled = true; });
  const response = await adminFetch(state, runtimeApi('experiment/status'), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': state.csrfToken || '' },
    body: JSON.stringify({ status, verify_prolific: verifyProlific, participant_url_kind: participantUrlKind })
  });
  if (!response.ok) {
    buttons.forEach(button => { button.disabled = false; });
    window.alert(await response.text());
    return false;
  }
  if (statusChanged) await loadExperiment();
  else buttons.forEach(button => { button.disabled = false; });
  return true;
}

// Uses the installation-level one-way archival route without constructing a runtime.
async function archiveSelectedExperiment() {
  const experiment = state.experiment;
  if (!experiment || !window.confirm(`Archive ${experiment.experiment_id}?`)) return;
  const response = await adminFetch(state, `/api/admin/experiments/${encodeURIComponent(experiment.experiment_id)}/archive`, {
    method: 'POST', headers: { 'X-CSRF-Token': state.csrfToken || '' }
  });
  if (!response.ok) { window.alert(await response.text()); return; }
  await loadExperiment();
}

// Selects one experiment and scopes session, export, and configuration requests to it.
async function selectExperiment(experimentId) {
  state.experiment = state.experiments.find(item => item.experiment_id === experimentId) || null;
  state.selected = null;
  state.selectedSession = null;
  state.selectedParticipants = [];
  state.load = null;
  state.privacy = null;
  state.activationIssues = [];
  state.configLoadFailed = false;
  state.configLoadError = null;
  sessionDetail.hidden = true;
  closeExperimentMenu();
  renderExperiment();
  renderExperimentHeader();
  renderExperimentDetails();
  renderNotes();
  await Promise.all([loadConfigurationSafely(), loadSessions(), loadLoad()]);
  if (state.activeTab === 'privacy') await loadPrivacy();
}

// Returns an authenticated runtime API path for the selected experiment.
function runtimeApi(path) {
  return experimentRuntimeApi(state.experiment, path);
}

// Loads the selected experiment's authoritative database-backed configuration.
async function loadConfiguration() {
  if (!state.experiment) return;
  const id = encodeURIComponent(state.experiment.experiment_id);
  const response = await adminFetch(state, `/api/admin/experiments/${id}/config`);
  if (!response.ok) throw new Error(await response.text());
  const data = await response.json();
  state.configLoadFailed = false;
  state.agentFactories = data.agent_factories || [];
  state.configuredSecrets = data.configured_secrets || [];
  state.activationIssues = data.activation_issues || [];
  renderExperimentHeader();
  state.gameSecretKeys = state.configuredSecrets.filter(item => item.key.startsWith('game.')).map(item => item.key);
  state.secretDeletions = new Set();
  const authoritativeExperiment = data.experiment;
  if (state.experiment?.experiment_id === authoritativeExperiment.experiment_id) {
    Object.assign(state.experiment, authoritativeExperiment);
    const catalogueExperiment = state.experiments.find(item => item.experiment_id === authoritativeExperiment.experiment_id);
    if (catalogueExperiment && catalogueExperiment !== state.experiment) Object.assign(catalogueExperiment, authoritativeExperiment);
    renderExperiment();
    renderExperimentHeader();
    renderExperimentDetails();
  }
  renderConfigurationForm(data.experiment.config, data.game_yaml || '{}\n');
  renderExperimentHeader();
  upgradeYamlEditor();
  configRevision.textContent = `Revision ${data.experiment.config_revision}`;
  state.loadedConfigKey = `${data.experiment.experiment_id}:${data.experiment.config_revision}`;
  await loadRevisionHistory();
  updateConfigurationEditability(authoritativeExperiment);
}

// Keeps session browsing available and prevents stale configuration saves after a load error.
async function loadConfigurationSafely() {
  try {
    await loadConfiguration();
  } catch (error) {
    state.configLoadFailed = true;
    state.configLoadError = error.message;
    state.loadedConfigKey = null;
    state.configValue = null;
    configRevision.textContent = 'Unavailable';
    configForm.innerHTML = `<div class="empty">${escapeHtml(error.message)}</div>`;
    revisionHistory.innerHTML = '';
    saveConfiguration.disabled = true;
    renderExperimentHeader();
    renderExperimentWorkspaceState();
  }
}

// Reapplies editor availability after lifecycle-only refreshes that do not reload configuration data.
function updateConfigurationEditability(experiment = state.experiment) {
  const readOnly = state.configLoadFailed || !experiment || experiment.status !== 'inactive' || experiment.game_version !== state.game?.version;
  configForm.querySelectorAll('input, textarea, select, button:not([data-reveal-secret])').forEach(control => { control.disabled = readOnly; });
  saveConfiguration.disabled = readOnly;
}

// Loads compact immutable revision metadata for the selected experiment.
async function loadRevisionHistory() {
  const id = encodeURIComponent(state.experiment.experiment_id);
  const response = await adminFetch(state, `/api/admin/experiments/${id}/revisions`);
  if (!response.ok) throw new Error(await response.text());
  const data = await response.json();
  revisionHistory.innerHTML = (data.revisions || []).map(revision => `
    <div class="revision-row">
      <strong>Revision ${revision.revision}</strong>
      <span class="muted small">${escapeHtml(fmtTime(revision.created_at))}</span>
      ${revision.change_summary ? `<div class="small">${escapeHtml(revision.change_summary)}</div>` : ''}
    </div>
  `).join('') || '<div class="empty">No revisions stored.</div>';
}

const CONSENT_TEMPLATES = {
  eligibility: {
    id: 'eligibility_and_information_v1_0',
    title: 'Eligibility and study information',
    required: true,
    body: ({ informationVersion }) => `I confirm that I am at least 18 years old. I have read and understood the Participant Information and Privacy Notice${informationVersion ? `, version ${informationVersion},` : ''} linked above, and I have had the opportunity to ask questions.`
  },
  researchData: {
    id: 'research_data_consent_v1_0',
    title: 'Voluntary participation and research data',
    required: true,
    body: ({ institution }) => {
      if (!institution) throw new Error('Configure the responsible institution before adding this consent template.');
      return `I voluntarily agree to participate and consent to ${institution} processing the study-specific identifiers, game actions, timing, results, and the messages or final transcripts enabled for this study. I understand that these data will be used for this study, scientific verification and reuse, and the preparation and publication of anonymized research corpora as described in the Participant Information and Privacy Notice. I understand how to withdraw my consent while my data can still be linked to my study-specific participant identifier.`;
    }
  },
  privacyRules: {
    id: 'participant_privacy_rules_v1_0',
    title: 'Communication and privacy rules',
    required: true,
    body: "I will discuss only the fictional task. I will not disclose contact details, credentials, or sensitive real-world information, and I will not record, identify, copy, or redistribute another participant's communications."
  },
  voice: {
    id: 'voice_and_transcription_v1_0',
    title: 'Live voice and transcription',
    required: true,
    body: () => 'I consent to my live microphone audio being transmitted to the other participant and, when transcription is enabled, to Speechmatics for real-time transcription in the processing region described in the Participant Information and Privacy Notice. I consent to storage of the final transcript and timing when enabled for this study. I understand that Parlando does not store raw microphone audio.'
  }
};

const CONSENT_TEMPLATE_CHOICES = [
  ['eligibility', 'Eligibility and study information'],
  ['researchData', 'Voluntary participation and research data'],
  ['privacyRules', 'Communication and privacy rules'],
  ['voice', 'Live voice and transcription']
];

const PARLANDO_CONFIG_SECTIONS = [
  { title: 'Session lifecycle', description: 'Timeouts applied uniformly to every session in this experiment.', fields: [
    { path: 'session.waiting_session_timeout_seconds', label: 'Waiting-session timeout (seconds)', help: 'Time before an unmatched session expires.', type: 'number', min: 1 },
    { path: 'session.reconnect_grace_seconds', label: 'Reconnect grace (seconds)', help: 'Time a disconnected player may return.', type: 'number', min: 0 },
    { path: 'session.session_idle_timeout_seconds', label: 'Idle timeout (seconds)', help: 'Time without meaningful activity before an active session expires.', type: 'number', min: 1 },
    { path: 'session.session_max_lifetime_seconds', label: 'Maximum lifetime (seconds)', help: 'Absolute session lifetime, including waiting.', type: 'number', min: 1 }
  ]},
  { title: 'Participant access and consent', description: 'Controls direct participant entry and the exact information presented before play.', fields: [
    { path: 'direct.enabled', label: 'Enable direct participant access', help: 'Allow participants to enter this experiment from its public participant page.', type: 'boolean' },
    { path: 'direct.participant_information_version', label: 'Participant-information version', help: 'Version identifier recorded with the information shown to participants.' },
    { path: 'direct.participant_information_url', label: 'Participant-information URL', help: 'Public document describing the study and its data handling.', type: 'url' },
    { path: 'direct.consents', label: 'Consent items', help: 'Statements presented before participation. Accepted items are recorded with their exact presentation hash.', type: 'consents' }
  ]},
  { title: 'Voice transport', description: 'Live browser audio carried through Parlando. This is independent of speech recognition.', fields: [
    { path: 'voice.enabled', label: 'Enable voice transport', help: 'Allow participants to send and receive live audio. Parlando relays audio but does not store the raw recording.', type: 'boolean' },
    { path: 'voice.jitter_buffer_ms', label: 'Playback jitter buffer (ms)', help: 'Audio buffered before playback.', type: 'number', min: 20, max: 5000 },
  ]},
  { title: 'Speech recognition', description: 'Optional Speechmatics conversion of participant speech into text.', fields: [
    { path: 'transcription.enabled', label: 'Enable speech recognition', help: 'Send live participant audio to Speechmatics and make recognized text available to the game and agents.', type: 'boolean' },
    { path: 'transcription.model', label: 'Recognition model', help: 'Enhanced favors recognition quality; Standard is the lighter operating point.', type: 'select', options: [['enhanced', 'Enhanced'], ['standard', 'Standard']] },
    { path: 'transcription.language', label: 'Spoken language', help: 'Speechmatics language code for the expected participant speech, for example en or de.' },
    { path: 'speechmatics.realtime_url', label: 'Speechmatics realtime URL', help: 'Exact regional or private WebSocket endpoint that receives participant audio.' },
    { path: 'speechmatics.max_delay', label: 'Maximum transcript delay (seconds)', help: 'Maximum recognition delay; larger values may improve accuracy at the cost of responsiveness.', type: 'decimal', min: 0.1 },
    { path: 'speechmatics.enable_partials', label: 'Show partial transcripts', help: 'Deliver provisional text while the participant is still speaking.', type: 'boolean' },
    { path: 'speechmatics.end_of_utterance_silence_trigger', label: 'End-of-utterance silence (seconds)', help: 'Silence used to close an utterance.', type: 'decimal', min: 0.1 }
  ]},
  { title: 'Text to speech', description: 'Optional ElevenLabs speech synthesis for agent messages.', fields: [
    { path: 'tts.enabled', label: 'Enable text to speech', help: 'Synthesize agent messages with ElevenLabs and publish the audio through voice transport.', type: 'boolean' },
    { path: 'tts.model', label: 'Synthesis model', help: 'Choose the ElevenLabs balance of latency, language coverage, and expressiveness.', type: 'select', options: [['eleven_flash_v2_5', 'Flash v2.5'], ['eleven_flash_v2', 'Flash v2'], ['eleven_multilingual_v2', 'Multilingual v2'], ['eleven_v3', 'Eleven v3']] },
    { path: 'tts.voice_id', label: 'ElevenLabs voice id', help: 'Stable ElevenLabs identifier of the voice used for agent speech.' },
    { path: 'tts.voice_name', label: 'Voice display name', help: 'Human-readable voice label exposed to the participant client.' },
    { path: 'tts.base_url', label: 'ElevenLabs base URL', help: 'Exact WebSocket service origin that receives agent message text.' }
  ]},
  { title: 'Prolific study and completion paths', description: 'Link this experiment to exactly one Prolific study. Create six completion paths under Prolific → Data collection → Completion paths, then copy each Prolific-generated code into its matching field. Parlando never generates completion codes. Private intake identifiers never enter exports.', fields: [
    { type: 'prolific-setup-url', label: 'URL for Prolific study setup', help: 'Copy this URL into Prolific before linking the study here. It contains Prolific placeholders and accepts participants only while this experiment is running through Prolific.' },
    { path: 'recruitment.prolific.enabled', label: 'Enable Prolific intake', help: 'Require and privately retain the PROLIFIC_PID, STUDY_ID, and SESSION_ID URL parameters.', type: 'boolean' },
    { path: 'recruitment.prolific.study_id', label: 'Linked Prolific study ID', help: 'In app.prolific.com/researcher/studies/<study-id>, enter only the value after /studies/. Parlando derives its workspace from the study project.' },
    { path: 'recruitment.prolific.completion_paths.completed', label: 'Completed code', help: 'Copy the code for the Prolific path whose action approves the submission.', type: 'completion-code' },
    { path: 'recruitment.prolific.completion_paths.partner_left', label: 'Partner left code', help: 'Copy the code for the Prolific path whose action approves the good-faith participant’s submission.', type: 'completion-code' },
    { path: 'recruitment.prolific.completion_paths.game_did_not_start', label: 'Game did not start code', help: 'Copy the code for a custom Game did not start path whose action requests a return. Do not use Screened out.', type: 'completion-code' },
    { path: 'recruitment.prolific.completion_paths.participation_ended_early', label: 'Participation ended before completion code', help: 'Copy the code for the Prolific path whose action requests a return.', type: 'completion-code' },
    { path: 'recruitment.prolific.completion_paths.technical_failure', label: 'Technical failure code', help: 'Copy the code for the Prolific path whose action approves the submission.', type: 'completion-code' },
    { path: 'recruitment.prolific.completion_paths.no_consent', label: 'No consent code', help: 'Copy the code for Prolific’s built-in No consent path whose action requests a return.', type: 'completion-code' }
  ]},
  { title: 'Players and agents', description: 'Select human pairing or one server-side agent and configure its runtime limits.', fields: [
    { path: 'agents', label: 'Participants', help: 'Pair two people, or pair one person with an agent compiled into this game server.', type: 'agents' }
  ]},
  { title: 'Capacity', description: 'Admission limits and the disk reserve protecting durable storage.', fields: [
    { path: 'capacity.max_active_sessions', label: 'Maximum active sessions', help: 'Maximum sessions that have paired participants or otherwise occupy active runtime capacity.', type: 'number', min: 1 },
    { path: 'capacity.max_waiting_sessions', label: 'Maximum waiting sessions', help: 'Maximum unmatched participant sessions waiting for a compatible partner.', type: 'number', min: 1 },
    { path: 'capacity.max_unattached_participants', label: 'Maximum unattached participants', help: 'Maximum issued participant identities that have not yet joined a session.', type: 'number', min: 1 },
    { path: 'capacity.max_transcription_streams', label: 'Maximum recognition streams', help: 'Maximum simultaneous participant audio streams sent to speech recognition.', type: 'number', min: 1 },
    { path: 'capacity.storage_reserve_megabytes', label: 'Disk reserve (MB)', help: 'Pause new session admission when available disk space falls below this reserve.', type: 'number', min: 1 }
  ]}
];

// Returns one nested configuration value without evaluating a user-controlled expression.
function configAt(config, path) {
  return path.split('.').reduce((value, key) => value == null ? undefined : value[key], config);
}

// Writes one nested configuration value while preserving unedited server-owned defaults.
function setConfigAt(config, path, value) {
  const segments = path.split('.');
  let target = config;
  segments.slice(0, -1).forEach(segment => { target = target[segment] ||= {}; });
  target[segments.at(-1)] = value;
}

// Renders the curated Parlando editor, game YAML, and write-only secret controls.
function renderConfigurationForm(config, gameYaml = '{}\n') {
  state.configValue = structuredClone(config);
  const activationWarning = `<div id="activationWarning" class="activation-warning" role="alert" ${state.activationIssues.length ? '' : 'hidden'}><strong>This experiment cannot be started yet.</strong><span id="activationWarningItems">${state.activationIssues.map(issue => escapeHtml(issue)).join(' ')}</span></div>`;
  const sections = PARLANDO_CONFIG_SECTIONS.map(section => `
    <section class="config-group">
      <header class="config-group-copy"><h3>${escapeHtml(section.title)}</h3><p class="muted small">${escapeHtml(section.description)}</p></header>
      <div class="config-fields">${section.fields.map(field => renderConfigField(field, config)).join('')}</div>
    </section>`).join('');
  configForm.innerHTML = `${activationWarning}${sections}
    <section class="config-group">
      <header class="config-group-copy"><h3>Game configuration</h3><p class="muted small">Game-owned options. YAML is parsed and validated by the compiled game before it can be saved.</p></header>
      <div class="config-fields">
        <label class="config-field"><span>Game options</span><textarea id="gameYaml" class="game-yaml" spellcheck="false">${escapeHtml(gameYaml)}</textarea></label>
        <div class="validation-row"><button class="secondary" id="validateGameYaml" type="button">${icon('check')}Validate YAML</button><span id="gameYamlValidation" class="validation-result small"></span></div>
      </div>
    </section>
    <section class="config-group">
      <header class="config-group-copy"><h3>Game secrets</h3><p class="muted small">Named credentials available to game startup code. Values are omitted from revisions and exports.</p></header>
      <div class="config-fields"><div id="gameSecretList" class="game-secret-list"></div><div class="add-secret-row"><label class="config-field"><span>New secret name</span><input id="newGameSecretName" placeholder="service_api_key"></label><button class="secondary" id="addGameSecret" type="button">${icon('plus')}Add secret</button></div></div>
    </section>`;
  document.getElementById('validateGameYaml').addEventListener('click', validateGameYaml);
  document.getElementById('addGameSecret').addEventListener('click', addGameSecret);
  configForm.querySelectorAll('[data-reveal-secret]').forEach(button => button.addEventListener('click', () => revealSecret(button.dataset.revealSecret, button)));
  configForm.querySelectorAll('[data-delete-secret]').forEach(button => button.addEventListener('click', () => removeSecret(button.dataset.deleteSecret)));
  configForm.querySelectorAll('[data-secret-placeholder="true"]').forEach(installSecretReplacementInput);
  configForm.querySelectorAll('[data-consent-editor]').forEach(initializeConsentEditor);
  configForm.querySelectorAll('[data-agent-editor]').forEach(initializeAgentEditor);
  configForm.querySelector('[data-copy-prolific-setup-url]')?.addEventListener('click', event => copyProlificSetupUrl(event.currentTarget));
  configForm.querySelectorAll('[data-config-path], [data-secret-update]').forEach(control => {
    control.addEventListener('input', refreshDraftActivationWarning);
    control.addEventListener('change', refreshDraftActivationWarning);
  });
  renderGameSecrets();
  refreshDraftActivationWarning();
}

// Renders one curated typed Parlando setting.
function renderConfigField(field, config) {
  if (field.type === 'prolific-setup-url') {
    const value = prolificSetupUrl(state.experiment);
    return `<div class="config-field"><span>${escapeHtml(field.label)}</span><div class="setup-url-control"><input type="text" readonly value="${escapeHtml(value)}" aria-label="${escapeHtml(field.label)}"><button class="secondary" data-copy-prolific-setup-url type="button">${icon('copy')}Copy setup URL</button></div><span class="config-help">${escapeHtml(field.help)}</span></div>`;
  }
  const value = configAt(config, field.path);
  const help = field.help ? `<span class="config-help">${escapeHtml(field.help)}</span>` : '';
  let control;
  if (field.type === 'consents') return renderConsentEditor(field, Array.isArray(value) ? value : []);
  if (field.type === 'agents') return renderAgentEditor(field, value || {});
  if (field.type === 'boolean') return `<label class="config-field config-boolean"><span class="checkbox-line"><input data-config-path="${escapeHtml(field.path)}" data-config-type="boolean" type="checkbox" ${value ? 'checked' : ''}><span>${escapeHtml(field.label)}</span></span>${help}</label>`;
  else if (field.type === 'number') control = `<input data-config-path="${escapeHtml(field.path)}" data-config-type="number" type="number" value="${escapeHtml(value)}" ${field.min != null ? `min="${field.min}"` : ''} ${field.max != null ? `max="${field.max}"` : ''} step="${field.step || '1'}">`;
  else if (field.type === 'decimal') control = `<input data-config-path="${escapeHtml(field.path)}" data-config-type="number" type="text" inputmode="decimal" pattern="-?[0-9]+(?:\\.[0-9]+)?" value="${escapeHtml(value)}" placeholder="1.2" title="Use a decimal point, for example 1.2">`;
  else if (field.type === 'select') control = `<select data-config-path="${escapeHtml(field.path)}" data-config-type="string">${field.options.map(([option, label]) => `<option value="${escapeHtml(option)}" ${value === option ? 'selected' : ''}>${escapeHtml(label)}</option>`).join('')}</select>`;
  else if (field.type === 'json') control = `<textarea data-config-path="${escapeHtml(field.path)}" data-config-type="json" rows="${field.rows || 5}" spellcheck="false">${escapeHtml(JSON.stringify(value, null, 2))}</textarea>`;
  else if (field.type === 'completion-code') control = `<input data-config-path="${escapeHtml(field.path)}" data-config-type="string" type="text" maxlength="64" pattern="[A-Za-z0-9]*" value="${escapeHtml(value ?? '')}" autocomplete="off">`;
  else control = `<input data-config-path="${escapeHtml(field.path)}" data-config-type="string" type="${field.type === 'url' ? 'url' : 'text'}" value="${escapeHtml(value ?? '')}">`;
  return `<label class="config-field"><span>${escapeHtml(field.label)}</span>${control}${help}</label>`;
}

// Copies the inactive-safe URL template used to configure one linked Prolific study.
async function copyProlificSetupUrl(button) {
  const value = prolificSetupUrl(state.experiment);
  try {
    await navigator.clipboard.writeText(value);
  } catch (_error) {
    window.prompt('Copy Prolific setup URL', value);
    return;
  }
  const original = button.innerHTML;
  button.textContent = 'Copied';
  setTimeout(() => { button.innerHTML = original; }, 1400);
}

// Renders fields owned by one compiled agent factory without exposing raw JSON.
function agentFactoryFieldsMarkup(descriptor, values = {}) {
  if (!descriptor) return '<p class="muted small">This game binary does not provide an agent.</p>';
  return agentFieldsMarkup(descriptor.config_fields || [], values, '', descriptor.id);
}

// Reuses the game configuration editor's typed-control conventions recursively.
function agentFieldsMarkup(fields, values, parentPath, factoryId) {
  return fields.map(field => {
    const path = parentPath ? `${parentPath}.${field.key}` : field.key;
    const value = values?.[field.key] ?? field.default_value ?? '';
    if (field.type === 'object') {
      return `<fieldset class="config-group"><legend>${escapeHtml(field.label)}</legend>${agentFieldsMarkup(field.fields || [], value || {}, path, factoryId)}</fieldset>`;
    }
    let control;
    const attributes = `data-agent-config-path="${escapeHtml(path)}" data-agent-config-type="${escapeHtml(field.type)}" ${field.required ? 'required' : ''}`;
    if (field.type === 'boolean') control = `<input ${attributes} type="checkbox" ${value ? 'checked' : ''}>`;
    else if (field.type === 'integer' || field.type === 'number') control = `<input ${attributes} type="number" value="${escapeHtml(value)}" ${field.minimum != null ? `min="${field.minimum}"` : ''} ${field.maximum != null ? `max="${field.maximum}"` : ''} step="${field.type === 'integer' ? '1' : 'any'}">`;
    else if (field.type === 'choice') control = `<select ${attributes}>${(field.choices || []).map(choice => `<option value="${escapeHtml(choice.value)}" ${choice.value === value ? 'selected' : ''}>${escapeHtml(choice.label)}</option>`).join('')}</select>`;
    else if (field.type === 'secret_reference') return agentSecretFieldMarkup(field, path, value);
    else if (field.type === 'string' && field.format === 'yaml') control = `<textarea ${attributes} rows="8" spellcheck="false" placeholder="Optional; empty means {}">${escapeHtml(value ?? '')}</textarea>`;
    else control = `<input ${attributes} type="${field.format === 'uri' ? 'url' : 'text'}" value="${escapeHtml(value ?? '')}">`;
    return `<label class="config-field"><span>${escapeHtml(field.label)}</span>${control}<span class="config-help">${escapeHtml(field.help || '')}</span></label>`;
  }).join('');
}

// Uses the explicit stored reference or the server-owned default for one agent credential.
function agentSecretReference(field, configuredReference) {
  if (typeof configuredReference === 'string' && configuredReference.startsWith('game.')) return configuredReference;
  if (typeof field.secret_reference === 'string' && field.secret_reference.startsWith('game.')) return field.secret_reference;
  throw new Error(`Agent secret field ${field.key} has no server-defined reference`);
}

// Presents an agent secret as the same write-only control used by provider credentials.
function agentSecretFieldMarkup(field, path, configuredReference) {
  const reference = agentSecretReference(field, configuredReference);
  const configured = state.configuredSecrets.some(item => item.key === reference && item.configured) && !state.secretDeletions.has(reference);
  return `<label class="config-field ${configured ? 'secret-configured' : 'secret-missing'}" data-agent-secret-field>
    <span>${escapeHtml(field.label)}</span>
    <input type="hidden" data-agent-config-path="${escapeHtml(path)}" data-agent-config-type="secret_reference" data-agent-secret-reference="${escapeHtml(reference)}" value="${escapeHtml(reference)}">
    <span class="secret-control"><input type="password" autocomplete="new-password" data-secret-update="${escapeHtml(reference)}" value="${configured ? '••••••••••••' : ''}" ${configured ? 'data-secret-placeholder="true"' : ''} placeholder="${configured ? 'Enter a replacement key' : 'Paste API key'}"><button class="secondary" type="button" data-reveal-agent-secret="${escapeHtml(reference)}" ${configured ? '' : 'disabled'}>Reveal</button><button class="secondary" type="button" data-delete-agent-secret="${escapeHtml(reference)}" ${configured ? '' : 'disabled'}>Remove</button></span>
    <span class="secret-state">${configured ? 'Configured, hidden' : 'Not configured'}</span>
    <span class="config-help">${escapeHtml(field.help || '')}</span>
  </label>`;
}

// Connects dynamically rendered agent secret controls to write-only secret operations.
function initializeAgentSecretControls(container) {
  container.querySelectorAll('[data-secret-placeholder="true"]').forEach(installSecretReplacementInput);
  container.querySelectorAll('[data-reveal-agent-secret]').forEach(button => button.addEventListener('click', () => revealSecret(button.dataset.revealAgentSecret, button)));
  container.querySelectorAll('[data-delete-agent-secret]').forEach(button => button.addEventListener('click', () => removeAgentSecret(button.dataset.deleteAgentSecret, button)));
  container.querySelectorAll('[data-secret-update]').forEach(input => {
    input.addEventListener('input', refreshDraftActivationWarning);
    input.addEventListener('change', refreshDraftActivationWarning);
  });
}

// Marks one inline agent credential for deletion without exposing its reference selector.
function removeAgentSecret(key, button) {
  const status = state.configuredSecrets.find(item => item.key === key);
  if (status?.configured) state.secretDeletions.add(key);
  const field = button.closest('[data-agent-secret-field]');
  const input = field?.querySelector('[data-secret-update]');
  if (input) { input.type = 'password'; input.value = ''; input.dataset.secretPlaceholder = 'false'; }
  field?.classList.remove('secret-configured');
  field?.classList.add('secret-missing');
  const secretState = field?.querySelector('.secret-state');
  if (secretState) secretState.textContent = status?.configured ? 'Will be removed on save' : 'Not configured';
  field?.querySelector('[data-reveal-agent-secret]')?.setAttribute('disabled', '');
  button.setAttribute('disabled', '');
  refreshDraftActivationWarning();
}

// Renders pairing and runtime controls as one valid, atomic agent configuration.
function renderAgentEditor(field, agents) {
  const configured = agents.human_vs_agent || {};
  const selectedId = configured.factory || state.agentFactories[0]?.id || '';
  const descriptor = state.agentFactories.find(factory => factory.id === selectedId) || state.agentFactories[0] || null;
  const humanVsAgent = agents.mode === 'human_vs_agent';
  return `<div class="config-field agent-editor" data-agent-editor>
    <label class="config-field"><span>Participant pairing</span><select data-agent-mode><option value="human_vs_human" ${humanVsAgent ? '' : 'selected'}>Human vs human</option><option value="human_vs_agent" ${humanVsAgent ? 'selected' : ''} ${state.agentFactories.length ? '' : 'disabled'}>Human vs agent</option></select><span class="config-help">${escapeHtml(field.help || '')}</span></label>
    <div class="agent-settings" data-agent-settings ${humanVsAgent ? '' : 'hidden'}>
      <label class="config-field"><span>Agent</span><select data-agent-factory>${state.agentFactories.map(factory => `<option value="${escapeHtml(factory.id)}" ${factory.id === descriptor?.id ? 'selected' : ''}>${escapeHtml(factory.name)}</option>`).join('')}</select><span class="config-help" data-agent-description>${escapeHtml(descriptor?.description || 'Select one of the agent implementations compiled into this game binary.')}</span></label>
      <div class="agent-specific-fields" data-agent-specific-fields>${agentFactoryFieldsMarkup(descriptor, configured.config || {})}</div>
      <label class="config-field"><span>Action timeout (seconds)</span><input data-agent-timeout type="text" inputmode="decimal" value="${escapeHtml(configured.act_timeout_seconds ?? 30)}"><span class="config-help">Maximum time allowed for the agent to choose one action.</span></label>
      <label class="config-field"><span>Invalid-action limit</span><input data-agent-invalid-limit type="number" min="1" step="1" value="${escapeHtml(configured.invalid_action_limit ?? 3)}"><span class="config-help">End the session after this many invalid agent actions.</span></label>
      <label class="config-field"><span>Random seed</span><input data-agent-seed type="number" step="1" value="${escapeHtml(configured.seed ?? '')}" placeholder="Optional"><span class="config-help">Optional reproducible seed supplied to the agent.</span></label>
    </div>
  </div>`;
}

// Installs conditional behavior for the structured agent editor.
function initializeAgentEditor(editor) {
  const mode = editor.querySelector('[data-agent-mode]');
  const settings = editor.querySelector('[data-agent-settings]');
  mode.addEventListener('change', () => {
    settings.hidden = mode.value !== 'human_vs_agent';
    refreshDraftActivationWarning();
  });
  initializeAgentSecretControls(editor);
  editor.querySelector('[data-agent-factory]')?.addEventListener('change', event => {
    const descriptor = state.agentFactories.find(factory => factory.id === event.target.value);
    editor.querySelector('[data-agent-description]').textContent = descriptor?.description || '';
    const fields = editor.querySelector('[data-agent-specific-fields]');
    fields.innerHTML = agentFactoryFieldsMarkup(descriptor);
    initializeAgentSecretControls(fields);
  });
}

// Renders one structured consent statement with explicit editable fields.
function consentItemMarkup(item, index) {
  return `<section class="consent-item">
    <header class="consent-item-header"><strong>Consent item ${index + 1}</strong><button class="secondary" type="button" data-remove-consent>${icon('trash')}Remove</button></header>
    <div class="consent-item-fields">
      <label><span>Identifier</span><input data-consent-field="id" value="${escapeHtml(item.id || '')}" placeholder="data-processing" autocomplete="off"><span class="config-help">Stable identifier stored with the participant's decision.</span></label>
      <label><span>Title</span><input data-consent-field="title" value="${escapeHtml(item.title || '')}" placeholder="Data processing consent"><span class="config-help">Short heading shown to the participant.</span></label>
    </div>
    <label class="consent-body"><span>Statement</span><textarea data-consent-field="body" rows="5" placeholder="Explain what the participant is agreeing to.">${escapeHtml(item.body || '')}</textarea><span class="config-help">Plain text shown in full before the participant decides.</span></label>
    <label class="consent-required"><input data-consent-field="required" type="checkbox" ${item.required ? 'checked' : ''}><span>Acceptance is required to participate</span></label>
  </section>`;
}

// Renders the repeatable consent editor without exposing its JSON representation.
function renderConsentEditor(field, items) {
  const contents = items.length
    ? items.map(consentItemMarkup).join('')
    : '<p class="consent-empty muted small">No consent statements configured.</p>';
  const templateOptions = CONSENT_TEMPLATE_CHOICES.map(([value, label]) => `<option value="${escapeHtml(value)}">${escapeHtml(label)}</option>`).join('');
  return `<div class="config-field"><span>${escapeHtml(field.label)}</span>${field.help ? `<span class="config-help">${escapeHtml(field.help)}</span>` : ''}<div class="consent-editor" data-consent-editor data-consent-path="${escapeHtml(field.path)}"><div class="consent-items">${contents}</div><div class="consent-actions"><label class="consent-template-field"><span>Consent template</span><select data-consent-template>${templateOptions}</select></label><button class="secondary" type="button" data-add-consent-template>${icon('plus')}Add template</button><button class="secondary" type="button" data-add-consent>${icon('plus')}Blank item</button></div><span class="config-help">Templates come from Parlando's participant-information package and assume consent is the legal basis. Every added template is required by default. Review every placeholder and obtain local approval before collecting data.</span></div></div>`;
}

// Renumbers consent headings and restores the empty state after structural edits.
function refreshConsentEditor(editor) {
  const items = Array.from(editor.querySelectorAll('.consent-item'));
  items.forEach((item, index) => { item.querySelector('.consent-item-header strong').textContent = `Consent item ${index + 1}`; });
  const container = editor.querySelector('.consent-items');
  const empty = container.querySelector('.consent-empty');
  if (!items.length && !empty) container.innerHTML = '<p class="consent-empty muted small">No consent statements configured.</p>';
  if (items.length) empty?.remove();
}

// Chooses a non-conflicting editable identifier for a newly added statement.
function nextConsentId(editor) {
  const ids = new Set(Array.from(editor.querySelectorAll('[data-consent-field="id"]')).map(input => input.value.trim()));
  let number = ids.size + 1;
  while (ids.has(`consent-${number}`)) number += 1;
  return `consent-${number}`;
}

// Replaces template placeholders for installation facts already configured in the dashboard.
function hydrateConsentTemplate(template) {
  const informationVersion = configForm.querySelector('[data-config-path="direct.participant_information_version"]')?.value.trim();
  const institution = state.gameSettings?.institution?.trim();
  const body = typeof template.body === 'function' ? template.body({ informationVersion, institution }) : template.body;
  return { ...template, body };
}

// Adds one ordinary required consent item from the selected reviewed template.
function addConsentTemplate(editor) {
  const selected = editor.querySelector('[data-consent-template]').value;
  const template = CONSENT_TEMPLATES[selected];
  if (!template) return;
  const existingIds = new Set(Array.from(editor.querySelectorAll('[data-consent-field="id"]')).map(input => input.value.trim()));
  if (existingIds.has(template.id)) {
    window.alert('This consent template is already present.');
    return;
  }
  const container = editor.querySelector('.consent-items');
  container.querySelector('.consent-empty')?.remove();
  const count = container.querySelectorAll('.consent-item').length;
  let hydrated;
  try {
    hydrated = hydrateConsentTemplate(template);
  } catch (error) {
    window.alert(error instanceof Error ? error.message : 'Could not resolve consent template.');
    return;
  }
  container.insertAdjacentHTML('beforeend', consentItemMarkup(hydrated, count));
  refreshConsentEditor(editor);
  container.querySelector('.consent-item:last-child [data-consent-field="body"]')?.focus();
}

// Adds and removes structured consent items while preserving all other draft fields.
function initializeConsentEditor(editor) {
  editor.addEventListener('click', event => {
    if (event.target.closest('[data-add-consent-template]')) {
      addConsentTemplate(editor);
      return;
    }
    if (event.target.closest('[data-add-consent]')) {
      const container = editor.querySelector('.consent-items');
      container.querySelector('.consent-empty')?.remove();
      const count = container.querySelectorAll('.consent-item').length;
      container.insertAdjacentHTML('beforeend', consentItemMarkup({ id: nextConsentId(editor), title: '', body: '', required: true }, count));
      refreshConsentEditor(editor);
      container.querySelector('.consent-item:last-child [data-consent-field="id"]')?.focus();
      return;
    }
    const remove = event.target.closest('[data-remove-consent]');
    if (remove) {
      remove.closest('.consent-item')?.remove();
      refreshConsentEditor(editor);
    }
  });
}

// Turns the visual configured-secret mask into an empty field only when replacement begins.
function clearSecretPlaceholder(input) {
  if (input.dataset.secretPlaceholder !== 'true') return;
  input.value = '';
  input.dataset.secretPlaceholder = 'false';
}

// Preserves the hidden-key mask on focus while allowing typing or paste to replace it cleanly.
function installSecretReplacementInput(input) {
  input.addEventListener('keydown', event => {
    if (input.dataset.secretPlaceholder === 'true' && (event.key === 'Backspace' || event.key === 'Delete')) {
      event.preventDefault();
      return;
    }
    if (event.key.length === 1) clearSecretPlaceholder(input);
  });
  input.addEventListener('paste', () => clearSecretPlaceholder(input));
  input.addEventListener('cut', event => {
    if (input.dataset.secretPlaceholder === 'true') event.preventDefault();
  });
  input.addEventListener('input', () => {
    const status = state.configuredSecrets.find(item => item.key === input.dataset.secretUpdate);
    if (status?.configured && !state.secretDeletions.has(input.dataset.secretUpdate) && !input.value) {
      input.value = '••••••••••••';
      input.dataset.secretPlaceholder = 'true';
    }
  });
}

// Returns whether a draft has an existing or newly entered credential after pending deletions.
// Renders the activation blockers computed by the authoritative server validator.
function refreshDraftActivationWarning() {
  const warning = document.getElementById('activationWarning');
  const items = document.getElementById('activationWarningItems');
  if (!warning || !items) return;
  warning.hidden = state.activationIssues.length === 0;
  items.textContent = state.activationIssues.join(' ');
}

// Reconstructs typed Parlando JSON while leaving game YAML and credentials separate.
function configurationFromForm() {
  const config = structuredClone(state.configValue || {});
  configForm.querySelectorAll('[data-config-path]').forEach(control => {
    let value;
    if (control.dataset.configType === 'boolean') value = control.checked;
    else if (control.dataset.configType === 'number') {
      value = Number(control.value);
      if (!Number.isFinite(value)) throw new Error(`${control.dataset.configPath} must be a number written with a decimal point`);
    }
    else if (control.dataset.configType === 'json') value = JSON.parse(control.value);
    else value = control.value;
    setConfigAt(config, control.dataset.configPath, value);
  });
  configForm.querySelectorAll('[data-consent-editor]').forEach(editor => {
    const items = Array.from(editor.querySelectorAll('.consent-item')).map(item => ({
      id: item.querySelector('[data-consent-field="id"]').value.trim(),
      title: item.querySelector('[data-consent-field="title"]').value.trim(),
      body: item.querySelector('[data-consent-field="body"]').value.trim(),
      required: item.querySelector('[data-consent-field="required"]').checked
    }));
    setConfigAt(config, editor.dataset.consentPath, items);
  });
  configForm.querySelectorAll('[data-agent-editor]').forEach(editor => {
    const mode = editor.querySelector('[data-agent-mode]').value;
    if (mode === 'human_vs_human') {
      config.agents = { mode, human_vs_agent: null };
      return;
    }
    const factory = editor.querySelector('[data-agent-factory]').value;
    const agentConfig = {};
    editor.querySelectorAll('[data-agent-config-path]').forEach(control => {
      let value;
      if (control.dataset.agentConfigType === 'boolean') value = control.checked;
      else if (control.dataset.agentConfigType === 'integer') value = Number.parseInt(control.value, 10);
      else if (control.dataset.agentConfigType === 'number') value = Number(control.value);
      else value = control.value;
      if (control.value !== '' || control.dataset.agentConfigType === 'boolean') setConfigAt(agentConfig, control.dataset.agentConfigPath, value);
    });
    const timeout = Number(editor.querySelector('[data-agent-timeout]').value);
    const invalidLimit = Number(editor.querySelector('[data-agent-invalid-limit]').value);
    const seedValue = editor.querySelector('[data-agent-seed]').value;
    if (!Number.isFinite(timeout) || timeout <= 0) throw new Error('Agent action timeout must be a positive number');
    if (!Number.isInteger(invalidLimit) || invalidLimit <= 0) throw new Error('Agent invalid-action limit must be a positive integer');
    if (seedValue !== '' && !Number.isInteger(Number(seedValue))) throw new Error('Agent random seed must be an integer');
    config.agents = {
      mode,
      human_vs_agent: {
        factory,
        act_timeout_seconds: timeout,
        invalid_action_limit: invalidLimit,
        seed: seedValue === '' ? null : Number(seedValue),
        config: agentConfig
      }
    };
  });
  delete config.game;
  return config;
}

// Asks the compiled game to parse and validate the current YAML without saving it.
async function validateGameYaml() {
  const result = document.getElementById('gameYamlValidation');
  result.className = 'validation-result small';
  result.textContent = 'Validating…';
  const id = encodeURIComponent(state.experiment.experiment_id);
  const response = await adminFetch(state, `/api/admin/experiments/${id}/config/validate`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': state.csrfToken || '' },
    body: JSON.stringify({ game_yaml: document.getElementById('gameYaml').value })
  });
  if (!response.ok) {
    result.classList.add('invalid');
    result.textContent = await response.text();
    return false;
  }
  result.classList.add('valid');
  result.textContent = 'Valid';
  return true;
}

// Renders named game secrets with masked values and explicit reveal/remove actions.
function renderGameSecrets() {
  const list = document.getElementById('gameSecretList');
  if (!list) return;
  const agentReferences = new Set(Array.from(configForm.querySelectorAll('[data-agent-secret-reference]')).map(input => input.dataset.agentSecretReference));
  const visibleKeys = state.gameSecretKeys.filter(key => !agentReferences.has(key));
  list.innerHTML = visibleKeys.length ? visibleKeys.map(key => {
    const configured = state.configuredSecrets.some(item => item.key === key) && !state.secretDeletions.has(key);
    return `<div class="game-secret-row"><span class="game-secret-key">${escapeHtml(key.slice(5))}</span><input type="password" autocomplete="new-password" data-secret-update="${escapeHtml(key)}" value="${configured ? '••••••••••••' : ''}" ${configured ? 'data-secret-placeholder="true"' : ''} placeholder="${configured ? 'Enter a replacement value' : 'Enter value'}"><button class="secondary" type="button" data-reveal-secret="${escapeHtml(key)}" ${configured ? '' : 'disabled'}>Reveal</button><button class="secondary" type="button" data-delete-secret="${escapeHtml(key)}">Remove</button></div>`;
  }).join('') : '<p class="muted small">No game-specific secrets configured.</p>';
  list.querySelectorAll('[data-reveal-secret]').forEach(button => button.addEventListener('click', () => revealSecret(button.dataset.revealSecret, button)));
  list.querySelectorAll('[data-delete-secret]').forEach(button => button.addEventListener('click', () => removeSecret(button.dataset.deleteSecret)));
  list.querySelectorAll('[data-secret-placeholder="true"]').forEach(installSecretReplacementInput);
}

// Adds one named game credential row without assigning a value until save.
function addGameSecret() {
  const input = document.getElementById('newGameSecretName');
  const name = input.value.trim();
  if (!name || !/^[A-Za-z0-9_.-]+$/.test(name)) {
    window.alert('Use letters, digits, dots, dashes, or underscores for the secret name.');
    return;
  }
  const key = `game.${name}`;
  if (!state.gameSecretKeys.includes(key)) state.gameSecretKeys.push(key);
  state.secretDeletions.delete(key);
  input.value = '';
  renderGameSecrets();
}

// Marks an experiment-owned secret for deletion when the new revision is saved.
function removeSecret(key) {
  const status = state.configuredSecrets.find(item => item.key === key);
  if (status?.source === 'experiment') state.secretDeletions.add(key);
  if (key.startsWith('game.')) state.gameSecretKeys = state.gameSecretKeys.filter(item => item !== key);
  else {
    const input = configForm.querySelector(`[data-secret-update="${CSS.escape(key)}"]`);
    if (input) {
      input.value = '';
      input.dataset.secretPlaceholder = 'false';
      input.placeholder = status?.source === 'server' ? 'Server value will remain available' : 'Will be removed on save';
      const field = input.closest('.config-field');
      field?.classList.remove('secret-configured');
      field?.classList.add('secret-missing');
      const secretState = field?.querySelector('.secret-state');
      if (secretState) secretState.textContent = status?.source === 'server' ? 'Configured server fallback remains' : 'Will be removed on save';
    }
  }
  renderGameSecrets();
  refreshDraftActivationWarning();
}

// Fetches one secret only after an explicit administrator action and exposes it for copying.
async function revealSecret(key, button) {
  const input = configForm.querySelector(`[data-secret-update="${CSS.escape(key)}"]`);
  if (button.dataset.revealed === 'true') {
    if (input) { input.type = 'password'; input.value = '••••••••••••'; input.dataset.secretPlaceholder = 'true'; }
    button.dataset.revealed = 'false';
    button.textContent = 'Reveal';
    return;
  }
  const id = encodeURIComponent(state.experiment.experiment_id);
  const response = await adminFetch(state, `/api/admin/experiments/${id}/secrets/reveal`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': state.csrfToken || '' },
    body: JSON.stringify({ key })
  });
  if (!response.ok) { window.alert(await response.text()); return; }
  const data = await response.json();
  if (input) { input.type = 'text'; input.value = data.value; input.dataset.secretPlaceholder = 'false'; input.focus(); input.select(); }
  button.dataset.revealed = 'true';
  button.textContent = 'Hide';
}

// Creates an inactive experiment from the compiled game's current defaults.
async function createExperiment(experimentId, notes) {
  const response = await adminFetch(state, '/api/admin/experiments', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': state.csrfToken || '' },
    body: JSON.stringify({ experiment_id: experimentId, notes: notes.trim() || null })
  });
  if (!response.ok) throw new Error(await response.text());
  state.experiment = { experiment_id: experimentId };
  await loadExperiment();
  await selectExperiment(experimentId);
}

// Clones the selected configuration into a current-version inactive experiment.
async function cloneSelectedExperiment() {
  const source = state.experiment;
  if (!source) return;
  const experimentId = window.prompt('Experiment ID for the clone (unique and immutable)', `${source.experiment_id}-copy`);
  if (!experimentId) return;
  const response = await adminFetch(state, `/api/admin/experiments/${encodeURIComponent(source.experiment_id)}/clone`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': state.csrfToken || '' },
    body: JSON.stringify({ experiment_id: experimentId })
  });
  if (!response.ok) throw new Error(await response.text());
  state.experiment = { experiment_id: experimentId };
  await loadExperiment();
  await selectExperiment(experimentId);
}

// Updates catalogue metadata without changing runtime configuration.
async function updateCatalogue(changes) {
  const experiment = state.experiment;
  if (!experiment) return;
  const response = await adminFetch(state, `/api/admin/experiments/${encodeURIComponent(experiment.experiment_id)}/catalogue`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': state.csrfToken || '' },
    body: JSON.stringify({
      pinned: changes.pinned ?? experiment.pinned,
      notes: changes.notes ?? experiment.notes
    })
  });
  if (!response.ok) throw new Error(await response.text());
  await loadExperiment();
}

// Saves Markdown notes as catalogue metadata without creating a configuration revision.
async function saveNotes() {
  const selectedId = state.experiment?.experiment_id;
  if (!selectedId) return;
  const value = state.notesSource.trim() || null;
  const response = await adminFetch(state, `/api/admin/experiments/${encodeURIComponent(selectedId)}/catalogue`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': state.csrfToken || '' },
    body: JSON.stringify({ pinned: state.experiment.pinned, notes: value })
  });
  if (!response.ok) throw new Error(await response.text());
  if (state.experiment?.experiment_id !== selectedId) return;
  state.experiment.notes = value;
  const catalogue = state.experiments.find(item => item.experiment_id === selectedId);
  if (catalogue) catalogue.notes = value;
  state.notesSource = value || '';
  state.notesDirty = false;
  notesSaveState.textContent = 'Saved';
  renderExperiment();
}

// Validates and stores the editor contents as a new immutable revision.
async function saveConfigRevision() {
  const expectedRevision = Number(configRevision.textContent.replace(/\D/g, ''));
  let config;
  try { config = configurationFromForm(); }
  catch (error) { window.alert(`Invalid structured value: ${error.message}`); return; }
  const secretUpdates = {};
  configForm.querySelectorAll('[data-secret-update]').forEach(control => {
    if (control.value && control.dataset.secretPlaceholder !== 'true') secretUpdates[control.dataset.secretUpdate] = control.value;
  });
  const id = encodeURIComponent(state.experiment.experiment_id);
  const response = await adminFetch(state, `/api/admin/experiments/${id}/config`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': state.csrfToken || '' },
    body: JSON.stringify({
      expected_revision: expectedRevision,
      config,
      game_yaml: document.getElementById('gameYaml').value,
      secret_updates: secretUpdates,
      secret_deletions: [...state.secretDeletions],
      change_summary: document.getElementById('changeSummary').value || null
    })
  });
  if (!response.ok) { window.alert(await response.text()); return; }
  await loadExperiment();
}

// Copies the latest committed shared settings into controls when no local draft exists.
function synchronizeGameSettingsForm() {
  institutionInput.value = state.gameSettings?.institution || '';
  adminAllowedIpRangesInput.value = (state.gameSettings?.admin_allowed_ip_ranges || []).join('\n');
  speechmaticsRealtimeUrlInput.value = state.gameSettings?.speechmatics_realtime_url || 'wss://eu.rt.speechmatics.com/v2';
  ttsBaseUrlInput.value = state.gameSettings?.tts_base_url || 'wss://api.elevenlabs.io';
  prolificApiBaseUrlInput.value = state.gameSettings?.prolific_api_base_url || 'https://api.prolific.com';
  renderGameProviderSecrets();
}

// Preserves a revisioned local draft across periodic dashboard refreshes.
function markGameSettingsDirty() {
  if (!state.gameSettingsDirty) state.gameSettingsDraftRevision = state.gameSettings?.revision ?? null;
  state.gameSettingsDirty = true;
  showGameSettingsSaveState('Unsaved changes', 'unsaved');
}

// Displays persistent save progress or validation feedback beside the shared save action.
function showGameSettingsSaveState(message, tone = '') {
  const confirmation = document.getElementById('gameSettingsSaved');
  clearTimeout(state.gameSettingsConfirmationTimer);
  confirmation.classList.remove('fading', 'unsaved', 'error');
  if (tone) confirmation.classList.add(tone);
  confirmation.textContent = message;
  confirmation.hidden = false;
}

// Renders installation-wide provider credentials beside their matching provider settings.
function renderGameProviderSecrets() {
  const definitions = [
    ['speechmaticsProviderSecret', 'speechmatics.api_key', 'API key', 'Used to open live transcription streams.'],
    ['ttsProviderSecret', 'tts.api_key', 'API key', 'Used to synthesize agent speech.'],
    ['prolificProviderSecret', 'prolific.api_token', 'API token', 'Used only by the server to verify the workspace, studies, launches, completion paths, and submission status.']
  ];
  definitions.forEach(([containerId, key, label, help]) => {
    const container = document.getElementById(containerId);
    const status = state.gameProviderSecrets.find(item => item.key === key) || {};
    const pendingValue = state.gameProviderSecretUpdates.get(key);
    const removalPending = state.gameProviderSecretDeletions.has(key);
    const stored = Boolean(status.configured);
    const configured = Boolean(pendingValue) || (stored && !removalPending);
    const placeholder = stored && pendingValue === undefined && !removalPending;
    const value = pendingValue ?? (placeholder ? '••••••••••••' : '');
    const secretState = pendingValue !== undefined
      ? 'Replacement pending save'
      : removalPending
        ? 'Removal pending save'
        : configured
          ? `Configured (${escapeHtml(status.source)}), hidden`
          : 'Not configured';
    container.innerHTML = `<label class="config-field ${configured ? 'secret-configured' : 'secret-missing'}"><span>${escapeHtml(label)}</span><span class="secret-control"><input type="password" autocomplete="new-password" data-game-provider-secret="${escapeHtml(key)}" value="${escapeHtml(value)}" ${placeholder ? 'data-game-secret-placeholder="true"' : ''} placeholder="${stored ? 'Enter a replacement key' : 'Paste API key'}"><button class="secondary" type="button" data-reveal-game-secret="${escapeHtml(key)}" ${stored && !removalPending ? '' : 'disabled'}>Reveal</button><button class="secondary" type="button" data-delete-game-secret="${escapeHtml(key)}" ${status.source === 'game' && !removalPending ? '' : 'disabled'}>Remove</button></span><span class="secret-state">${secretState}</span><span class="config-help">${escapeHtml(help)}</span></label>`;
    const input = container.querySelector('[data-game-provider-secret]');
    input.addEventListener('beforeinput', event => {
      if (input.dataset.gameSecretPlaceholder === 'true' && (event.inputType.startsWith('insert') || event.inputType.startsWith('delete'))) {
        input.value = '';
        input.dataset.gameSecretPlaceholder = 'false';
      }
    });
    input.addEventListener('input', () => {
      input.dataset.gameSecretPlaceholder = 'false';
      if (input.value) {
        state.gameProviderSecretUpdates.set(key, input.value);
        state.gameProviderSecretDeletions.delete(key);
      } else {
        state.gameProviderSecretUpdates.delete(key);
        if (status.source === 'game') state.gameProviderSecretDeletions.add(key);
      }
      markGameSettingsDirty();
    });
    container.querySelector('[data-delete-game-secret]').addEventListener('click', () => {
      state.gameProviderSecretUpdates.delete(key);
      state.gameProviderSecretDeletions.add(key);
      markGameSettingsDirty();
      renderGameProviderSecrets();
    });
    container.querySelector('[data-reveal-game-secret]').addEventListener('click', event => revealGameProviderSecret(event.currentTarget));
  });
}

// Reveals or re-hides one game-wide provider credential for deliberate copying.
async function revealGameProviderSecret(button) {
  const key = button.dataset.revealGameSecret;
  const input = document.querySelector(`[data-game-provider-secret="${CSS.escape(key)}"]`);
  if (button.dataset.revealed === 'true') {
    input.type = 'password';
    input.value = '••••••••••••';
    input.dataset.gameSecretPlaceholder = 'true';
    button.dataset.revealed = 'false';
    button.textContent = 'Reveal';
    return;
  }
  const response = await adminFetch(state, '/api/admin/game/secrets/reveal', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': state.csrfToken || '' },
    body: JSON.stringify({ key })
  });
  if (!response.ok) { window.alert(await response.text()); return; }
  const data = await response.json();
  input.type = 'text';
  input.value = data.value;
  input.dataset.gameSecretPlaceholder = 'false';
  input.focus();
  input.select();
  button.dataset.revealed = 'true';
  button.textContent = 'Hide';
}

// Briefly confirms that the server committed the complete game-settings transaction.
function flashGameSettingsSaved() {
  const confirmation = document.getElementById('gameSettingsSaved');
  clearTimeout(state.gameSettingsConfirmationTimer);
  confirmation.classList.remove('fading', 'unsaved', 'error');
  confirmation.textContent = 'All game settings saved';
  confirmation.hidden = false;
  state.gameSettingsConfirmationTimer = setTimeout(() => {
    confirmation.classList.add('fading');
    setTimeout(() => { confirmation.hidden = true; }, 180);
  }, 1800);
}

// Saves installation-wide game and administrator-network settings.
async function saveSharedGameSettings() {
  const adminAllowedIpRanges = adminAllowedIpRangesInput.value
    .split(/[\n,]/)
    .map(value => value.trim())
    .filter(Boolean);
  const saveButton = document.getElementById('saveGameSettings');
  saveButton.disabled = true;
  let response;
  try {
    response = await adminFetch(state, '/api/admin/game/settings', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'X-CSRF-Token': state.csrfToken || '' },
      body: JSON.stringify({
        expected_revision: state.gameSettingsDraftRevision ?? state.gameSettings.revision,
        institution: institutionInput.value,
        admin_allowed_ip_ranges: adminAllowedIpRanges,
        speechmatics_realtime_url: speechmaticsRealtimeUrlInput.value,
        tts_base_url: ttsBaseUrlInput.value,
        prolific_api_base_url: prolificApiBaseUrlInput.value,
        secret_updates: Object.fromEntries(state.gameProviderSecretUpdates),
        secret_deletions: [...state.gameProviderSecretDeletions]
      })
    });
  } catch (error) {
    saveButton.disabled = false;
    showGameSettingsSaveState(`Could not save: ${error.message}`, 'error');
    return;
  }
  saveButton.disabled = false;
  if (!response.ok) {
    showGameSettingsSaveState(`Not saved: ${await response.text()}`, 'error');
    return;
  }
  state.gameProviderSecretDeletions = new Set();
  state.gameProviderSecretUpdates = new Map();
  state.gameSettingsDirty = false;
  state.gameSettingsDraftRevision = null;
  state.loadedConfigKey = null;
  flashGameSettingsSaved();
  await loadExperiment();
}

// Loads one selected session, its participants, and its initial event bundle.
async function selectSession(sessionId) {
  state.selected = sessionId;
  state.lastEventIndex = 0;
  renderSessions();
  liveStatus.textContent = 'Loading';
  const response = await adminFetch(state, runtimeApi(`sessions/${sessionId}`));
  if (!response.ok) throw new Error(await response.text());
  const data = await response.json();
  state.selectedSession = data.session;
  state.selectedParticipants = data.participants || [];
  renderSummary(data.session, state.selectedParticipants);
  sessionDetail.hidden = false;
  state.events = [];
  state.eventBundles = data.event_bundles || [];
  mergeEvents(data.events || []);
  renderEventBundles();
  liveStatus.textContent = 'Live';
}

// Switches among the selected experiment's sessions, export, and configuration views.
function renderTabs() {
  if (!state.experiment) {
    Object.values(tabPanels).forEach(panel => { panel.hidden = true; });
    return;
  }
  tabButtons.forEach(button => button.classList.toggle('active', button.dataset.tab === state.activeTab));
  Object.entries(tabPanels).forEach(([name, panel]) => {
    panel.hidden = name !== state.activeTab;
  });
}

// Switches between game-level dashboard areas and the experiment workspace.
function showScope(scope) {
  state.activeScope = scope;
  scopeButtons.forEach(button => button.classList.toggle('active', button.dataset.scope === scope));
  scopePanels.forEach(panel => { panel.hidden = panel.dataset.scopePanel !== scope; });
  menuButton.hidden = scope !== 'experiments';
  closeExperimentMenu();
  if (scope === 'operations') loadLoad().catch(error => { loadUpdated.textContent = error.message; });
}

// Renders a concise archival data record for the selected experiment.
function renderPrivacy() {
  const privacy = state.privacy;
  if (!privacy) return;
  const configurationRows = (privacy.configuration || []).map(item => `<tr><td>${escapeHtml(item.setting)}</td><td>${escapeHtml(item.status)}</td><td>${escapeHtml(item.detail)}</td></tr>`).join('');
  const storageRows = (privacy.storage || []).map(item => `<tr><td>${escapeHtml(item.category)}</td><td>${escapeHtml(item.purpose)}</td><td>${escapeHtml(item.detail)}</td></tr>`).join('');
  const nonRetainedRows = (privacy.not_retained || []).map(item => `<tr><td>${escapeHtml(item.category)}</td><td>${escapeHtml(item.behavior)}</td><td>${escapeHtml(item.boundary)}</td></tr>`).join('');
  const serviceRows = (privacy.external_services || []).map(service => `<tr><td>${escapeHtml(service.service)}</td><td>${escapeHtml(service.purpose)}</td><td>${escapeHtml(service.data_sent)}</td></tr>`).join('') || '<tr><td>None</td><td>Not applicable</td><td>No external speech service is enabled for this experiment.</td></tr>';
  const exportRows = (privacy.exports?.included_fields || []).map(item => `<tr><td>${escapeHtml(item.section)}</td><td>${escapeHtml(item.description)}</td></tr>`).join('');
  const notWrittenItems = (privacy.exports?.not_written || []).map(item => `<li>${escapeHtml(item)}</li>`).join('');
  const providerRecord = (privacy.external_services || []).length ? ', and the processor agreements and retention settings for the listed external speech services' : '';
  privacyContent.innerHTML = `
    <article class="privacy-document">
      <header class="section-intro"><h2>Data processing record: ${escapeHtml(privacy.experiment_id || state.experiment?.experiment_id || '')}</h2><p class="muted">Generated ${escapeHtml(fmtTime(privacy.generated_at))}; Parlando data-handling contract version ${escapeHtml(privacy.privacy_contract_version)}.</p></header>
      <h3>Scope and responsibility</h3><div class="table-wrap"><table class="privacy-table"><tbody><tr><th>Purpose</th><td>${escapeHtml(privacy.overview?.purpose || '')}</td></tr><tr><th>Primary storage</th><td>${escapeHtml(privacy.overview?.primary_storage || '')}</td></tr><tr><th>Access through Parlando</th><td>${escapeHtml(privacy.overview?.access || '')}</td></tr><tr><th>Retention</th><td>${escapeHtml(privacy.overview?.retention || '')}</td></tr></tbody></table></div>
      <h3>Privacy-relevant experiment settings</h3><div class="table-wrap"><table class="privacy-table"><thead><tr><th>Setting</th><th>Status</th><th>Effect on data processing</th></tr></thead><tbody>${configurationRows}</tbody></table></div>
      <h3>Data retained in Parlando's SQLite database</h3><div class="table-wrap"><table class="privacy-table"><thead><tr><th>Information</th><th>Why it is processed</th><th>What is retained</th></tr></thead><tbody>${storageRows}</tbody></table></div>
      <h3>Data not retained by Parlando</h3><div class="table-wrap"><table class="privacy-table"><thead><tr><th>Information</th><th>Parlando guarantee</th><th>Boundary of the guarantee</th></tr></thead><tbody>${nonRetainedRows}</tbody></table></div>
      <h3>External speech services</h3><div class="table-wrap"><table class="privacy-table"><thead><tr><th>Service</th><th>Purpose</th><th>Data sent</th></tr></thead><tbody>${serviceRows}</tbody></table></div>
      <h3>Corpus export</h3><p>${escapeHtml(privacy.exports?.structure || '')} ${escapeHtml(privacy.exports?.scope || '')}</p><p>${escapeHtml(privacy.exports?.identifiers || '')} ${escapeHtml(privacy.exports?.timing || '')}</p><p><strong>How fields are selected:</strong> ${escapeHtml(privacy.exports?.selection_rule || '')}</p><div class="table-wrap"><table class="privacy-table"><thead><tr><th>Part of the corpus</th><th>What it contains</th></tr></thead><tbody>${exportRows}</tbody></table></div><p><strong>Stored or used by Parlando but not written to the corpus:</strong></p><ul>${notWrittenItems}</ul><p><strong>Before sharing:</strong> ${escapeHtml(privacy.exports?.detail || '')}</p><p class="muted">Schema ${escapeHtml(privacy.exports?.schema_id || '')}; available as ${escapeHtml((privacy.exports?.formats || []).join(', '))}. <a href="/api/admin/export-schema" download>Download the machine-readable schema</a>.</p>
      <h3>Consent evidence and deletion</h3><div class="table-wrap"><table class="privacy-table"><thead><tr><th>Area</th><th>Behavior</th></tr></thead><tbody><tr><td>Consent evidence</td><td>${escapeHtml(privacy.consent_evidence?.detail || '')}</td></tr><tr><td>Participant deletion</td><td>${escapeHtml(privacy.participant_deletion?.detail || '')}</td></tr></tbody></table></div>
      <h3>Scope of this record</h3><p>This record documents behavior enforced by Parlando for this experiment. Store it with the exported data. Supplement it with the controller, legal basis, retention period, hosting arrangements${providerRecord} that apply at your institution.</p>
    </article>`;
}

// Loads privacy status for the selected experiment and updates its report links.
async function loadPrivacy() {
  if (!state.experiment) return;
  if (state.privacy) {
    renderPrivacy();
    return;
  }
  const id = encodeURIComponent(state.experiment.experiment_id);
  document.getElementById('privacyMarkdown').href = `/api/admin/experiments/${id}/privacy.md`;
  document.getElementById('privacyJson').href = `/api/admin/experiments/${id}/privacy.json`;
  const response = await adminFetch(state, `/api/admin/experiments/${id}/privacy`);
  if (!response.ok) throw new Error(await response.text());
  state.privacy = await response.json();
  renderPrivacy();
}

// Constrains one draggable catalogue width to useful desktop limits.
function clampWidth(value, minimum, maximum) {
  return Math.min(maximum, Math.max(minimum, value));
}

// Installs pointer and keyboard resizing for a full-height structural divider.
function makeResizable(handle, variable, minimum, maximum) {
  let startX = 0;
  let startWidth = 0;
  const resize = event => {
    document.documentElement.style.setProperty(variable, `${clampWidth(startWidth + event.clientX - startX, minimum, maximum)}px`);
  };
  const finish = event => {
    handle.classList.remove('dragging');
    handle.releasePointerCapture(event.pointerId);
    handle.removeEventListener('pointermove', resize);
    handle.removeEventListener('pointerup', finish);
  };
  handle.addEventListener('pointerdown', event => {
    startX = event.clientX;
    startWidth = parseFloat(getComputedStyle(document.documentElement).getPropertyValue(variable));
    handle.classList.add('dragging');
    handle.setPointerCapture(event.pointerId);
    handle.addEventListener('pointermove', resize);
    handle.addEventListener('pointerup', finish);
  });
  handle.addEventListener('keydown', event => {
    if (!['ArrowLeft', 'ArrowRight'].includes(event.key)) return;
    const current = parseFloat(getComputedStyle(document.documentElement).getPropertyValue(variable));
    const delta = event.key === 'ArrowRight' ? 16 : -16;
    document.documentElement.style.setProperty(variable, `${clampWidth(current + delta, minimum, maximum)}px`);
    event.preventDefault();
  });
}

// Closes the responsive experiment catalogue drawer.
function closeExperimentMenu() {
  document.body.classList.remove('sidebar-open');
  menuButton.setAttribute('aria-expanded', 'false');
}

// Toggles the responsive experiment catalogue drawer.
function toggleExperimentMenu() {
  const open = !document.body.classList.contains('sidebar-open');
  document.body.classList.toggle('sidebar-open', open);
  menuButton.setAttribute('aria-expanded', String(open));
}

// Renders labeled session state, health, purpose, recruitment, and contextual timing facts.
function renderSummary(session, participantRows) {
  const live = sessionLiveness(session.session_id);
  const health = live?.health || (session.lifecycle === 'ended' ? 'ended' : 'unavailable');
  const expectedHealth = session.lifecycle === 'forming' ? 'waiting' : session.lifecycle === 'ended' ? 'ended' : 'live';
  const isProlific = participantRows.some(row => row.identity_provider === 'prolific');
  const healthDetail = live ? `Meaningful activity ${formatAge(new Date(live.meaningful_activity_at).getTime())}` : session.lifecycle === 'ended' ? '' : 'Session is not present in this process runtime';
  const facts = [
    `<div><dt>Session state</dt><dd>${statusLabel(session.lifecycle)}</dd></div>`,
    `<div><dt>Health</dt><dd>${livenessBadge(health, healthDetail)}</dd></div>`,
    `<div><dt>Purpose</dt><dd>${statusLabel(session.purpose || 'research')}</dd></div>`,
    `<div><dt>Recruitment</dt><dd>${isProlific ? 'Prolific' : 'Direct'}</dd></div>`
  ];
  if (session.lifecycle === 'forming') {
    facts.push(`<div><dt>Waiting since</dt><dd>${escapeHtml(fmtTime(session.waiting_started_at || session.created_at))}</dd></div>`);
    facts.push(`<div><dt>Waiting deadline</dt><dd>${escapeHtml(fmtTime(session.waiting_deadline_at))}</dd></div>`);
  }
  if (session.lifecycle === 'ended') {
    facts.push(`<div><dt>End reason</dt><dd>${sessionEndBadge(session)}</dd></div>`);
    const waited = unsuccessfulWait(session).replace(/^ · /, '');
    if (waited) facts.push(`<div><dt>Unsuccessful wait</dt><dd>${escapeHtml(waited)}</dd></div>`);
  }
  if (session.lifecycle === 'running' && health !== expectedHealth) {
    facts.push(`<div><dt>Last meaningful activity</dt><dd>${escapeHtml(fmtTime(session.last_meaningful_activity_at))}</dd></div>`);
  }
  summary.innerHTML = `
    <div class="session-summary-heading"><h2>Session ${escapeHtml(session.session_id)}</h2>${session.dialogue_id ? `<strong class="session-dialogue-name">${escapeHtml(session.dialogue_id)}</strong>` : ''}</div>
    <dl class="session-facts">${facts.join('')}</dl>
    ${participantAssignments(participantRows, session)}
  `;
}

// Returns the durable participant identifier when no richer agent identity exists.
function participantLabel(row) {
  return row.research_id || row.participant_kind || 'Participant';
}

// Formats server agent type and version without exposing its synthetic research identifier.
function participantDisplayName(row) {
  if (row.participant_kind !== 'agent') return participantLabel(row);
  const metadata = row.metadata || {};
  const type = String(metadata.agent_name || metadata.agent_type || 'Agent').replace(/Agent$/, '');
  const version = metadata.agent_version;
  return version ? `${type} v ${version}` : type;
}

// Visualizes reproducible agent configuration identity separately from its display name.
function configurationIdentityMarkup(row) {
  if (row.participant_kind !== 'agent') return '';
  const metadata = row.metadata || {};
  const fingerprint = metadata.configuration_fingerprint;
  if (!fingerprint) return '<span class="muted small">Configuration identity unavailable</span>';
  const digest = fingerprint.includes(':') ? fingerprint.split(':').pop() : fingerprint;
  return `<details class="configuration-identity">
    <summary title="View agent configuration identity">${icon('settings')}${escapeHtml(digest.slice(0, 8))}</summary>
    <div class="configuration-identity-card">
      <strong>Agent configuration identity</strong>
      <dl>
        <dt>Agent</dt><dd>${escapeHtml(metadata.agent_name || 'Agent')}</dd>
        <dt>Version</dt><dd>${escapeHtml(metadata.agent_version || 'Unversioned')}</dd>
        <dt>Factory</dt><dd>${escapeHtml(metadata.factory || 'Unavailable')}</dd>
      </dl>
      <div class="configuration-fingerprint" title="Select to copy">${escapeHtml(fingerprint)}</div>
      <p class="muted small">Same badge means the same normalized settings and secret references. Secret values are never included.</p>
    </div>
  </details>`;
}

// Renders a small human or software-agent symbol without an external icon dependency.
function participantIcon(kind) {
  return icon(kind === 'agent' ? 'bot' : 'user', 'participant-icon');
}

// Returns current transport health for one participant without conflating it with participant state.
function participantTransportHealth(row) {
  const live = sessionLiveness(row.session_id);
  const participant = live?.participants?.find(item => item.role === row.role);
  return participant?.game_health || row.connection_status || 'unavailable';
}

// Describes a participant result in terms of the shared cause and this recipient's consequence.
function participantOutcomePresentation(outcome, session) {
  const cause = session.session_end?.cause?.type;
  if (outcome === 'completed') return { status: 'completed', label: 'Completed', detail: '' };
  if (outcome === 'left_waiting_room') return { status: 'explicit-left', label: 'Chose to leave while waiting', detail: 'The participant used Parlando’s leave action.' };
  if (outcome === 'left_game') return { status: 'explicit-left', label: 'Chose to leave', detail: 'The participant used Parlando’s leave action.' };
  if (outcome === 'connection_lost') return { status: 'connection-lost', label: 'Connection lost', detail: 'The browser connection ended and the participant did not reconnect. Parlando cannot distinguish a closed tab from a network interruption.' };
  if (outcome === 'partner_left' && cause === 'participant_left') return { status: 'explicit-left', label: 'Partner chose to leave', detail: 'The partner used Parlando’s leave action.' };
  if (outcome === 'partner_left' && cause === 'reconnect_timed_out') return { status: 'connection-lost', label: 'Partner connection lost', detail: 'The partner’s browser connection ended and they did not reconnect. Parlando cannot distinguish a closed tab from a network interruption.' };
  if (outcome === 'partner_left') return { status: 'partner-left', label: 'Partner left', detail: '' };
  if (outcome === 'partner_unavailable') return { status: 'no-partner', label: 'No partner found', detail: '' };
  if (outcome === 'idle_limit_reached') return { status: 'inactivity-timeout', label: 'Inactivity timeout', detail: '' };
  if (outcome === 'lifetime_limit_reached') return { status: 'duration-limit', label: 'Maximum duration reached', detail: '' };
  if (outcome === 'technical_failure') return { status: 'technical-failure', label: 'Technical failure', detail: '' };
  return { status: 'unavailable', label: statusText(outcome), detail: '' };
}

// Renders participant state only when it is not the ordinary active state or has an outcome.
function participantStateMarkup(row, session) {
  const live = sessionLiveness(row.session_id);
  const liveParticipant = live?.participants?.find(item => item.role === row.role);
  const phase = liveParticipant?.participant_state || row.participant_state?.state || 'unavailable';
  const outcome = row.participant_state?.result?.outcome || row.terminal_result?.outcome;
  const phaseMarkup = phase === 'active' || (phase === 'ended' && outcome) ? '' : statusLabel(phase);
  if (!outcome) return phaseMarkup;
  const presentation = participantOutcomePresentation(outcome, session);
  const title = presentation.detail ? ` title="${escapeHtml(presentation.detail)}"` : '';
  return `${phaseMarkup}<span${title}>${namedStatusLabel(presentation.status, presentation.label)}</span>`;
}

// Renders transport health only when it differs from the participant state's normal health.
function participantTransportMarkup(row) {
  const live = sessionLiveness(row.session_id);
  const liveParticipant = live?.participants?.find(item => item.role === row.role);
  const phase = liveParticipant?.participant_state || row.participant_state?.state || 'unavailable';
  if (phase === 'ended') return '';
  const health = participantTransportHealth(row);
  const expected = row.participant_kind === 'agent' ? 'server' : phase === 'waiting' ? 'waiting' : 'live';
  return health === expected ? '' : livenessBadge(health);
}

// Shows Prolific check state as one compact icon and keeps provider identifiers in its disclosure.
function prolificDetailsMarkup(row) {
  if (row.identity_provider !== 'prolific' || !row.prolific_participant_id) return '';
  const checked = Boolean(row.prolific_reconciled_at);
  const providerDetail = row.prolific_status ? ` Provider status: ${row.prolific_status}.` : '';
  const tooltip = checked
    ? `Prolific submission status checked.${providerDetail} Open details.`
    : 'Prolific submission status has not been checked. Open details.';
  return `<details class="configuration-identity provider-details ${checked ? 'checked' : 'unchecked'}">
    <summary title="${escapeHtml(tooltip)}" aria-label="${escapeHtml(tooltip)}">${icon(checked ? 'check' : 'help')}</summary>
    <div class="configuration-identity-card">
      <strong>Prolific recruitment</strong>
      <dl>
        <dt>Status check</dt><dd>${checked ? 'Has been checked' : 'Has not been checked'}</dd>
        <dt>Participant</dt><dd>${escapeHtml(row.prolific_participant_id)}</dd>
        <dt>Study</dt><dd>${escapeHtml(row.prolific_study_id)}</dd>
        <dt>Submission</dt><dd>${escapeHtml(row.prolific_session_id)}</dd>
        ${row.prolific_status ? `<dt>Provider status</dt><dd>${escapeHtml(row.prolific_status)}</dd>` : ''}
        ${row.prolific_entered_code ? `<dt>Entered code</dt><dd>${escapeHtml(row.prolific_entered_code)}</dd>` : ''}
        ${row.prolific_return_requested_at ? `<dt>Return requested</dt><dd>${escapeHtml(fmtTime(row.prolific_return_requested_at))}</dd>` : ''}
      </dl>
    </div>
  </details>`;
}

// Places participant phase, terminal outcome, and exceptional transport status below the name.
function participantStatusMarkup(row, session) {
  return `<span class="participant-status-line">${participantStateMarkup(row, session)}${participantTransportMarkup(row)}</span>`;
}

// Shows disconnect timing only while a future reconnect deadline remains actionable.
function participantDisconnectMarkup(row) {
  if (!row.disconnected_at) return '';
  if (row.participant_state?.state === 'ended' || row.terminal_result) return '';
  const health = participantTransportHealth(row);
  const reconnectDeadline = new Date(row.reconnect_deadline_at).getTime();
  if (health !== 'disconnected' || !Number.isFinite(reconnectDeadline) || reconnectDeadline <= Date.now()) return '';
  return `<span class="muted small">Connection lost ${escapeHtml(fmtTime(row.disconnected_at))} · reconnect by ${escapeHtml(fmtTime(row.reconnect_deadline_at))}</span>`;
}

// Renders the two session roles as crisp peer assignments.
function participantAssignments(rows, session = state.selectedSession) {
  if (!rows.length) return '';
  return `<div class="participant-assignments" aria-label="Session participants">${rows.map(row => `
    <div class="participant-assignment participant-role-${row.role === 'B' ? 'b' : 'a'}">
      ${roleBadge(row.role)}
      ${participantIcon(row.participant_kind)}
      <span class="participant-copy"><span class="participant-name-line"><strong>${escapeHtml(participantDisplayName(row))}</strong>${prolificDetailsMarkup(row)}${row.participant_kind !== 'agent' && row.research_id ? `<button class="danger delete-participant" data-research-id="${escapeHtml(row.research_id)}" type="button" title="Delete participant data" aria-label="Delete participant data">${icon('trash')}</button>` : ''}${configurationIdentityMarkup(row)}</span>${participantStatusMarkup(row, session)}${participantDisconnectMarkup(row)}</span>
    </div>`).join('')}</div>`;
}

// Previews and confirms deletion of one human participant's stored data.
async function deleteParticipantData(researchId) {
  if (!state.experiment) return;
  const path = runtimeApi(`participants/${encodeURIComponent(researchId)}/deletion`);
  const previewResponse = await adminFetch(state, path);
  if (!previewResponse.ok) throw new Error(await previewResponse.text());
  const preview = (await previewResponse.json()).preview;
  const confirmed = window.confirm(
    `Delete ${researchId}? This removes ${preview.content_event_count} message/transcript events and ${preview.consent_count} consent declarations, and anonymizes ${preview.other_event_count} other event references. This cannot be undone.`
  );
  if (!confirmed) return;
  const response = await adminFetch(state, path, {
    method: 'POST',
    headers: { 'X-CSRF-Token': state.csrfToken || '' }
  });
  if (!response.ok) throw new Error(await response.text());
  await selectSession(state.selected);
}

// Maps one event bundle to its restrained visual category.
function eventClass(bundle) {
  if (bundle.problem) return 'problem';
  if (bundle.kind === 'action') return 'action';
  if (bundle.kind === 'voice' || bundle.kind === 'transcript') return 'transcript';
  return '';
}

// Renders compact participant role markers for rows and event timeline entries.
function roleBadge(role) {
  const normalized = role === 'A' || role === 'B' ? role : '';
  if (!normalized) return '<span class="role-badge role-system">SYS</span>';
  return `<span class="role-badge role-${normalized.toLowerCase()}">${escapeHtml(normalized)}</span>`;
}

// Returns only extra event text that is not already implied by the title and badge.
function mergeEvents(events) {
  const known = new Set(state.events.map(event => `${event.event_id}:${event.event_index}`));
  for (const event of events) {
    const key = `${event.event_id}:${event.event_index}`;
    if (known.has(key)) continue;
    known.add(key);
    state.events.push(event);
    state.lastEventIndex = Math.max(state.lastEventIndex, event.event_index);
  }
  state.events.sort((left, right) => left.event_index - right.event_index);
}

// Renders filtered event bundles as the selected session's chronological log.
function renderEventBundles() {
  const bundles = (state.eventBundles || []).filter(bundle => {
    if (bundle.kind === 'log' && !showLogs.checked) return false;
    return showHousekeeping.checked || !bundle.housekeeping;
  });
  timeline.innerHTML = '';
  for (const bundle of bundles) {
    timeline.insertAdjacentHTML('beforeend', `
      <article class="event ${eventClass(bundle)}">
        <div class="event-line">
          <span class="game-time" title="Game time">${escapeHtml(fmtGameTime(bundle.game_time_ms))}</span>
          ${roleBadge(bundle.role)}
          <div class="event-main">
            <span class="event-title">${escapeHtml(bundle.title)}${bundle.problem ? '<span class="problem-badge">Problem</span>' : ''}</span>
            <span class="muted small">#${bundle.first_index}${bundle.first_index === bundle.last_index ? '' : `-${bundle.last_index}`}</span>
            ${bundle.steps ? `<div class="bundle-steps">${escapeHtml(bundle.steps)}</div>` : ''}
            ${bundle.problem_reason ? `<div class="problem-reason">${escapeHtml(bundle.problem_reason)}</div>` : ''}
          </div>
          <div class="event-text">${bundle.action ? `<div class="structured-action">${prettyAction(bundle.action, bundle.role)}</div>` : escapeHtml(bundle.text || '')}</div>
        </div> 
      </article>
    `);
  }
  if (!timeline.children.length) timeline.innerHTML = '<div class="empty">No action or message events recorded yet.</div>';
}

// Formats a structured action without repeating its already-visible actor role.
function prettyAction(action, role) {
  if (!action || typeof action !== 'object') return escapeHtml(String(action ?? ''));
  const type = action.type;
  const rows = Object.entries(action).filter(([key, value]) => {
    if (key === 'type') return false;
    if (key === 'player' && role && value === role) return false;
    return true;
  }).map(([key, value]) => `
    <div class="action-row">
      <span class="action-key">${escapeHtml(key)}</span>
      <span class="action-value">${escapeHtml(formatActionValue(value))}</span>
    </div>
  `).join('');
  return `${type ? `<strong class="action-type">${escapeHtml(type)}</strong>` : ''}${rows}`;
}

// Serializes one structured action value for compact inline display.
function formatActionValue(value) {
  if (value === null) return 'null';
  if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') return String(value);
  return JSON.stringify(value);
}

// Polls for new durable events in the currently selected session.
async function refreshEvents() {
  if (!state.selected) return;
  const params = new URLSearchParams({ after: String(state.lastEventIndex) });
  const response = await adminFetch(state, `${runtimeApi(`sessions/${state.selected}/events`)}?${params}`);
  if (!response.ok) throw new Error(await response.text());
  const data = await response.json();
  state.eventBundles = data.event_bundles || state.eventBundles;
  mergeEvents(data.events || []);
  renderEventBundles();
  liveStatus.textContent = `Last checked ${new Date().toLocaleTimeString()}`;
}

showHousekeeping.addEventListener('change', renderEventBundles);
showLogs.addEventListener('change', renderEventBundles);
document.getElementById('createExperimentButton').addEventListener('click', () => {
  document.getElementById('createExperimentForm').reset();
  createExperimentDialog.showModal();
  document.getElementById('newExperimentId').focus();
});
document.getElementById('cancelCreateExperiment').addEventListener('click', () => createExperimentDialog.close());
document.getElementById('createExperimentForm').addEventListener('submit', event => {
  event.preventDefault();
  const experimentId = document.getElementById('newExperimentId').value.trim();
  const notes = document.getElementById('newExperimentNotes').value;
  createExperiment(experimentId, notes).then(() => createExperimentDialog.close()).catch(error => window.alert(error.message));
});
document.getElementById('notesWriteMode').addEventListener('click', () => setNotesMode('write'));
document.getElementById('notesPreviewMode').addEventListener('click', () => setNotesMode('preview'));
document.getElementById('saveNotes').addEventListener('click', () => saveNotes().catch(error => window.alert(error.message)));
saveConfiguration.addEventListener('click', saveConfigRevision);
[institutionInput, adminAllowedIpRangesInput, speechmaticsRealtimeUrlInput, ttsBaseUrlInput, prolificApiBaseUrlInput]
  .forEach(control => control.addEventListener('input', markGameSettingsDirty));
document.getElementById('saveGameSettings').addEventListener('click', saveSharedGameSettings);
menuButton.addEventListener('click', toggleExperimentMenu);
document.addEventListener('keydown', event => {
  if (event.key !== 'Escape') return;
  closeExperimentMenu();
  closeStatusFilters();
});
document.addEventListener('click', event => {
  hideQuickTooltip();
  if (!event.target.closest('[data-status-filter]')) closeStatusFilters();
  if (!document.body.classList.contains('sidebar-open')) return;
  if (event.target.closest('#experimentSidebar') || event.target.closest('#menuButton')) return;
  closeExperimentMenu();
});
document.addEventListener('pointerover', event => {
  const target = quickTooltipTarget(event.target);
  if (target && target !== state.quickTooltipTarget) showQuickTooltip(target);
});
document.addEventListener('pointerout', event => {
  const target = state.quickTooltipTarget;
  if (!target || (event.relatedTarget && target.contains(event.relatedTarget))) return;
  hideQuickTooltip();
});
document.addEventListener('focusin', event => {
  const target = quickTooltipTarget(event.target);
  if (target) showQuickTooltip(target);
});
document.addEventListener('focusout', event => {
  const target = state.quickTooltipTarget;
  if (!target || (event.relatedTarget && target.contains(event.relatedTarget))) return;
  hideQuickTooltip();
});
window.addEventListener('resize', hideQuickTooltip);
document.addEventListener('scroll', hideQuickTooltip, true);
tabButtons.forEach(button => {
  button.addEventListener('click', () => {
    state.activeTab = button.dataset.tab;
    renderTabs();
    if (state.activeTab === 'privacy') loadPrivacy().catch(error => { privacyContent.innerHTML = `<div class="empty">${escapeHtml(error.message)}</div>`; });
  });
});
scopeButtons.forEach(button => {
  button.addEventListener('click', () => showScope(button.dataset.scope));
});
document.querySelectorAll('[data-status-filter]').forEach(initializeStatusFilter);
experimentStatusFilter.addEventListener('change', renderExperiment);
sessionLifecycleFilter.addEventListener('change', () => {
  state.selected = null;
  state.selectedSession = null;
  state.selectedParticipants = [];
  sessionDetail.hidden = true;
  loadSessions();
});
document.getElementById('downloadExport').addEventListener('click', () => {
  if (!state.experiment) return;
  const params = new URLSearchParams({
    format: document.getElementById('exportFormat').value,
    variant: document.getElementById('exportVariant').value
  });
  window.location.href = `${runtimeApi('export')}?${params}`;
});
summary.addEventListener('click', event => {
  const button = event.target.closest('.delete-participant');
  if (!button) return;
  button.disabled = true;
  deleteParticipantData(button.dataset.researchId).catch(error => {
    window.alert(error.message);
    button.disabled = false;
  });
});
makeResizable(document.getElementById('catalogueResizer'), '--catalogue-width', 230, 430);
makeResizable(document.getElementById('sessionResizer'), '--session-width', 250, 480);
loadExperiment().then(() => Promise.all([loadSessions(), loadLoad()])).catch(error => {
  sessionList.innerHTML = `<div class="empty">${escapeHtml(error.message)}</div>`;
});
renderTabs();
showScope('experiments');
setInterval(() => {
  loadExperiment().then(() => Promise.all([loadSessions(), loadLoad()])).catch(error => {
    liveStatus.textContent = error.message;
  });
}, 5000);
state.timer = setInterval(refreshEvents, 1500);
