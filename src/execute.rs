use crate::model::{
    DestParentRef, DirectoryIdentity, Journal, JournalFinalStatus, PlannedKind, SourceKind,
    StepState,
};
use crate::native::{copy_no_replace, rename_no_replace, symlink_no_replace};
use crate::scan::{directory_identity, fingerprint_from_metadata, lstat_fingerprint};
use crate::session::write_journal;
use crate::trash::{locate_trash, perform_trash, unique_trash_key};
use anyhow::{Context, Result, bail};
use std::fs;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

pub fn run_journal(session_id: &str, journal: &mut Journal) -> Result<()> {
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
        journal.steps[index].state = StepState::InProgress;
        write_journal(session_id, journal)?;
        if let Err(error) = apply_step(session_id, journal, index) {
            journal.steps[index].state = StepState::Failed;
            journal.steps[index].error = Some(format!("{error:#}"));
            journal.final_status = None;
            write_journal(session_id, journal)?;
            return Ok(());
        }
        journal.steps[index].state = StepState::Committed;
        write_journal(session_id, journal)?;
    }
    journal.final_status = Some(match journal.mode {
        crate::model::JournalMode::Execute => JournalFinalStatus::Executed,
        crate::model::JournalMode::Undo => JournalFinalStatus::Undone,
    });
    write_journal(session_id, journal)
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

fn apply_step(session_id: &str, journal: &mut Journal, index: usize) -> Result<()> {
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
            validate_source(
                from,
                planned
                    .fingerprint_from
                    .as_ref()
                    .context("rename has no fingerprint")?,
            )?;
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
            validate_source(from, fingerprint)?;
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
            validate_source(
                from,
                planned
                    .fingerprint_from
                    .as_ref()
                    .context("trash has no fingerprint")?,
            )?;
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
