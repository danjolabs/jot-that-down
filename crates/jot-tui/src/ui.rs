//! Rendering. Pure: every function here takes state and produces cells, and touches nothing else.
//!
//! Keeping the draw path free of side effects is what makes the snapshot tests worth having — a
//! rendered frame is a function of [`App`], so a diff in the snapshot is a real visual change and
//! never a timing artefact.

use chrono::{DateTime, Utc};
use jot_core::query::{Ref, Row, State};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, ViewKind, sort_name};
use crate::key::{Keymap, Mode, PREFIX_LABEL, Scope};

/// Paint the whole frame.
pub fn draw(frame: &mut Frame, app: &App, now: DateTime<Utc>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    let (list, reader) = split_main(chunks[0]);
    draw_list(frame, list, app, now);
    if let Some(reader) = reader {
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
#[must_use]
pub fn reader_text_width(frame_width: u16, frame_height: u16) -> Option<u16> {
    let area = Rect {
        x: 0,
        y: 0,
        width: frame_width,
        height: frame_height.saturating_sub(1),
    };
    split_main(area).1.map(|r| r.width.saturating_sub(2))
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
    // eight characters until a burst of notes shares a millisecond and forces nine; a meta cell is
    // three characters for a lone note and thirteen for a branching week-old thread. Sizing to the
    // widest present is what keeps the age beside the title instead of a fixed guess away from it
    // — which was the visible complaint: a right-aligned column in an 80-column frame puts the
    // time an inch of whitespace from the title it belongs to.
    let id_width = app
        .rows()
        .iter()
        .map(|row| app.short_id(row.note.id).width())
        .max()
        .unwrap_or(0);
    let meta_width = app
        .rows()
        .iter()
        .map(|row| meta_text(row, now).width())
        .max()
        .unwrap_or(0);

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
    // A meta column may never take more than half of what is left; a vault of nothing but
    // Untitled forks should still show some title.
    let meta_width = meta_width.min(avail / 2);
    let title_width = title_natural
        .min(avail.saturating_sub(meta_width + META_GAP))
        .min(TITLE_MAX);

    let items: Vec<ListItem> = app
        .rows()
        .iter()
        .map(|row| {
            ListItem::new(row_line(
                row,
                &app.short_id(row.note.id),
                id_width,
                title_width,
                meta_width,
                now,
            ))
        })
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

/// Columns the leading state marker occupies. Every marker is one glyph plus a space, and the
/// wide ones (`⌫`, `⚠`) still render in one column followed by a space.
const MARKER_WIDTH: usize = 2;

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

/// One row: marker, id, title, then the counts and age.
fn row_line<'a>(
    row: &Row,
    id: &str,
    id_width: usize,
    title_width: usize,
    meta_width: usize,
    now: DateTime<Utc>,
) -> Line<'a> {
    let title = title_of(row);
    let title_style = if row.note.title.is_some() {
        Style::default()
    } else {
        dim()
    };

    // A trashed parent is the state stage 2 exists to make visible: the note is live, its parent
    // is not, and a list that says nothing about it is lying by omission.
    let marker = match &row.parent {
        Some(Ref::Trashed(_)) => "⌫ ",
        Some(Ref::Deleted(_)) => "⚠ ",
        _ if row.state == State::Trashed => "⌫ ",
        _ => "  ",
    };

    let meta = meta_text(row, now);
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
        Span::styled(marker.to_string(), dim()),
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

/// The right-hand column: reply count, then relative age.
fn meta_text(row: &Row, now: DateTime<Utc>) -> String {
    let age = row
        .note
        .created_at
        .map_or_else(|| "—".to_string(), |t| relative(t, now));

    if row.replies == 0 {
        age
    } else if row.replies == row.descendants {
        format!("{} ▸  {age}", row.replies)
    } else {
        // Direct replies and the whole subtree differ, which means the thread branches. Showing
        // both is what makes a fork visible from the list rather than only after opening it.
        format!("{}/{} ▸  {age}", row.replies, row.descendants)
    }
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
    fn the_meta_column_shows_a_fork_as_direct_over_total() {
        let now = Utc.with_ymd_and_hms(2026, 9, 4, 12, 0, 0).unwrap();
        let mut row = fake_row(now);

        row.replies = 0;
        row.descendants = 0;
        assert_eq!(meta_text(&row, now), "now", "a leaf shows only its age");

        row.replies = 2;
        row.descendants = 2;
        assert_eq!(
            meta_text(&row, now),
            "2 ▸  now",
            "a flat thread shows one count"
        );

        row.replies = 2;
        row.descendants = 5;
        assert_eq!(
            meta_text(&row, now),
            "2/5 ▸  now",
            "a branching thread shows both, which is what makes a fork visible from the list"
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
            edited_at: None,
        }
    }
}
