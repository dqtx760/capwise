use std::{
    io::{Read, Write},
    os::windows::process::CommandExt,
    process::{ChildStdin, Command, Stdio},
};

use windows_sys::Win32::{
    Foundation::HWND,
    System::Threading::CREATE_NO_WINDOW,
    UI::WindowsAndMessaging::{
        FindWindowExW, GetWindowThreadProcessId, IsWindowVisible, SW_HIDE, SW_SHOWNA,
        ShowWindowAsync,
    },
};

const WATCHDOG_ARG: &str = "--restore-recording-taskbars";

#[derive(Clone, Copy, PartialEq, Eq)]
struct Taskbar {
    handle: isize,
    process: u32,
}

impl Taskbar {
    fn window(self) -> HWND {
        self.handle as HWND
    }
}

fn taskbars() -> impl Iterator<Item = Taskbar> {
    ["Shell_TrayWnd", "Shell_SecondaryTrayWnd"]
        .into_iter()
        .flat_map(|class| {
            let class: Vec<_> = class.encode_utf16().chain([0]).collect();
            let mut previous = std::ptr::null_mut();
            std::iter::from_fn(move || {
                let window = unsafe {
                    FindWindowExW(
                        std::ptr::null_mut(),
                        previous,
                        class.as_ptr(),
                        std::ptr::null(),
                    )
                };
                if window.is_null() {
                    return None;
                }
                previous = window;
                let mut process = 0;
                unsafe { GetWindowThreadProcessId(window, &mut process) };
                Some(Taskbar {
                    handle: window as isize,
                    process,
                })
            })
        })
}

fn restore(taskbars_to_restore: &[Taskbar]) -> bool {
    let mut posted = true;
    for taskbar in taskbars().filter(|taskbar| taskbars_to_restore.contains(taskbar)) {
        posted &= unsafe { ShowWindowAsync(taskbar.window(), SW_SHOWNA) } != 0;
    }
    posted
}

pub(crate) struct RecordingTaskbar {
    hidden: Vec<Taskbar>,
    watchdog: ChildStdin,
}

impl RecordingTaskbar {
    pub(crate) fn hide() -> std::io::Result<Option<Self>> {
        let visible: Vec<_> = taskbars()
            .filter(|taskbar| unsafe { IsWindowVisible(taskbar.window()) } != 0)
            .collect();
        if visible.is_empty() {
            return Ok(None);
        }
        let mut child = Command::new(std::env::current_exe()?)
            .arg(WATCHDOG_ARG)
            .args(
                visible
                    .iter()
                    .flat_map(|taskbar| [taskbar.handle.to_string(), taskbar.process.to_string()]),
            )
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let watchdog = child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("Could not open the taskbar watchdog pipe"))?;
        let hidden: Vec<_> = visible
            .into_iter()
            .filter(|taskbar| unsafe { ShowWindowAsync(taskbar.window(), SW_HIDE) } != 0)
            .collect();
        if !hidden.is_empty() {
            tracing::info!(count = hidden.len(), "Hid Windows taskbars for recording");
        }
        Ok(Some(Self { hidden, watchdog }))
    }
}

impl Drop for RecordingTaskbar {
    fn drop(&mut self) {
        if restore(&self.hidden) {
            // A closed pipe without this byte means the recorder lost its chance to restore.
            let _ = self.watchdog.write_all(&[1]);
        }
    }
}

pub(crate) fn run_watchdog_from_args() -> bool {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(WATCHDOG_ARG)) {
        return false;
    }
    let args: Vec<_> = args.collect();
    let taskbars: Vec<_> = args
        .chunks_exact(2)
        .filter_map(|pair| {
            Some(Taskbar {
                handle: pair[0].to_str()?.parse().ok()?,
                process: pair[1].to_str()?.parse().ok()?,
            })
        })
        .collect();
    if std::io::stdin().read_exact(&mut [0]).is_err() {
        restore(&taskbars);
    }
    true
}
