//! Drawing. Every colour comes from the Theme (see theme.rs). Each draw
//! records in the View where rows, buttons, toggles and menu items landed,
//! for the mouse.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Command, Create, Hint, Menu, Mode, Rename, Status, Tab, View};
use crate::edit::LineEdit;
use crate::model::State;
use crate::paths::AgentPath;
use crate::rows::{fit, short_path, split_host, Filter, New, Row, RowKind};
use crate::theme::{agent_icon, Theme};
#[cfg(test)]
use crate::theme::AgentIcons;

/// Narrower than this (a phone), the compact layout.
const COMPACT_BELOW: u16 = 80;
/// Narrower than this, footer hints are words alone, to tap.
const WORDS_BELOW: u16 = 60;
/// The preview shows beside the list from this many inner columns (list and
/// preview together), i.e. a popup at least 102 wide.
const PREVIEW_FROM: u16 = 100;
/// Narrower, it shows below the list in a popup this many rows high.
const PREVIEW_BELOW_FROM: u16 = 24;
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

/// TEXT in STYLE, as ROW shows it: the characters FILTER matched marked,
/// if the row matches (not one listed for context).
fn hl(text: String, style: Style, row: &Row, filter: Option<&Filter>, t: &Theme) -> Vec<Span<'static>> {
    let hits = filter.filter(|_| row.score.is_some()).map(|f| f.hits(&text)).unwrap_or_default();
    if hits.is_empty() {
        return vec![Span::styled(text, style)];
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, g) in text.graphemes(true).enumerate() {
        let s = if hits.binary_search(&(i as u32)).is_ok() { style.patch(t.matched()) } else { style };
        match spans.last_mut() {
            Some(last) if last.style == s => last.content.to_mut().push_str(g),
            _ => spans.push(Span::styled(g.to_string(), s)),
        }
    }
    spans
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

/// SPANS cut to MAX columns, whole graphemes, each keeping its style.
fn cut(spans: Vec<Span<'static>>, max: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut used = 0;
    for s in spans {
        if used + s.width() <= max {
            used += s.width();
            out.push(s);
            continue;
        }
        let mut text = String::new();
        for g in s.content.graphemes(true) {
            if used + g.width() > max {
                break;
            }
            used += g.width();
            text.push_str(g);
        }
        out.push(Span::styled(text, s.style));
        break;
    }
    out
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
    // Named, each ends in a space: a wider gap to the next.
    let group = |s: State, named: bool| {
        let n = counts[s as usize];
        [Span::raw(" "), dot(t, Some(s)), Span::raw(if named { format!(" {n} {} ", s.label()) } else { format!(" {n}") })]
    };
    let urgent = State::ALL.into_iter().rev().filter(|&s| counts[s as usize] > 0);
    let fits = |g: &[[Span; 3]]| g.is_empty() || cols(&title) + g.iter().map(|g| cols(g)).sum::<usize>() + 4 <= area.width as usize;
    // Named when they all fit; else the least urgent states go first, so no
    // count is left without its mark.
    let mut groups: Vec<[Span; 3]> = urgent.clone().map(|s| group(s, true)).collect();
    if !fits(&groups) {
        groups = urgent.map(|s| group(s, false)).collect();
    }
    while !fits(&groups) {
        groups.pop();
    }
    let mut tally: Vec<Span> = groups.into_iter().flatten().collect();
    if tally.last().is_some_and(|s| !s.content.ends_with(' ')) {
        tally.push(Span::raw(" "));
    }
    let mut frame = Block::new()
        .borders(Borders::ALL)
        .border_set(if t.frame { border::ROUNDED } else { border::EMPTY })
        .border_style(t.border())
        // Titles are drawn over the border and would keep its dim or
        // colour: start from the terminal's plain text.
        .title_style(Style::new().fg(Color::Reset).remove_modifier(Modifier::DIM))
        .title(Line::from(title))
        .title(Line::from(tally).right_aligned());
    // A server's trouble stays, under whatever is said meanwhile.
    let trouble = app.trouble.clone().map(Status::Failed);
    if let Some(status) = app.status.as_ref().or(trouble.as_ref()) {
        frame = frame.title_bottom(status_line(status, area.width.saturating_sub(4) as usize, t));
    }
    let inner = frame.inner(area);
    f.render_widget(frame, area);

    // A blank line under the input sets it apart from the list.
    let [input, _, body, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
    let pad = |r: Rect| Rect { x: r.x + 1, width: r.width.saturating_sub(2), ..r };

    draw_input(f, app, view.compact, pad(input), t);

    let beside = body.width >= PREVIEW_FROM;
    view.preview_fits = beside || area.height >= PREVIEW_BELOW_FROM;
    // Nothing listed: no preview either, and the list's hint has the room.
    let (list, side) = if !(app.preview_on && view.preview_fits) || app.rows.is_empty() {
        (body, None)
    } else if beside {
        let [l, r] = Layout::horizontal([Constraint::Length((body.width / 2).min(LIST_MAX)), Constraint::Fill(1)]).areas(body);
        (l, Some(r))
    } else {
        let [l, r] = Layout::vertical([Constraint::Fill(1), Constraint::Fill(1)]).areas(body);
        (l, Some(r))
    };
    draw_rows(f, app, view, pad(list), t);
    if let Some(side) = side {
        // Beside: a rule on its left and a title line over the screen; below:
        // the title on the rule above it.
        let (block, side, head) = if beside {
            (Block::new().borders(Borders::LEFT), side, 2)
        } else {
            let title = fit(preview.title, (side.width as usize).saturating_sub(4));
            let title = if title.is_empty() { title } else { format!(" {title} ") };
            (Block::new().borders(Borders::TOP).title(Line::styled(title, t.muted())), pad(side), 0)
        };
        // Typed into: its rule in the accent, as the focus.
        let typing = matches!(app.mode, Mode::Type(_));
        let block = block.border_style(if typing { Style::new().fg(t.accent) } else { t.border() });
        let inner = pad(block.inner(side));
        f.render_widget(block, side);
        let [head, text] = Layout::vertical([Constraint::Length(head), Constraint::Fill(1)]).areas(inner);
        f.render_widget(Paragraph::new(Line::styled(fit(preview.title, head.width as usize), t.muted())), head);
        // Bottom-align the tail of the pane, like the end of a terminal: the
        // lines that show, by reference.
        let skip = preview.body.lines.len().saturating_sub(text.height as usize);
        let tail = preview.body.lines[skip..].iter().map(|l| Line { spans: l.spans.iter().map(|s| Span::styled(s.content.as_ref(), s.style)).collect(), ..*l });
        f.render_widget(Paragraph::new(tail.collect::<Vec<_>>()), text);
    }

    draw_footer(f, app, view, area.width < WORDS_BELOW, pad(footer), t);
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
    // Typing into a pane: the line says which.
    if let Mode::Type(typing) = &app.mode {
        let lead = "› typing to ";
        let line = Line::from(vec![
            Span::styled(lead, Style::new().fg(t.accent)),
            Span::styled(fit(&typing.label, (area.width as usize).saturating_sub(lead.width())), Style::new().fg(t.accent).add_modifier(Modifier::BOLD)),
        ]);
        f.render_widget(Paragraph::new(line), area);
        return;
    }
    // Typing goes to a name: the query is shown muted, without a cursor.
    let naming = matches!(app.mode, Mode::Rename(_) | Mode::Create(_));
    let renaming = matches!(app.mode, Mode::Rename(_));
    let right = if renaming {
        Span::styled("✎ renaming", Style::new().fg(t.accent))
    } else if let Mode::Create(c) = &app.mode {
        Span::styled(format!("+ new {}", if matches!(c.new, New::Window { .. }) { app.caps.window() } else { "session" }), Style::new().fg(t.accent))
    } else if let Mode::Move(c) = &app.mode {
        // What is carried, in up to half the line: the query keeps the rest.
        let room = (area.width as usize / 2).saturating_sub("⇅ moving ".width());
        let text = if compact || room < 4 { "⇅ moving".to_string() } else { format!("⇅ moving {}", fit(&c.label, room)) };
        Span::styled(text, Style::new().fg(t.accent))
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

/// Key hints that are also buttons; very narrow (a phone, tapped) they are
/// words alone (WORDS). The last (close, cancel) keeps the right edge; when
/// the rest doesn't fit, the least needed go (the list's menu, always
/// shown, has them all).
fn draw_footer(f: &mut Frame, app: &App, view: &mut View, words: bool, area: Rect, t: &Theme) {
    let (hints, last) = app.hints(view);
    let text = |h: &Hint| if words { h.label.clone() } else { format!("{} {}", h.key, h.label) };
    let room = (area.width as usize).saturating_sub(text(&last).width() + GAP.len());
    // Columns the SHOWN hints take.
    let total = |shown: &[bool]| {
        let widths: Vec<usize> = hints.iter().zip(shown).filter(|(_, s)| **s).map(|(h, _)| text(h).width()).collect();
        widths.iter().sum::<usize>() + widths.len().saturating_sub(1) * GAP.len()
    };
    let mut shown = vec![true; hints.len()];
    if total(&shown) > room {
        shown = vec![false; hints.len()];
        let mut order: Vec<usize> = (0..hints.len()).collect();
        order.sort_by_key(|&i| hints[i].rank);
        for i in order {
            shown[i] = true;
            if total(&shown) > room {
                shown[i] = false;
            }
        }
    }
    let style = |h: &Hint| -> Vec<Span<'static>> {
        if words {
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
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    let w = (text(&last).width() as u16).min(area.width);
    view.buttons.push((Rect::new(area.right() - w, area.y, w, 1), last.command));
    f.render_widget(Paragraph::new(Line::from(style(&last))).right_aligned(), area);
}

/// Keep the same width-aware path/branch formatter in both tabs. Pane counts
/// remain supplemental: omit them when they would crowd out the Git suffix.
fn window_detail(path: &AgentPath, panes: usize, width: usize) -> String {
    let label = path.display(width);
    if panes > 1 {
        let count = format!(" · {panes} panes");
        if label.width() + count.width() <= width {
            return format!("{label}{count}");
        }
        if width >= count.width() + 5 {
            return format!("{}{count}", path.display(width - count.width()));
        }
    }
    label
}

/// A row's lines, and the column of its ▸/▾ (a window with panes). The
/// selected row's main text is bold in the accent; FILTER marks matches.
/// WIN: what a window is called.
fn row_lines(row: &Row, selected: bool, compact: bool, width: u16, filter: Option<&Filter>, win: &str, t: &Theme) -> (Vec<Line<'static>>, Option<u16>) {
    let width = width as usize;
    let mark = if selected { Span::styled("▌", Style::new().fg(t.accent)) } else { Span::raw(" ") };
    let main = |s: Style| if selected { t.selected() } else { s };
    let marked = |text: String, style: Style| hl(text, style, row, filter, t);
    let tree = |s: &str| Span::styled(s.to_string(), t.muted());
    match &row.kind {
        RowKind::Session { name, host, away, windows, current } => {
            let mut line = vec![mark, Span::raw(" ")];
            let room = width.saturating_sub(cols(&line) + usize::from(*current));
            // Where it is, and that its server is failing (the status line
            // says why): the row is then the last it said.
            let at = if host.is_empty() { String::new() } else { format!("  @{host}") };
            let failing = match (away, host.is_empty()) {
                (false, _) => "",
                (true, true) => "  not answering",
                (true, false) => " · away",
            };
            // Said whole or not at all: cut, only its colour would say it.
            let failing = if failing.width() + 4 > room { "" } else { failing };
            let n = if *windows == 1 { format!("1 {win}") } else { format!("{windows} {win}s") };
            // A long name drops the count first, then cuts the host down to
            // what leaves the name half the line.
            let count = if compact || name.width() + at.width() + failing.width() + n.width() + 2 > room { String::new() } else { format!("  {n}") };
            let at_room = at.width().min(room.saturating_sub(failing.width() + name.width().min(room / 2)));
            let at = if at_room < at.width().min(5) { String::new() } else { fit(&at, at_room) };
            line.extend(marked(fit(name, room.saturating_sub(at.width() + failing.width() + count.width())), main(Style::new())));
            // The one the client shows, as tmux marks its current window.
            if *current {
                line.push(Span::styled("*", Style::new().fg(t.accent)));
            }
            if !at.is_empty() {
                line.extend(marked(at, if *away { t.state(State::Asking) } else { Style::new().fg(t.hint) }));
            }
            // No name to match: never underlined as one.
            if !failing.is_empty() {
                line.push(Span::styled(failing, t.state(State::Asking)));
            }
            if !count.is_empty() {
                line.push(Span::styled(count, t.muted()));
            }
            (vec![Line::from(line)], None)
        }
        RowKind::Window { index, name, path, panes, state, active, last, expanded } => {
            let idx = if *active { Style::new().fg(t.accent) } else { t.muted() };
            let mut first = vec![mark.clone(), tree(if *last { " └─ " } else { " ├─ " }), Span::styled(format!("{index} "), idx), dot(t, *state), Span::raw(" ")];
            let indent = cols(&first);
            // The name gets what the toggle and the folder leave; when both
            // are long, the folder keeps half (compact, it has its own line).
            let room = width.saturating_sub(indent + if expanded.is_some() { 2 } else { 0 });
            let detail = if compact { 0 } else { Some(window_detail(path, *panes, usize::MAX).width()).filter(|w| *w > 0).map_or(0, |w| w + 2) };
            let name_room = name.width().min(room.saturating_sub(detail).max(if compact { room } else { room / 2 }));
            first.extend(marked(fit(name, name_room), main(Style::new())));
            let toggle = expanded.map(|open| {
                let at = cols(&first) + 1;
                first.push(Span::styled(if open { " ▾" } else { " ▸" }, t.muted()));
                at as u16
            });
            if !compact {
                let detail = window_detail(path, *panes, width.saturating_sub(cols(&first) + 2));
                if !detail.is_empty() {
                    first.extend(marked(format!("  {detail}"), t.muted()));
                }
                return (vec![Line::from(first)], toggle);
            }
            // Compact: the folder under the name.
            let mut second = vec![mark, tree(if *last { "    " } else { " │  " })];
            second.push(Span::raw(" ".repeat(indent.saturating_sub(cols(&second)))));
            second.extend(marked(window_detail(path, *panes, width.saturating_sub(indent)), t.muted()));
            (vec![Line::from(first), Line::from(second)], toggle)
        }
        RowKind::Pane { index, command, state, last, parent_last } => {
            let mut line = vec![
                mark,
                tree(if *parent_last { "    " } else { " │  " }),
                tree(if *last { "└─ " } else { "├─ " }),
                Span::styled(format!(".{index} "), t.muted()),
                dot(t, *state),
                Span::raw(" "),
            ];
            line.extend(marked(command.clone(), main(Style::new())));
            (vec![Line::from(line)], None)
        }
        RowKind::Agent { .. } => (card_lines(row, mark, width, selected, filter, t), None),
        RowKind::Header { label, count, gap } => {
            let line = Line::from(vec![
                Span::styled(format!(" {label} "), t.muted().add_modifier(Modifier::BOLD)),
                Span::styled(count.to_string(), t.muted()),
            ]);
            (if *gap && !compact { vec![Line::raw(""), line] } else { vec![line] }, None)
        }
        RowKind::Hidden { count, expanded, gap } => {
            let label = format!(" {} hidden · {count}", if *expanded { "▾" } else { "▸" });
            let line = Line::from(vec![mark, Span::styled(fit(&label, width.saturating_sub(1)), main(t.muted().add_modifier(Modifier::BOLD)))]);
            (if *gap && !compact { vec![Line::raw(""), line] } else { vec![line] }, None)
        }
        // Only listed while it is being named: see create_lines.
        RowKind::New => (vec![Line::from(vec![mark, Span::raw(" "), Span::styled("+ new", Style::new().fg(t.accent))])], None),
    }
}

/// The "+ new" row being named: the field where the window (WIN) or session
/// will appear, then its folder; the reason a name was refused at the end.
fn create_lines(c: &Create, compact: bool, width: u16, home: &str, win: &str, t: &Theme) -> Vec<Line<'static>> {
    let w = width as usize;
    let mut line = vec![Span::styled("▌", Style::new().fg(t.accent))];
    let (placeholder, dir) = match &c.new {
        New::Window { dir, .. } => {
            line.push(Span::styled(" ├─ ", t.muted()));
            (if compact { "name (or automatic)".to_string() } else { format!("{win} name (empty: named after what runs)") }, dir)
        }
        New::Session { dir } => {
            line.push(Span::raw(" "));
            ("session name".to_string(), dir)
        }
    };
    line.push(Span::styled("+ ", Style::new().fg(t.accent).add_modifier(Modifier::BOLD)));
    // The folder takes at most half of what is left: the name comes first.
    let room = w.saturating_sub(cols(&line)) / 2;
    let place = if compact || room < 6 { String::new() } else { format!("  {}", fit(&short_path(dir, home), room - 2)) };
    // The reason gets what the name and folder leave.
    let spare = w.saturating_sub(cols(&line) + place.width() + c.field.text().width().max(8) + 4);
    let slot = c.error.as_ref().map(|e| Span::styled(format!("✗ {}", fit(e, spare.max(12))), t.state(State::Asking)));
    let room = w.saturating_sub(cols(&line) + place.width() + slot.as_ref().map_or(0, |s| s.width() + 2));
    line.extend(field_spans(&c.field, &placeholder, room, t));
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

/// An agent's card: two lines in both layouts. The first names it, the
/// second says how it is (see card_first and card_second).
fn card_lines(row: &Row, mark: Span<'static>, width: usize, selected: bool, filter: Option<&Filter>, t: &Theme) -> Vec<Line<'static>> {
    let titled = matches!(&row.kind, RowKind::Agent { title, .. } if !title.is_empty());
    vec![card_first(row, mark.clone(), width, selected, filter, t), card_second(row, mark, width, titled, filter, t)]
}

/// What starts a card's first line: its state and agent.
fn card_head(row: &Row, mark: Span<'static>, t: &Theme) -> Vec<Span<'static>> {
    let RowKind::Agent { state, kind, .. } = &row.kind else { return vec![mark] };
    let icon = Span::styled(agent_icon(kind, t.agent_icons), bold());
    let padding = 3usize.saturating_sub(icon.width());
    vec![mark, Span::raw(" "), dot(t, Some(*state)), Span::raw(" "), icon, Span::raw(" ".repeat(padding))]
}

/// A folder as a card shows it: its host's `host:` in the hint colour, as
/// Sessions shows `@host`.
fn folder_spans(text: String, host: &str, style: Style, row: &Row, filter: Option<&Filter>, t: &Theme) -> Vec<Span<'static>> {
    match text.strip_prefix(host).filter(|rest| !host.is_empty() && rest.starts_with(':')) {
        Some(rest) => {
            let mut spans = hl(format!("{host}:"), Style::new().fg(t.hint), row, filter, t);
            spans.extend(hl(rest[1..].to_string(), style, row, filter, t));
            spans
        }
        None => hl(text, style, row, filter, t),
    }
}

/// A card's first line: its title, bright (in the accent when SELECTED), or
/// its folder while it has none; the host where no folder shown says it,
/// and the changes waiting at the right, the first to go when short.
fn card_first(row: &Row, mark: Span<'static>, width: usize, selected: bool, filter: Option<&Filter>, t: &Theme) -> Line<'static> {
    let mut first = card_head(row, mark, t);
    let RowKind::Agent { state, title, note, path, host, diff, .. } = &row.kind else { return Line::from(first) };
    let identity = if selected { t.selected() } else { bold() };
    let room = width.saturating_sub(cols(&first));
    // A titled card's folder is on its second line, unless news is.
    let folder_shown = title.is_empty() || *state == State::Idle || note.is_empty();
    let at = if host.is_empty() || (folder_shown && split_host(&path.full).0 == host) { String::new() } else { format!("  @{host}") };
    let at = if at.width() > room / 2 { String::new() } else { at };
    let room = room - at.width();
    let lead_w = if title.is_empty() { path.display(usize::MAX).width() } else { title.width() };
    let changes = if diff.is_empty() { String::new() } else { format!("  {diff}") };
    let changes = if changes.width() + lead_w > room { String::new() } else { changes };
    let room = room - changes.width();
    if title.is_empty() {
        first.extend(folder_spans(path.display(room), host, identity, row, filter, t));
    } else {
        first.extend(hl(fit(title, room), identity, row, filter, t));
    }
    if !at.is_empty() {
        first.extend(hl(at, Style::new().fg(t.hint), row, filter, t));
    }
    if !changes.is_empty() {
        push_right(&mut first, Span::styled(changes.trim_start().to_string(), t.muted()), width);
    }
    Line::from(first)
}

/// A card's second line: how long it has been in its state; what it asks
/// or says while it isn't idle, in its state's colour, else (TITLED, its
/// folder being on the first line otherwise) its folder, or what it last
/// said; and where it runs, at the right, the first to go when short.
fn card_second(row: &Row, mark: Span<'static>, width: usize, titled: bool, filter: Option<&Filter>, t: &Theme) -> Line<'static> {
    let mut second = agent_detail_prefix(row, mark, t);
    let RowKind::Agent { state, note, path, host, place, .. } = &row.kind else { return Line::from(second) };
    let room = width.saturating_sub(cols(&second));
    let news = *state != State::Idle && !note.is_empty();
    let folder = titled && !news;
    let body_w = if folder { path.display(usize::MAX).width() } else if news { note.width() + 2 } else { note.width() };
    let place_w = if place.is_empty() || body_w + place.width() + 2 > room { 0 } else { place.width() + 2 };
    let room = room - place_w;
    if folder {
        second.extend(folder_spans(path.display(room), host, t.muted(), row, filter, t));
    } else if news && room > 2 {
        second.push(Span::styled("› ", t.muted()));
        second.extend(hl(fit(note, room - 2), t.state(*state), row, filter, t));
    } else if !news && !note.is_empty() {
        second.extend(hl(fit(note, room), t.muted(), row, filter, t));
    }
    if place_w > 0 {
        let at = width.saturating_sub(place.width());
        second.push(Span::raw(" ".repeat(at.saturating_sub(cols(&second)))));
        second.extend(hl(place.clone(), t.muted(), row, filter, t));
    }
    Line::from(second)
}

/// The row being renamed: the field replaces the name/topic on its usual
/// line, alongside suggestion progress or an error. Other lines stay put.
fn rename_lines(row: &Row, r: &Rename, compact: bool, width: u16, t: &Theme) -> Vec<Line<'static>> {
    let w = width as usize;
    let mark = Span::styled("▌", Style::new().fg(t.accent));
    let slot = match (&r.suggesting, &r.error) {
        (Some(_), _) => Some(Span::styled(if compact { "✦ suggesting" } else { "✦ suggesting…" }, Style::new().fg(t.hint))),
        // Half the row, or what a short name leaves of it: the reason a
        // save was refused is the whole of what the server said.
        (None, Some(e)) => Some(Span::styled(format!("✗ {}", fit(e, (w / 2).max(12).max(w.saturating_sub(r.field.text().width() + 20)))), t.state(State::Asking))),
        (None, None) if r.field.is_empty() && !r.placeholder.is_empty() => Some(Span::styled("automatic", t.muted())),
        _ => None,
    };
    if matches!(row.kind, RowKind::Agent { .. }) {
        // The field stands for the title, on the first line; the folder
        // shows on the second, as for a titled card.
        let mut first = card_head(row, mark.clone(), t);
        first.extend([Span::styled("✎", Style::new().fg(t.accent)), Span::raw(" ")]);
        // Reserve a usable editor before fitting feedback, rather than
        // letting a long error overflow the row.
        let feedback_room = w.saturating_sub(cols(&first) + 6 + 2);
        let slot = slot.filter(|_| feedback_room > 0)
            .map(|s| Span::styled(fit(&s.content, feedback_room), s.style));
        let room = w.saturating_sub(cols(&first) + slot.as_ref().map_or(0, |s| s.width() + 2));
        first.extend(field_spans(&r.field, &r.placeholder, room, t));
        if let Some(slot) = slot {
            push_right(&mut first, slot, w);
        }
        return vec![Line::from(first), card_second(row, mark, w, true, None, t)];
    }
    // What comes before the name, as the row draws it.
    let (mut first, rest) = match &row.kind {
        RowKind::Window { .. } => {
            let (mut lines, _) = row_lines(row, true, compact, width, None, "window", t);
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
    // The row on the list's last line.
    let mut bottom = None;
    for (i, row) in app.rows.iter().enumerate().skip(view.offset) {
        if y >= area.bottom() {
            break;
        }
        let h = row.height(view.compact);
        if y + h >= area.bottom() {
            bottom = Some(i);
        }
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
            (Mode::Create(c), None) if matches!(row.kind, RowKind::New) => (create_lines(c, view.compact, area.width, app.home(), app.caps.window(), t), None),
            _ => row_lines(row, i == app.selected, view.compact, area.width, Some(&app.filter), app.caps.window(), t),
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
            // What to do about it, on the server it runs on.
            Tab::Agents if app.query.text().trim().is_empty() && app.caps.rex => {
                "no agents running\n\nagents show here once they report their state to Rex (Claude Code and pi do by themselves)"
            }
            Tab::Agents if app.query.text().trim().is_empty() => {
                "no agents running\n\nagents show here once agent-status.tmux reports their state (README: Agent state)"
            }
            _ => "no matches · esc clears the filter",
        };
        let area = Rect { x: area.x + 2, width: area.width.saturating_sub(2), ..area };
        f.render_widget(Paragraph::new(msg).style(t.muted()).wrap(Wrap { trim: true }), area);
        return;
    }
    // What the list holds past either edge, at the right end of the line
    // above it (the blank under the input) and of its last line.
    let (above, below) = beyond(&app.rows, view.offset, view.compact, area.height);
    let width = area.width as usize;
    let more = |arrow: &str, n: usize| Span::styled(format!("{arrow} {n} more"), t.muted());
    if above > 0 && area.y > 0 && area.height > 0 {
        let up = more("↑", above);
        f.render_widget(Paragraph::new(Line::from(up)).right_aligned(), Rect { y: area.y - 1, height: 1, ..area });
    }
    if below > 0 && let Some(last) = (area.height as usize).checked_sub(1).and_then(|i| lines.get_mut(i)) {
        let down = more("↓", below);
        // Over the end of a row that is not the selected one: that one's
        // (a note, a field being typed in, what ⏎ does) is read, so there
        // the mark shows only beside it. A card's place, at its end after
        // its gap, is not read: covered, it goes whole (a part of one would
        // name another).
        // The changes, at the end of a card's first line, likewise.
        let before_place = bottom.and_then(|b| match &app.rows[b].kind {
            RowKind::Agent { place, diff, .. } => {
                let text = last.to_string();
                [place, diff].into_iter().filter(|end| !end.is_empty())
                    .find_map(|end| text.strip_suffix(end.as_str()).filter(|rest| rest.ends_with("  ")))
                    .map(|rest| rest.trim_end().width())
            }
            _ => None,
        });
        let keep = if bottom == Some(app.selected) { before_place.unwrap_or(last.width()) + 2 } else { 0 };
        if width > down.width() + 1 && keep + down.width() <= width {
            let at = width - down.width() - 1;
            let mut spans = cut(std::mem::take(&mut last.spans), before_place.map_or(at, |p| at.min(p)));
            push_right(&mut spans, down, width);
            last.spans = spans;
            // A toggle it covers is not there to be clicked.
            let (edge, y) = (area.x + at as u16, area.bottom() - 1);
            view.toggles.retain(|(r, _)| r.y != y || r.right() <= edge);
        }
    }
    f.render_widget(Paragraph::new(lines), area);
}

/// How many selectable rows (with a target, or the hidden heading) are out
/// of view above OFFSET and, in HEIGHT lines from it, below or cut off.
fn beyond(rows: &[Row], offset: usize, compact: bool, height: u16) -> (usize, usize) {
    let selectable = |r: &&Row| r.target.is_some() || matches!(r.kind, RowKind::Hidden { .. });
    let mut used = 0;
    let shown = rows.iter().skip(offset).take_while(|r| {
        used += r.height(compact);
        used <= height
    });
    let end = offset + shown.count();
    (rows[..offset.min(rows.len())].iter().filter(selectable).count(), rows[end.min(rows.len())..].iter().filter(selectable).count())
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
    // Scrolled so the selected item shows; the borders say what is past
    // them. Items record their own index: a click runs what it shows.
    let rows = inner.height as usize;
    let top = (menu.selected + 1).saturating_sub(rows);
    let n = menu.items.len();
    if top > 0 && rect.width > 6 && rows > 0 {
        f.render_widget(Paragraph::new(Line::styled(format!("↑{top}"), t.muted())).right_aligned(), Rect { x: rect.x + 1, width: rect.width - 2, height: 1, ..rect });
    }
    if n > top + rows && rect.width > 6 && rows > 0 {
        let below = Rect { x: rect.x + 1, y: rect.bottom() - 1, width: rect.width - 2, height: 1 };
        f.render_widget(Paragraph::new(Line::styled(format!("↓{}", n - top - rows), t.muted())).right_aligned(), below);
    }
    let mut lines = Vec::new();
    for (row, (i, item)) in menu.items.iter().enumerate().skip(top).take(rows).enumerate() {
        let gap = (inner.width as usize).saturating_sub(item.label.width() + item.key.width() + 2);
        let chosen = i == menu.selected;
        let style = if chosen { Style::new().fg(t.accent).add_modifier(Modifier::REVERSED | Modifier::BOLD) } else { Style::new() };
        let key = if chosen { style } else { Style::new().fg(t.hint) };
        lines.push(Line::from(vec![
            Span::styled(format!(" {}{}", item.label, " ".repeat(gap)), style),
            Span::styled(format!("{} ", item.key), key),
        ]));
        view.items.push((Rect::new(inner.x, inner.y + row as u16, inner.width, 1), i));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_draw_as_many_lines_as_they_are_high() {
        let kinds = [
            RowKind::Session { name: "s".into(), host: "devbox".into(), away: false, windows: 2, current: true },
            RowKind::Window { index: "1".into(), name: "w".into(), path: AgentPath::plain("~/a", ""), panes: 2, state: None, active: true, last: false, expanded: Some(false) },
            RowKind::Pane { index: "0".into(), command: "zsh".into(), state: Some(State::Idle), last: true, parent_last: false },
            RowKind::Agent { state: State::Working, stale: false, kind: "claude".into(), title: "t".into(), note: "n".into(), path: crate::paths::AgentPath::plain("~/project", ""), age: "5m".into(), host: String::new(), place: String::new(), diff: String::new() },
            RowKind::Header { label: "working".into(), count: 1, gap: false },
            RowKind::Header { label: "idle".into(), count: 1, gap: true },
            RowKind::Hidden { count: 2, expanded: false, gap: false },
            RowKind::Hidden { count: 2, expanded: true, gap: true },
        ];
        let t = Theme::default();
        for kind in kinds {
            let row = Row { target: None, kind, preview: None, score: None };
            for compact in [false, true] {
                assert_eq!(row_lines(&row, true, compact, 40, None, "window", &t).0.len(), row.height(compact) as usize, "compact: {compact}");
            }
        }
    }

    #[test]
    fn an_untitled_agents_note_has_the_line() {
        let kind = RowKind::Agent { state: State::Asking, stale: false, kind: "claude".into(), title: String::new(), note: "Allow edit to token.rs?".into(), path: AgentPath::plain("~/p", ""), age: "<1m".into(), host: String::new(), place: String::new(), diff: String::new() };
        let row = Row { target: None, kind, preview: None, score: None };
        let text: String = row_lines(&row, false, false, 40, None, "window", &Theme::default()).0.iter().map(|l| l.to_string() + "\n").collect();
        assert!(text.contains("Allow edit to token.rs?") && !text.contains("untitled"), "{text}");
    }

    #[test]
    fn agent_cards_put_a_bright_title_above_their_age_and_folder() {
        let cases = [("claude", "✳", "\u{ec82}"), ("codex", "›_", "\u{ec81}"), ("pi", "π", "π"), ("opencode", "◇", "\u{f0763}"), ("gemini", "✦", "\u{ec10}"), ("custom", "▣", "▣"), ("", "▣", "▣")];
        for (mode, kind, icon) in cases.into_iter().flat_map(|(kind, unicode, nerd)| [(AgentIcons::Unicode, kind, unicode), (AgentIcons::Nerd, kind, nerd)]) {
            let mut t = Theme::default();
            if mode == AgentIcons::Nerd {
                t.set("muted", "colour8");
            }
            t.agent_icons = mode;
            assert_eq!(agent_icon(kind, mode), icon);
            assert!(icon.width() <= 2);
            for compact in [false, true] {
                for age in ["", "<1m", "5m", "12d"] {
                    for stale in [false, true] {
                        let row = Row {
                            target: None,
                            kind: RowKind::Agent {
                                state: State::Working, stale, kind: kind.into(), title: "task".into(), note: String::new(),
                                path: crate::paths::AgentPath::plain("~/a-long-project-path-日本語", ""), age: age.into(), host: String::new(), place: String::new(), diff: String::new(),
                            },
                            preview: None, score: None,
                        };
                        for width in [24, 40, 72] {
                            let (lines, _) = row_lines(&row, true, compact, width, None, "window", &t);
                            assert!(lines.iter().all(|l| l.width() <= width as usize), "{kind}: {lines:?}");
                            let identity = lines[0].to_string();
                            let detail = lines[1].to_string();
                            assert!(identity.starts_with(&format!("▌ ● {icon}")), "{identity}");
                            assert!(identity.ends_with("task"), "{identity}");
                            let mark = lines[0].spans.iter().find(|s| s.content == icon).unwrap();
                            assert_eq!(mark.style, bold(), "{kind}: bright provider icon");
                            assert_eq!(lines[0].spans.last().unwrap().style, t.selected(), "bright title, selected");
                            assert_eq!(lines[1].spans.last().unwrap().style, t.muted(), "the folder muted");
                            if width == 72 {
                                assert!(detail.contains("~/a-long-project-path-日本語"), "{detail}");
                            }
                            if !age.is_empty() {
                                assert!(detail.find(age).unwrap() < detail.find("~/a").unwrap(), "{detail}");
                            }
                            assert_eq!(detail.contains('?'), stale, "{detail}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn elapsed_time_matches_the_state_and_the_title_is_bold() {
        let t = Theme::default();
        for state in State::ALL {
            let row = Row {
                target: None, preview: None, score: None,
                kind: RowKind::Agent { state, stale: false, kind: "pi".into(),
                    title: "Branch labels".into(), note: String::new(), path: crate::paths::AgentPath::plain("/project", ""), age: "5m".into(), host: String::new(), place: String::new(), diff: String::new() },
            };
            for compact in [false, true] {
                let lines = row_lines(&row, false, compact, 40, None, "window", &t).0;
                let age = lines[1].spans.iter().find(|s| s.content.contains("5m")).unwrap();
                assert_eq!(age.style, t.state(state));
                assert_eq!(lines[0].spans.last().unwrap().style, bold());
                assert_eq!(lines[1].spans.last().unwrap().style, t.muted());
            }
        }
    }

    #[test]
    fn compact_agent_editor_fits_long_errors_after_stale_metadata() {
        let snap = crate::backend::tmux::fixture(&["$1|s|@1|1|w|1|0|%1|0|1|pi||working|0|pi|one||||/a"]);
        let mut app = App::new(snap, Tab::Agents, "$1".into(), "@1".into(), String::new(), true, false);
        if let RowKind::Agent { age, stale, .. } = &mut app.rows[app.selected].kind {
            *age = "5m".into(); *stale = true;
        }
        app.run(Command::Rename);
        let Mode::Rename(r) = &mut app.mode else { panic!("not renaming") };
        r.error = Some("pi timed out after 30s".into());
        let t = Theme::default();
        for width in [24, 36, 40, 72] {
            let lines = rename_lines(&app.rows[app.selected], r, true, width, &t);
            assert!(lines.iter().all(|l| l.width() <= width as usize), "{width}: {lines:?}");
            assert!(lines[0].to_string().contains("✎ one"), "editor remains usable");
            assert!(lines[1].to_string().contains("5m"), "{lines:?}");
            if width == 36 {
                assert!(lines[0].to_string().contains("✗ pi"));
                assert!(lines[0].to_string().ends_with('…'), "visible error truncation");
            }
        }
    }

    #[test]
    fn session_window_paths_use_the_shared_formatter_and_fit_both_layouts() {
        let t = Theme::default();
        let path = AgentPath::plain("~/source/project/日本語/src", "");
        for panes in [1, 2, 100] {
            let row = Row {
                target: None, preview: None, score: None,
                kind: RowKind::Window { index: "1".into(), name: "work".into(), path: path.clone(), panes,
                    state: Some(State::Working), active: true, last: true, expanded: (panes > 1).then_some(false) },
            };
            for compact in [false, true] {
                for width in [24, 40, 72, 120] {
                    let (lines, toggle) = row_lines(&row, true, compact, width, None, "window", &t);
                    assert_eq!(lines.len(), if compact { 2 } else { 1 });
                    assert!(lines.iter().all(|l| l.width() <= width as usize), "{lines:?}");
                    assert_eq!(toggle.is_some(), panes > 1);
                    assert_eq!(lines.last().unwrap().spans.last().unwrap().style, t.muted());
                    if width == 120 {
                        assert!(lines.last().unwrap().to_string().contains(&path.display(100)));
                        assert_eq!(lines.last().unwrap().to_string().contains("panes"), panes > 1);
                    }
                }
            }
        }
        let short = AgentPath::plain("~/a", "");
        assert_eq!(window_detail(&short, 2, 13), "~/a · 2 panes", "keep the count when a short path fits exactly");
        assert_eq!(window_detail(&short, 2, 12), "~/a", "omit the count when there is insufficient room");
        for width in 0..100 {
            assert_eq!(window_detail(&path, 1, width), path.display(width));
            assert!(window_detail(&path, 2, width).width() <= width);
        }
    }

    #[test]
    fn fits_by_display_width() {
        assert_eq!(fit("abcdef", 6), "abcdef");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("日本語テキスト", 7), "日本語…");
        assert_eq!(fit("abc", 0), "");
    }

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn render(app: &App, view: &mut View, width: u16, height: u16, title: &str) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
        let body = Text::default();
        term.draw(|f| draw(f, app, view, &Preview { title, body: &body }, &Theme::default())).unwrap();
        term
    }

    fn screen(term: &Terminal<TestBackend>) -> Vec<String> {
        let buf = term.backend().buffer();
        (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>().trim_end().to_string()).collect()
    }

    fn agent(title: &str, note: &str, path: &str) -> Row {
        let kind = RowKind::Agent { state: State::Asking, stale: false, kind: "claude".into(), title: title.into(), note: note.into(), path: AgentPath::plain(path, ""), age: "<1m".into(), host: String::new(), place: String::new(), diff: String::new() };
        Row { target: None, kind, preview: None, score: None }
    }

    #[test]
    fn a_card_names_its_title_above_what_it_asks() {
        let t = Theme::default();
        let lines = |title: &str, note: &str, width: u16| -> Vec<String> { row_lines(&agent(title, note, "~/p"), false, false, width, None, "window", &t).0.iter().map(|l| l.to_string()).collect() };
        let card = lines("Fix bug", "Allow edit to main.rs?", 40);
        assert!(card[0].ends_with("Fix bug") && card[1].ends_with("› Allow edit to main.rs?"), "{card:#?}");
        // The question takes the line, cut when long.
        let long = lines("Refactor the session parser", "Allow edit to src/backend/rex.rs?", 31);
        assert!(long[0].ends_with("Refactor the session pa…") && long[1].ends_with("› Allow edit to src/b…"), "{long:#?}");
        // The changes at the right of the title, the first to go when short.
        let mut changed = agent("Fix bug", "Allow edit?", "~/p");
        if let RowKind::Agent { diff, .. } = &mut changed.kind {
            *diff = "+12 −3 · 2 untracked".into();
        }
        let first = |width: u16| row_lines(&changed, false, false, width, None, "window", &t).0[0].to_string();
        assert!(first(40).contains(" Fix bug  ") && first(40).ends_with(" +12 −3 · 2 untracked"), "{}", first(40));
        assert!(first(30).ends_with("Fix bug"), "{}", first(30));
        for width in 0..100 {
            for (title, note) in [("Fix bug", "Allow edit to main.rs?"), ("", "Allow 👨‍👩‍👧 edit?"), ("👍🏽 ok ❤️", ""), ("a ☺️ b", "x")] {
                let mut row = agent(title, note, "~/p");
                if let RowKind::Agent { diff, .. } = &mut row.kind {
                    *diff = "+1204 −3670 · 2 untracked".into();
                }
                assert!(row_lines(&row, true, false, width, None, "window", &t).0.iter().all(|l| l.width() <= (width as usize).max(9)), "{width}");
                let lines = row_lines(&agent(title, note, "~/p"), true, false, width, None, "window", &t).0;
                assert!(lines.iter().all(|l| l.width() <= (width as usize).max(9)), "{width}: {lines:?}");
            }
        }
    }

    #[test]
    fn a_card_says_where_it_runs_and_an_idle_one_its_folder() {
        let t = Theme::default();
        let card = |state: State, title: &str, note: &str| {
            let mut row = agent(title, note, "~/p");
            if let RowKind::Agent { state: s, place, .. } = &mut row.kind {
                *s = state;
                *place = "ml:train".into();
            }
            row
        };
        let line = |row: &Row, n: usize, width: u16| row_lines(row, false, false, width, None, "window", &t).0[n].to_string();
        let asking = card(State::Asking, "Fix bug", "Allow edit?");
        assert!(line(&asking, 0, 60).ends_with("Fix bug"), "{}", line(&asking, 0, 60));
        assert!(line(&asking, 1, 60).contains("› Allow edit?") && line(&asking, 1, 60).ends_with(" ml:train"), "{}", line(&asking, 1, 60));
        // Short of room, the place goes first.
        assert!(line(&asking, 1, 26).ends_with("› Allow edit?") && !line(&asking, 1, 26).contains("ml:"), "{}", line(&asking, 1, 26));
        // What an idle agent said is in the preview: its card says where.
        let idle = card(State::Idle, "Fix bug", "Done, all tests pass.");
        assert!(line(&idle, 1, 60).contains("~/p ") && !line(&idle, 1, 60).contains("Done") && line(&idle, 1, 60).ends_with(" ml:train"), "{}", line(&idle, 1, 60));
        // Untitled, its folder names it, and its last words follow, muted.
        let untitled = row_lines(&card(State::Idle, "", "Done, all tests pass."), false, false, 60, None, "window", &t).0;
        assert!(untitled[0].to_string().ends_with("~/p"), "{}", untitled[0]);
        let words = untitled[1].spans.iter().find(|s| s.content.contains("Done")).unwrap();
        assert_eq!(words.style, t.muted());
        assert!(untitled[1].to_string().ends_with(" ml:train"));
        for width in 0..80 {
            for row in [&asking, &idle] {
                assert!(row_lines(row, true, false, width, None, "window", &t).0.iter().all(|l| l.width() <= (width as usize).max(9)), "{width}");
            }
        }
    }

    #[test]
    fn long_session_and_window_names_are_fitted_beside_their_details() {
        let t = Theme::default();
        let name = "pablo@pmac: ~/source/dac/core/b2m-dac-core";
        let window = Row {
            target: None, preview: None, score: None,
            kind: RowKind::Window { index: "1".into(), name: name.into(), path: AgentPath::plain("~/source/dac/core/b2m-dac-core", ""), panes: 2,
                state: Some(State::Working), active: true, last: false, expanded: Some(false) },
        };
        let session = Row {
            target: None, preview: None, score: None,
            kind: RowKind::Session { name: name.into(), host: "devbox".into(), away: false, windows: 3, current: true },
        };
        for width in 12..120u16 {
            for compact in [false, true] {
                let (lines, toggle) = row_lines(&window, false, compact, width, None, "window", &t);
                assert!(lines.iter().all(|l| l.width() <= width as usize), "{width}: {lines:?}");
                let first = lines[0].to_string();
                let at = toggle.unwrap() as usize;
                assert_eq!(first.chars().nth(at), Some('▸'), "{width}: the toggle stays on the row: {first}");
                let (lines, _) = row_lines(&session, false, compact, width, None, "window", &t);
                assert!(lines[0].width() <= width as usize, "{width}: {lines:?}");
            }
        }
        let first = row_lines(&window, false, false, 80, None, "window", &t).0[0].to_string();
        assert!(first.contains("pablo@pmac: ~/source/dac/core/b2m… ▸  ~/source/dac/core/b2m-… · 2 panes"), "the path keeps half: {first}");
        let short = Row { kind: RowKind::Window { index: "1".into(), name: "ml".into(), path: AgentPath::plain("~/source/dac/core/b2m-dac-core", ""), panes: 1,
            state: None, active: true, last: true, expanded: None }, ..window };
        assert!(row_lines(&short, false, false, 50, None, "window", &t).0[0].to_string().ends_with("ml  ~/source/dac/core/b2m-dac-core"), "a short name leaves the path its room");
        let first = row_lines(&session, false, false, 40, None, "window", &t).0[0].to_string();
        assert!(first.contains('…') && first.contains("*  @devbox") && !first.contains("windows"), "{first}");
        assert!(row_lines(&session, false, false, 80, None, "window", &t).0[0].to_string().ends_with(&format!("{name}*  @devbox  3 windows")));
        // A server that is failing: its rows are the last it said.
        let row = |host: &str| Row { target: None, preview: None, score: None, kind: RowKind::Session { name: "api".into(), host: host.into(), away: true, windows: 1, current: false } };
        let line = |host: &str| row_lines(&row(host), false, false, 60, None, "window", &t).0;
        assert!(line("devbox")[0].to_string().ends_with("api  @devbox · away  1 window"), "{:?}", line("devbox"));
        assert!(line("")[0].to_string().ends_with("api  not answering  1 window"));
        // Said whole or not at all, whatever the room, and never as a match.
        let long = |host: &str, width: u16| {
            let kind = RowKind::Session { name: "a-long-session-name-here".into(), host: host.into(), away: true, windows: 1, current: false };
            row_lines(&Row { target: None, preview: None, score: None, kind }, false, false, width, None, "window", &t).0[0].to_string()
        };
        for width in 0..80 {
            for host in ["build-01.internal", "m", ""] {
                let line = long(host, width);
                let said = if host.is_empty() { "not answering" } else { " · away" };
                assert!(line.contains(said) || !line.contains(said.trim().split(' ').next_back().unwrap()), "{width}: {line}");
                assert!(line.width() <= (width as usize).max(3), "{width}: {line}");
            }
        }
        assert!(long("build-01.internal", 45).ends_with(" · away"), "{}", long("build-01.internal", 45));
        assert!(long("m", 60).contains("  @m · away"), "a short host shows: {}", long("m", 60));
        let said = line("devbox")[0].spans.iter().find(|s| s.content.contains("away")).unwrap().style;
        assert_eq!(said.fg, t.state(State::Asking).fg, "as a failure is said");
    }

    fn many_agents() -> App {
        let lines: Vec<String> = (1..=6).map(|i| format!("$1|s|@{i}|{i}|w{i}|{}|0|%{i}|0|1|claude||working|0|claude|task {i}||||/a{i}", u8::from(i == 1))).collect();
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        let snap = crate::backend::tmux::fixture(&lines);
        App::new(snap, Tab::Agents, "$1".into(), "@1".into(), String::new(), true, false)
    }

    #[test]
    fn the_more_mark_covers_a_place_whole_and_never_a_title_ending_like_one() {
        let lines: Vec<String> = (1..=6).map(|i| format!("$1|s|@{i}|{i}|tests|{}|0|%{i}|0|1|claude||working|0|claude|Fix flaky tests||||/a{i}", u8::from(i == 1))).collect();
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        let mut app = App::new(crate::backend::tmux::fixture(&lines), Tab::Agents, "$1".into(), "@1".into(), String::new(), true, false);
        while app.group != crate::rows::Group::Session {
            app.run(Command::Group);
        }
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        for width in 16..70 {
            let screen = screen(&render(&app, &mut View::new(), width, 10, ""));
            for l in screen.iter().filter(|l| l.contains('↓')) {
                // The title whole or fitted (…), the place whole or gone.
                let (_, end) = l.split_at(l.find('↓').unwrap());
                let before = l[..l.len() - end.len()].trim_end_matches(' ');
                assert!(!before.contains("Fix flaky") || before.ends_with("tests") || before.ends_with('…'), "{width}: {screen:#?}");
                assert!(!before.ends_with("test") && !before.ends_with("tes"), "{width}: {screen:#?}");
            }
        }
    }

    #[test]
    fn the_more_mark_covers_changes_whole() {
        let mut app = many_agents();
        for row in &mut app.rows {
            if let RowKind::Agent { diff, .. } = &mut row.kind {
                *diff = "+1204 −3670 · 2 untracked".into();
            }
        }
        for width in 30..70 {
            // Input, blank, then a header, a card and the first line of the next.
            let lines = screen(&render(&app, &mut View::new(), width, 9, ""));
            let last = lines.iter().find(|l| l.contains('↓')).unwrap_or_else(|| panic!("{width}: {lines:#?}"));
            assert!(!last.contains('+') || last.contains("+1204 −3670 · 2 untracked"), "{width}: {lines:#?}");
        }
    }

    #[test]
    fn rows_past_the_viewport_are_counted_at_its_edges() {
        let mut app = many_agents();
        let mut view = View::new();
        // No room for the preview. Input, blank, then 5 list lines: a header and two cards.
        let term = render(&app, &mut view, 60, 10, "");
        let lines = screen(&term);
        assert!(!lines[2].contains('↑'), "{lines:#?}");
        assert!(lines[7].ends_with("↓ 4 more │"), "on the last list line: {lines:#?}");
        assert!(lines[7].contains("/a2"), "the line keeps its text: {lines:#?}");
        assert_eq!(view.rows.len(), 2, "the hit rects are the rows': {:?}", view.rows);
        assert!(view.rows.iter().all(|(r, ..)| r.y >= 3 && r.bottom() <= 8));
        for _ in 0..5 {
            app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        let term = render(&app, &mut view, 60, 10, "");
        let lines = screen(&term);
        assert!(lines[2].ends_with("↑ 4 more │"), "on the blank under the input: {lines:#?}");
        assert!(!lines.iter().any(|l| l.contains('↓')), "{lines:#?}");
        assert_eq!(beyond(&app.rows, 0, false, 100), (0, 0));
        assert_eq!(beyond(&app.rows, 0, false, 4), (0, 5), "a card cut off counts");
        // The selected row's end is read (a note, a field, what ⏎ does):
        // on the last line, the mark shows only where it covers none of it.
        let mut app = many_agents();
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let wide = screen(&render(&app, &mut View::new(), 60, 10, ""));
        assert!(wide[7].contains("/a2") && wide[7].ends_with("↓ 4 more │"), "beside it: {wide:#?}");
        let narrow = screen(&render(&app, &mut View::new(), 17, 10, ""));
        assert!(narrow[7].contains("/a2") && !narrow[7].contains('↓'), "not over it: {narrow:#?}");
        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        let narrow = screen(&render(&app, &mut View::new(), 17, 10, ""));
        assert!(narrow[7].ends_with("↓ 4 more │"), "over another row's: {narrow:#?}");
    }

    #[test]
    fn a_toggle_the_mark_covers_is_not_there_to_click() {
        // Windows of two panes, the last line's with a name that puts its ▸ at the row's end.
        let name = "n".repeat(60);
        let lines: Vec<String> = (1..=8).flat_map(|i| (0..2).map(move |p| (i, p))).map(|(i, p)| format!("$1|s|@{i}|{i}|{name}|{}|0|%{}|{p}|{}|zsh|||||||||/a", u8::from(i == 1), i * 2 + p, u8::from(p == 0))).collect();
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        let app = App::new(crate::backend::tmux::fixture(&lines), Tab::Sessions, "$1".into(), "@1".into(), String::new(), false, false);
        for (width, height) in [(40, 9), (60, 9), (90, 9), (40, 12)] {
            let mut view = View::new();
            let term = render(&app, &mut view, width, height, "");
            let lines = screen(&term);
            let y = lines.iter().position(|l| l.contains('↓')).unwrap_or_else(|| panic!("{lines:#?}")) as u16;
            let at = lines[y as usize].chars().position(|c| c == '↓').unwrap() as u16;
            assert!(view.toggles.iter().all(|(r, _)| r.y != y || r.right() <= at), "{width}x{height}: {:?} under the mark at {at}: {lines:#?}", view.toggles);
            assert!(view.toggles.iter().any(|(r, _)| r.y < y), "the others stay: {:?}", view.toggles);
        }
        for width in 0..90 {
            for height in 0..14 {
                render(&app, &mut View::new(), width, height, "");
            }
        }
    }

    #[test]
    fn a_menu_taller_than_its_popup_scrolls_to_the_selection() {
        let mut app = many_agents();
        let mut view = View::new();
        let size = (60, 10);
        render(&app, &mut view, size.0, size.1, "");
        app.run(Command::Menu);
        let Mode::Menu(menu) = &app.mode else { panic!("no menu") };
        let n = menu.items.len();
        assert!(n > 8, "more items than the popup has lines: {n}");
        for step in 0..=n {
            let term = render(&app, &mut view, size.0, size.1, "");
            let Mode::Menu(menu) = &app.mode else { panic!("no menu") };
            assert_eq!(menu.selected, step % n, "wraps");
            let lines = screen(&term);
            let row = |r: &Rect| lines[r.y as usize].chars().skip(r.x as usize).take(r.width as usize).collect::<String>();
            // Every drawn item is where its click lands, the selected one too.
            assert!(view.items.iter().any(|(_, i)| *i == menu.selected), "{step}: {:?}", view.items);
            for (r, i) in &view.items {
                assert!(view.menu.unwrap().contains(r.as_position()) && row(r).contains(&menu.items[*i].label), "{step}: {i} at {r:?}: {lines:#?}");
            }
            let first = view.items.first().unwrap().1;
            let menu_rect = view.menu.unwrap();
            assert_eq!(lines[menu_rect.y as usize].contains('↑'), first > 0, "{lines:#?}");
            assert_eq!(lines[menu_rect.bottom() as usize - 1].contains('↓'), view.items.last().unwrap().1 + 1 < n, "{lines:#?}");
            app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        render(&app, &mut view, size.0, size.1, "");
        let Mode::Menu(menu) = &app.mode else { panic!("no menu") };
        assert!(view.items.iter().any(|(_, i)| *i == menu.selected), "wrapping up to the end shows it");
    }

    #[test]
    fn the_preview_title_is_fitted_beside_the_list_too() {
        let app = many_agents();
        let title = format!("session:1.1 · {}", "x".repeat(200));
        for (width, height) in [(120, 20), (70, 30)] {
            let mut view = View::new();
            let lines = screen(&render(&app, &mut view, width, height, &title));
            assert!(lines.iter().any(|l| l.contains("session:1.1 · xxx") && l.contains("x…")), "{width}: {lines:#?}");
        }
    }

    #[test]
    fn a_long_carried_label_and_folder_leave_the_line_its_prompt() {
        let dir = format!("/srv/{}project", "deep/".repeat(20));
        let snap = crate::backend::tmux::fixture(&[&format!("$1|s|@1|1|{}|1|0|%1|0|1|zsh|||||||||{dir}", "w".repeat(80)), "$1|s|@2|2|v|0|0|%2|0|1|zsh|||||||||/a"]);
        let mut app = App::new(snap, Tab::Sessions, "$1".into(), "@1".into(), String::new(), true, false);
        app.run(Command::Move);
        assert!(matches!(app.mode, Mode::Move(_)));
        for width in [80, 90, 120] {
            let lines = screen(&render(&app, &mut View::new(), width, 12, ""));
            assert!(lines[1].contains("› filter") && lines[1].contains("⇅ moving") && lines[1].contains('…'), "{width}: {lines:#?}");
        }
        app.run(Command::Cancel);
        app.run(Command::NewWindow);
        let Mode::Create(c) = &app.mode else { panic!("not creating") };
        let t = Theme::default();
        for width in 0..120 {
            let lines = create_lines(c, false, width, "", "window", &t);
            assert!(lines.iter().all(|l| l.width() <= (width as usize).max(8)), "{width}: {lines:?}");
        }
        let line = create_lines(c, false, 60, "", "window", &t)[0].to_string();
        assert!(line.contains("▏window name") && line.contains("…/deep/project"), "{line}");
    }

    #[test]
    fn remote_agents_show_their_host_and_cards_where_they_run() {
        let mut snap = crate::backend::tmux::fixture(&[
            "$1|ml|@1|1|train|1|0|%1|0|1|claude||working|0|claude|fit||||devbox:/home/me/source/company/project",
            "$1|ml|@1|1|train|1|0|%2|1|0|claude||asking|0|claude|eval||||",
            "$2|web|@2|1|api|1|0|%3|0|1|claude||working|0|claude|serve||||/srv/api",
        ]);
        snap.sessions[0].host = "devbox".into();
        let t = Theme::default();
        let rows = crate::rows::agents(&snap, &mut Filter::new(), crate::rows::Group::Status, None, &mut crate::paths::PathCache::new("/home/me".into()), 0, false);
        let cards: Vec<&Row> = rows.iter().filter(|r| matches!(r.kind, RowKind::Agent { .. })).collect();
        let first = |r: &Row, w: u16| row_lines(r, false, false, w, None, "window", &t).0.remove(0);
        // Waiting first.
        let second = |r: &Row, w: u16| row_lines(r, false, false, w, None, "window", &t).0.remove(1);
        let line = second(cards[1], 60);
        assert!(line.to_string().contains("devbox:~/source/company/project "), "{line}");
        assert_eq!(line.spans.iter().find(|s| s.content == "devbox:").unwrap().style, Style::new().fg(t.hint));
        assert!(second(cards[1], 24).to_string().contains("devbox:"), "the host outlasts the path");
        assert!(!first(cards[1], 60).to_string().contains('@'), "the folder says it: {}", first(cards[1], 60));
        assert!(first(cards[0], 60).to_string().ends_with("@devbox"), "no path to carry it: {}", first(cards[0], 60));
        assert!(!first(cards[2], 60).to_string().contains('@'));
        assert_eq!(crate::rows::pane_title(&snap, "%1"), "ml:1.0 @devbox · claude");
        // Each card ends its second line with where it runs.
        let rows = crate::rows::agents(&snap, &mut Filter::new(), crate::rows::Group::Folder, None, &mut crate::paths::PathCache::new("/home/me".into()), 0, false);
        let text: Vec<String> = rows.iter().flat_map(|r| row_lines(r, false, false, 60, None, "window", &t).0).map(|l| l.to_string()).collect();
        assert!(text.contains(&" devbox:~/source/company/project 1".to_string()), "{text:#?}");
        assert!(text.windows(2).any(|l| l[0].ends_with("fit") && l[1].ends_with(" ml:train")), "{text:#?}");
        assert!(text.windows(2).any(|l| l[0].ends_with("serve") && l[1].ends_with(" web:api")), "{text:#?}");
        for width in 0..80 {
            for r in rows.iter().filter(|r| matches!(r.kind, RowKind::Agent { .. })) {
                assert!(row_lines(r, true, false, width, None, "window", &t).0.iter().all(|l| l.width() <= (width as usize).max(9)), "{width}");
            }
        }
    }
}
