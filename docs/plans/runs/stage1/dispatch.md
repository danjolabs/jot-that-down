# Stage 1 dispatch

Authoritative record of who was dispatched, on which model, in which wave — written at dispatch time
from what was actually passed to the `Agent` call. When this file and a commit's `Assisted-by:`
trailer disagree, **this file wins** (`orchestration.md`, Attribution).

Branch: `stage/1-vault-foundations`, cut from `prototype` at `cc99b81`.
Wave plan: `breakdown.md`, accepted without modification.

## Adjudications

Ten underspecifications were flagged by the planner and refused, correctly, at planning time. All ten
are resolved here. Implementers receive the ruling that applies to them; none of them re-decides.

Two were escalated to the user; eight were ruled by the orchestrator.

### U1 — key order versus byte-identical round-trip — USER

**Ruling: preserve on read, normalize on edit.** Two serialization paths, deliberately.

- **Preserving path.** `parse` retains the original frontmatter block, fence to fence, as verbatim
  bytes. Re-serializing an unmodified note emits those bytes unchanged. Byte-identity is therefore
  structural rather than a property the emitter has to earn, and the acceptance criterion
  ("`git diff` is empty") holds for any note whatever its key order, spacing, or scalar style.
- **Canonical path.** Notes this version *creates*, and — from stage 3 — notes it *edits*, are
  emitted with known keys in the fixed order `id, title, created_at, edited_at, reply_to, root,
  quote, trashed_at`, then unknown keys in their original relative order.
- A note therefore reshuffles into canonical form the first time it is genuinely edited, and never
  before. This is the intended behavior, not a leak.

**Consequence for T2.1, and it is a large one.** The YAML crate is no longer load-bearing for the
round-trip gate. It must parse faithfully and emit canonically for notes we author; it does **not**
need to round-trip arbitrary hand-written YAML byte-perfectly, because that path never runs the
emitter. Weight the crate choice accordingly, and say in the decision record that this is why.

### U2 — timestamp representation — orchestrator, follows from U1

Only the canonical path emits timestamps, so the risk of `Z` becoming `+00:00` or an unquoted YAML
timestamp scalar is confined to notes we author. **Canonical output is RFC 3339, UTC, `Z` suffix,
second precision, emitted as a quoted string** so no YAML emitter may reinterpret it as a timestamp
type. In memory, `created_at` / `edited_at` / `trashed_at` are typed values. On the preserving path
their lexical form is irrelevant — the original bytes are re-emitted. T2.1 picks the time crate and
records it with the YAML choice.

### U3 — `init` on an existing workspace — orchestrator

The doc's "idempotent" is the wrong word for what it then describes. **`init` errors** when
`.jot/` already exists at the target, naming the path; it never overwrites. "Existing workspace"
means `.jot/` exists as a directory — not that `workspace.toml` parses, and not that the directory
is non-empty. A target directory containing `.md` files and no `.jot/` is a **valid** init: adopting
a folder of existing markdown is a supported path, not an error. A target directory that does not
exist is created. `name` is not a parameter on `init(path, kind)`; it **defaults to the target
directory's basename** and is display-only, editable in `workspace.toml` by hand.

### U4 — "an interrupted write leaves the original intact" — orchestrator

Scope: **a failure injected between staging and rename.** The test writes a known note, then drives
`atomic_write` through a path where the staged temp file is written and fsynced but the rename does
not occur, and asserts the target's bytes are byte-identical to before. Process-kill and full-disk
simulation are **out of scope for stage 1** — name them in the run log as untested rather than
implying coverage. Asserting only that the tmp file was cleaned up does **not** satisfy this; the
assertion is on the target's contents.

### U5 — registry shape — orchestrator

- Location: OS config dir via `directories`, qualifier triple `("", "danjolabs", "jot")`.
- File: `workspaces.toml`. Format TOML, matching `workspace.toml` so there is one config format.
- **Entries are keyed by the workspace's minted `id`, not by path.** Path is a mutable field on the
  entry. This is the whole point of a self-identifying directory: moving a vault updates a field
  rather than orphaning an entry.
- Fields per entry: `id`, `path`, `name`, `last_opened` (RFC 3339 UTC).
- "The current one" is a single global `current` key holding an id.
- A registered path that no longer exists on disk: **entry retained**, reported as stale by the
  load path, never removed automatically and never an error.
- Load is total: missing file → empty registry; corrupt file → empty registry plus a recoverable
  signal. A bad registry costs one re-add, never data, and must never propagate into `open()`.

### U6 — UUIDv7 ordering — orchestrator

The property that matters is **creation order**: ids minted earlier compare less than ids minted
later, including within one millisecond. The test mints N ids in a tight loop and asserts the
sequence is strictly increasing. `stage1.md`'s parenthetical "(v7 handles this)" is an assumption to
be **verified, not relied on** — plain `Uuid::now_v7()` fills sub-millisecond bits randomly on some
versions, and the monotonic guarantee lives in the counter-carrying context type. T3.1 checks the
`uuid` version actually pinned; if the default is not monotonic it uses the context and says so.

### U7 — does `init` / `open` register the workspace? — orchestrator

**No.** Neither `init` nor `open` touches the registry. A library call with a global filesystem side
effect outside the vault is a testing problem and a surprise. Registration is an explicit
`registry::*` call, wired by the CLI in stage 4.

**This removes T4.1's dependency on T3.3 entirely.** T4.1 now depends only on T3.2.

### U8 — does CI run the acceptance suite? — orchestrator

Yes, as a **separate job** named `acceptance`, running
`cargo test -p jot-acceptance --features stage1` on both matrix platforms, with
`continue-on-error: true` for the duration of the stage — the suite is red by design from wave 2
until wave 4 lands, and it must not block implementers. **Flipped to blocking at seal**, as an
explicit item on the seal checklist. The main `test` job never builds the acceptance crate, which is
what `default-members = ["crates/jot-core"]` buys.

### U9 / U10 — reader strictness — USER

**Ruling: hard errors only. No `Anomaly` type is introduced in stage 1.**

Every deviation is an `Error` naming the path. Specifically:

- Missing `id`, missing `created_at`, missing `root`: **hard error**, one distinct variant each.
- No fence, unterminated fence, malformed YAML: hard error, one distinct variant each.
- Filename UUID disagreeing with frontmatter `id`: **hard error** carrying the path, the filename's
  id, and the frontmatter's id.

**The interpretation that keeps both of `stage1.md`'s sentences true**, and which implementers must
follow exactly:

- Parsing **from bytes** never consults a filename. The frontmatter's `id` is the note's identity,
  unconditionally — this is "the frontmatter wins", and it is a property of the format, not a
  conflict-resolution step.
- Loading **from a path** compares the two and returns the mismatch error. This is "reported by the
  scanner, not silently resolved."

A note without `root` cannot be hand-written and then loaded. That is a real cost of this ruling; it
is accepted, and stage 2's scanner is where a repair path belongs if one is ever wanted.

## Deviations from the planner's breakdown, accepted

- **T4.1 no longer depends on T3.3** (consequence of U7). Wave 4 is unchanged in composition; only
  the DAG edge is dropped.
- The `registry` module addition is **accepted** — doc fidelity is worth less than keeping `open()`
  free of a `directories` dependency, and it fills wave 3 to the ceiling.
- `.gitattributes` (`eol=lf` on fixtures) is **accepted** as real work in T1.1.
- Committing `Cargo.lock` is **accepted**.
- Phase A moves from wave 0 to wave 2, contradicting the worked example in `orchestration.md`. The
  planner's reasoning is correct and the example is wrong; `orchestration.md` gets corrected by the
  scribe at seal.
- `overview.md`'s Windows-rename risk is likely stale. T3.2 verifies rather than assumes and reports
  which it turned out to be; the finding is written back at seal.

## Dispatch log

| Wave | Task | Agent | Model | Effort | Isolation | Status |
| --- | --- | --- | --- | --- | --- | --- |
| 0 | Stage decomposition | stage-planner | claude-opus-5 | high | in place | done |
| 1 | T1.1 scaffold, manifests, CI, fixtures | implementer | claude-sonnet-5 | high | in place | dispatched |

## API contract, pinned at the wave 2/3 boundary

Phase A was written concurrently with T2.1, so the verifier had to guess names it could not see. The
guesses are now reconciled here. **These names are binding on wave 3.** An implementer that prefers a
different name files an appeal; it does not rename unilaterally.

Where T2.1 and the verifier disagreed, **T2.1 wins** — `error.rs` is frozen and landed, and the
acceptance suite is the cheaper of the two to change. That is a contract reconciliation, not a
weakened test.

### Error variants — the verifier must rename, five places

| Phase A wrote | Frozen name in `error.rs` |
| --- | --- |
| `Error::IdMismatch` | `Error::NoteIdMismatch` |
| `Error::MissingFence` | `Error::MissingFrontmatterFence` |
| `Error::UnterminatedFence` | `Error::UnterminatedFrontmatter` |
| `Error::AlreadyAWorkspace` | `Error::WorkspaceExists` |
| `Error::SchemaVersionFromFuture` | `Error::UnsupportedSchemaVersion` |

`NoteIdMismatch { path, filename_id: Uuid, frontmatter_id: Uuid }` — field *contents* are ruled by
U9 and are not negotiable; only the variant name moved.

### `note`

- `NoteId` — newtype over `uuid::Uuid`. `NoteId::new()` mints v7. Exposes `as_uuid(&self) -> Uuid`
  and `impl From<Uuid> for NoteId`, because `error.rs` carries ids as bare `Uuid` and callers must
  be able to build `NoteIdMismatch`. Also `short()` (8-char prefix), `Display`, `FromStr`, `Ord` by
  UUIDv7 timestamp.
- `Note { pub meta: NoteMeta, pub body: String }` — both fields public.
- `NoteMeta` — every known field public, `id` among them.
- `Note::parse(bytes: &[u8]) -> Result<Note>` — **never consults a filename.** Frontmatter `id` wins
  unconditionally. Takes bytes, not `&str`, per U9 and "the exact remaining bytes".
- `Note::load(path: &Path) -> Result<Note>` — parses, then compares the filename's uuid to the
  frontmatter `id` and returns `NoteIdMismatch` on disagreement. Lives in `note`, not `fs`, because
  `fs` may not depend on `note`.
- `Note::to_bytes(&self) -> Vec<u8>` — **preserving path.** Re-emits the retained original
  frontmatter block verbatim. Byte-identical for any unmodified note.
- `Note::to_canonical_bytes(&self) -> Vec<u8>` — **canonical path.** Known keys in the fixed order,
  then unknown keys in original relative order.

### `fs` — resolves the breakdown's contradiction

`breakdown.md` says both "`fs` must not depend on `note`" and that filename parsing "returns the
`NoteId`". T2.1 flagged the conflict rather than resolving it, correctly. **Ruling: `fs` returns
bare `uuid::Uuid`; callers wrap.** Resolving toward `NoteId` would make T3.2 uncompilable until T3.1
lands, destroying the concurrency wave 3 exists to buy.

- `fs::atomic_write(target: &Path, tmp_dir: &Path, bytes: &[u8]) -> Result<()>`
- `fs::parse_note_filename(path: &Path) -> Result<Uuid>`
- `fs::live_note_paths(root: &Path) -> Result<Vec<PathBuf>>`
- `fs::trashed_note_paths(root: &Path) -> Result<Vec<PathBuf>>`

### `workspace`

- `Workspace`, `WorkspaceKind::{Jot, Plain}`.
- `Workspace::root(&self) -> &Path` — required: without an accessor, acceptance criterion 6
  (`discover` from three deep) has no observable at all.

### `registry` — makes U7 testable

U7 ("`init`/`open` never touch the registry") is a negative with no observable, so the verifier
wrote no test for it and reported it UNVERIFIED. **Ruling: the registry API takes an injectable
path**, which makes U7 testable and satisfies U5's "no test touches the real OS config dir" at the
same time.

- `Registry::load_from(path: &Path) -> Result<Registry>` (total; never fails on missing/corrupt)
- `Registry::save_to(&self, path: &Path) -> Result<()>` (via `fs::atomic_write`)
- `registry::default_path() -> Result<PathBuf>` (the `directories` lookup, isolated in one function)

The U7 test then points `load_from` at a temp path, runs `init` and `open`, and asserts it stayed
empty.

### Open shape T3.1 owns, and must report

The relationship between `Note`/`NoteMeta` and `Frontmatter` is deliberately **not** pinned here —
it is T3.1's design call. Consequence, flagged by the verifier: phase A contains **no test that
references `Frontmatter` at all**; every unknown-key assertion goes through emitted bytes instead.
That is a real coverage gap. T3.1 reports the shape it lands, and the verifier closes the gap in
phase B.

Two notes from T2.1 that bear on the shape:

- No serde-lineage YAML crate can be told to quote a scalar. The canonical path must emit the
  known-key prefix itself; only unknown keys go through the crate. `indexmap` was provisioned so
  `Frontmatter` need not expose `yaml_serde::Mapping` in its public API.
- `chrono`'s serde derive writes subsecond precision. Anything emitting a timestamp must call
  `to_rfc3339_opts(SecondsFormat::Secs, true)` explicitly rather than rely on the derive — this
  bites `registry`'s `last_opened` as well as frontmatter.

## Dispatch log, continued

| Wave | Task | Agent | Model | Effort | Isolation | Status |
| --- | --- | --- | --- | --- | --- | --- |
| 2 | T2.1 deps, crate decisions, error taxonomy | implementer | claude-opus-5 | high | worktree | done, merged 7b3d52a |
| 2 | T2.2 phase A acceptance suite | verifier | claude-opus-5 | high | worktree | done, merged 7b96d51, correctly red |
| 2 | wave 2 merge + mechanical gate | integrator | claude-sonnet-5 | high | in place | done, gates 1-3 green, gate 4 red as designed |
