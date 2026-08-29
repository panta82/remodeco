use crate::cli::Cli;
use crate::scan::DEFAULT_EXCLUDES;
use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum EditorMode {
    Auto,
    Gui,
    Tty,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum PlanFormat {
    #[default]
    Properties,
    Yaml,
    PlainText,
}

impl PlanFormat {
    /// Every format, in `--format` order.
    pub const ALL: &'static [Self] = &[Self::Properties, Self::Yaml, Self::PlainText];

    pub fn file_name(self) -> &'static str {
        match self {
            Self::Properties => "plan.properties",
            Self::Yaml => "plan.yaml",
            Self::PlainText => "plan.txt",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum EditorCommand {
    String(String),
    Args(Vec<String>),
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub editor: Option<EditorCommand>,
    pub editor_mode: EditorMode,
    pub exclude: Vec<String>,
    pub include_hidden: bool,
    pub recursive: bool,
    pub open_editor: bool,
    pub plan_format: PlanFormat,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PartialConfig {
    editor: Option<EditorCommand>,
    editor_mode: Option<EditorMode>,
    exclude: Option<Vec<String>>,
    include_hidden: Option<bool>,
    recursive: Option<bool>,
    open_editor: Option<bool>,
    format: Option<PlanFormat>,
    plain_text: Option<bool>,
}

pub fn config_dir() -> Result<PathBuf> {
    if let Some(path) = env::var_os("REMODECO_CONFIG_DIR") {
        return Ok(path.into());
    }
    Ok(ProjectDirs::from("", "", "remodeco")
        .context("cannot determine config directory")?
        .config_dir()
        .to_path_buf())
}

pub fn data_dir() -> Result<PathBuf> {
    if let Some(path) = env::var_os("REMODECO_DATA_DIR") {
        return Ok(path.into());
    }
    Ok(ProjectDirs::from("", "", "remodeco")
        .context("cannot determine data directory")?
        .data_dir()
        .to_path_buf())
}

pub fn load_config(root: &Path, cli: &Cli) -> Result<AppConfig> {
    let mut config = AppConfig {
        editor: None,
        editor_mode: EditorMode::Auto,
        exclude: DEFAULT_EXCLUDES
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        include_hidden: false,
        recursive: true,
        open_editor: true,
        plan_format: PlanFormat::Properties,
    };
    merge_file(&mut config, &config_dir()?.join("config.json"))?;
    merge_file(&mut config, &root.join(".remodeco.json"))?;
    if cli.no_default_excludes {
        config.exclude.clear();
    }
    config.exclude.extend(cli.exclude.iter().cloned());
    if cli.hidden {
        config.include_hidden = true;
    }
    if cli.no_recursive {
        config.recursive = false;
    }
    if let Some(editor) = &cli.editor {
        config.editor = Some(EditorCommand::String(editor.clone()));
    }
    if let Some(mode) = cli.editor_mode {
        config.editor_mode = mode;
    }
    if cli.no_edit || env::var("REMODECO_NO_EDIT").as_deref() == Ok("1") {
        config.open_editor = false;
    }
    if let Some(format) = cli.format {
        config.plan_format = format;
    }
    if let Ok(editor) = env::var("REMODECO_EDITOR") {
        config.editor = Some(EditorCommand::String(editor));
    }
    Ok(config)
}

fn merge_file(config: &mut AppConfig, path: &Path) -> Result<()> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let partial: PartialConfig =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    if let Some(value) = partial.editor {
        config.editor = Some(value);
    }
    if let Some(value) = partial.editor_mode {
        config.editor_mode = value;
    }
    if let Some(value) = partial.exclude {
        config.exclude.extend(value);
    }
    if let Some(value) = partial.include_hidden {
        config.include_hidden = value;
    }
    if let Some(value) = partial.recursive {
        config.recursive = value;
    }
    if let Some(value) = partial.open_editor {
        config.open_editor = value;
    }
    if let Some(value) = partial.format {
        config.plan_format = value;
    } else if partial.plain_text == Some(true) {
        config.plan_format = PlanFormat::PlainText;
    }
    Ok(())
}
