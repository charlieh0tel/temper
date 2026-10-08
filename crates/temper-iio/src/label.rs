//! The label: the daemon's name under `/run/tempered`, owned through a
//! lock file, and the link to its IIO device.

use std::fmt;
use std::fs;
use std::fs::File;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;

use rustix::fs::FlockOperation;
use rustix::fs::Mode;
use rustix::io::Errno;

/// Longest label; it also becomes the uhid device name.
const MAX_LEN: usize = 64;

const LOCK_SUFFIX: &str = ".lock";

/// Lock files are readable by their owner only: `flock` works on any
/// file a process can open, whatever the open mode, so a lock others
/// could open could be taken by them, stalling the daemon.
const LOCK_MODE: u32 = 0o600;
const TEMPORARY_SUFFIX: &str = ".tmp";

/// A validated label: `[a-z0-9][a-z0-9_-]{0,63}`, safe as a file name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Label(String);

/// A label that fails validation; holds the rejected text.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "label {0:?} must be 1 to {MAX_LEN} of a-z, 0-9, '_' and '-', starting with a letter or digit"
)]
pub(crate) struct LabelError(String);

impl FromStr for Label {
    type Err = LabelError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let first_ok = text
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        let rest_ok = text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
        if first_ok && rest_ok && text.len() <= MAX_LEN {
            Ok(Self(text.to_owned()))
        } else {
            Err(LabelError(text.to_owned()))
        }
    }
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Label {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Ownership of a label in a directory, held until dropped or the
/// process exits (a crash releases it too).
#[derive(Debug)]
pub(crate) struct LabelLock {
    /// Holds the `flock`.
    _lock: File,
    link: PathBuf,
    temporary: PathBuf,
}

/// Removes `path`, which may not exist.
fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

impl LabelLock {
    /// Takes `<dir>/<label>.lock` without waiting; `None` if another
    /// process holds it.  A link left behind by a dead owner is removed.
    pub(crate) fn try_acquire(dir: &Path, label: &Label) -> io::Result<Option<Self>> {
        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(LOCK_MODE)
            .open(dir.join(format!("{label}{LOCK_SUFFIX}")))?;
        // A lock file left by an older version may be readable by others.
        rustix::fs::fchmod(&lock, Mode::from_raw_mode(LOCK_MODE))?;
        match rustix::fs::flock(&lock, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {}
            Err(Errno::WOULDBLOCK) => return Ok(None),
            Err(errno) => return Err(errno.into()),
        }
        let this = Self {
            _lock: lock,
            link: dir.join(label.as_str()),
            temporary: dir.join(format!(".{label}{TEMPORARY_SUFFIX}")),
        };
        this.unlink()?;
        Ok(Some(this))
    }

    /// Points the link at `target`, atomically replacing any old link.
    pub(crate) fn link(&self, target: &Path) -> io::Result<()> {
        remove_if_present(&self.temporary)?;
        symlink(target, &self.temporary)?;
        fs::rename(&self.temporary, &self.link)
    }

    /// Removes the link, if present.
    pub(crate) fn unlink(&self) -> io::Result<()> {
        remove_if_present(&self.link)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn label(text: &str) -> Label {
        text.parse().unwrap()
    }

    #[test]
    fn valid_labels() {
        for text in ["temperature", "a", "0", "room-2_b", &"x".repeat(MAX_LEN)] {
            assert_eq!(text.parse::<Label>().unwrap().as_str(), text);
        }
    }

    #[test]
    fn invalid_labels() {
        for text in [
            "",
            "-a",
            "_a",
            ".a",
            "A",
            "a/b",
            "a.lock",
            "..",
            "a b",
            &"x".repeat(MAX_LEN + 1),
        ] {
            assert!(text.parse::<Label>().is_err(), "{text:?}");
        }
    }

    #[test]
    fn second_lock_is_refused_until_released() {
        let dir = tempfile::tempdir().unwrap();
        let first = LabelLock::try_acquire(dir.path(), &label("t")).unwrap();
        assert!(first.is_some());
        assert!(
            LabelLock::try_acquire(dir.path(), &label("t"))
                .unwrap()
                .is_none()
        );
        assert!(
            LabelLock::try_acquire(dir.path(), &label("u"))
                .unwrap()
                .is_some()
        );
        drop(first);
        assert!(
            LabelLock::try_acquire(dir.path(), &label("t"))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn lock_file_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.lock");
        fs::write(&path, "").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let _lock = LabelLock::try_acquire(dir.path(), &label("t"))
            .unwrap()
            .unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, LOCK_MODE);
    }

    #[test]
    fn link_replaces_and_unlink_removes() {
        let dir = tempfile::tempdir().unwrap();
        let lock = LabelLock::try_acquire(dir.path(), &label("t"))
            .unwrap()
            .unwrap();
        let link = dir.path().join("t");
        lock.link(Path::new("/sys/a")).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), Path::new("/sys/a"));
        lock.link(Path::new("/sys/b")).unwrap();
        assert_eq!(fs::read_link(&link).unwrap(), Path::new("/sys/b"));
        lock.unlink().unwrap();
        assert!(fs::symlink_metadata(&link).is_err());
        lock.unlink().unwrap();
    }

    #[test]
    fn acquiring_removes_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        symlink("/sys/old", dir.path().join("t")).unwrap();
        symlink("/sys/old", dir.path().join(".t.tmp")).unwrap();
        let lock = LabelLock::try_acquire(dir.path(), &label("t"))
            .unwrap()
            .unwrap();
        assert!(fs::symlink_metadata(dir.path().join("t")).is_err());
        lock.link(Path::new("/sys/new")).unwrap();
        assert_eq!(
            fs::read_link(dir.path().join("t")).unwrap(),
            Path::new("/sys/new")
        );
    }
}
