# Pre-stage-4 refactor — typed frontmatter schema

**Status.** Empty. To be filled in from `docs/conversation/stage2-schema.md` once that
conversation settles.

**Not a numbered stage.** It has no number because it delivers nothing new — it reworks decisions
stage 1 and stage 1b already made, so that stage 4 can be built on them. Everything it touches
already exists.

**Why it exists.** The design discussion that opened stage 4 turned into a rewrite of stage 1's
schema decisions rather than a preamble to the index: roles become declared types rather than
hardcoded key literals, `workspace.kind` goes away, and `relation:root` stops being a thing a file
carries. Doing that after SQLite lands would mean carrying a schema migration through the index as
well, so it is sequenced before stage 4.

**Not in scope.** SQLite. This changes what a workspace declares and what a note file holds;
stage 4 caches the result.
