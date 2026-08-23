use crate::model::{ManifestEntry, OpKind, Operation, SessionMode};
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use thiserror::Error;

pub const MAX_FILE_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_LINE_BYTES: usize = 64 * 1024;
pub const MAX_LINES: usize = 200_000;
pub const MAX_DEST_BYTES: usize = 4096;

#[derive(Debug, Error)]
#[error("{message}")]
pub struct PlanError {
    pub code: &'static str,
    pub message: String,
}

impl PlanError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanHeader {
    pub remodeco: u64,
    pub id: String,
    pub root: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BulletKind {
    Trash,
    Destination,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedBullet {
    pub id: u64,
    pub destination: String,
    pub kind: BulletKind,
    pub line: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedPlan {
    pub header: Option<PlanHeader>,
    pub bullets: Vec<ParsedBullet>,
    pub unknown_lines: Vec<(usize, String)>,
}

pub fn id_width_for_count(count: usize) -> usize {
    count.max(1).to_string().len()
}

pub fn yaml_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub fn parse_yaml_single_quoted(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        value[1..value.len() - 1].replace("''", "'")
    } else {
        value.to_owned()
    }
}

pub fn generate_plan(
    session_id: &str,
    root: &str,
    entries: &[ManifestEntry],
    id_width: usize,
) -> String {
    let mut out = format!(
        "---\nremodeco: 1\nid: {session_id}\nroot: {}\n---\n\n\
         > Edit the path after `{{id}}` (the tab) to **rename/move**.\n\
         > Leave a path unchanged = do nothing. Tab then empty dest = **trash**.\n\
         > Delete a line, or `Ctrl+/` comment it, to **skip**.\n\
         > Headings are labels only — they do not move files.\n\
         > When done, Execute in the TUI (`e`).\n\n\
         # {root}\n",
        yaml_single_quote(root)
    );
    let mut current_parent: Option<String> = None;
    for entry in entries {
        let parent = Path::new(&entry.from)
            .parent()
            .unwrap_or_else(|| Path::new("/"));
        let relative = if parent == Path::new(root) {
            ".".to_owned()
        } else {
            parent.strip_prefix(root).map_or_else(
                |_| parent.to_string_lossy().into_owned(),
                |p| p.to_string_lossy().into_owned(),
            )
        };
        if current_parent.as_deref() != Some(&relative) {
            current_parent = Some(relative.clone());
            out.push_str(&format!("\n## {relative}\n"));
        }
        out.push_str(&format!(
            "- `{{{:0width$}}}`\t{}\n",
            entry.id,
            entry.from,
            width = id_width
        ));
    }
    out
}

pub fn drop_comment_lines<'a>(
    lines: &'a [&'a str],
) -> std::result::Result<Vec<&'a str>, PlanError> {
    let mut output = Vec::with_capacity(lines.len());
    let mut in_block = false;
    for line in lines {
        if in_block {
            if line.contains("-->") {
                in_block = false;
            }
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("<!--") {
            if !trimmed.contains("-->") {
                in_block = true;
            }
            continue;
        }
        output.push(*line);
    }
    if in_block {
        return Err(PlanError::new(
            "unterminated-html-comment",
            "unterminated HTML comment",
        ));
    }
    Ok(output)
}

pub fn parse_plan(raw: &str) -> std::result::Result<ParsedPlan, PlanError> {
    if raw.len() > MAX_FILE_BYTES {
        return Err(PlanError::new("plan-too-large", "plan too large"));
    }
    if raw.contains('\r') {
        return Err(PlanError::new("cr", "plan.md contains CR; save as LF"));
    }
    let raw_lines: Vec<&str> = raw.split('\n').collect();
    if raw_lines.len() > MAX_LINES {
        return Err(PlanError::new("too-many-lines", "plan has too many lines"));
    }
    if raw_lines.iter().any(|line| line.len() > MAX_LINE_BYTES) {
        return Err(PlanError::new(
            "line-too-long",
            "plan contains a line that is too long",
        ));
    }
    let lines = drop_comment_lines(&raw_lines)?;

    let mut body_start = 0;
    let mut header = None;
    if lines.first() == Some(&"---") {
        let mut fields = HashMap::new();
        let mut index = 1;
        while index < lines.len() && lines[index] != "---" {
            if let Some((key, value)) = lines[index].split_once(':') {
                fields.insert(key.trim().to_owned(), value.trim().to_owned());
            }
            index += 1;
        }
        if index >= lines.len() {
            return Err(PlanError::new("front-matter", "unterminated front matter"));
        }
        body_start = index + 1;
        header = Some(PlanHeader {
            remodeco: fields
                .get("remodeco")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            id: fields.get("id").cloned().unwrap_or_default(),
            root: parse_yaml_single_quoted(
                fields.get("root").map(String::as_str).unwrap_or_default(),
            ),
        });
    }

    let mut bullets = Vec::new();
    let mut unknown_lines = Vec::new();
    let mut seen = HashSet::new();
    for (index, text) in lines.iter().enumerate().skip(body_start) {
        let line_number = index + 1;
        let trimmed = text.trim();
        if trimmed.is_empty()
            || heading(text)
            || text.trim_start().starts_with('>')
            || trimmed.chars().all(|c| c == '-')
        {
            continue;
        }
        let Some(rest) = text.strip_prefix("- `{") else {
            unknown_lines.push((line_number, (*text).to_owned()));
            continue;
        };
        let Some((id_text, destination)) = rest.split_once("}`\t") else {
            unknown_lines.push((line_number, (*text).to_owned()));
            continue;
        };
        if id_text.is_empty() || !id_text.bytes().all(|b| b.is_ascii_digit()) {
            unknown_lines.push((line_number, (*text).to_owned()));
            continue;
        }
        let id = id_text
            .parse::<u64>()
            .map_err(|_| PlanError::new("bad-id", "invalid id"))?;
        if !seen.insert(id) {
            return Err(PlanError::new("duplicate-id", format!("duplicate id {id}")));
        }
        if destination.len() > MAX_DEST_BYTES {
            return Err(PlanError::new(
                "dest-too-long",
                format!("destination for {{{id}}} is too long"),
            ));
        }
        let kind = if destination.is_empty() {
            BulletKind::Trash
        } else {
            if !is_line_safe_path(destination) || !is_absolute_posix(destination) {
                return Err(PlanError::new(
                    "bad-dest",
                    format!("unrepresentable or non-absolute destination for {{{id}}}"),
                ));
            }
            BulletKind::Destination
        };
        bullets.push(ParsedBullet {
            id,
            destination: destination.to_owned(),
            kind,
            line: line_number,
        });
    }
    Ok(ParsedPlan {
        header,
        bullets,
        unknown_lines,
    })
}

fn heading(line: &str) -> bool {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    (1..=6).contains(&hashes) && line.as_bytes().get(hashes) == Some(&b' ')
}

pub fn classify_ops(
    parsed: &ParsedPlan,
    manifest: &[ManifestEntry],
    mode: SessionMode,
) -> std::result::Result<(Vec<Operation>, Vec<u64>), PlanError> {
    let by_id: HashMap<u64, &str> = manifest
        .iter()
        .map(|entry| (entry.id, entry.from.as_str()))
        .collect();
    let active: HashSet<u64> = parsed.bullets.iter().map(|bullet| bullet.id).collect();
    let skipped = manifest
        .iter()
        .filter(|entry| !active.contains(&entry.id))
        .map(|entry| entry.id)
        .collect();
    let mut operations = Vec::with_capacity(parsed.bullets.len());
    for bullet in &parsed.bullets {
        let from = by_id.get(&bullet.id).ok_or_else(|| {
            PlanError::new(
                "unknown-id",
                format!("id {{{}}} is not in the manifest", bullet.id),
            )
        })?;
        let (to, kind) = match bullet.kind {
            BulletKind::Trash => (String::new(), OpKind::Trash),
            BulletKind::Destination if bullet.destination == *from => {
                (bullet.destination.clone(), OpKind::Noop)
            }
            BulletKind::Destination => (
                bullet.destination.clone(),
                match mode {
                    SessionMode::Move => OpKind::Move,
                    SessionMode::Copy => OpKind::Copy,
                },
            ),
        };
        operations.push(Operation {
            id: bullet.id,
            from: (*from).to_owned(),
            to,
            kind,
        });
    }
    Ok((operations, skipped))
}

pub fn is_line_safe_path(path: &str) -> bool {
    !path.contains(['\0', '\r', '\n'])
}

pub fn unrepresentable_reason(path: &str) -> Option<&'static str> {
    if path.contains('\0') {
        Some("NUL byte")
    } else if path.contains('\r') {
        Some("CR")
    } else if path.contains('\n') {
        Some("LF")
    } else {
        None
    }
}

pub fn is_absolute_posix(path: &str) -> bool {
    if !path.starts_with('/') || path == "/" || path.ends_with('/') {
        return false;
    }
    path[1..]
        .split('/')
        .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

pub fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes.as_ref())))
}

pub fn whole_file_hash(raw: &str) -> String {
    sha256_hex(raw)
}

pub fn body_hash(raw: &str) -> String {
    let body = raw
        .find("\n---\n")
        .map_or(raw, |position| &raw[position + 5..]);
    sha256_hex(body)
}

pub fn validate_plan_header(parsed: &ParsedPlan, session_id: &str, root: &str) -> Result<()> {
    let header = parsed
        .header
        .as_ref()
        .ok_or_else(|| PlanError::new("front-matter", "missing front matter"))?;
    if header.remodeco != 1 || header.id != session_id || header.root != root {
        return Err(PlanError::new(
            "front-matter",
            "plan front matter does not match the session",
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SourceFingerprint, SourceKind};

    fn entry(id: u64, from: &str) -> ManifestEntry {
        ManifestEntry {
            id,
            from: from.to_owned(),
            fingerprint: SourceFingerprint {
                kind: SourceKind::File,
                dev: "1".into(),
                ino: id.to_string(),
                size: "0".into(),
                nlink: "1".into(),
                mtime_ns: "0".into(),
                mode: 0o100644,
            },
        }
    }

    #[test]
    fn generated_plan_round_trips_special_markdown_characters() {
        let entries = vec![entry(1, "/tmp/r/You've `made` <!-- it.mp3")];
        let raw = generate_plan("s", "/tmp/r", &entries, 2);
        let parsed = parse_plan(&raw).unwrap();
        assert!(parsed.unknown_lines.is_empty());
        assert_eq!(parsed.bullets[0].destination, entries[0].from);
        assert_eq!(parsed.header.unwrap().root, "/tmp/r");
    }

    #[test]
    fn comments_skip_but_comment_text_in_a_path_does_not() {
        let raw = "<!-- - `{1}`\t/tmp/skip -->\n- `{2}`\t/tmp/a<!--b\n";
        let parsed = parse_plan(raw).unwrap();
        assert_eq!(parsed.bullets.len(), 1);
        assert_eq!(parsed.bullets[0].id, 2);
    }

    #[test]
    fn missing_tab_is_unknown() {
        let parsed = parse_plan("- `{1}` /tmp/a\n").unwrap();
        assert_eq!(parsed.unknown_lines.len(), 1);
    }

    #[test]
    fn classifies_noop_move_trash_and_skip() {
        let manifest = vec![
            entry(1, "/tmp/a"),
            entry(2, "/tmp/b"),
            entry(3, "/tmp/c"),
            entry(4, "/tmp/d"),
        ];
        let parsed = parse_plan("- `{1}`\t/tmp/a\n- `{2}`\t/tmp/x\n- `{3}`\t\n").unwrap();
        let (ops, skipped) = classify_ops(&parsed, &manifest, SessionMode::Move).unwrap();
        assert_eq!(
            ops.iter().map(|o| &o.kind).collect::<Vec<_>>(),
            vec![&OpKind::Noop, &OpKind::Move, &OpKind::Trash]
        );
        assert_eq!(skipped, vec![4]);
    }
}
