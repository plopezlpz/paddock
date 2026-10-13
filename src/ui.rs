//! Drawing. Every colour comes from the Theme (see theme.rs). Each draw
//! records in the View where rows and buttons landed, for the mouse.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Button, Tab, View};
use crate::model::State;
use crate::rows::{Row, RowKind};
use crate::theme::Theme;

/// Narrower than this (a phone), the compact layout.
const COMPACT_BELOW: u16 = 80;
/// The preview shows from this many inner columns (list and preview
/// together), i.e. a popup at least 102 wide.
const PREVIEW_FROM: u16 = 100;

fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

fn dot(t: &Theme, s: Option<State>) -> Span<'static> {
    match s {
        Some(State::Idle) => Span::styled("○", t.state(State::Idle)),
        Some(s) => Span::styled("●", t.state(s)),
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

/// The end of S that fits in MAX display columns (the text being typed).
fn tail(s: &str, max: usize) -> &str {
    let mut w = 0;
    for (i, c) in s.char_indices().rev() {
        w += c.width().unwrap_or(0);
        if w > max {
            return &s[i + c.len_utf8()..];
        }
    }
    s
}

/// The previewed pane: a title line and its screen.
pub struct Preview<'a> {
    pub title: &'a str,
    pub body: &'a Text<'static>,
}

pub fn draw(f: &mut Frame, app: &App, view: &mut View, preview: &Preview<'_>, t: &Theme) {
    let area = f.area();
    view.compact = area.width < COMPACT_BELOW;
    view.rows.clear();
    view.buttons.clear();

    // Frame title: the tabs (buttons), then the agent counts.
    let mut title = vec![Span::raw(" ")];
    let mut x = area.x + 2;
    for tab in [Tab::Sessions, Tab::Agents] {
        let name = tab.label();
        let style = if tab == app.tab { bold().fg(t.accent) } else { Style::new() };
        view.buttons.push((Rect::new(x, area.y, name.len() as u16, 1), Button::Tab(tab)));
        title.push(Span::styled(name, style));
        title.push(Span::raw("  "));
        x += name.len() as u16 + 2;
    }
    let counts = app.snap.counts();
    for s in State::ALL.into_iter().rev() {
        let n = counts[s as usize];
        if n > 0 {
            title.push(dot(t, Some(s)));
            title.push(Span::raw(if view.compact { format!(" {n} ") } else { format!(" {n} {} ", s.label()) }));
        }
    }
    let frame = Block::new()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(t.border())
        // The title is drawn over the border and would keep its dim or
        // colour: start from the terminal's plain text.
        .title_style(Style::new().fg(Color::Reset).remove_modifier(Modifier::DIM))
        .title(Line::from(title));
    let inner = frame.inner(area);
    f.render_widget(frame, area);

    let [input, rule, body, footer] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
    let pad = |r: Rect| Rect { x: r.x + 1, width: r.width.saturating_sub(2), ..r };

    draw_input(f, app, view.compact, pad(input), t);
    f.render_widget(Paragraph::new(Line::styled("─".repeat(rule.width as usize), t.muted())), rule);

    view.preview_fits = body.width >= PREVIEW_FROM;
    let (list, side) = if app.preview_on && view.preview_fits {
        let [l, r] = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(body);
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
        f.render_widget(
            Paragraph::new(vec![
                Line::from(vec![Span::styled("live output  ", bold().fg(t.hint)), Span::styled(preview.title, t.muted())]),
                Line::styled("─".repeat(head.width as usize), t.muted()),
            ]),
            head,
        );
        // Bottom-align the tail of the pane, like the end of a terminal.
        let skip = preview.body.lines.len().saturating_sub(text.height as usize);
        f.render_widget(Paragraph::new(preview.body.lines[skip..].to_vec()), text);
    }

    draw_footer(f, app, view, pad(footer), t);
}

/// Prompt and query (its end, when long) or a hint, and the number of
/// matching rows on the right; while editing, the description instead.
fn draw_input(f: &mut Frame, app: &App, compact: bool, area: Rect, t: &Theme) {
    if let Some(edit) = &app.edit {
        let mut spans = vec![Span::styled("✎ ", Style::new().fg(t.accent))];
        if edit.text.is_empty() {
            spans.push(Span::styled(if compact { "empty: automatic" } else { "describe this agent (empty: the automatic description)" }, t.muted()));
        } else {
            spans.push(Span::raw(tail(&edit.text, (area.width as usize).saturating_sub(3)).to_string()));
        }
        spans.push(Span::styled("▏", Style::new().fg(t.accent)));
        f.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }
    let matches = app.rows.iter().filter(|r| r.target.is_some() && r.score.is_some()).count().to_string();
    let [left, right] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(matches.len() as u16 + 1)]).areas(area);
    let mut spans = vec![Span::styled("› ", Style::new().fg(t.accent))];
    if app.query.is_empty() {
        let ghost = match app.tab {
            _ if compact => "filter",
            Tab::Sessions => "filter sessions, windows, folders",
            Tab::Agents => "filter agents by task, project or state",
        };
        spans.push(Span::styled(ghost, t.muted()));
    } else {
        spans.push(Span::raw(tail(&app.query, (left.width as usize).saturating_sub(3)).to_string()));
        spans.push(Span::styled("▏", Style::new().fg(t.accent)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), left);
    f.render_widget(Paragraph::new(Line::styled(matches, t.muted())).right_aligned(), right);
}

/// Key hints that are also buttons; compact (a phone, tapped) they are
/// words alone. A debug status replaces them.
fn draw_footer(f: &mut Frame, app: &App, view: &mut View, area: Rect, t: &Theme) {
    if !app.status.is_empty() {
        f.render_widget(Paragraph::new(Line::styled(app.status.as_str(), t.muted())), area);
        return;
    }
    let other = app.tab.other();
    let group = format!("group: {}", app.group.label());
    let mut items: Vec<(&str, &str, Button)> = Vec::new();
    if app.edit.is_some() {
        items.extend([("⏎", "save", Button::Save), ("^g", "generate", Button::Generate), ("esc", "cancel", Button::Cancel)]);
    } else {
        items.push(("⏎", "open", Button::Open));
        items.push(if view.compact { ("", other.label(), Button::Tab(other)) } else { ("⇥", "tab", Button::Tab(other)) });
        match app.tab {
            Tab::Sessions => items.push(("→", "panes", Button::Panes)),
            Tab::Agents => {
                items.push(("^g", if view.compact { "group" } else { &group }, Button::Group));
                if app.can_describe {
                    items.push(("^e", if view.compact { "edit" } else { "describe" }, Button::Describe));
                }
            }
        }
        if !view.compact {
            items.push(("^o", "preview", Button::Preview));
        }
        items.push(("esc", "close", Button::Close));
    }

    let mut spans = Vec::new();
    let mut x = area.x;
    for (key, label, button) in items {
        // A button spans its text and the gap after it.
        let text = if view.compact { format!("{label}   ") } else { format!("{key} {label}   ") };
        let w = (text.width() as u16).min(area.right().saturating_sub(x));
        view.buttons.push((Rect::new(x, area.y, w, 1), button));
        x += w;
        if view.compact {
            spans.push(Span::styled(text, Style::new().fg(t.hint)));
        } else {
            spans.push(Span::styled(key, Style::new().fg(t.hint)));
            spans.push(Span::styled(format!(" {label}   "), t.muted()));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn row_lines(row: &Row, selected: bool, compact: bool, width: u16, t: &Theme) -> Vec<Line<'static>> {
    let width = width as usize;
    let mark = if selected { Span::styled("▌", Style::new().fg(t.accent)) } else { Span::raw(" ") };
    let sel = |s: Style| if selected { s.add_modifier(Modifier::BOLD) } else { s };
    let tree = |s: &str| Span::styled(s.to_string(), t.muted());
    match &row.kind {
        RowKind::Session { name, windows, current } => {
            let name = if *current { Span::styled(format!(" {name} "), t.badge()) } else { Span::styled(name.clone(), sel(bold())) };
            let mut line = vec![mark, Span::raw(" "), name];
            if !compact {
                let n = if *windows == 1 { "1 window".to_string() } else { format!("{windows} windows") };
                line.push(Span::styled(format!("  {n}"), t.muted()));
            }
            vec![Line::from(line)]
        }
        RowKind::Window { index, name, meta, state, active, last, expanded } => {
            let fold = match expanded {
                Some(true) => " ▾",
                Some(false) => " ▸",
                None => "",
            };
            let idx = if *active { Style::new().fg(t.accent) } else { t.muted() };
            let mut first = vec![mark.clone(), tree(if *last { " └─ " } else { " ├─ " }), Span::styled(format!("{index} "), idx), dot(t, *state), Span::raw(" ")];
            let indent = cols(&first);
            first.extend([Span::styled(name.clone(), sel(Style::new())), Span::styled(fold, t.muted())]);
            if !compact {
                first.push(Span::styled(format!("  {meta}"), t.muted()));
                return vec![Line::from(first)];
            }
            // Compact: the folder under the name.
            let mut second = vec![mark, tree(if *last { "    " } else { " │  " })];
            second.push(Span::raw(" ".repeat(indent.saturating_sub(cols(&second)))));
            second.push(Span::styled(fit(meta, width.saturating_sub(indent)), t.muted()));
            vec![Line::from(first), Line::from(second)]
        }
        RowKind::Pane { index, command, state, last, parent_last } => vec![Line::from(vec![
            mark,
            tree(if *parent_last { "    " } else { " │  " }),
            tree(if *last { "└─ " } else { "├─ " }),
            Span::styled(format!(".{index} "), t.muted()),
            dot(t, *state),
            Span::raw(" "),
            Span::styled(command.clone(), sel(Style::new())),
        ])],
        RowKind::Agent { state, stale, title, generating, session, meta, age } => {
            // A stale state is unconfirmed: "working?".
            let unsure = if *stale { "?" } else { "" };
            let busy = Span::styled(if *generating { "  ✦ generating…" } else { "" }, Style::new().fg(t.hint));
            let mut first = vec![mark.clone(), Span::raw(" "), dot(t, Some(*state)), Span::raw(" ")];
            first.push(if title.is_empty() {
                Span::styled("no description", t.muted().add_modifier(Modifier::ITALIC))
            } else {
                Span::styled(fit(title, width.saturating_sub(cols(&first) + busy.width())), sel(bold()))
            });
            first.push(busy);
            let mut second = vec![mark, Span::raw("   ")];
            if compact {
                // Compact: where and how long; the group header or the dot
                // says the state.
                let tail = [Span::styled(" · ", t.muted()), Span::styled(format!("{age}{unsure}"), t.state(*state))];
                second.push(Span::styled(fit(session, width.saturating_sub(cols(&second) + cols(&tail))), t.muted()));
                second.extend(tail);
                return vec![Line::from(first), Line::from(second)];
            }
            second.extend([Span::styled(format!("{}{unsure} {age}", state.label()), t.state(*state)), Span::styled("  ·  ", t.muted())]);
            second.push(Span::styled(fit(meta, width.saturating_sub(cols(&second))), t.muted()));
            vec![Line::from(first), Line::from(second), Line::raw("")]
        }
        RowKind::Header { label, count } => vec![Line::from(vec![
            Span::styled(format!(" {label} "), t.muted().add_modifier(Modifier::BOLD)),
            Span::styled(count.to_string(), t.muted()),
        ])],
    }
}

fn draw_rows(f: &mut Frame, app: &App, view: &mut View, area: Rect, t: &Theme) {
    view.scroll(&app.rows, app.selected, area.height);
    let mut y = area.y;
    let mut lines = Vec::new();
    for (i, row) in app.rows.iter().enumerate().skip(view.offset) {
        if y >= area.bottom() {
            break;
        }
        let h = row.height(view.compact);
        if let Some(target) = &row.target {
            // A row cut off at the bottom is only clickable where it shows.
            view.rows.push((Rect::new(area.x, y, area.width, h.min(area.bottom() - y)), target.clone()));
        }
        let drawn = row_lines(row, i == app.selected, view.compact, area.width, t);
        debug_assert_eq!(drawn.len(), h as usize, "Row::height");
        lines.extend(drawn);
        y += h;
    }
    if app.rows.is_empty() {
        let msg = match app.tab {
            Tab::Agents if app.query.trim().is_empty() => "no agents running",
            _ => "no matches",
        };
        lines.push(Line::styled(format!("  {msg}"), t.muted()));
    }
    f.render_widget(Paragraph::new(lines), area);
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
            RowKind::Agent { state: State::Working, stale: false, title: "t".into(), generating: true, session: "s".into(), meta: "m".into(), age: "5m".into() },
            RowKind::Header { label: "working".into(), count: 1 },
        ];
        let t = Theme::from_values(&[]);
        for kind in kinds {
            let row = Row { target: None, kind, preview: None, score: None };
            for compact in [false, true] {
                assert_eq!(row_lines(&row, true, compact, 40, &t).len(), row.height(compact) as usize, "compact: {compact}");
            }
        }
    }

    #[test]
    fn fits_by_display_width() {
        assert_eq!(fit("abcdef", 6), "abcdef");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("日本語テキスト", 7), "日本語…");
        assert_eq!(fit("abc", 0), "");
        assert_eq!(tail("hello world", 5), "world");
        assert_eq!(tail("日本語", 4), "本語");
        assert_eq!(tail("ab", 5), "ab");
    }
}
