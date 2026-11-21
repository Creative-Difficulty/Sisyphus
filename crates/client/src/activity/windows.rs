//! Windows: `GetLastInputInfo` for idle time, the foreground window's
//! process for the focused app, Toolhelp for the process table.

use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use super::process::{Process, agent_of, is_claude_build, is_version_name};
use super::{FocusReading, FocusedApp};

pub struct Monitor;

impl Monitor {
    pub fn start() -> Self {
        Monitor
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
        "input: GetLastInputInfo, focus: foreground window".to_string()
    }
}

/// The executable of a process, if we may query it.
fn exe_path(pid: u32) -> Option<String> {
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
