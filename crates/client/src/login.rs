//! Logging the app in with Hack Club, through the Sisyphus server.
//!
//! The app can't keep an OAuth client secret (it's open source), so the
//! server does the Hack Club login and hands the app a one-time code:
//!
//! 1. Listen on a random port on `127.0.0.1`, make a random `verifier`, and
//!    open the browser at `<server>/auth/app?port=…&challenge=…&device=…`
//!    (`challenge` = SHA-256 of the verifier, `device` = our public key).
//! 2. The user logs in with Hack Club in the browser.
//! 3. The server sends the browser to `http://127.0.0.1:<port>/done?code=…`.
//! 4. We post the code, the verifier and a device-key signature over the
//!    code to `<server>/auth/app/token`, which links this device to the
//!    account. Only this process knows the verifier and only this device has
//!    the key, so an intercepted code is useless.
//!
//! Blocking; run it off the UI thread. While it waits, the login page can
//! be opened again (`on_url` hands it out), for example when the first
//! browser tab went to the wrong place.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use ed25519_dalek::Signer;
use sha2::{Digest, Sha256};
use sisyphus_common::api::{AppTokenRequest, AppTokenResponse, device_link_message};
use sisyphus_common::hex::Hex;

use crate::device;

/// How long to wait for the user to finish logging in in the browser.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// Log in and link this device. Returns the account's display name.
/// `on_url` gets the login page's address before the browser opens it; the
/// same address can be opened again with [`open_browser`] while this waits.
pub fn login(server_url: &str, on_url: impl FnOnce(&str)) -> Result<Option<String>, String> {
    let server_url = server_url.trim_end_matches('/');
    if !is_safe_server(server_url) {
        return Err("The server address must use https.".to_string());
    }
    let key =
        device::load_or_create().map_err(|e| format!("Could not load the device key: {e}"))?;
    let device_id = hex::encode(key.verifying_key().to_bytes());

    let verifier = random_hex();
    let challenge = hex::encode(Sha256::digest(verifier.as_bytes()));

    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("Could not start the login listener: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Could not start the login listener: {e}"))?
        .port();

    let url = format!("{server_url}/auth/app?port={port}&challenge={challenge}&device={device_id}");
    on_url(&url);
    open_browser(&url)?;
    let code = wait_for_code(&listener)?;

    let signature = key.sign(&device_link_message(&code));
    let request = AppTokenRequest {
        code,
        verifier,
        device: Hex(key.verifying_key().to_bytes()),
        signature: Hex(signature.to_bytes()),
    };
    let http: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = http
        .post(&format!("{server_url}/auth/app/token"))
        .send_json(&request)
        .map_err(|e| format!("Could not reach the server: {e}"))?;
    match response.status().as_u16() {
        200 => response
            .body_mut()
            .read_json::<AppTokenResponse>()
            .map(|r| r.name)
            .map_err(|e| format!("Unexpected answer from the server: {e}")),
        409 => Err("This device is linked to another account.".to_string()),
        status => Err(format!("Login failed (server answered {status}).")),
    }
}

/// Only send login traffic over https, except to a server on this machine.
pub fn is_safe_server(url: &str) -> bool {
    let local = ["http://localhost", "http://127.0.0.1", "http://[::1]"];
    url.starts_with("https://")
        || local.iter().any(|prefix| {
            url == *prefix
                || url.starts_with(&format!("{prefix}:"))
                || url.starts_with(&format!("{prefix}/"))
        })
}

/// Wait for the browser to arrive at `/done?code=…`. Other requests (for
/// example a favicon) get a 404 and are ignored.
fn wait_for_code(listener: &TcpListener) -> Result<String, String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("Login listener failed: {e}"))?;
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    while Instant::now() < deadline {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(code) = handle(stream) {
                    return Ok(code);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(format!("Login listener failed: {e}")),
        }
    }
    Err("Login timed out. Please try again.".to_string())
}

fn handle(mut stream: TcpStream) -> Option<String> {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    // Only the request line matters; read at most 4 KiB of it.
    let mut request_line = String::new();
    BufReader::new((&stream).take(4096))
        .read_line(&mut request_line)
        .ok()?;
    let code = parse_code(&request_line);
    let (status, body) = match &code {
        Some(_) => ("200 OK", DONE_PAGE),
        None => ("404 Not Found", ""),
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\
         Referrer-Policy: no-referrer\r\n\r\n{body}",
        body.len()
    );
    code
}

/// `GET /done?code=<64 hex chars> HTTP/1.1` → the code.
fn parse_code(request_line: &str) -> Option<String> {
    let target = request_line.strip_prefix("GET ")?.split(' ').next()?;
    let query = target.strip_prefix("/done?")?;
    let code = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("code="))?;
    (code.len() == 64 && code.bytes().all(|b| b.is_ascii_hexdigit())).then(|| code.to_string())
}

fn random_hex() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the OS random number generator is available");
    hex::encode(bytes)
}

/// Open `url` in the default browser. The URL is passed as a single
/// argument, never through a shell.
pub fn open_browser(url: &str) -> Result<(), String> {
    let mut command = if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    command
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open the browser: {e}"))
}

const DONE_PAGE: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<title>Sisyphus</title><style>body{background:#ffd6e7;color:#4a1942;\
font-family:system-ui,sans-serif;text-align:center;padding-top:20vh}\
@media (prefers-color-scheme:dark){body{background:#2a1022;color:#ffe0ee}}</style>\
</head><body><h1>Sisyphus</h1><p>You're logged in. You can close this tab.</p></body></html>";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_code() {
        let code = "ab".repeat(32);
        assert_eq!(
            parse_code(&format!("GET /done?code={code} HTTP/1.1\r\n")),
            Some(code.clone())
        );
        assert_eq!(parse_code("GET /favicon.ico HTTP/1.1\r\n"), None);
        assert_eq!(parse_code("GET /done?code=short HTTP/1.1\r\n"), None);
        assert_eq!(
            parse_code(&format!("POST /done?code={code} HTTP/1.1\r\n")),
            None
        );
    }

    #[test]
    fn only_https_or_local_servers() {
        assert!(is_safe_server("https://sisyphus.example"));
        assert!(is_safe_server("http://localhost:8080"));
        assert!(is_safe_server("http://127.0.0.1:18080"));
        assert!(!is_safe_server("http://sisyphus.example"));
        assert!(!is_safe_server("http://localhost.evil.example"));
    }
}
