# The markdown crate — decision and verification

**Decision.** `markdown` 1.0.0 (markdown-rs). Added to `Cargo.toml` on 2026-08-31, replacing the
hand-rolled `split_fences` in `frontmatter.rs`.

This is the record `stage1b.md`'s open question asked for — "Where the markdown-crate decision is
recorded" — resolved toward opening `runs/stage1b/` early rather than appending to `yaml-crate.md`.
`runs/stage1/` is a sealed audit trail and this is a stage-1b decision.

## The rule that makes it safe

> **Parse with the crate, slice with your own offsets, never call its renderer.**

An AST crate is safe in a tool whose premise is not touching the user's bytes; a *rendering* crate
would not be. comrak and friends will happily normalize list markers, emphasis characters and
hard-break spacing on the way out, and `01a03d5b-7a8b-…` exists in the fixture corpus specifically
to make that failure visible.

`ParseOptions.constructs.frontmatter` yields a `Node::Yaml` whose `position` carries byte offsets
into the source. Those offsets partition the file and `jot` does the partitioning itself, so the
body is a slice of the original text and never passes through an emitter. "Plain markdown,
untouched" is structural rather than earned.

## Why this crate rather than `pulldown-cmark`

`stage2.md` already requires a markdown parser for `[[uuid]]` extraction, so the dependency was
arriving regardless. In markdown-rs's mdast a paragraph's `[[uuid|label]]` arrives as a single
`Text` node with byte offsets, while fenced and inline code are distinct node kinds to skip;
`pulldown-cmark` splits the same link across eight events and would need reassembly. One crate
covers both stages.

**Weight.** `markdown v1.0.0` plus one transitive crate, `unicode-id`. Confirmed against
`Cargo.lock`.

## What was verified, and how

Empirically, against the pinned crate, on Windows 11 (build 26200) with
`1.98.0-x86_64-pc-windows-msvc`, 2026-08-31. Every case below was run before any implementation
code was written, and every one is now pinned by a test rather than by this document.

| Input | Result |
| --- | --- |
| LF | `Yaml@0..16`, reconstitutes |
| CRLF | `Yaml@0..18`, carriage returns intact in the interior |
| leading BOM | `Yaml@3..19` — the BOM is a three-byte prefix *outside* the span, not consumed |
| no trailing newline | `Yaml@0..16`, body is the remainder |
| empty block (`---\n---`) | `Yaml@0..7`, empty interior |
| `---` rule in the body | `Yaml@0..16` then a separate `ThematicBreak`; the rule stays in the body |
| fence with trailing whitespace | tolerated, and the whitespace is inside the span |
| fence with a trailing tab | tolerated (matches stage 1's `is_fence`) |
| indented `  ---` | **no Yaml node** — correctly not frontmatter |
| block scalar + nested mapping | one `Yaml` node spanning both |
| `relation:root: <uuid>` | one key; the colon is not an indicator unless followed by whitespace |

In every case `doc[..start] + doc[start..end] + doc[end..]` reconstitutes the file exactly. That is
now an acceptance criterion, run over the whole fixture corpus.

## Two behaviours the stage doc got slightly wrong

Both were found by running the crate rather than by reading about it, and both are worked around in
`frontmatter.rs` with a test pinning the workaround.

### 1. The reported span stops *before* the closing fence's line terminator

`stage1b.md` describes the partition as `doc[start..end]` being "the fenced block, both fences
included", with the body at `doc[end..]`. True as far as it goes — but `end` lands on the last `-`
of the closing fence, so the body would begin with that fence's newline. Every note in the vault
would gain a leading blank line, and `01a03d56-2b3c-…`, whose body starts on the very next line,
would gain one it never had.

`split_document` extends the block over that terminator. Pinned by
`the_block_owns_the_closing_fence_terminator_and_the_body_starts_at_a_line`, which asserts both the
crate's raw span and the adjusted one, so a future release changing the convention fails loudly in
one place rather than quietly everywhere.

### 2. The "no fence" / "unterminated fence" distinction cannot be inferred from the AST

`stage1b.md` proposes recovering §U10's two distinct errors from the parser's output: "an
unterminated `---` parses as a `ThematicBreak` at offset 0, a file with no fence does not."

**That inference is unsound.** An *indented* `  ---` also parses as a `ThematicBreak` at offset 0,
and it is not an unterminated fence — stage 1's `split_fences` reported it as "no fence", correctly.
Classifying from the AST would have silently changed that verdict.

`classify_missing_block` reads the first line of the source instead, skipping at most one BOM, and
asks whether it trims to exactly `---`. That reproduces stage 1's behaviour on every case its tests
covered, and it is a decision the code makes rather than an inference about someone else's parser.
Pinned by `an_indented_fence_and_an_unterminated_fence_look_identical_to_the_parser`, which asserts
the identical AST shape *and* the two different errors — so it fails if either half stops being true.

## What it does not solve

The crate hands back the block's outer boundary and nothing else. The interior is still
`yaml_serde`'s, so the unknown-key problem is untouched by this change. That is where the stage's
risk actually lived, and it is handled by `top_level_key_spans` and `agree_or_refuse` in
`frontmatter.rs`.

## Errors from `to_mdast`

`markdown::to_mdast` returns `Result`, but only MDX syntax can populate the error side, and
`parse_options()` does not enable MDX. Rather than add an unreachable error variant, a failed parse
falls through to `classify_missing_block`, which yields whichever fence error the source actually
shows. Noted here because "we ignore an error path" deserves to be written down somewhere.
