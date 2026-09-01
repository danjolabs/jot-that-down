# Stage 1b — Declared frontmatter schema

> **Implemented 2026-08-31** on `stage/1b-declared-frontmatter-schema`. The run is in
> [`runs/stage1b/`](runs/stage1b/log.md): what was ratified, where the implementation deviated from
> this document and why, the per-criterion verdict, and an eleven-mutation spot-check. Two sections
> below carry inline corrections marked **Corrected at implementation**; the Open questions section
> records what was settled.

**Goal.** The note format stops carrying what the index can hold, and `workspace.toml` declares what
frontmatter looks like. One write path, ordered by the schema.

**Why between 1 and 2.** Stage 1 proved the file plumbing — atomic writes, enumeration, workspace
lifecycle — and all of that stands. What it also proved is that the *format* carries redundancy that
becomes expensive once SQLite exists. Changing it before stage 4 builds an index over it costs a
rewrite of `frontmatter.rs`; changing it after costs a migration over a year of notes.

**Not in this stage.** SQLite itself. This stage changes what is on disk and how it is written; stage
4 builds the index over the result.

## What changed, and what did not

Ratified in conversation, 2026-08-31. Two locked decisions were examined; one moved.

| Locked decision | Status |
| --- | --- |
| Source of truth — markdown files; SQLite derived and disposable | **Unchanged.** "The DB is the source of truth" was about jot-core's *read path* — queries go through the index rather than re-reading files. Ownership is unchanged: everything is rebuildable from markdown. |
| Trash — file moves to `.jot/.trash/`, location *is* the state | **Unchanged.** `deleted_at` in the index is a mirrored column, derived like everything else. |
| Frontmatter forward-compat — unknown keys preserved verbatim | **Unchanged, and load-bearing.** See "Unknown keys" below. |
| **Identity — UUIDv7 in the filename *and* the frontmatter** | **MOVED. Filename only.** |

### The identity change, stated plainly

`stage1.md` duplicated `id` into the frontmatter and made the frontmatter authoritative, on the
grounds that filenames are renamed by sync clients, conflict copies, and users. That rule is now
reversed: **the filename's UUID is the note's identity, and there is no copy in the file.**

The accepted cost: a rename that mangles the UUID produces a new note and orphans the old index row.
The note's history forks, silently. Stage 7's rename detection is the eventual mitigation; until
then this is a known, chosen hazard, not an oversight.

What this deletes from stage 1: the "frontmatter wins" rule, acceptance criterion 4, and
`Error::NoteIdMismatch` together with the fixture that exercises it.

### `workspace.toml`

```toml
schema_version = 1

[workspace]
id   = "01a03d4c-3680-7c70-aade-6c016dd177d2"
kind = "jot"
name = "Fixture Vault"

[schema]
frontmatter = [ "title", "relation:root", "relation:reply_to", "relation:quote" ]
```

`[notes] filename = "uuid" | "uuid_slug"` is **removed**. The slug was always decorative and always
ignored by the reader, so the knob controlled nothing the reader cared about. It is replaced by a
creation-time option governing whether a new note's filename gets a slug derived from its title.
Because the reader ignores everything after the UUID, re-slugging on a title change is safe — the
identity is the UUID and it does not move.

## The new note format

```markdown
---
title: Jot that down
relation:root: 01a03d20-a54c-7977-a1f4-1a88b38855dd
relation:reply_to: 01a03d20-a54c-7977-a1f4-1a88b38855dd
relation:quote: 01a03d10-3f8a-7bb1-9c22-0e1d5a6b7c88
---

The body. Plain markdown, untouched.
```

**The block is always present.** Every note carries frontmatter, at minimum a `title`. A file with no
fence is a malformed note, not an untitled one — which makes the fence a hard parse boundary rather
than an optional prelude, and is what lets the parse path below treat its absence as an error rather
than as a state to represent.

Four keys where there were seven. What left, and why it is safe:

- **`id`** — the filename carries it. See the identity change above.
- **`created_at`** — **derivable from the id, exactly.** UUIDv7 encodes a 48-bit millisecond
  timestamp; the creation time is recoverable from the identity itself with no external state. This
  is the strongest of the three removals and the reason the others became thinkable.
- **`edited_at`** — index-only, populated from filesystem mtime at scan time. **This is the one field
  a rebuild cannot reproduce faithfully**, and it is a deliberate, isolated exception to the rebuild
  invariant. See "The rebuild invariant exemption" below.

`relation:` prefixing is verified, not assumed: `relation:root` parses as a single key and round-trips byte-identically through `yaml_serde`, because a colon is only an indicator when followed by whitespace. Confirmed against the pinned crate on 2026-08-31.

## The parse path

**Fence splitting is delegated to `markdown` 1.0.0** (markdown-rs), replacing the hand-rolled
`split_fences` in `frontmatter.rs`. The rule that makes a markdown crate safe in a tool whose whole
premise is not touching the user's bytes:

> **Parse with the crate, slice with your own offsets, never call its renderer.**

`ParseOptions.constructs.frontmatter` yields a `Node::Yaml` whose `position` carries byte offsets into
the source. Those offsets partition the file, and jot does the partitioning itself:

```text
doc[..start]     the BOM, if any
doc[start..end]  the fenced block, both fences included
doc[end..]       the body, byte-for-byte
```

**Corrected at implementation.** The reported span stops at the last character of the closing fence
and **before its line terminator**, so `doc[end..]` would begin with that newline and every note's
body would gain a leading blank line — fatally so for `01a03d56…`, whose body starts on the very
next line. The block is extended over the terminator, and the adjustment is pinned by a test that
asserts the crate's raw span alongside it. See `runs/stage1b/markdown-crate.md`.

The body is a slice of the original text and never passes through a markdown emitter, so "plain
markdown, untouched" is structural rather than earned — the same shape of guarantee byte-replay used
to give, obtained from an offset instead of from a retained copy. This is why an AST crate is safe
here and a *rendering* crate would not be: comrak and friends will happily normalize list markers,
emphasis characters and hard-break spacing on the way out.

Verified against the pinned crate on 2026-08-31, on Windows 11, against every case the current
`split_fences` tests cover: LF and CRLF; a leading BOM (reported as a three-byte prefix *outside* the
span rather than silently consumed); no trailing newline; an empty block; a `---` rule in the body;
a fence with trailing whitespace; an indented `---`, correctly not frontmatter; and a block holding
both a block scalar and a nested mapping. In every case `doc[..start] + doc[start..end] + doc[end..]`
reconstitutes the file exactly.

**What it does not solve.** The crate hands back the block's outer boundary and nothing else. The
interior is still `yaml_serde`'s, so the unknown-key problem below is untouched by this change. That
section, not this one, is where the stage's risk lives.

**What it costs.** markdown-rs reports *no frontmatter node* for both "no fence" and "unterminated
fence", where §U10 wants two distinct errors. Because the block is always present, both cases are
hard errors on a malformed file; neither is a path a well-formed vault takes.

**Corrected at implementation.** The recovery proposed here — "an unterminated `---` parses as a
`ThematicBreak` at offset 0, a file with no fence does not" — is **unsound**. An *indented* `  ---`
also parses as a `ThematicBreak` at offset 0, and it is not an unterminated fence; stage 1's
`split_fences` called it "no fence", correctly. The implementation therefore does not infer the
distinction from the AST at all: it reads the source's first line, skipping at most one BOM, and
asks whether it trims to exactly `---`. That reproduces stage 1 on every case its tests covered and
is a decision the code makes rather than a guess about someone else's parser. The test this section
asked for exists and asserts both halves — identical AST shape, two different errors.

**Stage 2 is the reason to prefer this crate over `pulldown-cmark`.** `stage2.md` already requires a
markdown parser for `[[uuid]]` extraction, so the dependency was arriving regardless. In markdown-rs's
mdast a paragraph's `[[uuid|label]]` arrives as a single `Text` node with byte offsets, while fenced
and inline code are distinct node kinds to skip; `pulldown-cmark` splits the same link across eight
events and would need reassembly. One crate covers both stages. Weight is `markdown v1.0.0` plus one
transitive crate, `unicode-id`.

## The write path

**One path. Byte-replay is deleted.**

```text
render(schema.frontmatter, typed fields) ++ preserved unknown keys ++ body
```

This supersedes ruling §U1 ("preserve on read, normalize on edit") and the two-path design it produced. The argument that retires it: byte-replay existed so that *some* write path could be byte-identical, and under this design **jot writes a note's bytes only when the user edits it.** Trash, restore and purge are file moves and deletes; `sync` and `rebuild` are read-only. The byte-identical replay path has no caller left.

Consequences worth stating:

- **Key order is declared, not hardcoded.** Order comes from `schema.frontmatter`, not from a `KNOWN_KEYS` constant in Rust. Reordering frontmatter is a config edit.
- **`Note::to_bytes` / `to_canonical_bytes` collapse to one function**, which resolves finding F2 structurally rather than guarding it. F2 was: mutate a `pub` field, call `to_bytes()`, watch the edit vanish because the retained bytes were replayed instead. With one path that renders from typed state, there is no second method that ignores the fields, so the hazard becomes an impossible state rather than a documented one. `forget_verbatim()` disappears with it.
- **The byte-identical acceptance criterion is replaced**, not weakened. Phase B established that the
  old criterion was weak evidence by construction — under byte retention it could only fail if retention was not implemented at all. The replacements are stronger: render→parse→render is a fixed point, emitted key order equals the schema, and unknown keys survive byte-for-byte.

### Unknown keys — the hard part

`overview.md`'s forward-compat rule is unchanged and is the main technical problem of this stage.

A key in the file but not in `schema.frontmatter` — `summary:` written by Obsidian, a field from a future version — is **preserved, never interpreted, never dropped.** jot does not read it, act on it, or validate it; every write carries it through unchanged, appended after the schema keys.

Byte-replay gave "verbatim" for free. Rendering gives it only if unknown keys are preserved as **their original text slices** rather than as parsed values re-emitted by the YAML crate. Simple scalars round-trip byte-identically through `yaml_serde`, but the fixture corpus already contains a block scalar and a nested mapping, where an emitter may legally reformat without losing data. Slicing and re-splicing that text is this stage's main implementation risk.

**The markdown crate does not reach this.** It delimits the block; the keys inside it are still
`yaml_serde`'s. Two mechanisms are available and the cheaper one is recommended:

- **Capture each top-level key's source line range** in a pass over the block, before handing it to
  `yaml_serde`. A top-level key is a line at indentation zero; its span runs to the next such line or
  to the block's end. Block scalars, nested mappings and trailing comments then fall out for free as
  continuation lines, which covers both of the hard cases named above. Roughly eighty lines, no new
  dependency. Does not handle explicit `?` keys, anchors and aliases, or a flow collection continuing
  at column zero — all exotic in frontmatter, and none of them handled today either.
- **Move to a YAML parser that carries source spans.** This reopens `yaml-crate.md`'s decision, whose
  stated premise was that byte-replay made emitter fidelity irrelevant to the choice. This stage
  removes that premise, so the decision is genuinely live again rather than merely revisitable — but
  it is a heavier change than the problem currently justifies.

**Implemented: the first option, plus a guard the mechanism needs.** The line-range capture is
roughly what is described here. What is added is that the slicer's limits are **checked rather than
hoped**: the keys the line pass found are compared against the keys `yaml_serde` found, and a
disagreement raises `Error::UnpreservableFrontmatter` naming the file. Stage 1 could afford not to
notice — byte-replay preserved a block whether or not it understood it — and rendering cannot.

One of the exotica listed above turned out not to need the guard. **Anchors and aliases round-trip
fine**, because an alias resolves as long as its anchor is emitted first and unknown keys keep their
relative order. What the guard actually catches is a non-string top-level key, a key the slicer
missed, and a key it invented; a duplicated key is caught earlier still, by `yaml_serde`.

**Preservation is keyed on what jot interprets, not on the schema.** The four keys above become
typed fields; every other key is preserved. The schema governs order and nothing else — which is
what makes the ratified answer to the schema-validation question below non-lossy.

A `strict = true` opt-in that drops unlisted keys may be offered later. It is **not** the default and is out of scope here.

## Behavior against external edits

Two worlds: edits through a jot surface, where the file and the index change together, and edits
through anything else, where the file changes and the index goes stale. The reconciliation rule
follows from the source-of-truth decision and admits no exceptions:

> **The file wins. The index conforms.**

Where the file has *lost* information the index still holds, "the file wins" means **accepting the
loss.** Restoring a deleted value from the index inverts the source-of-truth decision, and must not
happen.

### Missing schema fields

Repaired **on open** — when a surface opens a single note, that one file is rewritten complete and in
schema order. `sync()` and `rebuild()` remain strictly read-only and never rewrite a file; a vault
scan must not produce a diff.

The three cases are not equivalent:

| Deleted externally | Repair |
| --- | --- |
| `title` | Absent means untitled. Optional; no repair value to invent. |
| `relation:root` | **Recompute.** Walk `reply_to` upward; a note with no `reply_to` is its own root. |
| `relation:reply_to` | **Unrecoverable.** The note becomes top-level. Accepted per the rule above; the index conforms. |

Do not write an empty `relation:reply_to:`. Absent means top-level, which is a real state; empty
means "something was here" and nothing can act on it.

### The rebuild invariant exemption

`overview.md` makes "a full rebuild produces the same logical content as an incremental sync" a CI
check from stage 4 onward. With `edited_at` populated from mtime, **that check will legitimately fail
on that field.** The exemption must be written into the invariant now, not discovered by whoever
hits it — the tempting "fix" is to make rebuild write mtime everywhere, which spreads the lossiness
instead of containing it.

## Acceptance

- A note written by jot has its frontmatter keys in exactly `schema.frontmatter` order.
- `render → parse → render` is a fixed point.
- A note carrying `summary:` (not in the schema) survives an edit to its title with `summary`'s bytes
  unchanged, including when its value is a block scalar or a nested mapping.
- A note whose `relation:root` was deleted externally has it recomputed on open; one whose
  `relation:reply_to` was deleted becomes top-level and is not written back as empty.
- `sync()` and `rebuild()` over a clean vault write nothing — `git status` stays empty.
  *(**Deferred to stage 4** — neither function exists yet, by this stage's own "Not in this stage".
  What is closed here is the property they inherit: a full read pass over the corpus changes no
  byte, and repair lives on `open_note`, one file and one user action, deliberately not in any
  vault-wide path.)*
- Two notes created in the same millisecond get distinct filenames and distinct identities.
- `created_at` recovered from a note's filename UUID equals the creation time it was minted with.
- ~~A workspace whose `schema.frontmatter` omits a relation key is rejected at `open`, naming what
  is missing.~~ **Ratified the other way: it warns and opens.** See Open questions.
- For every fixture note, the three slices the parse path cuts — BOM prefix, fenced block, body —
  concatenate back to the original file byte-for-byte. This is the structural property the "body
  untouched" guarantee rests on; if it fails, no later criterion can hold.
- A file with no fence and a file with an unterminated fence produce two *different* errors, each
  naming the path.
- A note whose body contains list markers, emphasis, and hard line breaks survives a title edit with
  every body byte unchanged — the check that no markdown renderer is in the write path.

## Open questions

- ~~**Schema validation for `jot` workspaces.**~~ **Settled 2026-08-31: warn, do not reject.**
  `init`/`open` collect a `Warning::SchemaMissingRelationKeys` and open normally.

  The recommendation to reject rested on "new notes stop carrying it and the vault stops being
  rebuildable from markdown". That premise is false under this stage's own rendering rule: the write
  path emits, after the declared keys, any *interpreted* key the schema omits that the note actually
  carries. A vault with a thin schema still writes `relation:reply_to` on every reply. The omission
  costs diff shape, never thread structure — so refusing to open would lock the user out of their
  notes over a config line, for no safety.

  It is the one case where emitted order is not exactly the schema's, and it cannot arise for a
  schema declaring all four interpreted keys.
- **Concurrent edit.** *(Promoted to `overview.md`'s open questions — it is no longer specific to
  this stage.)* `Workspace::open_note` is the first writer with the problem: it reads, renders and
  writes without re-stat'ing, so an external editor writing in between is clobbered. Stage 4's
  `files` table (size, mtime, hash) is the first place with the machinery to fix it.
- **Ordering churn.** jot writes in schema order; another editor writes in its own. Alternating edits
  produce diff noise in a git-tracked vault. Probably acceptable; worth measuring before deciding.
- **Externally deleted file** — not moved to `.jot/.trash/`, just gone. Not trashed, not purged. The
  index row drops on sync with no tombstone. Confirm that is wanted. *(Promoted to `overview.md` —
  there is no index in this stage for the question to be about.)*
- ~~**Is `title` required, or merely always present in practice?**~~ **Settled: optional**, by
  taking "left as written" at its word. A note with no `title` is untitled; an empty block is an
  untitled top-level note; neither is an error. This is the one place stage 1b *reverses* a stage-1
  behaviour rather than deleting it — `---\n---\n` used to be `FrontmatterNotAMapping`, because an
  empty block had no `id`.
- ~~**Where the markdown-crate decision is recorded.**~~ **Settled: `runs/stage1b/` was opened
  early.** [`runs/stage1b/markdown-crate.md`](runs/stage1b/markdown-crate.md) carries the decision,
  the verification table, the weight (`markdown v1.0.0` plus `unicode-id`), and the two behaviours
  this document got slightly wrong. `Cargo.toml` points at it.
- ~~**Whether the crate swap lands before 1b.**~~ **Settled: inside 1b, one diff.** The mitigation
  for losing the attribution a separate commit would have given: the crate was verified empirically,
  against every case the old `split_fences` tests covered, *before* any implementation code was
  written. That pass is `runs/stage1b/markdown-crate.md`.
