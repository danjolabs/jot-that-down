# jot-cli

`jot` — the command-line surface, and the only binary in the workspace. It parses arguments, chooses
a workspace, calls `jot-core`, and formats what comes back. It contains **no domain logic**:
`main.rs`'s module docs state the rule, and the crate is small enough (2512 lines against core's
14k) that the rule is visible in the shape.

It also owns the `jot` executable for the *terminal* surface: `jot tui` calls `jot_tui::run` with an
already-opened `Workspace`. `jot-tui` is a library with no `main`, so the CLI is the layer above it,
not below — see `crates/README.md`.

## Module dependency graph

An arrow means **depends on**: a `use crate::…` or a `module::` path in compiled code. Four modules
and five edges; there is no tier structure worth drawing, because everything below `main` is a leaf
except `compose`.

```text
                  ┌──────────────────────────────────────┐
                  │ main                                  │  clap, dispatch, exit codes.
                  │ the binary; every command lives here  │  Depends on all four.
                  └──┬────────┬───────────┬────────────┬──┘
                     │        │           │            │
         ┌───────────┘        │           │            └───────────┐
         ▼                    ▼           ▼                        ▼
   ┌───────────┐       ┌───────────┐  ┌──────────┐          ┌───────────┐
   │ context   │       │ compose   │  │ editor   │          │ output    │
   │ which     │       │ the       │  │ the      │          │ human and │
   │ vault,    │       │ `Composer`│─▶│ $EDITOR  │          │ --json    │
   │ and why   │       │ jot-tui   │  │ handoff  │          │ rendering │
   └───────────┘       │ asked for │  └──────────┘          └───────────┘
                       └───────────┘
                             │  implements `jot_tui::compose::Composer`
                             ▼  — the only edge that points at another surface
```

Re-derive it after a refactor:

```sh
cd crates/jot-cli/src
for f in *.rs; do
  echo "$(basename "$f" .rs) => $(grep -vE '^[[:space:]]*(//|\*)' "$f" \
    | grep -oE '(crate::|[^a-z_:])(compose|context|editor|output)::' \
    | grep -oE '(compose|context|editor|output)' | sort -u | tr '\n' ' ')"
done
```

## Every edge, exactly

| Module | Lines | Depends on | Core modules it names | What it answers |
| --- | ---: | --- | --- | --- |
| `context` | 187 | — | `registry` `workspace` | Which vault this command acts on, and *which rule chose it* — flag, then `JOT_WORKSPACE`, then discovery, then the registry's current. `--verbose` prints the reason, because a note captured into the wrong vault is the one failure nothing else reports. Also owns `JOT_REGISTRY`. |
| `editor` | 309 | — | `frontmatter` `note` | The `$EDITOR` handoff: temp file, `$VISUAL` then `$EDITOR`, parse the buffer back. The four `std::fs` calls in this crate are three of them, and none touches the vault. |
| `output` | 607 | — | `link` `note` `query` `registry` `snapshot` `thread` | Rendering. Human format and `--json` are built from the same core values, so they cannot disagree; colour is off when stdout is not a tty or `NO_COLOR` is set, and never carries meaning alone. |
| `compose` | 94 | `editor` | `frontmatter` `note` `query` `workspace` | The `jot_tui::compose::Composer` impl. One `$EDITOR` path serves `jot new`, `jot edit` and the TUI's `Space n`. |
| `main` | 1315 | `compose` `context` `editor` `output` | `frontmatter` `note` `query` `registry` `shortid` `workspace` | The clap tree, the dispatch, and the exit codes. |

## What the surface is

Commands, one line each. All of them resolve a workspace through `context` and act through
`Workspace`:

- **Capture and edit** — `new`, `edit`, `remove`, `restore`, `purge`
- **Read** — `list`, `show`, `thread`, `search`, `links`, `trash`
- **Vaults** — `workspace list | use | add | new | remove | prune`
- **Index** — `index status`, `index rebuild`
- **Other surfaces** — `tui`, `completions`

Exit codes are fixed because scripts depend on them: `0` success, `1` runtime, `2` usage (clap's),
`3` no such note or workspace, `4` ambiguous id prefix.

## Three things the picture is hiding

**`compose` is an inversion, not a dependency on the TUI.** `jot-tui` declares the trait; this crate
implements it over `editor`, which already existed in stage 3. The arrow in the graph points from
`jot-cli` to a *shape* `jot-tui` published, and no terminal code runs on the CLI's own `$EDITOR`
path. The alternative — moving `editor.rs` into `jot-tui` — would file the `$EDITOR` handoff under
the terminal browser and leave the CLI reaching across for it.

**`context` is the only module allowed to know where a vault is.** Discovery order is documented in
one function, in one place, and recorded rather than inferred. `main` never opens a `Workspace`
itself.

**Nothing here reads or writes a note.** `rusqlite` under this crate finds nothing and must keep
finding nothing. `std::fs` finds four lines: three in `editor.rs` for the scratch file handed to
`$EDITOR`, one in `context.rs` creating the parent of the registry. A fifth hit that touches the
vault is the regression to look for.

## Layout

Five files, no folders. Tests: `tests/cli.rs` drives the built binary end to end with `assert_cmd`;
the JSON shape it pins is specified in `docs/cli-json.md`.
