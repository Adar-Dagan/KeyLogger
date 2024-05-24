use daemon::run;
use log::trace;
use simplelog::{Config, LevelFilter, SimpleLogger};

fn main() {
    if let Err(err) = SimpleLogger::init(LevelFilter::Trace, Config::default()) {
        panic!("{err:?}");
    }

    trace!("Logger started");
    run();
}
