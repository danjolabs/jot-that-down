//! The `$EDITOR` handoff: a temp file, a wait, and a careful read-back.
//!
//! # Why a temp file and not the note itself
//!
//! Opening the real file would be simpler and would break the seam: the surface would be writing
//! into the vault, and `jot-core` would find out afterwards. Everything goes through
//! [`Workspace::create`](jot_core::workspace::Workspace::create) and
//! [`Workspace::edit`](jot_core::workspace::Workspace::edit) instead, so the write path — and its
//! unknown-key preservation, its no-op detection, and its re-slugging — applies to an edit made in
//! Vim exactly as it does to one made with `-t`.
//!
//! # What the editor is handed
//!
//! The note as jot would write it: frontmatter block first, body after. Editing a title in the
//! block therefore works. What is read back is parsed with the crate's own parser, so a block the
//! editor mangled fails the same way a hand-edited file would — loudly, and with the draft kept.
//!
//! # The limit, stated plainly
//!
//! Only `title`, `body`, and `relation:quote` are carried back. `relation:reply_to` and
//! `relation:root` are not editable by design — re-parenting is an explicit operation, not a side
//! effect of an edit — and **unknown keys changed in the editor are not applied**, because the
//! [`Edit`](jot_core::query::Edit) type does not carry them. They are preserved from the file, not
//! from the buffer. A change to one is detected and warned about rather than silently dropped.

use anyhow::{Context as _, Result, bail};
use jot_core::frontmatter::{Frontmatter, FrontmatterSchema};
use std::path::{Path, PathBuf};
use std::process::Command;

/// What came back from the editor.
pub struct Edited {
    /// The frontmatter as the editor left it.
    pub frontmatter: Frontmatter,
    /// The body as the editor left it.
    pub body: String,
    /// Whether the buffer is byte-identical to what was handed over.
    pub unchanged: bool,
}

impl Edited {
    /// Whether nothing at all was typed: no title, and a body that is empty or only whitespace.
    ///
    /// A title with no body is a note worth keeping — it is how most captures start, and
    /// dogfooding says the title is the field that always gets filled in. So the discard test is
    /// about the buffer as a whole, not the body alone. "Empty" has to include the newline every
    /// editor leaves behind, or a buffer nobody touched would look written-in.
    pub fn is_empty(&self) -> bool {
        self.body.trim().is_empty()
            && self
                .frontmatter
                .title
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty()
    }
}

/// Open `$EDITOR` on `seed` and return what came back.
///
/// The temp file keeps a `.md` suffix so the editor turns on markdown highlighting, and lives in
/// the system temp directory rather than the vault — a crashed editor must not leave a stray file
/// that the next scan mistakes for a note.
///
/// # Errors
///
/// If no editor is configured, if it cannot be launched, if it exits non-zero, or if what came back
/// does not parse. In the parse case the draft is **kept** and its path is named: losing what
/// someone just typed is the one failure this whole program exists to prevent.
pub fn edit(schema: &FrontmatterSchema, seed: &str) -> Result<Edited> {
    let editor = editor_command()?;
    let path = temp_path();

    std::fs::write(&path, seed)
        .with_context(|| format!("cannot write the draft `{}`", path.display()))?;

    let status = launch(&editor, &path)?;
    if !status.success() {
        // The draft is kept: the editor may have saved before failing.
        bail!(
            "editor `{editor}` exited with {status}; your draft is at `{}`",
            path.display()
        );
    }

    let after = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read the draft back from `{}`", path.display()))?;
    let unchanged = after == seed;

    let parsed = Frontmatter::parse_document(schema, &path, after.as_bytes());
    let (frontmatter, body) = match parsed {
        Ok(split) => split,
        Err(err) => {
            bail!(
                "the edited note does not parse: {err}\n\
                 your draft is kept at `{}` — fix it there and try again",
                path.display()
            );
        }
    };

    // Only remove the temp file once its contents are safely in hand.
    let _ = std::fs::remove_file(&path);
    Ok(Edited {
        frontmatter,
        body,
        unchanged,
    })
}

/// The text handed to the editor for a note with this frontmatter and body.
///
/// This is the same render a file gets, deliberately. The buffer used to show a blank placeholder
/// for *every* declared key; now `required = true` decides, so the vault says in its manifest
/// which keys it wants staring back at you. `jot_default` marks `document:title` required, so a
/// new vault behaves as it always did for the key anyone actually fills in.
///
/// A required placeholder left alone round-trips to nothing — `title:` is YAML null, and null is
/// read as absent — so the file written is exactly the one an untouched buffer would produce.
pub fn seed(frontmatter: &Frontmatter, body: &str, schema: &FrontmatterSchema) -> String {
    format!("{}{}", frontmatter.render(schema), body)
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
    use jot_core::note::NoteId;

    fn schema() -> FrontmatterSchema {
        FrontmatterSchema::jot_default()
    }

    #[test]
    fn the_seed_offers_the_required_keys_only_and_the_blanks_round_trip_to_nothing() {
        let text = seed(&Frontmatter::new(), "\n", &schema());

        // `jot_default` marks `document:title` required and the two relations not.
        assert!(text.contains("title:"), "no `title` in:\n{text}");
        for key in ["relation:reply_to", "relation:quote_to"] {
            assert!(
                !text.contains(&format!("{key}:")),
                "`{key}` is not required and must not be offered as a blank:\n{text}"
            );
        }

        // Left untouched, the buffer is worth nothing: the placeholder reads back as absent.
        let (parsed, _) =
            Frontmatter::parse_document(&schema(), Path::new("draft.md"), text.as_bytes()).unwrap();
        assert_eq!(parsed.title, None);
        assert_eq!(parsed.reply_to, None);
        assert_eq!(parsed.quote, None);
        assert_eq!(parsed, Frontmatter::new());
    }

    /// `required` is the manifest's to set, so a vault that asks for a blank relation gets one.
    #[test]
    fn a_relation_the_schema_marks_required_is_offered_as_a_blank() {
        use jot_core::frontmatter::{FieldType, FrontmatterEntry, Role};

        let schema = FrontmatterSchema::try_new([
            FrontmatterEntry::with_key("title", FieldType::Reserved(Role::Title)).required(true),
            FrontmatterEntry::new(FieldType::Reserved(Role::ReplyTo)).required(true),
        ])
        .unwrap();

        let text = seed(&Frontmatter::new(), "\n", &schema);
        assert!(text.contains("relation:reply_to:"), "in:\n{text}");

        let (parsed, _) =
            Frontmatter::parse_document(&schema, Path::new("draft.md"), text.as_bytes()).unwrap();
        assert_eq!(parsed, Frontmatter::new(), "the blank reads back as absent");
    }

    #[test]
    fn a_filled_in_placeholder_is_read_back() {
        let text = seed(&Frontmatter::new(), "\n", &schema())
            .replace("title:", "title: Typed in the editor");
        let (parsed, _) =
            Frontmatter::parse_document(&schema(), Path::new("draft.md"), text.as_bytes()).unwrap();
        assert_eq!(parsed.title.as_deref(), Some("Typed in the editor"));
    }

    #[test]
    fn a_key_the_note_already_carries_is_seeded_with_its_value_not_a_blank() {
        let mut frontmatter = Frontmatter::new();
        frontmatter.title = Some("Existing".into());
        let text = seed(&frontmatter, "\n", &schema());

        assert!(text.contains("title: Existing"), "{text}");
        assert!(
            !text.contains("title:\n"),
            "no blank beside the real one:\n{text}"
        );
    }

    #[test]
    fn the_seed_is_a_note_the_parser_accepts_back() {
        let id = NoteId::new();
        let mut frontmatter = Frontmatter::new();
        frontmatter.title = Some("A title".into());
        frontmatter.reply_to = Some(id);

        let text = seed(&frontmatter, "\nthe body\n", &schema());
        let (parsed, body) =
            Frontmatter::parse_document(&schema(), Path::new("draft.md"), text.as_bytes()).unwrap();

        assert_eq!(parsed.title.as_deref(), Some("A title"));
        assert_eq!(parsed.reply_to, Some(id));
        assert_eq!(body, "\nthe body\n");
    }

    #[test]
    fn a_temp_path_is_markdown_and_lives_outside_any_vault() {
        let path = temp_path();
        assert_eq!(path.extension().unwrap(), "md");
        assert!(path.starts_with(std::env::temp_dir()));
    }

    #[test]
    fn two_temp_paths_do_not_collide() {
        assert_ne!(temp_path(), temp_path());
    }

    #[test]
    fn an_empty_edit_is_detected_through_the_whitespace_an_editor_leaves() {
        let empty = Edited {
            frontmatter: Frontmatter::new(),
            body: "\n\n  \n".into(),
            unchanged: false,
        };
        assert!(empty.is_empty());

        let written = Edited {
            frontmatter: Frontmatter::new(),
            body: "\na thought\n".into(),
            unchanged: false,
        };
        assert!(!written.is_empty());
    }

    /// The title is the field that always gets filled in, so it alone makes a buffer worth saving.
    #[test]
    fn a_title_with_no_body_is_not_empty() {
        let mut frontmatter = Frontmatter::new();
        frontmatter.title = Some("a thought I have not written yet".into());
        let titled = Edited {
            frontmatter,
            body: "\n".into(),
            unchanged: false,
        };
        assert!(!titled.is_empty());

        let mut blank = Frontmatter::new();
        blank.title = Some("   ".into());
        let whitespace = Edited {
            frontmatter: blank,
            body: "\n".into(),
            unchanged: false,
        };
        assert!(whitespace.is_empty(), "a whitespace title is no title");
    }
}
