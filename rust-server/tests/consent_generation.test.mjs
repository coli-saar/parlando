import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';

const source = readFileSync(new URL('../src/app/admin_dashboard.js', import.meta.url), 'utf8');

// Evaluates the dashboard's actual template/render/generation units with small DOM stand-ins.
function fixture(selected = [], existing = [], texts = {}) {
  const inserted = [];
  const cards = [];
  let alerts = 0;
  const container = {
    querySelector: () => null,
    querySelectorAll: () => existing.map(() => ({})),
    insertAdjacentHTML: () => inserted.push(...cards.splice(0)),
  };
  const editor = {
    querySelector: selector => selector.startsWith('[data-consent-section-text=') ? { value: texts[selector.match(/"([^"]+)"/)[1]] || '' } : container,
    querySelectorAll: selector => (selector.includes(':checked') ? selected : existing).map(value => ({ value })),
  };
  const context = vm.createContext({
    window: { alert: () => alerts++ },
    escapeHtml: value => String(value ?? ''), icon: () => '',
    consentItemMarkup: item => { cards.push(item); return ''; },
    refreshConsentEditor: () => {},
  });
  const definitions = source.slice(source.indexOf('const CONSENT_TEMPLATE_OPTIONS ='), source.indexOf('const PARLANDO_CONFIG_SECTIONS ='));
  const render = source.slice(source.indexOf('function renderConsentEditor('), source.indexOf('// Renumbers consent headings'));
  const nextId = source.slice(source.indexOf('function nextConsentId('), source.indexOf('// Combines only checked sections'));
  const generate = source.slice(source.indexOf('function generateConsentForm('), source.indexOf('// Adds and removes structured consent items'));
  vm.runInContext(definitions + render + nextId + generate + '; globalThis.options = CONSENT_TEMPLATE_OPTIONS;', context);
  return { context, editor, inserted, alerts: () => alerts };
}

test('every individual option is visible with a description and no dropdown or old buttons', () => {
  const { context } = fixture();
  assert.equal(context.options.length, 18);
  const html = context.renderConsentEditor({ path: 'direct.consents' }, []);
  assert.equal((html.match(/data-consent-template-option/g) || []).length, 18);
  for (const option of context.options) {
    assert.ok(option.description);
    assert.ok(html.includes(option.title));
    assert.ok(html.includes(option.description));
  }
  assert.match(html, /Generate consent form/);
  assert.doesNotMatch(html, /<select|Add template|Blank item/);
});

test('selected sections merge in reading order into one item with one acceptance control', () => {
  const a = 'informed_consent_study_subject_v1';
  const b = 'informed_consent_declaration_v1';
  const f = fixture([b, a], ['consent-2'], { [a]: 'Study-specific text.\nSecond paragraph.', [b]: 'I agree to participate.' });
  f.context.generateConsentForm(f.editor);
  assert.equal(f.inserted.length, 1);
  const form = f.inserted[0];
  assert.equal(form.id, 'consent-3');
  assert.equal(form.title, 'Consent Form');
  assert.equal(form.required, true);
  assert.ok(form.body.startsWith('Subject of this study\n'));
  assert.match(form.body, /\n\nDeclaration of consent\n/);
  assert.doesNotMatch(form.body, /Duration and compensation/);
  assert.equal(form.body, 'Subject of this study\nStudy-specific text.\nSecond paragraph.\n\nDeclaration of consent\nI agree to participate.');
  const markup = source.slice(source.indexOf('function consentItemMarkup('), source.indexOf('// Renders the repeatable consent editor'));
  vm.runInContext(markup, f.context);
  const html = f.context.consentItemMarkup(form, 0);
  assert.equal((html.match(/data-consent-field="required"/g) || []).length, 1);
  assert.equal((html.match(/data-consent-field="id"/g) || []).length, 1);
  assert.equal((html.match(/data-consent-field="title"/g) || []).length, 1);
  assert.equal((html.match(/data-consent-field="body"/g) || []).length, 1);
});

test('no selection generates a blank form with an unused identifier', () => {
  const f = fixture([], ['consent-2']);
  f.context.generateConsentForm(f.editor);
  assert.equal(f.inserted.length, 1);
  assert.equal(f.inserted[0].id, 'consent-3');
  assert.equal(f.inserted[0].title, 'Consent Form');
  assert.equal(f.inserted[0].body, '');
});

test('unchecked section text is excluded and empty selected sections receive no default prose', () => {
  const a = 'informed_consent_study_subject_v1';
  const b = 'research_data_consent_v1_0';
  const f = fixture([b], [], { [a]: 'Do not include this.' });
  f.context.generateConsentForm(f.editor);
  assert.equal(f.inserted[0].body, 'Voluntary participation and research data\n');
});

test('checking a section reveals a prefilled editor and unchecking preserves its draft text', () => {
  const f = fixture();
  const html = f.context.renderConsentEditor({ path: 'direct.consents' }, []);
  assert.equal((html.match(/data-consent-section-editor hidden/g) || []).length, 18);
  assert.equal((html.match(/rows="4"><\/textarea>/g) || []).length, 0);
  for (const option of f.context.options) {
    assert.ok(option.body);
    assert.ok(html.includes(`rows="4">${option.body}</textarea>`));
  }
  const handlers = {};
  const input = { value: '' };
  const panel = { hidden: true };
  const checkbox = { checked: true, matches: () => true, closest: () => ({ querySelector: () => panel }) };
  const editor = { addEventListener: (name, handler) => { handlers[name] = handler; } };
  const binding = source.slice(source.indexOf('function initializeConsentEditor('), source.indexOf('// Turns the visual configured-secret'));
  vm.runInContext(binding, f.context);
  f.context.initializeConsentEditor(editor);
  handlers.change({ target: checkbox });
  assert.equal(panel.hidden, false);
  input.value = 'Keep my text';
  checkbox.checked = false;
  handlers.change({ target: checkbox });
  assert.equal(panel.hidden, true);
  checkbox.checked = true;
  handlers.change({ target: checkbox });
  assert.equal(panel.hidden, false);
  assert.equal(input.value, 'Keep my text');
});

// Bulk selection must reveal every field and preserve existing draft text.
test('Select all checks and reveals every section without changing its text', () => {
  const f = fixture();
  const html = f.context.renderConsentEditor({ path: 'direct.consents' }, []);
  assert.ok(html.indexOf('data-select-all-consent') < html.indexOf('data-consent-template-option'));
  const handlers = {};
  const sections = f.context.options.map(() => ({ hidden: true, text: 'My edited text' }));
  const checkboxes = sections.map(section => ({ checked: false, closest: () => ({ querySelector: () => section }) }));
  const editor = {
    addEventListener: (name, handler) => { handlers[name] = handler; },
    querySelectorAll: () => checkboxes,
  };
  const binding = source.slice(source.indexOf('function initializeConsentEditor('), source.indexOf('// Turns the visual configured-secret'));
  vm.runInContext(binding, f.context);
  f.context.initializeConsentEditor(editor);
  handlers.click({ target: { closest: selector => selector === '[data-select-all-consent]' } });
  assert.ok(checkboxes.every(checkbox => checkbox.checked));
  assert.ok(sections.every(section => !section.hidden && section.text === 'My edited text'));
});
