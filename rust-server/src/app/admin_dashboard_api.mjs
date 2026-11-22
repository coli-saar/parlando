// Builds experiment-scoped URLs and applies the dashboard request headers.
// SelectionRequests coalesces matching pending reads by resource and only delivers a
// completion to the selection that still owns it.

// Builds one public runtime path without exposing router layout to feature controllers.
export function experimentRuntimeApi(experiment, path) {
  if (!experiment) throw new Error('No experiment selected.');
  return `/api/admin/runtime/${encodeURIComponent(experiment.experiment_id)}/${path.replace(/^\//, '')}`;
}

// Preserves server-generated absolute templates and qualifies local participant paths.
export function absoluteParticipantUrl(currentOrigin, participantHref) {
  return /^https?:\/\//i.test(participantHref) ? participantHref : `${currentOrigin}${participantHref}`;
}

// Provider-backed participants must launch through Prolific, never through a literal template.
export function participantUrlCanOpen(experiment) {
  return experiment?.participant_url?.kind !== 'prolific';
}

// Applies the administrator request contract consistently to one Fetch options object.
export function adminRequestOptions(state, options = {}) {
  const method = String(options.method || 'GET').toUpperCase();
  const headers = new Headers(options.headers || {});
  if (!['GET', 'HEAD', 'OPTIONS'].includes(method)) {
    headers.set('X-CSRF-Token', state.csrfToken || '');
  }
  if (options.body != null && !headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json');
  }
  return { ...options, method, headers };
}

// Resolves an administrator API resource under the current dashboard's game namespace.
export function adminPath(state, path) {
  return `${state.adminPrefix || ''}${path}`;
}

// Sends one authenticated administrator request through the browser Fetch implementation.
export function adminFetch(state, input, options = {}) {
  return fetch(adminPath(state, input), adminRequestOptions(state, options));
}

// A selection owns the latest outstanding request for each resource it displays.
export class SelectionRequests {
  #requests = new Map();

  // Invalidates all pending completions, including a later return to the same selection.
  reset() {
    this.#requests.clear();
  }

  // Discards a read begun before a committed change to this resource.
  invalidate(resource) {
    this.#requests.delete(resource);
  }

  // Shares an identical pending read so regular polls cannot starve a slow response.
  json(resource, state, url, apply, options) {
    const pending = this.#requests.get(resource);
    const sharedUrl = options?.method && options.method !== 'GET' ? null : url;
    if (sharedUrl && pending?.url === sharedUrl) return pending.promise;
    const ticket = { url: sharedUrl };
    this.#requests.set(resource, ticket);
    ticket.promise = this.#load(ticket, resource, state, url, apply, options);
    return ticket.promise;
  }

  // Applies data and reports failures only to the selection that requested them.
  async #load(ticket, resource, state, url, apply, options) {
    try {
      const response = await adminFetch(state, url, options);
      if (!response.ok) throw new Error(await response.text());
      const data = await response.json();
      if (this.#requests.get(resource) === ticket) await apply(data);
    } catch (error) {
      if (this.#requests.get(resource) === ticket) throw error;
    } finally {
      if (this.#requests.get(resource) === ticket) this.#requests.delete(resource);
    }
  }
}
