/** Locate the definition in the checkout; full paths disambiguate similarly named modules. */
function address(name) {
  const item = sources[name];
  return (
    '<span class="address">' +
    escapeText(item.path) +
    " · L" +
    item.first +
    '</span><code class="identifier">' +
    escapeText(name) +
    "</code>"
  );
}

/** Render one code-backed role in a task-oriented reading map. */
function partTile(key) {
  const p = parts[key];
  return link(
    "guide/" + key,
    "<strong>" +
      modules[key][0] +
      "</strong><span>" +
      p.job +
      "</span>" +
      address(p.symbols[0]),
    "partTile"
  );
}

/** Explain the job of one subsystem and provide a reading order through its implementation. */
function partGuide(key) {
  const p = parts[key];
  return (
    link("guide/components", "Subsystem index", "testsLink") +
    '<section class="card margin"><p>' +
    p.why +
    '</p><dl class="exchange"><dt>Receives</dt><dd>' +
    p.input +
    "</dd><dt>Produces</dt><dd>" +
    p.output +
    '</dd></dl><h3>Read the implementation</h3><div class="readList">' +
    p.symbols
      .map(
        (n) =>
          "<div>" +
          symbol(n) +
          '<span class="address">' +
          sources[n].path +
          " · L" +
          sources[n].first +
          "</span>" +
          "<p>" +
          readingNotes[n] +
          "</p></div>"
      )
      .join("") +
    '</div><div class="links">' +
    link("coupling/" + key, "Inspect dependencies") +
    link("checks/" + key, "Inspect related checks") +
    "</div></section>"
  );
}

/** Trace a legal Cue Choice session and show the wrong-role check at its starting state. */
function cueExample(stage) {
  const scenes = {
    setup: ["Before the deal", "Waiting for B to deal", "Wait for the dealer", "You may deal",
      "The server already knows which word and answer apply. Neither player has seen the word yet.",
      "observation", "CueChoiceObservation::ReadyToDeal"],
    dealt: ["B deals", "Waiting for A to choose", "The word is “dax”", "Wait for the chooser",
      "Dealing reveals the word to A. A can now choose oak, pine, birch, or elm. The server sends B a waiting status with no cue or answer.",
      "observation", "CueChoiceObservation::DealerWaiting"],
    completed: ["A chooses oak", "Game complete", "Oak was correct", "Oak was correct",
      "The game compares A’s choice with the answer for this word. Both roles now receive the choice and whether it was correct.",
      "complete_session_preserves_private_cue_and_role_rules", "choice: Choice::Oak"],
    rejected: ["If A tries to deal", "Waiting for B to deal", "Wait for the dealer", "You may deal",
      "Suppose A tries to deal before B has acted. The rules return wrong_role. The game stays at its starting state and B can still deal.",
      "step", 'Rejection::new("wrong_role")'],
  };
  const key = known(scenes, stage, "setup");
  const scene = scenes[key];
  return (
    '<section class="card"><h2>One session of Cue Choice</h2><p>Cue Choice is a small game in the repository. One player is the dealer (role B). The other is the chooser (role A). The dealer reveals a word to the chooser, who tries to select the tree that goes with it. The game defines the pairs. In this example the word is “dax” and its correct answer is oak.</p>' +
    '<p>Follow the two moves below. The first column describes the state on the server. The other columns describe each player’s <em>observation</em>, the information the game lets that role see.</p><div class="rail">' +
    ["setup", "dealt", "completed"].map((id) =>
      link("guide/example/" + id, scenes[id][0], id === key ? "on" : "")
    ).join("") +
    '</div><dl class="snapshots">' +
    ["Server state", "Chooser (A) sees", "Dealer (B) sees"].map(
      (label, i) => "<div><dt>" + label + "</dt><dd>" + scene[i + 1] + "</dd></div>"
    ).join("") +
    "</dl><p>" + scene[4] + "</p>" + symbol(scene[5], scene[6]) +
    "<p>" + link("guide/example/rejected", "What if the chooser tries to deal?") +
    " · " + link("guide/observation", "Which code reads an observation?") +
    '</p><details class="margin"><summary>Run the game’s rule test</summary><pre class="command">cargo test --locked --manifest-path experiments/cue-choice-rl/server/Cargo.toml complete_session_preserves_private_cue_and_role_rules</pre></details></section>'
  );
}

/** Invert the conceptual map so a developer can start at a file in their checkout. */
function byFile() {
  const files = new Map();
  for (const [key, part] of Object.entries(parts)) {
    for (const name of part.symbols) {
      const path = sources[name].path;
      if (!files.has(path)) files.set(path, new Map());
      const responsibilities = files.get(path);
      if (!responsibilities.has(key)) responsibilities.set(key, []);
      responsibilities.get(key).push(name);
    }
  }
  return (
    "<section><h2>Where player and agent actions meet</h2><p>" +
    symbol("websocket_loop") + " owns the player’s game connection. " +
    symbol("handle_agent_response") + " interprets a software player’s decision. Both send moves to " +
    symbol("submit_action") + ", which holds the session lock while it commits the move and delivers each observation. The two input modules can therefore change independently while sharing the same ordering rule.</p></section>" +
    '<div class="links">' +
    link("guide", "Read the session example") +
    "</div>" +
    Array.from(files.keys())
      .sort()
      .map((path) => {
        const entries = Array.from(files.get(path));
        return (
          '<section class="card margin"><h2 class="fileTitle">' +
          path +
          "</h2>" +
          entries
            .map(
              ([key, names]) =>
                '<div class="fileRole"><h3>' +
                link("guide/" + key, modules[key][0], "testsLink") +
                "</h3><p>" +
                parts[key].job +
                '.</p><div class="links">' +
                names.map((n) => symbol(n)).join("") +
                "</div></div>"
            )
            .join("") +
          "</section>"
        );
      })
      .join("")
  );
}

/** Teach the shared session and private observation model using a concrete game. */
function guide(id, stage) {
  if (Object.hasOwn(parts, id)) return partGuide(id);
  if (id === "observation") return observationGuide();
  if (id === "files") return byFile();
  if (id === "components") {
    return readingGroups
      .map(
        ([title, description, keys]) =>
          '<section class="directoryGroup"><h2>' +
          title +
          "</h2><p>" + description + '</p><div class="partGrid">' +
          keys.map(partTile).join("") +
          "</div></section>"
      )
      .join("");
  }
  return (
    cueExample(stage) +
    "<section><h2>How the server accepts a move</h2><p>The rules above belong to the game. Its implementation of the Rust " +
    symbol("Game") +
    " trait supplies two functions used here. <code>step</code> takes the proposed move, and the acting role. It returns a new state or a reason to reject the move. Given a state and a role, <code>observation</code> computes what that role can see.</p><p>Parlando keeps the current state in a " +
    symbol("LiveSession") +
    ". Two requests can arrive together, so " + symbol("submit_action") +
    " takes a lock for this session before evaluating a move. It asks SQLite to record the accepted move before replacing the state in memory or sending observations. If the write fails, neither player receives the proposed change.</p><p>" +
    link("flows/action", "Trace a move from the browser to SQLite and back") +
    "</p></section>" +
    "<section><h2>Where the code lives</h2><table><thead><tr><th>Location</th><th>Responsibility</th></tr></thead><tbody>" +
    "<tr><td><code>rust-server/src/app/</code> and <code>app.rs</code></td><td>Session state, connections, and coordination between game rules and storage. " +
    link("guide/rooms", "Session lifecycle") +
    "</td></tr>" +
    "<tr><td><code>experiments/cue-choice-rl/server/src/game.rs</code></td><td>The word–tree pairs, the rules for dealing and choosing, and the observations in the example above. " + symbol("step") + "</td></tr>" +
    "<tr><td><code>rust-server/src/game.rs</code></td><td>The shared game interface. Each game provides its own implementation. " +
    link("guide/game", "Game rules") +
    "</td></tr>" +
    "<tr><td><code>rust-server/src/storage/events.rs</code></td><td>SQLite writes for session events and participant results. " +
    link("guide/storage", "Storage") +
    "</td></tr>" +
    "<tr><td><code>js-client/src/</code></td><td>The participant’s connection, including input and recovery after disconnection. " +
    link("guide/browser", "Browser client") +
    "</td></tr></tbody></table>" +
    "<p>" +
    link("guide/components", "Subsystem index") +
    " · " +
    link("guide/files", "File index") +
    " · " +
    link("coupling/matrix", "Dependency matrix") +
    "</p></section>"
  );
}

/** Follow a game observation through the three readers most sensitive to a format change. */
function observationGuide() {
  return (
    '<section class="card"><p>In the example game Cue Choice, a dealer reveals a word and a chooser selects its matching tree. Once the dealer acts, the game returns the word to the chooser (A) and a waiting status to the dealer (B). The two branches below run against the same stored game state.</p>' +
    symbol("observation", "CueChoiceObservation::DealerWaiting") +
    code("observation", "CueChoiceObservation::DealerWaiting") +
    '<ol class="guided"><li><strong>Live play.</strong> prepare_action asks for an observation per role. The runtime serializes browser observations and targets each player’s transition packet. ' +
    symbol("prepare_action", "let observation = session.game.observation") +
    "</li><li><strong>Software players.</strong> An agent is a program that plays one role. The same loop puts that role’s observation in AgentObservation::Action. If the agent runs in another process, the adapter encodes the observation for the program that chooses its next move. " +
    symbol("prepare_action", "resulting_observation: observation") +
    "</li><li><strong>Replay and training.</strong> Replay calls the game rules again on recorded moves. Changing the observation function can therefore change what a replay shows for an old session. In a training run, the batch runner saves the acting agent’s next observation alongside its action. " +
    symbol("record_trajectory", "self.game.observation") +
    '</li></ol><div class="links">' +
    link("flows/replay/1", "Follow replay") +
    link("flows/batch/2", "Follow training capture") +
    "</div></section>" +
    fixtureCard(
      "simultaneous_actions_publish_role_private_snapshots_in_replay_order"
    )
  );
}

/** Give the exact test selector and highlight the assertion that justifies the claim. */
function fixtureCard(name) {
  const item = checks[name],
    anchor = assertions[name],
    captured = runEvidence.passed.includes(name);
  const command =
    name === "real_browsers_exercise_the_participant_state_machine"
      ? "make test-browser-e2e"
      : "cargo test --locked --manifest-path rust-server/Cargo.toml --lib " +
        name;
  return (
    '<article class="fixture"><div class="sectionHead"><h3>' +
    item[0] +
    '</h3><span class="badge ' +
    (captured ? "" : "neutral") +
    '">' +
    (captured ? "Passed · " + runEvidence.date : "Source inspected") +
    "</span></div><p>" +
    item[1] +
    "</p>" +
    symbol(name, anchor || "") +
    (anchor ? code(name, anchor) : "") +
    '<details><summary>Run from the repository root</summary><pre class="command">' +
    command +
    "</pre></details></article>"
  );
}

/** Present the lock and failure consequences alongside the selected action step. */
function actionBoundary(i) {
  const notes = [
    "The callback can outlive its original connection. The client rechecks synchronization and active state before sending.",
    "The session transition lock is acquired here and held through publication. A due deadline ends the operation before game evaluation.",
    "Game rejection exits here. The candidate state and role-specific outputs are prepared while the live session still holds the previous state.",
    "The agent’s inbox has a fixed capacity. If the server cannot reserve space, it ends the session with a technical failure before recording the move.",
    "The server holds the session lock until SQLite replies. If another operation has already ended the session, the store returns AlreadyEnded. In that case, and on a database error, submit_action returns before updating state or sending a packet.",
    "The store has committed. The coordinator now takes the memory write lock and replaces the authoritative game state.",
    "The server still holds the lock when it sends the notifications. A browser can disconnect before receiving its packet. The accepted move remains in SQLite.",
  ];
  const related = [[0], [0], [0], [2], [1, 3, 0, 2], [1, 0], [0, 2]][i];
  return (
    '<section class="zone"><strong>Failure conditions</strong><p>' +
    notes[i] +
    "</p></section><h3>Related tests</h3>" +
    related
      .map(
        (j) =>
          '<details class="evidenceFold"><summary>' +
          checks[actionFixtures[j]][0] +
          "</summary>" +
          fixtureCard(actionFixtures[j]) +
          "</details>"
      )
      .join("")
  );
}

/** Evaluate one illustrative paused-session snapshot in the server guard’s priority order. */
function incident(id, time) {
  const key = known(incidentPresets, id, "idle"),
    [, idle, grace, lifetime] = incidentPresets[key];
  const t = Math.max(
    0,
    Math.min(35, Number.isFinite(Number(time)) ? Number(time) : 8)
  );
  const result =
    t >= lifetime
      ? "maximum_lifetime"
      : t >= grace
      ? "reconnect_timeout"
      : t >= idle
      ? "idle_timeout"
      : "resume";
  const resultText = {
    resume:
      "If B reconnects now, both players are connected and play can resume.",
    idle_timeout:
      "The server saw no qualifying activity before the idle deadline. Both players receive IdleLimitReached, even though B still had time to reclaim the connection.",
    reconnect_timeout:
      "B’s reconnect deadline has passed. B receives ConnectionLost and A receives PartnerLeft.",
    maximum_lifetime:
      "The session’s absolute lifetime is due. Both roles receive LifetimeLimitReached.",
  };
  const clocks = [
    ["Idle", idle],
    ["Reconnect grace", grace],
    ["Lifetime", lifetime],
  ];
  return (
    '<section class="card"><h2>Idle expiry during a disconnect</h2><p>Consider two human players in a running session. B disconnects at time zero. A stays connected. While the session is paused, its idle and lifetime deadlines continue to apply. Either can end the session before B’s grace period expires. The green markers are deadlines. The amber marker shows when the server checks them.</p><div class="rail">' +
    Object.entries(incidentPresets)
      .map(([k, v]) =>
        link("incident/" + k + "/" + t, v[0], k === key ? "on" : "")
      )
      .join("") +
    '</div><p class="dim">Times are seconds since B disconnected. The example assumes that the first check occurs at the selected time. A running server also checks deadlines during cleanup.</p><div class="clockRows">' +
    clocks
      .map(
        ([name, v]) =>
          "<div><span>" +
          name +
          '</span><div class="track"><i style="left:' +
          (v / 35) * 100 +
          '%"></i><b style="left:' +
          (t / 35) * 100 +
          '%"></b></div><code>+' +
          v +
          " s</code></div>"
      )
      .join("") +
    '</div><label class="timeLabel">Deadline check at <strong>+' +
    t +
    ' s</strong><input type="range" min="0" max="35" value="' +
    t +
    '" data-clock="' +
    key +
    '"></label><div class="zone" aria-live="polite"><strong>' +
    result +
    "</strong><p>" +
    resultText[result] +
    "</p></div>" +
    symbol("due_live_deadline") + " gathers the reconnect deadlines for disconnected participants. " +
    symbol("due_deadline") + " selects the expiry cause." +
    '<p class="dim">For a running session the server checks lifetime first, reconnect grace second, and idle time last. If several deadlines have passed by the time it checks, the first applicable condition determines the recorded cause.</p></section>' +
    '<section class="card"><h2>Server and client behavior</h2><ol class="guided"><li><strong>Socket generation.</strong> The server assigns an ID to each new connection for a role. On disconnect it compares that ID with the current connection’s ID. A replaced socket cannot mark its replacement absent. ' +
    symbol("unregister", "control.generation == generation") +
    "</li><li><strong>Session lock.</strong> On B’s new connection the socket loop checks expiry under the transition lock before registering the new generation. If a deadline has already passed, this check can end the session as B reconnects. " +
    symbol("websocket_loop", "expire_if_due_locked") +
    "</li><li><strong>Durable ending.</strong> Inspect the stored session_ended event and its per-role results. The terminal write must succeed before the live lifecycle becomes Ended. " +
    symbol("commit_end_locked", ".end_session(") +
    "</li><li><strong>What the returning browser sees.</strong> The new connection can update presence. The participant still receives Ended because the session has already ended. Once the server removes the ended session from memory, it reads the result from SQLite. " +
    symbol("existing_assignment", ".terminal_participant_state(") +
    '</li></ol></section><section class="card"><h2>Reconnect tests</h2><p>The first test reconnects a real WebSocket after idle expiry while grace is still open. The others cover a successful reconnect and expiry of the grace period.</p>' +
    fixtureCard(
      "reconnect_within_grace_after_idle_deadline_returns_ended_room"
    ) +
    fixtureCard(
      "brief_disconnect_pauses_partner_and_reconnect_preserves_session"
    ) +
    fixtureCard("disconnect_past_grace_gives_partner_a_terminal_outcome") +
    "</section>"
  );
}
