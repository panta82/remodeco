use crate::config::{AppConfig, EditorCommand, EditorMode};
use anyhow::{Context, Result};
use std::env;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const GUI_EDITORS: &[&str] = &["code", "codium", "subl", "kate", "gedit", "code-insiders"];
const TTY_EDITORS: &[&str] = &[
    "vi", "vim", "nvim", "nano", "pico", "emacs", "hx", "helix", "micro", "ed",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorKind {
    Gui,
    Tty,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedEditor {
    pub argv: Vec<String>,
    pub kind: EditorKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchResult {
    Detached,
    Waited,
    Missing,
}

pub fn is_headless_remote() -> bool {
    let ssh = ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"]
        .iter()
        .any(|name| env::var_os(name).is_some());
    ssh && env::var("TERM_PROGRAM").as_deref() != Ok("vscode")
}

pub fn resolve_editor(config: &AppConfig) -> Result<Option<ResolvedEditor>> {
    let mut explicit = false;
    let mut argv = match &config.editor {
        Some(EditorCommand::Args(args)) => {
            explicit = true;
            args.clone()
        }
        Some(EditorCommand::String(command)) => {
            explicit = true;
            shell_words::split(command).context("invalid editor command")?
        }
        None => Vec::new(),
    };
    if argv.is_empty()
        && let Ok(command) = env::var("VISUAL")
    {
        argv = shell_words::split(&command).context("invalid VISUAL")?;
    }
    if argv.is_empty()
        && let Ok(command) = env::var("EDITOR")
    {
        argv = shell_words::split(&command).context("invalid EDITOR")?;
    }
    let gui_allowed = config.editor_mode == EditorMode::Gui
        || (!is_headless_remote() && config.editor_mode != EditorMode::Tty);
    if argv.is_empty()
        && gui_allowed
        && let Some(editor) = GUI_EDITORS.iter().find(|editor| which(editor).is_some())
    {
        argv.push((*editor).to_owned());
    }
    if argv.is_empty()
        && let Some(editor) = ["vi", "nano"].iter().find(|editor| which(editor).is_some())
    {
        argv.push((*editor).to_owned());
    }
    if argv.is_empty() {
        return Ok(None);
    }
    let base = Path::new(&argv[0])
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or(&argv[0]);
    let mut kind = if GUI_EDITORS.contains(&base) {
        EditorKind::Gui
    } else {
        EditorKind::Tty
    };
    if TTY_EDITORS.contains(&base) {
        kind = EditorKind::Tty;
    }
    match config.editor_mode {
        EditorMode::Gui => kind = EditorKind::Gui,
        EditorMode::Tty => kind = EditorKind::Tty,
        EditorMode::Auto => {}
    }
    if is_headless_remote()
        && kind == EditorKind::Gui
        && !explicit
        && config.editor_mode == EditorMode::Auto
    {
        kind = EditorKind::Tty;
    }
    Ok(Some(ResolvedEditor { argv, kind }))
}

pub fn launch_editor(config: &AppConfig, plan: &Path) -> Result<LaunchResult> {
    if !config.open_editor {
        eprintln!("plan: {}", plan.display());
        return Ok(LaunchResult::Missing);
    }
    let Some(editor) = resolve_editor(config)? else {
        eprintln!("no editor found; plan: {}", plan.display());
        return Ok(LaunchResult::Missing);
    };
    if editor.kind == EditorKind::Gui {
        let mut command = Command::new(&editor.argv[0]);
        command
            .args(&editor.argv[1..])
            .arg(plan)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        reap_in_background(command.spawn()?);
        return Ok(LaunchResult::Detached);
    }
    if spawn_multiplexer(&editor, plan)? {
        eprintln!(
            "Opened {} in another pane. Edit and save, then Execute here (e).",
            basename(&editor.argv[0])
        );
        eprintln!("Plan: {}", plan.display());
        return Ok(LaunchResult::Detached);
    }
    attach_and_wait(&editor, plan)?;
    Ok(LaunchResult::Waited)
}

pub fn attach_and_wait(editor: &ResolvedEditor, plan: &Path) -> Result<i32> {
    eprintln!(
        "Opening {}. Save and quit to return to the remodeco preview.",
        basename(&editor.argv[0])
    );
    eprintln!("Plan: {}", plan.display());
    let mut child = Command::new(&editor.argv[0])
        .args(&editor.argv[1..])
        .arg(plan)
        .spawn()?;
    // The attached editor shares our foreground process group. Keep Ctrl-C
    // available to the editor without terminating remodeco while it waits.
    let _signal_guard = IgnoreSigint::new();
    let status = child.wait()?;
    Ok(status.code().unwrap_or(1))
}

fn spawn_multiplexer(editor: &ResolvedEditor, plan: &Path) -> Result<bool> {
    let plan = plan.as_os_str();
    let command = if env::var_os("TMUX").is_some() {
        Some(("tmux", vec!["split-window", "-h"]))
    } else if env::var_os("ZELLIJ").is_some() {
        Some(("zellij", vec!["action", "new-pane", "--"]))
    } else if env::var_os("KITTY_WINDOW_ID").is_some() {
        Some(("kitty", vec!["@", "launch", "--type=tab"]))
    } else if env::var_os("WEZTERM_PANE").is_some() {
        Some(("wezterm", vec!["cli", "spawn", "--new-tab"]))
    } else {
        None
    };
    let Some((program, prefix)) = command else {
        return Ok(false);
    };
    let child = Command::new(program)
        .args(prefix)
        .args(&editor.argv)
        .arg(plan)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Ok(child) = child {
        reap_in_background(child);
        Ok(true)
    } else {
        Ok(false)
    }
}

fn reap_in_background(mut child: std::process::Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

struct IgnoreSigint(libc::sighandler_t);

impl IgnoreSigint {
    fn new() -> Self {
        Self(unsafe { libc::signal(libc::SIGINT, libc::SIG_IGN) })
    }
}

impl Drop for IgnoreSigint {
    fn drop(&mut self) {
        unsafe {
            libc::signal(libc::SIGINT, self.0);
        }
    }
}

fn which(command: &str) -> Option<PathBuf> {
    if command.contains('/') {
        let path = PathBuf::from(command);
        return path.is_file().then_some(path);
    }
    env::split_paths(&env::var_os("PATH")?)
        .map(|directory| directory.join(command))
        .find(|path| path.is_file())
}

fn basename(command: &str) -> &str {
    Path::new(command)
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_words_keep_quoted_editor_arguments() {
        let config = AppConfig {
            editor: Some(EditorCommand::String("nvim -c 'set number'".into())),
            editor_mode: EditorMode::Tty,
            exclude: vec![],
            include_hidden: false,
            recursive: true,
            open_editor: true,
            plan_format: crate::config::PlanFormat::Properties,
        };
        let editor = resolve_editor(&config).unwrap().unwrap();
        assert_eq!(editor.argv, vec!["nvim", "-c", "set number"]);
    }
}
