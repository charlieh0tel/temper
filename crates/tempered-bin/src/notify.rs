//! systemd service notifications (`sd_notify(3)`) and watchdog
//! settings (`sd_watchdog_enabled(3)`), without libsystemd.

use std::io;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::SocketAddr;
use std::os::unix::net::UnixDatagram;
use std::process;
use std::time::Duration;

const NOTIFY_SOCKET: &str = "NOTIFY_SOCKET";
const WATCHDOG_USEC: &str = "WATCHDOG_USEC";
const WATCHDOG_PID: &str = "WATCHDOG_PID";

/// Leading character of an abstract socket name in `NOTIFY_SOCKET`.
const ABSTRACT_PREFIX: char = '@';

/// Pings per watchdog period.
const PINGS_PER_PERIOD: u32 = 4;

/// The service manager's notification socket, if there is one.
#[derive(Debug)]
pub(crate) struct Notifier {
    target: Option<(UnixDatagram, SocketAddr)>,
}

impl Notifier {
    /// From `NOTIFY_SOCKET`: a path, or an abstract name after `@`.
    /// Other forms, such as `vsock:`, are ignored.
    pub(crate) fn from_env(var: impl Fn(&str) -> Option<String>) -> io::Result<Self> {
        let Some(socket) = var(NOTIFY_SOCKET) else {
            return Ok(Self { target: None });
        };
        let address = if let Some(name) = socket.strip_prefix(ABSTRACT_PREFIX) {
            SocketAddr::from_abstract_name(name)?
        } else if socket.starts_with('/') {
            SocketAddr::from_pathname(&socket)?
        } else {
            return Ok(Self { target: None });
        };
        Ok(Self {
            target: Some((UnixDatagram::unbound()?, address)),
        })
    }

    /// Sends newline-separated `KEY=value` assignments.
    fn send(&self, state: &str) -> io::Result<()> {
        if let Some((socket, address)) = &self.target {
            socket.send_to_addr(state.as_bytes(), address)?;
        }
        Ok(())
    }

    pub(crate) fn ready(&self) -> io::Result<()> {
        self.send("READY=1")
    }

    pub(crate) fn watchdog(&self) -> io::Result<()> {
        self.send("WATCHDOG=1")
    }

    pub(crate) fn stopping(&self) -> io::Result<()> {
        self.send("STOPPING=1")
    }

    /// A one-line status for `systemctl status`.
    pub(crate) fn status(&self, status: &str) -> io::Result<()> {
        self.send(&format!("STATUS={status}"))
    }

    /// Asks for `extension` more time to start or stop.
    pub(crate) fn extend_timeout(&self, extension: Duration) -> io::Result<()> {
        self.send(&format!("EXTEND_TIMEOUT_USEC={}", extension.as_micros()))
    }
}

/// How often to ping the watchdog, if it is enabled for this process:
/// `WATCHDOG_USEC` is set and `WATCHDOG_PID` is unset or this process.
pub(crate) fn watchdog_tick(var: impl Fn(&str) -> Option<String>) -> Option<Duration> {
    let usec = var(WATCHDOG_USEC)?.parse::<u64>().ok()?;
    if let Some(pid) = var(WATCHDOG_PID)
        && pid.parse::<u32>().ok()? != process::id()
    {
        return None;
    }
    Some(Duration::from_micros(usec) / PINGS_PER_PERIOD)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn sends_to_path_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notify");
        let receiver = UnixDatagram::bind(&path).unwrap();
        let notifier = Notifier::from_env(env(&[(NOTIFY_SOCKET, path.to_str().unwrap())])).unwrap();
        notifier.ready().unwrap();
        notifier.status("present").unwrap();
        let mut buffer = [0; 64];
        let n = receiver.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..n], b"READY=1");
        let n = receiver.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..n], b"STATUS=present");
    }

    #[test]
    fn sends_to_abstract_socket() {
        let name = format!("tempered-test-{}", process::id());
        let address = SocketAddr::from_abstract_name(&name).unwrap();
        let receiver = UnixDatagram::bind_addr(&address).unwrap();
        let notifier = Notifier::from_env(env(&[(NOTIFY_SOCKET, &format!("@{name}"))])).unwrap();
        notifier.extend_timeout(Duration::from_secs(2)).unwrap();
        let mut buffer = [0; 64];
        let n = receiver.recv(&mut buffer).unwrap();
        assert_eq!(&buffer[..n], b"EXTEND_TIMEOUT_USEC=2000000");
    }

    #[test]
    fn unset_or_unsupported_socket_is_silent() {
        for pairs in [&[][..], &[(NOTIFY_SOCKET, "vsock:2:1234")][..]] {
            let notifier = Notifier::from_env(env(pairs)).unwrap();
            assert!(notifier.target.is_none());
            notifier.watchdog().unwrap();
        }
    }

    #[test]
    fn watchdog_tick_from_env() {
        let pid = process::id().to_string();
        assert_eq!(watchdog_tick(env(&[])), None);
        assert_eq!(
            watchdog_tick(env(&[(WATCHDOG_USEC, "60000000")])),
            Some(Duration::from_secs(15))
        );
        assert_eq!(
            watchdog_tick(env(&[(WATCHDOG_USEC, "60000000"), (WATCHDOG_PID, &pid)])),
            Some(Duration::from_secs(15))
        );
        assert_eq!(
            watchdog_tick(env(&[(WATCHDOG_USEC, "60000000"), (WATCHDOG_PID, "1")])),
            None
        );
        assert_eq!(watchdog_tick(env(&[(WATCHDOG_USEC, "junk")])), None);
    }
}
