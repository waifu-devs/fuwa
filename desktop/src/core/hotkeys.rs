//! Keys the app hears while another program is in front, a game most of
//! all: push to talk, the game overlay's key (`ui/game_overlay.rs`), muting
//! and deafening. Nothing hooks the keyboard: a thread reads whether the
//! keys being watched are down, about a hundred times a second
//! (`GetAsyncKeyState` on Windows, `CGEventSourceKeyState` on macOS,
//! `QueryKeymap` on X11), so the game still gets every key, and no other key
//! is read. It sleeps while nothing is watched. Wayland lets no program see
//! keys meant for another, so there the keys work only while fuwa is in front.
//!
//! Combos are the keybinds' own (`core::keybinds`): a combo is down while its
//! key and every modifier it names are, whatever else is held, so push to
//! talk keeps working while you run with Shift.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::mpsc;

#[cfg(target_os = "linux")]
pub use os::x11;

/// How often the keys are read while something is watched.
const EVERY: Duration = Duration::from_millis(10);

/// Whether keys can be heard while another program is in front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    Yes,
    /// macOS, until fuwa is allowed under Input Monitoring.
    NotAllowed,
    /// Wayland (or no display at all): only while fuwa's window is in front.
    No,
}

/// A watched combo went down or came up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Press {
    /// The keybind action's id (`core::keybinds::ACTIONS`).
    pub action: &'static str,
    pub down: bool,
}

/// A modifier, as the keyboard has it (left and right alike).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Modifier {
    Control,
    Alt,
    Shift,
    /// Cmd on a Mac, the Windows key elsewhere.
    Super,
}

/// A combo as keys to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Combo {
    pub modifiers: Vec<Modifier>,
    /// In the web's names ("K", "Backquote", "F13").
    pub key: String,
}

impl Combo {
    /// Reads a keybind's combo ("Mod+Shift+M"); None if it isn't one.
    pub fn parse(combo: &str) -> Option<Self> {
        Self::parse_for(combo, cfg!(target_os = "macos"))
    }

    fn parse_for(combo: &str, mac: bool) -> Option<Self> {
        if !super::keybinds::valid(combo) {
            return None;
        }
        let mut parts: Vec<&str> = combo.split('+').collect();
        let key = parts.pop()?.to_owned();
        let mut modifiers = Vec::new();
        for part in parts {
            let m = match part {
                "Mod" if mac => Modifier::Super,
                "Mod" | "Ctrl" => Modifier::Control,
                "Meta" => Modifier::Super,
                "Alt" => Modifier::Alt,
                "Shift" => Modifier::Shift,
                _ => return None,
            };
            if !modifiers.contains(&m) {
                modifiers.push(m);
            }
        }
        Some(Self { modifiers, key })
    }
}

struct Watching {
    list: Vec<(&'static str, Combo)>,
    /// Goes up with every new list, so the keyboard's layout is read again.
    version: u64,
}

pub struct Hotkeys {
    watching: Arc<Mutex<Watching>>,
    version: AtomicU64,
    thread: std::thread::Thread,
    presses: Mutex<Option<mpsc::UnboundedReceiver<Press>>>,
}

impl Hotkeys {
    /// Starts the thread that reads the keys; it sleeps until something's watched.
    pub fn start() -> Arc<Self> {
        let watching = Arc::new(Mutex::new(Watching { list: Vec::new(), version: 0 }));
        let (tx, rx) = mpsc::unbounded_channel();
        let shared = watching.clone();
        let thread = std::thread::Builder::new()
            .name("fuwa-hotkeys".into())
            .spawn(move || listen(&shared, &tx))
            .expect("couldn't start the hotkeys thread")
            .thread()
            .clone();
        Arc::new(Self { watching, version: AtomicU64::new(0), thread, presses: Mutex::new(Some(rx)) })
    }

    /// What's pressed and let go, for the window; only the first caller gets them.
    pub fn presses(&self) -> Option<mpsc::UnboundedReceiver<Press>> {
        self.presses.lock().take()
    }

    /// Watches these actions' combos from now on, and nothing else. A combo
    /// held as it stops being watched is let go.
    pub fn watch(&self, list: Vec<(&'static str, Combo)>) {
        let mut watching = self.watching.lock();
        if watching.list == list {
            return;
        }
        watching.list = list;
        watching.version = self.version.fetch_add(1, Ordering::Relaxed) + 1;
        drop(watching);
        self.thread.unpark();
    }

    /// Whether an action's combo is being watched now.
    pub fn watches(&self, action: &str) -> bool {
        self.watching.lock().list.iter().any(|(a, _)| *a == action)
    }

    /// Whether keys can be heard while another program is in front, here.
    pub fn reach() -> Reach {
        os::reach()
    }

    /// Asks the system to let fuwa hear keys (macOS's Input Monitoring): the
    /// system's own question the first time, its settings after that.
    pub fn ask() {
        os::ask();
    }
}

fn listen(watching: &Mutex<Watching>, tx: &mpsc::UnboundedSender<Press>) {
    let mut keyboard: Option<(u64, os::Keyboard)> = None;
    let mut down: Vec<(&'static str, Combo)> = Vec::new();
    let (mut list, mut version) = (Vec::new(), u64::MAX);
    loop {
        // A copy of what's watched, taken again only when it changes.
        {
            let w = watching.lock();
            if w.version != version {
                (list, version) = (w.list.clone(), w.version);
            }
        }
        // Let go of anything held that isn't watched any more.
        down.retain(|(action, combo)| {
            let kept = list.iter().any(|(a, c)| a == action && c == combo);
            if !kept {
                let _ = tx.send(Press { action, down: false });
            }
            kept
        });
        if list.is_empty() {
            keyboard = None;
            std::thread::park_timeout(Duration::from_secs(5));
            continue;
        }
        if keyboard.as_ref().is_none_or(|(v, _)| *v != version) {
            keyboard = os::Keyboard::open().map(|k| (version, k));
        }
        let Some((_, kb)) = keyboard.as_mut() else {
            // No keyboard to read (Wayland, or the display went): try again in a while.
            std::thread::park_timeout(Duration::from_secs(2));
            continue;
        };
        if !kb.read() {
            keyboard = None;
            continue;
        }
        for (action, combo) in &list {
            let now = combo.modifiers.iter().all(|m| kb.modifier(*m)) && kb.key(&combo.key);
            // An action may have more than one combo (custom keybinds): each goes down and up alone.
            let was = down.iter().any(|(a, c)| a == action && c == combo);
            if now && !was {
                down.push((action, combo.clone()));
                let _ = tx.send(Press { action, down: true });
            } else if !now && was {
                down.retain(|(a, c)| !(a == action && c == combo));
                let _ = tx.send(Press { action, down: false });
            }
        }
        std::thread::sleep(EVERY);
    }
}

/// A key's Windows virtual-key code, from its name in combos.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_vk(key: &str) -> Option<i32> {
    let code = match key {
        "ArrowUp" => 0x26,
        "ArrowDown" => 0x28,
        "ArrowLeft" => 0x25,
        "ArrowRight" => 0x27,
        "Escape" => 0x1B,
        "Tab" => 0x09,
        "Enter" => 0x0D,
        "Backspace" => 0x08,
        "Delete" => 0x2E,
        "Home" => 0x24,
        "End" => 0x23,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "Insert" => 0x2D,
        "Space" => 0x20,
        "Comma" => 0xBC,
        "Period" => 0xBE,
        "Slash" => 0xBF,
        "Backslash" => 0xDC,
        "Semicolon" => 0xBA,
        "Quote" => 0xDE,
        "BracketLeft" => 0xDB,
        "BracketRight" => 0xDD,
        "Minus" => 0xBD,
        "Equal" => 0xBB,
        "Backquote" => 0xC0,
        _ => return function_key(key).map(|n| 0x70 + n - 1).or_else(|| letter_or_digit(key).map(i32::from)),
    };
    Some(code)
}

/// A key's macOS virtual key code (`kVK_*` in Carbon's Events.h).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn mac_keycode(key: &str) -> Option<u16> {
    const LETTERS: [u16; 26] = [
        0x00, 0x0B, 0x08, 0x02, 0x0E, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2E, 0x2D, 0x1F, 0x23, 0x0C, 0x0F,
        0x01, 0x11, 0x20, 0x09, 0x0D, 0x07, 0x10, 0x06,
    ];
    const DIGITS: [u16; 10] = [0x1D, 0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C, 0x19];
    const FUNCTION: [u16; 20] = [
        0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F, 0x69, 0x6B, 0x71, 0x6A, 0x40, 0x4F,
        0x50, 0x5A,
    ];
    let code = match key {
        "ArrowUp" => 0x7E,
        "ArrowDown" => 0x7D,
        "ArrowLeft" => 0x7B,
        "ArrowRight" => 0x7C,
        "Escape" => 0x35,
        "Tab" => 0x30,
        "Enter" => 0x24,
        "Backspace" => 0x33,
        "Delete" => 0x75,
        "Home" => 0x73,
        "End" => 0x77,
        "PageUp" => 0x74,
        "PageDown" => 0x79,
        "Insert" => 0x72,
        "Space" => 0x31,
        "Comma" => 0x2B,
        "Period" => 0x2F,
        "Slash" => 0x2C,
        "Backslash" => 0x2A,
        "Semicolon" => 0x29,
        "Quote" => 0x27,
        "BracketLeft" => 0x21,
        "BracketRight" => 0x1E,
        "Minus" => 0x1B,
        "Equal" => 0x18,
        "Backquote" => 0x32,
        _ => {
            if let Some(n) = function_key(key) {
                return FUNCTION.get(usize::try_from(n - 1).ok()?).copied();
            }
            let c = letter_or_digit(key)?;
            return if c.is_ascii_digit() {
                Some(DIGITS[usize::from(c - b'0')])
            } else {
                Some(LETTERS[usize::from(c - b'A')])
            };
        }
    };
    Some(code)
}

/// A key's X11 keysym (X11/keysymdef.h).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn x11_keysym(key: &str) -> Option<u32> {
    let sym = match key {
        "ArrowUp" => 0xff52,
        "ArrowDown" => 0xff54,
        "ArrowLeft" => 0xff51,
        "ArrowRight" => 0xff53,
        "Escape" => 0xff1b,
        "Tab" => 0xff09,
        "Enter" => 0xff0d,
        "Backspace" => 0xff08,
        "Delete" => 0xffff,
        "Home" => 0xff50,
        "End" => 0xff57,
        "PageUp" => 0xff55,
        "PageDown" => 0xff56,
        "Insert" => 0xff63,
        "Space" => 0x20,
        "Comma" => 0x2c,
        "Period" => 0x2e,
        "Slash" => 0x2f,
        "Backslash" => 0x5c,
        "Semicolon" => 0x3b,
        "Quote" => 0x27,
        "BracketLeft" => 0x5b,
        "BracketRight" => 0x5d,
        "Minus" => 0x2d,
        "Equal" => 0x3d,
        "Backquote" => 0x60,
        _ => {
            if let Some(n) = function_key(key) {
                return Some(0xffbe + u32::try_from(n - 1).ok()?);
            }
            // Letters' keysyms are the lowercase ones.
            return letter_or_digit(key).map(|c| u32::from(c.to_ascii_lowercase()));
        }
    };
    Some(sym)
}

/// "F1" to "F24" as 1 to 24.
fn function_key(key: &str) -> Option<i32> {
    let n: i32 = key.strip_prefix('F')?.parse().ok()?;
    (1..=24).contains(&n).then_some(n)
}

/// "A" to "Z" and "0" to "9" as their ASCII byte (letters uppercase).
fn letter_or_digit(key: &str) -> Option<u8> {
    match key.as_bytes() {
        [c] if c.is_ascii_uppercase() || c.is_ascii_digit() => Some(*c),
        _ => None,
    }
}

#[cfg(windows)]
mod os {
    use super::{Modifier, Reach, windows_vk};
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

    pub fn reach() -> Reach {
        Reach::Yes
    }

    pub fn ask() {}

    pub struct Keyboard;

    fn down(vk: i32) -> bool {
        // SAFETY: only reads the key's state; the high bit is "down now".
        unsafe { GetAsyncKeyState(vk) < 0 }
    }

    impl Keyboard {
        pub fn open() -> Option<Self> {
            Some(Self)
        }

        pub fn read(&mut self) -> bool {
            true
        }

        pub fn modifier(&self, m: Modifier) -> bool {
            match m {
                Modifier::Control => down(0x11),
                Modifier::Alt => down(0x12),
                Modifier::Shift => down(0x10),
                Modifier::Super => down(0x5B) || down(0x5C),
            }
        }

        pub fn key(&self, key: &str) -> bool {
            windows_vk(key).is_some_and(down)
        }
    }
}

#[cfg(target_os = "macos")]
mod os {
    use super::{Modifier, Reach, mac_keycode};

    /// `kCGEventSourceStateHIDSystemState`: the keyboard itself, whoever's in front.
    const HID_SYSTEM_STATE: i32 = 1;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceKeyState(state: i32, key: u16) -> bool;
        fn CGPreflightListenEventAccess() -> bool;
        fn CGRequestListenEventAccess() -> bool;
    }

    pub fn reach() -> Reach {
        // SAFETY: asks whether the app may listen; no arguments.
        if unsafe { CGPreflightListenEventAccess() } { Reach::Yes } else { Reach::NotAllowed }
    }

    pub fn ask() {
        // SAFETY: shows the system's question once; after that it only answers.
        if !unsafe { CGRequestListenEventAccess() } {
            let _ = open::that_detached("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent");
        }
    }

    pub struct Keyboard;

    fn down(code: u16) -> bool {
        // SAFETY: only reads the key's state.
        unsafe { CGEventSourceKeyState(HID_SYSTEM_STATE, code) }
    }

    impl Keyboard {
        pub fn open() -> Option<Self> {
            Some(Self)
        }

        pub fn read(&mut self) -> bool {
            true
        }

        pub fn modifier(&self, m: Modifier) -> bool {
            // Each side's key: kVK_Control, kVK_Option, kVK_Shift, kVK_Command and their right ones.
            match m {
                Modifier::Control => down(0x3B) || down(0x3E),
                Modifier::Alt => down(0x3A) || down(0x3D),
                Modifier::Shift => down(0x38) || down(0x3C),
                Modifier::Super => down(0x37) || down(0x36),
            }
        }

        pub fn key(&self, key: &str) -> bool {
            mac_keycode(key).is_some_and(down)
        }
    }
}

#[cfg(target_os = "linux")]
mod os {
    use std::collections::HashMap;

    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;

    use super::{Modifier, Reach, x11_keysym};

    /// Whether this session is X11 (GPUI picks Wayland whenever it's there).
    pub fn x11() -> bool {
        let set = |name| std::env::var_os(name).is_some_and(|v| !v.is_empty());
        !set("WAYLAND_DISPLAY") && set("DISPLAY")
    }

    pub fn reach() -> Reach {
        if x11() { Reach::Yes } else { Reach::No }
    }

    pub fn ask() {}

    pub struct Keyboard {
        conn: RustConnection,
        /// Each keysym's keycodes, from the keyboard's layout.
        codes: HashMap<u32, Vec<u8>>,
        /// One bit per keycode, down or not, as of the last read.
        bits: [u8; 32],
    }

    impl Keyboard {
        pub fn open() -> Option<Self> {
            if !x11() {
                return None;
            }
            let (conn, _) = RustConnection::connect(None).ok()?;
            let setup = conn.setup();
            let (min, max) = (setup.min_keycode, setup.max_keycode);
            let mapping = conn.get_keyboard_mapping(min, max - min + 1).ok()?.reply().ok()?;
            let per = usize::from(mapping.keysyms_per_keycode.max(1));
            let mut codes: HashMap<u32, Vec<u8>> = HashMap::new();
            for (n, syms) in mapping.keysyms.chunks(per).enumerate() {
                let Ok(code) = u8::try_from(usize::from(min) + n) else { break };
                for &sym in syms.iter().filter(|s| **s != 0) {
                    let list = codes.entry(sym).or_default();
                    if !list.contains(&code) {
                        list.push(code);
                    }
                }
            }
            Some(Self { conn, codes, bits: [0; 32] })
        }

        pub fn read(&mut self) -> bool {
            match self.conn.query_keymap().ok().and_then(|c| c.reply().ok()) {
                Some(reply) => {
                    self.bits = reply.keys;
                    true
                }
                None => false,
            }
        }

        fn sym_down(&self, sym: u32) -> bool {
            self.codes
                .get(&sym)
                .is_some_and(|codes| codes.iter().any(|&c| self.bits[usize::from(c / 8)] & (1 << (c % 8)) != 0))
        }

        pub fn modifier(&self, m: Modifier) -> bool {
            let syms: &[u32] = match m {
                Modifier::Control => &[0xffe3, 0xffe4],
                Modifier::Alt => &[0xffe9, 0xffea],
                Modifier::Shift => &[0xffe1, 0xffe2],
                Modifier::Super => &[0xffeb, 0xffec],
            };
            syms.iter().any(|&s| self.sym_down(s))
        }

        pub fn key(&self, key: &str) -> bool {
            x11_keysym(key).is_some_and(|s| self.sym_down(s))
        }
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod os {
    use super::{Modifier, Reach};

    pub fn reach() -> Reach {
        Reach::No
    }

    pub fn ask() {}

    pub struct Keyboard;

    impl Keyboard {
        pub fn open() -> Option<Self> {
            None
        }

        pub fn read(&mut self) -> bool {
            false
        }

        pub fn modifier(&self, _: Modifier) -> bool {
            false
        }

        pub fn key(&self, _: &str) -> bool {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_read_as_the_keys_each_system_has() {
        let c = Combo::parse_for("Mod+Shift+M", false).unwrap();
        assert_eq!(c.modifiers, [Modifier::Control, Modifier::Shift]);
        assert_eq!(c.key, "M");
        assert_eq!(Combo::parse_for("Mod+K", true).unwrap().modifiers, [Modifier::Super]);
        assert_eq!(Combo::parse_for("Ctrl+K", true).unwrap().modifiers, [Modifier::Control]);
        assert_eq!(Combo::parse_for("Backquote", false).unwrap().modifiers, []);
        assert!(Combo::parse_for("Shift", false).is_none());
        assert!(Combo::parse_for("Hyper+K", false).is_none());
    }

    #[test]
    fn every_key_a_combo_can_name_has_a_code_everywhere() {
        let named = [
            "ArrowUp",
            "ArrowDown",
            "ArrowLeft",
            "ArrowRight",
            "Escape",
            "Tab",
            "Enter",
            "Backspace",
            "Delete",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "Insert",
            "Space",
            "Comma",
            "Period",
            "Slash",
            "Backslash",
            "Semicolon",
            "Quote",
            "BracketLeft",
            "BracketRight",
            "Minus",
            "Equal",
            "Backquote",
        ];
        let letters = (b'A'..=b'Z').chain(b'0'..=b'9').map(|c| (c as char).to_string());
        let function = (1..=20).map(|n| format!("F{n}"));
        for key in named.iter().map(|s| (*s).to_owned()).chain(letters).chain(function) {
            assert!(windows_vk(&key).is_some(), "{key} on Windows");
            assert!(mac_keycode(&key).is_some(), "{key} on macOS");
            assert!(x11_keysym(&key).is_some(), "{key} on X11");
        }
        // Function keys past F20 have no key on a Mac keyboard.
        assert!(windows_vk("F24").is_some() && mac_keycode("F24").is_none());
        assert_eq!(windows_vk("A"), Some(0x41));
        assert_eq!(windows_vk("7"), Some(0x37));
        assert_eq!(windows_vk("F1"), Some(0x70));
        assert_eq!(mac_keycode("A"), Some(0x00));
        assert_eq!(mac_keycode("Z"), Some(0x06));
        assert_eq!(mac_keycode("0"), Some(0x1D));
        assert_eq!(mac_keycode("F1"), Some(0x7A));
        assert_eq!(x11_keysym("A"), Some(0x61));
        assert_eq!(x11_keysym("F12"), Some(0xffc9));
        assert!(windows_vk("Shift").is_none() && windows_vk("F0").is_none() && windows_vk("AB").is_none());
    }
}
