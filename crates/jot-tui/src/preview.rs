//! The reader panel's source of styled text.
//!
//! Stage 5 asks for "terminal markdown styling: headings, bold, italics, inline code, fenced
//! blocks, lists, links. Nothing more." That is a markdown renderer, and `bat` is already one —
//! installed on most machines a terminal-first tool is used on, better at it than anything worth
//! writing here, and themed to the user's taste rather than to ours. So the reader *borrows* a
//! highlighter instead of growing one, and falls back cleanly when there is none to borrow.
//!
//! # Why this is a trait
//!
//! Spawning a process is the one thing in the draw path that is neither pure nor predictable: the
//! machine may have `bat` or may not, at one version or another, under one theme or another. A
//! snapshot test that shelled out would be a test of the tester's `$PATH`. So the production
//! renderer is [`Bat`] and the default is [`Plain`], which is deterministic, dependency-free, and
//! what every test renders through.
//!
//! # Nothing here opens a file
//!
//! `bat` is perfectly happy to be handed a path, and that is the obvious way to call it — and it
//! would put a surface back in the business of reading the vault off disk, which
//! `overview.md` locks shut. The markdown goes in over **stdin** instead: it came from
//! `jot-core`, it goes to the highlighter, and the file on disk is never opened by this crate.
//! The cost is one flag (`--language=md`, since there is no extension to sniff); the gain is that
//! the seam holds.

use std::io::Write as _;
use std::process::{Command, Stdio};

use ansi_to_tui::IntoText as _;
use ratatui::text::{Line, Text};

/// Turns a note's markdown into styled terminal lines.
pub trait Highlighter: Send {
    /// Render `markdown` to at most `width` columns.
    ///
    /// Wrapping is the renderer's job, not the caller's: `bat` wraps better than we can — it
    /// knows where its own decorations end — and [`Plain`] has to do it by hand anyway.
    fn render(&self, markdown: &str, width: u16) -> Vec<Line<'static>>;
}

/// The production renderer: `bat`, then `batcat`, then `cat`, then [`Plain`].
///
/// `batcat` is Debian's name for the same program — the `bat` binary name was already taken by an
/// unrelated package — and a Debian user is exactly the user who would otherwise conclude the
/// feature is broken. `cat` is the fallback the user asked for; over stdin it colours nothing, so
/// what it really buys is a check that the pipe works at all, and [`Plain`] catches the case where
/// even that is missing.
#[derive(Debug, Default, Clone, Copy)]
pub struct Bat;

/// The candidates, in the order they are tried. See [`Bat`].
const PROGRAMS: [&str; 3] = ["bat", "batcat", "cat"];

impl Highlighter for Bat {
    fn render(&self, markdown: &str, width: u16) -> Vec<Line<'static>> {
        for program in PROGRAMS {
            let Some(out) = pipe(program, markdown, width) else {
                continue;
            };
            // A highlighter that ran but emitted bytes we cannot parse is a highlighter that
            // failed, and falling through to the next one is better than painting mojibake.
            if let Ok(text) = out.into_text() {
                return owned(text);
            }
        }
        Plain.render(markdown, width)
    }
}

/// Feed `markdown` to `program` on stdin and collect stdout, or `None` if it could not be run.
///
/// The whole input is written before stdout is read, which is safe only because a note is small —
/// a pipe deadlock needs an input big enough to fill the kernel buffer while the child blocks
/// writing back, and a note that size is not a note. `Stdio::null()` on stderr keeps `bat`'s
/// warnings off the alternate screen, where they would land as corruption rather than as text.
fn pipe(program: &str, markdown: &str, width: u16) -> Option<Vec<u8>> {
    let mut command = Command::new(program);
    if program != "cat" {
        command.args([
            "--language=md",
            "--color=always",
            "--style=plain",
            "--paging=never",
            "--wrap=character",
            &format!("--terminal-width={width}"),
        ]);
    }

    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    child.stdin.take()?.write_all(markdown.as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    out.status.success().then_some(out.stdout)
}

/// The fallback renderer: the markdown as written, wrapped, with no colour.
///
/// Not a degraded mode to apologise for. A note is markdown a human wrote to be read as text, and
/// unstyled text at the right width is entirely readable — which is why this is also the default
/// [`Highlighter`], and why the snapshot tests are none the poorer for using it.
#[derive(Debug, Default, Clone, Copy)]
pub struct Plain;

impl Highlighter for Plain {
    fn render(&self, markdown: &str, width: u16) -> Vec<Line<'static>> {
        markdown
            .lines()
            .flat_map(|line| wrap(line, width as usize))
            .map(Line::from)
            .collect()
    }
}

/// Break one line into `width`-column pieces, on a word boundary where there is one.
///
/// Counts terminal columns rather than characters, for the same reason
/// [`crate::ui::truncate`](crate::ui) does: a CJK note wrapped on `chars()` overflows the panel and
/// paints over the frame's border.
fn wrap(line: &str, width: usize) -> Vec<String> {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

    if width == 0 {
        return vec![String::new()];
    }
    if line.width() <= width {
        return vec![line.to_string()];
    }

    let mut out = Vec::new();
    let mut current = String::new();
    let mut used = 0;
    // Where the last space sits in `current`, so a break can be moved back to it. `None` once a
    // single word has grown past the whole width, which is the case that has to break mid-word.
    let mut last_space: Option<(usize, usize)> = None;

    for c in line.chars() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w > width {
            match last_space {
                Some((at, before)) if before > 0 => {
                    let tail = current[at + 1..].to_string();
                    current.truncate(at);
                    out.push(std::mem::take(&mut current));
                    used = tail.width();
                    current = tail;
                }
                _ => {
                    out.push(std::mem::take(&mut current));
                    used = 0;
                }
            }
            last_space = None;
        }
        if c == ' ' {
            last_space = Some((current.len(), used));
        }
        current.push(c);
        used += w;
    }
    out.push(current);
    out
}

/// Detach a parsed [`Text`] from the buffer it borrowed, so it can be cached in `App`.
///
/// `ansi-to-tui` hands back spans borrowed from the bytes it parsed, and those bytes are the
/// child's stdout — a local. Owning the strings is what lets the render survive until the next
/// keystroke instead of being redone on every frame.
fn owned(text: Text<'_>) -> Vec<Line<'static>> {
    text.lines
        .into_iter()
        .map(|line| {
            let spans = line
                .spans
                .into_iter()
                .map(|span| ratatui::text::Span::styled(span.content.into_owned(), span.style))
                .collect::<Vec<_>>();
            let mut out = Line::from(spans);
            out.style = line.style;
            out.alignment = line.alignment;
            out
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn plain_wraps_on_a_word_boundary_and_never_exceeds_the_width() {
        let lines = Plain.render("the quick brown fox jumps over the lazy dog", 12);
        for line in &lines {
            assert!(
                line.width() <= 12,
                "`{line}` is {} columns wide",
                line.width()
            );
        }
        assert_eq!(lines[0].to_string(), "the quick");
    }

    #[test]
    fn a_word_longer_than_the_panel_is_broken_rather_than_overflowing() {
        // No space to fall back to, so the only correct answer is a hard break. Overflowing here
        // would paint over the panel's border.
        let lines = Plain.render("supercalifragilistic", 8);
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| l.width() <= 8));
    }

    #[test]
    fn a_cjk_line_wraps_on_columns_rather_than_characters() {
        let lines = Plain.render("안녕하세요 반갑습니다 이것은 아주 긴 줄입니다", 10);
        for line in &lines {
            assert!(
                UnicodeWidthStr::width(line.to_string().as_str()) <= 10,
                "`{line}` overflows a 10-column panel"
            );
        }
    }

    #[test]
    fn blank_lines_survive_so_paragraphs_stay_apart() {
        let lines = Plain.render("one\n\ntwo", 20);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].to_string(), "");
    }

    #[test]
    fn a_zero_width_panel_renders_rather_than_panicking() {
        // A frame narrow enough to leave the panel no inner width is a resize away at any moment.
        let lines = Plain.render("anything", 0);
        assert!(!lines.is_empty());
    }

    #[test]
    fn the_bat_chain_always_returns_something_to_paint() {
        // Environment-independent by construction: whichever of the three is on this machine — or
        // none of them — the chain ends at `Plain`, so there is no arrangement of `$PATH` in which
        // the reader panel comes back empty.
        let lines = Bat.render("# a heading\n\nand a paragraph\n", 40);
        let text: String = lines.iter().map(ToString::to_string).collect();
        assert!(
            text.contains("a heading") && text.contains("and a paragraph"),
            "the note's own words must survive whatever rendered them: {text:?}"
        );
        assert!(lines.iter().all(|l| l.width() <= 40), "{text:?}");
    }

    #[test]
    fn cat_is_in_the_chain_after_both_spellings_of_bat() {
        // The order is the contract: `bat` is the point, `batcat` is Debian's name for it, and
        // `cat` is the fallback that at least proves the pipe works.
        assert_eq!(PROGRAMS, ["bat", "batcat", "cat"]);
    }
}
