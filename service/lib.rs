use std::{
    env,
    path::{Path, PathBuf},
    process::exit,
    time::UNIX_EPOCH,
};

use anyhow::{bail, Context, Error, Result};
use chrono::Local;
use daemonize::{Daemonize, Outcome, Stdio};
use evdev::{Device, InputEvent, InputEventKind, Key, LedType};
use log::{debug, error, trace, LevelFilter};
use simplelog::{Config, SimpleLogger};
use tokio::{
    fs::{self, File, OpenOptions},
    io::AsyncWriteExt,
    sync::mpsc::{self, Receiver},
    task::JoinSet,
};

#[derive(Debug)]
struct State {
    caps_lock: bool,
    num_lock: bool,
    shift_count: i32,
    alt_count: i32,
    ctrl_count: i32,
    meta_count: i32,
}

impl State {
    const fn new(caps_lock: bool, num_lock: bool) -> Self {
        Self {
            caps_lock,
            num_lock,
            shift_count: 0,
            alt_count: 0,
            ctrl_count: 0,
            meta_count: 0,
        }
    }

    fn update(&mut self, event: InputEvent) {
        let value: i32 = event.value();
        match event.kind() {
            InputEventKind::Led(led) => match led {
                LedType::LED_CAPSL => self.caps_lock = value == 1,
                LedType::LED_NUML => self.num_lock = value == 1,
                _ => {}
            },
            InputEventKind::Key(key) => {
                if let 0 | 1 = value {
                    let counter = match key {
                        Key::KEY_LEFTSHIFT | Key::KEY_RIGHTSHIFT => &mut self.shift_count,
                        Key::KEY_RIGHTALT | Key::KEY_LEFTALT => &mut self.alt_count,
                        Key::KEY_RIGHTCTRL | Key::KEY_LEFTCTRL => &mut self.ctrl_count,
                        Key::KEY_LEFTMETA | Key::KEY_RIGHTMETA => &mut self.meta_count,
                        _ => return,
                    };
                    Self::update_counter(counter, value);
                }
            }
            _ => {}
        }
    }

    fn update_counter(counter: &mut i32, value: i32) {
        let new_val: i32 = *counter + value * 2 - 1;
        *counter = if new_val >= 0 { new_val } else { 0 }
    }
}

/// Creates keylogger daemon service. Will fork off a process and calls [run()]
///
/// # Errors
///
/// This function will return an error if daemon creation failed.
pub fn create() -> Result<()> {
    let daemonizer = Daemonize::new()
        .pid_file("/run/keylogger.pid")
        .privileged_action(|| "Executed before drop privileges");

    match daemonizer.execute() {
        Outcome::Parent(res) => {
            res.with_context(|| "Failed to start daemon")?;
        }
        Outcome::Child(res) => {
            error!(
                "{:?}",
                if let Err(err) = res.with_context(|| "Daemon failed to start") {
                    err
                } else {
                    debug!("Daemon started succesfully");
                    if let Err(err) = run().with_context(|| "Fatal error") {
                        err
                    } else {
                        Error::msg("Unknown error occured")
                    }
                }
            );
            exit(1);
        }
    }

    Ok(())
}

/// Runs the keylogger in the current process
///
/// # Errors
///
/// This function will return an error if a fatal error occurred, mean the keylogger could not
/// recover from the error.
#[tokio::main]
pub async fn run() -> Result<()> {
    trace!("keylogger starting");
    let devices: Vec<Device> = evdev::enumerate().map(|t| t.1).collect();

    if devices.is_empty() {
        bail!("Coundn't find any device.");
    }

    let initial_state: State =
        get_keyboard_state(&devices).with_context(|| "Failed to initialize keyboard state")?;

    let (rx, mut join_set) = spawn_device_listeners(devices);

    join_set.spawn(async move {
        spawn_event_reader(rx, initial_state).await?;
        Ok(())
    });

    if let Some(result) = join_set.join_next().await {
        result??;
    }

    bail!("Unkown error occured");
}

fn get_keyboard_state(devices: &[Device]) -> Result<State> {
    // let led_state: evdev::AttributeSet<LedType> = devices
    let led_device = devices
        .iter()
        .find(|device| {
            let Some(leds) = device.supported_leds() else {
                return false;
            };
            [LedType::LED_CAPSL, LedType::LED_NUML]
                .iter()
                .all(|led| leds.contains(*led))
        })
        .ok_or_else(|| Error::msg("Couldn't find device to read caps and num lock from"))?;
    let led_state = led_device.get_led_state()?;

    Ok(State::new(
        led_state.contains(LedType::LED_CAPSL),
        led_state.contains(LedType::LED_NUML),
    ))
}

fn spawn_device_listeners(devices: Vec<Device>) -> (Receiver<InputEvent>, JoinSet<Result<()>>) {
    let (tx, rx) = mpsc::channel(32);
    let mut join_set = JoinSet::<Result<()>>::new();

    for device in devices {
        let tx = tx.clone();
        join_set.spawn(async move {
            let mut stream = device
                .into_event_stream()
                .with_context(|| "Failed to create device stream")?;
            loop {
                let event = stream
                    .next_event()
                    .await
                    .with_context(|| "Failed to read event")?;
                tx.send(event)
                    .await
                    .with_context(|| "Failed to send event")?;
            }
        });
    }

    (rx, join_set)
}

async fn open_log_file() -> Result<File> {
    let user = env::var_os("SUDO_USER").with_context(|| "SUDO_USER not defined")?;
    let user = user.to_str().with_context(|| "Invalid user name")?;

    let datetime = Local::now();
    let date = datetime.format("%Y-%m-%d");

    let path = PathBuf::from(format!("/home/{user}/.keylogger/{date}.log"));

    fs::create_dir_all(
        path.parent()
            .with_context(|| "Something went wrong with path creation")?,
    )
    .await
    .with_context(|| "Failed to create log directory")?;

    Ok(OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .await?)
}

async fn spawn_event_reader(mut rx: Receiver<InputEvent>, initial_state: State) -> Result<()> {
    let mut state = initial_state;

    let mut log_file = open_log_file()
        .await
        .with_context(|| "Failed to open log file")?;

    while let Some(eventr) = rx.recv().await {
        let time = eventr.timestamp();
        let t = time
            .duration_since(UNIX_EPOCH)
            .with_context(|| "UNIX_EPOCHE is after the timestamp of the event???")?
            .as_nanos();
        log_file
            .write_all(format!("{t}\n").as_bytes())
            .await
            .with_context(|| "Failed to write to log")?;
        state.update(eventr);
        trace!("{eventr:?}");
    }
    bail!("Event receiver was closed")
}