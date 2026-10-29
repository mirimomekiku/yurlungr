use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use ignore::WalkBuilder;

pub struct Entry {
    pub name: String,
    pub is_dir: bool,
}

pub struct FileBrowser {
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    pub show_hidden: bool,
}

impl FileBrowser {
    pub fn new(dir: PathBuf) -> Result<Self> {
        let mut fb = Self { dir, entries: Vec::new(), show_hidden: true };
        fb.reload()?;
        Ok(fb)
    }

    pub fn reload(&mut self) -> Result<()> {
        let show_hidden = self.show_hidden;
        let mut entries: Vec<Entry> = WalkBuilder::new(&self.dir)
            .max_depth(Some(1))
            .standard_filters(false)
            .filter_entry(move |e| {
                let name = e.file_name();
                name != ".git"
                    && (show_hidden || e.depth() == 0 || !name.to_string_lossy().starts_with('.'))
            })
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

    pub fn path_at(&self, idx: usize) -> Option<PathBuf> {
        self.entries.get(idx).map(|e| self.dir.join(&e.name))
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.name == name)
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

// ---- file operations ---------------------------------------------------------
//
// None of these ever overwrite: a destination that already exists is an error,
// except `copy_into`, which picks a free name.

/// Replace a file's contents through a temp file in the same folder, so a crash
/// never leaves it half written. A symlink is followed (the link stays a link)
/// and the existing permissions are kept. Hard-linked files, and files in a
/// folder we cannot create files in, are written in place instead.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut tmp = target.as_os_str().to_owned();
    tmp.push(".yurlungr.tmp");
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        let mut f = match fs::File::create(&tmp) {
            Ok(f) => f,
            // A writable file in a folder we cannot add files to: write in place.
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied && target.is_file() => {
                return fs::write(&target, bytes);
            }
            Err(e) => return Err(e),
        };
        if let Ok(meta) = fs::metadata(&target) {
            f.set_permissions(meta.permissions())?;
        }
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, &target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// A single path component: no separators, not empty, not `.` or `..`.
    // A hard-linked file would be detached from its other names by a rename.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if fs::metadata(&target).is_ok_and(|m| m.nlink() > 1) {
            return fs::write(&target, bytes);
        }
    }
pub fn valid_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." {
        bail!("invalid name");
    }
    if name.contains(['/', '\\', '\0']) {
        bail!("name cannot contain path separators");
    }
    Ok(())
}

pub fn create_file(dir: &Path, name: &str) -> Result<PathBuf> {
    valid_name(name)?;
    let path = dir.join(name);
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| exists_hint(e, name))?;
    Ok(path)
}

pub fn create_dir(dir: &Path, name: &str) -> Result<PathBuf> {
    valid_name(name)?;
    let path = dir.join(name);
    fs::create_dir(&path).map_err(|e| exists_hint(e, name))?;
    Ok(path)
}

pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf> {
    valid_name(new_name)?;
    let dst = path.with_file_name(new_name);
    if dst == path {
        return Ok(dst);
    }
    if dst.symlink_metadata().is_ok() {
        bail!("'{new_name}' already exists");
    }
    fs::rename(path, &dst)?;
    Ok(dst)
}

/// Move `src` to `dest`, which is relative to `cwd` unless absolute or `~/...`.
/// A destination that is an existing directory receives `src` under its own name.
pub fn move_to(src: &Path, dest: &str, cwd: &Path) -> Result<PathBuf> {
    let dest = dest.trim();
    if dest.is_empty() {
        bail!("no destination given");
    }
    let home_var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let mut dst = match dest.strip_prefix("~/").or_else(|| dest.strip_prefix("~\\")) {
        Some(rest) => PathBuf::from(
            std::env::var_os(home_var).with_context(|| format!("{home_var} is not set"))?,
        )
        .join(rest),
        None => PathBuf::from(dest),
    };
    if dst.is_relative() {
        dst = cwd.join(dst);
    }
    if dst.is_dir() {
        dst = dst.join(src.file_name().context("nothing to move")?);
    }
    if dst.symlink_metadata().is_ok() {
        bail!("'{}' already exists", dst.display());
    }
    if src.is_dir() && dst.starts_with(src) {
        bail!("cannot move a folder into itself");
    }
    if !dst.parent().is_some_and(Path::is_dir) {
        bail!("destination folder does not exist");
    }
    match fs::rename(src, &dst) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            copy_recursive(src, &dst)?;
            delete(src)?;
        }
        Err(e) => return Err(e.into()),
    }
    Ok(dst)
}

/// Copy `src` into `dir`, appending " copy" / " copy N" if the name is taken.
pub fn copy_into(src: &Path, dir: &Path) -> Result<PathBuf> {
    let name = src.file_name().context("nothing to copy")?.to_string_lossy().into_owned();
    // Compare real paths too: `dir` may reach `src` through a symlink.
    let inside = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a.starts_with(b),
        _ => false,
    };
    if src.is_dir() && (dir.starts_with(src) || inside(dir, src)) {
        bail!("cannot copy a folder into itself");
    }
    let dst = unique_dest(dir, &name);
    copy_recursive(src, &dst)?;
    Ok(dst)
}

/// Delete a file, a symlink (never what it points to), or a folder and its contents.
pub fn delete(path: &Path) -> Result<()> {
    let meta = path.symlink_metadata()?;
    if meta.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn exists_hint(e: io::Error, name: &str) -> anyhow::Error {
    if e.kind() == io::ErrorKind::AlreadyExists {
        anyhow::anyhow!("'{name}' already exists")
    } else {
        e.into()
    }
}

fn unique_dest(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if first.symlink_metadata().is_err() {
        return first;
    }
    let (stem, ext) = match name.rfind('.').filter(|&i| i > 0) {
        Some(i) => (&name[..i], &name[i..]),
        None => (name, ""),
    };
    (1..)
        .map(|n| {
            let suffix = if n == 1 { " copy".to_string() } else { format!(" copy {n}") };
            dir.join(format!("{stem}{suffix}{ext}"))
        })
        .find(|p| p.symlink_metadata().is_err())
        .expect("an unbounded range always yields a free name")
}

fn copy_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    let meta = src.symlink_metadata()?;
    if meta.is_dir() {
        fs::create_dir(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dst.join(entry.file_name()))?;
        }
    } else if meta.file_type().is_symlink() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(fs::read_link(src)?, dst)?;
        #[cfg(not(unix))]
        return Err(io::Error::other("copying symlinks is not supported on this platform"));
    } else {
        fs::copy(src, dst)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn names_are_validated() {
        for bad in ["", ".", "..", "a/b", "a\\b"] {
            assert!(valid_name(bad).is_err(), "{bad:?}");
        }
        assert!(valid_name("ok.txt").is_ok());
    }

    #[test]
    fn create_refuses_to_overwrite() {
        let d = tmp();
        create_file(d.path(), "a.txt").unwrap();
        assert!(create_file(d.path(), "a.txt").is_err());
        create_dir(d.path(), "sub").unwrap();
        assert!(create_dir(d.path(), "sub").is_err());
    }

    #[test]
    fn rename_keeps_directory_and_refuses_clobber() {
        let d = tmp();
        let a = create_file(d.path(), "a.txt").unwrap();
        create_file(d.path(), "b.txt").unwrap();
        assert!(rename(&a, "b.txt").is_err());
        let c = rename(&a, "c.txt").unwrap();
        assert_eq!(c, d.path().join("c.txt"));
        assert!(c.exists() && !a.exists());
    }

    #[test]
    fn move_into_dir_relative_and_guards() {
        let d = tmp();
        let a = create_file(d.path(), "a.txt").unwrap();
        let sub = create_dir(d.path(), "sub").unwrap();
        let moved = move_to(&a, "sub", d.path()).unwrap();
        assert_eq!(moved, sub.join("a.txt"));
        assert!(move_to(&sub, "sub/inner", d.path()).is_err(), "folder into itself");
        let b = create_file(d.path(), "b.txt").unwrap();
        fs::write(sub.join("b.txt"), "x").unwrap();
        assert!(move_to(&b, "sub", d.path()).is_err(), "existing destination");
        assert!(move_to(&b, "missing/x", d.path()).is_err(), "missing parent");
    }

    #[test]
    fn copy_picks_free_names_and_recurses() {
        let d = tmp();
        let sub = create_dir(d.path(), "dir").unwrap();
        fs::write(sub.join("f.rs"), "hi").unwrap();
        let c1 = copy_into(&sub, d.path()).unwrap();
        let c2 = copy_into(&sub, d.path()).unwrap();
        assert_eq!(c1.file_name().unwrap(), "dir copy");
        assert_eq!(c2.file_name().unwrap(), "dir copy 2");
        assert_eq!(fs::read_to_string(c2.join("f.rs")).unwrap(), "hi");
        assert!(copy_into(&sub, &sub).is_err());

        let f = d.path().join("notes.md");
        fs::write(&f, "x").unwrap();
        assert_eq!(copy_into(&f, d.path()).unwrap().file_name().unwrap(), "notes copy.md");
    }

    #[test]
    fn delete_removes_trees_and_not_symlink_targets() {
        let d = tmp();
        let sub = create_dir(d.path(), "dir").unwrap();
        fs::write(sub.join("f"), "x").unwrap();
        delete(&sub).unwrap();
        assert!(!sub.exists());

        #[cfg(unix)]
        {
            let target = create_dir(d.path(), "keep").unwrap();
            fs::write(target.join("f"), "x").unwrap();
            let link = d.path().join("link");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            delete(&link).unwrap();
            assert!(!link.exists() && target.join("f").exists());
        }
    }

    #[test]
    fn write_atomic_replaces_contents_and_leaves_no_temp() {
        let d = tmp();
        let f = d.path().join("c.json");
        fs::write(&f, "old").unwrap();
        write_atomic(&f, b"new").unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "new");
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_keeps_mode_and_symlink() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp();
        let real = d.path().join("real");
        fs::write(&real, "old").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        let link = d.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, b"new").unwrap();
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(&real).unwrap(), "new");
        assert_eq!(fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn copy_through_a_symlink_into_itself_is_refused() {
        let d = tmp();
        let sub = create_dir(d.path(), "proj").unwrap();
        let link = d.path().join("link");
        std::os::unix::fs::symlink(&sub, &link).unwrap();
        assert!(copy_into(&sub, &link).is_err());
    }

    #[test]
    fn browser_lists_dirs_first_and_toggles_hidden() {
        let d = tmp();
        create_file(d.path(), "b.txt").unwrap();
        create_file(d.path(), ".hidden").unwrap();
        create_dir(d.path(), "zdir").unwrap();
        fs::create_dir(d.path().join(".git")).unwrap();
        let mut fb = FileBrowser::new(d.path().to_path_buf()).unwrap();
        let names: Vec<_> = fb.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["zdir", ".hidden", "b.txt"]);
    #[cfg(unix)]
    #[test]
    fn write_atomic_writes_in_place_for_hard_links() {
        let d = tmp();
        let a = d.path().join("a");
        let b = d.path().join("b");
        fs::write(&a, "old").unwrap();
        fs::hard_link(&a, &b).unwrap();
        write_atomic(&a, b"new").unwrap();
        assert_eq!(fs::read_to_string(&b).unwrap(), "new");
    }

        fb.show_hidden = false;
        fb.reload().unwrap();
        assert_eq!(fb.entries.len(), 2);
    }
}
