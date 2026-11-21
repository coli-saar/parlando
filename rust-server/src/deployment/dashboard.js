const API = "/api/hosting";
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
  experimentId: null,
  sessionId: null,
  sessionRequest: 0,
  sessionLimit: 50,
  resetAccountId: null,
};

const byId = id => document.getElementById(id);
const experimentForm = byId("experiment-form");

// Sends hosting requests to the operator origin and reports server validation messages.
async function request(path, { method = "GET", body, data = "json" } = {}) {
  const generation = state.generation;
  const headers = new Headers();
  if (method !== "GET" && state.account) {
    headers.set("X-CSRF-Token", state.account.csrf_token);
  }
  if (body !== undefined) headers.set("Content-Type", "application/json");
  const response = await fetch(API + path, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
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
    throw new Error(message || `Request failed (HTTP ${response.status}).`);
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
  const operator = account?.role === "operator";
  byId("sign-in").hidden = !!account;
  byId("identity").hidden = !account;
  byId("workspace").hidden = !account;
  byId("operator").hidden = !operator;
  byId("operator-nav").hidden = !operator;
  byId("signed-in-as").textContent = account ? `${account.username} · ${account.role}` : "";

  state.games = [];
  state.experiments = [];
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
  if (!account) byId("login").elements.username.focus();
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
  const scope = dashboardGame();
  byId("dashboard-title").textContent = scope ? `Game dashboard · ${scope}` : "Hosting dashboard";
  state.games = scope ? dashboard.games.filter(game => game.id === scope) : dashboard.games;
  state.experiments = scope ? dashboard.experiments.filter(experiment => experiment.game === scope) : dashboard.experiments;
  const games = byId("game-rows");
  const builds = state.games.flatMap(game => game.revisions.map(build => {
    const use = action("Create experiment", () => configure(game.id, build.revision));
    return row([gameLink(game.id), build.version, digest(build.revision), use]);
  }));
  if (builds.length) games.replaceChildren(...builds);
  else emptyTable(games, 4, "No game builds have been uploaded.");
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
  experimentForm.elements.config.value = JSON.stringify(config, null, 2);
  experimentForm.elements.id.focus();
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

// Mirrors the server's portable package path rules before any files leave the browser.
function validPackagePath(path) {
  return path.length <= 512 && path.split("/").every(part =>
    /^[A-Za-z0-9_-][A-Za-z0-9._-]{0,127}$/.test(part) && !part.endsWith("."));
}

// Encodes a selected file without passing a large byte array as function arguments.
function base64(file) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result.slice(reader.result.indexOf(",") + 1));
    reader.onerror = () => reject(new Error(`Could not read ${file.name}.`));
    reader.readAsDataURL(file);
  });
}

// Checks the selected folder and encodes only declared package files for upload.
async function packageFiles(directory, component, client) {
  if (!validPackagePath(component) || (client && !validPackagePath(client))) {
    throw new Error("Use relative package paths such as game.wasm or client.");
  }
  if (!directory.length || directory.length > 4096) throw new Error("Choose a package of up to 4096 files.");
  const paths = new Set();
  const entries = [];
  let encodedSize = 0;
  for (const file of directory) {
    const relative = file.webkitRelativePath;
    const path = relative.slice(relative.indexOf("/") + 1);
    const asset = client && path.startsWith(`${client}/`) ? path.slice(client.length + 1) : "";
    if (!validPackagePath(path) || paths.has(path.toLowerCase()) ||
        (path !== "game.yaml" && path !== component && !asset) ||
        (asset && /^(api|ws|admin|health)(\/|$)/i.test(asset))) {
      throw new Error(`Unexpected package file "${path}". Choose a directory containing only the prepared game build.`);
    }
    paths.add(path.toLowerCase());
    entries.push([path, file]);
    encodedSize += 4 * Math.ceil(file.size / 3) + path.length + 8;
  }

  const names = new Set(entries.map(([path]) => path));
  if (!names.has("game.yaml") || !names.has(component) ||
      (client && !names.has(`${client}/index.html`))) {
    throw new Error("The package needs game.yaml, the component, and index.html if a participant app is declared.");
  }
  if (encodedSize + 12 > UPLOAD_LIMIT) throw new Error("The encoded package exceeds the 64 MiB upload limit.");
  const files = Object.create(null);
  for (const [path, file] of entries) files[path] = await base64(file);
  return files;
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

onSubmit("login", async form => {
  const { username, password } = form.elements;
  try {
    await request("/auth/login", { method: "POST", body: { username: username.value, password: password.value }, data: "response" });
  } finally {
    password.value = "";
  }
  await openDashboard();
});

onSubmit("experiment-form", async form => {
  const { id, game, revision, config } = form.elements;
  let configuration;
  try {
    configuration = JSON.parse(config.value);
  } catch {
    throw new Error("Enter a valid JSON configuration.");
  }
  if (!configuration || Array.isArray(configuration) || typeof configuration !== "object") {
    throw new Error("Configuration must be a JSON object.");
  }
  const experimentId = id.value;
  await request(`/experiments/${encodeURIComponent(experimentId)}`, {
    method: "PUT", body: { game: game.value, revision: revision.value, config: configuration },
  });
  form.reset();
  byId("create-experiment").open = false;
  await refreshDashboard();
  openExperiment(experimentId);
  notice(`Experiment ${experimentId} is ready to configure for intake.`);
});

onSubmit("publish-form", async form => {
  const { id, component, client, directory } = form.elements;
  const generation = state.generation;
  const game = id.value;
  notice("Preparing the game package…");
  const files = await packageFiles(directory.files, component.value, client.value);
  if (state.generation !== generation) return;
  notice("Uploading and validating the game build…");
  const result = await request(`/packages/${encodeURIComponent(game)}`, { method: "PUT", body: { files } });
  form.reset();
  byId("publish-game").open = false;
  await refreshDashboard();
  configure(result.game, result.revision);
  notice(`Uploaded ${result.game} ${result.version} (${result.revision.slice(0, 12)}).`);
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
experimentForm.elements.game.addEventListener("change", updateRevisionOptions);

// Recognizes the standard participant app path produced by parlando build.
byId("publish-form").elements.directory.addEventListener("change", event => {
  const input = event.currentTarget;
  const client = input.form.elements.client;
  if (!client.value || client.value === "client") {
    const standardClient = [...input.files].some(file => /^[^/]+\/client\/index\.html$/.test(file.webkitRelativePath));
    client.value = standardClient ? "client" : "";
  }
});

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
