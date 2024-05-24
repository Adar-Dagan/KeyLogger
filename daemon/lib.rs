use std::{env, path::PathBuf, process::exit, time::UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use chrono::Local;
use daemonize::Daemonize;
use evdev::{Device, InputEvent, InputEventKind, Key, LedType};
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

/// .
///
/// # Panics
///
/// Panics if .
pub fn run() -> ! {
    let stdout = std::fs::File::create("./tmpout").unwrap();
    let stderr = std::fs::File::create("./tmperr").unwrap();

    let daemonizer = Daemonize::new()
        .pid_file("./tmp.pid")
        .stdout(stdout)
        .stderr(stderr)
        .privileged_action(|| "Executed before drop privileges");

    match daemonizer.start() {
        Err(_) => {
            println!("Fail to create daemon. Missing sudo?");
        }
        Ok(_) => {
            if let Err(err) = main() {
                println!("{err}");
            }
        }
    }

    println!("exiting");

    exit(1)
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("here");
    let devices: Vec<Device> = evdev::enumerate().map(|t| t.1).collect();
    println!("here");

    if devices.is_empty() {
        bail!("Coundn't find any device. Sudo Please");
    }

    let initial_state: State = get_keyboard_state(&devices)?;

    let mut join_set = JoinSet::new();
    let rx = spawn_device_listeners(devices, &mut join_set);

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
    let led_state: evdev::AttributeSet<LedType> = devices
        .iter()
        .find(|device| {
            let Some(leds) = device.supported_leds() else {
                return false;
            };
            [LedType::LED_CAPSL, LedType::LED_NUML]
                .iter()
                .all(|led| leds.contains(*led))
        })
        .and_then(|device| device.get_led_state().ok())
        .with_context(|| "Couldn't find device to read caps and num lock from")?;

    Ok(State::new(
        led_state.contains(LedType::LED_CAPSL),
        led_state.contains(LedType::LED_NUML),
    ))
}

fn spawn_device_listeners(
    devices: Vec<Device>,
    join_map: &mut JoinSet<Result<()>>,
) -> Receiver<InputEvent> {
    let (tx, rx) = mpsc::channel(32);

    for device in devices {
        let tx = tx.clone();
        join_map.spawn(async move {
            let mut stream = device
                .into_event_stream()
                .with_context(|| "Failed to create device stream")?;
            loop {
                let event = stream
                    .next_event()
                    .await
                    .with_context(|| "Failed to read event")?;
                if tx.send(event).await.is_err() {
                    bail!("Failed to send event");
                }
            }
        });
    }

    rx
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
        let t = time.duration_since(UNIX_EPOCH).unwrap().as_nanos();
        log_file
            .write_all(format!("{t}\n").as_bytes())
            .await
            .with_context(|| "Failed to write to log")?;
        state.update(eventr);
        println!("{eventr:?}");
    }
    bail!("Event receiver was closed")
}
