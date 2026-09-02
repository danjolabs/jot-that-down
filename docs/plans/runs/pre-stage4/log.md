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
