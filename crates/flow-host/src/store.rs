//! The host is the sole writer of the native runtime document.
use anyhow::{Context, Result, ensure};
use flow_core::{domain::RuntimeState, providers::Storage};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
pub const FORMAT_VERSION: u32 = 3;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    runtime: RuntimeState,
}
#[derive(Clone)]
pub struct Store {
    directory: PathBuf,
    _lock: Arc<File>,
}
impl Store {
    pub fn new(directory: PathBuf) -> Result<Self> {
        fs::create_dir_all(&directory)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(directory.join("runtime.lock"))?;
        ensure!(
            unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
            "Runtime data is already in use: {}",
            directory.display()
        );
        Ok(Self {
            directory,
            _lock: Arc::new(lock),
        })
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn load(&self) -> Result<RuntimeState> {
        let path = self.directory.join("runtime.json");
        if !path.exists() {
            ensure!(
                !["state.json", "runtime-v2.json"]
                    .iter()
                    .any(|name| self.directory.join(name).exists()),
                "Unsupported saved runtime in {}. Preserve it and use a matching data backup or an explicit offline conversion before starting the current host.",
                self.directory.display()
            );
            return Ok(RuntimeState::local());
        }
        let document: Document = serde_json::from_slice(&fs::read(&path)?)
            .context("Read native runtime; input has been preserved")?;
        ensure!(
            document.version == FORMAT_VERSION,
            "Unsupported runtime format {}; expected {}. Input has been preserved.",
            document.version,
            FORMAT_VERSION
        );
        document.runtime.validate()?;
        Ok(document.runtime)
    }
}
impl Storage for Store {
    fn save(&self, state: &RuntimeState) -> Result<()> {
        state.validate()?;
        let bytes = serde_json::to_vec_pretty(&Document {
            version: FORMAT_VERSION,
            runtime: state.clone(),
        })?;
        let temporary = self.directory.join("runtime.json.tmp");
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(temporary, self.directory.join("runtime.json"))?;
        File::open(&self.directory)?.sync_all()?;
        Ok(())
    }
}
