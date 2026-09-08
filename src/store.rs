//! On-disk layout and store for sessions and lock files.

use crate::atomic::{write_json_atomic, write_text_atomic};
use crate::config::{PlanFormat, data_dir};
use crate::lock::{HeldLock, require_lock};
use crate::model::{Journal, ManifestFile, SessionId, SessionRecord};
use crate::util::now;
use anyhow::{Context, Result, bail};
use chrono::Local;
use rand::RngCore;
use std::fs;
use std::path::{Path, PathBuf};

pub fn sessions_root() -> Result<PathBuf> {
    Ok(data_dir()?.join("sessions"))
}
pub fn session_dir(session_id: &SessionId) -> Result<PathBuf> {
    Ok(sessions_root()?.join(session_id.as_str()))
}

fn session_lock_path(session_id: &SessionId) -> Result<PathBuf> {
    Ok(session_dir(session_id)?.join("session.lock"))
}
pub fn require_session_lock(session_id: &SessionId) -> Result<HeldLock> {
    require_lock(
        &session_lock_path(session_id)?,
        &format!("session already locked: {session_id}"),
    )
}

fn execute_lock_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("execute.lock"))
}
pub fn require_execute_lock() -> Result<HeldLock> {
    require_lock(
        &execute_lock_path()?,
        "execute.lock held by another remodeco",
    )
}

pub fn make_session_id(root: &Path) -> SessionId {
    let stamp = Local::now().format("%Y%m%d-%H%M%S");
    let base: String = root
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("root")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut random = [0_u8; 2];
    rand::thread_rng().fill_bytes(&mut random);
    SessionId::new(format!(
        "{stamp}-{}-{}",
        if base.is_empty() { "root" } else { &base },
        hex::encode(random)
    ))
    .unwrap()
}

pub fn read_session(id: &SessionId) -> Result<SessionRecord> {
    let path = session_dir(id)?.join("session.json");
    let session: SessionRecord = serde_json::from_slice(
        &fs::read(&path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))?;
    if session.id.as_str() != id.as_str() {
        bail!(
            "{} claims id {}, but sits in directory {id}",
            path.display(),
            session.id
        );
    }
    Ok(session)
}

pub fn write_session(session: &mut SessionRecord) -> Result<()> {
    session.updated_at = now();
    write_json_atomic(&session_dir(&session.id)?.join("session.json"), session)
}

pub fn read_manifest(id: &SessionId) -> Result<ManifestFile> {
    let path = session_dir(id)?.join("manifest.json");
    serde_json::from_slice(&fs::read(&path)?).with_context(|| format!("parse {}", path.display()))
}

pub fn write_manifest(id: &SessionId, manifest: &ManifestFile) -> Result<()> {
    write_json_atomic(&session_dir(id)?.join("manifest.json"), manifest)
}

pub fn write_plan(id: &SessionId, text: &str, file_name: &str) -> Result<PathBuf> {
    let path = session_dir(id)?.join(file_name);
    write_text_atomic(&path, text)?;
    Ok(path)
}

/// Writes the plan under `file_name` and removes any plan left behind by another format.
pub fn replace_plan(id: &SessionId, text: &str, file_name: &str) -> Result<PathBuf> {
    let path = write_plan(id, text, file_name)?;
    for name in PlanFormat::ALL.iter().map(|format| format.file_name()) {
        if name != file_name {
            let extra = session_dir(id)?.join(name);
            if extra.exists() {
                fs::remove_file(&extra).with_context(|| format!("remove {}", extra.display()))?;
            }
        }
    }
    Ok(path)
}

pub fn list_session_ids() -> Result<Vec<SessionId>> {
    let root = sessions_root()?;
    let items = match fs::read_dir(&root) {
        Ok(items) => items,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut ids = Vec::new();
    for item in items {
        let item = item?;
        if item.path().join("session.json").is_file()
            && let Some(id) = item.file_name().to_str()
        {
            let session_id = match SessionId::new(id) {
                Ok(session_id) => session_id,
                Err(_) => {
                    eprintln!(
                        "warning: ignoring invalid session directory {}",
                        item.path().display()
                    );
                    continue;
                }
            };
            ids.push(session_id);
        }
    }
    ids.sort();
    Ok(ids)
}

pub fn list_unfinished(root: Option<&str>) -> Result<Vec<SessionRecord>> {
    let mut sessions = Vec::new();
    for session_id in list_session_ids()? {
        if let Ok(session) = read_session(&session_id)
            && session.status.unfinished()
            && root.is_none_or(|value| value == session.root)
        {
            sessions.push(session);
        }
    }
    sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    Ok(sessions)
}

pub fn journal_path(session_id: &SessionId, journal_id: &str) -> Result<PathBuf> {
    Ok(session_dir(session_id)?
        .join("journals")
        .join(format!("{journal_id}.json")))
}

pub fn write_journal(session_id: &SessionId, journal: &Journal) -> Result<()> {
    write_json_atomic(&journal_path(session_id, &journal.journal_id)?, journal)
}

pub fn read_journal(session_id: &SessionId, journal_id: &str) -> Result<Journal> {
    let path = journal_path(session_id, journal_id)?;
    serde_json::from_slice(&fs::read(&path)?).with_context(|| format!("parse {}", path.display()))
}

pub fn list_journals(session_id: &SessionId) -> Result<Vec<Journal>> {
    let directory = session_dir(session_id)?.join("journals");
    let items = match fs::read_dir(&directory) {
        Ok(items) => items,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut journals = Vec::new();
    for item in items {
        let path = item?.path();
        if path.extension().and_then(|v| v.to_str()) != Some("json") {
            continue;
        }
        if let Ok(journal) = serde_json::from_slice(&fs::read(path)?) {
            journals.push(journal);
        }
    }
    Ok(journals)
}

pub fn delete_session_dir(id: &SessionId) -> Result<bool> {
    let path = session_dir(id)?;
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_dir_all(&path).with_context(|| format!("remove {}", path.display()))?;
    Ok(true)
}
