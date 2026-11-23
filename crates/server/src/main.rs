//! Sisyphus server: accounts via Hack Club Auth, signed heartbeat uploads
//! stored in Postgres, and a small web dashboard.
//!
//! Configuration comes from the environment (or a `.env` file, see
//! `.env.example`):
//! - `DATABASE_URL`: Postgres connection URL (required)
//! - `BIND_ADDR`: address to listen on (default `0.0.0.0:8080`)
//! - `PUBLIC_URL`: the URL people reach the server at, e.g.
//!   `https://sisyphus.example` (default `http://localhost:8080`). Used for
//!   the login redirect and to decide whether cookies are `Secure`.
//! - `HACKCLUB_CLIENT_ID`, `HACKCLUB_CLIENT_SECRET`: the Hack Club Auth app.
//!   Without them, login is disabled.
//! - `HACKCLUB_AUTH_URL`: Hack Club Auth's base URL (default
//!   `https://auth.hackclub.com`).
//! - `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET`: the GitHub OAuth app for
//!   connecting GitHub and finding projects' repositories (`github.rs`).
//!   Without them, connecting GitHub is disabled.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::extract::Request;
use axum::http::HeaderValue;
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

mod app;
mod auth;
mod credit;
mod crypto;
mod dashboard;
mod devices;
mod github;
mod language;
mod oidc;
mod projects;
mod settings;
mod stats;
mod upload;
mod web;

/// Shared by every request handler.
#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    /// `None` when login isn't configured.
    pub hackclub: Option<Arc<oidc::HackClubAuth>>,
    /// `None` when connecting GitHub isn't configured.
    pub github: Option<Arc<github::GitHub>>,
    /// Without a trailing slash.
    pub public_url: Arc<str>,
    /// Cookies are only `Secure` (and use the `__Host-` prefix) on https.
    pub secure: bool,
    /// Signed app requests already answered, against replays.
    pub seen_requests: Arc<app::SeenRequests>,
}

impl AppState {
    /// The name of a cookie set with `path`. Setting and reading a cookie
    /// must both go through this, so the names always agree.
    pub fn cookie_name(&self, base: &str, path: &str) -> String {
        cookie_name(self.secure, base, path)
    }

    /// A `Set-Cookie` value.
    pub fn cookie(&self, base: &str, value: &str, path: &str, max_age_s: i64) -> HeaderValue {
        let name = self.cookie_name(base, path);
        let secure = if self.secure { "; Secure" } else { "" };
        HeaderValue::from_str(&format!(
            "{name}={value}; Path={path}; Max-Age={max_age_s}; HttpOnly; SameSite=Lax{secure}"
        ))
        .expect("cookie values are hex")
    }

    pub fn clear_cookie(&self, base: &str, path: &str) -> HeaderValue {
        self.cookie(base, "", path, 0)
    }

    /// `scheme://host[:port]` of `PUBLIC_URL`, as browsers send it in
    /// `Origin`.
    pub fn public_origin(&self) -> &str {
        let after_scheme = self.public_url.find("://").map_or(0, |i| i + 3);
        match self.public_url[after_scheme..].find('/') {
            Some(i) => &self.public_url[..after_scheme + i],
            None => &self.public_url,
        }
    }
}

#[tokio::main]
async fn main() {
    // A missing .env is fine; the variables may come from the environment.
    let _ = dotenvy::dotenv();

    let database_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        eprintln!("DATABASE_URL is not set (see .env.example)");
        std::process::exit(1);
    });
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let public_url = std::env::var("PUBLIC_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "http://localhost:8080".to_string())
        .trim_end_matches('/')
        .to_string();
    let secure = public_url.starts_with("https://");
    if !secure {
        eprintln!("warning: PUBLIC_URL is not https, so cookies are not marked Secure");
    }

    let hackclub = oidc::HackClubAuth::from_env(&public_url).map(Arc::new);
    match &hackclub {
        Some(h) => println!(
            "login with Hack Club enabled, redirect URI {}",
            h.redirect_uri()
        ),
        None => eprintln!(
            "warning: HACKCLUB_CLIENT_ID / HACKCLUB_CLIENT_SECRET not set, login is disabled"
        ),
    }

    let github = github::GitHub::from_env(&public_url).map(Arc::new);
    match &github {
        Some(g) => println!(
            "connecting GitHub enabled, callback URL {}",
            g.redirect_uri()
        ),
        None => eprintln!(
            "warning: GITHUB_CLIENT_ID / GITHUB_CLIENT_SECRET not set, connecting GitHub is disabled"
        ),
    }

    let db = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await
        .unwrap_or_else(|e| {
            eprintln!("could not connect to the database: {e}");
            std::process::exit(1);
        });
    if let Err(e) = sqlx::migrate!("./migrations").run(&db).await {
        eprintln!("could not run database migrations: {e}");
        std::process::exit(1);
    }
    // Credit heartbeats stored before crediting existed, or under an older
    // formula.
    match credit::backfill(&db).await {
        Ok(0) => {}
        Ok(n) => println!("credited {n} minutes"),
        Err(e) => eprintln!("crediting stored heartbeats failed: {e}"),
    }

    if let Some(github) = &github {
        github::spawn_matcher(db.clone(), github.clone());
    }
    let state = AppState {
        db,
        hackclub,
        github,
        public_url: public_url.into(),
        secure,
        seen_requests: Arc::default(),
    };
    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/upload", post(upload::upload))
        .route("/api/dashboard", get(stats::stats))
        .route("/api/projects", get(dashboard::projects))
        .route("/api/projects/settings", post(projects::save))
        .route("/api/devices", get(devices::list))
        .route("/api/devices/unlink", post(devices::unlink))
        .route("/api/heartbeats", get(dashboard::heartbeats))
        .route("/api/settings", get(settings::get).post(settings::save))
        .route("/api/app/today", get(app::today))
        .route("/api/app/logout", post(app::logout))
        .merge(auth::routes())
        .merge(github::routes())
        .merge(web::routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security_headers,
        ))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("could not listen on {bind_addr}: {e}");
            std::process::exit(1);
        });
    println!("sisyphus server listening on {bind_addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("server error");
}

/// Security headers on every response: no framing, no MIME sniffing, no
/// referrer to other sites (login URLs carry codes; `same-origin` rather
/// than `no-referrer`, which makes browsers send `Origin: null` on our own
/// form posts, such as logout), a strict content security policy (no inline
/// scripts), and HSTS on https.
async fn security_headers(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    let set = |headers: &mut axum::http::HeaderMap, name: &'static str, value: &'static str| {
        headers.insert(name, HeaderValue::from_static(value));
    };
    // A page may bring its own policy (the app login's confirmation page).
    if !headers.contains_key("content-security-policy") {
        set(
            headers,
            "content-security-policy",
            "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; \
         font-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; \
         form-action 'self'",
        );
    }
    set(headers, "x-frame-options", "DENY");
    set(headers, "x-content-type-options", "nosniff");
    set(headers, "referrer-policy", "same-origin");
    if state.secure {
        set(headers, "strict-transport-security", "max-age=31536000");
    }
    response
}

/// Cookie names: on https `__Host-` for site-wide cookies; browsers only
/// accept `__Host-` with `Path=/`, so narrower cookies use `__Secure-`.
/// Plain http (local testing) gets no prefix.
fn cookie_name(secure: bool, base: &str, path: &str) -> String {
    match (secure, path == "/") {
        (true, true) => format!("__Host-sisyphus_{base}"),
        (true, false) => format!("__Secure-sisyphus_{base}"),
        (false, _) => format!("sisyphus_{base}"),
    }
}

/// The server's clock (unix ms). This is the trusted time.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::cookie_name;

    #[test]
    fn cookie_names_match_their_path() {
        assert_eq!(cookie_name(true, "session", "/"), "__Host-sisyphus_session");
        assert_eq!(
            cookie_name(true, "login", "/auth/callback"),
            "__Secure-sisyphus_login"
        );
        assert_eq!(
            cookie_name(false, "login", "/auth/callback"),
            "sisyphus_login"
        );
    }
}
