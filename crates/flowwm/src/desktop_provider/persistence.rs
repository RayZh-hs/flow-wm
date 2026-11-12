//! A single pending snapshot keeps rapidly changing native metadata off the event loop.
use super::Bindings;
use anyhow::Result;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[derive(Default)]
struct Pending {
    bytes: Option<Arc<Vec<u8>>>,
    revision: u64,
    completed: u64,
    error: Option<String>,
    urgent: u64,
    closed: bool,
}

pub(super) struct Writer {
    pending: Arc<(Mutex<Pending>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

impl Writer {
    pub fn new(directory: PathBuf, interval: Duration) -> Result<Self> {
        Self::start(interval, move |bytes| {
            let temporary = directory.join("desktop.json.tmp");
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            std::fs::rename(temporary, directory.join("desktop.json"))?;
            File::open(&directory)?.sync_all()?;
            Ok(())
        })
    }

    fn start(
        interval: Duration,
        mut write: impl FnMut(&[u8]) -> Result<()> + Send + 'static,
    ) -> Result<Self> {
        let pending = Arc::new((Mutex::new(Pending::default()), Condvar::new()));
        let shared = pending.clone();
        let worker = thread::Builder::new()
            .name("desktop-storage".into())
            .spawn(move || {
                let mut last_save: Option<Instant> = None;
                loop {
                    let mut state = shared.0.lock().unwrap();
                    while state.revision == state.completed && !state.closed {
                        state = shared.1.wait(state).unwrap();
                    }
                    if state.revision == state.completed && state.closed {
                        break;
                    }
                    // Use a fixed deadline, rather than resetting a debounce on
                    // every title event: continuously animated titles still save.
                    if state.urgent <= state.completed
                        && !state.closed
                        && let Some(remaining) =
                            last_save.and_then(|last| interval.checked_sub(last.elapsed()))
                        && !remaining.is_zero()
                    {
                        drop(shared.1.wait_timeout(state, remaining).unwrap());
                        continue;
                    }
                    let bytes = state.bytes.as_ref().unwrap().clone();
                    let revision = state.revision;
                    drop(state);

                    // Never hold the producer's mutex during filesystem I/O.
                    let error = write(&bytes).err().map(|error| format!("{error:#}"));
                    if let Some(error) = &error {
                        eprintln!("FlowWM desktop persistence: {error}");
                    }
                    last_save = Some(Instant::now());
                    let mut state = shared.0.lock().unwrap();
                    state.completed = revision;
                    state.error = error;
                    shared.1.notify_all();
                }
            })?;
        Ok(Self {
            pending,
            worker: Some(worker),
        })
    }

    fn queue(&self, bytes: Vec<u8>) {
        let mut state = self.pending.0.lock().unwrap();
        if state.bytes.as_deref() == Some(&bytes)
            && (state.completed < state.revision || state.error.is_none())
        {
            return;
        }
        state.bytes = Some(Arc::new(bytes));
        state.revision += 1;
        self.pending.1.notify_all();
    }

    pub fn update(&self, bindings: &Bindings) -> Result<()> {
        self.queue(serde_json::to_vec_pretty(bindings)?);
        Ok(())
    }

    pub fn save(&self, bindings: &Bindings) -> Result<()> {
        self.update(bindings)?;
        self.flush()
    }

    fn flush(&self) -> Result<()> {
        let mut state = self.pending.0.lock().unwrap();
        let revision = state.revision;
        state.urgent = revision;
        self.pending.1.notify_all();
        while state.completed < revision {
            state = self.pending.1.wait(state).unwrap();
        }
        if let Some(error) = &state.error {
            anyhow::bail!("{error}");
        }
        Ok(())
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.pending.0.lock().unwrap().closed = true;
        self.pending.1.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };

    #[test]
    fn a_blocked_disk_does_not_block_updates_and_only_the_latest_snapshot_is_saved() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let saved = writes.clone();
        let (started, writing) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let writer = Arc::new(
            Writer::start(Duration::from_secs(60), move |bytes| {
                if saved.lock().unwrap().is_empty() {
                    started.send(()).unwrap();
                    resume.recv().unwrap();
                }
                saved.lock().unwrap().push(bytes.to_vec());
                Ok(())
            })
            .unwrap(),
        );
        writer.queue(b"initial".to_vec());
        writing.recv_timeout(Duration::from_secs(5)).unwrap();
        let producer = writer.clone();
        let (done, completed) = mpsc::channel();
        let updates = thread::spawn(move || {
            for i in 0..1000 {
                producer.queue(format!("title-{i}").into_bytes());
            }
            done.send(()).unwrap();
        });
        let responsive = completed.recv_timeout(Duration::from_secs(5)).is_ok();
        release.send(()).unwrap();
        updates.join().unwrap();
        assert!(responsive, "updates waited for disk I/O");
        writer.flush().unwrap();
        assert_eq!(
            *writes.lock().unwrap(),
            [b"initial".to_vec(), b"title-999".to_vec()]
        );
    }

    #[test]
    fn a_failed_save_is_reported_and_the_same_snapshot_can_be_retried() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let calls = attempts.clone();
        let writer = Writer::start(Duration::from_secs(60), move |_| {
            anyhow::ensure!(calls.fetch_add(1, Ordering::Relaxed) > 0, "disk failed");
            Ok(())
        })
        .unwrap();
        writer.queue(b"bindings".to_vec());
        assert!(
            writer
                .flush()
                .unwrap_err()
                .to_string()
                .contains("disk failed")
        );
        writer.queue(b"bindings".to_vec());
        writer.flush().unwrap();
        assert_eq!(attempts.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn closing_flushes_pending_metadata_without_waiting_for_the_interval() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let saved = writes.clone();
        let writer = Writer::start(Duration::from_secs(60), move |bytes| {
            saved.lock().unwrap().push(bytes.to_vec());
            Ok(())
        })
        .unwrap();
        writer.queue(b"initial".to_vec());
        writer.flush().unwrap();
        writer.queue(b"latest".to_vec());
        drop(writer);
        assert_eq!(
            *writes.lock().unwrap(),
            [b"initial".to_vec(), b"latest".to_vec()]
        );
    }
}
