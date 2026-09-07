# jot-tui

The terminal reading surface. **A library, not a binary** — `jot-cli` owns the `jot` executable and
hands this crate an already-opened `Workspace`. With no `main` there is nowhere for a
workspace-opening path to grow, so "surfaces never touch the filesystem or SQLite" is enforced by
the compiler at the crate boundary rather than by review.

`run` is the whole public entry point.

## Module dependency graph

An arrow means **depends on**: a `use crate::…` in compiled code. Doc-comment links and
`#[cfg(test)]` imports are excluded, and it matters here — `app` names `compose::Composer` a dozen
times in prose and in tests, and does not depend on it.

```text
                          jot-cli
                          calls `run(workspace, &composer)` and nothing else
                             │
                             ▼
      ┌────────────────────────────────────────────────┐
      │ run                                            │  terminal setup, the event
      │ the only I/O in the crate                      │  loop, teardown, the panic
      └──┬──────────┬───────────────┬───────────┬──────┘  hook that restores the tty
         │          │               │           │
         │          ▼               │           │
         │    ┌───────────┐         │           │
         │    │ ui        │         │           │  pure: state in, cells out
         │    └──┬─────┬──┘         │           │
         │       │     │            │           │
         ▼       ▼     │            │           │
      ┌────────────────┴─┐          │           │
      │ app              │          │           │  every state transition; holds
      │ state + dispatch │          │           │  the `Workspace`, no terminal
      └──┬────────────┬──┘          │           │
         │            │             │           │
         ▼            ▼             ▼           ▼
   ┌──────────┐  ┌──────────┐  ┌──────────┐  (nothing)
   │ key      │  │ preview  │  │ compose  │
   │ keymap + │  │ `bat`,   │  │ the trait│  no dependencies at all;
   │ prefix   │  │ or plain │  │ jot-cli  │  `compose` is a request, not
   └──────────┘  └──────────┘  │ fills in │  an implementation
                               └──────────┘
```

Re-derive it after a refactor:

```sh
cd crates/jot-tui/src
for f in *.rs; do
  echo "$(basename "$f" .rs) => $(grep -vE '^[[:space:]]*(//|\*)' "$f" \
    | grep -oE 'crate::[a-z_]+' | sed 's/crate:://' | sort -u | tr '\n' ' ')"
done
```

That script counts the `#[cfg(test)]` edges too; `app => compose` is the one to discount.

## Every edge, exactly

| Module | Lines | Depends on | Core modules it names | What it answers |
| --- | ---: | --- | --- | --- |
| `compose` | 107 | — | `note` `query` `workspace` | The `Composer` trait: the shape of the `$EDITOR` favour this crate needs and deliberately does not implement. `jot-cli` provides it; tests provide a canned draft, which is what lets the capture path run with no editor and no terminal. |
| `key` | 944 | — | — | Keys → `Action`, and the `Space` prefix state machine. Every key that writes to the vault sits behind the prefix and nothing else does. `?`'s help overlay is generated from the same table the loop dispatches on, so an undocumented binding is unrepresentable. |
| `preview` | 263 | — | — | The reader panel's styled text. `Bat` borrows the user's `bat` (then `batcat`, then `cat`, then `Plain`); markdown goes in over **stdin**, never a path, so this crate never opens a vault file. `Plain` is deterministic and is what every test renders through. |
| `app` | 1361 | `key` `preview` | `note` `query` `workspace` | Application state and the reduction of an `Action` onto it. No terminal, no clock, no `$EDITOR` — what needs those becomes a `Pending` for `run` to answer. Timeline, files, search and trash are one selection model over four `Vec<Row>` sources; thread detail is the one view shaped differently. |
| `ui` | 921 | `app` `key` | `query` | Drawing. Pure — a frame is a function of `App`, which is what makes the snapshot tests worth having. Owns the layout thresholds (the reader panel is dropped below 90 columns). |
| `run` | 251 | `app` `compose` `key` `preview` `ui` | `watch` `workspace` | Raw mode, the alternate screen, the 100 ms poll, teardown, and the panic hook. Decides nothing. First paint precedes the first sync, because a cold 10k vault syncs in 689 ms and the frame budget is 200. |
| `lib` | 20 | `run` | `workspace` | Declares the six modules and re-exports `run`. |

## Three things the picture is hiding

**`app` does not depend on `compose`, and that is the design.** `App` cannot open an editor — it
records a `Pending::Compose` or `Pending::EditNote` and `run` gives the screen back to `$EDITOR` and
answers it. That is why the entire interaction model is testable by pressing keys at `App::dispatch`
and reading the state back, with no pty and no sleeping.

**`preview` and `compose` are both seams for the same reason.** Each is the point where the draw
path would otherwise become non-deterministic — spawning `bat`, spawning an editor. Both are traits
with a test double, so the snapshot suite tests this crate rather than the tester's `$PATH`.

**The watcher is core's, not this crate's.** `run` consumes `jot_core::watch::Watcher`, which hands
back a `Receiver<Change>` carrying no paths and no `notify` types. Stage 6 needs the same events; a
surface that grew its own would be domain logic on the wrong side of the seam. `crossterm` is
likewise never a direct dependency — it comes through `ratatui::crossterm`, so there is only ever
one `KeyEvent` type in the build.

## Layout

Six files plus `lib.rs`, no folders. Tests: interaction tests live beside `app`; `tests/render.rs`
plus `tests/snapshots/` pin the frames with `insta`.
