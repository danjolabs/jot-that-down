# crates

Three crates. One of them knows what a note is; one is a way of looking at it; one checks the first.

| Crate | Lines | Kind | What it is |
| --- | ---: | --- | --- |
| [`jot-core`](jot-core/README.md) | ~14k | lib | The domain, the vault I/O, the index, the thread algebra. Everything jot knows how to do. |
| [`jot-cli`](jot-cli/README.md) | ~2.5k | **bin** | `jot`. The only executable in the workspace. |
| `jot-acceptance` | — | lib | Stage acceptance criteria, executable. Existing assertions change only with review. |

## The stack

Dependencies are unidirectional: **core → cli**. A terminal browser (`jot-tui`) was built in stage 5
and has since been removed; `jot` is CLI-only.

```text
   ┌──────────────────────────────────────────────────────────────┐
   │ jot-cli                          bin `jot`                   │
   │ argv → workspace → core → text.  Owns `fn main`, and with it │
   │ the process, the exit codes, and $EDITOR.                    │
   └────────────────────────────┬─────────────────────────────────┘
                                │ every command      ┌─────────────────┐
                                │                    │ jot-desktop     │
                                │                    │ (deferred)      │
                                │                    │ Tauri commands, │
                                │                    │ same rules      │
                                │                    └────────┬────────┘
                                ▼                             ▼
   ┌──────────────────────────────────────────────────────────────┐
   │ jot-core                                                lib  │
   │ `Workspace` is the whole door. Markdown files are the truth; │
   │ the SQLite index is derived and disposable.                  │
   └──────────────────────────────────────────────────────────────┘

   jot-acceptance ─── depends on jot-core, feature-gated per stage.
                      Nothing depends on it.
```

## What each layer exposes

### `jot-core` → every surface

Twelve public modules and one private one. The rule is not "call these modules" but **every
operation on the vault goes through `Workspace`**; the other modules exist because they are what
`Workspace`'s signatures are made of.

| What a surface reaches for | Where it comes from |
| --- | --- |
| `Workspace` — open, sync, create, edit, remove, restore, purge, every query | `workspace` |
| `NoteId`, `Note`, `NoteMeta` | `note` |
| `Draft`, `Edit`, `Row`, `Page`, `State`, `Ref`, `FileSort`, the query structs | `query` |
| `Thread`, `Segment`, `TreeNode` | `thread` |
| `Link` | `link` |
| `Frontmatter`, `FrontmatterSchema` | `frontmatter` |
| `Snapshot`, `Problem` | `snapshot` |
| `Registry`, `Entry` — the workspace list. `jot-cli` is the only caller | `registry` |
| `Watcher`, `Change`, `DEBOUNCE` — external edits. No caller since `jot-tui` was removed | `watch` |
| shortest-unique-prefix ids | `shortid` |
| `Error` | `error` |
| `index` | **not exposed.** `mod index;`, so no surface can name it |

`jot-cli` names nine of these.

### `jot-cli` → nothing

It is the top. A binary with a `[[bin]]` target and no `[lib]`, so nothing can depend on it.

## The rule the whole thing rests on

**Surfaces never touch the filesystem or SQLite.** `docs/plans/overview.md` locks it; the crate
graph makes it checkable rather than aspirational:

```sh
grep -rn 'rusqlite'  crates/jot-cli --include='*.rs'   # must stay empty
grep -rn 'std::fs'   crates/jot-cli/src                 # exactly 4
```

The four are the scratch file `editor.rs` hands `$EDITOR` (three) and the parent directory of the
registry that `context.rs` creates (one). All four are outside the vault. A fifth hit that touches a
note is the regression to look for.

## Adding a surface

The desktop plan's own risk register names the failure mode: *logic leaking into the frontend, which
quietly undoes stages 1–4.* That plan is [deferred](../docs/plans/todo/desktop.md) and the rule is
not — it is what makes deferring a surface cost nothing. The test is one question, asked of every
new function in a surface:

> **Should another surface have this too?**

If yes, it belongs in `jot-core`. `watch` is the worked example — the (since removed) TUI needed it
first, the desktop app needs the same events, so it was written in core with a `Change` payload that
carries no paths and no `notify` types.

The three structural habits worth copying into any new surface crate:

- **No `fn main` unless you own the process.** A library surface cannot acquire a workspace by
  accident because it has nowhere to acquire one from. Throughout these READMEs `fn main` is the process
  entry point and `main.rs` is `jot-cli`'s largest module; they are not the same claim.
- **Declare the favour, don't implement it.** Put the impure parts — an editor, a subprocess —
  behind a trait with a test double, so they sit outside the tested path.
- **Take core's types, don't mirror them.** A surface-local `struct NoteView` is where two surfaces
  start disagreeing about what a note is.
