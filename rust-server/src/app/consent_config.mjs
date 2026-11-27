// Builds the document editor for a dashboard, sharing the same template and editing behavior.
export function createConsentEditor({ templates, escapeHtml, safeMarkdown }) {
  const CONSENT_TEMPLATE_OPTIONS = templates;
// Defines the study facts and the template spellings that share each value.
const CONSENT_FACTS = [
  ['name', 'Name', ['name', 'your name']],
  ['institute', 'Institute', ['institute', 'institution', 'your institution', 'full legal name of the operating institution or individual']],
  ['email', 'Email', ['email', 'your email', 'study contact email', 'public contact email']],
  ['end_date', 'End date', ['end date']],
  ['license', 'License', ['license'], 'CC-by 4.0'],
  ['data_platform', 'Data platform', ['data platform', 'zenodo, huggingface'], 'zenodo'],
  ['payment', 'Payment', ['payment', 'amount and currency'], '€14 per hour'],
  ['server', 'Server', ['server'], 'Debian Webserver'],
];

// Locates supported placeholders in a new body; offsets use JavaScript UTF-16 units.
function consentFactBindings(body) {
  return Array.from(body.matchAll(/\{\{\s*([^{}]+?)\s*\}\}/g)).flatMap(match => {
    const fact = CONSENT_FACTS.find(([, , aliases]) => aliases.includes(match[1].trim().toLowerCase()));
    return fact ? [{ key: fact[0], start: match.index, end: match.index + match[0].length, placeholder: match[0] }] : [];
  });
}

// Keeps bindings outside a manual edit and detaches any occurrence edited directly.
function reconcileConsentFacts(data, previous, current) {
  if (previous === current) return;
  let start = 0;
  while (start < previous.length && start < current.length && previous[start] === current[start]) start++;
  let oldEnd = previous.length;
  let newEnd = current.length;
  while (oldEnd > start && newEnd > start && previous[oldEnd - 1] === current[newEnd - 1]) { oldEnd--; newEnd--; }
  const delta = newEnd - oldEnd;
  data.bindings = data.bindings.flatMap(binding => {
    if (binding.end <= start) return [binding];
    if (binding.start >= oldEnd) return [{ ...binding, start: binding.start + delta, end: binding.end + delta }];
    return [];
  });
}

// Creates the same linked-text state for consent, imprint, and privacy drafts.
function createLinkedText(body, data) {
  return { body, data: structuredClone(data || { values: {}, bindings: consentFactBindings(body) }) };
}

// Retains untouched bindings and recognizes newly entered placeholders after a prose edit.
function editLinkedText(state, body) {
  reconcileConsentFacts(state.data, state.body, body);
  for (const binding of consentFactBindings(body)) {
    if (!state.data.bindings.some(other => binding.start < other.end && binding.end > other.start)) state.data.bindings.push(binding);
  }
  state.data.bindings.sort((a, b) => a.start - b.start);
  state.body = body;
}

// Replaces tracked occurrences in one pass; values are literal and never recursively expanded.
function updateLinkedText(state, key, value) {
  const oldBody = state.body;
  const oldValue = state.data.values[key] || '';
  state.data.values[key] = value;
  let cursor = 0;
  let body = '';
  const bindings = [];
  for (const binding of state.data.bindings) {
    body += oldBody.slice(cursor, binding.start);
    const previous = oldBody.slice(binding.start, binding.end);
    const replacement = binding.key === key && (previous === (oldValue || binding.placeholder) || previous === binding.placeholder)
      ? (value || binding.placeholder) : previous;
    const start = body.length;
    body += replacement;
    bindings.push({ ...binding, start, end: body.length });
    cursor = binding.end;
  }
  state.data.bindings = bindings;
  state.body = body + oldBody.slice(cursor);
  return state.body;
}

// Highlights shared placeholders and values in preview text nodes, never in HTML attributes.
// The document renderer remains the security boundary; marks are presentation-only DOM nodes.
function renderLinkedPreview(state) {
  const container = document.createElement('div');
  container.innerHTML = safeMarkdown(state.body);
  const values = [...new Set(state.data.bindings.flatMap(binding =>
    state.body.slice(binding.start, binding.end).split(/\r?\n/).filter(value => value.trim())
  ))].sort((a, b) => b.length - a.length);
  if (!values.length) return container.innerHTML;
  const walker = document.createTreeWalker(container, 4);
  const nodes = [];
  while (walker.nextNode()) nodes.push(walker.currentNode);
  for (const node of nodes) {
    const text = node.textContent;
    const fragment = document.createDocumentFragment();
    let cursor = 0;
    let changed = false;
    while (cursor < text.length) {
      let start = text.length;
      let value = '';
      for (const candidate of values) {
        const at = text.indexOf(candidate, cursor);
        if (at !== -1 && at < start) { start = at; value = candidate; }
      }
      if (!value) { fragment.append(document.createTextNode(text.slice(cursor))); break; }
      fragment.append(document.createTextNode(text.slice(cursor, start)));
      const mark = document.createElement('mark');
      mark.className = 'template-field-highlight';
      mark.textContent = value;
      fragment.append(mark);
      cursor = start + value.length;
      changed = true;
    }
    if (changed) node.replaceWith(fragment);
  }
  return container.innerHTML;
}

// Renders one shared set of eight inputs while retaining metadata for every saved form.
function consentFactsMarkup(item, showInputs) {
  const data = item.editor_data || { values: {}, bindings: consentFactBindings(item.body || '') };
  return `<div data-consent-facts data-consent-facts-state="${escapeHtml(JSON.stringify(data))}" class="consent-item-fields">${showInputs ? CONSENT_FACTS.map(([key, label, , placeholder = '']) => `<label><span>${label}</span><input type="text" data-consent-fact="${key}" placeholder="${escapeHtml(placeholder)}" value="${escapeHtml(data.values[key] || '')}"></label>`).join('') : ''}</div>`;
}

// Restores saved occurrence links without guessing values from ordinary prose.
function consentFactState(item) {
  if (!item.consentFactState) item.consentFactState = createLinkedText(
    item.querySelector('[data-consent-field="body"]').value,
    JSON.parse(item.querySelector('[data-consent-facts]').dataset.consentFactsState),
  );
  return item.consentFactState;
}

// Finds the two catalogue sections that may be edited, leaving the surrounding text fixed.
function collectionRanges(body) {
  const markers = CONSENT_TEMPLATE_OPTIONS.flatMap(template => {
    const marker = `${template.title}\n`;
    const at = body.startsWith(marker) ? 0 : body.indexOf(`\n\n${marker}`) + 2;
    return at >= 2 || body.startsWith(marker) ? [{ ...template, start: at, content: at + marker.length }] : [];
  }).sort((a, b) => a.start - b.start);
  return markers.flatMap((section, index) => [
    'informed_consent_personal_information_collected_v1',
    'informed_consent_non_personal_information_collected_v1',
  ].includes(section.id) ? [{ ...section, end: markers[index + 1] ? markers[index + 1].start - 2 : body.length }] : []);
}

// Updates the read-only document preview and the editable collection excerpts after field changes.
function refreshConsentItem(item, refreshCollection = true) {
  const body = consentFactState(item).body;
  item.querySelector('[data-consent-field="body"]').value = body;
  item.querySelector('[data-consent-preview]').innerHTML = renderLinkedPreview(consentFactState(item));
  if (refreshCollection) for (const section of collectionRanges(body)) {
    const control = Array.from(item.querySelectorAll('[data-consent-collection]')).find(input => input.dataset.consentCollection === section.id);
    if (control) control.value = body.slice(section.content, section.end);
  }
}

// Returns the shared study values for imprint and privacy rendering.
function sharedValues(editor) {
  return Object.fromEntries(Array.from(editor.querySelectorAll('[data-consent-fact]')).map(input => [input.dataset.consentFact, input.value]));
}

// Serializes the rendered statement and private field bindings together for save/reopen.
function readConsentItem(item) {
  const state = consentFactState(item);
  const rawBody = item.querySelector('[data-consent-field="body"]').value;
  reconcileConsentFacts(state.data, state.body, rawBody);
  state.body = rawBody;
  const body = rawBody.trim();
  const leading = rawBody.length - rawBody.trimStart().length;
  const data = structuredClone(state.data);
  data.bindings = data.bindings
    .filter(binding => binding.start >= leading && binding.end <= leading + body.length)
    .map(binding => ({ ...binding, start: binding.start - leading, end: binding.end - leading }));
  return {
    id: item.querySelector('[data-consent-field="id"]').value.trim(),
    title: item.querySelector('[data-consent-field="title"]').value.trim(),
    body,
    required: item.querySelector('[data-consent-field="required"]').checked,
    editor_data: data,
  };
}

// Shows collection editors and a generated preview; fixed consent prose has no editing control.
function consentItemMarkup(item, index) {
  const sections = collectionRanges(item.body || '');
  return `<section class="consent-item">
    ${consentFactsMarkup(item, index === 0)}
    <input type="hidden" data-consent-field="id" value="${escapeHtml(item.id || '')}">
    <input type="hidden" data-consent-field="title" value="${escapeHtml(item.title || '')}">
    <input type="checkbox" data-consent-field="required" hidden ${item.required ? 'checked' : ''}>
    <textarea data-consent-field="body" readonly hidden>${escapeHtml(item.body || '')}</textarea>
    ${sections.map(section => `<label class="consent-collection"><span>${escapeHtml(section.title)}</span><textarea rows="12" data-consent-collection="${escapeHtml(section.id)}">${escapeHtml(item.body.slice(section.content, section.end))}</textarea></label>`).join('')}
    <h3>Informed consent preview</h3><div class="document-markdown" data-consent-preview>${renderLinkedPreview(createLinkedText(item.body || '', item.editor_data))}</div>
  </section>`;
}

// Automatically supplies the complete startup catalogue when a template has no consent yet.
function renderConsentEditor(field, items = []) {
  const forms = items.length ? items : [{ id: 'consent-1', title: 'Consent Form', required: true,
    body: CONSENT_TEMPLATE_OPTIONS.map(template => `${template.title}\n${template.body}`).join('\n\n') }];
  return `<div class="consent-editor" data-consent-editor data-consent-path="${escapeHtml(field.path)}"><p class="config-help">Study details are shared with the imprint and privacy notice. Edit the data-collection sections below; the remaining consent text comes from the template.</p>${forms.map(consentItemMarkup).join('')}</div>`;
}

// Synchronizes the eight fields across forms and limits direct prose edits to data collection.
function initializeConsentEditor(editor) {
  editor.querySelectorAll('.consent-item').forEach(consentFactState);
  editor.addEventListener('input', event => {
    if (event.target.matches('[data-consent-fact]')) {
      editor.querySelectorAll('.consent-item').forEach(item => {
        updateLinkedText(consentFactState(item), event.target.dataset.consentFact, event.target.value);
        refreshConsentItem(item);
      });
    } else if (event.target.matches('[data-consent-collection]')) {
      const item = event.target.closest('.consent-item');
      const state = consentFactState(item);
      const range = collectionRanges(state.body).find(section => section.id === event.target.dataset.consentCollection);
      if (!range) return;
      editLinkedText(state, state.body.slice(0, range.content) + event.target.value + state.body.slice(range.end));
      refreshConsentItem(item, false);
    }
  });
}

  return { renderConsentEditor, initializeConsentEditor, readConsentItem, sharedValues, createLinkedText, editLinkedText, updateLinkedText, renderLinkedPreview };
}
