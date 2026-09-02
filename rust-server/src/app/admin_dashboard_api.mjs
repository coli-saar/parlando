// Builds one public runtime path without exposing router layout to feature controllers.
export function experimentRuntimeApi(experiment, path) {
  if (!experiment) throw new Error('No experiment selected.');
  return `/api/admin/runtime/${encodeURIComponent(experiment.experiment_id)}/${path.replace(/^\//, '')}`;
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
