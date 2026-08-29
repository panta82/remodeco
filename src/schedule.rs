use crate::model::{
    DestParentRef, ManifestEntry, OpKind, Operation, PlannedKind, PlannedStep, SessionMode,
};
use crate::scan::directory_identity;
use crate::util::path_text;
use anyhow::{Context, Result, bail};
use rand::RngCore;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct ScheduleResult {
    pub steps: Vec<PlannedStep>,
    pub outside_root: Vec<String>,
    pub trash_ids: Vec<u64>,
    pub errors: Vec<String>,
}

pub fn schedule(
    mode: SessionMode,
    root: &str,
    session_id: &str,
    manifest: &[ManifestEntry],
    operations: &[Operation],
) -> ScheduleResult {
    match try_schedule(mode, root, session_id, manifest, operations) {
        Ok(result) => result,
        Err(error) => ScheduleResult {
            errors: vec![format!("{error:#}")],
            ..ScheduleResult::default()
        },
    }
}

fn try_schedule(
    mode: SessionMode,
    root: &str,
    session_id: &str,
    manifest: &[ManifestEntry],
    operations: &[Operation],
) -> Result<ScheduleResult> {
    let by_id: HashMap<u64, &ManifestEntry> =
        manifest.iter().map(|entry| (entry.id, entry)).collect();
    let mut result = ScheduleResult::default();
    let mut mkdirs = HashMap::<PathBuf, String>::new();

    for operation in operations
        .iter()
        .filter(|operation| operation.kind != OpKind::Noop && operation.kind != OpKind::Skip)
    {
        by_id
            .get(&operation.id)
            .with_context(|| format!("missing manifest {}", operation.id))?;
    }

    let mut destination_counts = HashMap::<&str, usize>::new();
    for operation in operations
        .iter()
        .filter(|operation| matches!(operation.kind, OpKind::Move | OpKind::Copy))
    {
        *destination_counts.entry(&operation.to).or_default() += 1;
        if outside_root(root, &operation.to) && !result.outside_root.contains(&operation.to) {
            result.outside_root.push(operation.to.clone());
        }
    }
    for (destination, count) in destination_counts {
        if count > 1 {
            result.errors.push(format!("duplicate dest {destination}"));
        }
    }

    // Trash first: a later move is allowed to occupy the vacated path.
    let trash_sources: HashSet<&str> = operations
        .iter()
        .filter(|operation| operation.kind == OpKind::Trash)
        .map(|operation| operation.from.as_str())
        .collect();
    for operation in operations
        .iter()
        .filter(|operation| operation.kind == OpKind::Trash)
    {
        let entry = by_id[&operation.id];
        result.trash_ids.push(operation.id);
        result.steps.push(step(
            PlannedKind::Trash,
            Some(operation.from.clone()),
            None,
            Some(operation.id),
            Some(entry.fingerprint.clone()),
            None,
            None,
        ));
    }

    if mode == SessionMode::Copy {
        for operation in operations
            .iter()
            .filter(|operation| operation.kind == OpKind::Copy)
        {
            if fs::symlink_metadata(&operation.to).is_ok() {
                result.errors.push(format!(
                    "copy dest exists {{{}}} {}",
                    operation.id, operation.to
                ));
                continue;
            }
            let parent = ensure_mkdirs(Path::new(&operation.to), &mut result.steps, &mut mkdirs)?;
            result.steps.push(step(
                PlannedKind::Copy,
                Some(operation.from.clone()),
                Some(operation.to.clone()),
                Some(operation.id),
                Some(by_id[&operation.id].fingerprint.clone()),
                Some(parent),
                None,
            ));
        }
        return Ok(result);
    }

    #[derive(Clone)]
    struct PendingMove {
        id: u64,
        from: String,
        to: String,
        fingerprint: crate::model::SourceFingerprint,
    }
    let mut pending: Vec<PendingMove> = operations
        .iter()
        .filter(|operation| operation.kind == OpKind::Move)
        .map(|operation| PendingMove {
            id: operation.id,
            from: operation.from.clone(),
            to: operation.to.clone(),
            fingerprint: by_id[&operation.id].fingerprint.clone(),
        })
        .collect();
    let original_move_sources: HashSet<String> = pending
        .iter()
        .map(|operation| operation.from.clone())
        .collect();

    for operation in &pending {
        if fs::symlink_metadata(&operation.to).is_ok()
            && !original_move_sources.contains(&operation.to)
            && !trash_sources.contains(operation.to.as_str())
        {
            result
                .errors
                .push(format!("dest exists {{{}}} {}", operation.id, operation.to));
        }
    }
    if !result.errors.is_empty() {
        return Ok(result);
    }

    let mut staged = HashSet::new();
    while !pending.is_empty() {
        let remaining_sources: HashSet<&str> = pending
            .iter()
            .map(|operation| operation.from.as_str())
            .collect();
        if let Some(index) = pending
            .iter()
            .position(|operation| !remaining_sources.contains(operation.to.as_str()))
        {
            let operation = pending.remove(index);
            let parent = ensure_mkdirs(Path::new(&operation.to), &mut result.steps, &mut mkdirs)?;
            result.steps.push(step(
                PlannedKind::Commit,
                Some(operation.from),
                Some(operation.to),
                Some(operation.id),
                Some(operation.fingerprint),
                Some(parent),
                None,
            ));
            continue;
        }

        // Every remaining destination is another remaining source: break one cycle
        // by moving a source to a unique sibling first.
        let operation = &mut pending[0];
        if !staged.insert(operation.id) {
            result.errors.push(format!(
                "unresolvable destination cycle involving {{{}}}",
                operation.id
            ));
            break;
        }
        let original = Path::new(&operation.from);
        let temporary = original
            .parent()
            .unwrap_or_else(|| Path::new("/"))
            .join(format!(".remodeco-stage-{session_id}-{}", random_hex(6)));
        if fs::symlink_metadata(&temporary).is_ok() {
            bail!("staging collision: {}", temporary.display());
        }
        let parent = ensure_mkdirs(&temporary, &mut result.steps, &mut mkdirs)?;
        let temporary_text = temporary
            .to_str()
            .context("non-UTF-8 staging path")?
            .to_owned();
        result.steps.push(step(
            PlannedKind::Stage,
            Some(operation.from.clone()),
            Some(temporary_text.clone()),
            Some(operation.id),
            Some(operation.fingerprint.clone()),
            Some(parent),
            None,
        ));
        operation.from = temporary_text;
    }
    Ok(result)
}

fn step(
    op: PlannedKind,
    from: Option<String>,
    to: Option<String>,
    id: Option<u64>,
    fingerprint_from: Option<crate::model::SourceFingerprint>,
    dest_parent_ref: Option<DestParentRef>,
    mkdir_path: Option<String>,
) -> PlannedStep {
    PlannedStep {
        step_id: random_hex(6),
        op,
        from,
        to,
        id,
        fingerprint_from,
        dest_parent_ref,
        mkdir_path,
    }
}

fn random_hex(bytes: usize) -> String {
    let mut value = vec![0_u8; bytes];
    rand::thread_rng().fill_bytes(&mut value);
    hex::encode(value)
}

fn outside_root(root: &str, destination: &str) -> bool {
    destination != root
        && !destination
            .strip_prefix(root)
            .is_some_and(|tail| tail.starts_with('/'))
}

fn ensure_mkdirs(
    destination: &Path,
    steps: &mut Vec<PlannedStep>,
    made: &mut HashMap<PathBuf, String>,
) -> Result<DestParentRef> {
    let parent = destination.parent().context("destination has no parent")?;
    let (existing, missing) = resolve_existing_parent(parent)?;
    let metadata = fs::symlink_metadata(&existing)?;
    let mut previous = DestParentRef::Existing {
        identity: directory_identity(&metadata),
        parent_path: path_text(&existing)?,
        suffix: Vec::new(),
    };
    let mut current = existing;
    for component in missing {
        current.push(&component);
        if let Some(step_id) = made.get(&current) {
            previous = DestParentRef::MkdirStep {
                step_id: step_id.clone(),
                parent_path: path_text(&current)?,
                suffix: Vec::new(),
            };
            continue;
        }
        let current_text = path_text(&current)?;
        let planned = step(
            PlannedKind::Mkdir,
            None,
            None,
            None,
            None,
            Some(with_suffix(previous, vec![component.clone()])),
            Some(current_text.clone()),
        );
        let step_id = planned.step_id.clone();
        steps.push(planned);
        made.insert(current.clone(), step_id.clone());
        previous = DestParentRef::MkdirStep {
            step_id,
            parent_path: current_text,
            suffix: Vec::new(),
        };
    }
    let leaf = destination
        .file_name()
        .and_then(|v| v.to_str())
        .context("destination has invalid UTF-8 leaf")?
        .to_owned();
    Ok(with_suffix(previous, vec![leaf]))
}

pub(crate) fn capture_destination_parent(
    destination: &Path,
    steps: &mut Vec<PlannedStep>,
    made: &mut HashMap<PathBuf, String>,
) -> Result<DestParentRef> {
    ensure_mkdirs(destination, steps, made)
}

fn with_suffix(reference: DestParentRef, suffix: Vec<String>) -> DestParentRef {
    match reference {
        DestParentRef::Existing {
            identity,
            parent_path,
            ..
        } => DestParentRef::Existing {
            identity,
            parent_path,
            suffix,
        },
        DestParentRef::MkdirStep {
            step_id,
            parent_path,
            ..
        } => DestParentRef::MkdirStep {
            step_id,
            parent_path,
            suffix,
        },
    }
}

fn resolve_existing_parent(parent: &Path) -> Result<(PathBuf, Vec<String>)> {
    let mut current = parent.to_path_buf();
    let mut missing = Vec::new();
    loop {
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                missing.reverse();
                return Ok((current, missing));
            }
            Ok(_) => bail!(
                "destination ancestor is not a directory: {}",
                current.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = current
                    .file_name()
                    .and_then(|v| v.to_str())
                    .context("invalid destination ancestor")?
                    .to_owned();
                missing.push(name);
                if !current.pop() {
                    bail!("no existing destination ancestor");
                }
            }
            Err(error) => return Err(error).with_context(|| format!("stat {}", current.display())),
        }
    }
}

pub fn format_schedule(schedule: &ScheduleResult) -> String {
    let mut lines = Vec::new();
    for step in &schedule.steps {
        match step.op {
            PlannedKind::Mkdir => lines.push(format!(
                "mkdir {}",
                step.mkdir_path.as_deref().unwrap_or("?")
            )),
            PlannedKind::Trash => lines.push(format!(
                "trash {{{}}} {}",
                step.id.unwrap_or(0),
                step.from.as_deref().unwrap_or("?")
            )),
            _ => lines.push(
                format!(
                    "{:?} {{{}}} {} -> {}",
                    step.op,
                    step.id.unwrap_or(0),
                    step.from.as_deref().unwrap_or("?"),
                    step.to.as_deref().unwrap_or("?"),
                )
                .to_lowercase(),
            ),
        }
    }
    if !schedule.trash_ids.is_empty() {
        lines.push(format!("# trash ids: {}", join_ids(&schedule.trash_ids)));
    }
    if !schedule.outside_root.is_empty() {
        lines.push(format!(
            "# outside root: {}",
            schedule.outside_root.join(", ")
        ));
    }
    lines.join("\n")
}

fn join_ids(ids: &[u64]) -> String {
    ids.iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::scan_tree;

    fn manifest(dir: &Path) -> Vec<ManifestEntry> {
        scan_tree(dir, true, false, &[])
            .unwrap()
            .rows
            .into_iter()
            .enumerate()
            .map(|(index, row)| ManifestEntry {
                id: index as u64 + 1,
                from: row.from,
                fingerprint: row.fingerprint,
            })
            .collect()
    }

    #[test]
    fn new_directory_precedes_commit() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a"), "a").unwrap();
        let manifest = manifest(dir.path());
        let to = dir.path().join("nested/b").to_str().unwrap().to_owned();
        let operations = vec![Operation {
            id: 1,
            from: manifest[0].from.clone(),
            to,
            kind: OpKind::Move,
        }];
        let result = schedule(
            SessionMode::Move,
            dir.path().to_str().unwrap(),
            "s",
            &manifest,
            &operations,
        );
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.steps[0].op, PlannedKind::Mkdir);
        assert_eq!(result.steps[1].op, PlannedKind::Commit);
    }

    #[test]
    fn swap_uses_a_stage() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a"), "a").unwrap();
        fs::write(dir.path().join("b"), "b").unwrap();
        let manifest = manifest(dir.path());
        let a = manifest.iter().find(|e| e.from.ends_with("/a")).unwrap();
        let b = manifest.iter().find(|e| e.from.ends_with("/b")).unwrap();
        let operations = vec![
            Operation {
                id: a.id,
                from: a.from.clone(),
                to: b.from.clone(),
                kind: OpKind::Move,
            },
            Operation {
                id: b.id,
                from: b.from.clone(),
                to: a.from.clone(),
                kind: OpKind::Move,
            },
        ];
        let result = schedule(
            SessionMode::Move,
            dir.path().to_str().unwrap(),
            "s",
            &manifest,
            &operations,
        );
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(
            result
                .steps
                .iter()
                .filter(|s| s.op == PlannedKind::Stage)
                .count(),
            1
        );
        assert_eq!(
            result
                .steps
                .iter()
                .filter(|s| s.op == PlannedKind::Commit)
                .count(),
            2
        );
    }

    #[test]
    fn changed_fingerprint_still_schedules() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a"), "a").unwrap();
        let manifest = manifest(dir.path());
        fs::write(dir.path().join("a"), "changed").unwrap();
        let operations = vec![Operation {
            id: 1,
            from: manifest[0].from.clone(),
            to: dir.path().join("b").to_str().unwrap().to_owned(),
            kind: OpKind::Move,
        }];
        let result = schedule(
            SessionMode::Move,
            dir.path().to_str().unwrap(),
            "s",
            &manifest,
            &operations,
        );
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].op, PlannedKind::Commit);
    }

    #[test]
    fn missing_source_still_schedules() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a"), "a").unwrap();
        let manifest = manifest(dir.path());
        fs::remove_file(dir.path().join("a")).unwrap();
        let operations = vec![Operation {
            id: 1,
            from: manifest[0].from.clone(),
            to: dir.path().join("b").to_str().unwrap().to_owned(),
            kind: OpKind::Move,
        }];
        let result = schedule(
            SessionMode::Move,
            dir.path().to_str().unwrap(),
            "s",
            &manifest,
            &operations,
        );
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert_eq!(result.steps[0].op, PlannedKind::Commit);
    }
}
