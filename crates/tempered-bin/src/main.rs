mod log;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the daemon, which arrives in phase 4")
)]
mod sensor;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the daemon, which arrives in phase 4")
)]
mod uhid;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context;
use anyhow::ensure;
use clap::Parser;
use clap::Subcommand;
use tempered::protocol::Stick;

use crate::log::TimeFormat;

/// Shortest `log` interval; the stick is slow to answer.
const MIN_LOG_INTERVAL: Duration = Duration::from_secs(1);

/// Read a PCsensor TEMPerGold USB thermometer and present it as a Linux
/// IIO device.
#[derive(Debug, Parser)]
#[command(version)]
struct Cli {
    /// hidraw node of the stick's data interface; found if omitted.
    #[arg(long, global = true)]
    device: Option<PathBuf>,

    #[command(subcommand)]
    action: Action,
}

#[derive(Debug, Subcommand)]
enum Action {
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

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut stick = match &cli.device {
        Some(device) => {
            Stick::open(device).with_context(|| format!("opening {}", device.display()))?
        }
        None => Stick::find()?,
    };
    match cli.action {
        Action::Read => println!("{}", stick.temperature()?),
        Action::Info => {
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
        Action::Log {
            interval,
            count,
            time,
        } => log::run(&mut stick, interval, count, time)?,
    }
    Ok(())
}

/// Parses a duration such as `10s` or `1m` (jiff's friendly format).
fn parse_interval(text: &str) -> anyhow::Result<Duration> {
    let interval = Duration::try_from(text.parse::<jiff::SignedDuration>()?)?;
    ensure!(
        interval >= MIN_LOG_INTERVAL,
        "interval must be at least {MIN_LOG_INTERVAL:?}"
    );
    Ok(interval)
}
