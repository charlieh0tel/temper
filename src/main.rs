use std::path::PathBuf;

use anyhow::Context;
use anyhow::bail;
use clap::Parser;
use clap::Subcommand;

use tempered::hidraw;
use tempered::hidraw::Hidraw;
use tempered::temper::Stick;

/// Present a PCsensor TEMPerGold USB thermometer as a Linux IIO device.
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
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let device = match cli.device {
        Some(device) => device,
        None => find_device()?,
    };
    let hidraw = Hidraw::open(&device).with_context(|| format!("opening {}", device.display()))?;
    let mut stick = Stick::new(hidraw);
    match cli.action {
        Action::Read => println!("{}", stick.temperature()?),
        Action::Info => {
            println!("device: {}", device.display());
            println!("firmware: {}", stick.firmware()?);
            let sensor_type = stick.sensor_type()?;
            println!(
                "sensor_type: inner=0x{:02x} outer=0x{:02x}",
                sensor_type.inner, sensor_type.outer
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
    }
    Ok(())
}

/// The only attached stick's data interface.
fn find_device() -> anyhow::Result<PathBuf> {
    let found = hidraw::discover().context("scanning for TEMPerGold sticks")?;
    match found.as_slice() {
        [device] => Ok(device.clone()),
        [] => bail!("no TEMPerGold found"),
        _ => bail!(
            "several TEMPerGold sticks found, choose one with --device: {}",
            found
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(" ")
        ),
    }
}
