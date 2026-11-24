// Country-specific starting points for public, non-selling annotation studies.
// The saved plain text is authoritative; identity inputs are generation aids only.
export const IMPRINT_COUNTRIES = [
  ['', 'Select country'], ['DE', 'Germany'], ['US', 'United States'],
  ['FR', 'France'], ['AT', 'Austria'], ['ES', 'Spain'], ['IE', 'Ireland'],
  ['IT', 'Italy'], ['NL', 'Netherlands'], ['EU', 'EU — general template'],
];

// Adds only jurisdiction-specific prompts; conductors remove inapplicable lines.
const COUNTRY_DETAILS = {
  DE: ['Legal form: [if applicable]', 'Authorized legal representative: [if applicable]', 'Register and registration number: [if applicable]', 'VAT / business identification number: [if applicable]'],
  US: ['Legal form and state of establishment: [if applicable]'],
  FR: ['Legal form and share capital: [if applicable]', 'Registration and VAT number: [if applicable]', 'Publication director: [name]', 'Hosting provider: [legal name, address, telephone]'],
  AT: ['Business / institutional purpose: [description]', 'Legal form and authorized representative: [if applicable]', 'Register and VAT number: [if applicable]', 'Registered seat: [if different from the address above]'],
  ES: ['Tax identification number (NIF): [number]', 'Register and registration number: [if applicable]', 'Applicable codes of conduct: [name and link, if applicable]'],
  IE: ['Legal form: [if applicable]', 'Place of registration and registration number: [if applicable]', 'Registered office: [if different from the address above]', 'Winding-up status: [if applicable]'],
  IT: ['Legal form: [if applicable]', 'Registered office and business register / number: [if applicable]', 'VAT number: [if applicable; also display on the homepage where required]', 'Paid-up share capital: [if applicable]', 'Sole shareholder / liquidation / management and coordination: [if applicable]'],
  NL: ['Legal form: [if applicable]', 'Business Register (KVK) number: [if applicable]', 'VAT identification number: [if applicable]'],
  EU: ['Legal form and authorized representative: [if applicable]', 'Register and registration number: [if applicable]', 'VAT identification number: [if applicable]', 'Additional national disclosures: [check the operator’s country of establishment]'],
};

// References identify the template's disclosure framework, not a claim that every
// provision applies to every operator. Saved notices are never silently rewritten.
const COUNTRY_LAW_REFERENCES = {
  DE: '§ 5 Digitale-Dienste-Gesetz (DDG).',
  US: 'No general federal website imprint requirement. State and activity-specific disclosure requirements may apply.',
  FR: 'Article 1-1, Loi n° 2004-575 du 21 juin 2004 pour la confiance dans l’économie numérique (LCEN); Article 19 LCEN for electronic commerce.',
  AT: '§ 5 E-Commerce-Gesetz (ECG).',
  ES: 'Artículo 10, Ley 34/2002 de servicios de la sociedad de la información y de comercio electrónico (LSSI-CE).',
  IE: 'Section 151, Companies Act 2014; Regulation 7, European Communities (Directive 2000/31/EC) Regulations 2003 (S.I. No. 68/2003).',
  IT: 'Articolo 2250, Codice civile; Articolo 7, Decreto legislativo 9 aprile 2003, n. 70.',
  NL: 'Artikel 3:15d, Burgerlijk Wetboek (Dutch Civil Code).',
  EU: 'Article 5, Directive 2000/31/EC (E-Commerce Directive), as implemented in the operator’s Member State. Additional national disclosure rules may apply.',
};

// Official legislation or government guidance for the remaining disclosure references.
// Keep URLs on their own lines so both document renderers can link them safely.
const COUNTRY_LAW_SOURCES = {
  DE: ['https://www.gesetze-im-internet.de/ddg/__5.html'],
  US: [],
  FR: ['https://entreprendre.service-public.gouv.fr/vosdroits/F37351', 'https://www.legifrance.gouv.fr/codes/article_lc/LEGIARTI000053151918'],
  AT: ['https://www.ris.bka.gv.at/eli/bgbl/i/2001/152/P5/NOR40025801'],
  ES: ['https://www.boe.es/buscar/act.php?id=BOE-A-2002-13758#a10'],
  IE: ['https://www.irishstatutebook.ie/eli/2014/act/38/section/151/enacted/en/html', 'https://www.irishstatutebook.ie/eli/2003/si/68/made/en/print'],
  IT: ['https://www.to.camcom.it/sites/default/files/Ecommerce_fisco-garelli.pdf', 'https://www.mn.camcom.gov.it/files/CommercioEstero/ecommerce-Imodulo21maggio2021.pdf'],
  NL: ['https://www.acm.nl/nl/verkoop-aan-consumenten/consumenten-informeren/bedrijfsgegevens-vermelden'],
  EU: ['https://eur-lex.europa.eu/eli/dir/2000/31/oj/eng'],
};

// Builds editable text from explicit public identity fields, never administrator credentials.
export function generateImprint(country, identity = {}) {
  if (!country) return '';
  if (!COUNTRY_DETAILS[country]) throw new Error('Unsupported imprint country');
  const value = (key, fallback) => identity[key]?.trim() || fallback;
  return [
    'Legal notice / Imprint',
    `Selected template: ${IMPRINT_COUNTRIES.find(([code]) => code === country)[1]}`,
    `Legal references (where applicable): ${COUNTRY_LAW_REFERENCES[country]}`, '',
    `Operator: ${value('operator', '[full legal name of the operating institution or individual]')}`,
    `Address: ${value('address', '[full postal address, including country]')}`,
    `Responsible contact person: ${value('responsible_person', '[name]')}`,
    `Email: ${value('email', '[public contact email]')}`,
    `Telephone: ${value('phone', '[public telephone number]')}`,
    '', ...COUNTRY_DETAILS[country],
    ...(COUNTRY_LAW_SOURCES[country].length ? ['', 'Sources:', ...COUNTRY_LAW_SOURCES[country]] : []),
  ].join('\n');
}

// Escapes values for attribute and textarea contexts before inserting editor markup.
function escape(value) {
  return String(value ?? '').replace(/[&<>"']/g, ch => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[ch]));
}

// Only standalone HTTPS lines become links; all text and attributes remain escaped.
function linkedImprintText(text) {
  return text.split('\n').map(line => /^https:\/\/[^\s]+$/.test(line)
    ? `<a href="${escape(line)}" rel="noopener noreferrer">${escape(line)}</a>`
    : escape(line)).join('\n');
}

// Renders saved identity values, prefilling the operator from shared institution settings.
export function renderImprintEditor(imprint = {}, institution = '') {
  const fields = [['operator', 'Operating institution / legal operator'], ['responsible_person', 'Responsible person'], ['address', 'Public postal address'], ['email', 'Public contact email'], ['phone', 'Public telephone']];
  return `<div class="config-fields" data-imprint-editor>
    <label class="config-field"><span>Country of the operator</span><select data-config-path="direct.imprint.country" data-config-type="string">${IMPRINT_COUNTRIES.map(([code, label]) => `<option value="${code}" ${imprint.country === code ? 'selected' : ''}>${label}</option>`).join('')}</select></label>
    ${fields.map(([key, label]) => `<label class="config-field"><span>${label}</span><input data-config-path="direct.imprint.${key}" data-config-type="string" value="${escape(imprint[key] || (key === 'operator' ? institution : ''))}"></label>`).join('')}
    <label class="config-field"><span>Imprint text</span><textarea data-config-path="direct.imprint.text" data-config-type="string" rows="16" maxlength="20000">${escape(imprint.text)}</textarea></label>
    <p class="config-help">Choose the operator’s country to generate a starting point for a public annotation study. Changing the country replaces the template text. Complete placeholders and remove inapplicable lines before saving. Saving publishes this text on the study’s Imprint page. The responsible contact is not automatically the legal representative.</p>
    </div>`;
}

// Builds the Privacy-tab document from the editor's exact plain text, without executing HTML.
// Draft labeling distinguishes the immediate preview from the saved public page/download.
export function renderImprintPreview(text = '', savedText = '') {
  if (!text.trim()) return '<p class="muted">Choose a country and complete the imprint under Configuration → Participant access and consent → Imprint.</p>';
  const status = text === savedText ? 'Saved imprint' : 'Draft preview — save a new revision to update the public page and download.';
  return `<p class="muted small">${status}</p><article class="privacy-document"><h1>Imprint</h1><pre style="white-space:pre-wrap;overflow-wrap:anywhere;font:inherit">${linkedImprintText(text)}</pre></article>`;
}

// Country selection replaces the template directly. Contact edits update untouched copy;
// manual prose remains authoritative until another country is selected.
export function bindImprintEditor(editor, onChange = () => {}) {
  const control = key => editor.querySelector(`[data-config-path="direct.imprint.${key}"]`);
  const country = control('country');
  const text = control('text');
  const identity = () => Object.fromEntries(['operator', 'responsible_person', 'address', 'email', 'phone'].map(key => [key, control(key).value]));
  let generated = generateImprint(country.value, identity());
  country.addEventListener('change', () => {
    text.value = generated = generateImprint(country.value, identity());
    onChange(text.value);
  });
  text.addEventListener('input', () => onChange(text.value));
  for (const key of ['operator', 'responsible_person', 'address', 'email', 'phone']) {
    control(key).addEventListener('input', () => {
      const next = generateImprint(country.value, identity());
      if (!text.value || text.value === generated) text.value = next;
      generated = next;
      onChange(text.value);
    });
  }
  onChange(text.value);
}
