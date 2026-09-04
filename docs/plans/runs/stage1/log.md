# Stage 1 log

## What shipped

`jot-core`'s vault foundations: the `note`, `frontmatter`, `fs`, `error`, `registry`, and `workspace`
modules, backing a workspace that can be initialized, opened, and discovered on disk; notes that
parse and round-trip without losing a byte or an unrecognized key; atomic writes proven to replace an
existing file on Windows; and a registry of known workspaces in the OS config directory that degrades
to empty rather than propagating an error. At seal: **170 unit tests + 1 doctest** in `jot-core`, and
an acceptance suite of **102 tests across 5 binaries** in `crates/jot-acceptance`, all green — **on
Windows 11 / `1.97.1-x86_64-pc-windows-msvc`**. CI's Linux leg has run the fmt/clippy/test job, but
the acceptance job's Unix arm of the interruption injection (`sync_parent_dir`'s `cfg(unix)` path,
and the chmod-based `BlockedReplacement`/`BlockedRename` mechanism `an_interrupted_write_leaves_the_
original_intact` depends on) has **never executed** — it was reviewed by reading the code, not by
running it, and is this stage's one genuinely **UNVERIFIED** item (`verification.md`, "Linux, by
reading only"). `fsync` durability is separately untestable by construction (M39) and is named here
rather than left to look covered.

`docs/plans/stages/stage1b.md`, committed the same day as seal, supersedes part of what shipped: identity
moves from filename-and-frontmatter to filename-only, and the two-path (preserve/canonicalize)
serializer is replaced by one schema-driven render path. None of that is implemented yet — stage 1b
is a plan doc, not code — and everything above describes what stage 1 actually built and shipped.

## Waves

Model routing below follows `dispatch.md`'s dispatch log for waves 0–2, per this role's standing
instruction to prefer it over commit trailers. **`dispatch.md`'s dispatch log stops at the wave 2
merge** — it has no entries for wave 3, wave 4, or the three post-gate fix rounds. For those, the
model column is taken from `breakdown.md`'s routing table, which `dispatch.md` states was "accepted
without modification," rather than from trailers (this task's instruction was explicitly not to infer
from them). The three fix-round rows are marked accordingly — this is a real gap in the audit trail,
called out again under Deviations.

| Wave | Task | Agent | Model | Outcome |
| --- | --- | --- | --- | --- |
| 0 | Stage decomposition | stage-planner | opus | `breakdown.md`, accepted without modification |
| 1 | T1.1 — cargo workspace, toolchain pin, CI matrix, fixture corpus | implementer | sonnet | done, in place |
| 2 | T2.1 — dependency set, YAML/time crate decision, frozen error taxonomy | implementer | opus | done, worktree, merged `7b3d52a` |
| 2 | T2.2 — phase A acceptance suite | verifier | opus | done, worktree, merged `7b96d51`, correctly red |
| 2 | wave 2 merge + mechanical gate | integrator | sonnet | gates 1–3 green, gate 4 red as designed |
| 2/3 boundary | API contract pin (error-variant renames, module shapes) | (orchestrator) | — | `docs: pin the wave 2/3 API contract`, committed `9af5c62` |
| 3 | T3.1 — `NoteId`, `Note`, `NoteMeta`, `Frontmatter`, dual serialization paths | implementer | opus (per `breakdown.md`, not independently logged in `dispatch.md`) | done, worktree, `fba8102` |
| 3 | T3.2 — atomic write, filename parsing, enumeration | implementer | opus (per `breakdown.md`, not independently logged) | done, worktree, `2d0fe1d` |
| 3 | T3.3 — workspace registry | implementer | sonnet (per `breakdown.md`, not independently logged) | done, worktree, `f441228`; landed 1s after T3.2 — see Deviations |
| 4 | T4.1 — `init` / `open` / `discover`, `workspace.toml` | implementer | opus (per `breakdown.md`, not independently logged) | done, in place, `53cb57a` |
| gate | Phase B verification, mutation spot-check (23 mutations) | verifier | opus (routing table: verification is never sonnet) | **FAIL** — one blocking defect (F1), five acceptance-suite gaps, one deferred decision (F2) |
| fix round 1 | F1 fix (`note::load` / `fs::parse_note_filename` unified) | implementer | not recorded in `dispatch.md`; no trailer consulted per instruction | `c293114` |
| fix round 1 | Suite-gap closure (M15, M29, M31, M32, M38, M43) + lint decision | verifier | opus (same agent that owns `jot-acceptance`) | `989458c`, six-of-six re-measured, zero clippy warnings under `-D warnings` |
| fix round 2 | Code review findings F3–F7 | (agent not recorded — see Deviations, no `review.md`) | not recorded | `7ea4ab0` |
| fix round 3 | Close F5/F6 round, strengthen M43 symmetry | (agent not recorded) | not recorded | `bdef74d` |

## Deviations

- **Phase A moved from wave 0 to wave 2.** `orchestration.md`'s "First dispatch, concretely" worked
  example put the verifier at wave 0, before the cargo workspace existed. That is impossible as
  written: `crates/jot-acceptance` is a crate in a workspace nothing had created yet, and its tests
  need module paths (`jot_core::note::NoteId`, and so on) that nobody had fixed. `breakdown.md` moved
  phase A to wave 2, concurrent with the crate-error-taxonomy work, once wave 1's scaffold gave it a
  crate to `use` against — the rule phase A protects (no implementer of *behavior under test* runs
  before the tests exist) still held, because wave 1 and the rest of wave 2 land no stage-1 behavior.
  This is corrected in `orchestration.md` at this seal — see Plan-doc corrections.
- **T3.3 was placed in wave 3 despite depending on T3.2's `atomic_write` landing in the same wave.**
  `breakdown.md`'s own T3.3 definition-of-done says the registry's save path "uses the same
  staged-then-renamed discipline as `fs::atomic_write` (call it; do not reimplement it — T3.2 lands it
  in the same wave, so if it is not there yet, **block rather than fork**)." That is a real
  wave-planning flaw, not a hedge that resolved cleanly: an intra-wave dependency inside a wave whose
  entire point is that its three tasks are concurrency-safe. In practice T3.3 blocked on `atomic_write`,
  validated its own approach in a scratch project while waiting, and could not run its own gate until
  T3.2's worktree had something to depend on. The commit timestamps are consistent with this: T3.2
  landed at `2d0fe1d`, and T3.3 landed at `f441228` one second later, rather than in parallel. The
  correct fix would have been to either split `atomic_write` into its own micro-wave ahead of T3.1–T3.3,
  or accept that T3.3 is wave 4 material alongside T4.1 (which was already going to depend on T3.2).
- **Three of four agent worktrees were cut from the repo's initial commit rather than the branch
  HEAD**, and each of those agents had to reset before it could see `stage/1-vault-foundations`'s
  actual state. This is not independently documented in any run artifact — it is recorded here on the
  orchestrator's account, since no artifact in `runs/stage1/` captures it. Worth fixing in tooling
  before stage 4: a worktree cut from the wrong ref is a silent way to lose a wave's prerequisites.
- **The wave 2/3 API contract (`dispatch.md`'s error-variant renames and module shapes) was referenced
  in three implementer briefs before it was committed to the repo.** The content was settled and
  handed to wave-3 implementers first; `dispatch.md` itself records the commit landing at `9af5c62`,
  well before any wave-3 commit — so the *file* predates the wave-3 work, but the *briefing* predates
  the file. Sequencing detail, not a defect in the contract itself, but worth naming because it means
  an implementer reading only committed artifacts at the time would not yet have found the contract
  that governed their own task.
- **`review.md` does not exist.** `orchestration.md`'s stage loop names `/code-review high` on the
  stage diff as the third gate, alongside mechanical and phase B, and the artifact list in
  `orchestration.md` names `review.md` as where its findings live. Commits `7ea4ab0` ("fix code review
  findings F3-F7") and `bdef74d` ("close F5/F6 round, strengthen M43 symmetry") are the evidence the
  review happened and that something acted on its output. No `review.md` was ever written. I have not
  reconstructed one after the fact — an honest gap is worth more than an invented record. One
  consequence worth flagging: `7ea4ab0`'s F3–F7 numbering matches `verification.md`'s Findings section
  exactly (F3 = lossy `NoteMeta` rebuild, F4 = `yaml_serde` API leak, F5 = BOM message, F6 = duplicate
  frontmatter id, F7 = self-reference/dangling links), but `verification.md` explicitly characterized
  F3, F6, and F7 as accepted, non-blocking, or work to *inherit into stage 4* rather than defects
  requiring a stage-1 code fix. Whether the code-review pass independently reached the same
  conclusions and chose to fix some of them in stage 1 anyway, or whether scope was broadened at seal,
  cannot be determined without the missing artifact.
- **The byte-identical round-trip gate, the stage's stated single point of defense against silent
  frontmatter mangling, turned out to be structurally unfalsifiable under the U1 ruling.** §U1's
  "preserve on read, normalize on edit" ruling (escalated to the user) makes the round-trip criterion
  pass by construction for the preserving path — it can only fail if byte retention is not implemented
  at all. `verification.md` calls this out directly: "the criterion is weak evidence by construction
  ... the load-bearing work is on the canonical path." Phase B's 24-then-29 `phase_b.rs` tests exist
  because of this — closing a gap the stage's own written acceptance criteria could not see.
- **F1, a real defect, reached phase B rather than being caught earlier.** `note::load`'s private
  filename parser and `fs::parse_note_filename` diverged on five filename shapes, three of which
  (`.md`-suffixed) are exactly what `fs::live_note_paths` returns — meaning enumeration and load could
  disagree about whether a file was a note. `note.rs`'s own doc comment said the two must be kept in
  step; nothing enforced it. Fixed in `c293114` by deleting the private parser and delegating to
  `fs::parse_note_filename`, which the acceptance test (written before the fix, unedited after it)
  confirms by construction rather than by assertion.

## Fix rounds

Three rounds, all after the initial phase B **FAIL** verdict recorded in `verification.md`.

1. **F1 + suite-gap closure.** Blocking: F1 (filename-parser divergence). Non-blocking but requested
   in the same round: the five real acceptance-suite gaps mutation testing found (M15 dotfile
   enumeration, M29 `open` ignoring `kind`, M31 unsorted enumeration, M32 constant workspace id, M38
   `Registry::save_to` swallowing a write failure), plus strengthening M43's existing probe. Landed as
   `c293114` (F1, implementer, in place — concurrent with the verifier's worktree, which was still at
   `53cb57a` and explicitly could not measure F1 going green because editing `note.rs` was out of its
   ownership) and `989458c` (the six suite-gap tests plus a ratified lint decision: allow the two
   deliberate `__` names in `criteria.rs`, fix the seven `clippy::err_expect` warnings in `probes.rs`).
   All six gaps re-measured as caught after the round; suite left clean under
   `cargo clippy -p jot-acceptance --features stage1 --all-targets -- -D warnings`. One round, both
   items closed.
2. **Code review findings F3–F7**, `7ea4ab0`. No `review.md` exists to describe what was found or by
   whom; see Deviations for what can and cannot be inferred from the commit alone.
3. **"Close F5/F6 round, strengthen M43 symmetry"**, `bdef74d`, roughly 11 hours after the previous
   commit (`1788097414` → `1788137087` by commit timestamp) — a further pass on the BOM message (F5)
   and duplicate-frontmatter-id silence (F6), plus strengthening the M43 registry probe. Same gap as
   above: no artifact records what triggered a second pass on findings the first round's commit message
   already claimed to have fixed.

`verification.md`'s own verdict, after the F1 fix and gap closure: **FAIL until F1 lands, then PASS.**
It explicitly left F2 (the `to_bytes` write-hazard) as a decision to be *recorded*, not necessarily
implemented, before stage 4 starts. That decision is now moot in the form F2 posed it — see Plan-doc
corrections.

## Human checkpoints

`orchestration.md`'s "What Fable cannot verify" table lists **none** for stages 1, 2 and 4: "fully
mechanizable, which is why they come first." That held. The two items escalated to the user during
this stage were not human-checkpoint items in that sense — they were locked-decision-adjacent design
rulings the orchestrator could not make unilaterally:

- **U1 — key order versus the byte-identical round-trip.** Ruled: preserve on read, normalize on
  edit, two serialization paths. (Now itself superseded by stage 1b's single render path.)
- **U9/U10 — reader strictness, and whether a filename/frontmatter mismatch is a hard error.** Ruled:
  hard errors only, no `Anomaly` type in stage 1; frontmatter wins on `parse`, mismatch reported on
  `load`. (The filename/frontmatter mismatch case itself is deleted by stage 1b's identity change —
  there is no longer a frontmatter `id` to disagree with the filename.)

The eight remaining underspecifications (U2–U8) were ruled by the orchestrator without escalation —
see `dispatch.md`'s Adjudications section for the rulings themselves.

The one item this log treats as a standing, unresolved verification gap rather than a closed
human checkpoint: **the Unix arm of the interruption injection has never run.** CI's `ubuntu-latest`
leg runs the full mechanical gate and the acceptance suite, so the code executes on Linux — but nobody
has looked at a red or green result from `an_interrupted_write_leaves_the_original_intact` on Linux and
confirmed it is testing what it claims to. `verification.md`'s read-only review of the `cfg(unix)`
arms found them correct by inspection, including the caveat that the chmod-based injection does not
block `root`, and that the mutation-testing tripwire
(`harness_self_tests::blocked_replacement_actually_blocks_a_rename_on_this_platform`) is designed to
fail loudly rather than let the criterion go vacuously green if that ever changes. That is a strong
argument, not a substitute for the run.

## Timings

Derived from `.git/logs/HEAD` (commit timestamps, `+0900`), the only source available to this role —
this environment gave the scribe no `Bash` access this run, so `git log`'s trailer-aware formatting
could not be run directly; deltas below are computed from raw reflog timestamps instead. **Caveat
worth stating plainly:** several consecutive commits land 0–18 seconds apart (`7b96d51`/`d92d1e9`
at the same second; `fba8102` → `2d0fe1d` → `f441228` in 19 seconds total), which is not a plausible
elapsed time for independent opus agent sessions to have produced their diffs. These are almost
certainly commit-time artifacts of how this stage's work was assembled and merged rather than a
faithful record of each agent's actual working time, and the deltas below should be read as "time
between commits landing," not "time an agent spent." The larger gaps (hours) are more likely to
reflect real elapsed time.

Branch cut (`cc99b81`) is the reference point, `t = 0`.

| Commit | What | Δ from cut | Δ from previous commit |
| --- | --- | --- | --- |
| `104882b` | docs: stage 1 breakdown and dispatch adjudications (wave 0) | +30m14s | — |
| `0c3c6870` | T1.1 scaffold (wave 1) | +30m59s | +45s |
| `7b3d52a` | T2.1 deps/crate decisions/error taxonomy (wave 2) | +59m22s | +28m23s |
| `7b96d51` | T2.2 phase A acceptance suite (wave 2) | +59m23s | +1s |
| `d92d1e9` | chore: ignore agent worktrees | +59m23s | +0s |
| `9af5c62` | docs: pin the wave 2/3 API contract | +1h12m16s | +12m53s |
| `fba8102` | T3.1 note/frontmatter (wave 3) | +5h36m51s | +4h24m35s |
| `2d0fe1d` | T3.2 fs.rs (wave 3) | +5h37m9s | +18s |
| `f441228` | T3.3 registry (wave 3) | +5h37m10s | +1s |
| `857bb2e` | reconcile acceptance suite with frozen error taxonomy | +5h37m10s | +0s |
| `53cb57a` | T4.1 workspace init/open/discover/manifest (wave 4) | +5h48m2s | +10m52s |
| `c293114` | fix F1 (fix round 1) | +6h25m42s | +37m40s |
| `989458c` | phase B verification doc, mutation spot-check, suite gap closure (fix round 1) | +6h32m35s | +6m53s |
| `7ea4ab0` | fix code review findings F3–F7 (fix round 2) | +7h20m8s | +47m33s |
| `bdef74d` | close F5/F6 round, strengthen M43 symmetry (fix round 3) | +18h21m21s | +11h1m13s |
| `d0d90f4` | docs: stage 1b, declared frontmatter schema | +21h8m8s | +2h46m47s |

Measured numbers `stage1.md` and `verification.md` asked to be recorded, not timing but worth
collecting here since this is where the stage's numbers live:

- Suite state at seal: **170 unit tests + 1 doctest** (`jot-core`), **102 acceptance tests across 5
  binaries** (`jot-acceptance`), all green on **Windows 11 / `1.97.1-x86_64-pc-windows-msvc`**.
  `verification.md`'s own last independently-recorded checkpoint (end of the fixer round, before the
  code-review fix rounds) was 149+1 core / 101 acceptance across the 4 known test files
  (`lib`, `criteria.rs`, `probes.rs`, `phase_b.rs`); the growth to 170/102 is attributable to the two
  undocumented fix rounds (F3–F7, F5/F6+M43) this role could not independently re-run — no `Bash`
  access this session — so it is reported as told rather than re-measured.
- Mutation spot-check: **46 mutations applied across phase B and the fixer round, 43 caught**, 3
  accepted survivors (M16 equivalent mutant, M39 `fsync` untestable by construction, M44 low-stakes
  gap needing a seam that doesn't exist).
- `yaml_serde` 0.10.7 / `chrono` 0.4.45 / `toml` 1.1, decided 2026-08-30, evidence in
  `runs/stage1/yaml-crate.md`.
- Windows rename verification: `std::fs::rename` confirmed to map to `MoveFileExW` with
  `MOVEFILE_REPLACE_EXISTING` on Windows 11 (build 26200) / `1.97.1-x86_64-pc-windows-msvc`,
  2026-08-30 — see Plan-doc corrections.

## Plan-doc corrections applied at this seal

Applied per this task's explicit authorization; the ratification for the one locked-decision change
(Identity) is `stage1b.md`'s own text — "Ratified in conversation, 2026-08-31" — not a call made by
this role.

- **`overview.md`**
  - Locked-decisions table: Identity changed from "UUIDv7, in the filename **and** the frontmatter"
    to filename-only, linking `stage1b.md`.
  - Global risks table: the Windows atomic-rename risk marked verified/resolved, with the platform,
    toolchain, and date, and the replacement-vs-never-fails caveat stated explicitly.
  - Stages table: added stage 1b between 1 and 2; stage 4's "Depends on" updated from `1` to `1b`.
  - Rebuild invariant: added the `edited_at` exemption and named the tempting wrong fix (writing mtime
    everywhere) explicitly, so whoever hits the CI failure doesn't reach for it.
- **`orchestration.md`**
  - "First dispatch, concretely": replaced the wave-0-verifier worked example with the wave plan
    stage 1 actually ran (phase A at wave 2, three-task wave 3, wave 4), with the reasoning stated in
    prose above the code block rather than left implicit.
  - Attribution section: corrected the claim that `Assisted-by:` is "the only attribution in this
    repo's history" — every commit also carries a `Claude-Session:` trailer by the user's explicit
    choice. `Co-Authored-By` remains the one trailer genuinely absent, via `attribution.commit = ""`.
- **`stage1.md`** — added a header note pointing to `stage1b.md` for the superseded format sections
  (Note format, Frontmatter, the round-trip acceptance criterion), without rewriting or deleting the
  body — it remains the record of what stage 1 built and what `verification.md`'s verdicts are about.
- **`stage2.md`** — added a note after the lifecycle table stating that F2 (the `to_bytes` write
  hazard) is resolved structurally by stage 1b's single write path, not by a stage-3 constraint on
  `edit`. No "`edit` must call `to_canonical_bytes()`" constraint was added, because that method no
  longer exists as a distinct thing to require.
- **`stage4.md`** — added one Risks-section item: a write must never originate from an index row
  (stage 1's F3 finding, mechanism confirmed by phase B). This is the one surviving sub-item of
  `verification.md`'s three-part "stage4.md should inherit" write-back — see Skipped below for why
  the other two did not land.

### Skipped as superseded by `stage1b.md`, or otherwise moot

- **`stage1.md`'s "Round-trip test as the gate" sentence being stale**, and **"unknown keys are
  preserved verbatim" needing two asterisks for anchor-expansion and `!!`-tag-dropping** — both are
  about the two-path serializer's canonical-emit behavior, which stage 1b deletes outright (one render
  path, and unknown keys are meant to be preserved as original text slices specifically to avoid this
  class of loss). Rather than patch stale specifics into a section that is about to be replaced, the
  header note added to `stage1.md` covers both at once: "the Note format, Frontmatter, and the
  round-trip criterion are no longer current."
- **`stage2.md`'s original write-back ("`Workspace::edit` must use `to_canonical_bytes()`")** — not
  applied as worded, because `to_canonical_bytes()` doesn't survive stage 1b as a distinct method.
  Replaced with the structural note described above.
- **`stage4.md` inheriting F6** (a note may share its frontmatter `id` with another file with no
  complaint) — moot under stage 1b: identity moves to the filename only, so there is no frontmatter
  `id` left for two files to collide on in the way F6 describes. `stage4.md`'s existing Risks section
  already separately names "duplicate `id` across two files" as a copy-paste hazard on the (now
  filename-based) id, so nothing was lost by not adding F6's framing on top of it.
- **`stage4.md` inheriting F1** (filename-parser unification) — moot; already fixed within stage 1
  itself (`c293114`), not something for stage 4 to inherit.
- **§U4's out-of-scope list "should name `fsync`"** — `dispatch.md`'s U4 ruling text itself says to
  "name [untested things] in the run log," not to edit the ruling. Applied by naming `fsync` as
  untested-by-construction in this log (What shipped, and the Timings mutation-count note) rather than
  editing `dispatch.md`, which is a point-in-time dispatch record, not a specification doc this role
  edits.

### Task 3 — CI `continue-on-error`

Already applied. `.github/workflows/ci.yml`'s `acceptance` job has no `continue-on-error` key (i.e.
it is blocking, the default), and its comment already states "is now **blocking**, per `dispatch.md`
U8's 'flipped to blocking at seal.'" No edit was needed or made.

### Noticed, not authorized to fix

- `overview.md`'s "Open questions" section still says "stage 1 makes [the filename slug] a
  `workspace.toml` knob defaulting to bare UUID." Stage 1b removes that knob (`[notes] filename` is
  gone) and replaces it with a creation-time slug option. This is stale in the same way the write-backs
  above were, but it was not on the list of ratified corrections for this seal, so it is left as-is
  and flagged here.
- `runs/stage1/dispatch.md`'s dispatch log has no entries for waves 3, 4, or any of the three fix
  rounds — see Deviations and the Waves table above. Worth closing before stage 4's dispatch log is
  written, so this role isn't reconstructing model attribution from a routing table again next time.
