use anyhow::{Context, Result, bail};
use chrono::DateTime;
use clap::Parser;
use comfy_table::{Table, presets::UTF8_FULL_CONDENSED};
use remodeco::cli::{Cli, Command};
use remodeco::config::load_config;
use remodeco::controller::{
    create_session, delete_session, dry_run, execute_prepared_session, execute_session,
    normalize_session_plan, open_session, refresh_hashes, undo_session,
};
use remodeco::editor::launch_editor;
use remodeco::execute::StdioExecuteUi;
use remodeco::model::{JournalFinalStatus, SessionMode};
use remodeco::schedule::format_schedule;
use remodeco::store::{list_session_ids, list_unfinished, read_session, write_session};
use remodeco::tui::{TuiAction, run_tui};
use remodeco::util::path_text;
use std::fs;
use std::io::{IsTerminal, stdin, stdout};
use std::path::{Path, PathBuf};

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    require_supported_platform()?;
    if cli.verbose {
        eprintln!(
            "remodeco {} on {}-{}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        );
    }

    if let Some(command) = &cli.command {
        match command {
            Command::List {} => {
                let mut table = Table::new();
                table.load_preset(UTF8_FULL_CONDENSED);
                table.set_header(vec!["ID", "Status", "Root", "Created", "Changes"]);

                for id in list_session_ids()? {
                    match read_session(&id) {
                        Ok(session) => {
                            let date_str = DateTime::parse_from_rfc3339(&session.created_at)
                                .map(|date| date.format("%Y-%m-%d %H:%M:%S").to_string())
                                .unwrap_or("".to_string());

                            table.add_row(vec![
                                session.id,
                                session.status.to_string(),
                                session.root,
                                date_str,
                                session.stats.changes.to_string(),
                            ]);
                        }
                        Err(_) => {
                            table.add_row(vec![
                                id,
                                "corrupted".to_string(),
                                "".to_string(),
                                "".to_string(),
                                "".to_string(),
                            ]);
                        }
                    }
                }

                println!("{table}");
                return Ok(());
            }

            Command::Delete { session_id, force } => {
                delete_session(session_id, *force)?;
                println!("Session deleted: {session_id}");
                return Ok(());
            }

            Command::Execute { session_id } => {
                let mut opened = open_session(session_id)?;
                let mut ui = StdioExecuteUi;
                let journal = execute_prepared_session(&mut opened.session, &mut ui)?;
                if journal.final_status == Some(JournalFinalStatus::Executed) {
                    println!("executed {}", journal.journal_id);
                    return Ok(());
                }
                let error = journal
                    .steps
                    .iter()
                    .find_map(|step| step.error.as_deref())
                    .unwrap_or("unknown failure");
                bail!("execution interrupted: {error}");
            }
            Command::Undo { session_id } => {
                let mut opened = open_session(session_id)?;
                let mut ui = StdioExecuteUi;
                let journal = undo_session(&mut opened.session, &mut ui)?;
                if journal.final_status == Some(JournalFinalStatus::Undone) {
                    println!("undone {}", journal.journal_id);
                    return Ok(());
                }
                bail!("undo interrupted {}", journal.journal_id);
            }
        }
    }

    let is_tty = stdin().is_terminal() && stdout().is_terminal();
    let requested_root = canonical_root(cli.directory.as_deref())?;
    let mut opened = if let Some(id) = &cli.session {
        let opened = open_session(id)?;
        if let Some(root) = &requested_root {
            let root = path_text(root)?;
            if root != opened.session.root {
                eprintln!(
                    "warning: session root is {}; ignoring {root}",
                    opened.session.root
                );
            }
        }
        opened
    } else {
        let root = requested_root.unwrap_or(std::env::current_dir()?.canonicalize()?);
        let root_text = path_text(&root)?;
        let mut unfinished = if cli.new_session {
            Vec::new()
        } else {
            list_unfinished(Some(&root_text))?
        };
        // The TUI can only execute, so a session stuck mid-undo would dead-end there.
        // Name it and start fresh instead; `--session <id>` still reaches it.
        unfinished.retain(|session| {
            if session.status.executable() {
                return true;
            }
            eprintln!(
                "skipping {} ({}); use --session {} to inspect it",
                session.id, session.status, session.id
            );
            false
        });
        if let Some(first) = unfinished.first() {
            for other in unfinished.iter().skip(1) {
                eprintln!(
                    "also unfinished: {} ({}, updated {}) — use --session {}",
                    other.id, other.status, other.updated_at, other.id
                );
            }
            eprintln!("resuming {}", first.id);
            open_session(&first.id)?
        } else {
            let config = load_config(&root, &cli)?;
            create_session(
                &root,
                if cli.copy {
                    SessionMode::Copy
                } else {
                    SessionMode::Move
                },
                &config,
                cli.yes,
            )?
        }
    };
    let _session_lock = &opened.lock;
    if cli.verbose {
        eprintln!(
            "session {} · {} · {}",
            opened.session.id, opened.session.mode, opened.session.root
        );
    }
    if cli.copy && opened.session.mode != SessionMode::Copy {
        if !is_tty {
            bail!("--copy does not match stored session mode");
        }
        opened.session.mode = SessionMode::Copy;
        opened.session.revision += 1;
        write_session(&mut opened.session)?;
    }
    let config = load_config(Path::new(&opened.session.root), &cli)?;
    normalize_session_plan(&mut opened.session, config.plan_format)?;
    refresh_hashes(&mut opened.session)?;
    launch_editor(&config, Path::new(&opened.session.plan_path))?;
    refresh_hashes(&mut opened.session)?;

    if cli.dry_run {
        let scheduled = dry_run(&opened.session)?;
        if !scheduled.errors.is_empty() {
            bail!(scheduled.errors.join("\n"));
        }
        println!("{}", format_schedule(&scheduled));
        return Ok(());
    }
    if !is_tty {
        eprintln!("plan: {}", opened.session.plan_path);
        bail!("non-TTY: use --dry-run, or run on a TTY to execute");
    }
    let confirmed = match run_tui(&mut opened.session, &config)? {
        TuiAction::Cancel => return Ok(()),
        TuiAction::Execute(confirmed) => confirmed,
    };
    refresh_hashes(&mut opened.session)?;
    let mut ui = StdioExecuteUi;
    let journal = execute_session(&mut opened.session, &confirmed, &mut ui)?;
    if journal.final_status == Some(JournalFinalStatus::Executed) {
        println!("executed {}", journal.journal_id);
        Ok(())
    } else {
        let error = journal
            .steps
            .iter()
            .find_map(|step| step.error.as_deref())
            .unwrap_or("unknown failure");
        bail!("execution interrupted: {error}")
    }
}

fn canonical_root(directory: Option<&Path>) -> Result<Option<PathBuf>> {
    directory
        .map(|path| fs::canonicalize(path).with_context(|| format!("resolve {}", path.display())))
        .transpose()
}

fn require_supported_platform() -> Result<()> {
    if cfg!(any(target_os = "linux", target_os = "macos")) {
        Ok(())
    } else {
        bail!("remodeco supports Linux and macOS")
    }
}
