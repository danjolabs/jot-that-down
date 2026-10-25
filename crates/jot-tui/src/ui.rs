//! Rendering. Pure: every function here takes state and produces cells, and touches nothing else.
//!
//! Keeping the draw path free of side effects is what makes the snapshot tests worth having — a
//! rendered frame is a function of [`App`], so a diff in the snapshot is a real visual change and
//! never a timing artefact.

use std::collections::BTreeSet;

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Utc};
use jot_core::query::{FileSort, Ref, Row, State};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Paragraph, Row as TableRow, Table, TableState,
};
#[cfg(test)]
use unicode_width::UnicodeWidthChar;
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
/// Seven day cells of [`DAY_CELL`] columns each — 28 — plus the borders. Fixed rather than a
/// fraction of the frame because a calendar is a *grid*: one column narrower it is not a narrower
/// month, it is a week with a day sliced off the end, and every row below the header lands one
/// column out from the row above it.
///
/// It was 22, with three-column cells. The user widened it to 30 (2026-10-25) so the grid could
/// breathe and the thread graph under it could keep a readable title beside a six-lane gutter; see
/// [`MAX_LANES`]. The eight columns come out of the reader, never the list: [`split_main`] still
/// gives the list its 40–56, and only the threshold at which the reader fits moved.
const SIDEBAR_WIDTH: u16 = 30;

/// Narrower than this and the frame carries no sidebar.
///
/// [`LIST_MIN`] one pane out: the 30 columns of sidebar plus the 40 a list needs to still be a
/// list. Below this the sidebar is the thing to drop, for the same reason the reader is dropped
/// below [`READER_MIN_FRAME`] — the list is what the surface is *for*, and a month is context
/// beside it.
const SIDEBAR_MIN_FRAME: u16 = SIDEBAR_WIDTH + LIST_MIN;

/// Where each pane landed, with `None` for one this frame could not carry.
///
/// The list is not optional: it is the surface. Whatever else goes, it gets what is left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneAreas {
    /// The sidebar — calendar, and thread graph under it — when there is room for it.
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
/// | 120+ | 30 | 40–56 | the rest |
/// | 70–119 | 30 | the rest | — |
/// | under 70 | — | the whole frame | — |
///
/// (Hide the sidebar and the reader comes back at 90, because the main pane is the frame again.)
///
/// # A toggle may only ever remove a pane
///
/// `panes` says what the user *wants*, and this function is what decides. Wanting a pane the
/// width cannot carry gets nothing: pressing the reader's key at 60 columns must not paint a
/// two-column panel over the list. The automatic rule wins, and the toggles are an escape hatch in
/// the one direction — 30 columns of sidebar is well over a third of an 80-column terminal, and
/// there has to be a way to spend it on the list instead.
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
/// **Answers for both side panes wanted** — the reader beside the sidebar — so `None` here means
/// the width rule has no room for a reader, never that one was closed. That is not the default:
/// the reader starts closed (see [`Panes::default`]), and a default-pane answer would be `None` at
/// every size. Anything holding the app's own [`Panes`] — the run loop does — should ask
/// [`reader_text_width_for`] instead.
#[must_use]
pub fn reader_text_width(frame_width: u16, frame_height: u16) -> Option<u16> {
    reader_text_width_for(EVERY_PANE, frame_width, frame_height)
}

/// Both side panes wanted: the most a frame is ever asked to carry.
const EVERY_PANE: Panes = Panes {
    sidebar: true,
    reader: true,
};

/// [`reader_text_width`], for a caller that knows which panes are wanted.
///
/// The sidebar is 30 columns of the reader's budget, so the two questions have different answers
/// the moment it is hidden — and between 90 and 120 columns they differ about whether there is a
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
/// *Ambiguous* and render two columns under a CJK locale, which in a four-column day cell shifts
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
/// Monday first: ISO 8601's week, and the one [`chrono`] counts from. Each is centred in a
/// [`DAY_CELL`]-column cell, so the header is exactly 28 columns — which is [`SIDEBAR_WIDTH`] less
/// its borders.
const WEEKDAYS: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];

/// Columns one day of the grid takes: a blank, the two-column day number, a blank.
///
/// The blanks are the grid's separators, split either side of the number rather than all on one
/// side, so the week sits centred in the pane with one column of air against each border. The dot
/// row uses the same cell, with the dot under the number's units digit.
const DAY_CELL: usize = 4;

/// The sidebar: the month, and the thread graph under it.
///
/// `now` is the frame's clock and `app.zone()` the offset it is read in — the same discipline as
/// everywhere else here. This module never asks the operating system what time it is or where it
/// is: a rendered frame is a function of the state handed to it, which is what makes a snapshot
/// worth taking. See [`App::zone`](crate::app::App::zone) for who supplies the offset.
fn draw_sidebar(frame: &mut Frame, area: Rect, app: &App, now: DateTime<Utc>) {
    let (calendar, graph) = split_sidebar(area);
    draw_calendar(frame, calendar, app, now);
    if let Some(graph) = graph {
        draw_graph(frame, graph, app);
    }
}

/// The shortest a graph panel is worth drawing: two borders and a single row of content.
///
/// Below this the calendar keeps the whole column. A bordered box with nothing inside it is an
/// affordance that says a thing is here and then does not show it, which is worse than no box.
const GRAPH_MIN_HEIGHT: u16 = 3;

/// Cut the sidebar into the calendar and the graph beneath it.
///
/// The calendar's height is **fixed** at [`CALENDAR_ROWS`] plus its borders rather than shared
/// proportionally, and that is the whole reason the loser of the two-calendar comparison had to
/// go: a grid whose height moves is a grid that has to be re-found every time the pane resizes,
/// and everything below it moves with it. So the month takes exactly what a month needs and the
/// graph takes the remainder, which on a 24-row terminal — minus the status line — is eight rows.
///
/// # Always there, except where it cannot be
///
/// There is no key for the graph: the user made it permanent on 2026-10-25, reversing the toggle
/// `stage6.md` had settled on. Two things still take it away, and neither is a want. The sidebar's
/// own key hides the whole column, graph included, through [`split_frame`]. And the graph is
/// dropped when the remainder is under [`GRAPH_MIN_HEIGHT`], by the same rule the frame drops the
/// reader and then the sidebar: the automatic decision wins, and on a short terminal the month
/// keeps the column rather than sharing it with an empty box.
fn split_sidebar(area: Rect) -> (Rect, Option<Rect>) {
    let calendar_height = CALENDAR_ROWS + 2;
    if area.height < calendar_height + GRAPH_MIN_HEIGHT {
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

/// The weekday header row, and the top of the grid: each day centred in its [`DAY_CELL`].
fn weekday_header() -> Line<'static> {
    let header: String = WEEKDAYS.iter().map(|day| format!(" {day} ")).collect();
    Line::from(Span::styled(header, dim()))
}

/// The calendar: the dot on its own row, directly under the day it belongs to.
///
/// Two rows per week — the numbers, then the dots — for [`CALENDAR_ROWS`] in total. Every day is
/// one [`DAY_CELL`]: a blank, the number right-aligned in two columns, a blank. The dot sits under
/// the units digit, which is where the eye is already looking on a right-aligned number, and an
/// empty day is blank rather than a placeholder: a grid of "no" marks says nothing and reads as
/// noise.
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
        for cell in week {
            match cell {
                None => {
                    numbers.push(Span::raw(" ".repeat(DAY_CELL)));
                    dots.push(Span::raw(" ".repeat(DAY_CELL)));
                }
                Some(day) => {
                    // Only the number is styled, so today's reverse video is the two digits and
                    // not the cell's blanks — which belong to the gaps between days, not to a day.
                    numbers.push(Span::raw(" "));
                    numbers.push(Span::styled(
                        format!("{:>2}", day.day()),
                        day_style(day, today),
                    ));
                    numbers.push(Span::raw(" "));
                    dots.push(Span::styled(
                        if days.contains(&day) {
                            format!("  {DOT} ")
                        } else {
                            " ".repeat(DAY_CELL)
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
/// Today is reversed rather than bracketed or arrowed: the blanks either side of a number are the
/// gaps between days, so a bracket there would read as belonging to a neighbour, and every glyph
/// that would fit is one column of the grid that some locale renders as two. Reverse video costs
/// nothing and cannot move a column.
fn day_style(day: NaiveDate, today: NaiveDate) -> Style {
    if day == today {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    }
}

// ============================================================================== the thread graph

/// The graph's glyphs: box-drawing, so the lines join up, in the curved style `jj log` draws.
///
/// `●` a note, `◉` the focused one, `│` a lane. On a connector row, the forking note's lane is
/// `├`, `┤` or `┼` as its forks lie right, left or both; `─` is the horizontal, `┼` where it crosses
/// a lane passing through; `╮` and `╭` are the outermost fork on each side and `┬` any fork short
/// of it. `┊` is the fold described in [`MAX_LANES`]. Which of these a lane gets is decided in
/// [`lane_glyph`], from the row's topology.
///
/// # Why not ASCII, which this was
///
/// The gutter was `* @ | / \ +`, on the argument that `●` and every box-drawing character are East
/// Asian **Ambiguous** — two columns under a CJK locale — where ASCII is Narrow everywhere. The
/// argument was true and protected nothing: the panel's own border, `┌─│┐`, is the same Ambiguous
/// set. A terminal that renders Ambiguous as wide has already broken the frame the gutter sits in,
/// and an ASCII gutter inside a broken frame is not a working panel. So the user chose seamless
/// lines over it, 2026-10-25. (`◉` is Neutral; `●` and the box-drawing set are Ambiguous.)
///
/// What is still asserted, in `every_graph_glyph_is_one_column`, is that each glyph is one column
/// at the default width — which is what catches a stray emoji or wide character in a fixed gutter.
const GLYPH_NODE: char = '●';
/// See [`GLYPH_NODE`]. The focused note, so "where am I" is answerable without reading colour.
const GLYPH_FOCUS: char = '◉';
/// See [`GLYPH_NODE`].
const GLYPH_LANE: char = '│';
/// See [`GLYPH_NODE`]. The horizontal on a connector row, in a lane column or a spacer.
const GLYPH_RULE: char = '─';
/// See [`GLYPH_NODE`]. A horizontal crossing a lane, or a branch with forks on both sides.
const GLYPH_CROSS: char = '┼';
/// See [`GLYPH_NODE`]. The forking note's lane, every fork to its right.
const GLYPH_BRANCH_RIGHT: char = '├';
/// See [`GLYPH_NODE`]. The forking note's lane, every fork to its left.
const GLYPH_BRANCH_LEFT: char = '┤';
/// See [`GLYPH_NODE`]. The rightmost fork, where the horizontal turns down.
const GLYPH_FORK_RIGHT: char = '╮';
/// See [`GLYPH_NODE`]. The leftmost fork, where the horizontal turns down.
const GLYPH_FORK_LEFT: char = '╭';
/// See [`GLYPH_NODE`]. A fork the horizontal carries on past.
const GLYPH_FORK_MID: char = '┬';
/// See [`GLYPH_NODE`] and [`MAX_LANES`].
const GLYPH_MORE: char = '┊';

/// Columns one lane takes: its glyph, then a spacer.
///
/// The spacer is what a connector's horizontal runs through between two lanes, and a space on
/// every other row — so the lines read as lines rather than a solid block of vertical strokes.
const LANE_WIDTH: usize = 2;

/// Columns between the gutter and the title.
const GRAPH_GAP: usize = 1;

/// The widest the gutter is drawn, however many lanes the thread actually has.
///
/// # Why there has to be a cap
///
/// The gutter is [`LANE_WIDTH`] columns per live lane and the number of live lanes is unbounded in
/// principle: a note with forty replies opens forty lanes. The sidebar has **28 columns inside its
/// borders**, which is the whole budget for gutter, gap and title together, so an uncapped gutter
/// is a title column that can reach zero — and a graph whose labels have been eaten by its own art
/// is not a minimap, it is a decoration.
///
/// # Why six
///
/// Six lanes of two columns plus the gap is 6 × 2 + 1 = 13 columns, which leaves 15 of the 28 for
/// the title — two or three short words, and more than the 13 the original one-column lanes left in
/// the old 20. Below about that a title stops distinguishing anything and the panel stops answering
/// the question it exists for. Six simultaneous open branches in one conversation is also well past
/// anything the vault this is dogfooded against has produced, so the cap is a guard rather than a
/// routine truncation.
///
/// The history, because the number moved twice in a day: six while a lane was one column wide;
/// four when the spacer that makes the lines join halved what 20 columns hold; six again when the
/// sidebar grew to 30 (2026-10-25).
///
/// # What happens past it
///
/// The lanes at and beyond the last drawn column **fold into that column**, and nothing is
/// dropped: every note still gets its row and its title. The folded column shows the node glyph
/// when this row's note is out there, [`GLYPH_MORE`] when some other lane is, and a blank when
/// nothing is. A connector's horizontal that runs past the cap runs into the fold and stops there.
/// So the *art* degrades and the *list* does not — which is the right way round, because the list
/// is what carries the titles and the focus.
const MAX_LANES: usize = 6;

/// The thread graph: the focused note's whole thread as a lane gutter and one title per node.
///
/// **This is a minimap, not a reading view.** Twenty-eight columns hold a gutter and a truncated
/// title and nothing else; it answers "where am I in this thread, and what else is in it" and
/// refers everything past that to the reader panel, which is already showing the note. Trying to
/// make it read — wrapping titles, showing bodies, adding a meta column — is how a 28-column pane
/// becomes unreadable in both jobs.
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
    // widest row decides it, capped; a linear thread is therefore one lane — a glyph and its
    // spacer — and the gap, three columns before the title.
    let gutter = rows
        .iter()
        .map(|row| row.lanes.len())
        .max()
        .unwrap_or(1)
        .clamp(1, MAX_LANES);
    let title_width = inner_width.saturating_sub(gutter * LANE_WIDTH + GRAPH_GAP);

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
                        " ".repeat(gutter * LANE_WIDTH + GRAPH_GAP),
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

/// One row: the gutter, a space, and as much of the title as is left. `gutter` counts lanes.
fn graph_line(row: &GraphRow, gutter: usize, title_width: usize) -> Line<'static> {
    let focused = row.node.as_ref().is_some_and(|node| node.is_focus);
    let mut spans = vec![
        Span::styled(fold_gutter(&row.lanes, gutter, focused), gutter_style()),
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

/// How lines and the focus mark are painted, in the graph's gutter and the list's selection lane
/// alike — one style, so the two panes draw the same thing the same way.
fn gutter_style() -> Style {
    dim()
}

/// Render `lanes` into exactly `width` lanes — `width * LANE_WIDTH` columns — folding anything past
/// the last one.
///
/// Each lane is its glyph and then a spacer. The spacer after lane `i` is the horizontal when the
/// row's connector span runs on past `i`, and a space otherwise; the spacer after the last drawn
/// lane is always a space, because nothing is to its right.
///
/// See [`MAX_LANES`] for what the fold means and why it is a fold rather than a truncation.
fn fold_gutter(lanes: &[Lane], width: usize, focused: bool) -> String {
    let node = if focused { GLYPH_FOCUS } else { GLYPH_NODE };
    // The last drawn lane, when the lanes at and past it have to fold into it.
    let fold = (lanes.len() > width).then(|| width.saturating_sub(1));
    // A horizontal that runs past the cap runs into the fold and stops there.
    let span = connector_span(lanes).map(|(lo, hi)| match fold {
        Some(fold) => (lo.min(fold), hi.min(fold)),
        None => (lo, hi),
    });

    let mut out = String::new();
    for at in 0..width {
        out.push(if fold == Some(at) {
            let rest = &lanes[at..];
            if rest.contains(&Lane::Node) {
                node
            } else if rest.iter().all(|lane| *lane == Lane::Empty) {
                ' '
            } else {
                GLYPH_MORE
            }
        } else {
            lane_glyph(lanes, at, span, node)
        });
        let runs_on = span.is_some_and(|(lo, hi)| lo <= at && at < hi);
        out.push(if runs_on && at + 1 < width {
            GLYPH_RULE
        } else {
            ' '
        });
    }
    out
}

/// The leftmost and rightmost lane a connector row's horizontal touches: every `Branch` and fork on
/// the row. `None` for a note row, which has no horizontal.
fn connector_span(lanes: &[Lane]) -> Option<(usize, usize)> {
    let mut joins = (0..lanes.len())
        .filter(|&at| matches!(lanes[at], Lane::Branch | Lane::ForkLeft | Lane::ForkRight));
    let lo = joins.next()?;
    Some((lo, joins.next_back().unwrap_or(lo)))
}

/// How lane `at` is painted, given the row it is on and that row's connector span.
///
/// A lane strictly inside the span is crossed by the horizontal: a lane passing through becomes a
/// crossing and an empty one becomes the horizontal itself. The `Branch` points at its forks, and a
/// fork curves down at the end of the span and tees off it short of the end. See [`GLYPH_NODE`] for
/// the width rule these all obey.
fn lane_glyph(lanes: &[Lane], at: usize, span: Option<(usize, usize)>, node: char) -> char {
    let inside = span.is_some_and(|(lo, hi)| lo < at && at < hi);
    let is_fork = |lane: &Lane| matches!(lane, Lane::ForkLeft | Lane::ForkRight);
    match lanes.get(at).copied().unwrap_or(Lane::Empty) {
        Lane::Empty if inside => GLYPH_RULE,
        Lane::Empty => ' ',
        Lane::Through if inside => GLYPH_CROSS,
        Lane::Through => GLYPH_LANE,
        Lane::Node => node,
        // Which side its forks are on is read from the whole row, folded lanes included: a fork
        // past the cap is still a fork, and the horizontal still leaves towards it.
        Lane::Branch => match (
            lanes[..at].iter().any(is_fork),
            lanes[at + 1..].iter().any(is_fork),
        ) {
            (true, true) => GLYPH_CROSS,
            (true, false) => GLYPH_BRANCH_LEFT,
            (false, true) => GLYPH_BRANCH_RIGHT,
            // Not something the sweep draws — a connector exists because there is a fork — and a
            // line with nothing to branch to is still a line.
            (false, false) => GLYPH_LANE,
        },
        Lane::ForkRight if span.is_some_and(|(_, hi)| at == hi) => GLYPH_FORK_RIGHT,
        Lane::ForkLeft if span.is_some_and(|(lo, _)| at == lo) => GLYPH_FORK_LEFT,
        Lane::ForkRight | Lane::ForkLeft => GLYPH_FORK_MID,
    }
}

/// The row table, which serves all four `Row` views.
///
/// A real [`Table`] with a header row (user decision, 2026-10-25), not a list of pre-padded lines.
/// The columns are marker, id, title, counts, `created` and `edited`, and their widths are still
/// decided here, against the rows on screen, by [`list_columns`]; the table only lays out what that
/// decided, as `Constraint::Length` columns with no spacing of its own.
///
/// Two times rather than one age, as `stage6.md` settled on 2026-09-09: `created` as an absolute
/// date, the fact that does not change, and `edited` as a relative age, read as "how stale is
/// this". One relative age from `created_at` left the files view's `edited` sort ordering rows by
/// a value nobody could see.
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

    let inner = area.width.saturating_sub(2) as usize;
    // The calendar's zone, so a note's `created` day is the day its dot is on.
    let zone = app.zone();

    // Every column is measured against the rows actually on screen rather than fixed, and against
    // its own header, so a label is never clipped by the data under it. An id is thirteen
    // characters — the whole millisecond timestamp, which is where the TUI has floored since stage
    // 5 and where `jot ls` now floors too — until a burst of notes shares a millisecond and forces
    // a fourteenth; a counts cell is empty for a lone note and seven characters for a quoted
    // branching thread; a `created` date is `Oct 25` this year and `Oct 2025` before it. Sizing to
    // the widest present is what keeps the times beside the title instead of a fixed guess away
    // from it — which was the visible complaint: a right-aligned column in an 80-column frame puts
    // the time an inch of whitespace from the title it belongs to.
    let widest =
        |measure: &dyn Fn(&Row) -> usize| app.rows().iter().map(measure).max().unwrap_or(0);
    let counts = widest(&|row| counts_text(row).width());
    let natural = Natural {
        id: widest(&|row| app.short_id(row.note.id).width()).max(HEADER_ID.width()),
        title: widest(&|row| title_of(row).width()).max(HEADER_TITLE.width()),
        // A view where no row has a count gets no counts column at all — not an empty one with a
        // label over it, which would spend seven columns announcing nothing.
        counts: if counts == 0 {
            0
        } else {
            counts.max(HEADER_COUNTS.width())
        },
        created: widest(&|row| created_text(row, zone, now).width()).max(HEADER_CREATED.width()),
        edited: widest(&|row| edited_text(row, now).width()).max(HEADER_EDITED.width()),
    };
    let columns = list_columns(natural, inner, ordered_by(app.view(), app.sort()));
    let in_trash = app.view() == ViewKind::Trash;
    let selected = app.selected();
    let parent = parent_row(app.rows(), selected);

    let rows: Vec<TableRow> = app
        .rows()
        .iter()
        .enumerate()
        .map(|(at, row)| {
            let id = app.short_id(row.note.id);
            let link = link_glyphs(at, selected, parent);
            table_row(row, &id, columns, link, in_trash, (zone, now))
        })
        .collect();

    let table = Table::new(rows, columns.constraints())
        .header(header_row(columns))
        .column_spacing(0)
        .block(block)
        .row_highlight_style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Cyan),
        );

    // Rebuilt every frame from the app's selection, as the list's state was: the offset it scrolls
    // to is a function of the selection and the pane's height, and the table works it out with the
    // header's row already taken off the height — so the selected row is always on screen below
    // it.
    let mut state = TableState::default().with_selected(Some(app.selected()));
    frame.render_stateful_widget(table, area, &mut state);
}

/// Columns the leading marker occupies: the selection lane's line and its end, a relation slot, a
/// quote slot, and a space.
///
/// ```text
/// ╭◉↳  01a1736a-b5b0  Ch. 3 notes: LSM trees vs…   ← selected
/// │ ↳  01a1736a-6164  The Pragmatic Programmer
/// │ ↳  01a1736a-615f  A Philosophy of Software …
/// ╰►↳  01a1736a-615b  Designing Data-Intensive …   ← its parent
///   ⚑  01a1736a-6156  Reading list
/// ```
///
/// 1. **Line** — `╭`, `│` or `╰`: the one link from the selected note to its parent, drawn by
///    [`link_glyphs`].
/// 2. **End** — `◉` on the selected row, `►` on its parent's.
/// 3. **Relation** — one glyph from [`relation_marker`].
/// 4. **Quote** — `❯` when the note quotes another, so a reply that quotes reads `↳❯`.
///
/// Every slot is reserved on every row, including the rows that fill none of them. Letting a glyph
/// slide left when the slot before it is empty would put the same symbol in different columns on
/// different rows, and a column you have to *read* to locate is not a column.
///
/// `⌫`, `⚠`, `↳`, `⚑`, `❯`, `◉` and `►` are East Asian **Neutral**. `╭`, `│` and `╰` are
/// **Ambiguous**, which the user accepted on 2026-10-25 by the same rule as the thread graph and
/// the frame borders: a terminal that widens box-drawing has already widened the frame. Each
/// character is still one column at the default width, which the test checks rather than claiming
/// the marker is CJK-narrow.
const MARKER_WIDTH: usize = 5;

/// The selection lane's line where it starts, on whichever of the two rows is higher.
const LINK_TOP: char = '╭';
/// The selection lane's line where it ends, on whichever of the two rows is lower.
const LINK_BOTTOM: char = '╰';
/// The selection lane's end on the parent's row. The selected row's is [`GLYPH_FOCUS`], the mark
/// the thread graph puts on the same note.
const LINK_PARENT: char = '►';

/// The row in `rows` holding the parent of the row at `selected`, if it is one of them.
///
/// `None` for a note with no parent, and for a parent the list is not showing — trashed, filtered
/// out by a search, or not a root in the thread-roots timeline. A line has to end on a row, and
/// there is no row to end on.
fn parent_row(rows: &[Row], selected: usize) -> Option<usize> {
    let parent = rows.get(selected)?.parent.as_ref()?.id();
    rows.iter()
        .position(|row| row.note.id == parent)
        // A hand-edited note can name itself as its parent. That is a line of no length, and
        // drawing it would paint a top corner with nothing under it.
        .filter(|&at| at != selected)
}

/// The selection lane's two columns on row `at`: the line, then its end.
///
/// **One link only** (user decision): the selected note to its parent. Its replies get no line,
/// and every other row shows only its own markers. The corners point at each other —
/// the higher row of the two gets [`LINK_TOP`] and the lower [`LINK_BOTTOM`] — so a parent above
/// the selection, as in the files view sorted by title, draws the same shape upside down.
///
/// Worked out against the whole row list rather than the rows on screen, which is what makes a
/// parent scrolled out of view still read: the visible part of the line runs to the pane's edge,
/// pointing the way the parent is. `◉` marks the selected row whether or not there is a line.
fn link_glyphs(at: usize, selected: usize, parent: Option<usize>) -> [char; 2] {
    let end = if at == selected {
        GLYPH_FOCUS
    } else if Some(at) == parent {
        LINK_PARENT
    } else {
        ' '
    };
    let line = match parent.map(|parent| (selected.min(parent), selected.max(parent))) {
        Some((top, _)) if at == top => LINK_TOP,
        Some((_, bottom)) if at == bottom => LINK_BOTTOM,
        Some((top, bottom)) if top < at && at < bottom => GLYPH_LANE,
        _ => ' ',
    };
    [line, end]
}

/// The relation slot: one column saying where this note sits, or what is wrong with where it sits.
///
/// The third of the marker's columns; see [`MARKER_WIDTH`]. Dim, like the quote slot after it —
/// except for the two warnings, which are coloured because they are the ones to act on.
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

/// Columns in front of every column after the title — counts, `created`, `edited`.
///
/// A floor, not a target. With the title column sized to content, the longest title would
/// otherwise touch its own date — `a long titleOct 25` — which is not a tighter layout but an
/// unreadable one. In front of the column rather than after the one before it, because these
/// columns are right-aligned and a trailing gap would be swallowed by the alignment.
const META_GAP: usize = 2;

/// The widest a title column grows before the columns after it stop following it.
///
/// Without a cap the times are pushed to the frame's right edge, which on a wide terminal strands
/// them a long way from the title they describe and makes the pair hard to read as one row. Past
/// this the row simply ends and the rest of the line stays empty.
const TITLE_MAX: usize = 44;

/// The header row's labels. The marker column has none: its glyphs explain themselves in `?`, and
/// a label over its four glyph columns would be wider than the column.
const HEADER_ID: &str = "id";
/// See [`HEADER_ID`].
const HEADER_TITLE: &str = "title";
/// See [`HEADER_ID`]. "counts" rather than "replies": the column carries the quote count beside
/// the reply count, and a label naming only one of them would misname the other.
const HEADER_COUNTS: &str = "counts";
/// See [`HEADER_ID`].
const HEADER_CREATED: &str = "created";
/// See [`HEADER_ID`].
const HEADER_EDITED: &str = "edited";

/// Which of the two times a view is ordered by — and so the time column it keeps longest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Time {
    Created,
    Edited,
}

/// The time a view's rows are ordered by.
///
/// Only the files view's `edited` sort is ordered by edit time. The timeline, the files view's
/// other sorts — `title` included, which orders by neither and keeps the time that does not change
/// — and search and trash all count as created.
fn ordered_by(view: ViewKind, sort: FileSort) -> Time {
    match view {
        ViewKind::Files if sort == FileSort::Edited => Time::Edited,
        _ => Time::Created,
    }
}

/// How wide each column would like to be: the widest cell on screen, or its header if that is
/// wider. `counts` is zero when no row has a count, which means no counts column at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Natural {
    id: usize,
    title: usize,
    counts: usize,
    created: usize,
    edited: usize,
}

/// The table's column widths, decided once for the whole frame.
///
/// Passed as one value rather than five: they are a single decision — every one of them is chosen
/// against the others and against the frame's width — and splitting them across a parameter list
/// invites a caller to compute one of them somewhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Columns {
    /// Width of the id, before its [`ID_GAP`].
    id: usize,
    /// Width of the title.
    title: usize,
    /// Width of the counts, or `None` when the column is dropped or there is nothing to count.
    counts: Option<usize>,
    /// Width of `created`, or `None` when it is dropped.
    created: Option<usize>,
    /// Width of `edited`, or `None` when it is dropped.
    edited: Option<usize>,
}

impl Columns {
    /// The columns after the title, in the order they are drawn, each with its width when shown.
    fn after_title(self) -> [Option<usize>; 3] {
        [self.counts, self.created, self.edited]
    }

    /// The table's constraints: one `Length` per column, the id's gap after it and every
    /// right-aligned column's [`META_GAP`] in front of it.
    fn constraints(self) -> Vec<Constraint> {
        let length = |n: usize| Constraint::Length(u16::try_from(n).unwrap_or(u16::MAX));
        let mut widths = vec![
            length(MARKER_WIDTH),
            length(self.id + ID_GAP),
            length(self.title),
        ];
        widths.extend(
            self.after_title()
                .into_iter()
                .flatten()
                .map(|width| length(META_GAP + width)),
        );
        widths
    }

    /// Columns the whole row occupies. What `list_columns` keeps at or under the pane's width, and
    /// what the tests hold it to.
    #[cfg(test)]
    fn total(self) -> usize {
        let after: usize = self
            .after_title()
            .into_iter()
            .flatten()
            .map(|width| META_GAP + width)
            .sum();
        MARKER_WIDTH + self.id + ID_GAP + self.title + after
    }
}

/// Decide the column widths for a pane `inner` columns wide.
///
/// # The arithmetic has to close exactly
///
/// A row is `MARKER + id + gap + title`, then `gap + width` for each column after the title, and
/// that must total *at most* the block's inner width. Wider and the table squeezes columns to fit,
/// which takes width from the times — invisible in a snapshot, because the trailing spaces get
/// trimmed and it simply looks as though notes have no date. [`Columns::total`] is that sum and
/// the tests hold it to `inner`.
///
/// # The title is sized to the titles, not to the space
///
/// Filling the width was what stranded the time an inch of whitespace away from the title it
/// describes: a column of five-character titles in a fifty-column list put every time at column
/// fifty. Sized to content and capped at [`TITLE_MAX`], the columns after it follow the titles and
/// the rest of the row stays empty.
///
/// # What goes first when it does not fit
///
/// 1. **The counts.** They are the one part of a row that is inferable from elsewhere — a thread's
///    size is visible the moment you open it.
/// 2. **The time the view is not ordered by** — see [`ordered_by`]. A list sorted by edit time is
///    read down its `edited` column, and a date nobody sorted on is the next least needed.
/// 3. **Then the title is clipped.** The time the view *is* ordered by stays as long as there is
///    room for any title at all; it is the column the order is about, and never inferable.
///
/// Each step happens before the next is touched by a single column: the counts go before the other
/// time is dropped, and both go before the title loses anything. Same shape as the key bar dropping
/// labels before it drops keys.
fn list_columns(natural: Natural, inner: usize, ordered: Time) -> Columns {
    let avail = inner.saturating_sub(MARKER_WIDTH + natural.id + ID_GAP);
    let title = natural.title.min(TITLE_MAX);
    let cost = |width: usize| if width == 0 { 0 } else { META_GAP + width };
    let (kept, other) = match ordered {
        Time::Created => (natural.created, natural.edited),
        Time::Edited => (natural.edited, natural.created),
    };

    let both_times = cost(kept) + cost(other);
    let counts = (natural.counts > 0 && title + cost(natural.counts) + both_times <= avail)
        .then_some(natural.counts);
    let show_other = counts.is_some() || title + both_times <= avail;
    let used = counts.map_or(0, cost) + cost(kept) + if show_other { cost(other) } else { 0 };

    let (created, edited) = match ordered {
        Time::Created => (Some(natural.created), show_other.then_some(natural.edited)),
        Time::Edited => (show_other.then_some(natural.created), Some(natural.edited)),
    };
    Columns {
        id: natural.id,
        title: title.min(avail.saturating_sub(used)),
        counts,
        created,
        edited,
    }
}

/// The header: a blank over the marker, then a label per column, aligned as the column's cells
/// are. Bold and dim, so it reads as furniture rather than as the first note; no separator row,
/// which would cost a row of a pane that is often only ten tall.
fn header_row(columns: Columns) -> TableRow<'static> {
    let mut cells = vec![
        Cell::from(""),
        Cell::from(truncate(HEADER_ID, columns.id)),
        Cell::from(truncate(HEADER_TITLE, columns.title)),
    ];
    let labels = [HEADER_COUNTS, HEADER_CREATED, HEADER_EDITED];
    for (label, width) in labels.into_iter().zip(columns.after_title()) {
        if let Some(width) = width {
            cells.push(Cell::from(
                Line::from(truncate(label, width)).right_aligned(),
            ));
        }
    }
    TableRow::new(cells).style(Style::default().add_modifier(Modifier::BOLD | Modifier::DIM))
}

/// One row: marker, id, title, then the counts and the two times, right-aligned in their columns.
///
/// `link` is this row's two selection-lane columns, from [`link_glyphs`]. The last argument is the
/// zone `created` is read in and the instant `edited` is measured from.
fn table_row(
    row: &Row,
    id: &str,
    columns: Columns,
    [line, end]: [char; 2],
    in_trash: bool,
    (zone, now): (FixedOffset, DateTime<Utc>),
) -> TableRow<'static> {
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

    // Truncated as well as sized. Every column is at least as wide as its widest cell, so this can
    // only fire on the title, or on a time in a pane too narrow to hold even that — but a cell
    // wider than its column is clipped by the table, and a clipped time is invisible in a snapshot.
    let mut cells = vec![
        Cell::from(Line::from(vec![
            Span::styled(String::from_iter([line, end]), gutter_style()),
            Span::styled(relation, relation_style),
            Span::styled(quote, dim()),
            Span::raw(" "),
        ])),
        // Yellow, and ahead of the title, exactly as `jot ls` prints it. An id you can read off the
        // browser and paste into `jot show` is the whole reason it is here, and it only reads as
        // the same id if it looks like the same id.
        Cell::from(Span::styled(
            id.to_string(),
            Style::default().fg(Color::Yellow),
        )),
        Cell::from(Span::styled(
            truncate(&title_of(row), columns.title),
            title_style,
        )),
    ];
    let texts = [
        counts_text(row),
        created_text(row, zone, now),
        edited_text(row, now),
    ];
    for (text, width) in texts.into_iter().zip(columns.after_title()) {
        if let Some(width) = width {
            cells.push(Cell::from(
                Line::from(Span::styled(truncate(&text, width), dim())).right_aligned(),
            ));
        }
    }
    TableRow::new(cells)
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

/// The counts column: reply count, then quote count. Empty for a note nothing has grown around.
fn counts_text(row: &Row) -> String {
    let mut parts = Vec::new();
    if row.replies > 0 {
        if row.replies == row.descendants {
            parts.push(format!("{} \u{25b8}", row.replies));
        } else {
            // Direct replies and the whole subtree differ, which means the thread branches.
            // Showing both is what makes a fork visible from the list rather than only after
            // opening it.
            parts.push(format!("{}/{} \u{25b8}", row.replies, row.descendants));
        }
    }
    // How many notes point *at* this one. A count rather than a list, and here rather than in the
    // marker, because it is the same kind of fact as the reply count and belongs beside it — the
    // marker says what this note is, the counts say what has accumulated around it.
    if row.quoted > 0 {
        parts.push(format!("{} \u{275e}", row.quoted));
    }
    parts.join(" ")
}

/// The `created` column: the day the note was written, in `zone`, as an absolute date — `Oct 25`
/// within the year `now` is in, `Oct 2025` before it. `—` for a note with no recoverable creation
/// time.
///
/// The day only, not the hour: the column answers "which day", the same question the calendar's
/// dot answers, and in the same zone so the two never disagree about a note near midnight.
fn created_text(row: &Row, zone: FixedOffset, now: DateTime<Utc>) -> String {
    let Some(created) = row.note.created_at else {
        return "—".to_string();
    };
    let created = created.with_timezone(&zone);
    if created.year() == now.with_timezone(&zone).year() {
        created.format("%b %e").to_string()
    } else {
        created.format("%b %Y").to_string()
    }
}

/// The `edited` column: how long ago the file was last written, coarse and relative, or `—` when
/// the index has no edit time for it.
fn edited_text(row: &Row, now: DateTime<Utc>) -> String {
    row.edited_at
        .map_or_else(|| "—".to_string(), |t| relative(t, now))
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
        let all = split_frame(frame(120), EVERY_PANE);
        assert_eq!(all.sidebar.map(|r| r.width), Some(SIDEBAR_WIDTH));
        assert_eq!(all.list.width, LIST_MIN);
        assert_eq!(
            all.reader.map(|r| r.width),
            Some(120 - SIDEBAR_WIDTH - LIST_MIN)
        );

        let two = split_frame(frame(119), EVERY_PANE);
        assert!(
            two.reader.is_none(),
            "one column short of three panes, the reader is what goes"
        );
        assert_eq!(two.sidebar.map(|r| r.width), Some(SIDEBAR_WIDTH));
        assert_eq!(two.list.width, 119 - SIDEBAR_WIDTH);

        let two = split_frame(frame(70), EVERY_PANE);
        assert_eq!(two.sidebar.map(|r| r.width), Some(SIDEBAR_WIDTH));
        assert_eq!(
            two.list.width, LIST_MIN,
            "70 is the sidebar plus a list's floor"
        );

        let one = split_frame(frame(69), EVERY_PANE);
        assert!(
            one.sidebar.is_none() && one.reader.is_none(),
            "and one column below that, the sidebar goes too"
        );
        assert_eq!(
            one.list.width, 69,
            "the list is never dropped: it is the surface"
        );
    }

    #[test]
    fn a_toggle_only_ever_removes_a_pane() {
        // Wanting everything at 60 columns gets a list, not a two-column reader painted over it.
        // The automatic rule wins; the toggles are an escape hatch in one direction only.
        let narrow = split_frame(frame(60), EVERY_PANE);
        assert!(narrow.sidebar.is_none() && narrow.reader.is_none());
        assert_eq!(narrow.list.width, 60);

        let hidden = Panes {
            sidebar: false,
            ..EVERY_PANE
        };
        let no_sidebar = split_frame(frame(100), hidden);
        assert!(no_sidebar.sidebar.is_none());
        assert!(
            no_sidebar.reader.is_some(),
            "100 columns carries no reader beside a sidebar and does without one — which is the \
             only way a toggle adds anything, and it adds it by taking something away"
        );

        let no_reader = split_frame(
            frame(120),
            Panes {
                reader: false,
                ..EVERY_PANE
            },
        );
        assert!(no_reader.reader.is_none());
        assert_eq!(
            no_reader.list.width,
            120 - SIDEBAR_WIDTH,
            "with the reader away the list takes the whole main pane"
        );
    }

    #[test]
    fn the_width_the_run_loop_asks_for_is_the_width_the_panel_gets() {
        // The size-only question answers for every pane wanted. That has to agree with what is
        // painted in that state, or the highlighter wraps text for a panel of a different width.
        let painted = split_frame(frame(120), EVERY_PANE).reader.unwrap();
        assert_eq!(reader_text_width(120, 21), Some(painted.width - 2));
        assert_eq!(reader_text_width(119, 21), None);

        // And the app-aware answer differs the moment the sidebar is hidden, which is why the
        // second entry point exists.
        let hidden = Panes {
            sidebar: false,
            ..EVERY_PANE
        };
        assert_eq!(reader_text_width(100, 21), None);
        assert!(reader_text_width_for(hidden, 100, 21).is_some());
    }

    #[test]
    fn the_sidebar_is_exactly_as_wide_as_the_grid_it_carries() {
        // The constant is derived from the calendar, not chosen for it. If the header ever grows
        // a column — a week-number gutter, a wider weekday abbreviation — this is what says so.
        assert_eq!(
            WEEKDAYS.len() * DAY_CELL + 2,
            SIDEBAR_WIDTH as usize,
            "seven cells of four columns, and two borders"
        );
        assert_eq!(
            weekday_header().width(),
            WEEKDAYS.len() * DAY_CELL,
            "and the header is the grid's own width, with no ragged edge"
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

        // Header, then two rows per week. The 9th is the Wednesday of the second week: the third
        // four-column cell, which starts at column 8 with a blank, so its number is columns 9
        // and 10.
        assert_eq!(&lines[3][8..12], "  9 ");
        assert_eq!(
            lines[4].chars().nth(10),
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
        // locale, which in a four-column day cell shifts the rest of the week sideways.
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
    fn every_graph_glyph_is_one_column() {
        // Box-drawing, by the user's choice of 2026-10-25, and every glyph here bar `◉` (Neutral)
        // is East Asian *Ambiguous*: two columns under a terminal that renders Ambiguous as wide.
        // That is not asserted against, because it buys nothing — the panel's own border `┌─│┐` is
        // the same Ambiguous set, so such a terminal has already broken the frame the gutter sits
        // in, and the ASCII gutter this replaced protected nothing inside it. What is asserted is
        // the default width, which still catches an emoji or a wide character in a fixed gutter.
        for glyph in [
            GLYPH_NODE,
            GLYPH_FOCUS,
            GLYPH_LANE,
            GLYPH_RULE,
            GLYPH_CROSS,
            GLYPH_BRANCH_RIGHT,
            GLYPH_BRANCH_LEFT,
            GLYPH_FORK_RIGHT,
            GLYPH_FORK_LEFT,
            GLYPH_FORK_MID,
            GLYPH_MORE,
        ] {
            assert_eq!(glyph.to_string().width(), 1, "`{glyph}` is not one column");
        }
    }

    /// `fold_gutter`, with the width it promises checked on the way out.
    fn gutter(lanes: &[Lane], width: usize) -> String {
        let drawn = fold_gutter(lanes, width, false);
        assert_eq!(
            drawn.width(),
            width * LANE_WIDTH,
            "`{drawn}` is not exactly {width} lanes wide"
        );
        drawn
    }

    /// A four-lane gutter for the fold tests, spelled out rather than borrowed from [`MAX_LANES`]:
    /// these say what folding *does*, which should not change every time the cap does.
    const NARROW: usize = 4;

    #[test]
    fn a_gutter_narrower_than_the_thread_folds_rather_than_dropping_the_row() {
        // Past the cap the art degrades and the list does not. Whichever lane this row's note is
        // in, the row still says which row it is.
        let wide = vec![Lane::Through; 9];
        assert_eq!(
            gutter(&wide, NARROW),
            "│ │ │ ┊ ",
            "the tail folds into one column"
        );

        // The node is what the fold must never hide: it is the only thing saying where you are.
        let mut with_node = vec![Lane::Through; 9];
        with_node[7] = Lane::Node;
        assert_eq!(gutter(&with_node, NARROW), "│ │ │ ● ");
        assert_eq!(fold_gutter(&with_node, NARROW, true), "│ │ │ ◉ ");

        // And a fold over nothing is a blank rather than a `┊` promising lanes that are not there.
        let mut sparse = vec![Lane::Empty; 9];
        sparse[0] = Lane::Node;
        assert_eq!(gutter(&sparse, NARROW), "●       ");
    }

    #[test]
    fn a_gutter_inside_the_cap_is_drawn_verbatim_and_padded() {
        assert_eq!(gutter(&[Lane::Node], 1), "● ");
        assert_eq!(gutter(&[Lane::Node], 3), "●     ");
        assert_eq!(fold_gutter(&[Lane::Node], 1, true), "◉ ");
        assert_eq!(
            gutter(&[Lane::Through, Lane::Node, Lane::Empty, Lane::Through], 4),
            "│ ●   │ ",
            "a note row has no horizontal, so every spacer is a space"
        );
    }

    #[test]
    fn a_connector_joins_its_branch_to_its_forks_in_one_unbroken_line() {
        use Lane::{Branch, ForkLeft, ForkRight};
        // One fork to the right: the shape nearly every fork in a conversation takes.
        assert_eq!(gutter(&[Branch, ForkRight], 2), "├─╮ ");
        // Two to the right: the nearer one tees off a horizontal that carries on to the last.
        assert_eq!(gutter(&[Branch, ForkRight, ForkRight], 3), "├─┬─╮ ");
        // A lane reused on the left, so the horizontal leaves the branch the other way.
        assert_eq!(gutter(&[ForkLeft, Branch], 2), "╭─┤ ");
        assert_eq!(gutter(&[ForkLeft, ForkLeft, Branch], 3), "╭─┬─┤ ");
        // Both sides at once, which makes the branch itself a crossing.
        assert_eq!(gutter(&[ForkLeft, Branch, ForkRight], 3), "╭─┼─╮ ");
        // Padding past the last lane stays blank: the horizontal stops at its last fork.
        assert_eq!(gutter(&[Branch, ForkRight], 4), "├─╮     ");
    }

    #[test]
    fn a_horizontal_crosses_a_lane_passing_through_rather_than_cutting_it() {
        use Lane::{Branch, ForkRight, Through};
        assert_eq!(gutter(&[Branch, Through, ForkRight], 3), "├─┼─╮ ");
        // A lane outside the span is untouched by it, on either side.
        assert_eq!(
            gutter(&[Through, Branch, ForkRight, Through], 4),
            "│ ├─╮ │ "
        );
    }

    #[test]
    fn a_horizontal_runs_straight_through_an_empty_lane_inside_its_span() {
        // Reachable: a note forking from lane 2 with lanes 0 and 1 both free reuses the leftmost,
        // leaving lane 1 empty between the fork and its branch.
        use Lane::{Branch, Empty, ForkLeft};
        assert_eq!(gutter(&[ForkLeft, Empty, Branch], 3), "╭───┤ ");
    }

    #[test]
    fn a_connector_that_runs_past_the_cap_runs_into_the_fold() {
        use Lane::{Branch, ForkRight, Through};
        // The branch is drawn, its fork is folded: the horizontal still leaves towards it and
        // stops at the fold, which is not a fork glyph because it stands for more than one lane.
        assert_eq!(
            gutter(&[Through, Through, Branch, Through, ForkRight], NARROW),
            "│ │ ├─┊ "
        );
        // A wide fan: every drawn fork tees off, because the last one is past the cap.
        assert_eq!(
            gutter(
                &[
                    Branch, ForkRight, ForkRight, ForkRight, ForkRight, ForkRight
                ],
                NARROW
            ),
            "├─┬─┬─┊ "
        );
        // The whole connector past the cap: no horizontal on the drawn side at all.
        assert_eq!(
            gutter(
                &[Through, Through, Through, Through, Branch, ForkRight],
                NARROW
            ),
            "│ │ │ ┊ "
        );
    }

    #[test]
    fn the_cap_leaves_a_title_worth_reading() {
        // The arithmetic the cap was chosen against: 28 columns inside the sidebar's borders, a
        // gutter of two columns a lane, a gap, and what is left for the title. If any constant
        // moves, this is what says the title column has stopped being able to say anything.
        let inner = SIDEBAR_WIDTH as usize - 2;
        let title = inner - (MAX_LANES * LANE_WIDTH + GRAPH_GAP);
        assert!(
            title >= 15,
            "a {MAX_LANES}-lane gutter leaves {title} columns of title"
        );
    }

    /// A thread built through a real workspace and opened in the graph. Each entry is a title and
    /// the index of the entry it replies to; they are created in order, so the order written is
    /// the creation order the sweep follows. `focus` is the index the cursor is put on.
    fn thread(notes: &[(&str, Option<usize>)], focus: usize) -> (tempfile::TempDir, App) {
        let tmp = tempfile::tempdir().unwrap();
        let mut ws = jot_core::workspace::Workspace::init(tmp.path()).unwrap();
        let mut ids = Vec::new();
        for (title, parent) in notes {
            let mut draft = jot_core::query::Draft::new("b").title(*title);
            if let Some(parent) = parent {
                draft = draft.reply_to(ids[*parent]);
            }
            ids.push(ws.create(draft).unwrap().meta().id);
        }
        ws.sync().unwrap();

        let mut app = App::new(ws);
        let at = app
            .rows()
            .iter()
            .position(|row| row.note.id == ids[focus])
            .unwrap();
        while app.selected() < at {
            app.dispatch(crate::key::Action::MoveDown);
        }
        assert_eq!(app.selected(), at);
        (tmp, app)
    }

    /// The graph panel as it lands in a sidebar-wide terminal: inner rows only, trailing blanks
    /// trimmed, tall enough that nothing scrolls.
    fn rendered(app: &App) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let height = app.graph().len() as u16 + 2;
        let mut terminal = Terminal::new(TestBackend::new(SIDEBAR_WIDTH, height)).unwrap();
        terminal
            .draw(|frame| draw_graph(frame, frame.area(), app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (1..height - 1)
            .map(|y| {
                (1..SIDEBAR_WIDTH - 1)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn the_approved_thread_renders_line_for_line() {
        // The shape the user approved on 2026-10-25: root with replies a and b, and c replying to
        // a. Every lane is a glyph and a spacer, so a two-lane gutter is four columns, then the
        // gap — which is the three blanks after `◉` and the two between `│` and `a`.
        let (_tmp, app) = thread(
            &[
                ("root", None),
                ("a", Some(0)),
                ("b", Some(0)),
                ("c", Some(1)),
            ],
            0,
        );
        assert_eq!(
            rendered(&app),
            ["◉    root", "├─╮", "● │  a", "│ ●  b", "●    c"]
        );
    }

    #[test]
    fn a_fork_into_a_reused_lane_curves_left_back_to_its_branch() {
        // a is a leaf, so its lane is free when b forks, and b's second reply takes it.
        let (_tmp, app) = thread(
            &[
                ("root", None),
                ("a", Some(0)),
                ("b", Some(0)),
                ("c", Some(2)),
                ("d", Some(2)),
            ],
            3,
        );
        assert_eq!(
            rendered(&app),
            [
                "●    root",
                "├─╮",
                "● │  a",
                "  ●  b",
                "╭─┤",
                "│ ◉  c",
                "●    d"
            ]
        );
    }

    #[test]
    fn a_fork_past_a_live_lane_draws_a_crossing_and_keeps_the_titles_in_one_column() {
        // a forks while b's lane is still open, so a's second reply appends a third lane and the
        // horizontal out to it crosses b's.
        let (_tmp, app) = thread(
            &[
                ("root", None),
                ("a", Some(0)),
                ("b", Some(0)),
                ("c", Some(1)),
                ("d", Some(1)),
            ],
            2,
        );
        assert_eq!(
            rendered(&app),
            [
                "●      root",
                "├─╮",
                "● │    a",
                "├─┼─╮",
                "│ ◉ │  b",
                "●   │  c",
                "    ●  d"
            ]
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

        // No toggle: on a 24-row terminal minus the status line, 15 rows of calendar and 8 of
        // graph, which is the state the surface opens in.
        let (calendar, graph) = split_sidebar(column);
        assert_eq!(calendar.height, CALENDAR_ROWS + 2);
        assert_eq!(graph.map(|r| r.height), Some(23 - (CALENDAR_ROWS + 2)));

        // And a column too short to carry both keeps the month rather than drawing an empty box.
        let short = Rect {
            height: CALENDAR_ROWS + 2 + GRAPH_MIN_HEIGHT - 1,
            ..column
        };
        assert_eq!(split_sidebar(short), (short, None));

        // One row taller and the graph is back, without anything having been pressed.
        let enough = Rect {
            height: short.height + 1,
            ..column
        };
        assert_eq!(
            split_sidebar(enough).1.map(|r| r.height),
            Some(GRAPH_MIN_HEIGHT)
        );
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
        // own subtree is already announced by the counts column.
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
    fn every_marker_character_is_one_column_at_the_default_width() {
        // The lane's `╭│╰` are East Asian Ambiguous, so CJK width is deliberately not the
        // invariant here. They follow the same accepted rule as the frame borders and thread graph:
        // default-width glyphs must be one column, which keeps the fixed marker slots honest.
        let lane = String::from_iter([LINK_TOP, GLYPH_LANE, LINK_BOTTOM, GLYPH_FOCUS, LINK_PARENT]);
        for glyph in [
            lane.as_str(),
            "\u{232b}",
            "\u{26a0}",
            "\u{21b3}",
            "\u{2691}",
            "\u{276f}",
            "\u{275e}",
        ] {
            for character in glyph.chars() {
                assert_eq!(
                    character.width(),
                    Some(1),
                    "`{character}` in `{glyph}` is not one column at the default width"
                );
            }
        }
    }

    #[test]
    fn the_counts_column_shows_a_fork_as_direct_over_total() {
        let now = Utc.with_ymd_and_hms(2026, 9, 4, 12, 0, 0).unwrap();
        let mut row = fake_row(now);

        row.replies = 0;
        row.descendants = 0;
        assert_eq!(counts_text(&row), "", "a leaf has nothing to count");

        row.replies = 2;
        row.descendants = 2;
        assert_eq!(counts_text(&row), "2 ▸", "a flat thread shows one count");

        row.replies = 2;
        row.descendants = 5;
        assert_eq!(
            counts_text(&row),
            "2/5 ▸",
            "a branching thread shows both, which is what makes a fork visible from the list"
        );
    }

    #[test]
    fn the_counts_column_counts_what_points_at_a_note() {
        let now = Utc.with_ymd_and_hms(2026, 9, 4, 12, 0, 0).unwrap();
        let mut row = fake_row(now);

        row.quoted = 1;
        assert_eq!(counts_text(&row), "1 ❞");

        // Both counts, in the order the eye reads them: what grew under it, then what points at it.
        row.replies = 2;
        row.descendants = 2;
        assert_eq!(counts_text(&row), "2 ▸ 1 ❞");
    }

    /// Natural widths in the shape a real frame produces: thirteen-column ids, a counts column
    /// as wide as its label, and each time column as wide as its own.
    fn natural(title: usize) -> Natural {
        Natural {
            id: 13,
            title,
            counts: HEADER_COUNTS.width(),
            created: HEADER_CREATED.width(),
            edited: HEADER_EDITED.width(),
        }
    }

    /// The time column a view ordered by `ordered` keeps longest.
    fn kept(columns: Columns, ordered: Time) -> Option<usize> {
        match ordered {
            Time::Created => columns.created,
            Time::Edited => columns.edited,
        }
    }

    #[test]
    fn the_table_never_lays_out_wider_than_its_pane() {
        // The arithmetic has to close exactly: a row wider than the pane is squeezed by the table,
        // and what gets squeezed is a time, which a trimmed snapshot cannot show is missing.
        // Swept over every pane from too narrow for anything to far wider than needed, against
        // short titles, titles at the cap and titles far past it, in both orders.
        for ordered in [Time::Created, Time::Edited] {
            for title in [1, 5, 20, TITLE_MAX, 80] {
                let natural = natural(title);
                let floor = MARKER_WIDTH + natural.id + ID_GAP + META_GAP + natural.created;
                for inner in floor..200 {
                    let columns = list_columns(natural, inner, ordered);
                    assert!(
                        columns.total() <= inner,
                        "title {title} in {inner}: {columns:?} is {} wide",
                        columns.total()
                    );
                    assert!(
                        kept(columns, ordered).is_some(),
                        "the time the view is ordered by is never what gives"
                    );

                    let sum: u16 = columns
                        .constraints()
                        .iter()
                        .map(|c| match c {
                            Constraint::Length(n) => *n,
                            other => panic!("{other:?} is not a fixed width"),
                        })
                        .sum();
                    assert_eq!(
                        usize::from(sum),
                        columns.total(),
                        "the constraints the table is handed are the widths that were decided"
                    );
                }
            }
        }
    }

    #[test]
    fn the_columns_drop_in_order_before_the_title_is_clipped() {
        // Counts, then the time the view is not ordered by, then the title. Narrowing a pane one
        // column at a time, nothing later in that order loses a column while anything earlier is
        // still drawn.
        let natural = natural(20);
        for ordered in [Time::Created, Time::Edited] {
            for inner in 30..120 {
                let columns = list_columns(natural, inner, ordered);
                let other = match ordered {
                    Time::Created => columns.edited,
                    Time::Edited => columns.created,
                };
                if columns.counts.is_some() {
                    assert!(
                        other.is_some(),
                        "the other time went before the counts at {inner}"
                    );
                }
                if other.is_some() {
                    assert_eq!(columns.title, 20, "the title was clipped at {inner} first");
                }
            }
        }
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
