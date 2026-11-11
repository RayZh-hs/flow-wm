//! The desktop owns its presentation file; the headless host owns runtime state.
use crate::engine::{Snapshot, SnapshotObserver};
use anyhow::{Context, Result, ensure};
use flow_core::migration::PresentationDocument;
use std::{
    fs::{File, OpenOptions},
    io::Write,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::PathBuf,
    sync::{Arc, Mutex},
};

pub struct Writer {
    directory: PathBuf,
    _lock: File,
    previous: Mutex<Option<PresentationDocument>>,
}
impl Writer {
    pub fn new(directory: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&directory)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(directory.join("presentation.lock"))?;
        ensure!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
            "Another desktop is writing presentation in {}",
            directory.display()
        );
        Ok(Self {
            directory,
            _lock: lock,
            previous: Mutex::new(None),
        })
    }
    pub fn observer(self) -> SnapshotObserver {
        let writer = Arc::new(self);
        Arc::new(move |snapshot| writer.save(snapshot))
    }
    fn save(&self, snapshot: &Snapshot) -> Result<()> {
        let document = PresentationDocument::capture(&snapshot.state);
        let mut previous = self.previous.lock().unwrap();
        if previous.as_ref() == Some(&document) {
            return Ok(());
        }
        let temporary = self.directory.join("presentation-v2.json.tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(&document)?)?;
        file.sync_all()?;
        std::fs::rename(temporary, self.directory.join("presentation-v2.json"))
            .context("Save desktop presentation")?;
        File::open(&self.directory)?.sync_all()?;
        *previous = Some(document);
        Ok(())
    }
}
