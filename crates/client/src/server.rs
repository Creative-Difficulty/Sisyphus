//! Requests the app makes to the Sisyphus server, signed with the device
//! key (see `sisyphus_common::api::request_message`). The server only
//! answers devices linked to an account. Blocking; the window runs these on
//! worker threads.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ed25519_dalek::Signer;
use sisyphus_common::api::{HEADER_DEVICE, HEADER_SIGNATURE, HEADER_TIME, Today, request_message};

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// Credited time since the start of today, local time.
pub fn today(server: &str) -> Result<u64, String> {
    let path = format!("/api/app/today?start_ms={}", local_day_start_ms(now_ms()));
    let mut response = request(server, "GET", &path)?;
    match response.status().as_u16() {
        200 => response
            .body_mut()
            .read_json::<Today>()
            .map(|t| t.tracked_ms)
            .map_err(|e| format!("Unexpected answer from the server: {e}")),
        status => Err(refused(status)),
    }
}

/// Unlink this device from its account on the server. A device that isn't
/// linked (anymore) counts as logged out.
pub fn logout(server: &str) -> Result<(), String> {
    let response = request(server, "POST", "/api/app/logout")?;
    match response.status().as_u16() {
        200..=299 | 403 => Ok(()),
        status => Err(refused(status)),
    }
}

fn refused(status: u16) -> String {
    match status {
        401 => "The server refused this device. Is the computer's clock right?".to_string(),
        403 => "This device isn't logged in on the server.".to_string(),
        status => format!("The server answered {status}."),
    }
}

/// A signed request without a body.
fn request(
    server: &str,
    method: &str,
    path: &str,
) -> Result<ureq::http::Response<ureq::Body>, String> {
    if !crate::login::is_safe_server(server) {
        return Err("Only https servers are allowed.".to_string());
    }
    let key = crate::device::load_or_create().map_err(|e| e.to_string())?;
    let time = now_ms();
    let signature = key.sign(&request_message(method, path, time, &[]));
    let http: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
    let url = format!("{server}{path}");
    let device = hex::encode(key.verifying_key().to_bytes());
    let signature = hex::encode(signature.to_bytes());
    let time = time.to_string();
    let result = match method {
        "POST" => http
            .post(&url)
            .header(HEADER_DEVICE, &device)
            .header(HEADER_TIME, &time)
            .header(HEADER_SIGNATURE, &signature)
            .send_empty(),
        _ => http
            .get(&url)
            .header(HEADER_DEVICE, &device)
            .header(HEADER_TIME, &time)
            .header(HEADER_SIGNATURE, &signature)
            .call(),
    };
    result.map_err(|e| format!("Could not reach the server: {e}"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Midnight at the start of the local day containing `now_ms`.
pub fn local_day_start_ms(now_ms: u64) -> u64 {
    const DAY_MS: i64 = 24 * 60 * 60 * 1000;
    let offset = utc_offset_ms(now_ms);
    let local = now_ms as i64 + offset;
    (local - local.rem_euclid(DAY_MS) - offset).max(0) as u64
}

/// The local time zone's offset from UTC at `now_ms`, from the OS.
#[cfg(unix)]
fn utc_offset_ms(now_ms: u64) -> i64 {
    let seconds = (now_ms / 1000) as libc::time_t;
    // SAFETY: `tm` is plain data; localtime_r fills it from `seconds`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&seconds, &mut tm) }.is_null() {
        return 0;
    }
    tm.tm_gmtoff as i64 * 1000
}

#[cfg(windows)]
fn utc_offset_ms(_now_ms: u64) -> i64 {
    use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
    // SAFETY: TIME_ZONE_INFORMATION is plain data; the call fills it.
    let mut info: TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
    let kind = unsafe { GetTimeZoneInformation(&mut info) };
    // UTC = local + bias, in minutes. 2: daylight saving time is in effect.
    let bias = info.Bias
        + if kind == 2 {
            info.DaylightBias
        } else {
            info.StandardBias
        };
    -(bias as i64) * 60_000
}

#[cfg(not(any(unix, windows)))]
fn utc_offset_ms(_now_ms: u64) -> i64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_start_is_a_local_midnight_before_now() {
        let now = 1_795_259_280_000;
        let start = local_day_start_ms(now);
        assert!(start <= now && now - start < 24 * 60 * 60 * 1000);
        let local = start as i64 + utc_offset_ms(start);
        assert_eq!(local.rem_euclid(24 * 60 * 60 * 1000), 0);
    }
}
