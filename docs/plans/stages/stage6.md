# Stage 6 — Refactor and polish

**Goal.** Consolidation over what stages 1–5 built. Two surfaces exist and one person has been
using them daily; this stage is what that use turned up. No new surface, no new subsystem — the CLI
and the TUI, refined against the way they actually get used.

**Why now.** The desktop app [left this slot](../todo/desktop.md) because after five stages the
thing worth doing next is making two surfaces good rather than adding a third. That argument is
only worth anything if the refinement is driven by real use rather than by taste, so this document
is a **dogfooding log first and a plan second**: every item below starts as something that went
wrong, or grated, while using `jot` for its actual purpose.

**Not in this stage.** The desktop app (`../todo/desktop.md`). The schema tail — enums, per-key
defaults, field filters — which is [stage 7](stage7.md).

**Thread detail is in**, which is the one place this stage adds rather than refines. Stage 5
specified the view and did not build it, and dogfooding produced not a complaint about it but a
*shape* for it — see the last item under TUI improvements. A stage about refinement is the right
place for a view whose design finally arrived; a stage about refinement is the wrong place to
discover that design mid-implementation, so the open questions on that item close before it is
dispatched.

**This document is still being filled in.** Items arrive as they are found. An empty bucket means
nothing has been reported there yet, not that the surface is clean.

## How the buckets work

Two axes: the surface it happens on, and whether it is a bug or an improvement.

- A **bug** is the surface not doing what it already promises — in a stage doc, in `--help`, in the
  `?` overlay, or in a message it prints itself. It is fixable without deciding anything new.
- An **improvement** is the surface doing what it promised, where the promise was wrong. It needs a
  decision before it needs code, and if it touches a locked decision in `../overview.md` it stops
  and asks.

Each entry carries **Observed**, **Expected**, **Repro**, and **Where it lives**. `Where it lives`
is a starting point read off the source, not a verified diagnosis — confirm it before changing it.

---

## CLI — bugs

*Nothing reported yet.*

---

## CLI — improvements

### `jot ls` should print the whole millisecond timestamp in the id column

- **Observed.** `jot ls` abbreviates ids to a floor of **8** characters
  (`crates/jot-cli/src/output.rs:27`), while the TUI floors at **13** (`crates/jot-tui/src/app.rs:109`).
  So the same note is `01a05a57` in the CLI and `01a05a57bcd12` in the browser, and the CLI's
  spelling is the one that carries no information: a UUIDv7's leading 48 bits are a millisecond
  timestamp, so eight hex characters are a *shared* value across roughly a minute of captures and
  randomness does not start until character 13. `shortid.rs`'s own module docs open on this exact
  failure, and the uniqueness rule then pushes the width up anyway the moment two notes land in the
  same minute — which is most of them.
- **Expected.** The CLI prints ids at the same 13-character floor the TUI uses, so an id read in
  one surface is recognisable in the other, and every printed id carries its capture time rather
  than a prefix shared with its neighbours.
- **Repro.** `jot ls` against a vault with several notes captured in one sitting; compare the id
  column with `jot tui`'s.
- **Where it lives.** `crates/jot-cli/src/output.rs` (`MIN_ID_WIDTH`). One constant, but it changes
  the width of every human-readable listing, so the snapshot expectations in `crates/jot-cli` move
  with it. `shortid::MIN_WIDTH` stays 8: it is the registry's floor too, and workspace ids are a
  much smaller set.
- **Settled 2026-09-09, at the user's direction: reading (a).** Widen the id to the full timestamp
  prefix — one constant, no new column. The alternative, decoding the timestamp into a rendered
  creation-time column, is a different change and overlaps the relative age the row already ends
  with.

### `jot --help` should group its commands rather than list fifteen of them flat

- **Observed.** `jot --help` prints one undifferentiated `Commands:` block of fifteen entries, in
  declaration order: `new`, `list`, `show`, `thread`, `edit`, `remove`, `restore`, `purge`, `trash`,
  `search`, `links`, `workspace`, `index`, `tui`, `completions`, `help`. Nothing on the page says
  that four of those are the trash lifecycle, that two of them are about the vault rather than about
  notes, or that `completions` is not something you run twice. The list is the first thing a new
  reader sees and the thing the author scans daily, and it is sorted by an order neither of them
  cares about.
- **Expected.** Commands appear under headings that match the way they are reached for — capture,
  read, trash, vault, browse — so the eye lands on a group of two or three rather than scanning
  fifteen lines for the one it wants.
- **Where it lives.** `crates/jot-cli/src/main.rs` (`enum Command`), and whatever renders the list.
- **The mechanism matters, and clap does not do this for you.** `Command::subcommand_help_heading`
  (clap 4.6) renames the *single* heading; it does not partition the list. The two ways to get
  groups are:
  - a custom `help_template` with the commands block written out by hand — which is the drift that
    produced the undo bug, one step removed: the list would be maintained separately from the enum
    it describes, and nothing would fail when a command is added.
  - **generate the grouped list from the `clap::Command` at runtime**, with a group table this
    crate owns mapping command name → heading, the real list hidden. One source of truth for the
    commands (the derive), one for the grouping (the table), and a test asserting every subcommand
    the derive produces appears in exactly one group — so a new command breaks a test rather than
    quietly falling off the help page.

  The second is the one that matches how `?` works in the TUI, and the reason to prefer it is the
  same reason.
- **Not this.** Making the grouping real by nesting — `jot trash rm`, `jot note new` — buys tidier
  help at the cost of every keystroke the author has already learned. `jot ws` and `jot index` nest
  because they are genuinely about a different noun; `new` and `ls` are the whole point of the tool
  and stay at the top level.
- **Open.** The groups themselves. A first cut: **capture** (`new`, `edit`), **read** (`list`,
  `show`, `thread`, `search`, `links`), **trash** (`remove`, `restore`, `purge`, `trash`),
  **browse** (`tui`), **vault** (`workspace`, `index`), **shell** (`completions`). Whether `edit`
  is capture or read is arguable; whether `tui` deserves a heading of its own, less so.

---

### `jot` should have a configuration of its own

Everything adjustable today is either a flag typed every time, an environment variable, or a
constant compiled in. Several items in this stage — the id width, the calendar's shape, whether the
reader opens by default — are decisions that only one person has an opinion about, and hardcoding
each one is how a tool accumulates a settings page nobody can find.

**There is already a place for it, and on Linux it is the place you would have chosen.**
`registry::default_path()` resolves `directories::ProjectDirs::from("", "danjolabs", "jot")
.config_dir()` and joins `workspaces.toml` (`crates/jot-core/src/registry.rs:141`). Confirmed on the
machine this is being written on: **`~/.config/jot/workspaces.toml`**, 492 bytes, written 2026-09-02.
A `config.toml` sits beside it and needs no new decision about where things live.

The three platforms differ, read off the pinned `directories` 6.0.0:

| platform | directory | why |
| --- | --- | --- |
| Linux | `~/.config/jot/` | `lin.rs:92` uses the application name alone; `$XDG_CONFIG_HOME` wins if set |
| macOS | `~/Library/Application Support/danjolabs.jot/` | `mac.rs:90` joins the triple into one bundle-id component |
| Windows | `%APPDATA%\danjolabs\jot\config\` | `win.rs:98` keeps organization and application as two components |

**Settled 2026-09-09, at the user's direction: keep the platform-native paths.** `directories`
stays, and the three rows above are the answer on their respective platforms rather than something
to be normalised away. On the machine this is dogfooded on that is `~/.config/jot/`, which is what
was wanted; the other two are what those platforms expect, and overriding them would be this tool
inventing a convention for platforms it is not being used on.

**A test in that module is wrong on macOS, and this item is where it gets fixed.**
`default_path_ends_in_workspaces_toml` (`registry.rs:641`) asserts that some component of the path
equals `"jot"`. On macOS the bundle id is a *single* component, `danjolabs.jot`, so no component
equals `"jot"` and none equals `"danjolabs"` either — both that assertion and the `cfg!(macos)`
branch below it fail there. The test passes on Linux and Windows. `default_path()` itself is correct
on all three; only the shape assertion is wrong. Its own comment records the same class of mistake
being caught on Linux and gated with a `cfg!`, which is what makes leaving the macOS shape unchecked
worth writing down rather than quietly patching. It is also **not a CLI or TUI item** — it is
`jot-core`, and the buckets in this document have no row for that. One item does not justify adding
one.

**The boundary that keeps this from becoming a second schema.** Two files, and the line between
them is not "user versus project" — it is **what the setting can change**:

- `workspace.toml` is per-vault and governs **what gets written into note files**: the frontmatter
  schema, the roles, the keys. It travels with the notes because the notes cannot be read without
  it.
- `config.toml` is per-user and governs **how things are shown to you**. It must never change a
  byte in a note file. A key that would is in the wrong file.

Apply that test to every proposed key and most of the arguments answer themselves.

**What it plausibly holds**, drawn from this stage's own items rather than invented: the id width
(the `jot ls` entry above), the default sort, whether the reader and sidebar start open, the
calendar's dot style and row height, the highlighter command — currently a hardcoded `bat` →
`batcat` → `cat` fallback chain in `crates/jot-tui/src/preview.rs` — and an editor, which today can
only be `$VISUAL` / `$EDITOR` (`crates/jot-cli/src/editor.rs:131`).

**Rules it inherits, and one invariant it breaks.**

- **Precedence is flag, then environment, then file, then default.** The existing chain for choosing
  a workspace is `--workspace` → `JOT_WORKSPACE` → discovery → the registry's current
  (`crates/jot-cli/src/context.rs:70-79`), and an explicit choice that fails is an error rather than
  a fallback. A config file slots *below* the environment and must not disturb that rule.
- **Do not add a second way to say the same thing.** A `default_workspace` key would duplicate the
  registry's current workspace, which `jot ws use` already sets. Two mechanisms for one fact is how
  they drift apart.
- **A broken config warns and falls back; it never refuses.** The registry recovers a corrupt file
  to an empty one deliberately — a bad list of workspaces must not stop you capturing a note — and
  the same reasoning applies here, with the same `recovered()`-style signal so the fallback is
  visible rather than silent.
- **Unknown keys are preserved verbatim**, as everywhere else in this app.
- **It breaks a stated invariant, and that has to be handled deliberately.**
  `registry::default_path`'s doc comment says it is the *only* function in `jot-core` that calls
  into `directories`, and that isolation is what keeps tests off the real config directory. A second
  such function would quietly falsify a written rule. Give both the registry and the config one
  owner for OS paths, and give the config the `JOT_REGISTRY`-style environment override
  (`context.rs:34`) that makes it testable.

**Not in this stage: remappable keys.** A config file invites them, and the TUI's keymap is a single
table from which `?` and the footer are both generated — that is the property that makes an
undocumented binding unrepresentable, and it is the property the undo bug above shows the cost of
losing. User remaps mean help generated from a *merged* map, and every message that names a key
resolved through it. Worth doing one day; not while the same stage is fixing a key that was spelled
twice.

**Open.**

- Does this ship with a `jot config` command — `path`, `show`, `edit` — or only a file? A command
  is discoverable and puts a new entry in the grouped `--help` above; a file alone is less to build
  and less to keep in step.
- Whether `config.toml` may set anything per-workspace, or is strictly global. Per-workspace
  overrides are the obvious next request and the obvious way to end up with three files.

---

---

## TUI — bugs

### Undo is unreachable: every message on screen advertises a key that is not bound

- **Observed.** `Space x` trashes the note and toasts ``trashed `…` — U to undo``. Pressing `U`
  does nothing at all — not even an error — because `U` is unbound in normal mode; the binding is
  `Space U` (`crates/jot-tui/src/key.rs:482`, behind the prefix with every other write). The toast
  is not the only place: trashing something already trashed says `already in the trash — U restores
  it`, with the same missing prefix.
- **Compounding it, the footer hides the key where it is needed.** The `Space U` binding is scoped
  `Scope::Trash` (`key.rs:355-362`), so the hint appears only in the trash view — while the trash
  *happens* in the timeline, which is where the undo offer is made and where the footer stays
  silent about it. `key.rs:763` pins that behaviour as intended, so this is a decision to revisit,
  not just a typo.
- **Expected.** Whatever key undoes the trash is spelled the same way in the toast, in the footer,
  and in `?`, and is offered in the view where the offer stands. A recovery key you have to guess
  is not a recovery key.
- **Repro.** `jot tui` → `Space x` on any note → press `U`. Nothing happens. Press `Space U` and it
  restores.
- **Where it lives.** `crates/jot-tui/src/app.rs:458` and `:449` (the two toast strings),
  `crates/jot-tui/src/key.rs` (the `Scope` on the undo binding).
- **Note for the fix.** `PREFIX_LABEL` already exists in `key.rs`. A message that names a key
  should be built from the keymap rather than spelling the key again in a format string — that is
  what let these two drift apart, and the same class of drift is what `?` is generated from the
  table to avoid.

### The trash view has no purge, and no restore either

- **Observed.** There is no purge in the TUI. `Action` has no `Purge` and no `Restore` variant, no
  key resolves to one, and `?` lists neither; the only way out of the trash is `Space U`, which
  restores **the note this session last trashed** rather than the note under the cursor. So a note
  trashed yesterday cannot be restored from the browser at all, and nothing can be purged from it.
- **Expected.** Stage 5 specifies "trash lists trashed notes with restore and purge; purge
  confirms" (`stage5.md`, *Search and trash*). Both operations exist in core (`Workspace::restore`,
  `Workspace::purge`) and both are already exposed by the CLI (`jot restore`, `jot purge`, which
  confirms — `crates/jot-cli/src/main.rs:801-826`). The TUI is the only surface missing them.
- **Repro.** `jot tui` → `Tab` to trash → `?`. Neither operation is listed; no key performs either.
- **Where it lives.** `crates/jot-tui/src/key.rs` (`Action`, the binding table, `resolve_prefixed`),
  `crates/jot-tui/src/app.rs` (`dispatch`, and the confirmation state purge needs).
- **Decisions the fix has to take.**
  - **Restore-under-cursor is not undo.** Undo is "put back what I just did" and is
    session-scoped; restore is "take this row out of the trash" and works on anything in the list.
    Both should exist, scoped to where each makes sense, and they must not share a key.
  - **Purge confirms.** It is the one irreversible operation in the app, and the CLI already
    confirms. The TUI needs a confirmation the keymap can represent — a modal state, not a second
    prefix — and it must not be dismissible by the same key that armed it.

### "parent is hidden" says a key will help when it cannot

- **Observed.** Pressing `u` on a row whose parent is not in the list toasts `parent is hidden — f
  shows every note`. In the one state that message can actually appear, it is false: `f` will not
  show the parent, because the parent does not exist.
- **Why that is the only state.** The message is guarded by "timeline, roots-only", and roots-only
  filters on `Row::is_root`, which is `!parent.is_some_and(Ref::exists)`
  (`crates/jot-core/src/query.rs:378`); `Ref::exists` is true for both `Present` and `Trashed`
  (`:96`). So a row listed in roots-only whose `reply_to` is `Some` must have a parent that is
  **`Deleted`** — purged. A present parent would have made the row a non-root and kept it off the
  list; a trashed one likewise. Flat mode lists more notes; it cannot list one that was purged.
- **Expected.** The message says what is actually true — the parent was purged and is not coming
  back — which is a thing stage 2 already has a vocabulary for: `Ref::Deleted`, "the id is all that
  remains".
- **Repro.** Purge a note that has a reply, then press `u` on the reply in the roots-only timeline.
- **Where it lives.** `crates/jot-tui/src/app.rs`, `up_to_parent`.
- **Found by** the implementer fixing the undo message, while looking for a way to test that
  branch — it could not reach the state the sentence describes, because there isn't one. Same family
  as the bug above: a message asserting something the surface does not check. The key in it is now
  taken from the binding table, so what is left is the *sentence*, not the spelling.

---

---

## TUI — improvements

### Nothing on screen says how many views there are, or which one this is

- **Observed.** The surface has three views and two of them have modes, and none of that is stated
  anywhere except in the `?` overlay:
  - **timeline** — every note (default), or thread heads only (toggled with `f`)
  - **files** — sorted by title, created, or edited (cycled with `s`)
  - **trash**

  `Tab` cycles between them, so the way to find out how many there are is to press it until
  something looks familiar, and the way to find out which mode a view is in is to remember what you
  last pressed. The footer offers the keys but never says what the current state *is*.
- **Expected.** The set of views is visible, the current one is marked, and the current view's mode
  is shown beside it — so "where am I, what else is there, and what is this list actually showing"
  are all answerable without pressing anything.
- **Why it is an improvement and not a bug.** The surface does exactly what stage 5 specified. The
  specification assumed a reader who had just read the key table, which is true once and false every
  day after.
- **Where it lives.** `crates/jot-tui/src/ui.rs`, and whatever holds the current mode in
  `crates/jot-tui/src/app.rs` (`view`, `flat`, `sort` are all there already — this reads them, it
  does not add state).
- **Constraints.**
  - The mode belongs *with* the view it qualifies, not in a separate status field. `files` sorted by
    title and `timeline` showing roots-only are the same kind of fact and should read the same way.
  - It must not become a second way to change view. This is an indicator; `Tab`, `f` and `s` remain
    the way things change. (Whether that stays true once the sidebar exists is the focus question
    recorded under the layout item.)
  - Whatever draws it is generated from the same enumeration the cycle uses, so a fourth view cannot
    be added without appearing here — the rule the keymap already follows for `?`.
- **Revised 2026-09-28, and the item shrank.** The list panel's block title already reads
  `timeline — every note` — that is stage 5's, not new — so *"which view is this, and what mode is it
  in"* is **already answered**. What is missing is only the other half: nothing says what else
  exists. So this is an edit to a title that is already there, not a strip to be designed, and it
  should stay in the title rather than becoming a second place to look.
- **Open.** How to name the other views without spending the title's width on them, given the title
  already carries view and mode and the panel can be 40 columns wide.

---

### The lists should be a table — id, title, created, edited — with the reader on a toggle

- **Observed.** A timeline row is a two-slot marker, the id, the title, and a meta column that
  degrades from `replies · quotes · age` down to the age alone when the width runs out
  (`crates/jot-tui/src/ui.rs:304`, `:376`). One time is shown, `created_at`, as a coarse relative
  age (`2d`), and the files view sorts by `edited` without ever showing what it sorted on. The
  reader panel is not optional: it appears whenever the frame is at least 90 columns
  (`ui.rs:44`) and there is no key to put it away.
- **Expected.** The list views read as a **table** with aligned columns — id, title, created,
  edited, and whatever else earns its width — and a **toggle key hides the reader** so the table
  gets the whole frame when what you want is to scan rather than to read.
- **Why.** Two separate things, reported together because they are the same want. Scanning a week
  of notes is a *table* problem: you compare down a column, and a column you cannot see (`edited`)
  or that changes shape per row (the degrading meta cell) defeats that. And the reader is exactly
  what makes the table too narrow to hold the columns — it takes at least a third of the frame and
  cannot be dismissed, so the answer is a key, not a wider terminal.
- **Where it lives.** `crates/jot-tui/src/ui.rs` (`row_line`, `Columns`, `split_main`,
  `reader_text_width`), `crates/jot-tui/src/key.rs` (a new non-writing binding — no prefix; it
  changes nothing in the vault), `crates/jot-tui/src/app.rs` (the toggle's state).
- **Constraints this must not break.**
  - **The reader's own rule stands.** It is dropped below 90 columns because two bordered panels
    fit in neither. A toggle adds a way to hide it; it does not add a way to force it on at 60
    columns.
  - **The id column may not change width as the cursor moves.** That is why the TUI floors at 13,
    and it is the reason the abbreviation table is rebuilt per reload rather than per row.
  - **Every glyph stays East Asian Neutral.** Box-drawing a table is where Ambiguous-width
    characters get in, and a two-column glyph in a fixed cell breaks the frame under a CJK locale.
  - **Thread-agnostic still applies.** A workspace whose schema declares no `relation:*` entry has
    no replies and no parents; the table must not reserve a gutter for columns that are always
    empty there.
- **Settled 2026-09-09, at the user's direction: `created` absolute, `edited` relative.** Two
  relative ages side by side (`2d` / `2d`) say less than one does; an absolute creation time is the
  fact that does not change, and a relative edit time is the one you read as "how stale is this".
- **Open.** Which columns beyond those four, at which widths, and what drops first as the frame
  narrows.
- **Superseded in part by the next item.** The three-pane layout puts this table in the middle
  pane and adds a second thing competing for width. The toggle is still right; the width budget
  below is where it gets decided.

---

### Thread detail as a lane graph, in the shape of `undotree`

The one item here that adds a view rather than sanding one. Stage 5 specified thread detail and
did not build it; what arrived from dogfooding is a *rendering* for it, borrowed from
[`undotree`](https://github.com/mbbill/undotree), the nvim plugin that draws Vim's undo history.

**Why that plugin is the right thing to borrow from.** Vim's undo history is a tree for exactly
one reason: undo, then type something new, and the history *branches* rather than overwriting.
That is structurally the same event as replying to an older note instead of the newest one — which
is the event this whole app is built around. undotree is a solved, ten-year-old answer to "render
a small tree where every node is a thing you might want to go to", and the answer is a `git log
--graph` style **lane gutter beside a one-line label per node**, not a canvas.

Its own worked example, from `autoload/undotree.vim`:

```text
 6 8  7
 |/   |
 2    4
  \   |
   1  3  5
    \ | /
      0
```

The algorithm is a sweep over a list of **lanes**: repeatedly emit the live node with the lowest
sequence number, replace its lane with its children, and expand a fork into extra lanes on the next
pass. Nodes are `*`, lanes `|`, forks `/` and `\`.

**What jot already has that makes this cheap.**

- `Thread { focus, ancestors, tree: TreeNode }` from stage 2 is the input, unchanged.
- Sibling order is creation order from UUIDv7, and a reply is always created after its parent — so
  the total order the sweep needs exists for free and needs no `position` column, which stage 2
  refused to add for good reasons that still hold.
- A thread is tens of nodes. There is no performance question and nothing to persist.
- **The reader panel is already undotree's diff panel.** Graph on the left, the selected note's
  content on the right, on the split this surface already draws.

**What it buys, beyond looking right.** Stage 5 specified thread detail as *three* sections —
ancestors above, focus in the middle, descendants below as segments — which is three layouts,
three scroll behaviours, and a cursor that means something different in each. A lane graph is one
list. Ancestors are the first few nodes of the trunk, so "collapse past three" becomes a display
rule on one lane rather than a section with its own rules, and `j`, `k` and `Enter` behave here
exactly as they do in every other view. A linear thread — which is most of them — costs two
columns of gutter, because the gutter is only as wide as there are live lanes.

**Settled, at the user's direction, 2026-09-09:**

- **Root at top, growing downward.** undotree is root-at-bottom, newest-at-top; a conversation is
  not. Reversing the sweep is trivial and this is how every threaded reader has ever worked.
- **Quotes stay out of the graph.** `quote_to` is a second edge type, and a tree render plus
  cross-lane edges is a *DAG* render — which is where lane graphs stop being cheap and start being
  the hard part of `git log`. Quotes remain the reader's embedded card, where stage 5 already put
  them.
- **Holes are out of scope here.** A `Deleted` or `Trashed` node mid-thread needs a lane slot or
  its children visually reparent, and that decision is being deferred deliberately: this item
  assumes every note in the thread resolves. **It is a deferral, not an answer** — dangling
  references are a designed state in this app, so the graph will meet one. Revisit before the view
  is called done.

**Built 2026-09-28 in `bcb5f4e`**, with three judgements the plan had left open and which are argued
in that commit: `t` to toggle rather than a third bracket (`[` and `]` name panes of the *frame*, and
the graph is a panel inside one); a six-lane cap that **folds** into the last drawn column rather than
dropping rows, because the list is what carries the focus and the art is not; and an over-tall thread
windowing on the focus with `+n above` / `+n below`, where a marker may never displace the focus row.

**One thing it left stale.** `Action::Open` — `Enter` — still toasts `thread detail is not built yet`,
which `t` has arguably made untrue. Deliberately not changed in that commit: what `Enter` should do now
that a thread view exists is its own question, and the honest answers include "the same as `t`",
"nothing, remove the key", and "open the thread *in the main pane*, which is what stage 5 meant".

**Consequences to record elsewhere.**

- **`segments()` is no longer what the TUI renders.** Stage 2's form 2 was named in `stage5.md` as
  the thread view's rendering form; a lane graph wants a topological sweep over `TreeNode` instead.
  Both projections stay in core, still tested, still correct — but `stage5.md`'s thread-detail
  section is now describing a view that is not being built, and should say so rather than be left
  to contradict this one.
- **Only ASCII in the gutter.** Checked with `unicodedata.east_asian_width`: `*`, `|`, `/`, `\` are
  Narrow, while `●`, `│`, `╱`, `╲` and every box-drawing character in U+2500–U+2573 are
  **Ambiguous** — two columns under a CJK locale, which in a fixed-width gutter is the broken frame
  stage 5's marker column was designed to avoid. undotree offers the Unicode set as an option; do
  not take it.

**Open, and it is the one that decides how this reads.** In what order are nodes swept?

- **Chronological**, as undotree does it — one global creation order, the vertical axis is time.
  Honest about *when*, but a branch opened early and answered late holds an idle lane down the
  whole thread.
- **Subtree order** — depth-first, each branch drawn to completion before the next begins, with
  siblings still in creation order. Branches stay contiguous and idle lanes stay short, at the cost
  of the vertical axis no longer being a timeline.

undotree chose chronological because for an undo history time *is* the subject. For a conversation
it probably is not: you read a branch, not a minute — which was the recommendation here.

**Settled 2026-09-09, at the user's direction: chronological.** Which is the same order the
timeline already reads in, and the same order the sweep gets for free from UUIDv7. The idle-lane
cost stands and is accepted.

**Placed below the calendar in the sidebar, and rendered only while toggled.** Not permanently
resident: that is what keeps the sidebar's height budget honest once the calendar has taken 13 of
its rows.

**Un-deferred 2026-09-09**, after the calendar comparison — the space it needs is exactly the space
deleting the losing calendar freed.

**The sidebar is 22 columns, 20 inside its borders, so this is a minimap and not a reading view.**
A lane gutter plus a truncated title is what fits. The question it answers is "where am I in this
thread", and the reader panel goes on answering "what is this one". Two panels, two questions —
which is the same division the reader already has with the list.

---

### Three panes: sidebar, table, reader — and a calendar that is mostly dots

The layout the two items above are really about. **A fixed-width sidebar on the left; the main pane
takes the rest; the reader is a split of the main pane, not a third pane of the frame.** The sidebar
carries a **calendar**: days, an indicator for today, and a dot under any day that has a note created
on it. Nothing else — no week numbers, no counts, no heat map. **Settled 2026-09-09, at the user's
direction**, including the empty space below the calendar: the sidebar is the calendar alone for
now, and what fills the rest of that column is a later question, not an unfinished part of this one.

**The calendar has to do something, or it is decoration.** A dot that only says "something happened
here" is a fact you cannot act on. The version worth building is one where **selecting a day filters
the table to that day**, which the core query layer already supports: `TimelineQuery` carries
`since` and `until` (`crates/jot-core/src/query.rs:412-423`), so a day is a bounded pair and the
table is the same table with a narrower query. No new projection, no new view.

**The volume is not the problem.** The question this raises is whether the dots cost anything to
compute, and they do not:

- The calendar needs, at most, **31 booleans per visible month**. Even the whole-vault answer — the
  set of local dates that have a note — is one entry per day of capture: a year of daily use is 365
  of them, ten years 3650. It is a `BTreeSet<NaiveDate>`, not a table.
- Deriving it is one pass over notes that already exist in the snapshot. Stage 4's measurements at
  10k notes put `timeline(50)` at **1.8 ms** and a warm `sync()` at **73 ms**; a distinct-day scan
  is the same order as the former, and it is recomputed on reload rather than per frame.

**The nesting is what makes this cheap.** Because the reader splits the *main pane* rather than the
frame, `split_main` keeps its current meaning exactly — subtract the sidebar first, then hand the
remainder to the function that already exists. `READER_MIN_FRAME`, `LIST_MIN` and `LIST_MAX` are
unchanged; they simply apply to a smaller area. One subtraction, no new geometry.

**The width arithmetic that follows**, against the constants in `crates/jot-tui/src/ui.rs`:

| | columns | where the number comes from |
| --- | --- | --- |
| sidebar | **22** | 7 day cells × 3 columns − 1, plus two border columns |
| main pane, table alone | **40** | `LIST_MIN` |
| main pane, table + reader | **90** | `READER_MIN_FRAME`, unchanged |
| frame, sidebar + table | **62** | |
| frame, all three | **112** | |

So the reader appears once the frame reaches **112 columns**, and below that the main pane keeps the
table alone — which is the rule already in the code, one pane deeper. At 80 columns, still the
default almost everywhere, you get sidebar plus table with 18 columns spare.

**Two toggles and a drop order.** The reader already drops itself when the main pane is under 90;
the sidebar needs the same treatment and the same escape hatch, because 22 columns is a quarter of
an 80-column terminal spent on one month. Decide the order once — reader first, then sidebar, then
the table alone — and make both togglable so the rule can be overridden in the direction that
*removes* a pane, never the one that forces one in.

**The height is the other problem, and it is what "dot under the day" costs.** A month spans up to
six week rows. Putting the dot on its own line under each row doubles them:

- **dot under the day** — 6 × 2 + weekday header + month header = **14 rows**
- **dot in the cell**, as a trailing glyph or a styled day number — **8 rows**

On a 24-row terminal, minus the status line, 14 rows is 61% of the sidebar's height spent on one
month.

**Settled 2026-09-09: both were built and compared; the dot under the day wins.** The sidebar
rendered the two variants stacked, at one width against one vault, and the comparison was made on
sight. The dot-in-cell variant is deleted, along with the scaffolding that carried it — which is
what the scaffolding comment promised would happen, made good on rather than left as an intention.

So the calendar is 13 content rows, and the roughly seven rows that frees are where the thread
graph goes.

**Constraints this inherits.**

- **The dot glyph is one more width trap.** Checked with `unicodedata.east_asian_width`: `·`
  (U+00B7) and `•` (U+2022) are **Ambiguous** — two columns under a CJK locale, which in a 3-column
  day cell breaks the grid. `∙` (U+2219) and `◦` (U+25E6) are Neutral, and `.` is Narrow. Use one of
  those three.
- **Days are local, note ids are UTC.** `created_at` comes from a UUIDv7 and is UTC; a calendar is
  local by definition, and a note captured at 23:30 local can fall on the next UTC day. Bucketing
  must happen in local time. The surface already has the shape for this — `App` does not read the
  clock and `now` is passed into `ui::draw` (`crates/jot-tui/src/app.rs:68`) — so the offset arrives
  the same way rather than becoming a second source of time.

  **Built, with one compromise still live.** `Workspace::days_with_notes` is generic over
  `chrono::TimeZone` on purpose, so a caller holding `Local` gets the offset in force at *each note's
  own instant*. `App::with_zone` takes a `FixedOffset` and flattens that away, because holding a real
  zone would mean `App` consulting the system timezone database on every reload — the environment read
  the design avoids. The cost, documented where it happens: notes from the other side of a
  daylight-saving change can land on the neighbouring day within an hour of midnight, for half the
  year. A tradeoff, not a settled decision; revisit if a dot ever looks wrong.

- **What a selected day *means* is not built, and is the harder half.** This item says "`TimelineQuery`
  carries `since` and `until`, so a day is a bounded pair" — true, but a day only becomes a bounded
  pair after a **local date → UTC instant** conversion, and that is the direction that is *not* total.
  A local midnight can be skipped (spring forward) or repeated (fall back), so the conversion returns
  a mapped result with `None` and `Ambiguous` cases somebody has to decide. Left unowned this becomes
  an `.unwrap()` and a panic in someone's timezone.

  Two ways out, and it is a decision rather than a patch because it changes a struct three surfaces
  build: either `TimelineQuery::on_day(day, zone)` in core with the gap/ambiguity policy written down,
  or `TimelineQuery` carries the **local day itself** and core buckets on the UTC → local direction —
  which never constructs a local midnight at all. The second is recommended.
- **Core owns the query.** Surfaces never touch SQLite, so "which days have notes" is a new
  `jot-core` read, not a scan in the TUI. That makes it a **core change**, and stage 5's lesson
  about discovering `FileSort` mid-TUI applies exactly: schedule it with the wave that owns core,
  not with the wave that draws the sidebar.

**Open.**

- **What moves focus between the sidebar and the table.** This is the live version of a worry that
  was stated badly earlier, and the sidebar being calendar-only narrows it rather than removing it.
  Today `Tab` **changes the view** — timeline → files → trash — because there has only ever been one
  pane to be in. A calendar you can select a day in is a second focusable thing, so something has to
  move between them, and `Tab` is the key every terminal user reaches for. It is taken. The options
  are to give focus movement a different key and leave `Tab` cycling views, or to move `Tab` to
  focus and find another key for the view cycle. Neither is free: `Tab` cycling views is documented,
  in `?`, and in the muscle memory of the one person using this.
- **Dot under the day, at 14 rows, or dot in the cell, at 8.**
- **What a selected day does to the other views.** Filtering the timeline is obvious; whether it
  also filters files, search and trash is not.

---

## Work

Ordering: bugs before improvements, and within the bugs, whatever is being hit daily. The run log
is `docs/runs/stage6/log.md`.

- [x] TUI: undo's key is spelled the same everywhere it is named — `7631b83`. **Half done:** the
      messages are derived from the keymap; whether `Space U` is *offered* where the offer stands is
      still open, because it is coupled to the trash keys below.
- [ ] TUI: restore and purge in the trash view, purge behind a confirmation. **Blocked on naming the
      keys.** The oldest unfixed item here and the one being hit daily.
- [ ] TUI: `up_to_parent`'s "parent is hidden" message, which can only fire when the parent is purged.
- [x] CLI: `jot ls` ids at the TUI's floor — `1d55a94`. Workspace ids stayed at 8; they are v4.
- [ ] CLI: grouped `--help`, generated from the derive rather than written twice. Parked.
- [ ] A `config.toml` beside the registry, with one owner for OS paths and an environment override.
      Parked.
- [ ] Core: `default_path`'s shape test, which asserts a path component macOS does not produce.
- [ ] TUI: a view indicator. **Smaller than written** — see the item.
- [ ] TUI: table layout for the list views. The reader's toggle landed with the layout; the columns
      did not.
- [x] TUI: thread detail as a lane graph — `bcb5f4e`. Chronological, root at top, in the sidebar
      below the calendar, on `t`.
- [x] Core: which days have notes, as a read the TUI can ask for — `ecbd08b`.
- [x] TUI: the three-pane layout, with a drop order for the side panes and a calendar — `40d1fc4`,
      and `bcb5f4e` deleted the calendar variant that lost.
- [x] Docs: `stage5.md`'s thread-detail section, now marked superseded, with its key table pointing
      here for the keys this stage added.
- [ ] Day selection: what a dot is *for*. Blocked — see the calendar item's local-midnight note.

## Acceptance

- The key named in the trash toast restores the note when pressed, exactly as written.
- A note trashed in an earlier session can be restored from the trash view, and purged from it,
  with purge refusing to fire on a single keystroke.
- An id copied out of `jot ls` and an id read off `jot tui` are the same string for the same note.
- A corrupt `config.toml` produces a warning and a working `jot`, not a failure to capture.
- No key in `config.toml` can change a byte written into a note file.
- With the reader hidden, the list occupies the full frame and every column still aligns; the
  reader comes back on the same key.
- A thread with a fork renders as one list with a lane gutter, the root at the top, and `j` / `k`
  moving through it the way they move through every other view.
- A thread with no forks renders as a straight column two glyphs wide, and reads no worse than an
  indented list would.
- The gutter contains no character wider than one column under a CJK locale.
- Selecting a day in the calendar narrows the table to notes created on that day, in local time,
  including one captured within an hour of local midnight.
- At 80 columns the layout drops panes in the defined order rather than squeezing all three, and
  the calendar grid stays aligned under a CJK locale.
- No new binding is reachable without appearing in `?`, and no message names a key the keymap does
  not resolve.

## Risks

- **Polish has no natural end.** Every one of these is a small change to a surface that already
  works, which is the exact shape of work that fills whatever time it is given. The bar is
  "dogfooding stopped complaining", and the buckets above are the list — not a starting point for
  a list.
- **The keymap is the shared thing.** Three of the four items add or move bindings, and `?`, the
  footer, and the toast strings are all supposed to be derived from one table. Changes that spell a
  key twice reintroduce the undo bug under a different name.
- **A table invites columns.** Every field is arguably worth a column, and the frame is 80 columns
  wide on the terminal that matters. Decide what drops first before deciding what to add.

## Open questions

- Undo, restore, and purge: three operations, and only undo has a key today. Which of them belong
  behind the `Space` prefix, and does restore-under-cursor make session-scoped undo redundant?
- The `--help` groups, and which side of the capture/read line `edit` falls on. **Parked until the
  layout work lands**, at the user's direction, along with the config's shape (`jot config` command
  or file alone, and whether it may ever be scoped per workspace).
- What the graph draws where a thread has a hole — deferred on purpose, and still owed. Now deferred
  *in code* as well: core truncates `ancestors` at the first missing note and never descends through
  one, so a purged mid-thread note yields a smaller graph rooted at the highest ancestor that still
  resolves. It will look wrong the first time something is purged mid-thread.
- What `Enter` does now that `t` shows a thread.
- Whether `App` should hold a real timezone rather than a `FixedOffset` — see the calendar item.
- The local-midnight policy that day filtering needs.
- Which key moves focus between the sidebar and the table, given `Tab` already cycles views.
- Where the view indicator goes: a strip above the table, or a segment of the status line.
