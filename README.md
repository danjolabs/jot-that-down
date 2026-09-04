# Jot That Down

A personal capture tool over a directory of markdown files.

Every note is a `<uuid>[_slug].md` file whose **filename is its identity** and whose frontmatter
carries its title and its relations. Notes reply to notes and quote notes, so an idea can grow a
structure *while it is being written* instead of demanding a folder decision before the thought is
finished.

The files are the truth. Everything else — the index, the caches, the views — is derived and
disposable, and the vault stays readable by any editor, greppable by any tool, and versionable by
git.

## Status

**Usable from the command line.** Stages 1–4 of [`docs/plans/overview.md`](docs/plans/overview.md)
are built.

| | |
| --- | --- |
| ✅ Vault, frontmatter round-trip, atomic writes | stages 1 and 1b |
| ✅ Note lifecycle, thread algebra, links | stage 2 |
| ✅ `jot` CLI | stage 3 |
| ✅ SQLite index, incremental sync, deterministic rebuild | stage 4 |
| ⏳ TUI, desktop app, user-declared schema fields | stages 5–7 |

The version is `0.0.<stage>-<letter>` while this is a prototype — the patch is the stage number, the
letter is a round of change made after that stage landed, and a bare `0.0.<stage>` means the stage is
sealed. `jot --version` therefore says which stage the binary on your PATH came from. Nothing is
published to crates.io at these versions; see the versioning convention in
[`docs/plans/overview.md`](docs/plans/overview.md).

Stages 2 and 3 were built **before** stage 4, because nothing in them needs a database: threads,
reference resolution, links and prefix resolution are all functions of the set of notes in the
vault, and the index is a speed layer over that set. `jot-core` read the whole vault into memory
instead, behind the same public API SQLite now sits behind.

Stage 4 made that layer real without changing the API above it. A command no longer rescans the
vault: `(size, mtime_ns)` decides whether a file is worth opening and a blake3 hash decides whether
to believe it, so a note nothing has touched is answered from the index. At 5,000 notes on Windows
that is the difference between **~700 ms per command and ~80 ms**, and the 700 ms is not gone —
it is what the first run after `rm .jot/index.db` still costs.

The index is derived and disposable. Deleting it loses nothing, `jot index rebuild` recreates it
from the files, and nothing is ever stored there that the markdown does not already say.

## Install

Rust 1.97 or newer.

```sh
cargo install --path crates/jot-cli     # puts `jot` on your PATH
```

Or, inside a checkout with [direnv](https://direnv.net):

```sh
cargo build --release                   # .envrc puts target/release on PATH
```

## Quick start

```sh
jot workspace new ~/notes    # create a vault
cd ~/notes                   # commands below discover it by walking up

jot new -t "First thought" -m "Something worth keeping."
echo "captured from a pipe" | jot new
jot new -t "A title alone"   # a title is a note; the body is optional
jot new                      # opens $EDITOR; an untouched buffer cancels

jot list                     # thread roots, newest first
jot list --flat              # replies too
jot show <id>
```

Threads are the point:

```console
$ jot thread 01a05a59-c2c
01a05a59-c2c  Jot that down
├─ 01a05a59-c2d0  Untitled
│  └─ 01a05a59-c2d5  Untitled
│     ├─ 01a05a59-c2d9  Untitled
│     └─ 01a05a59-c2df  Untitled
└─ 01a05a59-c2e  Second branch
```

`--path` prints the single line through one note; `--segments` prints the thread cut into chains at
its branch points.

Editing and deleting:

```sh
jot edit <id> -t "New title"      # or -m "new body", or --no-title
jot edit <id>                     # no flags: opens $EDITOR on the whole note

jot remove <id>                   # trash — reversible, never cascades
jot restore <id>
jot purge <id>                    # irreversible; confirms
```

**Command names are full words, with short aliases**: `list`/`ls`, `remove`/`rm`,
`workspace`/`ws`. Both spellings work.

## Things worth knowing

**Short ids are computed, not truncated.** `jot` prints the shortest prefix that is unique in your
vault, so whatever it prints you can paste back. It is not a fixed eight characters: a UUIDv7's
leading bits are a timestamp, so notes captured in the same minute need more of the id to tell
apart. `--long` gives full UUIDs; `--json` always does.

**Nothing guesses.** An ambiguous id lists the candidates and exits non-zero. So does a workspace
name that matches two vaults.

**Trash never cascades.** Trashing a note with replies moves exactly one file; the replies stay live
and show a trashed-parent marker. Trashing is a file move into `.jot/.trash/` — the directory *is*
the state, so a restore leaves the file byte-identical.

**Frontmatter keys jot does not understand are preserved verbatim** through every write. A vault
imported from another tool keeps its keys.

**Links** are `[[<uuid>]]` or `[[<uuid>|label]]` in a note's body, and are ignored inside code
fences and inline code. `jot links <id>` shows outgoing links, backlinks, and quotes.

**`--json` on every read command**, with a documented stable shape — see
[`docs/cli-json.md`](docs/cli-json.md).

## Where notes live

```text
~/notes/
  .jot/
    workspace.toml     # identity and config
    .trash/            # trashed notes keep their filename
    tmp/               # staging for atomic writes
    index.db           # derived and disposable; appears on the first note
    .gitignore         # index.db*, tmp/
  01a03d4c-….md
  01a03d4d-…_first_thoughts.md
```

The slug after the UUID is decorative — the reader ignores it, so renaming a title does not move the
note.

## Environment

| Variable | Effect |
| --- | --- |
| `JOT_WORKSPACE` | The vault to act on, between `--workspace` and directory discovery. |
| `JOT_REGISTRY` | Where the workspace registry lives, overriding the OS config directory. |
| `EDITOR` / `VISUAL` | The editor `jot new` and `jot edit` hand off to. |
| `NO_COLOR` | Any value disables colour. |

Which vault a command acts on is resolved in one fixed order — `--workspace`, then `JOT_WORKSPACE`,
then a `.jot/` found by walking up from the working directory, then the registry's current
workspace. `--verbose` says which rule won, because a note captured into the wrong vault is the one
mistake this tool could make quietly.

## Development

```sh
cargo test --workspace                              # unit and integration
cargo test -p jot-acceptance --features stage4      # executable acceptance criteria
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

`crates/jot-acceptance` is the executable form of each stage's acceptance criteria. It is
deliberately owned by a different agent than the implementation, so the tests cannot be weakened to
fit the code — see [`docs/plans/orchestration.md`](docs/plans/orchestration.md).

### Dependencies

`Cargo.toml` is a manifest and stays one; the reasoning lives here. Every version is a caret
requirement, so `cargo update` moves the lock and only a `0.x` crate needs the manifest edited to
gain a minor release.

| Crate | Why this one, rather than a default |
| --- | --- |
| `blake3` | `notes.content_hash`. The `(size, mtime_ns)` fast path is a *hint*; the hash is what makes trusting it safe. Named in `stage4.md`'s schema. |
| `chrono` | Times. `default-features = false` because the crate's default pulls in more than dates. |
| `directories` | The OS config directory, for the workspace registry. |
| `indexmap` | Insertion-ordered maps. Frontmatter key order is part of the file, so a `HashMap` would silently reorder someone's note. |
| `markdown` | markdown-rs, chosen in [`docs/plans/runs/stage1b/markdown-crate.md`](docs/plans/runs/stage1b/markdown-crate.md): an AST parser and never a renderer, so a note's body stays a slice of the original bytes. Stage 3 needs it for `[[uuid]]` extraction anyway. |
| `rusqlite` | The index. `bundled` compiles SQLite from vendored C rather than linking a system one, so every platform builds the same version and a machine without `libsqlite3` is not a special case. |
| `serde`, `serde_json`, `toml`, `yaml_serde` | The four formats a vault touches. `serde_json`'s `preserve_order` keeps `notes.raw` in the key order the file writes, which is what `Record::undeclared` is ordered by. |
| `thiserror` | The error taxonomy in `jot-core`. `anyhow` is for the binaries; the two never swap. |
| `uuid` | `v4` for workspace ids, `v7` for note ids. The asymmetry is argued in `workspace::Manifest::id`: only a *note* has its creation time and its ordering read back out of its identity, and v7's timestamp prefix is what makes a short id long. |

The YAML, TOML and time choices were settled together in
[`docs/plans/runs/stage1/yaml-crate.md`](docs/plans/runs/stage1/yaml-crate.md) — read it before
changing any of them.

**Two of the groupings in `Cargo.toml` are rules, not tidiness.**

*Surfaces only* — `anyhow`, `clap`, `clap_complete` may not reach `jot-core`. `anyhow` is the
binaries' error type by convention (`overview.md`), and a `clap` type in a core signature would
make the domain depend on how one surface happens to present it.

*`serde_json` is the deliberate exception*, and sits with the core crates rather than the surface
ones. `jot-core` uses it for the index's `raw` column — a JSON projection of the frontmatter block
— inside a module that is private to the crate. The rule above is about **signatures**: no
`serde_json` type appears in a public `jot-core` signature, so the domain does not depend on it.

## Documentation

| | |
| --- | --- |
| [`docs/plans/overview.md`](docs/plans/overview.md) | Locked decisions, architecture, the core API. **Read first.** |
| [`docs/plans/stages/stage<N>.md`](docs/plans/stages) | One file per stage, self-contained. |
| [`docs/plans/runs/`](docs/plans/runs) | What each run actually did, what it found, what it cost. |
| [`docs/cli-json.md`](docs/cli-json.md) | The `--json` contract and exit codes. |

## License

MIT.
