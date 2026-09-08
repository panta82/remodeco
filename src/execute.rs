use crate::model::{
    DestParentRef, DirectoryIdentity, Journal, JournalFinalStatus, PlannedKind, PlannedStep,
    SessionId, SourceKind, StepState,
};
use crate::native::{copy_no_replace, rename_no_replace, symlink_no_replace};
use crate::scan::{directory_identity, fingerprint_from_metadata, lstat_fingerprint};
use crate::store::write_journal;
use crate::trash::{locate_trash, perform_trash, unique_trash_key};
use anyhow::{Context, Result, bail};
use std::fs;
use std::io::{self, BufReader, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MismatchAction {
    Proceed,
    Skip,
    All,
    Abort,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrashFallbackAction {
    Delete,
    Skip,
    All,
    Abort,
}

pub trait ExecuteUi {
    fn print_command(&mut self, command: &str);
    fn resolve_mismatch(&mut self, kind: &str, path: &str, missing: bool)
    -> Result<MismatchAction>;
    fn resolve_unsafe_trash(&mut self, path: &str, reason: &str) -> Result<TrashFallbackAction>;
}

pub struct StdioExecuteUi;

impl ExecuteUi for StdioExecuteUi {
    fn print_command(&mut self, command: &str) {
        println!("{command}");
        let _ = io::stdout().flush();
    }

    fn resolve_mismatch(
        &mut self,
        kind: &str,
        path: &str,
        missing: bool,
    ) -> Result<MismatchAction> {
        let message = if missing {
            format!("⚠️ {kind} target is missing: {path}")
        } else {
            format!("⚠️ {kind} target has changed since the plan was made: {path}")
        };
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            bail!("{message}");
        }
        println!("{message}");
        loop {
            if missing {
                print!("(s)kip   (a)ll   (q)uit ");
            } else {
                print!("(p)roceed   (s)kip   (a)ll   (q)uit ");
            }
            io::stdout().flush()?;
            let mut input = String::new();
            if io::stdin().read_line(&mut input)? == 0 {
                return Ok(MismatchAction::Abort);
            }
            match input.trim().chars().next() {
                Some('p' | 'P') if !missing => return Ok(MismatchAction::Proceed),
                Some('s' | 'S') => return Ok(MismatchAction::Skip),
                Some('a' | 'A') => return Ok(MismatchAction::All),
                Some('q' | 'Q') => return Ok(MismatchAction::Abort),
                _ => {}
            }
        }
    }

    fn resolve_unsafe_trash(&mut self, path: &str, reason: &str) -> Result<TrashFallbackAction> {
        let message = format!("⚠️ cannot send to trash ({reason}): {path}");
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            bail!("{message}");
        }
        println!("{message}");
        loop {
            print!("(d)elete   (s)kip   (a)ll   (q)uit ");
            io::stdout().flush()?;
            let mut input = String::new();
            if io::stdin().read_line(&mut input)? == 0 {
                return Ok(TrashFallbackAction::Abort);
            }
            match input.trim().chars().next() {
                Some('d' | 'D') => return Ok(TrashFallbackAction::Delete),
                Some('s' | 'S') => return Ok(TrashFallbackAction::Skip),
                Some('a' | 'A') => return Ok(TrashFallbackAction::All),
                Some('q' | 'Q') => return Ok(TrashFallbackAction::Abort),
                _ => {}
            }
        }
    }
}

pub struct QuietExecuteUi;

impl ExecuteUi for QuietExecuteUi {
    fn print_command(&mut self, _command: &str) {}

    fn resolve_mismatch(
        &mut self,
        kind: &str,
        path: &str,
        missing: bool,
    ) -> Result<MismatchAction> {
        if missing {
            bail!("{kind} target is missing: {path}");
        }
        bail!("{kind} target has changed since the plan was made: {path}");
    }

    fn resolve_unsafe_trash(&mut self, path: &str, reason: &str) -> Result<TrashFallbackAction> {
        bail!("cannot send to trash: {reason}: {path}");
    }
}

pub fn run_journal(
    session_id: &SessionId,
    journal: &mut Journal,
    root: &str,
    ui: &mut dyn ExecuteUi,
) -> Result<()> {
    let mut proceed_all = false;
    let mut skip_all_missing = false;
    let mut delete_all_untrashable = false;
    for index in 0..journal.steps.len() {
        if matches!(
            journal.steps[index].state,
            StepState::Committed | StepState::Skipped
        ) {
            continue;
        }
        if journal.steps[index].state == StepState::InProgress && reconcile_applied(journal, index)?
        {
            journal.steps[index].state = StepState::Committed;
            write_journal(session_id, journal)?;
            continue;
        }
        let mut verify_source = true;
        if let Some((kind, path)) = source_to_verify(&journal.steps[index].planned) {
            let kind = kind.to_string();
            let path = path.to_owned();
            let expected = journal.steps[index]
                .planned
                .fingerprint_from
                .clone()
                .context("step has no fingerprint")?;
            match source_status(Path::new(&path), &expected)? {
                SourceStatus::Ok => {}
                SourceStatus::Changed => {
                    let action = if proceed_all {
                        MismatchAction::Proceed
                    } else {
                        ui.resolve_mismatch(&kind, &path, false)?
                    };
                    match action {
                        MismatchAction::Proceed | MismatchAction::All => {
                            proceed_all = proceed_all || action == MismatchAction::All;
                            verify_source = false;
                        }
                        MismatchAction::Skip => {
                            skip_step(journal, index);
                            write_journal(session_id, journal)?;
                            continue;
                        }
                        MismatchAction::Abort => {
                            journal.steps[index].error = Some("aborted".into());
                            journal.final_status = None;
                            write_journal(session_id, journal)?;
                            return Ok(());
                        }
                    }
                }
                SourceStatus::Missing => {
                    let action = if skip_all_missing {
                        MismatchAction::Skip
                    } else {
                        ui.resolve_mismatch(&kind, &path, true)?
                    };
                    match action {
                        MismatchAction::Skip | MismatchAction::All => {
                            skip_all_missing = skip_all_missing || action == MismatchAction::All;
                            skip_step(journal, index);
                            write_journal(session_id, journal)?;
                            continue;
                        }
                        MismatchAction::Abort => {
                            journal.steps[index].error = Some("aborted".into());
                            journal.final_status = None;
                            write_journal(session_id, journal)?;
                            return Ok(());
                        }
                        MismatchAction::Proceed => {
                            journal.steps[index].state = StepState::Failed;
                            journal.steps[index].error =
                                Some(format!("{kind} target is missing: {path}"));
                            journal.final_status = None;
                            write_journal(session_id, journal)?;
                            return Ok(());
                        }
                    }
                }
            }
        }
        let mut permanent_delete = false;
        if journal.steps[index].planned.op == PlannedKind::Trash
            && let Some(from) = journal.steps[index].planned.from.clone()
            && let Err(error) = locate_trash(Path::new(&from))
        {
            let reason = format!("{error:#}");
            let action = if delete_all_untrashable {
                TrashFallbackAction::Delete
            } else {
                ui.resolve_unsafe_trash(&from, &reason)?
            };
            match action {
                TrashFallbackAction::Delete | TrashFallbackAction::All => {
                    delete_all_untrashable =
                        delete_all_untrashable || action == TrashFallbackAction::All;
                    permanent_delete = true;
                }
                TrashFallbackAction::Skip => {
                    skip_step(journal, index);
                    write_journal(session_id, journal)?;
                    continue;
                }
                TrashFallbackAction::Abort => {
                    journal.steps[index].error = Some("aborted".into());
                    journal.final_status = None;
                    write_journal(session_id, journal)?;
                    return Ok(());
                }
            }
        }
        if let Some(command) = command_line(&journal.steps[index].planned) {
            ui.print_command(&command);
        }
        journal.steps[index].state = StepState::InProgress;
        write_journal(session_id, journal)?;
        if let Err(error) = apply_step(session_id, journal, index, verify_source, permanent_delete)
        {
            journal.steps[index].state = StepState::Failed;
            journal.steps[index].error = Some(format!("{error:#}"));
            journal.final_status = None;
            write_journal(session_id, journal)?;
            return Ok(());
        }
        journal.steps[index].state = StepState::Committed;
        write_journal(session_id, journal)?;
        prune_empty_source_dirs(journal, index, root, ui);
    }
    journal.final_status = Some(match journal.mode {
        crate::model::JournalMode::Execute => JournalFinalStatus::Executed,
        crate::model::JournalMode::Undo => JournalFinalStatus::Undone,
    });
    write_journal(session_id, journal)
}

fn source_to_verify(planned: &PlannedStep) -> Option<(&'static str, &str)> {
    let path = planned.from.as_deref()?;
    let kind = match planned.op {
        PlannedKind::Trash => "Deletion",
        PlannedKind::Copy => "Copy",
        PlannedKind::Stage | PlannedKind::Commit => "Move",
        PlannedKind::Mkdir | PlannedKind::Noop => return None,
    };
    Some((kind, path))
}

pub(crate) fn command_line(planned: &PlannedStep) -> Option<String> {
    match planned.op {
        PlannedKind::Mkdir => Some(format!(
            "mkdir {}",
            planned.mkdir_path.as_deref().unwrap_or("?")
        )),
        PlannedKind::Trash => Some(format!("rm {}", planned.from.as_deref().unwrap_or("?"))),
        PlannedKind::Copy => Some(format!(
            "cp {} {}",
            planned.from.as_deref().unwrap_or("?"),
            planned.to.as_deref().unwrap_or("?")
        )),
        PlannedKind::Stage | PlannedKind::Commit => Some(format!(
            "mv {} {}",
            planned.from.as_deref().unwrap_or("?"),
            planned.to.as_deref().unwrap_or("?")
        )),
        PlannedKind::Noop => None,
    }
}

enum SourceStatus {
    Ok,
    Changed,
    Missing,
}

fn source_status(path: &Path, expected: &crate::model::SourceFingerprint) -> Result<SourceStatus> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(SourceStatus::Missing);
        }
        Err(error) => {
            return Err(error).with_context(|| format!("stat {}", path.display()));
        }
    };
    let actual = if metadata.file_type().is_symlink() {
        SourceKind::Symlink
    } else {
        SourceKind::File
    };
    if actual != expected.kind {
        return Ok(SourceStatus::Changed);
    }
    let live = fingerprint_from_metadata(&metadata, actual);
    if &live == expected {
        Ok(SourceStatus::Ok)
    } else {
        Ok(SourceStatus::Changed)
    }
}

fn prune_empty_source_dirs(journal: &Journal, index: usize, root: &str, ui: &mut dyn ExecuteUi) {
    let planned = &journal.steps[index].planned;
    if !matches!(
        planned.op,
        PlannedKind::Stage | PlannedKind::Commit | PlannedKind::Trash
    ) {
        return;
    }
    let Some(from) = planned.from.as_deref() else {
        return;
    };
    remove_empty_ancestors(Path::new(from), Path::new(root), ui);
}

fn remove_empty_ancestors(leaf: &Path, root: &Path, ui: &mut dyn ExecuteUi) {
    let mut current = match leaf.parent() {
        Some(parent) => parent.to_path_buf(),
        None => return,
    };
    loop {
        if current == root {
            break;
        }
        if current.strip_prefix(root).is_err() {
            break;
        }
        if !is_safe_empty_dir(&current) {
            break;
        }
        match fs::remove_dir(&current) {
            Ok(()) => ui.print_command(&format!("rmdir {}", current.display())),
            Err(_) => break,
        }
        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => break,
        }
    }
}

fn is_safe_empty_dir(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return false;
    }
    let Ok(mut entries) = fs::read_dir(path) else {
        return false;
    };
    entries.next().is_none()
}

fn skip_step(journal: &mut Journal, index: usize) {
    let id = journal.steps[index].planned.id;
    journal.steps[index].state = StepState::Skipped;
    if let Some(id) = id {
        for step in journal.steps.iter_mut().skip(index + 1) {
            if step.planned.id == Some(id) && step.state == StepState::Pending {
                step.state = StepState::Skipped;
            }
        }
    }
}

fn reconcile_applied(journal: &mut Journal, index: usize) -> Result<bool> {
    let step = &journal.steps[index];
    let planned = &step.planned;
    match planned.op {
        PlannedKind::Stage | PlannedKind::Commit => {
            let from = Path::new(planned.from.as_deref().context("rename has no source")?);
            let to = Path::new(planned.to.as_deref().context("rename has no destination")?);
            if fs::symlink_metadata(from).is_ok() {
                return Ok(false);
            }
            let expected = planned
                .fingerprint_from
                .as_ref()
                .context("rename has no fingerprint")?;
            let Ok(live) = lstat_fingerprint(to, expected.kind) else {
                return Ok(false);
            };
            if &live == expected {
                journal.steps[index].fingerprint_to = Some(live);
                return Ok(true);
            }
        }
        PlannedKind::Trash => {
            let Some(key) = &step.trash_restore_key else {
                return Ok(false);
            };
            let from = Path::new(planned.from.as_deref().context("trash has no source")?);
            if fs::symlink_metadata(from).is_ok() {
                return Ok(false);
            }
            let expected = planned
                .fingerprint_from
                .as_ref()
                .context("trash has no fingerprint")?;
            let Ok(live) = lstat_fingerprint(Path::new(&key.files_path), expected.kind) else {
                return Ok(false);
            };
            if &live == expected {
                journal.steps[index].fingerprint_to = Some(live);
                return Ok(true);
            }
        }
        PlannedKind::Copy => {
            let from = Path::new(planned.from.as_deref().context("copy has no source")?);
            let to = Path::new(planned.to.as_deref().context("copy has no destination")?);
            let expected = planned
                .fingerprint_from
                .as_ref()
                .context("copy has no fingerprint")?;
            if validate_source(from, expected).is_ok()
                && copied_content_matches(from, to, expected.kind)?
            {
                journal.steps[index].fingerprint_to = Some(fingerprint_at(to)?);
                return Ok(true);
            }
        }
        _ => {}
    }
    Ok(false)
}

fn copied_content_matches(from: &Path, to: &Path, kind: SourceKind) -> Result<bool> {
    if kind == SourceKind::Symlink {
        return Ok(fs::read_link(from).ok() == fs::read_link(to).ok());
    }
    let from_metadata = fs::metadata(from)?;
    let to_metadata = match fs::metadata(to) {
        Ok(metadata) if metadata.is_file() => metadata,
        _ => return Ok(false),
    };
    if from_metadata.len() != to_metadata.len() {
        return Ok(false);
    }
    let mut from_reader = BufReader::new(fs::File::open(from)?);
    let mut to_reader = BufReader::new(fs::File::open(to)?);
    let mut left = [0_u8; 128 * 1024];
    let mut right = [0_u8; 128 * 1024];
    loop {
        let left_len = from_reader.read(&mut left)?;
        let right_len = to_reader.read(&mut right)?;
        if left_len != right_len || left[..left_len] != right[..right_len] {
            return Ok(false);
        }
        if left_len == 0 {
            return Ok(true);
        }
    }
}

fn apply_step(
    session_id: &SessionId,
    journal: &mut Journal,
    index: usize,
    verify_source: bool,
    permanent_delete: bool,
) -> Result<()> {
    let planned = journal.steps[index].planned.clone();
    match planned.op {
        PlannedKind::Mkdir => {
            let path = Path::new(
                planned
                    .mkdir_path
                    .as_deref()
                    .context("mkdir step has no path")?,
            );
            validate_parent(
                journal,
                index,
                path,
                planned
                    .dest_parent_ref
                    .as_ref()
                    .context("mkdir has no parent reference")?,
            )?;
            let created = match fs::create_dir(path) {
                Ok(()) => true,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let metadata = fs::symlink_metadata(path)?;
                    if !metadata.is_dir() || metadata.file_type().is_symlink() {
                        return Err(error.into());
                    }
                    false
                }
                Err(error) => return Err(error.into()),
            };
            let identity = directory_identity(&fs::symlink_metadata(path)?);
            journal.steps[index].created_by_us = Some(created);
            journal.steps[index].created_dir_fingerprint = Some(identity);
        }
        PlannedKind::Stage | PlannedKind::Commit => {
            let from = Path::new(
                planned
                    .from
                    .as_deref()
                    .context("rename step has no source")?,
            );
            let to = Path::new(
                planned
                    .to
                    .as_deref()
                    .context("rename step has no destination")?,
            );
            if verify_source {
                validate_source(
                    from,
                    planned
                        .fingerprint_from
                        .as_ref()
                        .context("rename has no fingerprint")?,
                )?;
            }
            validate_parent(
                journal,
                index,
                to,
                planned
                    .dest_parent_ref
                    .as_ref()
                    .context("rename has no parent reference")?,
            )?;
            rename_no_replace(from, to)?;
            journal.steps[index].fingerprint_to = Some(fingerprint_at(to)?);
        }
        PlannedKind::Copy => {
            let from = Path::new(planned.from.as_deref().context("copy step has no source")?);
            let to = Path::new(
                planned
                    .to
                    .as_deref()
                    .context("copy step has no destination")?,
            );
            let fingerprint = planned
                .fingerprint_from
                .as_ref()
                .context("copy has no fingerprint")?;
            if verify_source {
                validate_source(from, fingerprint)?;
            }
            validate_parent(
                journal,
                index,
                to,
                planned
                    .dest_parent_ref
                    .as_ref()
                    .context("copy has no parent reference")?,
            )?;
            if fingerprint.kind == SourceKind::Symlink {
                symlink_no_replace(&fs::read_link(from)?, to)?;
            } else {
                copy_no_replace(from, to)?;
            }
            journal.steps[index].fingerprint_to = Some(fingerprint_at(to)?);
        }
        PlannedKind::Trash => {
            let from = Path::new(
                planned
                    .from
                    .as_deref()
                    .context("trash step has no source")?,
            );
            if verify_source {
                validate_source(
                    from,
                    planned
                        .fingerprint_from
                        .as_ref()
                        .context("trash has no fingerprint")?,
                )?;
            }
            if permanent_delete {
                fs::remove_file(from).with_context(|| format!("delete {}", from.display()))?;
                return Ok(());
            }
            let location = locate_trash(from)?;
            let key = unique_trash_key(&location, from)?;
            journal.steps[index].trash_restore_key = Some(key.clone());
            write_journal(session_id, journal)?;
            perform_trash(from, &key, &journal.journal_id, &location)?;
            journal.steps[index].fingerprint_to = Some(fingerprint_at(Path::new(&key.files_path))?);
        }
        PlannedKind::Noop => {}
    }
    Ok(())
}

fn validate_source(path: &Path, expected: &crate::model::SourceFingerprint) -> Result<()> {
    let live = lstat_fingerprint(path, expected.kind)?;
    if &live != expected {
        bail!("fingerprint mismatch: {}", path.display());
    }
    Ok(())
}

fn fingerprint_at(path: &Path) -> Result<crate::model::SourceFingerprint> {
    let metadata = fs::symlink_metadata(path)?;
    let kind = if metadata.file_type().is_symlink() {
        SourceKind::Symlink
    } else {
        SourceKind::File
    };
    Ok(fingerprint_from_metadata(&metadata, kind))
}

fn validate_parent(
    journal: &Journal,
    index: usize,
    destination: &Path,
    reference: &DestParentRef,
) -> Result<()> {
    let (parent_path, suffix, expected_identity): (&str, &[String], DirectoryIdentity) =
        match reference {
            DestParentRef::Existing {
                identity,
                parent_path,
                suffix,
            } => (parent_path, suffix, identity.clone()),
            DestParentRef::MkdirStep {
                step_id,
                parent_path,
                suffix,
            } => {
                let prior = journal.steps[..index]
                    .iter()
                    .find(|step| step.planned.step_id == *step_id)
                    .context("destination parent mkdir step not found")?;
                if prior.state != StepState::Committed {
                    bail!("destination parent mkdir was not committed");
                }
                let identity = prior
                    .created_dir_fingerprint
                    .clone()
                    .context("mkdir has no directory fingerprint")?;
                (parent_path, suffix, identity)
            }
        };
    let metadata = fs::symlink_metadata(parent_path)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || directory_identity(&metadata) != expected_identity
    {
        bail!("destination parent changed: {parent_path}");
    }
    let mut expected = PathBuf::from(parent_path);
    for component in suffix {
        expected.push(component);
    }
    if expected != destination {
        bail!(
            "destination does not match captured parent: {}",
            destination.display()
        );
    }
    Ok(())
}

#[cfg(test)]
pub struct ScriptedExecuteUi {
    pub commands: Vec<String>,
    pub choice: MismatchAction,
    pub prompts: usize,
    pub trash_choice: TrashFallbackAction,
    pub trash_prompts: usize,
}

#[cfg(test)]
impl Default for ScriptedExecuteUi {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
            choice: MismatchAction::Abort,
            prompts: 0,
            trash_choice: TrashFallbackAction::Abort,
            trash_prompts: 0,
        }
    }
}

#[cfg(test)]
impl ExecuteUi for ScriptedExecuteUi {
    fn print_command(&mut self, command: &str) {
        self.commands.push(command.to_owned());
    }

    fn resolve_mismatch(
        &mut self,
        _kind: &str,
        _path: &str,
        _missing: bool,
    ) -> Result<MismatchAction> {
        self.prompts += 1;
        Ok(self.choice)
    }

    fn resolve_unsafe_trash(&mut self, _path: &str, _reason: &str) -> Result<TrashFallbackAction> {
        self.trash_prompts += 1;
        Ok(self.trash_choice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::PlannedKind;

    fn planned(op: PlannedKind, from: &str, to: Option<&str>) -> PlannedStep {
        PlannedStep {
            step_id: "s".into(),
            op,
            from: Some(from.into()),
            to: to.map(str::to_owned),
            id: Some(1),
            fingerprint_from: None,
            dest_parent_ref: None,
            mkdir_path: None,
        }
    }

    #[test]
    fn command_line_uses_rm_mv_cp() {
        assert_eq!(
            command_line(&planned(PlannedKind::Trash, "/a", None)).as_deref(),
            Some("rm /a")
        );
        assert_eq!(
            command_line(&planned(PlannedKind::Commit, "/a", Some("/b"))).as_deref(),
            Some("mv /a /b")
        );
        assert_eq!(
            command_line(&planned(PlannedKind::Copy, "/a", Some("/b"))).as_deref(),
            Some("cp /a /b")
        );
        let mkdir = PlannedStep {
            mkdir_path: Some("/new".into()),
            from: None,
            ..planned(PlannedKind::Mkdir, "", None)
        };
        assert_eq!(command_line(&mkdir).as_deref(), Some("mkdir /new"));
    }

    #[test]
    fn rmdir_walks_empty_ancestors_and_keeps_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let nested = root.join("old").join("album");
        fs::create_dir_all(&nested).unwrap();
        let file = nested.join("track.mp3");
        fs::write(&file, "x").unwrap();
        fs::remove_file(&file).unwrap();
        let mut ui = QuietExecuteUi;
        remove_empty_ancestors(&file, &root, &mut ui);
        assert!(!nested.exists());
        assert!(!root.join("old").exists());
        assert!(root.exists());
    }

    #[test]
    fn rmdir_stops_when_a_directory_still_has_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let album = root.join("album");
        fs::create_dir_all(&album).unwrap();
        fs::write(album.join("keep.txt"), "k").unwrap();
        let gone = album.join("gone.txt");
        fs::write(&gone, "g").unwrap();
        fs::remove_file(&gone).unwrap();
        let mut ui = QuietExecuteUi;
        remove_empty_ancestors(&gone, &root, &mut ui);
        assert!(album.exists());
        assert!(album.join("keep.txt").exists());
    }

    #[test]
    fn rmdir_prints_removed_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let album = root.join("album");
        fs::create_dir_all(&album).unwrap();
        let file = album.join("t.mp3");
        fs::write(&file, "x").unwrap();
        fs::remove_file(&file).unwrap();
        let mut ui = ScriptedExecuteUi::default();
        remove_empty_ancestors(&file, &root, &mut ui);
        assert_eq!(ui.commands, vec![format!("rmdir {}", album.display())]);
        assert!(!album.exists());
    }
}
