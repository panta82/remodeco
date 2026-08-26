use crate::config::AppConfig;
use crate::controller::{
    ExpectedSession, expected, operation_counts, parse_session_plan, refresh_hashes,
    reset_session_plan,
};
use crate::diff::{DiffPart, DiffTag, path_diff_inline};
use crate::editor::launch_editor;
use crate::model::{OpKind, Operation, SessionRecord};
use anyhow::Result;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use std::io::Write;
use std::io::{self, stdout};
use std::path::Path;
use std::sync::Once;
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
    Reset,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Confirm {
    None,
    Trash,
    Reset,
}

pub fn run_tui(session: &mut SessionRecord, config: &AppConfig) -> Result<TuiAction> {
    loop {
        match run_once(session, config)? {
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
            LoopAction::Reset => {}
        }
    }
}

fn run_once(session: &mut SessionRecord, config: &AppConfig) -> Result<LoopAction> {
    install_panic_hook();
    let cleanup = match enter_fullscreen() {
        Ok(cleanup) => cleanup,
        Err(_) => return plain_prompt(session, config),
    };
    let mut terminal = match Terminal::new(CrosstermBackend::new(stdout())) {
        Ok(terminal) => terminal,
        Err(_) => {
            drop(cleanup);
            return plain_prompt(session, config);
        }
    };
    let result = event_loop(&mut terminal, session, config);
    drop(terminal);
    drop(cleanup);
    result
}

fn plain_prompt(session: &mut SessionRecord, config: &AppConfig) -> Result<LoopAction> {
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
                OpKind::Trash => println!(
                    "{:0width$}  {}",
                    operation.id,
                    operation.from,
                    width = session.id_width.max(1)
                ),
                OpKind::Move | OpKind::Copy => {
                    println!(
                        "{:0width$}  {} -> {}",
                        operation.id,
                        operation.from,
                        operation.to,
                        width = session.id_width.max(1)
                    );
                }
                OpKind::Noop | OpKind::Skip => {}
            }
        }
    }
    loop {
        print!("[e] execute  [o] editor  [r] reset  [c/q] cancel > ");
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
                return Ok(LoopAction::Execute);
            }
            'o' => return Ok(LoopAction::OpenEditor),
            'r' => {
                print!("Reset plan to original paths? y/N > ");
                io::stdout().flush()?;
                input.clear();
                io::stdin().read_line(&mut input)?;
                if matches!(input.trim().chars().next(), Some('y' | 'Y')) {
                    reset_session_plan(session, config.plan_format)?;
                    println!("plan reset");
                    return Ok(LoopAction::Reset);
                }
                println!("reset cancelled");
            }
            _ => return Ok(LoopAction::Cancel),
        }
    }
}

struct TerminalCleanup;

impl Drop for TerminalCleanup {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen);
    let _ = execute!(stdout(), Show);
}

fn install_panic_hook() {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let original = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            original(info);
        }));
    });
}

fn enter_fullscreen() -> io::Result<TerminalCleanup> {
    enable_raw_mode()?;
    if let Err(error) = execute!(stdout(), EnterAlternateScreen) {
        let _ = disable_raw_mode();
        return Err(error);
    }
    let _ = execute!(stdout(), Hide);
    Ok(TerminalCleanup)
}

struct ViewState {
    operations: Vec<Operation>,
    parse_error: Option<String>,
    scroll: u16,
    page: u16,
    confirm: Confirm,
    status: String,
    last_check: Instant,
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    session: &mut SessionRecord,
    config: &AppConfig,
) -> Result<LoopAction> {
    let (operations, parse_error) = load_operations(session);
    let mut state = ViewState {
        operations,
        parse_error,
        scroll: 0,
        page: 1,
        confirm: Confirm::None,
        status: String::new(),
        last_check: Instant::now(),
    };
    loop {
        terminal.draw(|frame| render(frame, session, &mut state))?;
        if event::poll(Duration::from_millis(150))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    if let Some(action) = handle_key(key, &mut state) {
                        if matches!(action, LoopAction::Reset) {
                            match reset_session_plan(session, config.plan_format) {
                                Ok(()) => {
                                    let loaded = load_operations(session);
                                    state.operations = loaded.0;
                                    state.parse_error = loaded.1;
                                    state.scroll = 0;
                                    state.confirm = Confirm::None;
                                    state.status = "plan reset".into();
                                }
                                Err(error) => {
                                    state.confirm = Confirm::None;
                                    state.status = format!("reset failed: {error:#}");
                                }
                            }
                        } else {
                            return Ok(action);
                        }
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
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
                    state.confirm = Confirm::None;
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
    if state.confirm != Confirm::None {
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                return match state.confirm {
                    Confirm::Trash => Some(LoopAction::Execute),
                    Confirm::Reset => Some(LoopAction::Reset),
                    Confirm::None => None,
                };
            }
            _ => {
                state.status = match state.confirm {
                    Confirm::Trash => "execution cancelled".into(),
                    Confirm::Reset => "reset cancelled".into(),
                    Confirm::None => state.status.clone(),
                };
                state.confirm = Confirm::None;
                return None;
            }
        }
    }
    match key.code {
        KeyCode::Esc | KeyCode::Char('c' | 'q') => Some(LoopAction::Cancel),
        KeyCode::Char('o') => Some(LoopAction::OpenEditor),
        KeyCode::Char('r') => {
            state.confirm = Confirm::Reset;
            None
        }
        KeyCode::Char('e') if state.parse_error.is_none() => {
            if state
                .operations
                .iter()
                .any(|operation| operation.kind == OpKind::Trash)
            {
                state.confirm = Confirm::Trash;
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
            state.scroll = state.scroll.saturating_add(state.page.max(1));
            None
        }
        KeyCode::PageUp => {
            state.scroll = state.scroll.saturating_sub(state.page.max(1));
            None
        }
        KeyCode::Home => {
            state.scroll = 0;
            None
        }
        KeyCode::End => {
            state.scroll = u16::MAX;
            None
        }
        _ => None,
    }
}

fn render(frame: &mut Frame<'_>, session: &SessionRecord, state: &mut ViewState) {
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
        Line::from("[e] execute  [o] editor  [r] reset  [c/q] cancel  [j/k] scroll"),
    ];
    if state.confirm == Confirm::Trash {
        header.push(Line::from(Span::styled(
            format!(
                "Trash {trash} item{}? y/N",
                if trash == 1 { "" } else { "s" }
            ),
            color_style(Color::Yellow, no_color).add_modifier(Modifier::BOLD),
        )));
    } else if state.confirm == Confirm::Reset {
        header.push(Line::from(Span::styled(
            if changes == 0 {
                "Reset plan to original paths? y/N".into()
            } else {
                format!(
                    "Reset {changes} change{} to original paths? y/N",
                    if changes == 1 { "" } else { "s" }
                )
            },
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

    let lines = operation_lines(
        &state.operations,
        &session.root,
        session.id_width,
        chunks[1].width,
        no_color,
    );
    let line_count = lines.len();
    let list_height = chunks[1].height.max(1);
    state.page = list_height;
    let max_scroll = line_count.saturating_sub(list_height as usize) as u16;
    state.scroll = state.scroll.min(max_scroll);
    let scroll = state.scroll;
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

fn operation_lines(
    operations: &[Operation],
    root: &str,
    id_width: usize,
    width: u16,
    no_color: bool,
) -> Vec<Line<'static>> {
    let visible: Vec<&Operation> = operations
        .iter()
        .filter(|operation| !matches!(operation.kind, OpKind::Noop | OpKind::Skip))
        .collect();
    let mut groups: Vec<(String, Vec<&Operation>)> = Vec::new();
    for operation in visible {
        let directory = parent_relative(&operation.from, root);
        match groups.last_mut() {
            Some((current, items)) if current == &directory => items.push(operation),
            _ => groups.push((directory, vec![operation])),
        }
    }
    let mut lines = Vec::new();
    for (directory, items) in groups {
        lines.push(directory_header(&directory, items.len(), width, no_color));
        for operation in items {
            lines.push(operation_line(operation, root, id_width, no_color));
        }
    }
    lines
}

fn directory_header(directory: &str, count: usize, width: u16, no_color: bool) -> Line<'static> {
    let text = format!(" {directory} {count}");
    let width = width as usize;
    let padded = match width.checked_sub(text.chars().count()) {
        Some(padding) if padding > 0 => format!("{text}{}", " ".repeat(padding)),
        _ => text,
    };
    let style = if no_color {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::LightBlue)
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD)
    };
    Line::from(Span::styled(padded, style))
}

fn operation_line(
    operation: &Operation,
    root: &str,
    id_width: usize,
    no_color: bool,
) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!(" {:0width$}  ", operation.id, width = id_width.max(1)),
        Style::default().add_modifier(Modifier::DIM),
    )];
    match operation.kind {
        OpKind::Trash => {
            spans.push(Span::styled(
                display_path(&operation.from, root),
                color_style(Color::Red, no_color).add_modifier(Modifier::BOLD),
            ));
        }
        OpKind::Move | OpKind::Copy => {
            let from = display_path(&operation.from, root);
            let to = display_path(&operation.to, root);
            spans.extend(parts_to_spans(path_diff_inline(&from, &to), no_color));
        }
        OpKind::Noop | OpKind::Skip => {}
    }
    Line::from(spans)
}

fn parent_relative(path: &str, root: &str) -> String {
    let parent = Path::new(path).parent().unwrap_or_else(|| Path::new("/"));
    if parent == Path::new(root) {
        ".".to_owned()
    } else {
        parent.strip_prefix(root).map_or_else(
            |_| parent.to_string_lossy().into_owned(),
            |relative| relative.to_string_lossy().into_owned(),
        )
    }
}

fn display_path(path: &str, root: &str) -> String {
    let root = root.trim_end_matches('/');
    path.strip_prefix(root)
        .and_then(|rest| rest.strip_prefix('/'))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(path)
        .to_owned()
}

fn parts_to_spans(parts: Vec<DiffPart>, no_color: bool) -> Vec<Span<'static>> {
    parts
        .into_iter()
        .map(|part| {
            let style = match part.tag {
                DiffTag::Equal => Style::default(),
                DiffTag::Delete => color_style(Color::Red, no_color)
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::CROSSED_OUT),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn op(id: u64, kind: OpKind, from: &str, to: &str) -> Operation {
        Operation {
            id,
            from: from.to_owned(),
            to: to.to_owned(),
            kind,
        }
    }

    #[test]
    fn groups_by_directory_with_padded_ids_and_inline_diff() {
        let operations = vec![
            op(
                1,
                OpKind::Move,
                "/music/_NEW20/old-one.mp3",
                "/music/_NEW20/new-one.mp3",
            ),
            op(
                2,
                OpKind::Move,
                "/music/_NEW20/old-two.mp3",
                "/music/_NEW20/new-two.mp3",
            ),
            op(12, OpKind::Trash, "/music/other/gone.mp3", ""),
        ];
        let lines = operation_lines(&operations, "/music", 4, 40, true);
        let text: Vec<String> = lines.iter().map(line_text).collect();
        assert!(text[0].starts_with(" _NEW20 2"));
        assert!(text[1].starts_with(" 0001  "));
        assert!(text[1].contains("old"));
        assert!(text[1].contains("new"));
        assert!(!text[1].contains('\n'));
        assert!(text[2].starts_with(" 0002  "));
        assert!(text[3].starts_with(" other 1"));
        assert_eq!(text[4].trim_start(), "0012  other/gone.mp3");
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn skips_noop_and_skip_ops() {
        let operations = vec![
            op(1, OpKind::Noop, "/music/a.txt", "/music/a.txt"),
            op(2, OpKind::Skip, "/music/b.txt", "/music/b.txt"),
            op(3, OpKind::Move, "/music/old.txt", "/music/new.txt"),
        ];
        let lines = operation_lines(&operations, "/music", 1, 20, true);
        assert_eq!(lines.len(), 2);
        assert!(line_text(&lines[0]).starts_with(" . 1"));
        assert!(line_text(&lines[1]).contains("old"));
        assert!(line_text(&lines[1]).contains("new"));
    }
}
