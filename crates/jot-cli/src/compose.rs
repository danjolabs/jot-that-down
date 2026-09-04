//! The CLI's `$EDITOR` handoff, offered to the TUI.
//!
//! `jot-tui` declares [`jot_tui::compose::Composer`] as the shape of the favour it needs and
//! implements none of it. This is the implementation, and it is deliberately the *same* one the
//! `jot new` and `jot edit` commands use — the temp file, `$VISUAL` then `$EDITOR`, the parse, and
//! the rule that a draft which does not parse is kept and named.
//!
//! Two surfaces, one editor path. The alternative was moving [`crate::editor`] into `jot-tui`,
//! which would file the `$EDITOR` handoff under the terminal browser and leave the CLI reaching
//! across for it.

use anyhow::Result;
use jot_core::note::NoteId;
use jot_core::query::{Draft, Edit, Field};
use jot_core::workspace::Workspace;
use jot_tui::compose::Composer;

use crate::editor;

/// The `$EDITOR` handoff, as the TUI wants it.
pub struct Editor;

impl Composer for Editor {
    fn compose(
        &self,
        workspace: &Workspace,
        reply_to: Option<NoteId>,
        quote: Option<NoteId>,
    ) -> Result<Option<Draft>> {
        // Seed the block with the relations so the editor opens on something already shaped like
        // the note being written — and so `r` and `q` differ from `n` by what is in the buffer
        // rather than by a flag the writer cannot see.
        let mut frontmatter = jot_core::frontmatter::Frontmatter::new();
        frontmatter.reply_to = reply_to;
        frontmatter.quote = quote;

        let seed = editor::seed(&frontmatter, "\n", &workspace.manifest().schema);
        let edited = editor::edit(workspace.schema(), &seed)?;

        // A buffer left wholly untouched is how every editor-driven tool says "cancel".
        if edited.is_empty() {
            return Ok(None);
        }

        // The buffer is authoritative for everything a new note may declare, `reply_to` included:
        // choosing a parent while writing is normal, and unlike an *edit* it re-parents nothing.
        Ok(Some(Draft {
            body: edited.body,
            title: edited.frontmatter.title.clone(),
            reply_to: edited.frontmatter.reply_to,
            quote: edited.frontmatter.quote,
            // Carries any key the schema declares that jot does not interpret, so a filled-in
            // custom field survives instead of being silently dropped.
            extra: Some(edited.frontmatter.clone()),
            ..Draft::default()
        }))
    }

    fn edit(&self, workspace: &Workspace, id: NoteId) -> Result<Option<Edit>> {
        let note = workspace
            .get(id)?
            .ok_or_else(|| anyhow::anyhow!("no note `{id}` in this workspace"))?;

        let seed = editor::seed(&note.frontmatter, &note.body, &workspace.manifest().schema);
        let edited = editor::edit(workspace.schema(), &seed)?;

        // Not an error, and deliberately not a write: identical bytes still move mtime, and
        // `edited_at` follows mtime, so a no-op save would make every note look recently touched.
        if edited.unchanged {
            return Ok(None);
        }

        // The rule `jot edit` applies, applied here too: a title *or* a body, either alone being
        // enough. Emptying both is not an edit anyone means to make — trashing is how a note goes
        // away — so it is refused rather than written.
        if edited.frontmatter.title.is_none() && edited.body.trim().is_empty() {
            anyhow::bail!("that would leave no title and no body; use x to trash it instead");
        }

        Ok(Some(Edit {
            body: Some(edited.body),
            title: field(edited.frontmatter.title),
            quote: field(edited.frontmatter.quote),
        }))
    }
}

/// `Some` sets, `None` clears — the buffer is authoritative for both.
fn field<T>(value: Option<T>) -> Field<T> {
    match value {
        Some(value) => Field::Set(value),
        None => Field::Cleared,
    }
}
