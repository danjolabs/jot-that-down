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
//! `index.db` is stage 4 and is deliberately **not** created here: the index is derived and
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
//!   `registry::*` call that the CLI wires up in stage 3.
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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::error::{Error, Result};
use crate::frontmatter::{FieldType, FrontmatterEntry, FrontmatterSchema, Role};
use crate::fs::{self, FilenameSlug};
use crate::link::{self, Link};
use crate::note::{Note, NoteId, NoteMeta};
use crate::query::{
    Draft, Edit, FileSort, Page, Ref, Resolution, Row, SearchQuery, State, TimelineQuery,
};
use crate::snapshot::{Problem, Snapshot, SyncReport};
use crate::thread::Thread;

/// The manifest schema version this build writes, and the highest it will open.
pub const SCHEMA_VERSION: u32 = 2;

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
// The manifest
// =============================================================================================

/// The contents of `.jot/workspace.toml`, flattened.
///
/// The TOML file nests `id`/`name` under `[workspace]` and declares each frontmatter entry as its
/// own `[[schema.frontmatter]]` table; in
/// memory that nesting buys nothing, so the on-disk shape lives in the private `file` types below
/// and this is what callers see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// The version of the manifest format this file was written against.
    pub schema_version: u32,
    /// Minted at `init`, immutable thereafter. What makes the directory self-identifying.
    ///
    /// **UUIDv4, where a note id is v7**, and the asymmetry is the point. A note id is v7 because
    /// two things read the timestamp out of it: `created_at` is decoded from the identity, and id
    /// order *is* creation order, which is what sibling ordering, the timeline, and keyset
    /// pagination all rest on. A workspace id is asked for none of that — nothing in the crate
    /// decodes its time or sorts on it.
    ///
    /// Using v7 anyway would cost two things and buy nothing:
    ///
    /// * **Short ids stop working.** A v7's leading 48 bits are a millisecond timestamp, so two
    ///   workspaces created in the same minute share their first eight hex characters and
    ///   `jot ws ls` has to widen the abbreviation to tell them apart. A v4 is random from its
    ///   first bit, so eight characters separate every workspace anyone will have.
    /// * **It leaks a date.** This id is written into `workspace.toml`, which people commit, and
    ///   a v7 encodes when the vault was created.
    ///
    /// Reading is unaffected either way — `open` parses any UUID version — so vaults created
    /// before this are correct and stay correct.
    pub id: Uuid,
    /// Display only. Defaults to the target directory's basename at `init` (§U3) and is expected
    /// to be edited by hand.
    pub name: String,
    /// The declared frontmatter entries, in the order notes are written in.
    ///
    /// Absent from the file means [`FrontmatterSchema::jot_default`] — which is what lets a
    /// stage-1 manifest, written before `[schema]` existed, open without a migration.
    pub schema: FrontmatterSchema,
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
                name: &self.name,
            },
            schema: file::Schema {
                frontmatter: self
                    .schema
                    .entries()
                    .iter()
                    .map(|entry| {
                        let declared = entry.field_type().as_str();
                        file::Entry {
                            // `key` defaults to the type string verbatim, so an entry that takes
                            // the default writes no `key` at all. That is what keeps a manifest
                            // reading as `type = "relation:reply_to"` on one line rather than as
                            // the same string twice.
                            key: (entry.key() != declared).then(|| entry.key().to_string()),
                            field_type: declared,
                            required: entry.is_required().then_some(true),
                        }
                    })
                    .collect(),
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
    ///
    /// # v1 manifests are promoted, not refused
    ///
    /// v1 declared `[schema] frontmatter` as an array of plain strings, which said what keys a
    /// note may carry and nothing about what any of them meant. Each such string becomes an entry
    /// here — the four names v1 interpreted map to their roles, everything else becomes `text` —
    /// and the manifest is rewritten as v2 the next time anything writes it.
    ///
    /// The **manifest is strict**: a v2 manifest that contradicts itself, by claiming a reserved
    /// role twice, is an [`Error::ManifestParse`] rather than a silent first-wins. Note *files*
    /// are never rejected; the two policies are stated together so the type system's strictness
    /// cannot leak into user data.
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

        let entries = match raw.schema.frontmatter {
            // A manifest with no `[schema]` predates the key, so it gets the default rather than
            // an empty schema — an empty one would render notes with no keys at all.
            None => None,
            Some(file::Declared::V1(keys)) => Some(promote_v1(&keys)),
            Some(file::Declared::V2(entries)) => Some(
                entries
                    .into_iter()
                    .map(|e| {
                        let field_type = FieldType::parse(&e.field_type);
                        let entry = match e.key {
                            Some(key) => FrontmatterEntry::with_key(key, field_type),
                            None => FrontmatterEntry::new(field_type),
                        };
                        entry.required(e.required.unwrap_or(false))
                    })
                    .collect(),
            ),
        };

        let schema = match entries {
            None => FrontmatterSchema::jot_default(),
            Some(entries) => FrontmatterSchema::try_new(entries)
                .map_err(|message| parse_err(format!("`[[schema.frontmatter]]`: {message}")))?,
        };

        Ok(Manifest {
            schema_version: found
                .try_into()
                .expect("checked to be within 1..=SCHEMA_VERSION above"),
            id,
            name: raw
                .workspace
                .name
                .unwrap_or_else(|| default_name.to_string()),
            schema,
        })
    }
}

/// A v1 `[schema] frontmatter` string list, read as typed entries.
///
/// The four names v1 interpreted map to their roles; every other string becomes `text`, which is
/// the honest reading — v1 said nothing about what those keys held.
///
/// `relation:root` is **dropped**. It was a denormalized cache of a walk that is now done at scan
/// time, so a schema entry for it would declare a key nothing writes. In note files it is not
/// migrated either: it simply becomes an undeclared key, preserved and ignored, which is the one
/// place the forward-compat rule visibly pays for itself.
///
/// The title comes back `required = true`, matching [`FrontmatterSchema::jot_default`]. v1 had no
/// way to say it, and v1's `$EDITOR` buffer offered a blank for every declared key — so requiring
/// the title is what *preserves* the promoted vault's behaviour for the one key that had it. No
/// other entry is promoted to required: for the rest, v1's buffer blanks were core's decision and
/// this refactor hands that decision to the manifest.
fn promote_v1(keys: &[String]) -> Vec<FrontmatterEntry> {
    keys.iter()
        .filter(|key| key.as_str() != "relation:root")
        .map(|key| match key.as_str() {
            "title" => {
                FrontmatterEntry::with_key("title", FieldType::Reserved(Role::Title)).required(true)
            }
            "relation:reply_to" => FrontmatterEntry::new(FieldType::Reserved(Role::ReplyTo)),
            // v1 spelled it `relation:quote`; v2 renames it for symmetry with `relation:reply_to`.
            // The *key* is kept as it was, so notes written under v1 still round-trip.
            "relation:quote" => {
                FrontmatterEntry::with_key("relation:quote", FieldType::Reserved(Role::QuoteTo))
            }
            other => FrontmatterEntry::with_key(other, FieldType::Text(None)),
        })
        .collect()
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
        pub schema: Schema,
    }

    #[derive(serde::Serialize)]
    pub(super) struct Workspace<'a> {
        pub id: String,
        pub name: &'a str,
    }

    #[derive(serde::Serialize)]
    pub(super) struct Schema {
        pub frontmatter: Vec<Entry>,
    }

    /// One `[[schema.frontmatter]]` table.
    ///
    /// `key` and `required` are skipped when they are at their defaults, so the common entry is
    /// one line. An array of tables rather than an array of strings because order is load-bearing
    /// — it fixes emission order, and stage 3's `$EDITOR` template depends on it — and because
    /// each entry needs room to grow per-key fields.
    #[derive(serde::Serialize)]
    pub(super) struct Entry {
        #[serde(skip_serializing_if = "Option::is_none")]
        pub key: Option<String>,
        #[serde(rename = "type")]
        pub field_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub required: Option<bool>,
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
        pub schema: SchemaIn,
    }

    #[derive(serde::Deserialize)]
    pub(super) struct WorkspaceIn {
        pub id: String,
        pub name: Option<String>,
    }

    #[derive(Default, serde::Deserialize)]
    pub(super) struct SchemaIn {
        pub frontmatter: Option<Declared>,
    }

    /// Both spellings of `schema.frontmatter`, told apart by shape rather than by the declared
    /// version.
    ///
    /// Reading the shape is what lets a v1 manifest open: TOML cannot hold both an array of
    /// strings and an array of tables under one key, so the two are unambiguous, and a file whose
    /// `schema_version` disagrees with its own contents still opens as whatever it actually is.
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    pub(super) enum Declared {
        /// v1: `frontmatter = ["title", "relation:reply_to", …]`.
        V1(Vec<String>),
        /// v2: a `[[schema.frontmatter]]` per entry.
        V2(Vec<EntryIn>),
    }

    #[derive(serde::Deserialize)]
    pub(super) struct EntryIn {
        pub key: Option<String>,
        #[serde(rename = "type")]
        pub field_type: String,
        pub required: Option<bool>,
    }
}

// =============================================================================================
// Warnings
// =============================================================================================

/// Something about an opened workspace that is worth telling the user and is not worth refusing to
/// open over.
///
/// A warning channel rather than an error because the alternative in each case is worse. Refusing
/// to open a vault whose `[schema] frontmatter` omits `relation:reply_to` would lock the user out
/// of their own notes over a config line they can fix in ten seconds — and it is not needed for
/// safety, because [`crate::frontmatter::Frontmatter::try_render`] writes an interpreted key the
/// schema omits anyway. The omission costs diff shape, never thread structure.
///
/// Collected at `init` and `open`, read through [`Workspace::warnings`], and surfaced by whichever
/// surface is in front of the user. `jot-core` does not log.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Warning {
    /// A `[[schema.frontmatter]]` entry declares a `type` this build does not understand.
    ///
    /// The key still round-trips untouched: an unknown type behaves exactly like an undeclared
    /// key, which is the forward-compat rule applied to the type system *before* the type system
    /// gives anyone the chance to break it. A newer jot will write types an older binary has never
    /// heard of, and the older binary must not damage the file.
    UnknownFrontmatterTypes {
        /// The manifest the schema was read from.
        path: PathBuf,
        /// `(key, declared type)` for each entry this build cannot interpret, in declared order.
        entries: Vec<(String, String)>,
    },
}

impl std::fmt::Display for Warning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Warning::UnknownFrontmatterTypes { path, entries } => write!(
                f,
                "`{}`: unknown frontmatter {} {} — {} preserved and never interpreted",
                path.display(),
                if entries.len() == 1 { "type" } else { "types" },
                entries
                    .iter()
                    .map(|(key, ty)| format!("`{ty}` on `{key}`"))
                    .collect::<Vec<_>>()
                    .join(", "),
                if entries.len() == 1 {
                    "that key is"
                } else {
                    "those keys are"
                },
            ),
        }
    }
}

// =============================================================================================
// Workspace
// =============================================================================================

/// An opened workspace: a root directory, the manifest that identifies it, and an in-memory view
/// of the notes inside it.
///
/// The view is a [`Snapshot`] — a scan, standing in for stage 4's SQLite index. Every read method
/// answers from it and every mutation updates it, so the seam the whole project rests on is the
/// same one it will be when a database is behind it: **surfaces call these methods and never touch
/// the filesystem.**
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    root: PathBuf,
    manifest: Manifest,
    warnings: Vec<Warning>,
    snapshot: Snapshot,
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
    pub fn init(path: &Path) -> Result<Self> {
        let root = absolutize(path);
        let jot = jot_dir(&root);

        if jot.is_dir() {
            return Err(Error::WorkspaceExists { path: root });
        }

        create_dir(&root)?;
        create_dir(&jot)?;

        let manifest = Manifest {
            schema_version: SCHEMA_VERSION,
            // v4, deliberately, where a note id is v7 — see `Manifest::id`.
            id: Uuid::new_v4(),
            name: default_name(&root),
            schema: FrontmatterSchema::jot_default(),
        };

        match Self::write_new_tree(&root, &jot, &manifest) {
            Ok(()) => Workspace::assembled(root, manifest).synced(),
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
    ///   for a manifest this build cannot make sense of.
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
        Workspace::assembled(root, manifest).synced()
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

    /// The declared frontmatter schema: `[schema] frontmatter` in the manifest.
    #[must_use]
    pub fn schema(&self) -> &FrontmatterSchema {
        &self.manifest.schema
    }

    /// What `init` or `open` noticed and did not refuse over. Empty for a well-formed workspace.
    ///
    /// See [`Warning`] for why these are warnings rather than errors.
    #[must_use]
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// Build a workspace and collect whatever its manifest is worth warning about.
    fn assembled(root: PathBuf, manifest: Manifest) -> Self {
        let mut warnings = Vec::new();
        let unknown: Vec<(String, String)> = manifest
            .schema
            .unknown_types()
            .into_iter()
            .map(|(key, ty)| (key.to_string(), ty.to_string()))
            .collect();
        if !unknown.is_empty() {
            warnings.push(Warning::UnknownFrontmatterTypes {
                path: manifest_path_of(&root),
                entries: unknown,
            });
        }
        Workspace {
            root,
            manifest,
            warnings,
            // Populated by `synced`, which `init` and `open` both run before handing a workspace
            // out. Assembling one with an empty snapshot and returning it would make every read
            // silently answer "nothing here".
            snapshot: Snapshot::default(),
        }
    }

    /// Scan the vault once, so a freshly opened workspace is never stale.
    fn synced(mut self) -> Result<Self> {
        self.sync()?;
        Ok(self)
    }

    // ------------------------------------------------------------------ opening a single note

    /// The path of the live note with this id, if the vault holds one.
    ///
    /// A linear scan, which is what stage 1b has: the id-to-path map is stage 4's index. The scan
    /// goes through [`fs::live_note_paths`] and [`fs::parse_note_filename`] so that "which files
    /// are notes" has exactly one answer in this crate.
    ///
    /// # Errors
    ///
    /// [`Error::ReadDir`] if the vault root cannot be listed.
    pub fn note_path(&self, id: NoteId) -> Result<Option<PathBuf>> {
        let wanted = id.as_uuid();
        for path in fs::live_note_paths(&self.root)? {
            if fs::parse_note_filename(&path).is_ok_and(|found| found == wanted) {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    /// Open one note.
    ///
    /// **A pure read.** It was not always: until `relation:root` was deleted this method repaired
    /// a missing root by walking the reply chain and writing the file back, and it was the only
    /// read path in the crate that wrote. There is nothing left to repair — a root is derived at
    /// scan time and never stored — so stage 4's "`sync()` never writes a note file" stops being a
    /// rule to enforce and becomes a property of the design.
    ///
    /// `title` and `relation:reply_to` were never repairable anyway: an absent title means
    /// untitled, and an absent parent means the note has none. Inventing either would be the index
    /// correcting the file, which is the one thing the source-of-truth decision forbids.
    ///
    /// # Errors
    ///
    /// [`Error::ReadDir`] if the vault root cannot be listed, [`Error::Read`] if the file cannot
    /// be read, plus whatever [`Note::parse_at`] raises.
    pub fn open_note(&self, id: NoteId) -> Result<Option<OpenedNote>> {
        let Some(path) = self.note_path(id)? else {
            return Ok(None);
        };
        let bytes = std::fs::read(&path).map_err(|source| Error::Read {
            path: path.clone(),
            source,
        })?;
        let note = Note::parse_at(&self.manifest.schema, &path, &bytes)?;
        Ok(Some(OpenedNote { note, path }))
    }

    // =========================================================================================
    // The vault snapshot
    // =========================================================================================

    /// Bring the in-memory view of the vault up to date, and report what moved.
    ///
    /// Every read below answers from this snapshot, so a surface calls `sync` before reading —
    /// which `init` and `open` have already done once, so a freshly opened workspace is never
    /// stale.
    ///
    /// # Errors
    ///
    /// [`Error::ReadDir`] if the vault root or the trash cannot be listed. A single unreadable
    /// *note* is a [`Problem`] on the report, not an error: one bad file must not make the vault
    /// unusable.
    pub fn sync(&mut self) -> Result<SyncReport> {
        let fresh = Snapshot::scan(&self.manifest.schema, &self.root)?;
        let report = fresh.diff(&self.snapshot);
        self.snapshot = fresh;
        Ok(report)
    }

    /// Discard the in-memory view and build it again from the files.
    ///
    /// Identical to [`Workspace::sync`] **in this build**, because the snapshot has no persistent
    /// half to be wrong: every scan is a cold one. The method exists anyway, and is called by
    /// `jot index rebuild`, because it is the operation whose meaning must not change when stage 4
    /// puts SQLite behind this seam — at which point `sync` becomes incremental and this one stops
    /// being its synonym. The rebuild invariant (`overview.md`) is the property that the two agree,
    /// and it is trivially true here.
    pub fn rebuild(&mut self) -> Result<SyncReport> {
        self.snapshot = Snapshot::default();
        self.sync()
    }

    /// The in-memory view, for this module's own tests only.
    ///
    /// **Deliberately not public.** It used to be, and `jot-cli` reached through it for four
    /// different things — which made the index's *representation* part of the API. Stage 4 puts
    /// SQLite here, and there is no `&Snapshot` to hand back once it does; the four methods below
    /// are what the surfaces actually needed, and each of them is answerable by a query. See
    /// `docs/plans/stage4.md`, "the swap is invisible".
    #[cfg(test)]
    fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// How many notes the vault holds, as `(active, trashed)`.
    #[must_use]
    pub fn counts(&self) -> (usize, usize) {
        self.snapshot.counts()
    }

    /// What the last scan could not make sense of.
    ///
    /// Never empty-by-error: a problem is something to tell a person about, not something that
    /// stops a command. The full list from the current scan, not a diff.
    #[must_use]
    pub fn problems(&self) -> &[Problem] {
        self.snapshot.problems()
    }

    /// The shortest id prefix that is unique *in this vault*, per note, floored at `min`.
    ///
    /// A property of the vault rather than of any one id, which is why it is asked of the
    /// workspace and why it never enters `--json`. See [`crate::shortid`].
    #[must_use]
    pub fn abbreviations(&self, min: usize) -> BTreeMap<NoteId, String> {
        self.snapshot.abbreviations(min)
    }

    /// A note's indexed metadata, without touching the file.
    ///
    /// This is the cheap read. [`Workspace::get`] is the one that re-reads the file and so is the
    /// only one a write may be built from — an index row carries no unknown frontmatter keys, and
    /// writing one back would destroy them.
    ///
    /// `None` when the vault holds no note with this id.
    #[must_use]
    pub fn meta(&self, id: NoteId) -> Option<&NoteMeta> {
        self.snapshot.get(id).map(|record| &record.meta)
    }

    /// Whether a note is active or trashed, decided by the directory its file is in.
    ///
    /// `None` when the vault holds no note with this id — which a caller rendering a dangling
    /// reference must distinguish from [`State::Active`].
    #[must_use]
    pub fn state_of(&self, id: NoteId) -> Option<State> {
        self.snapshot.get(id).map(|record| record.state)
    }

    // =========================================================================================
    // Note lifecycle
    // =========================================================================================

    /// Write a new note and return it.
    ///
    /// Mints the id here rather than taking one, because a UUIDv7 minted at write time is what
    /// makes the id's embedded timestamp the note's real creation time — the only place
    /// `created_at` is ever recorded.
    ///
    /// # The thread root is not written
    ///
    /// A note's root is **derived** — the transitive closure of `relation:reply_to`, computed by
    /// the scan. Nothing is stored here for it, which is why creating a reply no longer has to
    /// find and copy its parent's root, and why a hand-edited parent missing its own root can no
    /// longer leave `create` with nothing to copy.
    ///
    /// # What is refused
    ///
    /// A `reply_to` naming a note the vault does not hold, with [`Error::ReplyTargetMissing`].
    /// Replying to a **trashed** note is allowed: it is still a real note, trash never cascades,
    /// and refusing would make the trash a place replies go to die.
    ///
    /// A `quote`, by contrast, is not checked at all. A quote is a cross-tree reference like a
    /// link, dangling references are a designed state, and there is no structural invariant for a
    /// missing quote target to break.
    ///
    /// # Errors
    ///
    /// [`Error::ReplyTargetMissing`], plus [`Error::Write`] or [`Error::Rename`] if the file
    /// cannot be written.
    pub fn create(&mut self, draft: Draft) -> Result<Note> {
        let id = NoteId::new();
        if let Some(parent) = draft.reply_to
            && self.snapshot.get(parent).is_none()
        {
            return Err(Error::ReplyTargetMissing {
                id: parent.as_uuid(),
            });
        }

        // Start from whatever the caller parsed, so a key jot does not interpret — typed into an
        // `$EDITOR` buffer, or carried in by an importer — survives creation. The fields the
        // schema gives a role to are then overwritten unconditionally from the draft.
        let mut frontmatter = draft.extra.clone().unwrap_or_default();
        frontmatter.title = draft.title.clone();
        frontmatter.reply_to = draft.reply_to;
        frontmatter.quote = draft.quote;

        let note = Note::new(id, frontmatter, body_text(&draft.body));
        let path = self.root.join(fs::note_filename(
            id.as_uuid(),
            draft.title.as_deref(),
            draft.slug,
        ));

        let bytes = note.try_to_bytes(&self.manifest.schema)?;
        fs::atomic_write(&path, &self.tmp_dir(), &bytes)?;
        self.snapshot
            .reindex(&self.manifest.schema, id, &path, State::Active);
        Ok(note)
    }

    /// Apply an edit to an existing note and return it as it now stands.
    ///
    /// # The one write path
    ///
    /// Always `load(path)` → mutate → write. Never `NoteMeta` → write: a record from a query
    /// carries no unknown frontmatter keys, so writing one back would delete every key jot does not
    /// interpret — the "expensive failure" stage 4's risk table exists to prevent. Reloading the
    /// file also means an edit picks up whatever an external editor changed in the meantime,
    /// rather than clobbering it wholesale.
    ///
    /// # No-op edits do not write
    ///
    /// The rendered bytes are compared with the file's, and an identical result is not written.
    /// `edited_at` is filesystem mtime, and mtime moves even for an identical write, so writing a
    /// no-op would make every note look recently touched and poison the "recently edited" sort.
    ///
    /// # Re-slugging
    ///
    /// A title change re-derives the filename's slug **only for a note whose filename already has
    /// one**. The file chose its own form at creation and an edit does not overrule it. Renaming is
    /// safe either way: identity is the UUID and the reader ignores everything after it.
    ///
    /// # Errors
    ///
    /// [`Error::NoteNotFound`] if the vault holds no such note, plus whatever loading and writing
    /// the file raise.
    pub fn edit(&mut self, id: NoteId, edit: Edit) -> Result<Note> {
        let record = self
            .snapshot
            .get(id)
            .ok_or(Error::NoteNotFound { id: id.as_uuid() })?;
        let path = record.path.clone();
        let state = record.state;

        let before = std::fs::read(&path).map_err(|source| Error::Read {
            path: path.clone(),
            source,
        })?;
        let mut note = Note::parse_at(&self.manifest.schema, &path, &before)?;

        edit.title.clone().apply(&mut note.frontmatter.title);
        edit.quote.clone().apply(&mut note.frontmatter.quote);
        if let Some(body) = &edit.body {
            note.body = body_text(body);
        }

        let after = note.try_to_bytes(&self.manifest.schema)?;
        let target = self.renamed_for(&path, &note);

        if after == before && target == path {
            return Ok(note);
        }

        fs::atomic_write(&target, &self.tmp_dir(), &after)?;
        if target != path {
            // The new name is on disk before the old one goes, so a crash between the two leaves
            // two files claiming one id — which the next scan reports as a `DuplicateId` problem
            // and a person can resolve — rather than leaving no file at all.
            std::fs::remove_file(&path).map_err(|source| Error::Remove {
                path: path.clone(),
                source,
            })?;
        }
        self.snapshot
            .reindex(&self.manifest.schema, id, &target, state);
        Ok(note)
    }

    /// The path `note` should live at, given where it currently lives.
    ///
    /// Unchanged unless the file already carries a slug and the title has moved.
    fn renamed_for(&self, current: &Path, note: &Note) -> PathBuf {
        let bare = fs::note_filename(note.id.as_uuid(), None, FilenameSlug::None);
        let has_slug = current
            .file_name()
            .is_some_and(|name| name != bare.as_str());
        if !has_slug {
            return current.to_path_buf();
        }
        let parent = current.parent().unwrap_or(&self.root);
        parent.join(fs::note_filename(
            note.id.as_uuid(),
            note.frontmatter.title.as_deref(),
            FilenameSlug::FromTitle,
        ))
    }

    /// Move a note into `.jot/.trash/`.
    ///
    /// **Never cascades.** Trashing a note with replies moves exactly one file; the replies stay
    /// live and render a trashed-parent placeholder from [`Ref::Trashed`]. That is both the stated
    /// design and the only behavior that survives a rebuild without extra bookkeeping.
    ///
    /// The move *is* the state change — there is no frontmatter stamp to write — so this is one
    /// rename and nothing else.
    ///
    /// # Errors
    ///
    /// [`Error::NoteNotFound`], [`Error::AlreadyTrashed`], or [`Error::Rename`].
    pub fn trash(&mut self, id: NoteId) -> Result<()> {
        let record = self
            .snapshot
            .get(id)
            .ok_or(Error::NoteNotFound { id: id.as_uuid() })?;
        if record.state == State::Trashed {
            return Err(Error::AlreadyTrashed { id: id.as_uuid() });
        }
        let from = record.path.clone();
        let to = self.trash_dir().join(file_name_of(&from));
        self.relocate(id, &from, &to, State::Trashed)
    }

    /// Move a note out of `.jot/.trash/` and back into the vault root.
    ///
    /// The file keeps its name in the trash, so a restore puts it back exactly where it was.
    /// Nothing in the frontmatter changes, because nothing in the frontmatter recorded the trashing.
    ///
    /// # Errors
    ///
    /// [`Error::NoteNotFound`], [`Error::NotTrashed`], or [`Error::Rename`].
    pub fn restore(&mut self, id: NoteId) -> Result<()> {
        let record = self
            .snapshot
            .get(id)
            .ok_or(Error::NoteNotFound { id: id.as_uuid() })?;
        if record.state == State::Active {
            return Err(Error::NotTrashed { id: id.as_uuid() });
        }
        let from = record.path.clone();
        let to = self.root.join(file_name_of(&from));
        self.relocate(id, &from, &to, State::Active)
    }

    /// The rename shared by [`Workspace::trash`] and [`Workspace::restore`].
    ///
    /// Filesystem first, then the snapshot — the order `stage2.md` requires, so that an
    /// interruption leaves the index stale rather than the vault wrong. Stale is repaired by the
    /// next [`Workspace::sync`]; wrong is not repairable at all.
    fn relocate(&mut self, id: NoteId, from: &Path, to: &Path, state: State) -> Result<()> {
        if let Some(parent) = to.parent() {
            create_dir(parent)?;
        }
        std::fs::rename(from, to).map_err(|source| Error::Rename {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            source,
        })?;
        self.snapshot.reindex(&self.manifest.schema, id, to, state);
        Ok(())
    }

    /// Delete a note's file for good.
    ///
    /// **The only irreversible operation in the crate**, which is why every surface confirms before
    /// calling it. It removes exactly one file: replies keep their now-dangling `reply_to`, which
    /// is the [`Ref::Deleted`] state, and they stay grouped under the original `relation:root`
    /// because that field is assigned at creation and never recomputed.
    ///
    /// A note may be purged from either state — the trash is a convenience, not a required stop.
    ///
    /// # Errors
    ///
    /// [`Error::NoteNotFound`] or [`Error::Remove`].
    pub fn purge(&mut self, id: NoteId) -> Result<()> {
        let record = self
            .snapshot
            .get(id)
            .ok_or(Error::NoteNotFound { id: id.as_uuid() })?;
        let path = record.path.clone();
        std::fs::remove_file(&path).map_err(|source| Error::Remove {
            path: path.clone(),
            source,
        })?;
        self.snapshot.forget(id);
        Ok(())
    }

    // =========================================================================================
    // Reading
    // =========================================================================================

    /// Load one note in full, body included.
    ///
    /// Reads the file rather than answering from the snapshot, because the snapshot deliberately
    /// does not keep bodies.
    ///
    /// # Errors
    ///
    /// Whatever [`Note::load`] raises.
    pub fn get(&self, id: NoteId) -> Result<Option<Note>> {
        match self.snapshot.get(id) {
            None => Ok(None),
            Some(record) => Note::load(&self.manifest.schema, &record.path).map(Some),
        }
    }

    /// Resolve a git-style id prefix to a note. See [`Snapshot::resolve`].
    #[must_use]
    pub fn resolve(&self, prefix: &str) -> Resolution {
        self.snapshot.resolve(prefix)
    }

    /// Resolve an id to [`Ref::Present`], [`Ref::Trashed`], or [`Ref::Deleted`].
    #[must_use]
    pub fn reference(&self, id: NoteId) -> Ref {
        self.snapshot.reference(id)
    }

    /// A page of the timeline. See [`Snapshot::timeline`].
    #[must_use]
    pub fn timeline(&self, query: &TimelineQuery) -> Page<Row> {
        self.snapshot.timeline(query)
    }

    /// The thread a note sits in — ancestors and subtree. See [`Snapshot::thread`].
    #[must_use]
    pub fn thread(&self, id: NoteId) -> Option<Thread> {
        self.snapshot.thread(id)
    }

    /// Every active note, flat, in the requested order.
    #[must_use]
    pub fn files(&self, sort: FileSort) -> Vec<Row> {
        self.snapshot.files(sort)
    }

    /// Title-and-metadata search. See [`Snapshot::search`].
    #[must_use]
    pub fn search(&self, query: &SearchQuery) -> Vec<Row> {
        self.snapshot.search(query)
    }

    /// Everything in the trash, most recently trashed first.
    #[must_use]
    pub fn trashed(&self) -> Vec<Row> {
        self.snapshot.trashed()
    }

    /// Notes whose body links to this one.
    #[must_use]
    pub fn backlinks(&self, id: NoteId) -> Vec<NoteMeta> {
        self.snapshot.backlinks(id)
    }

    /// Notes whose `relation:quote` names this one.
    #[must_use]
    pub fn quoted_by(&self, id: NoteId) -> Vec<NoteMeta> {
        self.snapshot.quoted_by(id)
    }

    /// The links in one note's body, with their byte offsets and their resolved state.
    ///
    /// Re-reads the file, because offsets belong to a body and the snapshot keeps only the edge
    /// set. Extraction still never consults the index — the resolution is applied afterwards, to
    /// each target, which is what lets a link to a purged note extract normally and resolve to
    /// [`Ref::Deleted`].
    ///
    /// # Errors
    ///
    /// Whatever [`Note::load`] raises.
    pub fn links_in(&self, id: NoteId) -> Result<Vec<(Link, Ref)>> {
        let Some(note) = self.get(id)? else {
            return Ok(Vec::new());
        };
        Ok(link::extract(&note.body)
            .into_iter()
            .map(|link| {
                let target = self.reference(link.target);
                (link, target)
            })
            .collect())
    }
}

/// The result of [`Workspace::open_note`]: the note, where it lives, and whether opening it wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedNote {
    /// The note as it now stands on disk.
    pub note: Note,
    /// The file it was read from.
    pub path: PathBuf,
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

/// A body as it is written into a file: separated from the closing fence by exactly one blank
/// line, and newline-terminated.
///
/// Applied when a caller *supplies* a body — [`Workspace::create`], and an [`Edit`] that sets one.
/// It is not a markdown transformation and nothing is re-rendered: an edit that leaves the body
/// alone does not pass through here at all, which is what keeps "the write path leaves the body
/// alone" true.
///
/// **Both ends are normalized, and the leading end is why.** A body supplied by `-m` has no leading
/// newline; a body read back from an `$EDITOR` buffer always does, because it is the text that
/// followed the closing fence. Trimming only the trailing end would therefore add a blank line
/// every time a note went through the editor, and the note would gain another on each subsequent
/// edit. Normalizing both ends makes this idempotent — `body_text(body_text(x)) == body_text(x)` —
/// which is also what lets the no-op check in [`Workspace::edit`] work at all: an unchanged body
/// re-rendered has to produce identical bytes, or every save would move mtime and `edited_at` with
/// it.
///
/// Leading blank lines are not content: markdown ignores them before the first block, and the one
/// separating line is put back unconditionally.
fn body_text(text: &str) -> String {
    let trimmed = text.trim_matches(['\n', '\r']);
    if trimmed.is_empty() {
        String::from("\n")
    } else {
        format!("\n{trimmed}\n")
    }
}

/// The final component of a note path. Every path this is called with came from an enumerator that
/// already parsed a UUID out of its filename, so the fallback cannot be reached in practice.
fn file_name_of(path: &Path) -> PathBuf {
    path.file_name()
        .map_or_else(|| path.to_path_buf(), PathBuf::from)
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

    /// Hand-write a manifest into `root`, creating `.jot/` on the way.
    fn write_manifest(root: &Path, text: &str) {
        std::fs::create_dir_all(root.join(".jot")).unwrap();
        std::fs::write(root.join(".jot/workspace.toml"), text).unwrap();
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

        Workspace::init(&root).expect("init");

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
        assert!(!root.join(".jot/index.db").exists(), "index.db is stage 4");
    }

    /// `atomic_write` stages inside `.jot/tmp/`; if a staged file were ever left behind, the very
    /// first thing a user did with a fresh vault would be to see it.
    #[test]
    fn init_leaves_the_staging_directory_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        Workspace::init(&root).unwrap();

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
        let ws = Workspace::init(&root).unwrap();

        let text = std::fs::read_to_string(root.join(".jot/workspace.toml")).unwrap();
        let value: toml::Value = toml::from_str(&text).expect("manifest is valid TOML");

        assert_eq!(value["schema_version"].as_integer(), Some(2));
        assert_eq!(
            value["workspace"]["id"].as_str(),
            Some(ws.id().to_string().as_str())
        );
        assert_eq!(value["workspace"]["name"].as_str(), Some("Thoughts"));
        // An array of tables, each with a `type`; `key` appears only where it differs from the
        // type string.
        let entries = value["schema"]["frontmatter"].as_array().unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|e| e["type"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["document:title", "relation:reply_to", "relation:quote_to"]
        );
        assert_eq!(entries[0]["key"].as_str(), Some("title"));
        assert!(
            entries[1].get("key").is_none(),
            "a default key is not written"
        );
    }

    #[test]
    fn init_mints_a_uuid_v4_workspace_id() {
        let tmp = tempfile::tempdir().unwrap();
        let a = Workspace::init(&dir(tmp.path(), "a")).unwrap();
        let b = Workspace::init(&dir(tmp.path(), "b")).unwrap();

        // v4 rather than a note's v7: nothing decodes a workspace's creation time or sorts on it,
        // and a random-from-bit-one id is what keeps `jot ws ls`'s short ids short. See
        // `Manifest::id`.
        assert_eq!(a.id().get_version_num(), 4, "workspace ids are UUIDv4");
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
        Workspace::init(&root).unwrap();

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
        Workspace::init(&root).unwrap();

        let before = std::fs::read(root.join(".jot/workspace.toml")).unwrap();
        let err = Workspace::init(&root).unwrap_err();

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

        let err = Workspace::init(&root).unwrap_err();
        assert!(matches!(err, Error::WorkspaceExists { .. }), "{err:?}");
    }

    #[test]
    fn init_creates_a_target_directory_that_does_not_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("not").join("yet").join("there");

        let ws = Workspace::init(&root).expect("init creates the target");
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

        Workspace::init(&root).expect("adoption is a valid init");

        assert_eq!(std::fs::read(&note).unwrap(), bytes);
        assert_eq!(
            Workspace::open(&root).unwrap().name(),
            "existing notes",
            "the basename becomes the name, spaces and all"
        );
    }

    /// `[workspace] kind` is gone, and a manifest still carrying it opens anyway: `serde` ignores
    /// unknown fields, which is the same forward-compat courtesy the note format extends.
    ///
    /// The distinction it drew moved into the schema. A workspace declaring no `relation:*` entry
    /// **is** what `plain` meant.
    #[test]
    fn a_manifest_still_carrying_kind_opens_and_the_key_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        std::fs::create_dir(root.join(".jot")).unwrap();
        std::fs::write(
            root.join(".jot/workspace.toml"),
            "schema_version = 2\n\n[workspace]\n\
             id = \"01a03d4c-3680-7c70-aade-6c016dd177d2\"\nkind = \"plain\"\nname = \"v\"\n",
        )
        .unwrap();
        let ws = Workspace::open(&root).unwrap();
        assert_eq!(ws.name(), "v");
    }

    /// `.jot` as a *file* is not `.jot/` as a directory, so `WorkspaceExists` would be the wrong
    /// answer; the honest one names the directory that could not be created, and the user's file
    /// is left alone.
    #[test]
    fn init_errors_when_jot_exists_as_a_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        std::fs::write(root.join(".jot"), b"not a directory").unwrap();

        let err = Workspace::init(&root).unwrap_err();
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
        let ws = Workspace::init(&root).unwrap();

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

        let ws = Workspace::init(&relative).unwrap();
        assert!(ws.root().is_absolute(), "{}", ws.root().display());
    }

    // ----------------------------------------------------------------------------------- open

    #[test]
    fn open_round_trips_what_init_wrote_without_rewriting_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "Thoughts");
        let written = Workspace::init(&root).unwrap();

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
        Workspace::init(&root).unwrap();

        let manifest = root.join(".jot/workspace.toml");
        let bumped = std::fs::read_to_string(&manifest)
            .unwrap()
            .replace("schema_version = 2", "schema_version = 9999");
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
    fn open_rejects_a_non_uuid_id() {
        let tmp = tempfile::tempdir().unwrap();

        let bad_id = dir(tmp.path(), "i");
        std::fs::create_dir(bad_id.join(".jot")).unwrap();
        std::fs::write(
            bad_id.join(".jot/workspace.toml"),
            "schema_version = 2\n\n[workspace]\nid = \"nope\"\n",
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
        // No `[schema]` at all — a stage-1 manifest, which must open on the default
        // rather than on an empty schema that would render notes with no keys.
        assert_eq!(*ws.schema(), FrontmatterSchema::jot_default());
        assert!(ws.warnings().is_empty());
    }

    /// A stage-1 manifest still carries `[notes] filename`. It is not an error — `serde`
    /// ignores unknown fields, which is the same forward-compat courtesy the note format
    /// extends to unknown frontmatter keys — and the vault opens on the default schema.
    #[test]
    fn a_workspace_id_of_any_uuid_version_still_opens() {
        // Vaults created before the switch carry a v7 id, and an id is only ever compared and
        // displayed. Reading must not care which version it is.
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "vault");
        Workspace::init(&root).unwrap();

        let path = root.join(".jot").join("workspace.toml");
        let v7 = Uuid::now_v7();
        let text = std::fs::read_to_string(&path).unwrap();
        let line = text
            .lines()
            .find(|l| l.starts_with("id = "))
            .unwrap()
            .to_owned();
        std::fs::write(&path, text.replace(&line, &format!("id = \"{v7}\""))).unwrap();

        let reopened = Workspace::open(&root).unwrap();
        assert_eq!(reopened.id(), v7);
        assert_eq!(reopened.id().get_version_num(), 7);
    }

    #[test]
    fn a_stage_one_manifest_opens_on_the_default_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        std::fs::create_dir(root.join(".jot")).unwrap();
        std::fs::write(
            root.join(".jot/workspace.toml"),
            "schema_version = 1\n\n[workspace]\nid = \"01a03d4c-3680-7c70-aade-6c016dd177d2\"\n\
             kind = \"jot\"\nname = \"V\"\n\n[notes]\nfilename = \"uuid_slug\"\n",
        )
        .unwrap();

        let ws = Workspace::open(&root).unwrap();
        assert_eq!(*ws.schema(), FrontmatterSchema::jot_default());
        assert!(ws.warnings().is_empty());
    }

    /// The declared order is the whole content of the schema, so it must survive a round-trip
    /// through the file rather than being sorted into something tidier.
    #[test]
    fn a_declared_schema_keeps_its_order_and_its_types() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        write_manifest(
            &root,
            "schema_version = 2\n\n[workspace]\nid = \"01a03d4c-3680-7c70-aade-6c016dd177d2\"\n\
             name = \"V\"\n\n\
             [[schema.frontmatter]]\nkey = \"summary\"\ntype = \"text\"\nrequired = true\n\n\
             [[schema.frontmatter]]\nkey = \"heading\"\ntype = \"document:title\"\n\n\
             [[schema.frontmatter]]\ntype = \"relation:reply_to\"\n\n\
             [[schema.frontmatter]]\nkey = \"tags\"\ntype = \"multitext\"\n",
        );

        let ws = Workspace::open(&root).unwrap();
        assert_eq!(
            ws.schema().keys(),
            ["summary", "heading", "relation:reply_to", "tags"],
            "declared order is kept verbatim"
        );
        // The role is the type's, not the key's — that is the whole point of the change.
        assert_eq!(ws.schema().key_for(Role::Title), Some("heading"));
        assert!(ws.schema().entry("summary").unwrap().is_required());
        assert_eq!(
            ws.schema().entry("tags").unwrap().field_type(),
            &FieldType::Multitext(None)
        );
        assert!(ws.warnings().is_empty());
    }

    /// A v1 manifest opens, is promoted, and rewrites as v2 with the same emission order.
    #[test]
    fn a_v1_manifest_is_promoted_to_typed_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        write_manifest(
            &root,
            "schema_version = 1\n\n[workspace]\nid = \"01a03d4c-3680-7c70-aade-6c016dd177d2\"\n\
             kind = \"jot\"\nname = \"V\"\n\n[schema]\n\
             frontmatter = [ \"title\", \"relation:root\", \"relation:reply_to\", \
             \"relation:quote\", \"summary\" ]\n",
        );

        let ws = Workspace::open(&root).unwrap();
        assert_eq!(ws.manifest().schema_version, 1, "the file still says 1");
        assert_eq!(
            ws.schema().keys(),
            ["title", "relation:reply_to", "relation:quote", "summary"],
            "`relation:root` is dropped; everything else keeps its declared position"
        );
        assert_eq!(ws.schema().key_for(Role::Title), Some("title"));
        assert_eq!(
            ws.schema().key_for(Role::ReplyTo),
            Some("relation:reply_to")
        );
        // v1 spelled it `relation:quote`. The *key* is kept so existing notes round-trip; only the
        // type is renamed.
        assert_eq!(ws.schema().key_for(Role::QuoteTo), Some("relation:quote"));
        // v1 said nothing about what `summary` held, so `text` is the honest reading.
        assert_eq!(
            ws.schema().entry("summary").unwrap().field_type(),
            &FieldType::Text(None)
        );

        // And it rewrites as v2, with the promoted schema unchanged.
        let rewritten = Manifest {
            schema_version: SCHEMA_VERSION,
            ..ws.manifest().clone()
        };
        let text = rewritten.to_toml(&ws.manifest_path()).unwrap();
        let reread = Manifest::from_toml(&text, &ws.manifest_path(), "ignored").unwrap();
        assert_eq!(reread.schema_version, 2);
        assert_eq!(reread.schema, *ws.schema());
    }

    /// The manifest is configuration, so it is validated hard — unlike a note file, which is user
    /// data and is never rejected.
    #[test]
    fn two_entries_claiming_one_role_is_a_manifest_error_naming_both_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        write_manifest(
            &root,
            "schema_version = 2\n\n[workspace]\nid = \"01a03d4c-3680-7c70-aade-6c016dd177d2\"\n\
             name = \"V\"\n\n\
             [[schema.frontmatter]]\nkey = \"title\"\ntype = \"document:title\"\n\n\
             [[schema.frontmatter]]\nkey = \"heading\"\ntype = \"document:title\"\n",
        );

        let err = Workspace::open(&root).unwrap_err();
        assert!(matches!(err, Error::ManifestParse { .. }), "{err:?}");
        let shown = err.to_string();
        assert!(shown.contains("document:title"), "{shown}");
        assert!(
            shown.contains("title") && shown.contains("heading"),
            "{shown}"
        );
    }

    /// An unknown `type` warns, and every key under it survives untouched. This is the
    /// forward-compat rule applied to the type system.
    #[test]
    fn an_unknown_type_warns_rather_than_refusing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        write_manifest(
            &root,
            "schema_version = 2\n\n[workspace]\nid = \"01a03d4c-3680-7c70-aade-6c016dd177d2\"\n\
             name = \"V\"\n\n\
             [[schema.frontmatter]]\nkey = \"title\"\ntype = \"document:title\"\n\n\
             [[schema.frontmatter]]\nkey = \"mood\"\ntype = \"document:mood\"\n",
        );

        let ws = Workspace::open(&root).expect("an unknown type is a warning, not a refusal");
        match ws.warnings() {
            [Warning::UnknownFrontmatterTypes { path, entries }] => {
                assert_eq!(
                    entries,
                    &[("mood".to_string(), "document:mood".to_string())]
                );
                assert_eq!(path, &ws.manifest_path());
            }
            other => panic!("expected one unknown-type warning, got {other:?}"),
        }
        let shown = ws.warnings()[0].to_string();
        assert!(shown.contains("document:mood"), "{shown}");
    }

    #[test]
    fn an_empty_schema_key_is_a_parse_error() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        std::fs::create_dir(root.join(".jot")).unwrap();
        std::fs::write(
            root.join(".jot/workspace.toml"),
            "schema_version = 1\n\n[workspace]\nid = \"01a03d4c-3680-7c70-aade-6c016dd177d2\"\n\
             kind = \"jot\"\nname = \"V\"\n\n[schema]\nfrontmatter = [ \"title\", \"  \" ]\n",
        )
        .unwrap();

        let err = Workspace::open(&root).unwrap_err();
        assert!(matches!(err, Error::ManifestParse { .. }), "{err:?}");
        assert!(err.to_string().contains("empty `key`"), "{err}");
    }

    /// Forward compatibility inside one schema version: a key this build has never heard of must
    /// not stop it opening the vault. `open` never rewrites, so the key survives by construction.
    #[test]
    fn open_ignores_manifest_keys_it_does_not_know() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "v");
        Workspace::init(&root).unwrap();

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
        assert_eq!(ws.name(), "Fixture Vault");
        assert_eq!(ws.id().to_string(), "01a03d4c-3680-7c70-aade-6c016dd177d2");
    }

    // ------------------------------------------------------------------------------- discover

    #[test]
    fn discover_finds_the_workspace_from_three_directories_deep() {
        let tmp = tempfile::tempdir().unwrap();
        let root = dir(tmp.path(), "vault");
        Workspace::init(&root).unwrap();
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
        Workspace::init(&root).unwrap();

        assert!(same_dir(Workspace::discover(&root).unwrap().root(), &root));
    }

    /// The failure this guards is the expensive one: a note captured into the outer vault when the
    /// user is standing in the inner one is silently lost.
    #[test]
    fn discover_stops_at_the_nearest_workspace_not_the_outermost() {
        let tmp = tempfile::tempdir().unwrap();
        let outer = dir(tmp.path(), "outer");
        let inner = dir(&outer, "a/inner");
        Workspace::init(&outer).unwrap();
        Workspace::init(&inner).unwrap();

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
        Workspace::init(&root).unwrap();

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
        Workspace::init(&outer).unwrap();
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
        Workspace::init(&work).unwrap();

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
        Workspace::init(&root).unwrap();
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
        let ws = Workspace::init(&weird).expect("init");
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

    /// The emitted manifest must parse back into the same values, or `init` and `open` disagree
    /// about the vault they are looking at.
    #[test]
    fn manifest_text_round_trips_through_itself() {
        let path = Path::new("v/.jot/workspace.toml");
        let manifest = Manifest {
            schema_version: SCHEMA_VERSION,
            id: Uuid::parse_str("01a03d4c-3680-7c70-aade-6c016dd177d2").unwrap(),
            name: "Name with spaces, a \" quote and 한글".to_string(),
            schema: FrontmatterSchema::try_new(vec![
                FrontmatterEntry::with_key("title", FieldType::Reserved(Role::Title))
                    .required(true),
                FrontmatterEntry::new(FieldType::Reserved(Role::ReplyTo)),
                FrontmatterEntry::with_key("summary", FieldType::Text(None)),
                FrontmatterEntry::with_key("mood", FieldType::parse("document:mood")),
            ])
            .unwrap(),
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
            name: "Thoughts".to_string(),
            schema: FrontmatterSchema::jot_default(),
        };

        assert_eq!(
            manifest
                .to_toml(Path::new("v/.jot/workspace.toml"))
                .unwrap(),
            "\
# jot workspace manifest.
# `id` is minted once and must never change — it is what makes this directory self-identifying.
# `name` is display-only and safe to edit by hand.

schema_version = 2

[workspace]
id = \"01a03d4c-3680-7c70-aade-6c016dd177d2\"
name = \"Thoughts\"

[[schema.frontmatter]]
key = \"title\"
type = \"document:title\"
required = true

[[schema.frontmatter]]
type = \"relation:reply_to\"

[[schema.frontmatter]]
type = \"relation:quote_to\"
"
        );
    }

    #[test]
    fn default_name_falls_back_when_a_path_has_no_final_component() {
        assert_eq!(default_name(Path::new("/")), "jot");
        assert_eq!(default_name(Path::new("a/b/Field Notes")), "Field Notes");
    }

    // ============================================================== opening a single note

    /// A copy of the shared fixture vault, so a test may mutate notes without touching the corpus.
    fn fixture_copy(tmp: &Path) -> PathBuf {
        let src = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("tests")
            .join("fixtures")
            .join("vault");
        let dst = tmp.join("vault");
        copy_tree(&src, &dst);
        dst
    }

    fn copy_tree(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).unwrap();
        for entry in std::fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let to = dst.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &to);
            } else {
                std::fs::copy(entry.path(), &to).unwrap();
            }
        }
    }

    /// A vault holding exactly the notes given as `(filename, contents)`.
    fn vault_of(tmp: &Path, notes: &[(&str, &str)]) -> Workspace {
        let root = dir(tmp, "v");
        let mut ws = Workspace::init(&root).unwrap();
        for (name, text) in notes {
            std::fs::write(root.join(name), text).unwrap();
        }
        // The files land after `init` scanned, so the snapshot has to catch up before any read
        // that answers from it — the derived root among them.
        ws.sync().unwrap();
        ws
    }

    fn nid(s: &str) -> NoteId {
        s.parse().unwrap()
    }

    const A: &str = "01a03d60-0000-7000-8000-00000000000a";
    const B: &str = "01a03d61-0000-7000-8000-00000000000b";
    const C: &str = "01a03d62-0000-7000-8000-00000000000c";
    const D: &str = "01a03d63-0000-7000-8000-00000000000d";
    /// A note id no file in these vaults carries.
    const GONE: &str = "01a03dff-0000-7000-8000-00000000ffff";

    #[test]
    fn note_path_finds_a_note_by_id_and_reports_absence_rather_than_failing() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = vault_of(
            tmp.path(),
            &[(
                &format!("{A}.md"),
                &format!("---\nrelation:root: {A}\n---\n"),
            )],
        );
        assert_eq!(
            ws.note_path(nid(A)).unwrap(),
            Some(ws.root().join(format!("{A}.md")))
        );
        assert_eq!(ws.note_path(nid(B)).unwrap(), None);
        assert!(ws.open_note(nid(B)).unwrap().is_none());
    }

    #[test]
    fn note_path_finds_a_note_filed_under_a_slug() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = vault_of(
            tmp.path(),
            &[(
                &format!("{A}_a_slug.md"),
                &format!("---\nrelation:root: {A}\n---\n"),
            )],
        );
        assert_eq!(
            ws.note_path(nid(A)).unwrap(),
            Some(ws.root().join(format!("{A}_a_slug.md")))
        );
    }

    /// `open_note` is a **pure read**. It was the crate's only read path that wrote, and the
    /// reason it wrote — filling in a missing `relation:root` — no longer exists.
    #[test]
    fn opening_a_note_never_writes_even_when_the_file_is_out_of_schema_order() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = vault_of(
            tmp.path(),
            &[(
                &format!("{A}.md"),
                &format!("---\nsummary: kept\nrelation:reply_to: {B}\ntitle: t\n---\n\nBody.\n"),
            )],
        );
        let path = ws.root().join(format!("{A}.md"));
        let before = std::fs::read(&path).unwrap();
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();

        let opened = ws.open_note(nid(A)).unwrap().unwrap();
        assert_eq!(opened.note.frontmatter.title.as_deref(), Some("t"));
        assert_eq!(opened.note.frontmatter.reply_to, Some(nid(B)));
        assert_eq!(opened.note.body, "\nBody.\n");

        assert_eq!(std::fs::read(&path).unwrap(), before, "the bytes changed");
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            mtime,
            "the file was touched"
        );
        assert_eq!(
            std::fs::read_dir(ws.tmp_dir()).unwrap().count(),
            0,
            "a staged file was left behind"
        );
    }

    /// A `relation:root` written before this change is an ordinary undeclared key. It is not
    /// migrated, it is preserved — and the file round-trips byte-for-byte through an edit that
    /// does not touch it.
    #[test]
    fn a_legacy_relation_root_survives_an_edit_byte_for_byte() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ws = vault_of(
            tmp.path(),
            &[(
                &format!("{A}.md"),
                &format!("---\ntitle: t\nrelation:root: {A}\n---\n\nBody.\n"),
            )],
        );
        ws.edit(nid(A), Edit::new().body("Changed.\n")).unwrap();
        assert_eq!(
            std::fs::read_to_string(ws.root().join(format!("{A}.md"))).unwrap(),
            format!("---\ntitle: t\nrelation:root: {A}\n---\n\nChanged.\n"),
            "the undeclared key kept its bytes and its position"
        );
    }

    // ================================================= the derived root, and reply cycles

    /// The root is derived from `relation:reply_to` alone, over records already in memory.
    #[test]
    fn a_root_is_derived_by_walking_reply_to_upward() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = vault_of(
            tmp.path(),
            &[
                (&format!("{A}.md"), "---\ntitle: root\n---\n"),
                (
                    &format!("{B}.md"),
                    &format!("---\ntitle: mid\nrelation:reply_to: {A}\n---\n"),
                ),
                (
                    &format!("{C}.md"),
                    &format!("---\ntitle: leaf\nrelation:reply_to: {B}\n---\n"),
                ),
            ],
        );
        for id in [A, B, C] {
            assert_eq!(
                ws.meta(nid(id)).unwrap().root,
                Some(nid(A)),
                "every note in the chain roots at A"
            );
        }
        assert!(ws.snapshot().problems().is_empty());
    }

    /// Dangling references are a designed state: the chain named an ancestor, so that ancestor is
    /// the root even though its file is not here. It is also what keeps the children of a purged
    /// note grouped with each other — they carry the same missing id.
    #[test]
    fn a_reply_chain_that_runs_into_a_missing_note_roots_at_that_note() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = vault_of(
            tmp.path(),
            &[
                (
                    &format!("{C}.md"),
                    &format!("---\nrelation:reply_to: {GONE}\n---\n"),
                ),
                (
                    &format!("{D}.md"),
                    &format!("---\nrelation:reply_to: {GONE}\n---\n"),
                ),
            ],
        );
        assert_eq!(ws.meta(nid(C)).unwrap().root, Some(nid(GONE)));
        assert_eq!(
            ws.meta(nid(D)).unwrap().root,
            ws.meta(nid(C)).unwrap().root,
            "orphaned siblings can still be grouped by the id they both name"
        );
    }

    /// A cycle is a [`Problem`], not an [`Error`]: one bad file must not make the other nine
    /// hundred unreadable. The note becomes its own root, so it stays visible in the timeline.
    #[test]
    fn a_note_that_replies_to_itself_is_reported_and_roots_at_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = vault_of(
            tmp.path(),
            &[
                (
                    &format!("{A}.md"),
                    &format!("---\ntitle: looped\nrelation:reply_to: {A}\n---\n"),
                ),
                (&format!("{B}.md"), "---\ntitle: fine\n---\n"),
            ],
        );

        assert_eq!(ws.meta(nid(A)).unwrap().root, Some(nid(A)));
        match ws.snapshot().problems() {
            [Problem::ReplyCycle { id, path }] => {
                assert_eq!(*id, nid(A));
                assert!(path.ends_with(format!("{A}.md")));
            }
            other => panic!("expected one cycle problem, got {other:?}"),
        }

        // Visible, because something that needs fixing has to be findable — and the other note is
        // unaffected.
        let shown: Vec<_> = ws
            .timeline(&TimelineQuery::default())
            .items
            .iter()
            .map(|row| row.note.id)
            .collect();
        assert!(shown.contains(&nid(A)), "the looped note vanished");
        assert!(shown.contains(&nid(B)));
    }

    #[test]
    fn a_three_note_reply_cycle_is_reported_and_does_not_hang() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = vault_of(
            tmp.path(),
            &[
                (
                    &format!("{A}.md"),
                    &format!("---\nrelation:reply_to: {B}\n---\n"),
                ),
                (
                    &format!("{B}.md"),
                    &format!("---\nrelation:reply_to: {C}\n---\n"),
                ),
                (
                    &format!("{C}.md"),
                    &format!("---\nrelation:reply_to: {A}\n---\n"),
                ),
            ],
        );
        assert!(
            !ws.snapshot().problems().is_empty(),
            "a cycle must be reported"
        );
        for id in [A, B, C] {
            assert!(
                ws.meta(nid(id)).unwrap().root.is_some(),
                "every note still has an answer"
            );
        }
    }

    /// Stage 1b acceptance, in the form stage 1b can state it: a read pass over a clean vault
    /// writes nothing.
    ///
    /// The criterion as written names `sync()` and `rebuild()`, which are stage 4's. What is
    /// testable now is the property they will inherit — enumerating and parsing every note in the
    /// corpus leaves every byte, and the whole directory tree, exactly as it was.
    #[test]
    fn reading_the_whole_fixture_vault_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = fixture_copy(tmp.path());
        let before = tree_snapshot(&root);

        let ws = Workspace::open(&root).unwrap();
        assert!(ws.warnings().is_empty(), "{:?}", ws.warnings());
        for path in fs::live_note_paths(&root).unwrap() {
            Note::load(ws.schema(), &path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        }
        for path in fs::trashed_note_paths(&root).unwrap() {
            Note::load(ws.schema(), &path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        }

        assert_eq!(
            tree_snapshot(&root),
            before,
            "a read pass changed the vault"
        );
    }

    /// Every file under `root`, relative path and bytes, so a diff shows what moved.
    fn tree_snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        collect(root, root, &mut out);
        out.sort();
        return out;

        fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    collect(root, &path, out);
                } else {
                    let rel = path
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.push((rel, std::fs::read(&path).unwrap()));
                }
            }
        }
    }

    /// Opening every note in the corpus writes nothing at all — the property that replaced
    /// "the first open may repair, the second must not".
    #[test]
    fn opening_every_fixture_note_writes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = fixture_copy(tmp.path());
        let before = tree_snapshot(&root);
        let ws = Workspace::open(&root).unwrap();

        for path in fs::live_note_paths(&root).unwrap() {
            let id = NoteId::from(fs::parse_note_filename(&path).unwrap());
            ws.open_note(id).unwrap().unwrap();
            ws.open_note(id).unwrap().unwrap();
        }
        assert_eq!(
            tree_snapshot(&root),
            before,
            "opening a note wrote to the vault"
        );
    }
}

/// Stage 2's note lifecycle: `create`, `edit`, `trash`, `restore`, `purge`, and the reads that
/// have to keep agreeing with them.
#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::query::FileSort;

    fn workspace() -> (tempfile::TempDir, Workspace) {
        let tmp = tempfile::tempdir().unwrap();
        let ws = Workspace::init(tmp.path()).unwrap();
        (tmp, ws)
    }

    fn bytes_of(ws: &Workspace, id: NoteId) -> Vec<u8> {
        std::fs::read(&ws.snapshot().get(id).unwrap().path).unwrap()
    }

    fn mtime_of(ws: &Workspace, id: NoteId) -> std::time::SystemTime {
        std::fs::metadata(&ws.snapshot().get(id).unwrap().path)
            .unwrap()
            .modified()
            .unwrap()
    }

    fn file_count(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .map(|entries| entries.flatten().filter(|e| e.path().is_file()).count())
            .unwrap_or(0)
    }

    // ------------------------------------------------------------------------------- create

    #[test]
    fn create_writes_a_file_named_by_the_id_it_minted() {
        let (_tmp, mut ws) = workspace();
        let note = ws.create(Draft::new("a thought")).unwrap();

        let path = ws.root().join(format!("{}.md", note.id));
        assert!(path.exists(), "the note is at its bare-uuid filename");
        assert_eq!(fs::parse_note_filename(&path).unwrap(), note.id.as_uuid());
    }

    #[test]
    fn a_created_note_is_immediately_readable_without_a_sync() {
        let (_tmp, mut ws) = workspace();
        let note = ws.create(Draft::new("a thought").title("t")).unwrap();

        assert_eq!(ws.get(note.id).unwrap().unwrap().body, note.body);
        assert!(matches!(ws.reference(note.id), Ref::Present(_)));
        assert_eq!(ws.timeline(&TimelineQuery::new()).len(), 1);
    }

    #[test]
    fn created_at_comes_from_the_minted_id_and_is_in_no_file() {
        let (_tmp, mut ws) = workspace();
        let note = ws.create(Draft::new("x")).unwrap();

        assert!(note.created_at().is_some());
        let text = String::from_utf8(bytes_of(&ws, note.id)).unwrap();
        assert!(!text.contains("created_at"), "{text}");
        assert!(!text.contains("id:"), "{text}");
    }

    /// Nothing about a root is written to a note file, and `create` no longer copies one from a
    /// parent. The root is derived, and the snapshot is where it shows up.
    #[test]
    fn create_writes_no_root_key_and_the_derived_root_is_the_threads() {
        let (_tmp, mut ws) = workspace();
        let root = ws.create(Draft::new("root")).unwrap();
        let mid = ws.create(Draft::new("mid").reply_to(root.id)).unwrap();
        let deep = ws.create(Draft::new("deep").reply_to(mid.id)).unwrap();

        let text = String::from_utf8(bytes_of(&ws, deep.id)).unwrap();
        assert!(!text.contains("relation:root"), "{text}");
        assert_eq!(deep.frontmatter.reply_to, Some(mid.id));

        for note in [&root, &mid, &deep] {
            assert_eq!(
                ws.meta(note.id).unwrap().root,
                Some(root.id),
                "root is the thread's, not the parent's"
            );
        }
    }

    #[test]
    fn replying_to_a_note_that_does_not_exist_is_refused() {
        let (_tmp, mut ws) = workspace();
        let missing = NoteId::new();
        let err = ws
            .create(Draft::new("orphan").reply_to(missing))
            .unwrap_err();

        assert!(matches!(err, Error::ReplyTargetMissing { id } if id == missing.as_uuid()));
        assert_eq!(file_count(ws.root()), 0, "nothing was written");
    }

    #[test]
    fn replying_to_a_trashed_note_is_allowed_because_it_is_still_a_real_note() {
        let (_tmp, mut ws) = workspace();
        let parent = ws.create(Draft::new("parent")).unwrap();
        ws.trash(parent.id).unwrap();

        let reply = ws.create(Draft::new("reply").reply_to(parent.id)).unwrap();
        assert_eq!(reply.frontmatter.reply_to, Some(parent.id));
        assert_eq!(ws.meta(reply.id).unwrap().root, Some(parent.id));
    }

    #[test]
    fn a_quote_is_never_validated_and_never_changes_the_root() {
        let (_tmp, mut ws) = workspace();
        let dangling = NoteId::new();
        let note = ws.create(Draft::new("x").quote(dangling)).unwrap();

        assert_eq!(note.frontmatter.quote, Some(dangling));
        assert_eq!(
            ws.meta(note.id).unwrap().root,
            Some(note.id),
            "not the quoted note's"
        );
        assert!(matches!(ws.reference(dangling), Ref::Deleted(_)));
    }

    #[test]
    fn a_quote_does_not_join_the_quoted_notes_thread() {
        let (_tmp, mut ws) = workspace();
        let quoted = ws.create(Draft::new("quoted")).unwrap();
        let quoting = ws.create(Draft::new("quoting").quote(quoted.id)).unwrap();

        let tree = ws.thread(quoted.id).unwrap().tree;
        assert_eq!(tree.len(), 1, "the quoting note is not in the tree");
        assert_eq!(ws.quoted_by(quoted.id)[0].id, quoting.id);
    }

    #[test]
    fn the_slug_option_decides_the_filename_and_the_id_still_parses_back() {
        let (_tmp, mut ws) = workspace();
        let bare = ws.create(Draft::new("x").title("Hello There")).unwrap();
        let slugged = ws
            .create(Draft::new("x").title("Hello There").slugged())
            .unwrap();

        assert!(ws.root().join(format!("{}.md", bare.id)).exists());
        let path = ws.root().join(format!("{}_hello_there.md", slugged.id));
        assert!(path.exists(), "slug derived from the title");
        assert_eq!(
            fs::parse_note_filename(&path).unwrap(),
            slugged.id.as_uuid()
        );
    }

    #[test]
    fn a_created_note_round_trips_through_the_parser() {
        let (_tmp, mut ws) = workspace();
        let note = ws.create(Draft::new("body text").title("A title")).unwrap();

        let reloaded = ws.get(note.id).unwrap().unwrap();
        assert_eq!(reloaded, note);
        assert_eq!(reloaded.frontmatter.title.as_deref(), Some("A title"));
        assert!(reloaded.body.contains("body text"));
    }

    #[test]
    fn a_supplied_body_is_separated_from_the_fence_by_exactly_one_blank_line() {
        let (_tmp, mut ws) = workspace();
        // Every shape a caller might hand over: no leading newline (`-m`), one (an `$EDITOR`
        // buffer), and several (a buffer that has already been through the editor once).
        for supplied in ["a thought", "\na thought", "\n\n\na thought\n\n"] {
            let note = ws.create(Draft::new(supplied)).unwrap();
            assert_eq!(note.body, "\na thought\n", "supplied: {supplied:?}");
        }
    }

    #[test]
    fn a_body_round_tripped_through_an_editor_does_not_gain_a_blank_line_each_time() {
        // The compounding bug this normalization exists to prevent: an `$EDITOR` buffer's body is
        // the text after the closing fence, so it always arrives with a leading newline. Feeding
        // that straight back must be a fixed point.
        let (_tmp, mut ws) = workspace();
        let note = ws.create(Draft::new("a thought")).unwrap();
        let original = note.body.clone();

        let mut current = note;
        for round in 0..3 {
            let fed_back = current.body.clone();
            current = ws.edit(current.id, Edit::new().body(fed_back)).unwrap();
            assert_eq!(current.body, original, "grew on round {round}");
        }
    }

    #[test]
    fn re_saving_an_unchanged_body_from_an_editor_does_not_write() {
        // Follows from the fixed point above, and is the property that keeps `edited_at` honest.
        let (_tmp, mut ws) = workspace();
        let note = ws.create(Draft::new("a thought").title("t")).unwrap();
        let before = mtime_of(&ws, note.id);

        ws.edit(note.id, Edit::new().body(note.body.clone()))
            .unwrap();
        assert_eq!(mtime_of(&ws, note.id), before);
    }

    #[test]
    fn an_empty_body_still_produces_a_well_formed_note() {
        let (_tmp, mut ws) = workspace();
        let note = ws.create(Draft::new("")).unwrap();
        assert!(ws.get(note.id).unwrap().is_some(), "it parses back");
    }

    // --------------------------------------------------------------------------------- edit

    #[test]
    fn editing_a_title_changes_the_file_and_the_snapshot() {
        let (_tmp, mut ws) = workspace();
        let note = ws.create(Draft::new("b").title("before")).unwrap();

        let edited = ws.edit(note.id, Edit::new().title("after")).unwrap();
        assert_eq!(edited.frontmatter.title.as_deref(), Some("after"));
        assert_eq!(
            ws.snapshot().get(note.id).unwrap().meta.title.as_deref(),
            Some("after")
        );
    }

    #[test]
    fn editing_a_body_leaves_the_frontmatter_alone_and_the_reverse() {
        let (_tmp, mut ws) = workspace();
        let note = ws.create(Draft::new("before").title("t")).unwrap();

        let edited = ws.edit(note.id, Edit::new().body("after")).unwrap();
        assert!(edited.body.contains("after"));
        assert_eq!(edited.frontmatter.title.as_deref(), Some("t"));
        assert_eq!(edited.frontmatter.reply_to, note.frontmatter.reply_to);
    }

    /// `jot_default` marks `document:title` required, so clearing a title leaves the key behind,
    /// empty. That is the point of `required` — the key a vault always wants to see stays visible
    /// in the file and in the `$EDITOR` buffer — and it costs nothing, because an empty value
    /// parses as absent.
    #[test]
    fn clearing_a_title_leaves_the_required_key_behind_empty() {
        let (_tmp, mut ws) = workspace();
        let note = ws.create(Draft::new("b").title("gone")).unwrap();

        ws.edit(note.id, Edit::new().clear_title()).unwrap();
        let text = String::from_utf8(bytes_of(&ws, note.id)).unwrap();
        assert!(text.contains("title:"), "the required key stays: {text}");
        assert!(!text.contains("gone"), "the value is gone: {text}");
        assert_eq!(
            ws.get(note.id).unwrap().unwrap().frontmatter.title,
            None,
            "an empty required key reads back as no title"
        );
    }

    #[test]
    fn an_edit_never_touches_reply_to() {
        // Re-parenting is not supported, and must not happen as a side effect of an edit.
        let (_tmp, mut ws) = workspace();
        let root = ws.create(Draft::new("root")).unwrap();
        let reply = ws.create(Draft::new("reply").reply_to(root.id)).unwrap();

        let edited = ws.edit(reply.id, Edit::new().title("new")).unwrap();
        assert_eq!(edited.frontmatter.reply_to, Some(root.id));
        assert_eq!(ws.meta(reply.id).unwrap().root, Some(root.id));
    }

    #[test]
    fn an_unknown_frontmatter_key_survives_an_edit_byte_for_byte() {
        // The expensive failure, guarded: an edit that rebuilt the file from a query result would
        // drop every key jot does not interpret. `load -> mutate -> write` is what prevents it.
        let (_tmp, mut ws) = workspace();
        let note = ws.create(Draft::new("b").title("t")).unwrap();
        let path = ws.snapshot().get(note.id).unwrap().path.clone();

        let original = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
        let hand_edited = original.replace(
            "title: t",
            "title: t\nsource: obsidian-import\ntags:\n  - migration",
        );
        std::fs::write(&path, &hand_edited).unwrap();
        ws.sync().unwrap();

        ws.edit(note.id, Edit::new().title("retitled")).unwrap();

        let after = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
        assert!(after.contains("source: obsidian-import"), "{after}");
        assert!(after.contains("- migration"), "{after}");
        assert!(after.contains("title: retitled"), "{after}");
    }

    #[test]
    fn an_edit_that_changes_nothing_does_not_write() {
        // `edited_at` is mtime, so an identical write would still make the note look touched.
        let (_tmp, mut ws) = workspace();
        let note = ws.create(Draft::new("body").title("t")).unwrap();
        let before = mtime_of(&ws, note.id);
        let bytes = bytes_of(&ws, note.id);

        ws.edit(note.id, Edit::new().title("t")).unwrap();
        ws.edit(note.id, Edit::new().body("body")).unwrap();
        ws.edit(note.id, Edit::new()).unwrap();

        assert_eq!(bytes_of(&ws, note.id), bytes);
        assert_eq!(mtime_of(&ws, note.id), before, "mtime did not move");
    }

    #[test]
    fn editing_a_note_the_vault_does_not_hold_is_an_error() {
        let (_tmp, mut ws) = workspace();
        let missing = NoteId::new();
        let err = ws.edit(missing, Edit::new().title("x")).unwrap_err();
        assert!(matches!(err, Error::NoteNotFound { id } if id == missing.as_uuid()));
    }

    #[test]
    fn a_slugged_file_is_renamed_on_a_title_change_and_keeps_its_identity() {
        let (_tmp, mut ws) = workspace();
        let note = ws
            .create(Draft::new("b").title("First Title").slugged())
            .unwrap();
        assert!(
            ws.root()
                .join(format!("{}_first_title.md", note.id))
                .exists()
        );

        ws.edit(note.id, Edit::new().title("Second Title")).unwrap();

        assert!(
            !ws.root()
                .join(format!("{}_first_title.md", note.id))
                .exists()
        );
        assert!(
            ws.root()
                .join(format!("{}_second_title.md", note.id))
                .exists()
        );
        assert_eq!(file_count(ws.root()), 1, "the old name is gone");
        assert!(matches!(ws.reference(note.id), Ref::Present(_)), "same id");
    }

    #[test]
    fn a_bare_filename_is_not_given_a_slug_by_an_edit() {
        // The file chose its form at creation; an edit does not overrule it.
        let (_tmp, mut ws) = workspace();
        let note = ws.create(Draft::new("b").title("First")).unwrap();

        ws.edit(note.id, Edit::new().title("Second")).unwrap();
        assert!(ws.root().join(format!("{}.md", note.id)).exists());
        assert_eq!(file_count(ws.root()), 1);
    }

    #[test]
    fn a_quote_can_be_set_and_cleared_by_an_edit() {
        let (_tmp, mut ws) = workspace();
        let quoted = ws.create(Draft::new("quoted")).unwrap();
        let note = ws.create(Draft::new("x")).unwrap();

        let set = ws.edit(note.id, Edit::new().quote(quoted.id)).unwrap();
        assert_eq!(set.frontmatter.quote, Some(quoted.id));
        assert_eq!(ws.quoted_by(quoted.id).len(), 1);

        let cleared = ws.edit(note.id, Edit::new().clear_quote()).unwrap();
        assert_eq!(cleared.frontmatter.quote, None);
        assert!(ws.quoted_by(quoted.id).is_empty());
    }

    // -------------------------------------------------------------------------------- trash

    #[test]
    fn trashing_moves_exactly_one_file_and_flips_the_state() {
        let (_tmp, mut ws) = workspace();
        let note = ws.create(Draft::new("x")).unwrap();

        ws.trash(note.id).unwrap();

        assert_eq!(file_count(ws.root()), 0);
        assert_eq!(file_count(&ws.trash_dir()), 1);
        assert!(matches!(ws.reference(note.id), Ref::Trashed(_)));
    }

    #[test]
    fn trash_never_cascades_and_the_replies_report_a_trashed_parent() {
        let (_tmp, mut ws) = workspace();
        let root = ws.create(Draft::new("root")).unwrap();
        let mid = ws.create(Draft::new("mid").reply_to(root.id)).unwrap();
        let leaf = ws.create(Draft::new("leaf").reply_to(mid.id)).unwrap();

        ws.trash(mid.id).unwrap();

        assert!(matches!(ws.reference(root.id), Ref::Present(_)));
        assert!(matches!(ws.reference(mid.id), Ref::Trashed(_)));
        assert!(
            matches!(ws.reference(leaf.id), Ref::Present(_)),
            "children stay live"
        );
        assert_eq!(ws.trashed().len(), 1);
        assert_eq!(
            ws.thread(root.id).unwrap().tree.len(),
            3,
            "the thread is still whole"
        );
    }

    #[test]
    fn a_trashed_note_leaves_the_timeline_but_keeps_its_replies_in_it() {
        let (_tmp, mut ws) = workspace();
        let root = ws.create(Draft::new("root")).unwrap();
        let reply = ws.create(Draft::new("reply").reply_to(root.id)).unwrap();

        ws.trash(root.id).unwrap();
        let flat = ws.timeline(&TimelineQuery::new().flat());
        let ids: Vec<NoteId> = flat.items.iter().map(|r| r.note.id).collect();

        assert!(!ids.contains(&root.id));
        assert!(ids.contains(&reply.id));
    }

    #[test]
    fn trashing_twice_is_an_error_rather_than_a_second_move() {
        let (_tmp, mut ws) = workspace();
        let note = ws.create(Draft::new("x")).unwrap();
        ws.trash(note.id).unwrap();

        let err = ws.trash(note.id).unwrap_err();
        assert!(matches!(err, Error::AlreadyTrashed { id } if id == note.id.as_uuid()));
        assert_eq!(file_count(&ws.trash_dir()), 1);
    }

    #[test]
    fn trashing_a_note_that_does_not_exist_is_an_error() {
        let (_tmp, mut ws) = workspace();
        let missing = NoteId::new();
        assert!(matches!(
            ws.trash(missing).unwrap_err(),
            Error::NoteNotFound { .. }
        ));
    }

    // ------------------------------------------------------------------------------ restore

    #[test]
    fn restore_puts_the_file_back_under_the_name_it_left_with() {
        let (_tmp, mut ws) = workspace();
        let note = ws
            .create(Draft::new("x").title("Some Title").slugged())
            .unwrap();
        let name = ws
            .snapshot()
            .get(note.id)
            .unwrap()
            .path
            .file_name()
            .unwrap()
            .to_owned();

        ws.trash(note.id).unwrap();
        ws.restore(note.id).unwrap();

        assert!(ws.root().join(&name).exists());
        assert_eq!(file_count(&ws.trash_dir()), 0);
        assert!(matches!(ws.reference(note.id), Ref::Present(_)));
    }

    #[test]
    fn a_trash_and_restore_round_trip_leaves_the_bytes_untouched() {
        // The directory is the state; there is no stamp to write or wipe.
        let (_tmp, mut ws) = workspace();
        let note = ws.create(Draft::new("body").title("t")).unwrap();
        let before = bytes_of(&ws, note.id);

        ws.trash(note.id).unwrap();
        ws.restore(note.id).unwrap();

        assert_eq!(bytes_of(&ws, note.id), before);
    }

    #[test]
    fn restoring_a_note_that_is_not_in_the_trash_is_an_error() {
        let (_tmp, mut ws) = workspace();
        let note = ws.create(Draft::new("x")).unwrap();
        assert!(matches!(
            ws.restore(note.id).unwrap_err(),
            Error::NotTrashed { .. }
        ));
    }

    // -------------------------------------------------------------------------------- purge

    /// Purging the middle of a chain **splits the subtree**, and that is the intended behaviour —
    /// it reverses `stage2.md`'s "assigned once, never recomputed".
    ///
    /// The reversal is safe because `root_id` was never what provided the property it was defended
    /// for. "There was a chain here and a post is gone" comes from the surviving child's dangling
    /// `relation:reply_to`, which lives in the file and is untouched by this change. What is
    /// genuinely lost is grouping across *two* purges — and at that point the chain really has
    /// been broken twice.
    #[test]
    fn purging_removes_one_file_and_leaves_the_children_live_but_no_longer_grouped() {
        let (_tmp, mut ws) = workspace();
        let root = ws.create(Draft::new("root")).unwrap();
        let mid = ws.create(Draft::new("mid").reply_to(root.id)).unwrap();
        let leaf = ws.create(Draft::new("leaf").reply_to(mid.id)).unwrap();

        ws.purge(mid.id).unwrap();

        // Still live, and still visibly the reply to something that is gone.
        assert!(matches!(ws.reference(mid.id), Ref::Deleted(_)));
        assert!(matches!(ws.reference(leaf.id), Ref::Present(_)));
        assert_eq!(
            ws.get(leaf.id).unwrap().unwrap().frontmatter.reply_to,
            Some(mid.id),
            "the evidence of the broken chain is in the file, and survives"
        );

        // No longer grouped under `root`: the note that grouped them is genuinely gone, and the
        // root walk now stops at the id the file still names.
        assert_eq!(ws.meta(leaf.id).unwrap().root, Some(mid.id));
        assert_eq!(ws.meta(root.id).unwrap().root, Some(root.id));
        assert_eq!(file_count(ws.root()), 2);
    }

    #[test]
    fn purging_the_root_leaves_its_children_as_orphan_roots_in_the_timeline() {
        let (_tmp, mut ws) = workspace();
        let root = ws.create(Draft::new("root")).unwrap();
        let a = ws.create(Draft::new("a").reply_to(root.id)).unwrap();
        let b = ws.create(Draft::new("b").reply_to(root.id)).unwrap();

        ws.purge(root.id).unwrap();

        let ids: Vec<NoteId> = ws
            .timeline(&TimelineQuery::new())
            .items
            .iter()
            .map(|r| r.note.id)
            .collect();
        assert!(ids.contains(&a.id) && ids.contains(&b.id), "{ids:?}");
    }

    #[test]
    fn a_note_can_be_purged_straight_from_the_vault_or_out_of_the_trash() {
        let (_tmp, mut ws) = workspace();
        let live = ws.create(Draft::new("live")).unwrap();
        let binned = ws.create(Draft::new("binned")).unwrap();
        ws.trash(binned.id).unwrap();

        ws.purge(live.id).unwrap();
        ws.purge(binned.id).unwrap();

        assert_eq!(file_count(ws.root()), 0);
        assert_eq!(file_count(&ws.trash_dir()), 0);
        assert!(ws.snapshot().is_empty());
    }

    #[test]
    fn a_link_to_a_purged_note_still_extracts_and_resolves_to_deleted() {
        let (_tmp, mut ws) = workspace();
        let target = ws.create(Draft::new("target")).unwrap();
        let linking = ws
            .create(Draft::new(format!("see [[{}]]", target.id)))
            .unwrap();
        assert_eq!(ws.backlinks(target.id).len(), 1);

        ws.purge(target.id).unwrap();

        let links = ws.links_in(linking.id).unwrap();
        assert_eq!(links.len(), 1, "extraction never consults the index");
        assert!(matches!(links[0].1, Ref::Deleted(_)));
        assert_eq!(ws.backlinks(target.id).len(), 1, "the edge still exists");
    }

    // --------------------------------------------------------------------- sync and repair

    #[test]
    fn a_file_moved_into_the_trash_by_hand_is_picked_up_by_the_next_sync() {
        // This is the "killed between the file move and the index write" case: the vault is
        // correct and the snapshot is stale, and a sync is all it takes to agree again.
        let (_tmp, mut ws) = workspace();
        let note = ws.create(Draft::new("x")).unwrap();
        let from = ws.snapshot().get(note.id).unwrap().path.clone();

        std::fs::rename(&from, ws.trash_dir().join(from.file_name().unwrap())).unwrap();
        assert!(
            matches!(ws.reference(note.id), Ref::Present(_)),
            "still stale"
        );

        let report = ws.sync().unwrap();
        assert_eq!(report.updated, vec![note.id]);
        assert!(matches!(ws.reference(note.id), Ref::Trashed(_)));
    }

    #[test]
    fn a_file_deleted_by_hand_drops_out_on_the_next_sync_and_its_children_survive() {
        let (_tmp, mut ws) = workspace();
        let root = ws.create(Draft::new("root")).unwrap();
        let reply = ws.create(Draft::new("reply").reply_to(root.id)).unwrap();

        std::fs::remove_file(&ws.snapshot().get(root.id).unwrap().path).unwrap();
        let report = ws.sync().unwrap();

        assert_eq!(report.removed, vec![root.id]);
        assert!(matches!(ws.reference(root.id), Ref::Deleted(_)));
        assert!(matches!(ws.reference(reply.id), Ref::Present(_)));
    }

    #[test]
    fn rebuild_and_sync_agree_after_a_sequence_of_mutations() {
        // The rebuild invariant, in the form this build can state it: the two produce the same
        // logical content. `edited_at` is exempt everywhere else and needs no exemption here,
        // because neither path writes and both read the same mtimes.
        let (_tmp, mut ws) = workspace();
        let root = ws.create(Draft::new("root").title("r")).unwrap();
        let a = ws.create(Draft::new("a").reply_to(root.id)).unwrap();
        let b = ws.create(Draft::new("b").reply_to(a.id)).unwrap();
        ws.edit(a.id, Edit::new().title("edited")).unwrap();
        ws.trash(b.id).unwrap();
        let doomed = ws.create(Draft::new("doomed")).unwrap();
        ws.purge(doomed.id).unwrap();

        let incremental = ws.snapshot().clone();
        ws.rebuild().unwrap();
        assert_eq!(&incremental, ws.snapshot());
    }

    #[test]
    fn every_mutation_keeps_the_snapshot_and_the_disk_in_agreement() {
        let (_tmp, mut ws) = workspace();
        let root = ws.create(Draft::new("root")).unwrap();
        let reply = ws.create(Draft::new("reply").reply_to(root.id)).unwrap();
        ws.edit(reply.id, Edit::new().title("t")).unwrap();
        ws.trash(reply.id).unwrap();
        ws.restore(reply.id).unwrap();

        let in_memory = ws.snapshot().clone();
        let on_disk = Snapshot::scan(ws.schema(), ws.root()).unwrap();
        assert_eq!(in_memory, on_disk);
    }

    #[test]
    fn the_reads_all_agree_about_a_vault_after_a_realistic_session() {
        let (_tmp, mut ws) = workspace();
        let root = ws.create(Draft::new("root").title("Root note")).unwrap();
        let one = ws.create(Draft::new("one").reply_to(root.id)).unwrap();
        let _two = ws.create(Draft::new("two").reply_to(one.id)).unwrap();
        let three = ws.create(Draft::new("three").reply_to(root.id)).unwrap();
        ws.trash(three.id).unwrap();

        assert_eq!(ws.files(FileSort::Created).len(), 3);
        assert_eq!(ws.trashed().len(), 1);
        assert_eq!(ws.timeline(&TimelineQuery::new()).len(), 1);
        assert_eq!(ws.timeline(&TimelineQuery::new().flat()).len(), 3);
        assert_eq!(ws.search(&SearchQuery::new("root")).len(), 1);
        assert_eq!(ws.thread(root.id).unwrap().tree.len(), 4);
        assert_eq!(
            ws.resolve(&root.id.to_string()).unique().unwrap().id,
            root.id
        );
    }

    #[test]
    fn a_short_prefix_over_notes_captured_together_is_ambiguous_not_unique() {
        // A finding, pinned rather than worked around. Git's short ids work because a SHA is
        // random from its first bit; a UUIDv7's leading 48 bits are a **millisecond timestamp**,
        // so notes captured in the same session share a long prefix and only the random tail
        // separates them. Eight characters — git's default, and `NoteId::short`'s length — cover
        // only the top 32 bits of that timestamp, so a burst of captures collides every time.
        //
        // Ambiguity is handled correctly: it lists candidates and never guesses. But a surface
        // that prints `short()` and then accepts it back will hand people an id that does not
        // resolve, which is a stage 3 problem and is written up in `docs/plans/stage3.md`.
        let (_tmp, mut ws) = workspace();
        let first = ws.create(Draft::new("one")).unwrap();
        let second = ws.create(Draft::new("two")).unwrap();
        assert_eq!(
            first.id.short(),
            second.id.short(),
            "two captures in one millisecond share their first eight characters"
        );

        assert!(matches!(
            ws.resolve(&first.id.short()),
            Resolution::Ambiguous(_)
        ));
        // The full id always works, which is why `--long` exists.
        assert_eq!(
            ws.resolve(&first.id.to_string()).unique().unwrap().id,
            first.id
        );
    }
}
