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
jot list [--flat] [--since <when>] [--limit <n>]        # alias: ls
jot show <id> [--raw]
jot thread <id> [--tree | --path | --segments]
jot edit <id>
jot remove <id>        # trash; alias: rm
jot restore <id>
jot purge <id>         # irreversible; confirms
jot trash              # list what is in the trash
jot search <query> [--since] [--until]
jot links <id>         # backlinks and quoted-by
jot open <id>          # hands off to the desktop app (stage 6)
jot workspace list | use <id> | --id <id> | --name <name>
              | add <path> | new <path> [--kind jot|plain]
              | remove <id> | --id <id> | --name <name> | prune   # alias: ws, and ls/rm within
jot index status | rebuild
```

### Names are words; the short forms are aliases

**The full word is the command; `ls`, `rm` and `ws` are `visible_alias`es.** This was the other way
around until dogfooding, and the reason for the flip is that abbreviations are only obvious to
someone who already knows the tool. `jot remove` reads correctly to a person who has never run it;
`jot rm` is muscle memory for a person who has. Both work, both appear in `--help`, and nothing is
taken away — but the name that *teaches* is the one the help text leads with.

It also removes a genuine ambiguity now that there are two `remove`s: `jot remove` trashes a note and
`jot workspace remove` unregisters a workspace. Spelled out, the difference is legible; as `jot rm`
and `jot ws rm` it is a diff of two characters.

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

> **Finding, and a design change: a fixed-width short id does not work here.**
>
> Git's eight characters work because a SHA is random from its first bit. **A UUIDv7's leading 48
> bits are a millisecond timestamp**, so eight hex characters cover only the top 32 of them — one
> shared value per roughly 65 seconds. Randomness does not begin until character 13.
>
> The consequence is not subtle. The first `jot ls` run against a real vault rendered every row as
> the same string, `01a05a57`, and none of those ids resolved: they were all ambiguous. Notes
> captured in the same minute share a prefix, and notes captured in the same minute are exactly the
> ones you refer to by short id ("jot a thought, reply to it"). A surface that prints an id it
> cannot accept back is worse than one that prints full UUIDs.
>
> **Implemented instead:** `shortid::abbreviate` computes the shortest prefix that is unique within
> the set an id is displayed beside, floored at 8 — which is what git actually does, rather than what
> git appears to do. It lives in core rather than in this crate because there are two callers and
> both are UUIDv7: note ids in a vault, and **workspace ids in the registry**, which collide the same
> way when two workspaces are created in one minute. A burst of same-millisecond captures naturally produces longer ids, which is honest: those
> notes really are that similar. The result is always a genuine prefix, so anything printed can be
> handed straight back.
>
> Two consequences worth carrying forward:
>
> - **The width is a property of the set, not of the id.** It can change when a note is written.
>   So it is a display convenience only and never appears in `--json`, which always carries full
>   UUIDs. Stages 5 and 6 inherit this rule.
> - **Workspace ids are v4, not v7** — see below. That is the same finding applied at its source:
>   where an id has no need to carry a timestamp, not putting one in it is better than abbreviating
>   around it.
> - **An id the vault does not hold cannot be abbreviated at all** — a dangling `reply_to`, a link to
>   a purged note. There is nothing for it to be unique against, so it renders as a full UUID.
>   Pinned by `a_printed_short_id_can_always_be_handed_straight_back`.

### Output

- **Human** by default: title or `Untitled`, relative time, short id, and the first line of the body.
  `jot workspace list` follows the same shape — `<id>  <name>  <path>` — because it answers
  the same question a note listing does: *which one of these do I mean?* Found during dogfooding:
  without an id the listing is genuinely ambiguous, since the registry keys on workspace id and a
  vault deleted and remade therefore appears twice under one name.
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
- [ ] `JOT_REGISTRY` moves the registry itself. Added after a Windows-only CI failure: `directories`
      resolves the config dir through `SHGetKnownFolderPath` there, a syscall rather than an
      environment variable, so `XDG_CONFIG_HOME` and `HOME` isolate on Linux **only** and this
      suite was writing into the developer's real `%APPDATA%` registry — and reading other tests'
      workspaces back out of it. The escape hatch `registry::default_path`'s own docs anticipated;
      also useful for a portable install or separate work and personal registries. Policy lives in
      the CLI, since `jot-core` keeps `directories` behind one function and takes explicit paths
      everywhere else.
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

### `$EDITOR`, and what it can and cannot carry back

The handoff writes a temp file **outside the vault**, launches the editor, and feeds the result back
through `Workspace::edit`. Editing the note file in place would be simpler and would break the seam:
the surface would be writing into the vault and core would find out afterwards. Going through the
write path means an edit made in Vim gets the same unknown-key preservation, no-op detection, and
re-slugging as one made with `-t`.

The editor is handed the whole note as a **template**: every key the workspace schema declares is
present, and the ones the note does not carry appear as empty placeholders (`title:`). An empty block
tells you nothing about what a note in this vault may hold. The placeholders round-trip to nothing —
`key:` is YAML null, null reads as absent, and an absent key is never written back — so a template
left alone produces exactly the file it would have produced without one.

What comes back is parsed with the crate's own parser, so a mangled block fails loudly and **the
draft is kept and named** rather than discarded.

For `jot new` the buffer is authoritative for everything a new note may declare, `relation:reply_to`
included: choosing a parent while writing is normal and, unlike an edit, re-parents nothing.
`relation:root` is the exception — it is assigned by `create` and never taken from input, or
"assigned once, never recomputed" would be a suggestion rather than an invariant. A value typed
there is ignored, and warned about.

The limit, which is real and applies to **`jot edit`**: `Edit` carries `title`, `body`, and `quote`,
so a change to an unknown frontmatter key made in the editor is not applied — unknown keys are
preserved from the *file*, not from the buffer. `jot new` does not have this limit: `Draft::extra`
carries the parsed block through, so a schema-declared custom field filled in at creation survives. The same is true of `relation:reply_to`, deliberately — re-parenting is not an edit.
Both cases are detected and warned about on stderr rather than silently dropped. Making them
editable means either widening `Edit` to carry arbitrary keys or letting the surface write, and
neither is worth doing before dogfooding says it matters.

The command is split on whitespace so `EDITOR="code --wait"` works, and is **not** run through a
shell — passing a user-controlled string to `sh -c` would make a filename with a space a command
injection.

## Acceptance

> Built. Every criterion below is covered by `crates/jot-cli/tests/cli.rs` except the two marked
> **open**, which need a large vault and a week of real use respectively.

- `echo "a thought" | jot new` writes a file, indexes it, prints its short id, in under 100 ms warm.
  *(Written and printed: covered. The **timing is open** — there is no index yet, so `jot new` pays a
  full vault scan at startup. Meaningless at tens of notes and the thing to measure in stage 2.)*
- `jot new --reply <prefix>` produces a note whose `root` matches its parent's.
- `jot thread` on the worked example renders the tree from stage 3 correctly.
- An ambiguous prefix lists candidates and exits `4`.
- `jot remove` then `jot list` hides the note; `jot trash` shows it; `jot restore` brings it back.
  *(Amended: this originally ended "with `trashed_at` gone from the frontmatter". Stage 1b deleted
  that key — the directory the file sits in **is** the state — so the criterion named a field that
  no longer exists. What replaces it is stronger and is tested: a trash-and-restore round trip
  leaves the file byte-identical, because nothing about trashing is written into it at all.)*
- `jot list --json | jq` round-trips; the schema is documented in [`docs/cli-json.md`](../cli-json.md).
- Every command works from a subdirectory of the workspace.
- One full week of real capture with no data loss and no manual index repair. **Open** — this is the
  dogfooding criterion and it can only be closed by time.

## Risks

- **Startup latency.** `sync()` on every invocation is the whole risk, and it is **larger than this
  entry assumed**: with stage 2 deferred there is no `files` fast path at all, so every invocation
  reads and reparses every note in the vault. Fine at hundreds of notes, and unmeasured above that.
  Measure at 10k during stage 2; if the fast path is not enough, add a `--no-sync` flag for hot paths
  and a background sync, but do not optimize before measuring.
- **The wrong workspace.** A note captured into the wrong vault is quietly lost. Make the resolution
  order visible and make `jot ws` output unambiguous.
- **Feature creep from dogfooding.** Everything you want during that first week should land in a list,
  not in the code. Stages 5 and 6 are where they get built.

## Two findings from dogfooding the workspace commands

### `jot workspace remove` and `prune`, and why `prune` confirms

Nothing could unregister a workspace: a stale entry could only go by hand-editing
`workspaces.toml`. Two commands close that.

`workspace remove <name|id>` unregisters one. It says *"the directory is untouched"* every time,
unprompted, because there are now two `remove`s and the other one moves a file — someone arriving
from `jot remove` should not have to wonder which kind this is. If the removed entry was current,
the surface clears `current`: `Registry::remove` deliberately leaves a dangling one, which is right
for a library and wrong to leave for the next bare `jot new`.

`workspace prune` unregisters every entry whose directory is gone — and **confirms first**, which
looks like ceremony and is not. "Stale" is `!path.exists()`, and an external drive that is merely
*unmounted* is indistinguishable from a vault that was deleted. Pruning it discards the registration
for a vault whose notes are perfectly fine, and getting it back means finding the path again. The
prompt lists the candidates and says so; `--yes` skips it for scripts.

### Selecting a workspace: an id by default, a name only when asked for

The registry keys on workspace **id** — correct, since the id lives in `workspace.toml` and survives
the folder being moved. But that means two entries can share a name: two `notes` directories under
different parents, or one vault deleted and remade. `ws use <name>` originally matched on name alone
and took the first hit, which made the second workspace **unreachable** and silently picked one of
them. That is the "never guess between your things" rule broken in the one place that decides where
notes get captured.

The first fix accepted a name **or** an id prefix, trying the name first. That removed the silent
pick but introduced a subtler fault: **what the argument means depended on what happened to be
registered.** `workspace use notes` was a name lookup right up until someone registered a vault
whose id began `notes`, at which point the same command meant something else. A selector whose
interpretation shifts under you is the same class of problem as an id that stops resolving.

Settled shape — the two are an exclusive, required pair, enforced by a clap `ArgGroup`:

```text
jot workspace use <ID>          # a bare argument is an id, or a unique prefix
jot workspace use --id <ID>     # the same, said explicitly
jot workspace use --name <NAME> # a name, which may match several
```

`workspace remove` takes the same selector, from the same type, so the two cannot drift apart. The
resolver is split into `resolve_workspace_by_id` and `resolve_workspace_by_name` so neither can
quietly reacquire the other's fallback. A name matching several entries still exits 4 with the
candidates listed; nothing guesses.

**The cost, recorded:** selecting by name is the common case — a name is what `workspace new` prints
and what people remember — and it now needs a flag, while the rarer disambiguation-by-id case gets
the bare argument. The trade was made deliberately, for a bare argument that means exactly one thing
regardless of what is registered. If dogfooding says the flag is friction, flipping which of the two
is bare is a one-line change to the `ArgGroup` and nothing else moves.

### Workspace ids are UUIDv4; note ids stay UUIDv7

A note id is v7 because two things read the timestamp back out of it: `created_at` is decoded from
the identity, and id order *is* creation order, which sibling ordering, the timeline, and keyset
pagination all rest on.

A workspace id is asked for none of that. Nothing in the crate decodes its time or sorts on it — the
only thing that referenced its version was a test asserting the version. Minting it v7 anyway cost
two things and bought nothing:

- **Short ids stopped working.** Two workspaces created in one minute shared their first eight hex
  characters, so `jot ws ls` had to widen the abbreviation to tell them apart — the same problem as
  notes, in a place with no reason to have it. With v4 the id is random from its first bit and eight
  characters separate every workspace anyone will have.
- **It leaked a date.** The id is written into `workspace.toml`, which people commit to git.

Reading is unaffected: `open` parses any UUID version, so vaults created before this keep their v7
ids and stay correct. Pinned by `a_workspace_id_of_any_uuid_version_still_opens`.
