//! Windows: `GetLastInputInfo` for idle time, the foreground window's
//! process for the focused app, Toolhelp for the process table. Keys are
//! told from mouse by polling the last input time, the cursor and the mouse
//! buttons ([`super::sampler`]); no keyboard hook.

use std::time::Duration;

use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::SystemInformation::{GetTickCount, GetTickCount64};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetLastInputInfo, LASTINPUTINFO, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON,
    VK_XBUTTON1, VK_XBUTTON2,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId,
};

use super::process::{Process, agent_of, is_claude_build, is_version_name};
use super::sampler::{Inferred, Probe};
use super::{FocusReading, FocusedApp, InputCounts, InputKinds};

pub struct Monitor {
    kinds: Option<Inferred>,
}

impl Monitor {
    pub fn start() -> Self {
        // GetTickCount ticks in steps of about 16 ms, and the two clocks
        // below are read one after the other.
        Monitor {
            kinds: Inferred::start(20, probe),
        }
    }

    pub fn take_input(&mut self) -> Option<(InputCounts, InputKinds)> {
        Some((self.kinds.as_ref()?.take(), InputKinds::Inferred))
    }

    pub fn since_last_input(&mut self) -> Option<Duration> {
        let mut info = LASTINPUTINFO {
            cbSize: size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        // SAFETY: `info` is a LASTINPUTINFO with its size set.
        if unsafe { GetLastInputInfo(&mut info) } == 0 {
            return None;
        }
        // Both are milliseconds since boot and wrap after 49.7 days.
        // SAFETY: no arguments.
        let now = unsafe { GetTickCount() };
        Some(Duration::from_millis(now.wrapping_sub(info.dwTime).into()))
    }

    pub fn focused(&mut self) -> FocusReading {
        // SAFETY: no arguments.
        let window = unsafe { GetForegroundWindow() };
        if window.is_null() {
            return FocusReading::Nothing;
        }
        let mut pid = 0u32;
        // SAFETY: `window` is a window handle, `pid` a valid out pointer.
        unsafe { GetWindowThreadProcessId(window, &mut pid) };
        if pid == 0 {
            return FocusReading::Unknown;
        }
        let app_id = exe_path(pid)
            .and_then(|path| path.rsplit('\\').next().map(|name| name.to_string()))
            .unwrap_or_default();
        FocusReading::App(FocusedApp {
            app_id,
            pid: Some(pid),
        })
    }

    pub fn describe(&self) -> String {
        "input: GetLastInputInfo, focus: foreground window, keys/mouse: idle time and cursor"
            .to_string()
    }
}

/// The last input time, the cursor and whether a mouse button is down.
/// Only the mouse buttons' state is asked for, never a key's.
fn probe() -> Option<Probe> {
    let mut info = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: `info` is a LASTINPUTINFO with its size set.
    if unsafe { GetLastInputInfo(&mut info) } == 0 {
        return None;
    }
    // SAFETY: no arguments.
    let (now32, now64) = unsafe { (GetTickCount(), GetTickCount64()) };
    let idle = u64::from(now32.wrapping_sub(info.dwTime));
    let mut point = POINT { x: 0, y: 0 };
    // SAFETY: `point` is a valid out pointer.
    if unsafe { GetCursorPos(&mut point) } == 0 {
        return None;
    }
    let button_down = [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON, VK_XBUTTON1, VK_XBUTTON2]
        .into_iter()
        // SAFETY: plain call with a virtual-key code; the high bit means down.
        .any(|vk| unsafe { GetAsyncKeyState(vk.into()) } as u16 & 0x8000 != 0);
    Some(Probe {
        last_input_ms: now64.saturating_sub(idle),
        cursor: (point.x, point.y),
        button_down,
    })
}

/// The executable of a process, if we may query it.
pub(super) fn exe_path(pid: u32) -> Option<String> {
    // SAFETY: plain call; a null handle means failure.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return None;
    }
    let _guard = Handle(process);
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` holds `len` UTF-16 units.
    let ok = unsafe {
        QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len)
    };
    (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

/// All processes, with parent and executable name. Windows doesn't offer
/// other processes' working directories, so agents match any project.
pub fn processes() -> Vec<Process> {
    // SAFETY: plain call; INVALID_HANDLE_VALUE means failure.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let _guard = Handle(snapshot);
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut list = Vec::new();
    // SAFETY: `entry` is a PROCESSENTRY32W with its size set.
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) };
    while ok != 0 {
        let len = entry
            .szExeFile
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(entry.szExeFile.len());
        let mut name = String::from_utf16_lossy(&entry.szExeFile[..len]);
        // Claude Code's native builds are named after their version.
        if is_version_name(&name)
            && exe_path(entry.th32ProcessID)
                .is_some_and(|p| is_claude_build(std::path::Path::new(&p.replace('\\', "/"))))
        {
            name = "claude.exe".to_string();
        }
        let lower = name.to_lowercase();
        list.push(Process {
            pid: entry.th32ProcessID,
            ppid: entry.th32ParentProcessID,
            agent: agent_of(&name, &[]),
            name: lower.strip_suffix(".exe").unwrap_or(&lower).to_string(),
            cwd: None,
            session: None,
            background: false,
            manager: None,
        });
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snapshot, &mut entry) };
    }
    list
}

/// Closes a handle when dropped.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle is open and owned by us.
        unsafe { CloseHandle(self.0) };
    }
}
