use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn b(s: &str) -> Vec<u8> {
    s.as_bytes().to_vec()
}

/// Encode a key press as the bytes a terminal application expects on stdin.
pub fn key_bytes(key: KeyEvent, app_cursor: bool) -> Vec<u8> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let arrow = |c: char| {
        if app_cursor {
            format!("\x1bO{c}")
        } else {
            format!("\x1b[{c}")
        }
    };

    let body = match key.code {
        KeyCode::Char(c) if ctrl => match c.to_ascii_lowercase() {
            l @ 'a'..='z' => vec![l as u8 - b'a' + 1],
            '@' | ' ' => vec![0],
            '[' => vec![0x1b],
            '\\' => vec![0x1c],
            ']' => vec![0x1d],
            '^' => vec![0x1e],
            '_' => vec![0x1f],
            _ => c.to_string().into_bytes(),
        },
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => b("\r"),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => b("\t"),
        KeyCode::BackTab => b("\x1b[Z"),
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => b(&arrow('A')),
        KeyCode::Down => b(&arrow('B')),
        KeyCode::Right => b(&arrow('C')),
        KeyCode::Left => b(&arrow('D')),
        KeyCode::Home => b("\x1b[H"),
        KeyCode::End => b("\x1b[F"),
        KeyCode::PageUp => b("\x1b[5~"),
        KeyCode::PageDown => b("\x1b[6~"),
        KeyCode::Delete => b("\x1b[3~"),
        KeyCode::Insert => b("\x1b[2~"),
        KeyCode::F(n) => match n {
            1 => b("\x1bOP"),
            2 => b("\x1bOQ"),
            3 => b("\x1bOR"),
            4 => b("\x1bOS"),
            5 => b("\x1b[15~"),
            6 => b("\x1b[17~"),
            7 => b("\x1b[18~"),
            8 => b("\x1b[19~"),
            9 => b("\x1b[20~"),
            10 => b("\x1b[21~"),
            11 => b("\x1b[23~"),
            12 => b("\x1b[24~"),
            _ => return Vec::new(),
        },
        _ => return Vec::new(),
    };

    if alt {
        let mut out = vec![0x1b];
        out.extend(body);
        out
    } else {
        body
    }
}
