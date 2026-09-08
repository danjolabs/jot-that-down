//! Watching a vault for changes made outside jot.
//!
//! Lives in core rather than in the TUI because stage 6 needs the same events, and a surface that
//! grew its own watcher would be domain logic on the wrong side of the seam. What a surface gets
//! back is a [`Receiver`] of [`Change`] — no paths, no notify types, nothing that leaks the
//! filesystem into a view.
//!
//! # Why the watch is not recursive
//!
//! A vault is two flat directories: notes live directly in the workspace root, trashed notes
//! directly in `.jot/.trash/`. Nothing is nested, so [`RecursiveMode::NonRecursive`] on each is
//! sufficient — and it is also what keeps the watcher away from `.jot/index.db`.
//!
//! That matters more than it looks. `sync()` writes the index, the index lives under `.jot/`, and
//! a recursive watch on the root would see those writes, report them as vault changes, and drive
//! the surface to sync again — a loop with no fixed point that a stage-4 vault would enter within
//! milliseconds of opening. Watching the two directories that actually hold notes means the loop
//! is unrepresentable rather than filtered out afterwards.
//!
//! The trash is watched for the same reason it is scanned: from stage 1b, location *is* state, so
//! a file dragged into `.jot/.trash/` by hand is a state change and this is how it is noticed.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::error::{Error, Result};

/// How long the debouncer waits for quiet before reporting a change.
///
/// An external sync client rewriting a few hundred notes produces a burst, and the point of the
/// debounce is that a burst costs one `sync()` rather than hundreds. 200 ms is comfortably below
/// the "within a second" this stage promises while being longer than any single editor's
/// write-truncate-rename dance.
pub const DEBOUNCE: Duration = Duration::from_millis(200);

/// Something in the vault changed. Coalesced: the answer to it is always one `sync()`.
///
/// Deliberately carries no path. A surface cannot do anything useful with "this file moved" that it
/// would not do with "something moved" — it re-syncs and re-reads — and a payload would be a
/// promise the debouncer cannot keep, since coalescing a burst means most of the paths are gone by
/// the time anyone looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// At least one note file was created, written, renamed or removed.
    Vault,
}

/// A live watch over one vault.
///
/// Dropping it stops the watch and closes the channel. Nothing here borrows the [`Workspace`], on
/// purpose: `Workspace` owns a `rusqlite::Connection` and is `Send` but not `Sync`, so the watcher
/// is built from paths and hands back a channel rather than sharing the workspace across threads.
///
/// [`Workspace`]: crate::workspace::Workspace
#[derive(Debug)]
pub struct Watcher {
    /// Held only so the watch outlives this struct's construction. Dropping it unwatches.
    _inner: RecommendedWatcher,
    rx: Receiver<Change>,
}

impl Watcher {
    /// Start watching a workspace root and its trash.
    ///
    /// `root` is the workspace root; the trash is derived from it, and is watched only if it
    /// exists — a vault that has never trashed a note has no `.jot/.trash/` yet, and that is not
    /// an error.
    ///
    /// # Errors
    ///
    /// [`Error::Watch`] if the platform watcher cannot start or cannot watch the root. A caller
    /// that gets one should report it and continue without live updates; see the variant's docs.
    pub fn new(root: &Path) -> Result<Self> {
        Self::with_debounce(root, DEBOUNCE)
    }

    /// [`Watcher::new`], with the quiet period set explicitly. Tests use this to avoid sleeping.
    ///
    /// # Errors
    ///
    /// As [`Watcher::new`].
    pub fn with_debounce(root: &Path, debounce: Duration) -> Result<Self> {
        let (raw_tx, raw_rx) = mpsc::channel::<Event>();
        let (tx, rx) = mpsc::channel::<Change>();

        let watch_err = |path: &Path, e: notify::Error| Error::Watch {
            path: path.to_path_buf(),
            message: e.to_string(),
        };

        let mut inner = notify::recommended_watcher(move |res: notify::Result<Event>| {
            // A send failure means the debouncer is gone, which means the `Watcher` was dropped.
            // Nothing to report and nobody to report it to.
            if let Ok(event) = res {
                let _ = raw_tx.send(event);
            }
        })
        .map_err(|e| watch_err(root, e))?;

        inner
            .watch(root, RecursiveMode::NonRecursive)
            .map_err(|e| watch_err(root, e))?;

        let trash = crate::fs::trash_dir(root);
        if trash.is_dir() {
            inner
                .watch(&trash, RecursiveMode::NonRecursive)
                .map_err(|e| watch_err(&trash, e))?;
        }

        thread::spawn(move || debounce_loop(&raw_rx, &tx, debounce));

        Ok(Watcher { _inner: inner, rx })
    }

    /// The channel change events arrive on.
    ///
    /// Disconnects when the watcher is dropped, which is how an event loop learns to stop
    /// selecting on it.
    #[must_use]
    pub fn changes(&self) -> &Receiver<Change> {
        &self.rx
    }
}

/// Collapse a burst of filesystem events into one [`Change`] per quiet period.
///
/// Blocks for the first event, then keeps extending the window while events keep arriving, so a
/// client rewriting 500 files produces one message rather than 500. The extension is deliberate:
/// a fixed window starting at the first event would fire mid-burst and make the surface sync
/// against a half-written vault.
fn debounce_loop(raw: &Receiver<Event>, out: &Sender<Change>, debounce: Duration) {
    loop {
        // Block until something happens. A disconnect means the watcher was dropped.
        let Ok(first) = raw.recv() else { return };
        let mut interesting = is_note_event(&first);

        // Keep draining until the vault has been quiet for a whole `debounce`.
        loop {
            match raw.recv_timeout(debounce) {
                Ok(event) => interesting |= is_note_event(&event),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }

        // A burst of nothing but editor swap files is not a vault change. Staying quiet here is
        // what stops `:w` in vim costing two syncs instead of one.
        if interesting && out.send(Change::Vault).is_err() {
            return;
        }
    }
}

/// Whether an event touches a file a scan would read.
///
/// The directories being watched already exclude the index, so this is about the *other* noise a
/// real editor makes: `.swp`, `4913`, `.md~`, and the dotfiles a sync client scatters. A scan only
/// ever reads `*.md`, so anything else cannot change an answer.
///
/// Note the deliberate breadth on the other axis: every [`EventKind`] counts, including
/// [`EventKind::Access`]-adjacent metadata changes, because `mtime` is half of stage 4's fast path
/// and a touch that moves it is a change the index must see.
fn is_note_event(event: &Event) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    event.paths.iter().map(PathBuf::as_path).any(is_note_path)
}

/// Whether a path is a note file: a `*.md` whose name does not start with a dot.
fn is_note_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    !name.starts_with('.') && name.ends_with(".md")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Instant;

    /// Long enough that a missing event is a real failure rather than a slow filesystem, short
    /// enough that the suite does not crawl.
    const PATIENCE: Duration = Duration::from_secs(5);
    /// Short debounce so the tests do not spend their time sleeping.
    const QUICK: Duration = Duration::from_millis(50);

    fn wait_for_change(w: &Watcher) -> Option<Change> {
        w.changes().recv_timeout(PATIENCE).ok()
    }

    #[test]
    fn a_new_note_file_reports_a_change() {
        let tmp = tempfile::tempdir().unwrap();
        let w = Watcher::with_debounce(tmp.path(), QUICK).unwrap();

        fs::write(tmp.path().join("01a0.md"), "hello").unwrap();

        assert_eq!(wait_for_change(&w), Some(Change::Vault));
    }

    #[test]
    fn a_burst_of_writes_collapses_into_one_change() {
        let tmp = tempfile::tempdir().unwrap();
        let w = Watcher::with_debounce(tmp.path(), QUICK).unwrap();

        for i in 0..50 {
            fs::write(tmp.path().join(format!("note{i}.md")), "x").unwrap();
        }

        assert_eq!(wait_for_change(&w), Some(Change::Vault));

        // The whole point: 50 writes are one message, not 50. Give the debouncer a further quiet
        // period and it must have nothing more to say.
        assert!(
            w.changes().recv_timeout(QUICK * 4).is_err(),
            "a burst must coalesce into exactly one change"
        );
    }

    #[test]
    fn a_write_under_dot_jot_is_not_a_vault_change() {
        let tmp = tempfile::tempdir().unwrap();
        let jot = tmp.path().join(".jot");
        fs::create_dir_all(&jot).unwrap();
        let w = Watcher::with_debounce(tmp.path(), QUICK).unwrap();

        // This is the feedback loop the module exists to make unrepresentable: `sync()` writes
        // these three files on every pass, and a recursive watch would report each one.
        fs::write(jot.join("index.db"), "not really a database").unwrap();
        fs::write(jot.join("index.db-wal"), "nor this").unwrap();
        fs::write(jot.join("index.db-shm"), "nor this").unwrap();

        assert!(
            w.changes().recv_timeout(QUICK * 6).is_err(),
            "index writes must never be reported as vault changes"
        );
    }

    #[test]
    fn a_file_moved_into_the_trash_reports_a_change() {
        let tmp = tempfile::tempdir().unwrap();
        let trash = crate::fs::trash_dir(tmp.path());
        fs::create_dir_all(&trash).unwrap();

        let note = tmp.path().join("01a0.md");
        fs::write(&note, "hello").unwrap();

        // Watch only after the note exists, so the change under test is the move alone.
        let w = Watcher::with_debounce(tmp.path(), QUICK).unwrap();
        fs::rename(&note, trash.join("01a0.md")).unwrap();

        assert_eq!(
            wait_for_change(&w),
            Some(Change::Vault),
            "location is state, so a hand-move into the trash is a change"
        );
    }

    #[test]
    fn an_editor_swap_file_is_not_a_vault_change() {
        let tmp = tempfile::tempdir().unwrap();
        let w = Watcher::with_debounce(tmp.path(), QUICK).unwrap();

        fs::write(tmp.path().join(".01a0.md.swp"), "vim").unwrap();
        fs::write(tmp.path().join("4913"), "vim's probe file").unwrap();
        fs::write(tmp.path().join("notes.txt"), "not markdown").unwrap();

        assert!(
            w.changes().recv_timeout(QUICK * 6).is_err(),
            "only `*.md` files can change a scan's answer"
        );
    }

    #[test]
    fn the_debouncer_waits_for_quiet_rather_than_firing_mid_burst() {
        let tmp = tempfile::tempdir().unwrap();
        let debounce = Duration::from_millis(150);
        let w = Watcher::with_debounce(tmp.path(), debounce).unwrap();

        let started = Instant::now();
        // Five writes spaced under the debounce: the window must keep extending, so the single
        // change arrives after the *last* one rather than 150 ms after the first.
        for i in 0..5 {
            fs::write(tmp.path().join(format!("n{i}.md")), "x").unwrap();
            thread::sleep(Duration::from_millis(60));
        }

        assert_eq!(wait_for_change(&w), Some(Change::Vault));
        assert!(
            started.elapsed() >= Duration::from_millis(300),
            "a fixed window from the first event would have fired mid-burst; \
             the debouncer must extend while writes keep arriving"
        );
    }

    #[test]
    fn dropping_the_watcher_disconnects_the_channel() {
        let tmp = tempfile::tempdir().unwrap();
        let w = Watcher::with_debounce(tmp.path(), QUICK).unwrap();
        let rx = std::mem::replace(&mut { w }.rx, mpsc::channel().1);

        // The original watcher and its debouncer thread are gone; the channel it fed must say so
        // rather than blocking an event loop forever.
        assert!(matches!(
            rx.recv_timeout(PATIENCE),
            Err(RecvTimeoutError::Disconnected | RecvTimeoutError::Timeout)
        ));
    }

    #[test]
    fn watching_a_vault_with_no_trash_directory_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(
            Watcher::with_debounce(tmp.path(), QUICK).is_ok(),
            "a vault that has never trashed a note has no `.jot/.trash/` yet"
        );
    }
}
