//! What colours the host terminal can show. The server always renders with the
//! colours of the theme (often truecolor); the client lowers them to what its
//! own terminal understands, so the UI stays readable anywhere.

use ratatui::style::Color;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorLevel {
    Ansi16,
    Ansi256,
    TrueColor,
}

impl ColorLevel {
    pub fn detect() -> Self {
        Self::from_env(|k| std::env::var(k).ok(), cfg!(windows))
    }

    /// `YURLUNGR_COLORS=truecolor|256|16` overrides the guess.
    fn from_env(get: impl Fn(&str) -> Option<String>, windows: bool) -> Self {
        let has = |key: &str, needles: &[&str]| {
            get(key).is_some_and(|v| {
                let v = v.to_lowercase();
                needles.iter().any(|n| v.contains(n))
            })
        };
        match get("YURLUNGR_COLORS").map(|v| v.to_lowercase()).as_deref() {
            Some("truecolor" | "24bit") => return Self::TrueColor,
            Some("256") => return Self::Ansi256,
            Some("16") => return Self::Ansi16,
            _ => {}
        }
        if windows
            || has("COLORTERM", &["truecolor", "24bit"])
            || has("TERM", &["direct", "truecolor", "kitty", "alacritty", "wezterm", "ghostty"])
            || has("TERM_PROGRAM", &["iterm", "wezterm", "vscode", "ghostty"])
            || get("WT_SESSION").is_some()
            || get("KITTY_WINDOW_ID").is_some()
        {
            return Self::TrueColor;
        }
        let term = get("TERM").unwrap_or_default().to_lowercase();
        if term.contains("256") {
            return Self::Ansi256;
        }
        if matches!(term.as_str(), "linux" | "dumb" | "ansi" | "cons25") || term.starts_with("vt") {
            return Self::Ansi16;
        }
        Self::Ansi256
    }

    pub fn apply(self, c: Color) -> Color {
        match (self, c) {
            (Self::TrueColor, _) => c,
            (Self::Ansi256, Color::Rgb(r, g, b)) => Color::Indexed(nearest_256(r, g, b)),
            (Self::Ansi16, Color::Rgb(r, g, b)) => to_16(r, g, b),
            (Self::Ansi16, Color::Indexed(n)) if n >= 16 => {
                let (r, g, b) = indexed_rgb(n);
                to_16(r, g, b)
            }
            _ => c,
        }
    }
}

const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn cube_step(v: u8) -> usize {
    (0..6).min_by_key(|&i| (i32::from(CUBE[i]) - i32::from(v)).abs()).unwrap_or(0)
}

fn dist(a: (u8, u8, u8), b: (u8, u8, u8)) -> i32 {
    let d = |x: u8, y: u8| (i32::from(x) - i32::from(y)).pow(2);
    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
}

/// The closest of the 6x6x6 colour cube and the 24 greys.
fn nearest_256(r: u8, g: u8, b: u8) -> u8 {
    let (ri, gi, bi) = (cube_step(r), cube_step(g), cube_step(b));
    let cube = (CUBE[ri], CUBE[gi], CUBE[bi]);
    let cube_idx = 16 + 36 * ri + 6 * gi + bi;

    let avg = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    let grey_step = ((avg.saturating_sub(8) + 5) / 10).min(23) as u8;
    let level = 8 + 10 * grey_step;
    let grey = (level, level, level);

    if dist((r, g, b), grey) < dist((r, g, b), cube) {
        232 + grey_step
    } else {
        cube_idx as u8
    }
}

/// RGB of an extended palette entry (16..=255).
fn indexed_rgb(n: u8) -> (u8, u8, u8) {
    if n >= 232 {
        let v = 8 + 10 * (n - 232);
        (v, v, v)
    } else {
        let i = usize::from(n.saturating_sub(16));
        (CUBE[i / 36], CUBE[i / 6 % 6], CUBE[i % 6])
    }
}

/// Map to the 16 named colours: greys by brightness, others by dominant channels.
fn to_16(r: u8, g: u8, b: u8) -> Color {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max - min < 32 {
        let lum = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
        return match lum {
            0..48 => Color::Black,
            48..112 => Color::DarkGray,
            112..192 => Color::Gray,
            _ => Color::White,
        };
    }
    let half = max / 2;
    let bright = max >= 192;
    match (r > half, g > half, b > half, bright) {
        (true, false, false, false) => Color::Red,
        (true, false, false, true) => Color::LightRed,
        (false, true, false, false) => Color::Green,
        (false, true, false, true) => Color::LightGreen,
        (true, true, false, false) => Color::Yellow,
        (true, true, false, true) => Color::LightYellow,
        (false, false, true, false) => Color::Blue,
        (false, false, true, true) => Color::LightBlue,
        (true, false, true, false) => Color::Magenta,
        (true, false, true, true) => Color::LightMagenta,
        (false, true, true, false) => Color::Cyan,
        (false, true, true, true) => Color::LightCyan,
        (_, _, _, false) => Color::Gray,
        (_, _, _, true) => Color::White,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level(vars: &[(&str, &str)], windows: bool) -> ColorLevel {
        ColorLevel::from_env(
            |k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string()),
            windows,
        )
    }

    #[test]
    fn detects_the_colour_depth() {
        use ColorLevel::*;
        assert_eq!(level(&[("COLORTERM", "truecolor")], false), TrueColor);
        assert_eq!(level(&[("TERM", "xterm-256color")], false), Ansi256);
        assert_eq!(level(&[("TERM", "xterm-kitty")], false), TrueColor);
        assert_eq!(level(&[("TERM", "linux")], false), Ansi16);
        assert_eq!(level(&[("TERM", "screen")], false), Ansi256);
        assert_eq!(level(&[], true), TrueColor);
        assert_eq!(level(&[("WT_SESSION", "x"), ("TERM", "xterm")], false), TrueColor);
        assert_eq!(level(&[("COLORTERM", "truecolor"), ("YURLUNGR_COLORS", "16")], false), Ansi16);
    }

    #[test]
    fn rgb_lowers_to_the_nearest_palette_entry() {
        assert_eq!(nearest_256(255, 0, 0), 196);
        assert_eq!(nearest_256(0, 0, 0), 16);
        assert!((232..=255).contains(&nearest_256(100, 100, 100)));
        assert_eq!(ColorLevel::Ansi256.apply(Color::Rgb(255, 0, 0)), Color::Indexed(196));
        assert_eq!(ColorLevel::Ansi256.apply(Color::Red), Color::Red);
        assert_eq!(ColorLevel::TrueColor.apply(Color::Rgb(1, 2, 3)), Color::Rgb(1, 2, 3));
    }

    #[test]
    fn sixteen_colour_mapping() {
        let a = ColorLevel::Ansi16;
        assert_eq!(a.apply(Color::Rgb(20, 20, 30)), Color::Black);
        assert_eq!(a.apply(Color::Rgb(100, 100, 100)), Color::DarkGray);
        assert_eq!(a.apply(Color::Rgb(255, 40, 40)), Color::LightRed);
        assert_eq!(a.apply(Color::Rgb(40, 40, 200)), Color::LightBlue);
        assert_eq!(a.apply(Color::Indexed(196)), Color::LightRed);
        assert_eq!(a.apply(Color::Indexed(4)), Color::Indexed(4));
    }
}
