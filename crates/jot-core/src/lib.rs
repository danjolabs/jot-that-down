//! `jot-core` — domain, vault I/O, index, and thread algebra for jot-that-down.
//!
//! Surfaces (CLI, TUI, desktop) never touch the filesystem or SQLite directly; everything goes
//! through this crate's public API. See `docs/plans/overview.md` for the seam this enforces.

pub mod error;
pub mod frontmatter;
pub mod fs;
// Private to the crate, and that is the seam: `overview.md` puts SQLite behind `jot-core` and
// nothing else. There is no `&Index` to hand a surface, which is what stops the index's
// *representation* becoming part of the API the way `&Snapshot` briefly did.
mod index;
pub mod link;
pub mod note;
pub mod query;
pub mod registry;
pub mod shortid;
pub mod snapshot;
pub mod thread;
pub mod workspace;
