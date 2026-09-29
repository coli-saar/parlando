import { chromium } from "playwright";

/** Reads the scenario request supplied by the Rust coordinator. */
async function readRequest() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

/** Waits for the reusable client to expose its initial participant entry action. */
async function openParticipant(browser, origin) {
  const context = await browser.newContext({ permissions: ["microphone"] });
  const page = await context.newPage();
  await page.goto(origin, { waitUntil: "domcontentloaded" });
  await page.getByRole("button", { name: "Enter waiting room" }).waitFor();
  const requiredConsent = page.getByRole("checkbox");
  if (await requiredConsent.count()) await requiredConsent.check();
  return { context, page };
}

/** Converts a rendered M:SS countdown into seconds for monotonicity assertions. */
function countdownSeconds(text) {
  const match = text.match(/(\d+):(\d{2}) remaining/);
  if (!match) throw new Error(`could not parse countdown from ${JSON.stringify(text)}`);
  return Number(match[1]) * 60 + Number(match[2]);
}

/** Prepares Playwright's fake microphone and confirms the participant may enter. */
async function prepareVoice(participant) {
  await participant.page.getByRole("button", { name: "Prepare voice" }).click();
  await participant.page.waitForFunction(() => {
    const button = [...document.querySelectorAll("button")]
      .find((candidate) => candidate.textContent?.trim() === "Enter waiting room");
    return button instanceof HTMLButtonElement && !button.disabled;
  }, undefined, { timeout: 10_000 });
}

/** Enters matchmaking through the same visible action used by a participant. */
async function enter(page) {
  await page.getByRole("button", { name: "Enter waiting room" }).click();
}

/** Creates two independent browser sessions and waits until the game becomes active. */
async function activePair(browser, origin) {
  const a = await openParticipant(browser, origin);
  const b = await openParticipant(browser, origin);
  await enter(a.page);
  await a.page.getByRole("heading", { name: "Waiting for another participant" }).waitFor();
  await enter(b.page);
  await a.page.getByRole("heading", { name: "Contract game" }).waitFor();
  await b.page.getByRole("heading", { name: "Contract game" }).waitFor();
  return { a, b };
}

/** Closes every context in a partially or completely constructed participant pair. */
async function closePair(pair) {
  await Promise.allSettled([pair?.a?.context?.close(), pair?.b?.context?.close()].filter(Boolean));
}

/** Exercises registration, waiting, pairing, chat, rejection, transition, and completion. */
async function happyPath(browser, origin) {
  const pair = await activePair(browser, origin);
  try {
    const { a, b } = pair;
    if (await a.page.getByTestId("role").textContent() !== "A") throw new Error("first browser was not role A");
    if (await b.page.getByTestId("role").textContent() !== "B") throw new Error("second browser was not role B");
    await a.page.getByLabel("Message").fill("browser-to-browser message");
    await a.page.getByRole("button", { name: "Send message" }).click();
    await b.page.getByTestId("messages").filter({ hasText: "browser-to-browser message" }).waitFor();
    await a.page.getByRole("button", { name: "Reject action" }).click();
    await a.page.getByText("Action rejected: fixture_rejected").waitFor();
    if (await a.page.getByTestId("actions").textContent() !== "0") throw new Error("rejection changed A's observation");
    await a.page.getByRole("button", { name: "Advance" }).click();
    await a.page.getByTestId("actions").filter({ hasText: /^1$/ }).waitFor();
    await b.page.getByTestId("actions").filter({ hasText: /^1$/ }).waitFor();
    await b.page.getByRole("button", { name: "Finish game" }).click();
    await a.page.getByRole("heading", { name: "Session complete" }).waitFor();
    await b.page.getByRole("heading", { name: "Session complete" }).waitFor();
    await a.page.getByTestId("completion").filter({ hasText: "2 actions" }).waitFor();
    await a.page.reload();
    await a.page.getByRole("heading", { name: "Session complete" }).waitFor();
    return {
      transitions: ["registered->waiting", "waiting->active", "active->ended"],
      outcomes: ["completed"],
      endCauses: ["game_completed"]
    };
  } finally {
    await closePair(pair);
  }
}

/** Exercises a reliable participant leave before a partner is assigned. */
async function waitingLeave(browser, origin) {
  const participant = await openParticipant(browser, origin);
  try {
    await enter(participant.page);
    await participant.page.getByRole("heading", { name: "Waiting for another participant" }).waitFor();
    await participant.page.getByRole("button", { name: "Leave waiting room" }).click();
    await participant.page.getByRole("heading", { name: "Session ended" }).waitFor();
    await participant.page.getByText(/left before a playable game began/i).waitFor();
    return {
      transitions: ["registered->waiting", "waiting->ended"],
      outcomes: ["left_waiting_room"],
      endCauses: ["participant_left"]
    };
  } finally {
    await participant.context.close();
  }
}

/** Exercises the server-owned waiting deadline using the configured short duration. */
async function waitingTimeout(browser, origin) {
  const participant = await openParticipant(browser, origin);
  try {
    await enter(participant.page);
    const countdown = participant.page.locator(".parlando-waiting-countdown");
    await countdown.waitFor();
    const before = countdownSeconds(await countdown.textContent());
    await participant.page.waitForTimeout(1_100);
    const after = countdownSeconds(await countdown.textContent());
    if (after >= before) throw new Error(`waiting countdown did not decrease: ${before} -> ${after}`);
    await participant.page.getByRole("heading", { name: "Session ended" }).waitFor({ timeout: 10_000 });
    await participant.page.getByText(/no partner became available/i).waitFor();
    return {
      transitions: ["registered->waiting", "waiting->ended"],
      outcomes: ["partner_unavailable"],
      endCauses: ["partner_unavailable"]
    };
  } finally {
    await participant.context.close();
  }
}

/** Verifies a reload retains the assigned waiting session and its original deadline. */
async function waitingRefresh(browser, origin) {
  const participant = await openParticipant(browser, origin);
  try {
    await enter(participant.page);
    const countdown = participant.page.locator(".parlando-waiting-countdown");
    await countdown.waitFor();
    const before = countdownSeconds(await countdown.textContent());
    await participant.page.reload();
    await participant.page.getByRole("heading", { name: "Waiting for another participant" }).waitFor();
    const after = countdownSeconds(await countdown.textContent());
    if (after > before) throw new Error(`waiting reload replaced or extended the deadline: ${before} -> ${after}`);
    await participant.page.getByRole("heading", { name: "Session ended" }).waitFor({ timeout: 10_000 });
    return {
      transitions: ["registered->waiting", "waiting->ended"],
      outcomes: ["partner_unavailable"],
      endCauses: ["partner_unavailable"]
    };
  } finally {
    await participant.context.close();
  }
}

/** Exercises successful game-channel recovery without changing the session assignment. */
async function reconnect(browser, origin) {
  const pair = await activePair(browser, origin);
  try {
    await pair.b.context.setOffline(true);
    await pair.a.page.getByText(/partner lost their connection/i).waitFor({ timeout: 10_000 });
    await pair.a.page.getByTestId("interaction").filter({ hasText: "paused" }).waitFor();
    await pair.b.context.setOffline(false);
    await pair.b.page.reload();
    await pair.a.page.getByTestId("interaction").filter({ hasText: "enabled" }).waitFor({ timeout: 10_000 });
    await pair.b.page.getByRole("heading", { name: "Contract game" }).waitFor();
    return {
      transitions: ["registered->waiting", "waiting->active", "active->paused", "paused->active"],
      outcomes: [],
      endCauses: []
    };
  } finally {
    await closePair(pair);
  }
}

/** Verifies a connected participant can reload while the partner remains disconnected. */
async function pausedRefresh(browser, origin) {
  const pair = await activePair(browser, origin);
  try {
    await pair.b.context.setOffline(true);
    await pair.a.page.getByText(/partner lost their connection/i).waitFor({ timeout: 10_000 });
    await pair.a.page.reload();
    await pair.a.page.getByText(/partner lost their connection/i).waitFor({ timeout: 10_000 });
    await pair.a.page.getByTestId("interaction").filter({ hasText: "paused" }).waitFor();
    await pair.b.context.setOffline(false);
    await pair.b.page.reload();
    await pair.a.page.getByTestId("interaction").filter({ hasText: "enabled" }).waitFor({ timeout: 10_000 });
    await pair.b.page.getByRole("heading", { name: "Contract game" }).waitFor();
    return {
      transitions: ["registered->waiting", "waiting->active", "active->paused", "paused->active"],
      outcomes: [],
      endCauses: []
    };
  } finally {
    await closePair(pair);
  }
}

/** Verifies a duplicated tab restores one participant without duplicating game activity. */
async function duplicateTab(browser, origin) {
  const pair = await activePair(browser, origin);
  let duplicate;
  try {
    const opened = pair.a.context.waitForEvent("page");
    await pair.a.page.evaluate(() => window.open(window.location.href, "_blank"));
    duplicate = await opened;
    await duplicate.getByRole("heading", { name: "Contract game" }).waitFor({ timeout: 10_000 });
    await duplicate.getByRole("button", { name: "Advance" }).click();
    await pair.b.page.getByTestId("actions").filter({ hasText: /^1$/ }).waitFor();
    await duplicate.getByTestId("actions").filter({ hasText: /^1$/ }).waitFor();
    await duplicate.waitForTimeout(1_000);
    if (await pair.b.page.getByTestId("actions").textContent() !== "1") {
      throw new Error("one duplicated-tab action was applied more than once");
    }
    return {
      transitions: ["registered->waiting", "waiting->active", "active->paused", "paused->active"],
      outcomes: [],
      endCauses: []
    };
  } finally {
    await duplicate?.close().catch(() => undefined);
    await closePair(pair);
  }
}

/** Exercises the participant-specific results produced by an expired reconnect deadline. */
async function reconnectExpires(browser, origin) {
  const pair = await activePair(browser, origin);
  try {
    await pair.b.context.setOffline(true);
    await pair.a.page.getByText(/partner lost their connection/i).waitFor({ timeout: 10_000 });
    await pair.a.page.getByRole("heading", { name: "Session ended" }).waitFor({ timeout: 10_000 });
    await pair.a.page.getByText(/partner left or could not reconnect/i).waitFor();
    await pair.b.context.setOffline(false);
    await pair.b.page.reload();
    await pair.b.page.getByRole("heading", { name: "Session ended" }).waitFor({ timeout: 10_000 });
    await pair.b.page.getByText(/connection did not return/i).waitFor();
    return {
      transitions: ["registered->waiting", "waiting->active", "active->paused", "paused->ended"],
      outcomes: ["partner_left", "connection_lost"],
      endCauses: ["reconnect_timed_out"]
    };
  } finally {
    await closePair(pair);
  }
}

/** Exercises an active participant leave and its asymmetric partner consequence. */
async function activeLeave(browser, origin) {
  const pair = await activePair(browser, origin);
  try {
    await pair.a.page.getByRole("button", { name: "Leave game" }).click();
    await pair.a.page.getByText(/left after the game started/i).waitFor();
    await pair.b.page.getByText(/partner left or could not reconnect/i).waitFor();
    return {
      transitions: ["registered->waiting", "waiting->active", "active->ended"],
      outcomes: ["left_game", "partner_left"],
      endCauses: ["participant_left"]
    };
  } finally {
    await closePair(pair);
  }
}

/** Exercises the inactivity deadline while browser heartbeats remain active. */
async function idleTimeout(browser, origin) {
  const pair = await activePair(browser, origin);
  try {
    await pair.a.page.getByRole("heading", { name: "Session ended" }).waitFor({ timeout: 10_000 });
    await pair.b.page.getByRole("heading", { name: "Session ended" }).waitFor({ timeout: 10_000 });
    await pair.a.page.getByText(/neither participant produced meaningful activity/i).waitFor();
    return {
      transitions: ["registered->waiting", "waiting->active", "active->ended"],
      outcomes: ["idle_limit_reached"],
      endCauses: ["idle_timed_out"]
    };
  } finally {
    await closePair(pair);
  }
}

/** Exercises the absolute lifetime despite repeated accepted activity. */
async function lifetimeTimeout(browser, origin) {
  const pair = await activePair(browser, origin);
  try {
    const deadline = Date.now() + 8_000;
    while (Date.now() < deadline && await pair.a.page.getByRole("heading", { name: "Contract game" }).isVisible()) {
      const clicked = await pair.a.page
        .getByRole("button", { name: "Advance" })
        .click({ timeout: 500 })
        .then(() => true, () => false);
      if (!clicked) break;
      await pair.a.page.waitForTimeout(200);
    }
    await pair.a.page.getByRole("heading", { name: "Session ended" }).waitFor({ timeout: 10_000 });
    await pair.a.page.getByText(/absolute lifetime limit/i).waitFor();
    return {
      transitions: ["registered->waiting", "waiting->active", "active->ended"],
      outcomes: ["lifetime_limit_reached"],
      endCauses: ["lifetime_timed_out"]
    };
  } finally {
    await closePair(pair);
  }
}

/** Exercises a browser-visible technical failure after asynchronous session setup fails. */
async function technicalFailure(browser, origin) {
  const participant = await openParticipant(browser, origin);
  try {
    await enter(participant.page);
    await participant.page.getByRole("heading", { name: "Session ended" }).waitFor({ timeout: 10_000 });
    await participant.page.getByText(/technical problem prevented the session/i).waitFor();
    return {
      transitions: ["registered->waiting", "waiting->ended"],
      outcomes: ["technical_failure"],
      endCauses: ["technical_failure"]
    };
  } finally {
    await participant.context.close();
  }
}

/** Exercises real browser media, relay, and the configured post-completion voice deadline. */
async function farewellVoice(browser, origin) {
  const a = await openParticipant(browser, origin);
  const b = await openParticipant(browser, origin);
  const pair = { a, b };
  try {
    await prepareVoice(a);
    await prepareVoice(b);
    await enter(a.page);
    await enter(b.page);
    await a.page.getByRole("heading", { name: "Contract game" }).waitFor();
    await b.page.getByRole("heading", { name: "Contract game" }).waitFor();
    await a.page.getByTestId("voice").filter({ hasText: "Microphone live" }).waitFor({ timeout: 15_000 });
    await b.page.getByTestId("voice").filter({ hasText: "Microphone live" }).waitFor({ timeout: 15_000 });
    await Promise.any([
      a.page.getByTestId("remote-audio").filter({ hasText: "heard" }).waitFor({ timeout: 15_000 }),
      b.page.getByTestId("remote-audio").filter({ hasText: "heard" }).waitFor({ timeout: 15_000 })
    ]);
    await b.page.getByRole("button", { name: "Finish game" }).click();
    await a.page.getByText("Voice chat remains open").waitFor();
    const countdown = a.page.locator(".parlando-farewell-voice");
    const before = countdownSeconds(await countdown.textContent());
    await a.page.waitForTimeout(1_100);
    const after = countdownSeconds(await countdown.textContent());
    if (after >= before) throw new Error(`farewell countdown did not decrease: ${before} -> ${after}`);
    await a.page.getByText("Voice chat has closed.").waitFor({ timeout: 10_000 });
    await b.page.getByText("Voice chat has closed.").waitFor({ timeout: 10_000 });
    return {
      transitions: ["registered->waiting", "waiting->active", "active->ended"],
      outcomes: ["completed"],
      endCauses: ["game_completed"]
    };
  } finally {
    await closePair(pair);
  }
}

const scenarios = {
  "happy-path": happyPath,
  "waiting-leave": waitingLeave,
  "waiting-timeout": waitingTimeout,
  "waiting-refresh": waitingRefresh,
  reconnect,
  "paused-refresh": pausedRefresh,
  "duplicate-tab": duplicateTab,
  "reconnect-expires": reconnectExpires,
  "active-leave": activeLeave,
  "idle-timeout": idleTimeout,
  "lifetime-timeout": lifetimeTimeout,
  "technical-failure": technicalFailure,
  "farewell-voice": farewellVoice
};

const request = await readRequest();
const scenario = scenarios[request.fixture.scenario];
if (!scenario) throw new Error(`unknown browser scenario ${request.fixture.scenario}`);

/** Launches only Playwright's isolated headless Chromium, never the user's desktop browser. */
async function launchBrowser() {
  const options = {
    headless: true,
    args: ["--use-fake-device-for-media-stream", "--use-fake-ui-for-media-stream"]
  };
  return chromium.launch(options);
}

const browser = await launchBrowser();
const started = Date.now();
try {
  const coverage = await scenario(browser, request.origin);
  process.stdout.write(`${JSON.stringify({
    scenario: request.fixture.scenario,
    status: "passed",
    elapsedMs: Date.now() - started,
    ...coverage
  })}\n`);
} catch (error) {
  process.stdout.write(`${JSON.stringify({
    scenario: request.fixture.scenario,
    status: "failed",
    elapsedMs: Date.now() - started,
    error: error instanceof Error ? error.stack ?? error.message : String(error),
    transitions: [], outcomes: [], endCauses: []
  })}\n`);
} finally {
  await browser.close();
}
