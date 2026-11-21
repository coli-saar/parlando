"use strict";
// The navigation labels name topics. The introductions state the governing behavior.
const views = {
  guide: [
    "Overview",
    "Games, experiments, and sessions",
    "Parlando runs two-player dialogue experiments. A game defines what each player can do and see. An experiment selects a game and its settings for a study. A session is one play of that game, with a human or software agent in each role.",
  ],
  coupling: [
    "Dependencies",
    "Dependencies between subsystems",
    "Each box groups code by responsibility and names a source definition where you can start reading. Read from left to right. The boxes on the left call or send data to the selected subsystem. Those on the right receive its calls or messages. Select a connection to see what crosses it and where that happens in the code.",
  ],
  flows: [
    "Execution paths",
    "Execution paths",
    "These traces follow one operation through its callers, its effects on session state, and its outputs.",
  ],
  life: [
    "Session lifecycle",
    "Session lifecycle",
    "Follow a participant from registration to the final result. The server creates a session when it assigns a player to a match. The session can be Forming, Running, or Ended. The status sent to a player also depends on who is connected. If a partner disconnects during play, the session remains Running and the player sees Paused.",
  ],
  owners: [
    "State ownership",
    "State ownership",
    "To trace a change, first identify who owns the value. Two sessions in the same experiment share the list used for matchmaking. Each session has its own game state and lock. A browser keeps the last view it received for one participant. The table identifies the code that controls writes at each scope.",
  ],
  incident: [
    "Reconnect deadlines",
    "Reconnect deadlines",
    "The reconnect grace period limits how long a participant may reclaim a connection. It does not extend the session’s idle deadline or its maximum lifetime.",
  ],
  checks: [
    "Tests",
    "Regression tests",
    "Choose a subsystem to find a tested behavior, the test code, and its command. A dated pass records an earlier local run. “Source inspected” means the reference includes the test code but has no recorded pass for it.",
  ],
};
const main = document.querySelector("main"),
  dialog = document.querySelector("dialog");
let lastFocus = null;
/** Escape repository content before interpolating it into HTML. */
function escapeText(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}
/** Return a curated key, including when users open a stale or malformed fragment. */
function known(dict, key, fallback) {
  return Object.hasOwn(dict, key) ? key : fallback;
}
/** Build the reusable symbol inspector trigger. */
function symbol(name, anchor = "") {
  return (
    '<button class="code" data-symbol="' +
    escapeText(name) +
    '" data-anchor="' +
    escapeText(anchor) +
    '">' +
    escapeText(name) +
    "</button>"
  );
}
/** Build a route with a normal link for browser navigation and keyboard access. */
function link(route, label, style = "") {
  return '<a class="' + style + '" href="#' + route + '">' + label + "</a>";
}
/** Resolve a witness line within a complete definition. */
function offset(name, anchor) {
  return anchor
    ? sources[name].lines.findIndex((line) => line.includes(anchor))
    : 0;
}
/** Render an excerpt or full definition with absolute source line numbers. */
function code(name, anchor = "", full = false) {
  const s = sources[name];
  const pos = Math.max(0, offset(name, anchor));
  const start = full ? 0 : Math.max(0, pos - 2),
    end = full ? s.lines.length : Math.min(s.lines.length, start + 9);
  const lines = s.lines
    .slice(start, end)
    .map(
      (line, i) =>
        "<span " +
        (anchor && start + i === pos ? 'class="hit"' : "") +
        '><span class="ln">' +
        (s.first + start + i) +
        "  </span>" +
        escapeText(line) +
        "</span>"
    )
    .join("\n");
  return (
    '<div class="source"><div class="sourceHead">' +
    s.path +
    " · L" +
    (s.first + start) +
    "–" +
    (s.first + end - 1) +
    '</div><pre tabindex="0">' +
    lines +
    "</pre></div>"
  );
}
/** Generate readable subsystem options for the two filter controls. */
function options(values, selected) {
  return Object.entries(values)
    .map(
      ([id, record]) =>
        '<option value="' +
        id +
        '" ' +
        (id === selected ? "selected" : "") +
        ">" +
        record[0] +
        "</option>"
    )
    .join("");
}
/** Draw a directed neighborhood; links select the contract and expose its witness. */
function lane(items, key, side, selected) {
  return (
    '<div class="lane ' +
    side +
    '"><div class="boardLabel">' +
    (side === "left" ? "Incoming" : "Outgoing") +
    '</div><div class="stack" style="--n:' +
    Math.max(1, items.length) +
    '">' +
    (items
      .map((edge) => {
        const other = edge[side === "left" ? 0 : 1];
        return link(
          "coupling/" + key + "/" + edges.indexOf(edge),
          modules[other][0] +
            "<small>" +
            edge[2] +
            "</small>" +
            address(parts[other].symbols[0]),
          edge === selected ? "chosen" : ""
        );
      })
      .join("") || '<span class="empty">None mapped</span>') +
    "</div></div>"
  );
}
/** Compare the curated outgoing and incoming contracts across all subsystems. */
function matrix() {
  const keys = Object.keys(modules);
  const headers = keys
    .map((k) => '<th scope="col"><span>' + modules[k][0] + "</span></th>")
    .join("");
  const rows = keys
    .map((from) => {
      const cells = keys
        .map((to) => {
          const index = edges.findIndex((e) => e[0] === from && e[1] === to);
          if (index < 0) return "<td></td>";
          const label =
            modules[from][0] + " → " + modules[to][0] + ": " + edges[index][3];
          return (
            '<td><a href="#coupling/' +
            from +
            "/" +
            index +
            '" title="' +
            escapeText(label) +
            '" aria-label="' +
            escapeText(label) +
            '">●</a></td>'
          );
        })
        .join("");
      return (
        '<tr><th scope="row">' +
        link("coupling/" + from, modules[from][0]) +
        "</th>" +
        cells +
        "</tr>"
      );
    })
    .join("");
  return (
    '<div class="sectionHead"><h2>Dependency matrix</h2>' +
    link("coupling/transitions", "Focused view", "chip") +
    '</div><p class="margin">The row names the caller or sender. The column names the component it uses. Select a dot to inspect the payload and source line. Blank cells have no contract indexed here.</p><div class="tableWrap margin" tabindex="0"><table class="matrix"><thead><tr><th>Caller / sender</th>' +
    headers +
    "</tr></thead><tbody>" +
    rows +
    "</tbody></table></div>"
  );
}
/** Inspect the selected subsystem and one of its evidenced runtime relationships. */
function coupling(id, edgeId) {
  if (id === "matrix") return matrix();
  const key = known(modules, id, "transitions"),
    m = modules[key];
  const incoming = edges.filter((e) => e[1] === key),
    outgoing = edges.filter((e) => e[0] === key);
  const picked = edges[edgeId];
  const e = [...incoming, ...outgoing].includes(picked)
    ? picked
    : outgoing[0] || incoming[0];
  const peer = e[0] === key ? e[1] : e[0];
  return (
    '<section class="board"><div class="boardTop"><span class="boardLabel">Subsystem</span><select aria-label="Inspect subsystem" data-select="coupling">' +
    options(modules, key) +
    '</select></div><div class="boardBody">' +
    lane(incoming, key, "left", e) +
    '<div class="hub"><small>' +
    m[1] +
    "</small><strong>" +
    m[0] +
    "</strong>" +
    address(parts[key].symbols[0]) +
    "</div>" +
    lane(outgoing, key, "right", e) +
    '</div><div class="boardBottom"><a class="matrixLink" href="#coupling/matrix">Dependency matrix</a></div></section><div class="split"><section class="card"><span class="mini">Selected dependency</span><h2>' +
    modules[e[0]][0] +
    " → " +
    modules[e[1]][0] +
    "</h2><p>" +
    e[3] +
    ".</p>" +
    symbol(e[4], e[5]) +
    code(e[4], e[5]) +
    '<div class="links">' +
    link(contractRoutes[e[0] + "/" + e[1]], "Related execution path") +
    link("guide/" + peer, modules[peer][0] + " implementation") +
    "</div>" +
    '</section><section class="card"><h2>' +
    m[0] +
    "</h2><p>" +
    m[2] +
    "</p><h3>Consequences for callers</h3><p>" +
    m[4] +
    '</p><div class="links">' +
    link("guide/" + key, "Implementation") +
    symbol(m[3]) +
    link("checks/" + key, "Related tests") +
    "</div></section></div>"
  );
}
/** Render a reusable navigation strip for data flow selection. */
function rail(dict, view, key) {
  return (
    '<div class="rail">' +
    Object.entries(dict)
      .map(([id, d]) => link(view + "/" + id, d[0], id === key ? "on" : ""))
      .join("") +
    "</div>"
  );
}
/** Render a data path with a selectable step, a source witness and a relevant fixture. */
function flow(id, step) {
  const key = known(flows, id, "action"),
    [name, summary, steps, test] = flows[key];
  const i = Math.max(0, Math.min(steps.length - 1, parseInt(step, 10) || 0)),
    s = steps[i];
  return (
    rail(flows, "flows", key) +
    '<div class="sectionHead"><div><h2>' +
    name +
    "</h2><p>" +
    summary +
    '</p></div></div><div class="twoCol"><div class="list">' +
    steps
      .map((v, j) =>
        link(
          "flows/" + key + "/" + j,
          '<span class="num">' +
            String(j + 1).padStart(2, "0") +
            "</span>" +
            v[1],
          j === i ? "on" : ""
        )
      )
      .join("") +
    '</div><section class="card"><span class="mini">' +
    s[0] +
    "</span><h2>" +
    s[1] +
    "</h2><p>" +
    s[2] +
    "</p>" +
    symbol(s[3], s[4]) +
    code(s[3], s[4]) +
    (key === "action"
      ? actionBoundary(i)
      : test
      ? "<h3>Test for this path</h3>" + symbol(test)
      : '<p class="scope">Inspect the conversation commit and projection together when changing speech semantics.</p>') +
    '<div class="chainNav">' +
    (i
      ? link("flows/" + key + "/" + (i - 1), "Previous step")
      : "<span></span>") +
    (i + 1 < steps.length
      ? link("flows/" + key + "/" + (i + 1), "Next step")
      : "") +
    "</div></section></div>"
  );
}
/** Offer only outgoing model events and retain the specific transition as route provenance. */
function lifecycle(id, edgeId) {
  const key = known(states, id, "registered"),
    s = states[key],
    e = transitions[edgeId],
    arrived = e?.[1] === key ? e : null;
  let history = arrived
    ? '<div class="zone"><strong>' +
      states[arrived[0]][0] +
      " → " +
      s[0] +
      "</strong><br>" +
      arrived[5] +
      "</div>" +
      symbol(arrived[3], arrived[4]) +
      code(arrived[3], arrived[4])
    : "<p>Select an event to see when the transition is permitted and which function applies it.</p>";
  return (
    '<div class="board"><div class="boardTop"><span class="boardLabel">Stages of participation</span>' +
    link("life/registered", "Reset path", "boardLabel") +
    '</div><div class="lifeTop"><div class="flowState">' +
    Object.entries(states)
      .map(([k, v]) =>
        link("life/" + k, v[0], key === k ? "currentPhase" : "phase")
      )
      .join("") +
    "</div><p>Session lifecycle <b>" +
    s[1] +
    "</b> · Participant view <b>" +
    s[2] +
    '</b></p></div></div><div class="split"><section class="card"><h2>' +
    s[0] +
    "</h2><p>" +
    s[3] +
    "</p>" +
    symbol(s[4], s[5]) +
    '<h3 class="margin">Transitions</h3><div class="links margin">' +
    (transitions
      .map((v, i) =>
        v[0] === key
          ? link("life/" + v[1] + "/" + i, v[2] + " → " + states[v[1]][0])
          : ""
      )
      .join("") ||
      "<p>Session resources have been released. Its persisted result remains readable.</p>") +
    '</div></section><section class="card"><h2>' +
    (arrived ? arrived[2] : "Transition details") +
    "</h2>" +
    history +
    '</section></div><details class="card"><summary>Experiment admission and browser retries</summary><p>An experiment accepts new participants in its testing and active states. Once a player has a session, the server enforces deadlines for that session. The browser separately limits how often it retries a failed connection. A browser can stop retrying while the server still holds the player’s place.</p><div class="links">' +
    ["can_transition_to", "allows_intake", "due_deadline", "scheduleRetry"]
      .map((n) => symbol(n))
      .join("") +
    "</div></details>" +
    link("incident", "Reconnect deadlines", "chip")
  );
}
/** Compare mutable owners, lock scope and durability across the installation. */
function ownership() {
  return (
    '<div class="tableWrap"><table><thead><tr><th>Lifetime</th><th>State and owner</th><th>What orders changes</th></tr></thead><tbody>' +
    owners
      .map(
        ([scope, title, fields, src, rule, guard]) =>
          "<tr><td><strong>" +
          scope +
          "</strong></td><td><h3>" +
          escapeText(title) +
          "</h3><p>" +
          fields +
          "</p>" +
          symbol(src) +
          "</td><td><p>" +
          rule +
          "</p>" +
          symbol(guard) +
          "</td></tr>"
      )
      .join("") +
    '</tbody></table></div><section class="card margin"><h2>Across a process restart</h2><p>SQLite preserves the experiment history. A newly built runtime finalizes interrupted sessions with a technical-failure result. Replay reconstructs game states from the initial state and the recorded actions. Connections, agent tasks, and timers belong to the original server process.</p><div class="links">' +
    symbol("finalize_interrupted_sessions") +
    link("flows/replay", "Inspect reconstruction") +
    "</div></section>"
  );
}
/** Display dated checks and keep separately invoked browser coverage identifiable. */
function verification(id) {
  const key = known(modules, id, "all");
  const subset = Object.entries(checks).filter(
    ([n, v]) =>
      key === "all" || v[2] === key || sharedFixtures[n]?.includes(key)
  );
  return (
    '<section class="card"><div class="sectionHead"><h2>Tests by subsystem</h2><select aria-label="Filter tests by subsystem" data-select="checks">' +
    options({ all: ["All subsystems"], ...modules }, key) +
    "</select></div>" +
    (subset.map(([name]) => fixtureCard(name)).join("") ||
      "<p>This reference lists no test for this subsystem. " +
        link("checks/all", "Browse indexed checks", "testsLink") +
        "</p>") +
    '</section><details class="card"><summary>Library suite run · ' +
    runEvidence.date +
    "</summary><p>" +
    runEvidence.total +
    " Rust library tests passed locally. Browser lifecycle tests use a separate harness.</p><code>cargo test --locked --manifest-path rust-server/Cargo.toml --lib</code></details>"
  );
}
/** Rebuild the requested workspace from a shareable fragment. */
function render() {
  const [v, id, detail] = location.hash.slice(1).split("/"),
    view = known(views, v, "guide");
  document.querySelector("#nav").innerHTML = Object.entries(views)
    .map(([key, [name]]) => link(key, name, key === view ? "on" : ""))
    .join("");
  let [, title, desc] = views[view];
  if (view === "guide" && Object.hasOwn(parts, id)) {
    title = modules[id][0];
    desc = modules[id][2];
  } else if (view === "guide" && id === "components") {
    title = "Subsystem index";
    desc =
      "These groups describe responsibilities in the current implementation. Each entry identifies the file and definition where that responsibility is easiest to trace.";
  } else if (view === "guide" && id === "files") {
    title = "File index";
    desc =
      "Each repository path lists the responsibilities and source definitions indexed in that file.";
  } else if (view === "guide" && id === "observation") {
    title = "Consumers of game observations";
    desc =
      "An observation is the part of the game state that one role may see. The Wasm guest computes it for each role. The browser, software players, replay, and training code depend on that choice of information.";
  }
  main.innerHTML =
    "<h1>" +
    title +
    '</h1><p class="intro">' +
    desc +
    "</p>" +
    {
      guide,
      incident,
      coupling,
      flows: flow,
      life: lifecycle,
      owners: ownership,
      checks: verification,
    }[view](id, detail);
  document.title = title + " · Parlando internals";
}
/** Open the modal and retain the originating control for keyboard navigation. */
function showDialog(title, body) {
  if (!dialog.open) lastFocus = document.activeElement;
  document.querySelector("#dialogTitle").textContent = title;
  document.querySelector("#dialogBody").innerHTML = body;
  if (!dialog.open) dialog.showModal();
  dialog.scrollTop = 0;
}
/** Show the full, line-numbered symbol, centered on its selected call site. */
function inspect(name, anchor) {
  if (!Object.hasOwn(sources, name)) return;
  showDialog(name, code(name, anchor, true));
  dialog.querySelector(".hit")?.scrollIntoView({ block: "center" });
}
/** Filter by symbol and repository path without refetching any source. */
function results(q) {
  const list = Object.entries(sources).filter(([n, s]) =>
    (n + " " + s.path).toLowerCase().includes(q.trim().toLowerCase())
  );
  document.querySelector("#results").innerHTML =
    list
      .map(
        ([n, s]) =>
          '<button data-symbol="' +
          n +
          '"><strong>' +
          n +
          "</strong><small>" +
          s.path +
          " · L" +
          s.first +
          "</small></button>"
      )
      .join("") ||
    "<p>No match in the atlas selection. Search the checkout for additional definitions.</p>";
}
/** Open the searchable implementation and fixture index. */
function findSymbol() {
  showDialog(
    "Find a symbol in the atlas",
    '<p class="dim">Search the ' +
      Object.keys(sources).length +
      ' definitions included in this guide by symbol or repository path.</p><input id="query" class="filter" type="search" aria-label="Search symbols and files" placeholder="Search a symbol or file path"><div id="results"></div>'
  );
  results("");
  document.querySelector("#query").focus();
}
/** Delegate source inspection to a single listener across route changes. */
function handleClick(event) {
  const target = event.target.closest("[data-symbol]");
  if (target) inspect(target.dataset.symbol, target.dataset.anchor || "");
}
document.addEventListener("click", handleClick);
document.addEventListener("change", (event) => {
  const route = event.target.dataset.select;
  if (route) location.hash = route + "/" + event.target.value;
});
document.addEventListener("input", (event) => {
  if (event.target.id === "query") results(event.target.value);
});
document.addEventListener("keydown", (event) => {
  if (event.key === "/" && !dialog.open && !event.metaKey && !event.ctrlKey) {
    event.preventDefault();
    findSymbol();
  }
});
document.querySelector("#find").addEventListener("click", findSymbol);
document
  .querySelector("#close")
  .addEventListener("click", () => dialog.close());
dialog.addEventListener("close", () =>
  lastFocus?.focus({ preventScroll: true })
);
window.addEventListener("hashchange", render);
render();

/** Keep a changed incident time shareable and return keyboard focus to its slider. */
document.addEventListener("change", (event) => {
  if (!event.target.matches("[data-clock]")) return;
  history.replaceState(
    null,
    "",
    "#incident/" + event.target.dataset.clock + "/" + event.target.value
  );
  render();
  document.querySelector("[data-clock]").focus({ preventScroll: true });
});
