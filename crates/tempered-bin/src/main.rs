mod daemon;
#[cfg(test)]
mod hardware_tests;
mod iio;
mod label;
mod listen;
mod log;
mod logger;
mod mutex;
mod notify;
mod schedule;
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
use clap::Subcommand;
use clap::error::ErrorKind;
use tempered_hid::protocol::Stick;

use crate::daemon::Config;
use crate::label::Label;
use crate::log::TimeFormat;

/// The build's version, stamped by `build.rs`.
const VERSION: &str = env!("TEMPERED_VERSION");

/// Shortest polling interval; the stick is slow to answer.
const MIN_INTERVAL: Duration = Duration::from_secs(1);

/// The hold must outlast this many intervals, so one missed reading
/// never expires it.
const MIN_HOLD_INTERVALS: u32 = 2;

/// Read a PCsensor TEMPerGold USB thermometer and present it as a Linux
/// IIO device.
#[derive(Debug, Parser)]
#[command(name = "tempered", version = VERSION)]
struct Cli {
    /// hidraw node of the stick's data interface; found if omitted.
    #[arg(long, global = true)]
    device: Option<PathBuf>,

    #[command(subcommand)]
    action: Action,
}

/// The subcommands.
#[derive(Debug, Subcommand)]
enum Action {
    #[command(flatten)]
    Tool(Tool),
    /// Present the stick as an IIO device until stopped.  Runs in the
    /// foreground, for systemd.
    Daemon {
        /// Name of the link to the IIO device under the run directory.
        #[arg(long, env = "TEMPERED_LABEL", default_value = "temperature")]
        label: Label,
        /// Time between readings, at least 1s.
        #[arg(long, env = "TEMPERED_INTERVAL", default_value = "10s", value_parser = parse_interval)]
        interval: Duration,
        /// How long to serve the last reading once the stick stops
        /// answering; at least twice the interval.
        #[arg(long, env = "TEMPERED_HOLD", default_value = "60s", value_parser = parse_duration)]
        hold: Duration,
        /// Where the label's link and lock live.
        #[arg(long, hide = true, default_value = "/run/tempered")]
        run_dir: PathBuf,
    },
}

/// The diagnostic subcommands, which read the stick directly.
#[derive(Debug, Subcommand)]
enum Tool {
    /// Print the temperature in degrees C.
    Read,
    /// Print everything the stick reports about itself.
    Info,
    /// Print a JSON line per reading, every interval, until stopped.
    Log {
        /// Time between readings, at least 1s, e.g. `10s`, `1m`, `1.5s`.
        #[arg(long, default_value = "10s", value_parser = parse_interval)]
        interval: Duration,
        /// Stop after this many readings.
        #[arg(long)]
        count: Option<u64>,
        /// How to write each line's `time`.
        #[arg(long, value_enum, default_value_t = TimeFormat::Rfc3339)]
        time: TimeFormat,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.action {
        Action::Daemon {
            label,
            interval,
            hold,
            run_dir,
        } => {
            if hold < interval * MIN_HOLD_INTERVALS {
                Cli::command()
                    .error(
                        ErrorKind::ValueValidation,
                        format!(
                            "--hold ({hold:?}) must be at least {MIN_HOLD_INTERVALS} \
                             times --interval ({interval:?})"
                        ),
                    )
                    .exit();
            }
            daemon::run(Config {
                device: cli.device,
                label,
                interval,
                hold,
                run_dir,
            })
            .into()
        }
        Action::Tool(action) => match tool(cli.device, action) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Error: {error:#}");
                ExitCode::FAILURE
            }
        },
    }
}

/// The diagnostic commands.
fn tool(device: Option<PathBuf>, action: Tool) -> anyhow::Result<()> {
    // The library's errors already name the node.
    let mut stick = match &device {
        Some(device) => Stick::open(device)?,
        None => Stick::find()?,
    };
    match action {
        Tool::Read => println!("{}", stick.temperature()?),
        Tool::Info => {
            println!("device: {}", stick.transport().path().display());
            println!("firmware: {}", stick.firmware()?);
            let sensor_type = stick.sensor_type()?;
            println!(
                "sensor_type: inner=0x{:02x} outer=0x{:02x}",
                sensor_type.inner.code(),
                sensor_type.outer.code()
            );
            let calibration = stick.calibration()?;
            println!(
                "calibration: inner_temperature={} inner_humidity={} \
                 outer_temperature={} outer_humidity={}",
                calibration.inner_temperature,
                calibration.inner_humidity,
                calibration.outer_temperature,
                calibration.outer_humidity
            );
            println!("manufacture_date: {}", stick.manufacture_date()?);
            println!("temperature: {} C", stick.temperature()?);
        }
        Tool::Log {
            interval,
            count,
            time,
        } => log::run(&mut stick, interval, count, time)?,
    }
    Ok(())
}

/// Parses a duration such as `10s` or `1m` (jiff's friendly format).
fn parse_duration(text: &str) -> anyhow::Result<Duration> {
    Ok(Duration::try_from(text.parse::<jiff::SignedDuration>()?)?)
}

/// A polling interval: a duration of at least [`MIN_INTERVAL`].
fn parse_interval(text: &str) -> anyhow::Result<Duration> {
    let interval = parse_duration(text)?;
    ensure!(
        interval >= MIN_INTERVAL,
        "interval must be at least {MIN_INTERVAL:?}"
    );
    Ok(interval)
}
