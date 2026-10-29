//! Files pasted into a document (Markdown paste): written or copied into a
//! target directory under a name that never replaces an existing file.

use crate::ops;
use ide_core::error::ErrorKind;
use ide_core::{IdeError, IdeResult};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Attempts at a free name (`name-1.ext`, `name-2.ext`, and so on) before giving up.
const MAX_ATTEMPTS: u32 = 10_000;

/// The last component of a file name that came from outside (clipboard, other
/// OS): path separators never escape the target directory.
fn clean_name(name: &str) -> IdeResult<String> {
    let last = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    if last.is_empty() || last == "." || last == ".." {
        return Err(IdeError::invalid(format!("invalid file name: {name:?}")));
    }
    Ok(last.to_string())
}

/// `dir/name`, or `dir/stem-N.ext` for the first N that is free.
fn candidate(dir: &Path, name: &str, attempt: u32) -> PathBuf {
    if attempt == 0 {
        return dir.join(name);
    }
    let p = Path::new(name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    match p.extension().and_then(|e| e.to_str()) {
        Some(ext) => dir.join(format!("{stem}-{attempt}.{ext}")),
        None => dir.join(format!("{stem}-{attempt}")),
    }
}

/// Writes `bytes` to a new file `name` in `dir` (created when missing);
/// returns the path used.
pub fn save_unique(dir: &Path, name: &str, bytes: &[u8]) -> IdeResult<PathBuf> {
    let name = clean_name(name)?;
    fs::create_dir_all(dir)?;
    for attempt in 0..MAX_ATTEMPTS {
        let path = candidate(dir, &name, attempt);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                if let Err(e) = f.write_all(bytes) {
                    drop(f);
                    let _ = fs::remove_file(&path);
                    return Err(e.into());
                }
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(IdeError::new(ErrorKind::AlreadyExists, format!("no free name for {name}")))
}

/// Copies the file or folder `from` into `dir` (created when missing) under
/// its own name, or a numbered one when taken; returns the path used.
pub fn copy_unique(from: &Path, dir: &Path) -> IdeResult<PathBuf> {
    let name = from
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| IdeError::invalid(format!("not a file path: {}", from.display())))?
        .to_string();
    if from.is_dir() && dir.starts_with(from) {
        return Err(IdeError::invalid("cannot copy a folder into itself"));
    }
    fs::create_dir_all(dir)?;
    for attempt in 0..MAX_ATTEMPTS {
        let path = candidate(dir, &name, attempt);
        if path.symlink_metadata().is_ok() {
            continue;
        }
        return ops::copy(from, &path).map(|()| path);
    }
    Err(IdeError::new(ErrorKind::AlreadyExists, format!("no free name for {name}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_under_a_free_name() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("assets");
        let a = save_unique(&dir, "image.png", b"1").unwrap();
        let b = save_unique(&dir, "image.png", b"2").unwrap();
        let c = save_unique(&dir, "notes", b"3").unwrap();
        let d = save_unique(&dir, "notes", b"4").unwrap();
        assert_eq!(a, dir.join("image.png"));
        assert_eq!(b, dir.join("image-1.png"));
        assert_eq!(c, dir.join("notes"));
        assert_eq!(d, dir.join("notes-1"));
        assert_eq!(fs::read(&a).unwrap(), b"1");
        assert_eq!(fs::read(&b).unwrap(), b"2");
    }

    #[test]
    fn names_cannot_leave_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let p = save_unique(tmp.path(), "../../evil.txt", b"x").unwrap();
        assert_eq!(p, tmp.path().join("evil.txt"));
        assert!(save_unique(tmp.path(), "..", b"x").is_err());
        assert!(save_unique(tmp.path(), "", b"x").is_err());
    }

    #[test]
    fn copies_files_and_folders_without_overwriting() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("a.txt");
        fs::write(&src, "hi").unwrap();
        let folder = tmp.path().join("pics");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join("x.png"), "px").unwrap();
        let dir = tmp.path().join("out").join("assets");
        assert_eq!(copy_unique(&src, &dir).unwrap(), dir.join("a.txt"));
        assert_eq!(copy_unique(&src, &dir).unwrap(), dir.join("a-1.txt"));
        assert_eq!(copy_unique(&folder, &dir).unwrap(), dir.join("pics"));
        assert_eq!(fs::read(dir.join("pics").join("x.png")).unwrap(), b"px");
        assert!(copy_unique(&folder, &folder.join("inner")).is_err());
    }
}
