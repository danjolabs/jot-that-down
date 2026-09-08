# Stage 1b — run log

**Branch.** `stage/1b-declared-frontmatter-schema`, cut from `prototype` at `0027676`.
**Dates.** 2026-08-31, single session.
**Platform.** Windows 11 (build 26200), `cargo 1.98.0` / `rustc 1.98.0-x86_64-pc-windows-msvc`.

## How this stage was run, and how that differs from the plan

`orchestration.md` prescribes a wave loop: `stage-planner` → verifier phase A → implementer waves →
integrator → verifier phase B. **None of that happened.** The user was asked at the start and chose
a direct implementation in one session over the orchestrated loop.

So there is no `breakdown.md` and no `dispatch.md` in this directory, and their absence is a fact
about the run rather than a gap to fill in later. What that costs is stated plainly at the top of
`verification.md`: rule 2 — "whoever implements does not judge" — did not hold, so the acceptance
suite is not independent evidence. The mutation spot-check is what partially compensates, and it is
the reason that section of `verification.md` is longer than it would otherwise be.

Recommendation for stage 4: go back to the loop, or at minimum dispatch the verifier separately.
Stage 4 builds the index over this format, and `overview.md` calls the derived-index invariant "the
project's foundation".

## Decisions ratified during the run

Three of `stage1b.md`'s open questions were settled. The first two were put to the user; the third
followed the stage doc's own instruction.

### 1. Schema validation for `jot` workspaces — **warn, do not reject**

The stage doc recommended rejecting a `jot` schema missing any relation key, and made the last
acceptance criterion contingent on it. **Ratified the other way.** `Workspace::open` and
`Workspace::init` collect a `Warning::SchemaMissingRelationKeys` and open normally.

The doc's argument for rejecting was that without relation keys "new notes stop carrying them and
the vault stops being rebuildable from markdown". That premise turns out to be false under the
rendering rule the same stage introduced: `Frontmatter::try_render` emits, after the declared keys,
any *interpreted* key the schema omits that the note actually carries. A vault with a thin schema
therefore still writes `relation:reply_to` on every reply. The omission costs diff shape, never
thread structure.

Two consequences worth naming:

- It is the one case where emitted key order is not exactly the schema's, and it cannot arise for a
  schema declaring all four interpreted keys.
- `jot-core` now has a warning channel (`Workspace::warnings()`) it did not have before. It returns
  data, not log lines — the seam holds, and surfaces decide how to show it.

### 2. The markdown-crate swap lands **inside** 1b, one diff

The doc offered landing it first as a behaviour-preserving refactor gated by the existing
`split_fences` tests. The user chose one diff. Recorded because it is the reason a regression in
either half is harder to attribute, and because the mitigation was to verify the crate empirically
*before* writing any implementation code — see `markdown-crate.md`, which is the artifact of that
pass.

### 3. `title` stays optional

`stage1b.md`: "Left as written until ratified." Taken at its word. A note with no `title` is
untitled, an empty block is an untitled top-level note, and neither is an error. This is the one
place stage 1b genuinely *reverses* a stage-1 behaviour rather than deleting it: `---\n---\n` used
to be `FrontmatterNotAMapping`, because an empty block had no `id`. Pinned by
`probe_a_fence_only_file_is_an_untitled_top_level_note_not_an_error`.

## Deviations from the stage doc, and why

Every one of these is a place the doc said something the implementation could not do exactly.

### The markdown-rs span stops before the closing fence's terminator

The doc's partition (`doc[start..end]` is "the fenced block, both fences included", body at
`doc[end..]`) would give every note's body a leading newline it never had. `split_document` extends
the block over the terminator. Full detail and the pinning test in `markdown-crate.md`.

### The "no fence" / "unterminated fence" distinction is read from the source, not inferred from the AST

The doc proposed recovering it from the parser's output — an unterminated `---` arrives as a
`ThematicBreak` at offset 0. **That inference is unsound**: an indented `  ---`, which is not a
fence at all, arrives the same way, and stage 1 correctly called it "no fence".
`classify_missing_block` reads the first line instead. This is a *simplification* of what the doc
asked for, not a shortcut around it — the doc wanted a test rather than a comment, and the test
(`an_indented_fence_and_an_unterminated_fence_look_identical_to_the_parser`) asserts both the
identical AST shape and the two different errors.

### The line terminator is carried on `Frontmatter`

Not in the doc at all, and it is retained state, so it needs a justification. Unknown keys are
emitted as the bytes they were read as. A CRLF note rendered with LF-terminated known keys comes
back with mixed terminators inside one block — a file no editor would have produced, and a git diff
nobody asked for. `Frontmatter::newline` is the lexical minimum needed to stop the verbatim
guarantee from producing that. It is not content: the typed fields remain the only source of what
the block says.

### `agree_or_refuse` — a guard the doc did not ask for

The doc names what the slicer does not handle (explicit `?` keys, anchors and aliases, a flow
collection continuing at column zero) and moves on. Stage 1 could afford that: byte-replay preserved
such a block whether or not it understood it. Rendering cannot, so the parse path **checks** — it
compares the keys the line slicer found against the keys `yaml_serde` found and raises
`Error::UnpreservableFrontmatter` when they disagree.

Anchors and aliases turned out *not* to need it: an alias resolves fine as long as its anchor is
emitted first, and unknown keys keep their relative order, so `unpreservable.md` had to be rewritten
around a non-string key instead. What the guard actually catches is a non-string key, a key the
slicer missed, and a key it invented. Duplicated keys land in `MalformedYaml` first, which is a
different variant and an equally loud one.

The triggers are exotic by construction. That is the point rather than a weakness: the guard exists
so the slicer's limits are loud instead of silent.

### A file ending at the closing fence gains a terminator

`---\ntitle: x\n---` (no trailing newline, no body) writes back as
`---\ntitle: x\n---\n`. Rendering always terminates the closing fence so the body begins at a line
start; the alternative is retaining one more piece of lexical state to reproduce a file with no
content after its frontmatter. Not a byte-preservation failure — there is no body to preserve — and
render → parse → render is still a fixed point. Pinned by
`a_file_ending_at_the_closing_fence_gains_a_terminator`.

### `init` writes the schema array multi-line

`stage1b.md` illustrates `frontmatter = [ "title", … ]` on one line; `toml::to_string_pretty`
expands arrays. Both parse, and the fixture vault's hand-written manifest keeps the compact form on
purpose — it exercises the parser on the shape a human writes. The pinned literal in
`the_emitted_manifest_looks_exactly_like_the_documented_shape` was updated to the emitter's form
rather than fighting it.

### Load order reversed

`Note::load` parses the **filename first**, then the bytes. Stage 1 did the opposite, so a malformed
file reported what was wrong with it rather than an id mismatch it could not have evaluated. Under
filename identity there is no note for a parse error to be *about* until the name yields an id, so
the ordering flips. Pinned by `load_reports_a_bad_filename_before_a_parse_failure`.

### A surviving `relation:root` is kept when `relation:reply_to` was deleted

The repair table says a deleted `relation:reply_to` means "the note becomes top-level". Read
literally that could mean recomputing the root to the note's own id. It does not: root is recomputed
only when it is **absent**. The file wins, and the file still states what the root is; inventing a
different one from the absence of a sibling key would be the index correcting the file, which the
source-of-truth decision forbids. The acceptance fixture deletes both keys, so "becomes top-level"
is exercised in the unambiguous case.

## What was built

| Area | Change |
| --- | --- |
| `frontmatter.rs` | Rewritten. `FrontmatterSchema`, `UnknownKey`, `Newline`, markdown-rs splitting, `top_level_key_spans`, `agree_or_refuse`, one `render`. `KNOWN_KEYS`, `verbatim`, `forget_verbatim`, `to_preserved_string`, `to_canonical_string` all gone. |
| `note.rs` | `NoteId::created_at()` decodes the v7 timestamp. `NoteMeta` promoted from an alias to a struct. `Note { id, frontmatter, body }`. `Note::parse` takes an id; `to_bytes` takes a schema. |
| `fs.rs` | `FilenameSlug`, `note_filename`, `slugify`. |
| `workspace.rs` | `[schema] frontmatter` replaces `[notes] filename`; `FilenameStyle` deleted. `Warning`, `Workspace::warnings`, `schema()`, `note_path()`, `open_note()`, `OpenedNote`, `recompute_root`. |
| `error.rs` | 32 variants → 29. |
| fixtures | Whole corpus rewritten. Two fixtures deleted with the rules they tested; five added. |
| `jot-acceptance` | Feature `stage1` → `stage1b`. `criteria.rs` rewritten against stage 1b's list; `probes.rs` and `phase_b.rs` rewritten where the format moved. |
| CI | Feature name updated. |

### Error taxonomy

Removed, each with the key or rule it was about: `NoteIdMismatch`, `MissingId`, `MissingCreatedAt`,
`MissingRoot`, `InvalidTimestamp`. Added: `UnpreservableFrontmatter`, `ReplyCycle`.
`SerializeFrontmatter` now carries a `key` rather than a note id, because rendering runs on a
`Frontmatter` that knows neither an id nor a path.

`ReplyCycle` is new scope the doc did not mention and could not avoid: recomputing a root walks
`relation:reply_to` upward, and a hand-written cycle would hang. Dangling references are a designed
state; a cycle is corruption, and it is reported naming both the file and the note it returns to.
`stage2.md` already expects this ("A hand-written cycle in `reply_to` produces an error naming both
notes, and no hang"), so the variant arrives one stage early rather than out of nowhere.

### Fixture corpus

Deleted: `01a03d50-bac0-…` (the filename/frontmatter mismatch) and `invalid/missing_id.md`, both
testing rules stage 1b removes. Added: `01a03d59-…` (`summary` as a block scalar), `01a03d5a-…`
(`summary` as a nested mapping), `01a03d5b-…` (a body of markdown a renderer would normalize),
`invalid/not_a_mapping.md`, `invalid/unpreservable.md`.

Two of the new fixtures carry **significant trailing whitespace**, which is the point of them. If
an editor or a formatter ever strips it, criteria 3 and 11 go quietly weaker rather than red.

## Things a later stage inherits

- **Two files claiming one identity.** `<uuid>.md` beside `<uuid>_slug.md` is two files claiming to
  be one note. Nothing detects it; `note_path` returns the first match. Pinned as a characterization
  by `probe_b_two_files_claiming_one_identity_both_enumerate_without_complaint`, and written into
  `stage4.md`'s problems list.
- **`note_path` is a linear scan.** Every `open_note` walks the vault. Correct and slow; stage 4's
  index is the fix.
- **Concurrent external edit.** `open_note` reads, renders, and writes without re-stat'ing. Promoted
  from `stage1b.md`'s open questions to `overview.md`'s, because it is no longer specific to one
  stage.
- **The rebuild-invariant exemption for `edited_at`** was already written into `overview.md` before
  this stage started. It is still there, and stage 4 must honour it rather than "fixing" rebuild to
  write mtime everywhere.

## Plan-doc write-backs

Per `overview.md`'s definition of done, item 4.

- **`overview.md`** — the product paragraph (identity is the filename); the Trash row (no
  frontmatter stamp); the Thread storage and Quote rows (`relation:` spellings); the **Time**
  convention, which flatly contradicted this stage ("filesystem mtime … never a fact about a note");
  the forward-compat bullet, which now has to say *how* the guarantee is carried; stage 7's
  deliverable, since part of it landed here; two open questions promoted from `stage1b.md`.
- **`stage4.md`** — `created_at` nullable and decoded, `edited_at` and `trashed_at` from mtime;
  `trashed_at` no longer read from frontmatter; `SyncReport.problems` loses id/filename
  disagreements and gains duplicate *filenames*; three constraints stage 1b imposed on the scanner.
- **`stage2.md`** — the lifecycle table (`create` writes only relations; `trash`/`restore` write
  nothing); `root` assigned once, now `relation:root`, plus the filename-slug option; `edited_at`
  churn largely settled by making it mtime.
- **`stage7.md`** — the reserved-fields list, four keys rather than eight, and the note that they
  are already declared in `[schema] frontmatter`.
- **`stage1b.md`** — open questions resolved or moved; the acceptance list annotated with what was
  ratified.

## Not done

- `sync()` / `rebuild()` — stage 4, by the stage doc's own scope.
- An independent verifier. See the top of this file and of `verification.md`.
- Linux. CI's business; every result recorded here is Windows.
