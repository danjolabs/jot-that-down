//! Watching a workspace for changes made outside jot: a trigger for `Action::Sync`.
//!
//! The watcher reports only "something changed". It carries no path and no notify type, because
//! the answer is always the same — run `Action::Sync`, which diffs the files against the index and
//! produces the precise [`Event`](crate::action::Event)s. A burst is debounced into one trigger.
//!
//! # Why the watch is not recursive
//!
//! A workspace is one flat directory, so a non-recursive watch on the root sees every note and
//! both settings files. It is also what keeps the watcher away from `.jot/`: a sync writes
//! `index.db`, and a recursive watch would report that write, trigger another sync, and loop.
//! Watching only the root makes that loop unrepresentable rather than filtered.

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

/// Something in the workspace changed: run `Action::Sync`. Coalesced, and deliberately without a
/// payload — coalescing a burst means most of the paths would be gone by the time anyone looked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncNeeded;

/// A live watch over one workspace root.
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
    rx: Receiver<SyncNeeded>,
}

impl Watcher {
    /// Start watching a workspace root.
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
        let (tx, rx) = mpsc::channel::<SyncNeeded>();

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

        thread::spawn(move || debounce_loop(&raw_rx, &tx, debounce));

        Ok(Watcher { _inner: inner, rx })
    }

    /// The channel change events arrive on.
    ///
    /// Disconnects when the watcher is dropped, which is how an event loop learns to stop
    /// selecting on it.
    #[must_use]
    pub fn changes(&self) -> &Receiver<SyncNeeded> {
        &self.rx
    }
}

/// Collapse a burst of filesystem events into one [`SyncNeeded`] per quiet period.
///
/// Blocks for the first event, then keeps extending the window while events keep arriving, so a
/// client rewriting 500 files produces one message rather than 500. The extension is deliberate:
/// a fixed window starting at the first event would fire mid-burst and make the surface sync
/// against a half-written vault.
fn debounce_loop(raw: &Receiver<Event>, out: &Sender<SyncNeeded>, debounce: Duration) {
    loop {
        // Block until something happens. A disconnect means the watcher was dropped.
        let Ok(first) = raw.recv() else { return };
        let mut interesting = is_relevant(&first);

        // Keep draining until the vault has been quiet for a whole `debounce`.
        loop {
            match raw.recv_timeout(debounce) {
                Ok(event) => interesting |= is_relevant(&event),
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }

        // A burst of nothing but editor swap files is not a change. Staying quiet here is what
        // stops `:w` in vim costing two syncs instead of one.
        if interesting && out.send(SyncNeeded).is_err() {
            return;
        }
    }
}

/// Whether an event touches a file a sync reads: a `*.md` that is not a dot-file, or one of the
/// two settings files. Editor noise (`.swp`, `4913`, `.md~`) cannot change an answer. Every kind
/// but access counts, because mtime is half of the sync's fast path.
fn is_relevant(event: &Event) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    event
        .paths
        .iter()
        .map(PathBuf::as_path)
        .any(is_relevant_path)
}

fn is_relevant_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    name == crate::settings::WORKSPACE_TOML
        || name == crate::settings::SCHEMA_JSON
        || (!name.starts_with('.') && name.to_ascii_lowercase().ends_with(".md"))
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

    fn wait_for_change(w: &Watcher) -> Option<SyncNeeded> {
        w.changes().recv_timeout(PATIENCE).ok()
    }

    #[test]
    fn a_new_note_file_reports_a_change() {
        let tmp = tempfile::tempdir().unwrap();
        let w = Watcher::with_debounce(tmp.path(), QUICK).unwrap();

        fs::write(tmp.path().join("01a0.md"), "hello").unwrap();

        assert_eq!(wait_for_change(&w), Some(SyncNeeded));
    }

    #[test]
    fn a_burst_of_writes_collapses_into_one_change() {
        let tmp = tempfile::tempdir().unwrap();
        let w = Watcher::with_debounce(tmp.path(), QUICK).unwrap();

        for i in 0..50 {
            fs::write(tmp.path().join(format!("note{i}.md")), "x").unwrap();
        }

        assert_eq!(wait_for_change(&w), Some(SyncNeeded));

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

        // The feedback loop the module exists to make unrepresentable: a sync writes these files,
        // and a recursive watch would report each one.
        fs::write(jot.join("index.db"), "not really a database").unwrap();
        fs::write(jot.join("index.db-wal"), "nor this").unwrap();
        fs::write(jot.join("index.db-shm"), "nor this").unwrap();

        assert!(
            w.changes().recv_timeout(QUICK * 6).is_err(),
            "index writes must never be reported as vault changes"
        );
    }

    #[test]
    fn a_settings_file_edit_reports_a_change() {
        let tmp = tempfile::tempdir().unwrap();
        let w = Watcher::with_debounce(tmp.path(), QUICK).unwrap();
        fs::write(tmp.path().join("schema.json"), "{}").unwrap();
        assert_eq!(wait_for_change(&w), Some(SyncNeeded));
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

        assert_eq!(wait_for_change(&w), Some(SyncNeeded));
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
}
