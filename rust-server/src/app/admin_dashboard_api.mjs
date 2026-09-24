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

// Sends one authenticated administrator request through the browser Fetch implementation.
export function adminFetch(state, input, options = {}) {
  return fetch(input, adminRequestOptions(state, options));
}
