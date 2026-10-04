//! Reading a directory that is somebody else's — an imported bundle — so that nothing under it is reached through a link, whenever the link was put there.
//!
//! Asking whether a path is a link and then opening it is two looks at the filesystem, and whoever owns the directory can swap a directory for a link between them: `assets` checked as a directory, then read as a link to `~/.ssh`. So every directory is opened once, as a descriptor, refusing a link at that very open (`O_NOFOLLOW | O_DIRECTORY`), and everything under it is opened relative to that descriptor one name at a time, each refusing a link at its own open. A name is one component — never a path — so no lookup ever walks through a directory that was not opened this way, and a swap after a check changes nothing that is read.
//!
//! `openat2` with `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS` would say the same thing in one call for a whole path, and is not used: every name here is a single component already, which `openat` with `O_NOFOLLOW` resolves without walking anything, on every kernel.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::Read;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use rustix::fs::{AtFlags, FileType, Mode, OFlags};

/// A directory opened so that nothing under it is reached through a link.
#[derive(Debug)]
pub struct PlainDir {
    fd: OwnedFd,
    path: PathBuf,
}

/// What a name in a [`PlainDir`] is, looked at without following it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    Missing,
    Link,
    Directory,
    File {
        len: u64,
    },
    /// A FIFO, a socket or a device.
    Other,
}

impl PlainDir {
    /// Opens the directory at `path`, which whoever asks named: a link on the way to it is theirs to follow, so it is.
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let fd = rustix::fs::open(path, directory_flags() & !OFlags::NOFOLLOW, Mode::empty())
            .map_err(|why| explained(why.into()))?;
        Ok(Self {
            fd,
            path: path.to_path_buf(),
        })
    }

    /// Where this directory is, for a message to name.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Where `name` in this directory is, for a message to name.
    pub fn join(&self, name: impl AsRef<OsStr>) -> PathBuf {
        self.path.join(name.as_ref())
    }

    /// What `name` is, without following it.
    pub fn entry(&self, name: impl AsRef<OsStr>) -> std::io::Result<Entry> {
        let name = component(name.as_ref())?;
        let stat = match rustix::fs::statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(rustix::io::Errno::NOENT) => return Ok(Entry::Missing),
            Err(why) => return Err(why.into()),
        };
        Ok(match FileType::from_raw_mode(stat.st_mode) {
            FileType::Symlink => Entry::Link,
            FileType::Directory => Entry::Directory,
            FileType::RegularFile => Entry::File {
                len: u64::try_from(stat.st_size).unwrap_or(0),
            },
            _ => Entry::Other,
        })
    }

    /// The directory `name` in this one, refused at the open if it is a link or not a directory.
    pub fn dir(&self, name: impl AsRef<OsStr>) -> std::io::Result<PlainDir> {
        let name = component(name.as_ref())?;
        let fd = match rustix::fs::openat(&self.fd, name, directory_flags(), Mode::empty()) {
            Ok(fd) => fd,
            // `O_DIRECTORY | O_NOFOLLOW` on a link to a directory fails as "not a directory", which would hide what was refused.
            Err(rustix::io::Errno::NOTDIR) if self.entry(name)? == Entry::Link => {
                return Err(explained(rustix::io::Errno::LOOP.into()));
            }
            Err(why) => return Err(explained(why.into())),
        };
        Ok(Self {
            fd,
            path: self.path.join(name),
        })
    }

    /// The regular file `name` in this one, refused at the open if it is a link, and refused if it is not a regular file or holds more than `limit` bytes ([`read_open`]). Opened without blocking, so a FIFO is refused rather than waited on.
    pub fn read(&self, name: impl AsRef<OsStr>, limit: u64) -> std::io::Result<Vec<u8>> {
        let name = component(name.as_ref())?;
        let flags =
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::NOCTTY | OFlags::CLOEXEC;
        let fd = rustix::fs::openat(&self.fd, name, flags, Mode::empty())
            .map_err(|why| explained(why.into()))?;
        read_open(File::from(fd), limit)
    }

    /// The name of everything in this directory, but `.` and `..`, in no particular order; at most `limit` of them, or `None` where it holds more.
    pub fn names(&self, limit: usize) -> std::io::Result<Option<Vec<OsString>>> {
        let mut names = Vec::new();
        for entry in rustix::fs::Dir::read_from(&self.fd)? {
            let entry = entry?;
            let name = OsStr::from_bytes(entry.file_name().to_bytes());
            if name == "." || name == ".." {
                continue;
            }
            if names.len() == limit {
                return Ok(None);
            }
            names.push(name.to_os_string());
        }
        Ok(Some(names))
    }
}

fn directory_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

/// `name` where it is one component of a path — not empty, not `.` or `..`, no `/` — so that opening it relative to a directory walks nothing.
fn component(name: &OsStr) -> std::io::Result<&OsStr> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("`{}` is not one file name", name.to_string_lossy()),
        ));
    }
    Ok(name)
}

/// The error an open gave, said as what it means here: a link refused, or something else where a directory was asked for.
fn explained(why: std::io::Error) -> std::io::Error {
    match why.raw_os_error() {
        Some(libc::ELOOP) => std::io::Error::other("it is a symbolic link"),
        Some(libc::ENOTDIR) => std::io::Error::other("it is not a directory"),
        _ => why,
    }
}

/// Reads the whole of `file`, which must be a regular file of at most `limit` bytes: checked through the descriptor it was opened as, so a FIFO or a device swapped in after a listing is refused rather than read, and a file that grows past `limit` while it is read is refused rather than read to its end.
pub fn read_open(file: File, limit: u64) -> std::io::Result<Vec<u8>> {
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(std::io::Error::other("it is not a regular file"));
    }
    if meta.len() > limit {
        return Err(too_large(limit));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(too_large(limit));
    }
    Ok(bytes)
}

fn too_large(limit: u64) -> std::io::Error {
    std::io::Error::other(format!("it holds more than {limit} bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hogar-shell-beneath-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The race this module exists for: a directory checked and opened, then swapped for a link to somewhere else. What is read through the descriptor is the directory that was opened, never what the link points at, and a link met at an open is refused there.
    #[test]
    fn a_directory_swapped_for_a_link_after_it_was_opened_redirects_nothing() {
        let dir = scratch("swap");
        let secret = scratch("swap-secret");
        std::fs::write(secret.join("wall.png"), b"the user's key").unwrap();
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("assets/wall.png"), b"pixels").unwrap();

        let root = PlainDir::open(&dir).unwrap();
        let assets = root.dir("assets").unwrap();
        std::fs::rename(dir.join("assets"), dir.join("moved")).unwrap();
        std::os::unix::fs::symlink(&secret, dir.join("assets")).unwrap();

        assert_eq!(assets.read("wall.png", 64).unwrap(), b"pixels");
        assert_eq!(root.entry("assets").unwrap(), Entry::Link);
        let refused = root.dir("assets").unwrap_err();
        assert!(refused.to_string().contains("symbolic link"), "{refused}");

        std::os::unix::fs::symlink(secret.join("wall.png"), dir.join("moved/link.png")).unwrap();
        let moved = root.dir("moved").unwrap();
        let link = moved.read("link.png", 64).unwrap_err();
        assert!(link.to_string().contains("symbolic link"), "{link}");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&secret).ok();
    }

    /// A name is one component, so nothing is ever looked up through a directory this module did not open itself.
    #[test]
    fn a_path_is_not_a_name() {
        let dir = scratch("names");
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::write(dir.join("a/b"), b"x").unwrap();
        let root = PlainDir::open(&dir).unwrap();
        for name in ["a/b", "..", ".", ""] {
            assert_eq!(
                root.read(name, 8).unwrap_err().kind(),
                std::io::ErrorKind::InvalidInput,
                "`{name}`"
            );
        }
        assert_eq!(root.entry("a").unwrap(), Entry::Directory);
        assert_eq!(
            root.dir("a").unwrap().entry("b").unwrap(),
            Entry::File { len: 1 }
        );
        assert_eq!(root.entry("none").unwrap(), Entry::Missing);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A FIFO is refused rather than waited on, a file over the limit rather than read whole, and a listing longer than its limit says so.
    #[test]
    fn a_fifo_an_oversized_file_and_a_long_listing_are_refused() {
        let dir = scratch("fifo");
        let fifo = std::ffi::CString::new(dir.join("pipe").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        std::fs::write(dir.join("big"), vec![b'#'; 65]).unwrap();
        let root = PlainDir::open(&dir).unwrap();

        assert_eq!(root.entry("pipe").unwrap(), Entry::Other);
        assert!(
            root.read("pipe", 64)
                .unwrap_err()
                .to_string()
                .contains("not a regular file")
        );
        assert!(
            root.read("big", 64)
                .unwrap_err()
                .to_string()
                .contains("more than 64")
        );
        assert_eq!(root.names(1).unwrap(), None);
        let mut names = root.names(2).unwrap().unwrap();
        names.sort();
        assert_eq!(names, ["big", "pipe"]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
