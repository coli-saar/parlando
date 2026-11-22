import { bindSpeechSettings, COMMUNICATION_FIELD_PATHS, COMMUNICATION_MODE_FIELD, configAt, readConfigFields, renderSpeechSettings, renderStandardConfigField, setConfigAt } from "./communication-config.js";

const API = "/api/hosting";
const SETUP_STORAGE_KEY = "parlando.hosting.setup";
const SESSION_LIMIT = 10_000;
const UPLOAD_LIMIT = 64 * 1024 * 1024;
const LIFECYCLE_ACTIONS = {
  inactive: [["testing", "Start testing"], ["active", "Open intake"], ["archived", "Archive"]],
  testing: [["inactive", "Stop testing"]],
  active: [["inactive", "Pause intake"], ["completed", "Complete experiment"]],
  completed: [["archived", "Archive"]],
  archived: [["inactive", "Restore experiment"]],
};

// Owns the current account and selections. A generation prevents requests from an old
// account or experiment from repopulating their views after navigation.
const state = {
  account: null,
  generation: 0,
  games: [],
  experiments: [],
  configDefaults: {},
  experimentId: null,
  sessionId: null,
  sessionRequest: 0,
  sessionLimit: 50,
  resetAccountId: null,
};

const byId = id => document.getElementById(id);
const experimentForm = byId("experiment-form");
const communicationConfig = byId("communication-config");
let setupToken = readSetupToken();

// Reads the current link or a credential retained for a reload in this tab.
function readSetupToken() {
  const fragment = new URLSearchParams(location.hash.slice(1));
  const hasLink = fragment.has("setup");
  let token = hasLink ? fragment.get("setup") : null;
  if (hasLink) history.replaceState(null, "", location.pathname + location.search);
  try {
    if (!hasLink) token = sessionStorage.getItem(SETUP_STORAGE_KEY);
    if (!/^[a-f0-9]{64}$/i.test(token)) {
      sessionStorage.removeItem(SETUP_STORAGE_KEY);
      return null;
    }
    if (hasLink) sessionStorage.setItem(SETUP_STORAGE_KEY, token);
  } catch {
    // Setup still works on this page when the browser blocks storage.
    if (!/^[a-f0-9]{64}$/i.test(token)) return null;
  }
  return token;
}

// Erases the credential used by this view without touching a newer one in storage.
function clearSetupToken() {
  try {
    if (sessionStorage.getItem(SETUP_STORAGE_KEY) === setupToken) {
      sessionStorage.removeItem(SETUP_STORAGE_KEY);
    }
  } catch {
    // A browser that blocks storage can still clear the in-memory credential.
  }
  setupToken = null;
}

// Sends hosting requests to the operator origin and reports server validation messages.
async function request(path, { method = "GET", body, data = "json", contentType = "application/json", bearer } = {}) {
  const generation = state.generation;
  const headers = new Headers();
  if (method !== "GET" && state.account) {
    headers.set("X-CSRF-Token", state.account.csrf_token);
  }
  if (bearer) headers.set("Authorization", `Bearer ${bearer}`);
  if (body !== undefined) headers.set("Content-Type", contentType);
  const response = await fetch(API + path, {
    method,
    headers,
    body: body === undefined || contentType !== "application/json" ? body : JSON.stringify(body),
    credentials: "same-origin",
    cache: "no-store",
  });

  if (state.generation !== generation) throw new DOMException("View changed", "AbortError");
  if (!response.ok) {
    const message = (await response.text()).trim();
    if (state.generation !== generation) throw new DOMException("View changed", "AbortError");
    if (response.status === 401 && state.account) {
      setAccount(null);
      throw new Error("Your session expired. Sign in again.");
    }
    throw Object.assign(new Error(message || `Request failed (HTTP ${response.status}).`), {
      status: response.status,
    });
  }
  if (response.status === 204) return;
  if (data === "response") return response;
  const result = await response.json();
  if (state.generation !== generation) throw new DOMException("View changed", "AbortError");
  return result;
}

// Publishes progress and errors without interpreting server messages as markup.
function notice(message, error = false) {
  const output = byId("notice");
  output.hidden = !message;
  output.textContent = message;
  output.classList.toggle("error", error);
}

// Keeps only the initiating control disabled while an operation is in flight.
async function perform(control, operation) {
  if (control?.disabled) return;
  if (control) control.disabled = true;
  notice("");
  try {
    await operation();
  } catch (error) {
    if (error.name !== "AbortError") notice(error.message, true);
  } finally {
    if (control) control.disabled = false;
  }
}

// Creates a text-only table row apart from explicitly constructed child elements.
function row(values) {
  const result = document.createElement("tr");
  for (const value of values) {
    const cell = result.insertCell();
    if (value instanceof Node) cell.append(value);
    else cell.textContent = value == null || value === "" ? "—" : String(value);
  }
  return result;
}

// Sets empty-state text across the full width of a table.
function emptyTable(target, columns, message) {
  const placeholder = row([message]);
  placeholder.cells[0].colSpan = columns;
  target.replaceChildren(placeholder);
}

// Creates an action button and routes its asynchronous errors to the notice area.
function action(label, operation) {
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = label;
  button.addEventListener("click", () => perform(button, operation));
  return button;
}

// Shows complete package digests while keeping the visible table compact.
function digest(value) {
  const element = document.createElement("code");
  element.className = "revision";
  element.textContent = value.slice(0, 12);
  element.title = value;
  element.setAttribute("aria-label", value);
  return element;
}

// Shows the login or workspace and discards account-specific data on a sign-out.
function setAccount(account) {
  state.generation++;
  state.account = account;
  if (account) clearSetupToken();
  const operator = account?.role === "operator";
  byId("setup").hidden = !!account || !setupToken;
  byId("sign-in").hidden = !!account || !!setupToken;
  byId("identity").hidden = !account;
  byId("workspace").hidden = !account;
  byId("operator").hidden = !operator;
  byId("operator-nav").hidden = !operator;
  byId("signed-in-as").textContent = account ? `${account.username} · ${account.role}` : "";

  state.games = [];
  state.experiments = [];
  state.configDefaults = {};
  state.experimentId = null;
  state.resetAccountId = null;
  clearSessions();
  for (const id of ["experiment-rows", "game-rows", "account-rows", "audit-rows",
    "experiment-title", "experiment-status", "experiment-config", "experiment-actions", "reset-title"]) {
    byId(id).replaceChildren();
  }
  byId("experiment-detail").hidden = true;
  byId("reset-password").hidden = true;
  for (const form of document.querySelectorAll("form")) form.reset();
  communicationConfig.replaceChildren();
  if (!account) byId(setupToken ? "setup-form" : "login").elements.username.focus();
}

// Reads the optional game scope from the dashboard URL.
function dashboardGame() {
  return /^\/games\/([a-z0-9_-]+)\/admin$/.exec(location.pathname)?.[1] ?? null;
}

// Links to the game's own dashboard using a known package identifier.
function gameLink(id) {
  const link = document.createElement("a");
  link.href = `/games/${encodeURIComponent(id)}/admin`;
  link.textContent = id;
  return link;
}

// Rebuilds the build and experiment catalogues with their current selection.
async function refreshDashboard() {
  const dashboard = await request("/dashboard");
  state.configDefaults = dashboard.config_defaults;
  if (!communicationConfig.hasChildNodes()) renderConfiguration({});
  const scope = dashboardGame();
  byId("dashboard-title").textContent = scope ? `Game dashboard · ${scope}` : "Hosting dashboard";
  state.games = scope ? dashboard.games.filter(game => game.id === scope) : dashboard.games;
  state.experiments = scope ? dashboard.experiments.filter(experiment => experiment.game === scope) : dashboard.experiments;
  const games = byId("game-rows");
  const builds = state.games.flatMap(game => game.revisions.map(build => {
    const use = action("Create experiment", () => configure(game.id, build.revision));
    return row([gameLink(game.id), build.version, digest(build.revision), Object.keys(build.agents ?? {}).join(", ") || "—", use]);
  }));
  if (builds.length) games.replaceChildren(...builds);
  else emptyTable(games, 5, "No game builds have been uploaded.");
  const experiments = byId("experiment-rows");
  const entries = state.experiments.map(experiment => {
    const manage = action("Manage", () => openExperiment(experiment.id));
    return row([experiment.id, experiment.game, digest(experiment.revision), experiment.status, manage]);
  });
  if (entries.length) experiments.replaceChildren(...entries);
  else emptyTable(experiments, 5, "No experiments have been created.");

  const gameSelect = experimentForm.elements.game;
  const game = gameSelect.value;
  gameSelect.replaceChildren(new Option("Choose a game", ""), ...state.games.map(item => new Option(item.id, item.id)));
  gameSelect.value = state.games.some(item => item.id === game) ? game : scope ?? "";
  updateRevisionOptions();
  renderExperiment();
}

// Keeps the selected revision within the selected game's revisions.
function updateRevisionOptions() {
  const game = state.games.find(item => item.id === experimentForm.elements.game.value);
  const revisionSelect = experimentForm.elements.revision;
  const revision = revisionSelect.value;
  const revisions = game?.revisions ?? [];
  revisionSelect.replaceChildren(new Option("Choose a revision", ""), ...revisions.map(item =>
    new Option(`${item.version} · ${item.revision.slice(0, 12)}`, item.revision)));
  if (revisions.some(item => item.revision === revision)) revisionSelect.value = revision;
  else if (revisions.length === 1) revisionSelect.value = revisions[0].revision;
}

// Opens a fresh experiment form prefilled from a package or pinned configuration.
function configure(game, revision, config = {}) {
  byId("create-experiment").open = true;
  experimentForm.reset();
  experimentForm.elements.game.value = game;
  updateRevisionOptions();
  experimentForm.elements.revision.value = revision;
  renderConfiguration(config);
  experimentForm.elements.id.focus();
}

// Splits an existing pin into controls and remaining JSON without dropping neighboring fields.
function renderConfiguration(config) {
  const other = structuredClone(config);
  const communication = {};
  for (const path of COMMUNICATION_FIELD_PATHS) {
    const [group, name] = path.split(".");
    setConfigAt(communication, path, configAt(config, path) ?? configAt(state.configDefaults, path));
    if (other[group]) {
      delete other[group][name];
      if (!Object.keys(other[group]).length) delete other[group];
    }
  }
  communicationConfig.innerHTML = renderStandardConfigField(COMMUNICATION_MODE_FIELD, communication)
    + renderSpeechSettings(communication);
  bindSpeechSettings(communicationConfig);
  experimentForm.elements.config.value = JSON.stringify(other, null, 2);
}

// Constructs an API path from the selected experiment's validated identifier.
function experimentApi(id) {
  return `/experiments/${encodeURIComponent(id)}/admin`;
}

// Shows the pinned experiment and applicable intake actions.
function renderExperiment() {
  const experiment = state.experiments.find(item => item.id === state.experimentId);
  byId("experiment-detail").hidden = !experiment;
  if (!experiment) return;
  byId("experiment-title").textContent = experiment.id;
  byId("experiment-config").textContent = JSON.stringify(experiment.config, null, 2);
  byId("experiment-status").textContent = experiment.status || "Unavailable";

  const actions = byId("experiment-actions");
  actions.replaceChildren();
  if (experiment.participant_url) {
    const url = new URL(experiment.participant_url, location.origin);
    if (url.protocol === "https:" || url.protocol === "http:") {
      const link = document.createElement("a");
      link.className = "button";
      link.href = url.href;
      link.target = "_blank";
      link.rel = "noopener noreferrer";
      link.textContent = "Participant page";
      actions.append(link);
    }
  }
  for (const [status, label] of LIFECYCLE_ACTIONS[experiment.status] ?? []) {
    actions.append(action(label, () => updateStatus(experiment.id, status)));
  }
  actions.append(action("Copy configuration to new experiment",
    () => configure(experiment.game, experiment.revision, experiment.config)));
}

// Changes intake status and retrieves the latest durable status.
async function updateStatus(id, status) {
  if (["completed", "archived"].includes(status) &&
      !window.confirm(`Set experiment "${id}" to ${status}?`)) return;
  const result = await request(`${experimentApi(id)}/experiment/status`, { method: "POST", body: { status } });
  const experiment = state.experiments.find(item => item.id === id);
  if (experiment) experiment.status = result.status;
  renderExperiment();
  await refreshDashboard();
  notice(`Updated ${id} to ${result.status}.`);
}

// Switches to the selected experiment and drops data belonging to the previous one.
function openExperiment(id) {
  state.experimentId = id;
  clearSessions();
  renderExperiment();
  byId("experiment-title").focus();
}

// Clears session identity, data and paging controls.
function clearSessions() {
  state.sessionId = null;
  state.sessionRequest++;
  state.sessionLimit = 50;
  byId("session-list").hidden = true;
  byId("load-more-sessions").hidden = true;
  byId("session-rows").replaceChildren();
  byId("session-detail").hidden = true;
  byId("session-title").replaceChildren();
  byId("session-data").replaceChildren();
}

// Fetches recent sessions for the selected experiment and can expand the result window.
async function loadSessions(limit) {
  const id = state.experimentId;
  const result = await request(`${experimentApi(id)}/sessions?limit=${limit}`);
  if (state.experimentId !== id) return;
  state.sessionLimit = limit;
  const rows = byId("session-rows");
  if (!result.sessions.length) emptyTable(rows, 6, "This experiment has no sessions.");
  else rows.replaceChildren(...result.sessions.map(session => row([
    session.dialogue_id, session.lifecycle, session.purpose, session.started_at, session.event_count,
    action("View", () => {
      state.sessionId = session.session_id;
      byId("session-title").textContent = session.dialogue_id;
      return loadSessionRecord("", true);
    }),
  ])));
  byId("session-list").hidden = false;
  byId("load-more-sessions").hidden = result.sessions.length < limit || limit >= SESSION_LIMIT;
}

// Displays a session's record, timeline, or replay using only textual JSON.
async function loadSessionRecord(suffix, focus = false) {
  const id = state.experimentId;
  const session = state.sessionId;
  const current = ++state.sessionRequest;
  byId("session-detail").hidden = false;
  byId("session-data").textContent = "Loading…";
  try {
    const output = await request(`${experimentApi(id)}/sessions/${encodeURIComponent(session)}${suffix}`);
    if (state.sessionRequest !== current) return;
    byId("session-data").textContent = JSON.stringify(output, null, 2);
  } catch (error) {
    if (state.sessionRequest !== current) return;
    byId("session-data").textContent = error.message;
    throw error;
  }
  if (focus) byId("session-title").focus();
}

// Downloads an authenticated export without navigating away or embedding response HTML.
async function exportCorpus(format) {
  const id = state.experimentId;
  const response = await request(`${experimentApi(id)}/export?format=${format}`, { data: "response" });
  const objectUrl = URL.createObjectURL(await response.blob());
  try {
    const link = document.createElement("a");
    link.href = objectUrl;
    link.download = `${id}-corpus-v1.${format}`;
    link.click();
  } finally {
    setTimeout(() => URL.revokeObjectURL(objectUrl), 1000);
  }
}

// Refreshes account and audit views for operators.
async function refreshOperator() {
  const [{ accounts }, { events }] = await Promise.all([request("/accounts"), request("/audit")]);
  const accountRows = byId("account-rows");
  if (accounts.length) {
    accountRows.replaceChildren(...accounts.map(account => {
      const controls = document.createElement("div");
      controls.className = "toolbar";
      if (!account.disabled) {
        controls.append(
          action("Reset password", () => openPasswordReset(account)),
          action("Disable", () => disableAccount(account)),
        );
      }
      return row([account.username, account.role, account.disabled ? "Disabled" : "Enabled", controls]);
    }));
  } else emptyTable(accountRows, 4, "No accounts found.");

  const auditRows = byId("audit-rows");
  if (events.length) auditRows.replaceChildren(...events.map(event => row([
    new Date(event.created_at * 1000).toLocaleString(), event.actor, event.operation, event.target, event.outcome,
  ])));
  else emptyTable(auditRows, 5, "No audit events recorded.");
}

// Opens the password reset form for the selected identity.
function openPasswordReset(account) {
  state.resetAccountId = account.id;
  const form = byId("reset-password");
  form.reset();
  byId("reset-title").textContent = `Reset password for ${account.username}`;
  form.hidden = false;
  form.elements.password.focus();
}

// Clears the selected account and any typed reset password.
function closePasswordReset() {
  state.resetAccountId = null;
  byId("reset-password").reset();
  byId("reset-password").hidden = true;
}

// Updates account availability with confirmation when access is being revoked.
async function disableAccount(account) {
  if (!window.confirm(`Disable "${account.username}" and revoke its access?`)) return;
  await request(`/accounts/${encodeURIComponent(account.id)}`, { method: "PUT", body: { disabled: true } });
  if (account.id === state.account.id) {
    setAccount(null);
    notice("Your account was disabled.");
    return;
  }
  await refreshOperator();
  notice("Account disabled.");
}

// Wires form submissions to validated, same-origin requests.
function onSubmit(id, submit) {
  byId(id).addEventListener("submit", event => {
    event.preventDefault();
    perform(event.submitter, () => submit(event.currentTarget));
  });
}

// Exchanges operator credentials for a cookie-backed dashboard session.
async function signIn(credentials) {
  await request("/auth/login", { method: "POST", body: credentials, data: "response" });
  await openDashboard();
}

// Creates the first operator with the setup credential and then signs in.
onSubmit("setup-form", async form => {
  const { username, password } = form.elements;
  const credentials = { username: username.value, password: password.value };
  try {
    await request("/auth/setup", { method: "POST", body: credentials, data: "response", bearer: setupToken });
  } catch (error) {
    if (error.status === 401 || error.status === 409) {
      clearSetupToken();
      setAccount(null);
    }
    throw error;
  }
  clearSetupToken();
  setAccount(null);
  byId("login").elements.username.value = credentials.username;
  await signIn(credentials);
  notice("Operator account created.");
});

onSubmit("login", async form => {
  const { username, password } = form.elements;
  const credentials = { username: username.value, password: password.value };
  try {
    await signIn(credentials);
  } finally {
    password.value = "";
  }
});

onSubmit("experiment-form", async form => {
  const { id, game, revision, config } = form.elements;
  // The server validates speech settings in context, including disabled providers.
  if ([id, game, revision, config].some(control => !control.reportValidity())) return;
  let configuration;
  try {
    configuration = JSON.parse(config.value);
  } catch {
    throw new Error("Enter a valid JSON configuration.");
  }
  if (!configuration || Array.isArray(configuration) || typeof configuration !== "object") {
    throw new Error("Configuration must be a JSON object.");
  }
  const duplicate = COMMUNICATION_FIELD_PATHS.find(path => configAt(configuration, path) !== undefined);
  if (duplicate) throw new Error(`Set ${duplicate} in the communication controls above.`);
  readConfigFields(communicationConfig, configuration);
  const experimentId = id.value;
  await request(`/experiments/${encodeURIComponent(experimentId)}`, {
    method: "PUT", body: { game: game.value, revision: revision.value, config: configuration },
  });
  form.reset();
  renderConfiguration({});
  byId("create-experiment").open = false;
  await refreshDashboard();
  openExperiment(experimentId);
  notice(`Experiment ${experimentId} is ready to configure for intake.`);
});

// Sends the ZIP to the server, which validates its contents and reads its manifest.
onSubmit("publish-form", async form => {
  const [archive] = form.elements.archive.files;
  if (!archive || !/\.zip$/i.test(archive.name)) throw new Error("Choose a game build ZIP file.");
  if (archive.size > UPLOAD_LIMIT) throw new Error("The ZIP exceeds the 64 MiB upload limit.");
  const game = dashboardGame();
  const path = game ? `/packages/${encodeURIComponent(game)}` : "/packages";
  notice("Uploading and validating the build ZIP…");
  const result = await request(path, { method: "PUT", body: archive, contentType: "application/zip" });
  form.reset();
  byId("publish-game").open = false;
  await refreshDashboard();
  if (!result.agent) configure(result.game, result.revision);
  notice(`Uploaded ${result.agent ?? result.game} ${result.version} (${result.revision.slice(0, 12)}).`);
});

onSubmit("export", form => exportCorpus(form.elements.format.value));

onSubmit("account-form", async form => {
  const { username, password, role } = form.elements;
  await request("/accounts", {
    method: "POST", body: { username: username.value, password: password.value, role: role.value }, data: "response",
  });
  form.reset();
  byId("create-account").open = false;
  await refreshOperator();
  notice("Account created.");
});

onSubmit("reset-password", async form => {
  const account = state.resetAccountId;
  await request(`/accounts/${encodeURIComponent(account)}`, {
    method: "PUT", body: { password: form.elements.password.value }, data: "response",
  });
  if (account === state.account.id) {
    setAccount(null);
    notice("Password reset. Sign in with your new password.");
    return;
  }
  closePasswordReset();
  await refreshOperator();
  notice("Password reset.");
});

// Connects persistent buttons to their operations.
function onClick(id, operation) {
  const control = byId(id);
  control.addEventListener("click", () => perform(control, operation));
}

onClick("logout", async () => {
  await request("/auth/logout", { method: "POST", data: "response" });
  setAccount(null);
  notice("Signed out.");
});
onClick("refresh", refreshDashboard);
onClick("load-sessions", () => loadSessions(state.sessionLimit));
onClick("load-more-sessions", () => loadSessions(Math.min(SESSION_LIMIT, state.sessionLimit * 2)));
onClick("session-summary", () => loadSessionRecord(""));
onClick("session-events", () => loadSessionRecord("/events"));
onClick("session-replay", () => loadSessionRecord("/replay"));
onClick("load-operator", refreshOperator);
onClick("cancel-reset", closePasswordReset);
// Clears a stale or unwanted first-run credential and shows ordinary sign-in.
onClick("cancel-setup", () => {
  clearSetupToken();
  setAccount(null);
});
// Opening another setup link in the same tab replaces the previous credential.
window.addEventListener("hashchange", () => {
  if (!new URLSearchParams(location.hash.slice(1)).has("setup")) return;
  setupToken = readSetupToken();
  if (state.account) clearSetupToken();
  else setAccount(null);
});
experimentForm.elements.game.addEventListener("change", updateRevisionOptions);

// Loads the cookie-backed identity before requesting any account-protected data.
async function openDashboard() {
  const account = await request("/auth/session");
  setAccount(account);
  await refreshDashboard();
  if (state.account?.role === "operator") await refreshOperator();
}

try {
  await openDashboard();
} catch (error) {
  if (!state.account) setAccount(null);
  else if (error.name !== "AbortError") notice(error.message, true);
}
