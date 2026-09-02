# Pre-stage-4 refactor — typed frontmatter schema

**Status.** Implemented on `refactor-pre-stage4`, from `237d8fb`. The run is in
[`runs/pre-stage4/log.md`](runs/pre-stage4/log.md): what deviated from this document and why, the
acceptance-suite appeal and every edit made under it, and the two things this refactor found that the
plan did not anticipate.

**Goal.** A workspace declares *roles*, not key names. `workspace.toml` carries an ordered list of
typed frontmatter entries, and `jot-core` learns what a key means from its declared type instead of
from a hardcoded literal.

**Not a numbered stage.** It delivers no new capability. It rewrites decisions stage 1 and stage 1b
already made so that stage 4 can be built on them, and everything it touches already exists.

**Why before stage 4.** The index caches whatever the schema declares. Landing SQLite first means
doing this same work again *through* a database migration, and the derived-index invariant —
`overview.md` calls it "the project's foundation" — would be re-derived against a schema that is
still moving. It is also cheaper now than it will ever be again: the only vaults in this format are
the author's.

**Not in scope.** SQLite, and any query. This changes what a workspace declares and what a note file
holds. Stage 4 caches the result.

**Where the decisions come from.** `docs/conversation/stage2-schema.md`, held under the old stage
numbering — read every "stage 2" there as the index, i.e. stage 4. Everything below is settled in
that conversation unless it appears under "Open questions".

## The argument

This project started from the observation that a filename, a `title:` key, and an H1 heading are the
same thing stored in three places. The logical consequence of that observation is to separate the
**role** from the **location** — and today the code does the opposite:

- `frontmatter.rs:121` — `pub const TITLE: &str = "title"`. The answer to "where is the title" is a
  string literal.
- `frontmatter.rs:137` — `INTERPRETED_KEYS`, four hardcoded names.
- `frontmatter.rs:159` — `FrontmatterSchema { keys: Vec<String> }`. A permitted-key list and an
  emission order, and nothing else. It cannot say what any key *means*.

A declared type says "this key holds the title", so the key may be called `title`, `heading`, `name`,
or `제목`, and core still knows. That is the argument for this change. "More explicit" is a matter of
taste; *role separated from key name* follows directly from the project's own premise.

## The manifest

Today:

```toml
schema_version = 1

[workspace]
id = "<uuid-v4>"
kind = "jot"
name = "workspace"

[schema]
frontmatter = ["title", "relation:root", "relation:reply_to", "relation:quote"]
```

After:

```toml
schema_version = 2

[workspace]
id = "<uuid-v4>"
name = "workspace"

[[schema.frontmatter]]
key  = "title"
type = "document:title"

[[schema.frontmatter]]
type = "relation:reply_to"

[[schema.frontmatter]]
type = "relation:quote_to"
```

An array-of-tables rather than an array of strings, because order is load-bearing (it fixes emission
order, and stage 3's `$EDITOR` template depends on it) and because each entry needs room to grow
per-key fields. A vault may then declare whatever else it keeps by hand:

```toml
[[schema.frontmatter]]
key      = "summary"
type     = "text"
required = true

[[schema.frontmatter]]
key  = "source"
type = "text:url"

[[schema.frontmatter]]
key  = "tags"
type = "multitext"
```

## The type system

### `key` defaults to the type string, verbatim

`type` is the identity; `key` is the frontmatter key it is stored under. Omit `key` and it *is* the
type string. That is the whole rule — there is no per-type table of canonical default keys.

It reads naturally in both directions: `document:title` is always written with an explicit
`key = "title"`, and `relation:reply_to` omits `key` precisely because the two are the same string.

### Reserved namespaces

| Type | Meaning | Cardinality |
| --- | --- | --- |
| `document:title` | The note's display title | one |
| `relation:reply_to` | The note this one replies to. A real edge, and the sole evidence of thread shape | one |
| `relation:quote_to` | A cross-tree quote. Never a thread edge | one |
| `text` | An opaque scalar | one |
| `text:<refinement>` | A scalar with a known shape — `text:date`, `text:url` | one |
| `multitext[:<refinement>]` | A YAML sequence of the above | many |

`text`/`multitext` rather than `string`/`array` follows Obsidian, and keeps the refinement attached
to the *element* — `multitext:url` rather than the nested `array:string:url` that the other spelling
forces.

> Cardinality lives in the type name, which works only while relations are single-valued. If a note
> ever needs several parents, this is the design that has to change — a `multirelation:*` namespace,
> or cardinality promoted to its own field. Recorded so the assumption is visible when it breaks.

`relation:quote` is renamed to `relation:quote_to`, for symmetry with `relation:reply_to`.

### `required` is a render rule, not a validation rule

```toml
[[schema.frontmatter]]
key      = "title"
type     = "document:title"
required = true
```

`required = true` means **the key is always emitted, empty if the note has no value** — so a titleless
note renders `title:` rather than omitting the line. Default `false`. It is opt-in, because
defaulting to required would put `required = false` on nearly every entry.

It **never rejects a file**. Refusing to read a file a person wrote is the one thing this project
does not do.

The mechanism already exists and does not need inventing: `frontmatter.rs:571` `Absent { Skip,
Placeholder }`, today a whole-render mode used only for the `$EDITOR` buffer (`:449`). This change
moves it from a parameter of `try_render_with` (`:478`) to a per-entry property the schema sets.

### An empty value parses as absent

Required-rendering collides with a rule stage 1b wrote at `frontmatter.rs:127`: *"an empty
`relation:reply_to:` is not [a real state], and is never written."*

One rule resolves it: **an empty value is always parsed as absent.** An empty `title:` and a missing
`title:` already mean the same thing; with this rule an empty `relation:reply_to:` also means
"top-level", so `required` becomes purely cosmetic and is safe on every type including relations.

### Unknown types are preserved, never rejected

A `type` this build does not understand is a **warning**, and its key is preserved exactly like any
undeclared key. This is the forward-compat rule the project already lives by, applied to the type
system before the type system gives anyone the chance to break it: a newer jot will write types an
older binary has never heard of, and the older binary must not damage the file.

### The manifest is strict; note files are never rejected

Two different error policies, stated together so the type system's strictness cannot leak into user
data:

- **`workspace.toml` is configuration.** Validate it hard. Two entries claiming the same reserved
  role is an error at manifest-parse time, not a silent first-wins.
- **A note file is user data.** It is never rejected. Everything wrong with it is a `Problem` on the
  scan report, and the vault stays readable.

### Undeclared keys are reported

Reordering is already implemented — `try_render_with` (`frontmatter.rs:478-503`) emits schema order,
then interpreted keys the schema omits, then unknown keys verbatim at the end. Nothing to build.

What is new is the other half: a key present in a note but absent from the schema should be
**reported**, so a person can choose to declare it. A `Problem` variant, surfaced like every other
scan problem — never an error, because an undeclared key is a legitimate state.

## What leaves

### `workspace.kind`

Deleted, along with `WorkspaceKind` (`workspace.rs:118`) and the `plain` workspace type. Filenames
are always UUID-named.

This is now the *consistent* choice rather than a simplification: a workspace that declares no
`relation:*` entries **is** what `plain` meant. The distinction moves out of a field and into the
schema, which is where the type system already put it. The Rust-reserved-word problem that made
`kind` vs `type` awkward disappears with the field.

~89 references across 7 files, most of them `Workspace::init(&root, WorkspaceKind::Jot)` in tests.

### `relation:root`

Deleted from note files entirely. `RELATION_ROOT` (`frontmatter.rs:124`), its slot in
`INTERPRETED_KEYS` and `RELATION_KEYS`, `missing_relation_keys` (`frontmatter.rs:207`), and the
`Warning::SchemaMissingRelationKeys` machinery (`workspace.rs:370-395`) all go.

What this buys, beyond one less key:

- **`open_note`'s repair write disappears** (`workspace.rs:700`), and with it the last reason a read
  path writes a file. Stage 4's "`sync()` never writes a note file" stops being a rule to enforce and
  becomes a property of the design.
- **`reply_to` becomes the single authority** for thread shape. The `tree(root_id)` vs `thread()`
  divergence that stalled stage 4 cannot happen, because there is only one answer.
- **A whole class of bug goes with it** — the `runs/stage2-3/log.md` finding #2 fallback, where a
  hand-edited parent missing its own `relation:root` left `create` nothing to copy.

Existing notes are **not migrated**. A `relation:root` key in a file written before this change
simply becomes an undeclared key: preserved, reported, ignored. The forward-compat rule *is* the
migration, which is the one place that rule visibly pays for itself.

## Root becomes derived

`root` is computed, not stored — **in Rust, at scan time**, memoized over the record map.

- The scan already holds every note in memory, so walking `reply_to` upward re-reads nothing.
- The algorithm exists: `recompute_root` (`workspace.rs:736`), its `seen` vector unchanged. Only its
  input changes, from files on disk to records already loaded.
- Memoized it is O(n) overall — a walk stops as soon as it reaches an ancestor whose root is known.

Deliberately **not** a recursive CTE. Computing root in SQL was one option among several and it is
the one that makes cycles dangerous; doing it in Rust keeps the database a dumb cache and keeps
cycle detection free.

### Purge splits subtrees, and that is the intended behaviour

This reverses `stage2.md:44` (*"It is never recomputed, so purging a middle note leaves the subtree
grouped"*) and its acceptance criterion at `stage2.md:161`.

The reversal is safe because **`root_id` was never what provided the property it was defended for.**
The "there was a chain here and a post is gone" indication comes from the surviving child's dangling
`relation:reply_to`, which points at an id the vault no longer holds and resolves to `Ref::Deleted`.
That lives in the file and survives this change untouched. Sibling grouping survives too: children of
a purged parent all carry the *same* missing id, so they can still be grouped by it. What is
genuinely lost is grouping across **two** purges, and at that point the chain really has been broken
twice.

That decision was written in `bfb2352 docs: add staged implementation plan` — the first planning
commit, before stage 1 existed. It was never discovered by implementation or confirmed by use, which
is the same category as every other plan-time guess this project has since overturned.

### Cycles

A `reply_to` cycle needs a hand edit. In this project that is the premise, not the edge case: the
files are the truth and people edit them directly. It arrives via a pasted UUID pointing at the note
itself, a hand-made two-note loop, a copied file, a git or sync merge, or the `reparent` command
`stage2.md:46-48` reserves.

Policy:

- Detected during the scan — it is a by-product of the walk, since `seen` is needed to walk at all.
- Reported as `Problem::ReplyCycle { path, id }`. **A problem, not an error**: one bad file must not
  make the other nine hundred unreadable.
- The note in the cycle **becomes its own root**, so it appears in the timeline as a top-level note.
  It stays visible, because something that needs fixing has to be findable.

This closes the item left open in `runs/post-stage3/log.md`: `Error::ReplyCycle` currently fires only
in `open_note`, and the read path silently renders a truncated tree.

## Work

### Manifest and schema types

- [x] `SCHEMA_VERSION` (`workspace.rs:73`) → `2`.
- [x] `file::Schema` / `file::SchemaIn` (`workspace.rs:321`, `:348`) take an array of tables.
- [x] Read-time promotion of v1 manifests: each string key becomes an entry, with the interpreted
      names mapped to their roles (`title` → `document:title`, `relation:reply_to` →
      `relation:reply_to`, `relation:quote` → `relation:quote_to`, `relation:root` dropped) and every
      other key becoming `type = "text"`. A v1 manifest opens, and is rewritten as v2 on the next
      write.
- [x] `FrontmatterSchema` (`frontmatter.rs:159`) holds ordered typed entries, not `Vec<String>`.
      Keep `keys()` and `contains()` working — a lot depends on them.
- [x] Reserved type parsing, with unknown types kept as an opaque preserved variant.
- [x] Duplicate-role rejection at manifest parse.
- [x] `jot_default()` (`frontmatter.rs:166`) writes the three-entry schema above.

### Note rendering and parsing

- [x] `required` per entry; `Absent` (`frontmatter.rs:571`) becomes per-key rather than per-render.
- [x] An empty scalar parses as absent, everywhere.
- [x] Role lookup replaces the literals: `TITLE`, `RELATION_*`, `INTERPRETED_KEYS`, `RELATION_KEYS`
      (`frontmatter.rs:121-144`) are all deleted.
- [x] `Problem` variant for a key the schema does not declare.

### Removals

- [x] `WorkspaceKind` and `plain` (`workspace.rs:118`); `Workspace::init` (`:439`) loses its
      parameter.
- [x] `relation:root` from the schema, from `create` (`workspace.rs:886`), and from `open_note`'s
      repair path (`:700`).
- [x] `missing_relation_keys` (`frontmatter.rs:207`) and `Warning::SchemaMissingRelationKeys`
      (`workspace.rs:370`).

### Root and cycles

- [x] Root computed at scan time over records, memoized, in `snapshot.rs`.
- [x] `Problem::ReplyCycle`; a note in a cycle roots at itself.
- [x] `recompute_root` (`workspace.rs:736`) either moves to the snapshot or goes away.

### Documents and tests to correct

- [x] `stage2.md:44` and `:161` — the assigned-once rule and its acceptance criterion.
- [x] `workspace.rs:3112` `purging_removes_one_file_and_leaves_the_children_live_and_grouped` — the
      only test pinning the old behaviour. Rewrite, do not delete: the children must still be *live*,
      they simply are no longer grouped.
- [x] `stage4.md` — the schema section, per "Consequences" below.
- [x] `stage7.md` — mostly subsumed; see below.
- [x] `crates/jot-acceptance` — verifier-owned. `criteria.rs:463` embeds a v1 manifest verbatim, and
      ~89 `WorkspaceKind::Jot` call sites span all three test files. **Under `orchestration.md` rule
      2 an implementer files an appeal rather than editing these.** Given the suite is blocking in
      CI, agree the appeal before starting rather than at the end.

## Acceptance

- A workspace whose title key is named something other than `title` — `heading`, say — round-trips,
  and `jot list` shows the titles.
- Two entries declaring `document:title` is a manifest error naming both keys.
- A v1 manifest opens, is promoted, and rewrites as v2 with the same emission order.
- A note carrying `relation:root` from before the change round-trips byte-for-byte through an edit
  that does not touch it.
- An unknown `type` in the manifest warns, and every key under it survives a write untouched.
- `required = true` on a title-less note renders `title:`; reading that file back gives no title, and
  re-rendering is byte-identical (idempotence — the property `edit`'s no-op check depends on).
- Purging the middle of a chain leaves the children live, with `reply_to` resolving to `Deleted`.
- A note whose `reply_to` points at itself is reported as a problem, appears in the timeline as a
  root, and does not hang.
- A three-note `reply_to` cycle: same.
- Every remaining `jot-cli` test passes. The CLI's public behaviour does not change here.

## Risks

- **The acceptance suite is verifier-owned and blocking.** This is the largest single risk, and it is
  procedural rather than technical. `runs/post-stage3/log.md` §4 records the last time an implementer
  edited that suite, and it was a granted appeal, not a precedent.
- **Rule 2 has not held for two stages.** Stages 2 and 3 were written and judged by one author, and
  their acceptance suites were deliberately deferred (`runs/post-stage3/log.md` §8) with "revisit
  before stage 4" attached. This refactor rewrites the frontmatter layer *underneath* that
  unverified domain code. Whatever is decided, decide it deliberately.
- **`FrontmatterSchema` is 54 references across 8 files.** Changing its shape is the mechanical bulk
  of this work, and the compiler will find all of it — but it makes the diff large enough to hide a
  real change inside. Land the type change and the behaviour changes as separate commits.
- **Empty-parses-as-absent is a parsing change, not a rendering one.** It affects every key, not just
  the relations it was introduced for. Worth its own test per type.

## Consequences elsewhere

**`stage4.md`** — its schema section is superseded and must be rewritten:

- `notes` loses `root_id`, `reply_to_id` and `quoted_id`; the separate `links` table goes.
- One `relations(from_id, role, to_id)` table replaces them, `role` being a schema-declared relation
  type. Adding a relation becomes a manifest line rather than a migration.
- `root_id` stays on `notes` as a **derived column**, filled by the scan-time walk. It is not a row
  in `relations`: those rows are facts a file asserts, and a root is a transitive closure no file
  claims. Keeping them distinguishable is the same rule that motivated dropping `relation:root`.
- Wiki-link edges (`point_to`) are deferred with the wiki-link feature. When they arrive they need a
  provenance marker, because they come from the body rather than from declared frontmatter.

**`stage7.md`** — largely subsumed. Its two headline features were user-declared typed frontmatter
(this refactor) and `plain` workspaces (deleted). What is left of it is enum types with permitted
values, per-key defaults, and rename detection. It should be rewritten or folded away rather than
left describing work that has already happened.

## Open questions

1. **`document:created_at`.** Proposed in the conversation, never settled. `created_at` is decoded
   from the note's UUIDv7, so a declared key would duplicate it and could contradict it.
   *Recommendation:* do not reserve the type. A vault wanting the date visible in the file can
   declare `key = "created_at", type = "text:date"` and own it as ordinary data.
2. **Resetting `schema_version` to 1 at public release.** Raised, and argued against — it would make
   `2` mean two different formats, permanently, for any file that escapes. Left unresolved; the
   recommendation is to keep the number monotonic and let the release version carry release identity.

Two further questions are recorded in the conversation but belong to **stage 4**, not here: whether
the index's JSON column holds only declared keys, and whether a field earns a real column because
the index's own queries need it rather than because its type is `document:*`.

## Answer to Open questions

- `document:created_at` is provided as an example of `frontmatter.schema.type`
- For the public release, new schema will be the default so I don't think it would matter when I reset it later, for the future proof.

### As implemented

1. **`document:created_at` is not reserved.** Taken as the example it was offered as, not as a type
   to add. A vault that wants the date in the file declares `key = "created_at", type = "text:date"`
   and owns it as ordinary data; reserving it would duplicate what the UUIDv7 already encodes and
   create a value that can contradict the identity.
2. **`schema_version` stays monotonic**, and is `2`. Nothing about the public release resets it.
