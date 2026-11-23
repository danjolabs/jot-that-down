//! Filesystem primitives: atomic writes, create-new writes, and enumeration.
//!
//! This module is deliberately ignorant of what a note *is*. It moves bytes and lists `*.md`
//! files; whether a filename is a note or a stray is [`crate::id`]'s decision.
//!
//! # Atomic write
//!
//! [`atomic_write`] stages through a directory given as a **parameter** (`.jot/tmp/` in a
//! workspace) on the same filesystem as the target, `fsync`s the staged file, then renames it over
//! the target. Until the rename the target is untouched; after it the target is the new bytes.
//!
//! `std::fs::rename` maps to `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` on Windows and replaces
//! an existing file; `std_rename_replaces_an_existing_file_on_this_platform` pins that. A read-only
//! target or one held open without `FILE_SHARE_DELETE` still fails the rename, with the target
//! intact, as [`Error::Rename`].
//!
//! # Create-new write
//!
//! [`create_new_write`] is how a note comes into existence: `OpenOptions::create_new`, so the
//! filesystem itself refuses a collision and an existing note is never overwritten
//! (`docs/plans/redesign.md`, "Notes and ids"). It is not staged — a rename onto a path cannot be
//! made to fail when the path exists on every platform — so a failure part-way removes the file it
//! created, which is safe precisely because `create_new` proved nobody else owned it.

use std::fs::{DirEntry, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::error::{Error, Result};

/// The extension every note file carries.
pub(crate) const NOTE_EXTENSION: &str = ".md";

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
// Create-new write
// ---------------------------------------------------------------------------------------------

/// The outcome of a [`create_new_write`] that did not succeed.
#[derive(Debug)]
pub(crate) enum CreateNewError {
    /// Something already exists at the target. Nothing was written.
    Exists,
    /// Any other IO failure. A file this call created has been removed again.
    Io(std::io::Error),
}

/// Writes `bytes` to a file that must not exist yet.
pub(crate) fn create_new_write(
    target: &Path,
    bytes: &[u8],
) -> std::result::Result<(), CreateNewError> {
    let mut file = match File::options().write(true).create_new(true).open(target) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(CreateNewError::Exists);
        }
        Err(e) => return Err(CreateNewError::Io(e)),
    };
    let written = file
        .write_all(bytes)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all());
    if let Err(e) = written {
        drop(file);
        remove_best_effort(target);
        return Err(CreateNewError::Io(e));
    }
    drop(file);
    sync_parent_dir(target);
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Enumeration
// ---------------------------------------------------------------------------------------------

/// One `*.md` file directly inside the workspace root, and what the directory read already knew.
#[derive(Debug, Clone)]
pub(crate) struct MdEntry {
    /// The file.
    pub(crate) path: PathBuf,
    /// Its file name, which is valid UTF-8 (a name that is not cannot be a note and is skipped).
    pub(crate) name: String,
    /// Size in bytes and modification time — `None` when the platform declined to report them.
    pub(crate) stat: Option<(u64, std::time::SystemTime)>,
}

/// Every `*.md` file directly inside `root`, sorted by name.
///
/// Non-recursive, and dot-files are skipped — which excludes `.jot/` and the staging files. On
/// Windows a directory entry carries size and mtime for free, which is what makes the sync fast
/// path cost one directory read and no `stat` per file.
///
/// # Errors
///
/// [`Error::ReadDir`] naming `root` if it cannot be listed.
pub(crate) fn md_entries(root: &Path) -> Result<Vec<MdEntry>> {
    let read_dir_err = |source| Error::ReadDir {
        path: root.to_path_buf(),
        source,
    };
    let mut found = Vec::new();
    for entry in std::fs::read_dir(root).map_err(read_dir_err)? {
        let entry = entry.map_err(read_dir_err)?;
        if is_dir(&entry) {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name.starts_with('.') || !name.to_ascii_lowercase().ends_with(NOTE_EXTENSION) {
            continue;
        }
        let stat = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok().map(|time| (meta.len(), time)));
        found.push(MdEntry {
            path: entry.path(),
            name,
            stat,
        });
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}

/// Whether a directory entry is a directory, following symlinks.
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

    #[test]
    fn create_new_refuses_an_existing_file_and_leaves_it_intact() {
        let f = Fixture::new();
        let target = f.note("taken.md");
        std::fs::write(&target, b"original").unwrap();
        assert!(matches!(
            create_new_write(&target, b"replacement"),
            Err(CreateNewError::Exists)
        ));
        assert_eq!(std::fs::read(&target).unwrap(), b"original");
    }
}
