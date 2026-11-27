/**
 * Popover: a tiny anchored menu used by the notes editor.
 *
 * Owns open/closed state, closing on outside click or Escape, and keyboard
 * focus return to the trigger. Children are rendered in an absolutely
 * positioned panel under the trigger. It holds no game knowledge.
 */
import { useEffect, useRef, useState, type ReactNode } from "react";

/** A trigger button with a floating panel; `children` receives a `close` function. */
export function Popover({ label, className, title, disabled, children }: {
  label: ReactNode; className?: string; title?: string; disabled?: boolean;
  children: (close: () => void) => ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLSpanElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!open) return;
    const away = (e: PointerEvent) => { if (!root.current?.contains(e.target as Node)) setOpen(false); };
    const key = (e: KeyboardEvent) => { if (e.key === "Escape") { setOpen(false); trigger.current?.focus(); } };
    document.addEventListener("pointerdown", away);
    document.addEventListener("keydown", key);
    return () => { document.removeEventListener("pointerdown", away); document.removeEventListener("keydown", key); };
  }, [open]);
  return (
    <span className="nf-pop" ref={root}>
      <button ref={trigger} type="button" className={className} title={title} disabled={disabled}
        aria-haspopup="true" aria-expanded={open} onClick={() => setOpen(!open)}>{label}</button>
      {open && <div className="nf-pop-panel" role="menu">{children(() => setOpen(false))}</div>}
    </span>
  );
}
