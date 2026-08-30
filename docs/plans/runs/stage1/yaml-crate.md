# YAML crate, time crate, and the stage-1 dependency set

**Task.** T2.1, stage 1 wave 2.
**Decided.** 2026-08-30.
**Evidence.** A throwaway spike run on Windows 11 against `tests/fixtures/vault/` with
`rustc 1.97.1 (8bab26f4f 2026-07-14)`. The spike does not land; its outputs are quoted below.

| Decision | Choice |
| --- | --- |
| YAML | **`yaml_serde` 0.10.7** |
| Time | **`chrono` 0.4.45**, `default-features = false`, features `clock, serde, std` |
| Config format | **`toml` 1.1** (not `toml_edit`) |

---

## What the U1 ruling removed from this decision

The dispatch ruling on U1 is *preserve on read, normalize on edit*, implemented as: **parse retains
the original frontmatter block byte-for-byte, fence to fence, and re-serializing an unmodified note
re-emits those exact bytes.** The emitter never runs on that path.

**Byte-perfect emitter round-trip of arbitrary hand-written YAML is therefore NOT a requirement of
the YAML crate, and was not evaluated as one.** It is worth being explicit about why, because it is
the criterion this decision would have turned on a week ago:

- The acceptance criterion "a hand-written note file parses; re-serializing it changes nothing
  (`git diff` is empty)" is satisfied *structurally*, by retaining bytes. No emitter can fail it,
  and no emitter can earn it either.
- Asking a YAML emitter to reproduce a human's key order, indentation, comments, anchor usage, and
  scalar-style choices is asking for a round-tripping CST, which none of the serde-lineage crates
  are and which `saphyr`/`yaml-rust2` only partially are. Chasing it would have forced the choice
  toward a much heavier library for a guarantee we now get for free.
- The emitter only ever runs on notes **jot itself authors or edits**, where the input is a typed
  `Frontmatter`, not arbitrary YAML. That is a far smaller and far more controllable problem.

What the crate must actually do, in priority order:

1. **Parse faithfully and completely** — nested mappings, sequences, and unknown keys interleaved
   among known ones.
2. **Preserve the order of unknown keys.** The canonical emit path is *known keys in fixed order,
   then unknown keys in their original relative order*. A crate that hands back an unordered map
   destroys that ordering at parse time and it can never be recovered. **This is the sharpest
   selection criterion.**
3. **Emit cleanly and predictably** for notes we author.

---

## Candidates

`serde_yaml` is archived (`0.9.34+deprecated`, last published 2024-03-25) and is out, per the stage
doc. Five live options were evaluated. Figures are from the crates.io API on 2026-08-30.

| Crate | Latest | Published | Recent dl | Owner | Lineage |
| --- | --- | --- | --- | --- | --- |
| `yaml_serde` | 0.10.7 | 2026-08-18 | 1,488,836 | `ingydotnet`, repo `github.com/yaml/yaml-serde` | serde_yaml fork |
| `serde_yaml_bw` | 2.5.7 | 2026-08-15 | 114,744 | `bourumir-wyngs` | serde_yaml fork |
| `serde_norway` | 0.9.42 | 2024-12-21 | 3,335,533 | `cafkafk` | serde_yaml fork |
| `serde_yaml_ng` | 0.10.0 | 2024-05-26 | 5,608,758 | `acatton` | serde_yaml fork |
| `saphyr` | 0.0.12 | 2026-08-18 | 748,563 | `davvid`, `Ethiraric` | yaml-rust2 AST lineage |

`serde_yml` was not evaluated: its provenance dispute is well known and it is not a bet worth
taking on a file format meant to outlive the tool.

---

## Evidence 1 — unknown-key order (the sharpest criterion)

The spike parsed every fixture under `tests/fixtures/vault/` (8 live notes plus
`.jot/.trash/01a03d52-fce0-756a-8944-abff289098e4.md`), extracted the top-level key order from the
raw bytes with a naive line scan, and compared it against the key order the crate's own mapping
type reports.

The discriminating fixture is `01a03d4f-99b0-758b-8ea2-0e460e4bd005.md`, whose unknown keys are
interleaved among the known ones and include a sequence and a nested mapping:

```yaml
id: 01a03d4f-99b0-758b-8ea2-0e460e4bd005
source: obsidian-import          # unknown, before a known key
title: Notes on unknown keys
created_at: 2026-08-26T09:03:42Z
tags:                            # unknown, a sequence
  - migration
  - draft
root: 01a03d4f-99b0-758b-8ea2-0e460e4bd005
location:                        # unknown, a nested mapping
  city: Seoul
  country: KR
priority: 3                      # unknown, an integer
```

Result, identical for all five candidates:

```text
01a03d4f-99b0-758b-8ea2-0e460e4bd005.md   unknown=["source", "tags", "location", "priority"]   order_ok=true
==> yaml_serde 0.10.7   order+emit all_ok = true
==> serde_yaml_ng 0.10.0 order+emit all_ok = true
==> serde_norway 0.9.42  order+emit all_ok = true
==> serde_yaml_bw 2.5.7  order+emit all_ok = true
==> saphyr order all_ok = true
```

All five preserve order: the serde forks because `Mapping` is `IndexMap`-backed, `saphyr` because
its `Mapping` is a linked hash map. **Criterion 2 does not discriminate.** That is a useful
negative result — the fear that motivated it turned out not to apply to any live crate.

Order also survives `#[serde(flatten)]`, which is the form `Frontmatter` will actually use, and
which routes through serde's `Content` buffer where ordering guarantees are easy to lose:

```text
yaml_serde flatten unknown keys = ["source", "tags", "location", "priority"]
```

## Evidence 2 — nested mapping and sequence re-emit

The unknown sub-map was rebuilt and pushed back through each emitter, then re-parsed and compared
for structural equality. `emit_reparse_eq=true` for every fixture and every serde fork. The emitted
form:

```yaml
source: obsidian-import
tags:
- migration
- draft
location:
  city: Seoul
  country: KR
priority: 3
```

And a full canonical emit — known keys in the fixed order, `edited_at` injected as an edit would,
then unknown keys in their original relative order:

```yaml
id: 01a03d4f-99b0-758b-8ea2-0e460e4bd005
title: Notes on unknown keys
created_at: 2026-08-26T09:03:42Z
root: 01a03d4f-99b0-758b-8ea2-0e460e4bd005
edited_at: 2026-08-30T12:00:00Z
source: obsidian-import
tags:
- migration
- draft
location:
  city: Seoul
  country: KR
priority: 3
```

**Criterion 1 and 3 are met by all five.** So the decision came down to what else the spike turned
up.

## Evidence 3 — scalar-style fidelity, where `saphyr` and the serde forks differ

Seventeen adversarial strings were serialized as Rust `String` values and re-read, both by the
emitting crate and cross-checked with a second, independent parser.

```text
case           yaml_serde emit               saphyr emit (explicit Scalar::String)
timestamp      k: 2026-08-26T09:00:00Z       k: "2026-08-26T09:00:00Z"
date           k: 2026-08-26                 k: 2026-08-26
yes            k: yes                        k: "yes"
true           k: 'true'                     k: "true"
null_word      k: 'null'                     k: "null"
tilde          k: '~'                        k: "~"
number_like    k: '0123'                     k: "0123"
float_like     k: '1.5'                      k: "1.5"
colon          k: 'a: b'                     k: "a: b"
hash           k: 'a # b'                    k: "a # b"
empty          k: ''                         k: ""
newline        k: |-\n  a\n  b               k: "a\nb"
unicode        k: 한국어 제목                 k: 한국어 제목
```

Every one of the seventeen re-read as the identical Rust `String`, under both `yaml_serde` and
`saphyr`, in both directions. **No candidate mangles a string.** Notably `yes`/`no` stay strings
because both crates implement the YAML 1.2 core schema, not YAML 1.1.

One trap found and worth recording so nobody repeats it: `saphyr::Yaml::value_from_str` **infers a
type**. Building a value with it turns the string `"true"` into a boolean and `"0123"` into the
integer `123`. The first run of this spike used it and produced a table that made saphyr look
lossy. It is not — but any code using saphyr must construct `Yaml::Value(Scalar::String(..))`
explicitly, and getting that wrong is silent data corruption. The serde forks have no equivalent
footgun because serde carries the Rust type.

## Evidence 4 — strictness on malformed input

Stage 1 wants a hard error on anything the note format does not allow (U9/U10).

```text
input                 yaml_serde                               saphyr
"id: a\nid: b\n"      Err("duplicate entry with key \"id\"")   Ok  — last value silently wins
"a: 1\n---\nb: 2\n"   Err("more than one document")            Ok  — two documents
"\ta: 1\n"            Err("cannot start any token")            Ok
"a: [1, 2\n"          Err("did not find expected ',' or ']'    Err(...)
                           at line 2 column 1, ...")
```

The serde forks reject duplicate keys, multi-document input, and tab indentation; `saphyr` accepts
all three. For jot the strict behavior is the wanted one — a duplicate `id` key silently resolving
to the last occurrence is exactly the class of silent mangling stage 1 exists to prevent — and each
of these lands cleanly on `Error::MalformedYaml` with a message that already carries line and
column.

---

## Decision: `yaml_serde` 0.10.7

`yaml_serde` is the crate `github.com/yaml/yaml-serde` — a maintained continuation of dtolnay's
`serde_yaml` under the **official `yaml` GitHub organization**, published by `ingydotnet` (a YAML
spec author). Repo pushed 2026-08-18; six releases between 2026-01-24 and 2026-08-18. That is the
strongest maintenance and stewardship signal available among the forks: `serde_yaml_ng` (2024-05)
and `serde_norway` (2024-12) are stable but quiet, and `serde_yaml_bw` is a single-maintainer fork
with a tenth of `yaml_serde`'s usage.

Beyond maintenance, four things decided it against `saphyr`, which was the serious alternative:

1. **serde.** `Frontmatter` is a struct with eight typed known fields plus a flattened unknown map.
   With serde that is a derive; with `saphyr` it is hand-written extraction for every field, in
   both directions. Less hand-written mapping code is less places to silently drop a key.
2. **No lifetime parameter.** `saphyr::Yaml<'a>` borrows from the input. `Frontmatter` lives inside
   `NoteMeta`, which every list view is built from, so it would either carry a lifetime through the
   whole domain or pay `into_owned()` everywhere. `yaml_serde::Value` is owned.
3. **Strictness** (Evidence 4).
4. **`saphyr` is `0.0.12`** — six releases in the six weeks to 2026-08-18. That is a moving API to
   pin a note format to. `yaml_serde` is a fork of code that has ~90M downloads of field testing
   behind it.

One consequence of choosing serde: **`yaml_serde::Value` / `Mapping` will appear in `Frontmatter`'s
public API** if T3.1 stores unknowns that way. `indexmap` is provisioned as an alternative
(`IndexMap<String, yaml_serde::Value>`) if T3.1 would rather not; either is fine, and both preserve
order.

### The one thing this crate cannot do, and what T3.1 must do about it

**`yaml_serde` cannot be told to quote a scalar.** Its `Serializer::serialize_str` infers the style
(`src/ser.rs`, `InferScalarStyle`): a string that would parse as a different YAML 1.2 type is
single-quoted, everything else is emitted plain. There is no public option. The same is true of
`serde_yaml_ng`, `serde_norway`, and `serde_yaml_bw` — verified, all four emit
`created_at: 2026-08-26T09:00:00Z` unquoted for a Rust `String`. `saphyr` was the only candidate
that quotes it.

U2 requires timestamps on the canonical path to be **emitted as a quoted string** so no YAML
emitter can reinterpret them as a timestamp type. Under YAML 1.2 this is belt-and-braces — the
spike confirms both `yaml_serde` and `saphyr` re-read the unquoted form as the string
`"2026-08-26T09:00:00Z"`. It matters for **YAML 1.1** readers: PyYAML and js-yaml's default schema
both resolve an unquoted `2026-08-26T09:00:00Z` to a date object, and Obsidian is a js-yaml
consumer. Interop with hand-edited vaults is a stated premise, so the ruling is right.

**Guidance for T3.1** (not a ruling — T3.1 owns `frontmatter.rs`): do not produce the canonical
block with a single `to_string` over the whole `Frontmatter`. Emit the known-key prefix under your
own control, then append the unknown-key block from `yaml_serde::to_string` over the unknown
`Mapping`. The known keys are a closed set of eight and every one is trivially safe to emit:

- `id`, `reply_to`, `root`, `quote` — a hyphenated UUID is a plain scalar under every schema and is
  never ambiguous. Emit bare.
- `created_at`, `edited_at`, `trashed_at` — emit `key: "<rfc3339>"` with literal double quotes.
  RFC 3339 UTC second-precision is `[0-9]`, `-`, `:`, `T`, `Z` only: it contains no `"` and no `\`,
  so a double-quoted YAML scalar wrapping it needs no escaping and is unconditionally valid. This
  is not a hack, it is a provable property of the value domain.
- `title` — arbitrary user text; emit it through `yaml_serde` (as a one-pair mapping) so escaping
  and style selection stay the crate's problem.

That is the whole cost of choosing serde over `saphyr`, and it is about thirty lines with a test
per bullet.

### Revisit triggers

Reopen this decision if any of: `yaml_serde` goes a year without a release; a note format need
arises that requires comment or blank-line preservation *inside* a rewritten frontmatter block (the
byte-retention path covers the unmodified case, not the edited one); or `saphyr` reaches 0.1 with a
serde bridge and a stable `Mapping` API.

---

## Time crate: `chrono` 0.4.45

U2 fixes the canonical output: **RFC 3339, UTC, `Z` suffix, second precision, emitted as a quoted
string.** In memory, `created_at` / `edited_at` / `trashed_at` are typed values. Spike:

```text
chrono   "2026-08-26T09:00:00Z"       -> to_rfc3339_opts(Secs, true) = "2026-08-26T09:00:00Z"
time     "2026-08-26T09:00:00Z"       -> format(Rfc3339)             = "2026-08-26T09:00:00Z"
jiff     "2026-08-26T09:00:00Z"       -> to_string()                 = "2026-08-26T09:00:00Z"

chrono   "2026-08-26T09:00:00.123456Z" -> Secs      = "2026-08-26T09:00:00Z"   <- truncates
jiff     "2026-08-26T09:00:00.123456Z" -> to_string = "2026-08-26T09:00:00.123456Z"
time     "2026-08-26T09:00:00.123456Z" -> Rfc3339   = "2026-08-26T09:00:00.123456Z"

chrono   "2026-08-26T18:00:00+09:00"   -> "2026-08-26T09:00:00Z"     <- normalizes to UTC
jiff     "2026-08-26T18:00:00+09:00"   -> "2026-08-26T09:00:00Z"
```

`chrono::DateTime<Utc>::to_rfc3339_opts(SecondsFormat::Secs, true)` is the **only** one-call form
among the three that produces the required canonical string exactly, including truncating a
fractional-second input that arrived from a hand-written or Obsidian-written note. `time` and
`jiff` both keep the fraction on their default RFC 3339 output and need a hand-built format
description (`time`) or a `strftime` pattern (`jiff`) to comply — an extra place for the canonical
form to drift.

Secondary reasons:

- Stages 4–6 render human-facing timelines ("today", "yesterday"), which needs local time.
  `chrono`'s `Local` is first-class; `time` deliberately ships no timezone database and
  `OffsetDateTime::now_local()` fails outright on some platforms.
- The historical objection to `chrono` — RUSTSEC-2020-0159, the `localtime_r` soundness issue — is
  resolved and has been since 0.4.20. From `chrono-0.4.45/src/lib.rs:472`: *"Since version 4.20
  chrono no longer uses `localtime_r`, instead using Rust code to query the …"*. We pin ≥ 0.4.45.
- `jiff` is the better-designed library of the three and is the likely choice for a greenfield
  project in 2027, but it is `0.2.x` pre-1.0, and it buys us nothing here: jot stores UTC
  instants and does no calendar arithmetic and no zoned scheduling, which is the whole area where
  `jiff` is genuinely better.

Features: `default-features = false, features = ["clock", "serde", "std"]`. This drops `oldtime`
(the deprecated `time 0.1` re-export) and `wasmbind`, leaving a dependency tree of `num-traits`,
`serde`, and `windows-link`.

Note for T3.3: `chrono`'s serde impl writes `DateTime<Utc>` with **subsecond precision** —
`last_opened = "2026-08-30T07:08:43.250180Z"` in the spike's registry entry. U5 specifies RFC 3339
UTC for `last_opened`; if second precision is wanted there too, serialize via
`to_rfc3339_opts(SecondsFormat::Secs, true)` into a `String` field rather than relying on the
derive.

## Config format: `toml` 1.1, not `toml_edit`

`workspace.toml` is written once by `init` and only read thereafter; stage 1 never rewrites it.
`workspaces.toml` (the registry) is entirely machine-owned. Neither needs comment- or
formatting-preserving edits, which is the only thing `toml_edit` buys. `toml`'s serde interface is
what `init`, `open`, and the registry all want, and `toml::Table` is a `BTreeMap`, so output is
deterministically ordered without extra work.

Verified: `toml::to_string_pretty` over the stage-1 manifest struct reproduces the on-disk contract
in `stage1.md` exactly, section order included:

```toml
schema_version = 1

[workspace]
id = "01a0517f-c571-7e41-9c51-bc3995c267a5"
kind = "jot"
name = "Thoughts"

[notes]
filename = "uuid"
```

If stage 7's declared-schema work ever needs to edit a manifest in place while keeping a user's
comments, add `toml_edit` then; `toml` already sits on the same parser, so it is not a new tree.

---

## The landed dependency set

Root `[workspace.dependencies]`, referenced from `crates/jot-core/Cargo.toml` with
`workspace = true`. Wave 3 runs three agents in three worktrees who **cannot add dependencies**, so
this is provisioned against what T3.1, T3.2, T3.3 and T4.1 each need.

| Crate | Version / features | Who needs it |
| --- | --- | --- |
| `uuid` | `1.26`, `serde`, `v7` | T3.1 `NoteId`; T3.2 filename parsing; T4.1 workspace id; T3.3 registry keys; `error.rs` |
| `serde` | `1.0.229`, `derive` | everyone |
| `yaml_serde` | `0.10.7` | T3.1 |
| `toml` | `1.1` | T3.3 registry, T4.1 manifest |
| `thiserror` | `2.0.20` | `error.rs` |
| `directories` | `6.0.0` | T3.3 |
| `chrono` | `0.4.45`, `clock,serde,std`, no defaults | T3.1 timestamps, T3.3 `last_opened` |
| `indexmap` | `2.14`, `serde` | T3.1, only if it prefers `IndexMap` over `yaml_serde::Mapping` for unknown keys |
| `tempfile` | `3.27` (dev) | T3.2, T3.3, T4.1 |

Verified by compiling a probe against exactly this set on Windows:

- `uuid::ContextV7` and `uuid::Timestamp::now(&ctx)` are reachable with features `["serde", "v7"]`
  alone — **T3.1 needs no extra feature to satisfy U6.** For information, on this build
  `Uuid::now_v7()` was already strictly increasing across 2000 mints in a tight loop, as was
  `Uuid::new_v7(Timestamp::now(&ContextV7::new()))`; U6 still asks T3.1 to verify rather than
  assume, and the context type is there either way.
- `directories::ProjectDirs::from("", "danjolabs", "jot")` resolves to
  `C:\Users\<user>\AppData\Roaming\danjolabs\jot\config` on Windows — matching the U5 ruling.
- The manifest and registry structs round-trip through `toml` (output above).

`indexmap` is provisioned deliberately even though it may end up unused: an unused manifest entry
costs nothing, and a wave-3 agent blocked on a missing dependency costs a serialized amendment
across three worktrees.

Not provisioned, on purpose: `insta` and `assert_cmd` (`overview.md` wants them from stages 4–5,
nothing in stage 1 renders or shells out) and any atomic-write crate — see the note in the
breakdown that `std::fs::rename` on Windows already maps to `MoveFileExW` with
`MOVEFILE_REPLACE_EXISTING`; T3.2 verifies that rather than taking a dependency on faith.
