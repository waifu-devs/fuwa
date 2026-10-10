//! Whether a game looks to be in front: another program's window covering
//! its whole screen, for the game overlay (`ui/game_overlay.rs`). Windows
//! asks the window in front and its monitor; X11 asks the window manager
//! (`_NET_ACTIVE_WINDOW` and its `_NET_WM_STATE_FULLSCREEN`). macOS says
//! nothing about other apps' windows without Screen Recording, so there only
//! games that report what you're playing (`core::presence`) count.
//! Nothing about the window is kept or sent anywhere: only yes or no.

/// Whether another program covers its screen. None while one of fuwa's own
/// windows is in front (the overlay taking clicks), so nothing changes then.
pub fn full_screen() -> Option<bool> {
    os::full_screen()
}

#[cfg(windows)]
mod os {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetDesktopWindow, GetForegroundWindow, GetShellWindow, GetWindowRect, GetWindowThreadProcessId,
    };

    pub fn full_screen() -> Option<bool> {
        // SAFETY: each call only reads about the window in front, into memory owned here.
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.is_invalid() {
                return Some(false);
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == std::process::id() {
                return None;
            }
            if hwnd == GetShellWindow() || hwnd == GetDesktopWindow() {
                return Some(false);
            }
            // The desktop's own windows cover the screen too.
            let mut class = [0u16; 32];
            let n = usize::try_from(GetClassNameW(hwnd, &mut class)).unwrap_or(0);
            if matches!(String::from_utf16_lossy(&class[..n]).as_str(), "Progman" | "WorkerW") {
                return Some(false);
            }
            let mut rect = RECT::default();
            if GetWindowRect(hwnd, &mut rect).is_err() {
                return Some(false);
            }
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                return Some(false);
            }
            let m = info.rcMonitor;
            Some(rect.left <= m.left && rect.top <= m.top && rect.right >= m.right && rect.bottom >= m.bottom)
        }
    }
}

#[cfg(target_os = "linux")]
mod os {
    use parking_lot::Mutex;
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};
    use x11rb::rust_connection::RustConnection;

    struct X {
        conn: RustConnection,
        root: u32,
        active: u32,
        state: u32,
        fullscreen: u32,
        pid: u32,
    }

    /// One connection, kept while it works.
    static X: Mutex<Option<X>> = Mutex::new(None);

    fn connect() -> Option<X> {
        if !crate::core::hotkeys::x11() {
            return None;
        }
        let (conn, screen) = RustConnection::connect(None).ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        let atom = |name: &str| Some(conn.intern_atom(false, name.as_bytes()).ok()?.reply().ok()?.atom);
        let (active, state) = (atom("_NET_ACTIVE_WINDOW")?, atom("_NET_WM_STATE")?);
        let (fullscreen, pid) = (atom("_NET_WM_STATE_FULLSCREEN")?, atom("_NET_WM_PID")?);
        Some(X { conn, root, active, state, fullscreen, pid })
    }

    fn ask(x: &X) -> Option<Option<bool>> {
        let window = x
            .conn
            .get_property(false, x.root, x.active, AtomEnum::WINDOW, 0, 1)
            .ok()?
            .reply()
            .ok()?
            .value32()?
            .next()
            .unwrap_or(0);
        if window == 0 {
            return Some(Some(false));
        }
        let pid = x.conn.get_property(false, window, x.pid, AtomEnum::CARDINAL, 0, 1).ok()?.reply().ok();
        if pid.and_then(|p| p.value32()?.next()) == Some(std::process::id()) {
            return Some(None);
        }
        let states = x.conn.get_property(false, window, x.state, AtomEnum::ATOM, 0, 64).ok()?.reply().ok()?;
        Some(Some(states.value32().is_some_and(|mut s| s.any(|a| a == x.fullscreen))))
    }

    pub fn full_screen() -> Option<bool> {
        let mut x = X.lock();
        if x.is_none() {
            *x = connect();
        }
        match ask(x.as_ref()?) {
            Some(answer) => answer,
            None => {
                // The connection broke: a new one next time.
                *x = None;
                Some(false)
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod os {
    pub fn full_screen() -> Option<bool> {
        Some(false)
    }
}
