use std::{
    env,
    fs::{self, File},
    path::PathBuf,
};

use anyhow::{Context, Result};
use chrono::Local;
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
        /// File to put logs in
        #[arg(long, group = "file")]
        file_path: Option<PathBuf>,
        /// Directory to put logs in, will generate files in the given directory
        #[arg(long, group = "file")]
        directory_path: Option<PathBuf>,
    },
    /// Search for active keylogger and stop it
    Stop,
}

#[allow(clippy::unwrap_used)]
fn main() {
    if let Err(err) = SimpleLogger::init(LevelFilter::Trace, Config::default()) {
        panic!("{err:?}");
    }
    let cli = Cli::parse();

    match cli.command {
        Commands::Start {
            in_place,
            file_path,
            directory_path,
        } => {
            let log_file = match open_log_file(file_path, directory_path) {
                Ok(f) => f,
                Err(err) => {
                    error!("{err:?}");
                    return;
                }
            };

            let result = if in_place {
                trace!("Starting keylogger in place");
                start(log_file)
            } else {
                trace!("Creating daemon with keylogger");
                create(log_file)
            };

            if let Err(err) = result {
                error!("{err:?}");
            }
        }
        Commands::Stop => {
            if let Err(err) = stop() {
                error!("{err:?}");
            }
        }
    }
}

fn open_log_file(file_path: Option<PathBuf>, directory_path: Option<PathBuf>) -> Result<File> {
    if file_path.is_some() && directory_path.is_some() {
        unreachable!("The clap group 'file' should prevent this case from happenning");
    }

    let log_file_path: PathBuf = file_path.map(PathBuf::from).ok_or(()).or_else(|()| {
        let mut pathbuf = directory_path.ok_or(()).or_else(|()| {
            let user = env::var_os("SUDO_USER").context("SUDO_USER not defined")?;
            let user = user.to_str().context("Invalid user name")?;

            anyhow::Ok(PathBuf::from(format!("/home/{user}/.keylogger")))
        })?;

        let datetime = Local::now();
        let date = datetime.format("%Y-%m-%d");

        pathbuf.push(format!("{date}"));
        pathbuf.set_extension("log");

        anyhow::Ok(pathbuf)
    })?;

    fs::create_dir_all(
        log_file_path
            .parent()
            .context("Something went wrong with path creation")?,
    )
    .context("Failed to create log directory")?;

    let file = File::options()
        .append(true)
        .create(true)
        .open(log_file_path)
        .context("Failed to open log file")?;

    Ok(file)
}
