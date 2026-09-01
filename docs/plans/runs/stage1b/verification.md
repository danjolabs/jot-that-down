# Stage 1b — verification

Per-criterion verdict against the Acceptance section of `stage1b.md`, followed by the mutation
spot-check.

**Platform.** Windows 11 (build 26200), `cargo 1.98.0` / `rustc 1.98.0-x86_64-pc-windows-msvc`,
2026-08-31. CI covers Linux. "Tests pass" without a platform is not a fact, so: these results are
Windows-only until CI reports.

**Gate.**

```
cargo fmt --all --check                                              clean
cargo clippy --workspace --all-targets -- -D warnings                clean
cargo clippy -p jot-acceptance --features stage1b --all-targets -D   clean
cargo test --workspace                                    206 passed, 0 failed
cargo test -p jot-acceptance --features stage1b            110 passed, 0 failed
```

The acceptance suite runs behind `--features stage1b`; the feature was renamed from `stage1`
because the stage-1 suite tested a format that no longer exists. `.github/workflows/ci.yml` was
updated with it.

## A caveat this document must carry

`orchestration.md`'s rule 2 — "whoever implements does not judge" — **did not hold for this stage.**
The user chose a direct implementation over the orchestrated loop, so the same agent wrote the
implementation, the unit tests, and the acceptance suite. That is a deliberate, recorded trade, not
an oversight, and it changes what this document is worth: the acceptance tests are no longer
independent evidence, because nothing stopped them from being shaped to fit what was built.

The mutation spot-check below is what partially compensates. It is the one part of the gate that
does not care who wrote the tests: it breaks the implementation and asks whether anything notices.
A suite bent to fit its implementation tends to survive mutation, which is exactly what makes the
check worth more here than it would be under the orchestrated loop.

Read the per-criterion table as "the criterion is encoded and green", and the mutation table as
"the encoding is not vacuous". Neither is a substitute for an independent verifier on stage 4.

## Per-criterion verdict

| # | Criterion (`stage1b.md`, Acceptance) | Verdict | Test |
| --- | --- | --- | --- |
| 1 | A note written by jot has its frontmatter keys in exactly `schema.frontmatter` order | **PASS** | `a_note_written_by_jot_has_its_keys_in_exactly_schema_order`, plus `__a_different_schema_reorders_it` |
| 2 | `render → parse → render` is a fixed point | **PASS** | `render_parse_render_is_a_fixed_point`, over the whole corpus |
| 3 | A note carrying `summary:` survives a title edit with its bytes unchanged, block scalar and nested mapping included | **PASS** | `a_note_carrying_summary_survives_a_title_edit_with_its_bytes_unchanged`, over two fixtures written for it |
| 4 | A deleted `relation:root` is recomputed on open; a deleted `relation:reply_to` becomes top-level and is not written back as empty | **PASS** | `a_deleted_relation_root_is_recomputed_on_open`, `a_deleted_relation_reply_to_becomes_top_level_and_is_not_written_back_as_empty` |
| 5 | `sync()` and `rebuild()` over a clean vault write nothing | **PARTIAL — deferred to stage 4** | `a_read_pass_over_a_clean_vault_writes_nothing` |
| 6 | Two notes created in the same millisecond get distinct filenames and distinct identities | **PASS** | `two_notes_created_in_the_same_millisecond_get_distinct_filenames_and_identities` |
| 7 | `created_at` recovered from a note's filename UUID equals the creation time it was minted with | **PASS** | `created_at_recovered_from_the_filename_uuid_equals_the_mint_time` |
| 8 | A workspace whose `schema.frontmatter` omits a relation key is rejected at `open` | **RATIFIED THE OTHER WAY** | `a_thin_schema_warns_and_opens_rather_than_being_rejected` |
| 9 | For every fixture note, the three slices concatenate back to the original file byte-for-byte | **PASS** | `every_fixture_note_reconstitutes_from_the_three_slices` |
| 10 | A file with no fence and a file with an unterminated fence produce two different errors, each naming the path | **PASS** | `no_fence_and_an_unterminated_fence_produce_two_different_errors_each_naming_the_path` |
| 11 | A note whose body contains list markers, emphasis, and hard line breaks survives a title edit with every body byte unchanged | **PASS** | `a_markdown_body_survives_a_title_edit_with_every_byte_unchanged` |

### Criterion 5 — what "partial" means

`sync()` and `rebuild()` do not exist. `stage1b.md`'s own "Not in this stage" section puts SQLite in
stage 4, so the criterion is forward-looking and cannot be closed here. What *is* closed is the
property those two functions will inherit and the design decision that lets them keep it:

- A read pass over the whole corpus — `Workspace::open`, `live_note_paths`, `trashed_note_paths`,
  `Note::load` on every file — leaves every byte and the whole directory tree identical.
- Repair is on `Workspace::open_note`, which is one file and one user action. It is deliberately
  **not** in any vault-wide path, which is what makes the criterion reachable at all in stage 4.
- Opening a note twice writes at most once
  (`probe_b_opening_every_note_twice_writes_at_most_once`), so repair is itself a fixed point. A
  repair that were not would rewrite the vault on every open — a diff a day forever in a
  git-tracked vault.

The criterion as written moves to stage 4's suite. Flagged rather than quietly dropped.

### Criterion 8 — ratified the other way

Marked *(Contingent — see Open questions)* in the stage doc. Put to the user on 2026-08-31 and
ratified as **warn and open**, not reject.

What makes that safe is a rendering rule, not a hope: `Frontmatter::try_render` emits an
interpreted key the schema omits anyway, after the declared ones. So a vault whose
`[schema] frontmatter` omits `relation:reply_to` still writes the key on every note that has a
parent. The omission costs diff shape and never thread structure — which is the whole of what the
stage doc's rejection argument was protecting ("the vault stops being rebuildable from markdown").

That claim is asserted, not assumed, by `a_thin_schema_never_drops_a_relation_the_note_carries`,
and mutation M2 confirms the assertion bites. Without that rendering rule the recommendation to
reject would have been the right one.

The warning is `Workspace::warnings()`, read by whichever surface is in front of the user;
`jot-core` does not log.

## Probes beyond the criteria

110 acceptance tests across three files. The criteria are a floor; the two probe files cover
`stage1.md` and `stage1b.md` obligations stated in prose rather than in the Acceptance list, plus
inputs nobody wrote a criterion for. The ones worth naming:

- **`probe_b_the_write_path_carries_hostile_unknown_values_as_bytes`** — fourteen unknown-value
  shapes (all three block-scalar chomping indicators, folded scalars, flow collections, empty
  collections, hand alignment with a trailing comment, and the YAML-1.1 ambiguities `0644`, `yes`,
  `~`). Each must round-trip *and* keep its exact source lines. Stage 1 could not have this test:
  under byte-replay a hostile value never reached an emitter. Under one rendering path every note
  the user edits goes through it, so this is where the stage's stated main risk actually lives.
- **`probe_b_the_write_path_survives_a_hostile_title`** — thirty titles, including every YAML
  ambiguity worth naming, an embedded newline, an empty string, `---`, and `...`. The title is the
  one arbitrary user text the write path *emits* rather than copies.
- **`probe_b_every_relation_value_emits_as_a_plain_scalar_that_parses_back`** — relations are
  emitted by hand on the grounds that a hyphenated UUID is a plain scalar under every YAML schema.
  That is a claim about the value domain, so it is checked, including the v4 and uppercase forms a
  hand-written vault can contain.
- **`probe_b_writing_every_fixture_three_times_reaches_the_same_bytes`** — a one-shot fixed-point
  check is satisfied by a writer that oscillates with period two.
- **`probe_b_a_crlf_note_stays_crlf_throughout`** — a stage-1b-specific hazard that did not exist
  before. Byte-replay reproduced a CRLF block whatever the writer would have chosen; under one
  rendering path an LF-rendered `title:` above a CRLF-preserved `summary:` is a file no editor
  would have produced. `.gitattributes` pins the corpus to LF, so CRLF is exercised in-memory.
- **`probe_b_every_public_field_of_frontmatter_reaches_the_bytes`** — stage 1's finding F2 as a
  regression, written against the *symptom* rather than the mechanism, so it keeps working whatever
  the mechanism becomes.
- **`probe_b_note_load_and_fs_parse_note_filename_accept_the_same_filenames`** — stage 1's finding
  F1, now load-bearing in a way it was not: the filename is the identity, so a disagreement between
  the two parsers is a disagreement about *which note* a file is, not merely whether it is one.
- **`probe_b_every_filename_creation_can_produce_is_one_enumeration_accepts`** — the other half:
  fourteen `note_filename` outputs (including titles that slugify to nothing) written into a real
  vault and found again by id. Creation and enumeration meeting in the middle is what stops a note
  jot just wrote from being invisible.

## Mutation spot-check

Eleven deliberate breakages, each applied to a clean tree, both suites run, tree restored. A test
that stays green against a broken implementation is worth less than no test, because it manufactures
confidence.

| # | Mutation | Verdict | Caught by |
| --- | --- | --- | --- |
| M1 | Unknown keys re-emitted through `yaml_serde` instead of sliced | CAUGHT | **criterion 3**, plus 4 unit tests |
| M2 | An interpreted key the schema omits is dropped | CAUGHT | `a_thin_schema_never_drops_a_relation_the_note_carries` + 2 |
| M3 | The block does not own the closing fence terminator | CAUGHT | **criterion 2**, plus 4 unit tests |
| M4 | Both fence failures collapse into one error | CAUGHT | **criterion 10**, plus 3 unit tests |
| M5 | Emitted key order sorted rather than declared | CAUGHT | **criteria 1 and 4**, plus 4 unit tests |
| M6 | A deleted `relation:root` is not recomputed on open | CAUGHT | **criterion 4** (both halves), plus 4 unit tests |
| M7 | An absent `relation:reply_to` is written back as an empty key | CAUGHT | **criterion 4**, plus 4 unit tests |
| M8 | The body is normalized instead of copied | CAUGHT | **criteria 11 and 2** |
| M9 | `created_at` invented from the clock rather than decoded | CAUGHT | **criterion 7**, plus 3 unit tests |
| M10 | The slicer/parser agreement check dropped | CAUGHT | `probe_a_block_that_cannot_be_preserved_is_refused_rather_than_mangled` + 2 |
| M11 | A thin `jot` schema produces no warning | CAUGHT | **criterion 8**, plus 1 unit test |

Eleven of eleven caught, and — the part that matters — each one by the named criterion that claims
the behavior, not merely by some incidental unit test downstream of it. M1 is the one to look at:
it is the stage's own stated main implementation risk ("Slicing and re-splicing that text is this
stage's main implementation risk"), and criterion 3 catches it directly.

M5 is worth a second look for the opposite reason. Sorting the schema keys alphabetically happens
to put `relation:quote` before `relation:reply_to` before `relation:root` before `title`, which
also breaks criterion 4's on-disk assertion — a criterion about repair failing because of an
ordering bug is a coupling worth knowing about, not a problem.

## What is not verified here

- **Linux.** CI's business. Every result above is Windows.
- **Concurrent external edit.** `stage1b.md` leaves the mechanism unspecified and `open_note` is the
  first writer with the problem: it reads, renders, and writes without re-stat'ing. An external
  editor writing between the read and the write is clobbered. Recorded in `overview.md`'s open
  questions and carried to stage 4, which is the first stage with the machinery (`files`: size,
  mtime, hash) to fix it.
- **Ordering churn against another editor.** jot writes in schema order; Obsidian writes in its own.
  Alternating edits produce diff noise in a git-tracked vault. Unmeasured, and only measurable with
  real use.
- **`plain` workspaces.** A `plain` manifest round-trips and is exempt from the relation-key
  warning. Nothing else about `plain` is implemented or tested; that is stage 7.
