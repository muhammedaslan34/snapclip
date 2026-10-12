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
}

pub use imp::{activate, cursor_pos, foreground_window, scroll_at, send_paste};
