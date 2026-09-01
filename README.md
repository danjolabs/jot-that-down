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

**Usable from the command line.** Stages 1–3 of [`docs/plans/overview.md`](docs/plans/overview.md)
are built.

| | |
| --- | --- |
| ✅ Vault, frontmatter round-trip, atomic writes | stages 1 and 1b |
| ✅ Note lifecycle, thread algebra, links | stage 2 |
| ✅ `jot` CLI | stage 3 |
| ⏳ SQLite index | stage 4, **deliberately deferred** — see below |
| ⏳ TUI, desktop app, user-declared schema fields | stages 5–7 |

Stages 2 and 3 were built **before** stage 4. Nothing in them needs a database: threads, reference
resolution, links, and prefix resolution are all functions of the set of notes in the vault, and the
index is a speed layer over that set. So `jot-core` reads the vault into memory instead, behind the
same public API SQLite will sit behind later.

The practical consequence: **every command rescans the whole vault.** That is instant at hundreds of
notes and is what stage 4 exists to fix. It is a performance gap, not a correctness one.

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
jot new                      # opens $EDITOR; an empty body cancels

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
cargo test -p jot-acceptance --features stage1b     # executable acceptance criteria
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

`crates/jot-acceptance` is the executable form of each stage's acceptance criteria. It is
deliberately owned by a different agent than the implementation, so the tests cannot be weakened to
fit the code — see [`docs/plans/orchestration.md`](docs/plans/orchestration.md).

## Documentation

| | |
| --- | --- |
| [`docs/plans/overview.md`](docs/plans/overview.md) | Locked decisions, architecture, the core API. **Read first.** |
| [`docs/plans/stage<N>.md`](docs/plans) | One file per stage, self-contained. |
| [`docs/plans/runs/`](docs/plans/runs) | What each run actually did, what it found, what it cost. |
| [`docs/cli-json.md`](docs/cli-json.md) | The `--json` contract and exit codes. |

## License

MIT.
