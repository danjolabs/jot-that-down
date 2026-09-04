# Stage 1 — Vault foundations

> **Superseded in part by [stage1b.md](stage1b.md).** This stage shipped `id` duplicated in the
> filename and the frontmatter, the two-path (preserve-on-read, normalize-on-edit) serializer under
> §U1, and the byte-identical round-trip gate below. Stage 1b moves identity to the filename only,
> replaces the two-path serializer with one schema-driven render path, and replaces the round-trip
> gate with a stronger set of criteria. The content below is left as-is — it is the record of what
> stage 1 actually built and what phase B's verdicts in `docs/runs/stage1/verification.md` are about —
> but the "Note format", "Frontmatter", and the round-trip acceptance criterion are no longer current.
> Read `stage1b.md` for what replaced them.

**Goal.** A workspace exists on disk, identifies itself, and round-trips notes without losing a byte.

**Why first.** Everything downstream is derived from these files. A frontmatter writer that reorders
keys or drops unknown ones is a bug you discover in stage 7, after a year of notes have been through
it. Get the file format exactly right while it costs nothing.

**Not in this stage.** SQLite, queries, the CLI surface, threads. Notes can be written and read; they
cannot yet be *found*.

## On-disk contract

```text
<workspace_root>/
  .jot/
    workspace.toml            # identity + config
    index.db                  # stage 4
    .trash/                   # trashed notes keep their filename
    tmp/                      # staging area for atomic writes
    .gitignore                # index.db*, tmp/
  01a03d20-a54c-7977-a1f4-1a88b38855dd.md
  01a03d21-7c11-7a02-b3de-9f0e21c4a771_first_thoughts.md
```

`.trash/` lives inside `.jot/` so the workspace root stays a flat list of live notes and nothing but.
A vault under git therefore tracks notes and trash, and ignores the disposable index — the plain-files
premise buys you free history if you want it.

### `workspace.toml`

```toml
schema_version = 1

[workspace]
id   = "b4b4856a-e5db-4f9b-bd87-658b0be50741"   # UUIDv4, minted at init, immutable
kind = "jot"                                     # deleted in the pre-stage-4 refactor
name = "Thoughts"                                # display only

[notes]
filename = "uuid"      # "uuid" | "uuid_slug" — deferred decision, see overview
```

`id` is what makes the directory self-identifying: registering a workspace is pointing at a folder,
and moving the folder loses nothing.

**v4, not v7** — changed post stage 3, see `docs/runs/post-stage3/log.md`. This originally said UUIDv7,
by inheritance from note ids rather than by argument: the self-identifying property this paragraph
describes holds for any UUID version. A **note** id must be v7, because `created_at` is decoded from
it and id order is creation order. A workspace id is asked for neither, and v7 actively cost two
things — its timestamp prefix made short ids long, which broke `jot ws use <prefix>` for vaults
created in the same minute, and it wrote a creation date into a file people commit. Reading is
unaffected: `open` parses any version, so vaults minted before the change keep their v7 ids.

### Note format

```markdown
---
id: 01a03d21-7c11-7a02-b3de-9f0e21c4a771
title: Jot that down
created_at: 2026-08-26T09:00:00Z
edited_at: 2026-08-26T09:12:00Z
reply_to: 01a03d20-a54c-7977-a1f4-1a88b38855dd
root: 01a03d20-a54c-7977-a1f4-1a88b38855dd
quote: 01a03d10-3f8a-7bb1-9c22-0e1d5a6b7c88
---

The body. Plain markdown, untouched.
```

Required: `id`, `created_at`, `root`. Optional: everything else. `trashed_at` appears only while the
file sits in `.trash/`.

Three rules worth stating outright, because each one is a decision:

- **`id` is duplicated in the filename and the frontmatter, and the frontmatter wins.** The filename
  is a convenience for file explorers and can be renamed by anything; the frontmatter is the note's
  actual identity. A disagreement is reported by the scanner, not silently resolved.
- **`root` is stored, not derived.** It is computable by walking `reply_to` upward — until an ancestor
  is purged, at which point the chain breaks and the surviving subtree loses its grouping. Writing it
  once at creation makes the tree survive a hole punched in its middle.
- **Unknown keys are preserved verbatim.** Round-tripping a note written by a future version, or by
  Obsidian, or by hand, must not destroy fields this version doesn't know about.

## Work

### Scaffold

- [ ] Cargo workspace; `crates/jot-core` with the module skeleton (`workspace`, `note`, `frontmatter`, `fs`, `error`).
- [ ] Pin the toolchain (`rust-toolchain.toml`), enable `clippy -D warnings` and `rustfmt` in CI.
- [ ] CI matrix on Windows and Linux from day one — the atomic-write behavior genuinely differs.

### Domain types

- [ ] `NoteId(Uuid)` newtype — parse, display, `short()` for the 8-char prefix, ordering by the UUIDv7 timestamp.
- [ ] Minting: `NoteId::new()` from `uuid` with the `v7` feature. Two notes created in the same
      millisecond must still order deterministically (v7 handles this; assert it in a test).
- [ ] `Frontmatter` — typed known fields plus an ordered map of unknown ones.
- [ ] `Note { meta: NoteMeta, body: String }`; `NoteMeta` is everything except the body, and is what
      every list view will be built from.

### Frontmatter

- [ ] Pick a maintained YAML crate. `serde_yaml` is archived; evaluate the current forks and record
      the choice and date in this file. This is a small decision with a long tail — write down why.
      **Chosen 2026-08-30: `yaml_serde` 0.10.7** (the `serde_yaml` continuation under the official
      `yaml` GitHub org), with `chrono` 0.4.45 for timestamps and `toml` 1.1 for `workspace.toml`.
      Full evidence and the one caveat — no serde-lineage emitter can be told to quote a scalar, so
      the canonical path must emit timestamps as explicitly double-quoted strings itself — in
      [`docs/runs/stage1/yaml-crate.md`](../../runs/stage1/yaml-crate.md).
- [ ] Parse: split on the leading `---` fence, deserialize, keep the body as the exact remaining bytes.
- [ ] Serialize: known keys in a fixed order, then unknown keys in their original order.
- [ ] **Round-trip test as the gate**: parse → serialize of an unmodified note is byte-identical.
      Any file in the fixture vault that fails this is a bug in the writer, not in the file.
- [ ] Reject gracefully: no fence, unterminated fence, malformed YAML, missing `id` — each produces a
      distinct error naming the path.

### Filesystem

- [ ] Atomic write: stage in `.jot/tmp/`, `fsync`, rename over the target.
- [ ] **Windows**: confirm the rename replaces an existing file (`MOVEFILE_REPLACE_EXISTING`). Write
      the test that overwrites an existing note and run it on Windows before trusting it.
- [ ] Filename parsing: `<uuid>.md` and `<uuid>_<slug>.md`; the slug is decorative and ignored.
- [ ] Enumerate: workspace root (live) and `.jot/.trash/` (trashed), non-recursive for `jot` kind,
      skipping `.jot/` and any dotfile.

### Workspace

- [ ] `init(path, kind)` — create the tree, mint the id, write `workspace.toml` and `.jot/.gitignore`.
      Idempotent: running it on an existing workspace is an error, not a silent overwrite.
- [ ] `open(path)` — read and validate the manifest; refuse a `schema_version` from the future with a
      message saying so plainly.
- [ ] `discover(from)` — walk parent directories looking for `.jot/`, so the CLI works from any
      subdirectory later.
- [ ] Registry in the OS config dir (`directories` crate): known workspaces (path, name, last opened)
      and the current one. Treat it as a cache — a missing or corrupt registry costs one re-add,
      never data.

## Acceptance

- `Workspace::init` on an empty directory produces the exact tree above.
- A hand-written note file parses; re-serializing it changes nothing (`git diff` is empty).
- A note carrying an unknown frontmatter key survives a parse → write cycle with the key intact.
- A note whose filename UUID disagrees with its frontmatter `id` is reported, and the frontmatter wins.
- Overwriting an existing note file succeeds **on Windows**, and an interrupted write leaves the
  original intact.
- `discover()` finds the workspace from three directories deep.

## Risks

- **Silent frontmatter mangling** is the expensive failure here — it corrupts data slowly and
  invisibly. The byte-identical round-trip test is the only real defense; make it run over every
  fixture, and add every weird real-world note you meet to the fixtures.
- **YAML is a large format.** Restrict the known fields to scalars and strings. Do not let anchors,
  multi-document files, or exotic types into the note format.
