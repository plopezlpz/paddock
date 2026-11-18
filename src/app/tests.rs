use super::*;
use std::time::Duration;
use crate::backend::tmux::fixture;
use crate::model::Pane;
use crate::theme::Theme;
use crate::diffstat::Diff;
use crate::ui::{self, Preview};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyEventKind, KeyEventState, MouseButton, MouseEvent, MouseEventKind};
use ratatui::style::Modifier;
use ratatui::text::Text;

/// An app opened from window WINDOW of session $1, with a suggest command.
fn new(snap: Snapshot, tab: Tab, window: &str) -> App {
    App::new(snap, tab, "$1".into(), window.into(), String::new(), true, false)
}

/// Agents in two folders: header, card, card, header, card, card.
fn agents_app() -> App {
    let snap = fixture(&[
        "$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|one||||/a",
        "$1|s|@1|1|w|1|0|%2|1|0|claude||working|0|claude|two||||/a",
        "$1|s|@2|2|w|0|0|%3|0|1|claude||working|0|claude|three||||/b",
        "$1|s|@2|2|w|0|0|%4|1|0|claude||working|0|claude|four||||/b",
    ]);
    let mut app = new(snap, Tab::Agents, "@1");
    app.group = Group::Folder;
    app.rebuild(false);
    app
}

fn sessions_app(window: &str) -> App {
    let snap = fixture(&[
        "$1|work|@1|1|edit|1|0|%1|0|1|nvim|||||||||/h/a",
        "$1|work|@1|1|edit|1|0|%2|1|0|zsh|||||||||/h/a",
        "$1|work|@2|2|logs|0|0|%3|0|1|tail|||||||||/h/b",
        "$2|play|@3|1|game|1|0|%4|0|1|codex||asking|0|codex|deploy||||/h/c",
    ]);
    new(snap, Tab::Sessions, window)
}

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent { code, modifiers, kind: KeyEventKind::Press, state: KeyEventState::NONE }
}

fn ctrl(app: &mut App, c: char) -> Action {
    app.on_key(key(KeyCode::Char(c), KeyModifiers::CONTROL))
}

fn type_(app: &mut App, text: &str) {
    text.chars().for_each(|c| drop(app.on_key(key(KeyCode::Char(c), KeyModifiers::NONE))));
}

fn mouse(app: &mut App, view: &View, kind: MouseEventKind, at: (u16, u16), now: Instant) -> Action {
    app.on_mouse(MouseEvent { kind, column: at.0, row: at.1, modifiers: KeyModifiers::NONE }, view, now)
}

/// A press and release at AT, at NOW: the release's action.
fn click_at(app: &mut App, view: &View, button: MouseButton, at: (u16, u16), now: Instant) -> Action {
    let down = mouse(app, view, MouseEventKind::Down(button), at, now);
    assert!(matches!(down, Action::None), "a press does nothing yet: {down:?}");
    mouse(app, view, MouseEventKind::Up(button), at, now)
}

fn click(app: &mut App, view: &View, button: MouseButton, at: (u16, u16)) -> Action {
    click_at(app, view, button, at, Instant::now())
}

fn draw(app: &App, view: &mut View, width: u16, height: u16) -> Terminal<TestBackend> {
    let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
    let body = Text::default();
    term.draw(|f| ui::draw(f, app, view, &Preview { title: "", body: &body }, &Theme::default())).unwrap();
    term
}

fn screen(term: &Terminal<TestBackend>) -> Vec<String> {
    let buf = term.backend().buffer();
    (0..buf.area.height).map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>().trim_end().to_string()).collect()
}

/// Where TEXT starts on screen.
fn find(term: &Terminal<TestBackend>, text: &str) -> (u16, u16) {
    let lines = screen(term);
    let (y, line) = lines.iter().enumerate().find(|(_, l)| l.contains(text)).unwrap_or_else(|| panic!("{text:?} not in {lines:#?}"));
    let col = line[..line.find(text).unwrap()].chars().count();
    (col as u16, y as u16)
}

fn selected_title(app: &App) -> String {
    match &app.rows[app.selected].kind {
        RowKind::Agent { title, .. } => title.clone(),
        RowKind::Window { name, .. } => name.clone(),
        RowKind::Pane { command, .. } => command.clone(),
        RowKind::Session { name, .. } => name.clone(),
        RowKind::Header { .. } => "header".into(),
        RowKind::Hidden { .. } => "hidden".into(),
        RowKind::New => "new".into(),
    }
}

fn field(app: &App) -> Option<&str> {
    match &app.mode {
        Mode::Rename(r) => Some(r.field.text()),
        _ => None,
    }
}

/// Whether the "+ new" row is listed.
fn naming(app: &App) -> bool {
    app.rows.iter().any(|r| matches!(r.kind, RowKind::New))
}

fn renaming(app: &App) -> Option<&Rename> {
    match &app.mode {
        Mode::Rename(r) => Some(r),
        _ => None,
    }
}

#[test]
fn opens_on_the_current_window_and_selects_the_best_match() {
    let mut app = sessions_app("@2");
    assert_eq!(selected_title(&app), "logs");
    type_(&mut app, "codex");
    assert_eq!(selected_title(&app), "game", "the window whose agent matches");
}

#[test]
fn sessions_that_come_in_after_opening_select_the_current_window() {
    let full = sessions_app("@2").snap;
    let mut app = new(fixture(&[]), Tab::Sessions, "@2");
    app.set_snapshot(full);
    assert_eq!(selected_title(&app), "logs", "as if it had opened on them");
}

#[test]
fn agents_open_on_the_agent_you_are_in() {
    let snap = || {
        fixture(&[
            "$1|s|@1|1|w|1|0|%1|0|0|claude||working|0|claude|one||||/a",
            "$1|s|@1|1|w|1|0|%2|1|1|claude||asking|0|claude|two||||/a",
            "$1|s|@2|2|w|0|0|%3|0|1|nvim|||||||||/b",
            "$1|s|@2|2|w|0|0|%4|1|0|codex||idle|0|codex|four||||/b",
            "$1|s|@3|3|w|0|0|%5|0|1|claude||working|0|claude|five||||/c",
        ])
    };
    assert_eq!(selected_title(&new(snap(), Tab::Agents, "@1")), "two", "the active pane");
    assert_eq!(selected_title(&new(snap(), Tab::Agents, "@2")), "four", "the window's agent when its active pane is none");
    let mut app = new(snap(), Tab::Sessions, "@1");
    app.selected = app.position(&Target::Window { session: "$1".into(), window: "@3".into() }).unwrap();
    app.run(Command::Tab(Tab::Agents));
    assert_eq!(selected_title(&app), "five", "the window selected in sessions");
}

#[test]
fn moving_skips_headers_and_wraps_only_when_asked() {
    let mut app = agents_app();
    assert_eq!(app.selected, 1);
    app.move_by(1, true);
    assert_eq!(app.selected, 2);
    app.move_by(1, true);
    assert_eq!(app.selected, 4, "skips the header");
    app.move_by(-1, true);
    assert_eq!(app.selected, 2);
    app.move_by(-1, true);
    app.move_by(-1, true);
    assert_eq!(app.selected, 5, "up from the first card wraps to the last");
    app.move_by(1, false);
    assert_eq!(app.selected, 5, "no wrapping when not asked");
    app.move_by(10, false);
    assert_eq!(app.selected, 5, "a page stops at the end");
    app.move_by(-10, false);
    assert_eq!(app.selected, 1, "and at the first card, not the header");
}

#[test]
fn a_vanished_selection_moves_to_the_nearest_row() {
    let mut app = agents_app();
    app.selected = 4; // "three"
    app.set_snapshot(fixture(&[
        "$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|one||||/a",
        "$1|s|@1|1|w|1|0|%2|1|0|claude||working|0|claude|two||||/a",
        "$1|s|@2|2|w|0|0|%4|1|0|claude||working|0|claude|four||||/b",
    ]));
    assert_eq!(selected_title(&app), "four");
}

#[test]
fn a_window_without_panes_opens_and_draws() {
    // Its only pane line was malformed, so parse left it out.
    let snap = fixture(&["$1|s|@1|1|w|1|0|broken", "$1|s|@2|2|v|0|0|%2|0|1|claude||working|0|claude|two||||/a"]);
    let mut app = new(snap, Tab::Sessions, "@1");
    assert_eq!(app.selected_target(), Some(&Target::Window { session: "$1".into(), window: "@1".into() }));
    assert_eq!(app.preview_pane(&View::new()), None);
    app.expand(Some(true));
    draw(&app, &mut View::new(), 80, 12);
    app.run(Command::Tab(Tab::Agents));
    draw(&app, &mut View::new(), 80, 12);
}

#[test]
fn the_selection_follows_an_agent_into_another_group() {
    let line = |state: &str| format!("$1|s|@1|1|w|1|0|%2|1|0|claude||{state}|0|claude|two||||/a");
    let snap = |state: &str| fixture(&["$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|one||||/a", &line(state)]);
    let mut app = new(snap("working"), Tab::Agents, "@1");
    app.move_by(1, false);
    assert_eq!(selected_title(&app), "two");
    app.set_snapshot(snap("asking"));
    assert_eq!(selected_title(&app), "two");
    assert!(matches!(&app.rows[app.selected - 1].kind, RowKind::Header { label, .. } if label == "waiting"));
}

#[test]
fn switching_tabs_keeps_the_selection_or_falls_back_to_its_window() {
    let mut app = agents_app();
    app.move_by(2, false);
    assert_eq!(selected_title(&app), "three");
    app.run(Command::Tab(Tab::Sessions));
    assert_eq!(app.selected_target(), Some(&Target::Window { session: "$1".into(), window: "@2".into() }), "panes are not listed");
    let mut app = agents_app();
    app.run(Command::Tab(Tab::Sessions));
    assert_eq!(app.selected_target(), Some(&Target::Window { session: "$1".into(), window: "@1".into() }));
    app.move_by(2, false);
    app.expand(Some(true));
    app.move_by(2, false);
    let pane = Target::Pane { session: "$1".into(), window: "@2".into(), pane: "%4".into() };
    assert_eq!(app.selected_target(), Some(&pane));
    app.run(Command::Tab(Tab::Agents));
    assert_eq!(selected_title(&app), "four");
    app.run(Command::Tab(Tab::Sessions));
    assert_eq!(app.selected_target(), Some(&pane), "back on the listed pane");
}

#[test]
fn a_window_linked_twice_keeps_its_selected_row() {
    let mut app = new(fixture(&["$1|s|@1|1|w|1|0|%1|0|1|zsh|||||||||/a", "$1|s|@1|2|w|0|0|%1|0|1|zsh|||||||||/a"]), Tab::Sessions, "@1");
    assert_eq!(app.selected, 1);
    app.selected = 2;
    app.refresh();
    assert_eq!(app.selected, 2);
}

#[test]
fn a_negation_keeps_the_selection() {
    let mut app = agents_app();
    app.selected = 4;
    app.paste("!zzz");
    assert_eq!(selected_title(&app), "three", "every row ties");
}

#[test]
fn scrolling_keeps_the_group_header_and_fills_the_viewport() {
    let app = agents_app();
    let mut view = View::new();
    // Room for two cards (3 lines each) and a header.
    view.scroll(&app.rows, 5, 7);
    assert_eq!(view.offset, 3, "header of the second group, then its cards");
    view.offset = 4;
    view.scroll(&app.rows, 4, 7);
    assert_eq!(view.offset, 3, "scrolling up to a group's first card shows its header");
    // Rows shrink below the offset: pull back instead of showing blanks.
    view.offset = 5;
    view.scroll(&app.rows, 5, 100);
    assert_eq!(view.offset, 0);
}

#[test]
fn scrolling_keeps_a_windows_session_in_view() {
    let app = sessions_app("@1");
    let work = app.rows.iter().position(|r| matches!(&r.kind, RowKind::Session { name, .. } if name == "work")).unwrap();
    let logs = app.rows.iter().position(|r| matches!(&r.kind, RowKind::Window { name, .. } if name == "logs")).unwrap();
    let mut view = View::new();
    // Scrolled up onto logs, the second window of work: work shows above it.
    view.offset = logs;
    view.scroll(&app.rows, logs, 4);
    assert_eq!(view.offset, work);
    // No room for both: the selection wins.
    view.offset = logs;
    view.scroll(&app.rows, logs, 1);
    assert_eq!(view.offset, logs);
}

#[test]
fn a_double_click_opens_and_a_single_one_selects() {
    let mut app = agents_app();
    let mut view = View::new();
    let term = draw(&app, &mut view, 120, 20);
    let t0 = Instant::now();
    let at = find(&term, "two");
    assert!(matches!(click_at(&mut app, &view, MouseButton::Left, at, t0), Action::None));
    assert_eq!(selected_title(&app), "two");
    // Already selected, a slow second click still doesn't open.
    assert!(matches!(click_at(&mut app, &view, MouseButton::Left, at, t0 + Duration::from_secs(2)), Action::None));
    let open = click_at(&mut app, &view, MouseButton::Left, at, t0 + Duration::from_millis(2300));
    assert!(matches!(&open, Action::Activate(Target::Pane { pane, .. }) if pane == "%2"), "{open:?}");
}

#[test]
fn opening_waits_for_the_release_on_the_same_row() {
    let mut app = agents_app();
    let mut view = View::new();
    let term = draw(&app, &mut view, 120, 20);
    let (two, three) = (find(&term, "two"), find(&term, "three"));
    let t0 = Instant::now();
    click_at(&mut app, &view, MouseButton::Left, two, t0);
    assert!(matches!(mouse(&mut app, &view, MouseEventKind::Down(MouseButton::Left), two, t0), Action::None), "the press never opens");
    assert!(matches!(mouse(&mut app, &view, MouseEventKind::Up(MouseButton::Left), three, t0), Action::None), "released elsewhere");
    assert!(matches!(mouse(&mut app, &view, MouseEventKind::Up(MouseButton::Left), two, t0), Action::None), "a release without a press");
}

#[test]
fn on_a_phone_a_tap_on_the_selected_row_opens() {
    let mut app = agents_app();
    let mut view = View::new();
    let term = draw(&app, &mut view, 46, 20);
    assert!(view.compact);
    let at = find(&term, "two");
    let t0 = Instant::now();
    assert!(matches!(click_at(&mut app, &view, MouseButton::Left, at, t0), Action::None));
    let open = click_at(&mut app, &view, MouseButton::Left, at, t0 + Duration::from_secs(3));
    assert!(matches!(open, Action::Activate(_)));
}

#[test]
fn clicks_hit_list_rows_only() {
    let mut app = agents_app();
    let mut view = View::new();
    // 8 list lines: header, card, card, blank and header, then a card cut off after one line.
    let term = draw(&app, &mut view, 120, 13);
    assert!(view.preview_fits);
    let (rect, _, _) = view.rows.iter().find(|(_, _, target)| *target == pane("$1", "@2", "%3")).unwrap();
    let (x, y) = (rect.x + 4, rect.y);
    assert!(screen(&term)[y as usize].contains("three"), "only the title line fits");
    assert!(!screen(&term)[y as usize + 1].contains("/b"), "the folder is below the list");
    let before = app.selected;
    click(&mut app, &view, MouseButton::Left, (x, y - 1));
    assert_eq!(app.selected, before, "headers are not selectable");
    click(&mut app, &view, MouseButton::Right, (x, y - 1));
    assert_eq!(app.selected, before, "not even by right-click");
    assert!(matches!(app.mode, Mode::List), "and no menu");
    click(&mut app, &view, MouseButton::Left, (x + 70, y));
    assert_eq!(app.selected, before, "the preview is not the list");
    click(&mut app, &view, MouseButton::Left, (x, y + 1));
    assert_eq!(app.selected, before, "the cut-off card ends with the list");
    click(&mut app, &view, MouseButton::Left, (x, y));
    assert_eq!(selected_title(&app), "three");
}

#[test]
fn a_click_resolves_to_what_was_drawn() {
    let mut app = agents_app();
    let mut view = View::new();
    let term = draw(&app, &mut view, 120, 30);
    let at = find(&term, "four");
    // A key that empties the list, then a click on where a row was.
    type_(&mut app, "z");
    assert!(app.rows.is_empty());
    assert!(matches!(click(&mut app, &view, MouseButton::Left, at), Action::None));
}

#[test]
fn a_click_on_a_window_linked_twice_picks_the_copy_clicked() {
    let mut app = new(fixture(&["$1|s|@1|1|w|1|0|%1|0|1|zsh|||||||||/a", "$1|s|@1|2|w|0|0|%1|0|1|zsh|||||||||/a"]), Tab::Sessions, "@1");
    let mut view = View::new();
    let term = draw(&app, &mut view, 120, 20);
    let (_, y) = find(&term, "2   w");
    click(&mut app, &view, MouseButton::Left, (6, y));
    assert_eq!(app.selected, 2, "{:#?}", screen(&term));
}

#[test]
fn the_triangle_toggles_a_windows_panes() {
    let mut app = sessions_app("@1");
    let mut view = View::new();
    let term = draw(&app, &mut view, 120, 20);
    assert_eq!(app.rows.len(), 5);
    let open = click(&mut app, &view, MouseButton::Left, find(&term, "▸"));
    assert!(matches!(open, Action::None), "the triangle doesn't open the window");
    assert_eq!(app.rows.len(), 7, "it lists the panes");
    let term = draw(&app, &mut view, 120, 20);
    click(&mut app, &view, MouseButton::Left, find(&term, "▾"));
    assert_eq!(app.rows.len(), 5);
}

#[test]
fn title_and_footer_are_buttons() {
    let mut app = sessions_app("@1");
    let mut view = View::new();
    let term = draw(&app, &mut view, 46, 20);
    click(&mut app, &view, MouseButton::Left, find(&term, "agents"));
    assert_eq!(app.tab, Tab::Agents);
    let term = draw(&app, &mut view, 46, 20);
    let lines = screen(&term);
    assert!(lines.iter().any(|l| l.contains("open   sessions   rename   menu") && l.ends_with("close │")), "{lines:#?}");
    click(&mut app, &view, MouseButton::Left, find(&term, "rename"));
    assert!(renaming(&app).is_some());
    app.run(Command::Cancel);
    let term = draw(&app, &mut view, 46, 20);
    assert!(matches!(click(&mut app, &view, MouseButton::Left, find(&term, "close")), Action::Quit));
}

#[test]
fn the_menu_has_every_hint_with_its_key() {
    let mut app = sessions_app("@1");
    app.expanded.insert(window("$1", "@1"));
    let view = View::new();
    for tab in [Tab::Sessions, Tab::Agents] {
        app.run(Command::Tab(tab));
        let rows: Vec<usize> = (0..app.rows.len()).filter(|&i| app.selectable(i)).collect();
        for i in rows {
            app.selected = i;
            let menu = app.menu_items(view.preview_fits);
            for h in app.hints(&view).0.into_iter().filter(|h| h.command != Command::Menu) {
                assert!(menu.iter().any(|m| m.command == h.command && m.key == h.key), "{h:?} on row {i} of {tab:?}: {menu:?}");
            }
        }
    }
}

#[test]
fn the_footer_keeps_close_and_the_menu_and_folds_the_rest_into_it() {
    let mut app = agents_app();
    let mut view = View::new();
    let lines = screen(&draw(&app, &mut view, 38, 20));
    let footer = &lines[lines.len() - 2];
    assert!(footer.contains("open") && footer.contains("rename") && footer.contains("menu") && footer.ends_with("close │"), "{lines:#?}");
    assert!(!footer.contains("sessions"), "the tab switch folds first: {lines:#?}");
    let term = draw(&app, &mut view, 38, 20);
    click(&mut app, &view, MouseButton::Left, find(&term, "menu"));
    let Mode::Menu(menu) = &app.mode else { panic!("no menu: {:?}", app.mode) };
    assert!(menu.items.iter().any(|i| i.command == Command::Group), "{:?}", menu.items);
    // Wide enough, keys go with the words, and ^a opens the same menu.
    app.run(Command::Cancel);
    let lines = screen(&draw(&app, &mut view, 70, 20));
    assert!(view.compact && lines[lines.len() - 2].contains("^a menu") && lines[lines.len() - 2].ends_with("esc close │"), "{lines:#?}");
    ctrl(&mut app, 'a');
    assert!(matches!(&app.mode, Mode::Menu(m) if m.items.iter().any(|i| i.command == Command::Group)));
    ctrl(&mut app, 'a');
    assert!(matches!(app.mode, Mode::List), "^a again closes it");
    // At 80-101 columns the preview can't show: no hint for it.
    app.run(Command::Cancel);
    let lines = screen(&draw(&app, &mut view, 90, 20));
    assert!(!view.preview_fits);
    assert!(!lines.iter().any(|l| l.contains("preview")), "{lines:#?}");
    app.preview_fits = view.preview_fits; // as the loop keeps it
    ctrl(&mut app, 'a');
    assert!(matches!(&app.mode, Mode::Menu(m) if !m.items.iter().any(|i| i.command == Command::Preview)), "nor in the menu");
}

#[test]
fn selected_agent_does_not_expose_its_full_path_below_the_filter() {
    let snap = fixture(&[
        "$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|one||||/h/source/company/main-repo/worktrees/topic/src",
        "$1|s|@1|1|w|1|0|%2|1|0|claude||working|0|claude|two||||/h/other",
    ]);
    let mut app = App::new(snap, Tab::Agents, "$1".into(), "@1".into(), "/h".into(), false, false);
    let mut view = View::new();
    let full = "~/source/company/main-repo/worktrees/topic/src";
    for width in [30, 46, 90, 120] {
        let term = draw(&app, &mut view, width, 24);
        let lines = screen(&term);
        assert!(lines[2].trim_matches(['│', ' ']).is_empty(), "{width}: no path heading: {lines:#?}");
        assert!(!lines.iter().any(|line| line.contains(full)), "{width}: {lines:#?}");
    }
    ctrl(&mut app, 'n');
    let lines = screen(&draw(&app, &mut view, 46, 24));
    assert!(lines[2].trim_matches(['│', ' ']).is_empty(), "selection keeps the blank separator: {lines:#?}");
    app.run(Command::Tab(Tab::Sessions));
    let lines = screen(&draw(&app, &mut view, 46, 24));
    assert!(lines[2].trim_matches(['│', ' ']).is_empty(), "Sessions keeps its blank separator: {lines:#?}");
}

#[test]
fn selecting_a_long_path_does_not_move_mouse_targets_between_press_and_release() {
    for width in [46, 90] {
        let long = format!("/h/{}topic", "long-intermediate-folder/".repeat(8));
        let snap = fixture(&[
            "$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|first-task||||/h/short",
            &format!("$1|s|@1|1|w|1|0|%2|1|0|claude||working|0|claude|second-task||||{long}"),
        ]);
        let mut app = new(snap, Tab::Agents, "@1");
        let mut view = View::new();
        let term = draw(&app, &mut view, width, 24);
        let at = find(&term, "second-task");
        let now = Instant::now();
        mouse(&mut app, &view, MouseEventKind::Down(MouseButton::Left), at, now);
        let term = draw(&app, &mut view, width, 24);
        assert_eq!(find(&term, "second-task"), at, "{width}: selection must not shift the list");
        assert!(matches!(mouse(&mut app, &view, MouseEventKind::Up(MouseButton::Left), at, now), Action::None));
        assert!(app.last_click.is_some(), "the release still hits the pressed row");
        let later = now + Duration::from_millis(50);
        mouse(&mut app, &view, MouseEventKind::Down(MouseButton::Left), at, later);
        draw(&app, &mut view, width, 24);
        let action = mouse(&mut app, &view, MouseEventKind::Up(MouseButton::Left), at, later);
        assert!(matches!(action, Action::Activate(Target::Pane { pane, .. }) if pane == "%2"));
    }
}

#[test]
fn compact_rows_take_two_lines_with_less_detail() {
    let mut app = sessions_app("@1");
    let mut view = View::new();
    let lines = screen(&draw(&app, &mut view, 46, 20));
    assert!(view.compact);
    let at = lines.iter().position(|l| l.contains("edit")).unwrap();
    assert!(lines[at + 1].contains("/h/a"), "the folder under the window: {lines:#?}");
    assert!(!lines.iter().any(|l| l.contains("windows")), "no window counts");
    app.run(Command::Tab(Tab::Agents));
    let lines = screen(&draw(&app, &mut view, 46, 20));
    let at = lines.iter().position(|l| l.contains("deploy")).unwrap();
    assert!(lines[at + 1].contains("/h/c"), "the folder under the title: {lines:#?}");
    assert!(!lines[at].contains("play"), "no tmux location beside the title: {lines:#?}");
    assert!(!lines[at].contains("codex"), "icon instead of provider name");
}

#[test]
fn renaming_an_agent() {
    let mut app = agents_app();
    ctrl(&mut app, 'r');
    assert_eq!(field(&app), Some("one"), "starts from the current title");
    // An untouched automatic title is not set as the user's.
    assert!(matches!(app.on_key(key(KeyCode::Enter, KeyModifiers::NONE)), Action::None));
    assert!(field(&app).is_none());
    ctrl(&mut app, 'r');
    type_(&mut app, " more");
    let save = app.on_key(key(KeyCode::Enter, KeyModifiers::NONE));
    let Action::Run(job) = save else { panic!("{save:?}") };
    assert_eq!(job.op, Op::SetTitle { pane: "%1".into(), title: Some("one more".into()) });
    assert_eq!(job.undo, Some(Op::SetTitle { pane: "%1".into(), title: None }), "undo goes back to automatic");
    assert_eq!(app.query.text(), "", "typing went to the title, not the filter");
    app.ran(job, Ok(None));
    assert_eq!(app.status, Some(Status::Done("renamed   ^z undo".into())));
    ctrl(&mut app, 'r');
    assert!(matches!(app.on_key(key(KeyCode::Esc, KeyModifiers::NONE)), Action::None));
    assert!(field(&app).is_none(), "esc cancels the rename, not the popup");
}

#[test]
fn clearing_a_set_title_goes_back_to_automatic_and_undo_restores_it() {
    let snap = fixture(&["$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|auto|mine|||/a"]);
    let mut app = new(snap, Tab::Agents, "@1");
    ctrl(&mut app, 'r');
    assert_eq!(renaming(&app).map(|r| r.placeholder.as_str()), Some("auto"));
    ctrl(&mut app, 'u');
    let Action::Run(job) = app.on_key(key(KeyCode::Enter, KeyModifiers::NONE)) else { panic!() };
    assert_eq!(job.op, Op::SetTitle { pane: "%1".into(), title: None });
    app.ran(job, Ok(None));
    let Action::Run(undo) = ctrl(&mut app, 'z') else { panic!() };
    assert_eq!(undo.op, Op::SetTitle { pane: "%1".into(), title: Some("mine".into()) });
    app.ran(undo, Ok(None));
    assert_eq!(app.status, Some(Status::Done("undone: back to the automatic title".into())));
    assert!(matches!(ctrl(&mut app, 'z'), Action::None));
    assert_eq!(app.status, Some(Status::Note("nothing to undo".into())));
    // A failed change says why, in the field, and can't be undone.
    ctrl(&mut app, 'r');
    type_(&mut app, "!");
    let Action::Run(job) = app.on_key(key(KeyCode::Enter, KeyModifiers::NONE)) else { panic!() };
    app.ran(job, Err("can't find pane: %1".into()));
    assert_eq!(renaming(&app).and_then(|r| r.error.as_deref()), Some("can't find pane: %1"));
    assert!(app.undo.is_empty());
}

#[test]
fn an_unchanged_title_is_not_saved_over_a_newer_one() {
    let snap = fixture(&["$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|auto|mine|||/a"]);
    let mut app = new(snap, Tab::Agents, "@1");
    ctrl(&mut app, 'r');
    assert!(matches!(app.on_key(key(KeyCode::Enter, KeyModifiers::NONE)), Action::None));
}

#[test]
fn a_long_or_padded_name_left_as_it_was_is_not_saved() {
    let long = "a".repeat(TITLE_MAX + 20);
    let snap = fixture(&[&format!("$1| sp |@1|1|{long}|1|0|%1|0|1|claude||working|0|claude||{long}|||/a")]);
    let mut app = new(snap, Tab::Sessions, "@1");
    for target in [Target::Session { session: "$1".into() }, window("$1", "@1"), pane("$1", "@1", "%1")] {
        if matches!(target, Target::Pane { .. }) {
            app.set_tab(Tab::Agents);
        }
        select(&mut app, target.clone());
        ctrl(&mut app, 'r');
        assert!(renaming(&app).is_some(), "{target:?}");
        assert!(matches!(enter(&mut app), Action::None), "{target:?}: cut to {TITLE_MAX} or trimmed, not changed");
    }
}

#[test]
fn a_renamed_card_stays_listed_under_the_filter() {
    let mut app = agents_app();
    type_(&mut app, "three");
    assert_eq!(app.rows.len(), 2, "header and card");
    ctrl(&mut app, 'r');
    ctrl(&mut app, 'u');
    type_(&mut app, "renamed");
    let Action::Run(job) = app.on_key(key(KeyCode::Enter, KeyModifiers::NONE)) else { panic!() };
    app.ran(job, Ok(None));
    app.set_snapshot(fixture(&[
        "$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|one||||/a",
        "$1|s|@2|2|w|0|0|%3|0|1|claude||working|0|claude|three|renamed|||/b",
    ]));
    assert_eq!(selected_title(&app), "renamed", "it doesn't match \"three\" any more, but stays");
    type_(&mut app, "x");
    assert!(app.rows.is_empty(), "until the query changes");
}

#[test]
fn ctrl_r_again_fills_the_field_with_a_suggestion() {
    let mut app = agents_app();
    ctrl(&mut app, 'r');
    assert!(matches!(ctrl(&mut app, 'r'), Action::Suggest { pane, id: 1 } if pane == "%1"));
    assert!(matches!(ctrl(&mut app, 'r'), Action::None), "one suggestion at a time");
    assert_eq!(app.suggesting(), Some(1));
    let lines = screen(&draw(&app, &mut View::new(), 120, 12));
    assert!(lines.iter().any(|l| l.contains("✎ one") && l.contains("✦ suggesting…")), "{lines:#?}");
    assert!(lines.iter().any(|l| l.contains("⏎ save   ^r suggest   ^u clear") && l.ends_with("esc cancel │")), "{lines:#?}");
    app.suggested(1, Ok("Fix the cart page".into()));
    assert_eq!(field(&app), Some("Fix the cart page"));
    assert_eq!(app.suggesting(), None);
    // It is the field's text, to keep, change or drop.
    type_(&mut app, "s");
    let Action::Run(job) = app.on_key(key(KeyCode::Enter, KeyModifiers::NONE)) else { panic!() };
    assert_eq!(job.op, Op::SetTitle { pane: "%1".into(), title: Some("Fix the cart pages".into()) });
}

#[test]
fn a_suggestion_does_not_overwrite_what_came_after_it() {
    let mut app = agents_app();
    // Typing while it generates keeps the typing, and stops waiting.
    ctrl(&mut app, 'r');
    ctrl(&mut app, 'r');
    type_(&mut app, "!");
    assert_eq!(app.suggesting(), None, "the loop stops the suggest command");
    app.suggested(1, Ok("late".into()));
    assert_eq!(field(&app), Some("one!"));
    // Keys that change nothing keep waiting.
    assert!(matches!(ctrl(&mut app, 'r'), Action::Suggest { id: 2, .. }));
    app.on_key(key(KeyCode::Delete, KeyModifiers::NONE));
    assert_eq!(app.suggesting(), Some(2), "Delete at the end changes nothing");
    // An answer for a rename that was cancelled, reaching a new one.
    app.on_key(key(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.suggesting(), None);
    ctrl(&mut app, 'r');
    app.suggested(2, Ok("stale".into()));
    assert_eq!(field(&app), Some("one"));
    // A failure says why, by the field, and offers a retry.
    assert!(matches!(ctrl(&mut app, 'r'), Action::Suggest { id: 3, .. }));
    app.suggested(3, Err("pi timed out after 30s".into()));
    assert_eq!(field(&app), Some("one"));
    assert_eq!(renaming(&app).and_then(|r| r.error.as_deref()), Some("pi timed out after 30s"));
    let lines = screen(&draw(&app, &mut View::new(), 120, 12));
    assert!(lines.iter().any(|l| l.contains("✗ pi timed out after 30s")), "{lines:#?}");
    assert!(lines.iter().any(|l| l.contains("^r retry")), "{lines:#?}");
}

#[test]
fn no_suggestions_without_a_suggest_command() {
    let mut app = agents_app();
    app.can_suggest = false;
    ctrl(&mut app, 'r');
    let lines = screen(&draw(&app, &mut View::new(), 120, 12));
    assert!(!lines.iter().any(|l| l.contains("suggest")), "not offered: {lines:#?}");
    assert!(matches!(ctrl(&mut app, 'r'), Action::None));
    assert!(matches!(&app.status, Some(Status::Note(n)) if n.contains("@agent-suggest-command")), "but says why when asked");
}

#[test]
fn renaming_ends_when_the_agent_exits() {
    let mut app = agents_app();
    ctrl(&mut app, 'r');
    app.set_snapshot(fixture(&["$1|s|@1|1|w|1|0|%2|1|0|claude||working|0|claude|two||||/a"]));
    assert!(field(&app).is_none());
    assert_eq!(app.status, Some(Status::Failed("that agent has exited".into())));
}

#[test]
fn agent_topic_editor_keeps_identity_and_mouse_targets_in_place() {
    for width in [40, 80, 120] {
        let mut app = agents_app();
        for row in &mut app.rows {
            if let RowKind::Agent { age, stale, .. } = &mut row.kind {
                *age = "5m".into();
                *stale = true;
            }
        }
        let mut view = View::new();
        let before = draw(&app, &mut view, width, 24);
        let (_, y) = find(&before, "one");
        let detail = screen(&before)[y as usize + 1].clone();
        let targets = view.rows.clone();
        ctrl(&mut app, 'r');
        ctrl(&mut app, 'r');
        for failed in [false, true] {
            if failed { app.suggested(1, Err("pi timed out after 30s".into())); }
            let term = draw(&app, &mut view, width, 24);
            let lines = screen(&term);
            assert_eq!(lines[y as usize + 1], detail, "{width}: age, stale state and folder stay on line two");
            assert!(lines[y as usize + 1].contains("5m working?"), "{lines:#?}");
            assert!(lines[y as usize].contains("✎ "), "editor on the title line: {lines:#?}");
            assert_eq!(field(&app), Some("one"), "narrow editors scroll without changing the title");
            if width >= 80 { assert!(lines[y as usize].contains("✎ one")); }
            assert!(lines[y as usize].contains(if failed { "✗ pi" } else { "✦ sugg" }), "feedback on the title line: {lines:#?}");
            if width == 40 && failed { assert!(lines[y as usize].contains('…'), "feedback truncation stays visible: {lines:#?}"); }
            assert_eq!(view.rows, targets, "{width}: editing never moves mouse targets");
        }
    }
}

#[test]
fn the_title_is_edited_on_its_card() {
    let mut app = agents_app();
    type_(&mut app, "on");
    ctrl(&mut app, 'r');
    let lines = screen(&draw(&app, &mut View::new(), 120, 14));
    assert!(lines.iter().any(|l| l.contains("› on") && l.contains("renaming")), "the filter stays in view: {lines:#?}");
    assert!(lines.iter().any(|l| l.contains("▌ ● ✳  ✎ one")), "{lines:#?}");
    ctrl(&mut app, 'u');
    let lines = screen(&draw(&app, &mut View::new(), 120, 14));
    assert!(lines.iter().any(|l| l.contains("✎ ▏one") && l.contains("automatic")), "the automatic title as a placeholder: {lines:#?}");
}

#[test]
fn the_mouse_renames_too() {
    let mut app = agents_app();
    let mut view = View::new();
    let term = draw(&app, &mut view, 120, 30);
    click(&mut app, &view, MouseButton::Right, find(&term, "three"));
    let Mode::Menu(_) = &app.mode else { panic!("right-click opens the card's menu") };
    let term = draw(&app, &mut view, 120, 30);
    let action = click(&mut app, &view, MouseButton::Left, find(&term, "suggest a title"));
    assert!(matches!(&action, Action::Suggest { pane, .. } if pane == "%3"), "{action:?}");
    assert!(matches!(renaming(&app).map(|r| &r.target), Some(Target::Pane { pane, .. }) if pane == "%3"));
    // Clicking another card while renaming says how to finish.
    let term = draw(&app, &mut view, 120, 30);
    click(&mut app, &view, MouseButton::Left, find(&term, "one"));
    assert!(renaming(&app).is_some());
    assert_eq!(app.status, Some(Status::Note("⏎ save or esc cancel first".into())));
    let term = draw(&app, &mut view, 120, 30);
    click(&mut app, &view, MouseButton::Left, find(&term, "cancel"));
    assert!(renaming(&app).is_none());
}

#[test]
fn a_menu_is_picked_by_key_by_click_or_by_dragging_onto_an_item() {
    let mut app = agents_app();
    let mut view = View::new();
    let term = draw(&app, &mut view, 120, 30);
    let at = find(&term, "two");
    // Right press, then its release where it was pressed: nothing.
    mouse(&mut app, &view, MouseEventKind::Down(MouseButton::Right), at, Instant::now());
    let term = draw(&app, &mut view, 120, 30);
    assert!(matches!(mouse(&mut app, &view, MouseEventKind::Up(MouseButton::Right), at, Instant::now()), Action::None), "its own release picks nothing");
    assert!(matches!(app.mode, Mode::Menu(_)));
    // Right press, drag, release on "rename".
    let item = find(&term, "rename");
    app.mode = Mode::List;
    draw(&app, &mut view, 120, 30);
    mouse(&mut app, &view, MouseEventKind::Down(MouseButton::Right), at, Instant::now());
    draw(&app, &mut view, 120, 30);
    mouse(&mut app, &view, MouseEventKind::Up(MouseButton::Right), item, Instant::now());
    assert!(matches!(renaming(&app).map(|r| &r.target), Some(Target::Pane { pane, .. }) if pane == "%2"));
    app.run(Command::Cancel);
    // Keys: ↓ ↓ then ⏎; any other key closes it and acts.
    app.open_menu(Position::default(), true);
    app.on_key(key(KeyCode::Down, KeyModifiers::NONE));
    app.on_key(key(KeyCode::Down, KeyModifiers::NONE));
    app.on_key(key(KeyCode::Enter, KeyModifiers::NONE));
    assert!(renaming(&app).is_some(), "the third item (after open and type), rename");
    app.run(Command::Cancel);
    app.open_menu(Position::default(), true);
    type_(&mut app, "t");
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.query.text(), "t");
    // A click outside closes it.
    app.open_menu(Position::default(), true);
    draw(&app, &mut view, 120, 30);
    mouse(&mut app, &view, MouseEventKind::Down(MouseButton::Left), (110, 25), Instant::now());
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn a_menu_closes_when_its_row_goes() {
    let mut app = sessions_app("@1");
    select(&mut app, window("$1", "@2"));
    app.open_menu(Position::default(), true);
    let without_logs = || fixture(&["$1|work|@1|1|edit|1|0|%1|0|1|nvim|||||||||/h/a", "$2|play|@3|1|game|1|0|%4|0|1|codex||asking|0|codex|deploy||||/h/c"]);
    app.set_snapshot(without_logs());
    assert!(matches!(app.mode, Mode::List), "its move down would swap another window");
    assert!(matches!(&app.status, Some(Status::Failed(_))));
    // A refresh that keeps the row keeps the menu.
    app.open_menu(Position::default(), true);
    app.set_snapshot(without_logs());
    assert!(matches!(app.mode, Mode::Menu(_)));
}

#[test]
fn nerd_marks_follow_redraws_rename_filter_menu_and_resize() {
    let mut app = agents_app();
    let mut view = View::new();
    let mut theme = Theme::default();
    theme.agent_icons = crate::theme::AgentIcons::Nerd;
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let render = |term: &mut Terminal<TestBackend>, app: &App, view: &mut View| {
        term.draw(|f| ui::draw(f, app, view, &Preview { title: "", body: &Text::default() }, &theme)).unwrap();
    };
    let icon = "\u{ec82}";
    let count = |term: &Terminal<TestBackend>| term.backend().buffer().content.iter().filter(|c| c.symbol() == icon).count();
    render(&mut term, &app, &mut view);
    assert_eq!(count(&term), 4, "one glyph per card");
    let (x, y) = find(&term, icon);
    assert_eq!(term.backend().buffer()[(x, y)].fg, ratatui::style::Color::Reset);
    assert!(term.backend().buffer()[(x, y)].modifier.contains(Modifier::BOLD), "glyph is part of the bright identity");
    assert!(!term.backend().buffer()[(x, y)].modifier.contains(Modifier::DIM));
    assert!(screen(&term).iter().all(|l| !l.contains("claude")), "no duplicate provider name");

    app.open_menu(Position::new(x, y), true);
    render(&mut term, &app, &mut view);
    let menu = view.menu.unwrap();
    for row in menu.y..menu.bottom() {
        for col in menu.x..menu.right() {
            assert_ne!(term.backend().buffer()[(col, row)].symbol(), icon, "menu clears glyph cells");
        }
    }
    app.on_key(key(KeyCode::Esc, KeyModifiers::NONE));
    ctrl(&mut app, 'r');
    render(&mut term, &app, &mut view);
    assert_eq!(count(&term), 4, "renaming keeps the icon");
    app.on_key(key(KeyCode::Esc, KeyModifiers::NONE));
    type_(&mut app, "nothing-matches");
    render(&mut term, &app, &mut view);
    assert_eq!(count(&term), 0, "filtering removes old glyphs");
    ctrl(&mut app, 'u');
    for width in [24, 40, 120] {
        term.backend_mut().resize(width, 12);
        term.resize(ratatui::layout::Rect::new(0, 0, width, 12)).unwrap();
        render(&mut term, &app, &mut view);
        assert!(count(&term) > 0);
        for y in 0..12 {
            assert_ne!(term.backend().buffer()[(width - 1, y)].symbol(), icon, "no glyph outside the list");
        }
        app.move_by(1, false);
        render(&mut term, &app, &mut view);
    }
    app.set_tab(Tab::Sessions);
    render(&mut term, &app, &mut view);
    assert_eq!(count(&term), 0, "changing tabs clears the glyphs");
}

fn hidden_snapshot() -> Snapshot {
    crate::backend::tmux::fixture_hidden(&[
        "$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|visible||||/a",
        "$1|s|@1|1|w|1|0|%2|1|0|codex||asking|0|codex|parked question||||/b",
        "$1|s|@2|2|w|0|0|%3|0|1|pi||idle|0|pi|parked rest||||/a",
    ], &["%2", "%3"])
}

fn hidden_app(expanded: bool) -> App {
    App::new(hidden_snapshot(), Tab::Agents, "$1".into(), "@1".into(), String::new(), true, expanded)
}

/// Simulate a successful tmux write and the following fresh snapshot.
fn complete_hidden(app: &mut App, action: Action) {
    let job = job(action);
    if let Op::SetHidden { pane, hidden } = &job.op {
        for s in &mut app.snap.sessions {
            for w in &mut s.windows {
                for p in &mut w.panes {
                    if p.id == *pane && let Some(a) = &mut p.agent {
                        a.hidden = *hidden;
                    }
                }
            }
        }
    }
    app.ran(job, Ok(None));
    app.refresh();
}

#[test]
fn hide_show_is_non_destructive_and_only_changes_after_success() {
    let mut app = hidden_app(false);
    assert!(app.menu_items(true).iter().any(|h| h.command == Command::Hide));
    assert!(!app.menu_items(true).iter().any(|h| h.command == Command::Show));
    let pending = job(ctrl(&mut app, 'e'));
    assert_eq!(pending.op, Op::SetHidden { pane: "%1".into(), hidden: true });
    assert!(!app.snap.agent("%1").unwrap().hidden, "no optimistic visibility change");
    app.ran(pending, Err("write failed".into()));
    assert_eq!(selected_title(&app), "visible");
    assert!(matches!(app.status, Some(Status::Failed(_))));
    let action = ctrl(&mut app, 'e');
    complete_hidden(&mut app, action);
    assert_eq!(app.rows.len(), 1, "all agents can be hidden");
    assert!(app.hidden_selected());
    assert_eq!(app.snap.unique_panes().count(), 3, "no pane closed");
    assert_eq!(app.snap.agent("%1").unwrap().state, crate::model::State::Working);
    assert!(matches!(app.run(Command::Kill), Action::None));
    assert!(matches!(app.mode, Mode::List), "heading is never a kill target");
    type_(&mut app, "visible");
    assert_eq!(selected_title(&app), "visible");
    assert!(app.menu_items(true).iter().any(|h| h.command == Command::Show));
    assert!(matches!(enter(&mut app), Action::Activate(Target::Pane { pane, .. }) if pane == "%1"));
    assert!(app.snap.agent("%1").unwrap().hidden, "opening does not show");
    let action = ctrl(&mut app, 'e');
    assert!(matches!(&action, Action::Run(j) if j.op == Op::SetHidden { pane: "%1".into(), hidden: false }));
    complete_hidden(&mut app, action);
    assert_eq!(selected_title(&app), "visible");
    assert!(!app.snap.agent("%1").unwrap().hidden);
    assert!(app.snap.agent("%2").unwrap().hidden);
    ctrl(&mut app, 'u');
    app.set_tab(Tab::Sessions);
    assert!(!app.can(Command::Hide, true));
    assert!(app.rows.iter().any(|r| matches!(&r.target, Some(Target::Window { window, .. }) if window == "@2")));
}

#[test]
fn hidden_expansion_is_selectable_and_committed_only_after_success() {
    let mut app = hidden_app(false);
    app.move_by(1, false);
    assert!(app.hidden_selected(), "navigation reaches the collapsed heading");
    assert_eq!(app.preview_pane(&View::new()), None);
    let pending = job(enter(&mut app));
    assert_eq!(pending.op, Op::SetHiddenExpanded(true));
    assert!(!app.hidden_expanded);
    app.ran(pending, Err("write failed".into()));
    assert!(!app.hidden_expanded);
    let action = app.on_key(key(KeyCode::Right, KeyModifiers::NONE));
    complete_hidden(&mut app, action);
    assert!(app.hidden_expanded);
    assert!(app.hidden_selected(), "heading remains selected when expanded");
    app.move_by(1, false);
    assert_eq!(selected_title(&app), "parked question");
    assert_eq!(app.preview_pane(&View::new()), Some("%2"));
    ctrl(&mut app, 'r');
    assert!(renaming(&app).is_some(), "hidden cards still support renaming");
    app.run(Command::Cancel);
    let action = app.on_key(key(KeyCode::Left, KeyModifiers::NONE));
    complete_hidden(&mut app, action);
    assert!(!app.hidden_expanded);
    assert!(app.hidden_selected(), "collapsing from a hidden card selects the heading");
    let mut snapshot = hidden_snapshot();
    snapshot.sessions[0].windows[0].panes[1].agent.as_mut().unwrap().state = crate::model::State::Working;
    app.set_snapshot(snapshot);
    assert!(app.hidden_selected());
    assert!(app.snap.agent("%2").unwrap().hidden, "state changes never unhide");
    assert!(!app.hidden_expanded);
    app.open_menu(Position::default(), true);
    app.set_snapshot(fixture(&[]));
    assert!(matches!(app.mode, Mode::List), "a vanished heading closes its menu");
}

#[test]
fn searching_hidden_cards_never_changes_the_saved_expansion() {
    for expanded in [false, true] {
        let mut app = hidden_app(expanded);
        type_(&mut app, "parked");
        assert!(matches!(app.rows[0].kind, RowKind::Hidden { expanded: true, count: 2, .. }));
        assert_eq!(app.rows.len(), 3);
        assert_eq!(selected_title(&app), "parked question");
        assert!(matches!(app.on_key(key(KeyCode::Left, KeyModifiers::NONE)), Action::None));
        assert_eq!(app.hidden_expanded, expanded);
        assert_eq!(app.rows.len(), 3);
        app.run(Command::Group);
        assert_eq!(app.hidden_expanded, expanded);
        assert!(app.rows[1..].iter().all(|r| matches!(r.kind, RowKind::Agent { .. })));
        ctrl(&mut app, 'u');
        let heading = app.hidden_heading().unwrap();
        assert!(matches!(app.rows[heading].kind, RowKind::Hidden { expanded: open, count: 2, .. } if open == expanded));
        if !expanded { assert!(app.hidden_selected()); }
        app.set_tab(Tab::Sessions);
        app.set_tab(Tab::Agents);
        assert_eq!(app.hidden_expanded, expanded);
    }
}

#[test]
fn hidden_heading_clicks_toggle_only_the_heading_not_its_spacer() {
    for width in [24, 40, 120] {
        let mut app = hidden_app(false);
        let mut view = View::new();
        let term = draw(&app, &mut view, width, 14);
        let rect = view.hidden.unwrap();
        assert!(screen(&term)[rect.y as usize].contains("hidden · 2"));
        if !view.compact {
            assert!(view.hit(Position::new(rect.x, rect.y - 1)).is_none(), "blank line above the heading is not clickable");
        }
        let action = click(&mut app, &view, MouseButton::Left, (rect.x + 3, rect.y));
        complete_hidden(&mut app, action);
        let term = draw(&app, &mut view, width, 14);
        assert!(app.hidden_expanded);
        assert!(screen(&term).iter().any(|l| l.contains("parked quest")), "{width}: {:#?}", screen(&term));
        let rect = view.hidden.unwrap();
        let action = click(&mut app, &view, MouseButton::Left, (rect.x + 3, rect.y));
        complete_hidden(&mut app, action);
        assert!(!app.hidden_expanded);
        let term = draw(&app, &mut view, width, 14);
        assert!(screen(&term).iter().all(|l| !l.contains("parked")));
    }
}

#[test]
fn agent_marks_survive_renaming_and_fall_back_to_the_command() {
    for (kind, command, icon) in [("claude", "node", "✳"), ("codex", "node", "›_"), ("", "pi", "π"), ("custom", "node", "▣")] {
        let mut app = new(fixture(&[&format!("$1|s|@1|1|w|1|0|%1|0|1|{command}||working|0|{kind}|task||||/a")]), Tab::Agents, "@1");
        for renaming in [false, true] {
            if renaming {
                ctrl(&mut app, 'r');
            }
            for width in [40, 80, 120] {
                let lines = screen(&draw(&app, &mut View::new(), width, 12));
                let title = lines.iter().position(|l| l.contains("task")).unwrap();
                assert!(lines[title].contains(icon), "{lines:#?}");
            }
        }
    }
}

#[test]
fn a_stale_state_is_marked_unconfirmed() {
    let app = new(fixture(&["$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|task|||1|/a"]), Tab::Agents, "@1");
    let lines = screen(&draw(&app, &mut View::new(), 120, 12));
    assert!(lines.iter().any(|l| l.contains("working?")), "{lines:#?}");
}

#[test]
fn pasted_newlines_do_not_open() {
    let mut app = agents_app();
    app.paste("tw\no\n");
    assert_eq!(app.query.text(), "two");
    assert_eq!(selected_title(&app), "two");
}

#[test]
fn the_title_is_plain_text_over_a_dim_border() {
    let app = agents_app();
    let term = draw(&app, &mut View::new(), 120, 20);
    let buf = term.backend().buffer();
    let (x, y) = find(&term, "agents");
    assert!(!buf[(x, y)].modifier.contains(Modifier::DIM), "\"agents\" tab");
    assert!(buf[(0, 0)].modifier.contains(Modifier::DIM), "the border itself");
}

#[test]
fn a_narrow_title_drops_the_calmest_counts_whole() {
    let rows: Vec<String> = (1..=12)
        .map(|i| {
            let (state, unseen) = [("asking", "0"), ("working", "0"), ("idle", "1"), ("idle", "0")][i % 4];
            format!("$1|s|@{i}|{i}|w|1|0|%{i}|0|1|claude||{state}|{unseen}|claude|t{i}||||/a")
        })
        .collect();
    let app = new(fixture(&rows.iter().map(|r| r.as_str()).collect::<Vec<_>>()), Tab::Agents, "@1");
    let top = |w| screen(&draw(&app, &mut View::new(), w, 12))[0].clone();
    let marks = ['●', '○'];
    assert_eq!(top(80).matches(marks).count(), 4, "{}", top(80));
    assert!(top(80).contains("● 3 waiting  ● 3 working  ● 3 finished  ○ 3 idle ╮"), "named when they fit: {}", top(80));
    for w in 30..44 {
        let line = top(w);
        assert!(line.contains("sessions  agents"), "{line}");
        // Each count follows its dot.
        let words: Vec<&str> = line.trim_matches(|c| "╭╮─ ".contains(c)).split_whitespace().collect();
        for (i, word) in words.iter().enumerate() {
            if word.chars().all(|c| c.is_ascii_digit()) {
                assert!(words.get(i.wrapping_sub(1)).is_some_and(|m| m.chars().all(|c| marks.contains(&c))), "width {w}: {line}");
            }
        }
    }
    assert!(top(38).contains("● 3") && !top(38).contains('○'), "the most urgent stay, idle goes: {}", top(38));
}

#[test]
fn the_selection_is_bold_in_the_accent_and_the_current_session_only_marked() {
    let app = sessions_app("@1");
    let term = draw(&app, &mut View::new(), 120, 12);
    let buf = term.backend().buffer();
    let t = Theme::default();
    let (x, y) = find(&term, "edit");
    assert_eq!((buf[(x, y)].fg, buf[(x, y)].modifier), (t.accent, Modifier::BOLD), "the selected window's name");
    let (x, y) = find(&term, "play");
    assert!(!buf[(x, y)].modifier.contains(Modifier::BOLD), "other names are plain");
    let (x, y) = find(&term, "work*");
    assert!(!buf[(x, y)].modifier.contains(Modifier::REVERSED), "no badge");
    assert_eq!(buf[(x + 4, y)].fg, t.accent, "the current session's *");
}

#[test]
fn the_filter_marks_what_it_matched() {
    let mut app = sessions_app("@1");
    type_(&mut app, "logs");
    let term = draw(&app, &mut View::new(), 120, 12);
    let buf = term.backend().buffer();
    let (x, y) = find(&term, "logs  /h");
    assert!((x..x + 4).all(|x| buf[(x, y)].modifier.contains(Modifier::UNDERLINED)), "{:?}", screen(&term));
    let (x, y) = find(&term, "work");
    assert!(!buf[(x, y)].modifier.contains(Modifier::UNDERLINED), "a session listed for context");
    assert!(!buf.content.iter().any(|c| c.symbol() == "/" && c.modifier.contains(Modifier::UNDERLINED)), "nothing else");
}

#[test]
fn esc_clears_the_filter_then_closes() {
    let mut app = sessions_app("@1");
    type_(&mut app, "logs");
    assert_eq!(app.hints(&View::new()).1.label, "clear");
    assert!(matches!(app.on_key(key(KeyCode::Esc, KeyModifiers::NONE)), Action::None));
    assert_eq!(app.query.text(), "");
    assert_eq!(selected_title(&app), "logs", "the selection stays");
    assert_eq!(app.hints(&View::new()).1.label, "close");
    assert!(matches!(app.on_key(key(KeyCode::Esc, KeyModifiers::NONE)), Action::Quit));
    type_(&mut app, "x");
    assert!(matches!(ctrl(&mut app, 'c'), Action::Quit), "^c closes whatever");
}

#[test]
fn narrow_and_tall_the_preview_goes_below_the_list() {
    let mut app = agents_app();
    let mut view = View::new();
    let draw_preview = |app: &App, view: &mut View, width, height| {
        let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
        let body = Text::raw("pane text");
        term.draw(|f| ui::draw(f, app, view, &Preview { title: "s:1.0 · claude", body: &body }, &Theme::default())).unwrap();
        screen(&term)
    };
    // A phone's compact layout leaves it out until asked for (ctrl-o).
    let lines = draw_preview(&app, &mut view, 70, 30);
    assert!(view.preview_fits && !lines.iter().any(|l| l.contains("pane text")), "{lines:#?}");
    app.compact = true;
    assert_eq!(app.menu_items(true).iter().find(|h| h.key == "^o").map(|h| h.label.as_str()), Some("show preview"));
    app.run(Command::Preview);
    let lines = draw_preview(&app, &mut view, 70, 30);
    assert!(view.preview_fits);
    let rule = lines.iter().position(|l| l.contains("s:1.0 · claude ─")).unwrap_or_else(|| panic!("{lines:#?}"));
    assert!(lines[rule + 1..].iter().any(|l| l.contains("pane text")), "{lines:#?}");
    assert!(lines[..rule].iter().any(|l| l.contains("one")), "the list above: {lines:#?}");
    let lines = draw_preview(&app, &mut view, 70, 20);
    assert!(!view.preview_fits);
    assert!(!lines.iter().any(|l| l.contains("pane text")), "{lines:#?}");
    let lines = draw_preview(&app, &mut view, 120, 20);
    assert!(view.preview_fits && lines.iter().any(|l| l.contains("│ s:1.0 · claude")), "beside: {lines:#?}");
}

#[test]
fn an_empty_agents_tab_says_how_agents_get_there() {
    let mut app = new(fixture(&["$1|s|@1|1|w|1|0|%1|0|1|zsh|||||||||/a"]), Tab::Agents, "@1");
    // Whole, wrapped where narrow, and with no preview beside it; only
    // the server it runs on.
    for (rex, says, not) in [(false, ["agent-status.tmux", "state)"], "Rex"), (true, ["Rex", "themselves)"], "tmux")] {
        app.caps.rex = rex;
        for (w, h) in [(40, 10), (80, 16), (50, 20), (120, 40)] {
            let lines = screen(&draw(&app, &mut View::new(), w, h));
            assert!(lines.iter().any(|l| l.contains("no agents running")));
            assert!(says.iter().all(|s| lines.iter().any(|l| l.contains(s))), "{lines:#?}");
            assert!(!lines.iter().any(|l| l.contains(not)), "{lines:#?}");
        }
    }
}

#[test]
fn tabs_round_trip_through_their_labels() {
    for tab in [Tab::Sessions, Tab::Agents] {
        assert_eq!(Tab::parse(tab.label()), Some(tab));
    }
    assert_eq!(Tab::parse("Agents"), None);
    assert_eq!(Tab::parse(""), None);
}

#[test]
fn status_shows_on_the_bottom_edge_and_keeps_the_footer() {
    let mut app = agents_app();
    app.status = Some(Status::Failed("tmux said no".into()));
    let mut view = View::new();
    let term = draw(&app, &mut view, 120, 20);
    let lines = screen(&term);
    assert!(lines.last().unwrap().contains("✗ tmux said no"), "{lines:#?}");
    assert!(matches!(click(&mut app, &view, MouseButton::Left, find(&term, "close")), Action::Quit), "a tap still works");
}

#[test]
fn a_servers_trouble_stays_on_the_bottom_edge_under_any_status() {
    let mut app = agents_app();
    app.trouble = Some("rex is not answering".into());
    let last = |app: &App| screen(&draw(app, &mut View::new(), 120, 20)).pop().unwrap();
    assert!(last(&app).contains("✗ rex is not answering"), "{}", last(&app));
    app.status = Some(Status::Done("renamed".into()));
    assert!(last(&app).contains("✓ renamed") && !last(&app).contains("not answering"), "the status takes its place: {}", last(&app));
    app.status = None;
    assert!(last(&app).contains("✗ rex is not answering"), "and gives it back: {}", last(&app));
}

#[test]
fn alt_and_command_letters_are_not_typed_and_ctrl_h_leaves_the_query() {
    let mut app = agents_app();
    app.on_key(key(KeyCode::Char('x'), KeyModifiers::ALT));
    app.on_key(key(KeyCode::Char('j'), KeyModifiers::SUPER));
    assert_eq!(app.query.text(), "");
    app.on_key(key(KeyCode::Char('T'), KeyModifiers::SHIFT));
    assert_eq!(app.query.text(), "T");
    app.on_key(key(KeyCode::Home, KeyModifiers::NONE));
    type_(&mut app, "x");
    assert_eq!(app.query.text(), "Tx", "the query is typed at its end");
    ctrl(&mut app, 'h');
    assert_eq!(app.query.text(), "Tx", "^h leaves typing; held, it would eat the query");
    app.on_key(key(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(app.query.text(), "T");
}

#[test]
fn kitty_control_keys_act_as_a_legacy_terminal_sends_them() {
    let mut app = needy_app(Tab::Agents);
    ctrl(&mut app, 'i');
    assert_eq!(app.tab, Tab::Sessions, "^i is Tab");
    ctrl(&mut app, 'a');
    assert!(matches!(app.mode, Mode::Menu(_)));
    ctrl(&mut app, '[');
    assert!(matches!(app.mode, Mode::List), "^[ is Esc");
    assert!(matches!(ctrl(&mut app, 'm'), Action::Activate(_)), "^m is Enter");
    // Typing, the pane gets them as they are.
    let mut app = needy_app(Tab::Agents);
    ctrl(&mut app, 'l');
    assert!(matches!(ctrl(&mut app, '['), Action::Send(_, Input::Key(k)) if k.code == KeyCode::Char('[')));
}

#[test]
fn left_collapses_and_right_expands() {
    let snap = fixture(&["$1|s|@1|1|w|1|0|%1|0|1|nvim|||||||||/a", "$1|s|@1|1|w|1|0|%2|1|0|zsh|||||||||/a"]);
    let mut app = new(snap, Tab::Sessions, "@1");
    assert_eq!(app.rows.len(), 2);
    app.on_key(key(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(app.rows.len(), 2, "left does not expand");
    app.on_key(key(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.rows.len(), 4);
    app.on_key(key(KeyCode::Down, KeyModifiers::NONE));
    assert!(matches!(app.rows[app.selected].kind, RowKind::Pane { .. }));
    app.on_key(key(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(app.rows.len(), 2);
    assert!(matches!(app.rows[app.selected].kind, RowKind::Window { .. }), "back on the window");
}

#[test]
fn a_window_the_filter_opened_can_be_closed() {
    let mut app = sessions_app("@1");
    type_(&mut app, "zsh");
    let panes = |app: &App| app.rows.iter().filter(|r| matches!(r.kind, RowKind::Pane { .. })).count();
    assert_eq!(panes(&app), 1, "the matching pane explains the window");
    app.selected = app.rows.iter().position(|r| matches!(r.kind, RowKind::Window { name: ref n, .. } if n == "edit")).unwrap();
    app.on_key(key(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(panes(&app), 0, "closed");
    app.on_key(key(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(panes(&app), 2, "open: every pane");
}

fn select(app: &mut App, target: Target) {
    app.selected = app.position(&target).unwrap_or_else(|| panic!("{target:?} not listed"));
}

fn window(session: &str, window: &str) -> Target {
    Target::Window { session: session.into(), window: window.into() }
}

fn pane(session: &str, window: &str, pane: &str) -> Target {
    Target::Pane { session: session.into(), window: window.into(), pane: pane.into() }
}

fn job(action: Action) -> Job {
    match action {
        Action::Run(job) => job,
        other => panic!("not a job: {other:?}"),
    }
}

fn enter(app: &mut App) -> Action {
    app.on_key(key(KeyCode::Enter, KeyModifiers::NONE))
}

#[test]
fn sessions_and_windows_are_renamed_in_place() {
    let mut app = sessions_app("@1");
    select(&mut app, Target::Session { session: "$1".into() });
    ctrl(&mut app, 'r');
    assert_eq!(field(&app), Some("work"));
    // tmux refuses these; the field stays open and says why.
    ctrl(&mut app, 'u');
    type_(&mut app, "play");
    assert!(matches!(enter(&mut app), Action::None));
    assert_eq!(renaming(&app).and_then(|r| r.error.as_deref()), Some("a session called play exists"));
    ctrl(&mut app, 'u');
    type_(&mut app, "a.b");
    enter(&mut app);
    assert_eq!(renaming(&app).and_then(|r| r.error.as_deref()), Some("session names can't have . or :"));
    ctrl(&mut app, 'u');
    type_(&mut app, "office");
    assert_eq!(renaming(&app).and_then(|r| r.error.as_deref()), None, "typing clears the reason");
    let j = job(enter(&mut app));
    assert_eq!(j.op, Op::RenameSession { session: "$1".into(), name: "office".into() });
    assert_eq!(j.undo, Some(Op::RenameSession { session: "$1".into(), name: "work".into() }));
    // A window's empty name hands naming back to tmux.
    select(&mut app, window("$1", "@2"));
    ctrl(&mut app, 'r');
    ctrl(&mut app, 'u');
    let j = job(enter(&mut app));
    assert_eq!(j.op, Op::RenameWindow { window: "@2".into(), name: String::new() });
    assert_eq!(j.done, "named after what runs");
}

#[test]
fn renaming_a_window_tmux_names_is_undone_back_to_automatic() {
    let mut app = new(fixture(&["$1|s|@1|1|zsh|1|1|%1|0|1|zsh|||||||||/a"]), Tab::Sessions, "@1");
    select(&mut app, window("$1", "@1"));
    ctrl(&mut app, 'r');
    type_(&mut app, "x");
    let j = job(enter(&mut app));
    assert_eq!(j.op, Op::RenameWindow { window: "@1".into(), name: "zshx".into() });
    assert_eq!(j.undo, Some(Op::RenameWindow { window: "@1".into(), name: String::new() }), "not frozen at zsh");
    // Its name is already automatic: clearing it changes nothing.
    ctrl(&mut app, 'r');
    ctrl(&mut app, 'u');
    assert!(matches!(enter(&mut app), Action::None));
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn the_current_session_follows_the_client() {
    let mut app = sessions_app("@1");
    let current = |app: &App| -> Vec<String> {
        app.rows.iter().filter_map(|r| if let RowKind::Session { name, current: true, .. } = &r.kind { Some(name.clone()) } else { None }).collect()
    };
    assert_eq!(current(&app), ["work"]);
    app.moved("$2");
    app.refresh();
    assert_eq!(current(&app), ["play"]);
    assert_eq!(app.current_window, "@3", "at its active window");
}

#[test]
fn kills_ask_first_and_say_what_runs_there() {
    let mut app = sessions_app("@1");
    select(&mut app, window("$2", "@3"));
    ctrl(&mut app, 'x');
    let Mode::Confirm(c) = &app.mode else { panic!("{:?}", app.mode) };
    assert_eq!(c.question, "kill window play:1 game? 1 pane · codex waiting · play closes");
    let lines = screen(&draw(&app, &mut View::new(), 120, 12));
    assert!(lines.iter().any(|l| l.contains("✗ kill window play:1 game?") && l.contains("can't be undone")), "{lines:#?}");
    assert!(lines.iter().any(|l| l.contains("⏎ kill") && l.ends_with("esc cancel │")), "{lines:#?}");
    // Anything but yes is no.
    type_(&mut app, "y");
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.query.text(), "", "the no doesn't type");
    app.on_key(key(KeyCode::Delete, KeyModifiers::NONE));
    assert!(matches!(app.on_key(key(KeyCode::Delete, KeyModifiers::NONE)), Action::None), "a held Delete doesn't answer");
    app.on_key(key(KeyCode::Delete, KeyModifiers::NONE));
    assert!(matches!(ctrl(&mut app, 'x'), Action::None) && matches!(app.mode, Mode::List), "nor does ^x: only ⏎ kills");
    ctrl(&mut app, 'x');
    assert_eq!(job(enter(&mut app)).op, Op::Kill(window("$2", "@3")));
    let mut app = new(fixture(&["$1|s|@1|1|w|1|0|%1|0|1|zsh|||||||||/a"]), Tab::Sessions, "@1");
    select(&mut app, Target::Session { session: "$1".into() });
    ctrl(&mut app, 'x');
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.status, Some(Status::Note("that is the only session: kill the server instead".into())));
    // Its only window, or (an agent's card) that window's only pane, would
    // close it too.
    let only = || Status::Note("that is the only session: kill the server instead".into());
    app.status = None;
    select(&mut app, window("$1", "@1"));
    ctrl(&mut app, 'x');
    assert!(matches!(app.mode, Mode::List) && app.status == Some(only()));
    let mut app = new(fixture(&["$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|one||||/a"]), Tab::Agents, "@1");
    ctrl(&mut app, 'x');
    assert!(matches!(app.mode, Mode::List) && app.status == Some(only()));
    // With another session, the question says the session closes.
    let mut app = new(fixture(&["$1|s|@1|1|w|1|0|%1|0|1|zsh|||||||||/a", "$2|t|@2|1|w|1|0|%2|0|1|zsh|||||||||/a"]), Tab::Sessions, "@1");
    select(&mut app, window("$1", "@1"));
    ctrl(&mut app, 'x');
    assert!(matches!(&app.mode, Mode::Confirm(c) if c.question.ends_with("1 pane · s closes")), "{:?}", app.mode);
}

#[test]
fn a_swap_with_a_row_the_filter_hides_names_it() {
    let snap = fixture(&["$1|s|@1|1|edit|1|0|%1|0|1|zsh|||||||||/a", "$1|s|@2|2|logs|0|0|%2|0|1|zsh|||||||||/a", "$1|s|@3|3|mail|0|0|%3|0|1|zsh|||||||||/a"]);
    let mut app = new(snap, Tab::Sessions, "@1");
    type_(&mut app, "i");
    select(&mut app, window("$1", "@1"));
    let j = job(app.on_key(key(KeyCode::Down, KeyModifiers::SHIFT)));
    assert_eq!(j.op, Op::SwapWindows { session: "$1".into(), a: "@1".into(), b: "@2".into() });
    assert_eq!(j.done, "moved below logs");
}

#[test]
fn shift_arrows_swap_neighbours_and_cross_at_the_ends() {
    let shift = |app: &mut App, code| app.on_key(key(code, KeyModifiers::SHIFT));
    let mut app = sessions_app("@1");
    select(&mut app, window("$1", "@1"));
    let j = job(shift(&mut app, KeyCode::Down));
    let swap = Op::SwapWindows { session: "$1".into(), a: "@1".into(), b: "@2".into() };
    assert_eq!((j.op, j.undo), (swap.clone(), Some(swap)));
    assert!(matches!(shift(&mut app, KeyCode::Up), Action::None), "the first window of the first session");
    assert_eq!(app.status, Some(Status::Note("already the first window".into())));
    // The last window of work goes first into play.
    select(&mut app, window("$1", "@2"));
    let j = job(shift(&mut app, KeyCode::Down));
    assert_eq!(j.op, Op::MoveWindow { from: "$1".into(), window: "@2".into(), into: "$2".into(), to: "@3".into(), before: true });
    assert_eq!(j.undo, Some(Op::MoveWindow { from: "$2".into(), window: "@2".into(), into: "$1".into(), to: "@1".into(), before: false }));
    // play's only window leaving closes play: ask.
    select(&mut app, window("$2", "@3"));
    assert!(matches!(shift(&mut app, KeyCode::Up), Action::None));
    let Mode::Confirm(c) = &app.mode else { panic!() };
    assert_eq!(c.question, "move window game to work? play closes: it is its only window");
    assert!(matches!(ctrl(&mut app, 'x'), Action::None) && matches!(app.mode, Mode::List), "^x answers nothing");
    shift(&mut app, KeyCode::Up);
    assert_eq!(job(enter(&mut app)).op, Op::MoveWindow { from: "$2".into(), window: "@3".into(), into: "$1".into(), to: "@2".into(), before: false });
    // Panes swap inside their window, then join the next one.
    app.expanded.insert(window("$1", "@1"));
    app.refresh();
    select(&mut app, pane("$1", "@1", "%1"));
    assert_eq!(job(shift(&mut app, KeyCode::Down)).op, Op::SwapPanes("%1".into(), "%2".into()));
    select(&mut app, pane("$1", "@1", "%2"));
    let j = job(shift(&mut app, KeyCode::Down));
    assert_eq!(j.op, Op::JoinPane { pane: "%2".into(), to: "%3".into(), before: true });
    assert_eq!(j.undo, Some(Op::JoinPane { pane: "%2".into(), to: "%1".into(), before: false }));
    // Not on the agents tab, where cards are sorted by state.
    app.run(Command::Tab(Tab::Agents));
    assert!(matches!(shift(&mut app, KeyCode::Down), Action::None));
}

#[test]
fn ctrl_t_carries_to_where_enter_drops() {
    let mut app = sessions_app("@1");
    select(&mut app, window("$1", "@2"));
    ctrl(&mut app, 't');
    assert!(matches!(app.mode, Mode::Move(_)));
    // Find the spot by filtering: the carried row stays listed.
    type_(&mut app, "game");
    assert!(app.position(&window("$1", "@2")).is_some(), "what is carried stays listed");
    select(&mut app, window("$2", "@3"));
    let lines = screen(&draw(&app, &mut View::new(), 120, 14));
    assert!(lines.iter().any(|l| l.contains("game") && l.contains("⏎ after this window")), "{lines:#?}");
    assert!(lines.iter().any(|l| l.contains("⇅ moving window work:2 logs")), "{lines:#?}");
    assert!(lines.iter().any(|l| l.contains("⇅") && l.contains("logs")), "the carried row is marked: {lines:#?}");
    let j = job(enter(&mut app));
    assert_eq!(j.op, Op::MoveWindow { from: "$1".into(), window: "@2".into(), into: "$2".into(), to: "@3".into(), before: false });
    assert!(matches!(app.mode, Mode::List));
    // On a session: first in it. Already there: nothing to do.
    app.query.set("");
    app.requery();
    select(&mut app, window("$1", "@2"));
    ctrl(&mut app, 't');
    select(&mut app, Target::Session { session: "$2".into() });
    assert_eq!(job(enter(&mut app)).op, Op::MoveWindow { from: "$1".into(), window: "@2".into(), into: "$2".into(), to: "@3".into(), before: true });
    select(&mut app, window("$1", "@2"));
    ctrl(&mut app, 't');
    select(&mut app, window("$1", "@1"));
    assert!(matches!(enter(&mut app), Action::None));
    assert_eq!(app.status, Some(Status::Note("already there".into())));
    // Esc puts it down again.
    ctrl(&mut app, 't');
    app.on_key(key(KeyCode::Esc, KeyModifiers::NONE));
    assert!(matches!(app.mode, Mode::List), "esc cancels the move, not the popup");
}

#[test]
fn panes_drop_into_windows_beside_panes_or_on_their_own() {
    let mut app = sessions_app("@1");
    app.expanded.insert(window("$1", "@1"));
    app.refresh();
    let carry = |app: &mut App, to: Target| {
        select(app, pane("$1", "@1", "%2"));
        ctrl(app, 't');
        select(app, to);
        job(enter(app)).op
    };
    assert_eq!(carry(&mut app, window("$1", "@2")), Op::JoinPane { pane: "%2".into(), to: "%3".into(), before: false });
    assert_eq!(carry(&mut app, pane("$1", "@1", "%1")), Op::SwapPanes("%2".into(), "%1".into()));
    assert_eq!(carry(&mut app, Target::Session { session: "$2".into() }), Op::BreakPane { pane: "%2".into(), session: "$2".into() });
}

#[test]
fn a_server_that_keeps_moves_in_their_session_is_offered_no_others() {
    let mut app = sessions_app("@1");
    app.caps = Caps { cross_session_moves: false, tmux_session_names: false, activate_open: true, rex: true, send: true };
    app.expanded.insert(window("$1", "@1"));
    app.refresh();
    let shift = |app: &mut App, code| app.on_key(key(code, KeyModifiers::SHIFT));
    // ⇧↓ swaps within the session, and stops at its end.
    select(&mut app, window("$1", "@1"));
    assert_eq!(job(shift(&mut app, KeyCode::Down)).op, Op::SwapWindows { session: "$1".into(), a: "@1".into(), b: "@2".into() });
    select(&mut app, window("$1", "@2"));
    assert!(matches!(shift(&mut app, KeyCode::Down), Action::None));
    assert_eq!(app.status, Some(Status::Note("already the last tab in its session".into())));
    // A pane joins the next window of its own session only.
    select(&mut app, pane("$1", "@1", "%2"));
    assert_eq!(job(shift(&mut app, KeyCode::Down)).op, Op::JoinPane { pane: "%2".into(), to: "%3".into(), before: true });
    let mut app = sessions_app("@1");
    app.caps.cross_session_moves = false;
    select(&mut app, window("$2", "@3"));
    assert!(matches!(shift(&mut app, KeyCode::Up), Action::None));
    assert_eq!(app.status, Some(Status::Note("already the first window in its session".into())));
    // Carried, it drops in its own session only; the row says so.
    select(&mut app, window("$1", "@2"));
    ctrl(&mut app, 't');
    for to in [window("$2", "@3"), Target::Session { session: "$2".into() }] {
        select(&mut app, to);
        assert_eq!(app.drop_note(), Err("it moves only within its session".into()));
    }
    select(&mut app, Target::Session { session: "$1".into() });
    assert_eq!(app.drop_note(), Ok("first in this session".into()));
}

#[test]
fn session_names_follow_tmuxs_rules_only_on_tmux() {
    let mut app = sessions_app("@1");
    app.caps.tmux_session_names = false;
    select(&mut app, Target::Session { session: "$1".into() });
    ctrl(&mut app, 'r');
    ctrl(&mut app, 'u');
    type_(&mut app, "play.v2");
    assert_eq!(job(enter(&mut app)).op, Op::RenameSession { session: "$1".into(), name: "play.v2".into() });
    ctrl(&mut app, 'r');
    ctrl(&mut app, 'u');
    type_(&mut app, "play");
    assert_eq!(job(enter(&mut app)).op, Op::RenameSession { session: "$1".into(), name: "play".into() }, "names need not be unique");
    ctrl(&mut app, 'r');
    ctrl(&mut app, 'u');
    assert!(matches!(enter(&mut app), Action::None));
    assert_eq!(renaming(&app).and_then(|r| r.error.as_deref()), Some("a session needs a name"));
}

#[test]
fn dragging_a_row_moves_it() {
    let mut app = sessions_app("@1");
    let mut view = View::new();
    let term = draw(&app, &mut view, 120, 20);
    let (logs, game) = (find(&term, "logs"), find(&term, "game"));
    let now = Instant::now();
    mouse(&mut app, &view, MouseEventKind::Down(MouseButton::Left), logs, now);
    mouse(&mut app, &view, MouseEventKind::Drag(MouseButton::Left), logs, now);
    assert!(matches!(app.mode, Mode::List), "no move until the pointer leaves the row");
    mouse(&mut app, &view, MouseEventKind::Drag(MouseButton::Left), game, now);
    assert!(matches!(app.mode, Mode::Move(_)));
    let j = job(mouse(&mut app, &view, MouseEventKind::Up(MouseButton::Left), game, now));
    assert_eq!(j.op, Op::MoveWindow { from: "$1".into(), window: "@2".into(), into: "$2".into(), to: "@3".into(), before: false });
    // Released off the list: never mind.
    mouse(&mut app, &view, MouseEventKind::Down(MouseButton::Left), logs, now);
    mouse(&mut app, &view, MouseEventKind::Drag(MouseButton::Left), game, now);
    mouse(&mut app, &view, MouseEventKind::Up(MouseButton::Left), (110, 18), now);
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.status, Some(Status::Note("move cancelled".into())));
    // Released outside the popup (no release comes): the next click only
    // ends the drag; it doesn't drop.
    mouse(&mut app, &view, MouseEventKind::Down(MouseButton::Left), logs, now);
    mouse(&mut app, &view, MouseEventKind::Drag(MouseButton::Left), game, now);
    assert!(matches!(click(&mut app, &view, MouseButton::Left, game), Action::None));
    assert!(matches!(app.mode, Mode::List));
}

#[test]
fn a_moved_window_stays_selected_where_it_lands() {
    let mut app = sessions_app("@1");
    select(&mut app, window("$1", "@2"));
    let j = job(app.on_key(key(KeyCode::Down, KeyModifiers::SHIFT)));
    assert_eq!(app.ran(j, Ok(None)), None);
    app.set_snapshot(fixture(&[
        "$1|work|@1|1|edit|1|0|%1|0|1|nvim|||||||||/h/a",
        "$1|work|@1|1|edit|1|0|%2|1|0|zsh|||||||||/h/a",
        "$2|play|@2|1|logs|0|0|%3|0|1|tail|||||||||/h/b",
        "$2|play|@3|2|game|1|0|%4|0|1|codex||asking|0|codex|deploy||||/h/c",
    ]));
    assert_eq!(app.selected_target(), Some(&window("$2", "@2")));
    // A pane that becomes a window of its own is selected as that window.
    let mut app = sessions_app("@1");
    app.follow = Some("%4".into());
    app.set_snapshot(fixture(&["$2|play|@3|1|game|1|0|%4|0|1|codex||asking|0|codex|deploy||||/h/c"]));
    assert_eq!(app.selected_target(), Some(&window("$2", "@3")));
    let mut app = sessions_app("@1");
    select(&mut app, window("$1", "@2"));
    let j = job(app.on_key(key(KeyCode::Down, KeyModifiers::SHIFT)));
    app.ran(j, Ok(None));
    app.set_snapshot(fixture(&[
        "$1|work|@1|1|edit|1|0|%1|0|1|nvim|||||||||/h/a",
        "$1|work|@1|1|edit|1|0|%2|1|0|zsh|||||||||/h/a",
        "$2|play|@2|1|logs|0|0|%3|0|1|tail|||||||||/h/b",
        "$2|play|@3|2|game|1|0|%4|0|1|codex||asking|0|codex|deploy||||/h/c",
    ]));
    // And ^z puts it back, following it again.
    let undo = job(ctrl(&mut app, 'z'));
    assert_eq!(undo.op, Op::MoveWindow { from: "$2".into(), window: "@2".into(), into: "$1".into(), to: "@1".into(), before: false });
    assert_eq!(undo.follow.as_deref(), Some("@2"));
}

#[test]
fn ctrl_s_names_a_new_window_where_it_will_appear() {
    let mut app = sessions_app("@1");
    select(&mut app, window("$1", "@1"));
    ctrl(&mut app, 's');
    let Mode::Create(_) = &app.mode else { panic!("{:?}", app.mode) };
    // Right after "edit", before "logs", and selected.
    let names: Vec<String> = app.rows.iter().map(|r| match &r.kind {
        RowKind::Window { name, .. } => name.clone(),
        RowKind::New => "+".into(),
        _ => "·".into(),
    }).collect();
    assert_eq!(names, ["·", "edit", "+", "logs", "·", "game"]);
    assert!(matches!(app.rows[app.selected].kind, RowKind::New));
    let lines = screen(&draw(&app, &mut View::new(), 120, 14));
    assert!(lines.iter().any(|l| l.contains("├─ + ▏window name (empty: named after what runs)  /h/a")), "{lines:#?}");
    assert!(lines.iter().any(|l| l.contains("+ new window")), "{lines:#?}");
    assert!(lines.iter().any(|l| l.contains("⏎ create   ^s session   ^u clear") && l.ends_with("esc cancel │")), "{lines:#?}");
    let mut view = View::new();
    let term = draw(&app, &mut view, 120, 14);
    click(&mut app, &view, MouseButton::Left, find(&term, "▏window name"));
    assert_eq!(app.status, None, "a click on the row being named");
    click(&mut app, &view, MouseButton::Left, find(&term, "logs"));
    assert_eq!(app.status, Some(Status::Note("⏎ create or esc cancel first".into())));
    assert!(matches!(app.mode, Mode::Create(_)));
    // Moving and filtering wait: keys name it.
    app.on_key(key(KeyCode::Down, KeyModifiers::NONE));
    type_(&mut app, "notes");
    assert_eq!(app.query.text(), "");
    let j = job(enter(&mut app));
    assert_eq!(j.op, Op::NewWindow { session: "$1".into(), after: "@1".into(), name: "notes".into(), dir: "/h/a".into() });
    assert!(j.open, "and goes there");
    assert!(matches!(app.mode, Mode::List));
    assert!(!naming(&app), "the row goes with the mode");
    // On a session row: last in it. No name: tmux names it.
    select(&mut app, Target::Session { session: "$1".into() });
    ctrl(&mut app, 's');
    assert_eq!(job(enter(&mut app)).op, Op::NewWindow { session: "$1".into(), after: "@2".into(), name: String::new(), dir: "/h/a".into() });
    // Esc: nothing made, and the window naming began on stays selected
    // (not "logs", which takes the new row's place).
    select(&mut app, window("$1", "@1"));
    ctrl(&mut app, 's');
    assert!(matches!(app.on_key(key(KeyCode::Esc, KeyModifiers::NONE)), Action::None));
    assert!(matches!(app.mode, Mode::List) && !naming(&app));
    assert_eq!(app.selected_target(), Some(&window("$1", "@1")));
}

#[test]
fn ctrl_s_again_names_a_new_session_and_refuses_what_tmux_would() {
    let mut app = sessions_app("@1");
    select(&mut app, window("$2", "@3"));
    ctrl(&mut app, 's');
    type_(&mut app, "x");
    ctrl(&mut app, 's');
    assert!(matches!(app.rows.last().map(|r| &r.kind), Some(RowKind::New)), "at the end");
    assert!(matches!(&app.mode, Mode::Create(c) if matches!(c.new, New::Session { .. }) && c.field.text() == "x"), "the name stays");
    // And back again.
    ctrl(&mut app, 's');
    assert!(matches!(&app.mode, Mode::Create(c) if matches!(c.new, New::Window { .. })));
    ctrl(&mut app, 's');
    ctrl(&mut app, 'u');
    assert!(matches!(enter(&mut app), Action::None));
    let error = |app: &App| match &app.mode {
        Mode::Create(c) => c.error.clone(),
        _ => None,
    };
    assert_eq!(error(&app).as_deref(), Some("a session needs a name"));
    type_(&mut app, "play");
    assert_eq!(error(&app), None, "typing clears the reason");
    enter(&mut app);
    assert_eq!(error(&app).as_deref(), Some("a session called play exists"));
    let lines = screen(&draw(&app, &mut View::new(), 120, 14));
    assert!(lines.iter().any(|l| l.contains("+ play▏  /h/c") && l.contains("✗ a session called play exists")), "{lines:#?}");
    ctrl(&mut app, 'u');
    type_(&mut app, "blog");
    let j = job(enter(&mut app));
    assert_eq!(j.op, Op::NewSession { name: "blog".into(), dir: "/h/c".into() }, "in the selected row's folder");
    assert!(j.open);
    // From an agent card: the sessions tab, after the agent's window.
    app.run(Command::Tab(Tab::Agents));
    ctrl(&mut app, 's');
    assert_eq!(app.tab, Tab::Sessions);
    assert!(matches!(&app.mode, Mode::Create(Create { new: New::Window { after, .. }, .. }) if after == "@3"));
}

#[test]
fn a_title_is_not_undone_onto_an_exited_agent() {
    let mut app = agents_app();
    ctrl(&mut app, 'r');
    type_(&mut app, "!");
    let j = job(enter(&mut app));
    app.ran(j, Ok(None));
    app.set_snapshot(fixture(&["$1|s|@1|1|w|1|0|%1|0|1|zsh|||||||||/a"]));
    assert!(matches!(ctrl(&mut app, 'z'), Action::None));
    assert_eq!(app.status, Some(Status::Note("that agent has exited".into())));
}

#[test]
fn a_held_ctrl_x_asks_and_never_answers() {
    let mut app = sessions_app("@1");
    select(&mut app, window("$1", "@2"));
    for _ in 0..4 {
        assert!(matches!(ctrl(&mut app, 'x'), Action::None), "^x only asks");
    }
    assert!(matches!(app.mode, Mode::List), "and the next ^x is a no");
    ctrl(&mut app, 'x');
    assert_eq!(job(enter(&mut app)).op, Op::Kill(window("$1", "@2")));
}

#[test]
fn hiding_an_agent_is_undone_by_showing_it() {
    let mut app = hidden_app(false);
    let j = job(ctrl(&mut app, 'e'));
    assert_eq!(j.undo, Some(Op::SetHidden { pane: "%1".into(), hidden: false }));
    complete_hidden(&mut app, Action::Run(j));
    assert_eq!(app.status, Some(Status::Done("agent hidden (still running)   ^z undo".into())));
    assert_eq!(job(ctrl(&mut app, 'z')).op, Op::SetHidden { pane: "%1".into(), hidden: false });
    // Its agent gone, there is nothing to show.
    let mut app = hidden_app(false);
    let action = ctrl(&mut app, 'e');
    complete_hidden(&mut app, action);
    app.set_snapshot(fixture(&["$1|s|@1|1|w|1|0|%1|0|1|zsh|||||||||/a"]));
    assert!(matches!(ctrl(&mut app, 'z'), Action::None));
    assert_eq!(app.status, Some(Status::Note("that agent has exited".into())));
}

#[test]
fn a_pane_moved_into_a_closed_window_opens_it() {
    let mut app = sessions_app("@1");
    app.expanded.insert(window("$1", "@1"));
    app.refresh();
    select(&mut app, pane("$1", "@1", "%2"));
    let j = job(app.on_key(key(KeyCode::Down, KeyModifiers::SHIFT)));
    assert_eq!(j.op, Op::JoinPane { pane: "%2".into(), to: "%3".into(), before: true });
    app.ran(j, Ok(None));
    app.set_snapshot(fixture(&[
        "$1|work|@1|1|edit|1|0|%1|0|1|nvim|||||||||/h/a",
        "$1|work|@2|2|logs|0|0|%2|0|0|zsh|||||||||/h/a",
        "$1|work|@2|2|logs|0|0|%3|1|1|tail|||||||||/h/b",
        "$2|play|@3|1|game|1|0|%4|0|1|codex||asking|0|codex|deploy||||/h/c",
    ]));
    assert_eq!(app.selected_target(), Some(&pane("$1", "@2", "%2")));
    assert!(app.position(&pane("$1", "@2", "%3")).is_some(), "listed with its new neighbour");
    let open = app.position(&window("$1", "@2")).map(|i| &app.rows[i].kind);
    assert!(matches!(open, Some(RowKind::Window { expanded: Some(true), .. })), "drawn open");
}

#[test]
fn a_refused_rename_opens_again_with_what_was_typed() {
    let mut app = sessions_app("@1");
    select(&mut app, window("$1", "@2"));
    ctrl(&mut app, 'r');
    ctrl(&mut app, 'u');
    type_(&mut app, "build");
    let j = job(enter(&mut app));
    assert_eq!(app.ran(j, Err("timed out".into())), None);
    let r = renaming(&app).expect("renaming again");
    assert_eq!((r.target.clone(), r.field.text(), r.error.as_deref()), (window("$1", "@2"), "build", Some("timed out")));
    assert_eq!(app.status, None, "the field says why");
    assert_eq!(app.selected_target(), Some(&window("$1", "@2")));
    // Saved again, it is the same rename.
    let j = job(enter(&mut app));
    assert_eq!(j.op, Op::RenameWindow { window: "@2".into(), name: "build".into() });
    // Its window gone meanwhile: only the status says so.
    app.snap = fixture(&["$1|work|@1|1|edit|1|0|%1|0|1|nvim|||||||||/h/a"]);
    app.ran(j, Err("can't find window: @2".into()));
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(app.status, Some(Status::Failed("can't find window: @2".into())));
}

#[test]
fn a_refused_new_window_opens_again_and_cancels_back_to_its_tab() {
    let mut app = agents_app();
    select(&mut app, pane("$1", "@1", "%2"));
    ctrl(&mut app, 's');
    assert_eq!(app.tab, Tab::Sessions);
    type_(&mut app, "notes");
    let j = job(enter(&mut app));
    app.ran(j, Err("index 2 in use".into()));
    let Mode::Create(c) = &app.mode else { panic!("{:?}", app.mode) };
    assert_eq!((c.field.text(), c.error.as_deref()), ("notes", Some("index 2 in use")));
    assert!(matches!(app.rows[app.selected].kind, RowKind::New));
    assert_eq!(app.status, None);
    // Esc: back to the agents tab, on the card naming began on.
    app.on_key(key(KeyCode::Esc, KeyModifiers::NONE));
    assert!(matches!(app.mode, Mode::List) && !naming(&app));
    assert_eq!(app.tab, Tab::Agents);
    assert_eq!(app.selected_target(), Some(&pane("$1", "@1", "%2")));
}

#[test]
fn suggestions_are_offered_for_agent_titles_only() {
    let suggest = |app: &App| app.hints(&View::new()).0.iter().any(|h| h.command == Command::Suggest);
    let mut app = sessions_app("@1");
    for t in [Target::Session { session: "$1".into() }, window("$1", "@1")] {
        select(&mut app, t);
        ctrl(&mut app, 'r');
        assert!(!suggest(&app), "{:?}", app.mode);
        app.on_key(key(KeyCode::Esc, KeyModifiers::NONE));
    }
    let mut app = agents_app();
    ctrl(&mut app, 'r');
    assert!(suggest(&app));
    // "retry" is for a suggestion that failed, not a save: ^r would put a
    // suggestion over what was typed.
    let label = |app: &App| app.hints(&View::new()).0.iter().find(|h| h.command == Command::Suggest).map(|h| h.label.clone());
    assert!(matches!(ctrl(&mut app, 'r'), Action::Suggest { id: 1, .. }));
    app.suggested(1, Err("no model".into()));
    assert_eq!(label(&app).as_deref(), Some("retry"));
    type_(&mut app, "mine");
    let save = job(enter(&mut app));
    app.ran(save, Err("devbox is not answering".into()));
    assert!(matches!(&app.mode, Mode::Rename(r) if r.error.is_some()));
    assert_eq!(label(&app).as_deref(), Some("suggest"));
}

#[test]
fn a_failed_undo_can_be_tried_again() {
    let mut app = sessions_app("@1");
    select(&mut app, window("$1", "@1"));
    let j = job(app.on_key(key(KeyCode::Down, KeyModifiers::SHIFT)));
    app.ran(j, Ok(None));
    let undo = job(ctrl(&mut app, 'z'));
    let op = undo.op.clone();
    app.ran(undo, Err("no server".into()));
    assert_eq!(app.status, Some(Status::Failed("no server".into())));
    assert!(app.can(Command::Undo, true));
    let again = job(ctrl(&mut app, 'z'));
    assert_eq!(again.op, op);
    app.ran(again, Ok(None));
    assert!(!app.can(Command::Undo, true), "done, it is gone");
}

#[test]
fn an_undo_that_can_no_longer_be_made_makes_way_for_the_older_ones() {
    let mut app = sessions_app("@1");
    select(&mut app, Target::Session { session: "$2".into() });
    ctrl(&mut app, 'r');
    type_(&mut app, "x");
    let rename = job(enter(&mut app));
    app.ran(rename, Ok(None));
    select(&mut app, window("$1", "@1"));
    let swap = job(app.on_key(key(KeyCode::Down, KeyModifiers::SHIFT)));
    app.ran(swap, Ok(None));
    // The window it swapped with closes: that undo fails for good.
    app.set_snapshot(fixture(&["$1|work|@1|1|edit|1|0|%1|0|1|nvim|||||||||/h/a", "$2|play|@3|1|game|1|0|%4|0|1|codex||asking|0|codex|deploy||||/h/c"]));
    let undo = job(ctrl(&mut app, 'z'));
    assert!(matches!(undo.op, Op::SwapWindows { .. }));
    app.ran(undo, Err("can't find window: @2".into()));
    assert_eq!(app.status, Some(Status::Failed("can't find window: @2".into())));
    assert!(matches!(job(ctrl(&mut app, 'z')).op, Op::RenameSession { .. }), "not the swap again");
}

#[test]
fn an_op_whose_answer_was_lost_is_not_offered_a_second_time() {
    // It may have been made: a second new window, or an undo undone.
    let mut app = sessions_app("@1");
    ctrl(&mut app, 's');
    type_(&mut app, "notes");
    let j = job(enter(&mut app)).unsure();
    app.ran(j, Err("devbox stopped answering".into()));
    assert!(matches!(app.mode, Mode::List), "{:?}", app.mode);
    assert_eq!(app.status, Some(Status::Failed("devbox stopped answering".into())));
    select(&mut app, window("$1", "@1"));
    let swap = job(app.on_key(key(KeyCode::Down, KeyModifiers::SHIFT)));
    app.ran(swap, Ok(None));
    let undo = job(ctrl(&mut app, 'z')).unsure();
    app.ran(undo, Err("devbox stopped answering".into()));
    assert!(!app.can(Command::Undo, true));
    // A rename is the same made twice: its field opens again.
    select(&mut app, window("$1", "@1"));
    ctrl(&mut app, 'r');
    type_(&mut app, "x");
    let j = job(enter(&mut app)).unsure();
    app.ran(j, Err("devbox stopped answering".into()));
    assert!(matches!(&app.mode, Mode::Rename(r) if r.error.as_deref() == Some("devbox stopped answering")));
}

#[test]
fn every_way_of_putting_a_carried_row_down_unkeeps_it() {
    let mut app = sessions_app("@1");
    app.caps.cross_session_moves = false;
    let carry = |app: &mut App| {
        select(app, window("$1", "@2"));
        ctrl(app, 't');
        assert_eq!(app.keep, Some(window("$1", "@2")));
    };
    // Refused, already there, cancelled.
    carry(&mut app);
    select(&mut app, window("$2", "@3"));
    assert!(matches!(enter(&mut app), Action::None));
    assert_eq!((app.keep.clone(), app.status.clone()), (None, Some(Status::Note("it moves only within its session".into()))));
    carry(&mut app);
    select(&mut app, window("$1", "@1"));
    enter(&mut app);
    assert_eq!((app.keep.clone(), app.status.clone()), (None, Some(Status::Note("already there".into()))));
    carry(&mut app);
    app.on_key(key(KeyCode::Esc, KeyModifiers::NONE));
    assert!(matches!(app.mode, Mode::List) && app.keep.is_none());
    // Put down, a row the filter does not match is listed no longer.
    carry(&mut app);
    type_(&mut app, "game");
    assert!(app.position(&window("$1", "@2")).is_some(), "carried, it stays listed");
    select(&mut app, window("$2", "@3"));
    enter(&mut app);
    assert!(app.position(&window("$1", "@2")).is_none(), "{:?}", app.status);
    // A drop that asks first, and is answered no, puts it down too.
    let mut app = sessions_app("@1");
    select(&mut app, window("$2", "@3"));
    ctrl(&mut app, 't');
    select(&mut app, window("$1", "@2"));
    assert!(matches!(enter(&mut app), Action::None));
    assert!(matches!(&app.mode, Mode::Confirm(c) if c.question.contains("closes")), "{:?}", app.mode);
    app.on_key(key(KeyCode::Esc, KeyModifiers::NONE));
    assert!(matches!(app.mode, Mode::List) && app.keep.is_none());
}

/// A Rex-shaped server, built from the model: Rex's ids and caps, and
/// two sessions called api, the second on host devbox.
fn rex_app(tab: Tab) -> App {
    use crate::model::{Agent, State};
    let pane = |id: &str, index: &str, command: &str, title: Option<&str>| Pane {
        id: id.into(),
        index: index.into(),
        active: index == "1",
        command: command.into(),
        path: "/src/api".into(),
        agent: title.map(|t| Agent {
            state: State::Working,
            kind: command.into(),
            title: t.into(),
            auto: t.into(),
            renamed: false,
            since: None,
            hidden: false,
            stale: false,
            note: String::new(),
            transcript: String::new(),
            asks: String::new(),
        }),
    };
    let window = |id: &str, index: &str, name: &str, panes| Window { id: id.into(), index: index.into(), name: name.into(), active: index == "1", auto: false, panes };
    let session = |id: &str, host: &str, windows| Session { id: id.into(), name: "api".into(), host: host.into(), away: false, windows };
    let snap = Snapshot::new(vec![
        session("session:1", "", vec![
            window("window:1", "1", "edit", vec![pane("block:1", "1", "zsh", None), pane("block:2", "2", "claude", Some("fix login"))]),
            window("window:2", "2", "logs", vec![pane("block:3", "1", "tail", None)]),
        ]),
        session("session:2", "devbox", vec![window("window:3", "1", "web", vec![pane("block:4", "1", "codex", Some("deploy"))])]),
    ]);
    let mut app = App::new(snap, tab, "session:1".into(), "window:1".into(), String::new(), true, false);
    app.caps = Caps { cross_session_moves: false, tmux_session_names: false, activate_open: true, rex: true, send: true };
    app
}

#[test]
fn rex_says_tab_where_tmux_says_window() {
    let mut app = rex_app(Tab::Sessions);
    let lines = |app: &App| screen(&draw(app, &mut View::new(), 120, 20));
    let no_window = |lines: &[String]| assert!(!lines.iter().any(|l| l.to_lowercase().contains("window")), "{lines:#?}");
    let rows = lines(&app);
    assert!(rows.iter().any(|l| l.contains("api*") && l.contains("2 tabs")), "{rows:#?}");
    no_window(&rows);
    let logs = Target::Window { session: "session:1".into(), window: "window:2".into() };
    select(&mut app, logs);
    ctrl(&mut app, 'a');
    let menu = lines(&app);
    assert!(menu.iter().any(|l| l.contains("new tab here")), "{menu:#?}");
    no_window(&menu);
    app.on_key(key(KeyCode::Esc, KeyModifiers::NONE));
    ctrl(&mut app, 'x');
    let Mode::Confirm(c) = &app.mode else { panic!("{:?}", app.mode) };
    assert_eq!(c.question, "kill tab api:2 logs? 1 pane");
    let confirm = lines(&app);
    assert!(confirm.iter().any(|l| l.contains("✗ kill tab api:2 logs?")), "{confirm:#?}");
    no_window(&confirm);
    app.on_key(key(KeyCode::Esc, KeyModifiers::NONE));
    ctrl(&mut app, 's');
    let create = lines(&app);
    assert!(create.iter().any(|l| l.contains("+ new tab")) && create.iter().any(|l| l.contains("▏tab name")), "{create:#?}");
    no_window(&create);
}

#[test]
fn rex_sessions_sharing_a_name_are_selected_by_id() {
    let mut app = rex_app(Tab::Sessions);
    assert_eq!(app.selected_target(), Some(&Target::Window { session: "session:1".into(), window: "window:1".into() }));
    let remote = Target::Session { session: "session:2".into() };
    select(&mut app, remote.clone());
    assert!(matches!(enter(&mut app), Action::Activate(t) if t == remote));
    // A card names its own session.
    let mut app = rex_app(Tab::Agents);
    select(&mut app, Target::Pane { session: "session:2".into(), window: "window:3".into(), pane: "block:4".into() });
    assert_eq!(selected_title(&app), "deploy");
}

#[test]
fn a_rex_session_sharing_a_name_is_renamed_by_id() {
    let mut app = rex_app(Tab::Sessions);
    select(&mut app, Target::Session { session: "session:2".into() });
    ctrl(&mut app, 'r');
    assert_eq!(field(&app), Some("api"));
    ctrl(&mut app, 'u');
    type_(&mut app, "api.v2");
    let j = job(enter(&mut app));
    assert_eq!(j.op, Op::RenameSession { session: "session:2".into(), name: "api.v2".into() });
    assert_eq!(j.undo, Some(Op::RenameSession { session: "session:2".into(), name: "api".into() }));
    // The remote host timed out: the field again, on the same session.
    app.ran(j, Err("devbox: timed out".into()));
    let r = renaming(&app).expect("renaming again");
    assert_eq!((r.target.clone(), r.error.as_deref()), (Target::Session { session: "session:2".into() }, Some("devbox: timed out")));
}

#[test]
fn rex_moves_are_offered_within_their_session_only() {
    let mut app = rex_app(Tab::Sessions);
    let shift = |app: &mut App, code| app.on_key(key(code, KeyModifiers::SHIFT));
    let logs = Target::Window { session: "session:1".into(), window: "window:2".into() };
    select(&mut app, logs.clone());
    assert!(matches!(shift(&mut app, KeyCode::Down), Action::None));
    assert_eq!(app.status, Some(Status::Note("already the last tab in its session".into())));
    assert_eq!(job(shift(&mut app, KeyCode::Up)).op, Op::SwapWindows { session: "session:1".into(), a: "window:2".into(), b: "window:1".into() });
    // Carried to the other api, it says it stays; dropped, it does.
    ctrl(&mut app, 't');
    select(&mut app, Target::Window { session: "session:2".into(), window: "window:3".into() });
    assert_eq!(app.drop_note(), Err("it moves only within its session".into()));
    assert!(matches!(enter(&mut app), Action::None));
    assert!(matches!(app.mode, Mode::List) && app.keep.is_none());
    select(&mut app, Target::Window { session: "session:2".into(), window: "window:3".into() });
    assert!(matches!(shift(&mut app, KeyCode::Up), Action::None));
    assert_eq!(app.status, Some(Status::Note("already the first tab in its session".into())));
}

#[test]
fn typing_sends_every_key_but_the_one_that_leaves() {
    let mut app = agents_app();
    let view = View::new();
    let pane = app.rows[app.selected].preview.clone().unwrap();
    assert!(matches!(ctrl(&mut app, 'l'), Action::None));
    assert!(matches!(&app.mode, Mode::Type(t) if t.pane == pane), "{:?}", app.mode);
    for k in [key(KeyCode::Char('y'), KeyModifiers::NONE), key(KeyCode::Esc, KeyModifiers::NONE), key(KeyCode::Enter, KeyModifiers::NONE), key(KeyCode::Char('c'), KeyModifiers::CONTROL), key(KeyCode::Down, KeyModifiers::NONE)] {
        assert!(matches!(app.on_key(k), Action::Send(p, Input::Key(sent)) if p == pane && sent == k), "{k:?}");
    }
    assert!(matches!(app.paste("a\nb"), Action::Send(p, Input::Paste(t)) if p == pane && t == "a\nb"));
    assert!(matches!(app.mode, Mode::Type(_)), "esc and ^c went to the pane");
    // The preview stays on the pane typed into, whatever is selected.
    app.selected = app.rows.iter().position(|r| r.preview.as_deref() != Some(pane.as_str()) && r.preview.is_some()).unwrap();
    assert_eq!(app.preview_pane(&view), Some(pane.as_str()));
    assert_eq!(app.hints(&view).1.key, "^h");
    // ^] arrives as ^5.
    assert!(matches!(ctrl(&mut app, '5'), Action::None));
    assert!(matches!(app.mode, Mode::List));
    assert!(app.query.is_empty(), "nothing typed went to the filter");
    // ^h, back from ^l; Backspace goes to the pane.
    ctrl(&mut app, 'l');
    let backspace = key(KeyCode::Backspace, KeyModifiers::NONE);
    assert!(matches!(app.on_key(backspace), Action::Send(_, Input::Key(k)) if k == backspace));
    assert!(matches!(ctrl(&mut app, 'h'), Action::None));
    assert!(matches!(app.mode, Mode::List));
}

/// Agents that need you: %3 waiting longest, %1 waiting since, %2 finished;
/// %4 works.
fn needy_app(tab: Tab) -> App {
    let snap = fixture(&[
        "$1|s|@1|1|w|1|0|%1|0|1|claude||asking|0|claude|one||200||/a",
        "$1|s|@1|1|w|1|0|%2|1|0|claude||idle|1|claude|two||100||/a",
        "$1|s|@2|2|w|0|0|%3|0|1|claude||asking|0|claude|three||100||/b",
        "$1|s|@2|2|w|0|0|%4|1|0|claude||working|0|claude|four||||/b",
    ]);
    new(snap, tab, "@1")
}

fn selected_pane(app: &App) -> String {
    match app.selected_target() {
        Some(Target::Pane { pane, .. }) => pane.clone(),
        other => panic!("not an agent: {other:?}"),
    }
}

#[test]
fn ctrl_f_walks_the_agents_that_need_you() {
    let mut app = needy_app(Tab::Sessions);
    let view = View::new();
    assert!(app.hints(&view).0.iter().any(|h| h.command == Command::Next && h.key == "^f"));
    ctrl(&mut app, 'f');
    assert_eq!((app.tab, selected_pane(&app)), (Tab::Agents, "%3".to_string()), "to the agents tab, waiting longest first");
    ctrl(&mut app, 'f');
    assert_eq!(selected_pane(&app), "%1");
    ctrl(&mut app, 'f');
    assert_eq!(selected_pane(&app), "%2", "then the finished");
    ctrl(&mut app, 'f');
    assert_eq!(selected_pane(&app), "%3", "round again");
    // A query that hides the next one is cleared.
    type_(&mut app, "four");
    ctrl(&mut app, 'f');
    assert!(app.query.is_empty());
    assert_eq!(selected_pane(&app), "%3", "from a card not queued: the first");

    let mut calm = agents_app();
    ctrl(&mut calm, 'f');
    assert_eq!(calm.status, Some(Status::Note("no other agent needs you".into())));
    assert!(!calm.hints(&view).0.iter().any(|h| h.command == Command::Next), "offered only when someone needs you");
}

#[test]
fn ctrl_a_then_a_closes_as_prefix_a_opened() {
    let mut app = needy_app(Tab::Agents);
    ctrl(&mut app, 'a');
    assert!(matches!(app.mode, Mode::Menu(_)));
    assert!(matches!(app.on_key(key(KeyCode::Char('a'), KeyModifiers::NONE)), Action::Quit));
    // ^a A: prefix A, the next agent.
    let mut app = needy_app(Tab::Agents);
    ctrl(&mut app, 'a');
    app.on_key(key(KeyCode::Char('A'), KeyModifiers::SHIFT));
    assert!(matches!(app.mode, Mode::List));
    assert_eq!(selected_pane(&app), "%2", "opened on %1, waiting since; then the finished");
    // A menu opened by a tap: a is typed into the filter.
    let mut app = needy_app(Tab::Agents);
    app.run(Command::Menu);
    assert!(matches!(app.on_key(key(KeyCode::Char('a'), KeyModifiers::NONE)), Action::None));
    assert_eq!(app.query.text(), "a");
}

#[test]
fn a_click_only_ends_typing() {
    let mut app = agents_app();
    let mut view = View::new();
    draw(&app, &mut view, 120, 30);
    ctrl(&mut app, 'l');
    let before = app.selected;
    let term = draw(&app, &mut view, 120, 30);
    let at = find(&term, "four");
    assert!(matches!(mouse(&mut app, &view, MouseEventKind::Down(MouseButton::Left), at, Instant::now()), Action::None));
    assert!(matches!(app.mode, Mode::List));
    assert!(matches!(mouse(&mut app, &view, MouseEventKind::Up(MouseButton::Left), at, Instant::now()), Action::None), "its release opens nothing");
    assert_eq!(app.selected, before);
}

#[test]
fn typing_needs_the_preview_and_the_pane() {
    let mut app = agents_app();
    app.set_preview_fits(false);
    ctrl(&mut app, 'l');
    assert!(matches!(app.mode, Mode::List));
    assert!(matches!(&app.status, Some(Status::Note(n)) if n.contains("no room")));
    app.set_preview_fits(true);
    app.preview = Some(false);
    ctrl(&mut app, 'l');
    assert!(matches!(app.mode, Mode::Type(_)) && app.preview_on(false), "the preview comes on");
    app.set_preview_fits(false);
    assert!(matches!(app.mode, Mode::List), "no typing blind");
    ctrl(&mut app, 'l');
    app.set_preview_fits(true);
    ctrl(&mut app, 'l');
    let Mode::Type(t) = &app.mode else { panic!("typing") };
    let gone = t.pane.clone();
    let lines: Vec<String> = ["$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|one||||/a", "$1|s|@1|1|w|1|0|%2|1|0|claude||working|0|claude|two||||/a", "$1|s|@2|2|w|0|0|%3|0|1|claude||working|0|claude|three||||/b", "$1|s|@2|2|w|0|0|%4|1|0|claude||working|0|claude|four||||/b"]
        .into_iter()
        .filter(|l| !l.contains(&format!("|{gone}|")))
        .map(String::from)
        .collect();
    app.set_snapshot(fixture(&lines.iter().map(String::as_str).collect::<Vec<_>>()));
    assert!(matches!(app.mode, Mode::List));
    assert!(matches!(&app.status, Some(Status::Failed(f)) if f.contains("closed")));
    app.caps.send = false;
    let view = View::new();
    assert!(!app.hints(&view).0.iter().any(|h| h.command == Command::Type), "not offered where it can't be done");
}

#[test]
fn typing_keeps_the_preview_when_its_agent_leaves_the_list() {
    let mut app = new(fixture(&["$1|s|@1|1|w|1|0|%1|0|1|claude||working|0|claude|one||||/a"]), Tab::Agents, "@1");
    let mut view = View::new();
    ctrl(&mut app, 'l');
    assert!(matches!(app.mode, Mode::Type(_)));
    // The agent exits; its pane, now a shell, stays and is typed into.
    app.set_snapshot(fixture(&["$1|s|@1|1|w|1|0|%1|0|1|zsh|||||||||/a"]));
    assert!(app.rows.is_empty() && matches!(app.mode, Mode::Type(_)));
    let lines = screen(&draw(&app, &mut view, 120, 30));
    let ruled = |l: &String| l.chars().skip(1).take(117).any(|c| c == '│');
    assert!(lines[4..20].iter().all(ruled), "the preview is still beside the list: {lines:#?}");
}

#[test]
fn settled_cards_show_what_was_said_and_the_changes_waiting() {
    let mut snap = fixture(&[
        "$1|s|@1|1|w|1|0|%1|0|1|claude||asking|0|claude|one||||/a",
        "$1|s|@1|1|w|1|0|%2|1|0|claude||working|0|claude|two||||/a",
    ]);
    for (p, file) in [(0, "/t1"), (1, "/t2")] {
        snap.sessions[0].windows[0].panes[p].agent.as_mut().unwrap().transcript = file.into();
    }
    let mut app = new(snap, Tab::Agents, "@1");
    let card = |app: &App, pane: &str| {
        app.rows.iter().find_map(|r| match (&r.kind, r.preview.as_deref()) {
            (RowKind::Agent { note, diff, .. }, Some(p)) if p == pane => Some((note.clone(), diff.clone())),
            _ => None,
        })
    };
    assert!(app.set_note("/t1".into(), (State::Asking, None, String::new()), "Bash: cargo test".into()));
    assert!(app.set_note("/t2".into(), (State::Asking, None, String::new()), "read for another state".into()));
    assert!(app.set_diff("/a".into(), Some(Diff { added: 3, removed: 1, new: 0 })));
    assert_eq!(card(&app, "%1"), Some(("Bash: cargo test".into(), "+3 −1".into())));
    assert_eq!(card(&app, "%2"), Some((String::new(), String::new())), "working: neither");
    assert!(app.set_note("/t1".into(), (State::Asking, Some(9), String::new()), "an older ask".into()));
    assert_eq!(card(&app, "%1").unwrap().0, "", "read for the agent's state since another time");
    app.set_note("/t1".into(), (State::Asking, None, String::new()), "Bash: cargo test".into());
    type_(&mut app, "cargo");
    assert_eq!(app.rows.iter().filter(|r| r.target.is_some()).count(), 1, "what it said is searched too");
    app.run(Command::Clear);
    assert!(!app.set_diff("/a".into(), Some(Diff { added: 3, removed: 1, new: 0 })), "the same: no redraw");
    assert!(app.set_diff("/a".into(), Some(Diff::default())), "clean");
    assert_eq!(card(&app, "%1").unwrap().1, "");
}

#[test]
fn a_jump_to_a_pane_stays_in_the_clients_own_session() {
    // Window @1 is in $2 (listed first, by name) and in $1, where the client is.
    let snap = fixture(&["$2|a|@1|1|w|1|0|%1|0|1|claude||asking|0|claude|||||/a", "$1|b|@1|1|w|1|0|%1|0|1|claude||asking|0|claude|||||/a", "$1|b|@2|2|v|0|0|%2|0|1|zsh|||||||||/a"]);
    let app = new(snap, Tab::Agents, "@2");
    assert_eq!(app.local(pane("$2", "@1", "%1")), pane("$1", "@1", "%1"));
    assert_eq!(app.local(pane("$1", "@2", "%2")), pane("$1", "@2", "%2"));
    assert_eq!(app.local(window("$2", "@1")), window("$2", "@1"), "only panes");
}
