const API = "/api/hosting";
const UPLOAD_LIMIT = 64 * 1024 * 1024;
// Tracks the signed-in account, the scoped upload, and pending account actions.
const state = {
  account: null,
  generation: 0,
  uploadGame: null,
  expandedGames: new Set(),
  gamesRequest: 0,
  uploadedAgent: null,
  resetAccountId: null,
};

// A link opened by the CLI already identifies the terminal requesting approval.
const approvalCode = /^[A-F0-9]{16}$/.test(location.hash.slice(1)) ? location.hash.slice(1) : "";

// Looks up persistent dashboard controls.
const byId = id => document.getElementById(id);

// Sends dashboard requests to the configured origin and reports server validation messages.
async function request(path, { method = "GET", body, data = "json", contentType = "application/json" } = {}) {
  const generation = state.generation;
  const headers = new Headers();
  if (method !== "GET" && state.account) {
    headers.set("X-CSRF-Token", state.account.csrf_token);
  }
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

// Translates account roles for display without changing their API values.
function roleLabel(role) {
  return role === "operator" ? "Admin" : "Experimenter";
}

// Shows the authenticated workspace or the available sign-in form and clears account-specific data.
function setAccount(account, setupAvailable = false) {
  state.generation++;
  state.expandedGames.clear();
  state.uploadedAgent = null;
  state.account = account;
  const admin = account?.role === "operator";
  byId("auth").hidden = !!account;
  byId("setup").hidden = !!account || !setupAvailable;
  byId("sign-in").hidden = !!account || setupAvailable;
  byId("identity").hidden = !account;
  const cliLogin = new URLSearchParams(location.search).has("cli-login");
  byId("cli-login").hidden = !account || !cliLogin;
  if (cliLogin && approvalCode) {
    byId("cli-code").defaultValue = approvalCode;
    byId("cli-code").closest("label").hidden = true;
  }
  byId("workspace").hidden = !account || cliLogin;
  byId("workspace-nav").hidden = !account || cliLogin;
  byId("users-nav").hidden = !admin;
  byId("account-name").textContent = account?.username ?? "";
  byId("account-role").textContent = account ? roleLabel(account.role) : "";

  resetUpload();
  state.resetAccountId = null;
  for (const group of Array.from(byId("game-list").tBodies)) group.remove();
  for (const id of ["account-rows", "audit-rows", "reset-title"]) {
    byId(id).replaceChildren();
  }
  byId("reset-password").hidden = true;
  for (const disclosure of byId("workspace").querySelectorAll("details")) disclosure.open = false;
  for (const form of document.querySelectorAll("form")) form.reset();
  if (!account) {
    document.title = "Parlando";
    byId(setupAvailable ? "setup-form" : "login").elements.username.focus();
  }
}

// Restricts the Users view to authenticated administrators.
function showingUsers() {
  return location.hash === "#users" && state.account?.role === "operator";
}

// Resets the upload to deployment from any package and discards the selected file.
function resetUpload() {
  state.uploadGame = null;
  setUploadOpen(false);
  byId("publish-form").reset();
  byId("upload-target").textContent = "";
  byId("cancel-update").hidden = true;
  byId("submit-upload").textContent = "Upload package";
}

// Keeps the upload panel and its disclosure state in sync for both upload entry points.
function setUploadOpen(open) {
  byId("publish-game").hidden = !open;
  byId("open-upload").setAttribute("aria-expanded", String(open));
  if (open) byId("publish-form").elements.archive.focus();
}

// Scopes the upload to the selected game so the server can reject a mismatched ZIP.
function openUpdate(game) {
  resetUpload();
  state.uploadGame = game.id;
  byId("upload-target").textContent = `Updating ${game.name} (${game.id}).`;
  byId("cancel-update").hidden = false;
  byId("submit-upload").textContent = "Upload package";
  setUploadOpen(true);
}

// Pairs a prominent package name with its stable ID, or a version with its revision.
function stacked(primary, secondary) {
  const group = document.createElement("div");
  group.className = "package-stack";
  const title = document.createElement("span");
  title.append(primary);
  const detail = document.createElement("small");
  detail.append(secondary);
  group.append(title, detail);
  return group;
}

// Builds a game row and its adjacent, individually identifiable installed policies.
function gameRows(game, uploaded) {
  const link = document.createElement("a");
  link.href = `/${encodeURIComponent(game.id)}`;
  link.textContent = game.name;

  const status = document.createElement("span");
  status.className = game.active ? "game-status active" : "game-status";
  status.textContent = game.active ? "Active" : "Inactive";
  const identity = document.createElement("div");
  identity.className = "game-identity";
  identity.append(stacked(link, game.id));
  const update = action("Update", () => openUpdate(game));
  update.setAttribute("aria-label", `Update ${game.id}`);
  const parent = row([identity, stacked(game.version, digest(game.revision)), status, update]);
  parent.className = "game-row";
  if (!game.agents.length) return [parent];

  const children = game.agents.map(agent => {
    const child = row([stacked(agent.name, agent.id), stacked(agent.version, digest(agent.revision))]);
    child.className = "agent-row";
    child.hidden = !state.expandedGames.has(game.id);
    if (uploaded?.game === game.id && uploaded.agent === agent.id && uploaded.revision === agent.revision) {
      child.classList.add("uploaded-agent");
      child.tabIndex = -1;
    }
    return child;
  });
  const toggle = document.createElement("button");
  toggle.type = "button";
  toggle.className = "agent-toggle";
  const count = game.agents.length === 1 ? "1 agent" : `${game.agents.length} agents`;
  toggle.setAttribute("aria-label", `${count} for ${game.id}`);
  toggle.title = count;
  identity.querySelector("small").append(` · ${count}`);
  toggle.setAttribute("aria-expanded", String(state.expandedGames.has(game.id)));
  toggle.addEventListener("click", () => {
    const open = !state.expandedGames.has(game.id);
    if (open) state.expandedGames.add(game.id);
    else state.expandedGames.delete(game.id);
    toggle.setAttribute("aria-expanded", String(open));
    for (const child of children) child.hidden = !open;
  });
  identity.prepend(toggle);
  return [parent, ...children];
}

// Loads the currently deployed package and activation state for each game.
async function refreshGames() {
  const sequence = ++state.gamesRequest;
  const { games } = await request("/dashboard");
  if (sequence !== state.gamesRequest) return;
  const uploaded = state.uploadedAgent;
  state.uploadedAgent = null;
  const table = byId("game-list");
  for (const group of Array.from(table.tBodies)) group.remove();
  for (const game of games) {
    const group = table.createTBody();
    group.setAttribute("aria-label", game.name);
    group.append(...gameRows(game, uploaded));
  }
  if (!games.length) emptyTable(table.createTBody(), 4, "No games have been deployed.");
  const revealed = table.querySelector(".uploaded-agent");
  if (revealed && !byId("games").hidden) {
    revealed.focus({ preventScroll: true });
    revealed.scrollIntoView({ block: "nearest" });
  }
}

// Refreshes the Users list and its account controls.
async function refreshUsers() {
  const { accounts } = await request("/accounts");
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
      return row([account.username, roleLabel(account.role), account.disabled ? "Disabled" : "Enabled", controls]);
    }));
  } else emptyTable(accountRows, 4, "No accounts found.");
}

// Retrieves the latest audit events when an administrator opens the history.
async function refreshAudit() {
  const { events } = await request("/audit");
  const auditRows = byId("audit-rows");
  if (events.length) auditRows.replaceChildren(...events.map(event => row([
    new Date(event.created_at * 1000).toLocaleString(),
    event.actor === "host-operator-token" ? "Admin token" : event.actor,
    event.operation, event.target, event.outcome,
  ])));
  else emptyTable(auditRows, 5, "No audit events recorded.");
}

// Refreshes Users and includes the audit history when it is visible.
async function refreshUsersView() {
  await Promise.all([
    refreshUsers(),
    byId("audit-history").open ? refreshAudit() : undefined,
  ]);
}

// Applies the address to the workspace controls and loads the active view.
async function openWorkspaceView({ focus = false } = {}) {
  const users = showingUsers();
  const title = users ? "Users" : "Games";
  byId("page-title").textContent = title;
  document.title = `${title} · Parlando`;
  byId("games").hidden = users;
  byId("open-upload").hidden = users;
  byId("users").hidden = !users;
  if (!users) byId("audit-history").open = false;
  byId("games-nav").setAttribute("aria-current", users ? "false" : "page");
  byId("users-nav").setAttribute("aria-current", users ? "page" : "false");
  if (focus) byId("page-title").focus();
  if (users) await refreshUsersView();
  else await refreshGames();
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
  await refreshUsersView();
  notice("Account disabled.");
}

// Wires form submissions to validated, same-origin requests.
function onSubmit(id, submit) {
  byId(id).addEventListener("submit", event => {
    event.preventDefault();
    perform(event.submitter, () => submit(event.currentTarget));
  });
}

// Exchanges account credentials for a cookie-backed dashboard session.
async function signIn(credentials) {
  await request("/auth/login", { method: "POST", body: credentials, data: "response" });
  await openWorkspace({ allowSetup: false });
}

// Confirms the password, creates the first administrator, and signs in.
onSubmit("setup-form", async form => {
  const { username, password, password_confirmation } = form.elements;
  if (password.value !== password_confirmation.value) {
    password_confirmation.focus();
    throw new Error("Passwords do not match.");
  }
  const credentials = { username: username.value, password: password.value };
  try {
    await request("/auth/setup", { method: "POST", body: credentials, data: "response" });
  } catch (error) {
    if (error.status === 409) setAccount(null);
    throw error;
  }
  setAccount(null);
  byId("login").elements.username.value = credentials.username;
  await signIn(credentials);
  notice("Administrator account created.");
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

// Approves the pending CLI request using the current dashboard session.
onSubmit("cli-form", async form => {
  await request("/cli/approve", {method: "POST", body: {code: form.elements.code.value}});
  byId("cli-login").hidden = true;
  notice("CLI deployment authorized. Return to your terminal.");
});

// Sends the ZIP to the server, which validates its contents and reads its manifest.
onSubmit("publish-form", async form => {
  const [archive] = form.elements.archive.files;
  if (!archive || !/\.zip$/i.test(archive.name)) throw new Error("Choose a game or agent ZIP file.");
  if (archive.size > UPLOAD_LIMIT) throw new Error("The ZIP exceeds the 64 MiB upload limit.");
  const game = state.uploadGame;
  const path = game ? `/packages/${encodeURIComponent(game)}` : "/packages";
  notice("Uploading and validating the build ZIP…");
  const result = await request(path, { method: "PUT", body: archive, contentType: "application/zip" });
  resetUpload();
  if (result.agent) {
    state.expandedGames.add(result.game);
    state.uploadedAgent = result;
  }
  await refreshGames();
  notice(`Deployed ${result.name || result.agent || result.game} ${result.version} (${result.revision.slice(0, 12)}).`);
});

onSubmit("account-form", async form => {
  const { username, password, role } = form.elements;
  await request("/accounts", {
    method: "POST", body: { username: username.value, password: password.value, role: role.value }, data: "response",
  });
  form.reset();
  byId("create-account").open = false;
  await refreshUsersView();
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
  await refreshUsersView();
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
onClick("open-upload", () => {
  const open = byId("publish-game").hidden;
  resetUpload();
  setUploadOpen(open);
});
onClick("refresh", () => showingUsers() ? refreshUsersView() : refreshGames());
onClick("cancel-update", resetUpload);
onClick("cancel-reset", closePasswordReset);

// Reloads the visible view on same-document navigation, including browser Back.
window.addEventListener("hashchange", () => {
  if (state.account) perform(null, () => openWorkspaceView({ focus: true }));
});

// Updates account and game data when Back restores a cached document.
window.addEventListener("pageshow", event => {
  if (event.persisted) perform(null, openWorkspace);
});

// Keeps the audit request independent of ordinary Users navigation.
byId("audit-history").addEventListener("toggle", () => {
  if (showingUsers() && byId("audit-history").open) perform(null, refreshAudit);
});

// Loads the cookie-backed identity before requesting any account-protected data.
// An unsigned visitor can create an administrator only when the server offers setup.
async function openWorkspace({ allowSetup = true } = {}) {
  let account;
  try {
    account = await request("/auth/session");
  } catch (error) {
    if (error.status !== 401 || !allowSetup) throw error;
    const setup = await request("/auth/setup");
    setAccount(null, setup.available);
    return;
  }
  setAccount(account);
  if (!byId("cli-login").hidden) {
    if (approvalCode) byId("cli-form").querySelector("button").focus();
    else byId("cli-code").focus();
    return;
  }
  const next = new URLSearchParams(location.search).get("next");
  // Accept only a game mount path. The destination still checks the session and game.
  if (next && /^\/[a-z0-9][a-z0-9_-]{0,63}$/.test(next)
      && !["/admin", "/api", "/health"].includes(next)) {
    location.replace(next);
    return;
  }
  await openWorkspaceView();
}

try {
  await openWorkspace();
} catch (error) {
  if (error.name !== "AbortError") {
    if (!state.account) setAccount(null);
    notice(error.message, true);
  }
}
