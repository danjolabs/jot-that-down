# Stage 2 — Notes and threads

**Goal.** The complete domain: notes are created, edited, trashed, restored, purged; threads project
into both of the forms settled in `docs/conversation/initial.md`; links resolve within the workspace.

**Why now.** This is the app's actual logic, and it is the last stage with no user interface to hide
behind. Every rule proved here by a test is a rule three surfaces get for free. Every rule left
implicit here gets reimplemented three times, slightly differently.

**Not in this stage.** Anything a person can type at. Still library-only.

> **Built, and built before stage 4.** Everything below is implemented in `jot-core` against
> `snapshot::Snapshot` rather than SQLite — see [overview.md](overview.md), "Build order changed".
> No rule in this document was weakened to make that work; the index was only ever the *fast* way to
> get the note set, and a scan is the slow way to get the same one. Two findings from the build are
> recorded at the foot of this file.

## Note lifecycle

| Operation | Filesystem | Index | Frontmatter |
| --- | --- | --- | --- |
| `create` | write `<uuid>[_slug].md` in root | insert | `relation:root`, and any other relation |
| `edit` | rewrite in place | update | nothing new in the file; `edited_at` follows mtime |
| `trash` | move to `.jot/.trash/` | `state = 'trashed'` | nothing — the directory is the state |
| `restore` | move back to root | `state = 'active'` | nothing |
| `purge` | delete the file | delete the row | — |

Stage 1 phase B found a hazard on `edit`'s write: mutating `Note`'s public fields and then calling the
byte-preserving `to_bytes()` silently discarded the edit, because that path replayed retained bytes
rather than rendering current state. Stage 1b removes the hazard at the source rather than leaving it
for `edit` to guard against — the two-path serializer is gone, there is one write path that always
renders from typed state, and the impossible state (a write method that ignores the fields you just
set) no longer exists. `edit` has nothing to enforce here beyond calling the one render path there is.

Rules that follow from the locked decisions and must be enforced in one place:

- **Trash never cascades.** Trashing a note with replies moves exactly one file. The replies stay
  live and render a trashed-parent placeholder — your call, and it is also the only behavior that
  survives a rebuild without extra bookkeeping.
- **The frontmatter column carries only what stage 1b left in the file.** `id` and `created_at`
  are the filename's; `edited_at` is index-only. *(Amended: `trashed_at` went with it in
  `stage4.md` — one `mtime_ns` per note, and `state` says what it means.)* `create` mints the id, and the
  creation-time `FilenameSlug` option decides whether the filename gets a slug from the title.
  Re-slugging on a title change is safe: the identity is the UUID and it does not move.
- **`relation:root` does not exist.** It was assigned once at creation and never recomputed, on the
  argument that this kept a subtree grouped when a note in the middle of it was purged. The
  pre-stage-4 refactor deleted the key: a root is **derived** from `relation:reply_to` at scan time,
  and purging a middle note therefore *splits* the subtree.

  The reversal is safe because `root_id` was never what provided the property it was defended for.
  "There was a chain here and a post is gone" comes from the surviving child's dangling
  `relation:reply_to`, which points at an id the vault no longer holds and resolves to
  `Ref::Deleted`. That lives in the file and is untouched. Sibling grouping survives too — children
  of a purged parent all carry the *same* missing id. What is genuinely lost is grouping across
  **two** purges, and at that point the chain really has been broken twice.
- **Re-parenting is not supported.** Nothing in the design needs it. If it is ever wanted, it
  arrives as an explicit `reparent` that rewrites `relation:reply_to` — not as a side effect of an
  edit. It no longer implies a subtree rewrite, since there is no stored root to rewrite; what it
  does imply is a `reply_to` cycle interrupted halfway, which is one of the ways a cycle actually
  arrives.
- **A quote is not a thread edge.** `relation:quote_to` never affects the derived root, and the
  quoted note never joins the quoting note's tree.
- **Purge is the only irreversible operation.** It requires explicit confirmation at every surface.

### Reference resolution

Every reference — `reply_to`, `quote`, a link target — resolves to one of three states, computed, never stored:

```rust
enum Ref { Present(NoteMeta), Trashed(NoteMeta), Deleted(NoteId) }
```

- `Present` — an `active` row exists.
- `Trashed` — a `trashed` row exists (file is in `.jot/.trash/`).
- `Deleted` — no row at all. The id is all that remains, and that is what the UI shows.

Surfaces render these three and nothing else. A fourth case appearing in a surface means the rule
leaked out of core.

## Thread algebra

The heart of the stage, and the part worth writing property tests for. Load once, project many times:

```rust
struct Thread {
    focus:     NoteId,
    ancestors: Vec<NoteMeta>,   // root → parent, always linear
    tree:      TreeNode,        // focus and everything below it
}

impl TreeNode {
    fn paths(&self)    -> Vec<Vec<NoteId>>;  // form 1: every root-to-leaf path
    fn segments(&self) -> Vec<Segment>;      // form 2: chains from root or branch point
}
```

Both forms come from a single `SELECT * FROM notes WHERE root_id = ?` assembled in memory. Neither is
persisted; a thread is tens of nodes and there is nothing to win.

Using the example from `docs/conversation/initial.md` — edges `A→B`, `B→C`, `C→E`, `C→D`, `A→F`:

```text
  B - C - E          paths     (A,B,C,D), (A,B,C,E), (A,F)
 /     \             segments  (A,B,C), (A,F), (C,D), (C,E)
A       D
 \
  F
```

### Invariants to test as properties

Generate random trees; assert on every one:

- **Segments partition the edges.** Every edge appears in exactly one segment, and the total edge
  count across segments equals `nodes - 1`.
- **Segment count** equals the number of children of the root plus, for every branch point, its
  number of children. (Here: `A` contributes 2, `C` contributes 2 → 4.)
- **Segments cover every node**, each exactly once as a non-first element.
- **Paths cover every node**, and every path starts at the root and ends at a leaf; the number of
  paths equals the number of leaves.
- **Ancestors are linear and terminate** — no cycles, and the walk ends at a note whose `reply_to` is
  absent or unresolvable.
- **A cycle is rejected, not hung on.** Hand-edited frontmatter can produce `A→B→A`. Detect it during
  the walk and return a diagnosable error; never loop forever.

### Sibling order

Creation order, from UUIDv7. No `position` column, no ordering metadata, nothing to keep consistent.

## Links

**The parser is already a dependency.** Stage 1b delegates frontmatter fence splitting to
`markdown` 1.0.0 (markdown-rs), chosen partly *for* this stage — see `stage1b.md`, "The parse path".
This stage adds no new crate and inherits the same rule: parse to an AST, read byte offsets, never
call the renderer.

- [ ] Extract `[[<uuid>]]` and `[[<uuid>|label]]` from the body during scan by walking the mdast,
      not by regex over raw text, so links inside fenced code blocks are not picked up. The walk is
      the whole implementation: collect `Node::Text`, skip `Node::Code` and `Node::InlineCode`, and
      match the link syntax within each text node's value. Verified 2026-08-31 on the pinned crate —
      a paragraph's `[[uuid|label]]` arrives as a *single* `Text` node with its byte span intact, so
      no reassembly across events is needed. (`pulldown-cmark` splits the same link across eight
      events; that is why it was not chosen.)
- [ ] Inline code is excluded along with fenced blocks. `` `[[uuid]]` `` is a person writing *about*
      a link, not making one. This is a decision, not a side effect of the walk — say so in a test.
- [ ] Keep each link's byte offset from the extraction. Stage 5's reader wants to highlight a link in
      place, and recovering the offset later means re-parsing.
- [ ] Populate `links`; expose `backlinks(id)`.
- [ ] **Workspace-scoped only** — a target id absent from this workspace resolves to `Deleted`, and
      never reaches into another workspace. Independence is the point of a workspace.
- [ ] `quoted_by(id)` — the inverse of `quoted_id`, presented alongside backlinks.

## Work

- [ ] `Draft { body, title, reply_to, quote }` → `create`. Reject a `reply_to` that does not resolve
      to a `Present` or `Trashed` note; permit replying to a trashed note (it is still a real note).
- [ ] `Edit { body, title }` → `edit`. `edited_at` is not written — it is mtime, so it moves only
      when the file does, which is exactly "only when content actually changed" for free.
- [ ] `trash` / `restore` / `purge`, each a filesystem move plus a single index update, in that order,
      so an interruption leaves the index stale rather than the vault wrong. Stale is recoverable by
      `sync()`; wrong is not.
- [ ] `Ref` resolution, batched — resolving parents one query per note in a list view is the obvious
      performance trap here.
- [ ] `thread(id)` assembling ancestors and tree in one round trip.
- [ ] Reply and branch counts for the timeline, from one grouped query, not N.
- [ ] `resolve(prefix)` returning `Unique | Ambiguous(Vec<NoteMeta>) | None`.

## Acceptance

- Create a root, three replies, and a fork; `segments()` and `paths()` match the worked example above.
- Trash the middle note of a chain: children stay live, the parent reference reports `Trashed`.
- Purge that note instead: children stay live and the reference reports `Deleted`. The subtree is
  **no longer grouped** — each survivor now roots at the missing id its `relation:reply_to` names,
  which is what makes the broken chain visible without a stored root.
- Purge the root itself: the surviving children appear in the timeline as orphan roots.
- A hand-written cycle in `reply_to` is reported as `Problem::ReplyCycle` — not an error — the note
  in it roots at itself and appears in the timeline, and there is no hang.
- A body containing the same `[[uuid]]` in prose, in a fenced code block, and in inline code yields
  exactly one link — from the prose.
- A link to a purged note still extracts, and resolves to `Deleted`; extraction never consults the
  index.
- Property tests pass over several thousand generated trees.
- Killing the process between the file move and the index write leaves the vault correct and the
  index repaired by the next `sync()`.

## Risks

- **The N+1 in list rendering.** Every timeline row wants its parent's state and its reply count.
  Batch both, and add a test that asserts the query count for a 50-row page is constant.
- **Cycles and self-reference** come from hand-edited files, not from the app. They are a normal input.
- ~~**`edited_at` churn.**~~ **Largely settled by stage 1b**, which made `edited_at` mtime rather
  than a written field: a no-op save that writes identical bytes still touches mtime, so the guard
  moves from "do not bump the field" to "do not write the file when the bytes are unchanged" —
  which `Workspace::open_note` already does. The original concern, kept for the shape of it:
  bumping it on a no-op save makes every note look recently touched and
  poisons the "recently edited" sort in stage 5. Compare content before writing.

## Findings from the build

### `relation:root` on a reply to a note whose own root is missing

`create` copies the parent's `relation:root`. A parent that is itself missing that key — hand-edited,
or written by something else — leaves nothing to copy. The rule implemented: **fall back to the
parent's own id.** It keeps the reply grouped with the only ancestor actually known, and it never
invents a root by walking, because walking is `open_note`'s repair job and doing it here would make
`create` a write to a second file.

### The reply-to-a-trashed-note case is load-bearing

`create` rejects a `reply_to` that resolves to nothing and **permits** one that resolves to a
trashed note, exactly as this document specifies. Worth keeping the reason visible: trash never
cascades, so a trashed note is still a real note with live replies underneath it, and refusing to
reply to one would make the trash a place threads go to die. `Error::ReplyTargetMissing` is
therefore about absence only, and is a separate variant from `Error::NoteNotFound` because the
subject differs — the note you asked for is fine, it is the parent that is gone.
