//! `jot-core` — domain, vault I/O, index, and thread algebra for jot-that-down.
//!
//! Surfaces (CLI, TUI, desktop) never touch the filesystem or SQLite directly; everything goes
//! through this crate's public API. See `docs/plans/overview.md` for the seam this enforces.

pub mod error;
pub mod frontmatter;
pub mod fs;
pub mod link;
pub mod note;
pub mod query;
pub mod registry;
pub mod snapshot;
pub mod thread;
pub mod workspace;
