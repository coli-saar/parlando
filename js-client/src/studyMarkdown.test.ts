// @vitest-environment happy-dom
import { expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Evaluate the same dependency-free module that the server serves to public document pages.
const source = readFileSync(resolve(dirname(fileURLToPath(import.meta.url)), '../../rust-server/src/app/document_markdown.mjs'), 'utf8');
const { enhanceDocumentMarkdown } = new Function(source.replace(/^export /gm, '') + '\nreturn { enhanceDocumentMarkdown };')();

/** Public enhancement changes presentation, safely preserving source as text before rendering. */
it('renders marked public documents while leaving unrelated content untouched', () => {
  document.body.innerHTML = '<pre data-study-markdown></pre><pre id="other">**untouched**</pre>';
  const original = '# Privacy notice\n\n**Contact**\n- Email us\n\n<script>alert(1)</script>\n[Unsafe](javascript:alert)';
  document.querySelector('[data-study-markdown]')!.textContent = original;
  enhanceDocumentMarkdown();
  expect(document.querySelector('.document-markdown h1')?.textContent).toBe('Privacy notice');
  expect(document.querySelector('.document-markdown strong')?.textContent).toBe('Contact');
  expect(document.querySelector('.document-markdown li')?.textContent).toBe('Email us');
  expect(document.querySelector('script')).toBeNull();
  expect(document.querySelector('a[href^="javascript:"]')).toBeNull();
  expect(document.querySelector('#other')?.textContent).toBe('**untouched**');
  expect(document.querySelector('[data-study-markdown]')).toBeNull();
});

/** SDK builds must ship the same renderer as dashboard and public study documents. */
it('keeps the packaged Markdown renderer synchronized with the shared source', () => {
  const packaged = readFileSync(resolve(dirname(fileURLToPath(import.meta.url)), 'documentMarkdown.mjs'), 'utf8');
  expect(packaged.slice(packaged.indexOf('\n') + 1)).toBe(source);
});
