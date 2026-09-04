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
use crate::key::{Keymap, Mode};

/// Paint the whole frame.
pub fn draw(frame: &mut Frame, app: &App, now: DateTime<Utc>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(frame.area());

    draw_list(frame, chunks[0], app, now);
    draw_status(frame, chunks[1], app);

    if app.help_is_open() {
        draw_help(frame, frame.area());
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

    // Width available for the title: the frame's borders, the marker, and the right-hand meta
    // column all come off first, so a long title is truncated rather than pushing the age off the
    // edge.
    //
    // The arithmetic has to close exactly. A row is `MARKER + title + gap + meta`, and that must
    // total the block's inner width — two columns wider and the meta column is clipped off the
    // right edge, which is invisible in a snapshot because the trailing spaces get trimmed and it
    // simply looks as though notes have no age.
    let inner = area.width.saturating_sub(2) as usize;
    let meta_width = 18;
    let title_width = inner.saturating_sub(meta_width + MARKER_WIDTH);

    let items: Vec<ListItem> = app
        .rows()
        .iter()
        .map(|row| ListItem::new(row_line(row, title_width, meta_width, now)))
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

/// One row: marker, title, then the counts and age, right-aligned.
fn row_line<'a>(row: &Row, title_width: usize, meta_width: usize, now: DateTime<Utc>) -> Line<'a> {
    let title = row
        .note
        .title
        .clone()
        .unwrap_or_else(|| "Untitled".to_string());
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
    let gap = title_width.saturating_sub(title_cell.width()) + pad;

    Line::from(vec![
        Span::styled(marker.to_string(), dim()),
        Span::styled(title_cell, title_style),
        Span::raw(" ".repeat(gap)),
        Span::styled(meta, dim()),
    ])
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
            " Space — q to quit",
            Style::default().fg(Color::Cyan),
        ))
    } else if app.mode() == Mode::Input {
        Line::from(Span::styled(
            " typing — Enter to accept, Esc to stop",
            dim(),
        ))
    } else {
        let count = app.rows().len();
        let position = if count == 0 {
            String::new()
        } else {
            format!("{}/{count}  ", app.selected() + 1)
        };
        Line::from(Span::styled(
            format!(" {position}?  help    Tab  next view    Space q  quit"),
            dim(),
        ))
    };

    frame.render_widget(Paragraph::new(line), area);
}

/// The `?` overlay, rendered from the same table the event loop dispatches on.
fn draw_help(frame: &mut Frame, area: Rect) {
    let bindings = Keymap::bindings();

    // Two columns of key/description, centred, sized to the content rather than the terminal.
    let width = 46u16.min(area.width.saturating_sub(4));
    let height = (bindings.len() as u16 + 4).min(area.height.saturating_sub(2));
    let popup = centred(area, width, height);

    let lines: Vec<Line> = bindings
        .iter()
        .map(|b| {
            Line::from(vec![
                Span::styled(
                    format!("  {:<10}", b.keys),
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
