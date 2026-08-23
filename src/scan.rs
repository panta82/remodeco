use crate::model::{DirectoryIdentity, DirectoryKind, SourceFingerprint, SourceKind};
use crate::plan::unrepresentable_reason;
use anyhow::{Context, Result, bail};
use globset::{Glob, GlobMatcher};
use std::fs::{self, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub const DEFAULT_EXCLUDES: &[&str] = &[".git", "node_modules", ".hg", ".svn"];

#[derive(Clone, Debug)]
pub struct ScanRow {
    pub from: String,
    pub fingerprint: SourceFingerprint,
}

#[derive(Clone, Debug)]
pub struct ScanResult {
    pub root: String,
    pub rows: Vec<ScanRow>,
    pub skipped_special: usize,
}

pub fn fingerprint_from_metadata(metadata: &Metadata, kind: SourceKind) -> SourceFingerprint {
    let mtime_ns =
        i128::from(metadata.mtime()) * 1_000_000_000_i128 + i128::from(metadata.mtime_nsec());
    SourceFingerprint {
        kind,
        dev: metadata.dev().to_string(),
        ino: metadata.ino().to_string(),
        size: metadata.size().to_string(),
        nlink: metadata.nlink().to_string(),
        mtime_ns: mtime_ns.to_string(),
        mode: metadata.mode(),
    }
}

pub fn directory_identity(metadata: &Metadata) -> DirectoryIdentity {
    DirectoryIdentity {
        kind: DirectoryKind::Directory,
        dev: metadata.dev().to_string(),
        ino: metadata.ino().to_string(),
    }
}

pub fn lstat_fingerprint(path: &Path, kind: SourceKind) -> Result<SourceFingerprint> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("stat {}", path.display()))?;
    let actual = if metadata.file_type().is_symlink() {
        SourceKind::Symlink
    } else {
        SourceKind::File
    };
    if actual != kind {
        bail!("source type changed: {}", path.display());
    }
    Ok(fingerprint_from_metadata(&metadata, actual))
}

pub fn scan_tree(
    root: &Path,
    recursive: bool,
    hidden: bool,
    excludes: &[String],
) -> Result<ScanResult> {
    let canonical =
        fs::canonicalize(root).with_context(|| format!("resolve {}", root.display()))?;
    let root_text = canonical
        .to_str()
        .context("unrepresentable root path (invalid UTF-8)")?
        .to_owned();
    if let Some(reason) = unrepresentable_reason(&root_text) {
        bail!("unrepresentable root path ({reason}): {root_text}");
    }
    let matchers = compile_excludes(excludes)?;
    let mut result = ScanResult {
        root: root_text,
        rows: Vec::new(),
        skipped_special: 0,
    };
    walk(
        &canonical,
        &canonical,
        recursive,
        hidden,
        &matchers,
        &mut result,
    )?;
    result
        .rows
        .sort_by(|left, right| natural_cmp(&left.from, &right.from));
    Ok(result)
}

fn compile_excludes(patterns: &[String]) -> Result<Vec<GlobMatcher>> {
    patterns
        .iter()
        .map(|pattern| {
            Ok(Glob::new(pattern)
                .with_context(|| format!("invalid exclude pattern {pattern:?}"))?
                .compile_matcher())
        })
        .collect()
}

fn walk(
    root: &Path,
    directory: &Path,
    recursive: bool,
    hidden: bool,
    excludes: &[GlobMatcher],
    result: &mut ScanResult,
) -> Result<()> {
    for item in fs::read_dir(directory).with_context(|| format!("read {}", directory.display()))? {
        let item = item?;
        let name = item.file_name();
        let name = name
            .to_str()
            .context("unrepresentable filename (invalid UTF-8)")?;
        let absolute = item.path();
        let absolute_text = absolute
            .to_str()
            .context("unrepresentable path (invalid UTF-8)")?;
        if let Some(reason) = unrepresentable_reason(absolute_text) {
            bail!("unrepresentable path ({reason}): {absolute_text}");
        }
        let relative = absolute
            .strip_prefix(root)
            .unwrap_or(&absolute)
            .to_string_lossy()
            .replace('\\', "/");
        if (!hidden && name.starts_with('.')) || excluded(name, &relative, excludes) {
            continue;
        }
        let metadata = fs::symlink_metadata(&absolute)?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            result.rows.push(ScanRow {
                from: absolute_text.to_owned(),
                fingerprint: fingerprint_from_metadata(&metadata, SourceKind::Symlink),
            });
        } else if file_type.is_dir() {
            if recursive {
                walk(root, &absolute, recursive, hidden, excludes, result)?;
            }
        } else if file_type.is_file() {
            result.rows.push(ScanRow {
                from: absolute_text.to_owned(),
                fingerprint: fingerprint_from_metadata(&metadata, SourceKind::File),
            });
        } else {
            result.skipped_special += 1;
            eprintln!("skipping special file: {absolute_text}");
        }
    }
    Ok(())
}

fn excluded(basename: &str, relative: &str, matchers: &[GlobMatcher]) -> bool {
    matchers
        .iter()
        .any(|matcher| matcher.is_match(basename) || matcher.is_match(relative))
}

fn natural_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let mut a = left.chars().peekable();
    let mut b = right.chars().peekable();
    loop {
        match (a.peek(), b.peek()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let an: String = std::iter::from_fn(|| a.next_if(char::is_ascii_digit)).collect();
                let bn: String = std::iter::from_fn(|| b.next_if(char::is_ascii_digit)).collect();
                let az = an.trim_start_matches('0');
                let bz = bn.trim_start_matches('0');
                let order = az
                    .len()
                    .cmp(&bz.len())
                    .then_with(|| az.cmp(bz))
                    .then_with(|| an.len().cmp(&bn.len()));
                if order != Ordering::Equal {
                    return order;
                }
            }
            _ => {
                let ac = a.next().unwrap();
                let bc = b.next().unwrap();
                let order = ac.cmp(&bc);
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

pub fn deepest_existing(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    while current != Path::new("/") && fs::symlink_metadata(&current).is_err() {
        current.pop();
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_skips_hidden_and_default_excludes() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "a").unwrap();
        fs::write(dir.path().join(".secret"), "s").unwrap();
        fs::create_dir(dir.path().join("node_modules")).unwrap();
        fs::write(dir.path().join("node_modules/x.js"), "x").unwrap();
        let result = scan_tree(
            dir.path(),
            true,
            false,
            &DEFAULT_EXCLUDES
                .iter()
                .map(|v| (*v).to_owned())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert_eq!(result.rows.len(), 1);
        assert!(result.rows[0].from.ends_with("a.txt"));
    }

    #[test]
    fn sorts_numbers_naturally() {
        assert_eq!(natural_cmp("x2", "x10"), std::cmp::Ordering::Less);
    }
}
