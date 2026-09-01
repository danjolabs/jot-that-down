# Stage 7 — Frontmatter schema and `plain` workspaces

**Goal.** A workspace declares the frontmatter its notes carry, with types and defaults; and the
second workspace type — an ordinary markdown directory — becomes real.

**Why last.** Both features are generalizations, and a generalization written before you have used the
specific thing is a guess. By now you have months of real notes and a clear sense of which fields you
actually keep typing by hand. Stage 1's rule that unknown keys survive every write is what makes this
stage possible without a migration.

## Frontmatter schema

Your idea from `docs/conversation/initial.md`: every file in a workspace carries a known frontmatter shape,
with defaults and type checking.

### Declaration

In `workspace.toml`, so it travels with the vault:

```toml
[frontmatter.status]
type     = "enum"
values   = ["seed", "growing", "done"]
default  = "seed"
required = false

[frontmatter.source]
type     = "string"
required = false

[frontmatter.pinned]
type     = "bool"
default  = false
```

Types: `string`, `int`, `float`, `bool`, `date`, `datetime`, `enum`, and `list<T>`. That covers what a
note actually carries; resist anything more expressive — this is a note format, not a data modeling
language.

### Rules

- **Built-in fields are reserved.** After stage 1b there are four: `title`, `relation:root`,
  `relation:reply_to`, `relation:quote` — `jot_core::frontmatter::INTERPRETED_KEYS`. They cannot be
  redeclared with a different meaning or overridden. They *are* already declared in
  `[schema] frontmatter`, which is what fixes their emitted order, and a schema may name them in
  any order or omit one (a `jot` workspace warns, and the write path still emits an omitted
  relation a note carries).
- **Defaults apply at creation only.** Applying them retroactively would rewrite every file in the
  vault on a config change — a change to the schema must never touch existing notes.
- **Validation is advisory, never destructive.** A note that violates the schema is still a note: it
  loads, it renders, and the violation is surfaced as a problem in `SyncReport` and shown in the UI.
  Refusing to open a note because a field is the wrong type would make the schema more important than
  the writing, which is backwards.
- **Removing a field from the schema leaves the data.** The values stay in the files as unknown keys,
  exactly as stage 1 guaranteed.

### Where it shows up

- `jot new` prefills declared defaults into the template.
- The desktop composer renders a declared field as a small typed control beneath the collapsed title.
- `jot ls --field status=seed` and the same filter in search — this is the payoff, and the reason to
  index declared fields.
- Index: a `note_fields(note_id, key, value)` table, populated by the scanner. Simple, sparse, and it
  keeps the `notes` schema stable.

**Watch the boundary.** A schema with an `enum` and a filter is one honest step away from tags, and
tags are out of scope by decision. The difference worth holding: a declared field describes a note's
*state*, which changes; a tag asserts a note's *category*, which is the filing decision this app
exists to avoid. If the fields start looking like folders, delete them.

## `plain` workspaces

An ordinary markdown directory — arbitrary filenames, real folders, no threads.

| | `jot` | `plain` |
| --- | --- | --- |
| Filenames | UUIDv7 (+ optional slug) | arbitrary, user-chosen |
| Layout | flat | nested folders |
| Threads / quotes | yes | no |
| Links | yes | yes |
| Views | timeline, files+reader, search, trash | files+reader, search, trash |

### What changes in core

- **Identity without a UUID filename.** A `plain` note is identified by its path. Give it a stable id
  in frontmatter on first index — writing to a file the user did not just edit is intrusive, so make
  it opt-in per workspace and fall back to path-as-identity when declined.
- **Recursive scanning**, honoring `.gitignore`-style excludes.
- **Renames and moves** become real events; a path-identified note that moves must not look like a
  delete plus a create. Content hash from stage 4 is what makes this detectable.
- **Links become `[[filename]]`**, not `[[uuid]]` — resolution by path or basename, still strictly
  within one workspace.
- Threads, quotes, and the timeline are absent, not empty. The rail shows three destinations, and the
  files+reader view built type-agnostic in stage 5 does the work.

### Scope check

The honest question to ask before building this: is a `plain` workspace something you will use, or is
it symmetry for its own sake? Obsidian already handles that directory well. If the answer is "I want
one app open instead of two", build it. If it is "it seems incomplete without it", skip it — every
line here is a line not spent on the capture loop that is the actual point.

## Work

- [ ] Schema declaration parsing and validation in `workspace.toml`, with reserved-field enforcement.
- [ ] `note_fields` table, populated by the scanner; migration from stage 4's schema.
- [ ] Defaults applied at creation; violations reported through `SyncReport` and rendered as advisory.
- [ ] Field filters in `jot ls`, `jot search`, and both UIs.
- [ ] Typed field controls in the desktop composer.
- [ ] `plain` workspace kind: recursive scan, path identity, rename detection, filename links.
- [ ] Views degrade correctly by workspace kind — no empty timeline in a `plain` workspace.

## Acceptance

- Declaring a field with a default puts it in the next new note and leaves every existing note byte-identical.
- A note violating the schema still opens, and the violation appears in `jot index status`.
- Removing a field from the schema leaves its values in the files, still round-tripping.
- A `plain` workspace indexes a nested directory, and moving a file inside it is a move, not a
  delete-plus-create.
- Switching between a `jot` and a `plain` workspace changes the available destinations with no dead views.

## Risks

- **Schema becomes a filing system.** Named above; the mitigation is the state-versus-category test,
  applied honestly.
- **Two workspace types double the test matrix.** Every core operation now has two behaviors. Make the
  kind an explicit parameter in the fixture harness so both run everywhere, rather than testing `jot`
  and hoping `plain` follows.
- **Path identity is genuinely harder than UUID identity.** Renames, case-insensitive filesystems on
  Windows, and duplicate basenames are all real. This is most of the cost of the `plain` type — weigh
  it against the scope check above before starting.
