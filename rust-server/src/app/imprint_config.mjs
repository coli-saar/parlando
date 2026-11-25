// Disk-loaded country templates; saved plain text remains authoritative.
import IMPRINT_TEMPLATES from './imprint-templates.js';

export const IMPRINT_COUNTRIES = [
  ['', 'Select country'], ...IMPRINT_TEMPLATES.map(template => [template.id, template.title]),
];

// Substitutes only explicit public identity fields, preserving the file's fallback wording.
// A single replacement pass treats inserted contact values as literal text.
export function generateImprint(country, identity = {}) {
  if (!country) return '';
  const template = IMPRINT_TEMPLATES.find(item => item.id === country);
  if (!template) throw new Error('Unsupported imprint country');
  return template.body.replace(/{{(operator|address|responsible_person|email|phone)\|([^}]+)}}/g,
    (_, key, fallback) => identity[key]?.trim() || fallback);
}

// Escapes values for attribute and textarea contexts before inserting editor markup.
function escape(value) {
  return String(value ?? '').replace(/[&<>"']/g, ch => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[ch]));
}

// Only standalone HTTPS lines become links; all text and attributes remain escaped.
function linkedImprintText(text) {
  return text.split('\n').map(line => /^https:\/\/[^\s]+$/.test(line)
    ? `<a href="${escape(line)}" rel="noopener noreferrer">${escape(line)}</a>`
    : escape(line)).join('\n');
}

// Renders saved identity values, prefilling the operator from shared institution settings.
export function renderImprintEditor(imprint = {}, institution = '') {
  const fields = [['operator', 'Operating institution / legal operator'], ['responsible_person', 'Responsible person'], ['address', 'Public postal address'], ['email', 'Public contact email'], ['phone', 'Public telephone']];
  return `<div class="config-fields" data-imprint-editor>
    <label class="config-field"><span>Country of the operator</span><select data-config-path="direct.imprint.country" data-config-type="string">${IMPRINT_COUNTRIES.map(([code, label]) => `<option value="${escape(code)}" ${imprint.country === code ? 'selected' : ''}>${escape(label)}</option>`).join('')}</select></label>
    ${fields.map(([key, label]) => `<label class="config-field"><span>${label}</span><input data-config-path="direct.imprint.${key}" data-config-type="string" value="${escape(imprint[key] || (key === 'operator' ? institution : ''))}"></label>`).join('')}
    <label class="config-field"><span>Imprint text</span><textarea data-config-path="direct.imprint.text" data-config-type="string" rows="16" maxlength="20000">${escape(imprint.text)}</textarea></label>
    <p class="config-help">Choose the operator’s country to generate a starting point for a public annotation study. Changing the country replaces the template text. Complete placeholders and remove inapplicable lines before saving. Saving publishes this text on the study’s Imprint page. The responsible contact is not automatically the legal representative.</p>
    </div>`;
}

// Builds the Privacy-tab document from the editor's exact plain text, without executing HTML.
// Draft labeling distinguishes the immediate preview from the saved public page/download.
export function renderImprintPreview(text = '', savedText = '') {
  if (!text.trim()) return '<p class="muted">Choose a country and complete the imprint under Configuration → Participant access and consent → Imprint.</p>';
  const status = text === savedText ? 'Saved imprint' : 'Draft preview — save a new revision to update the public page and download.';
  return `<p class="muted small">${status}</p><article class="privacy-document"><h1>Imprint</h1><pre style="white-space:pre-wrap;overflow-wrap:anywhere;font:inherit">${linkedImprintText(text)}</pre></article>`;
}

// Country selection replaces the template directly. Contact edits update untouched copy;
// manual prose remains authoritative until another country is selected.
export function bindImprintEditor(editor, onChange = () => {}) {
  const control = key => editor.querySelector(`[data-config-path="direct.imprint.${key}"]`);
  const country = control('country');
  const text = control('text');
  const identity = () => Object.fromEntries(['operator', 'responsible_person', 'address', 'email', 'phone'].map(key => [key, control(key).value]));
  let generated = generateImprint(country.value, identity());
  country.addEventListener('change', () => {
    text.value = generated = generateImprint(country.value, identity());
    onChange(text.value);
  });
  text.addEventListener('input', () => onChange(text.value));
  for (const key of ['operator', 'responsible_person', 'address', 'email', 'phone']) {
    control(key).addEventListener('input', () => {
      const next = generateImprint(country.value, identity());
      if (!text.value || text.value === generated) text.value = next;
      generated = next;
      onChange(text.value);
    });
  }
  onChange(text.value);
}
