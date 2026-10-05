import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import { isSpeechBundle, escapeHtml, fmtGameTime, sessionTimeline, roleBadge } from '../src/app/admin_dashboard_format.mjs';

test('orders utterances by speech onset instead of ASR delivery time', () => {
  const a = { kind: 'transcript', text: 'earlier', first_index: 2, game_time_ms: 5000, utterance_timing: { start_ms: 500 } };
  const b = { kind: 'transcript', text: 'later', first_index: 1, game_time_ms: 4000, utterance_timing: { start_ms: 1000 } };
  const html = sessionTimeline([b, a]);
  assert.ok(html.indexOf('>earlier<') < html.indexOf('>later<'));
});

test('simultaneous speakers share a chronological row and escape recognized content', () => {
  const bundles = ['A', 'B'].map((role, index) => ({ kind: 'transcript', role, first_index: index + 1, text: '<script>', tokens: [{ kind: 'word', text: '<script>', start_ms: 2000, end_ms: 3000 }] }));
  const html = sessionTimeline(bundles, 'tokens');
  assert.equal((html.match(/class="session-timeline-row"/g) || []).length, 1);
  assert.ok(html.includes('speaker-a'));
  assert.ok(html.includes('speaker-b'));
  assert.ok(html.includes('&lt;script&gt;'));
  assert.ok(!html.includes('<script>'));
  assert.ok(html.includes('0:03.000 · 1000 ms'));
});

test('all utterances remain visible beyond ten seconds and sort by onset', () => {
  const bundles = [
    { kind: 'transcript', role: 'B', first_index: 3, tokens: [{ kind: 'word', text: 'third', start_ms: 125000, end_ms: 125500 }] },
    { kind: 'transcript', role: 'A', first_index: 1, tokens: [{ kind: 'word', text: 'first', start_ms: 500, end_ms: 900 }] },
    { kind: 'transcript', role: 'B', first_index: 2, tokens: [{ kind: 'word', text: 'second', start_ms: 30000, end_ms: 31000 }] },
  ];
  const html = sessionTimeline(bundles, 'tokens');
  assert.ok(html.indexOf('>first<') < html.indexOf('>second<'));
  assert.ok(html.indexOf('>second<') < html.indexOf('>third<'));
  assert.ok(html.includes('2:05.000'));
  assert.equal((html.match(/ speech-token"/g) || []).length, 3);
});

test('missing historical timings appear beside their utterance without fabricated tokens', () => {
  const html = sessionTimeline([
    { kind: 'transcript', role: 'A', first_index: 1, text: 'Older recording', utterance_timing: { start_ms: 20000 }, game_time_ms: 21000 },
    { kind: 'transcript', role: 'B', first_index: 2, tokens: [{ kind: 'word', text: 'later', start_ms: 30000, end_ms: 31000 }] },
  ], 'tokens');
  assert.ok(html.includes('Token timings unavailable'));
  assert.ok(html.includes('Older recording'));
  assert.ok(html.indexOf('Older recording') < html.indexOf('>later<'));
  assert.ok(html.includes('0:20.000'));
});

test('punctuation retains zero duration and token focus exposes utterance and confidence', () => {
  const html = sessionTimeline([{ kind: 'transcript', role: 'A', first_index: 5, text: 'Wait.', tokens: [
    { kind: 'word', text: 'Wait', start_ms: 100, end_ms: 500, confidence: 0.9 },
    { kind: 'punctuation', text: '.', start_ms: 500, end_ms: 500 },
  ] }], 'tokens');
  assert.ok(html.includes('0 ms · punctuation'));
  assert.ok(html.includes('tabindex="0"'));
  assert.ok(html.includes('confidence 0.9'));
  assert.ok(html.includes('Utterance #5: Wait.'));
});

test('token timeline preserves typed messages and reports an empty session', () => {
  assert.ok(sessionTimeline([{ kind: 'conversation', text: 'typed' }], 'tokens').includes('typed'));
  assert.ok(sessionTimeline([], 'tokens').includes('No action or message events recorded yet.'));
});

test('agent speech appears in B beside human ASR, including messages without historical timings', () => {
  const html = sessionTimeline([
    { kind: 'transcript', origin: 'voice_transcript', role: 'A', first_index: 1, tokens: [{ kind: 'word', text: 'human', start_ms: 100, end_ms: 500 }] },
    { kind: 'conversation', origin: 'agent', spoken: true, role: 'B', first_index: 2, tokens: [{ kind: 'word', text: 'agent', start_ms: 600, end_ms: 1000 }] },
    { kind: 'conversation', origin: 'agent', spoken: true, role: 'B', first_index: 3, text: 'Older agent reply', game_time_ms: 2000 },
    { kind: 'conversation', origin: 'agent', role: 'B', first_index: 5, text: 'Agent text without voice', game_time_ms: 4000 },
    { kind: 'conversation', origin: 'typed', role: 'A', first_index: 4, text: 'Typed message', game_time_ms: 3000 },
  ], 'tokens');
  assert.ok(html.includes('>human<'));
  assert.match(html, /speaker-b[^]*>agent</);
  assert.ok(html.includes('Older agent reply'));
  assert.ok(html.includes('Token timings unavailable'));
  assert.ok(html.includes('Typed message'));
  assert.ok(html.includes('Agent text without voice'));
});

// Switching from speech to typed dialogue must not leave the token view selected.
test('typed-only sessions hide the selector and restore the normal conversation view', () => {
  const render = dashboardRendererSource();
  const label = { hidden: false };
  const speechView = { value: 'tokens', closest: () => label };
  const state = { eventBundles: [{ kind: 'transcript', role: 'A', first_index: 1, text: 'Spoken turn' }] };
  const timeline = { innerHTML: '', children: [1], insertAdjacentHTML(_position, html) { this.innerHTML += html; } };
  const context = { state, speechView, timeline, showLogs: { checked: false }, showHousekeeping: { checked: false }, isSpeechBundle, sessionTimeline, roleBadge, escapeHtml, fmtGameTime };
  runInNewContext(render + '\nrenderEventBundles();', context);
  assert.equal(label.hidden, false);
  state.eventBundles = [{ kind: 'conversation', origin: 'agent', role: 'B', first_index: 2, text: 'Typed agent reply', game_time_ms: 100 }];
  runInNewContext(render + '\nrenderEventBundles();', context);
  assert.equal(label.hidden, true);
  assert.equal(speechView.value, 'utterances');
  assert.ok(timeline.innerHTML.includes('Typed agent reply'));
  assert.ok(!timeline.innerHTML.includes('Token timings unavailable'));
  state.eventBundles[0].spoken = true;
  runInNewContext(render + '\nrenderEventBundles();', context);
  assert.equal(label.hidden, false);
});

// Runs the production card renderer without the unrelated dashboard initialization.
function dashboardRendererSource() {
  const source = readFileSync(new URL('../src/app/admin_dashboard.js', import.meta.url), 'utf8');
  return ['renderEventBundles', 'renderEventBundle', 'eventClass', 'prettyAction', 'formatActionValue'].map(name => {
    const start = source.indexOf(`function ${name}(`);
    return source.slice(start, source.indexOf('\n}\n', start) + 3);
  }).join('\n');
}

// View changes must preserve game events, rich cards, typed messages, and filter behavior.
test('token and utterance views retain identical non-speech cards and filters', () => {
  const bundles = [
    {kind:'transcript', role:'A', first_index:1, last_index:1, game_time_ms:9000, text:'First second', tokens:[
      {kind:'word',text:'First',start_ms:100,end_ms:200}, {kind:'word',text:'second',start_ms:500,end_ms:600}]},
    {kind:'action', role:'B', first_index:2, last_index:3, game_time_ms:300, title:'Action', action:{type:'Open root'}, steps:'Submitted → accepted'},
    {kind:'log', role:'B', first_index:4, last_index:4, game_time_ms:400, title:'Agent log', text:'<private>'},
    {kind:'conversation', origin:'typed', role:'A', first_index:5, last_index:5, game_time_ms:450, text:'Typed turn'},
    {kind:'voice', role:'A', first_index:6, last_index:6, game_time_ms:700, housekeeping:true, title:'Setup', problem:true, problem_reason:'Disconnected'},
    {kind:'session_completed', first_index:7, last_index:7, game_time_ms:800, title:'Completed'},
  ];
  const context = {state:{eventBundles:bundles}, speechView:{value:'utterances',closest:()=>({})}, timeline:{innerHTML:''},
    showLogs:{checked:true}, showHousekeeping:{checked:true}, isSpeechBundle, sessionTimeline, roleBadge, escapeHtml, fmtGameTime};
  const render = dashboardRendererSource();
  runInNewContext(render + '\nrenderEventBundles();', context);
  const utterances = context.timeline.innerHTML;
  context.speechView.value = 'tokens';
  runInNewContext(render + '\nrenderEventBundles();', context);
  const tokens = context.timeline.innerHTML;
  for (const card of utterances.match(/<article[^]*?<\/article>/g).slice(0)) {
    if (!card.includes('First second')) assert.ok(tokens.includes(card), card);
  }
  assert.ok(tokens.indexOf('>First<') < tokens.indexOf('Open root'));
  assert.ok(tokens.indexOf('Open root') < tokens.indexOf('>second<'));
  assert.ok(tokens.includes('&lt;private&gt;'));
  assert.ok(!tokens.includes('Token timings unavailable'));
  for (const view of ['utterances', 'tokens']) {
    context.speechView.value = view;
    context.showLogs.checked = false;
    context.showHousekeeping.checked = false;
    runInNewContext(render + '\nrenderEventBundles();', context);
    assert.ok(!context.timeline.innerHTML.includes('Agent log'));
    assert.ok(!context.timeline.innerHTML.includes('Disconnected'));
    assert.ok(context.timeline.innerHTML.includes('Open root'));
    assert.ok(context.timeline.innerHTML.includes('Typed turn'));
    assert.ok(context.timeline.innerHTML.includes('Completed'));
  }
});

// Both resolutions share row structure, speaker anchors, and timestamp placement.
test('only speech expansion changes between timeline resolutions', () => {
  const bundle = {kind:'transcript', role:'B', first_index:1, game_time_ms:9000,
    text:'Hello there', utterance_timing:{start_ms:100,end_ms:600}, tokens:[
      {kind:'word',text:'Hello',start_ms:100,end_ms:200}, {kind:'word',text:'there',start_ms:400,end_ms:600}]};
  for (const view of ['utterances', 'tokens']) {
    const html = sessionTimeline([bundle], view);
    assert.match(html, /session-timeline-row[^]*class="game-time"[^]*session-timeline-content[^]*speech-bubble speaker-b/);
    assert.ok(!html.includes('<table'));
    assert.equal((html.match(/class="session-timeline-row"/g) || []).length, view === 'tokens' ? 2 : 1);
    assert.ok(html.includes('0:00.600'));
  }
});

// Actor identity stays in the gutter; actions and logs remain safe and compact.
test('actions are inline and logs omit labels while preserving actor badges and problems', () => {
  const bundles = [
    {kind:'conversation',role:'A',first_index:1,text:'Open it',game_time_ms:100},
    {kind:'action',role:'B',first_index:2,last_index:3,game_time_ms:200,title:'Action',
      action:{type:'SetFlow',root:'Silver',open:true,player:'B'},steps:'Submitted → accepted'},
    {kind:'log',role:'A',first_index:4,game_time_ms:300,title:'Agent log',text:'<checking>'},
    {kind:'log',role:'B',first_index:5,game_time_ms:400,title:'Agent log',text:'Failed',problem:true,problem_reason:'Unavailable'},
    {kind:'log',first_index:6,game_time_ms:500,title:'Game log',text:'Finished'},
  ];
  const context = {state:{eventBundles:bundles},speechView:{value:'utterances',closest:()=>({})},timeline:{innerHTML:''},
    showLogs:{checked:true},showHousekeeping:{checked:true},isSpeechBundle,sessionTimeline,roleBadge,escapeHtml,fmtGameTime};
  runInNewContext(dashboardRendererSource() + '\nrenderEventBundles();', context);
  const html=context.timeline.innerHTML;
  assert.match(html, /<strong>SetFlow<\/strong> root: Silver, open: true/);
  assert.ok(!html.includes('player: B'));
  assert.ok(!html.includes('<details'));
  assert.ok(!html.includes('<summary'));
  assert.match(html, /session-timeline-entry"><span class="role-badge role-a">A<\/span>/);
  assert.match(html, /session-timeline-entry"><span class="role-badge role-b">B<\/span>/);
  assert.match(html, /role-system">SYS/);
  assert.match(html, /timeline-log speaker-a[^]*&lt;checking&gt;/);
  assert.match(html, /timeline-log speaker-b problem/);
  assert.ok(html.includes('Unavailable'));
  assert.ok(!html.includes('Agent log'));
  assert.ok(!html.includes('Game log'));
});
