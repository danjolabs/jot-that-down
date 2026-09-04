//! `jot-tui` — the terminal reading surface.
//!
//! A library, not a binary: `jot-cli` owns the `jot` executable and hands this crate an already
//! opened [`Workspace`]. That is the seam doing its job at the crate boundary — with no `main`
//! there is nowhere for a workspace-opening path to grow, so the TUI cannot acquire one by
//! accident, and the rule that surfaces never touch the filesystem or SQLite stays enforceable by
//! the compiler rather than by review.
//!
//! [`run`] is the whole public entry point.
//!
//! [`Workspace`]: jot_core::workspace::Workspace

pub mod app;
pub mod compose;
pub mod key;
pub mod preview;
pub mod run;
pub mod ui;

pub use run::run;
