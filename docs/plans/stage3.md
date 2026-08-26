# Stage 3 — Notes and threads

**Goal.** The complete domain: notes are created, edited, trashed, restored, purged; threads project
into both of the forms settled in `docs/conversation.md`; links resolve within the workspace.

**Why now.** This is the app's actual logic, and it is the last stage with no user interface to hide
behind. Every rule proved here by a test is a rule three surfaces get for free. Every rule left
implicit here gets reimplemented three times, slightly differently.

**Not in this stage.** Anything a person can type at. Still library-only.

## Note lifecycle

| Operation | Filesystem | Index | Frontmatter |
| --- | --- | --- | --- |
| `create` | write `<uuid>.md` in root | insert | `id`, `created_at`, `root`, and any relation |
| `edit` | rewrite in place | update | `edited_at` bumped |
| `trash` | move to `.jot/.trash/` | `state = 'trashed'` | `trashed_at` stamped |
| `restore` | move back to root | `state = 'active'` | `trashed_at` removed |
| `purge` | delete the file | delete the row | — |

Rules that follow from the locked decisions and must be enforced in one place:

- **Trash never cascades.** Trashing a note with replies moves exactly one file. The replies stay
  live and render a trashed-parent placeholder — your call, and it is also the only behavior that
  survives a rebuild without extra bookkeeping.
- **`root` is assigned once, at creation**, copied from the parent (or set to the note's own id for a
  root). It is never recomputed, so purging a middle note leaves the subtree grouped.
- **Re-parenting is not supported.** Nothing in the design needs it, and it would be the one operation
  requiring a subtree rewrite. If it is ever wanted, it arrives as an explicit `reparent` that
  rewrites `root` across the subtree — not as a side effect of an edit.
- **A quote is not a thread edge.** `quote` never touches `root_id`, and the quoted note never joins
  the quoting note's tree.
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

Using the example from `docs/conversation.md` — edges `A→B`, `B→C`, `C→E`, `C→D`, `A→F`:

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

- [ ] Extract `[[<uuid>]]` and `[[<uuid>|label]]` from the body during scan, using a markdown parser
      rather than a regex over raw text, so links inside fenced code blocks are not picked up.
- [ ] Populate `links`; expose `backlinks(id)`.
- [ ] **Workspace-scoped only** — a target id absent from this workspace resolves to `Deleted`, and
      never reaches into another workspace. Independence is the point of a workspace.
- [ ] `quoted_by(id)` — the inverse of `quoted_id`, presented alongside backlinks.

## Work

- [ ] `Draft { body, title, reply_to, quote }` → `create`. Reject a `reply_to` that does not resolve
      to a `Present` or `Trashed` note; permit replying to a trashed note (it is still a real note).
- [ ] `Edit { body, title }` → `edit`, bumping `edited_at` only when content actually changed.
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
- Purge that note instead: children stay live, the reference reports `Deleted`, and the subtree is
  still grouped under the original `root_id`.
- Purge the root itself: the surviving children appear in the timeline as orphan roots.
- A hand-written cycle in `reply_to` produces an error naming both notes, and no hang.
- Property tests pass over several thousand generated trees.
- Killing the process between the file move and the index write leaves the vault correct and the
  index repaired by the next `sync()`.

## Risks

- **The N+1 in list rendering.** Every timeline row wants its parent's state and its reply count.
  Batch both, and add a test that asserts the query count for a 50-row page is constant.
- **Cycles and self-reference** come from hand-edited files, not from the app. They are a normal input.
- **`edited_at` churn.** Bumping it on a no-op save makes every note look recently touched and
  poisons the "recently edited" sort in stage 5. Compare content before writing.
