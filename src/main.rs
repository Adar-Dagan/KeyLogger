use std::{
    collections::BTreeSet,
    io::Error,
    mem::{size_of, transmute},
};

use chrono::Local;
use input_linux::sys::{input_event, EV_KEY};
use tokio::{
    fs::{File, OpenOptions},
    io::AsyncReadExt,
    sync::mpsc,
};

#[derive(Debug)]
struct Event {
    number: usize,
    keys: BTreeSet<usize>,
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let events = get_event_paths().await?;
    let keyboard_events = events.iter().filter(|event| {
        let mut current_key = 15;
        for key in &event.keys {
            if current_key != 54 && *key == current_key {
                current_key += 1;
            }
        }
        current_key == 54
    });

    let (tx, mut rx) = mpsc::channel(128);

    for event in keyboard_events {
        let tx = tx.clone();
        let event_number = event.number;
        tokio::spawn(async move {
            let event_path = format!("/dev/input/event{event_number}");
            let mut file = File::open(event_path)
                .await
                .expect("Failed to open event file. Sudo please");
            loop {
                let event = read_event(&mut file).await.unwrap();
                if i32::from(event.type_) == EV_KEY {
                    tx.send(event).await.unwrap();
                }
            }
        });
    }

    while let Some(eventr) = rx.recv().await {
        println!("{eventr:?}");
    }

    Ok(())
}

async fn get_event_paths() -> Result<Vec<Event>, Error> {
    let mut devices_file = File::open("/proc/bus/input/devices").await?;
    let mut buffer = Vec::new();

    devices_file.read_to_end(&mut buffer).await?;

    let result = String::from_utf8(buffer).unwrap();

    let devices = result
        .split("\n\n")
        .filter_map(|device| {
            if !device.contains("KEY") {
                return None;
            }
            let mut pairs = device.split('\n').filter_map(|line| {
                line.split_once(' ')
                    .and_then(|(_, pair)| pair.split_once('='))
            });

            let handlers = pairs
                .find(|(key, _)| *key == "Handlers")
                .map(|(_, value)| parse_events(value))?;
            let values = pairs
                .find(|(key, _)| *key == "KEY")
                .map(|(_, value)| parse_keys(value))
                .expect("Failed to find Keys");

            assert!(handlers.len() == 1);

            Some(Event {
                number: handlers[0],
                keys: values,
            })
        })
        .collect::<Vec<_>>();

    Ok(devices)
}

fn parse_events(value: &str) -> Vec<usize> {
    let event = value
        .split(' ')
        .filter(|s| s.contains("event"))
        .map(|event| {
            let (_, num) = event.split_once('t').unwrap();

            num.parse::<usize>().expect("Error parsing event number")
        })
        .collect::<Vec<_>>();
    event
}

fn parse_keys(value: &str) -> BTreeSet<usize> {
    let values = value
        .split(' ')
        .rev()
        .enumerate()
        .fold(BTreeSet::new(), |mut acc, (i, v)| {
            let mut bitmap = u64::from_str_radix(v, 16).expect("Failed to parse hex string");
            let mut counter = 0;
            while bitmap != 0 {
                if bitmap & 1 == 1 {
                    let index = i * 64 + counter;
                    acc.insert(index);
                }
                bitmap >>= 1;
                counter += 1;
            }
            acc
        });
    values
}

async fn read_event(file: &mut File) -> Result<input_event, std::io::Error> {
    let mut buf = [0u8; size_of::<input_event>()];
    file.read_exact(&mut buf).await?;

    let event: input_event = unsafe { transmute(buf) };
    Ok(event)
}
