use anyhow::{Context, Result, bail};
use clap::Parser;
use remodeco::cli::{Cli, Command};
use remodeco::config::load_config;
use remodeco::controller::{
    create_session, dry_run, execute_session, normalize_session_plan, open_session, refresh_hashes,
    undo_session,
};
use remodeco::editor::launch_editor;
use remodeco::model::{JournalFinalStatus, SessionMode};
use remodeco::schedule::format_schedule;
use remodeco::session::{list_session_ids, list_unfinished, read_session, write_session};
use remodeco::tui::{TuiAction, run_tui};
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

    if cli.list_sessions {
        for id in list_session_ids()? {
            match read_session(&id) {
                Ok(session) => println!(
                    "{}\t{}\t{}\t{}",
                    session.id, session.status, session.mode, session.root
                ),
                Err(_) => println!("{id}\t(unreadable)"),
            }
        }
        return Ok(());
    }

    if let Some(Command::Undo { session_id }) = &cli.command {
        let mut opened = open_session(session_id)?;
        let journal = undo_session(&mut opened.session)?;
        if journal.final_status == Some(JournalFinalStatus::Undone) {
            println!("undone {}", journal.journal_id);
            return Ok(());
        }
        bail!("undo interrupted {}", journal.journal_id);
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
        let unfinished = if cli.new_session {
            Vec::new()
        } else {
            list_unfinished(Some(&root_text))?
        };
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
    let journal = execute_session(&mut opened.session, &confirmed)?;
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

fn path_text(path: &Path) -> Result<String> {
    Ok(path.to_str().context("path is not valid UTF-8")?.to_owned())
}

fn require_supported_platform() -> Result<()> {
    if cfg!(any(target_os = "linux", target_os = "macos")) {
        Ok(())
    } else {
        bail!("remodeco supports Linux and macOS")
    }
}
