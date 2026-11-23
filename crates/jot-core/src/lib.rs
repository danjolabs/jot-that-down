//! `jot-core` — the schema-first workspace engine behind jot-that-down.
//!
//! A workspace is a flat folder of markdown files with typed frontmatter: `workspace.toml` and
//! `schema.json` at the root declare it, the files are the source of truth, and SQLite under
//! `.jot/` is a derived, disposable index. See `docs/plans/redesign.md`.
//!
//! Surfaces never touch the filesystem or SQLite; everything goes through [`workspace::Workspace`].
//! Mutations are [`action::Action`]s, decided purely and then executed; what happened comes back
//! as [`action::Event`]s.

pub mod action;
pub mod document;
pub mod error;
pub(crate) mod fs;
pub mod id;
// Private to the crate, and that is the seam: there is no `&Index` to hand a surface.
mod index;
pub mod problem;
pub mod registry;
pub mod settings;
pub mod shortid;
pub mod snapshot;
pub mod watch;
pub mod workspace;

pub use action::{Action, Event, Failure, Outcome, Plan, Rejection};
pub use document::{Document, Fields};
pub use error::{Error, Result};
pub use id::{IdFormat, NoteId};
pub use problem::Problem;
pub use snapshot::{Hash, Listing, Ordering, Row, Snapshot};
pub use workspace::{Note, Workspace};
