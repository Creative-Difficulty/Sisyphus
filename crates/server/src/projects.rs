//! `POST /api/projects/settings`: a project's links, set on the website
//! (decided 2026-11-23). `{ "name", "repo_url": string | null, "demo_url":
//! string | null }`; `Origin` must match `PUBLIC_URL`, and the project must
//! be one of the user's (a folder name their sessions reported).
//!
//! Links must be `http(s)://` URLs. Changing the repository (or clearing
//! it) marks it as the user's choice, which automatic matching on GitHub
//! (`github.rs`) never overrides.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use sqlx::Row;

use crate::AppState;
use crate::auth::{self, from_this_site};

#[derive(Deserialize)]
pub struct ProjectSettings {
    name: String,
    repo_url: Option<String>,
    demo_url: Option<String>,
}

/// Longest link accepted.
const MAX_URL: usize = 500;

pub async fn save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(settings): Json<ProjectSettings>,
) -> StatusCode {
    if !from_this_site(&state, &headers) {
        return StatusCode::FORBIDDEN;
    }
    let Some(user) = auth::current_user(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED;
    };
    let (Ok(repo_url), Ok(demo_url)) = (link(settings.repo_url), link(settings.demo_url)) else {
        return StatusCode::BAD_REQUEST;
    };
    let result = async {
        let exists: bool = sqlx::query(
            "SELECT EXISTS (SELECT 1 FROM agent_sessions WHERE user_id = $1 AND project = $2) AS ok",
        )
        .bind(user.id)
        .bind(&settings.name)
        .fetch_one(&state.db)
        .await?
        .get("ok");
        if !exists {
            return Ok(StatusCode::NOT_FOUND);
        }
        sqlx::query(
            "INSERT INTO projects (user_id, name, repo_url, repo_manual, demo_url)
             VALUES ($1, $2, $3, true, $4)
             ON CONFLICT (user_id, name) DO UPDATE SET
                 repo_manual = projects.repo_manual
                     OR projects.repo_url IS DISTINCT FROM EXCLUDED.repo_url,
                 repo_url = EXCLUDED.repo_url,
                 demo_url = EXCLUDED.demo_url",
        )
        .bind(user.id)
        .bind(&settings.name)
        .bind(&repo_url)
        .bind(&demo_url)
        .execute(&state.db)
        .await?;
        Ok::<_, sqlx::Error>(StatusCode::NO_CONTENT)
    }
    .await;
    result.unwrap_or_else(|e| {
        eprintln!("saving project settings failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

/// A link as stored: `None` when empty, an error unless it's a plain
/// `http(s)://host…` URL.
fn link(value: Option<String>) -> Result<Option<String>, ()> {
    let Some(value) = value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    else {
        return Ok(None);
    };
    let rest = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .ok_or(())?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let fine = value.len() <= MAX_URL
        && !host.is_empty()
        && !value
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '<' | '>' | '\\'));
    if fine { Ok(Some(value)) } else { Err(()) }
}

#[cfg(test)]
mod tests {
    use super::link;

    #[test]
    fn links() {
        let ok = |v: &str| link(Some(v.to_string()));
        assert_eq!(ok("  "), Ok(None));
        assert_eq!(link(None), Ok(None));
        assert_eq!(
            ok(" https://github.com/a/b "),
            Ok(Some("https://github.com/a/b".to_string()))
        );
        assert_eq!(
            ok("http://localhost:3000"),
            Ok(Some("http://localhost:3000".to_string()))
        );
        assert!(ok("javascript:alert(1)").is_err());
        assert!(ok("https://").is_err());
        assert!(ok("https:///path").is_err());
        assert!(ok("https://a b").is_err());
        assert!(ok("ftp://x").is_err());
        assert!(ok(&format!("https://x/{}", "a".repeat(500))).is_err());
    }
}
