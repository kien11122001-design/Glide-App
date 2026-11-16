//! Pure hotkey-string parser (`"ctrl+alt+s"`). Produces Win32 virtual-key
//! codes as plain numbers so this stays testable on any OS.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    /// Windows virtual-key code.
    pub vk: u32,
}

fn key_code(name: &str) -> Option<u32> {
    let b = name.as_bytes();
    if b.len() == 1 {
        return match b[0] {
            c @ b'a'..=b'z' => Some(u32::from(c - b'a') + 0x41),
            c @ b'0'..=b'9' => Some(u32::from(c - b'0') + 0x30),
            _ => None,
        };
    }
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return Some(0x70 + n - 1);
        }
    }
    Some(match name {
        "space" => 0x20,
        "tab" => 0x09,
        "enter" | "return" => 0x0D,
        "esc" | "escape" => 0x1B,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "end" => 0x23,
        "home" => 0x24,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "insert" => 0x2D,
        "delete" | "del" => 0x2E,
        _ => return None,
    })
}

/// Parse `"ctrl+alt+s"`. Requires at least one modifier and exactly one key
/// (a modifier-less global hotkey would hijack a normal key).
pub fn parse_hotkey(text: &str) -> Option<Hotkey> {
    let mut hk = Hotkey {
        ctrl: false,
        alt: false,
        shift: false,
        win: false,
        vk: 0,
    };
    let mut key = None;
    for part in text.split('+') {
        let p = part.trim().to_ascii_lowercase();
        match p.as_str() {
            "ctrl" | "control" => hk.ctrl = true,
            "alt" => hk.alt = true,
            "shift" => hk.shift = true,
            "win" | "super" | "meta" => hk.win = true,
            other => {
                if key.is_some() {
                    return None;
                }
                key = Some(key_code(other)?);
            }
        }
    }
    hk.vk = key?;
    (hk.ctrl || hk.alt || hk.shift || hk.win).then_some(hk)
}
