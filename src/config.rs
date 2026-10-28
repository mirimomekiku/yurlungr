//! The user config file: theme, colour overrides and key bindings.
//!
//! ```toml
//! theme = "nord"
//!
//! [colors]            # optional overrides on top of the theme
//! accent = "#ff8800"
//!
//! [keys]              # only what differs from the defaults
//! prefix = "ctrl+a"
//! close_pane = ["prefix x", "alt+w"]
//! split_left = []     # unbound
//! ```
//!
//! A broken file never stops yurlungr: bad entries are skipped and reported in
//! the settings dialog.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, Result};

use crate::keymap::Keymap;

#[derive(Clone)]
pub struct Config {
    pub theme: String,
    /// Raw `[colors]` overrides (validated when the theme is resolved).
    pub colors: HashMap<String, String>,
    pub keymap: Keymap,
    pub warnings: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "default".to_string(),
            colors: HashMap::new(),
            keymap: Keymap::default(),
            warnings: Vec::new(),
        }
    }
}

/// `$XDG_CONFIG_HOME/yurlungr/config.toml`, `~/.config/yurlungr/config.toml`, or
/// `%APPDATA%\yurlungr\config.toml`.
pub fn path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    Some(base?.join("yurlungr").join("config.toml"))
}

pub fn modified() -> Option<SystemTime> {
    fs::metadata(path()?).and_then(|m| m.modified()).ok()
}

impl Config {
    pub fn load() -> Self {
        let Some(path) = path() else { return Self::default() };
        match fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                let mut c = Self::default();
                c.warnings.push(format!("cannot read {}: {e}", path.display()));
                c
            }
        }
    }

    pub fn parse(text: &str) -> Self {
        let mut config = Self::default();
        let table: toml::Table = match toml::from_str(text) {
            Ok(t) => t,
            Err(e) => {
                config.warnings.push(format!("config.toml is not valid TOML, using defaults: {e}"));
                return config;
            }
        };
        match table.get("theme") {
            Some(toml::Value::String(name)) => config.theme = name.clone(),
            Some(_) => config.warnings.push("theme must be a string".into()),
            None => {}
        }
        if let Some(toml::Value::Table(colors)) = table.get("colors") {
            for (key, value) in colors {
                match value.as_str() {
                    Some(s) => {
                        config.colors.insert(key.clone(), s.to_string());
                    }
                    None => config.warnings.push(format!("colors.{key} must be a string")),
                }
            }
        }
        if let Some(toml::Value::Table(keys)) = table.get("keys") {
            let warnings = config.keymap.apply(keys);
            config.warnings.extend(warnings);
        }
        config
    }

    /// Write theme and key overrides back, keeping `[colors]` and any other
    /// content already in the file (comments are not preserved).
    pub fn save(&self) -> Result<()> {
        self.save_to(&path().context("cannot locate a config directory")?)
    }

    fn save_to(&self, path: &Path) -> Result<()> {
        let mut table: toml::Table =
            fs::read_to_string(path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default();

        if self.theme == "default" {
            table.remove("theme");
        } else {
            table.insert("theme".into(), toml::Value::String(self.theme.clone()));
        }
        let overrides = self.keymap.overrides();
        if overrides.is_empty() {
            table.remove("keys");
        } else {
            table.insert("keys".into(), toml::Value::Table(overrides));
        }

        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        fs::write(&tmp, toml::to_string_pretty(&table)?)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::{Action, Chord};

    #[test]
    fn empty_and_broken_files_fall_back_to_defaults() {
        let c = Config::parse("");
        assert_eq!(c.theme, "default");
        assert!(c.warnings.is_empty());

        let c = Config::parse("theme = [");
        assert_eq!(c.theme, "default");
        assert_eq!(c.warnings.len(), 1);
    }

    #[test]
    fn reads_theme_colors_and_keys() {
        let c = Config::parse(
            "theme = \"nord\"\n[colors]\naccent = \"#ff0000\"\n[keys]\nclose_pane = [\"alt+w\"]\n",
        );
        assert_eq!(c.theme, "nord");
        assert_eq!(c.colors["accent"], "#ff0000");
        assert_eq!(c.keymap.chords(Action::ClosePane), [Chord::parse("alt+w").unwrap()]);
        assert!(c.warnings.is_empty());
    }

    #[test]
    fn save_round_trips_and_keeps_unrelated_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("yurlungr").join("config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "[colors]\naccent = \"red\"\n\n[extra]\nkeep = 1\n").unwrap();

        let mut c = Config::parse(&fs::read_to_string(&path).unwrap());
        c.theme = "dracula".into();
        c.keymap.set(Action::ClosePane, vec![Chord::parse("alt+w").unwrap()]);
        c.keymap.set(Action::SplitLeft, vec![]);
        c.save_to(&path).unwrap();

        let back = Config::parse(&fs::read_to_string(&path).unwrap());
        assert_eq!(back.theme, "dracula");
        assert_eq!(back.colors["accent"], "red");
        assert!(back.keymap == c.keymap);
        assert!(back.warnings.is_empty());
        assert!(fs::read_to_string(&path).unwrap().contains("keep = 1"));

        // Back to defaults removes the sections again.
        let mut reset = back;
        reset.theme = "default".into();
        reset.keymap = Keymap::default();
        reset.save_to(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("theme") && !text.contains("[keys]"), "{text}");
    }
}
