// Shares communication fields and ordinary configuration controls between the experiment dashboards.

export const COMMUNICATION_MODE_FIELD = { path: 'communication.mode', label: 'Communication mode', help: 'Speech uses live microphone audio. Parlando does not store raw microphone audio.', type: 'select', options: [['typed', 'Typed'], ['speech', 'Speech']] };

const SPEECH_CONFIG_SECTIONS = [
  { title: 'Voice transport', description: 'Live browser audio carried through Parlando. This is independent of speech recognition.', fields: [
    { path: 'voice.sample_rate_hz', label: 'Sample rate (Hz)', help: 'The audio protocol requires 24000 Hz.', type: 'fixed-number' },
    { path: 'voice.frame_duration_ms', label: 'Frame duration (ms)', help: 'The audio protocol requires 20 ms frames.', type: 'fixed-number' },
    { path: 'voice.jitter_buffer_ms', label: 'Playback jitter buffer (ms)', help: 'Audio buffered before playback.', type: 'number', min: 20, max: 5000 },
    { path: 'voice.post_completion_seconds', label: 'Post-game voice time (seconds)', help: 'Time participants may keep talking after an ordinarily completed game.', type: 'number', min: 1, max: 3600 },
  ]},
  { title: 'Speech recognition', description: 'Optional Speechmatics conversion of participant speech into text.', fields: [
    { path: 'transcription.enabled', label: 'Enable speech recognition', help: 'Send live participant audio to Speechmatics to record final transcripts and make them available to agents.', type: 'boolean' },
    { path: 'transcription.provider', label: 'Recognition provider', help: 'Speech recognition uses Speechmatics.', type: 'fixed' },
    { path: 'transcription.model', label: 'Recognition model', help: 'Enhanced favors recognition quality; Standard is the lighter operating point.', type: 'select', options: [['enhanced', 'Enhanced'], ['standard', 'Standard']] },
    { path: 'transcription.language', label: 'Spoken language', help: 'Speechmatics language code for the expected participant speech, for example en or de.' },
    { path: 'capacity.max_transcription_streams', label: 'Maximum recognition streams', help: 'Maximum simultaneous participant audio streams sent to speech recognition.', type: 'number', min: 1 },
    { path: 'speechmatics.realtime_url', label: 'Speechmatics realtime URL', help: 'Exact regional or private WebSocket endpoint that receives participant audio.' },
    { path: 'speechmatics.max_delay', label: 'Maximum transcript delay (seconds)', help: 'Maximum recognition delay; larger values may improve accuracy at the cost of responsiveness.', type: 'decimal', min: 0.1 },
    { path: 'speechmatics.enable_partials', label: 'Show partial transcripts', help: 'Deliver provisional text while the participant is still speaking.', type: 'boolean' },
    { path: 'speechmatics.end_of_utterance_silence_trigger', label: 'End-of-utterance silence (seconds)', help: 'Silence used to close an utterance.', type: 'decimal', min: 0.1 }
  ]},
  { title: 'Text to speech', description: 'Optional ElevenLabs speech synthesis for agent messages.', fields: [
    { path: 'tts.enabled', label: 'Enable text to speech', help: 'Synthesize agent messages with ElevenLabs and publish the audio through voice transport.', type: 'boolean' },
    { path: 'tts.provider', label: 'Synthesis provider', help: 'Speech synthesis uses ElevenLabs.', type: 'fixed' },
    { path: 'tts.model', label: 'Synthesis model', help: 'Choose the ElevenLabs balance of latency, language coverage, and expressiveness.', type: 'select', options: [['eleven_flash_v2_5', 'Flash v2.5'], ['eleven_flash_v2', 'Flash v2'], ['eleven_multilingual_v2', 'Multilingual v2'], ['eleven_v3', 'Eleven v3']] },
    { path: 'tts.voice_id', label: 'ElevenLabs voice id', help: 'Stable ElevenLabs identifier of the voice used for agent speech.' },
    { path: 'tts.voice_name', label: 'Voice display name', help: 'Human-readable label for the configured voice.' },
    { path: 'tts.base_url', label: 'ElevenLabs base URL', help: 'Exact WebSocket service origin that receives agent message text.' },
    { path: 'tts.output_format', label: 'Synthesis output format', help: 'The audio protocol requires 24000 Hz PCM output.', type: 'fixed' }
  ]},
];

// Paths owned by the form, including the speech-only transcription capacity limit.
export const COMMUNICATION_FIELD_PATHS = [
  COMMUNICATION_MODE_FIELD.path,
  ...SPEECH_CONFIG_SECTIONS.flatMap(section => section.fields.map(field => field.path)),
];

// Returns one nested configuration value without evaluating a user-controlled expression.
export function configAt(config, path) {
  return path.split('.').reduce((value, key) => value == null ? undefined : value[key], config);
}

// Writes one nested configuration value while preserving unedited server-owned defaults.
export function setConfigAt(config, path, value) {
  const segments = path.split('.');
  let target = config;
  segments.slice(0, -1).forEach(segment => { target = target[segment] ||= {}; });
  target[segments.at(-1)] = value;
}

// Escapes server and operator text interpolated into field markup.
function escapeHtml(value) {
  return String(value ?? '').replace(/[&<>"']/g, ch => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[ch]));
}

// Renders speech-only controls inside a fieldset so disabling it preserves every draft value.
export function renderSpeechSettings(config) {
  return `<fieldset class="speech-settings" data-speech-settings ${configAt(config, 'communication.mode') === 'speech' ? '' : 'disabled'}>
    <legend>Speech settings</legend>
    ${SPEECH_CONFIG_SECTIONS.map(section => `
      <section class="config-fields">
        <div class="config-group-copy"><h4>${escapeHtml(section.title)}</h4><p class="muted small">${escapeHtml(section.description)}</p></div>
        ${section.fields.map(field => renderStandardConfigField(field, config)).join('')}
      </section>`).join('')}
  </fieldset>`;
}

// Makes the selected mode control the speech fieldset without clearing input values.
export function bindSpeechSettings(form) {
  const mode = form.querySelector('[data-config-path="communication.mode"]');
  const fields = form.querySelector('[data-speech-settings]');
  mode.addEventListener('change', () => { fields.disabled = mode.value !== 'speech'; });
}

// Renders one control with the value and constraints given by its field definition.
export function renderStandardConfigField(field, config) {
  const value = configAt(config, field.path);
  const help = field.help ? `<span class="config-help">${escapeHtml(field.help)}</span>` : '';
  let control;
  if (field.type === 'boolean') return `<label class="config-field config-boolean"><span class="checkbox-line"><input data-config-path="${escapeHtml(field.path)}" data-config-type="boolean" type="checkbox" ${value ? 'checked' : ''}><span>${escapeHtml(field.label)}</span></span>${help}</label>`;
  else if (field.type === 'fixed' || field.type === 'fixed-number') control = `<input data-config-path="${escapeHtml(field.path)}" data-config-type="${field.type === 'fixed-number' ? 'number' : 'string'}" type="text" value="${escapeHtml(value)}" readonly>`;
  else if (field.type === 'number') control = `<input data-config-path="${escapeHtml(field.path)}" data-config-type="number" type="number" value="${escapeHtml(value)}" ${field.min != null ? `min="${field.min}"` : ''} ${field.max != null ? `max="${field.max}"` : ''} step="${field.step || '1'}">`;
  else if (field.type === 'optional-number') control = `<input data-config-path="${escapeHtml(field.path)}" data-config-type="optional-number" type="number" value="${escapeHtml(value ?? '')}" ${field.min != null ? `min="${field.min}"` : ''} ${field.max != null ? `max="${field.max}"` : ''} step="${field.step || '1'}">`;
  else if (field.type === 'decimal') control = `<input data-config-path="${escapeHtml(field.path)}" data-config-type="number" type="text" inputmode="decimal" pattern="-?[0-9]+(?:\\.[0-9]+)?" value="${escapeHtml(value)}" placeholder="1.2" title="Use a decimal point, for example 1.2">`;
  else if (field.type === 'select') {
    const options = typeof value === 'string' && !field.options.some(([option]) => option === value)
      ? [[value, value], ...field.options]
      : field.options;
    control = `<select data-config-path="${escapeHtml(field.path)}" data-config-type="string">${options.map(([option, label]) => `<option value="${escapeHtml(option)}" ${value === option ? 'selected' : ''}>${escapeHtml(label)}</option>`).join('')}</select>`;
  }
  else if (field.type === 'json') control = `<textarea data-config-path="${escapeHtml(field.path)}" data-config-type="json" rows="${field.rows || 5}" spellcheck="false">${escapeHtml(JSON.stringify(value, null, 2))}</textarea>`;
  else if (field.type === 'completion-code') control = `<input data-config-path="${escapeHtml(field.path)}" data-config-type="string" type="text" maxlength="64" pattern="[A-Za-z0-9]*" value="${escapeHtml(value ?? '')}" autocomplete="off">`;
  else control = `<input data-config-path="${escapeHtml(field.path)}" data-config-type="string" type="${field.type === 'url' ? 'url' : 'text'}" value="${escapeHtml(value ?? '')}">`;
  return `<label class="config-field"><span>${escapeHtml(field.label)}</span>${control}${help}</label>`;
}

// Writes the rendered values into a draft, including values in disabled fieldsets.
export function readConfigFields(form, config) {
  form.querySelectorAll('[data-config-path]').forEach(control => {
    let value;
    if (control.dataset.configType === 'boolean') value = control.checked;
    else if (control.dataset.configType === 'optional-number') {
      value = control.value.trim() === '' ? null : Number(control.value);
      if (value !== null && !Number.isFinite(value)) throw new Error(`${control.dataset.configPath} must be a number`);
    }
    else if (control.dataset.configType === 'number') {
      value = Number(control.value);
      if (!Number.isFinite(value)) throw new Error(`${control.dataset.configPath} must be a number written with a decimal point`);
    }
    else if (control.dataset.configType === 'json') value = JSON.parse(control.value);
    else value = control.value;
    setConfigAt(config, control.dataset.configPath, value);
  });
}
