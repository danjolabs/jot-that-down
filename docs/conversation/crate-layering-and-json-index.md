# Conversation

## session_01RwWVq3PXJTg9F7Vwm58qM1

**Date:** 2026-09-07
**Topic:** Crate layering documented for stage 6, and what SQLite's JSON support can and cannot do
for `notes.raw`
**Branch:** `stage/5-tui`, mid-polish

-- Claude

> **This is history, not spec.** Written as a structured record rather than a verbatim transcript:
> the value here is the findings and their citations, not the phrasing. Anything that should bind
> future work belongs in a plan doc — see "What should move into a plan doc" at the end. Several
> claims below are dated measurements or version-specific facts; re-check before relying on one.

### What prompted it

Stage 6's risk register names the failure this stage can have:

> *Logic leaking into the frontend. The most likely failure of this stage, and the one that quietly
> undoes stages 1–4. Review every new frontend function against the question "should the TUI have
> this too?" — if yes, it belongs in core.*

Answering that question needs the layering written down somewhere other than in the head of the
person who built it. `jot-core/README.md` already did it for its own modules; `crates/README.md`,
`crates/jot-cli/README.md` and `crates/jot-tui/README.md` were empty files.

---

## 1. The crate READMEs

Three READMEs written, one corrected. Committed as `18b8be5`.

### The direction was backwards in the framing

The session opened with "core → cli → tui". It is **core → tui → cli**.

- `jot-tui` is a library with no `[[bin]]` and no `fn main`; `jot-cli` declares
  `jot-tui = { path = "../jot-tui" }`.
- `jot tui` is a subcommand: `main.rs:424` calls
  `jot_tui::run(context.workspace, &compose::Editor)`.
- So the CLI is the layer *above* the TUI, and it hands an already-opened `Workspace` down.

### The graphs, as derived

Both were produced by script and both scripts are in the READMEs, so they can be re-derived after a
refactor. Only real code dependencies count — the same rule `jot-core/README.md` set, excluding
doc-comment links and `#[cfg(test)]` imports.

`jot-cli` — 5 files, 5 edges:

```
main.rs → compose, context, editor, output
compose → editor
context, editor, output → (leaves)
```

`jot-tui` — 7 files:

```
run → app, compose, key, preview, ui
ui  → app, key
app → key, preview
compose, key, preview → (leaves)
```

**`app` does not depend on `compose`.** The naive grep reports that edge; it exists only in
`#[cfg(test)]` (`app.rs:807`, `app.rs:1117`) and a doc link. That is the point of `Pending`: `App`
records `Pending::Compose` / `Pending::EditNote` and `run` gives the screen back to `$EDITOR` and
answers it. It is what makes the whole interaction model testable with no pty.

### What crosses each boundary

- **`jot-core` → surfaces.** Twelve public modules, `index` private. `jot-cli` names nine of them,
  `jot-tui` four.
- **`jot-tui` → `jot-cli`.** Six public modules, and the CLI uses exactly **two** names: `run` and
  `compose::Composer`. `app`, `key`, `preview`, `ui` are `pub` for tests and docs. A
  `jot_tui::app::` path under `crates/jot-cli/src` is a layering regression.
- **`jot-cli` → nothing.** A `[[bin]]` with no `[lib]`.

### Seam checks, verified 2026-09-07

```sh
grep -rn 'rusqlite'  crates/jot-cli crates/jot-tui --include='*.rs'   # 0 hits
grep -rn 'std::fs'   crates/jot-cli/src crates/jot-tui/src            # exactly 4
```

The four: three in `editor.rs` for the `$EDITOR` scratch file, one in `context.rs` creating the
registry's parent. All outside the vault. `jot-tui` has zero — `preview.rs` documents what that
cost: `bat` is fed markdown over **stdin** rather than being handed a path.

### A gap found in `jot-core/README.md`

`watch` was missing from both the module graph and the edge table — 326 public lines with a surface
consumer and no row. Added. Like `registry`, nothing inside core references it: `registry` is
`jot-cli`'s alone, `watch` is `jot-tui`'s.

### `fn main` vs `main.rs`

The first drafts used `` `main` `` for two different things, and the ambiguity was hit immediately.
Fixed throughout, and a one-line convention note added at `crates/README.md:120`:

- **`fn main`** — the process entry point. The whole argument for `jot-tui` having none: with no
  `fn main` there is nowhere for a `Workspace::open` call to grow, so "surfaces never touch the
  filesystem" is enforced by the compiler rather than by review.
- **`main.rs`** — `jot-cli`'s 1315-line module holding the clap tree, every command body, and
  `fn main` itself.

Also fixed a pre-existing off-by-one in the `jot-cli` box of the stack diagram.

---

## 2. TEXT vs a JSON type for `notes.raw`

**Question:** SQLite supports JSON values — why did the schema choose TEXT?

**Answer: there was no such decision, because SQLite does not offer that choice.**

- SQLite has five storage classes: `NULL`, `INTEGER`, `REAL`, `TEXT`, `BLOB`. There is no `JSON`
  type and never has been.
- "SQLite supports JSON" means the JSON1 *function set* — `json_extract`, `json_each`, `json_valid`,
  generated columns over them. They operate on `TEXT`.
- `raw TEXT NOT NULL` (`schema.sql:20`) is not TEXT *instead of* JSON. TEXT is where JSON lives.
  Writing `raw JSON` would be legal SQL and resolve to TEXT affinity anyway.

### What the docs did decide

Three real decisions, none about storage type:

- **A JSON column at all, vs. a column per declared field** — `stage2-schema.md:573-590`. The rule
  that settled it, quoted into `stage4.md:139`: *a column exists because the index's own queries
  need it, not because its type is special.* A column per `document:*` type would make adding a type
  a migration again — the thing the frontmatter type system exists to escape.
- **Declared keys only, vs. every key** — recommended declared-only in conversation, then
  **reversed** in stage 4. `raw` replaced the earlier `fields` column and holds every key, keyed as
  written (`stage4.md:113-130`). The payoff: `Problem::UndeclaredKey` is recomputable from the index
  against whatever the manifest declares *now*, so no `undeclared` column and no migration when a
  key becomes declared.
- **Indexing deferred, not foreclosed** — `stage4.md:136`.

### Why TEXT is right today

Not in the docs; it is in the code. **No SQL JSON function is ever called on `raw`.** `row.rs:93`
selects it as a plain string and `row.rs:169` parses it in Rust with `serde_json`. To SQLite it is
opaque text.

### The real version of the question: JSONB

Genuinely open and genuinely undiscussed.

- Since 3.45 SQLite has **JSONB** — a binary encoding in a `BLOB`, read by the JSON functions
  without reparsing. Available here: `libsqlite3-sys 0.38.2` bundles SQLite **3.53.2** (checked in
  the vendored `sqlite3.h`).
- It would be a **regression today**: `serde_json` cannot read JSONB, so every read would need
  `json(raw)` to convert back — strictly more work than storing text.
- JSONB pays off only when SQLite itself does the extraction. Nothing does yet. The moment that
  changes is stage 7's field filters.

### One gap, unresolved

There is no `CHECK (json_valid(raw))` on the column and no doc says why not. `undeclared_from`
currently swallows a malformed `raw` and returns no undeclared keys (`row.rs:161-169`), so a corrupt
row degrades silently. The index is disposable, so the blast radius is small — probably why it never
came up. Whether the omission is deliberate could not be determined from the docs.

---

## 3. Generated columns over `json_extract` for **undeclared** keys

**Question:** can generated columns give a virtual index over key-value pairs that are not
predefined?

**Answer: no, and the B-tree in question is the wrong one.**

### The B-tree here is Rust's

`snapshot::Snapshot` is a `BTreeMap<NoteId, Record>` (`snapshot.rs:252`), described at
`overview.md:157`. SQLite's own indexes are B-trees too, so the words collide — but the one the
design talks about is the in-memory map. That matters because of a deviation easily lost track of:

> **`stage4.md:305` — "Not implemented as SQL. Deviation, taken 2026-09-02, reversible."**

Every query — timeline, search, threads, prefix resolution, backlinks — is a `Snapshot` method over
that `BTreeMap`. **SQLite runs no filtering `SELECT`.** It is change detection and persistence only.

### The mechanical blocker

- A generated column's expression is fixed at DDL time. `json_extract(raw, '$.status')` needs the
  path as a **literal**.
- Undeclared keys are by definition unknown when the DDL is written. One column per key = one
  `ALTER TABLE` per key = the migration treadmill the type system exists to escape.
- Expression indexes carry the identical limitation.
- `json_each(raw)` does handle arbitrary keys, but it is a table-valued function and its output
  cannot be indexed — a full scan by construction.

### What would work, and why not to build it

A materialized EAV table, populated in `scan.rs` beside `relations` and `links`:

```sql
CREATE TABLE fields (
  note_id TEXT NOT NULL,
  key     TEXT NOT NULL,
  value   TEXT NOT NULL,
  PRIMARY KEY (key, value, note_id)
);
```

No migration when a key appears — a new key is a new row. But the numbers say don't
(`stage4.md:372`, 10k notes): sync is **67 ms**, of which **26 ms already parses every `raw` in the
vault**. Once hydrated, filtering on any key is a walk over parsed data in memory. An EAV table adds
write cost to accelerate a read that is not slow, for a query engine that does not run in SQL.

### The design objection, larger than the mechanics

Indexing undeclared keys removes the reason to declare anything. `stage2-schema.md:588` is explicit:
undeclared keys are preserved in the file but not queryable, so `Problem::UndeclaredKey` can say
*"declare it to search it"* — 선언에 보상이 생깁니다. Index them anyway and the schema becomes
decorative.

---

## 4. Indexing a **declared but not reserved** field (`source`)

The intent, restated: a workspace declares extra keys in `[[schema.frontmatter]]`, and one of them
— `source` — should be indexed for faster search.

### The predefined set is smaller than assumed

The session's framing was `document:{title, created_at, edited_at}` plus `relation:{reply_to,
quote_to}`. Only **three** roles exist (`frontmatter.rs:151`): `document:title`,
`relation:reply_to`, `relation:quote_to`.

- **`created_at` is not a role and cannot be declared.** Decoded from the UUIDv7 in the filename.
  `stage7.md` open question #1 was answered explicitly: not reserved — it would create a value that
  can contradict the identity.
- **`edited_at` is not a role either.** It is the file's mtime, index-only since stage 1b
  (`note.rs:27`, `query.rs:369`), and is not a `notes` column — it comes from `mtime_ns`.

So `source` is a fourth category: **declared, but not reserved.**

### Here the mechanism does apply

Unlike undeclared keys, a declared key's name is known from `workspace.toml` at DDL time:

```toml
[[schema.frontmatter]]
key  = "source"
type = "text:url"
```

```sql
-- VIRTUAL, not STORED: ALTER TABLE ADD COLUMN refuses STORED generated columns.
ALTER TABLE notes ADD COLUMN f_source TEXT
  GENERATED ALWAYS AS (json_extract(raw, '$.source')) VIRTUAL;
CREATE INDEX notes_f_source ON notes(f_source);
```

Or without touching the table at all — an expression index, freely droppable:

```sql
CREATE INDEX notes_f_source ON notes(json_extract(raw, '$.source'));
```

Both are legal because `json_extract` is deterministic. This is exactly what `stage4.md:136`
reserved.

Two caveats:

- **`multitext` cannot use this.** One generated column holds one scalar; a multi-valued field needs
  `json_each` or the EAV table.
- **The DB schema becomes a function of `workspace.toml`.** `migrate.rs` versions the schema by
  `PRAGMA user_version`. Per-field indexes mean DDL driven by the manifest.
  `index_meta.schema_fingerprint` already notices a manifest change, but "recompute rows" and
  "recompute *columns*" are different jobs and only the first is built.

### And it still would not be read

Same blocker as §3: no query runs in SQL, so the index would be maintained on every write and
consulted by nothing. Making `source` search genuinely faster means **not hydrating the whole
snapshot** and letting SQL answer — reversing a documented deviation, not bolting on an index.
`stage4.md:319` names the trigger: profiling at 10k saying hydration is the cost. At 10k it does
not.

---

## 5. `.local/example-vault` does not index at all

Raised as the worked example for `source`. It is not a jot workspace, and the gap is not only the
missing `.jot/`:

```
contents/videos/01a05785-9bc6-79c0-8138-f5c2a18c0014.md   # title + source in frontmatter
journal/2026-08-31.md                                      # empty
journal/2026-08.md                                         # links to the note above
```

- No `workspace.toml`, no `.jot/`.
- **Enumeration is non-recursive** (`fs.rs:303`; `watch.rs` states "a vault is two flat
  directories"). `contents/videos/…` would never be walked.
- **Filenames must be `<uuid>.md` or `<uuid>_<slug>.md`** (`fs.rs:190`). The `journal/` files are
  not notes by that rule.

So `source` there is not an unindexed declared field — it is a key in a file jot would not see. If
the nested `contents/` + `journal/` shape is what the vault should actually hold, that collides with
a locked decision (flat vault, filename-as-identity) and is a much larger question than indexing.
**Left open. Worth raising deliberately rather than discovering at stage 6.**

---

## What should move into a plan doc

Nothing here binds anything yet. Candidates, in the order they were judged useful:

1. **The vault-layout question (§5).** The most consequential item in the session and the only one
   touching a locked decision. Belongs in an explicit ask, not in a conversation file.
2. **A "Why not generated columns over `raw`" note in `stage4.md`,** next to line 136: the
   `ALTER TABLE … VIRTUAL` restriction, the expression-index alternative, that `multitext` is
   excluded, and that undeclared keys are blocked by the DDL-literal rule. This was re-derived twice
   in one session.
3. **JSONB, recorded as belonging to stage 7,** not stage 4 — with the reason it would be a
   regression today.
4. **The `json_valid` question,** as an open item rather than an answer.

## Decisions actually taken

- Crate READMEs written and committed (`18b8be5`); `watch` added to `jot-core/README.md`.
- `fn main` / `main.rs` disambiguated across all four READMEs, with the convention stated at
  `crates/README.md:120`.

Everything in §2–§5 is a finding, not a decision. No code changed.
