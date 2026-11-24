# Jot That Down

A personal capture tool over a flat folder of markdown files.

Every note is a `<id>.md` file whose **filename is its identity**, and whose frontmatter is typed by
the workspace's `schema.json` — like a database that works offline, where each file is both a page
and a row. Meaning lives in that typed layer, not in folders, so a note never has to be filed
before the thought is finished. See [`docs/plans/redesign.md`](docs/plans/redesign.md).

The files are the truth. Everything else — the index, the caches, the views — is derived and
disposable, and the workspace stays readable by any editor, greppable by any tool, and versionable
by git.

## Status

**Usable from the command line.** The schema-first redesign
([`docs/plans/redesign.md`](docs/plans/redesign.md)) is built: `workspace.toml` and `schema.json`,
strict writes, per-field problems on read, field commands, and an index keyed by id. The terminal
browser from the earlier micro-blog design was removed, and the desktop app stays deferred.

The version is `0.0.<stage>-<letter>` while this is a prototype; see the versioning convention in
[`docs/plans/overview.md`](docs/plans/overview.md). `jot --version` says which build is on your
PATH. Nothing is published to crates.io at these versions.

A command does not rescan the workspace: `(size, mtime_ns)` decides whether a file is worth opening
and a blake3 hash decides whether to believe it, so a note nothing has touched is answered from the
index. The index is derived and disposable: delete `.jot/index.db` and the next command rebuilds
it from the files.

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
jot workspace new ~/notes    # writes workspace.toml and schema.json
cd ~/notes                   # commands below discover it by walking up

jot new -t "First thought" -m "Something worth keeping."
echo "captured from a pipe" | jot new -t "From a pipe"
jot new -t "A title alone"   # a title is a note; the body is optional
jot new -t "Seedling" --set status=seed   # any key; the value is read as YAML
jot new                      # opens $EDITOR on every schema key; untouched cancels

jot list                     # newest first, by the created_at role
jot show <id>                # frontmatter in schema order, problems beside their keys
```

Editing and deleting:

```sh
jot edit <id> -t "New title"      # or -m "new body", --set key=value, --unset key
jot edit <id>                     # no flags: opens $EDITOR on the whole note
jot remove <id>                   # deletes the file; confirms (--yes skips)
```

When the schema changes, the notes do not — explicit commands rewrite them:

```sh
jot check                         # notes with problems, stray files; exits 5 on problems
jot field fill status seed        # set a key on every note that lacks it
jot field remove status           # remove a key from every note; confirms
jot field rename title headline   # in every note, schema.json, and [roles]
```

**Command names are full words, with short aliases**: `list`/`ls`, `remove`/`rm`,
`workspace`/`ws`. Both spellings work.

## Things worth knowing

**jot never writes a note that fails `schema.json`.** A command that would is refused with the
reasons. In `$EDITOR`, a refused buffer is reopened on the same text with the reasons written at
the top of its frontmatter; quit without saving and the draft is kept, its path printed. Nothing
typed is lost.

**Reading never fixes anything.** A note that breaks the schema — `counter: two` where a number is
declared — stays listed, readable and editable, with the problem shown beside the key.

**Roles give keys their meaning.** `workspace.toml`'s `[roles]` maps `document:title` and
`document:created_at` to keys of `schema.json`. With no title role a note lists as `Untitled`; with
no created_at role `jot list` orders by modification time and says so.

**Short ids show the end of the id.** A UUIDv7 starts with a timestamp, so notes written in the
same minute share their first ten or more characters. `jot` prints the first four, `...`, and the
shortest tail that is unique in your workspace, at least four: `01a0...eefa`. Paste it back as
printed, or type only the tail as `...eefa`; a prefix or the full id works too. `--long` gives full
ids; `--json` always does.

**`jot new` opens `$EDITOR` on the whole schema.** Every key `schema.json` declares is listed, in
order, with `created_at` and any `default` filled in and a YAML comment saying what the key needs:

```yaml
---
title: # required
created_at: 2026-11-24T08:47:08Z # required
status: # seed | growing | done
counter:
tags:
---
```

Keys left empty are dropped rather than written as `null`, and the comments are not kept. The
default `schema.json` requires a title.

**Nothing guesses.** An ambiguous id lists the candidates and exits non-zero. So does a workspace
name that matches two workspaces.

**Delete is delete.** `jot remove` removes the file; there is no trash, and nothing cascades. Recovery
is git's job, or the OS's.

**`--json` on every read command**, with a documented stable shape — see
[`docs/cli-json.md`](docs/cli-json.md).

**An old-format vault** (`.jot/workspace.toml`, `<id>_<slug>.md`) is refused with a message saying
what changed. There is no converter yet, and nothing is touched.

## Where notes live

```text
~/notes/
  workspace.toml       # [id] format, [roles]
  schema.json          # JSON Schema for the frontmatter
  .jot/                # derived only: index.db, tmp/, .gitignore
  01a03d4c-….md        # a note is exactly <id>.md
  README.md            # anything else is a stray: reported, never a note
```

## Environment

| Variable | Effect |
| --- | --- |
| `JOT_WORKSPACE` | The workspace to act on, between `--workspace` and directory discovery. |
| `JOT_REGISTRY` | Where the workspace registry lives, overriding the OS config directory. |
| `EDITOR` / `VISUAL` | The editor `jot new` and `jot edit` hand off to. |
| `NO_COLOR` | Any value disables colour. |

Which workspace a command acts on is resolved in one fixed order — `--workspace`, then
`JOT_WORKSPACE`, then a `workspace.toml` found by walking up from the working directory, then the
registry's current workspace. `--verbose` says which rule won, because a note captured into the
wrong workspace is the one mistake this tool could make quietly.

## Development

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Core is tested against `tests/fixtures/workspace/` through its public API; the CLI with
`assert_cmd`. How changes get made is in [`docs/plans/orchestration.md`](docs/plans/orchestration.md).

### Dependencies

`Cargo.toml` is a manifest and stays one; the reasoning lives here. Every version is a caret
requirement, so `cargo update` moves the lock and only a `0.x` crate needs the manifest edited to
gain a minor release.

| Crate | Why this one, rather than a default |
| --- | --- |
| `blake3` | `notes.hash`. The `(size, mtime_ns)` fast path is a *hint*; the hash is what makes trusting it safe. |
| `chrono` | Times. `default-features = false` because the crate's default pulls in more than dates. |
| `directories` | The OS config directory, for the workspace registry. |
| `indexmap` | Insertion-ordered maps. Frontmatter key order is part of the file. |
| `jsonschema` | Validates frontmatter against `schema.json` (draft 2020-12). Default features off: they bring an HTTP client, and `$ref` resolves local files only. |
| `markdown` | markdown-rs, for one job: finding the frontmatter fences. An AST parser and never a renderer, so the body stays a slice of the original bytes. |
| `notify` | The watcher behind `Sync`. Lives in core so no surface grows its own. |
| `rusqlite` | The index. `bundled` compiles SQLite from vendored C, so every platform builds the same version. |
| `serde`, `serde_json`, `toml`, `yaml_serde` | The formats a workspace touches. `serde_json`'s `preserve_order` keeps `schema.json`'s `properties` in declaration order, which is the frontmatter key order. |
| `thiserror` | The error taxonomy in `jot-core`. `anyhow` is for the binary; the two never swap. |
| `uuid` | `v7` for note ids. |

The YAML, TOML and time choices were settled in
[`docs/runs/stage1/yaml-crate.md`](docs/runs/stage1/yaml-crate.md); read it before changing any of
them.

**Two of the groupings in `Cargo.toml` are rules, not tidiness.**

*Surfaces only* — `anyhow`, `clap`, `clap_complete` may not reach `jot-core`. `anyhow` is the
binaries' error type by convention (`overview.md`), and a `clap` type in a core signature would
make the domain depend on how one surface happens to present it.

*`serde_json` is the deliberate exception*, and sits with the core crates rather than the surface
ones. Since the redesign it **is** part of `jot-core`'s public API: frontmatter is JSON once parsed,
because `schema.json` validates JSON values, so `Fields` is a `serde_json::Map` and `Value` appears
in `Action` and `Event`. A wrapper type would only restate it.

## Documentation

| | |
| --- | --- |
| [`docs/plans/overview.md`](docs/plans/overview.md) | Locked decisions, architecture, the core API. **Read first.** |
| [`docs/plans/redesign.md`](docs/plans/redesign.md) | The detailed spec of the current design. |
| [`docs/plans/stages/stage<N>.md`](docs/plans/stages) | The stages that built the earlier micro-blog design. History, not spec. |
| [`docs/plans/todo/`](docs/plans/todo) | Plans that left the numbered stages. Deferred, not cancelled. |
| [`docs/runs/`](docs/runs) | What each run actually did, what it found, what it cost. |
| [`docs/cli-json.md`](docs/cli-json.md) | The `--json` contract and exit codes. |

## License

MIT.
