# Stage 7 — What is left of the frontmatter schema

**Status.** Mostly subsumed. This stage had two headline features and both are settled: user-declared
typed frontmatter **landed early**, in `docs/plans/stages/pre-stage4-refactor.md`, and `plain` workspaces are
**deleted**. What remains is the tail of the type system — enums, defaults — plus rename detection,
which never belonged to either feature and is the only genuinely hard thing here.

**Why it moved.** The argument for putting the schema last was that a generalization written before
you have used the specific thing is a guess. That held right up until the schema stopped being a
generalization and became the answer to a stage-4 question: what may the index cache, and what does a
key *mean*? Landing it after SQLite would have meant doing the same work again through a database
migration, and re-deriving the index invariant against a schema that was still moving.

**Why `plain` is gone.** Not a cut for scope: it became *incoherent*. `WorkspaceKind` governed two
things, filename policy and whether threads existed, and once relations are schema-declared the second
is answered by the schema. A workspace declaring no `relation:*` entry **is** what `plain` meant. What
was left was a filename policy wearing the name of a workspace type. The scope check this document
used to end on — "is a `plain` workspace something you will use, or is it symmetry for its own sake?"
— was answered by the type system rather than by a judgment call.

## What landed already

From the pre-stage-4 refactor, and no longer this stage's work:

- `[[schema.frontmatter]]` in `workspace.toml`: an ordered array of tables, each with a `key`, a
  `type`, and an optional `required`.
- `document:title`, `relation:reply_to`, `relation:quote_to` as reserved roles; `text`,
  `text:<refinement>`, `multitext`, `multitext:<refinement>` for everything else.
- `key` defaults to the type string verbatim. Roles are looked up by type, so a title key may be
  called `heading` or `제목` and core still knows.
- Unknown types are preserved and warned about, never refused — the forward-compat rule applied to
  the type system before the type system could break it.
- The manifest is strict (a duplicate role is a parse error); note files are never rejected.
- `required` is a **render** rule: the key is always emitted, empty when the note has no value. It
  never rejects a file, and an empty value parses as absent, so it changes nothing but the diff.

## What is left

### Enums and permitted values

```toml
[[schema.frontmatter]]
key    = "status"
type   = "enum"
values = ["seed", "growing", "done"]
```

The one type the refactor did not settle, because it is the one that needs a second field. It also
needs the rule the others get for free: **validation is advisory, never destructive.** A note whose
`status` is not in `values` is still a note — it loads, it renders, and the violation is a `Problem`
on the scan report. Refusing to open a note because a field is the wrong type would make the schema
more important than the writing, which is backwards.

### Per-key defaults

```toml
[[schema.frontmatter]]
key     = "status"
type    = "enum"
values  = ["seed", "growing", "done"]
default = "seed"
```

**Defaults apply at creation only.** Applying them retroactively would rewrite every file in the vault
on a config change, and a change to the schema must never touch existing notes. Note that this is a
different thing from `required`, which already exists: `required` writes an *empty* key, a default
writes a *value*.

### Field filters, and the boundary to watch

`jot ls --field status=seed`, and the same filter in search. This is the payoff, and the reason stage
4 indexes declared fields into the `fields` JSON column at all.

**Watch the boundary.** A schema with an `enum` and a filter is one honest step away from tags, and
tags are out of scope by decision. The difference worth holding: a declared field describes a note's
*state*, which changes; a tag asserts a note's *category*, which is the filing decision this app
exists to avoid. If the fields start looking like folders, delete them.

### Rename detection

The one item here that was never about the schema, and the only genuinely hard one. It arrived in this
document attached to `plain` workspaces, where path identity made it mandatory. With `plain` gone it is
no longer mandatory — a `jot` note's identity is its filename UUID and survives any rename — so it
demotes to a nicety: noticing that `<uuid>.md` became `<uuid>_a_slug.md` and reporting a move rather
than a delete plus a create. Stage 4's content hash is what makes it detectable.

If it is not worth the cost, it is now safe to drop entirely. That was not true while `plain` existed.

## Work

- [ ] `enum` type with `values`, and advisory validation reported through `SyncReport`.
- [ ] Per-key `default`, applied at creation only.
- [ ] Field filters in `jot ls`, `jot search`, and both UIs.
- [ ] Typed field controls in the desktop composer.
- [ ] Rename detection over stage 4's content hash — **optional**; decide by whether re-slugging
      actually shows up as churn in a real vault.

## Acceptance

- Declaring a field with a default puts it in the next new note and leaves every existing note
  byte-identical.
- A note violating an `enum` still opens, and the violation appears in `jot index status`.
- Removing a field from the schema leaves its values in the files, still round-tripping — the
  forward-compat rule, which is the same rule that made `relation:root`'s deletion a no-op.

## Risks

- **Schema becomes a filing system.** Named above; the mitigation is the state-versus-category test,
  applied honestly.
- **This stage may be empty.** That is a real outcome and an acceptable one. If a year of use never
  wants an enum, the remaining items are a list of things not to build, and the stage closes as
  subsumed rather than as done.
