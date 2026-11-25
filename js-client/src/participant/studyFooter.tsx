/** Keeps public study documents available throughout participation without navigating away from the session. */
export function StudyFooter({ participantInformationUrl, privacyNoticeUrl, imprintUrl }: {
  participantInformationUrl: string;
  privacyNoticeUrl: string;
  imprintUrl: string;
}) {
  return (
    <footer className="parlando-study-footer">
      <style>{`
        .parlando-study-footer{box-sizing:border-box;width:100%;min-width:0;margin-top:1rem;padding:.75rem 1rem;border-top:1px solid var(--parlando-tools-border,#d1d5db);font-size:.875rem;color:inherit}
        .parlando-study-footer nav{display:flex;flex-wrap:wrap;justify-content:center;gap:.5rem 1.5rem}
        .parlando-study-footer a{color:inherit;text-decoration:underline;text-underline-offset:.15em}
        .parlando-study-footer a:focus-visible{outline:2px solid currentColor;outline-offset:4px}
      `}</style>
      <nav aria-label="Study information">
        <a href={participantInformationUrl} target="_blank" rel="noopener noreferrer">Informed consent</a>
        <a href={privacyNoticeUrl} target="_blank" rel="noopener noreferrer">Privacy notice</a>
        <a href={imprintUrl} target="_blank" rel="noopener noreferrer">Imprint</a>
      </nav>
    </footer>
  );
}
