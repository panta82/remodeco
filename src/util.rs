use anyhow::{Context, Result};
use chrono::Local;
use std::path::Path;

/// Timestamp format used for every `createdAt` / `updatedAt` field on disk.
pub fn now() -> String {
    Local::now().to_rfc3339()
}

pub fn path_text(path: &Path) -> Result<String> {
    Ok(path.to_str().context("path is not UTF-8")?.to_owned())
}
