//! Daemon logging to stderr: `sd-daemon(3)` priority prefixes under
//! journald, timestamps otherwise.

use std::fmt;
use std::io;
use std::io::Write;

use jiff::Timestamp;

const JOURNAL_STREAM: &str = "JOURNAL_STREAM";

/// A message's priority, as `syslog(3)` numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Priority {
    Error,
    Warning,
    Info,
    Debug,
}

impl Priority {
    fn number(self) -> u8 {
        match self {
            Self::Error => 3,
            Self::Warning => 4,
            Self::Info => 6,
            Self::Debug => 7,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }
}

/// Where log lines go and how they are marked.
#[derive(Debug)]
pub(crate) struct Logger {
    /// Whether stderr is journald's stream, which timestamps lines and
    /// reads the priority prefix.
    journal: bool,
}

impl Logger {
    /// Checks `JOURNAL_STREAM` ("device:inode", decimal) against stderr,
    /// as `systemd.exec(5)` describes.
    pub(crate) fn from_env(var: impl Fn(&str) -> Option<String>) -> Self {
        let journal = var(JOURNAL_STREAM).is_some_and(|stream| {
            rustix::fs::fstat(io::stderr())
                .is_ok_and(|stat| stream == format!("{}:{}", stat.st_dev, stat.st_ino))
        });
        Self { journal }
    }

    /// Writes one line; write errors are ignored.
    fn log(&self, priority: Priority, message: fmt::Arguments<'_>) {
        let line = self.format(priority, message, Timestamp::now());
        let _ = io::stderr().lock().write_all(line.as_bytes());
    }

    fn format(&self, priority: Priority, message: fmt::Arguments<'_>, now: Timestamp) -> String {
        if self.journal {
            format!("<{}>{message}\n", priority.number())
        } else {
            format!("{now:.3} {}: {message}\n", priority.name())
        }
    }

    pub(crate) fn error(&self, message: fmt::Arguments<'_>) {
        self.log(Priority::Error, message);
    }

    pub(crate) fn warning(&self, message: fmt::Arguments<'_>) {
        self.log(Priority::Warning, message);
    }

    pub(crate) fn info(&self, message: fmt::Arguments<'_>) {
        self.log(Priority::Info, message);
    }

    pub(crate) fn debug(&self, message: fmt::Arguments<'_>) {
        self.log(Priority::Debug, message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Timestamp {
        "2026-10-06T18:40:12.345678Z".parse().unwrap()
    }

    #[test]
    fn journal_lines_carry_priority() {
        let logger = Logger { journal: true };
        assert_eq!(
            logger.format(Priority::Warning, format_args!("stick failing"), now()),
            "<4>stick failing\n"
        );
    }

    #[test]
    fn plain_lines_carry_time_and_priority() {
        let logger = Logger { journal: false };
        assert_eq!(
            logger.format(Priority::Info, format_args!("device created"), now()),
            "2026-10-06T18:40:12.345Z info: device created\n"
        );
    }

    #[test]
    fn mismatched_journal_stream_is_plain() {
        let logger = Logger::from_env(|_| Some("1:1".to_owned()));
        assert!(!logger.journal);
        assert!(!Logger::from_env(|_| None).journal);
    }
}
