//! The `$EDITOR` handoff, as a seam rather than an implementation.
//!
//! Composing a note means: build a seed buffer, write it to a temp file, launch `$VISUAL` or
//! `$EDITOR`, read back what returned, parse it, and keep the draft on disk if it does not parse.
//! All of that already exists in `jot-cli`, written carefully in stage 3, and none of it is
//! terminal-UI work.
//!
//! So this crate does not do it. [`Composer`] is the shape of the favour the TUI needs, `jot-cli`
//! implements it over the code it already has, and neither surface grows a second copy that can
//! drift from the first. The alternative — moving `editor.rs` into this crate — would put the
//! `$EDITOR` handoff behind a crate named for the terminal browser, where the CLI would then be
//! reaching for it.
//!
//! It is also what keeps [`crate::app::App`] testable: a test supplies a `Composer` that returns a
//! canned draft, and the whole capture path runs with no editor and no terminal.

use jot_core::note::NoteId;
use jot_core::query::{Draft, Edit};
use jot_core::workspace::Workspace;

/// Whatever this surface uses to let someone write a note.
///
/// Both methods return `Option`: `None` is an abandoned capture, which is a normal outcome and not
/// an error. An editor exiting with an untouched buffer is how every editor-driven tool has always
/// said "cancel", and it must cost nothing.
///
/// # Errors
///
/// Both return `Err` only when something genuinely failed — no editor configured, it could not be
/// launched, it exited non-zero, or what came back does not parse. The implementation is expected
/// to preserve the user's draft in that last case and name where it is; losing what someone just
/// typed is the one failure this whole program exists to prevent.
pub trait Composer {
    /// Write a new note. `reply_to` and `quote` seed the frontmatter block.
    ///
    /// # Errors
    ///
    /// See the trait docs.
    fn compose(
        &self,
        workspace: &Workspace,
        reply_to: Option<NoteId>,
        quote: Option<NoteId>,
    ) -> anyhow::Result<Option<Draft>>;

    /// Edit an existing note.
    ///
    /// # Errors
    ///
    /// See the trait docs.
    fn edit(&self, workspace: &Workspace, id: NoteId) -> anyhow::Result<Option<Edit>>;
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// A `Composer` that never opens an editor and returns whatever it was built with.
    ///
    /// The point of the trait: the capture path is exercised end to end — key press, pending
    /// request, draft, `Workspace::create`, reload, selection — with no `$EDITOR` and no pty.
    pub struct Canned {
        /// What [`Composer::compose`] returns.
        pub draft: Option<Draft>,
        /// What [`Composer::edit`] returns.
        pub edit: Option<Edit>,
    }

    impl Canned {
        /// A composer that captures a note with this title.
        pub fn titled(title: &str) -> Self {
            Canned {
                draft: Some(Draft::new("").title(title)),
                edit: None,
            }
        }

        /// A composer that abandons whatever it is asked to do.
        pub fn abandoned() -> Self {
            Canned {
                draft: None,
                edit: None,
            }
        }
    }

    impl Composer for Canned {
        fn compose(
            &self,
            _workspace: &Workspace,
            reply_to: Option<NoteId>,
            quote: Option<NoteId>,
        ) -> anyhow::Result<Option<Draft>> {
            // The relations are applied here rather than baked into `draft`, so a test can assert
            // that `r` and `q` actually reached the composer.
            Ok(self.draft.clone().map(|mut draft| {
                draft.reply_to = reply_to;
                draft.quote = quote;
                draft
            }))
        }

        fn edit(&self, _workspace: &Workspace, _id: NoteId) -> anyhow::Result<Option<Edit>> {
            Ok(self.edit.clone())
        }
    }
}
