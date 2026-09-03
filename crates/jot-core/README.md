# jot-core

The domain, the vault I/O, the index, and the thread algebra. Everything jot knows how to do lives
here. The surfaces — `jot-cli`, `jot-tui`, the Tauri backend — call this crate and never touch the
filesystem or SQLite themselves; `docs/plans/overview.md` calls that the seam the project rests on.

## Module dependency graph

Tiers, top to bottom. An arrow means **depends on**, and a module also depends on everything below
it that the tier under it depends on — the exact edge list is the table further down, because a
picture that drew all 36 edges would be unreadable.

Only real code dependencies count: a `use crate::…` in compiled code. A doc-comment link and a
`#[cfg(test)]` import are both excluded, and both matter — `query` mentions `workspace` a dozen
times in prose and does not depend on it, and `snapshot` imports `Workspace` in its tests only.

```text
      jot-cli    jot-tui    jot-desktop
      every operation goes through `Workspace`; only `index` is unreachable
                      │
                      ▼
      ┌────────────────────────────────────┐
      │ workspace                          │   the public facade — every operation
      └───────┬──────────────────┬─────────┘   a surface performs goes through it
              │                  │
              │                  │
              │                  ▼
              │         ┌────────────────────────┐
              │         │ index                  │   SQLite: change detection and
              │         │ migrate · row · scan   │   persistence. Private to the
              │         └────────┬───────────────┘   crate — no surface can name it
              │                  │
              │                  │
              ▼                  ▼
      ┌────────────────────────────────────┐
      │ snapshot                           │   the vault in memory. Every read
      └───────────────┬────────────────────┘   answers from here; the index is
                      │                        the cache underneath it
          ┌───────────┼───────────┬─────────────┐
          ▼           ▼           ▼             ▼
      ┌───────┐  ┌─────────┐  ┌───────┐   ┌───────────┐
      │ query │  │ thread  │  │ link  │   │ shortid   │  no dependencies at all
      └───┬───┘  └────┬────┘  └───┬───┘   └───────────┘
          └───────────┼───────────┘
                      ▼
      ┌──────────────────────────────────────────┐
      │ note  ◀──────────▶  frontmatter          │   mutually dependent, and
      └───────────────┬──────────────────────────┘   deliberately so — see below
                      │
                      ▼
                ┌───────────┐         ┌────────────┐  the workspace list, in the config dir.
                │ fs        │◀────────│ registry   │  Nothing in this crate depends on it —
                └─────┬─────┘         └─────┬──────┘  `jot-cli` is the only caller
                      │                     │
                      ▼                     │
                ┌───────────┐               │
                │ error     │◀──────────────┘ the leaf: depends on nothing
                └───────────┘
```

Re-derive it after a refactor:

```sh
for f in crates/jot-core/src/*.rs crates/jot-core/src/index/*.rs; do
  echo "$(basename "$f" .rs) => $(grep -vE '^[[:space:]]*(//|\*)' "$f" \
    | grep -oE 'crate::[a-z_]+' | sed 's/crate:://' | sort -u | tr '\n' ' ')"
done
```

## Every edge, exactly

| Module | Lines | Depends on | What it answers |
| --- | ---: | --- | --- |
| `error` | 719 | — | The crate-wide taxonomy. Every variant names the file or note it concerns; there is no `Other(String)` and no blanket `From<io::Error>`. |
| `shortid` | 198 | — | Shortest unambiguous prefixes. A pure algorithm, and the only other module that depends on nothing. |
| `fs` | 1259 | `error` | Atomic writes, note-filename parsing, directory enumeration. Deliberately ignorant of what a note *is*. |
| `note` | 782 | `error` `frontmatter` `fs` | `NoteId`, `Note`, `NoteMeta`. Identity comes from the filename and nowhere else. |
| `frontmatter` | 2178 | `error` `note` | The block: the declared schema, parsing, and rendering that preserves unknown keys byte-for-byte. |
| `link` | 357 | `note` | `[[uuid]]` extraction from a body. |
| `thread` | 622 | `note` | The reply tree — paths, segments, sibling order. |
| `query` | 626 | `frontmatter` `fs` `note` | The vocabulary of a read: `Draft`, `Edit`, `Row`, `Page`, `State`, `Ref`, and the query structs. |
| `registry` | 1285 | `error` `fs` | The list of known workspaces, in the user's config directory. |
| `snapshot` | 1738 | `error` `frontmatter` `fs` `link` `note` `query` `shortid` `thread` | The vault as a `BTreeMap`, plus every query over it. The derived `root` walk lives here. |
| `index` | 1673 | `error` `frontmatter` `fs` `note` `query` `snapshot` | SQLite: change detection and persistence. Private to the crate. |
| `workspace` | 3448 | `error` `frontmatter` `fs` `index` `link` `note` `query` `snapshot` `thread` | The public facade. Every operation a surface performs goes through it. |

## Four things the picture is hiding

**`note` and `frontmatter` are mutually recursive, and that is not an accident.** `frontmatter`
needs `NoteId` to type a `relation:` value; `note` needs `Frontmatter` to be a note at all. Rust
allows a cycle inside a crate, and breaking this one would mean inventing a third module to hold a
single type — strictly more structure for strictly no gain. It is the only cycle; every other edge
is acyclic.

**`index` depends on `snapshot`, never the reverse.** The index reconstructs a `snapshot::Record`
and hands it up; the snapshot has never heard of SQLite. That direction is what makes deleting
`.jot/index.db` a non-event, and it is the first thing to check if the two ever look tangled.

**`registry` has no caller inside this crate.** It hangs off the side of the graph because nothing
here uses it: `workspace.rs` says why in its module docs (§U7) — neither `init` nor `open` consults
or writes the registry, so there is no `use crate::registry` anywhere in `jot-core`. `jot-cli` is
the only consumer. A grep that finds one appearing later is a design change, not a tidy-up.

**`index` is the one module that is not `pub`.** `lib.rs` declares it `mod index;`, so there is no
`&Index` to hand a surface and no way for one to depend on the index's *representation* — which is
what `&Snapshot` briefly was, before stage 4 took it back.

The seam is about *behaviour*, not about names: `jot-cli` imports types from nine of these modules —
`query` and `note` most of all — because that is what `Workspace`'s signatures are made of. What it
never does is act on *the vault* except by calling `Workspace`. `rusqlite` under `crates/jot-cli`
finds nothing and must keep finding nothing. `std::fs` finds four lines, and all four are outside
the vault: `editor.rs` writes the scratch file it hands `$EDITOR`, and `context.rs` creates the
parent directory of the registry. A fifth hit that touches a note is the regression to look for.

## Layout

Files are grouped into a folder only when a module grows private submodules that share one public
face. `index/` is the only one that has: `migrate`, `row` and `scan` are implementation detail
behind `Index`, and none of them is reachable from outside the crate. The rest are single files
because a folder that merely groups peers would rename every public path — `jot_core::note::NoteId`
becoming `jot_core::format::note::NoteId` — for no change in what depends on what.
