# Pre-stage-4 refactor — run log

Implemented 2026-09-02 on `refactor-pre-stage4`, from `237d8fb`. Three commits, in the order
`pre-stage4-refactor.md` asked for them to be separable.

Every item on the plan's Work checklist landed. What follows is what the plan did **not** say —
deviations, two things the implementation discovered, and the full list of edits made under the
acceptance-suite appeal.

## The appeal, and what was done under it

`orchestration.md` rule 2 makes `crates/jot-acceptance/` verifier-owned; the plan flagged this as the
largest single risk and said to agree the appeal **before** starting rather than at the end. It was
agreed before the first edit, on these terms: mechanical adaptation plus per-test rewrites to the
successor property, **no assertion weakened or deleted**, and every substantive edit listed here so a
later verifier pass can audit it rather than re-derive it from a diff.

Mechanical, in `criteria.rs`, `probes.rs`, `phase_b.rs` and `src/lib.rs`: drop the `WorkspaceKind::Jot`
argument (29 call sites), thread the schema through `Note::parse` / `parse_at` / `load`,
`relation:quote` → `relation:quote_to`, `schema_version = 1` → `2`.

Substantive, one line each:

| Was | Now | Why |
| --- | --- | --- |
| `a_deleted_relation_root_is_recomputed_on_open` | `a_thread_root_is_derived_by_walking_reply_to_to_the_top` | Same property, new mechanism. Its second half **inverts**: stage 1b demanded the repair reach the file; it now asserts deriving a root reaches no file. |
| `a_thin_schema_warns_and_opens_rather_than_being_rejected` | `an_unknown_declared_type_warns_and_opens_rather_than_being_rejected` | "Warn, never refuse" keeps its shape. Its subject changes, because a schema omitting a relation is no longer an omission. |
| `a_thin_schema_never_drops_a_relation_the_note_carries` | `a_schema_that_names_no_relation_never_drops_a_relation_the_note_carries` | The guarantee is unchanged and load-bearing. What a second emission pass used to write, preservation now carries. |
| Mutant **M29** — `open` ignores the manifest's `kind` | `probe_b_open_reports_the_role_the_manifest_declares_not_a_default_key_name` | The old mutant died with the field. Its replacement is the mutant the new design makes possible: falling back to the key name a constant used to hold. |
| `probe_b_opening_every_note_twice_writes_at_most_once` | `…_writes_nothing` | Strictly stronger, and available only once the repair is gone. |
| `""` in the hostile-title list | `probe_b_an_empty_title_round_trips_to_untitled_rather_than_to_an_empty_string` | Promoted out of a loop into its own probe rather than deleted — see finding 1. |

Two constants split in `src/lib.rs`: `SCHEMA_KEY_ORDER` is now what `jot_default` writes (three
entries), and `ALL_INTERPRETED_KEYS_NOTE_ORDER` describes the fixture that additionally carries a
legacy `relation:root`. They were one constant while the two were the same list.

**Recommendation for the next verifier pass:** the substantive rewrites above are the ones to
re-derive independently. The mechanical ones are compiler-checked and not worth re-reading.

## Findings the plan did not anticipate

### 1. Parsing had to become schema-aware, and that is the real diff

The plan says role lookup replaces the literals. What it does not say is that `Frontmatter::parse_document`
had no schema argument, and neither did `Note::parse_at`, `Note::load`, or `Snapshot::scan`. Once a
key's meaning comes from the manifest, the manifest has to reach the parser — so the schema now
threads through all four. This is the widest part of the change and the least visible in the plan.

It also **simplifies** the write path rather than complicating it. `try_render_with` had three
passes: schema order, then interpreted keys the schema omits, then unknown keys. The second pass
existed so a schema missing `relation:reply_to` stayed non-lossy. With roles declared there is no
such thing as an interpreted key the schema omits — an undeclared role is never parsed into a typed
field — so the key stays a preserved key and pass three carries it. The pass is gone, and with it the
one case where emitted order was not exactly the schema's.

### 2. Empty-parses-as-absent needed a matching rule on the *write* side

The plan calls this "a parsing change, not a rendering one" and flags that it touches every key.
Correct, and incomplete: it makes `Frontmatter::title = Some(String::new())` unreachable-by-parsing
but still constructible, and rendering it emitted `title: ''`, which parsed back as `None`. So
render → parse → render took **two** steps to settle for that one in-memory state.

Caught by the acceptance probe for hostile titles, which had `""` in its list. Fixed in
`frontmatter.rs` by filtering an empty title on render the same way `present` filters it on parse.
The fixed point is now one step for every in-memory state, not only for states reachable from a file.

Worth noting as a category: the plan's risk list said empty-parses-as-absent "affects every key, not
just the relations it was introduced for". The half it did not predict is that it affects every
*direction*, not just every key.

### 3. A cycle would have been invisible in the timeline

`Snapshot::is_root` answered "no parent, or a parent the vault does not hold". A note replying to
itself has a parent the vault *does* hold, so under that rule it filed under a thread with no head
and never appeared in a default timeline — the exact invisibility the plan's cycle policy exists to
prevent ("something that needs fixing has to be findable"). `is_root` now answers yes when the
derived root is the note itself, which covers the cycle without changing any other case.

`Row::is_root` in `query.rs` was deliberately **not** changed. It answers a different question — "is
there a parent to render a placeholder for" — and a self-replying note does have one.

## Deviations

- **`NoteMeta.root` was kept, not removed.** It is now derived: `Note::meta()` leaves it `None` and
  `Snapshot::derive_roots` fills it in. Removing it would have dropped `root` from `meta_json`, which
  `docs/cli-json.md` documents, and the plan is explicit that the CLI's public behaviour does not
  change here. It is also what stage 4's derived `root_id` column will be populated from.
- **`jot ws new --kind` was removed.** Unavoidable — it is the surface of a deleted type — and the
  one exception to "the CLI's public behaviour does not change".
- **The fixture vault's manifest moved to v2**, and its notes keep their `relation:root` keys. Those
  keys are now the corpus's coverage of preserve-and-never-migrate, which is more valuable than the
  v1-promotion coverage the old manifest gave; promotion is covered by a unit test instead.
- **`Warning::SchemaMissingRelationKeys` was replaced, not just deleted.** The warning channel would
  otherwise have had no variants. `Warning::UnknownFrontmatterTypes` takes its place and carries the
  forward-compat rule's user-visible half.

## Known gap

A workspace whose schema declares no `relation:reply_to` will accept `create(Draft::reply_to(x))`,
set the field, and then drop it at render because there is no key to write it under. Existing files
are unaffected — an undeclared role is never parsed, so the key round-trips as a preserved key — so
this is reachable only by asking a vault to make a relation it has said it does not keep. Left alone
deliberately rather than fixed by inventing a refusal the plan does not specify. If it should be an
error, `Error::UndeclaredRole` at `create` is the shape.

## Gate

Both CI jobs, on Linux (`cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`,
`cargo test`; then the same two for `-p jot-acceptance --features stage1b`). All green. 465 tests
pass across the default members and 113 in the acceptance suite. Not run on Windows.

**Not done, and deliberately out of scope:** the mutation spot-check. It is phase B, it is the
verifier's, and this run was implementation under a granted appeal — running it here would collapse
the role split the appeal was careful to preserve.

---

# Sealing pass — 2026-09-02

Three decisions taken after the first implementation landed, folded back into
`pre-stage4-refactor.md` and implemented here.

## 1. `multirelation:*` is ruled out, not deferred

The plan carried the cardinality note as a live assumption ("recorded so the assumption is visible
when it breaks"). It is now a decision: `relation:reply_to` and `relation:quote_to` are one-to-many
in the direction that matters — many notes may reply to or quote one note — and each is
unidirectional, so the note holding the key needs exactly one value. There is no `multirelation:*`
namespace and none is planned. Doc-only, in `FieldType` and the plan.

## 2. `required` decides the `$EDITOR` buffer, and `document:title` is required by default

The plan moved `Absent` from a whole-render mode to a per-entry property but left `render_template`
using `Absent::Placeholder` — so the buffer still forced a blank for **every** declared key, and
`required` decided nothing a person could see. That was the half of the decision that did not land.

- `Frontmatter::render_template` and the `Absent` enum are **gone**. `try_render` is one path, and
  `editor::seed` uses the same render a file gets.
- `jot_default` marks `document:title` `required = true`. That is what keeps `title:` in a new
  vault's buffer, where stage 3 put it. The relations are not required: `--reply` and `--quote` fill
  them in before the editor opens, so a blank would be noise.
- `promote_v1` marks the promoted title required too. v1 had no way to say it and v1's buffer
  blanked every declared key, so requiring the title *preserves* a promoted vault's behaviour for
  the one key that had it. No other entry is promoted — for the rest, v1's blanks were core's
  decision and this refactor hands that decision to the manifest.

**The consequence, stated plainly:** a hand-written file that omits `title:` **gains** the line on
its first write. Nothing is lost (an empty value parses as absent, every other byte is untouched)
but a write is no longer byte-identity for such a file — only the second write is. Three probes were
asserting the old property; see below.

## 3. `Problem::UndeclaredKey` is implemented

The plan's Work checklist ticked this and no code existed — a checkbox ticked in error, found by
renaming a vault's title key and watching `index status` report `problems 0`.

`UndeclaredKey { key, example, notes }`, **aggregated per key across the vault** rather than raised
per file. `report_problems` prints the problem list on stderr for every command, so a per-file
variant would put nine hundred lines in front of a person whose fix is one manifest line. `Record`
gains `undeclared: Vec<String>`, and the tally is rebuilt inside `derive_roots` next to the cycle
walk — both are functions of the record set, so an incremental `reindex`/`forget` corrects the count
and a key that gets declared stops being reported.

## The acceptance suite, under a second granted appeal

Same terms as the first: no assertion weakened or deleted, every substantive edit listed.

| Test | Change | Why |
| --- | --- | --- |
| `probe_b_a_non_v7_uuid_is_a_valid_note_id_with_no_creation_time` | Fixture text gains `title:` | Byte-exact round trip must be against the file jot writes |
| `probe_b_self_referential_and_dangling_links_parse_without_complaint` | Same, for the dangling-parent fixture | Same |
| `probe_b_clearing_a_field_removes_its_key_rather_than_emptying_it` → `…_unless_the_schema_requires_it` | Split into both halves | The successor property. The old assertion is kept verbatim as the not-required half, and the required half is new |
| `probe_a_note_with_an_empty_body_round_trips`, `…_whose_body_starts_on_the_next_line_keeps_its_first_character`, `…_body_containing_a_fence_line_at_column_zero_is_not_a_second_fence` | Round-trip compares under a new `schema_without_required()` | These test **body slicing** against titleless fixtures; an added `title:` masks the byte they exist to catch. Parsing still uses `schema()` |
| — | **New:** `probe_a_a_titleless_fixture_gains_the_required_key_once_and_then_settles` | The coverage the three probes above no longer carry, made explicit: first write adds the key, second write is the fixed point, and the note still means what it meant |

The fixture vault was **not** touched. Keeping titleless notes in the corpus is what makes the new
probe possible, and a corpus that only contains files jot wrote cannot test what jot does to files
it did not.

## Gate

`cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace`
(470 tests), and `-p jot-acceptance --features stage1b` with clippy (117 tests). All green on Linux.
Not run on Windows.

## Criteria written for the two new Acceptance lines

Appeal extended to cover writing them. Three tests in `criteria.rs`, under the file's existing
conventions (a `__` sub-case name for the second, as `stage1b.md`'s criteria already use):

| Test | Criterion |
| --- | --- |
| `an_undeclared_key_is_reported_once_for_the_vault_with_a_count_and_an_example` | The report's shape — one problem per key, the count, an example path that exists — plus the two things it must not do: reject the vault, or cost the note its key on the next write. Ends by stripping the key from the last notes carrying it and asserting the report retires |
| `an_undeclared_key_is_reported_once_for_the_vault__declaring_it_retires_the_report` | The other way it retires, and the reason to raise it at all |
| `a_required_key_is_rendered_blank_and_an_optional_one_is_omitted` | The `$EDITOR` buffer criterion, reduced to what this crate can reach. `jot-cli` is not a dependency, but since this refactor the buffer **is** the file's render, so the criterion is two claims about `render` plus `jot_default`'s settings — and the fixed-point check that keeps `required` cosmetic |

**Mutation-checked, both directions:**

- `report_undeclared_keys` made a no-op → the first criterion fails, the other 22 pass.
- `required(true)` dropped from `jot_default` → the third criterion fails, the other 22 pass.

Each mutant is killed by exactly the criterion written for it, which is the property that says these
are criteria rather than restatements of the implementation.
