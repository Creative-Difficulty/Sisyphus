//! Accounts: login with Hack Club Auth for the website and the desktop app,
//! sessions, logout, and linking devices to users.
//!
//! Website: `/auth/login` → Hack Club Auth → `/auth/callback` → session
//! cookie.
//!
//! Desktop app (it can't keep a client secret, so it logs in through us):
//! 1. The app listens on `127.0.0.1:<port>`, makes a random `verifier`, and
//!    opens `/auth/app?port=…&challenge=sha256hex(verifier)&device=<key>`.
//! 2. The user logs in in the browser as on the website.
//! 3. `/auth/callback` shows a confirmation page (`/auth/app/confirm`),
//!    tied to this browser by a cookie: "Log in the app as <name>?". Only a
//!    click on "Log in" there sends the browser to `http://127.0.0.1:<port>/
//!    done?code=…` (a one-time code, valid for 2 minutes from then), so
//!    following a login link by accident (Hack Club may skip its own
//!    screen) links nothing.
//! 4. The app posts the code, the `verifier` and a device signature over the
//!    code to `/auth/app/token`, which links the device to the user. A stolen
//!    code is useless without the verifier and the device's private key.
//!
//! Uploads are signed by the device key, so linking the device is all the
//! app needs; it gets no bearer token.

use axum::extract::{Query, State};
use axum::http::header::{COOKIE, LOCATION, ORIGIN, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sisyphus_common::api::{AppTokenRequest, AppTokenResponse, device_link_message};
use sisyphus_common::hex::Hash;
use sqlx::Row;

use crate::crypto::{constant_time_eq, random_token, sha256_base64url, sha256_hex};
use crate::oidc::Identity;
use crate::{AppState, now_ms};

/// How long a login may take between leaving for Hack Club and coming back.
const LOGIN_TTL_MS: i64 = 10 * 60 * 1000;
/// How long the app has to exchange its one-time code.
const APP_CODE_TTL_MS: i64 = 2 * 60 * 1000;
/// How long a website session lasts.
const SESSION_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", get(login))
        .route("/auth/app", get(app_login))
        .route("/auth/callback", get(callback))
        .route("/auth/app/confirm", get(confirm_page).post(confirm))
        .route("/auth/app/confirm/cancel", post(cancel))
        .route("/auth/app/token", post(app_token))
        .route("/auth/logout", post(logout))
        .route("/api/me", get(me))
}

/// The logged-in user, if the request carries a valid session cookie.
pub struct User {
    pub id: i64,
    pub name: Option<String>,
}

pub async fn current_user(state: &AppState, headers: &HeaderMap) -> Option<User> {
    let token = cookie(headers, &state.cookie_name("session", "/"))?;
    let row = sqlx::query(
        "SELECT u.id, u.name FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.token_hash = $1 AND s.expires_ms > $2",
    )
    .bind(sha256_hex(&token))
    .bind(now_ms())
    .fetch_optional(&state.db)
    .await
    .ok()??;
    Some(User {
        id: row.get("id"),
        name: row.get("name"),
    })
}

// ---------------------------------------------------------------------------
// Starting a login

async fn login(State(state): State<AppState>) -> Response {
    start(&state, None).await
}

#[derive(Deserialize)]
struct AppLoginParams {
    port: u16,
    challenge: String,
    device: String,
}

struct AppLogin {
    port: u16,
    challenge: String,
    device_id: String,
}

async fn app_login(
    State(state): State<AppState>,
    Query(params): Query<AppLoginParams>,
) -> Response {
    // Unprivileged ports only; the challenge is a SHA-256 in hex; the device
    // id must be a valid ed25519 public key.
    let valid = params.port >= 1024
        && is_hex(&params.challenge, 64)
        && is_hex(&params.device, 64)
        && parse_device_key(&params.device).is_some();
    if !valid {
        return message_page(StatusCode::BAD_REQUEST, "This login link is invalid.");
    }
    let app = AppLogin {
        port: params.port,
        challenge: params.challenge.to_lowercase(),
        device_id: params.device.to_lowercase(),
    };
    start(&state, Some(app)).await
}

async fn start(state: &AppState, app: Option<AppLogin>) -> Response {
    let Some(hackclub) = &state.hackclub else {
        return message_page(
            StatusCode::SERVICE_UNAVAILABLE,
            "Login isn't set up on this server.",
        );
    };
    let login_state = random_token();
    let nonce = random_token();
    let pkce_verifier = random_token();

    let now = now_ms();
    let stored = async {
        // Drop abandoned logins and codes while we're here.
        sqlx::query("DELETE FROM login_attempts WHERE expires_ms <= $1")
            .bind(now)
            .execute(&state.db)
            .await?;
        sqlx::query("DELETE FROM app_codes WHERE expires_ms <= $1")
            .bind(now)
            .execute(&state.db)
            .await?;
        sqlx::query(
            "INSERT INTO login_attempts
                 (state_hash, nonce, pkce_verifier, app_port, app_challenge, app_device_id, expires_ms)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(sha256_hex(&login_state))
        .bind(&nonce)
        .bind(&pkce_verifier)
        .bind(app.as_ref().map(|a| i32::from(a.port)))
        .bind(app.as_ref().map(|a| a.challenge.clone()))
        .bind(app.as_ref().map(|a| a.device_id.clone()))
        .bind(now + LOGIN_TTL_MS)
        .execute(&state.db)
        .await
    }
    .await;
    if let Err(e) = stored {
        eprintln!("login start failed: {e}");
        return message_page(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Something went wrong. Please try again.",
        );
    }

    let url = match hackclub
        .authorize_url(&login_state, &nonce, &sha256_base64url(&pkce_verifier))
        .await
    {
        Ok(url) => url,
        Err(e) => {
            eprintln!("login start failed: {e}");
            return message_page(
                StatusCode::BAD_GATEWAY,
                "Hack Club Auth isn't reachable. Please try again.",
            );
        }
    };

    // Tie the login to this browser, so nobody can send someone else a
    // callback link that logs them into the wrong account.
    let cookie = state.cookie("login", &login_state, "/auth/callback", LOGIN_TTL_MS / 1000);
    redirect(&url, &[cookie])
}

// ---------------------------------------------------------------------------
// Coming back from Hack Club Auth

#[derive(Deserialize)]
struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<CallbackParams>,
) -> Response {
    let clear_login = state.clear_cookie("login", "/auth/callback");
    if params.error.is_some() {
        return with_cookies(
            message_page(StatusCode::OK, "Login was cancelled."),
            &[clear_login],
        );
    }
    let (Some(code), Some(login_state), Some(hackclub)) =
        (params.code, params.state, state.hackclub.as_ref())
    else {
        return message_page(StatusCode::BAD_REQUEST, "This login link is invalid.");
    };
    let browser_state =
        cookie(&headers, &state.cookie_name("login", "/auth/callback")).unwrap_or_default();
    if !constant_time_eq(&browser_state, &login_state) {
        return with_cookies(
            message_page(
                StatusCode::BAD_REQUEST,
                "This login didn't start in this browser. Please try again.",
            ),
            &[clear_login],
        );
    }

    // Use the login attempt exactly once.
    let attempt = sqlx::query(
        "DELETE FROM login_attempts WHERE state_hash = $1 AND expires_ms > $2
         RETURNING nonce, pkce_verifier, app_port, app_challenge, app_device_id",
    )
    .bind(sha256_hex(&login_state))
    .bind(now_ms())
    .fetch_optional(&state.db)
    .await;
    let attempt = match attempt {
        Ok(Some(row)) => row,
        Ok(None) => {
            return with_cookies(
                message_page(
                    StatusCode::BAD_REQUEST,
                    "This login expired. Please try again.",
                ),
                &[clear_login],
            );
        }
        Err(e) => {
            eprintln!("login callback failed: {e}");
            return message_page(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Something went wrong. Please try again.",
            );
        }
    };
    let nonce: String = attempt.get("nonce");
    let pkce_verifier: String = attempt.get("pkce_verifier");

    let identity = match hackclub.finish_login(&code, &pkce_verifier, &nonce).await {
        Ok(identity) => identity,
        Err(e) => {
            eprintln!("login failed: {e}");
            return with_cookies(
                message_page(
                    StatusCode::BAD_GATEWAY,
                    "Hack Club login failed. Please try again.",
                ),
                &[clear_login],
            );
        }
    };
    let user_id = match upsert_user(&state, &identity).await {
        Ok(id) => id,
        Err(e) => {
            eprintln!("saving user failed: {e}");
            return message_page(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Something went wrong. Please try again.",
            );
        }
    };

    let app_port: Option<i32> = attempt.get("app_port");
    match app_port {
        // Desktop app: an unconfirmed one-time code, held by this browser
        // until the user confirms on the next page.
        Some(port) => {
            let code = random_token();
            let saved = sqlx::query(
                "INSERT INTO app_codes
                     (code_hash, user_id, device_id, challenge, app_port, confirmed, expires_ms)
                 VALUES ($1, $2, $3, $4, $5, FALSE, $6)",
            )
            .bind(sha256_hex(&code))
            .bind(user_id)
            .bind(attempt.get::<String, _>("app_device_id"))
            .bind(attempt.get::<String, _>("app_challenge"))
            .bind(port)
            .bind(now_ms() + LOGIN_TTL_MS)
            .execute(&state.db)
            .await;
            if let Err(e) = saved {
                eprintln!("saving app code failed: {e}");
                return message_page(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Something went wrong. Please try again.",
                );
            }
            let pending = state.cookie("confirm", &code, CONFIRM_PATH, LOGIN_TTL_MS / 1000);
            redirect(CONFIRM_PATH, &[clear_login, pending])
        }
        // Website: start a session.
        None => {
            let token = random_token();
            let now = now_ms();
            let saved = sqlx::query(
                "INSERT INTO sessions (token_hash, user_id, created_ms, expires_ms)
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(sha256_hex(&token))
            .bind(user_id)
            .bind(now)
            .bind(now + SESSION_TTL_MS)
            .execute(&state.db)
            .await;
            if let Err(e) = saved {
                eprintln!("saving session failed: {e}");
                return message_page(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Something went wrong. Please try again.",
                );
            }
            let session = state.cookie("session", &token, "/", SESSION_TTL_MS / 1000);
            redirect("/", &[clear_login, session])
        }
    }
}

// ---------------------------------------------------------------------------
// The desktop app's login waits for the user to confirm it

/// The confirmation page; its cookie is scoped here (and to `…/cancel`).
const CONFIRM_PATH: &str = "/auth/app/confirm";

/// The unconfirmed code this browser holds, if it is still valid.
fn pending_code(state: &AppState, headers: &HeaderMap) -> Option<String> {
    cookie(headers, &state.cookie_name("confirm", CONFIRM_PATH)).filter(|c| is_hex(c, 64))
}

pub(crate) fn from_this_site(state: &AppState, headers: &HeaderMap) -> bool {
    headers
        .get(ORIGIN)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|origin| origin == state.public_origin())
}

async fn confirm_page(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(code) = pending_code(&state, &headers) else {
        return message_page(StatusCode::BAD_REQUEST, EXPIRED);
    };
    let row = sqlx::query(
        "SELECT u.name FROM app_codes c JOIN users u ON u.id = c.user_id
         WHERE c.code_hash = $1 AND NOT c.confirmed AND c.expires_ms > $2",
    )
    .bind(sha256_hex(&code))
    .bind(now_ms())
    .fetch_optional(&state.db)
    .await;
    let name: Option<String> = match row {
        Ok(Some(row)) => row.get("name"),
        Ok(None) => return message_page(StatusCode::BAD_REQUEST, EXPIRED),
        Err(e) => {
            eprintln!("confirm page failed: {e}");
            return message_page(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Something went wrong. Please try again.",
            );
        }
    };
    let account = match name {
        Some(name) => format!(" as <strong>{}</strong>", html_escape(&name)),
        None => String::new(),
    };
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>Sisyphus</title><link rel=\"stylesheet\" href=\"/style.css\">\
         <script src=\"/theme.js\"></script></head>\
         <body><main class=\"message\"><h1>Sisyphus</h1>\
         <p>Log in the Sisyphus app on this computer{account}?</p>\
         <p class=\"muted\">Only continue if you just clicked \u{201c}Log in with Hack Club\u{201d} \
         in the app.</p>\
         <form method=\"post\" action=\"{CONFIRM_PATH}\">\
         <p><button class=\"pill confirm\" type=\"submit\">Log in</button></p></form>\
         <form method=\"post\" action=\"{CONFIRM_PATH}/cancel\">\
         <button class=\"link\" type=\"submit\">Cancel</button></form>\
         </main></body></html>"
    );
    let mut response = (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response();
    // The "Log in" form redirects to the app's loopback listener, which
    // the site-wide `form-action 'self'` would block.
    response.headers_mut().insert(
        axum::http::header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
             font-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; \
             form-action 'self' http://127.0.0.1:*",
        ),
    );
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    response
}

const EXPIRED: &str = "This login expired. Please start it again in the app.";

async fn confirm(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !from_this_site(&state, &headers) {
        return message_page(
            StatusCode::FORBIDDEN,
            "Login must be confirmed on this site.",
        );
    }
    let clear = state.clear_cookie("confirm", CONFIRM_PATH);
    let Some(code) = pending_code(&state, &headers) else {
        return message_page(StatusCode::BAD_REQUEST, EXPIRED);
    };
    let now = now_ms();
    // Confirm once; from now on the app has the usual two minutes.
    let row = sqlx::query(
        "UPDATE app_codes SET confirmed = TRUE, expires_ms = $3
         WHERE code_hash = $1 AND NOT confirmed AND expires_ms > $2
         RETURNING app_port",
    )
    .bind(sha256_hex(&code))
    .bind(now)
    .bind(now + APP_CODE_TTL_MS)
    .fetch_optional(&state.db)
    .await;
    match row {
        Ok(Some(row)) => {
            let port: i32 = row.get("app_port");
            redirect(
                &format!("http://127.0.0.1:{port}/done?code={code}"),
                &[clear],
            )
        }
        Ok(None) => with_cookies(message_page(StatusCode::BAD_REQUEST, EXPIRED), &[clear]),
        Err(e) => {
            eprintln!("confirming app login failed: {e}");
            message_page(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Something went wrong. Please try again.",
            )
        }
    }
}

async fn cancel(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !from_this_site(&state, &headers) {
        return message_page(
            StatusCode::FORBIDDEN,
            "Login must be cancelled on this site.",
        );
    }
    if let Some(code) = pending_code(&state, &headers)
        && let Err(e) = sqlx::query("DELETE FROM app_codes WHERE code_hash = $1 AND NOT confirmed")
            .bind(sha256_hex(&code))
            .execute(&state.db)
            .await
    {
        eprintln!("cancelling app login failed: {e}");
    }
    with_cookies(
        message_page(
            StatusCode::OK,
            "Login was cancelled. Nothing was linked to your account.",
        ),
        &[state.clear_cookie("confirm", CONFIRM_PATH)],
    )
}

fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

async fn upsert_user(state: &AppState, identity: &Identity) -> Result<i64, sqlx::Error> {
    let now = now_ms();
    sqlx::query(
        "INSERT INTO users
             (hackclub_id, name, slack_id, created_ms, last_login_ms)
         VALUES ($1, $2, $3, $4, $4)
         ON CONFLICT (hackclub_id) DO UPDATE SET
             name = EXCLUDED.name,
             slack_id = EXCLUDED.slack_id,
             last_login_ms = EXCLUDED.last_login_ms
         RETURNING id",
    )
    .bind(&identity.hackclub_id)
    .bind(&identity.name)
    .bind(&identity.slack_id)
    .bind(now)
    .fetch_one(&state.db)
    .await
    .map(|row| row.get("id"))
}

// ---------------------------------------------------------------------------
// The desktop app exchanges its code and links its device

#[derive(Serialize)]
struct ApiError {
    error: &'static str,
}

fn api_error(status: StatusCode, error: &'static str) -> Response {
    (status, Json(ApiError { error })).into_response()
}

async fn app_token(
    State(state): State<AppState>,
    Json(request): Json<AppTokenRequest>,
) -> Response {
    let device_id = request.device.to_string();
    let Some(device_key) = parse_device_key(&device_id) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid device key");
    };

    // Use the code exactly once, whatever happens next.
    let row = sqlx::query(
        "DELETE FROM app_codes WHERE code_hash = $1 AND confirmed
         RETURNING user_id, device_id, challenge, expires_ms",
    )
    .bind(sha256_hex(&request.code))
    .fetch_optional(&state.db)
    .await;
    let row = match row {
        Ok(Some(row)) => row,
        Ok(None) => return api_error(StatusCode::UNAUTHORIZED, "unknown or used code"),
        Err(e) => {
            eprintln!("app token failed: {e}");
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "database error");
        }
    };
    let user_id: i64 = row.get("user_id");
    let expected_device: String = row.get("device_id");
    let challenge: String = row.get("challenge");
    let expires_ms: i64 = row.get("expires_ms");

    if expires_ms <= now_ms() {
        return api_error(StatusCode::UNAUTHORIZED, "code expired");
    }
    if !constant_time_eq(&sha256_hex(&request.verifier), &challenge) {
        return api_error(StatusCode::UNAUTHORIZED, "verifier doesn't match");
    }
    if device_id != expected_device {
        return api_error(
            StatusCode::UNAUTHORIZED,
            "code was issued for another device",
        );
    }
    let signature = Signature::from_bytes(&request.signature.0);
    if device_key
        .verify(&device_link_message(&request.code), &signature)
        .is_err()
    {
        return api_error(StatusCode::UNAUTHORIZED, "bad device signature");
    }

    // Link the device, unless it already belongs to someone else.
    let now = now_ms();
    let linked = sqlx::query(
        "INSERT INTO devices
             (device_id, next_seq, prev, first_seen_ms, last_seen_ms, user_id, linked_ms)
         VALUES ($1, 0, $2, $3, $3, $4, $3)
         ON CONFLICT (device_id) DO UPDATE SET
             user_id = EXCLUDED.user_id,
             -- Heartbeats dated before a new link earn nothing (see upload.rs).
             linked_ms = CASE WHEN devices.user_id IS NULL
                              THEN EXCLUDED.linked_ms ELSE devices.linked_ms END
         WHERE devices.user_id IS NULL OR devices.user_id = EXCLUDED.user_id
         RETURNING device_id",
    )
    .bind(&device_id)
    .bind(Hash::ZERO.to_string())
    .bind(now)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await;
    match linked {
        Ok(Some(_)) => {}
        Ok(None) => return api_error(StatusCode::CONFLICT, "device belongs to another account"),
        Err(e) => {
            eprintln!("linking device failed: {e}");
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, "database error");
        }
    }

    let name = sqlx::query("SELECT name FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(&state.db)
        .await
        .ok()
        .and_then(|row| row.get("name"));
    Json(AppTokenResponse { name }).into_response()
}

// ---------------------------------------------------------------------------
// Logout and the current user

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    // Only our own pages may log someone out.
    let same_origin = headers
        .get(ORIGIN)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|origin| origin == state.public_origin());
    if !same_origin {
        return message_page(StatusCode::FORBIDDEN, "Logout must come from this site.");
    }
    if let Some(token) = cookie(&headers, &state.cookie_name("session", "/"))
        && let Err(e) = sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(sha256_hex(&token))
            .execute(&state.db)
            .await
    {
        eprintln!("logout failed: {e}");
    }
    redirect("/", &[state.clear_cookie("session", "/")])
}

#[derive(Serialize)]
struct Me {
    name: Option<String>,
    /// May use the review page (`/review`).
    reviewer: bool,
}

async fn me(State(state): State<AppState>, headers: HeaderMap) -> Response {
    match current_user(&state, &headers).await {
        Some(user) => Json(Me {
            reviewer: crate::review::is_reviewer(&state, user.id).await,
            name: user.name,
        })
        .into_response(),
        None => api_error(StatusCode::UNAUTHORIZED, "not logged in"),
    }
}

// ---------------------------------------------------------------------------
// Helpers

fn parse_device_key(hex_key: &str) -> Option<VerifyingKey> {
    let bytes: [u8; 32] = hex::decode(hex_key).ok()?.try_into().ok()?;
    VerifyingKey::from_bytes(&bytes).ok()
}

fn is_hex(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|b| b.is_ascii_hexdigit())
}

pub(crate) fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| {
            let (key, value) = pair.trim().split_once('=')?;
            (key == name).then(|| value.to_string())
        })
}

pub(crate) fn redirect(location: &str, cookies: &[HeaderValue]) -> Response {
    let mut response = (StatusCode::SEE_OTHER, [(LOCATION, location)]).into_response();
    for cookie in cookies {
        response.headers_mut().append(SET_COOKIE, cookie.clone());
    }
    response
}

fn with_cookies(mut response: Response, cookies: &[HeaderValue]) -> Response {
    for cookie in cookies {
        response.headers_mut().append(SET_COOKIE, cookie.clone());
    }
    response
}

/// A small page in the site's style for login results and errors. Messages
/// are fixed strings, never user input.
pub(crate) fn message_page(status: StatusCode, message: &'static str) -> Response {
    let html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>Sisyphus</title><link rel=\"stylesheet\" href=\"/style.css\"></head>\
         <body><main class=\"message\"><h1>Sisyphus</h1><p>{message}</p>\
         <p><a class=\"pill\" href=\"/\">Back</a></p></main></body></html>"
    );
    (
        status,
        [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_right_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static("a=1; sisyphus_session=abc; b=2"),
        );
        assert_eq!(cookie(&headers, "sisyphus_session").as_deref(), Some("abc"));
        assert_eq!(cookie(&headers, "missing"), None);
        assert_eq!(cookie(&headers, "sisyphus"), None);
    }

    #[test]
    fn escapes_names_for_html() {
        assert_eq!(
            html_escape("<b>\"Tom\" & 'Jo'</b>"),
            "&lt;b&gt;&quot;Tom&quot; &amp; &#39;Jo&#39;&lt;/b&gt;"
        );
    }

    #[test]
    fn hex_checks() {
        assert!(is_hex(&"a".repeat(64), 64));
        assert!(!is_hex(&"g".repeat(64), 64));
        assert!(!is_hex("abc", 64));
    }
}
