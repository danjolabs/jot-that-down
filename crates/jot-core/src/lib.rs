//! `jot-core` — domain, vault I/O, index, and thread algebra for jot-that-down.
//!
//! Surfaces (CLI, TUI, desktop) never touch the filesystem or SQLite directly; everything goes
//! through this crate's public API. See `docs/plans/overview.md` for the seam this enforces.

pub mod error;
pub mod frontmatter;
pub mod fs;
pub mod note;
pub mod registry;
pub mod workspace;
