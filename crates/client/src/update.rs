//! Over-the-air updates (decided 2026-11-24).
//!
//! Only installed copies update themselves: the Windows installer's
//! (`unins000.exe` next to the program), a macOS `.app` bundle, and a Linux
//! AppImage (`$APPIMAGE`). Development builds and the plain Linux archive
//! never do.
//!
//! 1. [`Updater`] asks the server (`GET /api/app/update`) shortly after
//!    start and then every [`CHECK_EVERY`]. The server passes on the newest
//!    release's `update.json`, its signature and the release's download
//!    URLs.
//! 2. The signature must check out against [`RELEASE_PUBLIC_KEY`], whose
//!    private half only the release workflow has (a GitHub Actions secret),
//!    so neither the server nor GitHub can hand out anything the release
//!    key didn't sign. Only a newer version than this one is taken, so an
//!    old signed manifest can't roll the app back.
//! 3. This platform's file is downloaded into `update/` next to the config
//!    and kept only if its size and SHA-256 match the manifest.
//! 4. The window installs it at a quiet moment ([`apply`]): window closed,
//!    no agent working. Then Sisyphus quits and the new version starts in
//!    the tray (`--background`).
//!
//! `update.json` (made by `.github/workflows/release.yml`):
//! `{ "kind": "sisyphus-update/1", "version": "0.2.0", "files": {
//! "windows-x86_64": { "name", "sha256", "size" }, "macos-universal": …,
//! "linux-x86_64": …, "linux-aarch64": … } }`.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config::Config;

/// The release key's public half (ed25519, hex). Its private half is the
/// `RELEASE_SIGNING_KEY` secret of the GitHub repository.
pub const RELEASE_PUBLIC_KEY: &str =
    "6450f7678a3c10c4e9bb04ae54211fb1d659bbad0fb89688ef7ea123cfdf72b5";
/// What `update.json` must say it is.
const MANIFEST_KIND: &str = "sisyphus-update/1";
/// This build's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const FIRST_CHECK_AFTER: Duration = Duration::from_secs(30);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
/// After a failed check or download.
const RETRY_AFTER: Duration = Duration::from_secs(3600);
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// No release file is anywhere near this.
const MAX_SIZE: u64 = 500 * 1024 * 1024;

/// This build's entry in `update.json`.
pub fn platform() -> Option<&'static str> {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("windows-x86_64")
    } else if cfg!(target_os = "macos") {
        Some("macos-universal")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("linux-x86_64")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("linux-aarch64")
    } else {
        None
    }
}

/// How this copy was installed, if it can update itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Install {
    /// Installed by `Sisyphus-…-setup.exe`; an update runs the new setup.
    WindowsSetup,
    /// A macOS app bundle; an update swaps the bundle.
    MacApp(PathBuf),
    /// A Linux AppImage; an update replaces the file.
    AppImage(PathBuf),
}

pub fn install() -> Option<Install> {
    let exe = std::env::current_exe().ok()?;
    if cfg!(target_os = "windows") {
        return exe
            .parent()?
            .join("unins000.exe")
            .is_file()
            .then_some(Install::WindowsSetup);
    }
    if cfg!(target_os = "macos") {
        return app_bundle(&exe).map(Install::MacApp);
    }
    std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .map(Install::AppImage)
}

/// `…/Sisyphus.app` for `…/Sisyphus.app/Contents/MacOS/sisyphus`.
fn app_bundle(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then(|| bundle.to_path_buf())
}

/// Where the installed copy lives must be writable for us to replace it.
fn replaceable(install: &Install) -> bool {
    let dir = match install {
        Install::WindowsSetup => return true,
        Install::MacApp(bundle) | Install::AppImage(bundle) => match bundle.parent() {
            Some(dir) => dir.to_path_buf(),
            None => return false,
        },
    };
    let probe = dir.join(".sisyphus-write-test");
    let ok = fs::File::create(&probe).is_ok();
    let _ = fs::remove_file(&probe);
    ok
}

/// A downloaded, checked update, waiting for a quiet moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ready {
    pub version: String,
    pub file: PathBuf,
    sha256: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Waiting,
    UpToDate,
    Downloading(String),
    Ready(Ready),
    Failed(String),
}

/// Checks for updates on a thread of its own.
pub struct Updater {
    state: Arc<Mutex<State>>,
}

impl Updater {
    /// Start checking, if this copy can update itself.
    pub fn start() -> Option<Self> {
        let install = install()?;
        let platform = platform()?;
        if !replaceable(&install) {
            eprintln!("not updating: can't write where Sisyphus is installed");
            return None;
        }
        let state = Arc::new(Mutex::new(State::Waiting));
        let shared = state.clone();
        std::thread::spawn(move || {
            std::thread::sleep(FIRST_CHECK_AFTER);
            loop {
                let set = |s: State| {
                    if let Ok(mut state) = shared.lock() {
                        *state = s;
                    }
                };
                let server = Config::load().server_url();
                let wait = match check(&server, platform, &set) {
                    Ok(Some(ready)) => {
                        set(State::Ready(ready));
                        // Nothing more to do until the window installs it.
                        return;
                    }
                    Ok(None) => {
                        set(State::UpToDate);
                        CHECK_EVERY
                    }
                    Err(e) => {
                        eprintln!("update check failed: {e}");
                        set(State::Failed(e));
                        RETRY_AFTER
                    }
                };
                std::thread::sleep(wait);
            }
        });
        Some(Self { state })
    }

    pub fn state(&self) -> State {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn ready(&self) -> Option<Ready> {
        match self.state() {
            State::Ready(ready) => Some(ready),
            _ => None,
        }
    }

    /// Installing failed; don't try again in this run.
    pub fn failed(&self, error: String) {
        if let Ok(mut state) = self.state.lock() {
            *state = State::Failed(error);
        }
    }
}

/// The folder downloads go to.
fn update_dir() -> Option<PathBuf> {
    Some(Config::path()?.parent()?.join("update"))
}

/// One check: `Some` once a newer version is downloaded and checked.
fn check(server: &str, platform: &str, set: &dyn Fn(State)) -> Result<Option<Ready>, String> {
    let server = server.trim_end_matches('/');
    if !crate::login::is_safe_server(server) {
        return Err("the server address isn't https".into());
    }
    let http: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(CHECK_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
    let url = format!("{server}/api/app/update");
    let mut response = http
        .get(&url)
        .call()
        .map_err(|e| format!("could not reach the server: {e}"))?;
    match response.status().as_u16() {
        200 => {}
        // No release with an update manifest.
        404 => return Ok(None),
        status => return Err(format!("the server answered {status}")),
    }
    let answer: Value = response
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_json()
        .map_err(|e| format!("invalid answer: {e}"))?;
    let offer = match verify_offer(&answer, platform, &release_key())? {
        Some(offer) => offer,
        None => {
            // Up to date: drop leftovers of earlier updates.
            if let Some(dir) = update_dir() {
                let _ = fs::remove_dir_all(dir);
            }
            return Ok(None);
        }
    };
    let dir = update_dir().ok_or("no config folder")?;
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let file = dir.join(&offer.name);
    if file_matches(&file, offer.size, &offer.sha256) {
        return Ok(Some(offer.ready(file)));
    }
    set(State::Downloading(offer.version.clone()));
    download(&offer, &file)?;
    Ok(Some(offer.ready(file)))
}

fn release_key() -> VerifyingKey {
    let bytes: [u8; 32] = hex::decode(RELEASE_PUBLIC_KEY)
        .ok()
        .and_then(|b| b.try_into().ok())
        .expect("RELEASE_PUBLIC_KEY is 32 bytes of hex");
    VerifyingKey::from_bytes(&bytes).expect("RELEASE_PUBLIC_KEY is a valid key")
}

/// A newer version for this platform, from a signed manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Offer {
    version: String,
    name: String,
    sha256: String,
    size: u64,
    url: String,
}

impl Offer {
    fn ready(self, file: PathBuf) -> Ready {
        Ready {
            version: self.version,
            file,
            sha256: self.sha256,
        }
    }
}

/// Check the server's answer: the manifest's signature, its kind, that its
/// version is newer than this one, and this platform's file and its URL.
/// `Ok(None)` when there's nothing newer.
fn verify_offer(
    answer: &Value,
    platform: &str,
    key: &VerifyingKey,
) -> Result<Option<Offer>, String> {
    let manifest = answer["manifest"].as_str().ok_or("no manifest")?;
    let signature: [u8; 64] = answer["signature"]
        .as_str()
        .and_then(|s| hex::decode(s.trim()).ok())
        .and_then(|b| b.try_into().ok())
        .ok_or("no valid signature")?;
    key.verify_strict(manifest.as_bytes(), &Signature::from_bytes(&signature))
        .map_err(|_| "the update's signature doesn't match the release key")?;
    let manifest: Value =
        serde_json::from_str(manifest).map_err(|e| format!("invalid manifest: {e}"))?;
    if manifest["kind"] != MANIFEST_KIND {
        return Err("unknown manifest kind".into());
    }
    let version = manifest["version"].as_str().ok_or("no version")?;
    if !is_newer(version, VERSION) {
        return Ok(None);
    }
    let entry = &manifest["files"][platform];
    let name = entry["name"].as_str().ok_or("no file for this system")?;
    // Only a plain file name: it becomes a path in the update folder.
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        return Err("bad file name".into());
    }
    let sha256 = entry["sha256"]
        .as_str()
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("no file hash")?
        .to_ascii_lowercase();
    let size = entry["size"]
        .as_u64()
        .filter(|&s| s > 0 && s <= MAX_SIZE)
        .ok_or("bad file size")?;
    let url = answer["files"][name]
        .as_str()
        .filter(|u| crate::login::is_safe_server(u))
        .ok_or("no download link")?;
    Ok(Some(Offer {
        version: version.to_string(),
        name: name.to_string(),
        sha256,
        size,
        url: url.to_string(),
    }))
}

/// `1.2.3` as numbers; anything else (pre-releases too) is `None`.
fn parse_version(v: &str) -> Option<[u64; 3]> {
    let mut parts = v.split('.').map(|p| p.parse::<u64>().ok());
    let version = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(version)
}

fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

fn sha256_of(file: &Path) -> Option<(u64, String)> {
    let mut f = fs::File::open(file).ok()?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        size += n as u64;
    }
    Some((size, hex::encode(hash.finalize())))
}

fn file_matches(file: &Path, size: u64, sha256: &str) -> bool {
    sha256_of(file).is_some_and(|(s, h)| s == size && h == sha256)
}

fn download(offer: &Offer, target: &Path) -> Result<(), String> {
    let http: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(DOWNLOAD_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = http
        .get(&offer.url)
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    if response.status() != 200 {
        return Err(format!("download answered {}", response.status().as_u16()));
    }
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(offer.size + 1)
        .reader();
    let partial = target.with_extension("part");
    let mut file = fs::File::create(&partial).map_err(|e| format!("saving failed: {e}"))?;
    let mut hash = Sha256::new();
    let mut done = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("download interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(|e| format!("saving failed: {e}"))?;
        done += n as u64;
    }
    drop(file);
    if done != offer.size || hex::encode(hash.finalize()) != offer.sha256 {
        let _ = fs::remove_file(&partial);
        return Err("the download doesn't match the signed manifest".into());
    }
    fs::rename(&partial, target).map_err(|e| format!("saving failed: {e}"))
}

/// Install `ready` and start the new version in the background. On
/// success the caller must quit right away (the new version waits a
/// moment for that, and the Windows setup closes what's still running).
pub fn apply(ready: &Ready) -> Result<(), String> {
    let install = install().ok_or("this copy can't update itself")?;
    // Checked again: the file sat on disk since it was downloaded.
    if sha256_of(&ready.file).map(|(_, h)| h) != Some(ready.sha256.clone()) {
        let _ = fs::remove_file(&ready.file);
        return Err("the downloaded update changed on disk".into());
    }
    match install {
        Install::WindowsSetup => apply_windows(&ready.file),
        Install::MacApp(bundle) => apply_mac(&ready.file, &bundle),
        Install::AppImage(appimage) => apply_appimage(&ready.file, &appimage),
    }
}

/// Run the new setup silently. It closes this copy if it's still running,
/// installs over it (keeping the earlier choices, such as starting at
/// login) and, with `/relaunch=1`, starts the new version in the tray.
fn apply_windows(setup: &Path) -> Result<(), String> {
    std::process::Command::new(setup)
        .args([
            "/VERYSILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/relaunch=1",
        ])
        .spawn()
        .map(drop)
        .map_err(|e| format!("could not start the setup: {e}"))
}

/// Unpack the new `Sisyphus.app` next to the old one, swap them, and open
/// the new one once this one has quit.
fn apply_mac(archive: &Path, bundle: &Path) -> Result<(), String> {
    let parent = bundle.parent().ok_or("bad app location")?;
    let staging = parent.join(".sisyphus-update");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
    let status = std::process::Command::new("/usr/bin/tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(&staging)
        .status()
        .map_err(|e| format!("could not unpack the update: {e}"))?;
    let new = staging.join("Sisyphus.app");
    if !status.success() || !new.join("Contents/Info.plist").is_file() {
        let _ = fs::remove_dir_all(&staging);
        return Err("the update didn't unpack".into());
    }
    let old = parent.join(".Sisyphus-old.app");
    let _ = fs::remove_dir_all(&old);
    fs::rename(bundle, &old).map_err(|e| format!("could not move the old app: {e}"))?;
    if let Err(e) = fs::rename(&new, bundle) {
        let _ = fs::rename(&old, bundle);
        let _ = fs::remove_dir_all(&staging);
        return Err(format!("could not move the new app in place: {e}"));
    }
    let _ = fs::remove_dir_all(&old);
    let _ = fs::remove_dir_all(&staging);
    relaunch_later("open -n \"$0\" --args --background", bundle)
}

/// Put the new AppImage in place of the old one (the running copy keeps
/// the old file open) and start it once this one has quit.
fn apply_appimage(new: &Path, appimage: &Path) -> Result<(), String> {
    let dir = appimage.parent().ok_or("bad AppImage location")?;
    let tmp = dir.join(".Sisyphus.AppImage.new");
    fs::copy(new, &tmp).map_err(|e| format!("could not copy the update: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    fs::rename(&tmp, appimage).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("could not replace the AppImage: {e}")
    })?;
    let _ = fs::remove_file(new);
    relaunch_later("exec \"$0\" --background", appimage)
}

/// Run `command` (with `$0` = `path`) through the shell after a short wait,
/// so this copy has quit and released the tracker's lock by then.
fn relaunch_later(command: &str, path: &Path) -> Result<(), String> {
    std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(format!("sleep 3; {command}"))
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(drop)
        .map_err(|e| format!("could not start the new version: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn signed(manifest: &Value, key: &SigningKey) -> Value {
        let text = manifest.to_string();
        let signature = hex::encode(key.sign(text.as_bytes()).to_bytes());
        serde_json::json!({
            "version": manifest["version"],
            "manifest": text,
            "signature": signature,
            "files": {
                "Sisyphus-linux-x86_64.AppImage": "https://github.com/o/r/releases/download/v99.0.0/Sisyphus-linux-x86_64.AppImage",
            },
        })
    }

    fn manifest(version: &str) -> Value {
        serde_json::json!({
            "kind": MANIFEST_KIND,
            "version": version,
            "files": {
                "linux-x86_64": {
                    "name": "Sisyphus-linux-x86_64.AppImage",
                    "sha256": "ab".repeat(32),
                    "size": 1234,
                },
            },
        })
    }

    #[test]
    fn takes_a_signed_newer_version() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let answer = signed(&manifest("99.0.0"), &key);
        let offer = verify_offer(&answer, "linux-x86_64", &key.verifying_key())
            .unwrap()
            .unwrap();
        assert_eq!(offer.version, "99.0.0");
        assert_eq!(offer.size, 1234);
        assert!(offer.url.ends_with("/Sisyphus-linux-x86_64.AppImage"));
    }

    #[test]
    fn refuses_other_keys_and_tampering() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let other = SigningKey::from_bytes(&[8; 32]);
        let answer = signed(&manifest("99.0.0"), &other);
        assert!(verify_offer(&answer, "linux-x86_64", &key.verifying_key()).is_err());

        let mut answer = signed(&manifest("99.0.0"), &key);
        let tampered = answer["manifest"].as_str().unwrap().replace("1234", "1235");
        answer["manifest"] = tampered.into();
        assert!(verify_offer(&answer, "linux-x86_64", &key.verifying_key()).is_err());
    }

    #[test]
    fn ignores_older_and_missing_platforms() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let answer = signed(&manifest("0.0.1"), &key);
        assert_eq!(
            verify_offer(&answer, "linux-x86_64", &key.verifying_key()),
            Ok(None)
        );
        let answer = signed(&manifest("99.0.0"), &key);
        assert!(verify_offer(&answer, "windows-x86_64", &key.verifying_key()).is_err());
    }

    #[test]
    fn refuses_path_names() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let mut m = manifest("99.0.0");
        m["files"]["linux-x86_64"]["name"] = "../../.bashrc".into();
        let answer = signed(&m, &key);
        assert!(verify_offer(&answer, "linux-x86_64", &key.verifying_key()).is_err());
    }

    #[test]
    fn versions() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.2.0-beta", "0.1.0"));
        assert!(!is_newer("1.0", "0.1.0"));
    }

    #[test]
    fn release_key_is_valid() {
        release_key();
    }

    #[test]
    fn finds_the_app_bundle() {
        assert_eq!(
            app_bundle(Path::new(
                "/Applications/Sisyphus.app/Contents/MacOS/sisyphus"
            )),
            Some(PathBuf::from("/Applications/Sisyphus.app"))
        );
        assert_eq!(app_bundle(Path::new("/usr/bin/sisyphus")), None);
    }
}
