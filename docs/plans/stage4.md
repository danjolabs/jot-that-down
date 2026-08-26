# Stage 4 — CLI

**Goal.** `jot` becomes a tool you actually use every day. This is the first stage with a user.

**Why now.** The CLI is the thinnest possible shell over stage 3 — a few hundred lines of argument
parsing and formatting. It proves the data model against real notes weeks before any pixel exists,
and it is the surface that makes the app scriptable, which was the stated reason for wanting more
than a desktop app in the first place.

**Start dogfooding the day this ships.** Everything after it should be reordered by what real use
teaches. Plans written past this point are guesses; the CLI is where they start becoming evidence.

## Command surface

```text
jot new [-t <title>] [--reply <id>] [--quote <id>] [-m <body>]
jot ls [--flat] [--since <when>] [--limit <n>]
jot show <id> [--raw]
jot thread <id> [--tree | --path]
jot edit <id>
jot rm <id>            # trash
jot restore <id>
jot purge <id>         # irreversible; confirms
jot trash              # list what is in the trash
jot search <query> [--since] [--until]
jot links <id>         # backlinks and quoted-by
jot open <id>          # hands off to the desktop app (stage 6)
jot ws ls | use <name> | add <path> | new <path> [--kind jot|plain]
jot index status | rebuild
```

### Input paths for `jot new`

Three, in priority order — each one exists because it removes friction from a different context:

1. `-m "text"` — one-liner, scriptable.
2. stdin when piped — `echo "..." | jot new`, `pbpaste | jot new`. This is the integration point.
3. `$EDITOR` otherwise — opens a template with the frontmatter pre-filled, saves on exit, discards on
   an empty body.

A note that is never written is the failure mode this app exists to prevent. `jot new` with a piped
body should complete in well under 100 ms, and that budget is a real constraint on stage 2's `sync()`.

### Short ids

Git-style prefixes everywhere an id is accepted. Ambiguity lists the candidates with titles and dates
and exits non-zero; it never guesses. Every command that prints an id prints the short form, and
`--long` prints full UUIDs for scripting.

### Output

- **Human** by default: title or `Untitled`, relative time, short id, and the first line of the body.
- **`--json`** on every read command, with a stable documented shape. This is what makes the tool
  compose with everything else you use, and it costs almost nothing to add now versus later.
- Colors via a standard detection path, honoring `NO_COLOR` and non-tty output.

### `jot thread` rendering

The two projections from stage 3, made visible:

```text
$ jot thread 01a03d20 --tree           # form 2: segments
01a03d20  Jot that down
├─ 01a03d21  ...
│  └─ 01a03d22  ...
│     ├─ 01a03d24  ...
│     └─ 01a03d25  ...
└─ 01a03d23  ...

$ jot thread 01a03d25 --path           # form 1: the one line through the focus
01a03d20 → 01a03d21 → 01a03d22 → 01a03d25
```

`--tree` is the default. `--path` walks ancestors from the focus, which is the "read one branch"
case.

## Work

- [ ] `clap` derive command tree matching the surface above; `jot` with no arguments prints help
      until stage 5 makes it launch the TUI.
- [ ] Workspace resolution order: `--workspace` flag → `JOT_WORKSPACE` env → `discover()` from cwd →
      registry's current. Print which one was chosen under `--verbose`; ambiguity about *where a note
      landed* is the worst possible CLI bug here.
- [ ] `sync()` before every read command; report problems from `SyncReport` on stderr without
      blocking the command.
- [ ] `$EDITOR` integration: temp file, wait, detect no-change and empty-body.
- [ ] `--json` for `ls`, `show`, `thread`, `search`, `links`, `trash`, `ws ls`.
- [ ] Confirmation on `purge`, skippable with `--yes` for scripts.
- [ ] Exit codes: `0` ok, `1` runtime error, `2` usage, `3` not found, `4` ambiguous prefix.
- [ ] Shell completions for PowerShell, bash, zsh, fish via `clap_complete`.
- [ ] Integration tests against a temp workspace with `assert_cmd`; snapshot the human output with
      `insta` so formatting changes are deliberate.
- [ ] `jot index status` — note count, last sync, and any problems the scanner is holding.

## Acceptance

- `echo "a thought" | jot new` writes a file, indexes it, prints its short id, in under 100 ms warm.
- `jot new --reply <prefix>` produces a note whose `root` matches its parent's.
- `jot thread` on the worked example renders the tree from stage 3 correctly.
- An ambiguous prefix lists candidates and exits `4`.
- `jot rm` then `jot ls` hides the note; `jot trash` shows it; `jot restore` brings it back with
  `trashed_at` gone from the frontmatter.
- `jot ls --json | jq` round-trips; the schema is documented in `docs/`.
- Every command works from a subdirectory of the workspace.
- One full week of real capture with no data loss and no manual index repair.

## Risks

- **Startup latency.** `sync()` on every invocation is the whole risk. Measure it at 10k notes; if
  the fast path is not enough, add a `--no-sync` flag for hot paths and a background sync, but do not
  optimize before measuring.
- **The wrong workspace.** A note captured into the wrong vault is quietly lost. Make the resolution
  order visible and make `jot ws` output unambiguous.
- **Feature creep from dogfooding.** Everything you want during that first week should land in a list,
  not in the code. Stages 5 and 6 are where they get built.
