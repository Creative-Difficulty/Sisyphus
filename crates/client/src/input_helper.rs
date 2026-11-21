//! The input helper for Linux Wayland, where the desktop doesn't tell apps
//! whether the user typed or used the mouse (decided 2026-11-21).
//!
//! After the user agreed in the window and entered their password (polkit,
//! `pkexec`), [`install`] copies this binary to [`INSTALLED`] and sets up
//! the system service [`SERVICE`], which runs [`serve`]:
//! - It reads the keyboard and mouse devices (`/dev/input/event*`) and
//!   counts key presses and mouse events (movements, clicks, scrolling,
//!   touchpad touches). Which keys were pressed is looked at only to tell
//!   keys from mouse buttons and is never stored or sent.
//! - Every quarter second with anything counted, it sends `<keys> <mouse>`
//!   to the connected app over [`SOCKET`], and only to the user who
//!   installed it (checked with `SO_PEERCRED`).
//! - It doesn't run as root: systemd runs it as a throwaway user
//!   (`DynamicUser`) in the `input` group, without network access or
//!   capabilities, unable to write anywhere but its socket's folder, and
//!   allowed to open input devices only.
//!
//! [`uninstall`] removes both again.

use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Where the service listens.
pub const SOCKET: &str = "/run/sisyphus-input/socket";
/// The installed copy of this binary.
pub const INSTALLED: &str = "/usr/local/libexec/sisyphus-input-helper";
pub const SERVICE: &str = "sisyphus-input-helper.service";
const UNIT: &str = "/etc/systemd/system/sisyphus-input-helper.service";

/// Look for new devices this often.
const RESCAN: Duration = Duration::from_secs(5);
/// Send counts this often (when there are any).
const FLUSH: Duration = Duration::from_millis(250);

// From linux/input-event-codes.h.
const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const EV_ABS: u16 = 0x03;
const SYN_REPORT: u16 = 0;
/// Codes from here up to `KEY_OK` are buttons (mouse, touchpad, pen,
/// gamepad), the rest keys.
const BTN_MISC: u16 = 0x100;
const KEY_OK: u16 = 0x160;
const BTN_TOUCH: u16 = 0x14a;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub keys: u32,
    pub mouse: u32,
}

/// What a device is in the middle of.
#[derive(Debug, Default)]
pub struct DeviceState {
    /// A finger or pen touches (touchpads, touchscreens, tablets). Absolute
    /// movement counts only then, so sensors like accelerometers don't.
    touching: bool,
    /// Pointer movement since the last frame (`SYN_REPORT`).
    moved: bool,
}

/// Count one input event. The key code only decides key or button.
pub fn count(dev: &mut DeviceState, kind: u16, code: u16, value: i32, counts: &mut Counts) {
    match kind {
        EV_KEY if code == BTN_TOUCH => {
            dev.touching = value != 0;
            if value == 1 {
                counts.mouse = counts.mouse.saturating_add(1);
            }
        }
        // 1 is a press; 0 a release and 2 a key repeat don't count.
        EV_KEY if value == 1 => {
            if (BTN_MISC..KEY_OK).contains(&code) {
                counts.mouse = counts.mouse.saturating_add(1);
            } else {
                counts.keys = counts.keys.saturating_add(1);
            }
        }
        EV_REL => dev.moved = true,
        EV_ABS if dev.touching => dev.moved = true,
        EV_SYN if code == SYN_REPORT && dev.moved => {
            dev.moved = false;
            counts.mouse = counts.mouse.saturating_add(1);
        }
        _ => {}
    }
}

struct Device {
    path: PathBuf,
    file: File,
    state: DeviceState,
}

/// Run the service: count and serve until killed.
pub fn serve(uid: u32) -> Result<(), String> {
    let path = Path::new(SOCKET);
    let _ = fs::remove_file(path);
    let listener = UnixListener::bind(path).map_err(|e| format!("{SOCKET}: {e}"))?;
    // Anyone may connect; only `uid` is answered (checked on accept).
    fs::set_permissions(path, fs::Permissions::from_mode(0o666)).map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;

    let mut devices: Vec<Device> = Vec::new();
    let mut clients: Vec<UnixStream> = Vec::new();
    let mut counts = Counts::default();
    let mut scanned: Option<Instant> = None;
    let mut flushed = Instant::now();
    // SAFETY: input_event is plain data; all zero is a valid value.
    let mut events: [libc::input_event; 64] = unsafe { std::mem::zeroed() };

    loop {
        if scanned.is_none_or(|t| t.elapsed() >= RESCAN) {
            scan(&mut devices);
            scanned = Some(Instant::now());
        }

        let mut fds: Vec<libc::pollfd> = std::iter::once(listener.as_raw_fd())
            .chain(devices.iter().map(|d| d.file.as_raw_fd()))
            .map(|fd| libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        // SAFETY: `fds` holds `fds.len()` pollfds.
        unsafe {
            libc::poll(
                fds.as_mut_ptr(),
                fds.len() as libc::nfds_t,
                FLUSH.as_millis() as i32,
            )
        };

        if fds[0].revents & libc::POLLIN != 0 {
            while let Ok((stream, _)) = listener.accept() {
                if peer_uid(&stream) == Some(uid) && stream.set_nonblocking(true).is_ok() {
                    clients.push(stream);
                }
            }
        }

        let mut gone = Vec::new();
        for (i, device) in devices.iter_mut().enumerate() {
            if fds[i + 1].revents == 0 {
                continue;
            }
            loop {
                // SAFETY: reads at most the buffer's size into it.
                let n = unsafe {
                    libc::read(
                        device.file.as_raw_fd(),
                        events.as_mut_ptr().cast(),
                        size_of_val(&events),
                    )
                };
                if n <= 0 {
                    let err = std::io::Error::last_os_error();
                    if n == 0 || err.kind() != ErrorKind::WouldBlock {
                        gone.push(i);
                    }
                    break;
                }
                let read = n as usize / size_of::<libc::input_event>();
                for e in &events[..read] {
                    count(&mut device.state, e.type_, e.code, e.value, &mut counts);
                }
            }
        }
        for i in gone.into_iter().rev() {
            devices.remove(i);
        }

        if flushed.elapsed() >= FLUSH {
            flushed = Instant::now();
            if counts != Counts::default() {
                let line = format!("{} {}\n", counts.keys, counts.mouse);
                clients.retain_mut(|c| c.write_all(line.as_bytes()).is_ok());
                counts = Counts::default();
            }
        }
    }
}

/// Open input devices not opened yet.
fn scan(devices: &mut Vec<Device>) {
    let Ok(dir) = fs::read_dir("/dev/input") else {
        return;
    };
    for entry in dir.flatten() {
        let path = entry.path();
        let is_event = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("event"));
        if !is_event || devices.iter().any(|d| d.path == path) {
            continue;
        }
        if let Ok(file) = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&path)
        {
            devices.push(Device {
                path,
                file,
                state: DeviceState::default(),
            });
        }
    }
}

/// The user id of the process at the other end of `stream`.
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` is a ucred of `len` bytes.
    let ok = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &mut len,
        )
    };
    (ok == 0).then_some(cred.uid)
}

/// The systemd unit, for the user `uid`.
fn unit(uid: u32) -> String {
    format!(
        "# Installed by Sisyphus (https://github.com/Creative-Difficulty/Sisyphus).
# Counts key presses and mouse events for one user, never which keys.
# Remove with: sudo {INSTALLED} input-helper uninstall

[Unit]
Description=Sisyphus input helper (counts key presses and mouse events)

[Service]
ExecStart={INSTALLED} input-helper serve --uid {uid}
Restart=on-failure
DynamicUser=yes
SupplementaryGroups=input
RuntimeDirectory=sisyphus-input
RuntimeDirectoryMode=0755
DevicePolicy=closed
DeviceAllow=char-input r
PrivateNetwork=yes
RestrictAddressFamilies=AF_UNIX
CapabilityBoundingSet=
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectControlGroups=yes
ProtectClock=yes
ProtectHostname=yes
RestrictNamespaces=yes
RestrictRealtime=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
UMask=0077
SystemCallArchitectures=native

[Install]
WantedBy=multi-user.target
"
    )
}

fn systemctl(args: &[&str]) -> Result<(), String> {
    let status = Command::new("systemctl")
        .args(args)
        .status()
        .map_err(|e| format!("systemctl: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("systemctl {} failed", args.join(" ")))
    }
}

/// Install and start the service. Runs as root, through `pkexec` (or
/// `sudo`), for the user who started it.
pub fn install() -> Result<(), String> {
    // SAFETY: no arguments.
    if unsafe { libc::geteuid() } != 0 {
        return Err("run this through pkexec or sudo".into());
    }
    let uid: u32 = ["PKEXEC_UID", "SUDO_UID"]
        .iter()
        .find_map(|v| std::env::var(v).ok()?.parse().ok())
        .ok_or("can't tell which user this is for (no PKEXEC_UID or SUDO_UID)")?;
    // SAFETY: a NUL-terminated name; the result is only checked for null.
    if unsafe { libc::getgrnam(c"input".as_ptr()) }.is_null() {
        return Err("this system has no `input` group".into());
    }
    if !Path::new("/run/systemd/system").exists() {
        return Err("this system doesn't run systemd".into());
    }

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let target = Path::new(INSTALLED);
    let dir = target.parent().ok_or("bad install path")?;
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = dir.join(".sisyphus-input-helper.new");
    fs::copy(&exe, &tmp).map_err(|e| format!("copying {}: {e}", exe.display()))?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    fs::rename(&tmp, target).map_err(|e| e.to_string())?;

    fs::write(UNIT, unit(uid)).map_err(|e| format!("{UNIT}: {e}"))?;
    fs::set_permissions(UNIT, fs::Permissions::from_mode(0o644)).map_err(|e| e.to_string())?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", SERVICE])?;
    systemctl(&["restart", SERVICE])?;
    Ok(())
}

/// Stop and remove the service. Runs as root.
pub fn uninstall() -> Result<(), String> {
    // SAFETY: no arguments.
    if unsafe { libc::geteuid() } != 0 {
        return Err("run this with sudo".into());
    }
    let _ = systemctl(&["disable", "--now", SERVICE]);
    for path in [UNIT, INSTALLED] {
        match fs::remove_file(path) {
            Err(e) if e.kind() != ErrorKind::NotFound => return Err(format!("{path}: {e}")),
            _ => {}
        }
    }
    systemctl(&["daemon-reload"])
}

/// Restart the service. Runs as root.
pub fn restart() -> Result<(), String> {
    // SAFETY: no arguments.
    if unsafe { libc::geteuid() } != 0 {
        return Err("run this through pkexec or sudo".into());
    }
    systemctl(&["restart", SERVICE])
}

/// Whether the service is installed (its unit file exists).
pub fn installed() -> bool {
    Path::new(UNIT).exists()
}

/// Whether the service answers.
pub fn running() -> bool {
    UnixStream::connect(SOCKET).is_ok()
}

/// What the window can ask for with the user's password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Install,
    Restart,
    Uninstall,
}

impl Action {
    fn arg(self) -> &'static str {
        match self {
            Action::Install => "install",
            Action::Restart => "restart",
            Action::Uninstall => "uninstall",
        }
    }
}

/// From the window: run `action` as root through polkit's password dialog,
/// then (installing or restarting) wait for the helper to answer.
/// Blocking.
pub fn run_as_admin(action: Action) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let output = Command::new("pkexec")
        .arg(&exe)
        .args(["input-helper", action.arg()])
        .output()
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => "pkexec (polkit) isn't installed on this system.".to_string(),
            _ => format!("Couldn't start pkexec: {e}"),
        })?;
    match output.status.code() {
        Some(0) => {}
        Some(126) => return Err("Cancelled.".into()),
        Some(127) => {
            return Err(format!(
                "Couldn't ask for your password. If no password dialog appeared, your desktop \
                 has no polkit agent running. You can run this in a terminal instead: \
                 pkexec {} input-helper {}",
                exe.display(),
                action.arg()
            ));
        }
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let reason = stderr.lines().last().unwrap_or("unknown error").trim();
            return Err(format!("That failed: {reason}"));
        }
    }
    if action == Action::Uninstall {
        return Ok(());
    }
    for _ in 0..50 {
        if running() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!(
        "The helper doesn't answer. `systemctl status {SERVICE}` says why."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(dev: &mut DeviceState, events: &[(u16, u16, i32)]) -> Counts {
        let mut counts = Counts::default();
        for &(kind, code, value) in events {
            count(dev, kind, code, value, &mut counts);
        }
        counts
    }

    #[test]
    fn counts_key_presses_not_repeats_or_releases() {
        let mut dev = DeviceState::default();
        // KEY_A down, repeat, up; KEY_ENTER down.
        let c = feed(
            &mut dev,
            &[
                (EV_KEY, 30, 1),
                (EV_KEY, 30, 2),
                (EV_KEY, 30, 0),
                (EV_KEY, 28, 1),
            ],
        );
        assert_eq!(c, Counts { keys: 2, mouse: 0 });
    }

    #[test]
    fn mouse_frames_and_buttons_count_as_mouse() {
        let mut dev = DeviceState::default();
        // Two movement frames, a wheel frame, BTN_LEFT down and up.
        let c = feed(
            &mut dev,
            &[
                (EV_REL, 0, 3),
                (EV_REL, 1, -1),
                (EV_SYN, SYN_REPORT, 0),
                (EV_REL, 0, 1),
                (EV_SYN, SYN_REPORT, 0),
                (EV_REL, 8, 1),
                (EV_SYN, SYN_REPORT, 0),
                (EV_KEY, 0x110, 1),
                (EV_KEY, 0x110, 0),
                (EV_SYN, SYN_REPORT, 0),
            ],
        );
        assert_eq!(c, Counts { keys: 0, mouse: 4 });
    }

    #[test]
    fn absolute_movement_counts_only_while_touching() {
        let mut dev = DeviceState::default();
        // An accelerometer-like stream: nothing.
        let c = feed(&mut dev, &[(EV_ABS, 0, 5), (EV_SYN, SYN_REPORT, 0)]);
        assert_eq!(c, Counts::default());
        // A touchpad: touch, move, lift.
        let c = feed(
            &mut dev,
            &[
                (EV_KEY, BTN_TOUCH, 1),
                (EV_ABS, 0, 5),
                (EV_SYN, SYN_REPORT, 0),
                (EV_ABS, 0, 6),
                (EV_SYN, SYN_REPORT, 0),
                (EV_KEY, BTN_TOUCH, 0),
                (EV_SYN, SYN_REPORT, 0),
                (EV_ABS, 0, 7),
                (EV_SYN, SYN_REPORT, 0),
            ],
        );
        assert_eq!(c, Counts { keys: 0, mouse: 3 });
    }

    #[test]
    fn the_unit_runs_unprivileged() {
        let unit = unit(1000);
        assert!(unit.contains("--uid 1000"));
        assert!(unit.contains("DynamicUser=yes"));
        assert!(unit.contains("CapabilityBoundingSet=\n"));
        assert!(unit.contains("PrivateNetwork=yes"));
    }
}
