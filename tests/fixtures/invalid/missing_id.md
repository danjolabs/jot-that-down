---
created_at: 2026-08-26T09:10:29Z
root: 01a03d54-cf80-7c22-9d17-4f2a5b6c7d8e
---

`id` is one of the three hard-required keys (`id`, `created_at`, `root`). This file has the other
two but not `id`, and must be rejected with a distinct error naming the path.
