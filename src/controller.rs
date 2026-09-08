use crate::config::{AppConfig, PlanFormat};
use crate::execute::{ExecuteUi, run_journal};
use crate::lock::HeldLock;
use crate::model::{
    Journal, JournalFinalStatus, JournalMode, ManifestEntry, ManifestFile, OpKind, Operation,
    PlannedKind, PlannedStep, SessionId, SessionMode, SessionRecord, SessionStats, SessionStatus,
};
use crate::plan::{
    body_hash, classify_ops, generate_plan, id_width_for_count, parse_plan, render_plan,
    validate_plan_header, whole_file_hash,
};
use crate::scan::scan_tree;
use crate::schedule::{ScheduleResult, capture_destination_parent, schedule};
use crate::store::{
    delete_session_dir, list_journals, make_session_id, read_journal, read_manifest, read_session,
    replace_plan, require_execute_lock, require_session_lock, session_dir, write_journal,
    write_manifest, write_plan, write_session,
};
use crate::util::{now, path_text};
use anyhow::{Context, Result, bail};
use rand::RngCore;
use std::collections::{HashMap, HashSet};
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
    let plan_path = write_plan(&id, &plan, config.plan_format.file_name())?;
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
    let lock = require_session_lock(&id)?;
    Ok(OpenedSession { session, lock })
}

pub fn open_session(id: &SessionId) -> Result<OpenedSession> {
    // Check first: taking the lock would create the directory we are about to report on.
    if !session_dir(id)?.exists() {
        bail!("no such session: {id}");
    }
    let lock = require_session_lock(id)?;
    let session = read_session(id)?;
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

pub fn reset_session_plan(session: &mut SessionRecord, format: PlanFormat) -> Result<()> {
    if session.active_journal_id.is_some() {
        bail!("cannot reset a session with an active journal");
    }
    if session.status != SessionStatus::Draft {
        bail!("can only reset a draft session");
    }
    let manifest = read_manifest(&session.id)?;
    let plan = generate_plan(
        &session.id,
        &session.root,
        &manifest.entries,
        session.id_width,
    );
    apply_plan(session, &plan, format)?;
    session.generated_whole_file_hash = session.whole_file_hash.clone();
    session.plan_body_diverged = false;
    session.stats.changes = 0;
    Ok(())
}

/// Move a draft session onto the preferred plan filename, preserving edited destinations.
pub fn normalize_session_plan(session: &mut SessionRecord, format: PlanFormat) -> Result<bool> {
    if session.status != SessionStatus::Draft {
        return Ok(false);
    }
    let preferred = format.file_name();
    let current_name = Path::new(&session.plan_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let raw = match fs::read_to_string(&session.plan_path) {
        Ok(raw) => raw,
        Err(_) => return Ok(false),
    };
    let parsed = match parse_plan(&raw) {
        Ok(parsed) => parsed,
        Err(_) => return Ok(false),
    };
    if parsed.unknown_lines.is_empty() && current_name == preferred {
        return Ok(false);
    }
    if parsed.unknown_lines.is_empty() {
        if let Err(error) = validate_plan_header(&parsed, &session.id, &session.root) {
            eprintln!("warning: leaving plan in place ({error})");
            return Ok(false);
        }
        let destinations: HashMap<u64, String> = parsed
            .bullets
            .iter()
            .map(|bullet| (bullet.id, bullet.destination.clone()))
            .collect();
        let manifest = read_manifest(&session.id)?;
        let plan = render_plan(
            &session.id,
            &session.root,
            &manifest.entries,
            session.id_width,
            Some(&destinations),
        );
        apply_plan(session, &plan, format)?;
        return Ok(true);
    }
    Ok(false)
}

fn apply_plan(session: &mut SessionRecord, plan: &str, format: PlanFormat) -> Result<()> {
    let plan_path = replace_plan(&session.id, plan, format.file_name())?;
    session.plan_path = path_text(&plan_path)?;
    session.whole_file_hash = whole_file_hash(plan);
    session.body_hash = body_hash(plan);
    session.revision += 1;
    write_session(session)?;
    Ok(())
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
    let raw = String::from_utf8(bytes).context("plan is not UTF-8")?;
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

pub fn execute_session(
    session: &mut SessionRecord,
    expected: &ExpectedSession,
    ui: &mut dyn ExecuteUi,
) -> Result<Journal> {
    refresh_hashes(session)?;
    if !session.status.executable() {
        bail!("cannot execute a session with status {}", session.status);
    }
    if session.revision != expected.revision
        || session.mode != expected.mode
        || session.whole_file_hash != expected.plan_hash
    {
        bail!("session changed since confirmation; reload");
    }
    let _execution_lock = require_execute_lock()?;

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
        Journal::new(
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
    run_journal(&session.id, &mut journal, &session.root, ui)?;
    if journal.final_status == Some(JournalFinalStatus::Executed) {
        session.status = SessionStatus::Executed;
        session.stats.changes = journal
            .steps
            .iter()
            .filter(|step| {
                step.state == crate::model::StepState::Committed
                    && step.planned.op != PlannedKind::Mkdir
            })
            .count();
    } else {
        session.status = SessionStatus::ExecuteInterrupted;
    }
    write_session(session)?;
    Ok(journal)
}

pub fn execute_prepared_session(
    session: &mut SessionRecord,
    ui: &mut dyn ExecuteUi,
) -> Result<Journal> {
    refresh_hashes(session)?;
    let snapshot = expected(session);
    execute_session(session, &snapshot, ui)
}

/// The journal `undo` should reverse: the session's active one if it still needs
/// undoing, else any other execute journal that ran and has not been undone.
///
/// `active_journal_id` tracks the last journal we touched, which after an undo is the
/// undo journal itself, so both branches must check the mode. Journals carry no
/// timestamp, so when several qualify the choice among them is arbitrary.
fn active_executed_journal(session: &SessionRecord) -> Result<Journal> {
    let journals = list_journals(&session.id)?;
    let undone: HashSet<&str> = journals
        .iter()
        .filter(|journal| {
            journal.mode == JournalMode::Undo
                && journal.final_status == Some(JournalFinalStatus::Undone)
        })
        .filter_map(|journal| journal.parent_journal_id.as_deref())
        .collect();
    let undoable = |journal: &Journal| {
        journal.mode == JournalMode::Execute
            && journal.final_status == Some(JournalFinalStatus::Executed)
            && !undone.contains(journal.journal_id.as_str())
    };
    session
        .active_journal_id
        .as_deref()
        .and_then(|id| journals.iter().find(|journal| journal.journal_id == id))
        .filter(|journal| undoable(journal))
        .or_else(|| journals.iter().find(|journal| undoable(journal)))
        .cloned()
        .context("no executed journal to undo")
}

pub fn undo_session(session: &mut SessionRecord, ui: &mut dyn ExecuteUi) -> Result<Journal> {
    if matches!(
        session.status,
        SessionStatus::Undoing | SessionStatus::UndoInterrupted
    ) {
        bail!(
            "session {} has an interrupted undo (status {}); resuming an undo is not supported",
            session.id,
            session.status
        );
    }
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
                let Some(key) = source.trash_restore_key.as_ref() else {
                    continue;
                };
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
    let _execution_lock = require_execute_lock()?;
    let mut journal = Journal::new(
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
    run_journal(&session.id, &mut journal, &session.root, ui)?;
    session.status = if journal.final_status == Some(JournalFinalStatus::Undone) {
        for step in &active.steps {
            if step.planned.op == PlannedKind::Trash
                && let Some(info_path) = step
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

/// Removes a session and everything under it. This includes `journals/`, so deleting an
/// executed session gives up the ability to undo it; `force` is required in that case.
pub fn delete_session(id: &SessionId, force: bool) -> Result<()> {
    // Check first: taking the lock would create the directory we are about to report on.
    if !session_dir(id)?.exists() {
        bail!("no such session: {id}");
    }
    let _session_lock = require_session_lock(id)?;
    if !force && active_executed_journal(&read_session(id)?).is_ok() {
        bail!("session {id} can still be undone; run `remodeco undo {id}` first, or pass --force");
    }
    if !delete_session_dir(id)? {
        bail!("no such session: {id}");
    }
    Ok(())
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
    use crate::config::{EditorMode, PlanFormat};
    use std::os::unix::fs::PermissionsExt;
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
            plan_format: PlanFormat::Properties,
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
            .replace(&format!("\t:\t{a_text}\n"), &format!("\t:\t{sentinel}\n"))
            .replace(&format!("\t:\t{b_text}\n"), &format!("\t:\t{a_text}\n"))
            .replace(&format!("\t:\t{sentinel}\n"), &format!("\t:\t{b_text}\n"));
        fs::write(&opened.session.plan_path, edited).unwrap();
        refresh_hashes(&mut opened.session).unwrap();

        let snapshot = expected(&opened.session);
        let mut ui = crate::execute::QuietExecuteUi;
        let journal = execute_session(&mut opened.session, &snapshot, &mut ui).unwrap();
        assert_eq!(journal.final_status, Some(JournalFinalStatus::Executed));
        assert_eq!(fs::read_to_string(&a).unwrap(), "B");
        assert_eq!(fs::read_to_string(&b).unwrap(), "A");

        let undo = undo_session(&mut opened.session, &mut ui).unwrap();
        assert_eq!(undo.final_status, Some(JournalFinalStatus::Undone));
        assert_eq!(fs::read_to_string(&a).unwrap(), "A");
        assert_eq!(fs::read_to_string(&b).unwrap(), "B");

        // Undoing again must not invert the undo journal and re-apply the swap.
        let error = undo_session(&mut opened.session, &mut ui).unwrap_err();
        assert_eq!(error.to_string(), "no executed journal to undo");
        assert_eq!(fs::read_to_string(&a).unwrap(), "A");
        assert_eq!(fs::read_to_string(&b).unwrap(), "B");
    }

    #[test]
    fn delete_rejects_unknown_ids_without_creating_them() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore = isolated_environment(&temp);

        let id = SessionId::new("no-such-session").unwrap();
        let error = delete_session(&id, false).unwrap_err();
        assert_eq!(error.to_string(), "no such session: no-such-session");
        assert!(!session_dir(&id).unwrap().exists());
    }

    #[test]
    fn delete_keeps_an_undoable_session_unless_forced() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore = isolated_environment(&temp);
        let root = temp.path().join("files");
        fs::create_dir(&root).unwrap();
        let source = root.join("a.txt");
        let destination = root.join("b.txt");
        fs::write(&source, "A").unwrap();

        let mut opened = create_session(&root, SessionMode::Move, &config(), true).unwrap();
        let raw = fs::read_to_string(&opened.session.plan_path).unwrap();
        fs::write(
            &opened.session.plan_path,
            raw.replace(
                &format!("\t:\t{}\n", path_text(&source).unwrap()),
                &format!("\t:\t{}\n", path_text(&destination).unwrap()),
            ),
        )
        .unwrap();

        let mut ui = crate::execute::QuietExecuteUi;
        execute_prepared_session(&mut opened.session, &mut ui).unwrap();
        let id = opened.session.id.clone();
        drop(opened);

        let error = delete_session(&id, false).unwrap_err();
        assert!(
            error.to_string().contains("can still be undone"),
            "unexpected error: {error}"
        );
        assert!(session_dir(&id).unwrap().exists());

        delete_session(&id, true).unwrap();
        assert!(!session_dir(&id).unwrap().exists());
    }

    #[test]
    fn executes_a_prepared_session_and_rejects_replay() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore = isolated_environment(&temp);
        let root = temp.path().join("files");
        fs::create_dir(&root).unwrap();
        let source = root.join("a.txt");
        let destination = root.join("b.txt");
        fs::write(&source, "A").unwrap();

        let mut opened = create_session(&root, SessionMode::Move, &config(), true).unwrap();
        let raw = fs::read_to_string(&opened.session.plan_path).unwrap();
        let edited = raw.replace(
            &format!("\t:\t{}\n", path_text(&source).unwrap()),
            &format!("\t:\t{}\n", path_text(&destination).unwrap()),
        );
        fs::write(&opened.session.plan_path, edited).unwrap();

        let mut ui = crate::execute::QuietExecuteUi;
        let journal = execute_prepared_session(&mut opened.session, &mut ui).unwrap();
        assert_eq!(journal.final_status, Some(JournalFinalStatus::Executed));
        assert_eq!(fs::read_to_string(&destination).unwrap(), "A");

        let error = execute_prepared_session(&mut opened.session, &mut ui).unwrap_err();
        assert_eq!(
            error.to_string(),
            "cannot execute a session with status executed"
        );
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
        let edited = raw.replace(&format!("\t:\t{source_text}\n"), "\t:\t\n");
        fs::write(&opened.session.plan_path, edited).unwrap();
        refresh_hashes(&mut opened.session).unwrap();

        let snapshot = expected(&opened.session);
        let mut ui = crate::execute::QuietExecuteUi;
        let journal = execute_session(&mut opened.session, &snapshot, &mut ui).unwrap();
        assert_eq!(journal.final_status, Some(JournalFinalStatus::Executed));
        assert!(!source.exists());
        let key = journal
            .steps
            .iter()
            .find_map(|step| step.trash_restore_key.as_ref())
            .unwrap();
        assert!(Path::new(&key.files_path).exists());
        assert!(Path::new(key.info_path.as_ref().unwrap()).exists());

        let undo = undo_session(&mut opened.session, &mut ui).unwrap();
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
            &format!("\t:\t{}\n", path_text(&source).unwrap()),
            &format!("\t:\t{destination}\n"),
        );
        fs::write(&opened.session.plan_path, edited).unwrap();

        let mut ui = crate::execute::QuietExecuteUi;
        let error = execute_session(&mut opened.session, &confirmed, &mut ui).unwrap_err();
        assert!(error.to_string().contains("changed since confirmation"));
        assert!(source.exists());
        assert!(!Path::new(&destination).exists());
    }

    #[test]
    fn reset_restores_original_plan_destinations() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore = isolated_environment(&temp);
        let root = temp.path().join("files");
        fs::create_dir(&root).unwrap();
        let source = root.join("a.txt");
        fs::write(&source, "A").unwrap();
        let mut opened = create_session(&root, SessionMode::Move, &config(), true).unwrap();
        let original = fs::read_to_string(&opened.session.plan_path).unwrap();
        let source_text = path_text(&source).unwrap();
        let edited = original.replace(
            &format!("\t:\t{source_text}\n"),
            &format!("\t:\t{}\n", root.join("b.txt").display()),
        );
        fs::write(&opened.session.plan_path, edited).unwrap();
        refresh_hashes(&mut opened.session).unwrap();
        assert!(
            parse_session_plan(&opened.session)
                .unwrap()
                .iter()
                .any(|operation| operation.kind == OpKind::Move)
        );

        reset_session_plan(&mut opened.session, PlanFormat::Properties).unwrap();
        let operations = parse_session_plan(&opened.session).unwrap();
        assert!(
            operations
                .iter()
                .all(|operation| operation.kind == OpKind::Noop)
        );
        assert_eq!(
            fs::read_to_string(&opened.session.plan_path).unwrap(),
            generate_plan(
                &opened.session.id,
                &opened.session.root,
                &read_manifest(&opened.session.id).unwrap().entries,
                opened.session.id_width,
            )
        );

        opened.session.status = SessionStatus::Executed;
        assert!(reset_session_plan(&mut opened.session, PlanFormat::Properties).is_err());
    }

    #[test]
    fn writes_plan_properties_by_default_and_honors_format() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore = isolated_environment(&temp);
        let root = temp.path().join("files");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("a.txt"), "A").unwrap();

        let properties = create_session(&root, SessionMode::Move, &config(), true).unwrap();
        assert!(properties.session.plan_path.ends_with("plan.properties"));
        let raw = fs::read_to_string(&properties.session.plan_path).unwrap();
        assert!(raw.contains("F1\t:\t"));
        drop(properties);

        let mut yaml = config();
        yaml.plan_format = PlanFormat::Yaml;
        let yaml_session = create_session(&root, SessionMode::Move, &yaml, true).unwrap();
        assert!(yaml_session.session.plan_path.ends_with("plan.yaml"));
        drop(yaml_session);

        let mut plain = config();
        plain.plan_format = PlanFormat::PlainText;
        let txt = create_session(&root, SessionMode::Move, &plain, true).unwrap();
        assert!(txt.session.plan_path.ends_with("plan.txt"));
    }

    fn plan_two_renames(root: &Path) -> (OpenedSession, PathBuf, PathBuf, PathBuf, PathBuf) {
        fs::create_dir(root).unwrap();
        let a = root.join("a.txt");
        let b = root.join("b.txt");
        let a_to = root.join("a-new.txt");
        let b_to = root.join("b-new.txt");
        fs::write(&a, "A").unwrap();
        fs::write(&b, "B").unwrap();
        let mut opened = create_session(root, SessionMode::Move, &config(), true).unwrap();
        let raw = fs::read_to_string(&opened.session.plan_path).unwrap();
        let edited = raw
            .replace(
                &format!("\t:\t{}\n", path_text(&a).unwrap()),
                &format!("\t:\t{}\n", path_text(&a_to).unwrap()),
            )
            .replace(
                &format!("\t:\t{}\n", path_text(&b).unwrap()),
                &format!("\t:\t{}\n", path_text(&b_to).unwrap()),
            );
        fs::write(&opened.session.plan_path, edited).unwrap();
        refresh_hashes(&mut opened.session).unwrap();
        (opened, a, b, a_to, b_to)
    }

    #[test]
    fn skip_leaves_changed_file_and_continues() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore = isolated_environment(&temp);
        let root = temp.path().join("files");
        let (mut opened, a, b, a_to, b_to) = plan_two_renames(&root);
        fs::write(&b, "B-changed").unwrap();

        let snapshot = expected(&opened.session);
        let mut ui = crate::execute::ScriptedExecuteUi {
            choice: crate::execute::MismatchAction::Skip,
            ..Default::default()
        };
        let journal = execute_session(&mut opened.session, &snapshot, &mut ui).unwrap();
        assert_eq!(journal.final_status, Some(JournalFinalStatus::Executed));
        assert!(a_to.exists());
        assert!(!a.exists());
        assert!(b.exists());
        assert!(!b_to.exists());
        assert!(
            journal
                .steps
                .iter()
                .any(|step| step.state == crate::model::StepState::Skipped)
        );
        assert!(ui.commands.iter().any(|line| line.starts_with("mv ")));
    }

    #[test]
    fn abort_stops_before_the_changed_file() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore = isolated_environment(&temp);
        let root = temp.path().join("files");
        let (mut opened, a, b, a_to, b_to) = plan_two_renames(&root);
        fs::write(&a, "A-changed").unwrap();

        let snapshot = expected(&opened.session);
        let mut ui = crate::execute::ScriptedExecuteUi {
            choice: crate::execute::MismatchAction::Abort,
            ..Default::default()
        };
        let journal = execute_session(&mut opened.session, &snapshot, &mut ui).unwrap();
        assert_ne!(journal.final_status, Some(JournalFinalStatus::Executed));
        assert_eq!(opened.session.status, SessionStatus::ExecuteInterrupted);
        assert!(a.exists());
        assert!(b.exists());
        assert!(!a_to.exists());
        assert!(!b_to.exists());
        assert!(
            journal
                .steps
                .iter()
                .any(|step| step.error.as_deref() == Some("aborted"))
        );
    }

    #[test]
    fn proceed_renames_the_changed_file() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore = isolated_environment(&temp);
        let root = temp.path().join("files");
        let (mut opened, a, b, a_to, b_to) = plan_two_renames(&root);
        fs::write(&b, "B-changed").unwrap();

        let snapshot = expected(&opened.session);
        let mut ui = crate::execute::ScriptedExecuteUi {
            choice: crate::execute::MismatchAction::Proceed,
            ..Default::default()
        };
        let journal = execute_session(&mut opened.session, &snapshot, &mut ui).unwrap();
        assert_eq!(journal.final_status, Some(JournalFinalStatus::Executed));
        assert!(!a.exists());
        assert!(!b.exists());
        assert_eq!(fs::read_to_string(&a_to).unwrap(), "A");
        assert_eq!(fs::read_to_string(&b_to).unwrap(), "B-changed");
        assert_eq!(ui.commands.len(), 2);
    }

    #[test]
    fn all_proceeds_remaining_changed_files() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore = isolated_environment(&temp);
        let root = temp.path().join("files");
        let (mut opened, a, b, a_to, b_to) = plan_two_renames(&root);
        fs::write(&a, "A-changed").unwrap();
        fs::write(&b, "B-changed").unwrap();

        let snapshot = expected(&opened.session);
        let mut ui = crate::execute::ScriptedExecuteUi {
            choice: crate::execute::MismatchAction::All,
            ..Default::default()
        };
        let journal = execute_session(&mut opened.session, &snapshot, &mut ui).unwrap();
        assert_eq!(journal.final_status, Some(JournalFinalStatus::Executed));
        assert_eq!(ui.prompts, 1);
        assert!(!a.exists());
        assert!(!b.exists());
        assert_eq!(fs::read_to_string(&a_to).unwrap(), "A-changed");
        assert_eq!(fs::read_to_string(&b_to).unwrap(), "B-changed");
    }

    #[test]
    fn skip_leaves_missing_file_and_continues() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore = isolated_environment(&temp);
        let root = temp.path().join("files");
        let (mut opened, a, b, a_to, b_to) = plan_two_renames(&root);
        fs::remove_file(&b).unwrap();

        let snapshot = expected(&opened.session);
        let mut ui = crate::execute::ScriptedExecuteUi {
            choice: crate::execute::MismatchAction::Skip,
            ..Default::default()
        };
        let journal = execute_session(&mut opened.session, &snapshot, &mut ui).unwrap();
        assert_eq!(journal.final_status, Some(JournalFinalStatus::Executed));
        assert!(a_to.exists());
        assert!(!a.exists());
        assert!(!b.exists());
        assert!(!b_to.exists());
        assert_eq!(ui.prompts, 1);
    }

    #[test]
    fn all_skips_remaining_missing_files() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore = isolated_environment(&temp);
        let root = temp.path().join("files");
        let (mut opened, a, b, a_to, b_to) = plan_two_renames(&root);
        fs::remove_file(&a).unwrap();
        fs::remove_file(&b).unwrap();

        let snapshot = expected(&opened.session);
        let mut ui = crate::execute::ScriptedExecuteUi {
            choice: crate::execute::MismatchAction::All,
            ..Default::default()
        };
        let journal = execute_session(&mut opened.session, &snapshot, &mut ui).unwrap();
        assert_eq!(journal.final_status, Some(JournalFinalStatus::Executed));
        assert_eq!(ui.prompts, 1);
        assert!(!a.exists());
        assert!(!b.exists());
        assert!(!a_to.exists());
        assert!(!b_to.exists());
        assert!(
            journal
                .steps
                .iter()
                .filter(|step| step.state == crate::model::StepState::Skipped)
                .count()
                >= 2
        );
    }

    fn poison_home_trash(temp: &tempfile::TempDir) {
        let trash = temp.path().join("xdg/Trash");
        fs::create_dir_all(trash.join("files")).unwrap();
        fs::create_dir_all(trash.join("info")).unwrap();
        for path in [trash.clone(), trash.join("files"), trash.join("info")] {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn plan_trashes(root: &Path, names: &[&str]) -> (OpenedSession, Vec<PathBuf>) {
        fs::create_dir(root).unwrap();
        let mut paths = Vec::new();
        for name in names {
            let path = root.join(name);
            fs::write(&path, name).unwrap();
            paths.push(path);
        }
        let mut opened = create_session(root, SessionMode::Move, &config(), true).unwrap();
        let mut raw = fs::read_to_string(&opened.session.plan_path).unwrap();
        for path in &paths {
            raw = raw.replace(&format!("\t:\t{}\n", path_text(path).unwrap()), "\t:\t\n");
        }
        fs::write(&opened.session.plan_path, raw).unwrap();
        refresh_hashes(&mut opened.session).unwrap();
        (opened, paths)
    }

    #[test]
    fn unsafe_trash_can_permanently_delete() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore = isolated_environment(&temp);
        poison_home_trash(&temp);
        let root = temp.path().join("files");
        let (mut opened, paths) = plan_trashes(&root, &["gone.txt"]);
        let source = &paths[0];

        let snapshot = expected(&opened.session);
        let mut ui = crate::execute::ScriptedExecuteUi {
            trash_choice: crate::execute::TrashFallbackAction::Delete,
            ..Default::default()
        };
        let journal = execute_session(&mut opened.session, &snapshot, &mut ui).unwrap();
        assert_eq!(journal.final_status, Some(JournalFinalStatus::Executed));
        assert_eq!(ui.trash_prompts, 1);
        assert!(!source.exists());
        assert!(
            journal
                .steps
                .iter()
                .filter(|step| step.planned.op == PlannedKind::Trash)
                .all(|step| step.trash_restore_key.is_none())
        );
    }

    #[test]
    fn unsafe_trash_skip_keeps_the_file() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore = isolated_environment(&temp);
        poison_home_trash(&temp);
        let root = temp.path().join("files");
        let (mut opened, paths) = plan_trashes(&root, &["keep.txt"]);
        let source = &paths[0];

        let snapshot = expected(&opened.session);
        let mut ui = crate::execute::ScriptedExecuteUi {
            trash_choice: crate::execute::TrashFallbackAction::Skip,
            ..Default::default()
        };
        let journal = execute_session(&mut opened.session, &snapshot, &mut ui).unwrap();
        assert_eq!(journal.final_status, Some(JournalFinalStatus::Executed));
        assert!(source.exists());
        assert_eq!(fs::read_to_string(source).unwrap(), "keep.txt");
        assert!(
            journal
                .steps
                .iter()
                .any(|step| step.state == crate::model::StepState::Skipped)
        );
    }

    #[test]
    fn unsafe_trash_all_deletes_remaining() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore = isolated_environment(&temp);
        poison_home_trash(&temp);
        let root = temp.path().join("files");
        let (mut opened, paths) = plan_trashes(&root, &["one.txt", "two.txt"]);

        let snapshot = expected(&opened.session);
        let mut ui = crate::execute::ScriptedExecuteUi {
            trash_choice: crate::execute::TrashFallbackAction::All,
            ..Default::default()
        };
        let journal = execute_session(&mut opened.session, &snapshot, &mut ui).unwrap();
        assert_eq!(journal.final_status, Some(JournalFinalStatus::Executed));
        assert_eq!(ui.trash_prompts, 1);
        assert!(!paths[0].exists());
        assert!(!paths[1].exists());
        assert!(
            journal
                .steps
                .iter()
                .filter(|step| step.planned.op == PlannedKind::Trash)
                .all(|step| step.state == crate::model::StepState::Committed
                    && step.trash_restore_key.is_none())
        );
    }

    #[test]
    fn moving_all_files_removes_empty_source_dir() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore = isolated_environment(&temp);
        let root = temp.path().join("files");
        fs::create_dir(&root).unwrap();
        let album = root.join("old-album");
        fs::create_dir(&album).unwrap();
        let a = album.join("a.txt");
        let b = album.join("b.txt");
        fs::write(&a, "A").unwrap();
        fs::write(&b, "B").unwrap();
        let dest_dir = root.join("artist").join("album");
        let a_to = dest_dir.join("a.txt");
        let b_to = dest_dir.join("b.txt");

        let mut opened = create_session(&root, SessionMode::Move, &config(), true).unwrap();
        let raw = fs::read_to_string(&opened.session.plan_path).unwrap();
        let edited = raw
            .replace(
                &format!("\t:\t{}\n", path_text(&a).unwrap()),
                &format!("\t:\t{}\n", path_text(&a_to).unwrap()),
            )
            .replace(
                &format!("\t:\t{}\n", path_text(&b).unwrap()),
                &format!("\t:\t{}\n", path_text(&b_to).unwrap()),
            );
        fs::write(&opened.session.plan_path, edited).unwrap();
        refresh_hashes(&mut opened.session).unwrap();

        let snapshot = expected(&opened.session);
        let mut ui = crate::execute::ScriptedExecuteUi::default();
        let journal = execute_session(&mut opened.session, &snapshot, &mut ui).unwrap();
        assert_eq!(journal.final_status, Some(JournalFinalStatus::Executed));
        assert!(!album.exists());
        assert!(root.exists());
        assert_eq!(fs::read_to_string(&a_to).unwrap(), "A");
        assert_eq!(fs::read_to_string(&b_to).unwrap(), "B");
        assert!(
            ui.commands
                .iter()
                .any(|line| line == &format!("rmdir {}", album.display()))
        );

        let undo = undo_session(&mut opened.session, &mut ui).unwrap();
        assert_eq!(undo.final_status, Some(JournalFinalStatus::Undone));
        assert_eq!(fs::read_to_string(&a).unwrap(), "A");
        assert_eq!(fs::read_to_string(&b).unwrap(), "B");
        assert!(album.exists());
    }

    #[test]
    fn copy_does_not_remove_source_dir() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore = isolated_environment(&temp);
        let root = temp.path().join("files");
        fs::create_dir(&root).unwrap();
        let album = root.join("album");
        fs::create_dir(&album).unwrap();
        let source = album.join("song.mp3");
        fs::write(&source, "music").unwrap();
        let dest = root.join("copy").join("song.mp3");

        let mut opened = create_session(&root, SessionMode::Copy, &config(), true).unwrap();
        let raw = fs::read_to_string(&opened.session.plan_path).unwrap();
        let edited = raw.replace(
            &format!("\t:\t{}\n", path_text(&source).unwrap()),
            &format!("\t:\t{}\n", path_text(&dest).unwrap()),
        );
        fs::write(&opened.session.plan_path, edited).unwrap();
        refresh_hashes(&mut opened.session).unwrap();

        let snapshot = expected(&opened.session);
        let mut ui = crate::execute::QuietExecuteUi;
        execute_session(&mut opened.session, &snapshot, &mut ui).unwrap();
        assert!(album.exists());
        assert!(source.exists());
        assert_eq!(fs::read_to_string(&dest).unwrap(), "music");
    }
}
