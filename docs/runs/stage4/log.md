# Stage 4 — run log

Branch `stage4`, 2026-09-02, one day. Windows 11 build 26200; CI has not run this branch yet, so
every green result below is a Windows result and nothing here is a claim about Linux.

Companion artifacts: [`breakdown.md`](breakdown.md) (the plan), [`dispatch.md`](dispatch.md) (who
did what), [`phase-a.md`](phase-a.md) and [`verification.md`](verification.md) (the verifier).

## Outcome

**Phase B verdict: PASS.** 10/10 criteria, none `UNVERIFIED`. Mechanical gate green:
401 core + 54 CLI + 25 + 16 harness + 51 stage-4 acceptance; `cargo fmt --all --check` clean;
`cargo clippy --workspace --all-targets -- -D warnings` clean.

| | before | after |
| --- | --- | --- |
| warm `sync()`, 10k notes | 648 ms | **60–67 ms** |
| cold rebuild, 10k notes | — | 1.1 s |
| `timeline(50)` | — | 3.7 ms |

Two independent 10k runs — the orchestrator's 67 ms and the verifier's 60.6 ms — agree.

## What actually took the time

Not SQLite. The index made a warm sync possible; two other things made it fast.

- **Enumeration was throwing away metadata it had already read.** A `DirEntry` on Windows carries
  size and mtime (`FindNextFile` returns them), and the scanner was calling `metadata()` per file
  anyway — 10k avoidable syscalls. Handing the stat back from `fs::live_note_entries` took the
  per-file pass from **270 ms to 32 ms**, which is four times what the database itself saved.
- **`set_roots` was issuing 10k no-op `UPDATE`s** on syncs where nothing had happened. Filtering to
  the rows whose root actually moved: 13 ms to nothing.

Worth carrying forward: the first profile pointed at the part of the code that was *new*, and the
cost was in the part that was old and merely called more carefully.

## Decisions taken during the stage

Each is written into `stage4.md` or `overview.md`; listed here with the reasoning that produced it.

1. **The index is the persistence layer under the scan, not a second query engine.** `stage4.md`'s
   `Queries` section is not implemented as SQL — every query in it already exists as a `Snapshot`
   method with 400-odd tests on it, and a second implementation would be a duplicate query engine
   nothing calls. Reversible; the acceptance suite is written either way.
2. **Lazy materialisation.** The database file appears on the first row there is to write. This was
   the verifier's finding, and it is what let every pre-existing test pass unchanged.
3. **`index_meta`, a fourth table.** One row: a digest of the declared frontmatter schema. Forced by
   a real bug — see below.
4. **`serde_json` reaches `jot-core`**, with `preserve_order`, inside the private index module. The
   manifest's surface-only rule is about signatures, and no `serde_json` type appears in one.
5. **`Workspace` loses `Clone, PartialEq, Eq`.** It owns a `Connection`. Nothing cloned or compared
   one. The only public-API fact this stage changes.
6. **`SyncReport` gains `reparsed` and `files_read`.** The verifier asked for the first; the second
   turned out to be the more useful half — "which file was opened" rather than "how much work was
   avoided" — and three acceptance tests rest on it.

## The bug the process caught

`title` and both relation roles are projections **by role**, and a role is assigned by
`workspace.toml`. So renaming the key that carries a role changes what every note means *without
changing one byte of any note*, and the `(size, mtime_ns)` fast path skips past it forever: a vault
whose title key moved from `title` to `heading` would keep answering with titles read under the old
key until something happened to touch each file.

The implementation had this wrong. `a_vault_whose_title_key_is_not_title_fills_the_title_column…`
— written before the implementation existed, by an agent that did not write the implementation —
failed, and that is the entire argument for rule 2 of `orchestration.md` in one test. Fixed with a
blake3 fingerprint of the declared schema in `index_meta`; a mismatch drops the whole index.

## Where the suite was weaker than it looked

The mutation spot-check broke 24 behaviours and the suite caught 19. **Six of the misses were holes
in the tests, not in the code**, and five are now closed. They shared one shape, and it is the thing
most worth carrying into stage 5:

> **A lingering index row is invisible through every query.** The snapshot is built from the files
> the scan found, not from the table, so a row that should have been deleted never appears in an
> answer — until the fast path consults it, and then it speaks with stale content.

Every probe that closes one works the same way: break the file, sync, restore a file with the
original size and mtime but different bytes, and see whether the stale row is believed. Five
survivors were confirmed unreachable rather than untested — including `notes.root_id`, which is
write-only, and which now has a unit test inside `index/` because nothing else in the project would
have noticed that column being wrong.

## Appeals

Two, both filed rather than fixed by editing the suite, both accepted by the verifier.

1. Two tests unlinked `.jot/index.db` while a `Workspace` still held the connection. Windows cannot
   unlink an open file, and no implementation choice changes that. Resolved with `drop(ws)`;
   nothing weakened.
2. `touching_a_file…produces_zero_reparses` asserted the report was quiet. `SyncReport::updated`
   has meant "…or mtime moved" since stage 2 and the pre-stage-4 scanner reports the same for the
   same touch, so the assertion asked stage 4 to change behaviour that "the swap is invisible"
   tells it to preserve. Now asserts `reparsed == 0`, `files_read == 1`, and `updated == [A]`.

## Deviations from `orchestration.md`

The stage ran **hybrid** at the user's direction: orchestrator planning and implementing inline,
verifier subagent for phases A and B. Rule 1 (the orchestrator never writes code) was not held.
Rule 2 was, and caught both a real bug and a process error. See `dispatch.md`.

## Still open

- **CI has not run this branch.** Every result here is Windows. The Linux half of the matrix is
  unexercised, and `fs::live_note_entries` is exactly the kind of change whose behaviour differs:
  `DirEntry::metadata()` is free on Windows and a `stat` on Linux, so the 270 ms saving may not be
  there. It will not be *slower* than the code it replaced, but the number in `stage4.md` is a
  Windows number and should not be quoted as anything else.
- **The human checkpoint is not closed.** `orchestration.md` says stage 4 is done not when tests
  pass but when a week of real capture has gone through it without loss. Nothing above touches
  that.
