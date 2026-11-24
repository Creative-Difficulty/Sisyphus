//! Downloads and app updates, from the newest GitHub release of the app.
//!
//! - `GET /download/{platform}`: redirects to that platform's file of the
//!   newest release (`windows`, `macos`, `linux-x86_64`, `linux-aarch64`),
//!   or to the release's page when it has none.
//! - `GET /api/releases/latest`: `{ version, platforms }`, for the website's
//!   download buttons.
//! - `GET /api/app/update`: the release's signed update manifest
//!   (`update.json` and `update.json.sig`, made by the release workflow),
//!   with a download URL for each file. The app checks the signature
//!   against the release key built into it, so this server (or GitHub)
//!   can't hand it anything the release key didn't sign.
//!
//! The release is looked up through GitHub's API at most every
//! [`CACHE_FOR`] (a failed lookup is retried after [`RETRY_AFTER`]), so
//! the website and every app share one request. `RELEASES_REPO` (default
//! `Creative-Difficulty/Sisyphus`) names the repository; `GITHUB_API_URL`
//! points at a fake GitHub for tests (https, or http on localhost).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::AppState;

const DEFAULT_REPO: &str = "Creative-Difficulty/Sisyphus";
const CACHE_FOR: Duration = Duration::from_secs(300);
const RETRY_AFTER: Duration = Duration::from_secs(60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
/// The update manifest is a few hundred bytes.
const MANIFEST_LIMIT: u64 = 64 * 1024;

/// The platforms the website offers, each with the file names it takes,
/// best first. The later patterns match releases from before the
/// installers (v0.1.0: `sisyphus-v0.1.0-windows-x86_64.zip`, …).
type FileMatcher = fn(&str) -> bool;
const PLATFORMS: &[(&str, &[FileMatcher])] = &[
    (
        "windows",
        &[
            |n| n.ends_with("-setup.exe"),
            |n| n.contains("windows-x86_64") && n.ends_with(".zip"),
        ],
    ),
    (
        "macos",
        &[
            |n| n.ends_with(".dmg"),
            |n| n.contains("macos-aarch64") && n.ends_with(".tar.gz"),
        ],
    ),
    (
        "linux-x86_64",
        &[
            |n| n.contains("linux-x86_64") && n.ends_with(".AppImage"),
            |n| n.contains("linux-x86_64") && n.ends_with(".tar.gz"),
        ],
    ),
    (
        "linux-aarch64",
        &[
            |n| n.contains("linux-aarch64") && n.ends_with(".AppImage"),
            |n| n.contains("linux-aarch64") && n.ends_with(".tar.gz"),
        ],
    ),
];

pub struct Releases {
    repo: String,
    api_url: String,
    http: ureq::Agent,
    cache: tokio::sync::Mutex<Option<Cached>>,
}

struct Cached {
    at: Instant,
    /// `None`: the lookup failed or there is no release.
    latest: Option<Arc<Latest>>,
}

/// The newest release.
#[derive(Debug)]
pub struct Latest {
    /// Without the `v`.
    pub version: String,
    /// The release's page on GitHub.
    pub page: String,
    pub assets: Vec<Asset>,
    /// `update.json` and its signature (hex), when the release has them.
    pub update: Option<(String, String)>,
}

#[derive(Debug)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

impl Releases {
    pub fn from_env() -> Self {
        let env = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
        let mut api_url = env("GITHUB_API_URL")
            .unwrap_or_else(|| "https://api.github.com".to_string())
            .trim_end_matches('/')
            .to_string();
        if !is_allowed_url(&api_url) {
            eprintln!("warning: GITHUB_API_URL must be https, using api.github.com for releases");
            api_url = "https://api.github.com".to_string();
        }
        Self {
            repo: env("RELEASES_REPO").unwrap_or_else(|| DEFAULT_REPO.to_string()),
            api_url,
            http: ureq::Agent::config_builder()
                .timeout_global(Some(HTTP_TIMEOUT))
                .http_status_as_error(false)
                .user_agent("sisyphus-server")
                .build()
                .into(),
            cache: tokio::sync::Mutex::new(None),
        }
    }

    /// The newest release, from the cache or GitHub.
    pub async fn latest(self: &Arc<Self>) -> Option<Arc<Latest>> {
        let mut cache = self.cache.lock().await;
        if let Some(cached) = cache.as_ref() {
            let fresh_for = if cached.latest.is_some() {
                CACHE_FOR
            } else {
                RETRY_AFTER
            };
            if cached.at.elapsed() < fresh_for {
                return cached.latest.clone();
            }
        }
        let this = self.clone();
        let latest = match tokio::task::spawn_blocking(move || this.fetch()).await {
            Ok(Ok(latest)) => Some(Arc::new(latest)),
            Ok(Err(e)) => {
                eprintln!("looking up the newest release failed: {e}");
                // Keep serving the last release we knew, if any.
                cache.as_ref().and_then(|c| c.latest.clone())
            }
            Err(_) => None,
        };
        *cache = Some(Cached {
            at: Instant::now(),
            latest: latest.clone(),
        });
        latest
    }

    fn fetch(&self) -> Result<Latest, String> {
        let url = format!("{}/repos/{}/releases/latest", self.api_url, self.repo);
        let mut response = self
            .http
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .call()
            .map_err(|e| format!("request to {url} failed: {e}"))?;
        if response.status() != 200 {
            return Err(format!("{url} answered {}", response.status()));
        }
        let body: serde_json::Value = response
            .body_mut()
            .read_json()
            .map_err(|e| format!("invalid JSON from {url}: {e}"))?;
        let mut latest = parse_release(&body).ok_or("the release has no tag")?;
        let asset_url = |name: &str| {
            latest
                .assets
                .iter()
                .find(|a| a.name == name)
                .map(|a| a.url.clone())
        };
        if let (Some(manifest), Some(signature)) =
            (asset_url("update.json"), asset_url("update.json.sig"))
        {
            let manifest = self.text(&manifest)?;
            let signature = self.text(&signature)?.trim().to_string();
            latest.update = Some((manifest, signature));
        }
        Ok(latest)
    }

    /// A small text file (GitHub redirects asset downloads; ureq follows).
    fn text(&self, url: &str) -> Result<String, String> {
        let mut response = self
            .http
            .get(url)
            .call()
            .map_err(|e| format!("request to {url} failed: {e}"))?;
        if response.status() != 200 {
            return Err(format!("{url} answered {}", response.status()));
        }
        response
            .body_mut()
            .with_config()
            .limit(MANIFEST_LIMIT)
            .read_to_string()
            .map_err(|e| format!("reading {url} failed: {e}"))
    }
}

/// The release's version, page and downloadable files. Only https (or
/// loopback http, for tests) URLs are kept.
fn parse_release(body: &serde_json::Value) -> Option<Latest> {
    let tag = body["tag_name"].as_str()?;
    let assets = body["assets"]
        .as_array()
        .map(|assets| {
            assets
                .iter()
                .filter_map(|a| {
                    let name = a["name"].as_str()?;
                    let url = a["browser_download_url"].as_str()?;
                    is_allowed_url(url).then(|| Asset {
                        name: name.to_string(),
                        url: url.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Latest {
        version: tag.strip_prefix('v').unwrap_or(tag).to_string(),
        page: body["html_url"]
            .as_str()
            .filter(|u| is_allowed_url(u))
            .unwrap_or_default()
            .to_string(),
        assets,
        update: None,
    })
}

fn is_allowed_url(url: &str) -> bool {
    url.starts_with("https://")
        || ["http://localhost:", "http://127.0.0.1:"]
            .iter()
            .any(|p| url.starts_with(p))
}

/// The file for `platform`, best match first.
fn asset_for<'a>(latest: &'a Latest, platform: &str) -> Option<&'a Asset> {
    let (_, patterns) = PLATFORMS.iter().find(|(p, _)| *p == platform)?;
    patterns
        .iter()
        .find_map(|matches| latest.assets.iter().find(|a| matches(&a.name)))
}

/// `GET /download/{platform}`.
pub async fn download(State(state): State<AppState>, Path(platform): Path<String>) -> Response {
    if !PLATFORMS.iter().any(|(p, _)| *p == platform) {
        return (StatusCode::NOT_FOUND, "Unknown platform").into_response();
    }
    let Some(latest) = state.releases.latest().await else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "No download is available right now. Try again in a minute.",
        )
            .into_response();
    };
    let target = match asset_for(&latest, &platform) {
        Some(asset) => asset.url.as_str(),
        None if !latest.page.is_empty() => latest.page.as_str(),
        None => return (StatusCode::NOT_FOUND, "No download for this system yet").into_response(),
    };
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, target.to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
    )
        .into_response()
}

#[derive(Serialize)]
struct LatestView {
    version: String,
    platforms: Vec<&'static str>,
}

/// `GET /api/releases/latest`.
pub async fn latest(State(state): State<AppState>) -> Response {
    match state.releases.latest().await {
        Some(latest) => Json(LatestView {
            version: latest.version.clone(),
            platforms: PLATFORMS
                .iter()
                .map(|(p, _)| *p)
                .filter(|p| asset_for(&latest, p).is_some())
                .collect(),
        })
        .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Serialize)]
struct UpdateView {
    version: String,
    /// `update.json`, exactly as signed.
    manifest: String,
    /// Hex ed25519 signature over `manifest`'s bytes.
    signature: String,
    /// Download URL of each file of the release.
    files: BTreeMap<String, String>,
}

/// `GET /api/app/update`. Public: it only says what the release says.
pub async fn update(State(state): State<AppState>) -> Response {
    let Some(latest) = state.releases.latest().await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some((manifest, signature)) = latest.update.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    Json(UpdateView {
        version: latest.version.clone(),
        manifest,
        signature,
        files: latest
            .assets
            .iter()
            .map(|a| (a.name.clone(), a.url.clone()))
            .collect(),
    })
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(names: &[&str]) -> Latest {
        let body = serde_json::json!({
            "tag_name": "v0.2.0",
            "html_url": "https://github.com/o/r/releases/tag/v0.2.0",
            "assets": names.iter().map(|n| serde_json::json!({
                "name": n,
                "browser_download_url": format!("https://github.com/o/r/releases/download/v0.2.0/{n}"),
            })).collect::<Vec<_>>(),
        });
        parse_release(&body).unwrap()
    }

    #[test]
    fn installers_win_over_archives() {
        let latest = release(&[
            "SHA256SUMS.txt",
            "sisyphus-linux-x86_64.tar.gz",
            "Sisyphus-linux-x86_64.AppImage",
            "Sisyphus-linux-aarch64.AppImage",
            "Sisyphus-macos-universal.app.tar.gz",
            "Sisyphus-macos-universal.dmg",
            "Sisyphus-windows-x86_64-setup.exe",
            "update.json",
        ]);
        assert_eq!(latest.version, "0.2.0");
        let name = |p| asset_for(&latest, p).map(|a| a.name.as_str());
        assert_eq!(name("windows"), Some("Sisyphus-windows-x86_64-setup.exe"));
        assert_eq!(name("macos"), Some("Sisyphus-macos-universal.dmg"));
        assert_eq!(name("linux-x86_64"), Some("Sisyphus-linux-x86_64.AppImage"));
        assert_eq!(
            name("linux-aarch64"),
            Some("Sisyphus-linux-aarch64.AppImage")
        );
    }

    #[test]
    fn old_releases_fall_back_to_archives() {
        let latest = release(&[
            "sisyphus-v0.1.0-linux-aarch64.tar.gz",
            "sisyphus-v0.1.0-linux-x86_64.tar.gz",
            "sisyphus-v0.1.0-macos-aarch64.tar.gz",
            "sisyphus-v0.1.0-macos-x86_64.tar.gz",
            "sisyphus-v0.1.0-windows-x86_64.zip",
        ]);
        let name = |p| asset_for(&latest, p).map(|a| a.name.as_str());
        assert_eq!(name("windows"), Some("sisyphus-v0.1.0-windows-x86_64.zip"));
        assert_eq!(name("macos"), Some("sisyphus-v0.1.0-macos-aarch64.tar.gz"));
        assert_eq!(
            name("linux-x86_64"),
            Some("sisyphus-v0.1.0-linux-x86_64.tar.gz")
        );
        assert_eq!(name("nonsense"), None);
    }

    #[test]
    fn only_https_links() {
        let body = serde_json::json!({
            "tag_name": "v1.0.0",
            "html_url": "javascript:alert(1)",
            "assets": [
                { "name": "a-setup.exe", "browser_download_url": "http://evil.example/a-setup.exe" },
            ],
        });
        let latest = parse_release(&body).unwrap();
        assert!(latest.assets.is_empty());
        assert!(latest.page.is_empty());
    }
}
