# crates

Four crates. One of them knows what a note is; the rest are ways of looking at one.

| Crate | Lines | Kind | What it is |
| --- | ---: | --- | --- |
| [`jot-core`](jot-core/README.md) | ~14k | lib | The domain, the vault I/O, the index, the thread algebra. Everything jot knows how to do. |
| [`jot-tui`](jot-tui/README.md) | ~3.9k | lib | The terminal reading surface. No `fn main`, on purpose. |
| [`jot-cli`](jot-cli/README.md) | ~2.5k | **bin** | `jot`. The only executable in the workspace. |
| `jot-acceptance` | — | lib | Stage acceptance criteria, executable. Owned by the verifier; read-only to implementers. |

## The stack

Dependencies are unidirectional, and the direction is **core → tui → cli** — not core → cli → tui.
`jot-tui` is a library with no binary of its own, so the CLI sits *above* it: `jot tui` is a
subcommand that hands an already-opened `Workspace` down.

```text
   ┌──────────────────────────────────────────────────────────────┐
   │ jot-cli                          bin `jot`                   │
   │ argv → workspace → core → text.  Owns `fn main`, and with it │
   │ the process, the exit codes, and $EDITOR.                    │
   └────────────┬─────────────────────────────┬───────────────────┘
                │ `jot_tui::run(ws, &Editor)` │ every command
                ▼                             │
   ┌──────────────────────────────────┐       │
   │ jot-tui                     lib  │       │      ┌─────────────────┐
   │ keys → state → cells.            │       │      │ jot-desktop     │
   │ Cannot open a workspace: it has  │       │      │ (deferred)      │
   │ no `fn main` to open one from.   │       │      │ Tauri commands, │
   └────────────┬─────────────────────┘       │      │ same rules      │
                │                             │      └────────┬────────┘
                ▼                             ▼               ▼
   ┌──────────────────────────────────────────────────────────────┐
   │ jot-core                                                lib  │
   │ `Workspace` is the whole door. Markdown files are the truth; │
   │ the SQLite index is derived and disposable.                  │
   └──────────────────────────────────────────────────────────────┘

   jot-acceptance ─── depends on jot-core, feature-gated per stage.
                      Nothing depends on it.
```

`jot-cli → jot-tui` is the only surface-to-surface edge, and it carries exactly two names.

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
| `Watcher`, `Change`, `DEBOUNCE` — external edits. `jot-tui` is the only caller | `watch` |
| shortest-unique-prefix ids | `shortid` |
| `Error` | `error` |
| `index` | **not exposed.** `mod index;`, so no surface can name it |

`jot-cli` names nine of these, `jot-tui` four. That difference is the honest measure of the two
surfaces: the CLI renders every shape the domain has, the TUI drives `Workspace` and `Row`.

### `jot-tui` → `jot-cli`

Six public modules, and the CLI uses two names out of all of them:

- **`run(ws: Workspace, composer: &dyn Composer) -> Result<()>`** — the entry point. Takes an opened
  workspace, because it has no way to open one.
- **`compose::Composer`** — a trait this crate declares and does not implement. `jot-cli` implements
  it over the `$EDITOR` code it already had in stage 3, so `jot new` and the TUI's `Space n` are one
  path, not two that can drift.

`app`, `key`, `preview` and `ui` are `pub` for tests and for documentation, not for the CLI. A
`jot_tui::app::` or `jot_tui::ui::` path appearing under `crates/jot-cli/src` is a layering
regression.

### `jot-cli` → nothing

It is the top. A binary with a `[[bin]]` target and no `[lib]`, so nothing can depend on it.

## The rule the whole thing rests on

**Surfaces never touch the filesystem or SQLite.** `docs/plans/overview.md` locks it; the crate
graph makes it checkable rather than aspirational:

```sh
grep -rn 'rusqlite'  crates/jot-cli crates/jot-tui --include='*.rs'   # must stay empty
grep -rn 'std::fs'   crates/jot-cli/src crates/jot-tui/src            # exactly 4, all in jot-cli
```

The four are the scratch file `editor.rs` hands `$EDITOR` (three) and the parent directory of the
registry that `context.rs` creates (one). All four are outside the vault. A fifth hit that touches a
note is the regression to look for. `jot-tui` has zero, and `preview.rs` explains what it cost to
keep it that way: `bat` takes the markdown over stdin rather than being handed a path.

## Adding a surface

The desktop plan's own risk register names the failure mode: *logic leaking into the frontend, which
quietly undoes stages 1–4.* That plan is [deferred](../docs/plans/todo/desktop.md) and the rule is
not — it is what makes deferring a surface cost nothing. The test is one question, asked of every
new function in a surface:

> **Should the TUI have this too?**

If yes, it belongs in `jot-core`. `watch` is the worked example — the TUI needed it first, the
desktop app needs the same events, so it was written in core with a `Change` payload that carries no
paths and no `notify` types. A watcher grown inside `jot-tui` would have been domain logic on the
wrong side of the seam, and the desktop app would have grown a second one.

The three structural habits worth copying into any new surface crate:

- **No `fn main` unless you own the process.** `jot-tui` cannot acquire a workspace by accident
  because it has nowhere to acquire one from. Throughout these READMEs `fn main` is the process
  entry point and `main.rs` is `jot-cli`'s largest module; they are not the same claim.
- **Declare the favour, don't implement it.** `Composer` and `Highlighter` are both traits with a
  test double, so the impure parts — an editor, a subprocess — sit outside the tested path.
- **Take core's types, don't mirror them.** A surface-local `struct NoteView` is where two surfaces
  start disagreeing about what a note is.
