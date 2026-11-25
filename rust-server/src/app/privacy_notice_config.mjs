import { safeMarkdown } from './document-markdown.js';
// Jurisdiction Markdown is loaded at startup and supplied by the authenticated catalogue asset.
import PRIVACY_TEMPLATES from './privacy-notice-templates.js';

// Returns the complete template without interpolating or inferring any study facts.
export function generatePrivacyNotice(jurisdiction) {
  if (!jurisdiction) return '';
  const template = PRIVACY_TEMPLATES.find(t => t.id === jurisdiction);
  if (!template) throw new Error('Unsupported privacy notice jurisdiction');
  return template.body;
}

// Escapes all draft values before inserting them into HTML attributes or textareas.
function escape(value) {
  return String(value ?? '').replace(/[&<>"']/g, ch => ({'&':'&amp;', '<':'&lt;', '>':'&gt;', '"':'&quot;', "'":'&#39;'}[ch]));
}

// Renders a jurisdiction selector and one editable text area; the saved text is authoritative.
export function renderPrivacyNoticeEditor(notice = {}) {
  return `<div class="config-fields" data-privacy-notice-editor>
    <label class="config-field"><span>Jurisdiction template</span><select data-config-path="direct.privacy_notice.jurisdiction" data-config-type="string"><option value="">Select jurisdiction</option>${PRIVACY_TEMPLATES.map(t => `<option value="${escape(t.id)}" ${notice.jurisdiction === t.id ? 'selected' : ''}>${escape(t.title)}</option>`).join('')}</select></label>
    <p class="config-help" data-privacy-scope></p>
    <label class="config-field"><span>Privacy notice text</span><textarea rows="20" maxlength="100000" data-config-path="direct.privacy_notice.text" data-config-type="string">${escape(notice.text || '')}</textarea></label>
    <p class="config-help">Choose a jurisdiction to load its template. Replace the {{placeholders}} directly in the text. Changing jurisdiction replaces this draft. Review applicable requirements and save a new revision to publish it.</p>
  </div>`;
}

// Updates the preview on typing; only an explicit selector change loads new template text.
export function bindPrivacyNoticeEditor(editor, onChange = () => {}) {
  const country = editor.querySelector('[data-config-path="direct.privacy_notice.jurisdiction"]');
  const text = editor.querySelector('[data-config-path="direct.privacy_notice.text"]');
  const scope = editor.querySelector('[data-privacy-scope]');
  // Displays applicability guidance without changing the saved or edited notice.
  const preview = () => {
    scope.textContent = PRIVACY_TEMPLATES.find(t => t.id === country.value)?.description || '';
    onChange(text.value);
  };
  country.addEventListener('change', () => { text.value = generatePrivacyNotice(country.value); preview(); });
  text.addEventListener('input', preview);
  preview();
}


// Escaped previews use the exact snapshot that will be saved; links open the saved public notice.
export function renderPrivacyNoticePreview(text = '', saved = '') {
  if (!text.trim()) return '<p class="muted">Please configure the privacy notice.</p>';
  return `<p class="muted small">${text === saved ? 'Saved privacy notice' : 'Draft preview — save a new revision to publish.'}</p><article class="privacy-document"><h1>Privacy notice</h1><div class="document-markdown">${safeMarkdown(text)}</div></article>`;
}
