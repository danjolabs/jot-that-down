//! The `$EDITOR` handoff: a scratch file, a wait, and a loop that never loses what was typed.
//!
//! # Why a scratch file and not the note itself
//!
//! Opening the real file would be simpler and would break the seam: the surface would be writing
//! into the workspace, and `jot-core` would find out afterwards — and jot never writes a note that
//! fails `schema.json`, which a hand-saved file could. So the editor gets a scratch file in the
//! system temp directory, and what comes back goes through core's one write path.
//!
//! # The loop
//!
//! ```text
//! seed ─▶ $EDITOR ─▶ untouched? ─ yes ─▶ Untouched (scratch file removed)
//!            ▲            │ no
//!            │            ▼
//!            │         submit ─ Accept ─▶ Done (scratch file removed)
//!            │            │ Retry(reasons)
//!            └── reasons written into the buffer as `# jot:` comments
//!                         │ Abort(message)
//!                         ▼
//!                      error naming the kept scratch file
//! ```
//!
//! **What was typed is never lost.** A refused buffer is reopened, the same text with the reasons
//! at the top of its frontmatter block; quitting without saving then stops, keeps the scratch file,
//! and prints its path. Every other way out that has not written the note keeps it too.

use anyhow::{Context as _, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

/// What `submit` made of a buffer.
pub enum Verdict<T> {
    /// It was written (or there was nothing to write). The scratch file goes.
    Accept(T),
    /// It was refused for reasons the person can fix in the buffer: reopen it with these.
    Retry(Vec<String>),
    /// It was refused for a reason editing will not fix. The scratch file is kept.
    Abort(String),
}

/// How a [`compose`] ended without an error.
pub enum Composed<T> {
    /// `submit` accepted the buffer.
    Done(T),
    /// The buffer came back exactly as it was handed over: the editor's way of saying "cancel".
    Untouched,
}

/// The prefix of the lines jot writes into a refused buffer, and strips again on read-back.
const MARK: &str = "# jot:";

/// Open `$EDITOR` on `seed` until `submit` accepts what comes back, or the person gives up.
///
/// # Errors
///
/// No editor configured, the editor failing to launch or exiting non-zero, an `Abort`, or an
/// untouched buffer after a refusal. Every one of those names the scratch file when it holds
/// anything typed.
pub fn compose<T>(seed: &str, mut submit: impl FnMut(&str) -> Verdict<T>) -> Result<Composed<T>> {
    let editor = editor_command()?;
    let path = temp_path();
    write(&path, seed)?;

    let mut handed = seed.to_owned();
    let mut refused = false;
    loop {
        let kept = || format!("your draft is kept at `{}`", path.display());
        let status = match launch(&editor, &path) {
            Ok(status) => status,
            Err(e) if !refused => {
                let _ = std::fs::remove_file(&path);
                return Err(e);
            }
            Err(e) => return Err(e.context(kept())),
        };
        if !status.success() {
            // The draft is kept: the editor may have saved before failing.
            bail!("editor `{editor}` exited with {status}; {}", kept());
        }

        let after = std::fs::read_to_string(&path)
            .with_context(|| format!("cannot read the draft back from `{}`", path.display()))?;
        if after == handed {
            if refused {
                bail!("the note was not saved; {}", kept());
            }
            let _ = std::fs::remove_file(&path);
            return Ok(Composed::Untouched);
        }

        let text = strip_reasons(&after);
        match submit(&text) {
            Verdict::Accept(value) => {
                let _ = std::fs::remove_file(&path);
                return Ok(Composed::Done(value));
            }
            Verdict::Retry(reasons) => {
                eprintln!("jot: the note was not saved:");
                for reason in &reasons {
                    eprintln!("  {reason}");
                }
                eprintln!(
                    "jot: reopening the editor — fix it and save, or quit without saving to stop"
                );
                handed = with_reasons(&text, &reasons);
                write(&path, &handed)?;
                refused = true;
            }
            Verdict::Abort(message) => bail!("{message}\n{}", kept()),
        }
    }
}

/// `text` with `reasons` written as `# jot:` comment lines at the top of its frontmatter block.
///
/// Inside the block, because a note's first line must be the `---` fence; YAML comments, so the
/// buffer still parses whether or not they are deleted. A buffer with no block gets one holding
/// only the comments, which [`strip_reasons`] removes whole.
fn with_reasons(text: &str, reasons: &[String]) -> String {
    let mut block = format!(
        "{MARK} not saved: fix what is below and save, or quit without saving to keep this draft\n"
    );
    for reason in reasons {
        for line in reason.lines() {
            block.push_str(&format!("{MARK}   {line}\n"));
        }
    }
    match opening_fence(text) {
        Some(fence) => format!("{fence}{block}{}", &text[fence.len()..]),
        None => format!("---\n{block}---\n{text}"),
    }
}

/// `text` without the lines [`with_reasons`] added, so they never reach a note.
fn strip_reasons(text: &str) -> String {
    let Some(fence) = opening_fence(text) else {
        return text.to_owned();
    };
    let mut rest = &text[fence.len()..];
    let mut stripped = false;
    while rest.starts_with(MARK) {
        let end = rest.find('\n').map_or(rest.len(), |i| i + 1);
        rest = &rest[end..];
        stripped = true;
    }
    if !stripped {
        return text.to_owned();
    }
    // A block that held nothing but the reasons was ours; the buffer had none.
    if let Some(close) = opening_fence(rest) {
        return rest[close.len()..].to_owned();
    }
    format!("{fence}{rest}")
}

/// The `---` line a frontmatter block opens with, terminator included.
fn opening_fence(text: &str) -> Option<&str> {
    ["---\n", "---\r\n"]
        .into_iter()
        .find(|fence| text.starts_with(fence))
        .map(|fence| &text[..fence.len()])
}

fn write(path: &Path, text: &str) -> Result<()> {
    std::fs::write(path, text)
        .with_context(|| format!("cannot write the draft `{}`", path.display()))
}

/// The editor to launch: `$VISUAL`, then `$EDITOR`.
///
/// `VISUAL` first is the long-standing convention — `EDITOR` may be a line editor for use where a
/// full-screen one cannot run, which is not this situation. No built-in default: guessing `vi` on a
/// machine that does not have it produces a worse message than saying what is missing.
fn editor_command() -> Result<String> {
    for var in ["VISUAL", "EDITOR"] {
        if let Some(value) = std::env::var_os(var) {
            let value = value.to_string_lossy().trim().to_owned();
            if !value.is_empty() {
                return Ok(value);
            }
        }
    }
    bail!(
        "no editor configured: set $EDITOR (or $VISUAL)\n\
         hint: pass the note inline with `-m` instead, or pipe it in on stdin"
    )
}

/// Launch the editor, inheriting the terminal so a full-screen editor works.
///
/// The command is split on whitespace so `EDITOR="code --wait"` and `EDITOR="emacs -nw"` behave,
/// which is what people actually have configured. It is **not** run through a shell: passing a
/// user-controlled string to `sh -c` would make a filename with a space a command injection.
fn launch(editor: &str, path: &Path) -> Result<std::process::ExitStatus> {
    let mut parts = editor.split_whitespace();
    let program = parts.next().context("the configured editor is empty")?;

    Command::new(program)
        .args(parts)
        .arg(path)
        .status()
        .with_context(|| format!("cannot launch editor `{editor}`"))
}

/// A unique path in the system temp directory, ending in `.md`.
///
/// Outside the workspace on purpose: a crashed editor must not leave a file the next sync reports
/// as a stray — or, named like a note, mistakes for one.
fn temp_path() -> PathBuf {
    // The process id plus a nanosecond clock reading is enough: two `jot` processes on one machine
    // cannot share a pid, and one process never has two drafts open at once.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    std::env::temp_dir().join(format!("jot-{}-{nanos}.md", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reasons are for the person, never for the note: whatever they look like on the way in,
    /// read-back removes exactly them and leaves the typed text byte-for-byte.
    #[test]
    fn reasons_written_into_a_buffer_are_stripped_from_it_exactly() {
        let reasons = vec![
            String::from("`created_at` (format): \"yesterday\" is not a \"date-time\""),
            String::from("two\nlines"),
        ];
        for typed in [
            "---\ntitle: t\ncreated_at: yesterday\n---\n\nbody\n",
            "---\r\ntitle: t\r\n---\r\nbody\r\n",
            "no frontmatter at all\n",
            "---\n---\nan empty block\n",
        ] {
            let shown = with_reasons(typed, &reasons);
            assert!(shown.starts_with("---"), "still a note:\n{shown}");
            assert!(shown.contains("# jot:   two\n# jot:   lines\n"), "{shown}");
            let back = strip_reasons(&shown);
            let expected = if typed.starts_with("---\n---\n") {
                // An empty block and no block are the same note.
                "an empty block\n"
            } else {
                typed
            };
            assert_eq!(back, expected, "from:\n{shown}");
        }
    }

    #[test]
    fn a_buffer_without_reasons_is_left_alone() {
        let text = "---\n# a comment of the person's own\ntitle: t\n---\n";
        assert_eq!(strip_reasons(text), text);
    }

    #[test]
    fn a_temp_path_is_markdown_and_lives_outside_any_workspace() {
        let path = temp_path();
        assert_eq!(path.extension().unwrap(), "md");
        assert!(path.starts_with(std::env::temp_dir()));
    }
}
