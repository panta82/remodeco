use crate::config::AppConfig;
use crate::controller::{
    ExpectedSession, expected, operation_counts, parse_session_plan, refresh_hashes,
};
use crate::diff::{DiffPart, DiffTag, path_diff};
use crate::editor::launch_editor;
use crate::model::{OpKind, Operation, SessionRecord};
use anyhow::Result;
use crossterm::cursor::{MoveToColumn, MoveToNextLine, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};
use std::io::Write;
use std::io::{self, stdout};
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub enum TuiAction {
    Execute(ExpectedSession),
    Cancel,
}

enum LoopAction {
    Execute,
    Cancel,
    OpenEditor,
}

pub fn run_tui(session: &mut SessionRecord, config: &AppConfig) -> Result<TuiAction> {
    loop {
        match run_once(session)? {
            LoopAction::Execute => {
                // Bind execution to exactly what was rendered and confirmed.
                // If the editor wrote between the last poll and this keypress,
                // reload and require confirmation again.
                if refresh_hashes(session)? {
                    continue;
                }
                return Ok(TuiAction::Execute(expected(session)));
            }
            LoopAction::Cancel => return Ok(TuiAction::Cancel),
            LoopAction::OpenEditor => {
                launch_editor(config, Path::new(&session.plan_path))?;
                refresh_hashes(session)?;
            }
        }
    }
}

fn run_once(session: &mut SessionRecord) -> Result<LoopAction> {
    let (_, rows) = size()?;
    let height = rows.saturating_sub(1).clamp(1, 22);
    enable_raw_mode()?;
    let raw_mode = RawModeGuard;
    let backend = CrosstermBackend::new(stdout());
    let terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(height),
        },
    );
    let mut terminal = match terminal {
        Ok(terminal) => terminal,
        Err(_) => {
            drop(raw_mode);
            return plain_prompt(session);
        }
    };
    let result = event_loop(&mut terminal, session);
    let _ = terminal.show_cursor();
    drop(terminal);
    drop(raw_mode);
    let _ = execute!(stdout(), Show, MoveToColumn(0), MoveToNextLine(1));
    result
}

fn plain_prompt(session: &mut SessionRecord) -> Result<LoopAction> {
    let (operations, parse_error) = load_operations(session);
    println!("remodeco {} · {}", session.mode, session.status);
    println!("{}", session.plan_path);
    if let Some(error) = parse_error {
        println!("plan error: {error}");
    } else {
        for operation in operations
            .iter()
            .filter(|operation| !matches!(operation.kind, OpKind::Noop | OpKind::Skip))
        {
            match operation.kind {
                OpKind::Trash => println!("{{{}}} - {}", operation.id, operation.from),
                OpKind::Move | OpKind::Copy => {
                    println!("{{{}}} - {}", operation.id, operation.from);
                    println!("      + {}", operation.to);
                }
                OpKind::Noop | OpKind::Skip => {}
            }
        }
    }
    print!("[e] execute  [o] editor  [c/q] cancel > ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    match input.trim().chars().next().unwrap_or('c') {
        'e' => {
            let latest = match parse_session_plan(session) {
                Ok(operations) => operations,
                Err(error) => {
                    println!("plan error: {error:#}");
                    return Ok(LoopAction::OpenEditor);
                }
            };
            if latest
                .iter()
                .any(|operation| operation.kind == OpKind::Trash)
            {
                print!("Trash planned items? y/N > ");
                io::stdout().flush()?;
                input.clear();
                io::stdin().read_line(&mut input)?;
                if !matches!(input.trim().chars().next(), Some('y' | 'Y')) {
                    return Ok(LoopAction::Cancel);
                }
            }
            Ok(LoopAction::Execute)
        }
        'o' => Ok(LoopAction::OpenEditor),
        _ => Ok(LoopAction::Cancel),
    }
}

struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), Show);
    }
}

struct ViewState {
    operations: Vec<Operation>,
    parse_error: Option<String>,
    scroll: u16,
    confirm_trash: bool,
    status: String,
    last_check: Instant,
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    session: &mut SessionRecord,
) -> Result<LoopAction> {
    let (operations, parse_error) = load_operations(session);
    let mut state = ViewState {
        operations,
        parse_error,
        scroll: 0,
        confirm_trash: false,
        status: String::new(),
        last_check: Instant::now(),
    };
    loop {
        terminal.draw(|frame| render(frame, session, &state))?;
        if event::poll(Duration::from_millis(150))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Release {
                    if let Some(action) = handle_key(key, &mut state) {
                        return Ok(action);
                    }
                }
            }
        }
        if state.last_check.elapsed() >= Duration::from_millis(300) {
            state.last_check = Instant::now();
            match refresh_hashes(session) {
                Ok(true) => {
                    let loaded = load_operations(session);
                    state.operations = loaded.0;
                    state.parse_error = loaded.1;
                    state.status = "plan reloaded".into();
                    state.confirm_trash = false;
                }
                Ok(false) => {}
                Err(error) => state.parse_error = Some(format!("{error:#}")),
            }
        }
    }
}

fn load_operations(session: &SessionRecord) -> (Vec<Operation>, Option<String>) {
    match parse_session_plan(session) {
        Ok(operations) => (operations, None),
        Err(error) => (Vec::new(), Some(format!("{error:#}"))),
    }
}

fn handle_key(key: KeyEvent, state: &mut ViewState) -> Option<LoopAction> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Some(LoopAction::Cancel);
    }
    if state.confirm_trash {
        match key.code {
            KeyCode::Char('y' | 'Y') => return Some(LoopAction::Execute),
            _ => {
                state.confirm_trash = false;
                state.status = "execution cancelled".into();
                return None;
            }
        }
    }
    match key.code {
        KeyCode::Esc | KeyCode::Char('c' | 'q') => Some(LoopAction::Cancel),
        KeyCode::Char('o') => Some(LoopAction::OpenEditor),
        KeyCode::Char('e') if state.parse_error.is_none() => {
            if state
                .operations
                .iter()
                .any(|operation| operation.kind == OpKind::Trash)
            {
                state.confirm_trash = true;
                None
            } else {
                Some(LoopAction::Execute)
            }
        }
        KeyCode::Char('e') => {
            state.status = "fix the plan error before executing".into();
            None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.scroll = state.scroll.saturating_add(1);
            None
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.scroll = state.scroll.saturating_sub(1);
            None
        }
        KeyCode::PageDown => {
            state.scroll = state.scroll.saturating_add(8);
            None
        }
        KeyCode::PageUp => {
            state.scroll = state.scroll.saturating_sub(8);
            None
        }
        KeyCode::Home => {
            state.scroll = 0;
            None
        }
        _ => None,
    }
}

fn render(frame: &mut Frame<'_>, session: &SessionRecord, state: &ViewState) {
    let no_color = std::env::var_os("NO_COLOR").is_some();
    let chunks = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .split(frame.area());
    let (changes, trash) = operation_counts(&state.operations);
    let title = format!(
        "remodeco {} · {} · {} change{} / {} file{}",
        session.mode,
        session.status,
        changes,
        if changes == 1 { "" } else { "s" },
        state.operations.len(),
        if state.operations.len() == 1 { "" } else { "s" },
    );
    let mut header = vec![
        Line::from(Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            session.plan_path.clone(),
            Style::default().add_modifier(Modifier::DIM),
        )),
        Line::from("[e] execute  [o] editor  [c/q] cancel  [j/k] scroll"),
    ];
    if state.confirm_trash {
        header.push(Line::from(Span::styled(
            format!(
                "Trash {trash} item{}? y/N",
                if trash == 1 { "" } else { "s" }
            ),
            color_style(Color::Yellow, no_color).add_modifier(Modifier::BOLD),
        )));
    } else if let Some(error) = &state.parse_error {
        header.push(Line::from(Span::styled(
            error.clone(),
            color_style(Color::Red, no_color),
        )));
    } else if changes == 0 {
        header.push(Line::from(Span::styled(
            "No operations — edit the plan with o.",
            Style::default().add_modifier(Modifier::DIM),
        )));
    } else {
        header.push(Line::default());
    }
    frame.render_widget(Paragraph::new(header).wrap(Wrap { trim: false }), chunks[0]);

    let lines = operation_lines(&state.operations, no_color);
    let line_count = lines.len();
    let max_scroll = line_count.saturating_sub(chunks[1].height as usize) as u16;
    let scroll = state.scroll.min(max_scroll);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        chunks[1],
    );

    let footer = if state.status.is_empty() {
        if max_scroll > 0 {
            format!("line {} / {}", scroll + 1, line_count)
        } else {
            String::new()
        }
    } else {
        state.status.clone()
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            footer,
            Style::default().add_modifier(Modifier::DIM),
        )),
        chunks[2],
    );
}

fn operation_lines(operations: &[Operation], no_color: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for operation in operations
        .iter()
        .filter(|operation| !matches!(operation.kind, OpKind::Noop | OpKind::Skip))
    {
        match operation.kind {
            OpKind::Trash => lines.push(Line::from(vec![
                Span::styled(
                    format!("{{{}}} - ", operation.id),
                    color_style(Color::Red, no_color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(operation.from.clone(), color_style(Color::Red, no_color)),
            ])),
            OpKind::Move | OpKind::Copy => {
                let (old, new) = path_diff(&operation.from, &operation.to);
                let prefix = format!("{{{}}}", operation.id);
                let mut old_spans = vec![Span::styled(
                    format!("{prefix} - "),
                    color_style(Color::Red, no_color).add_modifier(Modifier::BOLD),
                )];
                old_spans.extend(parts_to_spans(old, no_color));
                let mut new_spans = vec![Span::styled(
                    format!("{} + ", " ".repeat(prefix.len())),
                    color_style(Color::Green, no_color).add_modifier(Modifier::BOLD),
                )];
                new_spans.extend(parts_to_spans(new, no_color));
                lines.push(Line::from(old_spans));
                lines.push(Line::from(new_spans));
            }
            OpKind::Noop | OpKind::Skip => {}
        }
    }
    lines
}

fn parts_to_spans(parts: Vec<DiffPart>, no_color: bool) -> Vec<Span<'static>> {
    parts
        .into_iter()
        .map(|part| {
            let style = match part.tag {
                DiffTag::Equal => Style::default().add_modifier(Modifier::DIM),
                DiffTag::Delete => color_style(Color::Red, no_color).add_modifier(Modifier::BOLD),
                DiffTag::Insert => color_style(Color::Green, no_color).add_modifier(Modifier::BOLD),
            };
            Span::styled(part.text, style)
        })
        .collect()
}

fn color_style(color: Color, no_color: bool) -> Style {
    if no_color {
        Style::default()
    } else {
        Style::default().fg(color)
    }
}
