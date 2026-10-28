//! Colour themes for yurlungr's own chrome (borders, dock, dialogs, status
//! colours). Terminal panes keep the colours their programs draw.

use std::collections::HashMap;

use ratatui::style::Color;

#[derive(Clone, Copy)]
pub struct Theme {
    /// Focused border, menu borders, dialog frames.
    pub accent: Color,
    pub border: Color,
    pub working: Color,
    pub blocked: Color,
    pub idle: Color,
    /// Folders in the file browser.
    pub dir: Color,
    /// Clipboard markers, prompt labels, the current line number.
    pub marker: Color,
    pub danger: Color,
    pub graph_lane: Color,
    pub graph_hash: Color,
    pub graph_ref: Color,
    pub button_new: Color,
    pub button_menu: Color,
    /// Text on the dock buttons.
    pub button_fg: Color,
    pub dim: Color,
    /// Current-line highlight in the editor's view mode.
    pub line_bg: Color,
}

pub const NAMES: [&str; 8] =
    ["default", "light", "dracula", "nord", "gruvbox", "catppuccin", "solarized", "mono"];

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Colours in field order: accent, border, working, blocked, idle, dir, marker,
/// danger, graph lane/hash/ref, button new/menu/fg, dim, line_bg.
const fn palette(c: [u32; 16]) -> Theme {
    Theme {
        accent: rgb(c[0]),
        border: rgb(c[1]),
        working: rgb(c[2]),
        blocked: rgb(c[3]),
        idle: rgb(c[4]),
        dir: rgb(c[5]),
        marker: rgb(c[6]),
        danger: rgb(c[7]),
        graph_lane: rgb(c[8]),
        graph_hash: rgb(c[9]),
        graph_ref: rgb(c[10]),
        button_new: rgb(c[11]),
        button_menu: rgb(c[12]),
        button_fg: rgb(c[13]),
        dim: rgb(c[14]),
        line_bg: rgb(c[15]),
    }
}

pub fn builtin(name: &str) -> Option<Theme> {
    Some(match name {
        // Named ANSI colours: follows the terminal's own palette.
        "default" => Theme {
            accent: Color::Cyan,
            border: Color::DarkGray,
            working: Color::Green,
            blocked: Color::Yellow,
            idle: Color::DarkGray,
            dir: Color::Blue,
            marker: Color::Yellow,
            danger: Color::Red,
            graph_lane: Color::Green,
            graph_hash: Color::Yellow,
            graph_ref: Color::Magenta,
            button_new: Color::Green,
            button_menu: Color::Cyan,
            button_fg: Color::Black,
            dim: Color::DarkGray,
            line_bg: Color::DarkGray,
        },
        // For light terminal backgrounds.
        "light" => Theme {
            accent: Color::Blue,
            border: rgb(0x969696),
            working: rgb(0x007800),
            blocked: rgb(0xa06400),
            idle: rgb(0x969696),
            dir: Color::Blue,
            marker: rgb(0xa05000),
            danger: Color::Red,
            graph_lane: rgb(0x007800),
            graph_hash: rgb(0xa06400),
            graph_ref: Color::Magenta,
            button_new: rgb(0x008200),
            button_menu: Color::Blue,
            button_fg: Color::White,
            dim: rgb(0x808080),
            line_bg: rgb(0xe1e1e1),
        },
        "dracula" => palette([
            0xbd93f9, 0x6272a4, 0x50fa7b, 0xf1fa8c, 0x6272a4, 0x8be9fd, 0xffb86c, 0xff5555,
            0x50fa7b, 0xf1fa8c, 0xff79c6, 0x50fa7b, 0xbd93f9, 0x282a36, 0x6272a4, 0x44475a,
        ]),
        "nord" => palette([
            0x88c0d0, 0x4c566a, 0xa3be8c, 0xebcb8b, 0x4c566a, 0x81a1c1, 0xd08770, 0xbf616a,
            0xa3be8c, 0xebcb8b, 0xb48ead, 0xa3be8c, 0x88c0d0, 0x2e3440, 0x616e88, 0x3b4252,
        ]),
        "gruvbox" => palette([
            0x83a598, 0x665c54, 0xb8bb26, 0xfabd2f, 0x928374, 0x83a598, 0xfe8019, 0xfb4934,
            0xb8bb26, 0xfabd2f, 0xd3869b, 0xb8bb26, 0x83a598, 0x282828, 0x928374, 0x3c3836,
        ]),
        "catppuccin" => palette([
            0x89b4fa, 0x585b70, 0xa6e3a1, 0xf9e2af, 0x6c7086, 0x89b4fa, 0xfab387, 0xf38ba8,
            0xa6e3a1, 0xf9e2af, 0xcba6f7, 0xa6e3a1, 0x89b4fa, 0x1e1e2e, 0x6c7086, 0x313244,
        ]),
        "solarized" => palette([
            0x268bd2, 0x586e75, 0x859900, 0xb58900, 0x586e75, 0x268bd2, 0xcb4b16, 0xdc322f,
            0x859900, 0xb58900, 0xd33682, 0x859900, 0x268bd2, 0x002b36, 0x586e75, 0x073642,
        ]),
        // Greys only.
        "mono" => palette([
            0xffffff, 0x585858, 0xffffff, 0xd0d0d0, 0x808080, 0xffffff, 0xd0d0d0, 0xffffff,
            0xb0b0b0, 0xd0d0d0, 0xffffff, 0xe0e0e0, 0xb0b0b0, 0x000000, 0x808080, 0x303030,
        ]),
        _ => return None,
    })
}

impl Theme {
    fn slot(&mut self, key: &str) -> Option<&mut Color> {
        Some(match key {
            "accent" => &mut self.accent,
            "border" => &mut self.border,
            "working" => &mut self.working,
            "blocked" => &mut self.blocked,
            "idle" => &mut self.idle,
            "dir" => &mut self.dir,
            "marker" => &mut self.marker,
            "danger" => &mut self.danger,
            "graph_lane" => &mut self.graph_lane,
            "graph_hash" => &mut self.graph_hash,
            "graph_ref" => &mut self.graph_ref,
            "button_new" => &mut self.button_new,
            "button_menu" => &mut self.button_menu,
            "button_fg" => &mut self.button_fg,
            "dim" => &mut self.dim,
            "line_bg" => &mut self.line_bg,
            _ => return None,
        })
    }
}

/// The named theme with the user's `[colors]` overrides on top. Unknown theme
/// names fall back to `default`; every problem becomes a warning.
pub fn resolve(name: &str, overrides: &HashMap<String, String>) -> (Theme, Vec<String>) {
    let mut warnings = Vec::new();
    let mut theme = builtin(name).unwrap_or_else(|| {
        warnings.push(format!("unknown theme '{name}' (known: {})", NAMES.join(", ")));
        builtin("default").expect("the default theme exists")
    });
    let mut keys: Vec<_> = overrides.keys().collect();
    keys.sort();
    for key in keys {
        let value = &overrides[key];
        match (theme.slot(key), value.parse::<Color>()) {
            (Some(slot), Ok(color)) => *slot = color,
            (None, _) => warnings.push(format!("unknown colour '{key}' in [colors]")),
            (_, Err(_)) => warnings.push(format!("colour '{key}': cannot parse '{value}'")),
        }
    }
    (theme, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key accepted in the `[colors]` table of the config file.
    pub const COLOR_KEYS: [&str; 16] = [
        "accent",
        "border",
        "working",
        "blocked",
        "idle",
        "dir",
        "marker",
        "danger",
        "graph_lane",
        "graph_hash",
        "graph_ref",
        "button_new",
        "button_menu",
        "button_fg",
        "dim",
        "line_bg",
    ];

    #[test]
    fn every_listed_name_and_key_exists() {
        for name in NAMES {
            let mut t = builtin(name).unwrap_or_else(|| panic!("missing theme {name}"));
            for key in COLOR_KEYS {
                assert!(t.slot(key).is_some(), "missing slot {key}");
            }
        }
        assert!(builtin("nope").is_none());
    }

    #[test]
    fn overrides_apply_and_bad_ones_warn() {
        let mut o = HashMap::new();
        o.insert("accent".to_string(), "#ff0000".to_string());
        o.insert("border".to_string(), "not-a-colour".to_string());
        o.insert("bogus".to_string(), "red".to_string());
        let (t, warnings) = resolve("nord", &o);
        assert_eq!(t.accent, Color::Rgb(255, 0, 0));
        assert_eq!(t.border, builtin("nord").unwrap().border);
        assert_eq!(warnings.len(), 2);
    }

    #[test]
    fn unknown_theme_falls_back() {
        let (t, warnings) = resolve("???", &HashMap::new());
        assert_eq!(t.accent, Color::Cyan);
        assert_eq!(warnings.len(), 1);
    }
}
