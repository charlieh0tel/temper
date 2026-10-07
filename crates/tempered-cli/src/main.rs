use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use clap::Subcommand;
use tempered::protocol::Stick;

/// Read a PCsensor TEMPerGold USB thermometer.
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
    }
    Ok(())
}
