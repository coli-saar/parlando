//! Administrator sign-in, setup, and session cookies.
//!
//! The setup and login routes pass credentials to the installation authenticator. A
//! successful login issues the session cookie consumed by administrator middleware.
//! Logout revokes the session and clears the browser cookie.

use super::{
    cookie_value, json, validate_admin_request_origin, AdminLoginResult, AppError, Arc, CspNonce,
    Deserialize, ExperimentConfig, Extension, Game, HeaderMap, HeaderValue, Html, IntoResponse,
    Json, Redirect, Response, Result, ServerContext, State, StatusCode, ADMIN_ABSOLUTE_SECONDS,
    SET_COOKIE,
};

/// Redirects the short administrator URL away from the participant SPA fallback.
pub(super) async fn admin_entry() -> Redirect {
    Redirect::temporary("/admin/login")
}

/// Renders first-run administrator setup or the normal login form.
pub(super) async fn admin_login_page<A: Game>(
    State(state): State<Arc<ServerContext<A>>>,
    Extension(nonce): Extension<CspNonce>,
) -> Html<String> {
    let setup = !state.installation.admin_auth.is_configured().await;
    let (
        title,
        eyebrow,
        description,
        button,
        endpoint,
        confirmation,
        autocomplete,
        password_minimum,
        setup_note,
    ) = if setup {
        (
            "Create your administrator account",
            "First-time setup",
            "Set the credentials you will use to manage experiments and inspect sessions.",
            "Create administrator",
            "/api/admin/setup",
            r#"<label class="field"><span>Confirm password</span><input name="password_confirmation" type="password" autocomplete="new-password" required minlength="12"><small>Enter the same password again.</small></label>"#,
            "new-password",
            r#" minlength="12""#,
            r#"<div class="setup-note"><span class="setup-note-icon">✓</span><div><strong>Stored securely</strong><span>Only an Argon2id password hash is saved in this database.</span></div></div>"#,
        )
    } else {
        (
            "Welcome back",
            "Administrator access",
            "Sign in to manage experiments and review sessions.",
            "Sign in",
            "/api/admin/login",
            "",
            "current-password",
            "",
            "",
        )
    };
    Html(format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title} · Parlando</title>
  <style>
    :root {{ font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; background: #f6f7f9; color: #182026; }}
    * {{ box-sizing: border-box; }}
    body {{ margin: 0; min-height: 100vh; }}
    button, input {{ font: inherit; }}
    .app-header {{ align-items: center; background: #fff; border-bottom: 1px solid #dde2e7; display: flex; min-height: 64px; padding: 12px 24px; }}
    .brand {{ display: grid; gap: 3px; }}
    .brand-name {{ font-size: 20px; font-weight: 800; line-height: 1.15; }}
    .brand-section {{ color: #65717c; font-size: 12px; font-weight: 700; text-transform: uppercase; }}
    .page {{ align-items: center; display: flex; justify-content: center; min-height: calc(100vh - 64px); padding: 48px 20px; }}
    .auth-shell {{ display: grid; grid-template-columns: minmax(0, 42%) minmax(0, 58%); max-width: 880px; overflow: hidden; width: 100%; background: #fff; border: 1px solid #dce2e8; border-radius: 12px; box-shadow: 0 18px 60px rgba(20, 34, 45, .12); }}
    .context {{ background: #185f8f; color: #fff; display: flex; flex-direction: column; justify-content: space-between; min-height: 500px; padding: 42px; position: relative; }}
    .context::after {{ background: rgba(255,255,255,.08); border-radius: 999px; content: ""; height: 240px; position: absolute; right: -110px; top: -90px; width: 240px; }}
    .mark {{ align-items: center; background: rgba(255,255,255,.14); border: 1px solid rgba(255,255,255,.22); border-radius: 10px; display: inline-flex; font-size: 24px; font-weight: 850; height: 48px; justify-content: center; letter-spacing: -.04em; width: 48px; }}
    .context-copy {{ position: relative; z-index: 1; }}
    .context h2 {{ font-size: 29px; letter-spacing: -.02em; line-height: 1.15; margin: 24px 0 12px; max-width: 300px; }}
    .context p {{ color: #dbeaf4; line-height: 1.6; margin: 0; max-width: 310px; }}
    .context-footer {{ color: #c9dfed; font-size: 12px; font-weight: 650; position: relative; z-index: 1; }}
    .auth-panel {{ display: flex; flex-direction: column; justify-content: center; padding: 48px; }}
    .eyebrow {{ color: #185f8f; font-size: 12px; font-weight: 800; letter-spacing: .08em; margin-bottom: 10px; text-transform: uppercase; }}
    h1 {{ font-size: 28px; letter-spacing: -.02em; line-height: 1.2; margin: 0; }}
    .intro {{ color: #65717c; line-height: 1.55; margin: 10px 0 28px; }}
    form {{ display: grid; gap: 18px; }}
    .field {{ display: grid; gap: 7px; }}
    .field > span {{ color: #31404c; font-size: 13px; font-weight: 750; }}
    .field small {{ color: #76838e; font-size: 12px; }}
    input {{ appearance: none; background: #fff; border: 1px solid #c9d2da; border-radius: 7px; color: #182026; outline: none; padding: 11px 12px; transition: border-color 120ms ease, box-shadow 120ms ease; width: 100%; }}
    input:hover {{ border-color: #9fadb8; }}
    input:focus {{ border-color: #185f8f; box-shadow: 0 0 0 3px rgba(24,95,143,.14); }}
    .primary {{ background: #185f8f; border: 1px solid #185f8f; border-radius: 7px; color: #fff; cursor: pointer; font-weight: 750; margin-top: 4px; min-height: 44px; padding: 10px 16px; transition: background 120ms ease, transform 80ms ease; }}
    .primary:hover {{ background: #124f79; }}
    .primary:active {{ transform: translateY(1px); }}
    .primary:disabled {{ cursor: wait; opacity: .7; }}
    .setup-note {{ align-items: flex-start; background: #f1f7fb; border: 1px solid #cfe2ef; border-radius: 8px; color: #31404c; display: flex; gap: 10px; margin-bottom: 22px; padding: 12px; }}
    .setup-note-icon {{ align-items: center; background: #dceef8; border-radius: 999px; color: #135276; display: inline-flex; flex: 0 0 auto; font-size: 12px; font-weight: 900; height: 23px; justify-content: center; width: 23px; }}
    .setup-note div {{ display: grid; gap: 2px; }}
    .setup-note strong {{ font-size: 13px; }}
    .setup-note div span {{ color: #5b6873; font-size: 12px; line-height: 1.4; }}
    .error {{ background: #fff8f7; border: 1px solid #efc3bf; border-radius: 7px; color: #9f1d16; display: none; font-size: 13px; line-height: 1.4; margin: 16px 0 0; padding: 10px 12px; }}
    .error.visible {{ display: block; }}
    @media (max-width: 720px) {{
      .app-header {{ padding: 10px 16px; }}
      .brand-name {{ font-size: 18px; }}
      .page {{ align-items: flex-start; padding: 24px 14px; }}
      .auth-shell {{ display: block; }}
      .context {{ min-height: auto; padding: 26px; }}
      .context h2 {{ font-size: 23px; margin-top: 18px; }}
      .context-footer {{ display: none; }}
      .auth-panel {{ padding: 30px 26px 34px; }}
      h1 {{ font-size: 24px; }}
    }}
  </style>
</head>
<body>
  <header class="app-header"><div class="brand"><span class="brand-name">Parlando</span><span class="brand-section">Experimenter dashboard</span></div></header>
  <main class="page">
    <section class="auth-shell" aria-labelledby="auth-title">
      <aside class="context"><div class="context-copy"><div class="mark" aria-hidden="true">P</div><h2>Dialogue experiments, clearly managed.</h2><p>Configure experiments, follow live sessions, and export research data from one focused workspace.</p></div><div class="context-footer">Secure administrator area</div></aside>
      <div class="auth-panel">
        <div class="eyebrow">{eyebrow}</div>
        <h1 id="auth-title">{title}</h1>
        <p class="intro">{description}</p>
        {setup_note}
        <form id="admin-form">
          <label class="field"><span>Username</span><input name="username" autocomplete="username" required maxlength="128" autofocus></label>
          <label class="field"><span>Password</span><input name="password" type="password" autocomplete="{autocomplete}" required{password_minimum}><small>{password_help}</small></label>
          {confirmation}
          <button class="primary" type="submit">{button}</button>
        </form>
        <p class="error" id="error" role="alert"></p>
      </div>
    </section>
  </main>
  <script nonce="{nonce}">const form=document.getElementById('admin-form');const error=document.getElementById('error');const button=form.querySelector('button');const showError=message=>{{error.textContent=message;error.classList.add('visible');}};form.addEventListener('submit',async e=>{{e.preventDefault();error.classList.remove('visible');const f=new FormData(form);const password=f.get('password');const confirmation=f.get('password_confirmation');if(confirmation!==null&&password!==confirmation){{showError('Passwords do not match.');return;}}button.disabled=true;button.textContent='{pending_label}';try{{const r=await fetch('{endpoint}',{{method:'POST',headers:{{'content-type':'application/json'}},body:JSON.stringify({{username:f.get('username'),password}})}});if(r.ok){{location.href='/admin/experiments';return;}}showError(await r.text()||'Request failed.');}}catch(_){{showError('Could not reach the server. Please try again.');}}finally{{button.disabled=false;button.textContent='{button}';}}}});</script>
</body>
</html>"#,
        password_help = if setup {
            "Use at least 12 characters."
        } else {
            "Enter the password for this administrator account."
        },
        pending_label = if setup {
            "Creating…"
        } else {
            "Signing in…"
        },
        nonce = nonce.0,
    ))
}

#[derive(Deserialize)]
pub(super) struct AdminLoginRequest {
    username: String,
    password: String,
}

/// Persists the first administrator credential and signs that administrator in.
pub(super) async fn admin_setup<A: Game>(
    State(state): State<Arc<ServerContext<A>>>,
    headers: HeaderMap,
    Json(request): Json<AdminLoginRequest>,
) -> Result<Response, AppError> {
    validate_admin_request_origin(&state.config, &headers)?;
    if !state
        .installation
        .admin_auth
        .setup(&request.username, &request.password)
        .await
        .map_err(|error| AppError::bad_request(error.to_string()))?
    {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "Administrator setup is already complete",
        ));
    }
    tracing::info!(username = %request.username, "first administrator created");
    admin_login_response(&state, &request.username, &request.password).await
}

/// Verifies the built-in administrator credential and issues a secure session cookie.
pub(super) async fn admin_login<A: Game>(
    State(state): State<Arc<ServerContext<A>>>,
    headers: HeaderMap,
    Json(request): Json<AdminLoginRequest>,
) -> Result<Response, AppError> {
    validate_admin_request_origin(&state.config, &headers)?;
    admin_login_response(&state, &request.username, &request.password).await
}

/// Verifies a credential and builds the secure browser-session response.
async fn admin_login_response<A: Game>(
    state: &ServerContext<A>,
    username: &str,
    password: &str,
) -> Result<Response, AppError> {
    let login = state
        .installation
        .admin_auth
        .login(username, password)
        .await
        .map_err(AppError::from)?;
    let (token, session) = match login {
        AdminLoginResult::Authenticated(token, session) => (token, session),
        AdminLoginResult::Invalid => {
            tracing::warn!(username, "administrator login failed");
            return Err(AppError::new(
                StatusCode::UNAUTHORIZED,
                "Invalid administrator credentials",
            ));
        }
        AdminLoginResult::Busy => {
            tracing::warn!("administrator login concurrency limit reached");
            return Err(AppError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "Administrator login is busy; try again shortly",
            ));
        }
    };
    tracing::info!(username, role = ?session.role, "administrator login succeeded");
    let mut response = Json(json!({"ok": true, "csrf_token": session.csrf_token})).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&administrator_cookie(
            &token,
            ADMIN_ABSOLUTE_SECONDS,
            &state.config,
        ))
        .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?,
    );
    Ok(response)
}

/// Revokes the current administrator session and expires its cookie.
pub(super) async fn admin_logout<A: Game>(
    State(state): State<Arc<ServerContext<A>>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    if let Some(token) = cookie_value(&headers, "parlando_admin") {
        state.installation.admin_auth.logout(token).await?;
    }
    tracing::info!("administrator logged out");
    let mut response = Json(json!({"ok": true})).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&administrator_cookie("", 0, &state.config))
            .map_err(|error| AppError::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?,
    );
    Ok(response)
}

/// Builds one host-only first-party administrator cookie for HTTPS or loopback HTTP.
pub(super) fn administrator_cookie(token: &str, max_age: i64, config: &ExperimentConfig) -> String {
    let secure = config.server.public_base_url.starts_with("https://");
    format!(
        "parlando_admin={token}; Path=/; Max-Age={max_age}; {}HttpOnly; SameSite=Strict",
        if secure { "Secure; " } else { "" }
    )
}
