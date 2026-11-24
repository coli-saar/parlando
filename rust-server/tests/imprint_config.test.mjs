import assert from 'node:assert/strict';
import test from 'node:test';
import { bindImprintEditor, generateImprint, IMPRINT_COUNTRIES, renderImprintEditor, renderImprintPreview } from '../src/app/imprint_config.mjs';

// Models the browser controls used by the editor without requiring a browser dependency.
function editorFixture(country = 'DE', savedText) {
  const values = { country, operator: 'Research Institute', responsible_person: 'Ada', address: 'Main Street 1', email: 'study@example.org', phone: '+49 123', text: '' };
  values.text = savedText ?? generateImprint(country, values);
  const controls = Object.fromEntries(Object.entries(values).map(([key, value]) => {
    const control = new EventTarget();
    control.value = value;
    return [key, control];
  }));
  return { controls, querySelector(selector) { return controls[selector.match(/direct\.imprint\.(\w+)/)[1]]; } };
}

test('all offered countries generate distinct annotation-study templates with public identity', () => {
  const texts = IMPRINT_COUNTRIES.filter(([code]) => code).map(([country]) => generateImprint(country, { operator: 'Institute', responsible_person: 'Ada', address: 'Main Street 1' }));
  assert.equal(new Set(texts).size, texts.length);
  for (const text of texts) {
    assert.match(text, /Operator: Institute/);
    assert.match(text, /Responsible contact person: Ada/);
    assert.match(text, /Address: Main Street 1/);
  }
  for (const [country, label] of IMPRINT_COUNTRIES.filter(([code]) => code)) {
    assert.ok(generateImprint(country).includes(`Selected template: ${label}`));
    assert.match(generateImprint(country), /Legal references \(where applicable\): \S/);
  }
  assert.match(generateImprint('DE'), /§ 5 Digitale-Dienste-Gesetz \(DDG\)/);
  assert.match(generateImprint('US'), /No general federal website imprint requirement/);
  assert.match(generateImprint('FR'), /Hosting provider:/);
  assert.match(generateImprint('ES'), /NIF/);
  assert.equal(generateImprint(''), '');
  assert.throws(() => generateImprint('ZZ'));
});

test('editor prefills the institution, preserves saved identity, and escapes HTML', () => {
  assert.match(renderImprintEditor({}, 'Shared Institute'), /value="Shared Institute"/);
  const html = renderImprintEditor({ operator: 'Saved Institute', text: '</textarea><script>bad()</script>' }, 'Shared Institute');
  assert.match(html, /value="Saved Institute"/);
  assert.doesNotMatch(html, /Shared Institute|<script>/);
  assert.match(html, /&lt;\/textarea&gt;/);
});

test('country selection and contact changes update untouched text', () => {
  const editor = editorFixture('', '');
  bindImprintEditor(editor);
  editor.controls.country.value = 'FR';
  editor.controls.country.dispatchEvent(new Event('change'));
  assert.match(editor.controls.text.value, /Hosting provider:/);
  editor.controls.address.value = 'New address';
  editor.controls.address.dispatchEvent(new Event('input'));
  assert.match(editor.controls.text.value, /Address: New address/);
});

test('contact edits preserve custom copy but country selection replaces it without prompts', () => {
  const editor = editorFixture('DE', 'Reviewed notice');
  const previews = [];
  bindImprintEditor(editor, text => previews.push(text));
  assert.equal(previews.at(-1), 'Reviewed notice');
  editor.controls.responsible_person.value = 'Grace';
  editor.controls.responsible_person.dispatchEvent(new Event('input'));
  assert.equal(editor.controls.text.value, 'Reviewed notice');
  editor.controls.country.value = 'US';
  editor.controls.country.dispatchEvent(new Event('change'));
  assert.match(previews.at(-1), /state of establishment/);
  assert.match(previews.at(-1), /Responsible contact person: Grace/);
  editor.controls.text.value = 'Final edited notice';
  editor.controls.text.dispatchEvent(new Event('input'));
  assert.equal(previews.at(-1), 'Final edited notice');
  assert.doesNotMatch(renderImprintEditor(), /data-regenerate|data-imprint-replace|data-imprint-cancel/);
});

test('Privacy preview escapes markup, distinguishes saved and draft copy, and clears empty notices', () => {
  const text = 'Institute & team\n<script>alert(1)</script>';
  const draft = renderImprintPreview(text, 'Old notice');
  assert.match(draft, /Draft preview/);
  assert.match(draft, /Institute &amp; team\n&lt;script&gt;/);
  assert.doesNotMatch(draft, /<script>/);
  assert.match(renderImprintPreview(text, text), /Saved imprint/);
  assert.doesNotMatch(renderImprintPreview('', text), /Institute|Saved imprint/);
});

// Links must remain usable without allowing operator text to inject markup or script URLs.
test('source links are clickable and safely escaped, without media-law references', () => {
  assert.doesNotMatch(generateImprint('DE'), /MStV|Medienstaatsvertrag/);
  assert.doesNotMatch(generateImprint('AT'), /Mediengesetz|MedienG|Media owner/);
  for (const [country] of IMPRINT_COUNTRIES.filter(([code]) => code && code !== 'US')) {
    assert.match(renderImprintPreview(generateImprint(country)), /<a href="https:\/\//);
  }
  const html = renderImprintPreview('https://example.org/?a="x"&b=1\njavascript:alert(1)');
  assert.match(html, /href="https:\/\/example.org\/\?a=&quot;x&quot;&amp;b=1"/);
  assert.doesNotMatch(html, /href="javascript:/);
});
