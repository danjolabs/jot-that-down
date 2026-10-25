//! Snapshot tests over the rendered ratatui buffer.
//!
//! `stage5.md` asks for these so layout changes are deliberate. They render into a `TestBackend`
//! at a fixed size, which makes a frame a pure function of the app state and a fixed clock — no
//! pty, no terminal, no timing.
//!
//! The clock is pinned rather than `Utc::now()` on purpose: the `edited` column would otherwise
//! make every snapshot fail a second after it was taken.

use chrono::{DateTime, Datelike, TimeZone, Utc};
use jot_core::query::Draft;
use jot_core::workspace::Workspace;
use jot_tui::app::App;
use jot_tui::key::Action;
use jot_tui::ui;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;
use tempfile::TempDir;
use unicode_width::UnicodeWidthStr;

/// The size every snapshot renders at. Wide enough for a title and its age, short enough
/// that a snapshot stays readable in a diff.
const SIZE: (u16, u16) = (72, 12);

/// A fixed "now", so the relative `edited` column is stable.
///
/// **In 2025, a year before any note these tests create.** The `created` column prints `Oct 25`
/// for a note from the clock's own year and `Oct 2026` for any other, and the two are different
/// widths. With the clock in the same year as the notes, every snapshot would change shape on the
/// first of January, when the notes' year moves past the clock's. A clock in a year the notes can
/// never be from keeps every date in the one eight-column form, which [`mask_dates`] then pins.
fn clock() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2025, 9, 4, 12, 0, 0).unwrap()
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

/// A width wide enough for all three panes.
///
/// 30 columns of sidebar, 40 of list — `LIST_MIN` — and the reader in what is left. It was 110
/// while the frame carried two panes and 112 while the sidebar was 22 columns; the sidebar grew to
/// 30 (2026-10-25) and the threshold moved with it, by the rule the layout has always had.
const WIDE: (u16, u16) = (120, 12);

/// A frame wide enough that the table keeps its counts column beside the other two panes.
///
/// At [`WIDE`] the list is squeezed to `LIST_MIN` and the counts column is dropped, then the time
/// the view is not sorted by — which is the table's own rule working exactly as written, and
/// therefore the wrong frame to look for the counts in. 170 is where the list reaches `LIST_MAX`,
/// and a quoted thread's counts beside both times needs nearly all of it.
const ROOMY: (u16, u16) = (170, 12);

/// The calendar's weekday header as the sidebar paints it: each day centred in a four-column cell.
/// What the pane-layout tests look for to ask "is the sidebar on screen".
const WEEKDAYS: &str = "Mo  Tu  We  Th  Fr  Sa  Su";

/// A frame that carries the sidebar and the list and no reader — the default terminal, and the
/// middle band of the drop order.
const EIGHTY: (u16, u16) = (80, 16);

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
    render_raw_with_clock(app, width, height, clock())
}

/// [`render_raw_at`], at a clock of the caller's choosing.
///
/// The calendar is the only thing on screen that cares *which day* it is rather than how long ago
/// something was, so it is the only thing that needs a clock other than [`clock`]: a note created
/// while the suite runs lands on the real current date, and the grid has to be looking at that
/// month for the dot to be on screen at all.
fn render_raw_with_clock(app: &mut App, width: u16, height: u16, now: DateTime<Utc>) -> String {
    app.prepare_preview(ui::reader_text_width_for(app.panes(), width, height));
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| ui::draw(frame, app, now)).unwrap();

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
            out = mask_header(&out);
            // An overlay can land on top of a row and leave only the front of an id showing. That
            // stub is still random, so it still has to go — masked to its own width, which needs
            // no compensating fill.
            for id in &ids {
                for len in (4..id.len()).rev() {
                    out = out.replace(&id[..len], &stub(len));
                }
            }
            out = mask_ages(&out);
            out = mask_dates(&out);
            out = mask_dots(&out);
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
/// The ages are the other thing in a frame that nothing can pin. A note's `edited` time is the
/// file's mtime, written at test time while [`clock`] is fixed, so the *text* moves with the hour —
/// and, worse, so does its *width*: `4h` is two columns and `ahead` is five. That is a bomb on a
/// timer, and it went off. These snapshots were recorded in the morning and began failing at noon,
/// when real time passed the fixed clock and every age flipped from `4h` to `ahead`, shifting three
/// columns of padding on every row of every frame.
///
/// Substituting a fixed-width token defuses it. The grammar is the whole of `ui::relative`'s
/// output — `now`, `ahead`, or a count and a unit — matched on whole space-separated tokens, so a
/// title that happens to contain one of those words is left alone.
fn mask_ages(line: &str) -> String {
    // Split on the panel borders as well as on spaces. A row that fills its pane exactly puts the
    // age flush against the border, and where the list abuts the reader the token is `ahead\u{2502}\u{2502}#`
    // — which no age grammar matches, so three snapshots kept a live `ahead` the moment the
    // sidebar made the list narrower. A border is a separator here for the same reason a space is.
    line.split(' ')
        .map(|token| {
            token
                .split('\u{2502}')
                .map(|part| {
                    if is_age(part) {
                        AGE_MASK.to_string()
                    } else {
                        part.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\u{2502}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Replace every `created` date in a line with [`DATE_MASK`].
///
/// A note's `created` date is the day the suite ran, so it moves every day. Under [`clock`] it is
/// always the `Oct 2026` form — eight columns, the width of the mask — so substituting changes the
/// text and never the layout. The calendar's own title is the same shape and is left alone: it is
/// the clock's month, which is pinned, and it opens a border (`┌ Sep 2025`).
fn mask_dates(line: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut out = line.to_string();
    for month in MONTHS {
        let mut from = 0;
        while let Some(found) = out[from..].find(&format!("{month} ")) {
            let at = from + found;
            let year = out.get(at + 4..at + 8).unwrap_or("");
            let in_title = out[..at].ends_with("\u{250c} ");
            if year.len() == 4 && year.bytes().all(|b| b.is_ascii_digit()) && !in_title {
                out.replace_range(at..at + 8, DATE_MASK);
            }
            from = at + 4;
        }
    }
    out
}

/// What a `created` date is masked to: the eight columns of the `Oct 2026` form.
const DATE_MASK: &str = "MMM YYYY";

/// Blank out the calendar's dots, which are on a timer of their own.
///
/// A note created while the suite runs is created *today*, and the calendar draws the month the
/// frame's clock is in — so a snapshot taken in September and holding a dot on the 9th starts
/// failing on the 1st of October, and moves a column every day until then. Same bomb as the ages,
/// with a longer fuse.
///
/// Blanking rather than substituting, because the dot's information is entirely its *position*
/// and there is no fixed-width token that could stand in for a column number. What the dots do is
/// checked where it can be checked deterministically: `the_calendar_dots_the_day_a_note_was_created`
/// renders at a clock derived from the note itself, and `ui`'s own tests build a day set by hand.
fn mask_dots(line: &str) -> String {
    line.replace(DOT, " ")
}

/// The calendar's dot, as `ui` paints it.
const DOT: &str = "\u{2219}";

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

/// Give the table header's `id` cell the width [`MASK`] gives the ids under it.
///
/// The header's `id` is padded to the id column, and the id column is as wide as the widest id on
/// screen — thirteen characters on one run, fourteen on the next when two notes share a
/// millisecond. Masking the ids and not their header would leave the header's padding as the one
/// random width in an otherwise fixed frame. Recognised as `id` followed by blanks and the start of
/// the title label, which no row and no note body produces.
fn mask_header(line: &str) -> String {
    let Some(at) = line.find(" id ") else {
        return line.to_string();
    };
    let rest = &line[at + " id".len()..];
    let label = rest.trim_start_matches(' ');
    if !label.starts_with("ti") {
        return line.to_string();
    }
    format!(
        "{} id{}{label}",
        &line[..at],
        " ".repeat(MASK.len() - "id".len())
    )
}

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
    app.dispatch(Action::ToggleReader);
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
    app.dispatch(Action::ToggleReader);
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
    app.dispatch(Action::ToggleReader);

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
    app.dispatch(Action::ToggleReader);
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
    app.dispatch(Action::ToggleReader);
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
    app.dispatch(Action::ToggleReader);
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
            short.len() >= 13,
            "`{short}` is shorter than the whole millisecond timestamp, which is where both this \
             surface and `jot ls` floor — anything less and the two disagree about a note"
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
            // Columns, not bytes, up to the id: the selected row's `◉` is three bytes wide in a
            // string and one column on screen.
            line[..at].width() + short.len() + (rest.len() - rest.trim_start_matches(' ').len())
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
    // Newest first, so the selection opens on the reply, and the quoter sits between it and the
    // parent it is linked to. The lane runs down from the selection to its parent and nowhere
    // else; the relation slot says which row is a reply, which a head, and which neither.
    let (_tmp, mut app) = threaded();
    let frame = render_at(&mut app, WIDE.0, WIDE.1);

    assert_eq!(
        [
            marker_of(&frame, "a reply"),
            marker_of(&frame, "a quoter"),
            marker_of(&frame, "the head"),
        ],
        ["╭◉↳  ", "│  ❯ ", "╰►⚑  "],
        "a reply must not look like a standalone note — the whole reason flat was hiding — and \
         the lane must join the selection to its parent through the row between:\n{frame}"
    );
    assert_frame!(frame);
}

#[test]
fn a_parent_above_the_selection_turns_the_lane_upside_down() {
    // Oldest first puts the parent at the top and the reply at the bottom. The corners point at
    // each other whichever way round the two rows are.
    let (_tmp, mut app) = threaded();
    app.dispatch(Action::NextView); // files
    app.dispatch(Action::CycleSort); // oldest
    app.dispatch(Action::Bottom);
    let frame = render_at(&mut app, WIDE.0, WIDE.1);

    assert_eq!(
        [
            marker_of(&frame, "the head"),
            marker_of(&frame, "a quoter"),
            marker_of(&frame, "a reply"),
        ],
        ["╭►⚑  ", "│  ❯ ", "╰◉↳  "],
        "{frame}"
    );
}

#[test]
fn a_narrow_table_keeps_the_time_its_view_is_sorted_by() {
    // Too narrow for both times beside the title: the files view keeps `created` while it is
    // sorted by creation, and `edited` once it is sorted by edit time — the column the order is
    // about is the one that stays.
    let (_tmp, mut app) = vault(&["b note", "a note"]);
    let header = |app: &mut App| {
        let frame = render_raw_at(app, 40, 8);
        frame
            .lines()
            .find(|line| line.contains("title"))
            .unwrap_or_else(|| panic!("no header row:\n{frame}"))
            .to_string()
    };

    app.dispatch(Action::NextView); // files, newest first
    let by_created = header(&mut app);
    assert!(
        by_created.contains("created") && !by_created.contains("edited"),
        "{by_created}"
    );

    app.dispatch(Action::CycleSort); // oldest
    app.dispatch(Action::CycleSort); // edited
    let by_edited = header(&mut app);
    assert!(
        by_edited.contains("edited") && !by_edited.contains("created"),
        "{by_edited}"
    );
}

#[test]
fn the_table_keeps_the_selection_on_screen_below_its_header() {
    // Twenty notes in a pane of eight rows once the header has taken one. Walking to the oldest
    // must scroll the table, keep the header where it is, and put the selected row on screen.
    let titles: Vec<String> = (0..20).map(|n| format!("note {n:02}")).collect();
    let (_tmp, mut app) = vault(&titles.iter().map(String::as_str).collect::<Vec<_>>());

    let list_rows = |frame: &str| -> Vec<String> {
        frame
            .lines()
            .skip(1)
            .take(SIZE.1 as usize - 3)
            .map(|line| line.split('\u{2502}').nth(3).unwrap_or("").to_string())
            .collect()
    };

    for step in 0..20 {
        let rows = list_rows(&render(&mut app));
        let want = format!("note {:02}", 19 - step);
        assert!(
            rows[0].contains("title"),
            "at step {step} the header scrolled away: {rows:?}"
        );
        assert!(
            rows[1..].iter().any(|row| row.contains(&want)),
            "at step {step} the selected `{want}` is off screen: {rows:?}"
        );
        app.dispatch(Action::MoveDown);
    }
}

#[test]
fn one_hundred_and_twenty_by_thirty_carries_every_pane() {
    // The smallest frame that carries all of it: the calendar with the thread graph under it, the
    // table, and the reader.
    let (_tmp, mut app) = forked();
    app.dispatch(Action::Bottom);
    app.dispatch(Action::ToggleReader);
    let frame = render_at(&mut app, 120, 30);
    assert!(frame.contains(WEEKDAYS), "{frame}");
    assert!(frame.contains(" thread "), "{frame}");
    assert!(frame.contains("the body of the root"), "{frame}");
    assert_frame!(frame);
}

#[test]
fn a_quote_fills_the_second_slot_without_moving_the_first() {
    let (_tmp, mut app) = threaded();
    let frame = render_at(&mut app, ROOMY.0, ROOMY.1);

    // Every row's id starts in the same column whether it carries zero, one or two glyphs. That
    // is the whole reason both slots are reserved rather than packed.
    // List rows only. The reader's title bar carries a masked *full* id, which starts with the
    // same characters and would otherwise be read as a fourth column position.
    let columns: Vec<usize> = frame
        .lines()
        .filter(|line| line.starts_with('\u{2502}') && line.contains(MASK.trim_end()))
        .map(|line| line[..line.find(MASK.trim_end()).unwrap()].width())
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

#[test]
fn eighty_columns_carries_the_sidebar_and_the_list_and_no_reader() {
    // The terminal that matters, and the middle band of the drop order: 30 columns of sidebar,
    // 50 of list, and the reader dropped by its own rule rather than squeezed in beside them.
    let (_tmp, mut app) = bodies(&[
        ("older", "an earlier thought"),
        ("the note in focus", "the body of the focused note"),
    ]);
    // Wanted, so what drops it is the width rule and not the default.
    app.dispatch(Action::ToggleReader);
    let frame = render_at(&mut app, EIGHTY.0, EIGHTY.1);

    assert!(
        !frame.contains("the body of the focused note"),
        "80 columns cannot carry a calendar and two bordered panels:\n{frame}"
    );
    assert!(
        frame.contains(WEEKDAYS),
        "but it carries the calendar, with 10 columns to spare:\n{frame}"
    );
    assert_eq!(
        ui::reader_text_width(EIGHTY.0, EIGHTY.1),
        None,
        "and the run loop must be told, or it renders a panel into nothing"
    );
    assert_frame!(frame);
}

#[test]
fn a_hundred_and_twenty_columns_carries_all_three_panes() {
    let (_tmp, mut app) = bodies(&[
        ("older", "an earlier thought"),
        ("the note in focus", "the body of the focused note"),
    ]);
    app.dispatch(Action::ToggleReader);
    let frame = render_at(&mut app, WIDE.0, 16);

    assert!(
        frame.contains(WEEKDAYS),
        "the sidebar is the left pane:\n{frame}"
    );
    assert!(
        frame.contains("the body of the focused note"),
        "and the reader still gets the right of the main pane:\n{frame}"
    );
    assert_frame!(frame);
}

#[test]
fn the_panes_drop_in_the_settled_order_as_the_frame_narrows() {
    // Reader first, then sidebar, then the list alone — read off the painted frame rather than
    // off the geometry, because what is on screen is the thing being promised.
    let (_tmp, mut app) = bodies(&[("a note", "a body to look for")]);
    app.dispatch(Action::ToggleReader);

    let calendar = |frame: &str| frame.contains(WEEKDAYS);
    let reader = |frame: &str| frame.contains("a body to look for");

    let three = render_at(&mut app, 120, 16);
    assert!(calendar(&three) && reader(&three));

    let two = render_at(&mut app, 119, 16);
    assert!(calendar(&two), "the sidebar stays:\n{two}");
    assert!(!reader(&two), "the reader is what goes first:\n{two}");

    let one = render_at(&mut app, 69, 16);
    assert!(!calendar(&one), "and the sidebar goes next:\n{one}");
    assert!(!reader(&one));
    assert!(
        one.contains("a note"),
        "the list is never dropped — it is the surface:\n{one}"
    );
}

#[test]
fn hiding_the_reader_gives_the_whole_main_pane_to_the_list() {
    let (_tmp, mut app) = bodies(&[("a note", "a body to look for")]);
    // It starts closed (user decision, 2026-10-26); open it so there is something to hide.
    app.dispatch(Action::ToggleReader);
    let with = render_at(&mut app, WIDE.0, 16);
    assert!(with.contains("a body to look for"));

    app.dispatch(Action::ToggleReader);
    let without = render_at(&mut app, WIDE.0, 16);
    assert!(
        !without.contains("a body to look for"),
        "the toggle takes the panel away:\n{without}"
    );
    assert!(
        without.contains(WEEKDAYS),
        "and takes nothing else with it:\n{without}"
    );

    // Same key back. A toggle that cannot be undone is a setting.
    app.dispatch(Action::ToggleReader);
    assert_eq!(render_at(&mut app, WIDE.0, 16), with);
}

#[test]
fn hiding_the_sidebar_is_the_only_way_a_toggle_adds_anything() {
    // 100 columns is under the 120 three panes need and over the 90 two need, so the reader is
    // reachable there only by spending the sidebar's 30 columns on it. That is the toggle
    // working in the one direction it may: by removal.
    let (_tmp, mut app) = bodies(&[("a note", "a body to look for")]);
    app.dispatch(Action::ToggleReader);
    let with = render_at(&mut app, 100, 16);
    assert!(with.contains(WEEKDAYS));
    assert!(!with.contains("a body to look for"));

    app.dispatch(Action::ToggleSidebar);
    let without = render_at(&mut app, 100, 16);
    assert!(!without.contains(WEEKDAYS), "the calendar goes:\n{without}");
    assert!(
        without.contains("a body to look for"),
        "and the reader fits in what it was using:\n{without}"
    );
}

#[test]
fn a_toggle_cannot_force_a_pane_into_a_frame_too_narrow_for_it() {
    // 60 columns carries a list and nothing else. Asking for the panes back at that width must
    // get nothing: the automatic rule wins, and a two-column reader painted over the list would
    // be the toggle overriding it in the direction it may not.
    let (_tmp, mut app) = bodies(&[("a note", "a body to look for")]);
    // Both wanted to begin with, so the loop below turns both off and then asks for both back.
    app.dispatch(Action::ToggleReader);
    let alone = render_at(&mut app, 60, 16);

    for _ in 0..2 {
        app.dispatch(Action::ToggleReader);
        app.dispatch(Action::ToggleSidebar);
        assert_eq!(
            render_at(&mut app, 60, 16),
            alone,
            "60 columns is a list, whatever the toggles say"
        );
    }
}

#[test]
fn the_calendar_dots_the_day_a_note_was_created() {
    let (_tmp, mut app) = vault(&["a note"]);

    // Asked of the app rather than of the system clock, so the test bucket and the sidebar's
    // bucket are the same one; then the frame is rendered at noon on that day, which is the only
    // clock at which the grid is showing the month the note is in.
    let day = *app
        .days_with_notes()
        .iter()
        .next()
        .expect("a vault with one note has one day with a note");
    let noon = day.and_hms_opt(12, 0, 0).unwrap().and_utc();
    let frame = render_raw_with_clock(&mut app, WIDE.0, 20, noon);

    // The dot sits on its own row under the day it belongs to, so the row after the one carrying
    // the day's number is that day's dot row.
    let rows = sidebar_rows(&frame);
    let cell = format!("{:>2}", day.day());
    let (row, at) = rows
        .iter()
        .enumerate()
        .find_map(|(i, r)| r.iter().collect::<String>().find(&cell).map(|at| (i, at)))
        .unwrap_or_else(|| panic!("the {cell} is not on the calendar at all:\n{frame}"));

    assert_eq!(
        rows[row + 1].get(at + 1),
        Some(&'\u{2219}'),
        "the dot must sit under the day it belongs to:\n{frame}"
    );
    assert_eq!(
        rows.iter().flatten().filter(|c| **c == '\u{2219}').count(),
        1,
        "one day with notes, one dot — a second would be a heat map:\n{frame}"
    );
}

#[test]
fn the_calendar_marks_today_without_spending_a_column_on_it() {
    let (_tmp, mut app) = vault(&["a note"]);
    app.prepare_preview(ui::reader_text_width_for(app.panes(), WIDE.0, 20));

    let backend = TestBackend::new(WIDE.0, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| ui::draw(frame, &app, clock()))
        .unwrap();
    let buffer = terminal.backend().buffer();

    // `clock()` is the 4th of September 2025 and the app's zone is UTC, so today is the 4th: one
    // reversed number, two columns wide, and nothing else in the sidebar reversed.
    let sidebar = SIDEBAR_INNER as u16 + 2;
    let marked: String = (0..20)
        .flat_map(|y| (0..sidebar).map(move |x| (x, y)))
        .filter(|at| {
            buffer[*at]
                .style()
                .add_modifier
                .contains(Modifier::REVERSED)
        })
        .map(|at| buffer[at].symbol().to_string())
        .collect();

    assert_eq!(
        marked.replace(' ', ""),
        "4",
        "today is reverse video and nothing else is: {marked:?}"
    );
}

/// The list's marker column on the row titled `title`: its five columns, as painted.
///
/// Read by column, past the sidebar and the list's own border, because the selection lane's `│`
/// is the same character as a panel border and splitting on it would cut the marker in half.
fn marker_of(frame: &str, title: &str) -> String {
    frame
        .lines()
        .map(|line| line.chars().skip(SIDEBAR_INNER + 3).collect::<String>())
        .find(|list| list.contains(title))
        .unwrap_or_else(|| panic!("`{title}` is not in the list:\n{frame}"))
        .chars()
        .take(5)
        .collect()
}

/// The sidebar's inner columns, one `Vec<char>` per row of the frame.
///
/// Indexed by *column* rather than by byte, because the frame's borders are three bytes each and
/// the whole point of these assertions is which column a glyph landed in.
fn sidebar_rows(frame: &str) -> Vec<Vec<char>> {
    frame
        .lines()
        .map(|line| line.chars().skip(1).take(SIDEBAR_INNER).collect())
        .collect()
}

/// A vault holding one thread and one note that quotes its root, synced and loaded.
///
/// Built through the workspace rather than by hand because the marker is a function of `Row`, and
/// `Row`'s relation fields are exactly what the index computes. The quoter is written between the
/// head and its reply, so in either creation order it is the row between a reply and its parent —
/// which is what puts a `│` in the selection lane.
fn threaded() -> (TempDir, App) {
    use jot_core::query::Draft;

    let tmp = tempfile::tempdir().unwrap();
    let mut ws = Workspace::init(tmp.path()).unwrap();
    let root = ws.create(Draft::new("body").title("the head")).unwrap();
    let root = root.meta().id;
    ws.create(Draft::new("body").title("a quoter").quote(root))
        .unwrap();
    ws.create(Draft::new("body").title("a reply").reply_to(root))
        .unwrap();
    ws.sync().unwrap();
    (tmp, App::new(ws))
}

/// A vault holding one forking thread, synced and loaded. The graph needs nothing pressed: it is
/// always under the calendar.
///
/// The shape is the one `stage6.md`'s acceptance criterion names — "a thread with a fork" — and
/// it is deliberately the smallest tree with every feature the sweep has: a trunk, a fork, and a
/// reply arriving *after* the fork so the two branches interleave chronologically.
fn forked() -> (TempDir, App) {
    let tmp = tempfile::tempdir().unwrap();
    let mut ws = Workspace::init(tmp.path()).unwrap();
    let root = ws
        .create(Draft::new("the body of the root").title("the question"))
        .unwrap()
        .meta()
        .id;
    let first = ws
        .create(Draft::new("b").title("first answer").reply_to(root))
        .unwrap()
        .meta()
        .id;
    ws.create(Draft::new("b").title("second answer").reply_to(root))
        .unwrap();
    ws.create(Draft::new("b").title("a follow up").reply_to(first))
        .unwrap();
    ws.sync().unwrap();

    (tmp, App::new(ws))
}

/// The sidebar's inner rows as text, one string per *content* row.
///
/// Filtered to lines that open with the frame's left border, which drops the horizontal rules
/// between the panels and the status line below them — neither is sidebar content, and both would
/// otherwise turn up in an assertion about what the calendar or the graph drew.
fn sidebar_text(frame: &str) -> Vec<String> {
    frame
        .lines()
        .filter(|line| line.starts_with('\u{2502}'))
        .map(|line| {
            line.chars()
                .skip(1)
                .take(SIDEBAR_INNER)
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

/// Columns inside the sidebar's borders. `ui::SIDEBAR_WIDTH` less the two it spends on them.
const SIDEBAR_INNER: usize = 28;

/// Tall enough that the calendar's 15 rows leave the graph a pane worth drawing into.
const TALL: (u16, u16) = (112, 24);

#[test]
fn the_thread_graph_is_always_under_the_calendar() {
    // No key for it (user decision, 2026-10-25): the first frame has it, with nothing pressed, and
    // it costs the calendar nothing.
    let (_tmp, mut app) = forked();
    let first = render_at(&mut app, TALL.0, TALL.1);
    assert!(
        first.contains(" thread "),
        "the graph is there from the first frame:\n{first}"
    );
    assert!(
        first.contains(WEEKDAYS),
        "without costing the calendar a row:\n{first}"
    );
}

#[test]
fn a_forking_thread_is_one_list_with_a_lane_gutter_and_the_root_at_the_top() {
    let (_tmp, mut app) = forked();
    // The timeline is newest first, so the root is the last row. Focusing it is what makes the
    // `◉` land at the top of the graph, which is the reading this test is about.
    app.dispatch(Action::Bottom);
    let frame = render_at(&mut app, TALL.0, TALL.1);

    let graph: Vec<String> = sidebar_text(&frame)
        .into_iter()
        .skip_while(|row| !row.contains("question"))
        // The pane is taller than the thread, so the rows after it are blank.
        .take_while(|row| !row.is_empty())
        .collect();
    assert_eq!(
        graph,
        [
            // Root at the top, growing downward: `undotree` is root-at-bottom and a conversation
            // is not. `◉` is the focused note and `●` every other one. The gutter is two lanes,
            // each a glyph and a spacer, because the thread forks once — a linear thread would be
            // one — and `a follow up` is drawn after `second answer` because the sweep is
            // chronological, which is also why it sits in the trunk's lane rather than under its
            // own parent's row.
            "◉    the question",
            "├─╮",
            "● │  first answer",
            "│ ●  second answer",
            "●    a follow up",
        ],
        "the graph is not the shape it should be:\n{frame}"
    );
    assert_frame!(frame);
}

#[test]
fn the_gutter_carries_nothing_wider_than_one_column() {
    // Read off a painted frame rather than off the glyph table. This used to demand the same width
    // under a CJK locale, which kept the gutter ASCII; the user chose box-drawing lines on
    // 2026-10-25 instead, because the frame's own border `┌─│┐` is the same East Asian Ambiguous
    // set, so a terminal that widens Ambiguous has already broken the panel the gutter sits in.
    // What is left to catch is a wide character or an emoji in a fixed-width gutter.
    let (_tmp, mut app) = forked();
    let frame = render_raw_at(&mut app, TALL.0, TALL.1);

    for row in sidebar_text(&frame) {
        for c in row.chars() {
            assert_eq!(
                c.to_string().width(),
                1,
                "`{c}` (U+{:04X}) is not one column wide",
                c as u32
            );
        }
    }
}

#[test]
fn the_graph_follows_the_cursor_and_marks_where_you_are() {
    let (_tmp, mut app) = forked();
    // Newest first, so the surface opens on the leaf and `G` walks back to the root.
    let at_leaf = sidebar_text(&render_at(&mut app, TALL.0, TALL.1));
    assert!(
        at_leaf.contains(&"◉    a follow up".to_string()),
        "the mark starts where the cursor is: {at_leaf:?}"
    );
    assert!(
        at_leaf.contains(&"●    the question".to_string()),
        "and the thread is still drawn from its root — `thread(focus)` alone would have hidden \
         everything above the leaf: {at_leaf:?}"
    );

    app.dispatch(Action::Bottom);
    let at_root = sidebar_text(&render_at(&mut app, TALL.0, TALL.1));
    assert!(
        at_root.contains(&"◉    the question".to_string()),
        "the mark must move with the cursor: {at_root:?}"
    );
    assert!(
        at_root.contains(&"●    a follow up".to_string()),
        "and nothing else may wear it: {at_root:?}"
    );
}

#[test]
fn a_workspace_with_no_replies_gets_a_note_and_a_lane_rather_than_an_empty_panel() {
    // Thread-agnostic: a schema declaring no `relation:*` entry gives every note a thread of one.
    // A single row that says "you are here" is a fact; a bordered box with nothing in it is an
    // affordance that can never fire.
    let (_tmp, mut app) = vault(&["a note on its own"]);
    let sidebar = sidebar_text(&render_at(&mut app, TALL.0, TALL.1));

    assert!(
        sidebar.contains(&"◉  a note on its own".to_string()),
        "one lane, one node, one title: {sidebar:?}"
    );
}

#[test]
fn a_thread_taller_than_the_pane_keeps_the_focus_and_says_what_it_hid() {
    // Twelve replies in a line, against a graph pane six rows tall. Whichever note is focused, it
    // is on screen — and the rows that are not are counted rather than silently missing.
    let tmp = tempfile::tempdir().unwrap();
    let mut ws = Workspace::init(tmp.path()).unwrap();
    let mut parent = ws
        .create(Draft::new("b").title("note 00"))
        .unwrap()
        .meta()
        .id;
    for n in 1..12 {
        parent = ws
            .create(
                Draft::new("b")
                    .title(format!("note {n:02}"))
                    .reply_to(parent),
            )
            .unwrap()
            .meta()
            .id;
    }
    ws.sync().unwrap();
    let mut app = App::new(ws);

    // Newest first, so the top row is the deepest note and `G` walks back to the root.
    for step in 0..12 {
        let sidebar = sidebar_text(&render_at(&mut app, TALL.0, TALL.1));
        let want = format!("◉  note {:02}", 11 - step);
        assert!(
            sidebar.iter().any(|row| row.starts_with(&want)),
            "at step {step} the focus `{want}` is not on screen: {sidebar:?}"
        );
        app.dispatch(Action::MoveDown);
    }

    // From the middle, both markers are up and both count the row they displaced.
    app.dispatch(Action::Top);
    for _ in 0..6 {
        app.dispatch(Action::MoveDown);
    }
    let sidebar = sidebar_text(&render_at(&mut app, TALL.0, TALL.1));
    assert!(
        sidebar
            .iter()
            .any(|row| row.trim_start().starts_with('+') && row.ends_with("above")),
        "a window that does not say it is a window: {sidebar:?}"
    );
    assert!(
        sidebar
            .iter()
            .any(|row| row.trim_start().starts_with('+') && row.ends_with("below")),
        "{sidebar:?}"
    );
}

#[test]
fn a_thread_wider_than_the_gutter_folds_its_lanes_and_keeps_every_title() {
    // Eight replies to one note is eight simultaneous lanes, against a cap of six. What has to
    // survive is the *list*: every reply still gets a row and a legible title, and only the art
    // degrades — because the art is the part you can do without and the titles are not.
    let tmp = tempfile::tempdir().unwrap();
    let mut ws = Workspace::init(tmp.path()).unwrap();
    let root = ws
        .create(Draft::new("b").title("the root"))
        .unwrap()
        .meta()
        .id;
    for n in 0..8 {
        ws.create(Draft::new("b").title(format!("reply {n}")).reply_to(root))
            .unwrap();
    }
    ws.sync().unwrap();
    let mut app = App::new(ws);
    app.dispatch(Action::Bottom);

    // Nine notes and a connector is ten rows, against six of pane, so this also exercises the
    // window: the root is focused and therefore pinned on screen.
    let sidebar = sidebar_text(&render_at(&mut app, TALL.0, 34));
    let graph: Vec<String> = sidebar
        .into_iter()
        .skip_while(|row| !row.contains("the root"))
        .take_while(|row| !row.is_empty())
        .collect();

    for n in 0..8 {
        let want = format!("reply {n}");
        assert!(
            graph.iter().any(|row| row.ends_with(&want)),
            "`{want}` lost its row to the fold: {graph:?}"
        );
    }
    assert!(
        graph.iter().any(|row| row.contains('┊')),
        "the fold must say there are lanes it is not drawing: {graph:?}"
    );
    // Six lanes of two columns, then the gap: no title may start further right than that. A
    // gutter has spaces in it now, so the title is found by where it starts rather than by where
    // the gutter's first blank is.
    for row in &graph {
        if let Some(title_at) = row.chars().position(char::is_alphanumeric) {
            assert!(
                title_at <= 6 * 2 + 1,
                "`{row}` draws a gutter wider than the cap"
            );
        }
    }
}
