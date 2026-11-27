// @vitest-environment happy-dom
import { expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { safeMarkdown } from './documentMarkdown.mjs';

const source = readFileSync(resolve(dirname(fileURLToPath(import.meta.url)), '../../rust-server/src/app/consent_config.mjs'), 'utf8');
const { createConsentEditor } = new Function(source.replace(/^export /gm, '') + '\nreturn { createConsentEditor };')();
const escapeHtml = (value: unknown) => String(value ?? '').replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;').replaceAll('"', '&quot;');
const templates = [
  { id: 'study', title: 'Study', body: '😀 {{name}} / {{name}}; {{institution}} / {{institute}}; {{email}} / {{study contact email}}; {{end date}}; {{license}}; {{data platform}}; {{research question}}; {{payment}}; {{server}}.' },
  { id: 'informed_consent_personal_information_collected_v1', title: 'Personal information collected', body: '### Personal data\n\nVoice and IDs. Contact {{email}}.' },
  { id: 'informed_consent_non_personal_information_collected_v1', title: 'Other research data', body: '### Research data\n\nActions and scores.' },
  { id: 'rights', title: 'Rights', body: 'Contact {{email}} before {{end date}}.' },
];
const api = createConsentEditor({ templates, escapeHtml, safeMarkdown });

/** Mounts the real editor; default consent is present immediately without generation. */
function mount(items: unknown[] = []) {
  document.body.innerHTML = api.renderConsentEditor({ path: 'direct.consents' }, items);
  const editor = document.querySelector('[data-consent-editor]')!;
  api.initializeConsentEditor(editor);
  return editor;
}

/** Dispatches normal editing input to the production event handlers. */
function type(editor: Element, selector: string, value: string) {
  const input = editor.querySelector<HTMLInputElement | HTMLTextAreaElement>(selector)!;
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
}

/** The default includes all sections but exposes only eight facts and two collection editors. */
it('automatically shows a complete form with fixed prose and editable data collection', () => {
  const editor = mount();
  expect(editor.querySelectorAll('.consent-item')).toHaveLength(1);
  expect(editor.querySelectorAll('[data-consent-fact]')).toHaveLength(8);
  expect(editor.querySelectorAll('textarea:not([readonly])')).toHaveLength(2);
  expect(editor.querySelector('[data-generate-consent]')).toBeNull();
  expect(editor.querySelector('[data-consent-template-option]')).toBeNull();
  expect(editor.querySelector('[data-remove-consent]')).toBeNull();
  const saved = api.readConsentItem(editor.querySelector('.consent-item'));
  expect(saved.required).toBe(true);
  expect(editor.querySelector('mark.template-field-highlight')!.textContent).toBe('{{name}}');
  expect(saved.body).not.toContain('<mark');
  for (const template of templates) expect(saved.body).toContain(template.body);
});

/** Shared aliases and repeated facts remain linked through save/reopen and clearing. */
it('updates repeated facts and preserves bindings on reopening', () => {
  let editor = mount();
  for (const [key, example] of Object.entries({ data_platform: 'zenodo', license: 'CC-by 4.0', payment: '€14 per hour', server: 'Debian Webserver' })) {
    expect(editor.querySelector<HTMLInputElement>(`[data-consent-fact="${key}"]`)!.placeholder).toBe(example);
  }
  type(editor, '[data-consent-fact="name"]', 'Alex');
  type(editor, '[data-consent-fact="institute"]', 'Institute');
  type(editor, '[data-consent-fact="email"]', 'a@example.org');
  const saved = api.readConsentItem(editor.querySelector('.consent-item'));
  expect(saved.body).toContain('Alex / Alex; Institute / Institute; a@example.org / a@example.org');
  expect(saved.body).toContain('{{research question}}');
  editor = mount([JSON.parse(JSON.stringify(saved))]);
  type(editor, '[data-consent-fact="name"]', 'Sam');
  type(editor, '[data-consent-fact="email"]', '');
  const updated = api.readConsentItem(editor.querySelector('.consent-item'));
  expect(updated.body).toContain('Sam / Sam');
  expect(Array.from(editor.querySelectorAll('mark'), mark => mark.textContent)).toContain('Sam');
  expect(updated.body).not.toContain('template-field-highlight');
  expect(updated.body).toContain('{{email}} / {{study contact email}}');
});

/** Editing collection prose preserves fixed sections and never turns input text into HTML. */
it('edits only the collection excerpt while preserving fixed prose and field updates', () => {
  const editor = mount();
  type(editor, '[data-consent-collection="informed_consent_personal_information_collected_v1"]', 'New collection description. Contact {{email}}. <script>unsafe</script>');
  type(editor, '[data-consent-fact="email"]', 'new@example.org');
  const saved = api.readConsentItem(editor.querySelector('.consent-item'));
  expect(saved.body).toContain('New collection description. Contact new@example.org.');
  expect(saved.body).toContain('Rights\nContact new@example.org before {{end date}}.');
  expect(saved.body).toContain('Actions and scores.');
  expect(editor.querySelector('script')).toBeNull();
});
