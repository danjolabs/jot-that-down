//! Support code for the acceptance suites. Deliberately std-only.
//!
//! Nothing here may depend on `jot-core`: when the stage feature is off this crate is compiled by
//! `cargo clippy --workspace --all-targets` in the main CI job, and it has to stay clean there
//! whatever state `jot-core` is in. The suites themselves live in `tests/` behind
//! `#![cfg(feature = "stage1")]`.

use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------------------------
// Fixture corpus
// ---------------------------------------------------------------------------------------------

/// Repo root, reached from this crate via `CARGO_MANIFEST_DIR` and the `../..` hop that
/// `breakdown.md` fixes as the way both crates find the shared corpus.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/jot-acceptance is two levels below the repo root")
        .to_path_buf()
}

/// `tests/fixtures/vault/` — the one shared vault. Add to it, never fork it.
pub fn fixture_vault() -> PathBuf {
    repo_root().join("tests").join("fixtures").join("vault")
}

/// `tests/fixtures/invalid/` — deliberately unparseable specimens, kept out of the vault so that
/// enumeration and round-trip walks never trip over them.
pub fn fixture_invalid() -> PathBuf {
    repo_root().join("tests").join("fixtures").join("invalid")
}

/// The fixture whose interpreted keys are deliberately out of schema order. Named because two
/// criteria turn on it: opening it must reorder it, and the reorder must be a fixed point.
pub const NON_SCHEMA_ORDER_NOTE: &str = "01a03d50-2a38-7db1-b33b-20f9083fb0ef.md";

/// The fixture with unknown keys interleaved among interpreted ones, including a nested mapping
/// and a list.
pub const UNKNOWN_KEYS_NOTE: &str = "01a03d4f-99b0-758b-8ea2-0e460e4bd005.md";

/// The fixture carrying all four interpreted keys, already in schema order.
pub const ALL_INTERPRETED_KEYS_NOTE: &str = "01a03d4e-78a0-76bc-be78-8ae41b38eefa.md";

/// The fixture whose unknown `summary` key holds a **block scalar** — named by the acceptance
/// criterion about `summary` surviving a title edit.
pub const SUMMARY_BLOCK_SCALAR_NOTE: &str = "01a03d59-5e6f-7a8b-9c0d-1e2f3a4b5c6d.md";

/// The fixture whose unknown `summary` key holds a **nested mapping** — the other half of that
/// criterion.
pub const SUMMARY_NESTED_MAPPING_NOTE: &str = "01a03d5a-6f7a-7b8c-9d0e-2f3a4b5c6d7e.md";

/// The fixture whose body is full of markdown a renderer would normalize: non-canonical list
/// markers, underscore emphasis, both kinds of hard line break, an indented code block, trailing
/// whitespace, and a literal tab.
pub const MARKDOWN_BODY_NOTE: &str = "01a03d5b-7a8b-7c9d-8e0f-3a4b5c6d7e8f.md";

/// The fixture with an empty body.
pub const EMPTY_BODY_NOTE: &str = "01a03d51-dbd0-7abc-b6d1-c5e69a9e7f65.md";

/// The fixture whose body contains a `---` line at column zero.
pub const FENCE_IN_BODY_NOTE: &str = "01a03d52-6c58-75de-81f8-1b3940ecc38b.md";

/// The fixture whose body starts on the line immediately after the closing fence, and which has
/// no final newline.
pub const TIGHT_BODY_NOTE: &str = "01a03d56-2b3c-7d4e-8f5a-6b7c8d9e0f1a.md";

/// The fixture exercising the `<uuid>_<slug>.md` filename form.
pub const SLUG_FILENAME_NOTE: &str = "01a03d4d-5790-7855-9af5-c362987fc91e_first_thoughts.md";

/// The trashed fixture. It carries no `trashed_at`: from stage 1b the location on disk *is* the
/// state.
pub const TRASHED_NOTE: &str = "01a03d52-fce0-756a-8944-abff289098e4.md";

/// The schema the fixture vault declares, which is also `FrontmatterSchema::jot_default`.
///
/// `relation:root` is deliberately absent: the key was deleted with the pre-stage-4 refactor and a
/// root is derived from `relation:reply_to` at scan time. Fixture notes still carrying one exercise
/// the other half of that decision — it is an undeclared key now, preserved and never migrated.
pub const SCHEMA_KEY_ORDER: [&str; 3] = ["title", "relation:reply_to", "relation:quote_to"];

/// The declared keys of [`ALL_INTERPRETED_KEYS_NOTE`], which carries a legacy `relation:root`
/// between them.
pub const ALL_INTERPRETED_KEYS_NOTE_ORDER: [&str; 4] = [
    "title",
    "relation:root",
    "relation:reply_to",
    "relation:quote_to",
];

/// Every `.md` file the vault contains, live notes plus `.jot/.trash/`, sorted for a stable
/// failure order. This is the set the byte-identical gate must walk in full.
pub fn vault_note_paths() -> Vec<PathBuf> {
    let vault = fixture_vault();
    let mut out = md_files_in(&vault);
    out.extend(md_files_in(&vault.join(".jot").join(".trash")));
    out.sort();
    assert!(
        !out.is_empty(),
        "no fixture notes found under {} — the shared corpus is missing, which would make the \
         round-trip gate vacuously green",
        vault.display()
    );
    out
}

fn md_files_in(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Byte-level assertions
// ---------------------------------------------------------------------------------------------

pub fn read_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

pub fn read_text(path: &Path) -> String {
    String::from_utf8(read_bytes(path))
        .unwrap_or_else(|e| panic!("{} is not utf-8: {e}", path.display()))
}

/// `assert_eq!` on `Vec<u8>` prints two walls of integers. This prints the text and the first
/// differing byte offset instead, because the failure this guards is "one byte moved" and the
/// report has to say which one.
pub fn assert_bytes_eq(actual: &[u8], expected: &[u8], context: &str) {
    if actual == expected {
        return;
    }
    let offset = actual
        .iter()
        .zip(expected.iter())
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| actual.len().min(expected.len()));
    panic!(
        "{context}\nfirst difference at byte {offset} (lengths {} vs {})\n\
         --- actual ---\n{}\n--- expected ---\n{}\n",
        actual.len(),
        expected.len(),
        String::from_utf8_lossy(actual),
        String::from_utf8_lossy(expected),
    );
}

// ---------------------------------------------------------------------------------------------
// Frontmatter inspection, done without a YAML parser
// ---------------------------------------------------------------------------------------------
//
// The suite must be able to say "these keys, in this order" about bytes an implementation
// produced, without adopting the same YAML crate the implementation chose — otherwise a bug in
// that crate is invisible to both sides.

/// The text between the opening `---` line and the next `---` line at column zero, exclusive of
/// both fences. Panics if the block is not there, because every caller has already asserted that
/// the parse succeeded.
pub fn frontmatter_block(bytes: &[u8]) -> String {
    let text = String::from_utf8(bytes.to_vec()).expect("frontmatter output must be utf-8");
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();
    assert_eq!(
        first.trim_end(),
        "---",
        "output does not open with a `---` fence:\n{text}"
    );
    let mut block = Vec::new();
    for line in lines {
        if line.trim_end() == "---" {
            return block.join("\n");
        }
        block.push(line);
    }
    panic!("output has an unterminated frontmatter fence:\n{text}");
}

/// Top-level mapping keys of a frontmatter block, in the order they appear. Nested keys
/// (indented) and sequence items (`- ...`) are not top-level and are skipped.
///
/// The key ends at the first `:` **followed by whitespace or the end of the line**, which is
/// YAML's own rule and is what makes `relation:root` one key rather than a nested mapping. That
/// rule is re-implemented here rather than borrowed from `jot-core`, so that a suite assertion
/// about key order cannot be satisfied by the same mistake the implementation made.
pub fn top_level_keys(block: &str) -> Vec<String> {
    let mut keys = Vec::new();
    for line in block.lines() {
        if let Some(key) = top_level_key_on(line) {
            keys.push(key.to_string());
        }
    }
    keys
}

fn top_level_key_on(line: &str) -> Option<&str> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let first = line.chars().next()?;
    if first.is_whitespace() || first == '#' || line == "-" || line.starts_with("- ") {
        return None;
    }
    let bytes = line.as_bytes();
    let end = bytes.iter().enumerate().find_map(|(i, b)| {
        (*b == b':' && matches!(bytes.get(i + 1), None | Some(b' ') | Some(b'\t'))).then_some(i)
    })?;
    (end > 0).then(|| &line[..end])
}

/// The raw text to the right of a top-level `key:`, trimmed of surrounding spaces.
pub fn top_level_value(block: &str, key: &str) -> Option<String> {
    for line in block.lines() {
        if top_level_key_on(line) == Some(key) {
            let rest = &line[key.len() + 1..];
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// Strips one layer of YAML quoting, either character, so a caller can assert that *some* quoting
/// was applied without pinning which the emitter chose.
pub fn unquote(value: &str) -> Option<&str> {
    for q in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(q) && value.ends_with(q) {
            return Some(&value[1..value.len() - 1]);
        }
    }
    None
}

/// Asserts `subset` appears inside `whole` in the same relative order, reporting both when not.
pub fn assert_subsequence(whole: &[String], subset: &[&str], context: &str) {
    let filtered: Vec<&str> = whole
        .iter()
        .map(String::as_str)
        .filter(|k| subset.contains(k))
        .collect();
    assert_eq!(filtered, subset, "{context}\nfull key order was {whole:?}");
}

// ---------------------------------------------------------------------------------------------
// Directory trees
// ---------------------------------------------------------------------------------------------

/// Every path under `root`, relative, forward-slashed, sorted. Directories included. Used to
/// assert `init` produced *exactly* the on-disk contract and nothing more.
pub fn relative_tree(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    collect_tree(root, root, &mut out);
    out.sort();
    out
}

fn collect_tree(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => panic!("read_dir {}: {e}", dir.display()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .expect("walked path is under root")
            .to_string_lossy()
            .replace('\\', "/");
        out.push(rel);
        if path.is_dir() {
            collect_tree(root, &path, out);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// UUID shape, checked without taking a `uuid` dependency
// ---------------------------------------------------------------------------------------------
//
// Deliberately not using the `uuid` crate: if this crate pinned a different major than `jot-core`
// does, `NoteId`'s inner type and ours would be unrelated types and the suite would start lying
// about what it compared.

/// True for a lowercase hyphenated UUID of `version` whose variant bits are RFC 4122.
///
/// Two versions are in use and the difference is deliberate — see
/// `jot_core::workspace::Manifest::id`. A **note** id is v7, because `created_at` is decoded from
/// it and id order is creation order. A **workspace** id is v4, because nothing reads a time out of
/// it or sorts on it, and a random-from-bit-one id is what keeps a short id short for
/// `jot ws use <prefix>`.
pub fn is_uuid_of_version(s: &str, version: u8) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (i, c) in s.chars().enumerate() {
        let expect_hyphen = matches!(i, 8 | 13 | 18 | 23);
        if expect_hyphen {
            if c != '-' {
                return false;
            }
        } else if !c.is_ascii_hexdigit() || c.is_ascii_uppercase() {
            return false;
        }
    }
    bytes[14] == b'0' + version && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
}

/// True for a lowercase hyphenated UUIDv4 — the shape of a **workspace** id.
pub fn is_uuid_v4(s: &str) -> bool {
    is_uuid_of_version(s, 4)
}

/// True for a lowercase hyphenated UUIDv7 — the shape of a **note** id.
pub fn is_uuid_v7(s: &str) -> bool {
    is_uuid_of_version(s, 7)
}

// ---------------------------------------------------------------------------------------------
// Failure injection between staging and rename (dispatch.md §U4)
// ---------------------------------------------------------------------------------------------

/// Makes `target` un-replaceable by a rename while leaving it readable, so that a call to
/// `atomic_write` gets all the way through staging and fsync and then fails at the rename. The
/// mechanism differs per platform because the thing being tested does:
///
/// * Windows — a read-only destination makes `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` fail with
///   access denied.
/// * Unix — replacing an entry requires write permission on the *containing directory*, so the
///   parent is dropped to `r-xr-xr-x`.
///
/// Undone on drop, so the temp dir can still be cleaned up.
#[must_use]
pub struct BlockedReplacement {
    target: PathBuf,
}

impl BlockedReplacement {
    pub fn new(target: &Path) -> Self {
        #[cfg(windows)]
        {
            let mut perms = fs::metadata(target)
                .unwrap_or_else(|e| panic!("stat {}: {e}", target.display()))
                .permissions();
            perms.set_readonly(true);
            fs::set_permissions(target, perms)
                .unwrap_or_else(|e| panic!("set readonly on {}: {e}", target.display()));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let parent = target.parent().expect("target has a parent directory");
            fs::set_permissions(parent, fs::Permissions::from_mode(0o555))
                .unwrap_or_else(|e| panic!("chmod 555 {}: {e}", parent.display()));
        }
        Self {
            target: target.to_path_buf(),
        }
    }
}

impl Drop for BlockedReplacement {
    // The lint warns that clearing the read-only bit makes a file world-writable on Unix. This
    // arm is `cfg(windows)` and the file is inside a `tempfile::TempDir`; without it the temp dir
    // cannot be removed on Windows.
    #[allow(clippy::permissions_set_readonly_false)]
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            if let Ok(meta) = fs::metadata(&self.target) {
                let mut perms = meta.permissions();
                perms.set_readonly(false);
                let _ = fs::set_permissions(&self.target, perms);
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(parent) = self.target.parent() {
                let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o755));
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Self-tests for the harness
// ---------------------------------------------------------------------------------------------
//
// These run under a plain `cargo test -p jot-acceptance`, with no stage feature and no
// `jot-core`. They exist because a helper that silently returns an empty vector would make the
// acceptance suite green against anything, which is worse than having no suite. The main CI
// `cargo test` never builds this crate (`default-members`), so they cost implementers nothing.

#[cfg(test)]
mod harness_self_tests {
    use super::*;

    #[test]
    fn the_shared_fixture_corpus_is_where_the_breakdown_says_it_is() {
        assert!(
            fixture_vault().is_dir(),
            "{} is not a directory — the `../..` hop from CARGO_MANIFEST_DIR is wrong",
            fixture_vault().display()
        );
        assert!(fixture_invalid().is_dir());
        for name in [
            NON_SCHEMA_ORDER_NOTE,
            UNKNOWN_KEYS_NOTE,
            ALL_INTERPRETED_KEYS_NOTE,
            SUMMARY_BLOCK_SCALAR_NOTE,
            SUMMARY_NESTED_MAPPING_NOTE,
            MARKDOWN_BODY_NOTE,
            EMPTY_BODY_NOTE,
            FENCE_IN_BODY_NOTE,
            TIGHT_BODY_NOTE,
            SLUG_FILENAME_NOTE,
        ] {
            assert!(
                fixture_vault().join(name).is_file(),
                "fixture {name} is missing"
            );
        }
        assert!(
            fixture_vault()
                .join(".jot")
                .join(".trash")
                .join(TRASHED_NOTE)
                .is_file()
        );
    }

    #[test]
    fn the_walk_covers_live_notes_and_the_trash() {
        let paths = vault_note_paths();
        assert!(paths.len() >= 9, "found only {paths:?}");
        assert!(
            paths.iter().any(|p| p.ends_with(TRASHED_NOTE)),
            "the round-trip walk must include .jot/.trash/, per breakdown.md T3.1"
        );
        assert!(
            !paths.iter().any(|p| p.ends_with("workspace.toml")),
            "the walk is over notes only"
        );
    }

    #[test]
    fn frontmatter_block_and_key_extraction_agree_with_the_fixtures() {
        let out_of_order = read_bytes(&fixture_vault().join(NON_SCHEMA_ORDER_NOTE));
        assert_eq!(
            top_level_keys(&frontmatter_block(&out_of_order)),
            vec!["relation:root", "title"],
            "the key ends at the first colon followed by whitespace, so `relation:root` is one \
             key and not a nested mapping"
        );

        let unknown = read_bytes(&fixture_vault().join(UNKNOWN_KEYS_NOTE));
        assert_eq!(
            top_level_keys(&frontmatter_block(&unknown)),
            vec![
                "source",
                "title",
                "tags",
                "relation:root",
                "location",
                "priority"
            ],
            "nested keys (city/country) and list items must not be counted as top level"
        );

        let all_interpreted = read_bytes(&fixture_vault().join(ALL_INTERPRETED_KEYS_NOTE));
        assert_eq!(
            top_level_keys(&frontmatter_block(&all_interpreted)),
            ALL_INTERPRETED_KEYS_NOTE_ORDER.to_vec(),
            "this fixture is already in schema order, which is why it cannot be the only input \
             to the key-order test"
        );
    }

    /// A block scalar's interior lines are continuation, not keys — including one that would
    /// otherwise look like `key: value`. If this helper counted them the suite would report a key
    /// order the file does not have.
    #[test]
    fn key_extraction_ignores_the_interior_of_a_block_scalar() {
        let bytes = read_bytes(&fixture_vault().join(SUMMARY_NESTED_MAPPING_NOTE));
        assert_eq!(
            top_level_keys(&frontmatter_block(&bytes)),
            vec!["title", "relation:root", "summary"]
        );
    }

    #[test]
    fn frontmatter_block_stops_at_the_closing_fence_not_at_a_rule_in_the_body() {
        let bytes = read_bytes(&fixture_vault().join(FENCE_IN_BODY_NOTE));
        let block = frontmatter_block(&bytes);
        assert!(!block.contains("horizontal rule"), "block was:\n{block}");
        assert_eq!(top_level_keys(&block), vec!["relation:root"]);
    }

    #[test]
    fn top_level_value_and_unquote_behave() {
        let block = "title: abc\nrelation:root: 01a03d4c-3680-7c70-aade-6c016dd177d2\n\
                     quoted: \"a value\"\nsingle: 'another'\nbare: plain\n";
        assert_eq!(top_level_value(block, "title").as_deref(), Some("abc"));
        assert_eq!(
            top_level_value(block, "relation:root").as_deref(),
            Some("01a03d4c-3680-7c70-aade-6c016dd177d2"),
            "a key containing a colon must be readable by its whole name"
        );
        assert_eq!(
            unquote(&top_level_value(block, "quoted").unwrap()),
            Some("a value")
        );
        assert_eq!(
            unquote(&top_level_value(block, "single").unwrap()),
            Some("another")
        );
        assert_eq!(
            unquote(&top_level_value(block, "bare").unwrap()),
            None,
            "an unquoted value must not be mistaken for a quoted one"
        );
        assert_eq!(top_level_value(block, "missing"), None);
    }

    #[test]
    fn is_uuid_v4_accepts_a_workspace_id_and_rejects_a_note_id() {
        // The two shapes are not interchangeable, and the checker must not blur them.
        assert!(is_uuid_v4("b4b4856a-e5db-4f9b-bd87-658b0be50741"));
        assert!(
            !is_uuid_v4("01a03d4c-3680-7c70-aade-6c016dd177d2"),
            "that is a v7"
        );
        assert!(
            !is_uuid_v7("b4b4856a-e5db-4f9b-bd87-658b0be50741"),
            "that is a v4"
        );
        // The variant nibble is checked for both.
        assert!(!is_uuid_v4("b4b4856a-e5db-4f9b-0d87-658b0be50741"));
    }

    #[test]
    fn is_uuid_v7_accepts_the_fixture_ids_and_rejects_near_misses() {
        assert!(is_uuid_v7("01a03d4c-3680-7c70-aade-6c016dd177d2"));
        assert!(is_uuid_v7("01a03d51-4b48-72e2-9f30-f180030c06ab"));
        // v4, not v7.
        assert!(!is_uuid_v7("01a03d4c-3680-4c70-aade-6c016dd177d2"));
        // uppercase, wrong length, non-hex, bad variant.
        assert!(!is_uuid_v7("01A03D4C-3680-7C70-AADE-6C016DD177D2"));
        assert!(!is_uuid_v7("01a03d4c-3680-7c70-aade-6c016dd177d"));
        assert!(!is_uuid_v7("zzzzzzzz-3680-7c70-aade-6c016dd177d2"));
        assert!(!is_uuid_v7("01a03d4c-3680-7c70-cade-6c016dd177d2"));
    }

    #[test]
    fn relative_tree_is_recursive_forward_slashed_and_includes_directories() {
        let tree = relative_tree(&fixture_vault());
        assert!(tree.contains(&".jot".to_string()));
        assert!(tree.contains(&".jot/.trash".to_string()));
        assert!(tree.contains(&format!(".jot/.trash/{TRASHED_NOTE}")));
        assert!(tree.contains(&".jot/workspace.toml".to_string()));
        assert!(
            tree.iter().all(|p| !p.contains('\\')),
            "paths must be forward-slashed so the expected tree is one literal on both platforms"
        );
    }

    #[test]
    fn assert_subsequence_rejects_a_reordering() {
        let keys: Vec<String> = ["id", "x", "title", "created_at"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_subsequence(&keys, &["id", "title", "created_at"], "ok");

        let result = std::panic::catch_unwind(|| {
            let bad: Vec<String> = ["title", "id"].iter().map(|s| s.to_string()).collect();
            assert_subsequence(&bad, &["id", "title"], "should fail");
        });
        assert!(
            result.is_err(),
            "assert_subsequence must actually fail on a reordering, or every canonical-order \
             assertion built on it is theatre"
        );
    }

    #[test]
    fn assert_bytes_eq_actually_fails_on_a_one_byte_difference() {
        assert_bytes_eq(b"same", b"same", "identical");
        let result =
            std::panic::catch_unwind(|| assert_bytes_eq(b"a\nb\n", b"a\r\nb\n", "differs"));
        assert!(result.is_err(), "a CRLF/LF difference must be caught");
    }

    #[test]
    fn blocked_replacement_actually_blocks_a_rename_on_this_platform() {
        // The whole weight of the "interrupted write" criterion rests on this injection working.
        // If it silently does not, the criterion test would assert "target unchanged" against a
        // write that never even attempted to land — green, and meaningless. So: prove it against
        // a plain `std::fs::rename`, which is what `atomic_write` reduces to at the last step.
        let tmp = tempfile::tempdir().unwrap();
        let vault = tmp.path().join("vault");
        let staging = tmp.path().join("staging");
        fs::create_dir(&vault).unwrap();
        fs::create_dir(&staging).unwrap();

        let target = vault.join("note.md");
        fs::write(&target, b"original").unwrap();
        let staged = staging.join("staged.tmp");
        fs::write(&staged, b"replacement").unwrap();

        let result = {
            let _blocked = BlockedReplacement::new(&target);
            fs::rename(&staged, &target)
        };

        assert!(
            result.is_err(),
            "the failure injection did not block a rename over the target on this platform; the \
             interrupted-write criterion cannot be tested this way here"
        );
        assert_eq!(fs::read(&target).unwrap(), b"original");
    }

    #[test]
    fn the_fixture_corpus_is_checked_out_with_lf_line_endings() {
        // `.gitattributes` forces this. If a Windows checkout ever normalizes to CRLF, the
        // byte-identical gate would start failing for a reason that has nothing to do with the
        // writer, and the failure would be blamed on stage 1's parser.
        for path in vault_note_paths() {
            let bytes = read_bytes(&path);
            assert!(
                !bytes.windows(2).any(|w| w == b"\r\n"),
                "{} contains CRLF; .gitattributes `text eol=lf` is not in effect",
                path.display()
            );
        }
    }
}
