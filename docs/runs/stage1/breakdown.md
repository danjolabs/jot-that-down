# Stage 1 breakdown

Planned against the repo as it is on `stage/1-vault-foundations`: no Rust code, no `Cargo.toml`, no
`.github/`. Everything below is greenfield, which is why the ownership sets can be clean and why the
serialization constraints are almost entirely about *shared* files (`Cargo.toml`, `lib.rs`,
`error.rs`) rather than about domain coupling.

Local toolchain confirmed: `rustc 1.97.1 (8bab26f4f 2026-07-14)`, `cargo 1.97.1`.

## Task DAG

```text
T1.1 scaffold + manifests + CI + fixtures        (wave 1, sonnet, in place)
  ├── T2.1 deps + YAML/time crate decision + error taxonomy   (wave 2, opus,   worktree)
  │     ├── T3.1 NoteId / Note / NoteMeta / frontmatter       (wave 3, opus,   worktree)
  │     ├── T3.2 atomic write / filename parse / enumerate    (wave 3, opus,   worktree)
  │     └── T3.3 workspace registry                           (wave 3, sonnet, worktree)
  │           └── T4.1 init / open / discover / workspace.toml (wave 4, opus,  in place)
  │                 (also depends on T3.2)
  └── T2.2 verifier phase A acceptance tests                  (wave 2, opus,   worktree)
```

`T4.1` depends on `T3.2` (atomic write) and `T3.3` (registry). It does **not** depend on `T3.1`.

## Waves

### Wave 1 — scaffold (one task, blocks everything)

| Task | Owns | Model | Why this model |
| --- | --- | --- | --- |
| T1.1 Cargo workspace, toolchain pin, CI matrix, module skeleton, shared fixture vault | `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `.gitignore`, `.gitattributes`, `.github/**`, `crates/jot-core/Cargo.toml`, `crates/jot-core/src/lib.rs`, `crates/jot-core/src/{note,frontmatter,fs,error,registry,workspace}.rs` (empty module stubs), `tests/fixtures/**` | sonnet | Bounded and obviously right or obviously broken; the routing table names "Cargo workspace, CI yaml, toolchain pinning" as sonnet, and fixtures as "volume work with a clear spec". |

**Owns the dependency manifests this wave.** T1.1 is the only task in wave 1, so this is trivially
satisfied; the constraint that matters is that it lands `Cargo.toml` with an *empty or near-empty*
dependency set, because T2.1 makes the actual crate choices.

**Done means.** A virtual workspace manifest exists at the repo root with `members = ["crates/*"]`,
`default-members = ["crates/jot-core"]`, `resolver = "3"`, `edition = "2024"`, and a
`[workspace.package]` block, so that adding `crates/jot-acceptance/` later requires no edit to the
root manifest and `cargo test` at the root never builds the acceptance crate. `crates/jot-core` is a
library crate whose `src/lib.rs` declares exactly `pub mod error; pub mod frontmatter; pub mod fs;
pub mod note; pub mod registry; pub mod workspace;` and nothing else — no crate-root `pub use`
re-exports this stage (see Shared contracts). Each named module file exists with a `//!` doc comment
and no items. `rust-toolchain.toml` pins `channel = "1.97.1"` with `components = ["rustfmt",
"clippy"]`. `.github/workflows/ci.yml` runs a matrix of `windows-latest` and `ubuntu-latest` on
push and PR, each running `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D
warnings`, and `cargo test`, using the pinned toolchain; the acceptance-suite job is a separate,
explicitly-named job (see Underspecified §U8). `.gitattributes` forces `eol=lf` on `tests/fixtures/**`
so that a Windows checkout cannot break the byte-identical round-trip gate before a single line of
parser code is written. `tests/fixtures/vault/` contains the corpus enumerated in Shared contracts
below, hand-written, with the invalid specimens in a sibling `tests/fixtures/invalid/` so that
enumeration tests over the vault never trip on a file that is deliberately unparseable. `cargo fmt
--check`, `cargo clippy -- -D warnings`, and `cargo test` all pass on Windows against the empty
skeleton.

### Wave 2 — decisions and the contract (two agents, disjoint)

| Task | Owns | Model | Why this model |
| --- | --- | --- | --- |
| T2.1 Dependency landing, YAML crate choice, timestamp representation choice, crate error taxonomy | `Cargo.toml`, `Cargo.lock`, `crates/jot-core/Cargo.toml`, `crates/jot-core/src/error.rs`, `docs/plans/stages/stage1.md`, `docs/runs/stage1/yaml-crate.md` | opus | The YAML crate and the timestamp type decide whether byte-identical round-trip is even achievable; failure here is silent data mangling discovered in stage 7. |
| T2.2 Phase A acceptance tests | `crates/jot-acceptance/**` (exclusive, whole crate incl. its `Cargo.toml`) | opus | Verification is never routed to sonnet; its whole value is skepticism about work that looks finished. |

**Owns the dependency manifests this wave: T2.1.** T2.2 owns `crates/jot-acceptance/Cargo.toml`,
which is a manifest but not a *shared* one — no other task in this or any later stage-1 wave may
create or touch that crate, so there is no contention. T2.2 must not add anything to the root
`[workspace.dependencies]`; if it needs a dev-dependency it declares it in its own crate manifest.

**T2.1 done means.** A written, dated decision record exists for the YAML crate: which crate, which
alternatives were evaluated, and — the part that matters — evidence from a throwaway spike run over
`tests/fixtures/vault/` that the chosen crate can reproduce every fixture's frontmatter block
byte-for-byte on an unmodified parse→serialize cycle, or an explicit statement of which fixtures it
cannot and why. `serde_yaml` is archived and out; the maintained serde forks and the AST-level
emitters (`saphyr` / `yaml-rust2` lineage) are different bets, and the round-trip gate may not be
reachable through a serde `Serializer` at all — that is the finding the spike exists to produce. The
same task decides how `created_at` / `edited_at` are represented in memory (see §U2) and lands that
decision in the same record. The one-line choice plus the date is appended to the "Pick a maintained
YAML crate" bullet in `docs/plans/stages/stage1.md`, as that file asks. The full stage-1 dependency set is
landed in root `[workspace.dependencies]` and referenced from `crates/jot-core/Cargo.toml` via
`workspace = true`: `uuid` (features `v7`, `serde`), `serde` (feature `derive`), the chosen YAML
crate, `toml` (or `toml_edit`, T2.1's call, for `workspace.toml`), `thiserror`, `directories`, the
chosen time crate if any, plus dev-dependency `tempfile`. `crates/jot-core/src/error.rs` contains
the complete flat stage-1 `Error` enum and `pub type Result<T> = std::result::Result<T, Error>`,
with one variant per failure mode named in `stage1.md` — every variant carrying the `PathBuf` or
`NoteId` it concerns, because "a message that says only 'parse error' is a bug". At minimum:
missing fence, unterminated fence, malformed YAML, missing required field, unreadable/unwritable
path, invalid note filename, not a workspace, already a workspace, manifest parse failure,
schema version from the future, no workspace found while walking up, registry unreadable. No
catch-all `Other(String)` variant. The spike is discarded; the only code that lands is `error.rs`
and the manifests. Wave 3 treats `error.rs` as frozen.

**T2.2 done means.** One test per named criterion in `stage1.md`'s Acceptance section, each named
after the criterion so a red result maps back to the doc line, written against the module paths
fixed in Shared contracts below rather than guessed. The crate is `crates/jot-acceptance`, picked up
by the root `members = ["crates/*"]` glob, excluded from `default-members`, with a `stage1` feature
so `cargo test -p jot-acceptance --features stage1` is the invocation. It will not compile at the
end of wave 2 — nothing it calls exists yet — and that is the correct state. Where a criterion
cannot be turned into a test because the doc does not say what to call or what to observe, T2.2 says
so and stops rather than inventing an API; §U1, §U3, §U4 and §U5 below are the four places I expect
that to happen, and having them listed here is not permission to skip reporting them.

### Wave 3 — the core modules (three agents, the ceiling)

| Task | Owns | Model | Why this model |
| --- | --- | --- | --- |
| T3.1 `NoteId`, `Frontmatter`, `Note`, `NoteMeta`, parse and serialize | `crates/jot-core/src/note.rs`, `crates/jot-core/src/frontmatter.rs`, `tests/fixtures/**` | opus | Routing table: "Frontmatter round-trip (S1) — opus. Silent data mangling; the expensive failure." |
| T3.2 Atomic write, filename parsing, enumeration | `crates/jot-core/src/fs.rs` | opus | Routing table: "atomic writes (S1) — opus"; platform-divergent behavior where a wrong result is a lost note, not a failed build. |
| T3.3 Workspace registry in the OS config dir | `crates/jot-core/src/registry.rs` | sonnet | Bounded serde-over-TOML with an explicit never-fatal degradation rule; a mistake costs one re-add, and the gate catches it. Escalate to opus if it comes back red twice. |

**Owns the dependency manifests this wave: nobody.** T2.1 landed the full set. Any wave-3 task that
finds it needs a crate that is not there must stop and report rather than edit `Cargo.toml` — that
is a serialized amendment by Fable, not a unilateral edit, precisely because three agents are
holding the same `Cargo.lock` in three worktrees.

**T3.1 done means.** `NoteId` is a newtype over `Uuid` with parse, `Display`, `short()` returning the
first 8 characters of the hyphenated form, `serde` round-trip as a plain string, and an `Ord` that
matches UUIDv7 timestamp order. Minting is `NoteId::new()`; the stage doc asserts that two ids minted
in the same millisecond still order deterministically and that "v7 handles this" — verify that claim
against the `uuid` crate's actual behavior before relying on it, because plain `Uuid::now_v7()` fills
the sub-millisecond bits randomly and the monotonic guarantee lives in the counter-carrying context
type (`ContextV7` / `Timestamp::now(ctx)`); if the default is not monotonic, use the context and say
so in the report (see §U6). `Frontmatter` holds the typed known fields — `id`, `title`, `created_at`,
`edited_at`, `reply_to`, `root`, `quote`, `trashed_at` — plus an **order-preserving** map of unknown
keys and their verbatim values. Parse splits on the leading `---` fence, deserializes the block, and
keeps the body as the exact remaining bytes, with distinct errors from `error.rs` for no fence,
unterminated fence, malformed YAML, and a missing required field, each naming the path. Serialize
emits known keys in a fixed order then unknown keys in their original relative order — *except* that
the byte-identical gate in the same doc contradicts a fixed order for a hand-written file whose keys
arrive in a different one; resolve per Fable's answer to §U1, do not pick silently. `Note { meta:
NoteMeta, body: String }` where `NoteMeta` is everything but the body. The gate is a test that walks
every file under `tests/fixtures/vault/` (including `.jot/.trash/`), parses, re-serializes, and
asserts byte equality; failures are bugs in the writer, not in the fixture. New fixtures may be added
by this task; nobody else touches that directory this wave.

**T3.2 done means.** `fs::atomic_write(target: &Path, tmp_dir: &Path, bytes: &[u8]) -> Result<()>`
stages into `tmp_dir`, flushes and `fsync`s the staged file, renames over `target`, and returns an
error naming the path on any failure — taking the tmp directory as a parameter rather than reading it
off a `Workspace` is what lets this task run before T4.1 exists. There is a test that overwrites an
existing file and it is **run on Windows and the platform is named in the report**; `overview.md`
records the risk as "`std::fs::rename` fails on Windows when the target exists", which is probably no
longer true of modern `std` (it maps to `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`), so the job
is to verify rather than to assume, and to record which it turned out to be. There is a test that an
interrupted write leaves the original file byte-intact — see §U4 for what "interrupted" is allowed to
mean, and if the answer is not settled, write the test against a failure injected between staging and
rename and flag the rest. Filename parsing accepts `<uuid>.md` and `<uuid>_<slug>.md`, returns the
`NoteId`, and discards the slug as decorative. Enumeration lists live notes in the workspace root and
trashed notes in `.jot/.trash/`, non-recursively for `jot` kind, skipping `.jot/` and every dotfile,
returning paths — not parsed notes, so this module never depends on T3.1. All tests use `tempfile`
temp directories; this task does not add to `tests/fixtures/`.

**T3.3 done means.** A registry of known workspaces — path, display name, last-opened timestamp —
plus a notion of the current one, persisted under the OS config directory located via `directories`.
The load path is total: a missing file yields an empty registry, and a corrupt or partially-written
file yields an empty registry plus a recoverable signal, never an error that propagates to a caller
trying to open a workspace — the registry is a cache and a bad one costs one re-add, never data. The
save path uses the same staged-then-renamed discipline as `fs::atomic_write` (call it; do not
reimplement it — T3.2 lands it in the same wave, so if it is not there yet, block rather than fork).
Every test injects an explicit registry file path and **no test reads or writes the real OS config
directory** — two agents running the suite concurrently on one machine would otherwise race on a
single global file, and so would a developer's own registry. The concrete file name, serialization
format, and `directories` qualifier/organization/application triple are unspecified by the stage doc
(§U5); pick, document in the module's doc comment, and report the choice.

### Wave 4 — the workspace (one task)

| Task | Owns | Model | Why this model |
| --- | --- | --- | --- |
| T4.1 `init` / `open` / `discover`, `workspace.toml`, `.jot/` tree | `crates/jot-core/src/workspace.rs` | opus | Routing table: "Workspace resolution — opus. A note captured into the wrong vault is silently lost." |

**Owns the dependency manifests this wave: nobody.**

**Done means.** `init(path, kind)` creates exactly the tree in `stage1.md`'s on-disk contract —
`.jot/`, `.jot/.trash/`, `.jot/tmp/`, `.jot/workspace.toml`, `.jot/.gitignore` containing `index.db*`
and `tmp/` — mints a UUIDv7 workspace id, and writes the manifest with `schema_version = 1`, the
`[workspace]` block (`id`, `kind`, `name`) and `[notes] filename = "uuid"`. Running it against a
directory that already contains `.jot/` is an error naming the path, never a silent overwrite (the
doc calls this "idempotent", which it is not; see §U3). `open(path)` reads and validates the
manifest and refuses a `schema_version` greater than the one this build knows with a message that
says plainly that the workspace was written by a newer version. `discover(from)` walks parent
directories looking for `.jot/` and is proven by a test that starts three directories deep, per the
acceptance criterion. Manifest writes go through `fs::atomic_write` with `.jot/tmp/` as the staging
directory. Whether `init` and `open` also record into the registry is unsettled (§U7): implement what
Fable rules, and if unruled, leave registration to an explicit call and say so in the report.

## Shared contracts

Implementers see only their own task, so anything two tasks must agree on is fixed here rather than
negotiated at integration time.

- **Module paths are the API surface this stage.** `jot_core::note::{NoteId, Note, NoteMeta}`,
  `jot_core::frontmatter::Frontmatter`, `jot_core::fs::*`, `jot_core::workspace::Workspace`,
  `jot_core::registry::*`, `jot_core::error::{Error, Result}`. **No crate-root `pub use` re-exports
  in stage 1.** This is not aesthetic: `lib.rs` is the one file every task would otherwise want to
  append a line to, and freezing it after wave 1 removes the collision entirely. A curated crate-root
  prelude is a stage-2 task, once the names have stopped moving.
- **`error.rs` is frozen after wave 2.** All three wave-3 tasks and T4.1 consume
  `crate::error::{Error, Result}` and add no variants.
- **`fs::atomic_write(target: &Path, tmp_dir: &Path, bytes: &[u8]) -> Result<()>`** — tmp directory is
  a parameter, not derived from a `Workspace`. This signature is what decouples T3.2 from T4.1.
- **Enumeration returns paths, not notes.** `fs` must not depend on `note` or `frontmatter`.
- **The fixture vault is `tests/fixtures/vault/` at the repo root**, shared by `jot-core` tests and
  `jot-acceptance`, reached from either crate via `env!("CARGO_MANIFEST_DIR")` and a `../..` hop
  behind a small local helper. Invalid specimens live in `tests/fixtures/invalid/` so that
  enumeration and round-trip walks over the vault never trip over a file that is deliberately
  unparseable. Add to it; never fork it.
- **Fixture corpus T1.1 must land** (names indicative, content is what matters):
  `vault/.jot/workspace.toml`; `vault/.jot/.trash/<uuid>.md` carrying `trashed_at`;
  `vault/<uuid>.md` with only the required keys; `vault/<uuid>_first_thoughts.md` exercising the slug
  form; a note with every known key including `reply_to`, `root`, `quote`, `edited_at`; a note with
  unknown keys interleaved among known ones, including a nested mapping and a list; a note whose
  known keys are in a non-canonical order (this is the fixture that exposes §U1); a note whose
  filename UUID disagrees with its frontmatter `id`; a note with an empty body; a note whose body
  contains a `---` line at column zero; and under `invalid/`, one file each for no fence,
  unterminated fence, malformed YAML, and missing `id`.

## Serialization constraints

The negative list. This is the part worth more than the task list.

- **T1.1 is alone in wave 1.** It owns every dependency manifest and creates the crate that every
  other task's first `use` line refers to. Nothing — not even a test — compiles before it lands.
- **T2.1 cannot share a wave with T3.1 / T3.2 / T3.3.** It owns `Cargo.toml`, `Cargo.lock`, and
  `error.rs`. All three wave-3 tasks read all three files, and two of them (`Cargo.lock`, `error.rs`)
  would be rewritten under a running agent. A dependency added mid-wave also invalidates every other
  worktree's build.
- **T2.1 cannot share a wave with T1.1** either, in the other direction: both own `Cargo.toml`, and
  T2.1's YAML spike needs the fixture corpus that T1.1 produces in order to mean anything.
- **T2.2 (verifier phase A) cannot precede T1.1**, contrary to the sketch in `orchestration.md`.
  There is no workspace, no crate named `jot_core`, and no agreed module path to write a `use`
  statement against. Phase A written against a guessed API is a Phase A that gets rewritten. See
  Deviations.
- **T2.2 can run concurrently with T2.1** only because their ownership sets are genuinely disjoint
  and Phase A tests are expected not to compile regardless of what T2.1 lands. If T2.2 ever needs to
  observe T2.1's error variants, that is a wave boundary, not a message.
- **`crates/jot-acceptance/` appears in exactly one ownership set in this entire document**, T2.2's.
  No implementer task may name it, create it, or add it to `default-members`. The root manifest
  reaches it through `members = ["crates/*"]` and excludes it via `default-members =
  ["crates/jot-core"]`, so no implementer ever needs to edit a manifest to accommodate it.
- **T3.1 and T3.2 cannot be split further.** Splitting "domain types" from "frontmatter parse" puts
  two agents in `note.rs`; splitting "atomic write" from "enumeration" puts two agents in `fs.rs`.
  Both are one-file tasks by construction.
- **Only T3.1 may write to `tests/fixtures/` in wave 3.** T3.2 and T3.3 use `tempfile` temp
  directories. This is the only cross-cutting directory in wave 3 and the only place where three
  otherwise-disjoint tasks could collide.
- **T3.3's tests may not touch the real OS config directory.** Three concurrent worktrees plus the
  developer's own machine all share one path under `%APPDATA%` / `$XDG_CONFIG_HOME`; a test that
  writes there is a race with the other two agents and a pollution of the user's machine. The
  registry path is injected in every test.
- **T4.1 cannot join wave 3.** It calls `fs::atomic_write` (T3.2) to write `workspace.toml` and, if
  §U7 resolves toward automatic registration, `registry` (T3.3). It does not depend on T3.1, which is
  why the wave-3/wave-4 boundary is drawn where it is rather than around the whole of "Workspace".
- **All three wave-3 tasks get `isolation: "worktree"`.** Not for file conflicts — those are already
  disjoint — but for the `target/` build lock: three concurrent `cargo test` runs in one checkout
  serialize, and each agent's feedback loop stretches to the sum of the other two. T2.1 also gets a
  worktree because its YAML spike is exploratory and gets discarded; T2.2 because it runs suites that
  are meant to be red. T1.1 and T4.1 run in place — each is alone in its wave and merge work is pure
  overhead.
- **`docs/plans/stages/stage1.md` is owned by T2.1 for the duration of stage 1.** The scribe must not touch
  it during the waves. The stage doc asks for the YAML choice to be recorded in itself, which puts an
  implementer in a plan doc — an intentional exception to "the scribe applies plan-doc edits", worth
  Fable's explicit approval of the choice before the line lands.

## Deviations from stage1.md

- **Wave order versus `orchestration.md`'s worked example.** The sketch puts the verifier at wave 0,
  before the cargo workspace exists. That cannot work: `crates/jot-acceptance` is a crate in a
  workspace that has not been created, and its tests must `use` module paths that nobody has fixed
  yet. I move Phase A to wave 2, concurrent with T2.1. The rule Phase A protects — no implementer of
  *behavior under test* runs before the tests exist — still holds; T1.1 and T2.1 implement no stage-1
  behavior, they land scaffolding, manifests, and an error taxonomy. The alternative (verifier guesses
  the API surface, implementers are then bound to the guess) is worse in both directions.
- **A `registry` module the stage doc's skeleton does not list.** `stage1.md` names the skeleton as
  `workspace`, `note`, `frontmatter`, `fs`, `error`, and files the registry under Workspace work. The
  registry shares no state with `init`/`open`/`discover`, lives in the OS config directory rather
  than the vault, and is the one piece of stage 1 that depends on nothing else in stage 1. Giving it
  `crates/jot-core/src/registry.rs` fills wave 3 to the three-implementer ceiling and keeps `open()`
  from being blocked behind `directories`. If Fable prefers doc fidelity, fold T3.3 into T4.1 and
  wave 3 drops to two agents; the plan is not otherwise affected.
- **`overview.md`'s Windows rename risk is likely stale.** It records that `std::fs::rename` fails on
  Windows when the target exists. Modern Rust `std` implements it via `MoveFileExW` with
  `MOVEFILE_REPLACE_EXISTING`, so the failure the risk describes is probably already handled and the
  live question is only whether to take a dependency at all. This does not change the work — the
  Windows overwrite test is written and run either way — but it may change T2.1's dependency choice,
  and the finding should be written back into `overview.md` at seal.
- **Line endings are a stage-1 concern the doc never mentions.** The byte-identical round-trip gate is
  the stage's whole defense, and a Windows checkout with `core.autocrlf=true` breaks it on fixture
  files before any parser runs. `.gitattributes` is therefore in T1.1's ownership set and in its
  definition of done. Flagged here because it is a real addition to the work list, not an
  interpretation of it.
- **`Cargo.lock` is committed.** Not stated in the docs. For a workspace with binaries and a
  reproducible CI matrix it should be; T1.1 lands it and T2.1 updates it.

## Underspecified

Flagged, not resolved. Each of these would make an implementer guess, and a guess here is a decision
made by whoever happened to be dispatched.

- **U1 — Fixed key order versus byte-identical round-trip.** `stage1.md` says "Serialize: known keys
  in a fixed order, then unknown keys in their original order" and, four lines later, "parse →
  serialize of an unmodified note is byte-identical", with the acceptance criterion "a hand-written
  note file parses; re-serializing it changes nothing (`git diff` is empty)". These are not
  compatible for a hand-written note whose known keys arrive in a different order. Missing: whether
  *all* keys preserve their as-parsed order on rewrite (canonical order applying only to notes this
  version creates), or whether a note not in canonical order is normalized on first write and the
  round-trip guarantee is only claimed for already-canonical files. This is the single most important
  gap in the stage: the verifier will hit it in Phase A and both readings pass a differently-written
  test suite.
- **U2 — Timestamp representation.** No time crate is named anywhere in `overview.md` or
  `stage1.md`, and `created_at` / `edited_at` are the fields most likely to break U1's byte
  guarantee: parsing `2026-08-26T09:00:00Z` into any typed datetime and re-emitting it can yield
  `2026-08-26T09:00:00+00:00`, or an unquoted YAML timestamp scalar, or a quoted string, depending on
  crate and emitter. Missing: whether frontmatter timestamps are a validated string that preserves
  its lexical form, or a typed value with a pinned output format. Routed to T2.1 as a decision, but
  it is a decision the doc should have made.
- **U3 — `init` on an existing workspace.** "Idempotent: running it on an existing workspace is an
  error, not a silent overwrite" defines idempotent as its opposite. Missing: which of the two
  behaviors is wanted, and whether "existing workspace" means `.jot/` exists, `.jot/workspace.toml`
  parses, or the target directory is merely non-empty. Also unstated: what `init` does when the
  directory does not exist, and where `name` comes from given that the API surface is
  `init(path, kind)` with no name parameter.
- **U4 — "An interrupted write leaves the original intact."** An acceptance criterion with no stated
  mechanism. Missing: what counts as an interruption for the test — a failure injected between
  staging and rename, a process killed mid-write, a full disk — and therefore what seam the
  implementation must expose to make it testable. A test that only asserts the tmp file was cleaned
  up would satisfy the letter of this and prove nothing.
- **U5 — Registry shape.** Missing: file name, serialization format, the `directories`
  qualifier/organization/application triple, whether "the current one" is a global single value or
  per-something, what happens when a registered path no longer exists on disk, and whether entries
  are keyed by path or by the workspace's minted `id` (the latter is what would survive the move that
  `stage1.md` advertises as the point of a self-identifying directory).
- **U6 — "Two notes created in the same millisecond must still order deterministically (v7 handles
  this)."** Three words doing a lot of work. "Deterministically" could mean total-ordered, stable
  across runs, or matching creation order; only the last is useful and it is the one plain
  `Uuid::now_v7()` does not give, since the sub-millisecond bits are random unless a monotonic
  context is used. Missing: which property the test must assert.
- **U7 — Does `init` / `open` register the workspace?** The registry is specified as a thing that
  exists; nothing says who writes to it. Missing: whether opening a workspace records it and stamps
  last-opened, or whether registration is an explicit later CLI action. Affects whether T4.1 depends
  on T3.3 at all.
- **U8 — Does CI run the acceptance suite?** `crates/jot-acceptance` is excluded from
  `default-members` so a red suite never blocks implementers, and the verifier runs it explicitly.
  Missing: whether the CI workflow gets a job that runs `cargo test -p jot-acceptance --features
  stage1` and, if so, whether it is allowed to fail during the stage and made blocking at seal. T1.1
  writes the workflow in wave 1, before any acceptance test exists, so this needs an answer at
  dispatch time, not at seal.
- **U9 — Filename/frontmatter mismatch is "reported" — to whom?** Stage 1 has no scanner (stage 4)
  and no CLI (stage 3), so there is no obvious channel. Missing: the observable. Is it a field on
  `NoteMeta`, a warnings vector returned alongside the note, a non-fatal `Error` variant returned by
  a `load`-style function? The verifier cannot write this acceptance test without one.
- **U10 — Required fields versus the error list.** Required keys are `id`, `created_at`, `root`, but
  the reject-gracefully list names only "missing `id`". Missing: whether missing `created_at` or
  `root` is an equally hard failure, and what `root` is for a note that is not a reply (presumably
  its own `id`, but a top-level note written by hand without `root` is a case the parser will meet).
