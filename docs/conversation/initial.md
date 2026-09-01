# Conversation

## 408707ec-6d89-41ee-8ad3-d5a8911d085e

**Date:** 2026-08-26
**Topic:** Initial framing of the note-taking app from `docs/ideas.md`

-- Claude

### User request

- Build a personal note-taking app.
- Read `docs/ideas.md` first, then record the session in this file.

### What `ideas.md` establishes

- Problem: plain Obsidian markdown forces a "what belongs in one file?" decision too early; folder structures constrain ideas rather than hold them.
- Solution: keep markdown files, add SQLite as an extra layer on top.
- File naming: UUIDv7 basename (`01a03d20-a54c-7977-a1f4-1a88b38855dd.md`), optionally suffixed with an underscore slug for human recognition in a file explorer.
- Note shape: required `content` (markdown body), optional `title` (YAML frontmatter).
- UX model: micro-blog timeline (Twitter/Mastodon-like), with **thread** and **quote** relations between notes.
- SQLite holds: title, created / edited / deleted dates, and the thread + quote relations.
- Open question carried in from `ideas.md`: quote is easy (one target note), thread representation is undecided — a separate normalized table is being considered.

### Discussion

#### Source of truth

- SQLite should be a **derived index**, not the only home of the relations.
- Relations (`reply_to`, `quotes`, thread membership) belong in the YAML frontmatter as well, so the vault survives a lost/corrupt database and stays readable from Obsidian.
- Implication: a rebuild path — scan the markdown directory, parse frontmatter, repopulate SQLite — must exist from day one.

#### Quote relation

- Single nullable FK on the note: `quoted_note_id`. Matches the "only one note can be quoted" constraint directly; no extra table needed.

#### Thread relation — two candidate models

1. **Adjacency list** (`reply_to_id`, plus a denormalized `root_note_id`)
   - Mirrors Twitter/Mastodon semantics: a thread is whatever chain grew from a root.
   - `root_note_id` makes "fetch the whole thread" one indexed query instead of a recursive walk.
   - Branching is free (two replies to the same note fork the thread).
   - Ordering within a level comes from UUIDv7, which is already time-sortable — no explicit position column.
2. **Explicit thread table** (`threads` + `thread_notes(thread_id, note_id, position)`)
   - Lets a user curate a thread *after the fact* from notes that were written independently.
   - A note can belong to several threads; ordering is arbitrary and editable.
   - Costs: no natural single-parent semantics, position maintenance on reorder, harder to express in frontmatter.

- **Recommendation:** adjacency list (`reply_to_id` + `root_note_id`) as the primary model, because it fits how notes are actually written (append a follow-up thought to the previous one) and keeps every relation expressible as a single frontmatter field.
- If retroactive grouping turns out to be needed, add the join table later as a separate "collection" concept rather than overloading threads — collections are a different intent (curation) from threads (chronology).

#### Soft delete

- `deleted_at` in SQLite implies soft delete. Decide what happens to the markdown file: leave it on disk, move it to a trash folder, or mirror the flag into frontmatter. Leaving the file and mirroring the flag keeps the vault self-describing.

### Decisions

- Markdown files stay the source of truth; SQLite is a rebuildable index.
- Quote: single nullable FK.
- Thread: adjacency list with a denormalized root pointer.

### Open questions

- Which platform / stack? (desktop app over a local vault, local web app, Obsidian plugin, CLI + TUI)
- Does the app own the vault directory, or does it point at an existing Obsidian vault?
- How is the filename slug generated and kept in sync when the title changes — regenerate the filename, or write it once and leave it?
- Are tags, links, or full-text search in scope for the first version?
- Does soft delete hide a note's replies as well, or do they reattach to the deleted note's parent?

-- User

### Overall

I think adjacency list is close to the right model, but the chain will essentially be a tree, so there should be a root.

```text
  B - C - E
 /     \
A       D
 \
  F
```

Let's say there is something like this, then can we model it as either:

- (A, B, C, D), (A, B, C, E), (A, F)
- (A, B, C), (A, F), (C, D), (C, E)

### on Soft delete

About soft delete, it will only be marked when the note is deleted through this interface.
Then it make sense to move to the trash directory.
As pointed out in the `Source of truth` section, I agree that SQLite should be derived index, and it is basically to improve time when querying a note with the title and metadata mainly.
(I haven't thought about the content, so let's not thinking about it yet)

### on Open questions

- I first want it to be desktop app and CLI + TUI as it will provide me options to integrate with other tools and interfaces I currently use.
- This app owns independent directory as it is mainly to capture thoughts
- I'm not thinking about tags, but links for sure. For the full-text search, let't not think about it at the moment
- For notes that are deleted, the interface should indicate there was parent
  - for soft delete, indicate it is trashed
  - for file deleted (hard), indicate it is deleted

### Additional points

I would like this to handle multiple workspaces. A workspace refers to the directory with its own dedicated sqlite file.

I'm thinking about having different types of the workspace

- one we are currently discussing; flatten file with UUID as file title
- a regular markdown editor

-- Claude

### on Thread model

Both forms you wrote are correct *views* of the same tree; neither should be the stored form.

- Form 1 — `(A,B,C,D), (A,B,C,E), (A,F)` — enumerates every root-to-leaf path.
  - `A,B,C` is repeated across two rows. Cost is O(leaves × depth), and appending one reply to `C` invalidates every path row passing through `C`.
  - Right shape for *reading one branch linearly*, which is what a micro-blog shows when you open a leaf.
- Form 2 — `(A,B,C), (A,F), (C,D), (C,E)` — decomposes the tree into chains that start at the root or at a branch point.
  - Every **edge** appears exactly once, and every node appears once as a segment member. Segment count = one per child of each branch point (`A` has 2, `C` has 2 → 4 segments).
  - Right shape for *rendering the tree*: each segment is a run the UI draws as a straight line, and each segment start is exactly where a collapse/expand control belongs.
- Stored form stays adjacency, because it is the only one where appending a note is a single insert with no rewrite:

  ```sql
  notes(
    id          TEXT PRIMARY KEY,  -- UUIDv7
    root_id     TEXT NOT NULL,     -- own id when the note is a root
    reply_to_id TEXT,              -- NULL when the note is a root
    quoted_id   TEXT,              -- single target, cross-tree, not part of root_id
    ...
  )
  ```

  - `root_id` is copied from the parent on insert — O(1) — and `WHERE root_id = ?` pulls the whole tree in one indexed query.
  - Form 1 and form 2 are then computed in memory from that result set. At personal-notes scale a tree is tens of nodes; there is nothing to win by persisting either projection.
- Sibling order comes from UUIDv7 — creation order, no `position` column.
- A quote is a cross-tree edge: it never changes `root_id`, and it never makes the quoted note part of the quoting note's thread.

### on Soft delete

Adopted as you described, with one addition:

- Trash is a directory inside the workspace: `<workspace>/.trash/<uuid>[_slug].md`. Location on disk *is* the trashed state, so the vault stays self-describing and the index stays purely derived.
- Write `trashed_at` into the frontmatter at move time. Filesystem mtime is not a reliable record of when the move happened (copies, syncs, and restores all rewrite it), and that timestamp is the one fact the directory location cannot carry.
- Rebuild scans the workspace root **and** `.trash`; whichever directory a file is found in determines its state.
- Index scope confirmed: title, dates, and relations only — body is not indexed. The scanner only needs to read each file up to the closing frontmatter delimiter.

Three parent states the interface has to distinguish, all derivable without extra bookkeeping:

| State | How it is detected | How it renders |
| --- | --- | --- |
| Present | file exists in the workspace root | normal card |
| Trashed | file exists in `.trash` | dimmed placeholder, "Trashed" + Restore |
| Deleted | `reply_to_id` points at a UUID with no file anywhere | placeholder showing the UUID only, "Deleted" |

"Deleted" is a dangling reference rather than a tombstone row — so it survives a full index rebuild, which a tombstone would not.

### on Workspaces

- **Manifest in the vault:** `<workspace>/.jot/workspace.toml` holding `id`, `type` (`jot` | `plain`), and a schema version. A directory identifies itself, so registering a workspace is just pointing at it.
- **Registry in app config:** the list of known workspaces (path, display name, last opened) lives in the app's config directory. It is a convenience cache; losing it costs one re-pick of the folder.
- **Where the SQLite file goes — needs your call.** It is one database per workspace either way; the only question is whether the file sits inside the vault.
  - In-vault (`.jot/index.db`) is simpler and matches how you described it, but a vault in Dropbox/iCloud/OneDrive will eventually corrupt a SQLite file that two machines touch — and that failure is confusing when it happens.
  - App-data (keyed by the manifest's `id`) never syncs, and the DB is disposable by design.
  - Recommendation: app-data by default, with an option to relocate. The vault then holds only markdown plus a small TOML manifest.
- **Type capabilities:** the core is shared — file I/O, frontmatter, index, links, trash, search. Only the surface differs.

  | | `jot` | `plain` |
  | --- | --- | --- |
  | Filenames | UUIDv7 (+ optional slug) | arbitrary, user-chosen |
  | Layout | flat | folders |
  | Threads / quotes | yes | no |
  | Links | yes | yes |
  | Primary view | timeline | file tree + editor |

### Information architecture

#### Principle

Capture is one keystroke; retrieval is progressive; **organization is deliberately absent**. No folders, no tags, no "where does this go?" — being forced into that decision too early is the problem the app exists to remove. Every structure in the interface is derived from relations made *while writing*, never from filing done afterward.

#### Nouns

`Workspace` → `Note` → relations (`reply`, `quote`, `link`). Plus `Trash` as a state, not a place things get filed into.

#### Navigation tree (`jot` workspace)

```text
Command palette ─ global, every action, keyboard-first
Capture overlay ─ global hotkey, an overlay not a destination
Workspace switcher
└─ Workspace
   ├─ Timeline (home)      roots, newest first
   │  └─ Note detail       ancestors ▲ | focus | descendants ▼
   ├─ Search               title + metadata filters
   └─ Trash                restore / purge
```

Three destinations. Depth stops at the note: a quoted note is embedded inline, never expanded in place — clicking it navigates.

#### Layout

```text
┌──────┬─────────────────────────────┬──────────────┐
│ rail │ main                        │ context      │
│      │                             │ (collapsed   │
│  ws  │ timeline · thread · search  │  by default) │
│ nav  │                             │              │
│      │                             │ metadata     │
│      │                             │ backlinks    │
│      │                             │ quoted-by    │
└──────┴─────────────────────────────┴──────────────┘
```

#### Capture

The core loop; everything else is retrieval.

- Global hotkey → overlay opens with the content field focused. Title is a collapsed optional field, never a required first step.
- Launched from a note, the overlay carries a relation chip (`Replying to …` / `Quoting …`) that can be dismissed.
- Save → toast with `Open` / `Reply` actions. No navigation required to write the next thought.

#### Timeline

- Default: **root notes only**, newest first, each showing its reply/branch count. One row per idea-chain; a flat firehose repeats thread bodies.
- Toggle: **flat** — every note, newest first. Answers "what did I write today".
- Day separators for grouping. Cursor pagination comes free from UUIDv7.

#### Note detail (thread view)

- **Ancestors** — the path from root to parent, always linear and unambiguous. Collapse to root + "N earlier notes" past ~3.
- **Focus** — full render, metadata visible.
- **Descendants** — form 2 (segments). Expand the first child chain by default, since chronological continuation is how the thread was written; collapse the rest as "N other continuations".
- **Quoted note** — nested card, exactly one level, never recursive.
- Sibling branches are what makes this different from Twitter: the interface should make forking feel routine, because a note that branches is exactly the case with no good answer in a folder-based vault.

#### Note card anatomy

Shared by timeline, thread, and search results:

- Header — title or `Untitled`, relative time, edited marker
- Body — rendered markdown, clamped to ~10 lines with `show more`
- Quote — nested card (one level)
- Footer — `reply` `quote` `link` `copy id` `open` `trash`, plus counts
- States — trashed (dimmed + Restore), missing-parent placeholder per the table above

#### CLI / TUI parity

Same nouns, one verb per action, so muscle memory transfers both directions. Short-prefix id resolution like git hashes.

```text
jot new [-t <title>] [--reply <id>] [--quote <id>]   # also reads stdin
jot ls [--flat] [--since <when>]
jot show <id>
jot thread <id> [--tree | --path]                    # form 2 | form 1
jot open <id>                                        # hands off to the desktop app
jot rm <id> | jot restore <id> | jot purge <id>
jot ws ls | use <name> | add <path> | new <path> --type jot|plain
```

- The TUI mirrors the same three regions. `j/k` move, `Enter` opens the thread, `r` reply, `q` quote, `u` up to parent, `/` search.
- A `jot://<workspace>/note/<uuid>` URI scheme gives other tools a stable way to link into the app — the integration point you named as the reason for wanting both frontends.

#### `plain` workspace

Same shell. The three destinations become `Files` (tree) / `Search` / `Trash`, the main pane is an editor, and capture writes into a configured default folder. Links and backlinks behave identically.

### Decisions (updated)

- Storage: adjacency list (`reply_to_id` + `root_id`); form 1 and form 2 are render-time projections.
- Quote: single nullable FK, cross-tree, outside the thread.
- Trash: move to `<workspace>/.trash/`, stamp `trashed_at` in frontmatter.
- Parent states: present / trashed / deleted, all derived.
- Index scope: title, dates, relations. Not body.
- Platforms: desktop app + CLI/TUI over a shared core.
- Workspaces: self-identifying via an in-vault manifest; two types (`jot`, `plain`).
- Tags out, links in, full-text search deferred.

### Open questions (updated)

- SQLite file location: in-vault vs. app-data (recommendation above).
- Language/stack for the shared core. One language across core, desktop shell, and TUI keeps the surface honest — Rust (rusqlite + Tauri + ratatui) or TypeScript (Tauri/Electron + Ink) are the realistic picks.
- Do links resolve across workspaces, or only within one?
- Filename slug on title change: regenerate the filename, or write it once and leave it?
- Does trashing a note trash its replies, or do they stay live with a trashed-parent placeholder?

-- User

- I would place sqlite file within the vault, and thinking about Tauri + Ratatui
- A workspace is an independent unit so links shouldn't resolve across workspaces
- For filename slug, this doesn't have to be decided at the moment.
- Trashing a note with its reply as soft delete should just display the placeholder.

For the workspace structure, this is what I'm thinking

```text
<workspace_root>/
  .jot/
    .trash/         # folder for trashed file
    workspace.toml  # where the configuration lives
    {data,index}.db # SQLite database file
```

There are two main views I want; `timeline view` and `file list and reader view`

- timeline view will display the latest notes
- file list and reader view will have list of files with different sort order on the left half and reader view on the right half

Also, I'm thinking about the frontmatter schema as an idea so that all the files within a workspace will have it within the frontmatter for sure. It could also have default value and type checking, but we can go further later.

-- Claude

All open questions are now closed except the two noted below. Plan written to `docs/plans/`.

### Decisions locked

- Stack: Rust core (`jot-core`), `clap` CLI, `ratatui` TUI, Tauri v2 desktop.
- Build order: CLI → TUI → desktop. The CLI is a thin shell over the core, so it proves the data model against real notes before any UI exists — and it is the surface that makes the app scriptable, which was the point of wanting more than a desktop app.
- SQLite in the vault at `.jot/index.db`, per your layout. `.jot/.gitignore` excludes `index.db*` so a vault can be tracked in git while the disposable index is not.
- Trash moves to `.jot/.trash/` — inside `.jot/`, as you drew it. The workspace root stays a flat list of live notes and nothing else.
- Links resolve within one workspace only.
- Trash never cascades: replies to a trashed note stay live and render a trashed-parent placeholder.
- Filename slug stays undecided — it becomes a `workspace.toml` knob (`[notes] filename = "uuid" | "uuid_slug"`) defaulting to the bare UUID, so it can be settled later without a migration.

### Three points from your layout worth a nod

- **`{data,index}.db`** — the plan assumes a single `.jot/index.db`. Naming a file `data.db` invites treating it as the source of truth, which is the one thing it must not become. Say if the second file was meant to hold something specific.
- **Two views** — `timeline` and `file list + reader` are now the two primary destinations, with search and trash behind them. The files+reader view is built type-agnostic from the start, because it is also the entire interface for a `plain` workspace later.
- **A correction to something I wrote earlier.** I said the scanner only needs to read each file up to the closing frontmatter fence. That does not survive links — extracting `[[uuid]]` requires the body. The scanner now reads whole files but still stores only metadata, so your decision (no content in the index) is unchanged; only my claim about the read was wrong.

### On the frontmatter schema idea

Worth building, with two constraints that keep it from becoming the thing you were escaping:

- **Advisory, never destructive.** A note violating the schema still opens and renders; the violation is surfaced as a problem, not an error. Otherwise the schema becomes more important than the writing.
- **Defaults apply at creation only.** A schema change must never rewrite existing notes.
- The boundary to watch: a declared `enum` field plus a filter is one step from tags. A useful test — a declared field describes a note's *state*, which changes; a tag asserts its *category*, which is the filing decision this app exists to avoid.

It lands in stage 7 rather than now, because it is a generalization, and a generalization written before months of real notes is a guess. Stage 1's rule that unknown frontmatter keys survive every write is what keeps that option open at zero cost.

### Plan shape

`docs/plans/overview.md` plus `stage1.md`–`stage7.md`.

| # | Stage | Delivers |
| --- | --- | --- |
| 1 | Vault foundations | workspace on disk, frontmatter round-trip, atomic writes |
| 2 | Index and rebuild | SQLite schema, scanner, deterministic rebuild |
| 3 | Notes and threads | lifecycle, thread algebra (forms 1 and 2), links |
| 4 | CLI | `jot` — daily-usable capture and retrieval |
| 5 | TUI | timeline, thread, files+reader, search, trash |
| 6 | Desktop | Tauri app, capture overlay, `jot://` deep links |
| 7 | Schema and `plain` workspaces | declared frontmatter, second workspace type |

Stages 1–3 are one continuous piece with nothing user-visible; the first usable moment is the end of stage 4, and everything after it should be reordered by what a week of real dogfooding teaches.
