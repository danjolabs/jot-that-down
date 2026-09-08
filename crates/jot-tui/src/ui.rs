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
