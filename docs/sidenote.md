# Full-text search: how it's usually implemented

Since you've deferred FTS, here's the landscape for when it comes back — with the SQLite-derived-index constraint in mind.

## The standard mechanism: an inverted index

Almost every FTS implementation is the same idea underneath:

- **Analysis pipeline** — take the raw text and produce tokens
  - `tokenize` → split on word boundaries
  - `normalize` → lowercase, strip accents/punctuation (`Café` → `cafe`)
  - `stem` or `lemmatize` → `running`/`ran` → `run` (optional, language-dependent)
  - `stopword removal` → drop `the`, `a`, `is` (optional; hurts phrase search)
- **Inverted index** — a map from `token → list of (document_id, position, frequency)`
  - The inversion is the whole trick: a `LIKE '%foo%'` scan is O(corpus); an index lookup is O(matches)
  - Positions are what make phrase queries (`"jot that down"`) and proximity queries (`NEAR`) possible
- **Ranking** — score matched documents so the best come first
  - `BM25` is the modern default (term frequency, damped; inverse document frequency; document-length normalization)
  - `TF-IDF` is its predecessor; still seen in older systems
  - Field weighting: a title hit counts more than a body hit

## Your case: SQLite FTS5

This is the obvious fit and it's built into SQLite — no extra dependency.

- You create a **virtual table** that holds the tokenized content, and query it with `MATCH`
- Rank with the built-in `bm25()` function, which accepts per-column weights
- Keep it **contentless or external-content** (`content=''` / `content='notes'`) so the text isn't stored twice — it points back at your `notes` table by rowid
- Tokenizers worth knowing:
  - `unicode61` — default, reasonable for Latin scripts
  - `porter` — adds English stemming on top of another tokenizer
  - `trigram` — enables substring/infix matching (`%foo%`-like) and, importantly, **works for CJK** where whitespace tokenization fails
- Snippet/highlight helpers (`snippet()`, `highlight()`) give you the search-result excerpt for free

Given you're Korean-speaking, the tokenizer choice is not incidental — `unicode61` will treat a Korean sentence as one giant token. `trigram` is the usual pragmatic answer without pulling in a custom tokenizer (ICU, or a morphological analyzer like mecab-ko).

## The rebuild problem, specific to your architecture

You've established markdown-as-source-of-truth, SQLite-as-derived-index. FTS sharpens the tradeoff:

- **Index size** — a contentless FTS5 index over a whole vault of note bodies is often comparable to the source text. Fine locally; worth knowing.
- **Reindex cost** — your "scan the directory, rebuild the DB" path now has to re-tokenize every note, not just parse frontmatter. Mitigations:
  - Store `(path, mtime, size)` or a content hash per note and only reindex what changed
  - Do it incrementally on file-watcher events rather than as a startup scan
- **Sync triggers** — FTS5 external-content tables need `INSERT`/`UPDATE`/`DELETE` triggers on the base table (or explicit `'delete'`/`'rebuild'` commands) to stay consistent. Getting this wrong produces silent stale results, which is the worst failure mode for search.

## Alternatives, and when they'd apply

| Approach | When it fits |
| --- | --- |
| `LIKE` / `GLOB` scan | Toy scale (<1k notes). Zero setup, no ranking, no CJK problem. Honestly viable for a personal vault for a long time. |
| SQLite FTS5 | Your default. Embedded, ranked, incremental. |
| Tantivy / Lucene / Bleve | Standalone index libraries. Better analyzers and faceting, but a second store to keep in sync — hard to justify next to a SQLite you already maintain. |
| Ripgrep-shelling-out | Genuinely reasonable for a CLI/TUI surface: no index at all, always fresh, fast on a local dir. Loses ranking and the desktop app's incremental-as-you-type feel. |
| Embeddings / vector search | Semantic ("notes about the thing I was worried about last spring") rather than lexical. Complementary to FTS, not a replacement — the usual pattern is hybrid: BM25 + vector, fused by reciprocal rank fusion. |

## What I'd suggest when you get to it

Given that this app's whole premise is capturing loose ideas you can't pre-classify, search is eventually load-bearing — it's the thing that replaces the folder structure you're deliberately not building. Two notes on sequencing:

1. Don't build it now, but **leave the `notes` table with a stable integer rowid** so an external-content FTS5 table can attach later without a migration.
2. When you do build it, the **hybrid lexical+semantic** direction is unusually well-matched to your use case — "I remember thinking something like this" is exactly the query lexical search fails at, and exactly the one your app invites.
