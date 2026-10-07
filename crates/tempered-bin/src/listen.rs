//! Getting `/dev/uhid`: adopted from systemd's `OpenFile=` (passed as in
//! `sd_listen_fds(3)`), or opened directly when not run by systemd.

use std::fs::File;
use std::io;
use std::os::fd::FromRawFd;
use std::os::fd::OwnedFd;
use std::os::fd::RawFd;
use std::path::Path;
use std::process;

use rustix::fs::FileType;
use rustix::io::FdFlags;

const LISTEN_PID: &str = "LISTEN_PID";
const LISTEN_FDS: &str = "LISTEN_FDS";
const LISTEN_FDNAMES: &str = "LISTEN_FDNAMES";

/// `SD_LISTEN_FDS_START`: the first passed fd.
const LISTEN_FDS_START: RawFd = 3;

/// The name the unit gives the fd: `OpenFile=/dev/uhid:uhid`.
const UHID_FD_NAME: &str = "uhid";

const UHID_PATH: &str = "/dev/uhid";

/// `MISC_MAJOR` (`include/uapi/linux/major.h`) and `UHID_MINOR`
/// (`include/linux/miscdevice.h`).
const UHID_MAJOR: u32 = 10;
const UHID_MINOR: u32 = 239;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ListenError {
    #[error("{LISTEN_FDS} is not a number: {0:?}")]
    BadCount(String),
    #[error("{LISTEN_FDNAMES} names {names} fds but {LISTEN_FDS} is {count}")]
    CountMismatch { names: usize, count: usize },
    #[error(
        "systemd passed no fd named {UHID_FD_NAME:?}; the unit needs OpenFile={UHID_PATH}:{UHID_FD_NAME}"
    )]
    NoUhid,
    #[error("the fd named {UHID_FD_NAME:?} is not {UHID_PATH}")]
    NotUhid,
    #[error("{UHID_PATH}")]
    Io(#[from] io::Error),
}

/// The fd number of the passed `uhid` fd, if the fds are for this
/// process.  `LISTEN_PID` naming another process means none were
/// passed to this one.
fn passed_uhid_fd(
    var: impl Fn(&str) -> Option<String>,
    pid: u32,
) -> Result<Option<RawFd>, ListenError> {
    if var(LISTEN_PID).and_then(|p| p.parse::<u32>().ok()) != Some(pid) {
        return Ok(None);
    }
    let count_text = var(LISTEN_FDS).unwrap_or_default();
    let count: usize = count_text
        .parse()
        .map_err(|_| ListenError::BadCount(count_text.clone()))?;
    let names_text = var(LISTEN_FDNAMES).unwrap_or_default();
    let names: Vec<&str> = names_text.split(':').collect();
    if names.len() != count {
        return Err(ListenError::CountMismatch {
            names: names.len(),
            count,
        });
    }
    let index = names
        .iter()
        .position(|name| *name == UHID_FD_NAME)
        .ok_or(ListenError::NoUhid)?;
    let index = RawFd::try_from(index).map_err(|_| ListenError::NoUhid)?;
    Ok(Some(LISTEN_FDS_START + index))
}

/// Whether `fd` is `/dev/uhid`: character device 10:239.
fn is_uhid(fd: &OwnedFd) -> io::Result<bool> {
    let stat = rustix::fs::fstat(fd)?;
    Ok(
        FileType::from_raw_mode(stat.st_mode) == FileType::CharacterDevice
            && rustix::fs::major(stat.st_rdev) == UHID_MAJOR
            && rustix::fs::minor(stat.st_rdev) == UHID_MINOR,
    )
}

/// `/dev/uhid`, from systemd if it passed it, else opened (needs root).
pub(crate) fn uhid(var: impl Fn(&str) -> Option<String>) -> Result<File, ListenError> {
    let Some(raw) = passed_uhid_fd(var, process::id())? else {
        return Ok(File::options()
            .read(true)
            .write(true)
            .open(Path::new(UHID_PATH))?);
    };
    // SAFETY: systemd passed this fd to this process (LISTEN_PID
    // matched) and nothing else in the process owns it; it is adopted
    // exactly once, here.
    #[expect(unsafe_code, reason = "adopting the fd systemd's OpenFile= passed")]
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    // systemd passes it without FD_CLOEXEC.
    rustix::io::fcntl_setfd(&fd, FdFlags::CLOEXEC).map_err(io::Error::from)?;
    if !is_uhid(&fd)? {
        return Err(ListenError::NotUhid);
    }
    Ok(File::from(fd))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    const PID: u32 = 4242;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn finds_named_fd() {
        let vars = env(&[
            (LISTEN_PID, "4242"),
            (LISTEN_FDS, "2"),
            (LISTEN_FDNAMES, "other:uhid"),
        ]);
        assert_eq!(passed_uhid_fd(vars, PID).unwrap(), Some(4));
    }

    #[test]
    fn other_or_no_pid_means_not_passed() {
        assert_eq!(passed_uhid_fd(env(&[]), PID).unwrap(), None);
        let vars = env(&[
            (LISTEN_PID, "1"),
            (LISTEN_FDS, "1"),
            (LISTEN_FDNAMES, "uhid"),
        ]);
        assert_eq!(passed_uhid_fd(vars, PID).unwrap(), None);
    }

    #[test]
    fn missing_uhid_is_an_error() {
        let vars = env(&[
            (LISTEN_PID, "4242"),
            (LISTEN_FDS, "1"),
            (LISTEN_FDNAMES, "null"),
        ]);
        assert!(matches!(
            passed_uhid_fd(vars, PID),
            Err(ListenError::NoUhid)
        ));
    }

    #[test]
    fn count_mismatch_is_an_error() {
        let vars = env(&[
            (LISTEN_PID, "4242"),
            (LISTEN_FDS, "2"),
            (LISTEN_FDNAMES, "uhid"),
        ]);
        assert!(matches!(
            passed_uhid_fd(vars, PID),
            Err(ListenError::CountMismatch { names: 1, count: 2 })
        ));
    }

    #[test]
    fn regular_file_is_not_uhid() {
        let file = tempfile::tempfile().unwrap();
        assert!(!is_uhid(&OwnedFd::from(file)).unwrap());
    }
}
