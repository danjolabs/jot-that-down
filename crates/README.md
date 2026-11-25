# crates

Two crates. One of them knows what a workspace is; the other is how you talk to it.

| Crate | Lines | Kind | What it is |
| --- | ---: | --- | --- |
| [`jot-core`](jot-core/README.md) | ~6.1k | lib | The engine: settings, documents, validation, actions and events, the index. Everything jot knows how to do. |
| [`jot-cli`](jot-cli/README.md) | ~2.0k | **bin** | `jot`. The only executable in the workspace. |

## The stack

Dependencies are unidirectional: **core → cli**.

```text
   ┌──────────────────────────────────────────────────────────────┐
   │ jot-cli                          bin `jot`                   │
   │ argv → workspace → core → text.  Owns `fn main`, and with it │
   │ the process, the exit codes, and $EDITOR.                    │
   └────────────────────────────┬─────────────────────────────────┘
                                │ every command
                                ▼
   ┌──────────────────────────────────────────────────────────────┐
   │ jot-core                                                lib  │
   │ `Workspace` is the whole door. Markdown files are the truth; │
   │ the SQLite index is derived and disposable.                  │
   └──────────────────────────────────────────────────────────────┘
```

## What `jot-core` exposes

The rule is not "call these modules" but **every operation on a workspace goes through
`Workspace`**; the other modules exist because `Workspace`'s signatures are made of them.

| What a surface reaches for | Where it comes from |
| --- | --- |
| `Workspace`, `Note` — open, decide/execute/apply, sync, subscribe, every read | `workspace` |
| `Action`, `Plan`, `Outcome`, `Rejection`, `Failure`, `Event` | `action` |
| `Document`, `Fields` | `document` |
| `NoteId`, `IdFormat` | `id` |
| `Problem` | `problem` |
| `Settings`, roles | `settings` |
| `Snapshot`, `Row`, `Listing`, `Ordering`, `Hash` | `snapshot` |
| `Registry` — the workspace list. `jot-cli` is the only caller | `registry` |
| `Watcher` — "something changed", the trigger for `Sync`. No caller yet | `watch` |
| short ids (`01a0...eefa`) | `shortid` |
| `Error` | `error` |
| `index`, `fs` | **not exposed**, so no surface can name them |

`jot-cli` is the top: a `[[bin]]` target with no `[lib]`, so nothing can depend on it.

## The rule the whole thing rests on

**Surfaces never touch the filesystem or SQLite.** `docs/plans/overview.md` locks it; the crate
graph makes it checkable:

```sh
grep -rn 'rusqlite'  crates/jot-cli --include='*.rs'   # must stay empty
grep -rn 'std::fs'   crates/jot-cli/src                 # exactly 5, all in editor.rs
```

The five are the scratch file `editor.rs` hands `$EDITOR`: written, read back, removed. It lives in
the OS temp directory, outside any workspace. A hit that touches a note is the regression to look
for.

## Adding a surface

The test is one question, asked of every new function in a surface:

> **Should another surface have this too?**

If yes, it belongs in `jot-core`. `watch` is the worked example: any long-running surface needs to
know when files change, so it was written in core, yielding a payload with no paths and no `notify`
types, and `Sync` turns it into precise events.

Three habits worth copying into any new surface:

- **No `fn main` unless you own the process.** A library surface cannot acquire a workspace by
  accident because it has nowhere to acquire one from.
- **Put the impure parts behind a seam you can test around** — an editor, a subprocess.
- **Take core's types, don't mirror them.** A surface-local `struct NoteView` is where two surfaces
  start disagreeing about what a note is.
