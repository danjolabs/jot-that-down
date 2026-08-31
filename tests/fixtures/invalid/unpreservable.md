---
2026: a top-level key that is not a string
title: A block this version cannot slice
---

The slicer that captures each unknown key's source lines reads a key as the text before the first
`:` — always a string. `yaml_serde` reads `2026` as a number, so the two disagree about what this
block's keys *are*, and a key the slicer cannot name is a key whose bytes it cannot promise to
carry through a write.

Stage 1's byte-replay path preserved such a block whether or not it understood it. One rendering
write path cannot, so this is refused loudly rather than mangled quietly — which is the only
option consistent with a tool whose premise is not touching the user's bytes.

The triggers are exotic by construction, and that is the point rather than a weakness: a
non-string key, a duplicated key, an explicit `?` key, an anchor whose alias would be reordered
ahead of it. None of them appear in frontmatter anyone writes, and none of them were handled by
stage 1 either — the difference is that stage 1 could afford not to notice.
