//! What GPUI can't do for the game overlay (`game_overlay.rs`), done to the
//! window the system made for it: letting every click through to the game
//! under it (and taking them again while it's in use), and staying above a
//! game that puts itself on top. Windows: `WS_EX_LAYERED | WS_EX_TRANSPARENT`
//! and `HWND_TOPMOST`. macOS: `ignoresMouseEvents` (GPUI's pop-up panel is
//! already above full-screen apps). X11: an empty input shape, and raising
//! it. Wayland has no way to, so the overlay isn't offered there.

use gpui_kit::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

fn raw(window: &Window) -> Option<RawWindowHandle> {
    // GPUI's `Window` has a `window_handle` of its own (its id); this is the system's.
    HasWindowHandle::window_handle(window).ok().map(|h| h.as_raw())
}

/// Lets clicks through the window (`through`), or takes them again; false
/// where that can't be done.
pub(crate) fn set(window: &Window, through: bool) -> bool {
    match raw(window) {
        Some(handle) => os::set(handle, through),
        None => false,
    }
}

/// Puts the window back above everything, where a game may have gone over it.
pub(crate) fn keep_on_top(window: &Window) {
    if let Some(handle) = raw(window) {
        os::keep_on_top(handle);
    }
}

#[cfg(windows)]
mod os {
    use raw_window_handle::RawWindowHandle;
    use windows::Win32::Foundation::{COLORREF, HWND};
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowLongPtrW, HWND_TOPMOST, LWA_ALPHA, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, WS_EX_LAYERED, WS_EX_NOACTIVATE,
        WS_EX_TRANSPARENT,
    };

    fn hwnd(handle: RawWindowHandle) -> Option<HWND> {
        match handle {
            RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut core::ffi::c_void)),
            _ => None,
        }
    }

    pub fn set(handle: RawWindowHandle, through: bool) -> bool {
        let Some(hwnd) = hwnd(handle) else { return false };
        // SAFETY: the window is GPUI's and alive while its `Window` is borrowed.
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            // Layered for good (taking it away again redraws the window from nothing),
            // fully opaque, so only the transparent bit decides where clicks go.
            let passing = (WS_EX_TRANSPARENT.0 | WS_EX_NOACTIVATE.0) as isize;
            let style = style | WS_EX_LAYERED.0 as isize;
            let style = if through { style | passing } else { style & !passing };
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style);
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            )
            .is_ok()
        }
    }

    pub fn keep_on_top(handle: RawWindowHandle) {
        let Some(hwnd) = hwnd(handle) else { return };
        // SAFETY: as above.
        unsafe {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

#[cfg(target_os = "macos")]
mod os {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use raw_window_handle::RawWindowHandle;

    pub fn set(handle: RawWindowHandle, through: bool) -> bool {
        let RawWindowHandle::AppKit(h) = handle else { return false };
        // SAFETY: the view is GPUI's and alive while its `Window` is borrowed;
        // this runs on the main thread, as GPUI's windows do.
        unsafe {
            let view = h.ns_view.as_ptr().cast::<AnyObject>();
            let window: *mut AnyObject = msg_send![view, window];
            if window.is_null() {
                return false;
            }
            let _: () = msg_send![window, setIgnoresMouseEvents: through];
        }
        true
    }

    pub fn keep_on_top(_: RawWindowHandle) {}
}

#[cfg(target_os = "linux")]
mod os {
    use raw_window_handle::RawWindowHandle;
    use x11rb::connection::Connection as _;
    use x11rb::protocol::shape::{self, ConnectionExt as _};
    use x11rb::protocol::xproto::{ClipOrdering, ConfigureWindowAux, ConnectionExt as _, StackMode};
    use x11rb::rust_connection::RustConnection;

    fn window(handle: RawWindowHandle) -> Option<u32> {
        match handle {
            RawWindowHandle::Xcb(h) => Some(h.window.get()),
            RawWindowHandle::Xlib(h) => u32::try_from(h.window).ok(),
            _ => None,
        }
    }

    pub fn set(handle: RawWindowHandle, through: bool) -> bool {
        let Some(win) = window(handle) else { return false };
        let Ok((conn, _)) = RustConnection::connect(None) else { return false };
        let done = if through {
            // No input region at all: every click lands on what's under it.
            conn.shape_rectangles(shape::SO::SET, shape::SK::INPUT, ClipOrdering::UNSORTED, win, 0, 0, &[]).map(|_| ())
        } else {
            // Back to the window's own shape.
            conn.shape_mask(shape::SO::SET, shape::SK::INPUT, win, 0, 0, x11rb::NONE).map(|_| ())
        };
        done.is_ok() && conn.flush().is_ok()
    }

    pub fn keep_on_top(handle: RawWindowHandle) {
        let Some(win) = window(handle) else { return };
        if let Ok((conn, _)) = RustConnection::connect(None) {
            let _ = conn.configure_window(win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
            let _ = conn.flush();
        }
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod os {
    use raw_window_handle::RawWindowHandle;

    pub fn set(_: RawWindowHandle, _: bool) -> bool {
        false
    }

    pub fn keep_on_top(_: RawWindowHandle) {}
}
