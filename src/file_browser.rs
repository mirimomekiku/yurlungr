use std::path::{Path, PathBuf};

use anyhow::Result;
use ignore::WalkBuilder;

pub struct Entry {
    pub name: String,
    pub is_dir: bool,
}

pub struct FileBrowser {
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
}

impl FileBrowser {
    pub fn new(dir: PathBuf) -> Result<Self> {
        let mut fb = Self { dir, entries: Vec::new() };
        fb.reload()?;
        Ok(fb)
    }

    pub fn reload(&mut self) -> Result<()> {
        let mut entries: Vec<Entry> = WalkBuilder::new(&self.dir)
            .max_depth(Some(1))
            .hidden(false)
            .filter_entry(|e| e.file_name() != ".git")
            .build()
            .flatten()
            .filter(|e| e.depth() == 1)
            .map(|e| Entry {
                name: e.file_name().to_string_lossy().into_owned(),
                is_dir: e.file_type().is_some_and(|t| t.is_dir()),
            })
            .collect();
        entries.sort_by(|a, b| {
            b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.entries = entries;
        Ok(())
    }

    /// Enter the directory at `idx`; returns false if it is not a directory.
    pub fn enter(&mut self, idx: usize) -> Result<bool> {
        match self.entries.get(idx) {
            Some(e) if e.is_dir => {
                self.dir = self.dir.join(&e.name);
                self.reload()?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub fn parent(&mut self) -> Result<bool> {
        match self.dir.parent().map(Path::to_path_buf) {
            Some(p) => {
                self.dir = p;
                self.reload()?;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}
