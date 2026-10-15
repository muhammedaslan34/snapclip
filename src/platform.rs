//! Small OS-specific helpers. Windows gets the full implementation; other
//! platforms fall back to doing nothing.

use std::fs::File;

/// Returns a lock that is held for the lifetime of the process, or `None` if
/// another instance is already running.
pub fn single_instance_lock() -> Option<File> {
    let dir = crate::settings::data_dir();
    crate::history::ensure_dir(&dir);
    let file = File::create(dir.join("instance.lock")).ok()?;
    match file.try_lock() {
        Ok(()) => Some(file),
        Err(_) => None,
    }
}

/// True if no other instance holds the lock (checked without keeping it).
pub fn single_instance_probe() -> bool {
    single_instance_lock().is_some()
}

/// Opaque handle of the window that had focus before one of our windows opened.
#[derive(Clone, Copy, Debug)]
pub struct ForeignWindow(#[allow(dead_code)] isize);

#[cfg(windows)]
mod imp {
    use super::ForeignWindow;
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
        KEYEVENTF_KEYUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput, VK_CONTROL,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, GetForegroundWindow, SetCursorPos, SetForegroundWindow, WHEEL_DELTA,
    };

    pub fn cursor_pos() -> Option<(i32, i32)> {
        let mut p = POINT { x: 0, y: 0 };
        // SAFETY: `p` is a valid, writable POINT.
        (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x, p.y))
    }

    pub fn foreground_window() -> Option<ForeignWindow> {
        // SAFETY: no arguments; returns null when no window has focus.
        let hwnd = unsafe { GetForegroundWindow() };
        (!hwnd.is_null()).then_some(ForeignWindow(hwnd as isize))
    }

    pub fn activate(window: ForeignWindow) {
        // SAFETY: a stale handle only makes the call fail.
        unsafe { SetForegroundWindow(window.0 as _) };
    }

    pub fn send_paste() {
        const VK_V: u16 = 0x56;
        fn key(vk: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
            INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: vk,
                        wScan: 0,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            }
        }
        let inputs = [
            key(VK_CONTROL, 0),
            key(VK_V, 0),
            key(VK_V, KEYEVENTF_KEYUP),
            key(VK_CONTROL, KEYEVENTF_KEYUP),
        ];
        send(&inputs);
    }

    /// Moves the mouse to `pos` and scrolls down by `notches` wheel clicks.
    pub fn scroll_at(pos: (i32, i32), notches: i32) {
        // SAFETY: plain call; fails harmlessly for off-screen positions.
        unsafe { SetCursorPos(pos.0, pos.1) };
        let wheel = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: (-(notches * WHEEL_DELTA as i32)) as u32,
                    dwFlags: MOUSEEVENTF_WHEEL,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        send(&[wheel]);
    }

    /// Keeps `window` out of screenshots, so it never ends up in its own capture.
    pub fn exclude_from_capture(window: &winit::window::Window) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE};
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(handle) = window.window_handle() else { return };
        if let RawWindowHandle::Win32(h) = handle.as_raw() {
            // SAFETY: the handle belongs to a live window of this process.
            unsafe { SetWindowDisplayAffinity(h.hwnd.get() as _, WDA_EXCLUDEFROMCAPTURE) };
        }
    }

    fn send(inputs: &[INPUT]) {
        // SAFETY: `inputs` is a valid slice of INPUT structs of the given size.
        unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            )
        };
    }
}

#[cfg(not(windows))]
mod imp {
    use super::ForeignWindow;

    pub fn cursor_pos() -> Option<(i32, i32)> {
        None
    }

    pub fn foreground_window() -> Option<ForeignWindow> {
        None
    }

    pub fn activate(_window: ForeignWindow) {}

    pub fn send_paste() {
        log::info!("auto-paste is only implemented on Windows; the item was copied");
    }

    /// Automatic scrolling is Windows-only; elsewhere the user scrolls by hand.
    pub fn scroll_at(_pos: (i32, i32), _notches: i32) {}

    pub fn exclude_from_capture(_window: &winit::window::Window) {}
}

pub use imp::{activate, cursor_pos, exclude_from_capture, foreground_window, scroll_at, send_paste};

/// What a second launch of the app asks the running instance to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Settings,
    Capture,
    Clipboard,
}

impl Request {
    #[cfg_attr(not(windows), allow(dead_code))]
    const ALL: [Request; 3] = [Request::Settings, Request::Capture, Request::Clipboard];

    /// Reads `--capture`, `--clipboard` or `--settings` from the command line.
    pub fn from_args() -> Option<Self> {
        std::env::args().skip(1).find_map(|arg| match arg.as_str() {
            "--settings" => Some(Request::Settings),
            "--capture" => Some(Request::Capture),
            "--clipboard" => Some(Request::Clipboard),
            _ => None,
        })
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    fn event_name(self) -> String {
        let name = match self {
            Request::Settings => "settings",
            Request::Capture => "capture",
            Request::Clipboard => "clipboard",
        };
        format!("Local\\SnapClip.Request.{name}")
    }
}

#[cfg(windows)]
mod ipc {
    use super::Request;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        CreateEventW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent, WaitForMultipleObjects,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MessageBoxW};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// Waits for requests from later launches and passes them to `on_request`.
    pub fn listen(on_request: impl Fn(Request) + Send + 'static) {
        let handles: Vec<HANDLE> = Request::ALL
            .iter()
            // SAFETY: valid null-terminated name; auto-reset, initially unsignaled event.
            .map(|r| unsafe { CreateEventW(std::ptr::null(), 0, 0, wide(&r.event_name()).as_ptr()) })
            .collect();
        if handles.iter().any(|h| h.is_null()) {
            log::warn!("cannot create instance events; later launches will be ignored");
            return;
        }
        // Raw handles aren't Send; they stay valid for the life of the process.
        let raw: Vec<usize> = handles.iter().map(|h| *h as usize).collect();
        std::thread::Builder::new()
            .name("instance-requests".into())
            .spawn(move || {
                let handles: Vec<HANDLE> = raw.iter().map(|h| *h as HANDLE).collect();
                loop {
                    // SAFETY: `handles` holds valid event handles.
                    let result = unsafe {
                        WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, INFINITE)
                    };
                    match Request::ALL.get(result.wrapping_sub(WAIT_OBJECT_0) as usize) {
                        Some(request) => on_request(*request),
                        None => break,
                    }
                }
            })
            .ok();
    }

    /// Signals the running instance. Returns false if none is listening.
    pub fn send(request: Request) -> bool {
        // SAFETY: valid null-terminated name; the handle is closed below.
        unsafe {
            let handle = OpenEventW(EVENT_MODIFY_STATE, 0, wide(&request.event_name()).as_ptr());
            if handle.is_null() {
                return false;
            }
            let ok = SetEvent(handle) != 0;
            CloseHandle(handle);
            ok
        }
    }

    pub fn error_box(message: &str) {
        let title = wide("SnapClip");
        let text = wide(message);
        // SAFETY: both strings are valid and null-terminated.
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
            )
        };
    }
}

#[cfg(not(windows))]
mod ipc {
    use super::Request;

    pub fn listen(_on_request: impl Fn(Request) + Send + 'static) {}

    pub fn send(_request: Request) -> bool {
        false
    }

    pub fn error_box(message: &str) {
        log::error!("{message}");
    }
}

pub use ipc::{error_box, listen as listen_for_requests, send as send_request};
