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
use unicode_width::UnicodeWidthStr;

/// The size every snapshot renders at. Wide enough for a title and the meta column, short enough
/// that a snapshot stays readable in a diff.
const SIZE: (u16, u16) = (72, 12);

/// A fixed "now", so the relative-age column is stable.
fn clock() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 4, 12, 0, 0).unwrap()
}

/// A vault with the given titles, oldest first.
fn vault(titles: &[&str]) -> (TempDir, App) {
    bodies(&titles.iter().map(|t| (*t, "body")).collect::<Vec<_>>())
}

/// A vault with the given title-and-body pairs, oldest first.
fn bodies(notes: &[(&str, &str)]) -> (TempDir, App) {
    let tmp = tempfile::tempdir().unwrap();
    let mut ws = Workspace::init(tmp.path()).unwrap();
    for (title, body) in notes {
        ws.create(Draft::new(*body).title(*title)).unwrap();
    }
    ws.sync().unwrap();
    let app = App::new(ws);
    (tmp, app)
}

/// Render one frame and return it as text, one line per row.
fn render(app: &mut App) -> String {
    render_at(app, SIZE.0, SIZE.1)
}

/// A width wide enough that [`ui::split_main`] puts a reader panel beside the list.
const WIDE: (u16, u16) = (110, 12);

/// [`render`], at an explicit size.
///
/// Prepares the reader panel first, because `ui` may only *read* the rendered lines — the run loop
/// is what asks for them, and at this size there is a panel to fill. The default highlighter is
/// `preview::Plain`, so no subprocess is spawned and the frame does not depend on whether the
/// machine running the suite happens to have `bat`.
fn render_at(app: &mut App, width: u16, height: u16) -> String {
    let frame = render_raw_at(app, width, height);
    normalise(frame, app)
}

/// The frame exactly as painted, ids and all.
///
/// What the assertions that measure widths use. [`render_at`] masks ids so a snapshot can be
/// stable, and a masked frame is the wrong thing to measure — the mask is not the width the
/// terminal actually painted.
fn render_raw_at(app: &mut App, width: u16, height: u16) -> String {
    app.prepare_preview(ui::reader_text_width(width, height));
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

/// Replace everything in a frame that a clock or a random id decides, keeping the frame's width.
///
/// An abbreviation is random and randomly *wide*: the first eight hex characters of a UUIDv7 are
/// the top 32 bits of a millisecond timestamp, so notes a test creates in a burst share them and
/// each id then grows independently until it is unique — twelve characters on one run, thirteen on
/// the next. Nothing about that can be pinned, so a snapshot that carried real ids would fail at
/// random, and one that carried same-width masks would fail whenever the width moved.
///
/// So the *cell* is masked, not the id: the abbreviation plus the padding that follows it becomes
/// one ten-column token, which is the same width on every row and on every run. Every column to
/// the right of it then lands in a fixed place, which is what these snapshots are checking. The
/// columns do sit further left here than the terminal painted them — the difference is put back as
/// filler before the closing border, so the frame's own width still holds and a panel that grew by
/// a column still fails the snapshot.
///
/// The assertions that measure real widths use [`render_raw_at`] instead, for the same reason.
fn normalise(frame: String, app: &App) -> String {
    // Full ids first: the reader's title bar carries one, and an abbreviation is a *prefix* of it,
    // so masking the short form first would eat the front of the long one and leave a tail behind.
    // A UUID is always 36 characters, so this substitution is width-preserving on its own.
    let full: Vec<String> = app
        .rows()
        .iter()
        .map(|row| row.note.id.to_string())
        .collect();
    let ids: Vec<String> = app
        .rows()
        .iter()
        .map(|row| app.short_id(row.note.id))
        .collect();

    frame
        .lines()
        .map(|line| {
            let before = line.width();
            let mut out = line.to_string();
            for id in &full {
                out = out.replace(id.as_str(), FULL_MASK);
            }
            for id in &ids {
                out = replace_cell(&out, id);
            }
            // An overlay can land on top of a row and leave only the front of an id showing. That
            // stub is still random, so it still has to go — masked to its own width, which needs
            // no compensating fill.
            for id in &ids {
                for len in (4..id.len()).rev() {
                    out = out.replace(&id[..len], &stub(len));
                }
            }
            out = mask_ages(&out);
            // Give back what the mask took, so the border stays where it was painted.
            let fill = before.saturating_sub(out.width());
            match out.pop() {
                Some(last) => {
                    let filler = if last == '\u{2510}' || last == '\u{2518}' {
                        '\u{2500}'
                    } else {
                        ' '
                    };
                    format!("{out}{}{last}", filler.to_string().repeat(fill))
                }
                None => out,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Replace every relative age in a line with a fixed-width token.
///
/// The ages are the other thing in a frame that nothing can pin. A note's `created_at` is decoded
/// from a UUIDv7 minted at test time while [`clock`] is fixed, so the *text* moves with the hour —
/// and, worse, so does its *width*: `4h` is two columns and `ahead` is five. That is a bomb on a
/// timer, and it went off. These snapshots were recorded in the morning and began failing at noon,
/// when real time passed the fixed clock and every age flipped from `4h` to `ahead`, shifting three
/// columns of padding on every row of every frame.
///
/// Substituting a fixed-width token defuses it. The grammar is the whole of `ui::relative`'s
/// output — `now`, `ahead`, or a count and a unit — matched on whole space-separated tokens, so a
/// title that happens to contain one of those words is left alone.
fn mask_ages(line: &str) -> String {
    line.split(' ')
        .map(|token| if is_age(token) { AGE_MASK } else { token })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether a token is something `ui::relative` could have produced.
fn is_age(token: &str) -> bool {
    if token == "now" || token == "ahead" {
        return true;
    }
    let Some(unit) = token.chars().last() else {
        return false;
    };
    "mhdw".contains(unit)
        && token.len() > 1
        && token[..token.len() - 1].chars().all(|c| c.is_ascii_digit())
}

/// What an age is masked to. Five columns, the widest `ui::relative` can produce.
const AGE_MASK: &str = "[age]";

/// The token an id cell is masked to: thirteen columns of id and two of gap, which is the column
/// the list paints in every vault that does not capture twice in one millisecond.
const MASK: &str = "id-----------  ";

/// A `len`-character stand-in for a truncated id.
fn stub(len: usize) -> String {
    format!("id{}", "-".repeat(len.saturating_sub(2)))
}

/// What a full UUID is masked to, at exactly a UUID's 36 characters.
const FULL_MASK: &str = "id----------------------------------";

/// Replace `id`, and the run of spaces that pads it out to the next column, with [`MASK`].
fn replace_cell(line: &str, id: &str) -> String {
    let Some(at) = line.find(id) else {
        return line.to_string();
    };
    let rest = &line[at + id.len()..];
    let padding = rest.len() - rest.trim_start_matches(' ').len();
    format!("{}{MASK}{}", &line[..at], &rest[padding..])
}

/// Snapshot a frame. Ids and ages are already normalised by [`render_at`].
///
/// This used to carry an `insta` filter for the age, which was not enough: a filter substitutes
/// text and leaves the *width* of what it replaced already baked into the surrounding padding.
/// [`mask_ages`] does it before the frame is measured, which is the only place it can be done
/// without lying about the layout.
macro_rules! assert_frame {
    ($frame:expr) => {
        insta::assert_snapshot!($frame);
    };
}

#[test]
fn the_timeline_lists_notes_newest_first() {
    let (_tmp, mut app) = vault(&["first thought", "second thought", "third"]);
    assert_frame!(render(&mut app));
}

#[test]
fn an_empty_vault_says_so_rather_than_painting_a_blank_frame() {
    let (_tmp, mut app) = vault(&[]);
    assert_frame!(render(&mut app));
}

#[test]
fn the_help_overlay_lists_every_binding() {
    let (_tmp, mut app) = vault(&["a note"]);
    app.dispatch(Action::Help);
    // Taller than the others: the overlay is one row per binding and must not be clipped.
    assert_frame!(render_at(&mut app, 72, 26));
}

#[test]
fn the_search_view_shows_the_query_in_its_header() {
    let (_tmp, mut app) = vault(&["alpha", "beta", "alphabet"]);
    app.dispatch(Action::Search);
    for c in "alpha".chars() {
        app.dispatch(Action::Insert(c));
    }
    assert_frame!(render(&mut app));
}

#[test]
fn the_files_view_names_its_sort_order() {
    let (_tmp, mut app) = vault(&["b note", "a note"]);
    app.dispatch(Action::NextView); // files
    app.dispatch(Action::CycleSort); // oldest
    assert_frame!(render(&mut app));
}

#[test]
fn a_long_title_is_truncated_rather_than_pushing_the_age_off_the_edge() {
    let (_tmp, mut app) =
        vault(&["a title long enough that it cannot possibly fit inside the column it is given"]);
    let raw = render_raw_at(&mut app, SIZE.0, SIZE.1);

    for line in raw.lines() {
        assert!(
            line.chars().count() <= SIZE.0 as usize,
            "`{line}` overflows the {}-column frame",
            SIZE.0
        );
    }
    assert_frame!(render(&mut app));
}

#[test]
fn a_cjk_title_does_not_break_the_frame() {
    // Two columns per character. Truncating on `chars()` rather than display width would overflow
    // the cell and corrupt the right-hand border, which is invisible in an ASCII-only test.
    let (_tmp, mut app) =
        vault(&["안녕하세요 반갑습니다 이것은 아주 긴 제목입니다 그리고 더 깁니다"]);
    assert_frame!(render(&mut app));
}

#[test]
fn the_help_overlay_shows_every_description_in_full() {
    let (_tmp, mut app) = vault(&["a note"]);
    app.dispatch(Action::Help);
    let frame = render_at(&mut app, 100, 26);

    // Sizing the popup by a guessed constant clipped `Tab`'s row, which is the longest and the one
    // a newcomer most needs whole. Assert every description survives rather than eyeballing it.
    for binding in jot_tui::key::Keymap::bindings() {
        assert!(
            frame.contains(binding.description),
            "`{}` is clipped out of the help overlay:\n{frame}",
            binding.description
        );
    }
}

#[test]
fn the_key_bar_is_wider_on_a_wider_terminal_and_never_overflows() {
    let (_tmp, mut app) = vault(&["a note"]);

    let narrow = render_at(&mut app, 40, 8);
    let wide = render_at(&mut app, 120, 8);

    let bar = |frame: &str| frame.lines().last().unwrap().to_string();
    let (narrow_bar, wide_bar) = (bar(&narrow), bar(&wide));

    assert!(
        wide_bar.width() > narrow_bar.width(),
        "a wider terminal should offer more keys:\n40: {narrow_bar}\n120: {wide_bar}"
    );
    assert!(
        narrow_bar.width() <= 40,
        "the key bar must drop hints rather than overflow: {narrow_bar}"
    );
    // Dropping happens from the right, and the pinned tail is reserved before anything else is
    // spent, so `?` and the way out survive every width.
    assert!(
        narrow_bar.ends_with("?  q"),
        "`?` finds every other key and `q` is the way out, and both are pinned to the end at \
         every width: {narrow_bar}"
    );
}

#[test]
fn the_key_bar_follows_the_view() {
    let (_tmp, mut app) = vault(&["a note"]);

    let timeline = render_at(&mut app, 120, 8);
    assert!(timeline.contains(" f "), "flat is offered on the timeline");
    assert!(!timeline.contains(" s "), "sort is not");

    app.dispatch(Action::NextView); // files
    let files = render_at(&mut app, 120, 8);
    assert!(files.contains(" s "), "sort is offered in the files view");
    assert!(!files.contains(" f "), "flat is not");
}

#[test]
fn the_status_line_shows_the_prefix_hint_while_it_is_armed() {
    use jot_tui::key::{Keymap, Mode, Resolved};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let (_tmp, mut app) = vault(&["a note"]);
    let space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
    assert_eq!(app.keymap().resolve(space, Mode::Normal), Resolved::Armed);

    let frame = render(&mut app);
    assert!(
        frame.contains("Space"),
        "an armed prefix must say so, or a swallowed keystroke reads as a hang:\n{frame}"
    );

    // And the hint names real bindings rather than inventing them.
    for keys in ["Space n", "Space r", "Space q", "Space e", "Space x"] {
        assert!(Keymap::bindings().iter().any(|b| b.keys == keys), "{keys}");
    }
}

#[test]
fn a_wide_frame_puts_a_reader_beside_the_list() {
    let (_tmp, mut app) = bodies(&[
        ("older", "an earlier thought"),
        ("the note in focus", "the body of the focused note"),
    ]);
    let frame = render_at(&mut app, WIDE.0, WIDE.1);

    assert!(
        frame.contains("the body of the focused note"),
        "the reader must show the focused note's body:\n{frame}"
    );
    assert!(
        frame.contains("# the note in focus"),
        "and its title, promoted out of the frontmatter nobody reads:\n{frame}"
    );
    assert_frame!(frame);
}

#[test]
fn a_narrow_frame_drops_the_reader_rather_than_squeezing_both() {
    let (_tmp, mut app) = bodies(&[("a note", "a body nobody can see")]);
    let frame = render_at(&mut app, 72, 12);

    assert!(
        !frame.contains("a body nobody can see"),
        "72 columns cannot carry two bordered panels:\n{frame}"
    );
    assert_eq!(
        ui::reader_text_width(72, 12),
        None,
        "and the run loop must be told so, or it would render a panel into nothing"
    );
}

#[test]
fn moving_the_selection_moves_the_reader_with_it() {
    let (_tmp, mut app) = bodies(&[("older", "the older body"), ("newer", "the newer body")]);

    // Newest first, so the selection starts on `newer`.
    let first = render_at(&mut app, WIDE.0, WIDE.1);
    assert!(first.contains("the newer body"), "{first}");

    app.dispatch(Action::MoveDown);
    let second = render_at(&mut app, WIDE.0, WIDE.1);
    assert!(second.contains("the older body"), "{second}");
    assert!(!second.contains("the newer body"), "{second}");
}

#[test]
fn the_reader_names_the_focused_note_by_its_full_id() {
    let (_tmp, mut app) = vault(&["a note"]);
    let raw = render_raw_at(&mut app, WIDE.0, WIDE.1);
    let id = app.focused().unwrap().note.id;

    assert!(
        raw.contains(&id.to_string()),
        "the panel has the width for the whole id, and an abbreviation there is only a shorter \
         thing to retype:\n{raw}"
    );
    assert_eq!(
        raw.matches(app.short_id(id).as_str()).count(),
        2,
        "the row's abbreviation is a prefix of the panel's full id, so it appears twice — once \
         short in the list, once inside the long one:\n{raw}"
    );
}

#[test]
fn no_row_overflows_the_frame_beside_a_reader() {
    // Measured unfiltered, because `assert_frame!` normalises the id width away. A long title in
    // a narrowed list is exactly where the column arithmetic would first fail to close.
    let (_tmp, mut app) =
        vault(&["a title long enough that it cannot possibly fit inside the column it is given"]);
    let frame = render_raw_at(&mut app, WIDE.0, WIDE.1);

    for line in frame.lines() {
        assert!(
            line.width() <= WIDE.0 as usize,
            "`{line}` overflows the {}-column frame",
            WIDE.0
        );
    }
}

#[test]
fn an_empty_list_leaves_the_reader_saying_so_rather_than_blank() {
    let (_tmp, mut app) = vault(&[]);
    let frame = render_at(&mut app, WIDE.0, WIDE.1);
    assert!(
        frame.contains("Nothing selected"),
        "an empty panel is indistinguishable from a broken one:\n{frame}"
    );
    assert_frame!(frame);
}

#[test]
fn every_row_shows_the_id_that_jot_show_would_accept() {
    let (_tmp, mut app) = vault(&["first", "second"]);
    let frame = render_raw_at(&mut app, SIZE.0, SIZE.1);

    for row in app.rows() {
        let short = app.short_id(row.note.id);
        assert!(
            short.len() >= 8,
            "`{short}` is shorter than the CLI's floor, so the two surfaces would disagree"
        );
        assert!(
            frame.contains(short.as_str()),
            "`{short}` is missing from the list:\n{frame}"
        );
    }
}

#[test]
fn every_row_starts_its_title_in_the_same_column() {
    // The snapshots cannot check this: they mask the id *cell* to a fixed width, so a row whose
    // id was padded wrong would come back looking straight. Measured on the real frame instead.
    // Titles of different lengths and ids of different lengths are the case that would break it —
    // abbreviations grow independently, so a burst of notes gives rows of unequal id widths.
    let (_tmp, mut app) = vault(&["a", "a much longer title", "another"]);
    let frame = render_raw_at(&mut app, WIDE.0, WIDE.1);

    let columns: Vec<usize> = app
        .rows()
        .iter()
        .map(|row| {
            let short = app.short_id(row.note.id);
            // List rows only. The reader's title bar carries the focused note's id too, and it
            // is not part of this column.
            let line = frame
                .lines()
                .find(|l| l.starts_with('\u{2502}') && l.contains(short.as_str()))
                .unwrap_or_else(|| panic!("`{short}` is not on screen:\n{frame}"));
            let at = line.find(short.as_str()).unwrap();
            let rest = &line[at + short.len()..];
            at + short.len() + (rest.len() - rest.trim_start_matches(' ').len())
        })
        .collect();

    assert!(
        columns.windows(2).all(|w| w[0] == w[1]),
        "titles start at {columns:?}, so the id column is not padded to one width:\n{frame}"
    );
}

#[test]
fn the_timeline_opens_on_every_note_rather_than_thread_roots() {
    let (_tmp, mut app) = vault(&["a root"]);
    app.dispatch(Action::Reply);
    // No composer here; the point is the header and the flag, not the reply.
    let _ = app.take_pending();

    let frame = render(&mut app);
    assert!(
        frame.contains("every note"),
        "roots-only was flood control for a vault with more than one author:\n{frame}"
    );
}

#[test]
fn the_marker_distinguishes_a_reply_from_a_note_on_its_own() {
    let (_tmp, mut app) = threaded();
    let frame = render_at(&mut app, WIDE.0, WIDE.1);

    assert!(
        frame.contains('\u{21b3}'),
        "a reply must not look like a standalone note — the whole reason flat was hiding:\n{frame}"
    );
    assert!(
        frame.contains('\u{2691}'),
        "and the head of the thread must say so:\n{frame}"
    );
    assert_frame!(frame);
}

#[test]
fn a_quote_fills_the_second_slot_without_moving_the_first() {
    let (_tmp, mut app) = threaded();
    let frame = render_at(&mut app, WIDE.0, WIDE.1);

    // Every row's id starts in the same column whether it carries zero, one or two glyphs. That
    // is the whole reason both slots are reserved rather than packed.
    // List rows only. The reader's title bar carries a masked *full* id, which starts with the
    // same characters and would otherwise be read as a fourth column position.
    let columns: Vec<usize> = frame
        .lines()
        .filter(|line| line.starts_with('\u{2502}') && line.contains(MASK.trim_end()))
        .map(|line| line.find(MASK.trim_end()).unwrap())
        .collect();
    assert_eq!(columns.len(), 3, "three rows expected:\n{frame}");
    assert!(
        columns.windows(2).all(|w| w[0] == w[1]),
        "ids start at {columns:?}, so a glyph is pushing its row sideways:\n{frame}"
    );
    assert!(
        frame.contains('\u{276f}'),
        "the quoting note must show it quotes:\n{frame}"
    );
    assert!(
        frame.contains('\u{275e}'),
        "and the quoted note must show something points at it:\n{frame}"
    );
}

/// A vault holding one thread and one note that quotes its root, synced and loaded.
///
/// Built through the workspace rather than by hand because the marker is a function of `Row`, and
/// `Row`'s relation fields are exactly what the index computes.
fn threaded() -> (TempDir, App) {
    use jot_core::query::Draft;

    let tmp = tempfile::tempdir().unwrap();
    let mut ws = Workspace::init(tmp.path()).unwrap();
    let root = ws.create(Draft::new("body").title("the head")).unwrap();
    let root = root.meta().id;
    ws.create(Draft::new("body").title("a reply").reply_to(root))
        .unwrap();
    ws.create(Draft::new("body").title("a quoter").quote(root))
        .unwrap();
    ws.sync().unwrap();
    (tmp, App::new(ws))
}
