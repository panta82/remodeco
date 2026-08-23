use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionMode {
    Move,
    Copy,
}

impl std::fmt::Display for SessionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Move => "move",
            Self::Copy => "copy",
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionStatus {
    Draft,
    Executing,
    ExecuteInterrupted,
    Executed,
    Undoing,
    UndoInterrupted,
    Undone,
}

impl SessionStatus {
    pub fn unfinished(self) -> bool {
        matches!(
            self,
            Self::Draft
                | Self::Executing
                | Self::ExecuteInterrupted
                | Self::Undoing
                | Self::UndoInterrupted
        )
    }
}

impl std::fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = serde_json::to_value(self).map_err(|_| std::fmt::Error)?;
        f.write_str(value.as_str().ok_or(std::fmt::Error)?)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceFingerprint {
    #[serde(rename = "type")]
    pub kind: SourceKind,
    pub dev: String,
    pub ino: String,
    pub size: String,
    pub nlink: String,
    #[serde(rename = "mtimeNs")]
    pub mtime_ns: String,
    pub mode: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    File,
    Symlink,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DirectoryIdentity {
    #[serde(rename = "type")]
    pub kind: DirectoryKind,
    pub dev: String,
    pub ino: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DirectoryKind {
    Directory,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ManifestEntry {
    pub id: u64,
    pub from: String,
    #[serde(flatten)]
    pub fingerprint: SourceFingerprint,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestFile {
    pub session_id: String,
    pub root: String,
    pub entries: Vec<ManifestEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    pub files: usize,
    pub symlinks: usize,
    pub skipped_special: usize,
    pub changes: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub id: String,
    pub root: String,
    pub mode: SessionMode,
    pub status: SessionStatus,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
    pub plan_path: String,
    pub manifest_path: String,
    pub generated_whole_file_hash: String,
    pub whole_file_hash: String,
    pub body_hash: String,
    pub plan_body_diverged: bool,
    pub active_journal_id: Option<String>,
    pub id_width: usize,
    pub tool_version: String,
    pub stats: SessionStats,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpKind {
    Skip,
    Noop,
    Trash,
    Move,
    Copy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Operation {
    pub id: u64,
    pub from: String,
    pub to: String,
    pub kind: OpKind,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind")]
pub enum DestParentRef {
    #[serde(rename = "existing")]
    Existing {
        identity: DirectoryIdentity,
        #[serde(rename = "parentPath")]
        parent_path: String,
        suffix: Vec<String>,
    },
    #[serde(rename = "mkdirStep")]
    MkdirStep {
        #[serde(rename = "stepId")]
        step_id: String,
        #[serde(rename = "parentPath")]
        parent_path: String,
        suffix: Vec<String>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlannedKind {
    Mkdir,
    Stage,
    Commit,
    Copy,
    Trash,
    Noop,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedStep {
    pub step_id: String,
    pub op: PlannedKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint_from: Option<SourceFingerprint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dest_parent_ref: Option<DestParentRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mkdir_path: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    Pending,
    InProgress,
    Committed,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashKey {
    pub files_path: String,
    pub info_path: Option<String>,
    pub original_path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalStep {
    #[serde(flatten)]
    pub planned: PlannedStep,
    pub state: StepState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by_us: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_dir_fingerprint: Option<DirectoryIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint_to: Option<SourceFingerprint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trash_restore_key: Option<TrashKey>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JournalMode {
    Execute,
    Undo,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JournalFinalStatus {
    Executed,
    Undone,
    Aborted,
    Superseded,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Journal {
    pub journal_id: String,
    pub mode: JournalMode,
    pub parent_journal_id: Option<String>,
    pub final_status: Option<JournalFinalStatus>,
    pub superseded_by_journal_id: Option<String>,
    pub expected_plan_hash: String,
    pub expected_revision: u64,
    pub expected_mode: SessionMode,
    pub steps: Vec<JournalStep>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_typescript_session_shape() {
        let raw = r#"{
          "id":"s","root":"/tmp/r","mode":"move","status":"execute-interrupted",
          "revision":2,"createdAt":"2026-01-01T00:00:00.000Z","updatedAt":"2026-01-01T00:00:01.000Z",
          "planPath":"/tmp/s/plan.md","manifestPath":"/tmp/s/manifest.json",
          "generatedWholeFileHash":"sha256:a","wholeFileHash":"sha256:b","bodyHash":"sha256:c",
          "planBodyDiverged":false,"activeJournalId":null,"idWidth":2,"toolVersion":"1.0.0",
          "stats":{"files":1,"symlinks":0,"skippedSpecial":0,"changes":0}
        }"#;
        let session: SessionRecord = serde_json::from_str(raw).unwrap();
        assert_eq!(session.status, SessionStatus::ExecuteInterrupted);
        assert_eq!(session.generated_whole_file_hash, "sha256:a");
    }

    #[test]
    fn journal_step_uses_compatible_flat_camel_case_keys() {
        let step = JournalStep {
            planned: PlannedStep {
                step_id: "abc".into(),
                op: PlannedKind::Commit,
                from: Some("/a".into()),
                to: Some("/b".into()),
                id: Some(1),
                fingerprint_from: None,
                dest_parent_ref: None,
                mkdir_path: None,
            },
            state: StepState::InProgress,
            error: None,
            created_by_us: None,
            created_dir_fingerprint: None,
            fingerprint_to: None,
            trash_restore_key: None,
        };
        let value = serde_json::to_value(step).unwrap();
        assert_eq!(value["stepId"], "abc");
        assert_eq!(value["state"], "in_progress");
        assert_eq!(value["from"], "/a");
        assert!(value.get("fingerprintFrom").is_none());
    }
}
