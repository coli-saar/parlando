// Runtime catalogues supply the jurisdiction copy and recognized consent section headings.
import PRIVACY_TEMPLATES from './privacy-notice-templates.js';
import CONSENT_TEMPLATES from './consent-templates.js';

// Supplemental facts are public text, never inferred from the operator's country or software settings.
const FIELD_HELP = {
  review_date: 'Date this notice was last reviewed',
  controller_details: 'Confirm the legal controller, privacy contact/DPO and representatives where required. Complete any gaps in the source contact details.',
  collection_details: 'Complete gaps: data categories (including sensitive data), sources, purposes per category, hosting logs, voice/AI providers and significant privacy risks.',
  storage_details: 'Complete gaps: storage locations/countries, retention per category (including logs, consent/payment records and backups), recipients, and research reuse/publication.',
  cookies: 'Describe actual cookies, local storage and similar technologies, purposes, retention and choices, or confirm none are used. Include hosting and embedded services.',
  legal_basis: 'State the legal basis for each purpose, legitimate interests if relied on, and any special-category condition. Study participation consent is not automatically the processing basis.',
  transfers: 'Identify foreign destinations, recipients, applicable safeguards and how to obtain a copy; state if no international transfers occur.',
  rights_contact: 'Give request methods/contact, verification steps, relevant limits, consent withdrawal procedure and the competent regulator/contact. The research anonymization window does not limit rights over other identifiable records.',
  required_data: 'Explain which information is required by law or contract or for participation, consequences of not supplying it, and optional choices.',
  automated_decisions: 'State whether legally or similarly significant automated decisions/profiling occur; if so explain logic, significance, consequences and available review/contestation rights.',
  california_collection: 'List California personal/sensitive information categories, purposes per category and retention periods or criteria per category at collection.',
  california_history: 'Describe categories collected, sources, business/commercial purposes and categories disclosed with recipient categories during the preceding 12 months.',
  california_sale_sharing: 'State sale/sharing practices and categories, opt-out methods/link and Global Privacy Control handling; describe sensitive-information uses and any applicable limit-use link. Do not assume research disclosure is exempt.',
  california_requests: 'Give applicable request methods and links, identity verification, authorized-agent procedure and appeal/contact options. Explain applicable exceptions without restricting statutory rights to two weeks.',
  california_minors: 'Describe applicable under-16 sale/sharing opt-in practices or confirm no such sale/sharing; include relevant financial-incentive disclosures, if any.',
  swiss_transfers: 'Name destination countries and, where relevant, safeguards or statutory exceptions for foreign disclosures.',
  canada_scope: 'Identify applicable federal/provincial/public-sector law and privacy officer/complaint route; add required provincial disclosures (including Quebec requirements if applicable).',
  canada_consent: 'Explain meaningful consent choices, significant residual risks, withdrawal and its consequences, and any processing without consent allowed by applicable law.',
  brazil_agents: 'Identify the controller and encarregado/contact channel, responsibilities of processing agents, shared use and its purposes; explain the option to refuse consent and consequences.',
};

// Escapes all draft values before inserting them into HTML attributes or textareas.
function escape(value) {
  return String(value ?? '').replace(/[&<>"']/g, ch => ({'&':'&amp;', '<':'&lt;', '>':'&gt;', '"':'&quot;', "'":'&#39;'}[ch]));
}

// Lists editable placeholders from the template, excluding automatically synchronized sources.
export function detailKeys(template) {
  return [...new Set([...template.body.matchAll(/{{([a-z][a-z0-9_]*)}}/g)].map(match => match[1]))];
}

// Extracts complete relevant sections from generated forms or individually titled items.
// Catalogue headings delimit sections; arbitrary prose is not guessed to be a controller or legal basis.
export function consentPrivacyText(consents = []) {
  const included = new Set(['informed_consent_study_subject_v1', 'informed_consent_data_processing_v1',
    'informed_consent_non_personal_information_collected_v1', 'informed_consent_personal_information_collected_v1',
    'informed_consent_confidentiality_v1', 'informed_consent_data_storage_v1',
    'informed_consent_disclosure_publication_v1', 'informed_consent_withdrawal_v1',
    'informed_consent_participant_rights_v1', 'informed_consent_controller_contact_v1', 'voice_and_transcription_v1_0']);
  const headings = new Map(CONSENT_TEMPLATES.map(t => [t.title, t.id]));
  const blocks = [];
  for (const item of consents) {
    let title = item.title?.trim() || '';
    let lines = [];
    // Flush only classified, nonempty copy; unknown custom forms require explicit supplemental disclosure.
    const flush = () => {
      if (included.has(headings.get(title)) && lines.join('\n').trim()) blocks.push(`${title}\n${lines.join('\n').trim()}`);
    };
    const body = String(item.body || '').replace(/\r\n/g, '\n').split('\n');
    body.forEach((line, index) => {
      if (headings.has(line.trim()) && (index === 0 || !body[index - 1].trim())) {
        flush(); title = line.trim(); lines = [];
      } else lines.push(line);
    });
    flush();
  }
  return blocks.join('\n\n');
}

// Single-pass substitution leaves source placeholders visible and treats inserted text literally.
export function generatePrivacyNotice(jurisdiction, details = {}, source = {}) {
  if (!jurisdiction) return '';
  const template = PRIVACY_TEMPLATES.find(t => t.id === jurisdiction);
  if (!template) throw new Error('Unsupported privacy notice jurisdiction');
  const imprint = source.imprint || {};
  const contacts = [['operator', 'Operator'], ['responsible_person', 'Responsible person'], ['address', 'Address'], ['email', 'Email'], ['phone', 'Telephone']]
    .filter(([key]) => imprint[key]?.trim()).map(([key, label]) => `${label}: ${imprint[key].trim()}`).join('\n');
  const sources = {
    'source.imprint': [contacts, imprint.text?.trim()].filter(Boolean).join('\n\n'),
    'source.consent': consentPrivacyText(source.consents),
  };
  return template.body.replace(/{{([a-z][a-z0-9_.]*)}}/g, (token, key) => (key.startsWith('source.') ? sources[key] : details[key])?.trim() || token);
}

// All jurisdiction details remain in the DOM so switching templates preserves previous additions.
export function renderPrivacyNoticeEditor(notice = {}) {
  const keys = [...new Set(PRIVACY_TEMPLATES.flatMap(detailKeys))];
  return `<div class="config-fields" data-privacy-notice-editor>
    <label class="config-field"><span>Jurisdiction template</span><select data-config-path="direct.privacy_notice.jurisdiction" data-config-type="string"><option value="">Select jurisdiction</option>${PRIVACY_TEMPLATES.map(t => `<option value="${escape(t.id)}" ${notice.jurisdiction === t.id ? 'selected' : ''}>${escape(t.title)}</option>`).join('')}</select></label>
    <p class="config-help" data-privacy-scope></p>
    <p class="config-help">The notice updates from the generated consent forms and imprint above. Recognized privacy sections are copied; include relevant custom-section content in the additional disclosures. Generate your selected consent sections first. Complete the additional disclosures and review all {{placeholders}} before publication. Several laws may apply; a single template does not establish coverage or compliance. Saving a new revision publishes the notice.</p>
    ${keys.map(key => `<label class="config-field" data-privacy-detail="${escape(key)}"><span>${escape(FIELD_HELP[key] || key.replaceAll('_', ' '))}</span><textarea rows="3" maxlength="20000" data-config-path="direct.privacy_notice.details.${escape(key)}" data-config-type="string">${escape(notice.details?.[key] || '')}</textarea></label>`).join('')}
    <label class="config-field"><span>Generated privacy notice</span><textarea readonly rows="20" data-config-path="direct.privacy_notice.text" data-config-type="string">${escape(notice.text || '')}</textarea></label>
    <p class="config-help" data-privacy-review role="status"></p>
  </div>`;
}

// Updates the generated draft after source edits, form generation/removal and jurisdiction changes.
// Delegated properties replace previous bindings when configuration is re-rendered.
export function bindPrivacyNoticeEditor(form, readSource, onChange) {
  const editor = form.querySelector('[data-privacy-notice-editor]');
  if (!editor) return;
  const control = key => editor.querySelector(`[data-config-path="direct.privacy_notice.${key}"]`);
  // Read current controls on every event, including form mutations handled earlier in bubbling.
  const refresh = () => {
    const template = PRIVACY_TEMPLATES.find(t => t.id === control('jurisdiction').value);
    const keys = template ? detailKeys(template) : [];
    const details = {};
    editor.querySelectorAll('[data-privacy-detail]').forEach(label => {
      const key = label.dataset.privacyDetail;
      label.hidden = !keys.includes(key);
      details[key] = control(`details.${key}`).value;
    });
    editor.querySelector('[data-privacy-scope]').textContent = template?.description || '';
    const text = generatePrivacyNotice(control('jurisdiction').value, details, readSource());
    control('text').value = text;
    editor.querySelector('[data-privacy-review]').textContent = text.includes('{{') ? 'This draft contains incomplete placeholders. Complete the source forms and additional disclosures before publishing.' : '';
    onChange(text);
  };
  form.oninput = refresh;
  form.onchange = refresh;
  form.onclick = refresh;
  refresh();
}

// Links only standalone HTTPS source lines; all other copy stays escaped plain text.
function linkedNoticeText(text) {
  return text.split('\n').map(line => /^https:\/\/[^\s]+$/.test(line)
    ? `<a href="${escape(line)}" rel="noopener noreferrer">${escape(line)}</a>` : escape(line)).join('\n');
}

// Escaped previews use the exact snapshot that will be saved; links open the saved public notice.
export function renderPrivacyNoticePreview(text = '', saved = '') {
  if (!text.trim()) return '<p class="muted">Please configure the privacy notice.</p>';
  return `<p class="muted small">${text === saved ? 'Saved privacy notice' : 'Draft preview — save a new revision to publish.'}</p><article class="privacy-document"><h1>Privacy notice</h1><pre style="white-space:pre-wrap;overflow-wrap:anywhere;font:inherit">${linkedNoticeText(text)}</pre></article>`;
}
