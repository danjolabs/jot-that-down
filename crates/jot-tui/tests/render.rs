//! Snapshot tests over the rendered ratatui buffer.
//!
//! `stage5.md` asks for these so layout changes are deliberate. They render into a `TestBackend`
//! at a fixed size, which makes a frame a pure function of the app state and a fixed clock — no
//! pty, no terminal, no timing.
//!
//! The clock is pinned rather than `Utc::now()` on purpose: the age column would otherwise make
//! every snapshot fail a second after it was taken.

use chrono::{DateTime, TimeZone, Utc};
use jot_core::query::Draft;
use jot_core::workspace::Workspace;
use jot_tui::app::App;
use jot_tui::key::Action;
use jot_tui::ui;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use tempfile::TempDir;

/// The size every snapshot renders at. Wide enough for a title and the meta column, short enough
/// that a snapshot stays readable in a diff.
const SIZE: (u16, u16) = (72, 12);

/// A fixed "now", so the relative-age column is stable.
fn clock() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 4, 12, 0, 0).unwrap()
}

/// A vault with the given titles, oldest first.
fn vault(titles: &[&str]) -> (TempDir, App) {
    let tmp = tempfile::tempdir().unwrap();
    let mut ws = Workspace::init(tmp.path()).unwrap();
    for title in titles {
        ws.create(Draft::new("body").title(*title)).unwrap();
    }
    ws.sync().unwrap();
    let app = App::new(ws);
    (tmp, app)
}

/// Render one frame and return it as text, one line per row.
fn render(app: &App) -> String {
    render_at(app, SIZE.0, SIZE.1)
}

/// [`render`], at an explicit size.
fn render_at(app: &App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| ui::draw(frame, app, clock()))
        .unwrap();

    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Snapshot a frame with the age column normalised.
///
/// A note's `created_at` is decoded from its UUIDv7, which is minted at *test* time, while the
/// clock above is fixed — so the rendered age is a function of when the suite happens to run and
/// would otherwise make every snapshot flaky by the hour. The column's presence and alignment are
/// what these snapshots are for; its exact value is covered by `ui`'s own unit tests.
macro_rules! assert_frame {
    ($frame:expr) => {
        insta::with_settings!({filters => vec![
            (r"\b(?:now|ahead|\d+[mhdw])\b", "[age]"),
        ]}, {
            insta::assert_snapshot!($frame);
        });
    };
}

#[test]
fn the_timeline_lists_notes_newest_first() {
    let (_tmp, app) = vault(&["first thought", "second thought", "third"]);
    assert_frame!(render(&app));
}

#[test]
fn an_empty_vault_says_so_rather_than_painting_a_blank_frame() {
    let (_tmp, app) = vault(&[]);
    assert_frame!(render(&app));
}

#[test]
fn the_help_overlay_lists_every_binding() {
    let (_tmp, mut app) = vault(&["a note"]);
    app.dispatch(Action::Help);
    // Taller than the others: the overlay is one row per binding and must not be clipped.
    assert_frame!(render_at(&app, 72, 26));
}

#[test]
fn the_search_view_shows_the_query_in_its_header() {
    let (_tmp, mut app) = vault(&["alpha", "beta", "alphabet"]);
    app.dispatch(Action::Search);
    for c in "alpha".chars() {
        app.dispatch(Action::Insert(c));
    }
    assert_frame!(render(&app));
}

#[test]
fn the_files_view_names_its_sort_order() {
    let (_tmp, mut app) = vault(&["b note", "a note"]);
    app.dispatch(Action::NextView); // files
    app.dispatch(Action::CycleSort); // oldest
    assert_frame!(render(&app));
}

#[test]
fn a_long_title_is_truncated_rather_than_pushing_the_age_off_the_edge() {
    let (_tmp, app) =
        vault(&["a title long enough that it cannot possibly fit inside the column it is given"]);
    let frame = render(&app);

    for line in frame.lines() {
        assert!(
            line.chars().count() <= SIZE.0 as usize,
            "`{line}` overflows the {}-column frame",
            SIZE.0
        );
    }
    assert_frame!(frame);
}

#[test]
fn a_cjk_title_does_not_break_the_frame() {
    // Two columns per character. Truncating on `chars()` rather than display width would overflow
    // the cell and corrupt the right-hand border, which is invisible in an ASCII-only test.
    let (_tmp, app) = vault(&["안녕하세요 반갑습니다 이것은 아주 긴 제목입니다 그리고 더 깁니다"]);
    assert_frame!(render(&app));
}

#[test]
fn the_status_line_shows_the_prefix_hint_while_it_is_armed() {
    use jot_tui::key::{Keymap, Mode, Resolved};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let (_tmp, mut app) = vault(&["a note"]);
    let space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
    assert_eq!(app.keymap().resolve(space, Mode::Normal), Resolved::Armed);

    let frame = render(&app);
    assert!(
        frame.contains("Space"),
        "an armed prefix must say so, or a swallowed keystroke reads as a hang:\n{frame}"
    );

    // And the hint names a real binding rather than inventing one.
    assert!(Keymap::bindings().iter().any(|b| b.keys == "Space q"));
}
