//! `Workspace`: `init`, `open`, `discover`, and the on-disk `.jot/` contract.
//!
//! A workspace is a **self-identifying directory**. Everything needed to know what it is lives
//! inside it, in `.jot/workspace.toml`, which is why registering a workspace is nothing more than
//! pointing at a folder and why moving that folder loses nothing.
//!
//! # The tree `init` produces
//!
//! ```text
//! <workspace_root>/
//!   .jot/
//!     workspace.toml   # identity + config
//!     .trash/          # trashed notes keep their filename
//!     tmp/             # staging area for atomic writes
//!     .gitignore       # index.db*, tmp/
//!   <uuid>.md
//! ```
//!
//! `index.db` is stage 2 and is deliberately **not** created here: the index is derived and
//! disposable, and an empty database file at init would be a lie about that.
//!
//! # Rulings this module implements
//!
//! Three of `docs/plans/runs/stage1/dispatch.md`'s adjudications land here verbatim, and none of
//! them is re-decided in code:
//!
//! * **§U3 — `init` on an existing workspace.** The stage doc calls `init` "idempotent" and then
//!   describes the opposite. The ruling: `init` **errors** with [`Error::WorkspaceExists`] naming
//!   the path when `.jot/` already exists, and never overwrites. "Existing workspace" means
//!   `.jot/` exists *as a directory* — not that `workspace.toml` parses, and not that the target is
//!   non-empty. A directory full of `.md` files and no `.jot/` is a **valid** init: adopting a
//!   folder of existing markdown is a supported path. A target that does not exist is created.
//!   `name` is not a parameter; it defaults to the target directory's basename and is display-only.
//! * **§U7 — neither `init` nor `open` touches the registry.** There is no `use crate::registry`
//!   in this file and there must not be one. A library call with a global filesystem side effect
//!   outside the vault is a testing problem and a surprise; registration is an explicit
//!   `registry::*` call that the CLI wires up in stage 4.
//! * **§U2 — timestamps.** There are none in this file, and that is on purpose. The manifest
//!   carries identity and configuration only; `last_opened` is the registry's business, precisely
//!   because recording it here would make `open` a write.
//!
//! # Writes
//!
//! Every file this module creates goes through [`crate::fs::atomic_write`] with `.jot/tmp/` as the
//! staging directory. `init` is the one operation here that writes at all — `open` and `discover`
//! are reads and must add nothing to the vault, not even a staging file.
//!
//! # Schema versioning
//!
//! [`SCHEMA_VERSION`] is what this build writes and the highest it will open. A manifest declaring
//! a higher version is refused with [`Error::UnsupportedSchemaVersion`], carrying both numbers so
//! the message can say plainly that the workspace was written by a newer version. The version is
//! read *before* the rest of the manifest is deserialized, so a future manifest that also adds
//! required keys still reports "written by a newer version" rather than an unhelpful parse error.

use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::error::{Error, Result};
use crate::fs;

/// The manifest schema version this build writes, and the highest it will open.
pub const SCHEMA_VERSION: u32 = 1;

/// The directory that makes a directory a workspace.
const JOT_DIR: &str = ".jot";

/// The manifest's filename inside [`JOT_DIR`].
const MANIFEST_FILE: &str = "workspace.toml";

/// The staging directory for atomic writes, inside [`JOT_DIR`].
const TMP_DIR: &str = "tmp";

/// The trash directory, inside [`JOT_DIR`]. Kept in step with [`crate::fs::trash_dir`].
const TRASH_DIR: &str = ".trash";

/// Contents of `.jot/.gitignore`.
///
/// The index is rebuildable from the notes, so committing it buys nothing and costs merge
/// conflicts; the staging directory never holds anything anyone wants. LF endings are written
/// explicitly rather than left to the platform, so a vault checked out on two machines does not
/// show a diff on a file nobody edited.
const GITIGNORE: &str = "\
# The SQLite index is derived from the notes and is disposable. Never commit it.
index.db*
# Staging area for atomic writes. Never holds anything worth keeping.
tmp/
";

/// Header written above the manifest, since `name` being hand-editable is not discoverable
/// otherwise. Deliberately free of the text `schema_version` so nothing greps it by accident.
const MANIFEST_HEADER: &str = "\
# jot workspace manifest.
# `id` is minted once and must never change — it is what makes this directory self-identifying.
# `name` is display-only and safe to edit by hand.
";

// =============================================================================================
// Kinds and knobs
// =============================================================================================

/// Which flavour of workspace this is.
///
/// `jot` is the flat, UUID-named, threaded vault this project is about. `plain` is a folder of
/// freely-named markdown with no threads; it is declared here so a `plain` manifest round-trips
/// from stage 1 onward, and gets its behavior in stage 7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkspaceKind {
    /// Flat, UUID-named notes with threads. The default and the one stages 1–6 implement.
    Jot,
    /// Folders and free filenames, no threads. Stage 7.
    Plain,
}

impl WorkspaceKind {
    /// The manifest spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            WorkspaceKind::Jot => "jot",
            WorkspaceKind::Plain => "plain",
        }
    }

    /// Parses a manifest spelling. `None` for anything else — callers turn that into an
    /// [`Error::InvalidWorkspaceKind`] naming the path they read it from.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "jot" => Some(WorkspaceKind::Jot),
            "plain" => Some(WorkspaceKind::Plain),
            _ => None,
        }
    }
}

impl std::fmt::Display for WorkspaceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How this workspace names its note files.
///
/// The choice between a bare UUID and a UUID plus a decorative slug is deferred (see
/// `overview.md`, Open questions). Making it a manifest knob now means it can be settled later
/// without a migration: the slug is decorative in both cases, and the reader already accepts both
/// forms regardless of what the manifest says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FilenameStyle {
    /// `<uuid>.md`. The default.
    #[default]
    Uuid,
    /// `<uuid>_<slug>.md`.
    UuidSlug,
}

impl FilenameStyle {
    /// The manifest spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            FilenameStyle::Uuid => "uuid",
            FilenameStyle::UuidSlug => "uuid_slug",
        }
    }

    /// Parses a manifest spelling.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "uuid" => Some(FilenameStyle::Uuid),
            "uuid_slug" => Some(FilenameStyle::UuidSlug),
            _ => None,
        }
    }
}

impl std::fmt::Display for FilenameStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// =============================================================================================
// The manifest
// =============================================================================================

/// The contents of `.jot/workspace.toml`, flattened.
///
/// The TOML file nests `id`/`kind`/`name` under `[workspace]` and `filename` under `[notes]`; in
/// memory that nesting buys nothing, so the on-disk shape lives in the private `file` types below
/// and this is what callers see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// The version of the manifest format this file was written against.
    pub schema_version: u32,
    /// Minted at `init`, immutable thereafter. What makes the directory self-identifying.
    pub id: Uuid,
    /// `jot` or `plain`.
    pub kind: WorkspaceKind,
    /// Display only. Defaults to the target directory's basename at `init` (§U3) and is expected
    /// to be edited by hand.
    pub name: String,
    /// How note files are named.
    pub filename_style: FilenameStyle,
}

impl Manifest {
    /// Renders the manifest as the TOML text that goes on disk.
    ///
    /// `path` is used only to name the file in an error; nothing is read or written here.
    fn to_toml(&self, path: &Path) -> Result<String> {
        let file = file::Manifest {
            schema_version: self.schema_version,
            workspace: file::Workspace {
                id: self.id.to_string(),
                kind: self.kind.as_str(),
                name: &self.name,
            },
            notes: file::Notes {
                filename: self.filename_style.as_str(),
            },
        };
        let body = toml::to_string_pretty(&file).map_err(|source| Error::ManifestSerialize {
            path: path.to_path_buf(),
            message: source.to_string(),
        })?;
        Ok(format!("{MANIFEST_HEADER}\n{body}"))
    }

    /// Parses manifest text read from `path`.
    ///
    /// The schema version is checked first and on its own, so a manifest from the future reports
    /// as such even when the rest of it no longer deserializes against this build's shape.
    ///
    /// `default_name` is the basename of the directory the manifest was found in, used when the
    /// manifest omits `name`. `name` is display-only; refusing to open a vault over a missing
    /// display string would be absurd.
    fn from_toml(text: &str, path: &Path, default_name: &str) -> Result<Self> {
        let parse_err = |message: String| Error::ManifestParse {
            path: path.to_path_buf(),
            message,
        };

        let probe: file::VersionProbe =
            toml::from_str(text).map_err(|source| parse_err(source.to_string()))?;
        let found = probe
            .schema_version
            .ok_or_else(|| parse_err("missing or non-integer key `schema_version`".to_string()))?;

        if found > i64::from(SCHEMA_VERSION) {
            return Err(Error::UnsupportedSchemaVersion {
                path: path.to_path_buf(),
                // Saturating rather than wrapping: a manifest claiming 2^40 is still "from the
                // future", and reporting it as some small number would be a lie.
                found: u32::try_from(found).unwrap_or(u32::MAX),
                supported: SCHEMA_VERSION,
            });
        }
        if found < 1 {
            return Err(parse_err(format!(
                "schema_version must be at least 1, found {found}"
            )));
        }

        let raw: file::ManifestIn =
            toml::from_str(text).map_err(|source| parse_err(source.to_string()))?;

        let id = Uuid::parse_str(&raw.workspace.id).map_err(|_| Error::InvalidWorkspaceId {
            path: path.to_path_buf(),
            value: raw.workspace.id.clone(),
        })?;
        let kind = WorkspaceKind::parse(&raw.workspace.kind).ok_or_else(|| {
            Error::InvalidWorkspaceKind {
                path: path.to_path_buf(),
                value: raw.workspace.kind.clone(),
            }
        })?;
        let filename_style = match raw.notes.filename.as_deref() {
            None => FilenameStyle::default(),
            Some(value) => FilenameStyle::parse(value).ok_or_else(|| {
                parse_err(format!(
                    "unknown `[notes] filename` style `{value}` (expected `uuid` or `uuid_slug`)"
                ))
            })?,
        };

        Ok(Manifest {
            schema_version: found
                .try_into()
                .expect("checked to be within 1..=SCHEMA_VERSION above"),
            id,
            kind,
            name: raw
                .workspace
                .name
                .unwrap_or_else(|| default_name.to_string()),
            filename_style,
        })
    }
}

/// The on-disk shape of `workspace.toml`, kept private so the nesting never leaks into the API.
///
/// Reading and writing use different types on purpose. The writer borrows and is exhaustive; the
/// reader owns, makes the display-only fields optional, and — because `serde` ignores unknown
/// fields by default — tolerates keys a future version added, which is the same forward-compat
/// courtesy the note format extends to unknown frontmatter keys.
mod file {
    /// Written form. Field order is the emitted order, and scalars must precede tables in TOML.
    #[derive(serde::Serialize)]
    pub(super) struct Manifest<'a> {
        pub schema_version: u32,
        pub workspace: Workspace<'a>,
        pub notes: Notes<'a>,
    }

    #[derive(serde::Serialize)]
    pub(super) struct Workspace<'a> {
        pub id: String,
        pub kind: &'a str,
        pub name: &'a str,
    }

    #[derive(serde::Serialize)]
    pub(super) struct Notes<'a> {
        pub filename: &'a str,
    }

    /// Read just far enough to learn the schema version, and nothing else. Everything is optional
    /// so that a manifest from the future gets past this step and is refused for the right reason.
    #[derive(serde::Deserialize)]
    pub(super) struct VersionProbe {
        pub schema_version: Option<i64>,
    }

    /// Read form.
    #[derive(serde::Deserialize)]
    pub(super) struct ManifestIn {
        pub workspace: WorkspaceIn,
        #[serde(default)]
        pub notes: NotesIn,
    }

    #[derive(serde::Deserialize)]
    pub(super) struct WorkspaceIn {
        pub id: String,
        pub kind: String,
        pub name: Option<String>,
    }

    #[derive(Default, serde::Deserialize)]
    pub(super) struct NotesIn {
        pub filename: Option<String>,
    }
}

// =============================================================================================
// Workspace
// =============================================================================================

/// An opened workspace: a root directory and the manifest that identifies it.
///
/// Holding one says nothing about the index — that arrives in stage 2. In stage 1 a `Workspace` is
/// exactly the answer to "which directory, and what is it".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    root: PathBuf,
    manifest: Manifest,
}

impl Workspace {
    /// Creates a new workspace at `path`.
    ///
    /// Mints a UUIDv7 id, creates `.jot/`, `.jot/.trash/`, `.jot/tmp/`, and writes
    /// `.jot/workspace.toml` and `.jot/.gitignore`. `path` is created if it does not exist, and
    /// any files already in it — a folder of existing markdown, say — are left untouched (§U3).
    ///
    /// The workspace's display `name` is the basename of `path`.
    ///
    /// # Errors
    ///
    /// - [`Error::WorkspaceExists`] naming `path` if `.jot/` is already a directory there. This is
    ///   checked before anything is created, and `init` never overwrites an existing workspace.
    /// - [`Error::CreateDir`] if any directory in the tree cannot be created — including the case
    ///   where `.jot` exists but is a *file*.
    /// - [`Error::ManifestSerialize`], [`Error::Write`] or [`Error::Rename`] if the manifest or the
    ///   `.gitignore` cannot be written.
    ///
    /// If a step after directory creation fails, the `.jot/` this call created is removed again on
    /// a best-effort basis, so a transient failure does not leave behind a half-workspace that
    /// [`Error::WorkspaceExists`] would then refuse to re-`init` forever.
    pub fn init(path: &Path, kind: WorkspaceKind) -> Result<Self> {
        let root = absolutize(path);
        let jot = jot_dir(&root);

        if jot.is_dir() {
            return Err(Error::WorkspaceExists { path: root });
        }

        create_dir(&root)?;
        create_dir(&jot)?;

        let manifest = Manifest {
            schema_version: SCHEMA_VERSION,
            id: Uuid::now_v7(),
            kind,
            name: default_name(&root),
            filename_style: FilenameStyle::default(),
        };

        match Self::write_new_tree(&root, &jot, &manifest) {
            Ok(()) => Ok(Workspace { root, manifest }),
            Err(e) => {
                // `.jot/` did not exist a moment ago — this call made it — so removing it takes
                // nothing that was not ours.
                let _ = std::fs::remove_dir_all(&jot);
                Err(e)
            }
        }
    }

    /// Everything `init` does after `.jot/` exists, factored out so the failure path has one place
    /// to roll back from.
    fn write_new_tree(root: &Path, jot: &Path, manifest: &Manifest) -> Result<()> {
        create_dir(&jot.join(TRASH_DIR))?;
        let tmp = jot.join(TMP_DIR);
        create_dir(&tmp)?;

        let manifest_path = jot.join(MANIFEST_FILE);
        let text = manifest.to_toml(&manifest_path)?;
        fs::atomic_write(&manifest_path, &tmp, text.as_bytes())?;

        fs::atomic_write(&jot.join(".gitignore"), &tmp, GITIGNORE.as_bytes())?;

        debug_assert!(root.join(JOT_DIR).is_dir());
        Ok(())
    }

    /// Opens the workspace rooted at `path`.
    ///
    /// A read, start to finish: nothing under `path` is created or modified, including the staging
    /// directory. Per §U7 the registry is not consulted or written.
    ///
    /// # Errors
    ///
    /// - [`Error::NotAWorkspace`] naming `path` if it has no `.jot/` directory.
    /// - [`Error::Read`] naming `.jot/workspace.toml` if the manifest is missing or unreadable, and
    ///   [`Error::NotUtf8`] if it is not text.
    /// - [`Error::UnsupportedSchemaVersion`] if the manifest was written by a newer version.
    /// - [`Error::ManifestParse`], [`Error::InvalidWorkspaceId`] or
    ///   [`Error::InvalidWorkspaceKind`] for a manifest this build cannot make sense of.
    pub fn open(path: &Path) -> Result<Self> {
        let root = absolutize(path);
        if !jot_dir(&root).is_dir() {
            return Err(Error::NotAWorkspace { path: root });
        }

        let manifest_path = manifest_path_of(&root);
        let bytes = std::fs::read(&manifest_path).map_err(|source| Error::Read {
            path: manifest_path.clone(),
            source,
        })?;
        let text = String::from_utf8(bytes).map_err(|_| Error::NotUtf8 {
            path: manifest_path.clone(),
        })?;

        let manifest = Manifest::from_toml(&text, &manifest_path, &default_name(&root))?;
        Ok(Workspace { root, manifest })
    }

    /// Walks up from `from` looking for a `.jot/` directory, and opens the first one found.
    ///
    /// `from` itself is considered before its parents, so calling this from a workspace root
    /// works. The walk stops at the **nearest** workspace, not the outermost: with vaults nested
    /// inside vaults, the innermost one wins, because a note captured into the wrong vault is
    /// silently lost.
    ///
    /// Starting inside `.jot/` — from `.jot/tmp/`, say — lands on the vault root, since `.jot/`
    /// does not contain a `.jot/` of its own.
    ///
    /// `from` is made absolute and lexically normal first (see [`absolutize`]), so the walk never
    /// climbs *through* a `..` into a directory the caller did not name — `discover("../sibling")`
    /// considers `sibling` and its parents, never the vault the process happens to be standing in.
    /// This is why the answer is the same on Windows and on Linux.
    ///
    /// # Errors
    ///
    /// - [`Error::WorkspaceNotFound`] carrying `from` if the walk reaches the filesystem root
    ///   without finding a `.jot/`.
    /// - Whatever [`Workspace::open`] returns for the workspace that was found. A broken nearest
    ///   workspace is a failure, not a reason to keep walking up into someone else's vault.
    pub fn discover(from: &Path) -> Result<Self> {
        let start = absolutize(from);
        for candidate in start.ancestors() {
            if jot_dir(candidate).is_dir() {
                return Self::open(candidate);
            }
        }
        Err(Error::WorkspaceNotFound {
            from: from.to_path_buf(),
        })
    }

    /// The workspace root — the directory that contains `.jot/`, and the flat list of live notes.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The parsed `workspace.toml`.
    #[must_use]
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The workspace's minted id. Immutable for the life of the directory.
    #[must_use]
    pub fn id(&self) -> Uuid {
        self.manifest.id
    }

    /// `jot` or `plain`.
    #[must_use]
    pub fn kind(&self) -> WorkspaceKind {
        self.manifest.kind
    }

    /// The display name. Not an identifier — two workspaces may share one.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    /// `<root>/.jot`.
    #[must_use]
    pub fn jot_dir(&self) -> PathBuf {
        jot_dir(&self.root)
    }

    /// `<root>/.jot/tmp` — the staging directory every write in this vault passes through.
    #[must_use]
    pub fn tmp_dir(&self) -> PathBuf {
        self.jot_dir().join(TMP_DIR)
    }

    /// `<root>/.jot/.trash`.
    #[must_use]
    pub fn trash_dir(&self) -> PathBuf {
        fs::trash_dir(&self.root)
    }

    /// `<root>/.jot/workspace.toml`.
    #[must_use]
    pub fn manifest_path(&self) -> PathBuf {
        manifest_path_of(&self.root)
    }
}

// =============================================================================================
// Path helpers
// =============================================================================================

fn jot_dir(root: &Path) -> PathBuf {
    root.join(JOT_DIR)
}

fn manifest_path_of(root: &Path) -> PathBuf {
    jot_dir(root).join(MANIFEST_FILE)
}

/// Makes `path` absolute **and lexically normal** without touching the filesystem: the result
/// contains no `.` and no `..` component on any platform.
///
/// [`std::path::absolute`] rather than [`Path::canonicalize`] on purpose: canonicalizing requires
/// the path to exist (it does not, for `init`), and on Windows it returns a `\\?\` verbatim prefix
/// that would then show up in every error message a user reads.
///
/// # Why the normalization is not optional
///
/// [`std::path::absolute`] normalizes `.` and `..` on Windows (it is `GetFullPathNameW`) and, by
/// documented design, does **not** on POSIX, where `..` components are retained because resolving
/// them without the filesystem is wrong in the presence of symlinks. [`Workspace::discover`] then
/// walks [`Path::ancestors`], which is purely lexical, so an un-normalized `..` made the same call
/// return different workspaces on the two platforms:
///
/// ```text
/// cwd = /home/u/vaults/work, which is a vault. /home/u/vaults/personal is not.
/// discover("../personal")
///   POSIX, before: ancestors of `/home/u/vaults/work/../personal` include
///                  `/home/u/vaults/work` -> the WORK vault is returned.
///   Windows:       `C:\...\vaults\personal` -> WorkspaceNotFound.
/// ```
///
/// Normalizing here makes the walk mean the same thing everywhere. The symlink caveat that stops
/// POSIX from doing this in the standard library is accepted deliberately: a `..` that crosses a
/// symlink now resolves lexically rather than through the link, which is the same answer Windows
/// has always given and the same answer a user reading the path aloud would give. Vault roots are
/// directories the user typed; silently capturing a note into a *different vault* than the one the
/// path names is the far more expensive failure.
///
/// # The empty path
///
/// `Path::new("")` is **the current directory**, exactly as `.` is, and is resolved to it here.
/// This is a decision, not a fallback: [`std::path::absolute`] errors on an empty path, and the
/// previous "keep the path as given" behavior left `init("")` and `open("")` operating on the
/// process working directory while reporting an *empty* `root()` — a workspace whose every derived
/// path was silently relative. Rejecting instead was considered and would need an error variant
/// the frozen stage-1 taxonomy does not have.
///
/// Infallible: if the process has no working directory (it was deleted out from under us) the
/// relative path is kept rather than raising, which is no worse than the input.
fn absolutize(path: &Path) -> PathBuf {
    let base = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    };
    let absolute = std::path::absolute(base).unwrap_or_else(|_| base.to_path_buf());
    normalize_lexically(&absolute)
}

/// Removes every `.` and `..` component from `path` by pure text manipulation — no `stat`, no
/// symlink resolution, no requirement that anything exist.
///
/// `..` at (or above) a root or a Windows prefix is absorbed, matching `GetFullPathNameW` and the
/// kernel's own treatment of `/..`. A leading `..` in a path that is still relative — reachable
/// only when `std::path::absolute` failed — is retained, because there is nothing to pop it
/// against.
fn normalize_lexically(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut out: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                // `/..` is `/`, and `C:\..` is `C:\`.
                Some(Component::RootDir) => {}
                _ => out.push(component),
            },
            other => out.push(other),
        }
    }

    if out.is_empty() {
        // Everything cancelled out — only reachable for a relative path such as `a/..`, which
        // named the current directory to begin with.
        return PathBuf::from(".");
    }
    out.iter().collect()
}

/// The display name a workspace gets when the manifest does not supply one: the directory's
/// basename (§U3), spaces and all.
///
/// A path with no final component — a filesystem root such as `/` or `C:\` — falls back to a
/// constant. Initializing a vault at a filesystem root is not a thing anyone should do, but it
/// should not produce an empty name if someone does.
fn default_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "jot".to_string())
}

/// `create_dir_all`, with the path named on failure.
fn create_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|source| Error::CreateDir {
        path: path.to_path_buf(),
        source,
    })
}

// =============================================================================================
// Tests
// =============================================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Every path under `root`, relative and forward-slashed, sorted. Mirrors the acceptance
    /// suite's own tree walk so "the exact tree" means the same thing on both sides.
    fn tree(root: &Path) -> Vec<String> {
        fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).expect("read_dir").flatten() {
                let path = entry.path();
                out.push(
                    path.strip_prefix(root)
                        .expect("under root")
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
                if path.is_dir() {
                    walk(root, &path, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        out.sort();
        out
    }

    fn dir(parent: &Path, name: &str) -> PathBuf {
        let path = parent.join(name);
        std::fs::create_dir_all(&path).expect("create_dir_all");
        path
    }

    fn same_dir(a: &Path, b: &Path) -> bool {
        match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => a == b,
        }
    }

    // ----------------------------------------------------------------------------------- init

    /// The on-disk contract, read literally. A stray staging file left in `.jot/tmp/`, an
    /// `index.db` created early, or a missing `.trash/` all fail here.
    #[test]
    fn init_produces_exactly_the_on_disk_contract() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "Thoughts");

        Workspace::init(&root, WorkspaceKind::Jot).expect("init");

        assert_eq!(
            tree(&root),
            vec![
                ".jot",
                ".jot/.gitignore",
                ".jot/.trash",
                ".jot/tmp",
                ".jot/workspace.toml",
            ]
        );
        assert!(root.join(".jot/.trash").is_dir());
        assert!(root.join(".jot/tmp").is_dir());
        assert!(!root.join(".jot/index.db").exists(), "index.db is stage 2");
    }

    /// `atomic_write` stages inside `.jot/tmp/`; if a staged file were ever left behind, the very
    /// first thing a user did with a fresh vault would be to see it.
    #[test]
    fn init_leaves_the_staging_directory_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        Workspace::init(&root, WorkspaceKind::Jot).unwrap();

        assert_eq!(
            std::fs::read_dir(root.join(".jot/tmp")).unwrap().count(),
            0,
            "a staged file survived a successful write"
        );
    }

    #[test]
    fn init_writes_a_manifest_that_matches_the_documented_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "Thoughts");
        let ws = Workspace::init(&root, WorkspaceKind::Jot).unwrap();

        let text = std::fs::read_to_string(root.join(".jot/workspace.toml")).unwrap();
        let value: toml::Value = toml::from_str(&text).expect("manifest is valid TOML");

        assert_eq!(value["schema_version"].as_integer(), Some(1));
        assert_eq!(
            value["workspace"]["id"].as_str(),
            Some(ws.id().to_string().as_str())
        );
        assert_eq!(value["workspace"]["kind"].as_str(), Some("jot"));
        assert_eq!(value["workspace"]["name"].as_str(), Some("Thoughts"));
        assert_eq!(value["notes"]["filename"].as_str(), Some("uuid"));
    }

    #[test]
    fn init_mints_a_uuid_v7_workspace_id() {
        let tmp = tempfile::tempdir().unwrap();
        let a = Workspace::init(&dir(tmp.path(), "a"), WorkspaceKind::Jot).unwrap();
        let b = Workspace::init(&dir(tmp.path(), "b"), WorkspaceKind::Jot).unwrap();

        assert_eq!(a.id().get_version_num(), 7, "workspace ids are UUIDv7");
        assert_ne!(a.id(), b.id(), "each workspace mints its own id");
        assert_eq!(
            a.id().to_string(),
            a.id().to_string().to_lowercase(),
            "ids are written in the lowercase hyphenated form"
        );
    }

    #[test]
    fn init_writes_a_gitignore_covering_the_index_and_the_staging_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        Workspace::init(&root, WorkspaceKind::Jot).unwrap();

        let text = std::fs::read_to_string(root.join(".jot/.gitignore")).unwrap();
        let lines: Vec<&str> = text.lines().map(str::trim).collect();
        assert!(lines.contains(&"index.db*"), "{text}");
        assert!(lines.contains(&"tmp/"), "{text}");
        assert!(
            !text.contains('\r'),
            "the gitignore is written with LF endings"
        );
    }

    /// §U3: the second `init` is an error, and — the part that matters — the first workspace's
    /// identity survives it. A re-minted id would orphan every registry entry pointing here.
    #[test]
    fn init_on_an_existing_workspace_errors_and_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        Workspace::init(&root, WorkspaceKind::Jot).unwrap();

        let before = std::fs::read(root.join(".jot/workspace.toml")).unwrap();
        let err = Workspace::init(&root, WorkspaceKind::Jot).unwrap_err();

        assert!(
            matches!(err, Error::WorkspaceExists { .. }),
            "expected WorkspaceExists, got {err:?}"
        );
        assert!(err.to_string().contains("already"), "{err}");
        assert_eq!(
            std::fs::read(root.join(".jot/workspace.toml")).unwrap(),
            before,
            "a refused init must not rewrite the manifest"
        );
    }

    /// §U3 spells out that "existing workspace" means the directory exists — not that the manifest
    /// parses. A vault whose manifest was deleted must not be silently re-initialized under a new
    /// id.
    #[test]
    fn init_errors_on_a_bare_jot_directory_with_no_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        std::fs::create_dir(root.join(".jot")).unwrap();

        let err = Workspace::init(&root, WorkspaceKind::Jot).unwrap_err();
        assert!(matches!(err, Error::WorkspaceExists { .. }), "{err:?}");
    }

    #[test]
    fn init_creates_a_target_directory_that_does_not_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("not").join("yet").join("there");

        let ws = Workspace::init(&root, WorkspaceKind::Jot).expect("init creates the target");
        assert!(root.join(".jot/workspace.toml").is_file());
        assert_eq!(ws.name(), "there");
    }

    /// Adopting a folder of existing markdown is a supported path (§U3), and adoption that
    /// rewrote the notes it adopted would be a catastrophe rather than a feature.
    #[test]
    fn init_adopts_a_directory_that_already_has_files_without_touching_them() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "existing notes");
        let note = root.join("01a03d4c-c708-7cbf-83c0-883cedb7f1d5.md");
        let bytes = b"---\nid: 01a03d4c-c708-7cbf-83c0-883cedb7f1d5\n---\n\nkeep me\n";
        std::fs::write(&note, bytes).unwrap();

        Workspace::init(&root, WorkspaceKind::Jot).expect("adoption is a valid init");

        assert_eq!(std::fs::read(&note).unwrap(), bytes);
        assert_eq!(
            Workspace::open(&root).unwrap().name(),
            "existing notes",
            "the basename becomes the name, spaces and all"
        );
    }

    #[test]
    fn init_records_the_kind_it_was_given() {
        let tmp = tempfile::tempdir().unwrap();
        for (name, kind) in [("j", WorkspaceKind::Jot), ("p", WorkspaceKind::Plain)] {
            let root = dir(tmp.path(), name);
            Workspace::init(&root, kind).unwrap();
            assert_eq!(Workspace::open(&root).unwrap().kind(), kind);
        }
    }

    /// `.jot` as a *file* is not `.jot/` as a directory, so `WorkspaceExists` would be the wrong
    /// answer; the honest one names the directory that could not be created, and the user's file
    /// is left alone.
    #[test]
    fn init_errors_when_jot_exists_as_a_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        std::fs::write(root.join(".jot"), b"not a directory").unwrap();

        let err = Workspace::init(&root, WorkspaceKind::Jot).unwrap_err();
        assert!(matches!(err, Error::CreateDir { .. }), "{err:?}");
        assert_eq!(
            std::fs::read(root.join(".jot")).unwrap(),
            b"not a directory"
        );
    }

    #[test]
    fn init_returns_a_workspace_whose_accessors_point_into_the_tree_it_made() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        let ws = Workspace::init(&root, WorkspaceKind::Jot).unwrap();

        assert!(same_dir(ws.root(), &root));
        assert_eq!(ws.jot_dir(), ws.root().join(".jot"));
        assert_eq!(ws.tmp_dir(), ws.root().join(".jot").join("tmp"));
        assert_eq!(ws.trash_dir(), ws.root().join(".jot").join(".trash"));
        assert_eq!(
            ws.manifest_path(),
            ws.root().join(".jot").join("workspace.toml")
        );
        assert!(ws.tmp_dir().is_dir() && ws.trash_dir().is_dir());
    }

    #[test]
    fn init_accepts_a_relative_path_and_reports_an_absolute_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        // A relative path is what a CLI gets; `root()` must still be usable as a base for joins
        // after the process changes directory.
        let relative = root.join("nested").join("..").join("nested");
        std::fs::create_dir_all(root.join("nested")).unwrap();

        let ws = Workspace::init(&relative, WorkspaceKind::Jot).unwrap();
        assert!(ws.root().is_absolute(), "{}", ws.root().display());
    }

    // ----------------------------------------------------------------------------------- open

    #[test]
    fn open_round_trips_what_init_wrote_without_rewriting_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "Thoughts");
        let written = Workspace::init(&root, WorkspaceKind::Jot).unwrap();

        let before = std::fs::read(root.join(".jot/workspace.toml")).unwrap();
        let opened = Workspace::open(&root).expect("open must accept what init wrote");

        assert_eq!(opened.manifest(), written.manifest());
        assert!(same_dir(opened.root(), &root));
        assert_eq!(
            std::fs::read(root.join(".jot/workspace.toml")).unwrap(),
            before,
            "open is a read"
        );
        assert_eq!(tree(&root).len(), 5, "open must not add to the vault");
    }

    #[test]
    fn open_on_a_directory_with_no_jot_is_not_a_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let err = Workspace::open(tmp.path()).unwrap_err();
        assert!(matches!(err, Error::NotAWorkspace { .. }), "{err:?}");
    }

    /// A `.jot/` with no manifest is a broken workspace, not a missing one, and the error has to
    /// name the file that is absent rather than the directory that is present.
    #[test]
    fn open_with_a_missing_manifest_names_the_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        std::fs::create_dir(root.join(".jot")).unwrap();

        let err = Workspace::open(&root).unwrap_err();
        assert!(matches!(err, Error::Read { .. }), "{err:?}");
        assert!(err.to_string().contains("workspace.toml"), "{err}");
    }

    #[test]
    fn open_refuses_a_schema_version_from_the_future() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        Workspace::init(&root, WorkspaceKind::Jot).unwrap();

        let manifest = root.join(".jot/workspace.toml");
        let bumped = std::fs::read_to_string(&manifest)
            .unwrap()
            .replace("schema_version = 1", "schema_version = 9999");
        std::fs::write(&manifest, bumped).unwrap();

        let err = Workspace::open(&root).unwrap_err();
        match err {
            Error::UnsupportedSchemaVersion {
                found, supported, ..
            } => {
                assert_eq!((found, supported), (9999, SCHEMA_VERSION));
            }
            other => panic!("expected UnsupportedSchemaVersion, got {other:?}"),
        }
        let message = Workspace::open(&root).unwrap_err().to_string();
        assert!(message.contains("9999"), "{message}");
        assert!(message.contains("newer version"), "{message}");
    }

    /// The version is read on its own, before the rest of the manifest, so a future format that
    /// also renames or drops keys still reports "written by a newer version" instead of a parse
    /// error that sends the user looking for corruption.
    #[test]
    fn open_reports_a_future_version_even_when_the_rest_is_unrecognizable() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        std::fs::create_dir(root.join(".jot")).unwrap();
        std::fs::write(
            root.join(".jot/workspace.toml"),
            "schema_version = 4\n\n[vault]\nurn = \"jot:whatever\"\n",
        )
        .unwrap();

        let err = Workspace::open(&root).unwrap_err();
        assert!(
            matches!(err, Error::UnsupportedSchemaVersion { found: 4, .. }),
            "{err:?}"
        );
    }

    /// A version below 1 is not "from the future", so saying so would be a lie; it is a manifest
    /// this build cannot make sense of.
    #[test]
    fn open_rejects_a_schema_version_below_one_as_a_parse_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        std::fs::create_dir(root.join(".jot")).unwrap();
        std::fs::write(root.join(".jot/workspace.toml"), "schema_version = 0\n").unwrap();

        let err = Workspace::open(&root).unwrap_err();
        assert!(matches!(err, Error::ManifestParse { .. }), "{err:?}");
        assert!(err.to_string().contains('0'), "{err}");
    }

    #[test]
    fn open_rejects_a_manifest_with_no_schema_version() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        std::fs::create_dir(root.join(".jot")).unwrap();
        std::fs::write(
            root.join(".jot/workspace.toml"),
            "[workspace]\nid = \"01a03d4c-3680-7c70-aade-6c016dd177d2\"\nkind = \"jot\"\n",
        )
        .unwrap();

        let err = Workspace::open(&root).unwrap_err();
        assert!(matches!(err, Error::ManifestParse { .. }), "{err:?}");
        assert!(err.to_string().contains("schema_version"), "{err}");
    }

    #[test]
    fn open_rejects_malformed_toml_naming_the_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        std::fs::create_dir(root.join(".jot")).unwrap();
        std::fs::write(root.join(".jot/workspace.toml"), "schema_version = = 1\n").unwrap();

        let err = Workspace::open(&root).unwrap_err();
        assert!(matches!(err, Error::ManifestParse { .. }), "{err:?}");
        assert!(err.to_string().contains("workspace.toml"), "{err}");
    }

    #[test]
    fn open_rejects_an_unknown_kind_and_a_non_uuid_id() {
        let tmp = tempfile::tempdir().unwrap();

        let bad_kind = dir(tmp.path(), "k");
        std::fs::create_dir(bad_kind.join(".jot")).unwrap();
        std::fs::write(
            bad_kind.join(".jot/workspace.toml"),
            "schema_version = 1\n\n[workspace]\nid = \"01a03d4c-3680-7c70-aade-6c016dd177d2\"\nkind = \"banana\"\n",
        )
        .unwrap();
        let err = Workspace::open(&bad_kind).unwrap_err();
        assert!(matches!(err, Error::InvalidWorkspaceKind { .. }), "{err:?}");
        assert!(err.to_string().contains("banana"), "{err}");

        let bad_id = dir(tmp.path(), "i");
        std::fs::create_dir(bad_id.join(".jot")).unwrap();
        std::fs::write(
            bad_id.join(".jot/workspace.toml"),
            "schema_version = 1\n\n[workspace]\nid = \"nope\"\nkind = \"jot\"\n",
        )
        .unwrap();
        let err = Workspace::open(&bad_id).unwrap_err();
        assert!(matches!(err, Error::InvalidWorkspaceId { .. }), "{err:?}");
        assert!(err.to_string().contains("nope"), "{err}");
    }

    /// `name` and `[notes]` are display and configuration, not identity. A hand-edited manifest
    /// that drops them must still open.
    #[test]
    fn open_defaults_the_name_and_the_filename_style_when_the_manifest_omits_them() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "Field Notes");
        std::fs::create_dir(root.join(".jot")).unwrap();
        std::fs::write(
            root.join(".jot/workspace.toml"),
            "schema_version = 1\n\n[workspace]\nid = \"01a03d4c-3680-7c70-aade-6c016dd177d2\"\nkind = \"jot\"\n",
        )
        .unwrap();

        let ws = Workspace::open(&root).unwrap();
        assert_eq!(ws.name(), "Field Notes");
        assert_eq!(ws.manifest().filename_style, FilenameStyle::Uuid);
    }

    #[test]
    fn open_accepts_the_uuid_slug_filename_style_and_rejects_anything_else() {
        let tmp = tempfile::tempdir().unwrap();
        let base = "schema_version = 1\n\n[workspace]\nid = \"01a03d4c-3680-7c70-aade-6c016dd177d2\"\nkind = \"jot\"\nname = \"V\"\n\n[notes]\nfilename = ";

        let good = dir(tmp.path(), "g");
        std::fs::create_dir(good.join(".jot")).unwrap();
        std::fs::write(
            good.join(".jot/workspace.toml"),
            format!("{base}\"uuid_slug\"\n"),
        )
        .unwrap();
        assert_eq!(
            Workspace::open(&good).unwrap().manifest().filename_style,
            FilenameStyle::UuidSlug
        );

        let bad = dir(tmp.path(), "b");
        std::fs::create_dir(bad.join(".jot")).unwrap();
        std::fs::write(
            bad.join(".jot/workspace.toml"),
            format!("{base}\"emoji\"\n"),
        )
        .unwrap();
        let err = Workspace::open(&bad).unwrap_err();
        assert!(matches!(err, Error::ManifestParse { .. }), "{err:?}");
        assert!(err.to_string().contains("emoji"), "{err}");
    }

    /// Forward compatibility inside one schema version: a key this build has never heard of must
    /// not stop it opening the vault. `open` never rewrites, so the key survives by construction.
    #[test]
    fn open_ignores_manifest_keys_it_does_not_know() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        Workspace::init(&root, WorkspaceKind::Jot).unwrap();

        let manifest = root.join(".jot/workspace.toml");
        let mut text = std::fs::read_to_string(&manifest).unwrap();
        text.push_str("\n[sync]\nprovider = \"none\"\n");
        std::fs::write(&manifest, &text).unwrap();

        Workspace::open(&root).expect("an unknown table must not break open");
        assert_eq!(
            std::fs::read_to_string(&manifest).unwrap(),
            text,
            "open preserves unknown keys by never writing"
        );
    }

    #[test]
    fn open_on_the_shared_fixture_vault_reads_its_identity() {
        let vault = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("tests")
            .join("fixtures")
            .join("vault");

        let ws = Workspace::open(&vault).expect("the fixture vault must open");
        assert_eq!(ws.kind(), WorkspaceKind::Jot);
        assert_eq!(ws.name(), "Fixture Vault");
        assert_eq!(ws.id().to_string(), "01a03d4c-3680-7c70-aade-6c016dd177d2");
    }

    // ------------------------------------------------------------------------------- discover

    #[test]
    fn discover_finds_the_workspace_from_three_directories_deep() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "vault");
        Workspace::init(&root, WorkspaceKind::Jot).unwrap();
        let deep = dir(&root, "one/two/three");

        let found = Workspace::discover(&deep).expect("discover walks up");
        assert!(same_dir(found.root(), &root));
        assert_eq!(
            std::fs::read_dir(&deep).unwrap().count(),
            0,
            "discover must create nothing where it was called from"
        );
    }

    #[test]
    fn discover_considers_the_starting_directory_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "vault");
        Workspace::init(&root, WorkspaceKind::Jot).unwrap();

        assert!(same_dir(Workspace::discover(&root).unwrap().root(), &root));
    }

    /// The failure this guards is the expensive one: a note captured into the outer vault when the
    /// user is standing in the inner one is silently lost.
    #[test]
    fn discover_stops_at_the_nearest_workspace_not_the_outermost() {
        let tmp = tempfile::tempdir().unwrap();
        let outer = dir(tmp.path(), "outer");
        let inner = dir(&outer, "a/inner");
        Workspace::init(&outer, WorkspaceKind::Jot).unwrap();
        Workspace::init(&inner, WorkspaceKind::Jot).unwrap();

        let found = Workspace::discover(&dir(&inner, "x/y/z")).unwrap();
        assert!(
            same_dir(found.root(), &inner),
            "discover returned {} instead of {}",
            found.root().display(),
            inner.display()
        );
        assert_ne!(found.id(), Workspace::open(&outer).unwrap().id());
    }

    /// `.jot/` contains no `.jot/`, so walking up from inside it lands on the vault root rather
    /// than treating the metadata directory as a workspace of its own.
    #[test]
    fn discover_from_inside_the_jot_directory_lands_on_the_vault_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "vault");
        Workspace::init(&root, WorkspaceKind::Jot).unwrap();

        for start in [root.join(".jot"), root.join(".jot").join("tmp")] {
            let found = Workspace::discover(&start).unwrap();
            assert!(same_dir(found.root(), &root), "from {}", start.display());
        }
    }

    #[test]
    fn discover_with_no_workspace_anywhere_above_names_the_starting_path() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = dir(tmp.path(), "a/b/c");

        let err = Workspace::discover(&deep).unwrap_err();
        match &err {
            Error::WorkspaceNotFound { from } => assert_eq!(from, &deep),
            other => panic!("expected WorkspaceNotFound, got {other:?}"),
        }
        assert!(err.to_string().contains("c"), "{err}");
    }

    /// A broken nearest workspace is a failure to report, not a reason to keep climbing into a
    /// vault the user was not standing in.
    #[test]
    fn discover_does_not_fall_through_a_broken_nearest_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let outer = dir(tmp.path(), "outer");
        Workspace::init(&outer, WorkspaceKind::Jot).unwrap();
        let inner = dir(&outer, "inner");
        std::fs::create_dir(inner.join(".jot")).unwrap();

        let err = Workspace::discover(&dir(&inner, "deep")).unwrap_err();
        assert!(
            matches!(err, Error::Read { .. }),
            "expected the inner vault's own failure, got {err:?}"
        );
    }

    // ------------------------------------------------------------ path normalization (F3)
    //
    // Every assertion in this section is written to hold *identically* on Windows and on POSIX.
    // That cross-platform agreement is the property being tested: `std::path::absolute` normalizes
    // `.` and `..` on Windows and deliberately does not on POSIX, so before `absolutize`
    // normalized for itself, the three behaviors below differed by platform and only the Windows
    // one was ever observed.

    /// The bug in one test. `work` is a vault, `personal` is not, and `personal` is reached
    /// through a `..` that passes over `work`.
    ///
    /// Lexically, `<tmp>/work/../personal`'s ancestors include `<tmp>/work` — so an un-normalized
    /// walk finds the *work* vault and hands back a workspace the caller never named. On Windows
    /// the same call has always returned `WorkspaceNotFound`, because `std::path::absolute` folded
    /// the `..` away before the walk started. Both platforms must now say `WorkspaceNotFound`.
    #[test]
    fn discover_does_not_climb_through_a_parent_dir_into_a_vault_the_caller_did_not_name() {
        let tmp = tempfile::tempdir().unwrap();
        let work = dir(tmp.path(), "work");
        dir(tmp.path(), "personal");
        Workspace::init(&work, WorkspaceKind::Jot).unwrap();

        let sibling = work.join("..").join("personal");
        match Workspace::discover(&sibling) {
            Err(Error::WorkspaceNotFound { .. }) => {}
            Ok(found) => panic!(
                "discover walked through `..` and returned {}",
                found.root().display()
            ),
            Err(other) => panic!("expected WorkspaceNotFound, got {other:?}"),
        }
    }

    /// The other direction: a `..` that cancels out *within* the vault must still find it, so the
    /// fix above is normalization and not a blanket refusal of `..`.
    #[test]
    fn discover_still_finds_the_vault_through_a_dot_and_a_parent_dir_that_cancel_out() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "vault");
        Workspace::init(&root, WorkspaceKind::Jot).unwrap();
        let sub = dir(&root, "sub");

        for start in [
            sub.join("..").join("sub"),
            root.join(".").join("sub"),
            sub.join("does-not-exist").join(".."),
        ] {
            let found = Workspace::discover(&start).unwrap_or_else(|e| {
                panic!("discover({}) failed: {e}", start.display());
            });
            assert_eq!(
                found.root(),
                root.as_path(),
                "from {}, root() must be the vault itself with nothing left to normalize",
                start.display()
            );
        }
    }

    /// `root()` is joined onto for every note path and printed in every error message, so a `.` or
    /// a `..` surviving in it leaks into both. On POSIX `discover(".")` used to report
    /// `<cwd>/.`; here the workspace is reached through both a `.` and a `..` that does not exist
    /// on disk — normalization is lexical, so the missing component is not a problem.
    #[test]
    fn root_never_keeps_a_dot_or_parent_dir_component_from_the_path_it_was_given() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "vault");

        let weird = root.join(".").join("nowhere").join("..");
        let ws = Workspace::init(&weird, WorkspaceKind::Jot).expect("init");
        assert_eq!(ws.root(), root.as_path());
        assert!(root.join(".jot").is_dir(), "and it initialized the vault");
        assert_eq!(Workspace::open(&weird).unwrap().root(), root.as_path());
        assert_eq!(ws.name(), "vault", "the basename, not `..`");
    }

    /// `absolutize` is the one place the platforms disagreed, so it is asserted directly as well
    /// as through its callers.
    #[test]
    fn absolutize_is_absolute_and_free_of_dot_components_on_every_platform() {
        use std::path::Component;

        let cwd = std::env::current_dir().unwrap();
        assert_eq!(absolutize(Path::new(".")), cwd);
        assert_eq!(absolutize(Path::new("a/./b/../c")), cwd.join("a").join("c"));
        assert_eq!(absolutize(Path::new("a/b/..")), cwd.join("a"));

        for input in ["", ".", "..", "a/./b", "../x", "a/b/../..", "./a/../a/"] {
            let out = absolutize(Path::new(input));
            assert!(out.is_absolute(), "`{input}` -> {}", out.display());
            assert!(
                out.components()
                    .all(|c| !matches!(c, Component::CurDir | Component::ParentDir)),
                "`{input}` -> {} still carries a `.` or `..`",
                out.display()
            );
        }
    }

    /// `..` above a root is absorbed rather than escaping into a path no filesystem can name.
    #[test]
    fn normalize_lexically_absorbs_parent_dirs_at_the_root() {
        let root = absolutize(Path::new("/"));
        assert_eq!(normalize_lexically(&root.join("..")), root);
        assert_eq!(normalize_lexically(&root.join("..").join("..")), root);
        assert_eq!(normalize_lexically(&root.join("a").join("..")), root);
    }

    /// An empty path is the current directory — the same as `.` — and says so in `root()` and in
    /// error messages. It used to be neither: `std::path::absolute("")` errors, the old fallback
    /// kept the empty path, and `init("")` then created `.jot/` in the process working directory
    /// while reporting `root() == ""`.
    #[test]
    fn an_empty_path_means_the_current_directory_and_reports_it_absolutely() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(absolutize(Path::new("")), cwd);

        // Whatever the working directory happens to be, `""` and `.` must be the same call.
        let outcome = |path: &str| match Workspace::open(Path::new(path)) {
            Ok(ws) => ws.root().to_path_buf(),
            Err(e) => e.path().expect("open names a path").to_path_buf(),
        };
        assert_eq!(outcome(""), cwd);
        assert_eq!(outcome(""), outcome("."));
    }

    // -------------------------------------------------------------------------------- pieces

    #[test]
    fn kind_and_filename_style_round_trip_through_their_manifest_spellings() {
        for kind in [WorkspaceKind::Jot, WorkspaceKind::Plain] {
            assert_eq!(WorkspaceKind::parse(kind.as_str()), Some(kind));
            assert_eq!(kind.to_string(), kind.as_str());
        }
        assert_eq!(WorkspaceKind::parse("Jot"), None, "the spelling is exact");
        assert_eq!(WorkspaceKind::parse(""), None);

        for style in [FilenameStyle::Uuid, FilenameStyle::UuidSlug] {
            assert_eq!(FilenameStyle::parse(style.as_str()), Some(style));
            assert_eq!(style.to_string(), style.as_str());
        }
        assert_eq!(FilenameStyle::parse("slug"), None);
        assert_eq!(FilenameStyle::default(), FilenameStyle::Uuid);
    }

    /// The emitted manifest must parse back into the same values, or `init` and `open` disagree
    /// about the vault they are looking at.
    #[test]
    fn manifest_text_round_trips_through_itself() {
        let path = Path::new("v/.jot/workspace.toml");
        let manifest = Manifest {
            schema_version: SCHEMA_VERSION,
            id: Uuid::parse_str("01a03d4c-3680-7c70-aade-6c016dd177d2").unwrap(),
            kind: WorkspaceKind::Plain,
            name: "Name with spaces, a \" quote and 한글".to_string(),
            filename_style: FilenameStyle::UuidSlug,
        };

        let text = manifest.to_toml(path).unwrap();
        assert_eq!(
            Manifest::from_toml(&text, path, "ignored").unwrap(),
            manifest
        );
    }

    /// The emitted manifest, pinned literally.
    ///
    /// `workspace.toml` is a file users open and edit; a TOML emitter that started inlining the
    /// tables or reordering the keys would still round-trip through `from_toml` and would still
    /// pass every other test here, while quietly making the file worse to read. This is the only
    /// place that notices.
    #[test]
    fn the_emitted_manifest_looks_exactly_like_the_documented_shape() {
        let manifest = Manifest {
            schema_version: SCHEMA_VERSION,
            id: Uuid::parse_str("01a03d4c-3680-7c70-aade-6c016dd177d2").unwrap(),
            kind: WorkspaceKind::Jot,
            name: "Thoughts".to_string(),
            filename_style: FilenameStyle::Uuid,
        };

        assert_eq!(
            manifest
                .to_toml(Path::new("v/.jot/workspace.toml"))
                .unwrap(),
            "\
# jot workspace manifest.
# `id` is minted once and must never change — it is what makes this directory self-identifying.
# `name` is display-only and safe to edit by hand.

schema_version = 1

[workspace]
id = \"01a03d4c-3680-7c70-aade-6c016dd177d2\"
kind = \"jot\"
name = \"Thoughts\"

[notes]
filename = \"uuid\"
"
        );
    }

    #[test]
    fn default_name_falls_back_when_a_path_has_no_final_component() {
        assert_eq!(default_name(Path::new("/")), "jot");
        assert_eq!(default_name(Path::new("a/b/Field Notes")), "Field Notes");
    }
}
