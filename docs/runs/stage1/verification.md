# Stage 1 verification

Phase B, run against `stage/1-vault-foundations` at `53cb57a` in an isolated worktree.

Platform: **Windows 11 (10.0.26200), toolchain `1.97.1-x86_64-pc-windows-msvc`.** Every green result
below was produced on that platform and nowhere else. Nothing in this report is evidence about
Linux; see "Linux, by reading only".

Suites, as they stand after this phase:

| Binary | Tests | Result |
| --- | --- | --- |
| `jot-core` unit + doctest | 149 + 1 | all pass |
| `jot-acceptance` lib (harness self-tests) | 11 | all pass |
| `jot-acceptance` `criteria.rs` | 14 | all pass |
| `jot-acceptance` `probes.rs` | 47 | all pass |
| `jot-acceptance` `phase_b.rs` (**new, this phase**) | 24 | 23 pass, **1 fails — a real defect** |

Gates: `cargo fmt --check` clean, `cargo clippy --workspace --all-targets -- -D warnings` clean,
`cargo test` 149/149. Tracked files are byte-identical to `53cb57a` (`git diff --stat` empty); the
only change to the tree is the new, untracked `crates/jot-acceptance/tests/phase_b.rs`.

## Criteria

Verbatim output of `cargo test -p jot-acceptance --features stage1 --test criteria`:

```
running 14 tests
test a_hand_written_note_file_parses_and_re_serializing_it_changes_nothing ... ok
test a_hand_written_note_file_parses_and_re_serializing_it_changes_nothing__out_of_order_keys ... ok
test a_note_whose_filename_uuid_disagrees_with_its_frontmatter_id__the_frontmatter_wins ... ok
test a_note_whose_filename_uuid_disagrees_with_its_frontmatter_id_is_reported ... ok
test a_note_with_an_unknown_frontmatter_key_survives_a_parse_write_cycle_with_the_key_intact ... ok
test an_interrupted_write_leaves_the_original_intact ... ok
test canonical_serialization_emits_known_keys_in_the_fixed_order ... ok
test canonical_serialization_emits_timestamps_as_quoted_rfc3339_utc ... ok
test canonical_serialization_keeps_unknown_keys_after_the_known_ones_in_their_original_order ... ok
test canonical_serialization_normalizes_a_note_whose_keys_were_written_out_of_order ... ok
test canonical_serialization_preserves_the_body_verbatim_and_is_a_fixed_point ... ok
test discover_finds_the_workspace_from_three_directories_deep ... ok
test overwriting_an_existing_note_file_succeeds_on_windows ... ok
test workspace_init_on_an_empty_directory_produces_the_exact_tree ... ok

test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
```

| Criterion (from stage1.md) | Verdict | Evidence |
| --- | --- | --- |
| 1. `Workspace::init` on an empty directory produces the exact tree | **PASS** | `workspace_init_on_an_empty_directory_produces_the_exact_tree ... ok`. Asserts `relative_tree(&root)` equals exactly `[".jot", ".jot/.gitignore", ".jot/.trash", ".jot/tmp", ".jot/workspace.toml"]`, plus `schema_version`, a v7 `id`, `kind`, `name` = basename (§U3) and `[notes] filename = "uuid"`. Mutation-confirmed: M24 (delete `.jot/tmp`), M25 (create `index.db`), M34 (drop `tmp/` from `.gitignore`), M37 (`schema_version = 2`), M22 (no `.gitignore`) each turn it red. |
| 2. A hand-written note parses; re-serializing changes nothing | **PASS, but see below** | `a_hand_written_note_file_parses_and_re_serializing_it_changes_nothing ... ok` and `..._out_of_order_keys ... ok`, over all 13 corpus notes including `.jot/.trash/`. Caught by M11, M12, M13, M14, M28. **The criterion is weak evidence by construction** — §U1 made this byte retention, so it can only fail if retention is not implemented at all. The load-bearing work is on the canonical path; see "Beyond the criteria". |
| 3. A note with an unknown frontmatter key survives parse → write with the key intact | **PASS** | `a_note_with_an_unknown_frontmatter_key_survives_a_parse_write_cycle_with_the_key_intact ... ok`. Goes through a real `fs::atomic_write` on both paths and checks the nested-mapping and list *values*, not just the keys. Caught by M2 (drop unknown keys), M23 (re-sort them), M30 (silently absorb `source` into the known set). Phase B adds 24 further hostile unknown-key shapes; all survive. |
| 4. A filename UUID disagreeing with frontmatter `id` is reported, frontmatter wins | **PASS** | `a_note_whose_filename_uuid_disagrees_with_its_frontmatter_id_is_reported ... ok` (checks the `NoteIdMismatch` variant, all three payload fields, and the rendered message) and `..._the_frontmatter_wins ... ok`. Caught by M4 (skip the comparison), M4b (let the filename win), M33 (swap the two ids in the payload). |
| 5. Overwriting an existing note succeeds on Windows; an interrupted write leaves the original intact | **PASS** on Windows; **UNVERIFIED on Linux** | `overwriting_an_existing_note_file_succeeds_on_windows ... ok` and `an_interrupted_write_leaves_the_original_intact ... ok`, the latter asserting on the *target's bytes* per §U4. But the injection mechanism is `cfg`-split and the Unix arm (chmod 555 on the parent) has never executed. See "Linux, by reading only". Separately, T3.2's warning is confirmed: criterion 5 does **not** discriminate a non-atomic writer — see M5 in the mutation table. |
| 6. `discover()` finds the workspace from three directories deep | **PASS** | `discover_finds_the_workspace_from_three_directories_deep ... ok`, comparing after `canonicalize` and asserting nothing was created in the start directory. Caught by M9 (climb to the outermost), M26 (`root()` returns `.jot/`), M41 (skip the start directory), M42 (drop the `.jot/` check). |

Rulings U1–U10 all have at least one executable observable after the wave 2/3 pin; none is
UNVERIFIED. U7's negative is covered by `probe_init_and_open_do_not_touch_the_registry`, which
snapshots the real `registry::default_path()` read-only in addition to the injected path.

## Beyond the criteria

New file: `crates/jot-acceptance/tests/phase_b.rs` (24 tests). It closes the gap `dispatch.md`
flagged under "Open shape T3.1 owns" — phase A referenced `Frontmatter`/`NoteMeta` nowhere.

### The canonical writer holds up

This is where I expected to find something and did not. `to_canonical_bytes` was given 24 hostile
unknown-key shapes and 29 hostile `title` values; every one parses back, keeps its unknown map
identical, leaves the body byte-identical, and is a fixed point. Notably:

- **No line folding.** A 200-character plain scalar and a scalar full of double spaces are emitted on
  one line. `yaml_serde`'s emitter is not wrapping, so the classic "folding ate a space" corruption
  is not reachable. This was my main hypothesis and it is wrong.
- **A `---` inside a value cannot split the document.** Emitted as an indented block scalar
  (`note: |-`), never at column zero.
- **A `title` of `"---"`, `"\n---\n"`, `"%directive"`, `"&anchor"`, `"a: b"`, or 500 `x`es** all
  round-trip. Hand-printing the known-key prefix and routing only `title` through the emitter is
  sound.
- **Timestamps.** All six spellings I threw at it (`+09:00`, `-09:00`, subsecond, lowercase `z`,
  lowercase `t`) normalize to quoted `...Z` at second precision. Subsecond input is *truncated* —
  the instant moves. That is §U2's ruling, not a bug, but it is a one-way loss the first time stage 2
  edits a note some other tool wrote with milliseconds, and it is now asserted explicitly
  (`probe_b_canonical_timestamps_normalize_to_the_one_form_u2_fixes`) rather than left implicit.
- **Three rounds of canonicalization over the whole corpus** are byte-identical
  (`probe_b_canonicalizing_every_fixture_three_times_reaches_the_same_bytes`).

Two small, real, accepted losses on the canonical path:

1. **Anchors and aliases are expanded.** `a: &x 1 / b: *x` canonicalizes to `a: 1 / b: 1`. The
   preserving path keeps the original. `stage1.md`'s Risks section says outright "Do not let anchors
   … into the note format", so this is consistent with the plan; naming it because the *file* does
   change shape the first time it is edited.
2. **Standard `!!` tags are dropped.** `t: !!binary aGk=` → `t: aGk=`; `!!str 123` → `'123'`;
   `!!omap []` → `[]`. Custom tags (`!custom foo`) *are* preserved. Same reasoning as above; low
   stakes, but "unknown keys are preserved verbatim" is now known to have this asterisk.

### Findings

**F1 — DEFECT. The two note-filename parsers disagree.** `note::load` parses the filename with a
private `filename_id` in `note.rs`; `fs::parse_note_filename` is the public one. `note.rs` says the
duplication is deliberate and adds "**Keep the two in step until** [stage 4 unifies them]". They are
not in step. `defect_note_load_and_fs_parse_note_filename_accept_the_same_filenames` is red:

```
  01a03d21-7c11-7a02-b3de-9f0e21c4a771_.md  (the separator with an empty slug): fs::parse_note_filename=false, Note::load=true
  01a03d21-7c11-7a02-b3de-9f0e21c4a771.txt  (the wrong extension):             fs::parse_note_filename=false, Note::load=true
  01a03d21-7c11-7a02-b3de-9f0e21c4a771      (no extension at all):             fs::parse_note_filename=false, Note::load=true
  01a03d217c117a02b3de9f0e21c4a771.md       (the unhyphenated uuid form):      fs::parse_note_filename=false, Note::load=true
  {01a03d21-7c11-7a02-b3de-9f0e21c4a771}.md (the braced uuid form):            fs::parse_note_filename=false, Note::load=true
```

The divergence is one-directional: `note::load` is a strict superset. `fs::parse_note_filename`
requires the 36-character hyphenated form and a literal `.md` suffix and rejects an empty slug;
`filename_id` uses `Path::file_stem()` (so any extension, or none, is accepted), splits on the first
`_` (so an empty slug is accepted), and calls `Uuid::parse_str` (so the braced, URN, and unhyphenated
forms are accepted).

Why it matters now rather than in stage 4: three of the five divergent names end in `.md` and are
therefore returned by `fs::live_note_paths`. A vault containing
`01a03d217c117a02b3de9f0e21c4a771.md` is a vault where enumeration hands the scanner a path,
`parse_note_filename` calls it `InvalidNoteFilename`, and `Note::load` on the same path returns a
perfectly good note. Two components disagree about whether a file is a note. Stage 4 inherits that
as an ambiguity rather than a decision.

**Fix, one pass:** delete `note.rs`'s private `filename_id` and have `Note::load` call
`crate::fs::parse_note_filename`. That is the direction the contract already points —
`breakdown.md` forbids `fs` depending on `note`, not the reverse, and `note.rs` already imports from
`crate::error`. Both existing `note::tests::filename_parsing_*` unit tests pass unchanged against
`fs::parse_note_filename`'s behavior except `a_filename_without_a_uuid_is_rejected`, which only
gets stricter. The acceptance test above then goes green with no edit to it.

**F2 — KNOWN HAZARD, confirmed live and reachable. The `forget_verbatim` footgun.** T3.1 reported it
and shipped a doc comment; it is real and a plausible caller loses data silently:

```rust
let mut note = Note::load(&path)?;
note.meta.title = Some("Edited".to_string());   // the obvious way to edit a note
fs::atomic_write(&path, &tmp, &note.to_bytes())?;  // the obvious way to write one
// the file on disk still says `title: Original`. No error, no warning.
```

Verified end to end (`probe_b_known_hazard_mutating_a_public_field_then_to_bytes_writes_the_pre_edit_bytes`):
after that sequence, re-parsing the written bytes yields `title: Some("Original")`. The same shape is
reachable through `unknown_mut()`, whose own doc comment says mutation does *not* invalidate the
block.

**Is the mitigation adequate? No.** `to_bytes` is the name a caller reaches for; `to_canonical_bytes`
is the one that is correct after an edit; and the doc comment that says so is on `Frontmatter`, not
on `Note::to_bytes` where the caller is standing. A doc comment is not a mitigation for a silent
data-loss path — it is a note about one. Stage 1 cannot trigger it, so this is **not a reason to
refuse the seal**, but it must not survive into stage 2 unaddressed. Options, in my order of
preference:

1. Make the known fields private behind setters that call `forget_verbatim()`. Removes the hazard
   entirely; costs a mechanical rewrite of ~15 call sites in `jot-core`'s own tests.
2. Have `to_preserved_string` compare the typed fields against a lazily-reparsed view of the retained
   block and fall back to canonical on any disagreement. Removes the hazard without an API change;
   costs a parse per write.
3. Keep the shape, and make `Workspace::edit` in stage 2 the only sanctioned edit path, documented as
   *required* to use `to_canonical_bytes()`. Cheapest, but leaves a loaded gun in the public API for
   the CLI and TUI to find.

I have pinned the current behavior in two characterization tests so that whichever is chosen, the
change is visible rather than silent.

**F3 — `NoteMeta` with `verbatim: None` is safe, but rebuilding one from index rows is lossy.**
Nothing panics and nothing misbehaves: `has_verbatim()` is `false`, `verbatim()` is `None`,
`to_bytes()` falls back to `to_canonical_bytes()` byte-for-byte, and the result parses back with
every field intact (`probe_b_a_note_meta_with_no_verbatim_writes_canonically_on_both_paths`,
`probe_b_a_note_meta_built_field_by_field_emits_every_field_it_was_given`). The alias is sound for
stage 4's read path.

The hazard is the *write* path: `NoteMeta::new` gives you an empty unknown map, so a note
reconstructed from SQLite and written back destroys every unknown key the file had — verified 2 → 0
in `probe_b_known_hazard_a_note_meta_rebuilt_from_fields_carries_no_unknown_keys`. This is exactly
`stage1.md`'s "expensive failure", displaced into stage 4. **Stage 4's plan must state that a write
never originates from an index row** — a write is always `load` → mutate → write, with the index used
only to find the path. Worth writing into `stage4.md` before stage 4 is decomposed.

**F4 — `Frontmatter`'s public API leaks `yaml_serde` with no re-export.** `unknown()` /
`unknown_mut()` return `&yaml_serde::Mapping`, and nothing in `jot-core` re-exports `yaml_serde`. A
surface crate that wants to read or set an unknown key must take a direct `yaml_serde` dependency,
which is precisely what `error.rs`'s design note ("keeps the frozen enum from pinning the public API
to a specific YAML crate") set out to avoid. `indexmap` was provisioned by T2.1 so that
`Frontmatter` "need not expose `yaml_serde::Mapping` in its public API" (dispatch.md); it does
anyway. Cheap fix: `pub use yaml_serde;` in `lib.rs`, or an opaque `UnknownKeys` wrapper.

**F5 — A UTF-8 BOM makes a note unparseable with a confusing message.** `\u{FEFF}---\n…` reports
``` `<path>` has no frontmatter: expected a `---` fence on the first line ```, on a file whose first
line, in every editor the user has, is `---`. Windows Notepad and several sync clients write BOMs.
Rejecting is defensible; the message is not. Either strip a leading BOM before splitting, or give it
its own error variant. `probe_b_a_utf8_bom_before_the_fence_is_rejected` pins the current behavior.

**F6 — Two notes may share one frontmatter `id` with no complaint.** Both files enumerate, both
`load` cleanly (`probe_b_two_notes_sharing_one_frontmatter_id_both_load_without_complaint`). Correct
for stage 1 — there is no cross-file pass — but it is stage 4's problem, inherited rather than
introduced, and `stage4.md` should name it.

**F7 — Self-reference and dangling links load without complaint.** A note whose `reply_to` is its own
`id`, whose `quote` is its own `id`, and whose `root` points at a note that does not exist all parse
and load. Consistent with `overview.md`'s "dangling references are a designed state"; pinned in
`probe_b_self_referential_and_dangling_links_load_without_complaint` so a later stage adding
validation does it on purpose.

**F8 — The registry silently drops unknown per-entry keys on save.** A `workspaces.toml` entry
carrying `color = "red"` loads fine and is written back without it
(`probe_b_known_hazard_registry_drops_unknown_entry_keys_on_save`). `workspace.toml` gets forward
compatibility for free because `open` never writes; the registry *does* write, so it does not. Not a
stage-1 criterion and low stakes (the registry is a cache), but it is the same class of loss the note
format goes to great lengths to avoid, and an older jot silently downgrading a newer jot's registry
is a real scenario.

**F9 — Registry edges, all benign, all now pinned.** Duplicate `[[workspace]]` ids collapse to the
last with no signal (correct under U5's id-keying; the silence is what is pinned). A non-UTC
`last_opened` loads and normalizes to `Z` on save. A *bare* TOML datetime — what a hand-editor
naturally writes — is `RegistryCorrupt` and recovers rather than propagating.

**F10 — `init` inside another vault's `.jot/` succeeds.** `Workspace::init(root/.jot/nested)`
creates `root/.jot/nested/.jot/`, and `discover` from there returns the nested vault. Nothing rules
on this and nothing forbids it; noting it because `.jot/` is otherwise treated as opaque metadata.

**F11 — Other things I probed that were fine.** An empty vault (init → enumerate → discover → write
→ enumerate). A 4 MiB atomic write. `atomic_write` onto a target whose parent does not exist, and
onto a target that is a directory — both fail cleanly, leave the directory intact, and leave no
staging debris. A note with an empty body, a body that is only a fence line, a body containing `---`
at column zero, and a CRLF note on both write paths. A v4 (non-v7) UUID as `id`. A 200-level-deep
nested frontmatter mapping (rejected as `MalformedYaml` with "recursion limit exceeded" — graceful).
Concurrent minting across four threads, 8000 ids, strictly increasing per thread and globally
unique. A closing fence with trailing whitespace (`--- `) is accepted, as intended for CRLF.

**F12 — A happy accident in the corpus, worth not losing.** The shared fixture vault contains **two**
pairs of colliding 8-character id prefixes (`01a03d51`, `01a03d52`). `NoteId::short()` is documented
as "not unique by construction", and stage 2's `resolve` therefore cannot be written as "first prefix
match wins" and pass against this corpus. `probe_b_short_is_a_real_prefix_and_the_corpus_contains_a_collision`
asserts the collision *exists*, so a future fixture cleanup cannot quietly remove it.

### Linux, by reading only

Everything above ran on Windows. CI's ubuntu leg has never run. Reading the `cfg` arms:

- **`sync_parent_dir` (`fs.rs`, `cfg(unix)`) — correct.** `File::open` on a directory succeeds on
  Linux and macOS (`O_RDONLY` on a directory is permitted), `sync_all` issues `fsync(2)` on the
  dirfd, and both failures are swallowed by `let Ok(..) else` / `let _ =`. `path.parent()` on a
  bare relative filename yields `Some("")`, `File::open("")` fails, and the function returns —
  no panic. The `cfg(not(unix))` arm is a no-op, which is right: opening a directory as a file fails
  on Windows.
- **The chmod-based injection (`BlockedReplacement` in `src/lib.rs`, and `BlockedRename` in
  `fs.rs`'s tests) — correct, with one caveat.** Dropping the target's *parent* to `0o555` blocks
  `rename(2)` because replacing a directory entry needs write on the containing directory. In every
  call site the staging directory is a sibling of the target's parent, not a child, so staging still
  succeeds and the failure lands at the rename — which is what §U4 requires. `Drop` restores `0o755`
  so `TempDir` can clean up. **Caveat: this does not block root.** GitHub's `ubuntu-latest` runner
  executes as the unprivileged `runner` user, so it will work; if the acceptance job ever moves into
  a container it will silently stop injecting. `harness_self_tests::blocked_replacement_actually_blocks_a_rename_on_this_platform`
  is the tripwire and will fail loudly rather than letting criterion 5 go vacuously green. That test
  is the right design and I would not change it.
- **A cross-platform difference I could not execute, flagged for the first CI run.**
  `workspace::absolutize` uses `std::path::absolute`, which normalizes `..` on Windows
  (`GetFullPathNameW`) but **not** on POSIX, where `..` components are documented as retained. So on
  Linux `Workspace::discover("<vault>/sub/../sub")` will match `.jot/` at the ancestor
  `<vault>/sub/..` (the `is_dir()` stat resolves it) and return a `Workspace` whose `root()` is
  `<vault>/sub/..`. Both `discover_finds_the_workspace_from_three_directories_deep` and
  `init_accepts_a_relative_path_and_reports_an_absolute_root` will still pass — the first compares
  after `canonicalize`, the second only asserts `is_absolute()` — so **this will not be caught by
  CI**. It is cosmetic in stage 1 and becomes real in stage 4, where index paths are stored relative
  to `root()`. Cheap hardening: fall back to `canonicalize()` when the path exists.
- Symlinks: `fs::is_dir` follows them, so on Linux a symlink to a `.md` file enumerates as a note and
  `atomic_write` renaming over it replaces the *link*, not its target. Out of scope for stage 1;
  worth a line in stage 4's scanner plan.
- `.gitattributes` `text eol=lf` is a no-op on a Linux checkout, so
  `the_fixture_corpus_is_checked_out_with_lf_line_endings` will pass there trivially.

## Mutation results

23 mutations applied one at a time in this worktree, each reverted with `git checkout` before the
next. Both suites were run with `--no-fail-fast` (the first pass without it produced incomplete data
— `cargo test` stops after the first failing target). The always-red `defect_*` test from F1 is
subtracted from every row.

The column that matters is **"acceptance tests that caught it"**. `jot-core`'s own unit tests are
listed only where the acceptance suite missed.

### Mutations the acceptance suite caught

| Behavior broken | Acceptance tests that caught it | Caught? |
| --- | --- | --- |
| M1 canonical key order: emit `created_at` before `title` | `canonical_serialization_emits_known_keys_in_the_fixed_order`, `..._keeps_unknown_keys_after_the_known_ones_...`, `..._normalizes_a_note_whose_keys_were_written_out_of_order`, `probe_the_trashed_fixture_parses_and_keeps_its_trashed_at`, `probe_b_canonical_writer_emits_every_known_key_...`, `probe_b_a_note_meta_built_field_by_field_...` (6) | yes |
| M2 canonical path drops unknown keys | `a_note_with_an_unknown_frontmatter_key_survives_...`, `canonical_serialization_emits_known_keys_...`, `..._keeps_unknown_keys_...`, `probe_b_canonical_writer_survives_hostile_unknown_values` (4) | yes |
| M3 canonical timestamps emitted unquoted | `canonical_serialization_emits_timestamps_as_quoted_rfc3339_utc`, `probe_b_canonical_timestamps_normalize_to_the_one_form_u2_fixes` (2) | yes |
| M3b canonical timestamps keep subsecond precision | same two (2) | yes |
| M4 `note::load` skips the id comparison | `a_note_whose_filename_uuid_disagrees_..._is_reported`, `probe_every_valid_fixture_loads_from_its_path_except_the_deliberate_mismatch` (2) | yes |
| M4b `note::load` lets the filename id win | same two (2) | yes |
| M5 `atomic_write` replaced by a naive `std::fs::write` | **`probe_atomic_write_actually_stages_in_the_tmp_dir_it_is_given` only** (1) | yes, barely — see below |
| M6 `atomic_write` ignores `tmp_dir`, stages beside the target | `probe_atomic_write_actually_stages_in_the_tmp_dir_it_is_given`, `probe_b_atomic_write_fails_cleanly_when_the_target_is_unwritable` (2) | yes |
| M7 `atomic_write` skips the rename and reports success | 23 tests including both criterion-5 tests, criterion 1, criterion 3, and every `discover`/`init` probe | yes |
| M8 `init` drops the `.jot/` existence check | `probe_init_errors_when_a_jot_directory_already_exists`, `probe_init_errors_when_jot_exists_even_if_its_manifest_is_unreadable` (2) | yes |
| M9 `discover` climbs past the nearest workspace | `probe_discover_stops_at_the_nearest_workspace_not_the_outermost` (1) | yes |
| M10 registry load propagates a corrupt file | `probe_registry_load_from_a_corrupt_file_is_total_and_never_propagates`, `probe_b_registry_with_a_native_toml_datetime_recovers_...` (2) | yes |
| M11 `to_bytes` uses the canonical path | 11 tests incl. both criterion-2 tests and criterion 3 | yes |
| M12 `parse` keeps no verbatim block | same 11 | yes |
| M13 the closing fence is the *last* `---`, not the first | `a_hand_written_note_file_parses_...`, `probe_a_body_containing_a_fence_line_at_column_zero_...`, `probe_every_valid_fixture_loads_...`, `probe_b_canonicalizing_every_fixture_three_times_...`, `probe_b_short_is_a_real_prefix_...` (5) | yes |
| M14 the body is trimmed instead of kept verbatim | 9 tests incl. both criterion-2 tests and criterion 3 | yes |
| M17 `open` accepts a `schema_version` from the future | `probe_open_refuses_a_schema_version_from_the_future` (1) | yes |
| M18 `parse_note_filename` accepts any filename | `probe_filename_parsing_rejects_names_that_are_not_notes` (1) | yes |
| M19 `NoteId::new` destroys creation ordering | `probe_ids_minted_earlier_compare_less_than_ids_minted_later`, `probe_b_concurrent_minting_produces_unique_ids` (2) | yes |
| M20 `MissingCreatedAt`/`MissingRoot` collapse into `MissingId` | `probe_a_note_missing_created_at_...`, `probe_a_note_missing_root_...`, `probe_the_three_required_field_errors_are_mutually_distinct` (3) | yes |
| M21 registry saves subsecond timestamps | `probe_b_registry_normalizes_a_non_utc_last_opened_on_save` (1) | yes |
| M22 `init` writes no `.gitignore` | `workspace_init_on_an_empty_directory_produces_the_exact_tree` (1) | yes |
| M23 unknown keys re-sorted alphabetically | `canonical_serialization_emits_known_keys_...`, `..._keeps_unknown_keys_after_the_known_ones_...` (2) | yes |
| M24 `init` deletes `.jot/tmp` after writing | `workspace_init_on_an_empty_directory_produces_the_exact_tree` (1) | yes |
| M25 `init` creates an empty `index.db` | same (1) | yes |
| M26 `Workspace::root()` returns `.jot/` | `discover_finds_the_workspace_from_three_directories_deep` + 4 probes (5) | yes |
| M27 `atomic_write` drops the last byte | `overwriting_an_existing_note_file_succeeds_on_windows`, criterion 3, 2 atomic-write probes (4) | yes |
| M28 `to_bytes` omits the body | 9 tests incl. both criterion-2 tests | yes |
| M30 `source` becomes a known key and is silently dropped | criterion 3, `canonical_serialization_keeps_unknown_keys_...`, `probe_b_known_hazard_a_note_meta_rebuilt_...` (3) | yes |
| M33 the mismatch error swaps `filename_id` and `frontmatter_id` | `a_note_whose_filename_uuid_disagrees_..._is_reported` (1) | yes |
| M34 `.gitignore` no longer covers `tmp/` | criterion 1 (1) | yes |
| M35 `trashed_note_paths` returns the live notes | `probe_enumeration_lists_trashed_notes_separately` (1) | yes |
| M36 a fence with trailing whitespace stops being a fence | `probe_b_a_crlf_note_survives_both_write_paths` (1) — **phase A missed this entirely** | yes, only after phase B |
| M37 the manifest writes `schema_version = 2` | criterion 1, `probe_open_refuses_a_schema_version_from_the_future` (2) | yes |
| M40 a missing `root` defaults to the note's own id | `probe_a_note_missing_root_...`, `probe_the_three_required_field_errors_...` (2) | yes |
| M41 `discover` does not consider the starting directory | `probe_discover_from_the_workspace_root_itself_finds_it`, `probe_b_an_empty_vault_enumerates_discovers_and_writes` (2) | yes |
| M42 `open` does not check that `.jot/` exists | `probe_open_on_a_directory_with_no_jot_is_an_error` (1) | yes |

### Mutations that produced NO acceptance failure

This is the important part of the table.

| Behavior broken | Acceptance | `jot-core` unit | Verdict |
| --- | --- | --- | --- |
| **M16 `init` does not create `.jot/tmp`** | none | none | **Equivalent mutant, not a gap.** Verified directly: `atomic_write`'s `ensure_dir(tmp_dir)` recreates `.jot/tmp` when the manifest is written, so `init` still produces the documented tree. Confirmed by instrumenting the mutant — `.jot/tmp` exists afterwards. M24 (delete it *after* writing) is the honest version and is caught. |
| **M15 enumeration stops skipping dotfiles** | none | 4 (`live_note_paths_skips_dotfiles_...` and 3 others) | **Real acceptance gap.** The fixture vault contains no dotfile `.md` in its root, so the acceptance suite cannot see this regression. `probe_enumeration_lists_live_notes_...` computes its expectation from `read_dir` with the same filter it is testing, so it is self-confirming here. Fix: add a `tests/fixtures/vault/.hidden-note.md` fixture and assert it is *not* enumerated. |
| **M29 `open` ignores the manifest `kind`, always reports `Jot`** | none | 3 | **Real acceptance gap.** Criterion 1 checks that the manifest *file* says `kind = "jot"`; nothing in the acceptance suite ever calls `Workspace::kind()`. A `plain` vault opening as `jot` would ship. Fix: `init(.., Plain)` → `open` → assert `kind() == Plain`. |
| **M31 enumeration is unsorted** | none | 2 | **Real acceptance gap.** `probe_enumeration_lists_live_notes_...` sorts both sides before comparing, so ordering is asserted nowhere in the acceptance suite. `fs.rs` documents stable ordering as the thing that lets stage 4's rebuild walk the vault identically twice. Fix: assert `live_note_paths` returns a sorted vector, not a set. |
| **M32 `init` mints a constant workspace id** | none | 2 | **Real acceptance gap, and a bad one.** Criterion 1 checks the id's shape but never that two `init`s differ. (Shape check amended post stage 3: workspace ids are v4 — `docs/runs/post-stage3/log.md`.) U5 keys the entire registry by workspace id; a constant id collapses every vault a user owns into one entry. Fix: `init` two vaults, assert the ids differ. |
| **M38 `Registry::save_to` swallows write failures** | none | none | **Real gap in both suites.** U5 says explicitly "save_to is **not** total … it propagates as an ordinary `Err`", and nothing tests it. `error.rs`'s `only_registry_reads_are_recoverable` checks the taxonomy, not the call. Fix: `save_to` into a path whose parent is a file, assert `Err`. |
| **M43 registry load conflates a missing file with a corrupt one** | none | 1 (`missing_file_is_an_empty_registry_not_an_error`) | **Real acceptance gap.** `probe_registry_load_from_a_missing_path_...` asserts only that the call succeeds and creates nothing; it never checks `recovered().is_none()`. U5 draws the distinction explicitly ("not a degraded state"). Fix: add `assert!(registry.recovered().is_none())` to that probe. |
| **M44 `init` writes the manifest with `std::fs::write` instead of `atomic_write`** | none | none | **Real gap in both suites**, low stakes. `workspace.rs`'s module doc says "Every file this module creates goes through `crate::fs::atomic_write`"; nothing enforces it. Hard to test without a seam. Acceptable to leave, but should be named rather than assumed. |
| **M39 `atomic_write` never `fsync`s the staged file** | none | none | **Untestable by construction, not a gap.** Durability against power loss is not observable from a test process. §U4 already scopes process-kill and full-disk out of stage 1; `fsync` belongs in the same sentence. Name it in the run log as untested rather than implying coverage. |

### Confirming T3.2 and T3.1 independently

**T3.2's claim: verified, exactly.** Replacing `atomic_write`'s body with a naive
`std::fs::write(target, bytes)` (M5) leaves **both** criterion-5 tests green:
`overwriting_an_existing_note_file_succeeds_on_windows` passes (the naive write does overwrite) and
`an_interrupted_write_leaves_the_original_intact` passes (the read-only injection stops the naive
writer at the `open`, so `result.is_err()` still holds and the target is still intact). I reproduced
the underlying mechanism directly, outside the mutant: under `BlockedReplacement`, a plain
`std::fs::write` on the target returns `is_err() == true` and leaves the original bytes in place.
The **only** acceptance test that caught M5 is
`probe_atomic_write_actually_stages_in_the_tmp_dir_it_is_given`, exactly as T3.2 reported. That probe
is doing more work than anything in `criteria.rs` and should not be weakened.

**T3.1's canonical-writer numbers: independently reproduced, and its four are a subset of my six.**
T3.1 reported 4/3/2/6 tests catching four canonical-writer mutations. My equivalents:

| T3.1's mutation | T3.1 reported | I measured (acceptance only) |
| --- | --- | --- |
| key order | 4 | 6 (M1) |
| drop unknown keys | 3 | 4 (M2) |
| timestamp quoting | 2 | 2 (M3) |
| preserving path replaced by canonical | 6 | 11 (M11) |

The differences are entirely accounted for by the 24 tests phase B added; the direction of every
number is the same and none of T3.1's claims is overstated. Its counts were honest.

## The two `non_snake_case` warnings

The question as posed is based on a premise that turns out to be false, so the answer is longer than
"fix them" or "allow them".

**Flipping `continue-on-error` to `false` at seal does not turn those warnings into failures.** The
`acceptance` CI job runs `cargo test -p jot-acceptance --features stage1` with no `-D warnings`, so
warnings there are cosmetic today and will remain so. And `cargo clippy --workspace --all-targets --
-D warnings` in the `test` job is currently **clean**, because `criteria.rs` and `probes.rs` are
`#![cfg(feature = "stage1")]` and compile to nothing without the feature. So today the CI never sees
them at all.

They only become failures if the seal checklist *also* adds `-D warnings` to a stage-1-featured
clippy invocation. If it does, here is the full set it would hit, measured with
`cargo clippy -p jot-acceptance --features stage1 --all-targets`:

- 2 × `non_snake_case` in `criteria.rs` (the deliberate `__` names)
- 7 × `clippy::err_expect` in `probes.rs` (`.err().expect(msg)` → `.expect_err(msg)`)

Nine, not two. **My recommendation: allow the names, fix the seven.** The `__` is load-bearing — it
separates the criterion from the sub-case, and `a_hand_written_note_file_parses_and_re_serializing_it_changes_nothing__out_of_order_keys`
is legible in a failure report in a way the snake-cased version is not. A file-level
`#![allow(non_snake_case)]` on `criteria.rs` with a one-line comment saying why costs nothing and
keeps the mapping from test name back to doc intact. The seven `err_expect` warnings are mechanical
and semantics-preserving and should just be fixed.

I have **not** made either change. Editing `criteria.rs` after the fact is the verifier moving its
own goalposts, and I would rather this be an explicit decision at seal. `phase_b.rs`, which I do own
outright and wrote this phase, is clean: zero clippy warnings under `--features stage1`.

Second, smaller CI note: `cargo clippy` is never run with `--features stage1` anywhere in
`ci.yml`, so the acceptance crate's own code is unlinted on both platforms. Worth adding to the
`acceptance` job whether or not `-D warnings` goes with it.

## Write-backs for the scribe at seal

The three already queued, confirmed:

1. **`overview.md`'s Windows-rename risk is stale.** `fs.rs`'s
   `std_rename_replaces_an_existing_file_on_this_platform` passes on Windows 11 / 1.97.1;
   `std::fs::rename` maps to `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`. No third-party rename
   crate is needed. Keep the test — it is the tripwire if this ever regresses.
2. **`orchestration.md`'s wave-0 example is wrong** about where phase A belongs. Confirmed by this
   stage: phase A written at wave 2 still had to guess five error-variant names, which the wave 2/3
   pin then reconciled. Writing it at wave 0 would have made that worse, not better.
3. **Stage 2's `Workspace::edit` must use `to_canonical_bytes()`.** Confirmed necessary by F2, and
   `stage2.md` should say it as a constraint rather than leaving it to the implementer. If option 1
   or 2 from F2 is taken instead, this constraint becomes unnecessary — record whichever.

Four more this phase produced:

4. **`stage1.md`'s "Round-trip test as the gate" is no longer the gate.** After §U1 the byte-identical
   round-trip is structural and cannot fail. The sentence "Any file in the fixture vault that fails
   this is a bug in the writer" is now false — the writer is not involved. The real gate is
   `every_fixture_canonicalizes_losslessly_and_reaches_a_fixed_point` plus the phase B hostile-input
   probes. Say so, or a later reader will trust the wrong test.
5. **`stage1.md`'s "Unknown keys are preserved verbatim" needs its two asterisks** (F-above):
   anchors are expanded and standard `!!` tags are resolved on the canonical path. Both are
   consistent with the Risks section's "do not let anchors … into the note format", but the flat
   claim as written is not quite true.
6. **§U4's out-of-scope list should name `fsync`.** It names process-kill and full-disk; M39 shows
   that the `fsync` itself is equally unobservable and equally untested. One more clause.
7. **`stage4.md` should inherit three things** before it is decomposed: a note may share its `id`
   with another file (F6), a write must never originate from an index row (F3), and the filename
   parsers must be unified (F1, whose fix is the unification stage 4 was already going to do).

## Verdict

**FAIL** — narrowly, and on one thing.

Everything the six acceptance criteria assert is true on Windows, and the mutation spot-check says
those assertions are load-bearing rather than decorative: 37 of 46 mutations were caught by the
acceptance suite, one of the nine survivors is an equivalent mutant and one is untestable by
construction. The canonical writer — the part I expected to break — held against every hostile input
I could construct. This is good work.

What must change before the seal:

1. **F1, the filename-parser divergence, must be fixed.** `note::load` accepts five filename shapes
   `fs::parse_note_filename` rejects, three of which end in `.md` and are therefore returned by
   `live_note_paths`. The implementation's own doc comment says the two must be kept in step and they
   are not. One-line-of-thinking fix: delete `note.rs`'s private `filename_id` and call
   `crate::fs::parse_note_filename`. `defect_note_load_and_fs_parse_note_filename_accept_the_same_filenames`
   in `crates/jot-acceptance/tests/phase_b.rs` goes green when it is done, with no edit to the test.

2. **The five real acceptance-suite gaps should be closed in the same pass**, because they are five
   small tests and the alternative is stage 4 building on a suite that cannot see them: enumeration
   skipping dotfiles (M15), `open` reporting the manifest's `kind` (M29), enumeration being sorted
   (M31), `init` minting a *distinct* id per workspace (M32), and `Registry::save_to` propagating a
   write failure (M38). M43's fix is one added assertion in an existing probe. I own
   `crates/jot-acceptance/` and will write these on request — say the word and they land in one
   commit; I did not add them unasked because four of the five need a fixture or an implementation
   seam decision that is not mine to make.

3. **A decision on F2 must be recorded, not deferred.** Not necessarily implemented in stage 1 —
   stage 1 cannot trigger it — but the choice among the three options must be written into
   `stage2.md` before stage 4 starts, or stage 2 will inherit a silent data-loss path with a doc
   comment in front of it.

Everything else in this report is a write-back, a stage-2 inheritance, or a pinned characterization.
None of it blocks.

Once F1 is fixed and the acceptance suite is green again, I would seal it.

---

# Fixer round — suite gaps closed, lint decision applied

Appended after the coordinator ratified the FAIL verdict and asked for the six gaps to be closed.
Same worktree, still at `53cb57a`; F1 itself is being fixed concurrently by an implementer working
in place on the main checkout, so `crates/jot-core/` here is untouched and
`defect_note_load_and_fs_parse_note_filename_accept_the_same_filenames` is still red **in this
worktree**. That is expected and is not a new finding.

**These gaps were closed in a fixer round. They were not found green.** Every test in section 7 of
`phase_b.rs` was written *after* the mutation that survived was known, which means none of them was
ever evidence that stage 1 was sound — they are the suite catching up to what the mutants proved it
could not see. Anyone reading the green suite later should know that six of its assertions are
retrofitted.

## What changed, and in whose files

Everything below is inside `crates/jot-acceptance/`, which I own for this round. No production code
was modified: `git diff --stat crates/jot-core` is empty. `tests/fixtures/` was **not** touched
either — see "Why no new fixtures".

| File | Change |
| --- | --- |
| `tests/phase_b.rs` | +5 tests (new section 7). 24 → 29 tests. |
| `tests/probes.rs` | 3 assertions added to `probe_registry_load_from_a_missing_path_...` (M43); 7 × `.err().expect(..)` → `.expect_err(..)`. |
| `tests/criteria.rs` | 2 × `#[allow(non_snake_case)]` with the rationale in a doc comment. No test logic touched. |

## The six gaps, closed

| Mutation | Test that now kills it | Where |
| --- | --- | --- |
| **M15** enumeration stops skipping dotfiles | `probe_b_enumeration_skips_dotfiles_the_jot_directory_and_subdirectories` | new |
| **M29** `open` ignores the manifest `kind` | `probe_b_open_reports_the_kind_the_manifest_records` | new |
| **M31** enumeration returns unsorted results | `probe_b_enumeration_is_sorted_and_therefore_deterministic` | new |
| **M32** `init` mints a constant workspace id | `probe_b_each_init_mints_a_distinct_workspace_id_that_survives_reopening` | new |
| **M38** `Registry::save_to` swallows write failures | `probe_b_registry_save_to_propagates_a_write_failure` | new |
| **M43** missing registry file conflated with a corrupt one | `probe_registry_load_from_a_missing_path_is_an_empty_registry_not_an_error` | strengthened |

Notes on three of them, because the assertion is doing something less obvious than its name says:

- **M31.** The test compares the returned vector against the expected list **without sorting the
  actual side**. That is the whole point — the pre-existing probe sorted both sides and so asserted
  set equality. It also asserts the shared corpus enumerates in ascending order via `windows(2)`,
  stated as a property rather than a literal list so a future fixture cannot turn it red for the
  wrong reason.
- **M32.** Beyond "three ids differ", it asserts each id survives a reopen (`init` returns what
  `open` reads back — the id is minted once and immutable) and that the three sort in creation
  order, which is the property that makes them v7 ids rather than merely unique ones.
  **Superseded post stage 3**: workspace ids are UUIDv4 and no longer sort by creation time. The
  ordering was unused — nothing asks "which vault did I make most recently" — and it was the reason
  short workspace ids were long. Note ids are unaffected and remain v7. See
  `docs/runs/post-stage3/log.md`.
- **M38.** The failure has to land on the *rename*, not earlier, or the test would pass against the
  mutant. Pointing `save_to` at a path whose parent is a file fails in `create_dir_all` and would
  prove nothing; pointing it at an existing **directory** lets `create_dir_all` and staging both
  succeed and fails at the rename, which is exactly the step M38 stops checking. Portable: renaming
  a file onto a directory fails on Windows (access denied, measured) and on Unix (`EISDIR`). The
  test also asserts a successful save first, so a `save_to` that returned `Err` unconditionally
  could not pass it either.

## Mutation results, re-measured

Each of the six re-applied to `jot-core` in this worktree, one at a time, reverted with
`git checkout` between runs; `cargo test -p jot-acceptance --features stage1 --no-fail-fast`; the
always-red `defect_*` row subtracted. None of these six touches `note.rs`, which the F1 fixer owns
concurrently.

| Behavior broken | Before (phase B) | After (this round) | Flipped? |
| --- | --- | --- | --- |
| M15 enumeration stops skipping dotfiles | **survivor** | `probe_b_enumeration_skips_dotfiles_the_jot_directory_and_subdirectories` | yes |
| M29 `open` ignores the manifest `kind` | **survivor** | `probe_b_open_reports_the_kind_the_manifest_records` | yes |
| M31 enumeration is unsorted | **survivor** | `probe_b_enumeration_is_sorted_and_therefore_deterministic`, `probe_b_enumeration_skips_dotfiles_...` | yes |
| M32 `init` mints a constant workspace id | **survivor** | `probe_b_each_init_mints_a_distinct_workspace_id_that_survives_reopening` | yes |
| M38 `Registry::save_to` swallows write failures | **survivor (both suites)** | `probe_b_registry_save_to_propagates_a_write_failure` | yes |
| M43 missing file conflated with corrupt | **survivor** | `probe_registry_load_from_a_missing_path_is_an_empty_registry_not_an_error` | yes |

Six of six. M31 is caught twice because the dotfile test also asserts an exact *ordered* vector;
that is redundancy, not coupling — either test alone kills it.

Running totals for the acceptance suite: **46 mutations applied across phase B and this round, 43
caught, 3 survivors** — M16 (equivalent mutant, `atomic_write`'s `ensure_dir` recreates `.jot/tmp`),
M39 (`fsync` removed; unobservable from a test process by construction), M44 (`init` bypassing
`atomic_write` for the manifest; low stakes, needs a seam that does not exist). All three are
documented above as accepted rather than outstanding.

## Why no new fixtures

`tests/fixtures/` is unchanged, and deliberately. Taking the coordinator's caution seriously, I
checked what a dotfile specimen would do to the shared corpus before reaching for one:

- `jot_acceptance::vault_note_paths` and `jot-core`'s `note::corpus::collect_md` both select on
  extension, and `Path::new(".hidden.md").extension()` is `Some("md")`. A `.hidden.md` fixture would
  therefore be pulled into the byte-identical round-trip gate, the canonicalization walk, and
  `probe_every_valid_fixture_loads_from_its_path_except_the_deliberate_mismatch` — which would then
  fail, because `.hidden.md` is not a note filename.
- `probe_enumeration_lists_trashed_notes_separately` asserts an exact set, as does
  `harness_self_tests::frontmatter_block_and_key_extraction_agree_with_the_fixtures`.

So the specimen that would have made M15 visible is precisely the specimen the corpus cannot hold.
All five new tests build their vault in a tempdir instead. `note::corpus`'s `>= 13` tripwire, the
`>= 9` and `>= 8` corpus assertions, and both enumeration probes are unaffected — verified by the
full suite still passing.

## Lint decision, applied as ratified

- **Allowed**, not renamed: the two `__` names in `criteria.rs` now carry
  `#[allow(non_snake_case)]` above a five-line doc comment explaining that everything left of the
  `__` is the criterion's name as `stage1.md` writes it and everything right of it is the sub-case,
  so a CI failure line maps back to a doc line without a lookup. No test logic changed.
- **Fixed**: all 7 `clippy::err_expect` in `probes.rs`, `.err().expect(msg)` → `.expect_err(msg)`.
  Mechanically identical. The one remaining `.err()` in that file is
  `.err().unwrap_or_else(|| panic!(..))`, which clippy does not flag and which needs the formatted
  message; left alone.

Measured after the change:

```
$ cargo clippy -p jot-acceptance --features stage1 --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.44s
```

Zero warnings. **The seal-checklist item that depends on this is unblocked**: the acceptance CI job
the fixer is adding can be flipped from bare `cargo clippy --features stage1` to `-D warnings`
whenever you want it. `cargo clippy --workspace --all-targets -- -D warnings` and
`cargo fmt --check` remain clean.

## On F1 going green with no edit to the test

I was asked to confirm the fixer's target and could not measure it: simulating the fix in this
worktree means editing `note.rs`, which I was told not to touch and which my tooling declined. I did
not work around it. The argument is structural rather than empirical, and is stronger for it:

`defect_note_load_and_fs_parse_note_filename_accept_the_same_filenames` compares, per filename,
`fs::parse_note_filename(name).is_ok()` against whether `Note::load(path)` returns anything other
than `Error::InvalidNoteFilename`. If `Note::load` delegates its filename parse to
`fs::parse_note_filename`, the two sides become the same function applied to the same input, and the
`InvalidNoteFilename` variant is exactly what that function returns on rejection. Agreement is then
guaranteed by construction for every row, present and future — which is the property the test was
written to assert, and why it needs no edit.

Two things for the fixer to watch, neither of which is my file to change:

1. `note.rs`'s `filename_id` has two unit tests of its own (`filename_parsing_accepts_both_forms_and_ignores_the_slug`,
   `a_filename_without_a_uuid_is_rejected`). Every case in them is either accepted or rejected
   identically by `fs::parse_note_filename`, so they can be deleted with the function or repointed
   at `fs::parse_note_filename` without changing a single expected outcome. Nothing in them depends
   on the looser behavior.
2. `Note::load` reads and parses the file *before* checking the filename, and must keep doing so —
   `load_reports_a_parse_failure_before_an_id_mismatch` pins that a malformed file says what is
   wrong with it rather than reporting a mismatch it could not evaluate. Delegating the parse must
   not move the check earlier.

## Suite state at the end of this round

| Binary | Tests | Result |
| --- | --- | --- |
| `jot-core` unit + doctest | 149 + 1 | all pass |
| `jot-acceptance` lib (harness self-tests) | 11 | all pass |
| `jot-acceptance` `criteria.rs` | 14 | all pass |
| `jot-acceptance` `probes.rs` | 47 | all pass |
| `jot-acceptance` `phase_b.rs` | 29 | 28 pass, 1 red = F1, unfixed here by design |

`git diff --stat crates/jot-core` empty; nothing committed.

## Verdict, unchanged

**FAIL until F1 lands**, and then PASS. The two items I flagged as not-blocking still stand: F2's
design decision is yours and the user's to make and the two characterization tests are untouched as
instructed; the seven `stage1.md` / `overview.md` / `stage4.md` write-backs in the section above are
for the scribe. Nothing in this round changed the verdict — it changed how much the green suite is
worth once the verdict clears.
