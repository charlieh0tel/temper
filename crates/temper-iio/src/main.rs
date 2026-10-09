mod daemon;
#[cfg(test)]
mod hardware_tests;
mod iio;
mod label;
mod listen;
mod logger;
mod mutex;
mod notify;
mod sensor;
mod supervisor;
#[cfg(test)]
mod test_support;
mod uhid;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::ensure;
use clap::CommandFactory;
use clap::Parser;
use clap::error::ErrorKind;
use temper_hid::schedule::MIN_INTERVAL;

use crate::daemon::Config;
use crate::label::Label;

/// The build's version, stamped by `temper-hid-cli`'s `build.rs`.
const VERSION: &str = env!("TEMPER_VERSION");

/// The hold must outlast this many intervals, so one missed reading
/// never expires it.
const MIN_HOLD_INTERVALS: u32 = 2;

/// Present a PCsensor TEMPerGold, TEMPerHUM or TEMPer2 USB stick as
/// Linux IIO devices until stopped.  Runs in the foreground, for systemd.
#[derive(Debug, Parser)]
#[command(name = "temper-iio", version = VERSION)]
struct Cli {
    /// hidraw node of the stick's data interface; found if omitted.
    #[arg(long)]
    device: Option<PathBuf>,
    /// Name of the link to the temperature IIO device under the run
    /// directory.
    #[arg(long, env = "TEMPER_IIO_LABEL", default_value = "temperature")]
    label: Label,
    /// Name of the link to the humidity IIO device, for a TEMPerHUM.
    #[arg(long, env = "TEMPER_IIO_HUMIDITY_LABEL", default_value = "humidity")]
    humidity_label: Label,
    /// Time between readings, at least 1s.
    #[arg(long, env = "TEMPER_IIO_INTERVAL", default_value = "10s", value_parser = parse_interval)]
    interval: Duration,
    /// How long to serve the last reading once the stick stops
    /// answering; at least twice the interval.
    #[arg(long, env = "TEMPER_IIO_HOLD", default_value = "60s", value_parser = parse_duration)]
    hold: Duration,
    /// Where the label's link and lock live.
    #[arg(long, hide = true, default_value = "/run/temper-iio")]
    run_dir: PathBuf,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.hold < cli.interval * MIN_HOLD_INTERVALS {
        invalid(format!(
            "--hold ({:?}) must be at least {MIN_HOLD_INTERVALS} times --interval ({:?})",
            cli.hold, cli.interval
        ));
    }
    if cli.label == cli.humidity_label {
        invalid(format!(
            "--label and --humidity-label must differ (both {})",
            cli.label
        ));
    }
    daemon::run(Config {
        device: cli.device,
        label: cli.label,
        humidity_label: cli.humidity_label,
        interval: cli.interval,
        hold: cli.hold,
        run_dir: cli.run_dir,
    })
    .into()
}

/// Exits with a clap validation error, status 2.
fn invalid(message: String) -> ! {
    Cli::command()
        .error(ErrorKind::ValueValidation, message)
        .exit()
}

/// Parses a duration such as `10s` or `1m` (jiff's friendly format).
fn parse_duration(text: &str) -> anyhow::Result<Duration> {
    Ok(Duration::try_from(text.parse::<jiff::SignedDuration>()?)?)
}

/// A polling interval: a duration of at least [`MIN_INTERVAL`].  Also
/// in `temper-hid-cli`.
fn parse_interval(text: &str) -> anyhow::Result<Duration> {
    let interval = parse_duration(text)?;
    ensure!(
        interval >= MIN_INTERVAL,
        "interval must be at least {MIN_INTERVAL:?}"
    );
    Ok(interval)
}
