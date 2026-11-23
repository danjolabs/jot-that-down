# Conversation

## session_01RRzBB8WzRPJsGCeK7XWa42

**Date:** 2026-09-09
**Topic:** Why the GUI left this repository — the cross-tool ecosystem, the `data`/`file` storage
split, and the inbox as the single front door
**Branch:** `prototype`, after stage 5 sealed at `0.0.5`

-- Claude

> **This is history, not spec.** A structured record, not a transcript. It spans three repositories
> and binds none of them. Most of it is proposal; the parts the user actually decided are marked as
> such and collected at the end. Claims about the other two repos are dated observations of their
> working trees on 2026-09-09 — re-check before relying on one.

### What prompted it

`docs/plans/todo/desktop.md` records that the desktop app left the numbered stages, and
`overview.md` gives the reason as sequencing: after five stages, refining two surfaces beat adding a
third. That was true but incomplete. The actual reason was never written down: the user does not
want a GUI *in this repository* because they want one shell across several small productivity
tools, and jot's desktop surface is meant to be a citizen of it rather than its own application.

The two other tools:

- `../chronocane/` — Tauri v2 + SolidJS tray-resident day planner, `com.danjolabs.chronocane`.
- `../quite-task-tracker/` — pnpm monorepo: axum + sqlx/Postgres API, React 19 web, Expo mobile.

---

## 1. The three projects, as found

| | Store | Language | Surface |
| --- | --- | --- | --- |
| jot-that-down | markdown files + derived SQLite | Rust | CLI, TUI |
| chronocane | `@tauri-apps/plugin-store`, local | Rust + Solid | tray popup |
| quite-task-tracker | PostgreSQL | Rust + React/Expo | web, mobile |

Details that mattered to the discussion:

- **jot already has a machine interface.** `--json` is a global flag (`crates/jot-cli/src/main.rs:98`)
  and every read command honours it; the shape is documented in `docs/cli-json.md`.
- **chronocane's schedulers are the valuable part.** `src-tauri/src/pomodoro.rs` and
  `hourly_chime.rs` run in Rust deliberately, so notifications fire when both windows are hidden and
  webview timer throttling would otherwise eat them. Its day view already renders *mock* calendar
  events (`src/lib/mock-events.ts`) — a seam cut in advance.
- **qtt's `desktop/` is an empty slot.** The directory holds only `node_modules/`, and
  `pnpm-workspace.yaml` has `# - "desktop"` commented out.
- **qtt's schema is mostly unwritten.** `api/migrations/20260704062645_init.sql:31-33` leaves
  `app.tasks`, `app.events` and `app.personas` commented out. `app.inbox` is the only implemented
  application table.
- **Both jot and qtt use UUIDv7.** `app.inbox.id UUID PRIMARY KEY DEFAULT uuidv7()`; jot's `NoteId`
  is v7 and `created_at` is *decoded from it* rather than stored.

## 2. The overlap finding

The three tools already implement the same three concepts twice each. A shell that does not resolve
this yields two of each behind one window frame.

| Concept | Implemented in | |
| --- | --- | --- |
| Unstructured capture | jot's timeline | qtt's `app.inbox` |
| Recurrence | chronocane weekly time blocks | qtt `RRULE` Routines/Schedules |
| Timers and notifications | chronocane only | nothing else can reach them |

Deciding ownership of these three is the architecture work. It is cheaper than any shell and it does
not depend on one existing.

## 3. The model the user settled on

Stated by the user, and it is the spine of everything below:

> Everything I want to capture falls into two categories — `data` in a database, or `file` in a
> filesystem. Everything goes to the inbox as data first, and that is the starting point, so the
> desktop app should work around it.

So:

- **Two stores, not three tools.** `data` = qtt/Postgres. `file` = jot/markdown.
- **One front door.** Everything lands in `app.inbox`.
- **One primitive.** Promotion: inbox item → task / routine / appointment / schedule / note.
- **chronocane is an add-on.** Once its time blocks become qtt `events` it holds no store of its
  own. Pomodoro and the o'clock chime are peripheral features, not a third pillar.

### What this retired

An earlier proposal in this session — a `dl-kit` crate with a `Provider` trait, a `Card` type, a
capability registry, and per-tool providers behind dynamic dispatch — was **withdrawn**. With a
single front door there are not three peers to abstract over; the abstraction was solving a problem
the user's model deletes. Recorded here only so it is not re-derived.

What survived from it: the `dl://<tool>/<kind>/<id>` URI form, and the argument for the shell
registering **one** OS scheme and routing internally rather than one scheme per tool (jot keeps
`jot://` as a standalone alias). `todo/desktop.md` currently plans the per-tool form.

### Where the desktop app lives

Proposed: **qtt's `desktop/` package**, not a fourth repository and not `apps/desktop/` here. A
Tauri v2 app whose `src-tauri` links `jot-core` directly and speaks HTTP to the axum API. That fills
a slot that already exists, and it keeps jot's stage discipline and `0.0.<stage>-<letter>` git-hook
versioning out of a monorepo that would break both.

## 4. Promotion, as proposed

`app.inbox` is already the right shape: `title VARCHAR(255) NOT NULL`, `description TEXT NULL`. It
maps onto a jot note with nothing left over — title to frontmatter, description to body.

Targets follow qtt's own README table, two axes plus an escape hatch:

| | no recurrence | `RRULE` |
| --- | --- | --- |
| `tasks` (deadline) | Task | Routine |
| `events` (duration) | Appointment | Schedule |

...plus optional project, plus persona, plus **keep as file**. One dialog: persona → shape →
optional project/date → confirm.

Two calls worth making while `tasks`/`events`/`personas` are still commented out:

- **The item survives promotion.** A `state` column (`open | promoted | discarded`) and a join table,
  so the inbox is a log rather than a queue that eats things. Default view filters to `open`.
- **One item may promote more than once.** "Meet Bob about the design" is plausibly an appointment
  *and* a note. A join table handles that; a nullable column does not.

```sql
CREATE TABLE app.inbox_promotions (
  inbox_id       UUID NOT NULL,        -- deliberately no FK; see below
  target_uri     TEXT NOT NULL,        -- dl://qtt/task/… | dl://jot/note/…
  title_snapshot TEXT NOT NULL,        -- so data-only surfaces can render a label
  date_created   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  PRIMARY KEY (inbox_id, target_uri)
);
```

**A cross-store reference can never be a foreign key**, because jot is not in Postgres. So jot's
"no cascading anything, dangling references are a designed state" rule is not being imported by
choice — it is forced the moment a row points at a file. Delete a note on disk and the promotion row
dangles, correctly.

`title_snapshot` exists because web and mobile will see `dl://jot/note/X` and can never resolve it.
It goes stale if the note is retitled; acceptable, since desktop resolves live and this is a label,
not truth.

## 5. Reusing the inbox UUID

Both stores are UUIDv7, and jot decodes `created_at` from the id rather than storing it. So if
promotion-to-note reuses `inbox.id` as the note's UUID:

- the note's `created_at` becomes **the moment of capture**, not the moment of triage — free, and the
  correct semantic;
- `dl://qtt/inbox/X` and `dl://jot/note/X` share `X`, so provenance needs no lookup;
- the same holds for data promotions.

Caveats: reuse the id only for the *first* promotion of a given kind, or two promotions to the same
kind collide. `X` living in two stores is disambiguated by the URI's tool and kind segments.

**The jot-core change this needs:** `Draft` (`crates/jot-core/src/query.rs:139`) has no `id` field —
`Workspace::create` generates it. A caller-supplied id would be a small change that touches identity,
so it is a real decision rather than a patch.

**A zero-change alternative that works today:** put `origin: dl://qtt/inbox/<uuid>` in `Draft.extra`.
jot preserves unrecognized frontmatter keys verbatim by design, so provenance rides along for
nothing — you only lose the capture-time property.

## 6. The asymmetry that justifies a desktop app

- **Promotion to data is server-side.** `POST /inbox/:id/promote` — any surface can do it.
- **Promotion to file is client-side.** The server cannot write your filesystem. Only a client with
  `jot-core` linked in can.

Therefore **web and mobile can capture and can promote to task/event/routine, but cannot promote to
a note.** Worth stating in the plan rather than discovering it as a greyed-out button on mobile.

## 7. Dropping browser-side local-first

**User's decision:** `data` stays in the database rather than local; `file` is what lives locally.
Reason given: OPFS-backed browser SQLite is not widely supported cross-device and needs an HTTPS
setup.

Agreed, and there is a stronger reason than the stated one. `web/src/routes/(app)/inbox/index.tsx`
renders **two tabs, Local and Cloud** — `localItemsCollection` on OPFS beside `TabContentCloud` on
the server. That is two front doors, which contradicts the model in §3: capture is the one place
nothing should have to be decided, and a tab is a decision made before the thought is finished.
Dropping local storage collapses the tabs, and that is the real win. The two had drifted anyway —
`LocalItem` carries `completed` and a lexicographic `order`; `app.inbox` carries neither.

The stated reason is also worse than it looks: `-collections/index.ts` calls
`openBrowserWASQLiteOPFSDatabase` at module top level with top-level `await`, so on a non-secure
context or a browser without OPFS SyncAccessHandle the **whole inbox route** fails to load, not just
the Local tab.

Three qualifications:

- **Scope the constraint.** OPFS and secure-context are *browser* limits. They do not apply to Tauri
  desktop (real filesystem, rusqlite) or Expo (expo-sqlite). "Data lives in the database" is right as
  a storage model; it should not harden into "no client may buffer a write."
- **Drop the adapter, not the collection layer.** `@tanstack/browser-db-sqlite-persistence` and
  `@tanstack/db` are separate dependencies. `createCollection` and `useLiveQuery` still give
  optimistic mutations and live queries against the server; only the persistence adapter and
  `migrateNumericOrders` need to go.
- **One live inconsistency this exposes.** `app.inbox.sort_order` is `INT`, but the client already
  migrated to `fractional-indexing` string ranks. If ordering moves server-side, that column wants
  to become `TEXT` before there are rows worth preserving.

### The resulting availability split

| | reach | network |
| --- | --- | --- |
| `data` | every device | required |
| `file` | one machine | not required |

Desktop is the only place both are live — which sharpens §6: promotion-to-note is not "desktop
only", it is "*that* desktop only". Multi-machine `file` is git or Syncthing, and jot was already
built for that: `.jot/index.db*` is gitignored and the index is disposable by design.

### The one carve-out still argued for

Capture over the network is fine except at the moment the network is absent, which is the moment
capture matters most. Under this decision that is no longer a local-first project — it is a **write
buffer**, and it is small:

- the client generates the UUIDv7 (wanted anyway, per §5), so `POST /inbox` upserts on the primary
  key: retry is idempotent by construction, no dedup logic;
- an append-only JSONL file in `src-tauri` suffices — capture volume is a few lines a day;
- the same API change serves both: `app.inbox.id` must accept a client-supplied id, not only
  `DEFAULT uuidv7()`.

Nothing to do now; web cannot buffer regardless. It lands when `desktop/` does.

## 8. What this does to this repository's docs

Nothing below has been changed. All of it is an ask.

- **`overview.md`'s opening sentence** — *"A personal capture tool over a directory of markdown
  files"* — stops being true of the ecosystem. jot becomes the **file store**, the place things are
  promoted *to*. Standalone `jot new` remains valid; it is no longer the front door.
- **The risk table's *"the premise is capture, not curation"*** inverts under §3. If the inbox is
  capture, curation is precisely jot's job. This is a stated premise, so it needs a decision rather
  than a quiet rewrite.
- **`todo/desktop.md` is superseded, not deferred.** Its capture overlay writes notes; the one in §3
  writes inbox items. Its app lives in qtt's `desktop/`, not `apps/desktop/`. Its
  *"Not in this plan: … plugins"* line is now wrong. The seam and `jot_core::watch` still pay off —
  the consumer moved repositories.
- **The framework question** (`overview.md`, open questions) can close: standardize on React. qtt web
  already has shadcn; chronocane is seven components in `src/components/`, which is cheaper to
  rewrite than any iframe bridge is to maintain.

---

## Decisions actually taken

By the user, in this session:

1. The GUI is deliberately out of this repository; the reason is the shared shell, not sequencing.
2. Two storage categories: `data` in a database, `file` in a filesystem.
3. `app.inbox` is the single front door. Everything arrives as data, then may be promoted.
4. Promotion targets: task (standalone or in a project), event, routine — or a markdown file in jot.
5. Pomodoro and the hourly chime are add-on features, not a pillar.
6. Browser-side local-first is dropped: `data` stays server-side, `file` stays local.

No code changed in any repository. Everything in §4–§8 is proposal or finding.

## What should move into a plan doc

1. **The `overview.md` premise question (§8).** The most consequential item here and the only one
   touching a stated premise. Belongs in an explicit ask.
2. **`todo/desktop.md` rewritten as superseded**, pointing at qtt's `desktop/`, with the capture
   overlay retargeted at the inbox.
3. **The promotion contract (§4–§5)** — join table, item survival, id reuse, `title_snapshot`. This
   belongs in qtt, not here, and is worth writing before `app.tasks` and `app.events` are created.
4. **`Draft` gaining a caller-supplied id (§5)**, or the explicit decision to use `Draft.extra`
   instead.

## Open questions

- Does the desktop app absorb chronocane's tray, or do both sit there? The timers must outlive a
  closed window either way.
- Is promotion forward-only? A note that later deserves to be a task has no path back.
- `app.inbox` carries `user_id`, not `persona_id` — persona is assigned at promotion. Deliberate and
  probably correct (the inbox is pre-structure), but it means capture can never be persona-tagged
  even when the persona is known.
- Does chronocane cache qtt `events` locally, or does the day view go blank offline?
- `sort_order INT` versus a lexicographic `TEXT` rank (§7).
