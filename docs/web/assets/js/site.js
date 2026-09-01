/** Initializes the manual search dialog and its keyboard shortcut. */
function initializeSearch() {
  const dialog = document.querySelector("#site-search");
  const openButton = document.querySelector(".search-open");
  const input = document.querySelector("#search-input");
  const results = document.querySelector("#search-results");
  const hint = document.querySelector("#search-hint");
  if (!(dialog instanceof HTMLDialogElement) || !(input instanceof HTMLInputElement) || !results || !hint) return;

  let indexPromise;

  /** Loads the generated manual search index once. */
  function loadIndex() {
    indexPromise ??= fetch(dialog.dataset.indexUrl).then((response) => {
      if (!response.ok) throw new Error("Search index unavailable");
      return response.json();
    });
    return indexPromise;
  }

  /** Opens search and places the cursor in the query field. */
  function openSearch() {
    if (!dialog.open) dialog.showModal();
    input.focus();
    loadIndex().catch(() => {
      hint.textContent = "Search is unavailable in this preview.";
    });
  }

  /** Converts text to a case-insensitive search representation. */
  function normalize(value) {
    return value.toLocaleLowerCase().normalize("NFKD");
  }

  /** Returns a short excerpt centered on the first matching term. */
  function excerpt(page, terms) {
    const text = `${page.summary} ${page.content}`.replace(/\s+/g, " ").trim();
    const normalized = normalize(text);
    const positions = terms.map((term) => normalized.indexOf(term)).filter((position) => position >= 0);
    const start = positions.length ? Math.max(0, Math.min(...positions) - 75) : 0;
    const clipped = text.slice(start, start + 190);
    return `${start > 0 ? "…" : ""}${clipped}${start + 190 < text.length ? "…" : ""}`;
  }

  /** Renders ranked title and full-text matches without injecting page HTML. */
  function renderMatches(pages, query) {
    const terms = normalize(query).split(/\s+/).filter(Boolean);
    results.replaceChildren();
    if (terms.join("").length < 2) {
      hint.textContent = "Enter at least two characters.";
      return;
    }

    const matches = pages.map((page) => {
      const title = normalize(page.title);
      const body = normalize(`${page.summary} ${page.content}`);
      if (!terms.every((term) => title.includes(term) || body.includes(term))) return null;
      const titleHits = terms.filter((term) => title.includes(term)).length;
      return { page, score: titleHits * 10 + terms.filter((term) => body.includes(term)).length };
    }).filter(Boolean).sort((left, right) => right.score - left.score).slice(0, 12);

    hint.textContent = matches.length ? `${matches.length} result${matches.length === 1 ? "" : "s"}` : "No matching pages.";
    for (const match of matches) {
      const item = document.createElement("li");
      const link = document.createElement("a");
      const title = document.createElement("strong");
      const summary = document.createElement("span");
      link.href = match.page.url;
      title.textContent = match.page.title;
      summary.textContent = excerpt(match.page, terms);
      link.append(title, summary);
      item.append(link);
      results.append(item);
    }
  }

  openButton?.addEventListener("click", openSearch);
  document.addEventListener("keydown", (event) => {
    const target = event.target;
    const typing = target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || target?.isContentEditable;
    if (event.key === "/" && !typing) {
      event.preventDefault();
      openSearch();
    }
  });
  input.addEventListener("input", () => {
    loadIndex().then((pages) => renderMatches(pages, input.value)).catch(() => {
      hint.textContent = "Search is unavailable in this preview.";
    });
  });
  dialog.addEventListener("click", (event) => {
    if (event.target === dialog) dialog.close();
  });
}

document.addEventListener("DOMContentLoaded", initializeSearch);
