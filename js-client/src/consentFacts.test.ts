// @vitest-environment happy-dom
import { expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const source = readFileSync(resolve(dirname(fileURLToPath(import.meta.url)), '../../rust-server/src/app/admin_dashboard.js'), 'utf8');
const units = source.slice(source.indexOf('const CONSENT_FACTS ='), source.indexOf('// Turns the visual configured-secret'));
const escapeHtml = (value: unknown) => String(value ?? '').replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;').replaceAll('"', '&quot;').replaceAll("'", '&#39;');
const templates = [{ id: 'study', title: 'Study', description: 'Study details', body: '😀 {{name}} / {{name}}; {{institution}} / {{institute}}; {{email}} / {{study contact email}}; {{end date}}; {{license}}; {{data platform}}; {{research question}}; {{payment}}; {{server}}; {{unrelated}}' }];
const api = new Function('escapeHtml', 'icon', 'CONSENT_TEMPLATE_OPTIONS', units + '\nreturn { renderConsentEditor, initializeConsentEditor, readConsentItem };')(escapeHtml, () => '', templates);

/** Mounts the production editor, including its real generate and input handlers. */
function mount(items: unknown[] = []) {
  document.body.innerHTML = api.renderConsentEditor({ path: 'direct.consents' }, items);
  const editor = document.querySelector('[data-consent-editor]')!;
  api.initializeConsentEditor(editor);
  return editor;
}

/** Types into a control and emits the same bubbling input event used by the dashboard. */
function type(editor: Element, selector: string, value: string) {
  const input = editor.querySelector<HTMLInputElement | HTMLTextAreaElement>(selector)!;
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
}

/** All requested controls update aliases and repeated occurrences, and survive reopening. */
it('fills all study facts after generation and preserves their bindings across save/reopen', () => {
  let editor = mount();
  editor.querySelector<HTMLButtonElement>('[data-generate-consent]')!.click();
  expect(editor.querySelectorAll('[data-consent-fact]')).toHaveLength(8);
  expect(editor.querySelector('[data-consent-fact="research_question"]')).toBeNull();
  for (const [key, example] of Object.entries({ data_platform: 'zenodo', license: 'CC-by 4.0', payment: '€14 per hour', server: 'Debian Webserver' })) {
    const input = editor.querySelector<HTMLInputElement>(`[data-consent-fact="${key}"]`)!;
    expect(input.placeholder).toBe(example);
    expect(input.value).toBe('');
  }
  const values = { name: 'Alex', institute: 'Research Institute', email: 'a@example.org', end_date: '31 December 2026', license: 'CC BY 4.0', data_platform: 'Zenodo', payment: '£5', server: 'EU server' };
  for (const [key, value] of Object.entries(values)) type(editor, `[data-consent-fact="${key}"]`, value);
  let item = editor.querySelector('.consent-item')!;
  let saved = api.readConsentItem(item);
  expect(saved.body).toContain('Alex / Alex; Research Institute / Research Institute; a@example.org / a@example.org');
  for (const value of Object.values(values)) expect(saved.body).toContain(value);
  expect(saved.body).toContain('{{unrelated}}');
  expect(saved.body).toContain('{{research question}}');
  editor = mount([JSON.parse(JSON.stringify(saved))]);
  expect(editor.querySelector<HTMLInputElement>('[data-consent-fact="name"]')!.value).toBe('Alex');
  type(editor, '[data-consent-fact="name"]', 'Sam');
  type(editor, '[data-consent-fact="email"]', '');
  item = editor.querySelector('.consent-item')!;
  saved = api.readConsentItem(item);
  expect(saved.body).toContain('Sam / Sam');
  expect(saved.body).toContain('{{email}} / {{study contact email}}');
});

/** Direct prose edits detach just the edited occurrence; values are literal and scoped per form. */
it('preserves manual edits and isolates forms even with identical field values', () => {
  const editor = mount();
  const generate = editor.querySelector<HTMLButtonElement>('[data-generate-consent]')!;
  generate.click();
  generate.click();
  const [first, second] = Array.from(editor.querySelectorAll('.consent-item'));
  type(first, '[data-consent-fact="name"]', 'Alex');
  type(first, '[data-consent-fact="institute"]', 'Alex');
  const textarea = first.querySelector<HTMLTextAreaElement>('[data-consent-field="body"]')!;
  type(first, '[data-consent-field="body"]', textarea.value.replace('Alex / Alex', 'Custom person / Alex'));
  type(first, '[data-consent-field="body"]', textarea.value + '\nUnrelated Alex.');
  type(first, '[data-consent-fact="name"]', '<script>literal</script> {{email}}');
  expect(textarea.value).toContain('Custom person / <script>literal</script> {{email}}');
  expect(textarea.value).toContain('Alex / Alex');
  expect(textarea.value).toContain('Unrelated Alex.');
  expect(second.querySelector<HTMLTextAreaElement>('[data-consent-field="body"]')!.value).toContain('{{name}} / {{name}}');
  expect(document.querySelector('script')).toBeNull();
  const saved = api.readConsentItem(first);
  mount([saved]);
  expect(document.querySelector('script')).toBeNull();
});

/** Unchecking every section still generates an empty form without inserting study details. */
it('keeps an empty generated form empty when study facts are entered', () => {
  const editor = mount();
  editor.querySelector<HTMLInputElement>('[data-consent-template-option]')!.checked = false;
  editor.querySelector<HTMLButtonElement>('[data-generate-consent]')!.click();
  type(editor, '[data-consent-fact="name"]', 'Alex');
  expect(api.readConsentItem(editor.querySelector('.consent-item')).body).toBe('');
});
