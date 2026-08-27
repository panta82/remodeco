use crate::diff::{DiffPart, DiffTag, path_diff_inline};
use crate::model::{OpKind, Operation};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use std::path::Path;

#[derive(Clone, Debug)]
enum Row {
    Header { directory: String, count: usize },
    Op(usize),
}

#[derive(Clone, Debug)]
struct PreparedOp {
    directory: String,
    haystack_lower: String,
    line: Line<'static>,
}

#[derive(Clone, Debug)]
pub struct Preview {
    prepared: Vec<PreparedOp>,
    rows: Vec<Row>,
    scroll: usize,
    h_scroll: usize,
    page: usize,
    list_width: usize,
    max_line_chars: usize,
    search: String,
    filter: String,
    matches: Vec<usize>,
    match_i: usize,
}

impl Preview {
    pub fn from_operations(
        operations: &[Operation],
        root: &str,
        id_width: usize,
        no_color: bool,
    ) -> Self {
        let mut preview = Self {
            prepared: Vec::new(),
            rows: Vec::new(),
            scroll: 0,
            h_scroll: 0,
            page: 1,
            list_width: 1,
            max_line_chars: 0,
            search: String::new(),
            filter: String::new(),
            matches: Vec::new(),
            match_i: 0,
        };
        preview.reload(operations, root, id_width, no_color);
        preview
    }

    pub fn reload(
        &mut self,
        operations: &[Operation],
        root: &str,
        id_width: usize,
        no_color: bool,
    ) {
        self.prepared = prepare_ops(operations, root, id_width, no_color);
        self.max_line_chars = self
            .prepared
            .iter()
            .map(|op| line_char_count(&op.line))
            .max()
            .unwrap_or(0);
        self.rebuild_rows();
    }

    pub fn reset_view(&mut self) {
        self.search.clear();
        self.filter.clear();
        self.scroll = 0;
        self.h_scroll = 0;
        self.rebuild_rows();
    }

    pub fn set_viewport(&mut self, height: usize, width: usize) {
        self.page = height.max(1);
        self.list_width = width.max(1);
        self.clamp_scroll();
    }

    pub fn search(&self) -> &str {
        &self.search
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn h_scroll(&self) -> usize {
        self.h_scroll
    }

    pub fn page(&self) -> usize {
        self.page
    }

    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    pub fn match_index(&self) -> usize {
        self.match_i
    }

    pub fn max_line_chars(&self) -> usize {
        self.max_line_chars
    }

    pub fn list_width(&self) -> usize {
        self.list_width
    }

    pub fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        self.scroll = 0;
        self.rebuild_rows();
        if !self.search.is_empty() {
            self.sync_search_from_current();
        }
    }

    pub fn clear_filter(&mut self) {
        self.filter.clear();
        self.rebuild_rows();
    }

    pub fn set_search(&mut self, search: String) -> String {
        self.search = search;
        self.sync_search_from_current()
    }

    pub fn clear_search(&mut self) {
        self.search.clear();
        self.rebuild_matches();
    }

    pub fn scroll_lines(&mut self, delta: isize) {
        apply_delta(&mut self.scroll, delta);
        if delta > 0 {
            self.clamp_scroll();
        }
    }

    pub fn scroll_pages(&mut self, delta: isize) {
        let amount = self.page.max(1) as isize;
        self.scroll_lines(delta.saturating_mul(amount));
    }

    pub fn scroll_half_pages(&mut self, delta: isize) {
        let amount = (self.page / 2).max(1) as isize;
        self.scroll_lines(delta.saturating_mul(amount));
    }

    pub fn pan(&mut self, delta: isize) {
        apply_delta(&mut self.h_scroll, delta.saturating_mul(4));
        if delta > 0 {
            self.clamp_scroll();
        }
    }

    pub fn goto_start(&mut self) {
        self.scroll = 0;
    }

    pub fn goto_end(&mut self) {
        self.scroll = usize::MAX;
        self.clamp_scroll();
    }

    pub fn jump_match(&mut self, next: bool) -> String {
        if self.matches.is_empty() {
            return "no matches".into();
        }
        if next {
            self.match_i = (self.match_i + 1) % self.matches.len();
        } else if self.match_i == 0 {
            self.match_i = self.matches.len() - 1;
        } else {
            self.match_i -= 1;
        }
        let row = self.matches[self.match_i];
        self.jump_to_row(row);
        format!("match {} / {}", self.match_i + 1, self.matches.len())
    }

    pub fn visible_lines(&self, no_color: bool) -> Vec<Line<'static>> {
        if self.rows.is_empty() {
            return Vec::new();
        }
        let start = self.scroll.min(self.rows.len());
        let end = (start + self.page).min(self.rows.len());
        self.rows[start..end]
            .iter()
            .map(|row| self.row_line(row, no_color))
            .collect()
    }

    fn rebuild_rows(&mut self) {
        self.rows = build_rows(&self.prepared, &self.filter);
        self.rebuild_matches();
        self.clamp_scroll();
    }

    fn rebuild_matches(&mut self) {
        self.matches = search_matches(&self.rows, &self.prepared, &self.search);
        if self.matches.is_empty() || self.match_i >= self.matches.len() {
            self.match_i = 0;
        }
    }

    fn clamp_scroll(&mut self) {
        let max = self.rows.len().saturating_sub(self.page.max(1));
        if self.scroll == usize::MAX || self.scroll > max {
            self.scroll = max;
        }
        let max_h = self.max_line_chars.saturating_sub(self.list_width.max(1));
        if self.h_scroll > max_h {
            self.h_scroll = max_h;
        }
    }

    fn jump_to_row(&mut self, row: usize) {
        if row < self.scroll {
            self.scroll = row;
        } else if row >= self.scroll.saturating_add(self.page) {
            self.scroll = row.saturating_sub(self.page.saturating_sub(1));
        }
        self.clamp_scroll();
    }

    fn sync_search_from_current(&mut self) -> String {
        self.rebuild_matches();
        if self.search.is_empty() {
            return String::new();
        }
        if self.matches.is_empty() {
            return "no matches".into();
        }
        let from = self.scroll;
        self.match_i = self
            .matches
            .iter()
            .position(|&row| row >= from)
            .unwrap_or(0);
        let row = self.matches[self.match_i];
        self.jump_to_row(row);
        format!("match {} / {}", self.match_i + 1, self.matches.len())
    }

    fn row_line(&self, row: &Row, no_color: bool) -> Line<'static> {
        match row {
            Row::Header { directory, count } => {
                directory_header(directory, *count, self.list_width as u16, no_color)
            }
            Row::Op(index) => {
                let op = &self.prepared[*index];
                let mut line = op.line.clone();
                if !self.search.is_empty() {
                    let visible = line_text(&line).to_lowercase();
                    if let Some((start, len)) = find_ignore_case(&visible, &self.search) {
                        line = highlight_range(line, start, len, search_style(no_color));
                    }
                }
                skip_line_chars(line, self.h_scroll)
            }
        }
    }
}

fn apply_delta(value: &mut usize, delta: isize) {
    if delta >= 0 {
        *value = value.saturating_add(delta as usize);
    } else {
        *value = value.saturating_sub(delta.unsigned_abs());
    }
}

fn prepare_ops(
    operations: &[Operation],
    root: &str,
    id_width: usize,
    no_color: bool,
) -> Vec<PreparedOp> {
    operations
        .iter()
        .filter(|operation| !matches!(operation.kind, OpKind::Noop | OpKind::Skip))
        .map(|operation| {
            let directory = parent_relative(&operation.from, root);
            let line = operation_line(operation, root, id_width, no_color);
            let from = display_path(&operation.from, root);
            let to = display_path(&operation.to, root);
            let haystack_lower = format!(
                "{:0width$} {from} {to} {directory} {}",
                operation.id,
                line_text(&line),
                width = id_width.max(1)
            )
            .to_lowercase();
            PreparedOp {
                directory,
                haystack_lower,
                line,
            }
        })
        .collect()
}

fn build_rows(prepared: &[PreparedOp], filter: &str) -> Vec<Row> {
    let needle = filter.to_lowercase();
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (index, operation) in prepared.iter().enumerate() {
        if !needle.is_empty() && !operation.haystack_lower.contains(&needle) {
            continue;
        }
        match groups.last_mut() {
            Some((current, items)) if current == &operation.directory => items.push(index),
            _ => groups.push((operation.directory.clone(), vec![index])),
        }
    }
    let mut rows = Vec::new();
    for (directory, items) in groups {
        rows.push(Row::Header {
            directory,
            count: items.len(),
        });
        for index in items {
            rows.push(Row::Op(index));
        }
    }
    rows
}

fn search_matches(rows: &[Row], prepared: &[PreparedOp], query: &str) -> Vec<usize> {
    let needle = query.to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut matches = Vec::new();
    for (row_index, row) in rows.iter().enumerate() {
        let matched = match row {
            Row::Header { directory, .. } => directory.to_lowercase().contains(&needle),
            Row::Op(index) => prepared[*index].haystack_lower.contains(&needle),
        };
        if matched {
            matches.push(row_index);
        }
    }
    matches
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

pub(crate) fn color_style(color: Color, no_color: bool) -> Style {
    if no_color {
        Style::default()
    } else {
        Style::default().fg(color)
    }
}

fn search_style(no_color: bool) -> Style {
    if no_color {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    }
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn line_char_count(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|span| span.content.chars().count())
        .sum()
}

fn skip_line_chars(line: Line<'static>, skip: usize) -> Line<'static> {
    if skip == 0 {
        return line;
    }
    let mut left = skip;
    let mut spans = Vec::new();
    for span in line.spans {
        if left == 0 {
            spans.push(span);
            continue;
        }
        let count = span.content.chars().count();
        if count <= left {
            left -= count;
            continue;
        }
        let kept: String = span.content.chars().skip(left).collect();
        left = 0;
        spans.push(Span::styled(kept, span.style));
    }
    Line::from(spans)
}

fn find_ignore_case(haystack_lower: &str, query: &str) -> Option<(usize, usize)> {
    let needle = query.to_lowercase();
    if needle.is_empty() {
        return None;
    }
    let byte = haystack_lower.find(&needle)?;
    let start = haystack_lower[..byte].chars().count();
    let len = needle.chars().count();
    Some((start, len))
}

fn highlight_range(line: Line<'static>, start: usize, len: usize, style: Style) -> Line<'static> {
    if len == 0 {
        return line;
    }
    let end = start.saturating_add(len);
    let mut out = Vec::new();
    let mut pos = 0usize;
    for span in line.spans {
        let chars: Vec<char> = span.content.chars().collect();
        let span_end = pos + chars.len();
        if span_end <= start || pos >= end {
            out.push(span);
            pos = span_end;
            continue;
        }
        let local_start = start.saturating_sub(pos);
        let local_end = end.min(span_end) - pos;
        if local_start > 0 {
            out.push(Span::styled(
                chars[..local_start].iter().collect::<String>(),
                span.style,
            ));
        }
        if local_end > local_start {
            out.push(Span::styled(
                chars[local_start..local_end].iter().collect::<String>(),
                span.style.patch(style),
            ));
        }
        if local_end < chars.len() {
            out.push(Span::styled(
                chars[local_end..].iter().collect::<String>(),
                span.style,
            ));
        }
        pos = span_end;
    }
    Line::from(out)
}

#[cfg(test)]
fn all_lines(preview: &Preview, width: u16, no_color: bool) -> Vec<Line<'static>> {
    preview
        .rows
        .iter()
        .map(|row| match row {
            Row::Header { directory, count } => {
                directory_header(directory, *count, width, no_color)
            }
            Row::Op(index) => preview.prepared[*index].line.clone(),
        })
        .collect()
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

    fn sample_preview() -> Preview {
        let mut preview = Preview::from_operations(&sample_ops(), "/music", 4, true);
        preview.set_viewport(10, 40);
        preview
    }

    #[test]
    fn groups_by_directory_with_padded_ids_and_inline_diff() {
        let preview = sample_preview();
        let lines = all_lines(&preview, 40, true);
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
        let preview = Preview::from_operations(&operations, "/music", 1, true);
        let lines = all_lines(&preview, 20, true);
        assert_eq!(lines.len(), 2);
        assert!(line_text(&lines[0]).starts_with(" . 1"));
        assert!(line_text(&lines[1]).contains("old"));
        assert!(line_text(&lines[1]).contains("new"));
    }

    #[test]
    fn filter_keeps_matching_ops_and_headers() {
        let mut preview = sample_preview();
        preview.set_filter("gone".into());
        assert_eq!(preview.row_count(), 2);
        match &preview.rows[0] {
            Row::Header { directory, count } => {
                assert_eq!(directory, "other");
                assert_eq!(*count, 1);
            }
            Row::Op(_) => panic!("expected header"),
        }
        assert!(matches!(preview.rows[1], Row::Op(_)));
    }

    #[test]
    fn search_matches_ids_and_paths() {
        let mut preview = sample_preview();
        preview.set_search("0002".into());
        assert_eq!(preview.match_count(), 1);
        preview.set_search("_new20".into());
        assert!(preview.match_count() >= 2);
    }

    #[test]
    fn viewport_only_materializes_visible_rows() {
        let mut preview = sample_preview();
        preview.set_viewport(2, 40);
        preview.goto_start();
        let lines = preview.visible_lines(true);
        assert_eq!(lines.len(), 2);
        assert!(line_text(&lines[0]).starts_with(" _NEW20 2"));
        preview.scroll = 3;
        let lines = preview.visible_lines(true);
        assert_eq!(lines.len(), 2);
        assert!(line_text(&lines[0]).starts_with(" other 1"));
    }

    #[test]
    fn horizontal_skip_drops_leading_chars() {
        let line = Line::from(vec![
            Span::raw("abcd"),
            Span::styled("efgh", Style::default().fg(Color::Green)),
        ]);
        let skipped = skip_line_chars(line, 5);
        assert_eq!(line_text(&skipped), "fgh");
    }

    #[test]
    fn highlight_splits_span_around_match() {
        let line = Line::from(vec![
            Span::raw("pre"),
            Span::raw("MATCH"),
            Span::raw("post"),
        ]);
        let highlighted =
            highlight_range(line, 3, 5, Style::default().add_modifier(Modifier::BOLD));
        assert_eq!(line_text(&highlighted), "preMATCHpost");
        assert_eq!(highlighted.spans.len(), 3);
        assert!(
            highlighted.spans[1]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn filter_matches_original_path_not_just_inline_diff() {
        let mut preview = sample_preview();
        preview.set_filter("old-one".into());
        assert_eq!(preview.row_count(), 2);
    }
}
