# Post stage 3 — dogfooding log

> **Renumbered.** This log was written when the index was stage 2 and this work was stages 3 and 4.
> The stages were later renumbered so that what was actually built comes first: the domain is now
> stage 2, the CLI stage 3, and the SQLite index stage 4. Stage numbers in this file have been
> updated to the new scheme; quoted words have not.

Changes made after stage 3 landed, driven by using `jot` rather than by a stage document. Each entry
records what real use exposed, what was decided, and what it cost.

`stage3.md` says everything wanted during the first week should land in a list, not in the code.
These are the ones that were not feature requests but **defects or design errors the CLI made
visible** — the category that stage doc explicitly wants acted on.

## 1. Short ids were unusable, and the fix belonged in core

The first `jot ls` against a real vault printed every row as `01a05a57` and not one of those ids
resolved. Git's eight characters work because a SHA is random from its first bit; a UUIDv7's leading
48 bits are a millisecond timestamp, so eight hex characters cover the top 32 of them — one shared
value per ~65 seconds. Notes captured in the same minute collide, and those are exactly the notes you
refer to by short id.

Replaced with the shortest prefix unique in the set an id is displayed beside, floored at 8, which is
what git actually does. Lives in `jot_core::shortid` because there are two callers and both are
UUIDv7. Written up in `stage3.md`.

## 2. `jot ws ls` could not be read

Two rows, both named `demo`, both the same path, nothing to tell them apart. The registry keys on
workspace id — correct, since the id survives the folder moving — so a vault deleted and remade
really is a second entry. The listing now leads with the id, the same shape `jot ls` uses.

## 3. `jot ws use` could not reach half of them

Following from 2: `ws use <name>` matched on name and took the first hit, so the second workspace was
**unreachable**, and one of two was picked silently. That is "never guess between your things" broken
in the place that decides where notes get captured.

The first fix accepted a name **or** an id prefix, trying the name first. That removed the
unreachability but kept a subtler version of the same fault: the *meaning* of the argument depended
on what happened to be registered. `jot workspace use notes` was a name lookup right up until
someone registered a vault whose id started `notes`.

**Settled**: id is what a bare argument means, with `--id` and `--name` as an exclusive, required
`ArgGroup`. A workspace name is display-only — seeded from the directory basename, documented as
safe to hand-edit, unique by no rule — so it is not a thing to resolve against by default. `--name`
is an exact match that may find several and says so, exiting 4. `workspace remove` shares the
selector so the two cannot drift.

## 4. Workspace ids are UUIDv4 — a ratified change to a stage 1 decision

**This one changed a documented decision and a verifier-owned test, so it is recorded in full.**

`stage1.md` specified the workspace id as UUIDv7. No rationale was ever written for the version: the
paragraph under it argues only the *self-identifying* property, which any UUID version gives. It
looks like inheritance from note ids rather than a reasoned choice.

The case for v4, which is the same finding as entry 1 applied at its source:

- **Nothing reads a workspace id's timestamp.** Grepping the crate, the only thing that referenced
  its version was a test asserting the version. `created_at` is decoded from a *note* id; id order is
  creation order for *notes*. A workspace is asked for neither.
- **v7 broke short workspace ids**, which entry 3 had just made load-bearing: `jot ws use <prefix>`
  needs a prefix that separates vaults, and two vaults created in one minute share eight characters
  under v7. Under v4 the id is random from its first bit.
- **v7 leaked a date** into `workspace.toml`, which people commit.

Note ids remain v7 and must: the asymmetry is now argued in `workspace::Manifest::id` rather than
left implicit.

### What it cost, stated rather than glossed

Workspace ids no longer sort by creation time, so "which vault did I make most recently" is no longer
answerable from the id. Nothing asked it — `ws ls` prints in registry order and the registry carries
`last_opened` for recency — and if it is ever wanted it belongs in the manifest as an explicit field,
which survives the id and needs no decoding. The trade is deliberate, not overlooked.

Reading is unaffected. `open` parses any UUID version, so vaults minted before this keep their v7 ids
and stay correct; `a_workspace_id_of_any_uuid_version_still_opens` pins that.

### How this was adjudicated, and the rule it bent

The change was caught by `crates/jot-acceptance/tests/phase_b.rs`, which asserted both the v7 shape
and that workspace ids sort by creation time. That suite is **blocking** in CI and, under
`orchestration.md` rule 2, is the verifier's file: *"an implementer who believes an acceptance test is
wrong files an appeal; it does not get to edit its way to green."*

So implementation stopped and the conflict was put to the user, who ratified v4 — on the ground that
randomness is what `ws use` needs and notes do not — and directed that the suite be updated.

**An implementer then edited a verifier-owned file.** That is the appeal being granted by the person
the rule protects, not the rule being ignored, but it is the second time this project has run without
an independent verifier and it should be reviewed rather than trusted. Two assertions changed:

| Assertion | Change |
| --- | --- |
| `is_uuid_v7(id)` | → `is_uuid_v4(id)`. Guards the same thing: a real, well-formed, distinct id per `init`. |
| ids sort by creation time | **Removed.** The property is gone by design; see the cost above. |

The mutation the probe exists to kill — M32, `init` minting a constant id — is untouched by either
change, and the distinctness and immutability assertions that actually kill it are unchanged.

`is_uuid_of_version` now backs both `is_uuid_v4` and `is_uuid_v7` so the two shapes cannot be blurred;
a test asserts a v4 fails the v7 check and vice versa.

## 5. The test suite was writing into the developer's real registry on Windows

A Windows-only CI failure (run 33464699374): two `ws ls` tests expected 2 workspaces and saw 24 —
other tests from the same parallel run. `Vault::cmd` isolated the registry with `XDG_CONFIG_HOME` and
`HOME`, which do nothing on Windows: `registry::default_path` resolves through `directories`, which
reads `FOLDERID_RoamingAppData` via `SHGetKnownFolderPath`. A syscall, not an environment variable.

So every test that registered a workspace had been writing into the real
`%APPDATA%\danjolabs\jot\config\workspaces.toml`. The assertions were right; the isolation was
imaginary, and the comment claiming otherwise was simply false on half the CI matrix.

Added `JOT_REGISTRY`, read by the CLI itself so it works on every platform — the escape hatch
`registry::default_path`'s own docs anticipated, and useful beyond tests for a portable install or
separate work and personal registries. Policy sits in the surface because `jot-core` keeps
`directories` behind one function and takes explicit paths everywhere else.

**Confirmed on the platform that had to confirm it.** The fix was authored on Linux, where the suite
was already passing, so it was unverified where it mattered until CI ran: `windows-latest` is green
from run **33472107961** onward, and has stayed green on every push since. Worth stating because a
Linux-only verification of a Windows-only bug is not verification at all — the same mistake that
produced the bug is the one that would have hidden the fix.

## 6. `$EDITOR` opened on an empty block, and the body grew

`jot new` handed the editor `---\n---`, which says nothing about what a note here may hold. It now
renders the declared schema as a template with empty placeholders, which round-trip to nothing when
ignored.

Building that exposed a compounding bug: `body_text` normalized only the trailing end, and an
`$EDITOR` body always arrives with a leading newline because it is the text after the closing fence.
A note gained a blank line every time it went through the editor, and another on each subsequent
edit. Now trimmed at both ends, which makes it idempotent — the property `edit`'s no-op check depends
on, since an unchanged body must re-render to identical bytes or every save moves mtime and
`edited_at` with it.

## 7. Nothing could unregister a workspace, and the names were abbreviations first

`workspace remove <name|id>` and `workspace prune` close the gap entry 2 opened: a stale registry
entry could previously only go by hand-editing `workspaces.toml`.

`prune` confirms before acting, which is worth defending because it looks like ceremony. `is_stale`
is `!path.exists()`, so an external drive that is merely **unmounted** presents exactly as a deleted
vault. Pruning it throws away the registration for a vault whose notes are fine. The prompt lists
what will go and names that hazard; `--yes` skips it.

`remove` says "the directory is untouched" on every run rather than only in `--help`. There are now
two `remove`s and the other one moves a file.

At the same time the command names were flipped: **the full word is the command, and `ls`/`rm`/`ws`
are aliases**, where it used to be the reverse. Abbreviations are only obvious to someone who already
knows the tool, and `--help` is read by someone who does not. It also disambiguates the two
`remove`s: `jot remove` versus `jot workspace remove` is legible where `jot rm` versus `jot ws rm` is
two characters. Nothing was taken away — both spellings work and both show in help.

## 8. The stage 2 and 3 acceptance suites are deliberately deferred

`docs/runs/stage2-3/log.md` recommended dispatching a verifier to write them retroactively before stage 4
builds on this domain. **That recommendation was considered and declined**, at the user's direction,
to keep the iteration cycle short. It is a decision rather than an oversight, and it is recorded here
so that a later reader does not mistake it for one.

The reasoning, and it is visible in this log's own contents: every entry above came from *using* the
tool, and four of them changed something a stage document had already settled — the short-id design,
the workspace id version, the workspace selector, the command names. A suite written against stages 2
and 3 before that dogfooding would have been written against a design still moving under it, and each
of those findings would have arrived as a suite to rewrite as well as an implementation to change.
Acceptance criteria are worth most when they pin something settled; written too early they mostly pin
the author's current opinion, and they make changing that opinion expensive at exactly the moment it
is cheapest to change.

### What it costs

Stated plainly, because the cost is real and is not reduced by the decision being deliberate.

- **`orchestration.md` rule 2 does not hold for stages 2 and 3.** Everything in them was written and
  judged by the same author. `crates/jot-cli/tests/cli.rs` covers the stage 3 criteria but is not
  independent evidence, which is precisely what the acceptance crate exists to provide.
- **The thread algebra has no independent check.** The routing table calls it "the crown jewel…
  wrong is invisible", and its invariants are asserted over eight fixed shapes rather than the
  several thousand generated trees `stage2.md` asks for.
- **Entry 4 is the demonstration.** The one blocking acceptance test that *did* exist caught a real
  conflict with a documented decision immediately — and the resolution was an implementer editing a
  verifier-owned file. That is what the absent suites would otherwise be doing for stages 2 and 3:
  catching this class of thing without a person having to notice.

### What partially compensates

- The thread algebra is checked against the worked example **printed in `stage2.md`** — both
  projections, exact sequences. An external oracle rather than a self-consistent one.
- The stage 1b acceptance suite is still blocking and still green, so nothing below stage 2 has
  silently regressed under all of this.
- 459 tests, fmt and clippy clean, on every commit.

### When to revisit

**Before stage 4**, which is where the cost stops being theoretical: the index is built on top of
this domain, `overview.md` calls the derived-index invariant "the project's foundation", and a stage
2 rule that turns out wrong is far more expensive to correct once queries depend on it. The
iteration-speed argument above applies to a design still being discovered by use; it stops applying
once something is being built on top.

## Still open

- The stage 2 and 3 acceptance suites — deferred deliberately, see entry 8, and due before stage 4.
- A reply cycle in `relation:reply_to` terminates but reports nothing. `stage2.md` requires "an error
  naming both notes, and no hang"; there is no hang, but the read path renders a truncated tree
  instead of raising. `Error::ReplyCycle` exists and fires only in `Workspace::open_note`.
- Property tests over generated trees, per `stage2.md`. The invariant bodies are generator-ready.
- `insta` snapshots for the human output, and the `jot new` latency budget, which needs stage 4 and a
  large vault to mean anything.

## Where this is left

`prototype`, green on both platforms. Stages 1–3 built, the CLI dogfoodable, 459 tests and 111
acceptance passing, `fmt` and `clippy -D warnings` clean.

**Stage 4 is next, and it is the reason entry 8 names a revisit point.** Everything above was built
against `snapshot::Snapshot` — a scan standing in for the index — so this stage is a *substitution
behind an existing seam* rather than a greenfield build. `stage4.md` carries a "What the snapshot
leaves for this stage" section mapping each planned query to the method that already answers it, and
one criterion that did not exist before:

> With SQLite behind it, `crates/jot-cli` and every existing `jot-core` test must pass **unchanged**.
> If a public signature has to move to accommodate a database, the seam was in the wrong place — and
> that is the finding, not the change.

The 459 tests are that stage's regression suite. What the snapshot does *not* do is all of it
performance — no `files` fast path, so every command reparses the whole vault — which is exactly the
shape a deferred index should leave behind, and is why deferring it was safe.
