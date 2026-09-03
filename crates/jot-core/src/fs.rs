//! Filesystem primitives: atomic writes, note-filename parsing, and enumeration.
//!
//! This module is deliberately ignorant of what a note *is*. It moves bytes, reads directory
//! listings, and recognizes a filename shape — nothing here parses frontmatter or constructs a
//! `Note`, and nothing here may `use` [`crate::note`] or [`crate::frontmatter`] (breakdown.md,
//! Shared contracts: "Enumeration returns paths, not notes"). That is what lets the vault's I/O
//! layer be tested without a note format, and the note format be tested without a disk.
//!
//! # Atomic write
//!
//! [`atomic_write`] takes its staging directory as a **parameter** rather than reading it off a
//! `Workspace`. Callers in the vault pass `.jot/tmp/`; tests pass a temp directory. The staging
//! directory must be on the same filesystem as the target, because the last step is a rename and a
//! cross-device rename is a copy at best and an error at worst.
//!
//! The sequence is: create a uniquely-named staged file inside `tmp_dir`, write every byte, flush,
//! `fsync` the staged file, close it, then rename it over the target. Until the rename the target
//! is untouched; after it the target is the new bytes. There is no window in which a reader sees a
//! half-written note.
//!
//! # Rename over an existing file, and Windows
//!
//! `overview.md` records the risk as "`std::fs::rename` fails on Windows when the target exists".
//! **Verified 2026-08-30 on Windows 11 with 1.97.1-x86_64-pc-windows-msvc: that is stale.**
//! `std::fs::rename` maps to `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`, and replacing an
//! existing file succeeds. `std_rename_replaces_an_existing_file_on_this_platform` in this module's
//! tests pins the platform behavior itself, so a regression in `std`, or a move to a platform where
//! this is not true, fails loudly here instead of silently corrupting a vault. No third-party
//! rename crate is needed.
//!
//! Two ways the rename can still legitimately fail on Windows, both of which surface as
//! [`Error::Rename`] with the target intact: the target carries the read-only attribute, or another
//! process holds it open without `FILE_SHARE_DELETE`. `atomic_write` does **not** clear a read-only
//! attribute to force the write through — a note the user marked read-only stays read-only, and the
//! caller gets an error naming both files.
//!
//! # Durability
//!
//! The staged file is `fsync`ed before the rename, so its bytes are on stable storage before
//! anything points at them. On Unix the target's parent directory is `fsync`ed after the rename as
//! well, on a best-effort basis, so the rename itself survives a power loss; a failure of that last
//! step is not reported, because by every observable measure the write has already succeeded.

use std::fs::{DirEntry, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::error::{Error, Result};

/// The extension every note file carries.
const NOTE_EXTENSION: &str = ".md";

/// The separator between the id and the decorative slug in a `<uuid>_<slug>.md` filename.
const SLUG_SEPARATOR: char = '_';

/// Length of a UUID in the canonical hyphenated form, which is the only form a note filename may
/// use.
const HYPHENATED_UUID_LEN: usize = 36;

// ---------------------------------------------------------------------------------------------
// Atomic write
// ---------------------------------------------------------------------------------------------

/// Writes `bytes` to `target`, atomically, staging through `tmp_dir`.
///
/// The target either keeps its previous contents or holds every byte of `bytes`; it is never
/// observed truncated or half-written. `tmp_dir` must be on the same filesystem as `target`. It is
/// created if it does not exist, because `.jot/tmp/` is exactly the sort of directory a temp
/// cleaner removes behind your back, and losing a capture over that would be absurd.
///
/// # Errors
///
/// - [`Error::CreateDir`] if `tmp_dir` is absent and cannot be created — including the case where
///   it exists but is not a directory.
/// - [`Error::Write`] if the staged file cannot be created, written, flushed, or `fsync`ed. The
///   path named is the staged file's, so the message says which staging directory was in play.
/// - [`Error::Rename`] if the rename fails, naming both the staged file and the target.
///
/// On any failure the staged file is removed on a best-effort basis and the target is left exactly
/// as it was.
pub fn atomic_write(target: &Path, tmp_dir: &Path, bytes: &[u8]) -> Result<()> {
    ensure_dir(tmp_dir)?;

    let staged = tmp_dir.join(staged_file_name());

    if let Err(e) = stage(&staged, bytes) {
        remove_best_effort(&staged);
        return Err(e);
    }

    if let Err(source) = std::fs::rename(&staged, target) {
        remove_best_effort(&staged);
        return Err(Error::Rename {
            from: staged,
            to: target.to_path_buf(),
            source,
        });
    }

    sync_parent_dir(target);
    Ok(())
}

/// Writes the staged file and gets it onto stable storage.
///
/// The handle is scoped to this function so that it is closed before the caller renames: Windows
/// can rename a file with an open handle only under the right share mode, and relying on that would
/// be relying on an accident.
fn stage(staged: &Path, bytes: &[u8]) -> Result<()> {
    let write_err = |source| Error::Write {
        path: staged.to_path_buf(),
        source,
    };

    // `create_new` rather than `create`: the staged name is unique, so a file already sitting there
    // is something we do not understand, and clobbering it is not our call.
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(staged)
        .map_err(write_err)?;

    file.write_all(bytes).map_err(write_err)?;
    file.flush().map_err(write_err)?;
    file.sync_all().map_err(write_err)?;
    Ok(())
}

/// A staging filename no concurrent write will pick, and one that enumeration would skip even if
/// the staging directory were the vault root: it starts with a dot and does not end in `.md`.
fn staged_file_name() -> String {
    format!(".jot-{}.tmp", Uuid::now_v7())
}

fn ensure_dir(dir: &Path) -> Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(dir).map_err(|source| Error::CreateDir {
        path: dir.to_path_buf(),
        source,
    })
}

/// Removes a staged file after a failure. Deliberately silent: the error the caller needs is the
/// one that made the write fail, not a follow-on complaint about the debris it left behind.
fn remove_best_effort(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// `fsync`s the directory holding `path`, so a completed rename survives a power loss.
///
/// Unix only, and best-effort. Opening a directory as a file is not portable — on Windows it fails
/// outright — and the durability of a directory entry is not something a caller can act on.
#[cfg(unix)]
fn sync_parent_dir(path: &Path) {
    let Some(parent) = path.parent() else { return };
    let Ok(dir) = File::open(parent) else { return };
    let _ = dir.sync_all();
}

#[cfg(not(unix))]
fn sync_parent_dir(_path: &Path) {}

// ---------------------------------------------------------------------------------------------
// Filename parsing
// ---------------------------------------------------------------------------------------------

/// Extracts the id from a note filename.
///
/// Accepts `<uuid>.md` and `<uuid>_<slug>.md`. The slug is decorative — it exists so that a file
/// explorer is navigable — and is ignored entirely, including when it contains further underscores:
/// only the *first* underscore separates the id from the slug.
///
/// The uuid must be in the canonical hyphenated 36-character form. Hex case is not enforced, so a
/// hand-uppercased filename still loads, but the braced, URN, and unhyphenated forms that `Uuid`'s
/// own parser would otherwise accept are rejected: several spellings of one filename mapping to one
/// note is an ambiguity a vault does not need.
///
/// Returns a bare [`Uuid`], not a `NoteId` — this module may not depend on [`crate::note`]. Callers
/// holding a `NoteId` wrap it themselves.
///
/// # Errors
///
/// [`Error::InvalidNoteFilename`] naming `path`, for every near-miss: a missing or wrong extension,
/// a stem that is not a uuid, a uuid followed by the separator and an empty slug, a path with no
/// filename at all, and a filename that is not valid UTF-8.
pub fn parse_note_filename(path: &Path) -> Result<Uuid> {
    let invalid = || Error::InvalidNoteFilename {
        path: path.to_path_buf(),
    };

    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(invalid)?;
    let stem = name.strip_suffix(NOTE_EXTENSION).ok_or_else(invalid)?;

    let id = match stem.split_once(SLUG_SEPARATOR) {
        // An empty slug is a near-miss, not a note: `<uuid>_.md` is a filename someone truncated.
        Some((_, "")) => return Err(invalid()),
        Some((id, _slug)) => id,
        None => stem,
    };

    if id.len() != HYPHENATED_UUID_LEN {
        return Err(invalid());
    }
    Uuid::parse_str(id).map_err(|_| invalid())
}

// ---------------------------------------------------------------------------------------------
// Filename construction
// ---------------------------------------------------------------------------------------------

/// Whether a new note's filename carries a slug derived from its title.
///
/// This is the **creation-time option** that replaced `workspace.toml`'s `[notes] filename` knob in
/// stage 1b. The knob governed nothing the reader cared about: [`parse_note_filename`] has always
/// accepted both forms and always ignored the slug, so declaring a vault-wide style controlled
/// only what `create` happened to emit — which is a per-creation decision, not a property of the
/// vault.
///
/// Because the reader ignores everything after the UUID, **re-slugging on a title change is safe**:
/// the identity is the UUID and it does not move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FilenameSlug {
    /// `<uuid>.md`. The default.
    #[default]
    None,
    /// `<uuid>_<slug>.md`, when the title yields a non-empty slug.
    FromTitle,
}

/// The longest slug [`note_filename`] will append.
///
/// Path length is the constraint, not aesthetics: Windows' traditional `MAX_PATH` is 260, a
/// hyphenated UUID plus `_` plus `.md` already costs 40, and a vault living several directories
/// deep inside a synced folder spends the rest quickly.
const MAX_SLUG_LEN: usize = 60;

/// The filename a note with this id and title takes.
///
/// `title` is consulted only when `slug` is [`FilenameSlug::FromTitle`]; a title that slugifies to
/// nothing — punctuation, emoji, an empty string — yields the bare `<uuid>.md` form rather than a
/// trailing separator, which [`parse_note_filename`] rejects as a truncated name.
#[must_use]
pub fn note_filename(id: Uuid, title: Option<&str>, slug: FilenameSlug) -> String {
    let hyphenated = id.hyphenated().to_string();
    let slug = match (slug, title) {
        (FilenameSlug::FromTitle, Some(title)) => slugify(title),
        _ => String::new(),
    };
    if slug.is_empty() {
        format!("{hyphenated}{NOTE_EXTENSION}")
    } else {
        format!("{hyphenated}{SLUG_SEPARATOR}{slug}{NOTE_EXTENSION}")
    }
}

/// A filename-safe slug: lowercase ASCII alphanumerics, runs of anything else collapsed to a
/// single `_`, trimmed, and capped at [`MAX_SLUG_LEN`].
///
/// Deliberately narrow. The slug is decorative and never read back, so the only properties that
/// matter are that it is legal on every platform jot runs on and that it does not confuse
/// [`parse_note_filename`] — which means no leading or trailing separator and no characters
/// Windows refuses (`<>:"/\|?*`, control characters, a trailing dot or space). Dropping non-ASCII
/// costs a Korean or Japanese title its slug and keeps the UUID, which is the part that matters;
/// transliteration would be a dependency and a set of judgement calls for a decoration.
#[must_use]
pub fn slugify(title: &str) -> String {
    let mut out = String::with_capacity(title.len().min(MAX_SLUG_LEN));
    let mut pending_separator = false;
    for c in title.chars() {
        if !c.is_ascii_alphanumeric() {
            pending_separator = true;
            continue;
        }
        // The separator and the character are budgeted together, so the cap can never be reached
        // *on* a separator — a slug ending in `_` is exactly the truncated name
        // `parse_note_filename` rejects.
        let separator = usize::from(pending_separator && !out.is_empty());
        if out.len() + separator + 1 > MAX_SLUG_LEN {
            break;
        }
        if separator == 1 {
            out.push(SLUG_SEPARATOR);
        }
        pending_separator = false;
        out.push(c.to_ascii_lowercase());
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Enumeration
// ---------------------------------------------------------------------------------------------

/// The live notes of a `jot` workspace: every `*.md` file directly inside `root`.
///
/// Non-recursive, and every entry whose name begins with `.` is skipped — which is what excludes
/// `.jot/`, and with it the trash, the index, and the staging directory. Sorted by path, so a
/// rebuild that walks this list twice walks it in the same order both times.
///
/// The filter is extension-and-dotfile, not id-validity: a `*.md` file whose name is *not* a note
/// filename is still returned. Hiding it here would make a note silently vanish from the vault
/// because something renamed the file, which is the failure this project can least afford; what to
/// do with a name [`parse_note_filename`] rejects is the caller's decision, and the caller can see
/// it only if enumeration hands it over.
///
/// # Errors
///
/// [`Error::ReadDir`] naming `root` if it cannot be listed — including when it does not exist. An
/// absent vault root is a real fault (the vault moved, or the caller has the wrong path), not the
/// ordinary empty case; an existing but empty root yields an empty vector.
pub fn live_note_paths(root: &Path) -> Result<Vec<PathBuf>> {
    Ok(live_note_entries(root)?
        .into_iter()
        .map(|entry| entry.path)
        .collect())
}

/// [`live_note_paths`], keeping the size and modification time the directory read already carried.
///
/// On Windows a directory entry *contains* both — `FindNextFile` returns them — so
/// [`std::fs::DirEntry::metadata`] costs nothing there, while a separate `metadata()` call per file
/// is a syscall per file. At 10k notes that is the difference between a warm sync you notice and
/// one you do not, and the scanner needs exactly these two numbers for every file it enumerates.
pub(crate) fn live_note_entries(root: &Path) -> Result<Vec<NoteEntry>> {
    note_entries_in(root)
}

/// The trashed notes of a `jot` workspace: every `*.md` file directly inside `<root>/.jot/.trash/`.
///
/// Takes the **workspace root**, not the trash directory, so no caller has to know where the trash
/// lives. Same filtering and same ordering as [`live_note_paths`].
///
/// # Errors
///
/// [`Error::ReadDir`] if the trash directory exists but cannot be listed. A *missing* trash
/// directory is not an error: it means nothing has ever been trashed, and it yields an empty
/// vector. The asymmetry with [`live_note_paths`] is deliberate — an absent vault root is a lost
/// vault, an absent trash is an empty one.
///
/// **Absent is the only silent case.** The guard is existence, not directory-ness: a `.jot/.trash`
/// that a sync client replaced with a regular file, or a symlink whose target is gone, is a fault
/// and is reported as [`Error::ReadDir`] naming the trash. Answering "no trashed notes" there
/// would hide every trashed note in the vault behind a successful, empty result — the same
/// reasoning that makes an absent *vault root* an error rather than an empty listing.
pub fn trashed_note_paths(root: &Path) -> Result<Vec<PathBuf>> {
    Ok(trashed_note_entries(root)?
        .into_iter()
        .map(|entry| entry.path)
        .collect())
}

/// [`trashed_note_paths`], with the metadata the directory read already carried. See
/// [`live_note_entries`].
pub(crate) fn trashed_note_entries(root: &Path) -> Result<Vec<NoteEntry>> {
    let trash = trash_dir(root);
    // `symlink_metadata`, not `metadata`: a dangling symlink is *something* standing where the
    // trash should be, and must not be mistaken for nothing being there at all.
    match std::fs::symlink_metadata(&trash) {
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(source) => Err(Error::ReadDir {
            path: trash,
            source,
        }),
        // Anything that is not a listable directory fails in `read_dir`, which names the path.
        Ok(_) => note_entries_in(&trash),
    }
}

/// One enumerated note file, and what the directory read already knew about it.
#[derive(Debug, Clone)]
pub(crate) struct NoteEntry {
    /// The file.
    pub(crate) path: PathBuf,
    /// Size in bytes, and modification time — `None` when the platform declined to report them,
    /// in which case the caller must `stat` or hash rather than assume.
    pub(crate) stat: Option<(u64, std::time::SystemTime)>,
}

/// `<root>/.jot/.trash` — the one place this module encodes the on-disk layout.
pub fn trash_dir(root: &Path) -> PathBuf {
    root.join(".jot").join(".trash")
}

fn note_entries_in(dir: &Path) -> Result<Vec<NoteEntry>> {
    let read_dir_err = |source| Error::ReadDir {
        path: dir.to_path_buf(),
        source,
    };

    let entries = std::fs::read_dir(dir).map_err(read_dir_err)?;

    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(read_dir_err)?;
        if is_dir(&entry) {
            continue;
        }
        let name = entry.file_name();
        // A filename that is not UTF-8 cannot be a note filename, so it is skipped rather than
        // failing the whole listing: one unreadable name must not hide every note beside it.
        let Some(name) = name.to_str() else { continue };
        if name.starts_with('.') || !name.ends_with(NOTE_EXTENSION) {
            continue;
        }
        let stat = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok().map(|time| (meta.len(), time)));
        found.push(NoteEntry {
            path: entry.path(),
            stat,
        });
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(found)
}

/// Whether a directory entry is a directory, following symlinks — a symlink pointing at a directory
/// is not a note either. Falls back to a `stat` only for the symlink case, so the common path costs
/// nothing beyond what `read_dir` already returned.
fn is_dir(entry: &DirEntry) -> bool {
    match entry.file_type() {
        Ok(file_type) if !file_type.is_symlink() => file_type.is_dir(),
        _ => entry.path().is_dir(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const ID_A: &str = "01a03d4c-c708-7cbf-83c0-883cedb7f1d5";
    const ID_B: &str = "01a03d51-4b48-72e2-9f30-f180030c06ab";

    /// A vault root and a sibling staging directory, both real, in a temp dir that cleans itself
    /// up. Staging is a *sibling* of the vault, not a child, so that a test which makes the vault
    /// directory unwritable does not accidentally break staging instead of the rename.
    struct Fixture {
        _tmp: TempDir,
        vault: PathBuf,
        staging: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let vault = tmp.path().join("vault");
            let staging = tmp.path().join("staging");
            std::fs::create_dir(&vault).unwrap();
            std::fs::create_dir(&staging).unwrap();
            Self {
                _tmp: tmp,
                vault,
                staging,
            }
        }

        fn note(&self, name: &str) -> PathBuf {
            self.vault.join(name)
        }

        fn staging_entries(&self) -> Vec<PathBuf> {
            let mut entries: Vec<PathBuf> = std::fs::read_dir(&self.staging)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .collect();
            entries.sort();
            entries
        }
    }

    fn names(paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    /// Makes `target` impossible to replace by a rename while leaving it readable, so that
    /// `atomic_write` gets all the way through staging and `fsync` and then fails at the rename.
    /// This is the failure injection dispatch.md §U4 scopes "interrupted" to.
    ///
    /// * Windows — a read-only destination makes `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` fail.
    /// * Unix — replacing a directory entry needs write permission on the *containing* directory,
    ///   so the parent is dropped to `r-xr-xr-x`.
    ///
    /// Reverted on drop, or the temp directory could not be cleaned up.
    struct BlockedRename {
        target: PathBuf,
    }

    impl BlockedRename {
        fn new(target: &Path) -> Self {
            #[cfg(windows)]
            {
                let mut perms = std::fs::metadata(target).unwrap().permissions();
                perms.set_readonly(true);
                std::fs::set_permissions(target, perms).unwrap();
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let parent = target.parent().unwrap();
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o555)).unwrap();
            }
            Self {
                target: target.to_path_buf(),
            }
        }
    }

    impl Drop for BlockedRename {
        // Clearing the read-only bit is what the lint warns about; this arm is `cfg(windows)`, the
        // file is inside a `TempDir`, and without it the temp directory cannot be removed.
        #[allow(clippy::permissions_set_readonly_false)]
        fn drop(&mut self) {
            #[cfg(windows)]
            {
                if let Ok(meta) = std::fs::metadata(&self.target) {
                    let mut perms = meta.permissions();
                    perms.set_readonly(false);
                    let _ = std::fs::set_permissions(&self.target, perms);
                }
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Some(parent) = self.target.parent() {
                    let _ =
                        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o755));
                }
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Atomic write
    // -----------------------------------------------------------------------------------------

    /// The platform assumption the whole module rests on, tested directly rather than through
    /// `atomic_write`, so that a failure points at the platform instead of at our code.
    /// `overview.md` claims this fails on Windows; if that ever becomes true again, this is the
    /// test that says so.
    #[test]
    fn std_rename_replaces_an_existing_file_on_this_platform() {
        let f = Fixture::new();
        let target = f.note("target.md");
        std::fs::write(&target, b"original").unwrap();
        let staged = f.staging.join("staged.tmp");
        std::fs::write(&staged, b"replacement").unwrap();

        std::fs::rename(&staged, &target).expect(
            "std::fs::rename must replace an existing target on this platform; if this fails on \
             Windows, overview.md's MOVEFILE_REPLACE_EXISTING risk is live and atomic_write needs \
             a platform-specific rename",
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"replacement");
        assert!(!staged.exists(), "the staged file must have been moved");
    }

    /// Catches a writer that cannot create a file that is not already there.
    #[test]
    fn atomic_write_creates_a_target_that_does_not_exist() {
        let f = Fixture::new();
        let target = f.note("new.md");
        atomic_write(&target, &f.staging, b"hello").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"hello");
    }

    /// The Windows criterion: overwriting an existing note must succeed, and must replace the
    /// contents entirely rather than leaving a tail of the longer previous body behind.
    #[test]
    fn atomic_write_overwrites_an_existing_file_completely() {
        let f = Fixture::new();
        let target = f.note(&format!("{ID_A}.md"));

        let long = b"a considerably longer previous body, written first\n";
        let short = b"short\n";

        atomic_write(&target, &f.staging, long).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), long);

        atomic_write(&target, &f.staging, short).expect(
            "rename over an existing file must succeed (MOVEFILE_REPLACE_EXISTING on Windows)",
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            short,
            "the overwrite left a tail of the previous, longer file"
        );
    }

    /// Text-mode translation on Windows would turn `\n` into `\r\n` and silently break the
    /// byte-identical round-trip gate for every note ever written on this platform.
    #[test]
    fn atomic_write_is_byte_exact_and_never_translates_line_endings() {
        let f = Fixture::new();
        let target = f.note("mixed.md");

        let mixed: &[u8] = b"---\r\nid: x\r\n---\r\n\r\nCRLF above, LF below\nand no trailing byte";
        atomic_write(&target, &f.staging, mixed).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), mixed);

        let lf: &[u8] = b"---\nid: x\n---\n\njust LF\n";
        atomic_write(&target, &f.staging, lf).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), lf);
    }

    /// An empty write must truncate, not no-op.
    #[test]
    fn atomic_write_of_no_bytes_leaves_an_empty_file() {
        let f = Fixture::new();
        let target = f.note("t.md");
        atomic_write(&target, &f.staging, b"a long previous body").unwrap();
        atomic_write(&target, &f.staging, b"").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"");
    }

    /// Catches a writer that leaks one temp file per write — a vault that fills `.jot/tmp/`
    /// forever. Necessary, and nowhere near sufficient for §U4 (see the interruption test).
    #[test]
    fn atomic_write_leaves_no_debris_in_the_staging_directory() {
        let f = Fixture::new();
        let target = f.note("t.md");
        for i in 0..5 {
            atomic_write(&target, &f.staging, format!("body {i}").as_bytes()).unwrap();
        }
        assert!(
            f.staging_entries().is_empty(),
            "staging directory not empty: {:?}",
            f.staging_entries()
        );
    }

    /// Anti-vacuity, and the discriminator the acceptance suite also uses: a writer that ignored
    /// `tmp_dir` and opened the target directly would pass every other test in this module,
    /// including the interruption test. Handing it a `tmp_dir` that is a *file* makes staging
    /// there impossible, and unlike a missing directory it cannot be defensibly created.
    #[test]
    fn atomic_write_genuinely_stages_inside_the_tmp_dir_it_is_given() {
        let f = Fixture::new();
        let not_a_dir = f.vault.join("not-a-directory");
        std::fs::write(&not_a_dir, b"a file where a staging directory should be").unwrap();
        let target = f.note("t.md");
        std::fs::write(&target, b"original").unwrap();

        let err = atomic_write(&target, &not_a_dir, b"new")
            .expect_err("staging into a file is impossible; this must not succeed");
        assert!(
            err.to_string().contains("not-a-directory"),
            "the error must name the staging path it could not use: {err}"
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"original",
            "a failure before the rename must leave the target untouched"
        );
    }

    /// A staging directory that has gone missing is recreated rather than failing the write.
    #[test]
    fn atomic_write_creates_a_missing_staging_directory() {
        let f = Fixture::new();
        let staging = f.staging.join("nested").join("deeper");
        let target = f.note("t.md");
        atomic_write(&target, &staging, b"body").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"body");
        assert!(staging.is_dir());
    }

    /// **dispatch.md §U4.** A failure injected between staging and rename must leave the target
    /// byte-identical. The assertion is on the *target's contents* — asserting only that the temp
    /// file was cleaned up does not satisfy this criterion.
    ///
    /// Out of scope, and untested here: a process killed mid-write, and a full disk.
    #[test]
    fn an_interrupted_write_leaves_the_original_byte_intact() {
        let f = Fixture::new();
        let target = f.note(&format!("{ID_A}.md"));
        let original: &[u8] = b"---\nid: 01a03d4c-c708-7cbf-83c0-883cedb7f1d5\n---\n\nThe original body, which must survive.\n";
        std::fs::write(&target, original).unwrap();

        let replacement: &[u8] = b"The replacement, which must never land.\n";
        let result = {
            let _blocked = BlockedRename::new(&target);
            atomic_write(&target, &f.staging, replacement)
        };

        let err = result.expect_err(
            "the injection did not block the rename, so this test proves nothing about \
             interruption",
        );
        assert!(
            matches!(err, Error::Rename { .. }),
            "a blocked rename must surface as Error::Rename, not as something vaguer: {err:?}"
        );
        assert_eq!(
            std::fs::read(&target).unwrap(),
            original,
            "a write that failed at the rename must leave the target byte-identical: not \
             truncated, not partially written, not deleted"
        );
    }

    /// The secondary property of the same failure, kept separate so it can never be mistaken for
    /// the §U4 assertion above: the staged file does not survive a failed write.
    #[test]
    fn a_failed_rename_cleans_up_its_staged_file() {
        let f = Fixture::new();
        let target = f.note("t.md");
        std::fs::write(&target, b"original").unwrap();

        let result = {
            let _blocked = BlockedRename::new(&target);
            atomic_write(&target, &f.staging, b"replacement")
        };
        assert!(result.is_err());
        assert!(
            f.staging_entries().is_empty(),
            "a failed write left debris behind: {:?}",
            f.staging_entries()
        );
    }

    /// The rename error must name both files: recovering a half-finished write by hand needs the
    /// staged path as well as the target.
    #[test]
    fn a_failed_rename_names_both_the_staged_file_and_the_target() {
        let f = Fixture::new();
        let target = f.note("named.md");
        std::fs::write(&target, b"original").unwrap();

        let err = {
            let _blocked = BlockedRename::new(&target);
            atomic_write(&target, &f.staging, b"replacement").unwrap_err()
        };
        let Error::Rename { from, to, .. } = &err else {
            panic!("expected Error::Rename, got {err:?}");
        };
        assert!(
            from.starts_with(&f.staging),
            "`from` must be the staged file inside the staging directory, was {}",
            from.display()
        );
        assert_eq!(to, &target);
        assert!(err.to_string().contains("named.md"), "{err}");
        assert_eq!(err.path(), Some(target.as_path()), "path() is the target");
    }

    /// Two writes to different targets through one staging directory must not collide on a staged
    /// filename. `create_new` would turn a collision into an error rather than silent corruption,
    /// so this is really a test that the names are unique.
    #[test]
    fn staged_filenames_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1_000 {
            assert!(
                seen.insert(staged_file_name()),
                "staged filenames collided within one process"
            );
        }
    }

    /// A staged file must be invisible to enumeration even in the pathological case where the
    /// staging directory is the vault root itself.
    #[test]
    fn a_staged_filename_is_not_enumerable() {
        let name = staged_file_name();
        assert!(name.starts_with('.'), "{name}");
        assert!(!name.ends_with(NOTE_EXTENSION), "{name}");
    }

    // -----------------------------------------------------------------------------------------
    // Filename parsing
    // -----------------------------------------------------------------------------------------

    #[test]
    fn parse_note_filename_accepts_the_bare_uuid_form() {
        let id = parse_note_filename(Path::new(&format!("{ID_A}.md"))).unwrap();
        assert_eq!(id.to_string(), ID_A);
    }

    /// The slug is decorative: both forms yield the same id, and a slug containing underscores must
    /// not confuse the split.
    #[test]
    fn parse_note_filename_ignores_the_slug() {
        let bare = parse_note_filename(Path::new(&format!("{ID_A}.md"))).unwrap();
        for name in [
            format!("{ID_A}_first_thoughts.md"),
            format!("{ID_A}_a_slug_with_many_parts.md"),
            format!("{ID_A}_slug.with.dots.md"),
            format!("{ID_A}_ .md"),
        ] {
            assert_eq!(
                parse_note_filename(Path::new(&name)).unwrap(),
                bare,
                "{name} must parse to the same id as the bare form"
            );
        }
    }

    /// Only the filename matters; the directories above it are irrelevant, including a parent
    /// directory that looks like a note.
    #[test]
    fn parse_note_filename_looks_only_at_the_final_component() {
        let path = PathBuf::from(format!("{ID_B}.md")).join(format!("{ID_A}_slug.md"));
        assert_eq!(
            parse_note_filename(&path).unwrap().to_string(),
            ID_A,
            "the id comes from the filename, not from an ancestor directory"
        );
    }

    /// Every near-miss. Each of these has been a real filename in somebody's notes directory.
    #[test]
    fn parse_note_filename_rejects_near_misses() {
        let cases: Vec<(String, &str)> = vec![
            ("notes.md".into(), "a stem that is not a uuid"),
            ("README.md".into(), "a stem that is not a uuid"),
            (format!("{ID_A}.txt"), "the wrong extension"),
            (format!("{ID_A}.markdown"), "the wrong extension"),
            (format!("{ID_A}.MD"), "the extension is case-sensitive"),
            (ID_A.to_string(), "no extension at all"),
            (format!("{ID_A}.md.bak"), "an extension after the extension"),
            ("01a03d4c-c708-7cbf-83c0.md".into(), "a truncated uuid"),
            (
                "01a03d4c-c708-7cbf-83c0-883cedb7f1d5f.md".into(),
                "an over-long uuid",
            ),
            (
                "01a03d4cc7087cbf83c0883cedb7f1d5.md".into(),
                "the unhyphenated form Uuid::parse_str would otherwise accept",
            ),
            (
                format!("{{{ID_A}}}.md"),
                "the braced form Uuid::parse_str would otherwise accept",
            ),
            (
                format!("urn:uuid:{ID_A}.md"),
                "the URN form Uuid::parse_str would otherwise accept",
            ),
            (format!("{ID_A}_.md"), "the separator with an empty slug"),
            ("_first_thoughts.md".into(), "a slug with no uuid"),
            (".md".into(), "the extension alone"),
            ("".into(), "an empty name"),
            (
                "01a03d4c-c708-7cbf-83c0-883cedb7g1d5.md".into(),
                "a non-hex character in the uuid",
            ),
            (format!(" {ID_A}.md"), "a leading space"),
            (
                format!("{ID_A} .md"),
                "a trailing space before the extension",
            ),
        ];

        for (name, why) in cases {
            let err = parse_note_filename(Path::new(&name))
                .expect_err(&format!("`{name}` must be rejected: {why}"));
            assert!(
                matches!(err, Error::InvalidNoteFilename { .. }),
                "`{name}` must be rejected as an invalid note filename, got {err:?}"
            );
            assert!(
                err.to_string().contains(&name) || name.is_empty(),
                "the error must name the path: {err}"
            );
        }
    }

    /// Hex case is not enforced — a hand-uppercased filename still loads, and yields the same id.
    #[test]
    fn parse_note_filename_accepts_uppercase_hex() {
        let upper = ID_A.to_uppercase();
        assert_eq!(
            parse_note_filename(Path::new(&format!("{upper}.md"))).unwrap(),
            Uuid::parse_str(ID_A).unwrap()
        );
    }

    /// A directory path with a trailing separator has no filename to speak of, and must not panic.
    #[test]
    fn parse_note_filename_rejects_a_path_with_no_filename() {
        for path in ["..", "/", "."] {
            let err = parse_note_filename(Path::new(path))
                .expect_err(&format!("`{path}` has no note filename"));
            assert!(
                matches!(err, Error::InvalidNoteFilename { .. }),
                "`{path}` must be rejected as an invalid note filename, got {err:?}"
            );
        }
    }

    // -----------------------------------------------------------------------------------------
    // Enumeration
    // -----------------------------------------------------------------------------------------

    /// Builds a small vault: three live notes, one trashed, and the several kinds of thing that
    /// must not be mistaken for a note.
    fn populated_vault() -> Fixture {
        let f = Fixture::new();
        let trash = trash_dir(&f.vault);
        std::fs::create_dir_all(&trash).unwrap();
        std::fs::create_dir_all(f.vault.join(".jot").join("tmp")).unwrap();

        for name in [
            format!("{ID_A}.md"),
            format!("{ID_B}_first_thoughts.md"),
            "01a03d52-6c58-75de-81f8-1b3940ecc38b.md".to_string(),
        ] {
            std::fs::write(f.vault.join(name), b"note").unwrap();
        }

        // None of these is a live note.
        std::fs::write(f.vault.join(".hidden.md"), b"dotfile").unwrap();
        std::fs::write(f.vault.join("notes.txt"), b"not markdown").unwrap();
        std::fs::write(f.vault.join(".jot").join("workspace.toml"), b"").unwrap();
        std::fs::write(f.vault.join(".jot").join("stray.md"), b"inside .jot").unwrap();
        std::fs::create_dir(f.vault.join("subdir")).unwrap();
        std::fs::write(f.vault.join("subdir").join("nested.md"), b"nested").unwrap();
        std::fs::create_dir(f.vault.join("looks-like-a-note.md")).unwrap();

        std::fs::write(
            trash.join("01a03d52-fce0-756a-8944-abff289098e4.md"),
            b"gone",
        )
        .unwrap();
        std::fs::write(trash.join(".hidden.md"), b"dotfile").unwrap();

        f
    }

    #[test]
    fn live_note_paths_lists_markdown_in_the_root_only() {
        let f = populated_vault();
        let live = live_note_paths(&f.vault).unwrap();
        assert_eq!(
            names(&live),
            vec![
                format!("{ID_A}.md"),
                "01a03d51-4b48-72e2-9f30-f180030c06ab_first_thoughts.md".to_string(),
                "01a03d52-6c58-75de-81f8-1b3940ecc38b.md".to_string(),
            ],
            "expected exactly the three live notes, sorted"
        );
    }

    /// The rules that keep everything that is not a live note out of the listing, each stated as
    /// its own assertion so a regression says which rule broke.
    #[test]
    fn live_note_paths_skips_dotfiles_the_jot_directory_and_subdirectories() {
        let f = populated_vault();
        let listed = names(&live_note_paths(&f.vault).unwrap());

        assert!(
            !listed.iter().any(|n| n.starts_with('.')),
            "dotfiles must be skipped: {listed:?}"
        );
        assert!(
            !listed.contains(&"stray.md".to_string()),
            "enumeration must not descend into .jot/: {listed:?}"
        );
        assert!(
            !listed.contains(&"nested.md".to_string()),
            "enumeration is non-recursive for the jot workspace kind: {listed:?}"
        );
        assert!(
            !listed.contains(&"notes.txt".to_string()),
            "only markdown: {listed:?}"
        );
        assert!(
            !listed.contains(&"looks-like-a-note.md".to_string()),
            "a directory named like a note is not a note: {listed:?}"
        );
        assert!(
            !listed.contains(&"01a03d52-fce0-756a-8944-abff289098e4.md".to_string()),
            "a trashed note is not live: {listed:?}"
        );
    }

    /// A `*.md` file whose name is not a note filename is still returned, and this is deliberate:
    /// a note whose filename got mangled by a sync client must not silently disappear.
    #[test]
    fn live_note_paths_returns_markdown_that_is_not_a_valid_note_filename() {
        let f = Fixture::new();
        std::fs::write(f.vault.join("README.md"), b"not a note").unwrap();
        let live = live_note_paths(&f.vault).unwrap();
        assert_eq!(names(&live), vec!["README.md".to_string()]);
        assert!(
            parse_note_filename(&live[0]).is_err(),
            "and the caller is the one that decides what to do about it"
        );
    }

    #[test]
    fn trashed_note_paths_lists_only_the_trash() {
        let f = populated_vault();
        assert_eq!(
            names(&trashed_note_paths(&f.vault).unwrap()),
            vec!["01a03d52-fce0-756a-8944-abff289098e4.md".to_string()],
            "trashed notes keep their filename and live in .jot/.trash/"
        );
    }

    #[test]
    fn enumeration_of_an_empty_vault_is_empty_not_an_error() {
        let f = Fixture::new();
        std::fs::create_dir_all(trash_dir(&f.vault)).unwrap();
        assert!(live_note_paths(&f.vault).unwrap().is_empty());
        assert!(trashed_note_paths(&f.vault).unwrap().is_empty());
    }

    /// A trash directory that has never been created means nothing has ever been trashed.
    #[test]
    fn trashed_note_paths_of_a_vault_with_no_trash_directory_is_empty() {
        let f = Fixture::new();
        assert!(!trash_dir(&f.vault).exists());
        assert!(
            trashed_note_paths(&f.vault).unwrap().is_empty(),
            "an absent trash is an empty trash, not a failure"
        );
    }

    /// A trash that is not there at all is empty; a trash that is there and is *not a listable
    /// directory* is a fault. The distinction matters because the failure mode of the wrong guard
    /// is invisible: a sync client that replaces `.jot/.trash` with a regular file would make
    /// every trashed note in the vault disappear behind a successful, empty result.
    #[test]
    fn trashed_note_paths_of_a_trash_that_is_a_file_is_an_error_not_an_empty_listing() {
        let f = Fixture::new();
        let trash = trash_dir(&f.vault);
        std::fs::create_dir_all(trash.parent().unwrap()).unwrap();
        std::fs::write(&trash, b"a sync client put a file here").unwrap();

        let err = trashed_note_paths(&f.vault)
            .expect_err("a trash that is a file is a fault, not an empty trash");
        assert!(matches!(err, Error::ReadDir { .. }), "{err:?}");
        assert!(
            err.to_string().contains(".trash"),
            "the error must name the trash: {err}"
        );
    }

    /// The dangling-symlink case, which is the one `is_dir()` and `metadata()` both answer
    /// "false"/"missing" to. Unix only: creating a symlink on Windows needs a privilege CI does
    /// not have, and the `is_dir()` guard this replaces was wrong on both platforms for the
    /// regular-file case above, which does run everywhere.
    #[cfg(unix)]
    #[test]
    fn trashed_note_paths_of_a_dangling_symlink_is_an_error_not_an_empty_listing() {
        let f = Fixture::new();
        let trash = trash_dir(&f.vault);
        std::fs::create_dir_all(trash.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(f.vault.join("gone"), &trash).unwrap();

        assert!(!trash.is_dir(), "the premise: is_dir() cannot see it");
        assert!(
            std::fs::symlink_metadata(&trash).is_ok(),
            "but something is standing there"
        );
        let err = trashed_note_paths(&f.vault)
            .expect_err("a dangling trash symlink is a fault, not an empty trash");
        assert!(matches!(err, Error::ReadDir { .. }), "{err:?}");
    }

    /// An absent vault root is a different thing entirely: the caller is pointing at nothing.
    #[test]
    fn live_note_paths_of_a_missing_root_is_an_error_naming_the_root() {
        let f = Fixture::new();
        let missing = f.vault.join("no-such-vault");
        let err = live_note_paths(&missing).expect_err("a vault root that is not there is a fault");
        assert!(matches!(err, Error::ReadDir { .. }), "{err:?}");
        assert!(
            err.to_string().contains("no-such-vault"),
            "the error must name the directory: {err}"
        );
    }

    /// Enumeration must be a *file* listing: pointing it at a file, not a directory, is an error
    /// rather than a silent empty result.
    #[test]
    fn live_note_paths_of_a_file_is_an_error() {
        let f = Fixture::new();
        let file = f.vault.join("a.md");
        std::fs::write(&file, b"x").unwrap();
        assert!(matches!(live_note_paths(&file), Err(Error::ReadDir { .. })));
    }

    /// The order is stable regardless of the order the filesystem hands entries back, so a rebuild
    /// and an incremental sync walk the vault identically.
    #[test]
    fn enumeration_is_sorted_and_therefore_deterministic() {
        let f = Fixture::new();
        for name in ["c.md", "a.md", "b.md"] {
            std::fs::write(f.vault.join(name), b"x").unwrap();
        }
        assert_eq!(
            names(&live_note_paths(&f.vault).unwrap()),
            vec!["a.md", "b.md", "c.md"]
        );
    }

    /// Enumeration returns absolute-from-the-root paths that can be opened directly, not bare
    /// filenames the caller has to rejoin.
    #[test]
    fn enumeration_returns_paths_that_can_be_opened() {
        let f = populated_vault();
        for path in live_note_paths(&f.vault)
            .unwrap()
            .into_iter()
            .chain(trashed_note_paths(&f.vault).unwrap())
        {
            assert_eq!(
                std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
                if path.starts_with(trash_dir(&f.vault)) {
                    b"gone".to_vec()
                } else {
                    b"note".to_vec()
                }
            );
        }
    }

    /// The round trip this module is for: write a note atomically, find it by enumeration, and
    /// recover its id from the path it was found at.
    #[test]
    fn a_written_note_is_enumerable_and_its_id_recoverable() {
        let f = Fixture::new();
        let id = Uuid::now_v7();
        let target = f.vault.join(format!("{id}_a_slug.md"));
        atomic_write(&target, &f.staging, b"body").unwrap();

        let live = live_note_paths(&f.vault).unwrap();
        assert_eq!(live.len(), 1, "{live:?}");
        assert_eq!(parse_note_filename(&live[0]).unwrap(), id);
    }

    // -------------------------------------------------------------- filename construction

    fn uuid() -> Uuid {
        Uuid::parse_str("01a03d21-7c11-7a02-b3de-9f0e21c4a771").unwrap()
    }

    #[test]
    fn the_default_filename_is_the_bare_uuid() {
        assert_eq!(
            note_filename(uuid(), Some("A Title"), FilenameSlug::None),
            "01a03d21-7c11-7a02-b3de-9f0e21c4a771.md"
        );
        assert_eq!(FilenameSlug::default(), FilenameSlug::None);
    }

    #[test]
    fn a_slugged_filename_carries_the_title_after_the_uuid() {
        assert_eq!(
            note_filename(uuid(), Some("First Thoughts!"), FilenameSlug::FromTitle),
            "01a03d21-7c11-7a02-b3de-9f0e21c4a771_first_thoughts.md"
        );
    }

    /// Every filename this module builds must be one it can read back, or a note created by jot
    /// is a note jot cannot enumerate.
    #[test]
    fn every_constructed_filename_parses_back_to_its_id() {
        let long = "a".repeat(400);
        let wordy = "word ".repeat(80);
        let titles = [
            None,
            Some(""),
            Some("   "),
            Some("!!!"),
            Some("한국어 제목"),
            Some("_leading and trailing_"),
            Some("--- dashes ---"),
            Some("A"),
            Some("Mixed CASE and 123"),
            Some(long.as_str()),
            Some(wordy.as_str()),
        ];
        for slug in [FilenameSlug::None, FilenameSlug::FromTitle] {
            for title in titles {
                let name = note_filename(uuid(), title, slug);
                let parsed = parse_note_filename(Path::new(&name))
                    .unwrap_or_else(|e| panic!("{name:?} ({slug:?}) does not parse back: {e}"));
                assert_eq!(parsed, uuid(), "{name:?}");
            }
        }
    }

    #[test]
    fn a_title_that_slugifies_to_nothing_falls_back_to_the_bare_form() {
        for title in ["", "   ", "!!!", "한국어", "___"] {
            assert_eq!(
                note_filename(uuid(), Some(title), FilenameSlug::FromTitle),
                "01a03d21-7c11-7a02-b3de-9f0e21c4a771.md",
                "{title:?} must not produce a trailing separator"
            );
        }
    }

    #[test]
    fn slugify_collapses_runs_and_never_leaves_a_separator_at_either_end() {
        assert_eq!(slugify("Hello, World!"), "hello_world");
        assert_eq!(slugify("  spaced   out  "), "spaced_out");
        assert_eq!(slugify("a---b___c"), "a_b_c");
        assert_eq!(slugify("한국어 mixed 제목 text"), "mixed_text");
        assert_eq!(slugify("2026: the year"), "2026_the_year");
        assert_eq!(slugify(""), "");
    }

    #[test]
    fn a_slug_is_capped_and_the_cap_never_lands_on_a_separator() {
        for title in [
            "x".repeat(300),
            "word ".repeat(100),
            format!("{} tail", "y".repeat(MAX_SLUG_LEN - 1)),
            format!("{} tail", "y".repeat(MAX_SLUG_LEN)),
        ] {
            let slug = slugify(&title);
            assert!(
                slug.len() <= MAX_SLUG_LEN,
                "{slug:?} is {} long",
                slug.len()
            );
            assert!(
                !slug.ends_with(SLUG_SEPARATOR),
                "{slug:?} ends with a separator"
            );
            assert!(
                !slug.starts_with(SLUG_SEPARATOR),
                "{slug:?} starts with one"
            );
        }
    }

    /// The slug goes into a path on Windows, macOS and Linux, so it may contain nothing any of
    /// them reserve. Narrowing to ASCII alphanumerics and `_` is what makes that true by
    /// construction rather than by a blocklist that will miss something.
    #[test]
    fn a_slug_holds_nothing_any_platform_reserves() {
        let hostile = "a<b>c:d\"e/f\\g|h?i*j\0k\tl.m ";
        let slug = slugify(hostile);
        assert!(
            slug.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "{slug:?}"
        );
        assert_eq!(slug, "a_b_c_d_e_f_g_h_i_j_k_l_m");
    }
}
