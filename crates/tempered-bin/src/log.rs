//! `tempered log`: one JSON object per reading on stdout (JSON Lines).

use std::io;
use std::io::Write;
use std::thread;
use std::time::Duration;
use std::time::Instant;

use clap::ValueEnum;
use jiff::Timestamp;
use serde_json::Value;
use serde_json::json;
use tempered::protocol::CentiCelsius;
use tempered::protocol::Stick;
use tempered::protocol::Transport;

/// Centidegrees per degree, for `temperature_c`.
const CENTI_PER_UNIT: f64 = 100.0;

/// Milliseconds per second, for `--time unix`.
const MILLIS_PER_SECOND: f64 = 1000.0;

/// How the `time` field is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum TimeFormat {
    /// RFC 3339 in UTC with milliseconds, e.g. `2026-10-06T18:40:12.345Z`.
    Rfc3339,
    /// Seconds since the Unix epoch with milliseconds, e.g. `1791312012.345`.
    Unix,
}

/// Reads the stick every `interval`, `count` times or forever, writing
/// one line per reading.  Readings follow a fixed schedule, so a slow
/// read does not make later ones drift; a missed slot is skipped.
pub(crate) fn run<T: Transport>(
    stick: &mut Stick<T>,
    interval: Duration,
    count: Option<u64>,
    time_format: TimeFormat,
) -> io::Result<()> {
    let start = Instant::now();
    let mut stdout = io::stdout().lock();
    for _ in 0..count.unwrap_or(u64::MAX) {
        let now = Timestamp::now();
        let reading = stick.temperature().map_err(anyhow::Error::from);
        let line = line(now, time_format, reading.as_ref().copied());
        if let Err(error) = writeln!(stdout, "{line}").and_then(|()| stdout.flush()) {
            return match error.kind() {
                io::ErrorKind::BrokenPipe => Ok(()),
                _ => Err(error),
            };
        }
        let elapsed = start.elapsed();
        let next = interval * slot_after(elapsed, interval);
        thread::sleep(next.saturating_sub(elapsed));
    }
    Ok(())
}

/// The first schedule slot strictly after `elapsed`.
fn slot_after(elapsed: Duration, interval: Duration) -> u32 {
    let slots = elapsed.as_nanos() / interval.as_nanos() + 1;
    u32::try_from(slots).unwrap_or(u32::MAX)
}

/// One output line for a reading taken at `time`.
fn line(
    time: Timestamp,
    time_format: TimeFormat,
    reading: Result<CentiCelsius, &anyhow::Error>,
) -> Value {
    let time = match time_format {
        TimeFormat::Rfc3339 => json!(format!("{time:.3}")),
        TimeFormat::Unix => json!(time.as_millisecond() as f64 / MILLIS_PER_SECOND),
    };
    match reading {
        Ok(temperature) => json!({
            "time": time,
            "temperature_c": f64::from(temperature.get()) / CENTI_PER_UNIT,
            "centi_celsius": temperature.get(),
        }),
        Err(error) => json!({
            "time": time,
            "error": format!("{error:#}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time() -> Timestamp {
        "2026-10-06T18:40:12.345678Z".parse().unwrap()
    }

    #[test]
    fn reading_rfc3339() {
        let reading = Ok(CentiCelsius::new(3493));
        assert_eq!(
            line(time(), TimeFormat::Rfc3339, reading).to_string(),
            r#"{"time":"2026-10-06T18:40:12.345Z","temperature_c":34.93,"centi_celsius":3493}"#
        );
    }

    #[test]
    fn reading_unix() {
        let reading = Ok(CentiCelsius::new(-50));
        assert_eq!(
            line(time(), TimeFormat::Unix, reading).to_string(),
            r#"{"time":1791312012.345,"temperature_c":-0.5,"centi_celsius":-50}"#
        );
    }

    #[test]
    fn error_line() {
        let error = anyhow::anyhow!("inner").context("outer \"quoted\"");
        assert_eq!(
            line(time(), TimeFormat::Rfc3339, Err(&error)).to_string(),
            r#"{"time":"2026-10-06T18:40:12.345Z","error":"outer \"quoted\": inner"}"#
        );
    }

    #[test]
    fn schedule_slots() {
        let interval = Duration::from_secs(10);
        assert_eq!(slot_after(Duration::ZERO, interval), 1);
        assert_eq!(slot_after(Duration::from_millis(9_999), interval), 1);
        assert_eq!(slot_after(Duration::from_secs(10), interval), 2);
        assert_eq!(slot_after(Duration::from_secs(35), interval), 4);
    }
}
