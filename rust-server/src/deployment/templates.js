import { createConsentEditor } from './consent-config.js';
import CONSENT_TEMPLATES from './consent-templates.js';
import PRIVACY_TEMPLATES from './privacy-notice-templates.js';
import { safeMarkdown } from './document-markdown.js';
import { renderImprintEditor, generateImprint } from './imprint-config.js';
import { renderPrivacyNoticeEditor, generatePrivacyNotice } from './privacy-notice-config.js';

// Escapes untrusted library names and document values for attribute and text contexts.
function escapeHtml(value) {
  return String(value ?? '').replace(/[&<>"']/g, ch => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[ch]));
}
const consent = createConsentEditor({ templates: CONSENT_TEMPLATES, escapeHtml, safeMarkdown });

// Owns shared document drafts; saving never writes an experiment configuration.
export function createTemplateLibrary(root, request, notify) {
  let records = [];
  let current = null;
  const drafts = new Map();
  let notices = {};
  root.innerHTML = `<p>Create reusable document templates, then apply a saved template in an experiment's configuration. Saving here does not change published experiment pages.</p>
    <div class="template-toolbar"><label>Saved template<select data-library-select><option value="">New template</option></select></label><button type="button" data-library-new>New template</button></div>
    <form data-library-form><label>Template name<input data-library-name maxlength="200" required></label><div data-library-editors></div><button class="primary" type="submit">Save template</button><span role="status" data-library-status></span></form>`;
  const select = root.querySelector('[data-library-select]');
  const form = root.querySelector('[data-library-form]');
  const editors = root.querySelector('[data-library-editors]');
  const name = root.querySelector('[data-library-name]');
  const status = root.querySelector('[data-library-status]');

  // Captures the three editors as a single reusable snapshot, including consent field bindings.
  function read() {
    const value = path => editors.querySelector(`[data-config-path="${path}"]`).value;
    return {
      id: current?.id ?? null, expected_revision: current?.revision ?? 0, name: name.value,
      documents: {
        consents: Array.from(editors.querySelectorAll('.consent-item')).map(consent.readConsentItem),
        imprint: { country: value('direct.imprint.country'), text: value('direct.imprint.text') },
        privacy_notice: { jurisdiction: value('direct.privacy_notice.jurisdiction'), text: value('direct.privacy_notice.text') },
        notice_fields: { imprint: structuredClone(notices.imprint.data), privacy_notice: structuredClone(notices.privacy_notice.data) },
      },
    };
  }

  // Keeps the current document draft when moving between saved templates.
  function stash() { drafts.set(String(current?.id || ''), read()); }

  // Applies the consent inputs to both notices without guessing facts in unrelated prose.
  function synchronizeNotices() {
    const values = consent.sharedValues(editors.querySelector('[data-consent-editor]'));
    for (const key of ['imprint', 'privacy_notice']) {
      const text = editors.querySelector(`[data-config-path="direct.${key}.text"]`);
      for (const [field, value] of Object.entries(values)) consent.updateLinkedText(notices[key], field, value);
      text.value = notices[key].body;
      editors.querySelector(`[data-notice-preview="${key}"]`).innerHTML = consent.renderLinkedPreview(notices[key]);
    }
  }

  // Restores links for notice editing and resets them when a different country template is chosen.
  function initializeNotices(documents) {
    for (const key of ['imprint', 'privacy_notice']) {
      const wrapper = editors.querySelector(key === 'imprint' ? '[data-imprint-editor]' : '[data-privacy-notice-editor]');
      const text = wrapper.querySelector('textarea');
      const selector = wrapper.querySelector('select');
      selector.value = documents[key]?.[key === 'imprint' ? 'country' : 'jurisdiction'] || '';
      notices[key] = consent.createLinkedText(text.value, documents.notice_fields?.[key]);
      wrapper.insertAdjacentHTML('beforeend', `<div class="document-markdown" data-notice-preview="${key}"></div>`);
      // Applicability guidance follows the current jurisdiction without changing its content.
      const updateScope = () => {
        const scope = wrapper.querySelector('[data-privacy-scope]');
        if (scope) scope.textContent = PRIVACY_TEMPLATES.find(template => template.id === selector.value)?.description || '';
      };
      selector.addEventListener('change', () => {
        notices[key] = consent.createLinkedText(key === 'imprint' ? generateImprint(selector.value) : generatePrivacyNotice(selector.value));
        updateScope();
        synchronizeNotices();
      });
      text.addEventListener('input', () => {
        consent.editLinkedText(notices[key], text.value);
        editors.querySelector(`[data-notice-preview="${key}"]`).innerHTML = consent.renderLinkedPreview(notices[key]);
      });
      updateScope();
    }
    synchronizeNotices();
  }

  // Restores a saved or in-session draft without discarding work when switching templates.
  function show(record) {
    current = record;
    const draft = drafts.get(String(record?.id || ''));
    if (draft && current) current = { ...current, revision: draft.expected_revision };
    const documents = structuredClone(draft?.documents || record?.documents || {});
    // Start empty notices with EU defaults while preserving saved text and country choices.
    if (!documents.imprint?.country && !documents.imprint?.text?.trim()) {
      documents.imprint = { country: 'EU', text: generateImprint('EU') };
      if (documents.notice_fields) delete documents.notice_fields.imprint;
    }
    if (!documents.privacy_notice?.jurisdiction && !documents.privacy_notice?.text?.trim()) {
      documents.privacy_notice = { jurisdiction: 'EU', text: generatePrivacyNotice('EU') };
      if (documents.notice_fields) delete documents.notice_fields.privacy_notice;
    }
    name.value = draft?.name ?? record?.name ?? 'Default template';
    select.value = record ? String(record.id) : '';
    editors.innerHTML = `<section class="template-section"><h2>Informed consent</h2><div>${consent.renderConsentEditor({ path: 'direct.consents' }, documents.consents || [])}</div></section>
      <section class="template-section"><h2>Imprint</h2><div>${renderImprintEditor(documents.imprint || {})}</div></section>
      <section class="template-section"><h2>Privacy notice</h2><div>${renderPrivacyNoticeEditor(documents.privacy_notice || {})}</div></section>`;
    consent.initializeConsentEditor(editors.querySelector('[data-consent-editor]'));
    initializeNotices(documents);
    editors.querySelector('[data-consent-editor]').addEventListener('input', event => {
      if (event.target.matches('[data-consent-fact]')) synchronizeNotices();
    });
    // The shared editors retain their original instructions when used outside the library.
    editors.querySelectorAll('.config-help').forEach(help => {
      help.textContent = help.textContent.replace('Save a new revision to publish it.', 'Save this template to reuse it in experiments.').replace('save a new revision to publish it.', 'save this template to reuse it in experiments.');
    });
    status.textContent = record ? `Template revision ${record.revision}` : 'New draft';
  }

  // Refreshes saved choices while leaving the open editor and unsaved text intact.
  async function refresh() {
    const result = await request('/templates');
    records = result.templates;
    select.innerHTML = '<option value="">New template</option>' + records.map(record => `<option value="${record.id}">${escapeHtml(record.name)}</option>`).join('');
    select.value = current ? String(current.id) : '';
    if (!editors.childElementCount) show(null);
  }

  select.addEventListener('change', () => {
    stash();
    show(records.find(record => String(record.id) === select.value) || null);
  });
  root.querySelector('[data-library-new]').addEventListener('click', () => {
    stash();
    show(null);
  });
  form.addEventListener('submit', async event => {
    event.preventDefault();
    const controls = Array.from(root.querySelectorAll('input, select, textarea, button')).map(control => [control, control.disabled]);
    controls.forEach(([control]) => { control.disabled = true; });
    status.textContent = 'Saving…';
    try {
      const saved = await request('/templates', { method: 'POST', body: read() });
      drafts.delete(String(current?.id || ''));
      current = saved;
      await refresh();
      show(saved);
      status.textContent = 'Template saved. Existing experiment pages are unchanged.';
    } catch (error) {
      status.textContent = error.message;
      notify(error.message, true);
    } finally { controls.forEach(([control, disabled]) => { control.disabled = disabled; }); }
  });
  show(null);
  return { refresh };
}
