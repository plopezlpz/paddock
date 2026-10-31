//! Drawing. Every colour comes from the Theme (see theme.rs). Each draw
//! records in the View where rows, buttons, toggles and menu items landed,
//! for the mouse.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Command, Create, Hint, Menu, Mode, Rename, Status, Tab, View};
use crate::edit::LineEdit;
use crate::model::State;
use crate::rows::{short_path, New, Row, RowKind};
use crate::theme::{agent_icon, Theme};
#[cfg(test)]
use crate::theme::AgentIcons;

/// Narrower than this (a phone), the compact layout.
const COMPACT_BELOW: u16 = 80;
/// The preview shows from this many inner columns (list and preview
/// together), i.e. a popup at least 102 wide.
const PREVIEW_FROM: u16 = 100;
/// Beside the preview, the list takes half the width, up to this: rows are
/// rarely longer, and the rest serves the preview better.
const LIST_MAX: u16 = 72;
/// Between footer hints.
const GAP: &str = "   ";

fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

fn dot(t: &Theme, s: Option<State>) -> Span<'static> {
    match s {
        Some(s) => Span::styled(t.icon(s).to_string(), t.state(s)),
        None => Span::raw(" "),
    }
}

/// S cut to MAX display columns, ending in … when cut.
fn fit(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    if max > 0 {
        out.push('…');
    }
    out
}

/// Display columns SPANS take.
fn cols(spans: &[Span]) -> usize {
    spans.iter().map(Span::width).sum()
}

/// SLOT at the right end of WIDTH columns, after LINE.
fn push_right(line: &mut Vec<Span<'static>>, slot: Span<'static>, width: usize) {
    line.push(Span::raw(" ".repeat(width.saturating_sub(cols(line) + slot.width()))));
    line.push(slot);
}

/// A name being typed, in ROOM columns: bold around the cursor; empty, the
/// cursor and PLACEHOLDER (what an empty name means).
fn field_spans(field: &LineEdit, placeholder: &str, room: usize, t: &Theme) -> Vec<Span<'static>> {
    let cursor = Span::styled("▏", Style::new().fg(t.accent));
    if field.is_empty() {
        return vec![cursor, Span::styled(fit(placeholder, room.saturating_sub(1)), t.muted().add_modifier(Modifier::ITALIC))];
    }
    let (before, after) = field.window(room);
    vec![Span::styled(before, bold()), cursor, Span::styled(after, bold())]
}

/// The previewed pane: a title line and its screen.
pub struct Preview<'a> {
    pub title: &'a str,
    pub body: &'a Text<'static>,
}

pub fn draw(f: &mut Frame, app: &App, view: &mut View, preview: &Preview<'_>, t: &Theme) {
    let area = f.area();
    view.compact = area.width < COMPACT_BELOW;
    view.clear();

    // Frame title: the tabs (buttons) on the left, the agent counts on the right.
    let mut title = vec![Span::raw(" ")];
    let mut x = area.x + 2;
    for tab in [Tab::Sessions, Tab::Agents] {
        let name = tab.label();
        let style = if tab == app.tab { bold().fg(t.accent) } else { Style::new() };
        view.buttons.push((Rect::new(x, area.y, name.len() as u16, 1), Command::Tab(tab)));
        title.push(Span::styled(name, style));
        title.push(Span::raw("  "));
        x += name.len() as u16 + 2;
    }
    let counts = app.snap.counts;
    let mut groups: Vec<[Span; 3]> = State::ALL
        .into_iter()
        .rev()
        .filter(|&s| counts[s as usize] > 0)
        .map(|s| [Span::raw(" "), dot(t, Some(s)), Span::raw(format!(" {}", counts[s as usize]))])
        .collect();
    // Drawn over the tabs where they meet: the least urgent states go
    // first, so no count is left without its dot.
    let fits = |g: &[[Span; 3]]| g.is_empty() || cols(&title) + g.iter().map(|g| cols(g)).sum::<usize>() + 4 <= area.width as usize;
    while !fits(&groups) {
        groups.pop();
    }
    let mut tally: Vec<Span> = groups.into_iter().flatten().collect();
    if !tally.is_empty() {
        tally.push(Span::raw(" "));
    }
    let mut frame = Block::new()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(t.border())
        // Titles are drawn over the border and would keep its dim or
        // colour: start from the terminal's plain text.
        .title_style(Style::new().fg(Color::Reset).remove_modifier(Modifier::DIM))
        .title(Line::from(title))
        .title(Line::from(tally).right_aligned());
    if let Some(status) = &app.status {
        frame = frame.title_bottom(status_line(status, area.width.saturating_sub(4) as usize, t));
    }
    let inner = frame.inner(area);
    f.render_widget(frame, area);

    // A blank line under the input sets it apart from the list.
    let [input, _, body, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
    let pad = |r: Rect| Rect { x: r.x + 1, width: r.width.saturating_sub(2), ..r };

    draw_input(f, app, view.compact, pad(input), t);

    view.preview_fits = body.width >= PREVIEW_FROM;
    let (list, side) = if app.preview_on && view.preview_fits {
        let [l, r] = Layout::horizontal([Constraint::Length((body.width / 2).min(LIST_MAX)), Constraint::Fill(1)]).areas(body);
        (l, Some(r))
    } else {
        (body, None)
    };
    draw_rows(f, app, view, pad(list), t);
    if let Some(side) = side {
        let block = Block::new().borders(Borders::LEFT).border_style(t.border());
        let inner = pad(block.inner(side));
        f.render_widget(block, side);
        let [head, text] = Layout::vertical([Constraint::Length(2), Constraint::Fill(1)]).areas(inner);
        f.render_widget(Paragraph::new(Line::styled(preview.title, t.muted())), head);
        // Bottom-align the tail of the pane, like the end of a terminal.
        let skip = preview.body.lines.len().saturating_sub(text.height as usize);
        f.render_widget(Paragraph::new(preview.body.lines[skip..].to_vec()), text);
    }

    draw_footer(f, app, view, pad(footer), t);
    if let Mode::Menu(menu) = &app.mode {
        draw_menu(f, menu, app.selected, view, area, t);
    }
}

/// A status message for the frame's bottom edge, in MAX columns.
fn status_line(status: &Status, max: usize, t: &Theme) -> Line<'static> {
    let (mark, text, style) = match status {
        Status::Done(s) => ("✓ ", s, Style::new().fg(t.hint)),
        Status::Failed(s) => ("✗ ", s, t.state(State::Asking)),
        Status::Note(s) => ("", s, Style::new()),
    };
    Line::from(vec![Span::raw(" "), Span::styled(format!("{mark}{}", fit(text, max.saturating_sub(4))), style), Span::raw(" ")])
}

/// Prompt and query (its end, when long) or a hint, and while filtering the
/// number of matching rows on the right. While renaming, the query is shown muted
/// (the list is still filtered by it) and the mode on the right.
fn draw_input(f: &mut Frame, app: &App, compact: bool, area: Rect, t: &Theme) {
    // A question replaces the line: what will happen, in the asking colour.
    if let Mode::Confirm(c) = &app.mode {
        let note = if c.kills() && !compact { "can't be undone" } else { "" };
        let [left, side] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(note.width() as u16 + 1)]).areas(area);
        let warn = t.state(State::Asking);
        let line = Line::from(vec![Span::styled("✗ ", warn), Span::styled(fit(&c.question, (left.width as usize).saturating_sub(2)), warn.add_modifier(Modifier::BOLD))]);
        f.render_widget(Paragraph::new(line), left);
        f.render_widget(Paragraph::new(Line::styled(note, t.muted())).right_aligned(), side);
        return;
    }
    // Typing goes to a name: the query is shown muted, without a cursor.
    let naming = matches!(app.mode, Mode::Rename(_) | Mode::Create(_));
    let renaming = matches!(app.mode, Mode::Rename(_));
    let right = if renaming {
        Span::styled("✎ renaming", Style::new().fg(t.accent))
    } else if let Mode::Create(c) = &app.mode {
        Span::styled(if matches!(c.new, New::Window { .. }) { "+ new window" } else { "+ new session" }, Style::new().fg(t.accent))
    } else if let Mode::Move(c) = &app.mode {
        Span::styled(if compact { "⇅ moving".to_string() } else { format!("⇅ moving {}", c.label) }, Style::new().fg(t.accent))
    } else if !app.query.is_empty() {
        Span::styled(app.rows.iter().filter(|r| r.target.is_some() && r.score.is_some()).count().to_string(), t.muted())
    } else {
        Span::raw("")
    };
    let [left, side] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(right.width() as u16 + 1)]).areas(area);
    let mut spans = vec![Span::styled("› ", if naming { t.muted() } else { Style::new().fg(t.accent) })];
    if app.query.is_empty() {
        spans.push(Span::styled("filter", t.muted()));
    } else {
        let (text, _) = app.query.window((left.width as usize).saturating_sub(2));
        spans.push(Span::styled(text, if naming { t.muted() } else { Style::new() }));
        if !naming {
            spans.push(Span::styled("▏", Style::new().fg(t.accent)));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), left);
    f.render_widget(Paragraph::new(Line::from(right)).right_aligned(), side);
}

/// Key hints that are also buttons; compact (a phone, tapped) they are
/// words alone. The last (close, cancel) keeps the right edge; when the rest
/// doesn't fit, the least needed fold into a ⋯ that opens the row's menu.
fn draw_footer(f: &mut Frame, app: &App, view: &mut View, area: Rect, t: &Theme) {
    let compact = view.compact;
    let (hints, last) = app.hints(view);
    let text = |h: &Hint| if compact { h.label.clone() } else { format!("{} {}", h.key, h.label) };
    let room = (area.width as usize).saturating_sub(text(&last).width() + GAP.len());
    // Columns the SHOWN hints take, with a ⋯ after them if MORE.
    let total = |shown: &[bool], more: bool| {
        let widths: Vec<usize> = hints.iter().zip(shown).filter(|(_, s)| **s).map(|(h, _)| text(h).width()).collect();
        let n = widths.len() + usize::from(more);
        widths.iter().sum::<usize>() + usize::from(more) + n.saturating_sub(1) * GAP.len()
    };
    let mut shown = vec![true; hints.len()];
    // Only the list has a menu to fold into.
    let more = matches!(app.mode, Mode::List) && total(&shown, false) > room;
    if total(&shown, false) > room {
        shown = vec![false; hints.len()];
        let mut order: Vec<usize> = (0..hints.len()).collect();
        order.sort_by_key(|&i| hints[i].rank);
        for i in order {
            shown[i] = true;
            if total(&shown, more) > room {
                shown[i] = false;
            }
        }
    }
    let style = |h: &Hint| -> Vec<Span<'static>> {
        if compact {
            vec![Span::styled(h.label.clone(), Style::new().fg(t.hint))]
        } else {
            vec![Span::styled(h.key, Style::new().fg(t.hint)), Span::styled(format!(" {}", h.label), t.muted())]
        }
    };
    let mut spans = Vec::new();
    let mut x = area.x;
    for (h, _) in hints.iter().zip(&shown).filter(|(_, s)| **s) {
        if !spans.is_empty() {
            spans.push(Span::raw(GAP));
            x += GAP.len() as u16;
        }
        let w = (text(h).width() as u16).min(area.right().saturating_sub(x));
        view.buttons.push((Rect::new(x, area.y, w, 1), h.command));
        spans.extend(style(h));
        x += w;
    }
    if more {
        if !spans.is_empty() {
            spans.push(Span::raw(GAP));
            x += GAP.len() as u16;
        }
        // The ⋯ is a wider target than its one column.
        view.buttons.push((Rect::new(x.saturating_sub(1), area.y, 3.min(area.right().saturating_sub(x) + 1), 1), Command::Menu));
        spans.push(Span::styled("⋯", Style::new().fg(t.hint)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    let w = (text(&last).width() as u16).min(area.width);
    view.buttons.push((Rect::new(area.right() - w, area.y, w, 1), last.command));
    f.render_widget(Paragraph::new(Line::from(style(&last))).right_aligned(), area);
}

/// A row's lines, and the column of its ▸/▾ (a window with panes).
fn row_lines(row: &Row, selected: bool, compact: bool, width: u16, t: &Theme) -> (Vec<Line<'static>>, Option<u16>) {
    let width = width as usize;
    let mark = if selected { Span::styled("▌", Style::new().fg(t.accent)) } else { Span::raw(" ") };
    let sel = |s: Style| if selected { s.add_modifier(Modifier::BOLD) } else { s };
    let tree = |s: &str| Span::styled(s.to_string(), t.muted());
    match &row.kind {
        RowKind::Session { name, windows, current } => {
            // The badge's padding stands in for the gap, so names line up.
            let mut line = if *current {
                vec![mark, Span::styled(format!(" {name} "), t.badge())]
            } else {
                vec![mark, Span::raw(" "), Span::styled(name.clone(), sel(bold()))]
            };
            if !compact {
                let n = if *windows == 1 { "1 window".to_string() } else { format!("{windows} windows") };
                line.push(Span::styled(format!("  {n}"), t.muted()));
            }
            (vec![Line::from(line)], None)
        }
        RowKind::Window { index, name, meta, state, active, last, expanded } => {
            let idx = if *active { Style::new().fg(t.accent) } else { t.muted() };
            let mut first = vec![mark.clone(), tree(if *last { " └─ " } else { " ├─ " }), Span::styled(format!("{index} "), idx), dot(t, *state), Span::raw(" ")];
            let indent = cols(&first);
            first.push(Span::styled(name.clone(), sel(Style::new())));
            let toggle = expanded.map(|open| {
                let at = cols(&first) + 1;
                first.push(Span::styled(if open { " ▾" } else { " ▸" }, t.muted()));
                at as u16
            });
            if !compact {
                first.push(Span::styled(format!("  {meta}"), t.muted()));
                return (vec![Line::from(first)], toggle);
            }
            // Compact: the folder under the name.
            let mut second = vec![mark, tree(if *last { "    " } else { " │  " })];
            second.push(Span::raw(" ".repeat(indent.saturating_sub(cols(&second)))));
            second.push(Span::styled(fit(meta, width.saturating_sub(indent)), t.muted()));
            (vec![Line::from(first), Line::from(second)], toggle)
        }
        RowKind::Pane { index, command, state, last, parent_last } => (
            vec![Line::from(vec![
                mark,
                tree(if *parent_last { "    " } else { " │  " }),
                tree(if *last { "└─ " } else { "├─ " }),
                Span::styled(format!(".{index} "), t.muted()),
                dot(t, *state),
                Span::raw(" "),
                Span::styled(command.clone(), sel(Style::new())),
            ])],
            None,
        ),
        RowKind::Agent { title, .. } => {
            let mut second = agent_detail_prefix(row, mark.clone(), t);
            let room = width.saturating_sub(cols(&second));
            second.push(if title.is_empty() {
                Span::styled(fit("untitled", room), t.muted().add_modifier(Modifier::ITALIC))
            } else {
                Span::styled(fit(title, room), t.muted())
            });
            (card_lines(Line::from(second), mark, row, width, t), None)
        }
        RowKind::Header { label, count, gap } => {
            let line = Line::from(vec![
                Span::styled(format!(" {label} "), t.muted().add_modifier(Modifier::BOLD)),
                Span::styled(count.to_string(), t.muted()),
            ]);
            (if *gap && !compact { vec![Line::raw(""), line] } else { vec![line] }, None)
        }
        RowKind::Hidden { count, expanded, gap } => {
            let label = format!(" {} hidden · {count}", if *expanded { "▾" } else { "▸" });
            let line = Line::from(vec![mark, Span::styled(fit(&label, width.saturating_sub(1)), sel(t.muted().add_modifier(Modifier::BOLD)))]);
            (if *gap && !compact { vec![Line::raw(""), line] } else { vec![line] }, None)
        }
        // Only listed while it is being named: see create_lines.
        RowKind::New => (vec![Line::from(vec![mark, Span::raw(" "), Span::styled("+ new", Style::new().fg(t.accent))])], None),
    }
}

/// The "+ new" row being named: the field where the window or session will
/// appear, then its folder; the reason a name was refused at the end.
fn create_lines(c: &Create, compact: bool, width: u16, home: &str, t: &Theme) -> Vec<Line<'static>> {
    let w = width as usize;
    let mut line = vec![Span::styled("▌", Style::new().fg(t.accent))];
    let (placeholder, dir) = match &c.new {
        New::Window { dir, .. } => {
            line.push(Span::styled(" ├─ ", t.muted()));
            (if compact { "name (or automatic)" } else { "window name (empty: named after what runs)" }, dir)
        }
        New::Session { dir } => {
            line.push(Span::raw(" "));
            ("session name", dir)
        }
    };
    line.push(Span::styled("+ ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)));
    let place = if compact { String::new() } else { format!("  {}", short_path(dir, home)) };
    // The reason gets what the name and folder leave.
    let spare = w.saturating_sub(cols(&line) + place.width() + c.field.text().width().max(8) + 4);
    let slot = c.error.as_ref().map(|e| Span::styled(format!("✗ {}", fit(e, spare.max(12))), t.state(State::Asking)));
    let room = w.saturating_sub(cols(&line) + place.width() + slot.as_ref().map_or(0, |s| s.width() + 2));
    line.extend(field_spans(&c.field, placeholder, room, t));
    line.push(Span::styled(place, t.muted()));
    if let Some(slot) = slot {
        push_right(&mut line, slot, w);
    }
    vec![Line::from(line)]
}

/// The second line starts with state-colored elapsed time. Unconfirmed states keep
/// their explicit question mark, including when there is no timestamp.
fn agent_detail_prefix(row: &Row, mark: Span<'static>, t: &Theme) -> Vec<Span<'static>> {
    let mut second = vec![mark, Span::raw("   ")];
    let RowKind::Agent { state, stale, age, .. } = &row.kind else { return second };
    let when = match (*stale, age.is_empty()) {
        (true, true) => format!("{}?", state.label()),
        (true, false) => format!("{age} {}?", state.label()),
        (false, true) => String::new(),
        (false, false) => format!("{age:<3}"),
    };
    if !when.is_empty() {
        second.extend([Span::styled(when, t.state(*state)), Span::raw("  ")]);
    }
    second
}

/// Directory-first cards stay two lines in both layouts. SECOND is either
/// the topic label or its editor; the bright identity line never moves.
fn card_lines(second: Line<'static>, mark: Span<'static>, row: &Row, width: usize, t: &Theme) -> Vec<Line<'static>> {
    let RowKind::Agent { state, kind, path, .. } = &row.kind else { return vec![second] };
    let icon = Span::styled(agent_icon(kind, t.agent_icons), bold());
    let padding = 3usize.saturating_sub(icon.width());
    let mut first = vec![mark, Span::raw(" "), dot(t, Some(*state)), Span::raw(" "), icon, Span::raw(" ".repeat(padding))];
    first.push(Span::styled(path.display(width.saturating_sub(cols(&first))), bold()));
    vec![Line::from(first), second]
}

/// The row being renamed: the field replaces the name/topic on its usual
/// line, alongside suggestion progress or an error. Other lines stay put.
fn rename_lines(row: &Row, r: &Rename, compact: bool, width: u16, t: &Theme) -> Vec<Line<'static>> {
    let w = width as usize;
    let mark = Span::styled("▌", Style::new().fg(t.accent));
    let slot = match (&r.suggesting, &r.error) {
        (Some(_), _) => Some(Span::styled(if compact { "✦ suggesting" } else { "✦ suggesting…" }, Style::new().fg(t.hint))),
        (None, Some(e)) => Some(Span::styled(format!("✗ {}", fit(e, (w / 2).max(12))), t.state(State::Asking))),
        (None, None) if r.field.is_empty() && !r.placeholder.is_empty() => Some(Span::styled("automatic", t.muted())),
        _ => None,
    };
    if matches!(row.kind, RowKind::Agent { .. }) {
        let mut second = agent_detail_prefix(row, mark.clone(), t);
        second.extend([Span::styled("✎", Style::new().fg(t.accent)), Span::raw(" ")]);
        // Age/stale metadata takes space too. Reserve a usable editor before
        // fitting feedback, rather than letting a long error overflow the row.
        let feedback_room = w.saturating_sub(cols(&second) + 6 + 2);
        let slot = slot.filter(|_| feedback_room > 0)
            .map(|s| Span::styled(fit(&s.content, feedback_room), s.style));
        let room = w.saturating_sub(cols(&second) + slot.as_ref().map_or(0, |s| s.width() + 2));
        second.extend(field_spans(&r.field, &r.placeholder, room, t));
        if let Some(slot) = slot {
            push_right(&mut second, slot, w);
        }
        return card_lines(Line::from(second), mark, row, w, t);
    }
    // What comes before the name, as the row draws it.
    let (mut first, rest) = match &row.kind {
        RowKind::Window { .. } => {
            let (mut lines, _) = row_lines(row, true, compact, width, t);
            let line = lines.remove(0);
            // The tree, index and dot: everything up to the name.
            let prefix: Vec<Span<'static>> = line.spans.into_iter().take(5).collect();
            (prefix, lines)
        }
        _ => (vec![mark.clone(), Span::raw(" ")], Vec::new()),
    };
    first.extend([Span::styled("✎", Style::new().fg(t.accent)), Span::raw(" ")]);
    let room = w.saturating_sub(cols(&first) + slot.as_ref().map_or(0, |s| s.width() + 2));
    first.extend(field_spans(&r.field, &r.placeholder, room, t));
    if let Some(slot) = slot {
        push_right(&mut first, slot, w);
    }
    let mut lines = vec![Line::from(first)];
    lines.extend(rest);
    lines
}

fn draw_rows(f: &mut Frame, app: &App, view: &mut View, area: Rect, t: &Theme) {
    view.scroll(&app.rows, app.selected, area.height);
    let renaming = match &app.mode {
        Mode::Rename(r) => Some(r),
        _ => None,
    };
    let mut y = area.y;
    let mut lines = Vec::new();
    for (i, row) in app.rows.iter().enumerate().skip(view.offset) {
        if y >= area.bottom() {
            break;
        }
        let h = row.height(view.compact);
        if let Some(target) = &row.target {
            // A row cut off at the bottom is only clickable where it shows.
            view.rows.push((Rect::new(area.x, y, area.width, h.min(area.bottom() - y)), i, target.clone()));
        }
        if let RowKind::Hidden { gap, .. } = row.kind {
            // The blank spacer above the heading is not a click target.
            let head_y = y + u16::from(gap && !view.compact);
            if head_y < area.bottom() {
                view.hidden = Some(Rect::new(area.x, head_y, area.width, 1));
            }
        }
        let (mut drawn, toggle) = match (&app.mode, renaming.filter(|r| row.target.as_ref() == Some(&r.target))) {
            (_, Some(r)) => (rename_lines(row, r, view.compact, area.width, t), None),
            (Mode::Create(c), None) if matches!(row.kind, RowKind::New) => (create_lines(c, view.compact, area.width, app.home(), t), None),
            _ => row_lines(row, i == app.selected, view.compact, area.width, t),
        };
        if let Mode::Move(carry) = &app.mode {
            if row.target.as_ref() == Some(&carry.target)
                && let Some(first) = drawn.first_mut()
                && let Some(mark) = first.spans.first_mut()
            {
                *mark = Span::styled("⇅", Style::new().fg(t.accent));
            }
            if i == app.selected
                && let Some(first) = drawn.first_mut()
            {
                // What ⏎ does here, at the end of the row.
                let (text, style) = match app.drop_note() {
                    Ok(note) => (format!("⏎ {note}"), Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
                    Err(why) => (why, t.muted()),
                };
                let used = first.width();
                let room = (area.width as usize).saturating_sub(used + 2);
                if room >= 8 {
                    let text = fit(&text, room);
                    first.spans.push(Span::raw(" ".repeat((area.width as usize).saturating_sub(used + text.width()))));
                    first.spans.push(Span::styled(text, style));
                }
            }
        }
        if let (Some(x), Some(target)) = (toggle, &row.target) {
            // The glyph and a column either side: an easy target.
            let x = area.x + x;
            view.toggles.push((Rect::new(x.saturating_sub(1), y, 3.min(area.right().saturating_sub(x - 1)), 1), target.clone()));
        }
        debug_assert_eq!(drawn.len(), h as usize, "Row::height");
        lines.extend(drawn);
        y += h;
    }
    if app.rows.is_empty() {
        let msg = match app.tab {
            Tab::Agents if app.query.text().trim().is_empty() => "no agents running",
            _ => "no matches",
        };
        lines.push(Line::styled(format!("  {msg}"), t.muted()));
    }
    f.render_widget(Paragraph::new(lines), area);
}

/// The open menu, a box hanging from its anchor (from the SELECTED row
/// when it has none), kept inside AREA.
fn draw_menu(f: &mut Frame, menu: &Menu, selected: usize, view: &mut View, area: Rect, t: &Theme) {
    let key_w = menu.items.iter().map(|i| i.key.width()).max().unwrap_or(0);
    let label_w = menu.items.iter().map(|i| i.label.width()).max().unwrap_or(0);
    let w = ((label_w + key_w + 7) as u16).min(area.width);
    let h = ((menu.items.len() + 2) as u16).min(area.height);
    let anchor = if menu.anchor != Position::default() {
        menu.anchor
    } else if let Some((r, ..)) = view.rows.iter().find(|(_, i, _)| *i == selected) {
        Position::new(r.x + 4, r.y + 1)
    } else {
        Position::new(area.x + area.width.saturating_sub(w) / 2, area.y + area.height.saturating_sub(h) / 2)
    };
    let x = anchor.x.min(area.right().saturating_sub(w));
    // Below the anchor if it fits, else above it.
    let y = if anchor.y + h <= area.bottom() { anchor.y } else { anchor.y.saturating_sub(h).max(area.y) };
    let rect = Rect::new(x, y, w, h);
    f.render_widget(Clear, rect);
    let block = Block::new().borders(Borders::ALL).border_set(border::ROUNDED).border_style(Style::new().fg(t.accent));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    view.menu = Some(rect);
    let mut lines = Vec::new();
    for (i, item) in menu.items.iter().enumerate().take(inner.height as usize) {
        let gap = (inner.width as usize).saturating_sub(item.label.width() + item.key.width() + 2);
        let chosen = i == menu.selected;
        let style = if chosen { Style::new().fg(t.accent).add_modifier(Modifier::REVERSED | Modifier::BOLD) } else { Style::new() };
        let key = if chosen { style } else { Style::new().fg(t.hint) };
        lines.push(Line::from(vec![
            Span::styled(format!(" {}{}", item.label, " ".repeat(gap)), style),
            Span::styled(format!("{} ", item.key), key),
        ]));
        view.items.push((Rect::new(inner.x, inner.y + i as u16, inner.width, 1), i));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_draw_as_many_lines_as_they_are_high() {
        let kinds = [
            RowKind::Session { name: "s".into(), windows: 2, current: true },
            RowKind::Window { index: "1".into(), name: "w".into(), meta: "~/a".into(), state: None, active: true, last: false, expanded: Some(false) },
            RowKind::Pane { index: "0".into(), command: "zsh".into(), state: Some(State::Idle), last: true, parent_last: false },
            RowKind::Agent { state: State::Working, stale: false, kind: "claude".into(), title: "t".into(), path: crate::paths::AgentPath::plain("~/project", ""), age: "5m".into() },
            RowKind::Header { label: "working".into(), count: 1, gap: false },
            RowKind::Header { label: "idle".into(), count: 1, gap: true },
            RowKind::Hidden { count: 2, expanded: false, gap: false },
            RowKind::Hidden { count: 2, expanded: true, gap: true },
        ];
        let t = Theme::from_values(&[]);
        for kind in kinds {
            let row = Row { target: None, kind, preview: None, score: None };
            for compact in [false, true] {
                assert_eq!(row_lines(&row, true, compact, 40, &t).0.len(), row.height(compact) as usize, "compact: {compact}");
            }
        }
    }

    #[test]
    fn agent_cards_put_bright_identity_above_muted_age_and_topic() {
        let cases = [("claude", "✳", "\u{ec82}"), ("codex", "›_", "\u{ec81}"), ("pi", "π", "π"), ("opencode", "◇", "\u{f0763}"), ("gemini", "✦", "\u{ec10}"), ("custom", "▣", "▣"), ("", "▣", "▣")];
        for (mode, kind, icon) in cases.into_iter().flat_map(|(kind, unicode, nerd)| [(AgentIcons::Unicode, kind, unicode), (AgentIcons::Nerd, kind, nerd)]) {
            let mut t = Theme::from_values(&["", if mode == AgentIcons::Nerd { "colour8" } else { "" }]);
            t.agent_icons = mode;
            assert_eq!(agent_icon(kind, mode), icon);
            assert!(icon.width() <= 2);
            for compact in [false, true] {
                for age in ["", "<1m", "5m", "12d"] {
                    for stale in [false, true] {
                        let row = Row {
                            target: None,
                            kind: RowKind::Agent {
                                state: State::Working, stale, kind: kind.into(), title: "task".into(),
                                path: crate::paths::AgentPath::plain("~/a-long-project-path-日本語", ""), age: age.into(),
                            },
                            preview: None, score: None,
                        };
                        for width in [24, 40, 72] {
                            let (lines, _) = row_lines(&row, true, compact, width, &t);
                            assert!(lines.iter().all(|l| l.width() <= width as usize), "{kind}: {lines:?}");
                            let identity = lines[0].to_string();
                            let detail = lines[1].to_string();
                            assert!(identity.starts_with(&format!("▌ ● {icon}")), "{identity}");
                            assert!(!identity.contains("task"), "{identity}");
                            let mark = lines[0].spans.iter().find(|s| s.content == icon).unwrap();
                            assert_eq!(mark.style, bold(), "{kind}: bright provider icon");
                            assert_eq!(lines[0].spans.last().unwrap().style, bold(), "bright path");
                            assert_eq!(lines[1].spans.last().unwrap().style, t.muted(), "muted topic");
                            assert!(detail.ends_with("task"), "{detail}");
                            if width == 72 {
                                assert!(identity.contains("~/a-long-project-path-日本語"), "{identity}");
                            }
                            if !age.is_empty() {
                                assert!(detail.find(age).unwrap() < detail.find("task").unwrap(), "{detail}");
                            }
                            assert_eq!(detail.contains('?'), stale, "{detail}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn elapsed_time_matches_the_state_while_the_topic_stays_muted() {
        let t = Theme::from_values(&[]);
        for state in State::ALL {
            let row = Row {
                target: None, preview: None, score: None,
                kind: RowKind::Agent { state, stale: false, kind: "pi".into(),
                    title: "Branch labels".into(), path: crate::paths::AgentPath::plain("/project", ""), age: "5m".into() },
            };
            for compact in [false, true] {
                let lines = row_lines(&row, false, compact, 40, &t).0;
                let age = lines[1].spans.iter().find(|s| s.content.contains("5m")).unwrap();
                assert_eq!(age.style, t.state(state));
                assert_eq!(lines[1].spans.last().unwrap().style, t.muted());
            }
        }
    }

    #[test]
    fn compact_agent_editor_fits_long_errors_after_stale_metadata() {
        let snap = crate::model::fixture(&["$1|s|@1|1|w|1|0|%1|0|1|pi||working|0|pi|one||||/a"]);
        let mut app = App::new(snap, Tab::Agents, "$1".into(), "@1".into(), String::new(), true, false);
        if let RowKind::Agent { age, stale, .. } = &mut app.rows[app.selected].kind {
            *age = "5m".into(); *stale = true;
        }
        app.run(Command::Rename);
        let Mode::Rename(r) = &mut app.mode else { panic!("not renaming") };
        r.error = Some("pi timed out after 30s".into());
        let t = Theme::from_values(&[]);
        for width in [24, 36, 40, 72] {
            let lines = rename_lines(&app.rows[app.selected], r, true, width, &t);
            assert!(lines.iter().all(|l| l.width() <= width as usize), "{width}: {lines:?}");
            assert!(lines[1].to_string().contains("✎ one"), "editor remains usable");
            if width == 36 {
                assert!(lines[1].to_string().contains("✗ pi"));
                assert!(lines[1].to_string().ends_with('…'), "visible error truncation");
            }
        }
    }

    #[test]
    fn fits_by_display_width() {
        assert_eq!(fit("abcdef", 6), "abcdef");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("日本語テキスト", 7), "日本語…");
        assert_eq!(fit("abc", 0), "");
    }
}
