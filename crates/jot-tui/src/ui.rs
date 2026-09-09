//! Rendering. Pure: every function here takes state and produces cells, and touches nothing else.
//!
//! Keeping the draw path free of side effects is what makes the snapshot tests worth having — a
//! rendered frame is a function of [`App`], so a diff in the snapshot is a real visual change and
//! never a timing artefact.

use std::collections::BTreeSet;

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use jot_core::query::{Ref, Row, State};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, GraphRow, Lane, Panes, ViewKind, sort_name};
use crate::key::{Keymap, Mode, PREFIX_LABEL, Scope};

/// Paint the whole frame.
pub fn draw(frame: &mut Frame, app: &App, now: DateTime<Utc>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    // The sidebar comes off the frame first, and what is left is the main pane `split_main` has
    // always been handed. That nesting is the whole of the three-pane layout: the reader splits
    // the *main pane* rather than the frame, so `READER_MIN_FRAME` keeps its meaning exactly and
    // simply applies to a smaller area.
    let areas = split_frame(chunks[0], app.panes());
    if let Some(sidebar) = areas.sidebar {
        draw_sidebar(frame, sidebar, app, now);
    }
    draw_list(frame, areas.list, app, now);
    if let Some(reader) = areas.reader {
        draw_reader(frame, reader, app);
    }
    draw_status(frame, chunks[1], app);

    if app.help_is_open() {
        draw_help(frame, frame.area());
    }
}

/// Narrower than this and the frame carries the list alone.
///
/// Two bordered panels cost four columns of chrome before a single character of content, and a
/// list squeezed under [`LIST_MIN`] loses the title column that is the whole point of it. Below
/// this width the reader is the thing to drop, because the list still answers "what is in here"
/// and a two-column reader answers nothing.
const READER_MIN_FRAME: u16 = 90;

/// The narrowest the list may be squeezed to make room for the reader.
const LIST_MIN: u16 = 40;

/// The widest the list grows before the reader gets the rest.
///
/// A list is a column of short titles; past this it is mostly whitespace, and the reader is where
/// extra width actually buys something.
const LIST_MAX: u16 = 56;

/// Columns the sidebar occupies, its two border columns included.
///
/// Seven day cells of three columns each, less the separator the last cell does not need — 20 —
/// plus the borders. Fixed rather than a fraction of the frame because a calendar is a *grid*: at
/// 21 columns it is not a narrower month, it is a week with a day sliced off the end, and every
/// row below the header lands one column out from the row above it.
const SIDEBAR_WIDTH: u16 = 22;

/// Narrower than this and the frame carries no sidebar.
///
/// [`LIST_MIN`] one pane out: the 22 columns of calendar plus the 40 a list needs to still be a
/// list. Below this the sidebar is the thing to drop, for the same reason the reader is dropped
/// below [`READER_MIN_FRAME`] — the list is what the surface is *for*, and a month is context
/// beside it.
const SIDEBAR_MIN_FRAME: u16 = SIDEBAR_WIDTH + LIST_MIN;

/// Where each pane landed, with `None` for one this frame could not carry.
///
/// The list is not optional: it is the surface. Whatever else goes, it gets what is left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneAreas {
    /// The calendar column, when there is room for it.
    pub sidebar: Option<Rect>,
    /// The row list. Always painted.
    pub list: Rect,
    /// The reader, when the main pane is wide enough for two bordered panels.
    pub reader: Option<Rect>,
}

/// Divide the frame into sidebar, list and reader, in that order of subtraction.
///
/// # The drop order, and why it is expressed as nesting
///
/// **Reader first, then sidebar, then the list alone.** That order is not written out as a chain
/// of `if`s: it falls out of doing the arithmetic in one direction. The sidebar comes off the
/// frame, and the remainder is handed to [`split_main`] — which already drops the reader when it
/// is under `READER_MIN_FRAME`. So a frame narrow enough to squeeze the main pane loses the
/// reader before anything else is even considered, and only a frame too narrow for
/// `SIDEBAR_MIN_FRAME` loses the sidebar as well.
///
/// | frame | sidebar | list | reader |
/// | --- | --- | --- | --- |
/// | 112+ | 22 | 40–56 | the rest |
/// | 62–111 | 22 | the rest | — |
/// | under 62 | — | the whole frame | — |
///
/// (Hide the sidebar and the reader comes back at 90, because the main pane is the frame again.)
///
/// # A toggle may only ever remove a pane
///
/// `panes` says what the user *wants*, and this function is what decides. Wanting a pane the
/// width cannot carry gets nothing: pressing the reader's key at 60 columns must not paint a
/// two-column panel over the list. The automatic rule wins, and the toggles are an escape hatch in
/// the one direction — 22 columns of calendar is a quarter of an 80-column terminal, and there has
/// to be a way to spend it on the list instead.
#[must_use]
pub fn split_frame(area: Rect, panes: Panes) -> PaneAreas {
    let (sidebar, main) = if panes.sidebar && area.width >= SIDEBAR_MIN_FRAME {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(1)])
            .split(area);
        (Some(chunks[0]), chunks[1])
    } else {
        (None, area)
    };

    let (list, reader) = if panes.reader {
        split_main(main)
    } else {
        (main, None)
    };

    PaneAreas {
        sidebar,
        list,
        reader,
    }
}

/// Split the main region into the list and, when there is room, the reader beside it.
///
/// Public because the run loop needs the reader's width *before* the draw: rendering the panel
/// means spawning a highlighter, this module is pure, and the two must agree on the width or the
/// text would be wrapped for a panel it is not being painted into. See
/// [`App::prepare_preview`](crate::app::App::prepare_preview).
#[must_use]
pub fn split_main(area: Rect) -> (Rect, Option<Rect>) {
    if area.width < READER_MIN_FRAME {
        return (area, None);
    }
    let list = (area.width * 2 / 5).clamp(LIST_MIN, LIST_MAX);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(list), Constraint::Min(1)])
        .split(area);
    (chunks[0], Some(chunks[1]))
}

/// The column budget the reader's text has at this frame size, or `None` when there is no reader.
///
/// The run loop's half of the contract in [`split_main`]. `frame_height` costs the status line;
/// the panel's own borders cost two columns and two rows.
///
/// **Answers for the default pane set** — both side panes wanted — because the run loop holds a
/// terminal size and not an [`App`], so this is the question it is able to ask. That is the right
/// answer in the state the surface starts in and stays in until a toggle is pressed; anything
/// holding the app's own [`Panes`] should ask [`reader_text_width_for`] instead. Getting it wrong
/// costs a rewrap, not a broken frame: text wrapped for a narrower panel wraps early inside a
/// wider one.
#[must_use]
pub fn reader_text_width(frame_width: u16, frame_height: u16) -> Option<u16> {
    reader_text_width_for(Panes::default(), frame_width, frame_height)
}

/// [`reader_text_width`], for a caller that knows which panes are wanted.
///
/// The sidebar is 22 columns of the reader's budget, so the two questions have different answers
/// the moment it is hidden — and between 90 and 112 columns they differ about whether there is a
/// reader at all.
#[must_use]
pub fn reader_text_width_for(panes: Panes, frame_width: u16, frame_height: u16) -> Option<u16> {
    let area = Rect {
        x: 0,
        y: 0,
        width: frame_width,
        height: frame_height.saturating_sub(1),
    };
    split_frame(area, panes)
        .reader
        .map(|r| r.width.saturating_sub(2))
}

/// The reader panel: the focused note, styled by whatever [`crate::preview`] could borrow.
fn draw_reader(frame: &mut Frame, area: Rect, app: &App) {
    // The full UUID, not the abbreviation the list carries. The panel has the width for it, and
    // an abbreviation is a *reading* convenience for a column that has to stay narrow — here it
    // would only be a shorter thing to retype. Labelled from what is *rendered* rather than from
    // what is selected; see `App::preview_id`.
    let title = match app.preview_id() {
        Some(id) => format!(" {id} "),
        None => " reader ".to_string(),
    };
    let block = Block::default().borders(Borders::ALL).title(title);

    if app.focused().is_none() {
        let paragraph = Paragraph::new("\n  Nothing selected.")
            .block(block)
            .style(dim());
        frame.render_widget(paragraph, area);
        return;
    }

    // The lines arrive pre-wrapped — `bat` wrapped them, or `Plain` did — so no `Wrap` here. Ask
    // ratatui to wrap them again and it would re-break lines that already carry the highlighter's
    // own indentation, which reads as ragged nonsense in any fenced block.
    let lines: Vec<Line> = app.preview().to_vec();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// The dot.
///
/// `∙` is U+2219 BULLET OPERATOR, East Asian **Neutral** — one column in every locale, checked in
/// `every_calendar_glyph_is_one_column_in_every_locale` rather than assumed. The obvious
/// candidates are traps: `·` (U+00B7), `•` (U+2022) and every box-drawing character are
/// *Ambiguous* and render two columns under a CJK locale, which in a three-column day cell shifts
/// the rest of the week sideways and breaks the grid the whole sidebar width was chosen for.
const DOT: &str = "\u{2219}";

/// Week rows the grid always draws, whatever the month needs.
///
/// A month spans four to six of them, and drawing only as many as it needs would make the
/// sidebar's contents change height from month to month — which moves everything below it, and
/// there is now something below it: the thread graph would slide up and down the sidebar as the
/// months turned. Six is the worst case, so six is the budget and short months end on blank rows.
const CALENDAR_WEEKS: usize = 6;

/// Rows the calendar occupies inside the sidebar's borders.
///
/// The weekday header, then two rows per week — the numbers and the dots under them. Thirteen,
/// which is the *whole* of the sidebar's calendar budget: `stage6.md` had two variants stacked
/// here for comparison, one row per week against two, and the dot-under-the-day variant won. What
/// deleting the loser freed is what [`draw_graph`] draws into.
const CALENDAR_ROWS: u16 = 1 + CALENDAR_WEEKS as u16 * 2;

/// The weekday header, and the source of the sidebar's width.
///
/// Monday first: ISO 8601's week, and the one [`chrono`] counts from. Joined by a single space
/// this is exactly 20 columns — 7 cells of 2, 6 separators of 1 — which is [`SIDEBAR_WIDTH`]
/// less its borders.
const WEEKDAYS: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];

/// The sidebar: the month, and the thread graph under it while that is toggled on.
///
/// `now` is the frame's clock and `app.zone()` the offset it is read in — the same discipline as
/// everywhere else here. This module never asks the operating system what time it is or where it
/// is: a rendered frame is a function of the state handed to it, which is what makes a snapshot
/// worth taking. See [`App::zone`](crate::app::App::zone) for who supplies the offset.
fn draw_sidebar(frame: &mut Frame, area: Rect, app: &App, now: DateTime<Utc>) {
    let (calendar, graph) = split_sidebar(area, app.graph_is_open());
    draw_calendar(frame, calendar, app, now);
    if let Some(graph) = graph {
        draw_graph(frame, graph, app);
    }
}

/// The shortest a graph panel is worth drawing: two borders and a single row of content.
///
/// Below this the calendar keeps the whole column. A bordered box with nothing inside it is an
/// affordance that says a thing is here and then does not show it, which is worse than the key
/// appearing to do nothing.
const GRAPH_MIN_HEIGHT: u16 = 3;

/// Cut the sidebar into the calendar and, while it is toggled on, the graph beneath it.
///
/// The calendar's height is **fixed** at [`CALENDAR_ROWS`] plus its borders rather than shared
/// proportionally, and that is the whole reason the loser of the two-calendar comparison had to
/// go: a grid whose height moves is a grid that has to be re-found every time the pane resizes,
/// and everything below it moves with it. So the month takes exactly what a month needs and the
/// graph takes the remainder, which on a 24-row terminal — minus the status line — is eight rows.
///
/// The graph is dropped when the remainder is under [`GRAPH_MIN_HEIGHT`], by the same rule the
/// frame drops the reader and then the sidebar: the automatic decision wins, and the toggle is an
/// escape hatch in the direction that *removes* a pane.
fn split_sidebar(area: Rect, graph: bool) -> (Rect, Option<Rect>) {
    let calendar_height = CALENDAR_ROWS + 2;
    if !graph || area.height < calendar_height + GRAPH_MIN_HEIGHT {
        return (area, None);
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(calendar_height), Constraint::Min(1)])
        .split(area);
    (chunks[0], Some(chunks[1]))
}

/// The month: a grid of day numbers with a dot under any day that has a note on it.
fn draw_calendar(frame: &mut Frame, area: Rect, app: &App, now: DateTime<Utc>) {
    let today = now.with_timezone(&app.zone()).date_naive();
    let days = app.days_with_notes();

    // The month goes in the title rather than in a row of its own: it costs no height, and it is
    // what a bordered box's title is for.
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", today.format("%b %Y")));

    let lines = calendar(today, days);

    // No `Wrap`: every line here is built to the pane's exact inner width, and wrapping one would
    // fold a week onto the next row and desynchronise the grid from the header above it.
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// The weeks of `anchor`'s month, Monday first, padded to [`CALENDAR_WEEKS`] rows.
///
/// `None` is a cell before the first or after the last of the month — drawn blank rather than
/// filled with the neighbouring month's numbers, which would put dots on days this grid is not
/// claiming to be about.
fn month_grid(anchor: NaiveDate) -> Vec<[Option<NaiveDate>; 7]> {
    let mut grid = vec![[None; 7]; CALENDAR_WEEKS];
    // Day 1 exists in every month of every year, so this cannot fail; `with_day` is fallible for
    // the 29ths and 31sts, not for this.
    let Some(first) = anchor.with_day(1) else {
        return grid;
    };

    let mut cell = first.weekday().num_days_from_monday() as usize;
    let mut date = first;
    while date.month() == first.month() {
        if cell / 7 >= CALENDAR_WEEKS {
            break;
        }
        grid[cell / 7][cell % 7] = Some(date);
        cell += 1;
        // `succ_opt` rather than `+ Duration::days(1)`: the end of `NaiveDate`'s range is a real
        // value a caller can hand us, and running off it should end the month rather than panic.
        match date.succ_opt() {
            Some(next) => date = next,
            None => break,
        }
    }
    grid
}

/// The weekday header row, and the top of the grid.
fn weekday_header() -> Line<'static> {
    Line::from(Span::styled(WEEKDAYS.join(" "), dim()))
}

/// The calendar: the dot on its own row, directly under the day it belongs to.
///
/// Two rows per week — the numbers, then the dots — for [`CALENDAR_ROWS`] in total. The dot sits
/// under the units digit, which is where the eye is already looking on a right-aligned number, and
/// an empty day is two spaces rather than a placeholder: a grid of "no" marks says nothing and
/// reads as noise.
///
/// # Why the dot gets a row of its own
///
/// It cost twice the height of the alternative, which was styling the day number itself, and
/// `stage6.md` settled the question by building both and looking at them in one terminal against
/// one vault. The dot-under variant won and the other is deleted. Two rows per week is what a dot
/// that is *separate from the number* costs, and the reason to pay it is that a styled number
/// carries two facts in one glyph — which is which day, and whether it has notes — so the second
/// fact is only legible against its neighbours. A dot is legible on its own.
fn calendar(today: NaiveDate, days: &BTreeSet<NaiveDate>) -> Vec<Line<'static>> {
    let mut lines = vec![weekday_header()];

    for week in month_grid(today) {
        let mut numbers = Vec::new();
        let mut dots = Vec::new();
        for (i, cell) in week.iter().enumerate() {
            if i > 0 {
                numbers.push(Span::raw(" "));
                dots.push(Span::raw(" "));
            }
            match cell {
                None => {
                    numbers.push(Span::raw("  "));
                    dots.push(Span::raw("  "));
                }
                Some(day) => {
                    numbers.push(Span::styled(
                        format!("{:>2}", day.day()),
                        day_style(*day, today),
                    ));
                    dots.push(Span::styled(
                        if days.contains(day) {
                            format!(" {DOT}")
                        } else {
                            "  ".to_string()
                        },
                        dim(),
                    ));
                }
            }
        }
        lines.push(Line::from(numbers));
        lines.push(Line::from(dots));
    }
    lines
}

/// How a day number is painted before anything is known about its notes.
///
/// Today is reversed rather than bracketed or arrowed: a two-column cell has no room for a
/// decoration, and every glyph that would fit is one column of the grid that some locale renders
/// as two. Reverse video costs nothing and cannot move a column.
fn day_style(day: NaiveDate, today: NaiveDate) -> Style {
    if day == today {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    }
}

// ============================================================================== the thread graph

/// The graph's glyphs. ASCII, and that is a constraint rather than a style.
///
/// `*` node, `@` the focused note, `|` a lane, `/` and `\` the diagonals under a fork, `+` the
/// fold described in [`MAX_LANES`]. Every one of them is East Asian **Narrow** — one column in any
/// locale — which is checked in `every_graph_glyph_is_one_column_in_every_locale` rather than
/// assumed. `undotree` offers a Unicode set as an option and this deliberately does not take it:
/// `●`, `○`, `│`, `╱`, `╲` and every box-drawing character in U+2500–U+2573 are **Ambiguous** and
/// render two columns under a CJK locale, which in a fixed gutter is a frame that has come apart.
const GLYPH_NODE: char = '*';
/// See [`GLYPH_NODE`]. The focused note, so "where am I" is answerable without reading colour.
const GLYPH_FOCUS: char = '@';
/// See [`GLYPH_NODE`].
const GLYPH_LANE: char = '|';
/// See [`GLYPH_NODE`].
const GLYPH_FORK_RIGHT: char = '\\';
/// See [`GLYPH_NODE`].
const GLYPH_FORK_LEFT: char = '/';
/// See [`GLYPH_NODE`] and [`MAX_LANES`].
const GLYPH_MORE: char = '+';

/// Columns between the gutter and the title.
const GRAPH_GAP: usize = 1;

/// The widest the gutter is drawn, however many lanes the thread actually has.
///
/// # Why there has to be a cap
///
/// The gutter is one column per live lane and the number of live lanes is unbounded in principle:
/// a note with forty replies opens forty lanes. The sidebar has **20 columns inside its borders**,
/// which is the whole budget for gutter, gap and title together, so an uncapped gutter is a title
/// column that can reach zero — and a graph whose labels have been eaten by its own art is not a
/// minimap, it is a decoration.
///
/// # Why six
///
/// Six lanes plus the gap leaves 13 columns of title, which is about two short words. Below that a
/// title stops distinguishing anything and the panel stops answering the question it exists for.
/// Six simultaneous open branches in one conversation is also well past anything the vault this is
/// dogfooded against has produced, so the cap is a guard rather than a routine truncation.
///
/// # What happens past it
///
/// The lanes at and beyond the last drawn column **fold into that column**, and nothing is
/// dropped: every note still gets its row and its title. The folded column shows the node glyph
/// when this row's note is out there, [`GLYPH_MORE`] when some other lane is, and a blank when
/// nothing is. So the *art* degrades and the *list* does not — which is the right way round,
/// because the list is what carries the titles and the focus.
const MAX_LANES: usize = 6;

/// The thread graph: the focused note's whole thread as a lane gutter and one title per node.
///
/// **This is a minimap, not a reading view.** Twenty columns hold a gutter and a truncated title
/// and nothing else; it answers "where am I in this thread, and what else is in it" and refers
/// everything past that to the reader panel, which is already showing the note. Trying to make it
/// read — wrapping titles, showing bodies, adding a meta column — is how a 20-column pane becomes
/// unreadable in both jobs.
fn draw_graph(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title(" thread ");

    let rows = app.graph();
    if rows.is_empty() {
        // Reachable with an empty list, and only then: every note is in a thread, even if that
        // thread is only itself. A workspace whose schema declares no `relation:*` entry lands on
        // the one-row graph below rather than here, which is the point — the panel says "you are
        // here, alone" instead of offering an affordance that could never fire.
        // No leading blank line, unlike the reader's version of this message: the graph's pane can
        // be a single row tall, and a message spent on a blank line is a message nobody reads.
        let paragraph = Paragraph::new("  Nothing selected.")
            .block(block)
            .style(dim());
        frame.render_widget(paragraph, area);
        return;
    }

    let inner_width = area.width.saturating_sub(2) as usize;
    let inner_height = area.height.saturating_sub(2) as usize;

    // One gutter width for the whole panel rather than per row, so the titles form a column. The
    // widest row decides it, capped; a linear thread is therefore two columns of gutter and gap,
    // exactly as promised.
    let gutter = rows
        .iter()
        .map(|row| row.lanes.len())
        .max()
        .unwrap_or(1)
        .clamp(1, MAX_LANES);
    let title_width = inner_width.saturating_sub(gutter + GRAPH_GAP);

    let focus = rows
        .iter()
        .position(|row| row.node.as_ref().is_some_and(|node| node.is_focus))
        .unwrap_or(0);
    let window = graph_window(rows.len(), focus, inner_height);

    let lines: Vec<Line> = window
        .clone()
        .map(|at| {
            // The counts include the row the marker itself displaces, which is the only honest
            // arithmetic: `+2 above` has to mean two rows you cannot see, not two plus this one.
            let elided = if at == window.start && window.start > 0 && at != focus {
                Some(format!("+{} above", window.start + 1))
            } else if at + 1 == window.end && window.end < rows.len() && at != focus {
                Some(format!("+{} below", rows.len() - window.end + 1))
            } else {
                None
            };
            match elided {
                // Blank gutter rather than a lane glyph: the lanes do carry on through the
                // elision, but drawing them would make this look like a connector row, and the
                // one thing this row has to say is a number.
                Some(text) => Line::from(Span::styled(
                    format!(
                        "{}{}",
                        " ".repeat(gutter + GRAPH_GAP),
                        truncate(&text, title_width)
                    ),
                    dim(),
                )),
                None => graph_line(&rows[at], gutter, title_width),
            }
        })
        .collect();

    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// Which slice of the graph a panel `height` rows tall shows.
///
/// # An over-tall thread scrolls to the focus rather than being cut off at the root
///
/// The sidebar's graph gets what the calendar leaves — eight rows on a 24-row terminal, six of
/// them content — and threads are routinely longer than that. Showing the first six rows would put
/// the root on screen and the note you are actually reading off it, which inverts what the panel
/// is for.
///
/// So the window is **centred on the focus** and clamped to both ends, and the focused note is
/// guaranteed to be inside it: `start` is never past `focus`, and `start + height` is always
/// beyond it. A short thread is shown whole and never scrolls.
///
/// The first and last visible rows are then spent on `+n above` / `+n below` markers when there is
/// anything out of view, which costs at most two nodes and buys the one thing a window cannot say
/// for itself — that it is a window. The markers never displace the focus: the check is at the
/// call site, and it is why this function returns the range rather than the lines.
fn graph_window(len: usize, focus: usize, height: usize) -> std::ops::Range<usize> {
    if height == 0 {
        return 0..0;
    }
    if len <= height {
        return 0..len;
    }
    let start = focus.saturating_sub(height / 2).min(len - height);
    start..start + height
}

/// One row: the gutter, a space, and as much of the title as is left.
fn graph_line(row: &GraphRow, gutter: usize, title_width: usize) -> Line<'static> {
    let focused = row.node.as_ref().is_some_and(|node| node.is_focus);
    let mut spans = vec![
        Span::styled(fold_gutter(&row.lanes, gutter, focused), dim()),
        Span::raw(" ".repeat(GRAPH_GAP)),
    ];

    if let Some(node) = &row.node {
        let (text, style) = match &node.title {
            Some(title) => (title.clone(), Style::default()),
            // Same word the list uses for the same state, so the two panes agree about what an
            // untitled note is called.
            None => ("Untitled".to_string(), dim()),
        };
        let style = if focused {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            style
        };
        spans.push(Span::styled(truncate(&text, title_width), style));
    }

    Line::from(spans)
}

/// Render `lanes` into exactly `width` columns, folding anything past the last one.
///
/// See [`MAX_LANES`] for what the fold means and why it is a fold rather than a truncation.
fn fold_gutter(lanes: &[Lane], width: usize, focused: bool) -> String {
    let node = if focused { GLYPH_FOCUS } else { GLYPH_NODE };
    (0..width)
        .map(|at| {
            let folds = at + 1 == width && lanes.len() > width;
            if !folds {
                return lane_glyph(lanes.get(at).copied().unwrap_or(Lane::Empty), node);
            }
            let rest = &lanes[at..];
            if rest.contains(&Lane::Node) {
                node
            } else if rest.iter().all(|lane| *lane == Lane::Empty) {
                ' '
            } else {
                GLYPH_MORE
            }
        })
        .collect()
}

/// How one lane column is painted. See [`GLYPH_NODE`] for the width rule these all obey.
fn lane_glyph(lane: Lane, node: char) -> char {
    match lane {
        Lane::Empty => ' ',
        Lane::Through => GLYPH_LANE,
        Lane::Node => node,
        Lane::ForkRight => GLYPH_FORK_RIGHT,
        Lane::ForkLeft => GLYPH_FORK_LEFT,
    }
}

/// The row list, which serves all four `Row` views.
fn draw_list(frame: &mut Frame, area: Rect, app: &App, now: DateTime<Utc>) {
    let header = match app.view() {
        ViewKind::Timeline => {
            if app.is_flat() {
                " timeline — every note ".to_string()
            } else {
                " timeline — thread roots ".to_string()
            }
        }
        ViewKind::Files => format!(" files — sort: {} ", sort_name(app.sort())),
        ViewKind::Search => format!(" search: {}▏", app.query()),
        ViewKind::Trash => " trash ".to_string(),
    };

    let block = Block::default().borders(Borders::ALL).title(header);

    if app.rows().is_empty() {
        let paragraph = Paragraph::new(empty_message(app)).block(block).style(dim());
        frame.render_widget(paragraph, area);
        return;
    }

    // Width available for the title: the frame's borders, the marker, the id column and the
    // right-hand meta column all come off first, so a long title is truncated rather than pushing
    // the age off the edge.
    //
    // The arithmetic has to close exactly. A row is `MARKER + id + title + gap + meta`, and that
    // must total *at most* the block's inner width — two columns wider and the meta column is
    // clipped off the right edge, which is invisible in a snapshot because the trailing spaces get
    // trimmed and it simply looks as though notes have no age.
    let inner = area.width.saturating_sub(2) as usize;

    // Both columns are measured against the rows actually on screen rather than fixed. An id is
    // thirteen characters — the whole millisecond timestamp, which is where the TUI has floored
    // since stage 5 and where `jot ls` now floors too — until a burst of notes shares a
    // millisecond and forces a fourteenth; a meta cell is three characters for a lone note and
    // thirteen for a branching week-old thread. Sizing to the
    // widest present is what keeps the age beside the title instead of a fixed guess away from it
    // — which was the visible complaint: a right-aligned column in an 80-column frame puts the
    // time an inch of whitespace from the title it belongs to.
    let id_width = app
        .rows()
        .iter()
        .map(|row| app.short_id(row.note.id).width())
        .max()
        .unwrap_or(0);
    let widest = |detail| {
        app.rows()
            .iter()
            .map(|row| meta_text(row, now, detail).width())
            .max()
            .unwrap_or(0)
    };

    // The title column is sized to the *titles*, not to the space available. Filling the width
    // was what stranded the age an inch of whitespace away from the title it describes: a column
    // of five-character titles in a fifty-column list put every age at column fifty. Sized to
    // content, the meta follows the titles and the rest of the row stays empty.
    let title_natural = app
        .rows()
        .iter()
        .map(|row| title_of(row).width())
        .max()
        .unwrap_or(0);

    let id_column = if id_width == 0 { 0 } else { id_width + ID_GAP };
    let avail = inner.saturating_sub(MARKER_WIDTH + id_column);

    // Priority when it does not all fit: the age, then the title, then the counts. The age is the
    // one part of a row that is never inferable from anything else on screen, and the counts are
    // the one part that is — a thread's size is visible the moment you open it. So the meta column
    // degrades to the age alone rather than being clipped, which would have taken the age and left
    // the counts. Same shape as the key bar dropping labels before it drops keys.
    let detail = if title_natural.min(TITLE_MAX) + META_GAP + widest(MetaDetail::Full) <= avail {
        MetaDetail::Full
    } else {
        MetaDetail::AgeOnly
    };
    let meta_width = widest(detail);
    let title_width = title_natural
        .min(avail.saturating_sub(meta_width + META_GAP))
        .min(TITLE_MAX);

    let columns = Columns {
        id: id_width,
        title: title_width,
        meta: meta_width,
        detail,
        in_trash: app.view() == ViewKind::Trash,
    };
    let items: Vec<ListItem> = app
        .rows()
        .iter()
        .map(|row| ListItem::new(row_line(row, &app.short_id(row.note.id), columns, now)))
        .collect();

    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(Color::Cyan),
    );

    let mut state = ListState::default();
    state.select(Some(app.selected()));
    frame.render_stateful_widget(list, area, &mut state);
}

/// Columns the leading marker occupies: two glyph slots and a space.
///
/// Both slots are reserved on every row, including the rows that fill neither. Letting the quote
/// glyph slide left when the relation slot is empty would put the same symbol in two different
/// columns, and a column you have to *read* to locate is not a column.
///
/// Every glyph used here — `⌫`, `⚠`, `↳`, `⚑`, `❯` — is East Asian **Neutral**, so all of them are
/// one column in any locale. That is a property that was checked rather than assumed: `◆`, `★`,
/// `┬`, `¶` and `“` are all *Ambiguous* and render two columns wide under a CJK locale, which in a
/// fixed-width cell is a broken frame in exactly the vault that has CJK titles in it.
const MARKER_WIDTH: usize = 3;

/// The first marker slot: where this note sits, or what is wrong with where it sits.
///
/// First match wins, and the order is the point — a fact about the *vault* outranks a fact about
/// the note, because it is the one you cannot find out any other way from a list.
///
/// Each glyph means one thing. `⌫` is always "this note is in the trash"; `⚠` is always "this
/// note's parent is not where it should be", with the colour saying whether that is recoverable.
/// Sharing a glyph between the two — which is what this did before there was anything else in the
/// column — made `⌫` ambiguous the moment a second row could carry it for a different reason.
fn relation_marker<'a>(row: &Row, in_trash: bool) -> (&'a str, Style) {
    let warn = Style::default().fg(Color::Yellow);

    match &row.parent {
        // Trashed-ness is the trash view's whole premise, so marking every row with it there says
        // nothing and costs the slot that would have said something.
        _ if row.state == State::Trashed && !in_trash => ("\u{232b}", warn),
        // A purged parent is unrecoverable, and the note is now a root that never asked to be one.
        Some(Ref::Deleted(_)) => ("\u{26a0}", Style::default().fg(Color::Red)),
        // A trashed parent is the state stage 2 exists to make visible: the note is live, its
        // parent is not, and a list that says nothing about it is lying by omission. Recoverable,
        // hence yellow rather than red.
        Some(Ref::Trashed(_)) => ("\u{26a0}", warn),
        // A reply with a live parent. The glyph is what the flat timeline was missing: before it,
        // a reply and a standalone note were the same row.
        Some(Ref::Present(_)) => ("\u{21b3}", dim()),
        // No parent. A head of something, or a note on its own — and the difference is worth a
        // glyph, because one of them is a thread you have not read yet.
        None if row.replies > 0 => ("\u{2691}", dim()),
        None => (" ", dim()),
    }
}

/// Columns between the id and the title. Two, matching `jot ls`, so the two surfaces read as one
/// listing rather than as two conventions.
const ID_GAP: usize = 2;

/// Columns between the longest title and the meta column.
///
/// A floor, not a target. With the title column sized to content, the longest title would
/// otherwise touch its own age — `a long title3h` — which is not a tighter layout but an
/// unreadable one.
const META_GAP: usize = 2;

/// The widest a title column grows before the meta column stops following it.
///
/// Without a cap the age is right-aligned to the frame, which on a wide terminal strands it a long
/// way from the title it describes and makes the pair hard to read as one row. Past this the row
/// simply ends and the rest of the line stays empty.
const TITLE_MAX: usize = 44;

/// The list's column widths, decided once for the whole frame.
///
/// Passed as one value rather than six: they are a single decision — every one of them is chosen
/// against the others and against the frame's width — and splitting them across a parameter list
/// invites a caller to compute one of them somewhere else.
#[derive(Debug, Clone, Copy)]
struct Columns {
    /// Width of the id column, before its gap.
    id: usize,
    /// Width of the title column.
    title: usize,
    /// Width of the right-hand meta column.
    meta: usize,
    /// How much of the meta column there is room for.
    detail: MetaDetail,
    /// Whether this is the trash, where trashed-ness is the premise rather than news.
    in_trash: bool,
}

/// One row: marker, id, title, then the counts and age.
fn row_line<'a>(row: &Row, id: &str, columns: Columns, now: DateTime<Utc>) -> Line<'a> {
    let Columns {
        id: id_width,
        title: title_width,
        meta: meta_width,
        detail,
        in_trash,
    } = columns;

    let title = title_of(row);
    let title_style = if row.note.title.is_some() {
        Style::default()
    } else {
        dim()
    };

    let (relation, relation_style) = relation_marker(row, in_trash);
    let quote = if row.note.quote.is_some() {
        "\u{276f}"
    } else {
        " "
    };

    // Truncated as well as padded. The column is sized to the widest row, so this can only fire
    // when even the age does not fit — but a frame two columns narrower than the arithmetic
    // expected paints over its own border, and that is invisible in a snapshot.
    let meta = truncate(&meta_text(row, now, detail), meta_width);
    let title_cell = truncate(&title, title_width);
    // Right-align the meta column: pad out the title's cell, then pad out the meta's own.
    let pad = meta_width.saturating_sub(meta.width());
    let gap = title_width.saturating_sub(title_cell.width()) + META_GAP + pad;

    // Yellow, and ahead of the title, exactly as `jot ls` prints it. An id you can read off the
    // browser and paste into `jot show` is the whole reason it is here, and it only reads as the
    // same id if it looks like the same id.
    let id_cell = format!(
        "{id}{}{}",
        " ".repeat(id_width.saturating_sub(id.width())),
        " ".repeat(if id_width == 0 { 0 } else { ID_GAP })
    );

    Line::from(vec![
        Span::styled(relation, relation_style),
        Span::styled(quote, dim()),
        Span::raw(" "),
        Span::styled(id_cell, Style::default().fg(Color::Yellow)),
        Span::styled(title_cell, title_style),
        Span::raw(" ".repeat(gap)),
        Span::styled(meta, dim()),
    ])
}

/// What a row calls itself. `Untitled` for a note with no title, which is a legal note.
///
/// Shared by the render and by the column measurement above, because a column sized against one
/// string and filled with another is a column that is wrong by exactly the difference.
fn title_of(row: &Row) -> String {
    row.note
        .title
        .clone()
        .unwrap_or_else(|| "Untitled".to_string())
}

/// How much of the right-hand column there is room for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaDetail {
    /// Counts and the age.
    Full,
    /// The age alone, for a list too narrow to carry both.
    AgeOnly,
}

/// The right-hand column: reply count, quote count, then relative age.
fn meta_text(row: &Row, now: DateTime<Utc>, detail: MetaDetail) -> String {
    let age = row
        .note
        .created_at
        .map_or_else(|| "—".to_string(), |t| relative(t, now));

    if detail == MetaDetail::AgeOnly {
        return age;
    }

    let mut out = String::new();
    if row.replies > 0 {
        if row.replies == row.descendants {
            out.push_str(&format!("{} \u{25b8} ", row.replies));
        } else {
            // Direct replies and the whole subtree differ, which means the thread branches.
            // Showing both is what makes a fork visible from the list rather than only after
            // opening it.
            out.push_str(&format!("{}/{} \u{25b8} ", row.replies, row.descendants));
        }
    }
    // How many notes point *at* this one. A count rather than a list, and here rather than in the
    // marker, because it is the same kind of fact as the reply count and belongs beside it — the
    // marker says what this note is, the meta says what has accumulated around it.
    if row.quoted > 0 {
        out.push_str(&format!("{} \u{275e} ", row.quoted));
    }
    out.push_str(&age);
    out
}

/// What an empty list should say. Never just blank — an empty frame is indistinguishable from a
/// broken one.
fn empty_message(app: &App) -> &'static str {
    match app.view() {
        ViewKind::Timeline => "\n  Nothing here yet. Press n to write the first note.",
        ViewKind::Files => "\n  No notes.",
        ViewKind::Search => "\n  Type to search titles.",
        ViewKind::Trash => "\n  The trash is empty.",
    }
}

/// The status line: a toast if there is one, otherwise the standing hint.
fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let line = if let Some(toast) = app.toast() {
        let style = if toast.is_error {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Yellow)
        };
        Line::from(Span::styled(format!(" {}", toast.message), style))
    } else if app.keymap_is_armed() {
        // An armed prefix that swallows the next key with no visible cause is indistinguishable
        // from the application having hung.
        Line::from(Span::styled(
            " Space — n new, r reply, q quote, e edit, x trash",
            Style::default().fg(Color::Cyan),
        ))
    } else if app.mode() == Mode::Input {
        Line::from(Span::styled(
            " typing — Enter to accept, Tab or Esc to leave",
            dim(),
        ))
    } else {
        return draw_key_bar(frame, area, app);
    };

    frame.render_widget(Paragraph::new(line), area);
}

/// The standing footer: position, then as many key hints as the width allows.
///
/// Built from [`Keymap::footer`] rather than a hand-written string, so a key cannot appear here
/// without existing, and `?` and the footer cannot drift apart.
///
/// # What the bar is for
///
/// Not documentation: `?` is documentation. The bar carries the keys you cannot guess and the ones
/// that change the vault, and it carries them in two runs — write, then view — separated by a dot,
/// because a single stream of pairs reads as a wall with nothing to tell the eye that `x` and
/// `Tab` are different kinds of thing. The destructive key is coloured differently for the same
/// reason: `x` in the same cyan as `n` says the two are the same kind of thing, and the bar is the
/// last place you see the key before pressing it.
///
/// # The prefix is printed once
///
/// Every write sits behind `Space`, so the write run is headed by a single dim `Space` and its
/// hints carry only the suffix: `Space  n new  r reply  q quote  e edit  x trash`. Spelling it out
/// five times would cost thirty columns to say one thing, on a line that is already dropping
/// labels to fit.
///
/// # Labels go before keys do
///
/// The bar used to shed whole hints from the right, which on an 80-column terminal meant losing
/// `x trash` while keeping hints for keys anyone would have guessed. Now the *labels* go first:
/// `n new` becomes `n`, and only if the keys alone still do not fit does anything get dropped. An
/// 80-column terminal therefore shows every key that works, just tersely — which is strictly
/// better than showing half of them in full. Whatever happens, `?` and `Space q` survive: `?` is
/// how you find every other key, and `Space q` is how you leave.
fn draw_key_bar(frame: &mut Frame, area: Rect, app: &App) {
    let scope = match app.view() {
        ViewKind::Timeline => Scope::Timeline,
        ViewKind::Files => Scope::Files,
        ViewKind::Trash => Scope::Trash,
        // Search has no keys of its own; typing is the interaction.
        ViewKind::Search => Scope::Always,
    };

    let count = app.rows().len();
    let position = if count == 0 {
        String::new()
    } else {
        format!("{}/{count}", app.selected() + 1)
    };

    let hints: Vec<&crate::key::Binding> = Keymap::footer(scope, app.focused().is_some()).collect();
    let pinned: Vec<&crate::key::Binding> = Keymap::footer_pinned().collect();

    let width = area.width as usize;
    let lead = 1 + position.width();
    // Widest tier that fits, then dropping from the right as a last resort. The pinned tail is
    // measured first and never spent, so it is there at every width.
    let labelled = fits(&hints, &pinned, Labels::Full, lead, width);
    let labels = if labelled { Labels::Full } else { Labels::None };

    let mut spans = Vec::new();
    if !position.is_empty() {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(position, Style::default().fg(Color::DarkGray)));
    }

    let budget = width.saturating_sub(tail_width(&pinned, labels));
    let mut used = lead;
    let mut group = None;
    for binding in hints {
        // A separator costs width too, and paying for one only to drop the run it introduces
        // would leave the bar ending on a dangling dot. The prefix marker is the same: it is only
        // worth its columns if the run it heads actually lands.
        let first = group.is_none();
        let starts_group = group.is_some_and(|g| g != binding.group);
        let marker = (first || starts_group) && binding.is_prefixed();
        let cost = hint_width(binding, labels)
            + if starts_group { SEPARATOR.width() } else { 0 }
            + if marker {
                2 + PREFIX_LABEL.trim_end().width()
            } else {
                0
            };
        if used + cost > budget {
            break;
        }
        used += cost;
        if starts_group {
            spans.push(Span::styled(SEPARATOR, dim()));
        }
        if marker {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                PREFIX_LABEL.trim_end(),
                Style::default().fg(Color::Cyan),
            ));
        }
        group = Some(binding.group);
        push_hint(&mut spans, binding, labels);
    }

    if group.is_some() {
        spans.push(Span::styled(SEPARATOR, dim()));
    }
    for binding in pinned {
        push_hint(&mut spans, binding, labels);
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Whether a hint carries its label as well as its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Labels {
    /// `n new`.
    Full,
    /// `n`, for a terminal too narrow to spell it out.
    None,
}

/// What sits between the footer's runs. Two spaces read as no boundary at all; a dot reads as one.
const SEPARATOR: &str = "  \u{b7}";

/// `"  n new"` — two spaces of separation, one between the key and its label.
fn hint_width(binding: &crate::key::Binding, labels: Labels) -> usize {
    2 + binding.footer_key().width()
        + match labels {
            Labels::Full => 1 + binding.short.width(),
            Labels::None => 0,
        }
}

/// The reserved right-hand end: the separator before it, and the pinned hints themselves.
fn tail_width(pinned: &[&crate::key::Binding], labels: Labels) -> usize {
    SEPARATOR.width() + pinned.iter().map(|b| hint_width(b, labels)).sum::<usize>()
}

/// Whether every hint fits at this label tier.
fn fits(
    hints: &[&crate::key::Binding],
    pinned: &[&crate::key::Binding],
    labels: Labels,
    lead: usize,
    width: usize,
) -> bool {
    let separators = hints
        .windows(2)
        .filter(|pair| pair[0].group != pair[1].group)
        .count()
        * SEPARATOR.width();
    let markers = hints
        .iter()
        .enumerate()
        .filter(|(i, b)| b.is_prefixed() && (*i == 0 || hints[i - 1].group != b.group))
        .count()
        * (2 + PREFIX_LABEL.trim_end().width());
    let body: usize = hints.iter().map(|b| hint_width(b, labels)).sum();
    lead + body + separators + markers + tail_width(pinned, labels) <= width
}

/// Append one hint's spans.
fn push_hint(spans: &mut Vec<Span<'static>>, binding: &crate::key::Binding, labels: Labels) {
    spans.push(Span::raw("  "));
    spans.push(Span::styled(binding.footer_key(), key_style(binding)));
    if labels == Labels::Full {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(binding.short, dim()));
    }
}

/// How a footer key is painted. Destructive keys are not the same kind of thing as the rest.
fn key_style(binding: &crate::key::Binding) -> Style {
    if binding.destructive {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::Cyan)
    }
}

/// Width of the key column in the help overlay. `Space q` is the longest key, at 7.
const KEY_COLUMN: usize = 10;

/// The `?` overlay, rendered from the same table the event loop dispatches on.
fn draw_help(frame: &mut Frame, area: Rect) {
    let bindings = Keymap::bindings();

    // Sized to the *content*: a guessed constant silently truncates the longest row, and the
    // longest row here is `Tab`'s, which is exactly the one a newcomer most needs to read whole.
    // 2 columns of border, 2 of indent, then the key column and the description.
    let widest = bindings
        .iter()
        .map(|b| KEY_COLUMN + b.description.width())
        .max()
        .unwrap_or(0);
    let width = u16::try_from(widest + 6)
        .unwrap_or(u16::MAX)
        .min(area.width.saturating_sub(4));
    let height = (bindings.len() as u16 + 4).min(area.height.saturating_sub(2));
    let popup = centred(area, width, height);

    let lines: Vec<Line> = bindings
        .iter()
        .map(|b| {
            Line::from(vec![
                Span::styled(
                    format!("  {:<width$}", b.keys, width = KEY_COLUMN),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(b.description),
            ])
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" keys — any key to close ");

    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

/// A `width` × `height` rectangle centred in `area`.
fn centred(area: Rect, width: u16, height: u16) -> Rect {
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

/// Dimmed text, for everything that is context rather than content.
fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

/// Truncate to a terminal *column* budget, not a character count.
///
/// A CJK title is two columns per character and an emoji can be two as well, so truncating on
/// `chars()` overflows the cell and corrupts the frame's right-hand border. The ellipsis costs one
/// column and is accounted for.
fn truncate(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }

    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = UnicodeWidthStr::width(c.to_string().as_str());
        if used + w > width.saturating_sub(1) {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// A short, human relative time: `2d`, `5h`, `now`.
///
/// Deliberately coarse. This column exists to answer "roughly when", and a precise timestamp in a
/// list is noise that costs the width a title needs.
fn relative(then: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let delta = now.signed_duration_since(then);
    let secs = delta.num_seconds();

    // A note created in the future means a clock skew or a hand-edited id. Say so quietly rather
    // than rendering a negative age.
    if secs < 0 {
        return "ahead".to_string();
    }
    match secs {
        0..=59 => "now".to_string(),
        60..=3599 => format!("{}m", delta.num_minutes()),
        3600..=86_399 => format!("{}h", delta.num_hours()),
        86_400..=2_591_999 => format!("{}d", delta.num_days()),
        _ => format!("{}w", delta.num_weeks()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    /// A frame `width` columns wide, tall enough that height never decides anything.
    fn frame(width: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width,
            height: 20,
        }
    }

    /// A date that exists, spelled where the test can read it.
    fn on(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("a real date")
    }

    #[test]
    fn the_panes_drop_in_the_settled_order_as_the_frame_narrows() {
        // Reader first, then sidebar, then the list alone. Checked one column either side of each
        // boundary, because a drop order is only a decision at the boundary.
        let all = split_frame(frame(112), Panes::default());
        assert_eq!(all.sidebar.map(|r| r.width), Some(SIDEBAR_WIDTH));
        assert_eq!(all.list.width, LIST_MIN);
        assert_eq!(
            all.reader.map(|r| r.width),
            Some(112 - SIDEBAR_WIDTH - LIST_MIN)
        );

        let two = split_frame(frame(111), Panes::default());
        assert!(
            two.reader.is_none(),
            "one column short of three panes, the reader is what goes"
        );
        assert_eq!(two.sidebar.map(|r| r.width), Some(SIDEBAR_WIDTH));
        assert_eq!(two.list.width, 111 - SIDEBAR_WIDTH);

        let two = split_frame(frame(62), Panes::default());
        assert_eq!(two.sidebar.map(|r| r.width), Some(SIDEBAR_WIDTH));
        assert_eq!(
            two.list.width, LIST_MIN,
            "62 is the sidebar plus a list's floor"
        );

        let one = split_frame(frame(61), Panes::default());
        assert!(
            one.sidebar.is_none() && one.reader.is_none(),
            "and one column below that, the sidebar goes too"
        );
        assert_eq!(
            one.list.width, 61,
            "the list is never dropped: it is the surface"
        );
    }

    #[test]
    fn a_toggle_only_ever_removes_a_pane() {
        // Wanting everything at 60 columns gets a list, not a two-column reader painted over it.
        // The automatic rule wins; the toggles are an escape hatch in one direction only.
        let narrow = split_frame(frame(60), Panes::default());
        assert!(narrow.sidebar.is_none() && narrow.reader.is_none());
        assert_eq!(narrow.list.width, 60);

        let hidden = Panes {
            sidebar: false,
            ..Panes::default()
        };
        let no_sidebar = split_frame(frame(100), hidden);
        assert!(no_sidebar.sidebar.is_none());
        assert!(
            no_sidebar.reader.is_some(),
            "100 columns carries no reader beside a sidebar and does without one — which is the \
             only way a toggle adds anything, and it adds it by taking something away"
        );

        let no_reader = split_frame(
            frame(112),
            Panes {
                reader: false,
                ..Panes::default()
            },
        );
        assert!(no_reader.reader.is_none());
        assert_eq!(
            no_reader.list.width,
            112 - SIDEBAR_WIDTH,
            "with the reader away the list takes the whole main pane"
        );
    }

    #[test]
    fn the_width_the_run_loop_asks_for_is_the_width_the_panel_gets() {
        // The run loop holds a terminal size and no `App`, so it asks the default-pane question.
        // That has to agree with what is painted in the default state, or the highlighter wraps
        // text for a panel of a different width.
        let painted = split_frame(frame(112), Panes::default()).reader.unwrap();
        assert_eq!(reader_text_width(112, 21), Some(painted.width - 2));
        assert_eq!(reader_text_width(111, 21), None);

        // And the app-aware answer differs the moment the sidebar is hidden, which is why the
        // second entry point exists.
        let hidden = Panes {
            sidebar: false,
            ..Panes::default()
        };
        assert_eq!(reader_text_width(100, 21), None);
        assert!(reader_text_width_for(hidden, 100, 21).is_some());
    }

    #[test]
    fn the_sidebar_is_exactly_as_wide_as_the_grid_it_carries() {
        // The constant is derived from the calendar, not chosen for it. If the header ever grows
        // a column — a week-number gutter, a wider weekday abbreviation — this is what says so.
        assert_eq!(
            WEEKDAYS.join(" ").width() + 2,
            SIDEBAR_WIDTH as usize,
            "seven cells of two columns, six separators, and two borders"
        );
    }

    #[test]
    fn the_month_grid_is_always_six_weeks_and_starts_on_the_right_weekday() {
        let grid = month_grid(on(2026, 9, 4));
        assert_eq!(
            grid.len(),
            CALENDAR_WEEKS,
            "a grid whose height followed the month would move everything under it"
        );
        assert_eq!(grid[0][0], None, "September 2026 opens on a Tuesday");
        assert_eq!(grid[0][1], Some(on(2026, 9, 1)));
        assert_eq!(grid[4][2], Some(on(2026, 9, 30)));
        assert!(
            grid[5].iter().all(Option::is_none),
            "a five-week month ends on a blank row rather than October's numbers"
        );

        // August 2026 opens on a Saturday and runs 31 days, which is the six-week worst case the
        // height budget was chosen against.
        let long = month_grid(on(2026, 8, 15));
        assert_eq!(long[0][5], Some(on(2026, 8, 1)));
        assert_eq!(long[5][0], Some(on(2026, 8, 31)));
    }

    #[test]
    fn a_month_grid_at_the_end_of_time_does_not_panic() {
        // `NaiveDate::MAX` is a value a caller can hand us — `days_with_notes` is bounded by it —
        // and running off the end of the calendar must end the month rather than the process.
        let grid = month_grid(NaiveDate::MAX);
        assert!(grid.iter().flatten().any(Option::is_some));
    }

    #[test]
    fn every_calendar_row_is_exactly_the_sidebars_inner_width() {
        // A row one column wide of the pane wraps, and a wrapped week desynchronises every row
        // below it from the header above it.
        let today = on(2026, 9, 4);
        let days = BTreeSet::from([on(2026, 9, 9), on(2026, 9, 30)]);
        let inner = SIDEBAR_WIDTH as usize - 2;

        for line in calendar(today, &days) {
            assert_eq!(line.width(), inner, "`{line}` is not {inner} columns");
        }
    }

    #[test]
    fn the_dot_sits_under_the_day_it_belongs_to() {
        let today = on(2026, 9, 4);
        let days = BTreeSet::from([on(2026, 9, 9)]);
        let lines: Vec<String> = calendar(today, &days)
            .iter()
            .map(ToString::to_string)
            .collect();

        // Header, then two rows per week. The 9th is the Wednesday of the second week, so it
        // occupies columns 6 and 7 of the numbers row.
        assert_eq!(&lines[3][6..8], " 9");
        assert_eq!(
            lines[4].chars().nth(7),
            Some('\u{2219}'),
            "the dot goes under the units digit, where the eye already is: `{}`",
            lines[4]
        );
        assert_eq!(
            lines
                .iter()
                .flat_map(|l| l.chars())
                .filter(|c| *c == '\u{2219}')
                .count(),
            1,
            "one note, one day, one dot — a count would be a heat map"
        );
    }

    #[test]
    fn a_month_with_no_notes_gets_no_dots_and_no_placeholders() {
        let today = on(2026, 9, 4);
        let rendered: String = calendar(today, &BTreeSet::new())
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(
            !rendered.contains(DOT),
            "an empty month draws nothing, rather than a grid of `no` marks: {rendered:?}"
        );
    }

    #[test]
    fn the_calendar_marks_today_without_spending_a_column_on_it() {
        let today = on(2026, 9, 4);
        let days = BTreeSet::new();

        let marked: Vec<String> = calendar(today, &days)
            .iter()
            .flat_map(|line| line.spans.iter())
            .filter(|span| span.style.add_modifier.contains(Modifier::REVERSED))
            .map(|span| span.content.to_string())
            .collect();
        assert_eq!(
            marked,
            [" 4"],
            "today is reverse video: a bracket or an arrow would cost a column the grid does not \
             have, and every glyph that fits is two columns in some locale"
        );
    }

    #[test]
    fn the_calendar_costs_exactly_the_rows_the_sidebar_budgeted_for_it() {
        // 13 content rows, whatever the month: the header and two rows per week. That number is
        // the sidebar's calendar budget and the thing `split_sidebar` subtracts, so a calendar
        // that grew a row would silently take one off the graph below it.
        let days = BTreeSet::new();
        for anchor in [on(2026, 9, 4), on(2026, 2, 1), on(2026, 8, 15)] {
            assert_eq!(
                calendar(anchor, &days).len(),
                CALENDAR_ROWS as usize,
                "{anchor} draws a different number of rows from every other month"
            );
        }
        assert_eq!(CALENDAR_ROWS as usize, 1 + CALENDAR_WEEKS * 2);
    }

    #[test]
    fn every_calendar_glyph_is_one_column_in_every_locale() {
        // The same property the marker column has, and the same trap: `·`, `•` and every
        // box-drawing character are East Asian Ambiguous and render two columns under a CJK
        // locale, which in a three-column day cell shifts the rest of the week sideways.
        assert_eq!(DOT.width(), 1);
        assert_eq!(
            UnicodeWidthStr::width_cjk(DOT),
            1,
            "`{DOT}` widens under a CJK locale and would break the grid"
        );
        for day in WEEKDAYS {
            assert_eq!(day.width(), 2);
            assert_eq!(UnicodeWidthStr::width_cjk(day), 2);
        }
    }

    // ------------------------------------------------------------------------- the thread graph

    #[test]
    fn every_graph_glyph_is_one_column_in_every_locale() {
        // The same trap as the calendar's dot, in a gutter where it is worse: `undotree` offers a
        // Unicode set, and `●`, `○`, `│`, `╱`, `╲` and every box-drawing character in
        // U+2500–U+2573 are East Asian *Ambiguous*. Two columns under a CJK locale in a
        // fixed-width gutter is a frame that has come apart.
        for glyph in [
            GLYPH_NODE,
            GLYPH_FOCUS,
            GLYPH_LANE,
            GLYPH_FORK_RIGHT,
            GLYPH_FORK_LEFT,
            GLYPH_MORE,
        ] {
            let s = glyph.to_string();
            assert_eq!(s.width(), 1, "`{glyph}` is not one column");
            assert_eq!(
                UnicodeWidthStr::width_cjk(s.as_str()),
                1,
                "`{glyph}` widens under a CJK locale and would break the gutter"
            );
            assert!(glyph.is_ascii(), "`{glyph}` is not ASCII");
        }
    }

    #[test]
    fn a_gutter_narrower_than_the_thread_folds_rather_than_dropping_the_row() {
        // Past `MAX_LANES` the art degrades and the list does not. Whichever lane this row's note
        // is in, the row still says which row it is.
        let wide = vec![Lane::Through; 9];
        let folded = fold_gutter(&wide, MAX_LANES, false);
        assert_eq!(
            folded.chars().count(),
            MAX_LANES,
            "the gutter must be exact"
        );
        assert_eq!(folded, "|||||+", "the tail folds into one column");

        // The node is what the fold must never hide: it is the only thing saying where you are.
        let mut with_node = vec![Lane::Through; 9];
        with_node[7] = Lane::Node;
        assert_eq!(fold_gutter(&with_node, MAX_LANES, false), "|||||*");
        assert_eq!(fold_gutter(&with_node, MAX_LANES, true), "|||||@");

        // And a fold over nothing is a blank rather than a `+` promising lanes that are not there.
        let mut sparse = vec![Lane::Empty; 9];
        sparse[0] = Lane::Node;
        assert_eq!(fold_gutter(&sparse, MAX_LANES, false), "*     ");
    }

    #[test]
    fn a_gutter_inside_the_cap_is_drawn_verbatim_and_padded() {
        assert_eq!(fold_gutter(&[Lane::Node], 1, false), "*");
        assert_eq!(fold_gutter(&[Lane::Node], 3, false), "*  ");
        assert_eq!(
            fold_gutter(
                &[Lane::Through, Lane::ForkRight, Lane::ForkLeft, Lane::Empty],
                4,
                false
            ),
            "|\\/ "
        );
    }

    #[test]
    fn the_cap_leaves_a_title_worth_reading() {
        // The arithmetic the cap was chosen against: 20 columns inside the sidebar's borders, a
        // gutter, a gap, and what is left for the title. If either constant moves, this is what
        // says the title column has stopped being able to say anything.
        let inner = SIDEBAR_WIDTH as usize - 2;
        assert!(
            inner - (MAX_LANES + GRAPH_GAP) >= 13,
            "a {MAX_LANES}-lane gutter leaves {} columns of title",
            inner - (MAX_LANES + GRAPH_GAP)
        );
    }

    #[test]
    fn a_thread_that_fits_is_shown_whole_and_never_scrolls() {
        assert_eq!(graph_window(3, 0, 8), 0..3);
        assert_eq!(graph_window(8, 7, 8), 0..8);
        assert_eq!(graph_window(0, 0, 8), 0..0);
    }

    #[test]
    fn an_over_tall_thread_always_keeps_the_focused_note_on_screen() {
        // The one row that must always be visible. Swept across every position in a thread twice
        // the height of the pane, because a window that loses the focus is worse than no window.
        for height in 1..=8 {
            for focus in 0..40 {
                let window = graph_window(40, focus, height);
                assert_eq!(
                    window.len(),
                    height.min(40),
                    "height {height}, focus {focus}"
                );
                assert!(
                    window.contains(&focus),
                    "focus {focus} fell out of {window:?} at height {height}"
                );
                assert!(window.end <= 40);
            }
        }
    }

    #[test]
    fn an_over_tall_thread_scrolls_rather_than_being_cut_off_at_the_root() {
        // Centred on the focus, clamped at both ends: the top of a long thread shows the root,
        // the bottom shows the last node, and the middle shows context either side.
        assert_eq!(graph_window(20, 0, 6), 0..6);
        assert_eq!(graph_window(20, 10, 6), 7..13);
        assert_eq!(graph_window(20, 19, 6), 14..20);
    }

    #[test]
    fn the_sidebar_gives_the_calendar_a_fixed_height_and_the_graph_the_remainder() {
        let column = Rect {
            x: 0,
            y: 0,
            width: SIDEBAR_WIDTH,
            height: 23,
        };

        // Off: the calendar has the column to itself, which is the state the surface opens in.
        assert_eq!(split_sidebar(column, false), (column, None));

        // On, on a 24-row terminal minus the status line: 15 rows of calendar and 8 of graph.
        let (calendar, graph) = split_sidebar(column, true);
        assert_eq!(calendar.height, CALENDAR_ROWS + 2);
        assert_eq!(graph.map(|r| r.height), Some(23 - (CALENDAR_ROWS + 2)));

        // And a column too short to carry both keeps the month rather than drawing an empty box.
        let short = Rect {
            height: CALENDAR_ROWS + 2 + GRAPH_MIN_HEIGHT - 1,
            ..column
        };
        assert_eq!(split_sidebar(short, true), (short, None));
    }

    #[test]
    fn relative_time_is_coarse_and_never_negative() {
        let now = at("2026-09-04T12:00:00Z");

        assert_eq!(relative(at("2026-09-04T11:59:30Z"), now), "now");
        assert_eq!(relative(at("2026-09-04T11:30:00Z"), now), "30m");
        assert_eq!(relative(at("2026-09-04T07:00:00Z"), now), "5h");
        assert_eq!(relative(at("2026-09-02T12:00:00Z"), now), "2d");
        assert_eq!(relative(at("2026-08-04T12:00:00Z"), now), "4w");

        assert_eq!(
            relative(at("2026-09-05T12:00:00Z"), now),
            "ahead",
            "clock skew must not render as a negative age"
        );
    }

    #[test]
    fn truncation_counts_terminal_columns_not_characters() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 8), "hello w…");

        // Each of these is two columns wide. Eight columns therefore fits three of them plus the
        // ellipsis, not seven characters.
        let cjk = "안녕하세요반갑";
        let out = truncate(cjk, 8);
        assert!(
            out.width() <= 8,
            "`{out}` is {} columns, which would overflow the cell and break the border",
            out.width()
        );
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncation_of_a_zero_width_budget_is_empty_rather_than_a_lone_ellipsis() {
        assert_eq!(truncate("hello", 0), "");
    }

    #[test]
    fn the_marker_says_where_a_note_sits_in_its_thread() {
        let now = Utc.with_ymd_and_hms(2026, 9, 4, 12, 0, 0).unwrap();
        let glyph = |row: &Row| relation_marker(row, false).0;

        let mut row = fake_row(now);
        assert_eq!(glyph(&row), " ", "a note on its own gets no glyph at all");

        row.replies = 1;
        row.descendants = 1;
        assert_eq!(glyph(&row), "\u{2691}", "a head of something is flagged");

        let mut reply = fake_row(now);
        reply.parent = Some(Ref::Present(fake_row(now).note));
        assert_eq!(glyph(&reply), "\u{21b3}");

        // A reply that has replies of its own is still a reply: the flag is for heads, and its
        // own subtree is already announced by the count in the meta column.
        reply.replies = 2;
        reply.descendants = 2;
        assert_eq!(glyph(&reply), "\u{21b3}");
    }

    #[test]
    fn each_marker_glyph_means_exactly_one_thing() {
        let now = Utc.with_ymd_and_hms(2026, 9, 4, 12, 0, 0).unwrap();
        let id = fake_row(now).note.id;

        let mut trashed = fake_row(now);
        trashed.state = State::Trashed;
        assert_eq!(
            relation_marker(&trashed, false).0,
            "\u{232b}",
            "`⌫` is this note being in the trash"
        );
        assert_eq!(
            relation_marker(&trashed, true).0,
            " ",
            "and says nothing in the view where every row is trashed"
        );

        let mut dangling = fake_row(now);
        dangling.parent = Some(Ref::Deleted(id));
        let mut orphaned = fake_row(now);
        orphaned.parent = Some(Ref::Trashed(fake_row(now).note));
        assert_eq!(
            relation_marker(&dangling, false).0,
            relation_marker(&orphaned, false).0,
            "`⚠` is always the parent not being where it should be"
        );
        assert_ne!(
            relation_marker(&dangling, false).1,
            relation_marker(&orphaned, false).1,
            "and the colour is what says whether that is recoverable"
        );
    }

    #[test]
    fn every_marker_glyph_is_one_column_in_every_locale() {
        // The property that keeps a two-slot marker from breaking the frame. `◆`, `★`, `┬` and `¶`
        // would all fail this: they are East Asian Ambiguous and render two columns under a CJK
        // locale, which is the vault most likely to have wide titles already.
        for glyph in [
            "\u{232b}", "\u{26a0}", "\u{21b3}", "\u{2691}", "\u{276f}", "\u{275e}",
        ] {
            assert_eq!(glyph.width(), 1, "`{glyph}` is not one column");
            assert_eq!(
                UnicodeWidthStr::width_cjk(glyph),
                1,
                "`{glyph}` widens to two columns under a CJK locale"
            );
        }
    }

    #[test]
    fn the_meta_column_shows_a_fork_as_direct_over_total() {
        let now = Utc.with_ymd_and_hms(2026, 9, 4, 12, 0, 0).unwrap();
        let mut row = fake_row(now);

        row.replies = 0;
        row.descendants = 0;
        assert_eq!(
            meta_text(&row, now, MetaDetail::Full),
            "now",
            "a leaf shows only its age"
        );

        row.replies = 2;
        row.descendants = 2;
        assert_eq!(
            meta_text(&row, now, MetaDetail::Full),
            "2 ▸ now",
            "a flat thread shows one count"
        );

        row.replies = 2;
        row.descendants = 5;
        assert_eq!(
            meta_text(&row, now, MetaDetail::Full),
            "2/5 ▸ now",
            "a branching thread shows both, which is what makes a fork visible from the list"
        );
    }

    #[test]
    fn the_meta_column_counts_what_points_at_a_note() {
        let now = Utc.with_ymd_and_hms(2026, 9, 4, 12, 0, 0).unwrap();
        let mut row = fake_row(now);

        row.quoted = 1;
        assert_eq!(meta_text(&row, now, MetaDetail::Full), "1 ❞ now");

        // Both counts, in the order the eye reads them: what grew under it, then what points at it.
        row.replies = 2;
        row.descendants = 2;
        assert_eq!(meta_text(&row, now, MetaDetail::Full), "2 ▸ 1 ❞ now");
        assert_eq!(
            meta_text(&row, now, MetaDetail::AgeOnly),
            "now",
            "a narrow list keeps the age and drops the counts, never the other way round"
        );
    }

    /// A `Row` with the fields these tests care about and defaults elsewhere.
    fn fake_row(now: DateTime<Utc>) -> Row {
        use jot_core::note::{NoteId, NoteMeta};
        let id: NoteId = "01a03d60-0000-7000-8000-00000000000a".parse().unwrap();
        Row {
            note: NoteMeta {
                id,
                created_at: Some(now),
                title: Some("t".into()),
                root: Some(id),
                reply_to: None,
                quote: None,
            },
            state: State::Active,
            parent: None,
            replies: 0,
            descendants: 0,
            quoted: 0,
            edited_at: None,
        }
    }
}
