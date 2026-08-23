use crate::config::AppConfig;
use crate::execute::run_journal;
use crate::model::{
    Journal, JournalFinalStatus, JournalMode, ManifestEntry, ManifestFile, OpKind, Operation,
    PlannedKind, PlannedStep, SessionMode, SessionRecord, SessionStats, SessionStatus,
};
use crate::plan::{
    body_hash, classify_ops, generate_plan, id_width_for_count, parse_plan, validate_plan_header,
    whole_file_hash,
};
use crate::scan::scan_tree;
use crate::schedule::{ScheduleResult, capture_destination_parent, schedule};
use crate::session::{
    HeldLock, active_executed_journal, execute_lock_path, make_session_id, new_journal, now,
    read_journal, read_manifest, read_session, require_lock, session_dir, write_journal,
    write_manifest, write_plan, write_session,
};
use anyhow::{Context, Result, bail};
use rand::RngCore;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct OpenedSession {
    pub session: SessionRecord,
    pub lock: HeldLock,
}

#[derive(Clone, Debug)]
pub struct ExpectedSession {
    pub revision: u64,
    pub mode: SessionMode,
    pub plan_hash: String,
}

pub fn create_session(
    root: &Path,
    mode: SessionMode,
    config: &AppConfig,
    yes: bool,
) -> Result<OpenedSession> {
    let scan = scan_tree(
        root,
        config.recursive,
        config.include_hidden,
        &config.exclude,
    )?;
    if scan.rows.len() >= 25_000 && !yes {
        bail!(
            "scan includes {} files; inspect excludes and rerun with --yes",
            scan.rows.len()
        );
    }
    if scan.rows.len() >= 5_000 {
        eprintln!("warning: {} files", scan.rows.len());
    }
    let root_path = Path::new(&scan.root);
    let id = make_session_id(root_path);
    let entries: Vec<ManifestEntry> = scan
        .rows
        .into_iter()
        .enumerate()
        .map(|(index, row)| ManifestEntry {
            id: index as u64 + 1,
            from: row.from,
            fingerprint: row.fingerprint,
        })
        .collect();
    let id_width = id_width_for_count(entries.len());
    let plan = generate_plan(&id, &scan.root, &entries, id_width);
    let plan_path = write_plan(&id, &plan)?;
    let manifest_path = session_dir(&id)?.join("manifest.json");
    write_manifest(
        &id,
        &ManifestFile {
            session_id: id.clone(),
            root: scan.root.clone(),
            entries: entries.clone(),
        },
    )?;
    let stamp = now();
    let mut session = SessionRecord {
        id: id.clone(),
        root: scan.root,
        mode,
        status: SessionStatus::Draft,
        revision: 1,
        created_at: stamp.clone(),
        updated_at: stamp,
        plan_path: path_text(&plan_path)?,
        manifest_path: path_text(&manifest_path)?,
        generated_whole_file_hash: whole_file_hash(&plan),
        whole_file_hash: whole_file_hash(&plan),
        body_hash: body_hash(&plan),
        plan_body_diverged: false,
        active_journal_id: None,
        id_width,
        tool_version: env!("CARGO_PKG_VERSION").to_owned(),
        stats: SessionStats {
            files: entries
                .iter()
                .filter(|entry| entry.fingerprint.kind == crate::model::SourceKind::File)
                .count(),
            symlinks: entries
                .iter()
                .filter(|entry| entry.fingerprint.kind == crate::model::SourceKind::Symlink)
                .count(),
            skipped_special: scan.skipped_special,
            changes: 0,
        },
    };
    write_session(&mut session)?;
    let lock = require_lock(
        &session_dir(&id)?.join("session.lock"),
        &format!("session locked: {id}"),
    )?;
    Ok(OpenedSession { session, lock })
}

pub fn open_session(id: &str) -> Result<OpenedSession> {
    crate::session::validate_session_id(id)?;
    let session = read_session(id)?;
    let lock = require_lock(
        &session_dir(id)?.join("session.lock"),
        &format!("session locked: {id}"),
    )?;
    Ok(OpenedSession { session, lock })
}

pub fn refresh_hashes(session: &mut SessionRecord) -> Result<bool> {
    let raw = fs::read_to_string(&session.plan_path)
        .with_context(|| format!("read {}", session.plan_path))?;
    let whole = whole_file_hash(&raw);
    let body = body_hash(&raw);
    if whole == session.whole_file_hash && body == session.body_hash {
        return Ok(false);
    }
    let old_body = session.body_hash.clone();
    session.whole_file_hash = whole;
    session.body_hash = body.clone();
    session.revision += 1;
    if matches!(
        session.status,
        SessionStatus::Executed | SessionStatus::Undone
    ) {
        session.plan_body_diverged |= body != old_body;
    }
    write_session(session)?;
    Ok(true)
}

pub fn parse_session_plan(session: &SessionRecord) -> Result<Vec<Operation>> {
    parse_session_plan_at_hash(session, None)
}

fn parse_session_plan_at_hash(
    session: &SessionRecord,
    required_hash: Option<&str>,
) -> Result<Vec<Operation>> {
    let bytes = fs::read(&session.plan_path)?;
    if bytes.len() > crate::plan::MAX_FILE_BYTES {
        bail!("plan too large");
    }
    let raw = String::from_utf8(bytes).context("plan.md is not UTF-8")?;
    if required_hash.is_some_and(|expected| whole_file_hash(&raw) != expected) {
        bail!("session changed since confirmation; reload");
    }
    let parsed = parse_plan(&raw)?;
    validate_plan_header(&parsed, &session.id, &session.root)?;
    if !parsed.unknown_lines.is_empty() {
        bail!(
            "unknown lines: {}",
            parsed
                .unknown_lines
                .iter()
                .map(|(line, _)| line.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let manifest = read_manifest(&session.id)?;
    Ok(classify_ops(&parsed, &manifest.entries, session.mode)?.0)
}

pub fn dry_run(session: &SessionRecord) -> Result<ScheduleResult> {
    let operations = parse_session_plan(session)?;
    let manifest = read_manifest(&session.id)?;
    Ok(schedule(
        session.mode,
        &session.root,
        &session.id,
        &manifest.entries,
        &operations,
    ))
}

pub fn expected(session: &SessionRecord) -> ExpectedSession {
    ExpectedSession {
        revision: session.revision,
        mode: session.mode,
        plan_hash: session.whole_file_hash.clone(),
    }
}

pub fn execute_session(session: &mut SessionRecord, expected: &ExpectedSession) -> Result<Journal> {
    refresh_hashes(session)?;
    if session.revision != expected.revision
        || session.mode != expected.mode
        || session.whole_file_hash != expected.plan_hash
    {
        bail!("session changed since confirmation; reload");
    }
    let _execution_lock = require_lock(
        &execute_lock_path()?,
        "execute.lock held by another remodeco",
    )?;

    let mut journal = if matches!(
        session.status,
        SessionStatus::Executing | SessionStatus::ExecuteInterrupted
    ) {
        let id = session
            .active_journal_id
            .as_deref()
            .context("interrupted session has no journal")?;
        let journal = read_journal(&session.id, id)?;
        if journal.expected_plan_hash != expected.plan_hash
            || journal.expected_revision != expected.revision
            || journal.expected_mode != expected.mode
        {
            bail!("interrupted journal does not match the current plan");
        }
        journal
    } else {
        let operations = parse_session_plan_at_hash(session, Some(&expected.plan_hash))?;
        let manifest = read_manifest(&session.id)?;
        let scheduled = schedule(
            session.mode,
            &session.root,
            &session.id,
            &manifest.entries,
            &operations,
        );
        if !scheduled.errors.is_empty() {
            bail!(scheduled.errors.join("\n"));
        }
        new_journal(
            scheduled.steps,
            JournalMode::Execute,
            None,
            expected.plan_hash.clone(),
            expected.revision,
            expected.mode,
        )
    };
    write_journal(&session.id, &journal)?;
    session.active_journal_id = Some(journal.journal_id.clone());
    session.status = SessionStatus::Executing;
    write_session(session)?;
    run_journal(&session.id, &mut journal)?;
    if journal
        .steps
        .iter()
        .any(|step| step.state == crate::model::StepState::Failed)
    {
        session.status = SessionStatus::ExecuteInterrupted;
    } else {
        session.status = SessionStatus::Executed;
        session.stats.changes = journal
            .steps
            .iter()
            .filter(|step| {
                step.state == crate::model::StepState::Committed
                    && step.planned.op != PlannedKind::Mkdir
            })
            .count();
    }
    write_session(session)?;
    Ok(journal)
}

pub fn undo_session(session: &mut SessionRecord) -> Result<Journal> {
    let active = active_executed_journal(session)?;
    let mut inverse = Vec::new();
    let mut mkdirs = HashMap::<PathBuf, String>::new();
    for source in active
        .steps
        .iter()
        .rev()
        .filter(|step| step.state == crate::model::StepState::Committed)
    {
        let planned = &source.planned;
        match planned.op {
            PlannedKind::Stage | PlannedKind::Commit => {
                let from = planned
                    .to
                    .clone()
                    .context("executed rename has no destination")?;
                let to = planned
                    .from
                    .clone()
                    .context("executed rename has no source")?;
                let parent = capture_destination_parent(Path::new(&to), &mut inverse, &mut mkdirs)?;
                inverse.push(inverse_step(
                    PlannedKind::Commit,
                    Some(from),
                    Some(to),
                    planned.id,
                    source.fingerprint_to.clone(),
                    Some(parent),
                ));
            }
            PlannedKind::Copy => {
                inverse.push(inverse_step(
                    PlannedKind::Trash,
                    planned.to.clone(),
                    None,
                    planned.id,
                    source.fingerprint_to.clone(),
                    None,
                ));
            }
            PlannedKind::Trash => {
                let key = source
                    .trash_restore_key
                    .as_ref()
                    .context("executed trash has no restore key")?;
                let parent = capture_destination_parent(
                    Path::new(&key.original_path),
                    &mut inverse,
                    &mut mkdirs,
                )?;
                inverse.push(inverse_step(
                    PlannedKind::Commit,
                    Some(key.files_path.clone()),
                    Some(key.original_path.clone()),
                    planned.id,
                    source.fingerprint_to.clone(),
                    Some(parent),
                ));
            }
            PlannedKind::Mkdir | PlannedKind::Noop => {}
        }
    }
    let _execution_lock = require_lock(
        &execute_lock_path()?,
        "execute.lock held by another remodeco",
    )?;
    let mut journal = new_journal(
        inverse,
        JournalMode::Undo,
        Some(active.journal_id),
        session.whole_file_hash.clone(),
        session.revision,
        session.mode,
    );
    write_journal(&session.id, &journal)?;
    session.active_journal_id = Some(journal.journal_id.clone());
    session.status = SessionStatus::Undoing;
    write_session(session)?;
    run_journal(&session.id, &mut journal)?;
    session.status = if journal.final_status == Some(JournalFinalStatus::Undone) {
        for step in &active.steps {
            if step.planned.op == PlannedKind::Trash {
                if let Some(info_path) = step
                    .trash_restore_key
                    .as_ref()
                    .and_then(|key| key.info_path.as_deref())
                {
                    match fs::remove_file(info_path) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => {
                            return Err(error)
                                .with_context(|| format!("remove trash metadata {info_path}"));
                        }
                    }
                }
            }
        }
        SessionStatus::Undone
    } else {
        SessionStatus::UndoInterrupted
    };
    write_session(session)?;
    Ok(journal)
}

fn inverse_step(
    op: PlannedKind,
    from: Option<String>,
    to: Option<String>,
    id: Option<u64>,
    fingerprint: Option<crate::model::SourceFingerprint>,
    parent: Option<crate::model::DestParentRef>,
) -> PlannedStep {
    let mut random = [0_u8; 6];
    rand::thread_rng().fill_bytes(&mut random);
    PlannedStep {
        step_id: format!("{}-u", hex::encode(random)),
        op,
        from,
        to,
        id,
        fingerprint_from: fingerprint,
        dest_parent_ref: parent,
        mkdir_path: None,
    }
}

fn path_text(path: &Path) -> Result<String> {
    Ok(path.to_str().context("path is not UTF-8")?.to_owned())
}

pub fn operation_counts(operations: &[Operation]) -> (usize, usize) {
    let changes = operations
        .iter()
        .filter(|op| !matches!(op.kind, OpKind::Noop | OpKind::Skip))
        .count();
    let trash = operations
        .iter()
        .filter(|op| op.kind == OpKind::Trash)
        .count();
    (changes, trash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EditorMode;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvRestore {
        data: Option<std::ffi::OsString>,
        config: Option<std::ffi::OsString>,
        xdg_data: Option<std::ffi::OsString>,
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            unsafe {
                match &self.data {
                    Some(value) => std::env::set_var("REMODECO_DATA_DIR", value),
                    None => std::env::remove_var("REMODECO_DATA_DIR"),
                }
                match &self.config {
                    Some(value) => std::env::set_var("REMODECO_CONFIG_DIR", value),
                    None => std::env::remove_var("REMODECO_CONFIG_DIR"),
                }
                match &self.xdg_data {
                    Some(value) => std::env::set_var("XDG_DATA_HOME", value),
                    None => std::env::remove_var("XDG_DATA_HOME"),
                }
            }
        }
    }

    fn isolated_environment(temp: &tempfile::TempDir) -> EnvRestore {
        let restore = EnvRestore {
            data: std::env::var_os("REMODECO_DATA_DIR"),
            config: std::env::var_os("REMODECO_CONFIG_DIR"),
            xdg_data: std::env::var_os("XDG_DATA_HOME"),
        };
        unsafe {
            std::env::set_var("REMODECO_DATA_DIR", temp.path().join("state"));
            std::env::set_var("REMODECO_CONFIG_DIR", temp.path().join("config"));
            std::env::set_var("XDG_DATA_HOME", temp.path().join("xdg"));
        }
        restore
    }

    fn config() -> AppConfig {
        AppConfig {
            editor: None,
            editor_mode: EditorMode::Auto,
            exclude: Vec::new(),
            include_hidden: false,
            recursive: true,
            open_editor: false,
        }
    }

    #[test]
    fn executes_and_undoes_a_rename_cycle() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore = isolated_environment(&temp);
        let root = temp.path().join("files");
        fs::create_dir(&root).unwrap();
        let a = root.join("a.txt");
        let b = root.join("b.txt");
        fs::write(&a, "A").unwrap();
        fs::write(&b, "B").unwrap();

        let mut opened = create_session(&root, SessionMode::Move, &config(), true).unwrap();
        let raw = fs::read_to_string(&opened.session.plan_path).unwrap();
        let a_text = path_text(&a).unwrap();
        let b_text = path_text(&b).unwrap();
        let sentinel = root.join(".swap-sentinel").to_string_lossy().into_owned();
        let edited = raw
            .replace(&format!("\t{a_text}\n"), &format!("\t{sentinel}\n"))
            .replace(&format!("\t{b_text}\n"), &format!("\t{a_text}\n"))
            .replace(&format!("\t{sentinel}\n"), &format!("\t{b_text}\n"));
        fs::write(&opened.session.plan_path, edited).unwrap();
        refresh_hashes(&mut opened.session).unwrap();

        let snapshot = expected(&opened.session);
        let journal = execute_session(&mut opened.session, &snapshot).unwrap();
        assert_eq!(journal.final_status, Some(JournalFinalStatus::Executed));
        assert_eq!(fs::read_to_string(&a).unwrap(), "B");
        assert_eq!(fs::read_to_string(&b).unwrap(), "A");

        let undo = undo_session(&mut opened.session).unwrap();
        assert_eq!(undo.final_status, Some(JournalFinalStatus::Undone));
        assert_eq!(fs::read_to_string(&a).unwrap(), "A");
        assert_eq!(fs::read_to_string(&b).unwrap(), "B");
    }

    #[test]
    fn trashes_and_restores_with_metadata_cleanup() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore = isolated_environment(&temp);
        let root = temp.path().join("files");
        fs::create_dir(&root).unwrap();
        let source = root.join("song.mp3");
        fs::write(&source, "music").unwrap();

        let mut opened = create_session(&root, SessionMode::Move, &config(), true).unwrap();
        let source_text = path_text(&source).unwrap();
        let raw = fs::read_to_string(&opened.session.plan_path).unwrap();
        let edited = raw.replace(&format!("\t{source_text}\n"), "\t\n");
        fs::write(&opened.session.plan_path, edited).unwrap();
        refresh_hashes(&mut opened.session).unwrap();

        let snapshot = expected(&opened.session);
        let journal = execute_session(&mut opened.session, &snapshot).unwrap();
        assert_eq!(journal.final_status, Some(JournalFinalStatus::Executed));
        assert!(!source.exists());
        let key = journal
            .steps
            .iter()
            .find_map(|step| step.trash_restore_key.as_ref())
            .unwrap();
        assert!(Path::new(&key.files_path).exists());
        assert!(Path::new(key.info_path.as_ref().unwrap()).exists());

        let undo = undo_session(&mut opened.session).unwrap();
        assert_eq!(undo.final_status, Some(JournalFinalStatus::Undone));
        assert_eq!(fs::read_to_string(&source).unwrap(), "music");
        assert!(!Path::new(key.info_path.as_ref().unwrap()).exists());
    }

    #[test]
    fn refuses_a_plan_changed_after_confirmation() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore = isolated_environment(&temp);
        let root = temp.path().join("files");
        fs::create_dir(&root).unwrap();
        let source = root.join("a.txt");
        fs::write(&source, "A").unwrap();
        let mut opened = create_session(&root, SessionMode::Move, &config(), true).unwrap();
        let confirmed = expected(&opened.session);

        let raw = fs::read_to_string(&opened.session.plan_path).unwrap();
        let destination = root.join("b.txt").to_string_lossy().into_owned();
        let edited = raw.replace(
            &format!("\t{}\n", path_text(&source).unwrap()),
            &format!("\t{destination}\n"),
        );
        fs::write(&opened.session.plan_path, edited).unwrap();

        let error = execute_session(&mut opened.session, &confirmed).unwrap_err();
        assert!(error.to_string().contains("changed since confirmation"));
        assert!(source.exists());
        assert!(!Path::new(&destination).exists());
    }
}
