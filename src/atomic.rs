use anyhow::{Context, Result};
use rand::RngCore;
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    write_atomic(path, &bytes, true)
}

pub fn write_text_atomic(path: &Path, text: &str) -> Result<()> {
    write_atomic(path, text.as_bytes(), false)
}

fn write_atomic(path: &Path, bytes: &[u8], sync_parent: bool) -> Result<()> {
    let parent = path.parent().context("atomic target has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    let mut random = [0_u8; 6];
    rand::thread_rng().fill_bytes(&mut random);
    let tmp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("remodeco"),
        hex::encode(random)
    ));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))?;
        if sync_parent {
            OpenOptions::new().read(true).open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}
