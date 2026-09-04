# Stage 4 — breakdown

Executed **hybrid** (user's call, 2026-09-02): the orchestrator plans and implements inline on
branch `stage4`; a `verifier` subagent owns phase A and phase B. `orchestration.md`'s rule 2 —
whoever implements does not judge — is held by the verifier, not by a separate implementer.

## The shape of the stage

Stage 4 is a substitution behind `snapshot::Snapshot`. The decision that sets the whole plan:

> **The index is the persistence layer under the scan, not a second query engine.**

`Snapshot` stays the thing every read answers from. What SQLite adds is the half the snapshot never
had — `(size, mtime_ns, content_hash)` change detection and a place for a `Record` to survive
between processes. `sync()` becomes: enumerate, skip what has not changed, hydrate those from the
index, read and parse only the rest.

### Why not the SQL queries in stage4.md's "Queries" section

`stage4.md` writes `timeline_roots`, `tree`, `resolve_prefix` and `search` as SQL, with the caveat
"enough to prove the schema; the surfaces come later". The surfaces did not come later — stages 2
and 3 shipped first, and that table's own right-hand column says every one of those queries is
already implemented as a `Snapshot` method, exercised by 423 tests.

Writing them a second time in SQL would produce a duplicate query engine that nothing calls: two
implementations of the timeline's orphan clause, two of prefix resolution, drifting apart. The
hydrated snapshot is fully in memory, so keyset pagination in SQL would buy nothing over the
`BTreeMap` walk that already exists.

**What the schema is proved by instead**: every column is load-bearing for `Record`
reconstruction. `raw` yields the undeclared set, `relations` yields `reply_to`/`quote`, `links`
yields the edge set, `root_id`/`title`/`state` are the derived columns. A column no reconstruction
needs would be dead weight, and there are none.

If profiling at 10k ever says the hydration pass is the cost, the SQL queries land then — behind
the same seam, with the acceptance suite already written. **Deviation, recorded, reversible.**

## Waves

| Wave | Work | Files owned |
| --- | --- | --- |
| A (parallel, throughout) | verifier: phase A acceptance tests | `crates/jot-acceptance/`, `docs/runs/stage4/phase-a.md` |
| 1 | deps; migrations keyed on `user_version`; open pragmas; schema | `Cargo.toml`, `crates/jot-core/Cargo.toml`, `crates/jot-core/src/index/{mod,migrate}.rs`, `index/schema.sql`, `error.rs` |
| 2 | `raw` JSON projection; row read/write; hydration | `frontmatter.rs`, `index/row.rs` |
| 3 | scanner: change detection, deletion pass, duplicate ids | `index/scan.rs`, `snapshot.rs` |
| 4 | wire `sync`/`rebuild`/`reindex`/`forget` | `workspace.rs` |
| gate | fmt, clippy -D warnings, full suite; verifier phase B; `/code-review high` | — |

Wave 1 is alone because migrations, scanner and hydration all depend on the schema —
`orchestration.md`, "Parallelism, and where it actually bites".

## Decisions taken during planning

- **`rusqlite` with `bundled`.** Probed on this machine before committing to it: `cc` finds MSVC
  without `cl` on PATH, and the build is 7s cold. No system SQLite to depend on, on any platform.
- **`blake3`** for `content_hash`, as the schema comment names.
- **`serde_json` reaches `jot-core`**, with `preserve_order`. The workspace manifest says the
  surface-only crates may not reach core; the rationale it gives is about *signatures* — "a `clap`
  or `serde_json` type in a core signature would make the domain depend on how one surface
  presents it". `raw` is a JSON column in a module that is private to the crate, and no
  `serde_json` type appears in any public signature. Flagged for review rather than assumed.
- **`Workspace` loses `Clone, PartialEq, Eq`.** A `Connection` is none of those. Nothing in the
  workspace clones or compares a `Workspace`; `Debug` survives. A derive is not a signature, so
  "the swap is invisible" holds, but it is the one public-API fact this stage changes.
- **`raw` does not change `Frontmatter`.** The JSON is built in `frontmatter.rs` from the interior
  source, as `pub(crate)`, and never stored on the type — `stage4.md` is explicit that the
  in-memory type keeps source text and the index gets the projection.
