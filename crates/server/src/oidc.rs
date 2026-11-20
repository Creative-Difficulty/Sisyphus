//! Login with Hack Club Auth (OpenID Connect, authorization code flow).
//!
//! The server is the only OAuth client: Hack Club Auth requires the client
//! secret on every token request, so the desktop app logs in through the
//! server instead (see `auth.rs`).
//!
//! Endpoints come from the provider's discovery document. The ID token is
//! verified fully (RS256 signature against the provider's keys, issuer,
//! audience, expiry and nonce). Hack Club tokens are used once to learn who
//! logged in and are never stored.

use std::time::Duration;

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::crypto::{constant_time_eq, url_encode};

/// What Sisyphus asks Hack Club Auth for: the stable user id, the name for
/// profiles, and the Slack id. Email and verification status are
/// deliberately not requested.
const SCOPES: &str = "openid profile slack_id";

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

pub struct HackClubAuth {
    client_id: String,
    client_secret: String,
    /// e.g. `https://auth.hackclub.com`; also the expected token issuer.
    base_url: String,
    redirect_uri: String,
    http: ureq::Agent,
    discovery: RwLock<Option<Discovery>>,
    keys: RwLock<Option<JwkSet>>,
}

#[derive(Debug, Clone, Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Claims {
    sub: String,
    nonce: Option<String>,
    name: Option<String>,
    slack_id: Option<String>,
}

/// Who logged in, from the verified ID token.
#[derive(Debug, Clone)]
pub struct Identity {
    pub hackclub_id: String,
    pub name: Option<String>,
    pub slack_id: Option<String>,
}

impl HackClubAuth {
    /// Reads `HACKCLUB_CLIENT_ID`, `HACKCLUB_CLIENT_SECRET` and optionally
    /// `HACKCLUB_AUTH_URL`. `None` if login isn't configured.
    pub fn from_env(public_url: &str) -> Option<Self> {
        let client_id = non_empty_env("HACKCLUB_CLIENT_ID")?;
        let client_secret = non_empty_env("HACKCLUB_CLIENT_SECRET")?;
        let base_url = non_empty_env("HACKCLUB_AUTH_URL")
            .unwrap_or_else(|| "https://auth.hackclub.com".to_string())
            .trim_end_matches('/')
            .to_string();
        let http: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(HTTP_TIMEOUT))
            .http_status_as_error(false)
            .build()
            .into();
        Some(Self {
            client_id,
            client_secret,
            redirect_uri: format!("{public_url}/auth/callback"),
            base_url,
            http,
            discovery: RwLock::new(None),
            keys: RwLock::new(None),
        })
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// Where to send the browser to log in.
    pub async fn authorize_url(
        &self,
        state: &str,
        nonce: &str,
        pkce_challenge: &str,
    ) -> Result<String, String> {
        let discovery = self.discovery().await?;
        Ok(format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&nonce={}\
             &code_challenge={}&code_challenge_method=S256",
            discovery.authorization_endpoint,
            url_encode(&self.client_id),
            url_encode(&self.redirect_uri),
            url_encode(SCOPES),
            url_encode(state),
            url_encode(nonce),
            url_encode(pkce_challenge),
        ))
    }

    /// Exchange the authorization code and verify the ID token.
    pub async fn finish_login(
        &self,
        code: &str,
        pkce_verifier: &str,
        expected_nonce: &str,
    ) -> Result<Identity, String> {
        let discovery = self.discovery().await?;
        let form = [
            ("grant_type".to_string(), "authorization_code".to_string()),
            ("code".to_string(), code.to_string()),
            ("redirect_uri".to_string(), self.redirect_uri.clone()),
            ("client_id".to_string(), self.client_id.clone()),
            ("client_secret".to_string(), self.client_secret.clone()),
            ("code_verifier".to_string(), pkce_verifier.to_string()),
        ];
        let http = self.http.clone();
        let token_endpoint = discovery.token_endpoint.clone();
        let response: TokenResponse = blocking(move || {
            let mut response = http
                .post(&token_endpoint)
                .send_form(form.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                .map_err(|e| format!("token request failed: {e}"))?;
            if !response.status().is_success() {
                return Err(format!("token endpoint answered {}", response.status()));
            }
            response
                .body_mut()
                .read_json()
                .map_err(|e| format!("invalid token response: {e}"))
        })
        .await?;
        let id_token = response.id_token.ok_or("token response has no id_token")?;
        let claims = self.verify(&id_token, &discovery).await?;

        match &claims.nonce {
            Some(nonce) if constant_time_eq(nonce, expected_nonce) => {}
            _ => return Err("ID token nonce doesn't match".to_string()),
        }
        Ok(Identity {
            hackclub_id: claims.sub,
            name: claims.name,
            slack_id: claims.slack_id,
        })
    }

    async fn verify(&self, id_token: &str, discovery: &Discovery) -> Result<Claims, String> {
        let header =
            jsonwebtoken::decode_header(id_token).map_err(|e| format!("bad ID token: {e}"))?;
        if header.alg != Algorithm::RS256 {
            return Err(format!("unexpected ID token algorithm {:?}", header.alg));
        }
        let kid = header.kid.ok_or("ID token has no key id")?;
        let key = self.signing_key(&kid, discovery).await?;

        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&[&discovery.issuer]);
        validation.set_audience(&[&self.client_id]);
        validation.set_required_spec_claims(&["exp", "iat", "iss", "aud", "sub"]);
        validation.leeway = 60;
        jsonwebtoken::decode::<Claims>(id_token, &key, &validation)
            .map(|data| data.claims)
            .map_err(|e| format!("ID token rejected: {e}"))
    }

    /// The provider's key with this id. Keys are cached and fetched again
    /// once when an unknown id shows up (key rotation).
    async fn signing_key(&self, kid: &str, discovery: &Discovery) -> Result<DecodingKey, String> {
        if let Some(keys) = self.keys.read().await.as_ref()
            && let Some(jwk) = keys.find(kid)
        {
            return DecodingKey::from_jwk(jwk).map_err(|e| format!("bad signing key: {e}"));
        }
        let http = self.http.clone();
        let jwks_uri = discovery.jwks_uri.clone();
        let keys: JwkSet = blocking(move || fetch_json(&http, &jwks_uri)).await?;
        let key = keys
            .find(kid)
            .ok_or_else(|| format!("unknown signing key {kid}"))
            .and_then(|jwk| {
                DecodingKey::from_jwk(jwk).map_err(|e| format!("bad signing key: {e}"))
            });
        *self.keys.write().await = Some(keys);
        key
    }

    async fn discovery(&self) -> Result<Discovery, String> {
        if let Some(discovery) = self.discovery.read().await.clone() {
            return Ok(discovery);
        }
        let http = self.http.clone();
        let url = format!("{}/.well-known/openid-configuration", self.base_url);
        let discovery: Discovery = blocking(move || fetch_json(&http, &url)).await?;
        // The document must describe the provider we were configured with.
        if discovery.issuer.trim_end_matches('/') != self.base_url {
            return Err(format!(
                "discovery issuer {} doesn't match {}",
                discovery.issuer, self.base_url
            ));
        }
        for endpoint in [
            &discovery.authorization_endpoint,
            &discovery.token_endpoint,
            &discovery.jwks_uri,
        ] {
            // Plain http only for a provider on this machine (local testing).
            let local = is_loopback_http(&self.base_url) && endpoint.starts_with(&self.base_url);
            if !endpoint.starts_with("https://") && !local {
                return Err(format!("insecure endpoint in discovery: {endpoint}"));
            }
        }
        *self.discovery.write().await = Some(discovery.clone());
        Ok(discovery)
    }
}

fn fetch_json<T: serde::de::DeserializeOwned>(http: &ureq::Agent, url: &str) -> Result<T, String> {
    let mut response = http
        .get(url)
        .call()
        .map_err(|e| format!("request to {url} failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("{url} answered {}", response.status()));
    }
    response
        .body_mut()
        .read_json()
        .map_err(|e| format!("invalid JSON from {url}: {e}"))
}

/// Run blocking HTTP off the async runtime.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| format!("worker failed: {e}"))?
}

/// `http://localhost[:port]` or `http://127.0.0.1[:port]`, nothing else.
fn is_loopback_http(url: &str) -> bool {
    ["http://localhost", "http://127.0.0.1"].iter().any(|host| {
        url.strip_prefix(host)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(':'))
    })
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_http_only_on_this_machine() {
        assert!(is_loopback_http("http://127.0.0.1:18090"));
        assert!(is_loopback_http("http://localhost"));
        assert!(!is_loopback_http("http://localhost.evil.example"));
        assert!(!is_loopback_http("http://auth.hackclub.com"));
        assert!(!is_loopback_http("https://auth.hackclub.com"));
    }
}
