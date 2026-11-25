// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from 'vitest';
import { readFileSync, readdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
const serverDirectory = resolve(dirname(fileURLToPath(import.meta.url)), '../../rust-server');

/** Reads the JSON-compatible YAML fixture catalogue; Rust tests cover general Markdown loading. */
function catalogue(folder: string) {
  const directory = resolve(serverDirectory, 'templates', folder);
  return readdirSync(directory).filter(name => name.endsWith('.md')).map(name => {
    const [, header, body] = readFileSync(resolve(directory, name), 'utf8').split(/---\n/, 3);
    return { ...Object.fromEntries(header.trim().split('\n').map(line => {
      const at = line.indexOf(':'); return [line.slice(0, at), JSON.parse(line.slice(at + 1).trim())];
    })), body: body.trim() };
  }).sort((a,b) => a.order - b.order);
}
const consents = catalogue('informed-consent');
const privacy = catalogue('privacy-notice');
const imprints = catalogue('imprint');
/** Executes the actual browser modules with startup catalogue imports supplied explicitly. */
function moduleFunctions(file: string, imports: Record<string, unknown>, exports: string[]) {
  const source = readFileSync(resolve(serverDirectory, 'src/app', file), 'utf8')
    .replace(/^import .*;\n/gm, '').replace(/^export /gm, '');
  return new Function(...Object.keys(imports), `${source}\nreturn {${exports.join(',')}};`)(...Object.values(imports));
}
const shared = moduleFunctions('document_fields.mjs', {CONSENT_TEMPLATES:consents, PRIVACY_TEMPLATES:privacy, IMPRINT_TEMPLATES:imprints}, ['bindDocumentFields', 'documentFieldsFromForm', 'renderLinkedText', 'editLinkedText']);
const imprintModule = moduleFunctions('imprint_config.mjs', {IMPRINT_TEMPLATES:imprints}, ['renderImprintEditor', 'generateImprint']);
const privacyModule = moduleFunctions('privacy_notice_config.mjs', {PRIVACY_TEMPLATES:privacy, CONSENT_TEMPLATES:consents}, ['renderPrivacyNoticeEditor', 'bindPrivacyNoticeEditor']);

/** Builds representative editor DOM using the real imprint/privacy renderers and shipped sections. */
function fixture(saved: any = {}) {
  const form = document.createElement('div');
  form.innerHTML = `<div data-consent-editor></div>${imprintModule.renderImprintEditor(saved.imprint || {country:'DE'})}${privacyModule.renderPrivacyNoticeEditor(saved.privacy_notice || {jurisdiction:'EU'})}`;
  const consentEditor = form.querySelector('[data-consent-editor]')!;
  for (const template of consents) {
    const section = document.createElement('div');
    section.innerHTML = `<input type="checkbox" data-consent-template-option><div data-consent-section-editor><textarea data-consent-section-text></textarea></div>`;
    const checkbox = section.querySelector('input')!;
    checkbox.value = template.id; checkbox.checked = true;
    const textarea = section.querySelector('textarea')!;
    textarea.dataset.consentSectionText = template.id; textarea.value = template.body;
    consentEditor.append(section);
  }
  const button = document.createElement('button');
  button.type = 'button'; button.dataset.generateConsent = ''; consentEditor.append(button);
  /** Mimics the existing generator's rendered snapshot; shared recipes are bound by its bubbling click. */
  const addForm = (id: string, body: string) => {
    const item = document.createElement('div'); item.className = 'consent-item';
    item.innerHTML = '<input data-consent-field="id"><input data-consent-field="title"><textarea data-consent-field="body"></textarea>';
    (item.querySelector('[data-consent-field="id"]') as HTMLInputElement).value = id;
    (item.querySelector('[data-consent-field="title"]') as HTMLInputElement).value = 'Consent Form';
    (item.querySelector('textarea') as HTMLTextAreaElement).value = body;
    consentEditor.append(item);
  };
  for (const item of saved.consents || []) addForm(item.id, item.body);
  button.addEventListener('click', () => addForm('generated', consents.map(t => `${t.title}\n${(form.querySelector(`[data-consent-section-text="${t.id}"]`) as HTMLTextAreaElement).value}`).join('\n\n')));
  document.body.append(form);
  const control = (path: string) => form.querySelector(`[data-config-path="${path}"]`) as HTMLInputElement;
  /** Reads current sources exactly as the dashboard preview callback does. */
  const readSource = () => ({
    imprint: Object.fromEntries(['operator','responsible_person','address','email','phone','text'].map(key => [key, control(`direct.imprint.${key}`).value])),
    consents: Array.from(form.querySelectorAll('.consent-item')).map(item => ({title:'Consent Form', body:(item.querySelector('textarea') as HTMLTextAreaElement).value})),
  });
  const refresh = privacyModule.bindPrivacyNoticeEditor(form, readSource, () => {});
  shared.bindDocumentFields(form, saved.document_fields, refresh);
  /** Emits browser input events so all shared controls and document previews update together. */
  const input = (control: HTMLInputElement | HTMLTextAreaElement, value: string) => {
    control.value = value; control.dispatchEvent(new Event('input', {bubbles:true}));
  };
  /** Captures the revision snapshot and its source recipes to verify reopen behavior. */
  const save = () => ({ ...readSource(),
    consents: Array.from(form.querySelectorAll('.consent-item')).map(item => ({id:(item.querySelector('input') as HTMLInputElement).value, body:(item.querySelector('textarea') as HTMLTextAreaElement).value})),
    document_fields: shared.documentFieldsFromForm(form),
    privacy_notice: {jurisdiction:control('direct.privacy_notice.jurisdiction').value, details:Object.fromEntries(Array.from(form.querySelectorAll('[data-config-path^="direct.privacy_notice.details."]')).map((node:any) => [node.dataset.documentField, node.value]))},
  });
  return {form, control, input, button, save};
}

beforeEach(() => { document.body.innerHTML = ''; });

describe('shared study facts', () => {
  it('prefills all editors in either direction and regenerates the public notice', () => {
    const f = fixture();
    f.input(f.control('direct.imprint.operator'), 'Institute A');
    expect([...f.form.querySelectorAll('[data-document-field="operator"]')].every((node:any) => node.value === 'Institute A')).toBe(true);
    f.button.click();
    expect((f.form.querySelector('.consent-item textarea') as HTMLTextAreaElement).value).toContain('Study operator: Institute A');
    f.input(f.control('direct.privacy_notice.details.legal_basis'), 'Article 6 public task');
    expect((f.form.querySelector('.consent-item textarea') as HTMLTextAreaElement).value).toContain('Article 6 public task');
    const consentContact = f.form.querySelector('[data-consent-editor] [data-document-field="email"]') as HTMLInputElement;
    f.input(consentContact, 'study@example.org');
    expect(f.control('direct.imprint.email').value).toBe('study@example.org');
    // Selecting a country generates text from all current facts without a replace prompt.
    f.control('direct.imprint.country').dispatchEvent(new Event('change', {bubbles:true}));
    expect(f.control('direct.imprint.text').value).toContain('study@example.org');
    expect(f.control('direct.privacy_notice.text').value).toContain('Article 6 public task');
    expect(f.control('direct.privacy_notice.text').value).toContain('Institute A');
  });

  it('preserves custom prose and linked facts through save/reopen, including edits inside generated text', () => {
    const first = fixture(); first.input(first.control('direct.imprint.operator'), 'Institute A'); first.button.click();
    const body = first.form.querySelector('.consent-item textarea') as HTMLTextAreaElement;
    first.input(body, body.value + '\n\nCustom closing text.');
    const reopened = fixture(first.save());
    reopened.input(reopened.control('direct.imprint.operator'), 'Institute B');
    const nextBody = reopened.form.querySelector('.consent-item textarea') as HTMLTextAreaElement;
    expect(nextBody.value).toContain('Study operator: Institute B');
    expect(nextBody.value).toContain('Custom closing text.');
    reopened.input(nextBody, nextBody.value.replace('Institute B', 'Institute C'));
    expect(reopened.control('direct.imprint.operator').value).toBe('Institute C');
    expect(reopened.control('direct.privacy_notice.text').value).toContain('Institute C');
  });

  it('shares repeated storage and voice answers without replacing unrelated matching words', () => {
    const f = fixture();
    const field = f.form.querySelector('[data-privacy-notice-editor] [data-document-field="research_data_storage_location_and_country"]') as HTMLInputElement;
    f.input(field, 'Institute secure server, Germany'); f.button.click();
    const body = (f.form.querySelector('.consent-item textarea') as HTMLTextAreaElement).value;
    expect(body.match(/Institute secure server, Germany/g)?.length).toBe(2);
    const template = 'Literal Institute A; operator {{operator}}.';
    expect(shared.renderLinkedText(template, {operator:'Institute B'}).text).toBe('Literal Institute A; operator Institute B.');
    const edited = shared.editLinkedText(template, {operator:'Institute A'}, 'Custom Literal Institute A; operator Institute A.');
    expect(shared.renderLinkedText(edited.template, {operator:'Institute B'}).text).toBe('Custom Literal Institute A; operator Institute B.');
  });

  it('preserves unlinked existing text and flags differing saved values until explicitly resolved', () => {
    const f = fixture({document_fields:{values:{operator:'Old Institute'}}, imprint:{country:'DE',operator:'Different Institute',text:'Reviewed custom notice.'}, consents:[{id:'old',body:'Previously reviewed consent.'}]});
    expect(f.form.textContent).toContain('Different saved values');
    expect(f.control('direct.imprint.operator').value).toBe('Different Institute');
    f.input(f.control('direct.imprint.operator'), 'Chosen Institute');
    expect(f.form.textContent).not.toContain('Different saved values');
    expect(f.control('direct.imprint.text').value).toBe('Reviewed custom notice.');
    expect((f.form.querySelector('.consent-item textarea') as HTMLTextAreaElement).value).toBe('Previously reviewed consent.');
  });

  it('clears linked facts, removes document recipes, and treats inserted markup literally', () => {
    const f = fixture(); f.input(f.control('direct.imprint.operator'), '<script>bad()</script>'); f.button.click();
    expect(f.form.querySelector('script')).toBeNull();
    expect(f.save().document_fields.templates['consent/generated']).toContain('{{operator}}');
    f.input(f.control('direct.imprint.operator'), '');
    expect((f.form.querySelector('.consent-item textarea') as HTMLTextAreaElement).value).toContain('{{operator}}');
    f.form.querySelector('.consent-item')!.remove();
    expect(f.save().document_fields.templates['consent/generated']).toBeUndefined();
    const changed = shared.editLinkedText('Before {{operator}} after', {}, 'Before  after');
    expect(shared.renderLinkedText(changed.template, {}).text).toBe('Before  after');
  });
});
