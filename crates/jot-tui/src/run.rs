//! Terminal setup, the event loop, and teardown.
//!
//! The thin shell around [`App`]. Everything here is I/O — raw mode, the alternate screen,
//! blocking on input — and none of it decides anything, which is why the interaction tests drive
//! [`App::dispatch`] directly and this module has almost nothing to test.
//!
//! # First paint precedes the first sync
//!
//! Stage 4 measured a cold open of a 10k vault at 689 ms, and this stage's budget for the first
//! frame is 200 ms. Those cannot both hold if the vault is synced before anything is drawn, so the
//! order here is: open the terminal, draw whatever the index already knows, *then* sync and
//! redraw. The skeleton is not a nicety; it is the only way the budget is meetable.
//!
//! # The terminal is restored even on a panic
//!
//! A panic with raw mode still on leaves the user's shell unusable — no echo, no line editing,
//! and a cursor that is still hidden. The hook installed by [`run`] restores the terminal before
//! the message prints, so a crash costs a backtrace rather than a `reset`.

use std::io::{self, Stdout};
use std::panic;
use std::time::Duration;

use anyhow::{Context as _, Result};
use chrono::Utc;
use jot_core::watch::Watcher;
use jot_core::workspace::Workspace;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::crossterm::{ExecutableCommand, cursor};

use crate::app::{App, Pending};
use crate::compose::Composer;
use crate::key::Resolved;
use crate::preview::Bat;
use crate::ui;

/// How long the loop blocks on input before waking to check the watcher.
///
/// The watcher's own channel is not selectable alongside crossterm's input, so the loop polls.
/// 100 ms keeps "updates within a second" comfortable while leaving the process asleep essentially
/// all of the time — an idle TUI should not be a busy loop.
const TICK: Duration = Duration::from_millis(100);

/// Open the TUI over an already-opened workspace, and return when the user quits.
///
/// # Errors
///
/// If the terminal cannot be put into raw mode or the alternate screen, or if drawing fails.
/// A workspace that cannot be watched is **not** an error — see [`jot_core::error::Error::Watch`].
pub fn run(ws: Workspace, composer: &dyn Composer) -> Result<()> {
    let mut terminal = setup().context("cannot start the terminal UI")?;

    // Everything after setup runs with the terminal captured, so failures have to be caught and
    // the terminal restored before they propagate. `restore` is idempotent.
    let outcome = event_loop(&mut terminal, ws, composer);
    restore();
    outcome
}

/// Put the terminal into the state a full-screen app needs, and arrange to undo it.
fn setup() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    // Installed before raw mode, so a panic between here and the first draw is still cleaned up.
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));

    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    io::stdout().execute(cursor::Hide)?;
    Ok(Terminal::new(CrosstermBackend::new(io::stdout()))?)
}

/// Undo [`setup`]. Safe to call twice, and deliberately ignores its own failures — there is
/// nowhere useful to report them, and trying harder risks masking the original error.
fn restore() {
    let _ = disable_raw_mode();
    let _ = io::stdout().execute(cursor::Show);
    let _ = io::stdout().execute(LeaveAlternateScreen);
}

/// Draw, wait, dispatch, repeat.
fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ws: Workspace,
    composer: &dyn Composer,
) -> Result<()> {
    // A failed watch costs live updates, not the session. The message reaches the user as a toast
    // on the first frame rather than as a refusal to open.
    let watch = Watcher::new(ws.root());

    // The real terminal is the one place a highlighter may be shelled out to, so this is where
    // `Bat` gets installed. `App::new` alone stays subprocess-free, which is what keeps the
    // interaction tests from depending on what happens to be on `$PATH`.
    let mut app = App::new(ws).with_highlighter(Box::new(Bat));

    // First paint, before the sync. See the module docs.
    terminal.draw(|frame| ui::draw(frame, &app, Utc::now()))?;

    app.sync();
    if let Err(err) = &watch {
        app.set_toast_error(format!("{err}; live updates are off"));
    }
    let watch = watch.ok();

    loop {
        // Before the draw, not during it: painting the reader means rendering markdown, rendering
        // markdown may mean spawning `bat`, and `ui` is pure. The width comes from the same
        // `split_main` the draw is about to use, so the text is wrapped for the panel it lands in.
        //
        // Skipped while input is still queued. A held `j` arrives as a burst of keystrokes, and
        // highlighting a note nobody will look at costs a process launch per frame — which is
        // exactly the stutter this stage promises a 10k vault will not have. The panel catches up
        // on the first frame after the burst, and labels itself from what it is showing meanwhile.
        if !event::poll(Duration::ZERO)? {
            let size = terminal.size()?;
            app.prepare_preview(ui::reader_text_width(size.width, size.height));
        }

        terminal.draw(|frame| ui::draw(frame, &app, Utc::now()))?;

        if event::poll(TICK)? {
            match event::read()? {
                // `KeyEventKind` matters on Windows, where a key press and its release both
                // arrive: without this filter every keystroke acts twice, which is the single
                // most common Windows-only TUI bug.
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    let mode = app.mode();
                    let resolved = app.keymap().resolve(key, mode);
                    if let Resolved::Act(action) = resolved {
                        app.dispatch(action);
                    }
                }
                Event::Resize(_, _) => { /* the next draw already reflows */ }
                _ => {}
            }
        }

        if let Some(pending) = app.take_pending() {
            serve(terminal, &mut app, composer, pending)?;
        }

        if let Some(watcher) = &watch {
            drain_watcher(watcher, &mut app);
        }

        if app.should_quit() {
            return Ok(());
        }
    }
}

/// Do the thing `App` asked for but cannot do itself. See [`Pending`].
fn serve(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    composer: &dyn Composer,
    pending: Pending,
) -> Result<()> {
    match pending {
        Pending::Compose { reply_to, quote } => {
            let outcome = with_terminal_released(terminal, || {
                composer.compose(app.workspace(), reply_to, quote)
            })?;
            match outcome {
                Ok(draft) => app.create(draft),
                Err(err) => app.set_toast_error(err.to_string()),
            }
        }

        Pending::EditNote(id) => {
            let outcome = with_terminal_released(terminal, || composer.edit(app.workspace(), id))?;
            match outcome {
                Ok(edit) => app.apply_edit(id, edit),
                Err(err) => app.set_toast_error(err.to_string()),
            }
        }
    }
    Ok(())
}

/// Hand the terminal back, run `f`, then take it again and repaint from scratch.
///
/// An editor needs the real terminal: raw mode off so it sees line discipline, the alternate
/// screen left so it gets its own, and the cursor visible so you can see what you are typing.
/// Leaving any of those set gives `$EDITOR` a terminal it cannot use.
///
/// `clear()` on the way back is not optional. The editor drew over the alternate screen, and
/// ratatui's next frame only paints the cells it believes changed — so without it the browser
/// comes back with the editor's leftovers showing through wherever the two happen to agree.
fn with_terminal_released<T>(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    f: impl FnOnce() -> T,
) -> Result<T> {
    restore();
    let outcome = f();

    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    io::stdout().execute(cursor::Hide)?;
    terminal.clear()?;
    Ok(outcome)
}

/// Apply whatever the watcher has to say, coalescing anything that queued up.
///
/// The watcher already debounces, but a burst that spans two polls can still leave two messages
/// waiting; syncing once for all of them is the point.
fn drain_watcher(watcher: &Watcher, app: &mut App) {
    let mut changed = false;
    while let Ok(_change) = watcher.changes().try_recv() {
        changed = true;
    }
    if changed {
        app.sync();
    }
}

/// Unused today, kept honest: the loop's tick is what bounds "within a second".
const _: () = assert!(TICK.as_millis() <= 1000);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tick_keeps_the_within_a_second_promise() {
        // Stage 5 promises an external edit shows up "within a second". The budget is the
        // watcher's debounce plus one tick, and this is the arithmetic that has to keep holding
        // if either constant is tuned.
        let worst_case = jot_core::watch::DEBOUNCE + TICK;
        assert!(
            worst_case < Duration::from_secs(1),
            "debounce ({:?}) + tick ({TICK:?}) = {worst_case:?}, which misses the promise",
            jot_core::watch::DEBOUNCE
        );
    }

    #[test]
    fn an_idle_loop_sleeps_rather_than_spinning() {
        // `event::poll` blocks for the tick when nothing arrives. A zero tick would turn the loop
        // into a busy wait that pins a core on an idle TUI, and this is the guard against it.
        assert!(TICK > Duration::ZERO);
    }
}
