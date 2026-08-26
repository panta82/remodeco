use crate::config::{EditorMode, PlanFormat};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

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
    #[arg(long, value_name = "ID")]
    pub session: Option<String>,

    /// Print sessions and exit.
    #[arg(long)]
    pub list_sessions: bool,

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
    /// Undo an executed session.
    Undo { session_id: String },
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
        &[Self::Properties, Self::Yaml, Self::PlainText]
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        Some(clap::builder::PossibleValue::new(match self {
            Self::Properties => "properties",
            Self::Yaml => "yaml",
            Self::PlainText => "plain-text",
        }))
    }
}
