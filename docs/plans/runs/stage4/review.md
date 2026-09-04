# Stage 4 — review gate

The third of `orchestration.md`'s three gates. It was **not run at seal**: mechanical and phase B
both have artifacts, this one had none, and the stage was sealed on two gates rather than three.

Written 2026-09-04 at the stage 4 → 5 gate, on Linux 6.18.48, against `stage4` (`0dbeadb`). This is
a late review rather than a review at seal, and the difference matters in one direction: it can no
longer block the seal, so everything below is either a confirmation or an item carried into stage 5.

## Mechanical re-run

`log.md` records a Windows-only gate and names that as the stage's largest open item. Re-run here in
full:

| Check | Result |
| --- | --- |
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | 503 pass |
| `cargo test -p jot-acceptance --features stage4` | 51 pass, 1 ignored (the 10k run) |
| `cargo test -p jot-acceptance --features stage1b` | 120 pass |

**The Linux half of the matrix is now exercised**, which closes the platform half of that open item.
It does not close the CI half — see "Carried into stage 5".

### Performance, on the platform the log could not measure

`log.md` flags `fs::live_note_entries` as the change most likely to differ, because a `DirEntry`
carries size and mtime for free on Windows and costs a `stat` on Linux. Measured, release, 10k
synthetic notes:

| | Windows 11 26200 (recorded) | Linux 6.18.48 (this review) |
| --- | --- | --- |
| cold rebuild | 1.12 s | 654 ms |
| warm `sync()` | 67 ms | 73 ms |
| `timeline(50)` | 3.7 ms | 1.8 ms |

The `stat` concern did not materialise. Warm sync stays in low tens of milliseconds, which is the
criterion's own test of whether the fast path is right.

## Seam check

`orchestration.md` makes this a standing review item from stage 3 onward:
`rusqlite|std::fs` under the surface crates should return nothing but the thin command layer.

`rusqlite` — no hits outside `jot-core`, and the index module is private to the crate. Clean.

`std::fs` — four hits in `crates/jot-cli`, **all dispositioned as compliant**:

| Site | Disposition |
| --- | --- |
| `editor.rs:75`, `:87`, `:104` | The `$EDITOR` temp file. Writes to the *system* temp directory, never the vault, and its own doc comment gives the reason: a crashed editor must not leave a stray file the next scan mistakes for a note. What comes back goes through core's write path. `main.rs`'s module docs already name this as the one deliberate exception. |
| `context.rs:182` | `create_dir_all` on the registry's parent before `Registry::save_to` writes it. The registry is not a vault and not the index; a redirected registry may name a directory that does not exist yet. |

The rule is "surfaces never touch the filesystem **or SQLite**" in the sense of vault and index. On
that reading all four are compliant and the grep is expected to be non-empty forever. Recorded here
so the next stage's review does not re-litigate it from scratch.

## The verifier's four follow-ups

`verification.md` closes with four non-blocking items. All four are done; confirmed by reading the
tree rather than by trusting the log.

1. **`Error::IndexTooNew`'s ten-space message** — fixed, `error.rs:275-280`, and it carries a comment
   explaining why the literal must stay on one line. This was the suite's only red test.
2. **The lazy-materialisation claim in `stage4.md`** — corrected, with the 2026-09-02 ruling written
   in as a decision rather than left as a doc comment only the verifier had read.
3. **The measured numbers in `stage4.md`** — present.
4. **A unit test for `notes.root_id`** — `index/mod.rs:419`. It covers the write-only column *and*
   the `root_id <> ?2` predicate that keeps a quiet sync quiet, which is more than was asked for.

## Findings

Nothing in the implementation. Two process findings, both about the audit trail rather than the code:

- **This gate was skipped at seal, and nothing noticed.** The stage loop has three gates and the
  seal happened on two. `runs/stage1/` has no `review.md` either, so this is a pattern rather than
  an oversight in stage 4 — the artifact list in `orchestration.md` names the file, but nothing
  fails when it is absent. Either the gate is real and something should check for the artifact, or
  it is aspirational and `orchestration.md` should stop listing it as one of three.
- **No stage had ever been tagged.** `git tag` was empty through stage 4, while step 0 of the stage
  loop reads "previous stage tagged". `stage4` now exists, annotated, at `0dbeadb` — the merge
  rather than the seal commit `dac6221`, because the merge is the tree that was verified above and
  the one stage 5 branches from. Stages 1–3 remain untagged; retro-tagging them is optional and was
  not done here.

## Carried into stage 5

- **CI does not run the stage-4 acceptance suite.** `.github/workflows/ci.yml` runs
  `-p jot-acceptance --features stage1b` only. `dispatch.md` records the suite being "flipped to
  blocking at seal", but with no `stage4` step those 51 tests cannot block anything — they run only
  when someone runs them locally. Add a `stage4` clippy + test pair alongside the `stage1b` one.
- **CI has never run on this branch, and structurally cannot.** Triggers are push to `main` /
  `develop` plus `pull_request`; there is no `develop` branch and `prototype` matches nothing. Every
  green result for stage 4, including this review's, came from a developer machine.
- **The human checkpoint is open.** `orchestration.md` says stage 4 is done when a week of real
  capture has gone through it without loss. The dogfood vault holds one note, dated 2026-09-02. No
  agent can close this, and stage 5 depends on stage 4.
