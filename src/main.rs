use anyhow::Context;
use keylogger_service::{create, run};
use log::trace;
use simplelog::{Config, LevelFilter, SimpleLogger};

macro_rules! check_error {
    ($e:expr) => {
        match $e {
            Ok(f) => f,
            Err(err) => {
                println!("{err:?}");
                return;
            }
        }
    };
}

fn main() {
    if let Err(err) = SimpleLogger::init(LevelFilter::Trace, Config::default()) {
        panic!("{err:?}");
    }

    trace!("Logger started");

    trace!("Starting daemon");
    check_error!(run());
}
