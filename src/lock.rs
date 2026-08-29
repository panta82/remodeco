use crate::util::now;
use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// An exclusive advisory lock, released when dropped.
pub struct HeldLock {
    file: File,
    pub path: PathBuf,
}

impl Drop for HeldLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Takes the lock, or returns `None` if another process holds it.
pub fn try_lock(path: &Path) -> Result<Option<HeldLock>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    match file.try_lock_exclusive() {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("lock {}", path.display())),
    }
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    writeln!(
        file,
        "{{\"pid\":{},\"startedAt\":{:?}}}",
        std::process::id(),
        now()
    )?;
    file.sync_all()?;
    Ok(Some(HeldLock {
        file,
        path: path.to_path_buf(),
    }))
}

/// Takes the lock, failing with `message` if another process holds it.
pub fn require_lock(path: &Path, message: &str) -> Result<HeldLock> {
    try_lock(path)?.ok_or_else(|| anyhow::anyhow!(message.to_owned()))
}
