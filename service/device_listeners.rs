use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{bail, Context, Result};
use evdev::{Device, InputEvent};
use log::{debug, error, info, trace};
use notify::{event::CreateKind, Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::{
    sync::{
        mpsc::{self, Receiver, Sender},
        Mutex,
    },
    task::{JoinHandle, JoinSet},
};

pub struct Listeners {
    rx: Receiver<InputEvent>,
    join_set: Arc<Mutex<JoinSet<()>>>,
    watcher_handle: JoinHandle<()>,
}

impl Listeners {
    pub fn spawn() -> Result<Self> {
        let devices: HashMap<PathBuf, Device> = evdev::enumerate().collect();

        if devices.is_empty() {
            bail!("Coundn't find any device. Missing sudo?");
        }
        trace!("Found {} devices", devices.len());

        let (tx, rx) = mpsc::channel(32);

        let listening_paths = devices.keys().cloned().collect::<HashSet<_>>();
        let listening_paths_lock = Arc::new(Mutex::new(listening_paths));

        let mut join_set = JoinSet::new();

        for (path, device) in devices {
            let tx = tx.clone();
            let listening_paths_lock = listening_paths_lock.clone();
            join_set.spawn(async move {
                let result = device_listener(device, tx).await;
                info!("Device closed, {result:?}");
                listening_paths_lock.lock().await.remove(&path);
            });
        }

        let join_set = Arc::new(Mutex::new(join_set));
        let watcher_handle = match Self::device_watcher(join_set.clone(), listening_paths_lock, tx)
        {
            Ok(handle) => handle,
            Err(e) => {
                join_set.blocking_lock().abort_all();
                return Err(e);
            }
        };

        Ok(Self {
            rx,
            join_set,
            watcher_handle,
        })
    }

    fn device_watcher(
        join_set: Arc<Mutex<JoinSet<()>>>,
        listening_paths_lock: Arc<Mutex<HashSet<PathBuf>>>,
        key_event_sender: Sender<InputEvent>,
    ) -> Result<JoinHandle<()>> {
        let (tx, mut rx) = mpsc::channel(8);
        let mut watcher = RecommendedWatcher::new(
            move |result| {
                if let Err(e) = tx.blocking_send(result) {
                    error!("Failed to send event to device watcher: {e}");
                }
            },
            Config::default(),
        )
        .context("Failed to create directory watcher")?;

        watcher
            .watch(Path::new("/dev/input/"), RecursiveMode::NonRecursive)
            .context("Failed to watch devices directory")?;

        // Look its a face
        let handle = tokio::spawn(async move {
            // Need to move watcher so that it isn't dropped
            let _watcher = watcher;
            while let Some(result) = rx.recv().await {
                match result {
                    Ok(event) if event.kind == EventKind::Create(CreateKind::File) => {
                        debug!("Received create file event: {event:?}");
                        spawn_devices_from_event(
                            event,
                            &listening_paths_lock,
                            &key_event_sender,
                            &join_set,
                        )
                        .await;
                    }
                    Ok(event) => {
                        debug!("Got event: {event:?}");
                        continue;
                    }
                    Err(e) => {
                        error!("Failed to read event: {e}");
                        continue;
                    }
                };
            }
        });
        Ok(handle)
    }

    pub fn next_event(&mut self) -> Option<InputEvent> {
        self.rx.blocking_recv()
    }
}

async fn spawn_devices_from_event(
    event: notify::Event,
    listening_paths_lock: &Arc<Mutex<HashSet<PathBuf>>>,
    key_event_sender: &Sender<InputEvent>,
    join_set: &Arc<Mutex<JoinSet<()>>>,
) {
    for path in event.paths {
        trace!("Device created: {path:?}");
        if !listening_paths_lock.lock().await.insert(path.clone()) {
            debug!("Device already listening: {path:?}");
            continue;
        }

        let tx = key_event_sender.clone();
        let listening_paths_lock = listening_paths_lock.clone();
        join_set.lock().await.spawn(async move {
            let result = Device::open(&path);
            if let Ok(device) = result {
                let result = device_listener(device, tx).await;
                info!("Device closed, {result:?}");
            } else {
                error!("Failed to open device {path:?}");
            }
            listening_paths_lock.lock().await.remove(&path);
        });
    }
}

async fn device_listener(device: Device, tx: Sender<InputEvent>) -> Result<()> {
    let mut stream = device
        .into_event_stream()
        .context("Failed to create device stream")?;
    loop {
        let event = stream.next_event().await.context("Failed to read event")?;
        tx.send(event).await.context("Failed to send event")?;
    }
}

impl Drop for Listeners {
    fn drop(&mut self) {
        self.watcher_handle.abort();
        self.join_set.blocking_lock().abort_all();
    }
}
