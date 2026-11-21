//! macOS: CoreGraphics for idle time and key/mouse event counts (counters
//! per event type, never which key), NSWorkspace for the frontmost app,
//! libproc for the process table. Needs no permission prompt: none of these
//! read window titles or screen contents.

use std::ffi::{CStr, c_int, c_void};
use std::path::PathBuf;
use std::time::Duration;

use objc2_app_kit::NSWorkspace;

use super::process::{Process, agent_of, is_claude_build, is_version_name};
use super::{FocusReading, FocusedApp, InputCounts, InputKinds};

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
    fn CGEventSourceCounterForEventType(state: i32, event_type: u32) -> u32;
}

// Part of the Objective-C runtime, which objc2 already links.
unsafe extern "C" {
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

/// `kCGEventSourceStateCombinedSessionState`
const COMBINED_SESSION_STATE: i32 = 0;
/// `kCGAnyInputEventType`
const ANY_INPUT_EVENT: u32 = !0;

/// `kCGEventKeyDown`
const KEY_EVENTS: [u32; 1] = [10];
/// Mouse down (left, right, other), moved, dragged (left, right, other)
/// and scroll wheel events.
const MOUSE_EVENTS: [u32; 8] = [1, 3, 25, 5, 6, 7, 27, 22];

pub struct Monitor {
    /// The event counters at the last `take_input`.
    counters: Option<(u32, u32)>,
}

impl Monitor {
    pub fn start() -> Self {
        Monitor { counters: None }
    }

    pub fn take_input(&mut self) -> Option<(InputCounts, InputKinds)> {
        let sum = |types: &[u32]| {
            types.iter().fold(0u32, |sum, &t| {
                // SAFETY: a plain C function taking two integers.
                sum.wrapping_add(unsafe {
                    CGEventSourceCounterForEventType(COMBINED_SESSION_STATE, t)
                })
            })
        };
        let now = (sum(&KEY_EVENTS), sum(&MOUSE_EVENTS));
        let before = self.counters.replace(now).unwrap_or(now);
        Some((
            InputCounts {
                keys: now.0.wrapping_sub(before.0),
                mouse: now.1.wrapping_sub(before.1),
            },
            InputKinds::Exact,
        ))
    }

    pub fn since_last_input(&mut self) -> Option<Duration> {
        // SAFETY: a plain C function taking two integers.
        let seconds = unsafe {
            CGEventSourceSecondsSinceLastEventType(COMBINED_SESSION_STATE, ANY_INPUT_EVENT)
        };
        (seconds.is_finite() && seconds >= 0.0).then(|| Duration::from_secs_f64(seconds))
    }

    /// The frontmost app. AppKit keeps this current through the main run
    /// loop, which the window runs; a command-line tool without one may see
    /// a stale value.
    pub fn focused(&mut self) -> FocusReading {
        // Autoreleased objects would otherwise pile up on a polling thread.
        // SAFETY: push and pop are paired on this thread.
        let pool = unsafe { objc_autoreleasePoolPush() };
        let reading = match NSWorkspace::sharedWorkspace().frontmostApplication() {
            None => FocusReading::Nothing,
            Some(app) => {
                let app_id = app
                    .bundleIdentifier()
                    .or_else(|| app.localizedName())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                FocusReading::App(FocusedApp {
                    app_id,
                    pid: u32::try_from(app.processIdentifier()).ok(),
                })
            }
        };
        // SAFETY: the pool pushed above; nothing from it is used afterwards.
        unsafe { objc_autoreleasePoolPop(pool) };
        reading
    }

    pub fn describe(&self) -> String {
        "input: CoreGraphics, focus: NSWorkspace, keys/mouse: CoreGraphics counters".to_string()
    }
}

/// All processes we may inspect, with parent and executable name.
pub fn processes() -> Vec<Process> {
    pids()
        .into_iter()
        .filter_map(|pid| {
            let info = bsd_info(pid)?;
            let mut name = c_chars(&info.pbi_name)
                .filter(|n| !n.is_empty())
                .or_else(|| c_chars(&info.pbi_comm))?;
            // Claude Code's native builds are named after their version.
            if is_version_name(&name) && exe_path(pid).is_some_and(|p| is_claude_build(&p)) {
                name = "claude".to_string();
            }
            let args = if matches!(
                name.as_str(),
                "node" | "bun" | "deno" | "claude" | "opencode"
            ) || super::process::is_codex_binary(&name)
            {
                args(pid)
            } else {
                Vec::new()
            };
            let agent = agent_of(&name, &args);
            let cwd = agent.and_then(|_| cwd(pid));
            Some(Process {
                pid: pid as u32,
                ppid: info.pbi_ppid,
                name: name.to_lowercase(),
                agent,
                cwd,
                session: None,
                background: false,
                manager: super::process::manager_of(&name, &args),
            })
        })
        .collect()
}

/// The process's executable path.
pub(super) fn exe_path(pid: c_int) -> Option<PathBuf> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer holds `buf.len()` bytes.
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    Some(PathBuf::from(String::from_utf8(buf).ok()?))
}

fn pids() -> Vec<c_int> {
    // SAFETY: a null buffer asks for the number of processes.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return Vec::new();
    }
    // Room for processes started in between.
    let mut pids = vec![0 as c_int; count as usize + 64];
    let size = (pids.len() * size_of::<c_int>()) as c_int;
    // SAFETY: the buffer holds `size` bytes.
    let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), size) };
    pids.truncate(n.max(0) as usize);
    pids.retain(|p| *p > 0);
    pids
}

fn bsd_info(pid: c_int) -> Option<libc::proc_bsdinfo> {
    // SAFETY: proc_bsdinfo is plain data; all-zero is a valid value.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = size_of::<libc::proc_bsdinfo>() as c_int;
    // SAFETY: the buffer is a proc_bsdinfo of `size` bytes.
    let n =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    (n == size).then_some(info)
}

fn cwd(pid: c_int) -> Option<PathBuf> {
    // SAFETY: plain data; all-zero is a valid value.
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = size_of::<libc::proc_vnodepathinfo>() as c_int;
    // SAFETY: the buffer is a proc_vnodepathinfo of `size` bytes.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    if n != size {
        return None;
    }
    // `vip_path` is a MAXPATHLEN char array, split up in libc's binding.
    let raw = info.pvi_cdir.vip_path.as_flattened();
    c_chars(raw).filter(|p| !p.is_empty()).map(PathBuf::from)
}

/// The command line, from `KERN_PROCARGS2`: argc, the executable path,
/// padding, then argc NUL-terminated arguments.
fn args(pid: c_int) -> Vec<String> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    // SAFETY: asks for the size only.
    let ok = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if ok != 0 || size < 4 {
        return Vec::new();
    }
    let mut buf = vec![0u8; size];
    // SAFETY: the buffer holds `size` bytes.
    let ok = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if ok != 0 || size < 4 {
        return Vec::new();
    }
    buf.truncate(size);
    let argc = i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]).max(0) as usize;
    let mut parts = buf[4..].split(|b| *b == 0).filter(|p| !p.is_empty());
    let _exec_path = parts.next();
    parts
        .take(argc)
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect()
}

/// A NUL-terminated C string in a fixed array.
fn c_chars(raw: &[libc::c_char]) -> Option<String> {
    // SAFETY: c_char and u8 have the same size and alignment.
    let bytes: &[u8] = unsafe { std::slice::from_raw_parts(raw.as_ptr().cast(), raw.len()) };
    let s = CStr::from_bytes_until_nul(bytes).ok()?;
    Some(s.to_string_lossy().into_owned())
}
