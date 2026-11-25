// Shared facts and linked text sources belong to one configuration revision.
import CONSENT_TEMPLATES from './consent-templates.js';
import PRIVACY_TEMPLATES from './privacy-notice-templates.js';
import IMPRINT_TEMPLATES from './imprint-templates.js';

const bindings = new WeakMap();
const CONTACTS = ['operator', 'responsible_person', 'address', 'email', 'phone'];
const LABELS = {
  operator: 'Operating institution / legal operator', responsible_person: 'Responsible study contact',
  address: 'Public postal address', email: 'Public contact email', phone: 'Public telephone',
  controller_details: 'Legal controller, data protection contact and representatives',
  legal_basis: 'Legal basis for each processing purpose and applicable special-category conditions',
  processing_purposes: 'Processing purposes and applicable data protection rules',
  storage_locations_and_access_safeguards: 'Access safeguards for the storage location specified separately',
  collection_details: 'Additional collection details: categories, sources and purposes',
  storage_details: 'Additional storage, retention and disclosure details',
  transfers: 'International transfers: countries, recipients, safeguards and how to obtain them',
  rights_contact: 'Rights request methods, verification, limits and regulator contact',
};

// Canonicalizes template placeholders without treating different legal roles as synonyms.
export function fieldKey(token) {
  return token.split('|')[0].trim().toLowerCase().replace(/[^a-z0-9]+/g, '_').replace(/^_|_$/g, '').slice(0, 128);
}

// Returns both literal rendered text and its token spans, allowing edits without global substitution.
export function renderLinkedText(template, values = {}) {
  const spans = [];
  let text = '', cursor = 0;
  for (const match of template.matchAll(/{{([^{}]+)}}/g)) {
    text += template.slice(cursor, match.index);
    const key = fieldKey(match[1]);
    const fallback = match[1].includes('|') ? match[1].slice(match[1].indexOf('|') + 1) : match[0];
    const value = Object.hasOwn(values, key) ? values[key] || fallback : fallback;
    spans.push({ key, start: text.length, end: text.length + value.length, token: match[0] });
    text += value;
    cursor = match.index + match[0].length;
  }
  return { text: text + template.slice(cursor), spans };
}

// Retains untouched token spans when prose is edited. Editing just one populated value
// updates that fact everywhere; edits crossing several facts become local document prose.
export function editLinkedText(template, values, edited) {
  const { text, spans } = renderLinkedText(template, values);
  if (edited === text) return { template, updates: {} };
  let start = 0, end = text.length, nextEnd = edited.length;
  while (start < end && start < nextEnd && text[start] === edited[start]) start++;
  while (end > start && nextEnd > start && text[end - 1] === edited[nextEnd - 1]) { end--; nextEnd--; }
  const containing = spans.filter(span => span.start <= start && span.end >= end &&
    (start !== end || (start > span.start && start < span.end)));
  if (containing.length === 1) {
    const span = containing[0];
    const value = edited.slice(span.start, nextEnd + span.end - end);
    if (value && !value.includes('{{') && !value.includes('}}')) return { template, updates: { [span.key]: value } };
  }
  const delta = edited.length - text.length;
  const retained = spans.filter(span => span.end <= start || span.start >= end).map(span =>
    span.start >= end ? { ...span, start: span.start + delta, end: span.end + delta } : span);
  let result = '', cursor = 0;
  for (const span of retained) {
    result += edited.slice(cursor, span.start) + span.token;
    cursor = span.end;
  }
  return { template: result + edited.slice(cursor), updates: {} };
}

// Escapes all template-derived labels and saved values before inserting form markup.
function escape(value) {
  return String(value ?? '').replace(/[&<>"']/g, ch => ({'&':'&amp;', '<':'&lt;', '>':'&gt;', '"':'&quot;', "'":'&#39;'}[ch]));
}

// Discovers shared answers directly from Markdown, including custom consent placeholders.
function fieldsIn(template) {
  return [...new Map([...template.matchAll(/{{([^{}]+)}}/g)].filter(m => !m[1].startsWith('source.')).map(m =>
    [fieldKey(m[1]), { key: fieldKey(m[1]), label: LABELS[fieldKey(m[1])] || m[1].split('|')[0].replaceAll('_', ' ') }])).values()];
}

// Repeated controls are views of the same answer; none writes an independent config path.
function fieldMarkup({ key, label }) {
  const attributes = `data-document-field="${escape(key)}" maxlength="${CONTACTS.includes(key) ? 2000 : 20000}"`;
  return `<label class="config-field"><span>${escape(label)}</span>${CONTACTS.includes(key)
    ? `<input ${attributes} type="text">` : `<textarea ${attributes} rows="2"></textarea>`}</label>`;
}

// Returns the persisted facts and source recipes for the form's current generated documents.
export function documentFieldsFromForm(form) {
  const state = bindings.get(form);
  if (!state) return { values: {}, templates: {} };
  const templates = {};
  for (const [control, recipe] of state.recipes) {
    if (!form.contains(control)) continue;
    if (control.matches('[data-config-path="direct.imprint.text"]')) templates.imprint = recipe;
    else if (control.matches('[data-consent-field="body"]')) {
      const id = control.closest('.consent-item').querySelector('[data-consent-field="id"]').value.trim();
      templates[`consent/${id}`] = recipe;
    }
  }
  return { values: { ...state.values }, templates };
}

// Binds all three editors to shared facts before preview handlers run. Existing unlinked
// prose is preserved; disagreeing saved values require an explicit edit rather than guessing.
export function bindDocumentFields(form, configured = {}, onChange = () => {}) {
  bindings.get(form)?.dispose();
  const values = { ...(configured.values || {}) }, recipes = new Map(), conflicts = new Set();
  const existing = new Map();
  form.querySelectorAll('[data-config-path]').forEach(control => {
    const path = control.dataset.configPath;
    const key = path.startsWith('direct.privacy_notice.details.') ? path.slice('direct.privacy_notice.details.'.length)
      : path === 'direct.participant_information_version' ? 'version'
      : CONTACTS.find(key => path === `direct.imprint.${key}`);
    if (!key) return;
    control.dataset.documentField = key;
    if (control.value.trim()) existing.set(key, [...(existing.get(key) || []), control.value]);
  });
  for (const [key, candidates] of existing) {
    const options = new Set([...(Object.hasOwn(values, key) ? [values[key]] : []), ...candidates]);
    if (options.size === 1) values[key] = candidates[0];
    else { delete values[key]; conflicts.add(key); }
  }
  const allFields = new Map([...CONSENT_TEMPLATES, ...PRIVACY_TEMPLATES].flatMap(t => fieldsIn(t.body)).map(field => [field.key, field]));
  CONTACTS.forEach(key => allFields.set(key, { key, label: LABELS[key] }));
  // Consent answers sit next to the section that uses them, including before generation.
  form.querySelectorAll('[data-consent-section-text]').forEach(control => {
    const fields = fieldsIn(control.value);
    const holder = document.createElement('div');
    holder.className = 'config-fields';
    holder.innerHTML = fields.map(fieldMarkup).join('');
    control.parentElement.append(holder);
    recipes.set(control, control.value);
  });
  const privacy = form.querySelector('[data-privacy-notice-editor]');
  if (privacy) {
    const extra = [...allFields.values()].filter(field => !privacy.querySelector(`[data-document-field="${field.key}"]`));
    const holder = document.createElement('details');
    holder.innerHTML = `<summary>Shared study information</summary><p class="config-help">Enter a fact here or beside its consent section or imprint field. Matching fields and linked text update together. Operator, controller and rights contacts remain distinct.</p><div class="config-fields">${extra.map(fieldMarkup).join('')}</div>`;
    privacy.prepend(holder);
  }
  const status = document.createElement('p');
  status.className = 'config-help'; status.setAttribute('role', 'status');
  form.querySelector('[data-consent-editor]')?.prepend(status);
  const imprintControl = form.querySelector('[data-config-path="direct.imprint.text"]');
  const countryControl = form.querySelector('[data-config-path="direct.imprint.country"]');
  // Recognize a recipe only when it reproduces the stored draft; custom copy is never reset.
  const register = (control, candidate) => {
    recipes.set(control, candidate && renderLinkedText(candidate, values).text.trim() === control.value.trim() ? candidate : control.value);
  };
  const imprintTemplate = () => IMPRINT_TEMPLATES.find(t => t.id === countryControl?.value)?.body || '';
  if (imprintControl) register(imprintControl, configured.templates?.imprint || imprintTemplate());
  form.querySelectorAll('.consent-item').forEach(item => {
    const id = item.querySelector('[data-consent-field="id"]').value.trim();
    register(item.querySelector('[data-consent-field="body"]'), configured.templates?.[`consent/${id}`]);
  });
  // Writes controls and documents atomically before notifying the existing preview renderer.
  const refresh = () => {
    form.querySelectorAll('[data-document-field]').forEach(control => {
      const key = control.dataset.documentField;
      if (!conflicts.has(key)) control.value = values[key] || '';
    });
    for (const [control, recipe] of recipes) {
      if (!form.contains(control)) { recipes.delete(control); continue; }
      control.value = renderLinkedText(recipe, values).text;
    }
    status.textContent = conflicts.size ? `Different saved values exist for: ${[...conflicts].join(', ')}. Edit a matching shared field to choose the value used everywhere.`
      : 'Shared answers prefill matching fields and linked text across consent, imprint and privacy notice. Edit a shared answer here or in either other editor. Custom prose is retained.';
    onChange();
  };
  // Events are delegated so newly generated forms participate without rebinding listeners.
  const handle = event => {
    const control = event.target;
    if (event.type !== 'click' && control.dataset?.documentField) {
      const key = control.dataset.documentField;
      values[key] = control.value; conflicts.delete(key);
    } else if (event.type !== 'click' && recipes.has(control)) {
      const edited = editLinkedText(recipes.get(control), values, control.value);
      recipes.set(control, edited.template);
      Object.assign(values, edited.updates);
      Object.keys(edited.updates).forEach(key => conflicts.delete(key));
    }
    if (event.type === 'change' && control === countryControl) recipes.set(imprintControl, imprintTemplate());
    if (event.type === 'click' && control.closest?.('[data-generate-consent]')) {
      const editor = control.closest('[data-consent-editor]');
      const selected = new Set([...editor.querySelectorAll('[data-consent-template-option]:checked')].map(input => input.value));
      const recipe = CONSENT_TEMPLATES.filter(t => selected.has(t.id)).map(t => {
        const source = editor.querySelector(`[data-consent-section-text="${t.id}"]`);
        return `${t.title}\n${recipes.get(source) ?? source.value}`;
      }).join('\n\n');
      const bodies = editor.querySelectorAll('[data-consent-field="body"]');
      if (bodies.length) recipes.set(bodies[bodies.length - 1], recipe);
    }
    refresh();
  };
  form.addEventListener('input', handle);
  form.addEventListener('change', handle);
  form.addEventListener('click', handle);
  bindings.set(form, { values, recipes, dispose() {
    form.removeEventListener('input', handle); form.removeEventListener('change', handle); form.removeEventListener('click', handle);
  } });
  refresh();
}
