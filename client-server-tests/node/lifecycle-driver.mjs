import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { ParticipantClient } from "../../js-client/dist/index.js";
import { decodeServerMessage } from "../../js-client/dist/protocol.js";

const requireFromClient = createRequire(new URL("../../js-client/package.json", import.meta.url));
const WebSocket = requireFromClient("ws");

/** Reads the single JSON fixture supplied by the Rust server owner. */
async function readFixture() {
  const chunks = [];
  for await (const chunk of process.stdin) chunks.push(chunk);
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

/** Waits for one WebSocket event and rejects with a stable timeout diagnostic. */
function waitForEvent(socket, type, timeoutMs = 5_000) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      cleanup();
      reject(new Error(`timed out waiting for WebSocket ${type}`));
    }, timeoutMs);
    const cleanup = () => {
      clearTimeout(timer);
      socket.removeEventListener(type, onEvent);
      socket.removeEventListener("error", onError);
    };
    const onEvent = (event) => {
      cleanup();
      resolve(event);
    };
    const onError = () => {
      cleanup();
      reject(new Error(`WebSocket failed while waiting for ${type}`));
    };
    socket.addEventListener(type, onEvent, { once: true });
    if (type !== "error") socket.addEventListener("error", onError, { once: true });
  });
}

/** Owns the messages received through one production game-channel connection. */
class GamePeer {
  messages = [];

  /** Attaches production decoding before any initial server snapshot can arrive. */
  constructor(socket) {
    this.socket = socket;
    socket.addEventListener("message", (event) => {
      const text = typeof event.data === "string" ? event.data : event.data.toString("utf8");
      this.messages.push(decodeServerMessage(JSON.parse(text)));
    });
  }

  /** Waits for and removes the first message satisfying one assertion predicate. */
  async next(predicate, description, timeoutMs = 8_000) {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      const index = this.messages.findIndex(predicate);
      if (index >= 0) return this.messages.splice(index, 1)[0];
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
    throw new Error(`timed out waiting for ${description}; queued: ${JSON.stringify(this.messages)}`);
  }

  /** Waits for one decoded message of the requested protocol type. */
  nextType(type) {
    return this.next((message) => message.type === type, type);
  }

  /** Waits for one authoritative participant-state message in the requested phase. */
  async nextState(state) {
    const message = await this.next(
      (candidate) => candidate.type === "participant_state" && candidate.participant_state.state === state,
      `${state} participant state`
    );
    return message.participant_state;
  }

  /** Sends one transport heartbeat without representing meaningful game activity. */
  heartbeat() {
    this.socket.send(JSON.stringify({ type: "heartbeat" }));
  }

  /** Closes the transport and waits until the local endpoint observes closure. */
  async close() {
    if (this.socket.readyState === WebSocket.CLOSED) return;
    const closed = waitForEvent(this.socket, "close");
    this.socket.close();
    await closed;
  }
}

/** Opens a game WebSocket using only plans and URL handling supplied by ParticipantClient. */
async function connectPeer(client, sessionId) {
  const plan = await client.getGameSession(sessionId);
  const socket = new WebSocket(client.socketUrl(plan));
  const peer = new GamePeer(socket);
  await waitForEvent(socket, "open");
  socket.send(JSON.stringify({ type: "ready" }));
  return peer;
}

/** Registers one production client and records the fixture's required consent. */
async function admit(client) {
  await client.register();
  await client.acceptConsents({ study: true });
}

/** Creates two independently authenticated JavaScript clients in one active dummy game. */
async function setupPair(origin) {
  const a = new ParticipantClient({ baseUrl: origin });
  const b = new ParticipantClient({ baseUrl: origin });
  await admit(a);
  await admit(b);
  const waiting = await a.join();
  const joined = await b.join();
  assert.equal(waiting.state, "waiting");
  assert.equal(joined.state, "waiting");
  assert.equal(joined.public_session_id, waiting.public_session_id);
  const peerA = await connectPeer(a, waiting.public_session_id);
  const peerB = await connectPeer(b, waiting.public_session_id);
  const activeA = await peerA.nextState("active");
  const activeB = await peerB.nextState("active");
  return { a, b, peerA, peerB, activeA, activeB, sessionId: waiting.public_session_id };
}

/** Polls the production reconciliation endpoint until one lifecycle phase becomes authoritative. */
async function waitForState(client, expected, timeoutMs = 8_000) {
  const deadline = Date.now() + timeoutMs;
  let state = null;
  while (Date.now() < deadline) {
    state = await client.getParticipantState();
    if (state.state === expected) return state;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  throw new Error(`timed out waiting for ${expected}; last state: ${JSON.stringify(state)}`);
}

/** Verifies the welcome client cannot register while server intake is inactive. */
async function inactiveWelcome(origin) {
  const client = new ParticipantClient({ baseUrl: origin });
  const config = await client.getExperiment();
  assert.equal(config.status, "inactive");
  await assert.rejects(() => client.register());
}

/** Verifies configuration mapping, consent enforcement, waiting, and reliable leave. */
async function welcomeWaitingLeave(origin) {
  const client = new ParticipantClient({ baseUrl: origin });
  const config = await client.getExperiment();
  assert.equal(config.status, "active");
  assert.equal(config.consents[0].id, "study");
  assert.equal(config.participantInformationVersion, "contract-v1");
  await client.register();
  await assert.rejects(() => client.join());
  await client.acceptConsents({ study: true });
  const waiting = await client.join();
  assert.equal(waiting.state, "waiting");
  assert.equal(waiting.role, "A");
  assert.equal(typeof waiting.waiting_started_at, "string");
  assert.equal(typeof waiting.waiting_deadline_at, "string");
  const ended = await client.leaveSession(waiting.public_session_id);
  assert.equal(ended.state, "ended");
  assert.equal(ended.result.outcome, "left_waiting_room");
  assert.equal(ended.result.completion, null);
  assert.deepEqual(await client.leaveSession(waiting.public_session_id), ended);
  assert.deepEqual(await client.getParticipantState(), ended);
}

/** Verifies role projections, rejection, chat, accepted transitions, and game completion. */
async function pairedGame(origin) {
  const pair = await setupPair(origin);
  assert.equal(pair.activeA.observation.role, "A");
  assert.equal(pair.activeB.observation.role, "B");
  assert.equal(pair.activeA.available_actions.length, 3);

  pair.a.sendAction(pair.peerA.socket, { type: "reject" });
  const rejection = await pair.peerA.nextType("action_rejected");
  assert.equal(rejection.code, "fixture_rejected");
  assert.equal((await pair.a.getParticipantState()).observation.actions, 0);

  pair.a.sendMessage(pair.peerA.socket, "contract message");
  const message = await pair.peerB.nextType("message");
  assert.equal(message.message.sender, "A");
  assert.equal(message.message.text, "contract message");

  pair.a.sendAction(pair.peerA.socket, { type: "mark", finish: false });
  const transitionA = await pair.peerA.nextType("transition");
  const transitionB = await pair.peerB.nextType("transition");
  assert.equal(transitionA.actor, "A");
  assert.equal(transitionA.observation.role, "A");
  assert.equal(transitionB.observation.role, "B");
  assert.equal(transitionA.observation.actions, 1);

  pair.b.sendAction(pair.peerB.socket, { type: "mark", finish: true });
  const endedA = await pair.peerA.nextState("ended");
  const endedB = await pair.peerB.nextState("ended");
  for (const ended of [endedA, endedB]) {
    assert.equal(ended.result.outcome, "completed");
    assert.equal(ended.result.completion.done, true);
    assert.equal(ended.result.completion.actions, 2);
  }
  assert.deepEqual(await pair.a.getParticipantState(), endedA);
  assert.deepEqual(await pair.b.getParticipantState(), endedB);
}

/** Verifies reliable active leave and its participant-specific terminal consequences. */
async function activeLeave(origin) {
  const pair = await setupPair(origin);
  const caller = await pair.a.leaveSession(pair.sessionId);
  assert.equal(caller.result.outcome, "left_game");
  const partner = await pair.peerB.nextState("ended");
  assert.equal(partner.result.outcome, "partner_left");
  assert.deepEqual(await pair.a.getParticipantState(), caller);
  assert.deepEqual(await pair.b.getParticipantState(), partner);
}

/** Verifies a fresh one-use ticket reconnects before grace and reconciles after expiry. */
async function reconnect(origin) {
  const pair = await setupPair(origin);
  await pair.peerB.close();
  const paused = await pair.peerA.nextState("paused");
  assert.equal(paused.reason.type, "partner_reconnecting");

  const reconnectedB = await connectPeer(pair.b, pair.sessionId);
  const resumedB = await reconnectedB.nextState("active");
  const resumedA = await pair.peerA.nextState("active");
  assert.equal(resumedA.public_session_id, pair.sessionId);
  assert.equal(resumedB.public_session_id, pair.sessionId);

  await reconnectedB.close();
  await pair.peerA.nextState("paused");
  const endedA = await pair.peerA.nextState("ended");
  const endedB = await waitForState(pair.b, "ended");
  assert.equal(endedA.result.outcome, "partner_left");
  assert.equal(endedB.result.outcome, "connection_lost");
}

/** Verifies that a lone JavaScript participant receives the waiting-room deadline outcome. */
async function waitingTimeout(origin) {
  const client = new ParticipantClient({ baseUrl: origin });
  await admit(client);
  assert.equal((await client.join()).state, "waiting");
  const ended = await waitForState(client, "ended");
  assert.equal(ended.result.outcome, "partner_unavailable");
}

/** Verifies heartbeats preserve transport presence but never reset meaningful idle activity. */
async function idleTimeout(origin) {
  const pair = await setupPair(origin);
  for (let index = 0; index < 2; index += 1) {
    pair.peerA.heartbeat();
    await new Promise((resolve) => setTimeout(resolve, 300));
  }
  const endedA = await pair.peerA.nextState("ended");
  const endedB = await pair.peerB.nextState("ended");
  assert.equal(endedA.result.outcome, "idle_limit_reached");
  assert.equal(endedB.result.outcome, "idle_limit_reached");
  assert.equal((await pair.a.getParticipantState()).result.outcome, "idle_limit_reached");
}

/** Verifies meaningful messages reset idle time without extending absolute lifetime. */
async function lifetimeTimeout(origin) {
  const pair = await setupPair(origin);
  for (let index = 0; index < 3; index += 1) {
    pair.a.sendMessage(pair.peerA.socket, `activity ${index}`);
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  const endedA = await pair.peerA.nextState("ended");
  const endedB = await pair.peerB.nextState("ended");
  assert.equal(endedA.result.outcome, "lifetime_limit_reached");
  assert.equal(endedB.result.outcome, "lifetime_limit_reached");
}

const { origin, fixture } = await readFixture();
globalThis.WebSocket = WebSocket;
globalThis.window = { location: { origin, pathname: "/", search: "", hash: "" } };

const scenarios = {
  "inactive-welcome": inactiveWelcome,
  "welcome-waiting-leave": welcomeWaitingLeave,
  "paired-game": pairedGame,
  "active-leave": activeLeave,
  reconnect,
  "waiting-timeout": waitingTimeout,
  "idle-timeout": idleTimeout,
  "lifetime-timeout": lifetimeTimeout
};
const scenario = scenarios[fixture.scenario];
if (!scenario) throw new Error(`unknown lifecycle scenario: ${fixture.scenario}`);
await scenario(origin);
process.stdout.write(`${JSON.stringify({ scenario: fixture.scenario, status: "passed" })}\n`);
process.exit(0);
