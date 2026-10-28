use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

fn button_code(b: MouseButton) -> u16 {
    match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

/// Encode a mouse event for an application that enabled mouse reporting.
/// `col`/`row` are zero-based cell offsets inside the pane. Returns `None` when
/// the application's mouse mode does not ask for this kind of event.
pub fn encode(
    kind: MouseEventKind,
    mods: KeyModifiers,
    col: u16,
    row: u16,
    mode: MouseProtocolMode,
    enc: MouseProtocolEncoding,
) -> Option<Vec<u8>> {
    use MouseProtocolMode::*;

    let (base, motion, release) = match kind {
        MouseEventKind::Down(b) => (button_code(b), false, false),
        MouseEventKind::Up(b) => (button_code(b), false, true),
        MouseEventKind::Drag(b) => (button_code(b), true, false),
        MouseEventKind::Moved => (3, true, false),
        MouseEventKind::ScrollUp => (64, false, false),
        MouseEventKind::ScrollDown => (65, false, false),
        MouseEventKind::ScrollLeft => (66, false, false),
        MouseEventKind::ScrollRight => (67, false, false),
    };
    let wheel = base >= 64;
    let wanted = match mode {
        None => false,
        Press => !release && !motion,
        PressRelease => !motion,
        ButtonMotion => !matches!(kind, MouseEventKind::Moved),
        AnyMotion => true,
    };
    if !wanted {
        return Option::None;
    }

    let mut code = base;
    if motion {
        code |= 32;
    }
    if mods.contains(KeyModifiers::SHIFT) {
        code |= 4;
    }
    if mods.contains(KeyModifiers::ALT) {
        code |= 8;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        code |= 16;
    }

    match enc {
        MouseProtocolEncoding::Sgr => {
            let end = if release { 'm' } else { 'M' };
            Some(format!("\x1b[<{};{};{}{}", code, col + 1, row + 1, end).into_bytes())
        }
        MouseProtocolEncoding::Default | MouseProtocolEncoding::Utf8 => {
            // Legacy encodings report every release as button 3.
            let code = if release && !wheel { (code & !3) | 3 } else { code };
            let vals = [code + 32, col + 33, row + 33];
            let mut out = b"\x1b[M".to_vec();
            for v in vals {
                if matches!(enc, MouseProtocolEncoding::Utf8) {
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(
                        char::from_u32(v as u32)?.encode_utf8(&mut buf).as_bytes(),
                    );
                } else {
                    out.push(u8::try_from(v).ok()?);
                }
            }
            Some(out)
        }
    }
}
