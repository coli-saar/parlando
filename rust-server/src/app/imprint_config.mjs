import { safeMarkdown } from './document-markdown.js';
// Disk-loaded country templates; saved plain text remains authoritative.
import IMPRINT_TEMPLATES from './imprint-templates.js';

export const IMPRINT_COUNTRIES = [
  ['', 'Select country'], ...IMPRINT_TEMPLATES.map(template => [template.id, template.title]),
];

// Loads the chosen Markdown verbatim so required facts stay visible as editable placeholders.
export function generateImprint(country) {
  if (!country) return '';
  const template = IMPRINT_TEMPLATES.find(item => item.id === country);
  if (!template) throw new Error('Unsupported imprint country');
  return template.body;
}

// Escapes values for attribute and textarea contexts before inserting editor markup.
function escape(value) {
  return String(value ?? '').replace(/[&<>"']/g, ch => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[ch]));
}


// One selector and one editable draft retain saved copy until the country is changed.
export function renderImprintEditor(imprint = {}) {
  return `<div class="config-fields" data-imprint-editor>
    <label class="config-field"><span>Country of the operator</span><select data-config-path="direct.imprint.country" data-config-type="string">${IMPRINT_COUNTRIES.map(([code, label]) => `<option value="${escape(code)}" ${imprint.country === code ? 'selected' : ''}>${escape(label)}</option>`).join('')}</select></label>
    <label class="config-field"><span>Imprint text</span><textarea data-config-path="direct.imprint.text" data-config-type="string" rows="16" maxlength="20000">${escape(imprint.text)}</textarea></label>
    <p class="config-help">Choose a country to load its template. Replace the {{placeholders}} directly in the text and remove inapplicable lines. Changing country replaces this draft. Save a new revision to publish it.</p>
    </div>`;
}

// Only selecting a country replaces text; typing and reopening preserve the conductor's wording.
export function bindImprintEditor(editor, onChange = () => {}) {
  const country = editor.querySelector('[data-config-path="direct.imprint.country"]');
  const text = editor.querySelector('[data-config-path="direct.imprint.text"]');
  country.addEventListener('change', () => {
    text.value = generateImprint(country.value);
    onChange(text.value);
  });
  text.addEventListener('input', () => onChange(text.value));
  onChange(text.value);
}

// Renders the editor's Markdown in the Privacy tab while keeping raw HTML inert.
// Draft labeling distinguishes the immediate preview from the saved public page/download.
export function renderImprintPreview(text = '', savedText = '') {
  if (!text.trim()) return '<p class="muted">Choose a country and complete the imprint under Configuration → Participant access and consent → Imprint.</p>';
  const status = text === savedText ? 'Saved imprint' : 'Draft preview — save a new revision to update the public page and download.';
  return `<p class="muted small">${status}</p><article class="privacy-document"><h1>Imprint</h1><div class="document-markdown">${safeMarkdown(text)}</div></article>`;
}

