use crate::config::{EditorMode, PlanFormat};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Session ids name a directory under the session store, so they must stay a single
/// path component. This is the only place untrusted ids enter remodeco.
fn parse_session_id(raw: &str) -> Result<String, String> {
    if raw.is_empty() || raw.contains(['/', '\\']) || raw == "." || raw == ".." {
        return Err("must be a session id, not a path".to_owned());
    }
    Ok(raw.to_owned())
}

#[derive(Clone, Debug, Parser)]
#[command(
    name = "remodeco",
    version,
    about = "Review and execute file renames in your editor, with a full-screen terminal diff"
)]
pub struct Cli {
    /// Directory to scan (defaults to the current directory).
    pub directory: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,

    /// Copy files instead of moving them.
    #[arg(long)]
    pub copy: bool,

    /// Do not launch an editor.
    #[arg(long)]
    pub no_edit: bool,

    /// Do not descend into directories.
    #[arg(long)]
    pub no_recursive: bool,

    /// Include hidden files and directories.
    #[arg(long)]
    pub hidden: bool,

    /// Additional exclude glob (repeatable).
    #[arg(long, value_name = "PATTERN")]
    pub exclude: Vec<String>,

    /// Do not skip .git, node_modules, .hg, and .svn by default.
    #[arg(long)]
    pub no_default_excludes: bool,

    /// Override the editor command.
    #[arg(long, value_name = "COMMAND")]
    pub editor: Option<String>,

    /// Override editor detection.
    #[arg(long, value_enum)]
    pub editor_mode: Option<EditorMode>,

    /// Never resume an unfinished session.
    #[arg(long = "new")]
    pub new_session: bool,

    /// Open an existing session.
    #[arg(long, value_name = "ID", value_parser = parse_session_id)]
    pub session: Option<String>,

    /// Validate and print the schedule without mutating files.
    #[arg(long)]
    pub dry_run: bool,

    /// Allow a scan of 25,000 files or more.
    #[arg(long)]
    pub yes: bool,

    /// Print extra diagnostics.
    #[arg(long)]
    pub verbose: bool,

    /// Plan file format: properties (default), yaml, or plain-text.
    #[arg(long, value_enum, value_name = "FORMAT")]
    pub format: Option<PlanFormat>,
}

#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    /// List sessions and exit.
    List {},

    /// Delete a session and its undo history.
    Delete {
        #[arg(value_parser = parse_session_id)]
        session_id: String,

        /// Delete even if the session could still be undone.
        #[arg(long)]
        force: bool,
    },

    /// Execute a prepared session.
    Execute {
        #[arg(value_parser = parse_session_id)]
        session_id: String,
    },

    /// Undo an executed session.
    Undo {
        #[arg(value_parser = parse_session_id)]
        session_id: String,
    },
}

impl clap::ValueEnum for EditorMode {
    fn value_variants<'a>() -> &'a [Self] {
        &[Self::Auto, Self::Gui, Self::Tty]
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        Some(clap::builder::PossibleValue::new(match self {
            Self::Auto => "auto",
            Self::Gui => "gui",
            Self::Tty => "tty",
        }))
    }
}

impl clap::ValueEnum for PlanFormat {
    fn value_variants<'a>() -> &'a [Self] {
        Self::ALL
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        Some(clap::builder::PossibleValue::new(match self {
            Self::Properties => "properties",
            Self::Yaml => "yaml",
            Self::PlainText => "plain-text",
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_execute_session_command() {
        let cli = Cli::try_parse_from(["remodeco", "execute", "session-123"]).unwrap();

        assert!(matches!(
            cli.command,
            Some(Command::Execute { session_id }) if session_id == "session-123"
        ));
    }

    #[test]
    fn rejects_session_ids_that_are_paths() {
        for raw in ["../escape", "a/b", "..", ".", ""] {
            assert!(
                Cli::try_parse_from(["remodeco", "execute", raw]).is_err(),
                "accepted {raw:?}"
            );
            assert!(
                Cli::try_parse_from(["remodeco", "--session", raw]).is_err(),
                "accepted {raw:?}"
            );
        }
    }
}
