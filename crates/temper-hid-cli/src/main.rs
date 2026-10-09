// The stick is reached through temper_hid::hid, which exists on Linux
// and Windows.
#[cfg(not(any(target_os = "linux", windows)))]
compile_error!("temper runs on Linux and Windows");

mod log;

use std::iter;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::ensure;
use clap::Parser;
use clap::Subcommand;
use temper_hid::protocol::Stick;
use temper_hid::schedule::MIN_INTERVAL;

use crate::log::TimeFormat;

/// The build's version, stamped by `build.rs`.
const VERSION: &str = env!("TEMPER_VERSION");

/// Read a PCsensor TEMPerGold, TEMPerHUM or TEMPer2 USB stick.
#[derive(Debug, Parser)]
#[command(name = "temper", version = VERSION)]
struct Cli {
    /// hidraw node of the stick's data interface; found if omitted.
    #[arg(long, global = true)]
    device: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

/// The subcommands, which read the stick directly.
#[derive(Debug, Subcommand)]
enum Command {
    /// Print the temperature in degrees C, then, for a stick that
    /// measures it, the relative humidity in percent; on a TEMPer2, the
    /// inner probe's temperature, then the outer probe's if fitted.
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
    match run(cli.device, cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(device: Option<PathBuf>, command: Command) -> anyhow::Result<()> {
    // The library's errors already name the node.
    let mut stick = match &device {
        Some(device) => Stick::open(device)?,
        None => Stick::find()?,
    };
    match command {
        Command::Read => {
            let reading = stick.reading()?;
            let values = iter::once(reading.temperature.to_string())
                .chain(reading.humidity.map(|humidity| humidity.to_string()))
                .chain(reading.outer_temperature.map(|outer| outer.to_string()))
                .collect::<Vec<_>>();
            println!("{}", values.join(" "));
        }
        Command::Info => {
            println!("device: {}", stick.transport().path().display());
            println!("firmware: {}", stick.firmware()?);
            println!("model: {}", stick.model()?);
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
            let reading = stick.reading()?;
            println!("temperature: {} C", reading.temperature);
            if let Some(humidity) = reading.humidity {
                println!("humidity: {humidity} %RH");
            }
            if let Some(outer) = reading.outer_temperature {
                println!("outer_temperature: {outer} C");
            }
        }
        Command::Log {
            interval,
            count,
            time,
        } => log::run(&mut stick, interval, count, time)?,
    }
    Ok(())
}

/// A polling interval such as `10s` or `1m` (jiff's friendly format),
/// at least [`MIN_INTERVAL`].
fn parse_interval(text: &str) -> anyhow::Result<Duration> {
    let interval = Duration::try_from(text.parse::<jiff::SignedDuration>()?)?;
    ensure!(
        interval >= MIN_INTERVAL,
        "interval must be at least {MIN_INTERVAL:?}"
    );
    Ok(interval)
}
