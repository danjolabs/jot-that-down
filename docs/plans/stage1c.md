# Stage 1c — Typed frontmatter schema

**Status.** Empty. To be filled in from `docs/conversation/stage2-schema.md` once that
conversation settles.

**Why this stage exists.** The design discussion that opened stage 2 turned into a rewrite of
stage 1's schema decisions rather than a preamble to the index: roles become declared types
rather than hardcoded key literals, `workspace.kind` goes away, and `relation:root` stops being
a thing a file carries. Doing that after SQLite lands would mean carrying a schema migration
through the index as well, so it is sequenced before stage 2.

**Not in this stage.** SQLite. This stage changes what a workspace declares and what a note file
holds; stage 2 caches the result.
