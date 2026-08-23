use crate::atomic::{write_json_atomic, write_text_atomic};
use crate::config::data_dir;
use crate::model::{
    Journal, JournalFinalStatus, JournalMode, JournalStep, ManifestFile, PlannedStep, SessionMode,
    SessionRecord, StepState,
};
use anyhow::{Context, Result, bail};
use chrono::Local;
use fs2::FileExt;
use rand::RngCore;
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub struct HeldLock {
    file: File,
    pub path: PathBuf,
}

impl Drop for HeldLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

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

pub fn sessions_root() -> Result<PathBuf> {
    Ok(data_dir()?.join("sessions"))
}
pub fn session_dir(id: &str) -> Result<PathBuf> {
    Ok(sessions_root()?.join(id))
}
pub fn execute_lock_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("execute.lock"))
}

pub fn make_session_id(root: &Path) -> String {
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
    format!(
        "{stamp}-{}-{}",
        if base.is_empty() { "root" } else { &base },
        hex::encode(random)
    )
}

pub fn now() -> String {
    Local::now().to_rfc3339()
}

pub fn read_session(id: &str) -> Result<SessionRecord> {
    let path = session_dir(id)?.join("session.json");
    serde_json::from_slice(&fs::read(&path).with_context(|| format!("read {}", path.display()))?)
        .with_context(|| format!("parse {}", path.display()))
}

pub fn write_session(session: &mut SessionRecord) -> Result<()> {
    session.updated_at = now();
    write_json_atomic(&session_dir(&session.id)?.join("session.json"), session)
}

pub fn read_manifest(id: &str) -> Result<ManifestFile> {
    let path = session_dir(id)?.join("manifest.json");
    serde_json::from_slice(&fs::read(&path)?).with_context(|| format!("parse {}", path.display()))
}

pub fn write_manifest(id: &str, manifest: &ManifestFile) -> Result<()> {
    write_json_atomic(&session_dir(id)?.join("manifest.json"), manifest)
}

pub fn write_plan(id: &str, text: &str) -> Result<PathBuf> {
    let path = session_dir(id)?.join("plan.md");
    write_text_atomic(&path, text)?;
    Ok(path)
}

pub fn list_session_ids() -> Result<Vec<String>> {
    let root = sessions_root()?;
    let items = match fs::read_dir(&root) {
        Ok(items) => items,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut ids = Vec::new();
    for item in items {
        let item = item?;
        if item.path().join("session.json").is_file() {
            if let Some(id) = item.file_name().to_str() {
                ids.push(id.to_owned());
            }
        }
    }
    ids.sort();
    Ok(ids)
}

pub fn list_unfinished(root: Option<&str>) -> Result<Vec<SessionRecord>> {
    let mut sessions = Vec::new();
    for id in list_session_ids()? {
        if let Ok(session) = read_session(&id) {
            if session.status.unfinished() && root.is_none_or(|value| value == session.root) {
                sessions.push(session);
            }
        }
    }
    sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    Ok(sessions)
}

pub fn new_journal(
    steps: Vec<PlannedStep>,
    mode: JournalMode,
    parent: Option<String>,
    expected_plan_hash: String,
    expected_revision: u64,
    expected_mode: SessionMode,
) -> Journal {
    let mut random = [0_u8; 8];
    rand::thread_rng().fill_bytes(&mut random);
    Journal {
        journal_id: hex::encode(random),
        mode,
        parent_journal_id: parent,
        final_status: None,
        superseded_by_journal_id: None,
        expected_plan_hash,
        expected_revision,
        expected_mode,
        steps: steps
            .into_iter()
            .map(|planned| JournalStep {
                planned,
                state: StepState::Pending,
                error: None,
                created_by_us: None,
                created_dir_fingerprint: None,
                fingerprint_to: None,
                trash_restore_key: None,
            })
            .collect(),
    }
}

pub fn journal_path(session_id: &str, journal_id: &str) -> Result<PathBuf> {
    Ok(session_dir(session_id)?
        .join("journals")
        .join(format!("{journal_id}.json")))
}

pub fn write_journal(session_id: &str, journal: &Journal) -> Result<()> {
    write_json_atomic(&journal_path(session_id, &journal.journal_id)?, journal)
}

pub fn read_journal(session_id: &str, journal_id: &str) -> Result<Journal> {
    let path = journal_path(session_id, journal_id)?;
    serde_json::from_slice(&fs::read(&path)?).with_context(|| format!("parse {}", path.display()))
}

pub fn list_journals(session_id: &str) -> Result<Vec<Journal>> {
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

pub fn active_executed_journal(session: &SessionRecord) -> Result<Journal> {
    let journals = list_journals(&session.id)?;
    let journal = session
        .active_journal_id
        .as_deref()
        .and_then(|id| journals.iter().find(|journal| journal.journal_id == id))
        .or_else(|| {
            journals.iter().find(|journal| {
                journal.mode == JournalMode::Execute
                    && journal.final_status == Some(JournalFinalStatus::Executed)
            })
        });
    journal.cloned().context("no executed journal to undo")
}

pub fn require_lock(path: &Path, message: &str) -> Result<HeldLock> {
    try_lock(path)?.ok_or_else(|| anyhow::anyhow!(message.to_owned()))
}

pub fn validate_session_id(id: &str) -> Result<()> {
    if id.is_empty() || id.contains('/') || id.contains('\\') || id == "." || id == ".." {
        bail!("invalid session id");
    }
    Ok(())
}
