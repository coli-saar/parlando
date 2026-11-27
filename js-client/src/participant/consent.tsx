import { useId, useState } from 'react';
import { safeMarkdown } from '../documentMarkdown.mjs';

/** A configured form; acceptance stays one server decision for its exact saved text. */
interface ConsentFormProps {
  consent: { id: string; title: string; body: string; required: boolean };
  onChange(accepted: boolean): void;
}

/** One Markdown passage, optionally containing a participant declaration to acknowledge. */
interface ConsentPassage { text: string; declaration: boolean }

/** Finds declaration paragraphs outside fenced code, preserving the complete document order. */
function consentPassages(body: string): ConsentPassage[] {
  const passages: ConsentPassage[] = [];
  let lines: string[] = [];
  let fenced = false;
  let declaration = false;
  /** Commits the current passage without interpreting or changing its prose. */
  const flush = () => {
    if (lines.length) passages.push({ text: lines.join('\n'), declaration });
    lines = [];
    declaration = false;
  };
  for (const line of body.replace(/\r\n?/g, '\n').split('\n')) {
    if (/^\s*```/.test(line)) fenced = !fenced;
    const startsDeclaration = !fenced && /^(?:I have read and adhere\b|I confirm that I am\b|I agree to participate\b)/.test(line.trim());
    if (startsDeclaration) { flush(); declaration = true; }
    if (declaration && !line.trim()) { flush(); }
    lines.push(line);
  }
  flush();
  return passages;
}

/** Renders safe Markdown with local acknowledgements beside the final declarations.
 * All present declarations must be checked to submit acceptance for the parent form.
 * Forms without recognized declarations receive one acceptance control after the text.
 */
export function ParticipantConsentForm({ consent, onChange }: ConsentFormProps) {
  const id = useId();
  const [checks, setChecks] = useState<Record<number, boolean>>({});
  const passages = consentPassages(consent.body);
  const declarations = passages.flatMap((passage, index) => passage.declaration ? [index] : []);
  const controls = declarations.length ? declarations : [passages.length];
  /** Propagates only aggregate acceptance, so partial acknowledgement cannot admit entry. */
  const update = (index: number, checked: boolean) => {
    const next = { ...checks, [index]: checked };
    setChecks(next);
    onChange(controls.every(control => Boolean(next[control])));
  };
  return <section className="consent-form" style={{ overflowWrap: 'anywhere', minWidth: 0 }} aria-labelledby={`${id}-title`}>
    <h3 id={`${id}-title`}>{consent.title}{consent.required && ' (required)'}</h3>
    {passages.map((passage, index) => passage.declaration
      ? <div className="consent-declaration" key={index} style={{ display: 'flex', alignItems: 'baseline', gap: '.5rem' }}>
          <input type="checkbox" checked={Boolean(checks[index])} aria-labelledby={`${id}-${index}`} onChange={event => update(index, event.target.checked)} />
          <div id={`${id}-${index}`} className="document-markdown" dangerouslySetInnerHTML={{ __html: safeMarkdown(passage.text) }} />
        </div>
      : <div key={index} className="document-markdown" dangerouslySetInnerHTML={{ __html: safeMarkdown(passage.text) }} />)}
    {!declarations.length && <label className="consent-row">
      <input type="checkbox" checked={Boolean(checks[passages.length])} onChange={event => update(passages.length, event.target.checked)} />
      <span>I have read and agree to {consent.title}{consent.required && ' (required)'}</span>
    </label>}
  </section>;
}
