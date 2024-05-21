use std::{env, path::PathBuf, time::UNIX_EPOCH};

use chrono::Local;
use evdev::EventType;
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
    sync::mpsc,
};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let devices = evdev::enumerate().map(|t| t.1);
    let keyboard_devices = devices.filter(|device| {
        let Some(keys) = device.supported_keys() else {
            return false;
        };
        let mut keymap = keys.iter().map(|k| k.0).collect::<Vec<_>>();
        keymap.sort_unstable();
        let mut key_iter = keymap.iter();

        !(15..=54).all(|v| !key_iter.any(|key| *key == v))
    });

    let (tx, mut rx) = mpsc::channel(128);

    for device in keyboard_devices {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut stream = device
                .into_event_stream()
                .expect("Failed to create device stream");
            loop {
                let event = stream.next_event().await.expect("Failed to read event");
                if event.event_type() == EventType::KEY {
                    tx.send(event).await.unwrap();
                }
            }
        });
    }

    let user = env::var_os("SUDO_USER").expect("SUDO_USER not defined");
    let user = user.to_str().expect("Invalid user name");

    let datetime = Local::now();
    let date = datetime.format("%Y-%m-%d");

    let path = PathBuf::from(format!("/home/{user}/.keylogger/{date}.log"));

    fs::create_dir_all(
        path.parent()
            .expect("Something went wrong with path creation"),
    )
    .await
    .expect("Failed to create log directory");

    let mut log_file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .await?;

    while let Some(eventr) = rx.recv().await {
        let time = eventr.timestamp();
        let t = time.duration_since(UNIX_EPOCH).unwrap().as_nanos();
        log_file.write_all(format!("{t}\n").as_bytes()).await?;
    }

    Ok(())
}
