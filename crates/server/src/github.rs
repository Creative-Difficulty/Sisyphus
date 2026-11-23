//! Connecting a GitHub account and finding each project's repository
//! (decided 2026-11-23, modelled on Hack Club's Hackatime).
//!
//! - `GET /auth/github` (logged in): to GitHub's authorization page, no
//!   extra scopes (public data only). The `state` is tied to this browser
//!   by a cookie scoped to the callback.
//! - `GET /auth/github/callback`: exchanges the code, reads the account
//!   (`/user`) and stores its id, login and token. A GitHub account is
//!   connected to one Sisyphus account at a time: connecting it elsewhere
//!   disconnects it here.
//! - `GET /api/github`: `{ "enabled": bool, "login": string | null }`.
//! - `POST /api/github/disconnect` (`Origin` must match `PUBLIC_URL`):
//!   deletes the token and asks GitHub to revoke it. Repositories already
//!   found stay.
//!
//! The matcher runs every 10 minutes (and right after connecting). For each
//! project of a connected user without a repository, it looks for a public
//! repository with the project's folder name: under the user's account,
//! else under the user's public organisations where the user has commits,
//! and only if exactly one organisation matches (a common name like
//! "website" isn't linked to the wrong one), else the one public
//! repository with that name the user contributed to (commits or pull
//! requests in the past year, GitHub's `repositoriesContributedTo`).
//! Private repositories are never linked. A miss is retried after 14
//! days; at most 50 GitHub requests per user per day. Projects whose
//! repository the user set or cleared are left alone.
//!
//! Configured by `GITHUB_CLIENT_ID` and `GITHUB_CLIENT_SECRET` (an OAuth
//! app whose callback URL is `PUBLIC_URL` + `/auth/github/callback`);
//! without them, connecting is disabled. `GITHUB_URL` and `GITHUB_API_URL`
//! (default `https://github.com`, `https://api.github.com`) are for tests;
//! plain http only on localhost.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};

use crate::auth::{self, cookie, from_this_site, message_page, redirect};
use crate::crypto::{constant_time_eq, random_token, url_encode};
use crate::{AppState, now_ms};

const STATE_TTL_S: i64 = 600;
const CALLBACK_PATH: &str = "/auth/github/callback";
const PASS_EVERY: Duration = Duration::from_secs(600);
/// How long a miss suppresses another lookup.
const RETRY_MISS_MS: i64 = 14 * 86_400_000;
/// GitHub requests the matcher may make per user per day.
const DAILY_BUDGET: u32 = 50;
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

pub struct GitHub {
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    web_url: String,
    api_url: String,
    http: ureq::Agent,
    /// Wakes the matcher early (after someone connects).
    wake: tokio::sync::Notify,
}

impl GitHub {
    /// `None` unless `GITHUB_CLIENT_ID` and `GITHUB_CLIENT_SECRET` are set.
    pub fn from_env(public_url: &str) -> Option<Self> {
        let env = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
        let client_id = env("GITHUB_CLIENT_ID")?;
        let client_secret = env("GITHUB_CLIENT_SECRET")?;
        let base = |key: &str, default: &str| -> Option<String> {
            let url = env(key).unwrap_or_else(|| default.to_string());
            let url = url.trim_end_matches('/').to_string();
            if url.starts_with("https://") || is_loopback_http(&url) {
                Some(url)
            } else {
                eprintln!("warning: {key} must be https, GitHub is disabled");
                None
            }
        };
        Some(Self {
            client_id,
            client_secret,
            redirect_uri: format!("{public_url}{CALLBACK_PATH}"),
            web_url: base("GITHUB_URL", "https://github.com")?,
            api_url: base("GITHUB_API_URL", "https://api.github.com")?,
            http: ureq::Agent::config_builder()
                .timeout_global(Some(HTTP_TIMEOUT))
                .http_status_as_error(false)
                .user_agent("sisyphus-server")
                .build()
                .into(),
            wake: tokio::sync::Notify::new(),
        })
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// A GET, or a POST of `body` (GraphQL).
    fn request(
        &self,
        token: &str,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<Answer, String> {
        let url = format!("{}{path}", self.api_url);
        let authorization = format!("Bearer {token}");
        let sent = match body {
            None => self
                .http
                .get(&url)
                .header("Authorization", &authorization)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .call(),
            Some(body) => self
                .http
                .post(&url)
                .header("Authorization", &authorization)
                .header("Accept", "application/vnd.github+json")
                .send_json(body),
        };
        let mut response = sent.map_err(|e| format!("request to {url} failed: {e}"))?;
        let status = response.status().as_u16();
        let body = if status == 200 {
            response
                .body_mut()
                .read_json::<serde_json::Value>()
                .map_err(|e| format!("invalid JSON from {url}: {e}"))?
        } else {
            serde_json::Value::Null
        };
        Ok(Answer { status, body })
    }
}

struct Answer {
    status: u16,
    body: serde_json::Value,
}

fn is_loopback_http(url: &str) -> bool {
    ["http://localhost", "http://127.0.0.1"].iter().any(|host| {
        url.strip_prefix(host)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(':') || rest.starts_with('/'))
    })
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/github", get(connect))
        .route(CALLBACK_PATH, get(callback))
        .route("/api/github", get(status))
        .route("/api/github/disconnect", post(disconnect))
}

// ---------------------------------------------------------------------------
// Connecting

async fn connect(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(github) = state.github.clone() else {
        return message_page(
            StatusCode::NOT_FOUND,
            "Connecting GitHub isn't set up here.",
        );
    };
    if auth::current_user(&state, &headers).await.is_none() {
        return redirect("/", &[]);
    }
    let oauth_state = random_token();
    let url = format!(
        "{}/login/oauth/authorize?client_id={}&redirect_uri={}&state={}&scope=&allow_signup=false",
        github.web_url,
        url_encode(&github.client_id),
        url_encode(&github.redirect_uri),
        oauth_state,
    );
    let cookie = state.cookie("github", &oauth_state, CALLBACK_PATH, STATE_TTL_S);
    redirect(&url, &[cookie])
}

#[derive(Deserialize)]
struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
}

const FAILED: &str = "Connecting GitHub didn't work. Please try again.";

async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<CallbackParams>,
) -> Response {
    let clear = state.clear_cookie("github", CALLBACK_PATH);
    let fail = |status, message| {
        let mut response = message_page(status, message);
        response
            .headers_mut()
            .append(axum::http::header::SET_COOKIE, clear.clone());
        response
    };
    let Some(github) = state.github.clone() else {
        return fail(
            StatusCode::NOT_FOUND,
            "Connecting GitHub isn't set up here.",
        );
    };
    let Some(user) = auth::current_user(&state, &headers).await else {
        return fail(StatusCode::UNAUTHORIZED, "Please log in first.");
    };
    let expected = cookie(&headers, &state.cookie_name("github", CALLBACK_PATH));
    let (Some(code), Some(got), Some(expected)) = (params.code, params.state, expected) else {
        // Also when the user declined on GitHub.
        return fail(StatusCode::BAD_REQUEST, "GitHub wasn't connected.");
    };
    if !constant_time_eq(&got, &expected) {
        return fail(StatusCode::BAD_REQUEST, FAILED);
    }

    let account = {
        let github = github.clone();
        tokio::task::spawn_blocking(move || exchange(&github, &code)).await
    };
    let (token, id, login) = match account {
        Ok(Ok(account)) => account,
        Ok(Err(e)) => {
            eprintln!("github connect failed: {e}");
            return fail(StatusCode::BAD_GATEWAY, FAILED);
        }
        Err(e) => {
            eprintln!("github connect failed: {e}");
            return fail(StatusCode::INTERNAL_SERVER_ERROR, FAILED);
        }
    };
    if let Err(e) = save_account(&state.db, user.id, id, &login, &token).await {
        eprintln!("github connect failed: {e}");
        return fail(StatusCode::INTERNAL_SERVER_ERROR, FAILED);
    }
    github.wake.notify_one();
    redirect("/#settings", &[clear])
}

/// Code → token → the account's id and login.
fn exchange(github: &GitHub, code: &str) -> Result<(String, i64, String), String> {
    let url = format!("{}/login/oauth/access_token", github.web_url);
    let mut response = github
        .http
        .post(&url)
        .header("Accept", "application/json")
        .send_form([
            ("client_id", github.client_id.as_str()),
            ("client_secret", github.client_secret.as_str()),
            ("code", code),
            ("redirect_uri", github.redirect_uri.as_str()),
        ])
        .map_err(|e| format!("request to {url} failed: {e}"))?;
    let body: serde_json::Value = response
        .body_mut()
        .read_json()
        .map_err(|e| format!("invalid JSON from {url}: {e}"))?;
    let token = body["access_token"]
        .as_str()
        .ok_or_else(|| format!("no token from GitHub: {}", body["error"]))?
        .to_string();
    let user = github.request(&token, "/user", None)?;
    if user.status != 200 {
        return Err(format!("/user answered {}", user.status));
    }
    let id = user.body["id"].as_i64().ok_or("no id in /user")?;
    let login = user.body["login"]
        .as_str()
        .ok_or("no login in /user")?
        .to_string();
    Ok((token, id, login))
}

async fn save_account(
    db: &PgPool,
    user_id: i64,
    github_id: i64,
    login: &str,
    token: &str,
) -> sqlx::Result<()> {
    let mut tx = db.begin().await?;
    // One Sisyphus account per GitHub account.
    sqlx::query(
        "UPDATE users SET github_id = NULL, github_login = NULL, github_token = NULL
         WHERE github_id = $1 AND id <> $2",
    )
    .bind(github_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE users SET github_id = $2, github_login = $3, github_token = $4 WHERE id = $1",
    )
    .bind(user_id)
    .bind(github_id)
    .bind(login)
    .bind(token)
    .execute(&mut *tx)
    .await?;
    // Misses were looked up for another account (or none): look again.
    sqlx::query(
        "UPDATE projects SET repo_checked_ms = NULL
         WHERE user_id = $1 AND NOT repo_manual AND repo_url IS NULL",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

#[derive(Serialize)]
struct Status {
    /// Whether this server can connect GitHub at all.
    enabled: bool,
    login: Option<String>,
}

async fn status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(user) = auth::current_user(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let login =
        sqlx::query("SELECT github_login FROM users WHERE id = $1 AND github_token IS NOT NULL")
            .bind(user.id)
            .fetch_optional(&state.db)
            .await;
    match login {
        Ok(row) => Json(Status {
            enabled: state.github.is_some(),
            login: row.map(|row| row.get("github_login")),
        })
        .into_response(),
        Err(e) => {
            eprintln!("github status failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn disconnect(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if !from_this_site(&state, &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(user) = auth::current_user(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let token = sqlx::query(
        "UPDATE users u SET github_id = NULL, github_login = NULL, github_token = NULL
         FROM (SELECT id, github_token FROM users WHERE id = $1) old
         WHERE u.id = old.id
         RETURNING old.github_token",
    )
    .bind(user.id)
    .fetch_one(&state.db)
    .await;
    let token: Option<String> = match token {
        Ok(row) => row.get("github_token"),
        Err(e) => {
            eprintln!("github disconnect failed: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    // Best effort: also revoke the grant on GitHub.
    if let (Some(github), Some(token)) = (state.github.clone(), token) {
        tokio::task::spawn_blocking(move || {
            let url = format!(
                "{}/applications/{}/grant",
                github.api_url,
                url_encode(&github.client_id)
            );
            let basic = basic_auth(&github.client_id, &github.client_secret);
            let result = github
                .http
                .delete(&url)
                .header("Authorization", &format!("Basic {basic}"))
                .header("Accept", "application/vnd.github+json")
                .force_send_body()
                .send_json(serde_json::json!({ "access_token": token }));
            if let Err(e) = result {
                eprintln!("revoking a GitHub token failed: {e}");
            }
        });
    }
    StatusCode::NO_CONTENT.into_response()
}

fn basic_auth(user: &str, password: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = format!("{user}:{password}").into_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (u32::from(b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Matching projects to repositories

/// Runs the matcher every 10 minutes, or when woken.
pub fn spawn_matcher(db: PgPool, github: Arc<GitHub>) {
    tokio::spawn(async move {
        let mut budget = Budget::default();
        loop {
            if let Err(e) = pass(&db, &github, &mut budget).await {
                eprintln!("repository matching failed: {e}");
            }
            tokio::select! {
                _ = tokio::time::sleep(PASS_EVERY) => {}
                _ = github.wake.notified() => {}
            }
        }
    });
}

/// Requests per user today.
#[derive(Default)]
struct Budget {
    day: i64,
    spent: HashMap<i64, u32>,
}

impl Budget {
    fn spend(&mut self, user_id: i64) -> bool {
        let day = now_ms() / 86_400_000;
        if day != self.day {
            self.day = day;
            self.spent.clear();
        }
        let spent = self.spent.entry(user_id).or_default();
        if *spent >= DAILY_BUDGET {
            return false;
        }
        *spent += 1;
        true
    }
}

/// Why a lookup stopped without an answer: it is tried again later, and
/// not remembered as a miss.
enum Stop {
    /// Out of today's budget, rate-limited or GitHub failed.
    Later,
    /// GitHub rejected the token: it is deleted.
    Revoked,
}

async fn pass(db: &PgPool, github: &Arc<GitHub>, budget: &mut Budget) -> sqlx::Result<()> {
    let now = now_ms();
    let users = sqlx::query(
        "SELECT id, github_login, github_token FROM users
         WHERE github_token IS NOT NULL AND github_login IS NOT NULL",
    )
    .fetch_all(db)
    .await?;
    for user in users {
        let user_id: i64 = user.get("id");
        let login: String = user.get("github_login");
        let token: String = user.get("github_token");
        let names: Vec<String> = sqlx::query(
            "SELECT DISTINCT s.project FROM agent_sessions s
             LEFT JOIN projects p ON p.user_id = s.user_id AND p.name = s.project
             WHERE s.user_id = $1
               AND (p.name IS NULL
                    OR (NOT p.repo_manual AND p.repo_url IS NULL
                        AND (p.repo_checked_ms IS NULL OR p.repo_checked_ms < $2)))",
        )
        .bind(user_id)
        .bind(now - RETRY_MISS_MS)
        .fetch_all(db)
        .await?
        .into_iter()
        .map(|row| row.get("project"))
        .filter(|name: &String| is_repo_name(name))
        .collect();
        let mut lists = Lists::default();
        for name in names {
            let found = lookup(github, budget, user_id, &login, &token, &name, &mut lists).await;
            match found {
                Ok(url) => {
                    sqlx::query(
                        "INSERT INTO projects (user_id, name, repo_url, repo_checked_ms)
                         VALUES ($1, $2, $3, $4)
                         ON CONFLICT (user_id, name) DO UPDATE SET
                             repo_url = CASE WHEN projects.repo_manual THEN projects.repo_url
                                             ELSE EXCLUDED.repo_url END,
                             repo_checked_ms = EXCLUDED.repo_checked_ms",
                    )
                    .bind(user_id)
                    .bind(&name)
                    .bind(&url)
                    .bind(now)
                    .execute(db)
                    .await?;
                }
                Err(Stop::Later) => break,
                Err(Stop::Revoked) => {
                    sqlx::query(
                        "UPDATE users SET github_token = NULL WHERE id = $1 AND github_token = $2",
                    )
                    .bind(user_id)
                    .bind(&token)
                    .execute(db)
                    .await?;
                    break;
                }
            }
        }
    }
    Ok(())
}

/// GitHub repository names: letters, digits, `.`, `_`, `-`, up to 100,
/// not `.` or `..`.
fn is_repo_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A user's organisations and the public repositories they contributed
/// to, fetched once per pass when first needed.
#[derive(Default)]
struct Lists {
    orgs: Option<Vec<String>>,
    /// (name, url)
    contributed: Option<Vec<(String, String)>>,
}

/// The project's public repository, or `None` if there's no unambiguous
/// one: the user's own, else one in exactly one of their organisations
/// with their commits, else exactly one public repository with that name
/// they contributed to (commits or pull requests, GitHub's
/// `repositoriesContributedTo`: the past year, not their own).
async fn lookup(
    github: &Arc<GitHub>,
    budget: &mut Budget,
    user_id: i64,
    login: &str,
    token: &str,
    name: &str,
    lists: &mut Lists,
) -> Result<Option<String>, Stop> {
    if let Some(url) = public_repo(github, budget, user_id, token, login, name).await? {
        return Ok(Some(url));
    }
    let orgs = &mut lists.orgs;
    if orgs.is_none() {
        let answer = api(
            github,
            budget,
            user_id,
            token,
            format!("/users/{}/orgs", url_encode(login)),
            None,
        )
        .await?;
        *orgs = Some(
            answer
                .body
                .as_array()
                .map(|list| {
                    list.iter()
                        .filter_map(|org| org["login"].as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        );
    }
    let mut matches = Vec::new();
    for org in orgs.clone().unwrap_or_default() {
        let Some(url) = public_repo(github, budget, user_id, token, &org, name).await? else {
            continue;
        };
        let commits = api(
            github,
            budget,
            user_id,
            token,
            format!(
                "/repos/{}/{name}/commits?author={}&per_page=1",
                url_encode(&org),
                url_encode(login)
            ),
            None,
        )
        .await?;
        if commits.body.as_array().is_some_and(|list| !list.is_empty()) {
            matches.push(url);
            if matches.len() > 1 {
                break;
            }
        }
    }
    match matches.len() {
        1 => return Ok(matches.pop()),
        0 => {}
        // Ambiguous between organisations: leave it to the user.
        _ => return Ok(None),
    }

    if lists.contributed.is_none() {
        let query = serde_json::json!({
            "query": "query($login: String!) { user(login: $login) { \
                repositoriesContributedTo(first: 100, privacy: PUBLIC, \
                includeUserRepositories: false, \
                contributionTypes: [COMMIT, PULL_REQUEST]) { nodes { name url isPrivate } } } }",
            "variables": { "login": login },
        });
        let answer = api(
            github,
            budget,
            user_id,
            token,
            "/graphql".into(),
            Some(query),
        )
        .await?;
        lists.contributed = Some(contributed(&answer.body));
    }
    let mut found = lists
        .contributed
        .iter()
        .flatten()
        .filter(|(repo, _)| repo.eq_ignore_ascii_case(name))
        .map(|(_, url)| url.clone())
        .collect::<Vec<_>>();
    Ok(if found.len() == 1 { found.pop() } else { None })
}

/// The public repositories in a `repositoriesContributedTo` answer.
fn contributed(body: &serde_json::Value) -> Vec<(String, String)> {
    body["data"]["user"]["repositoriesContributedTo"]["nodes"]
        .as_array()
        .map(|nodes| {
            nodes
                .iter()
                .filter(|node| node["isPrivate"].as_bool() == Some(false))
                .filter_map(|node| {
                    let name = node["name"].as_str()?;
                    let url = node["url"].as_str().filter(|u| u.starts_with("https://"))?;
                    Some((name.to_string(), url.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn public_repo(
    github: &Arc<GitHub>,
    budget: &mut Budget,
    user_id: i64,
    token: &str,
    owner: &str,
    name: &str,
) -> Result<Option<String>, Stop> {
    let answer = api(
        github,
        budget,
        user_id,
        token,
        format!("/repos/{}/{name}", url_encode(owner)),
        None,
    )
    .await?;
    if answer.status != 200 || answer.body["private"].as_bool() != Some(false) {
        return Ok(None);
    }
    Ok(answer.body["html_url"]
        .as_str()
        .filter(|url| url.starts_with("https://"))
        .map(str::to_string))
}

/// One API request, counted against the user's budget.
async fn api(
    github: &Arc<GitHub>,
    budget: &mut Budget,
    user_id: i64,
    token: &str,
    path: String,
    body: Option<serde_json::Value>,
) -> Result<Answer, Stop> {
    if !budget.spend(user_id) {
        return Err(Stop::Later);
    }
    let (github, token) = (github.clone(), token.to_string());
    let answer = tokio::task::spawn_blocking(move || github.request(&token, &path, body.as_ref()))
        .await
        .map_err(|_| Stop::Later)?
        .map_err(|e| {
            eprintln!("GitHub request failed: {e}");
            Stop::Later
        })?;
    match answer.status {
        401 => Err(Stop::Revoked),
        // Rate limits (403/429) and GitHub's own failures.
        403 | 429 | 500.. => Err(Stop::Later),
        _ => Ok(answer),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_names() {
        assert!(is_repo_name("Sisyphus"));
        assert!(is_repo_name("my-app_2.0"));
        assert!(!is_repo_name(".."));
        assert!(!is_repo_name("my app"));
        assert!(!is_repo_name("a/b"));
        assert!(!is_repo_name(""));
    }

    #[test]
    fn basic_auth_is_base64() {
        assert_eq!(
            basic_auth("Aladdin", "open sesame"),
            "QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
        assert_eq!(basic_auth("a", "b"), "YTpi");
    }

    #[test]
    fn contributed_repos() {
        let body = serde_json::json!({"data": {"user": {"repositoriesContributedTo": {"nodes": [
            {"name": "game", "url": "https://github.com/friend/game", "isPrivate": false},
            {"name": "secret", "url": "https://github.com/friend/secret", "isPrivate": true},
            {"name": "odd", "url": "javascript:x", "isPrivate": false}
        ]}}}});
        assert_eq!(
            contributed(&body),
            vec![(
                "game".to_string(),
                "https://github.com/friend/game".to_string()
            )]
        );
        assert!(contributed(&serde_json::json!({"errors": []})).is_empty());
    }

    #[test]
    fn budget_runs_out() {
        let mut budget = Budget::default();
        for _ in 0..DAILY_BUDGET {
            assert!(budget.spend(1));
        }
        assert!(!budget.spend(1));
        assert!(budget.spend(2));
    }
}
