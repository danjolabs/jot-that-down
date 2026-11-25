# jot-cli

`jot` — the command-line surface, and the only binary in the workspace. It parses arguments, chooses
a workspace, calls `jot-core`, and formats what comes back. It contains **no domain logic**: every
write is a core `Action`, every answer a core query, and whether a note is valid is core's to say.
`main.rs`'s module docs state the rule; the crate is small enough (about 2.1k lines against core's
6.7k) that the rule is visible in the shape.

## Module dependency graph

An arrow means **depends on**: a `use crate::…` or a `module::` path in compiled code. Everything
below `main.rs` is a leaf.

```text
                  ┌──────────────────────────────────────┐
                  │ main.rs                              │  clap, dispatch, exit codes,
                  │ every command, and `fn main` itself  │  outcomes → messages
                  └──┬──────────────────┬───────────────┬┘
                     │                  │               │
                     ▼                  ▼               ▼
               ┌───────────┐     ┌──────────┐    ┌───────────┐
               │ context   │     │ editor   │    │ output    │
               │ which     │     │ the      │    │ human and │
               │ workspace,│     │ $EDITOR  │    │ --json    │
               │ and why   │     │ loop     │    │ rendering │
               └───────────┘     └──────────┘    └───────────┘
```

Re-derive it after a refactor:

```sh
cd crates/jot-cli/src
for f in *.rs; do
  echo "$(basename "$f" .rs) => $(grep -vE '^[[:space:]]*(//|\*)' "$f" \
    | grep -oE '(crate::|[^a-z_:])(context|editor|output)::' \
    | grep -oE '(context|editor|output)' | sort -u | tr '\n' ' ')"
done
```

## Every edge, exactly

| Module | Lines | Depends on | Core it names | What it answers |
| --- | ---: | --- | --- | --- |
| `context` | 216 | — | `registry` `workspace` `Error` | Which workspace this command acts on, and *which rule chose it* — flag, then `JOT_WORKSPACE`, then discovery, then the registry's current. `--verbose` prints the reason. Prints settings warnings and errors, and turns an old-format refusal into a message with a hint. Owns `JOT_REGISTRY`. |
| `editor` | 263 | — | — | The `$EDITOR` loop: scratch file, `$VISUAL` then `$EDITOR`; a refused buffer is reopened with the reasons as `# jot:` comments, and every way out that did not write keeps the draft and prints its path. Every `std::fs` call in the crate is here, and none touches the workspace. |
| `output` | 382 | — | `registry` `snapshot` and the root re-exports | Rendering. Human format and `--json` are built from the same core values, so they cannot disagree; colour is off when stdout is not a tty or `NO_COLOR` is set, and never carries meaning alone. |
| `main.rs` | 1094 | `context` `editor` `output` | `registry` `snapshot` and the root re-exports | The clap tree, the dispatch, outcome handling, and the exit codes. |

## What the surface is

Every command resolves a workspace through `context` and acts through `Workspace`:

- **Capture and edit** — `new`, `edit`, `remove` (deletes the file; confirms)
- **Read** — `list`, `show`, `check`
- **Batches** — `field fill | remove | rename`
- **Workspaces** — `workspace list | use | add | new | remove | prune`
- **Shell** — `completions`

Exit codes are fixed because scripts depend on them: `0` success, `1` runtime (a refused write
included), `2` usage (clap's), `3` no such note or workspace, `4` ambiguous id or shared
workspace name, `5` `jot check` found problems.

## Two things the picture is hiding

**`context` is the only module allowed to know where a workspace is.** Discovery order is
documented in one function, in one place, and recorded rather than inferred. `main.rs` opens a
`Workspace` itself only for `ws add` and `ws new`, which are handed a path.

**Nothing here reads or writes a note.** `rusqlite` under this crate finds nothing and must keep
finding nothing; `std::fs` finds only `editor.rs`'s scratch file. A hit that touches the workspace
is the regression to look for. Two places come close and stay on the right side: `--set` values are
read as YAML by handing `jot_core::Document::parse` a one-key block, and `field remove` counts the
notes holding a key from `Snapshot::notes` to word its confirmation.

## Layout

Four files, no folders. Tests: `tests/cli.rs` drives the built binary end to end with `assert_cmd`,
including a scripted `$EDITOR` for the refused-buffer loop; the JSON shape it pins is specified in
`docs/cli-json.md`.
