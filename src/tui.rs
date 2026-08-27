mod preview;

use crate::config::AppConfig;
use crate::controller::{
    ExpectedSession, expected, operation_counts, parse_session_plan, refresh_hashes,
    reset_session_plan,
};
use crate::editor::launch_editor;
use crate::model::{OpKind, Operation, SessionRecord};
use anyhow::Result;
use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use preview::{Preview, color_style};
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Prompt {
    Inactive,
    Search,
    Filter,
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
    let _ = execute!(stdout(), DisableMouseCapture);
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
    let _ = execute!(stdout(), EnableMouseCapture);
    let _ = execute!(stdout(), Hide);
    Ok(TerminalCleanup)
}

struct ViewState {
    operations: Vec<Operation>,
    parse_error: Option<String>,
    preview: Preview,
    confirm: Confirm,
    prompt: Prompt,
    draft: String,
    status: String,
    last_check: Instant,
}

impl ViewState {
    fn from_plan(
        operations: Vec<Operation>,
        parse_error: Option<String>,
        root: &str,
        id_width: usize,
        no_color: bool,
    ) -> Self {
        let preview = Preview::from_operations(&operations, root, id_width, no_color);
        Self {
            operations,
            parse_error,
            preview,
            confirm: Confirm::None,
            prompt: Prompt::Inactive,
            draft: String::new(),
            status: String::new(),
            last_check: Instant::now(),
        }
    }

    fn reload_preview(&mut self, root: &str, id_width: usize, no_color: bool) {
        self.preview
            .reload(&self.operations, root, id_width, no_color);
    }
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    session: &mut SessionRecord,
    config: &AppConfig,
) -> Result<LoopAction> {
    let no_color = std::env::var_os("NO_COLOR").is_some();
    let (operations, parse_error) = load_operations(session);
    let mut state = ViewState::from_plan(
        operations,
        parse_error,
        &session.root,
        session.id_width,
        no_color,
    );
    let mut dirty = true;
    loop {
        if dirty {
            terminal.draw(|frame| render(frame, session, &mut state))?;
            dirty = false;
        }
        let wait = Duration::from_millis(300).saturating_sub(state.last_check.elapsed());
        if event::poll(wait)? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    if let Some(action) = handle_key(key, &mut state) {
                        if matches!(action, LoopAction::Reset) {
                            match reset_session_plan(session, config.plan_format) {
                                Ok(()) => {
                                    let loaded = load_operations(session);
                                    state.operations = loaded.0;
                                    state.parse_error = loaded.1;
                                    state.draft.clear();
                                    state.prompt = Prompt::Inactive;
                                    state.reload_preview(&session.root, session.id_width, no_color);
                                    state.preview.reset_view();
                                    state.confirm = Confirm::None;
                                    state.status = "plan reset".into();
                                }
                                Err(error) => {
                                    state.confirm = Confirm::None;
                                    state.status = format!("reset failed: {error:#}");
                                }
                            }
                            dirty = true;
                        } else {
                            return Ok(action);
                        }
                    } else {
                        dirty = true;
                    }
                }
                Event::Resize(_, _) => dirty = true,
                Event::Mouse(mouse) => {
                    handle_mouse(mouse, &mut state);
                    dirty = true;
                }
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
                    state.reload_preview(&session.root, session.id_width, no_color);
                    state.status = "plan reloaded".into();
                    state.confirm = Confirm::None;
                    dirty = true;
                }
                Ok(false) => {}
                Err(error) => {
                    state.parse_error = Some(format!("{error:#}"));
                    dirty = true;
                }
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

fn handle_mouse(mouse: MouseEvent, state: &mut ViewState) {
    match mouse.kind {
        MouseEventKind::ScrollDown => state.preview.scroll_lines(3),
        MouseEventKind::ScrollUp => state.preview.scroll_lines(-3),
        MouseEventKind::ScrollRight => state.preview.pan(1),
        MouseEventKind::ScrollLeft => state.preview.pan(-1),
        _ => {}
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
    if state.prompt != Prompt::Inactive {
        return handle_prompt_key(key, state);
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('d') => {
                state.preview.scroll_half_pages(1);
                return None;
            }
            KeyCode::Char('u') => {
                state.preview.scroll_half_pages(-1);
                return None;
            }
            _ => return None,
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
        KeyCode::Char('/') => {
            state.prompt = Prompt::Search;
            state.draft = state.preview.search().to_owned();
            state.status.clear();
            None
        }
        KeyCode::Char('f') => {
            state.prompt = Prompt::Filter;
            state.draft = state.preview.filter().to_owned();
            state.status.clear();
            None
        }
        KeyCode::Char('n') => {
            state.status = state.preview.jump_match(true);
            None
        }
        KeyCode::Char('N') => {
            state.status = state.preview.jump_match(false);
            None
        }
        KeyCode::Char('?') => {
            state.status =
                "j/k space/PgUp/Dn g/G C-d/u Home/End  h/l  / search  n/N  f filter  mouse wheel"
                    .into();
            None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.preview.scroll_lines(1);
            None
        }
        KeyCode::Up | KeyCode::Char('k') => {
            state.preview.scroll_lines(-1);
            None
        }
        KeyCode::PageDown | KeyCode::Char(' ') => {
            state.preview.scroll_pages(1);
            None
        }
        KeyCode::PageUp => {
            state.preview.scroll_pages(-1);
            None
        }
        KeyCode::Home | KeyCode::Char('g') => {
            state.preview.goto_start();
            None
        }
        KeyCode::End | KeyCode::Char('G') => {
            state.preview.goto_end();
            None
        }
        KeyCode::Right | KeyCode::Char('l') => {
            state.preview.pan(1);
            None
        }
        KeyCode::Left | KeyCode::Char('h') => {
            state.preview.pan(-1);
            None
        }
        _ => None,
    }
}

fn handle_prompt_key(key: KeyEvent, state: &mut ViewState) -> Option<LoopAction> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('d') => state.preview.scroll_half_pages(1),
            KeyCode::Char('u') => state.preview.scroll_half_pages(-1),
            _ => {}
        }
        return None;
    }
    match key.code {
        KeyCode::Esc => {
            match state.prompt {
                Prompt::Search => state.preview.clear_search(),
                Prompt::Filter => state.preview.clear_filter(),
                Prompt::Inactive => {}
            }
            state.draft.clear();
            state.prompt = Prompt::Inactive;
            state.status.clear();
        }
        KeyCode::Enter => {
            state.prompt = Prompt::Inactive;
        }
        KeyCode::Backspace => {
            state.draft.pop();
            apply_draft(state);
        }
        KeyCode::Down => state.preview.scroll_lines(1),
        KeyCode::Up => state.preview.scroll_lines(-1),
        KeyCode::PageDown => state.preview.scroll_pages(1),
        KeyCode::PageUp => state.preview.scroll_pages(-1),
        KeyCode::Home => state.preview.goto_start(),
        KeyCode::End => state.preview.goto_end(),
        KeyCode::Right => state.preview.pan(1),
        KeyCode::Left => state.preview.pan(-1),
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            state.draft.push(c);
            apply_draft(state);
        }
        _ => {}
    }
    None
}

fn apply_draft(state: &mut ViewState) {
    match state.prompt {
        Prompt::Search => {
            state.status = state.preview.set_search(state.draft.clone());
        }
        Prompt::Filter => {
            state.preview.set_filter(state.draft.clone());
        }
        Prompt::Inactive => {}
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
        header_keys(state),
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

    state.preview.set_viewport(
        chunks[1].height.max(1) as usize,
        chunks[1].width.max(1) as usize,
    );
    let lines = state.preview.visible_lines(no_color);
    frame.render_widget(Paragraph::new(Text::from(lines)), chunks[1]);

    frame.render_widget(
        Paragraph::new(Span::styled(
            footer(state),
            Style::default().add_modifier(Modifier::DIM),
        )),
        chunks[2],
    );
}

fn header_keys(state: &ViewState) -> Line<'static> {
    match state.prompt {
        Prompt::Search => prompt_line("Search", &state.draft),
        Prompt::Filter => prompt_line("Filter", &state.draft),
        Prompt::Inactive => Line::from(
            "[e] execute  [o] editor  [r] reset  [c/q] cancel  [/] search  [f] filter  [?] keys",
        ),
    }
}

fn prompt_line(label: &str, draft: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw(format!("{label}: {draft}")),
        Span::styled("█", Style::default().add_modifier(Modifier::REVERSED)),
        Span::styled(
            "  Enter keep  Esc clear",
            Style::default().add_modifier(Modifier::DIM),
        ),
    ])
}

fn footer(state: &ViewState) -> String {
    if !state.status.is_empty() {
        return state.status.clone();
    }
    if state.prompt != Prompt::Inactive {
        return "type to match  arrows scroll".into();
    }
    let preview = &state.preview;
    let total = preview.row_count();
    if total == 0 {
        return if preview.filter().is_empty() {
            String::new()
        } else {
            "no rows match filter".into()
        };
    }
    let start = preview.scroll() + 1;
    let end = (preview.scroll() + preview.page()).min(total);
    let mut parts = vec![format!("line {start}–{end} / {total}")];
    if !preview.filter().is_empty() {
        parts.push(format!("filter {}", preview.filter()));
    }
    if !preview.search().is_empty() {
        if preview.match_count() == 0 {
            parts.push("no matches".into());
        } else {
            parts.push(format!(
                "match {} / {}",
                preview.match_index() + 1,
                preview.match_count()
            ));
        }
    }
    if preview.max_line_chars() > preview.list_width() {
        parts.push(format!("col {}", preview.h_scroll() + 1));
    }
    parts.join("  ·  ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(id: u64, kind: OpKind, from: &str, to: &str) -> Operation {
        Operation {
            id,
            from: from.to_owned(),
            to: to.to_owned(),
            kind,
        }
    }

    fn sample_ops() -> Vec<Operation> {
        vec![
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
        ]
    }

    fn state_from(operations: Vec<Operation>) -> ViewState {
        let mut state = ViewState::from_plan(operations, None, "/music", 4, true);
        state.preview.set_viewport(10, 40);
        state
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn g_and_end_jump_the_list() {
        let mut state = state_from(sample_ops());
        state.preview.set_viewport(2, 40);
        assert_eq!(handle_key(key(KeyCode::Char('G')), &mut state), None);
        assert_eq!(
            state.preview.scroll(),
            state.preview.row_count().saturating_sub(2)
        );
        assert_eq!(handle_key(key(KeyCode::Char('g')), &mut state), None);
        assert_eq!(state.preview.scroll(), 0);
        assert_eq!(handle_key(key(KeyCode::End), &mut state), None);
        assert_eq!(
            state.preview.scroll(),
            state.preview.row_count().saturating_sub(2)
        );
        assert_eq!(handle_key(key(KeyCode::Home), &mut state), None);
        assert_eq!(state.preview.scroll(), 0);
    }

    #[test]
    fn ctrl_d_and_u_move_half_page() {
        let mut state = state_from(sample_ops());
        state.preview.set_viewport(2, 40);
        handle_key(ctrl('d'), &mut state);
        assert_eq!(state.preview.scroll(), 1);
        handle_key(ctrl('u'), &mut state);
        assert_eq!(state.preview.scroll(), 0);
    }

    #[test]
    fn slash_starts_search_and_n_walks_matches() {
        let mut state = state_from(sample_ops());
        assert_eq!(handle_key(key(KeyCode::Char('/')), &mut state), None);
        assert_eq!(state.prompt, Prompt::Search);
        handle_key(key(KeyCode::Char('o')), &mut state);
        handle_key(key(KeyCode::Char('l')), &mut state);
        handle_key(key(KeyCode::Char('d')), &mut state);
        assert!(state.preview.match_count() > 0);
        handle_key(key(KeyCode::Enter), &mut state);
        assert_eq!(state.prompt, Prompt::Inactive);
        let first = state.preview.match_index();
        handle_key(key(KeyCode::Char('n')), &mut state);
        assert_ne!(state.preview.match_index(), first);
    }

    #[test]
    fn filter_prompt_hides_non_matching_rows() {
        let mut state = state_from(sample_ops());
        handle_key(key(KeyCode::Char('f')), &mut state);
        for c in ['g', 'o', 'n', 'e'] {
            handle_key(key(KeyCode::Char(c)), &mut state);
        }
        assert_eq!(state.preview.filter(), "gone");
        assert_eq!(state.preview.row_count(), 2);
        handle_key(key(KeyCode::Esc), &mut state);
        assert!(state.preview.filter().is_empty());
        assert_eq!(state.preview.row_count(), 5);
    }

    #[test]
    fn execute_still_sees_unfiltered_trash() {
        let mut state = state_from(sample_ops());
        state.preview.set_filter("old-one".into());
        assert_eq!(state.preview.row_count(), 2);
        assert_eq!(handle_key(key(KeyCode::Char('e')), &mut state), None);
        assert_eq!(state.confirm, Confirm::Trash);
    }
}
