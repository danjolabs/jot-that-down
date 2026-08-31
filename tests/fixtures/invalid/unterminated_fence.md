---
title: A file whose fence is never closed
relation:root: 01a03d53-ae70-7b52-a1c0-2c9c4c1c6a2e

The opening fence above is never closed by a second `---` line, so the body and the frontmatter
block are indistinguishable. This must be a *different* error from "no fence" — markdown-rs
reports no frontmatter node for both, so the two are told apart from the source's first line.
