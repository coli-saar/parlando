// @vitest-environment happy-dom
import { expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { fireEvent, waitFor } from '@testing-library/react';
import { safeMarkdown } from './documentMarkdown.mjs';

const base = resolve(dirname(fileURLToPath(import.meta.url)), '../../rust-server/src');
/** Loads the actual browser modules with injected startup catalogues and no HTTP server. */
function unit(file: string, names: string[], values: unknown[], exports: string) {
  const source = readFileSync(resolve(base, file), 'utf8').replace(/^import .*;\n/gm, '').replace(/^export /gm, '');
  return new Function(...names, source + `\nreturn { ${exports} };`)(...values);
}
const { createConsentEditor } = unit('app/consent_config.mjs', [], [], 'createConsentEditor');
const imprint = unit('app/imprint_config.mjs', ['IMPRINT_TEMPLATES', 'safeMarkdown'], [[{ id: 'EU', title: 'EU', body: 'Operator: {{full legal name of the operating institution or individual}}. Contact {{name}}, {{public contact email}}.' }, { id: 'DE', title: 'Germany', body: 'Operator: {{full legal name of the operating institution or individual}}. Contact {{name}}, {{public contact email}}.' }], (s: string) => s], 'renderImprintEditor, generateImprint');
const privacy = unit('app/privacy_notice_config.mjs', ['PRIVACY_TEMPLATES', 'safeMarkdown'], [[{ id: 'EU', title: 'EU/EEA', body: 'Contact {{your name}} at {{your institution}}, {{your email}} until {{end date}}.' }], (s: string) => s], 'renderPrivacyNoticeEditor, generatePrivacyNotice');
const { createTemplateLibrary } = unit('deployment/templates.js', ['createConsentEditor', 'CONSENT_TEMPLATES', 'renderImprintEditor', 'generateImprint', 'renderPrivacyNoticeEditor', 'generatePrivacyNotice', 'PRIVACY_TEMPLATES', 'safeMarkdown'], [createConsentEditor, [{ id: 'study', title: 'Study', description: 'Study text', body: '{{name}} studies games.' }], imprint.renderImprintEditor, imprint.generateImprint, privacy.renderPrivacyNoticeEditor, privacy.generatePrivacyNotice, [], safeMarkdown], 'createTemplateLibrary');

/** Shared saves retain all three documents and editing metadata, without publishing an experiment. */
it('opens default consent, shares fields with notices, and preserves saved and unsaved drafts', async () => {
  document.body.innerHTML = '<section id="templates"></section>';
  const root = document.querySelector('#templates')!;
  let records: any[] = [];
  const request = vi.fn(async (_path: string, options?: any) => {
    if (options?.method === 'POST') {
      const saved = { id: 1, revision: 1, name: options.body.name, documents: structuredClone(options.body.documents) };
      records = [saved];
      return saved;
    }
    return { templates: records };
  });
  const library = createTemplateLibrary(root, request, vi.fn());
  await library.refresh();
  expect(root.querySelector<HTMLSelectElement>('[data-config-path="direct.imprint.country"]')!.value).toBe('EU');
  expect(root.querySelector<HTMLSelectElement>('[data-config-path="direct.privacy_notice.jurisdiction"]')!.value).toBe('EU');
  expect(root.querySelector('[data-generate-consent]')).toBeNull();
  expect(root.querySelectorAll('.consent-item')).toHaveLength(1);
  fireEvent.input(root.querySelector('[data-consent-fact="name"]')!, { target: { value: 'Alex' } });
  fireEvent.input(root.querySelector('[data-consent-fact="institute"]')!, { target: { value: 'Test Institute' } });
  fireEvent.input(root.querySelector('[data-consent-fact="email"]')!, { target: { value: 'study@example.org' } });
  fireEvent.input(root.querySelector('[data-library-name]')!, { target: { value: 'My study' } });
  expect(root.querySelector<HTMLTextAreaElement>('[data-config-path="direct.imprint.text"]')!.value).toContain('Operator: Test Institute. Contact Alex, study@example.org.');
  expect(root.querySelector<HTMLTextAreaElement>('[data-config-path="direct.privacy_notice.text"]')!.value).toContain('Contact Alex at Test Institute, study@example.org');
  fireEvent.change(root.querySelector('[data-config-path="direct.imprint.country"]')!, { target: { value: 'DE' } });
  fireEvent.change(root.querySelector('[data-config-path="direct.privacy_notice.jurisdiction"]')!, { target: { value: 'EU' } });
  expect(root.querySelector<HTMLSelectElement>('[data-config-path="direct.imprint.country"]')!.value).toBe('DE');
  fireEvent.submit(root.querySelector('form')!);
  await waitFor(() => expect(root.querySelector('[data-library-status]')!.textContent).toContain('Template saved'));
  expect(records[0].documents.imprint.country).toBe('DE');
  expect(root.querySelector<HTMLSelectElement>('[data-config-path="direct.imprint.country"]')!.value).toBe('DE');
  expect(records[0].documents.consents[0].body).toContain('Alex studies games.');
  expect(records[0].documents.consents[0].editor_data.values.name).toBe('Alex');
  expect(records[0].documents.imprint.text).toContain('Operator: Test Institute. Contact Alex, study@example.org.');
  expect(records[0].documents.notice_fields.imprint.values.name).toBe('Alex');
  expect(Array.from(root.querySelectorAll('[data-notice-preview="imprint"] mark'), mark => mark.textContent)).toContain('Alex');
  expect(records[0].documents.imprint.text).not.toContain('<mark');
  expect(records[0].documents.privacy_notice.text).toContain('Contact Alex at Test Institute, study@example.org');
  fireEvent.input(root.querySelector('[data-consent-fact="name"]')!, { target: { value: 'Sam' } });
  expect(root.querySelector<HTMLTextAreaElement>('[data-config-path="direct.imprint.text"]')!.value).toContain('Contact Sam,');
  expect(root.querySelector<HTMLTextAreaElement>('[data-config-path="direct.privacy_notice.text"]')!.value).toContain('Contact Sam at');
  fireEvent.input(root.querySelector('[data-library-name]')!, { target: { value: 'Unfinished edits' } });
  fireEvent.click(root.querySelector('[data-library-new]')!);
  fireEvent.change(root.querySelector('[data-library-select]')!, { target: { value: '1' } });
  expect(root.querySelector<HTMLInputElement>('[data-library-name]')!.value).toBe('Unfinished edits');
  expect(request.mock.calls.every(([path]) => path === '/templates')).toBe(true);
});

/** Selecting a shared template edits only a local document snapshot; published config stays intact. */
it('applies documents to an experiment draft without changing saved configuration', async () => {
  const source = readFileSync(resolve(base, 'app/admin_dashboard.js'), 'utf8');
  const start = source.indexOf('async function initializeDocumentTemplateSelection()');
  const end = source.indexOf('// Renders one curated typed Parlando setting.', start);
  document.body.innerHTML = '<div id="form"><div data-document-template-selection><select data-document-template-choice><option value="">Choose</option></select><button data-apply-document-template disabled></button><button data-copy-document-template></button><span data-document-template-status></span></div></div>';
  const state = { configValue: { direct: { consents: [{ body: 'Published' }], enabled: true }, session: { limit: 10 } }, documentTemplateDraft: {} };
  const documents = { consents: [{ body: 'Shared draft' }], imprint: { text: 'Imprint' }, privacy_notice: { text: 'Privacy' } };
  const fetch = vi.fn(async () => ({ ok: true, json: async () => ({ templates: [{ id: 1, name: 'Shared', documents: { ...documents, notice_fields: { private: true } } }] }) }));
  const initialize = new Function('configForm', 'state', 'fetch', 'escapeHtml', 'previewConsentFromEditor', 'refreshDraftActivationWarning', source.slice(start, end) + '\nreturn initializeDocumentTemplateSelection;')(document.querySelector('#form'), state, fetch, (s: string) => s, vi.fn(), vi.fn());
  await initialize();
  fireEvent.change(document.querySelector('select')!, { target: { value: '1' } });
  fireEvent.click(document.querySelector('[data-apply-document-template]')!);
  expect(state.documentTemplateDraft).toEqual(documents);
  expect(state.configValue.direct.consents[0].body).toBe('Published');
  expect(state.configValue.direct.enabled).toBe(true);
  expect(state.configValue.session.limit).toBe(10);
  expect(fetch).toHaveBeenCalledOnce();
});
