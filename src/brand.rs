//! The yurlungr brand: a 2.5D rainbow snake, red at the tail through violet at
//! the head, whose colours slowly shift. The picture is plain text in
//! `assets/snake.txt` (made by `scripts/make-snake.py`): a lit face, a darker
//! side and a faint shadow, told apart by their glyphs. Its colour is a
//! function of the column, the glyph and a time phase, so any program (the
//! installers are shell and PowerShell) can paint and animate it the same way
//! with no colour data of its own.

use std::{
    fmt::Write as _,
    io::{self, Write as _},
    thread,
    time::Duration,
};

/// The snake, as block and shade characters.
pub const SNAKE: &str = include_str!("../assets/snake.txt");

/// How bright each glyph class is painted: the lit face, the side, the shadow.
pub const SHADE_FACE: f64 = 1.0;
pub const SHADE_SIDE: f64 = 0.6;
pub const SHADE_SHADOW: f64 = 0.3;

/// The brightness of a snake glyph, or `None` for anything that is not paint.
pub fn shade(c: char) -> Option<f64> {
    match c {
        '█' | '▀' | '▄' => Some(SHADE_FACE),
        '▓' => Some(SHADE_SIDE),
        '░' => Some(SHADE_SHADOW),
        _ => None,
    }
}

/// Animation: one full turn of the rainbow over `FRAMES` frames.
const FRAMES: usize = 24;
const FRAME_TIME: Duration = Duration::from_millis(55);

pub const NAME: &str = "yurlungr";
pub const TAGLINE: &str = "a terminal workspace for coding agents";

/// How much colour the terminal can show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    None,
    Ansi256,
    Truecolor,
}

impl Color {
    /// Decide from the environment. `is_tty` is whether stdout is a terminal.
    pub fn detect(is_tty: bool, no_color: bool, colorterm: &str, term: &str) -> Color {
        if !is_tty || no_color || term == "dumb" {
            Color::None
        } else if colorterm.contains("truecolor") || colorterm.contains("24bit") {
            Color::Truecolor
        } else {
            Color::Ansi256
        }
    }

    pub fn from_env(is_tty: bool) -> Color {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        Color::detect(
            is_tty,
            std::env::var_os("NO_COLOR").is_some(),
            &var("COLORTERM"),
            &var("TERM"),
        )
    }
}

/// How far along the rainbow the head sits: red (hue 0) to magenta (about 300
/// degrees), stopping short of red so the two ends are distinct.
const HUE_SPAN: f64 = 0.83;

/// The rainbow colour at `position` in `0.0..=1.0`.
pub fn rainbow(position: f64) -> (u8, u8, u8) {
    rainbow_at(position, 0.0)
}

/// The rainbow at `position`, turned `phase` of the way round the hue circle.
/// Phase 0 (and every whole number) is the plain rainbow; raising it steadily
/// makes the colours flow along the snake.
pub fn rainbow_at(position: f64, phase: f64) -> (u8, u8, u8) {
    let turn = (position.clamp(0.0, 1.0) * HUE_SPAN + phase).rem_euclid(1.0);
    let hue = turn * 6.0; // 0..6
    let (saturation, value) = (0.9, 1.0);
    let sector = hue.floor() as i32;
    let f = hue - hue.floor();
    let (p, q, t) = (
        value * (1.0 - saturation),
        value * (1.0 - saturation * f),
        value * (1.0 - saturation * (1.0 - f)),
    );
    let (r, g, b) = match sector {
        0 => (value, t, p),
        1 => (q, value, p),
        2 => (p, value, t),
        3 => (p, q, value),
        4 => (t, p, value),
        _ => (value, p, q),
    };
    let byte = |c: f64| (c * 255.0).round() as u8;
    (byte(r), byte(g), byte(b))
}

/// The nearest xterm 256-colour index for an RGB colour (the 6x6x6 cube).
fn ansi256((r, g, b): (u8, u8, u8)) -> u8 {
    let level = |c: u8| (f64::from(c) / 255.0 * 5.0).round() as u8;
    16 + 36 * level(r) + 6 * level(g) + level(b)
}

fn paint(out: &mut String, color: Color, rgb: (u8, u8, u8)) {
    match color {
        Color::Truecolor => {
            let _ = write!(out, "\x1b[38;2;{};{};{}m", rgb.0, rgb.1, rgb.2);
        }
        Color::Ansi256 => {
            let _ = write!(out, "\x1b[38;5;{}m", ansi256(rgb));
        }
        Color::None => {}
    }
}

fn reset(out: &mut String, color: Color) {
    if color != Color::None {
        out.push_str("\x1b[0m");
    }
}

fn dim((r, g, b): (u8, u8, u8), k: f64) -> (u8, u8, u8) {
    let scale = |c: u8| (f64::from(c) * k).round() as u8;
    (scale(r), scale(g), scale(b))
}

/// `text` with the rainbow running across its width. Spaces stay uncoloured;
/// shade glyphs are painted darker, by class.
pub fn gradient(text: &str, width: usize, color: Color) -> String {
    gradient_at(text, width, color, 0.0)
}

pub fn gradient_at(text: &str, width: usize, color: Color, phase: f64) -> String {
    let mut out = String::new();
    let last = width.saturating_sub(1).max(1) as f64;
    for (i, ch) in text.chars().enumerate() {
        if ch != ' ' {
            let rgb = rainbow_at(i as f64 / last, phase);
            paint(&mut out, color, dim(rgb, shade(ch).unwrap_or(1.0)));
        }
        out.push(ch);
    }
    reset(&mut out, color);
    out
}

fn width() -> usize {
    SNAKE.lines().map(|l| l.chars().count()).max().unwrap_or(1)
}

/// The snake, the name and the tagline, ready to print.
pub fn banner(color: Color) -> String {
    banner_at(color, 0.0)
}

/// The banner with its colours turned `phase` of the way round the rainbow.
pub fn banner_at(color: Color, phase: f64) -> String {
    let w = width();
    let mut out = String::new();
    for line in SNAKE.lines() {
        out.push_str(&gradient_at(line, w, color, phase));
        out.push('\n');
    }
    out.push('\n');
    let name: String = NAME.chars().flat_map(|c| [c, ' ']).collect();
    let name = name.trim_end();
    out.push_str("  ");
    if color != Color::None {
        out.push_str("\x1b[1m");
    }
    out.push_str(&gradient_at(name, name.chars().count(), color, phase));
    out.push('\n');
    if color != Color::None {
        out.push_str(&format!("  \x1b[2m{TAGLINE}\x1b[0m\n"));
    } else {
        out.push_str(&format!("  {TAGLINE}\n"));
    }
    out
}

/// Whether the user turned the animation off (`YURLUNGR_NO_ANIM=1`).
fn motion_disabled() -> bool {
    std::env::var_os("YURLUNGR_NO_ANIM").is_some_and(|v| !v.is_empty() && v != "0")
}

/// Print the banner. With colour, and unless the animation is switched off, the
/// rainbow flows once around the snake first and settles on the plain banner,
/// which is what stays on screen (and all there is when output is piped).
pub fn show(color: Color) {
    let mut out = io::stdout().lock();
    let lines = banner(color).lines().count();
    // The cursor can only move back up over what is still on screen.
    let tall_enough =
        crossterm::terminal::size().is_ok_and(|(_, rows)| usize::from(rows) >= lines + 3);
    if color != Color::None && !motion_disabled() && tall_enough {
        let _ = write!(out, "\x1b[?25l"); // hide the cursor while it plays
        for frame in 0..FRAMES {
            let _ = write!(out, "{}\x1b[{lines}A", banner_at(color, frame as f64 / FRAMES as f64));
            let _ = out.flush();
            thread::sleep(FRAME_TIME);
        }
        let _ = write!(out, "\x1b[?25h");
    }
    let _ = write!(out, "{}", banner(color));
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_escapes(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for e in chars.by_ref() {
                    if e == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn the_rainbow_runs_from_red_to_violet() {
        let (r, g, b) = rainbow(0.0);
        assert!(r == 255 && g < 80 && b < 80, "red at the tail: {:?}", (r, g, b));
        let (r, g, b) = rainbow(0.4);
        assert!(g == 255 && r < 200 && b < 120, "green around the middle: {:?}", (r, g, b));
        let (r, g, b) = rainbow(1.0);
        assert!(b == 255 && r > 200 && g < 80, "violet-magenta at the head: {:?}", (r, g, b));
        assert_ne!(rainbow(0.0), rainbow(1.0), "the ends differ");
        assert_eq!(rainbow(-3.0), rainbow(0.0));
        assert_eq!(rainbow(9.0), rainbow(1.0));
    }

    #[test]
    fn every_hue_is_vivid() {
        for i in 0..=100 {
            let (r, g, b) = rainbow(f64::from(i) / 100.0);
            assert_eq!(r.max(g).max(b), 255, "full brightness at {i}%");
            assert!(r.min(g).min(b) <= 30, "strongly saturated at {i}%");
        }
    }

    #[test]
    fn colour_is_chosen_from_the_environment() {
        assert_eq!(Color::detect(true, false, "truecolor", "xterm"), Color::Truecolor);
        assert_eq!(Color::detect(true, false, "24bit", "xterm"), Color::Truecolor);
        assert_eq!(Color::detect(true, false, "", "xterm-256color"), Color::Ansi256);
        assert_eq!(
            Color::detect(false, false, "truecolor", "xterm"),
            Color::None,
            "not a terminal"
        );
        assert_eq!(Color::detect(true, true, "truecolor", "xterm"), Color::None, "NO_COLOR");
        assert_eq!(Color::detect(true, false, "", "dumb"), Color::None);
    }

    #[test]
    fn the_256_colour_fallback_stays_in_the_cube() {
        assert_eq!(ansi256((255, 0, 0)), 196);
        assert_eq!(ansi256((0, 255, 0)), 46);
        assert_eq!(ansi256((0, 0, 255)), 21);
        for i in 0..=20 {
            let n = ansi256(rainbow(f64::from(i) / 20.0));
            assert!((16..=231).contains(&n));
        }
    }

    #[test]
    fn a_plain_banner_has_no_escapes_and_matches_the_art() {
        let banner = banner(Color::None);
        assert!(!banner.contains('\x1b'));
        for line in SNAKE.lines() {
            assert!(banner.contains(line), "missing: {line:?}");
        }
        assert!(banner.contains("y u r l u n g r") && banner.contains(TAGLINE));
    }

    #[test]
    fn a_coloured_banner_colours_every_block_and_only_blocks() {
        let banner = banner(Color::Truecolor);
        assert!(banner.contains("\x1b[38;2;"));
        let art_part: String =
            banner.lines().take(SNAKE.lines().count()).collect::<Vec<_>>().join("\n");
        assert_eq!(
            strip_escapes(&art_part),
            SNAKE.trim_end(),
            "colouring never changes the picture"
        );
        let painted = SNAKE.chars().filter(|c| shade(*c).is_some()).count();
        assert_eq!(art_part.matches("\x1b[38;2;").count(), painted, "one colour code per glyph");
    }

    #[test]
    fn phase_zero_is_the_plain_rainbow_and_a_full_turn_comes_back() {
        for i in 0..=20 {
            let p = f64::from(i) / 20.0;
            assert_eq!(rainbow_at(p, 0.0), rainbow(p));
            assert_eq!(rainbow_at(p, 1.0), rainbow(p), "one whole turn is the same picture");
        }
        assert_ne!(rainbow_at(0.5, 0.0), rainbow_at(0.5, 0.25), "the colours move");
        assert_eq!(banner_at(Color::Truecolor, 0.0), banner(Color::Truecolor));
        assert_ne!(banner_at(Color::Truecolor, 0.3), banner(Color::Truecolor));
    }

    #[test]
    fn every_frame_keeps_the_picture_and_stays_vivid() {
        for frame in 0..FRAMES {
            let phase = frame as f64 / FRAMES as f64;
            let text = banner_at(Color::Truecolor, phase);
            assert_eq!(
                strip_escapes(
                    &text.lines().take(SNAKE.lines().count()).collect::<Vec<_>>().join("\n")
                ),
                SNAKE.trim_end()
            );
            let (r, g, b) = rainbow_at(0.5, phase);
            assert_eq!(r.max(g).max(b), 255, "vivid at frame {frame}");
        }
    }

    #[test]
    fn the_side_and_shadow_are_darker_than_the_face() {
        let bright = |c: char| {
            let s = gradient(&c.to_string(), 2, Color::Truecolor);
            let nums: Vec<u32> = s
                .trim_start_matches("\x1b[38;2;")
                .split('m')
                .next()
                .unwrap()
                .split(';')
                .map(|n| n.parse().unwrap())
                .collect();
            *nums.iter().max().unwrap()
        };
        assert_eq!(bright('█'), 255);
        assert_eq!(bright('▓'), (255.0 * SHADE_SIDE).round() as u32);
        assert_eq!(bright('░'), (255.0 * SHADE_SHADOW).round() as u32);
        assert!(shade('█') == shade('▀') && shade('▀') == shade('▄'));
        assert_eq!(shade('x'), None);
    }

    #[test]
    fn the_snake_has_a_face_a_side_and_a_shadow() {
        for glyph in ["█", "▀", "▄", "▓", "░"] {
            assert!(SNAKE.contains(glyph), "missing {glyph}");
        }
    }

    #[test]
    fn colour_depends_on_the_column_so_other_programs_can_reproduce_it() {
        // The first and last blocks of the picture are the two ends of the rainbow.
        let art = gradient("██", 2, Color::Truecolor);
        let codes: Vec<&str> = art.split('\x1b').filter(|s| s.starts_with("[38;2;")).collect();
        assert_eq!(codes.len(), 2);
        assert!(codes[0].starts_with("[38;2;255;"), "{codes:?}");
        assert_ne!(codes[0], codes[1]);
    }

    #[test]
    fn the_art_is_a_snake_of_a_sensible_size() {
        let rows = SNAKE.lines().count();
        assert!((5..=10).contains(&rows), "{rows} rows");
        assert!(width() <= 60, "fits an 80-column terminal with room to spare");
        assert!(
            SNAKE.chars().all(|c| "█▀▄▓░ \n".contains(c)),
            "only block and shade characters and spaces"
        );
    }

    /// The picture as the installers embed it: `#` full block, `^` upper half,
    /// `v` lower half, `%` side (dark shade), `:` shadow (light shade).
    fn ascii_coded() -> Vec<String> {
        SNAKE
            .lines()
            .map(|l| {
                l.chars()
                    .map(|c| match c {
                        '█' => '#',
                        '▀' => '^',
                        '▄' => 'v',
                        '▓' => '%',
                        '░' => ':',
                        _ => ' ',
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn the_shell_installer_embeds_the_same_snake() {
        let script = include_str!("../install.sh");
        let start = script.find("<<'SNAKE'\n").expect("the snake heredoc") + "<<'SNAKE'\n".len();
        let end = start + script[start..].find("\nSNAKE\n").expect("the end of the heredoc");
        let embedded: Vec<&str> = script[start..end].lines().collect();
        assert_eq!(embedded, ascii_coded(), "install.sh's snake drifted from assets/snake.txt");
        assert!(
            script.contains(&format!("rainbow {}", width())),
            "the width passed to the colouring must match the art"
        );
    }

    #[test]
    fn the_powershell_installer_embeds_the_same_snake() {
        let script = include_str!("../install.ps1");
        let start = script.find("$art = @(\n").expect("the art array") + "$art = @(\n".len();
        let end = start + script[start..].find("\n    )").expect("the end of the array");
        let embedded: Vec<String> = script[start..end]
            .lines()
            .map(|l| l.trim().trim_end_matches(',').trim_matches('\'').to_string())
            .collect();
        assert_eq!(embedded, ascii_coded(), "install.ps1's snake drifted from assets/snake.txt");
        assert!(script.contains(&format!("Write-Rainbow $line {}", width())));
        assert!(
            script.is_ascii(),
            "the script must be plain ASCII so `irm | iex` never mangles it"
        );
    }

    #[test]
    fn the_installers_use_the_same_rainbow_as_the_app() {
        // Same constants everywhere: 0.83 of the hue circle, 0.9 saturation (so the floor is 0.1).
        for script in [include_str!("../install.sh"), include_str!("../install.ps1")] {
            assert!(script.contains("0.83"), "hue span");
            assert!(script.contains("0.9"), "saturation");
        }
        assert!((HUE_SPAN - 0.83).abs() < 1e-9);
    }
}
