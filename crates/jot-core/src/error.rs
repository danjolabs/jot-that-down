//! The crate-wide error taxonomy and `Result` alias.
//!
//! # The rule this file exists to enforce
//!
//! From `docs/plans/overview.md`: *"Core errors name the file or note involved; a message that says
//! only 'parse error' is a bug."* Every variant below therefore carries the [`Path`] or the
//! [`Uuid`] it concerns, and its `Display` prints it. There is deliberately **no** catch-all
//! `Other(String)` variant and **no** `#[from]` conversion — a bare `impl From<io::Error>` would
//! discard the path at exactly the moment it becomes interesting, which is the failure mode this
//! taxonomy is built to prevent. Wrap explicitly at the call site instead:
//!
//! ```
//! use jot_core::error::{Error, Result};
//! use std::path::Path;
//!
//! fn read(path: &Path) -> Result<String> {
//!     std::fs::read_to_string(path).map_err(|source| Error::Read {
//!         path: path.to_path_buf(),
//!         source,
//!     })
//! }
//! ```
//!
//! # Strictness
//!
//! Per the stage-1 dispatch rulings U9/U10, stage 1 has **hard errors only** — there is no
//! `Anomaly` type and no warnings channel. Every deviation from the note format is an [`Error`]
//! naming the path, including a filename UUID that disagrees with the frontmatter `id`.
//!
//! The one exception is the workspace registry, which is a cache: a missing or corrupt registry
//! costs one re-add and never data, so its read failures must never propagate into
//! `Workspace::open`. Those variants are still `Error`s, but [`Error::is_registry_recoverable`]
//! lets the registry's load path swallow exactly them and nothing else.
//!
//! # Stability
//!
//! This file is **frozen for the remainder of stage 1**. Stages 2 and 3 extend it; wave-3 and
//! wave-4 stage-1 tasks consume it and add no variants.
//!
//! Two payload conventions worth knowing before you match on anything:
//!
//! - Ids are carried as [`uuid::Uuid`], not as `note::NoteId`. `NoteId` does not exist when this
//!   file lands, and keeping `error` free of a dependency on `note` avoids a module cycle. Callers
//!   holding a `NoteId` pass its inner `Uuid`.
//! - Failures originating in a third-party parser (YAML, TOML) carry the parser's own diagnostic as
//!   a `message: String` rather than as a `#[source]` of that crate's error type. That keeps the
//!   frozen enum from pinning the public API to a specific YAML or TOML crate, and those
//!   diagnostics already carry line and column — see `docs/plans/runs/stage1/yaml-crate.md`.

use std::path::{Path, PathBuf};

use uuid::Uuid;

/// The result type used throughout `jot-core`.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong inside `jot-core` during stage 1.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    // ---------------------------------------------------------------- filesystem
    /// A file could not be read.
    #[error("cannot read `{path}`: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A file could not be written.
    #[error("cannot write `{path}`: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A directory could not be created.
    #[error("cannot create directory `{path}`: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A directory could not be listed.
    #[error("cannot list directory `{path}`: {source}")]
    ReadDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The rename step of an atomic write failed. The staged file is named as well as the target,
    /// because recovering by hand needs both.
    #[error("cannot rename `{from}` onto `{to}`: {source}")]
    Rename {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A file could not be removed — a staged temp file after a failed atomic write, say.
    #[error("cannot remove `{path}`: {source}")]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A file on the vault path is not valid UTF-8. Notes and manifests are text; a note that is
    /// not decodable is not a note, and saying so beats a generic I/O error.
    #[error("`{path}` is not valid UTF-8")]
    NotUtf8 { path: PathBuf },

    // ------------------------------------------------------------- note filenames
    /// A file in the vault is not named `<uuid>.md` or `<uuid>_<slug>.md`.
    #[error("`{path}` is not a note filename (expected `<uuid>.md` or `<uuid>_<slug>.md`)")]
    InvalidNoteFilename { path: PathBuf },

    /// The UUID in the filename disagrees with the frontmatter `id`.
    ///
    /// The frontmatter wins as a matter of format — see U9 — but loading *from a path* reports the
    /// disagreement rather than silently resolving it, so both ids are carried.
    #[error(
        "`{path}`: filename id `{filename_id}` disagrees with frontmatter id `{frontmatter_id}`"
    )]
    NoteIdMismatch {
        path: PathBuf,
        filename_id: Uuid,
        frontmatter_id: Uuid,
    },

    // -------------------------------------------------------------- frontmatter
    /// The file does not open with a `---` fence.
    #[error("`{path}` has no frontmatter: expected a `---` fence on the first line")]
    MissingFrontmatterFence { path: PathBuf },

    /// The file opens with `---` but never closes the block.
    #[error("`{path}` has an unterminated frontmatter block: no closing `---` fence")]
    UnterminatedFrontmatter { path: PathBuf },

    /// The frontmatter block is not well-formed YAML. `message` is the YAML parser's own
    /// diagnostic, which carries line and column within the block.
    #[error("`{path}` has malformed frontmatter YAML: {message}")]
    MalformedYaml { path: PathBuf, message: String },

    /// The frontmatter block parsed as YAML but is not a mapping — an empty block, a bare scalar,
    /// or a sequence.
    #[error("`{path}`: frontmatter must be a YAML mapping")]
    FrontmatterNotAMapping { path: PathBuf },

    /// The required `id` key is absent.
    #[error("`{path}` is missing the required frontmatter key `id`")]
    MissingId { path: PathBuf },

    /// The required `created_at` key is absent.
    #[error("`{path}` is missing the required frontmatter key `created_at`")]
    MissingCreatedAt { path: PathBuf },

    /// The required `root` key is absent. A top-level note's `root` is its own `id`; a note written
    /// by hand without one cannot be loaded (U10).
    #[error("`{path}` is missing the required frontmatter key `root`")]
    MissingRoot { path: PathBuf },

    /// A known frontmatter key holds a value of the wrong YAML shape — `title` as a sequence, say.
    #[error("`{path}`: frontmatter key `{field}` has the wrong type: {message}")]
    InvalidFrontmatterField {
        path: PathBuf,
        field: &'static str,
        message: String,
    },

    /// A known id-bearing frontmatter key (`id`, `reply_to`, `root`, `quote`) holds something that
    /// is not a UUID.
    #[error("`{path}`: frontmatter key `{field}` is not a UUID: `{value}`")]
    InvalidNoteIdValue {
        path: PathBuf,
        field: &'static str,
        value: String,
    },

    /// A known timestamp key (`created_at`, `edited_at`, `trashed_at`) is not RFC 3339.
    #[error("`{path}`: frontmatter key `{field}` is not an RFC 3339 timestamp: `{value}`")]
    InvalidTimestamp {
        path: PathBuf,
        field: &'static str,
        value: String,
    },

    /// A note's frontmatter could not be emitted on the canonical path. The note is identified by
    /// id rather than by path because a note being created has an id before it has a filename.
    #[error("cannot serialize frontmatter for note `{id}`: {message}")]
    SerializeFrontmatter { id: Uuid, message: String },

    // ---------------------------------------------------------------- workspace
    /// The path is not a workspace: it has no `.jot/` directory.
    #[error("`{path}` is not a jot workspace: no `.jot/` directory")]
    NotAWorkspace { path: PathBuf },

    /// `init` was pointed at a directory that already has a `.jot/`. Never a silent overwrite (U3).
    #[error("`{path}` is already a jot workspace")]
    WorkspaceExists { path: PathBuf },

    /// `discover` walked up from this path to the filesystem root without finding a `.jot/`.
    #[error("no jot workspace found in `{from}` or any parent directory")]
    WorkspaceNotFound { from: PathBuf },

    /// `workspace.toml` is not well-formed TOML, or is missing a key it must have. `message` is the
    /// TOML parser's own diagnostic.
    #[error("cannot parse workspace manifest `{path}`: {message}")]
    ManifestParse { path: PathBuf, message: String },

    /// `workspace.toml` could not be emitted.
    #[error("cannot serialize workspace manifest `{path}`: {message}")]
    ManifestSerialize { path: PathBuf, message: String },

    /// The manifest declares a `schema_version` this build does not understand. Both versions are
    /// carried so the message can say plainly that the workspace was written by a newer version.
    #[error(
        "`{path}` declares schema_version {found}, but this build of jot supports at most {supported} — the workspace was written by a newer version"
    )]
    UnsupportedSchemaVersion {
        path: PathBuf,
        found: u32,
        supported: u32,
    },

    /// `[workspace] kind` is neither `jot` nor `plain`.
    #[error("`{path}`: unknown workspace kind `{value}` (expected `jot` or `plain`)")]
    InvalidWorkspaceKind { path: PathBuf, value: String },

    /// `[workspace] id` is not a UUID.
    #[error("`{path}`: workspace id `{value}` is not a UUID")]
    InvalidWorkspaceId { path: PathBuf, value: String },

    // ----------------------------------------------------------------- registry
    /// The OS config directory could not be located, so the registry has no home. Carries the
    /// application identity that was looked up.
    #[error("cannot locate the OS config directory for `{application}`")]
    ConfigDirUnavailable { application: String },

    /// The registry file exists but could not be read. **Recoverable** — see
    /// [`Error::is_registry_recoverable`].
    #[error("cannot read the workspace registry `{path}`: {message}")]
    RegistryUnreadable { path: PathBuf, message: String },

    /// The registry file was read but does not parse, or parses into a shape that is not a
    /// registry. **Recoverable** — see [`Error::is_registry_recoverable`].
    #[error("the workspace registry `{path}` is corrupt: {message}")]
    RegistryCorrupt { path: PathBuf, message: String },

    /// The registry could not be emitted. Not recoverable: a save that silently does nothing loses
    /// the user's action.
    #[error("cannot serialize the workspace registry `{path}`: {message}")]
    RegistrySerialize { path: PathBuf, message: String },
}

impl Error {
    /// The filesystem path this error is about, if it is about one.
    ///
    /// `None` for exactly two variants: [`Error::SerializeFrontmatter`], which identifies a note
    /// that may not have a filename yet, and [`Error::ConfigDirUnavailable`], which fires precisely
    /// because there is no path to name.
    ///
    /// For [`Error::Rename`] this is the *target* — the file the caller was trying to produce.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Error::Read { path, .. }
            | Error::Write { path, .. }
            | Error::CreateDir { path, .. }
            | Error::ReadDir { path, .. }
            | Error::Remove { path, .. }
            | Error::NotUtf8 { path }
            | Error::InvalidNoteFilename { path }
            | Error::NoteIdMismatch { path, .. }
            | Error::MissingFrontmatterFence { path }
            | Error::UnterminatedFrontmatter { path }
            | Error::MalformedYaml { path, .. }
            | Error::FrontmatterNotAMapping { path }
            | Error::MissingId { path }
            | Error::MissingCreatedAt { path }
            | Error::MissingRoot { path }
            | Error::InvalidFrontmatterField { path, .. }
            | Error::InvalidNoteIdValue { path, .. }
            | Error::InvalidTimestamp { path, .. }
            | Error::NotAWorkspace { path }
            | Error::WorkspaceExists { path }
            | Error::ManifestParse { path, .. }
            | Error::ManifestSerialize { path, .. }
            | Error::UnsupportedSchemaVersion { path, .. }
            | Error::InvalidWorkspaceKind { path, .. }
            | Error::InvalidWorkspaceId { path, .. }
            | Error::RegistryUnreadable { path, .. }
            | Error::RegistryCorrupt { path, .. }
            | Error::RegistrySerialize { path, .. } => Some(path),
            Error::Rename { to, .. } => Some(to),
            Error::WorkspaceNotFound { from } => Some(from),
            Error::SerializeFrontmatter { .. } | Error::ConfigDirUnavailable { .. } => None,
        }
    }

    /// Whether the registry's load path is allowed to swallow this error and continue with an empty
    /// registry.
    ///
    /// True for exactly the two registry *read* failures. The registry is a cache; per the U5
    /// ruling a bad one costs one re-add and never data, and it must never propagate into
    /// `Workspace::open`. Everything else — including a registry that cannot be *written* — is a
    /// real failure the caller must see.
    pub fn is_registry_recoverable(&self) -> bool {
        matches!(
            self,
            Error::RegistryUnreadable { .. } | Error::RegistryCorrupt { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    fn io_err() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access is denied")
    }

    const A: &str = "01a03d4c-c708-7cbf-83c0-883cedb7f1d5";
    const B: &str = "01a03d51-4b48-72e2-9f30-f180030c06ab";

    fn uuid_a() -> Uuid {
        Uuid::parse_str(A).unwrap()
    }

    fn uuid_b() -> Uuid {
        Uuid::parse_str(B).unwrap()
    }

    /// One sample of every variant, so the tests below are exhaustive by construction. Adding a
    /// variant without adding it here fails `variant_sample_list_is_exhaustive`.
    fn every_variant() -> Vec<Error> {
        vec![
            Error::Read {
                path: "notes/a.md".into(),
                source: io_err(),
            },
            Error::Write {
                path: "notes/b.md".into(),
                source: io_err(),
            },
            Error::CreateDir {
                path: "notes/.jot".into(),
                source: io_err(),
            },
            Error::ReadDir {
                path: "notes".into(),
                source: io_err(),
            },
            Error::Rename {
                from: "notes/.jot/tmp/x".into(),
                to: "notes/c.md".into(),
                source: io_err(),
            },
            Error::Remove {
                path: "notes/.jot/tmp/x".into(),
                source: io_err(),
            },
            Error::NotUtf8 {
                path: "notes/d.md".into(),
            },
            Error::InvalidNoteFilename {
                path: "notes/README.md".into(),
            },
            Error::NoteIdMismatch {
                path: "notes/e.md".into(),
                filename_id: uuid_a(),
                frontmatter_id: uuid_b(),
            },
            Error::MissingFrontmatterFence {
                path: "notes/f.md".into(),
            },
            Error::UnterminatedFrontmatter {
                path: "notes/g.md".into(),
            },
            Error::MalformedYaml {
                path: "notes/h.md".into(),
                message: "did not find expected ',' at line 2 column 1".into(),
            },
            Error::FrontmatterNotAMapping {
                path: "notes/i.md".into(),
            },
            Error::MissingId {
                path: "notes/j.md".into(),
            },
            Error::MissingCreatedAt {
                path: "notes/k.md".into(),
            },
            Error::MissingRoot {
                path: "notes/l.md".into(),
            },
            Error::InvalidFrontmatterField {
                path: "notes/m.md".into(),
                field: "title",
                message: "expected a string, found a sequence".into(),
            },
            Error::InvalidNoteIdValue {
                path: "notes/n.md".into(),
                field: "reply_to",
                value: "not-a-uuid".into(),
            },
            Error::InvalidTimestamp {
                path: "notes/o.md".into(),
                field: "created_at",
                value: "yesterday".into(),
            },
            Error::SerializeFrontmatter {
                id: uuid_a(),
                message: "recursion limit exceeded".into(),
            },
            Error::NotAWorkspace {
                path: "somewhere".into(),
            },
            Error::WorkspaceExists {
                path: "notes".into(),
            },
            Error::WorkspaceNotFound {
                from: "notes/a/b/c".into(),
            },
            Error::ManifestParse {
                path: "notes/.jot/workspace.toml".into(),
                message: "expected `=` at line 3".into(),
            },
            Error::ManifestSerialize {
                path: "notes/.jot/workspace.toml".into(),
                message: "unsupported type".into(),
            },
            Error::UnsupportedSchemaVersion {
                path: "notes/.jot/workspace.toml".into(),
                found: 7,
                supported: 1,
            },
            Error::InvalidWorkspaceKind {
                path: "notes/.jot/workspace.toml".into(),
                value: "banana".into(),
            },
            Error::InvalidWorkspaceId {
                path: "notes/.jot/workspace.toml".into(),
                value: "nope".into(),
            },
            Error::ConfigDirUnavailable {
                application: "danjolabs/jot".into(),
            },
            Error::RegistryUnreadable {
                path: "config/workspaces.toml".into(),
                message: "access is denied".into(),
            },
            Error::RegistryCorrupt {
                path: "config/workspaces.toml".into(),
                message: "expected a table".into(),
            },
            Error::RegistrySerialize {
                path: "config/workspaces.toml".into(),
                message: "unsupported type".into(),
            },
        ]
    }

    /// Catches a variant added to the enum without a sample here, which would silently shrink the
    /// coverage of every other test in this module.
    #[test]
    fn variant_sample_list_is_exhaustive() {
        let samples = every_variant();
        let mut discriminants: Vec<String> = samples
            .iter()
            .map(|e| {
                format!("{e:?}")
                    .split_whitespace()
                    .next()
                    .unwrap()
                    .to_string()
            })
            .collect();
        discriminants.sort();
        discriminants.dedup();
        assert_eq!(
            discriminants.len(),
            samples.len(),
            "every_variant() has a duplicate or missing variant: {discriminants:?}"
        );
        // Bump deliberately when the taxonomy grows; stage 1 froze it at 32.
        assert_eq!(
            samples.len(),
            32,
            "the stage-1 error taxonomy has 32 variants"
        );
    }

    /// The rule from `overview.md`: an error that says only "parse error" is a bug. Every variant
    /// that concerns a path must print that path.
    #[test]
    fn every_path_carrying_variant_prints_its_path() {
        for e in every_variant() {
            let Some(path) = e.path() else { continue };
            let shown = e.to_string();
            let needle = path.display().to_string();
            assert!(
                shown.contains(&needle),
                "`{shown}` does not name its path `{needle}`"
            );
        }
    }

    /// The two variants with no path must still identify what they concern.
    #[test]
    fn pathless_variants_still_identify_their_subject() {
        let e = Error::SerializeFrontmatter {
            id: uuid_a(),
            message: "boom".into(),
        };
        assert!(e.path().is_none());
        assert!(e.to_string().contains(A), "{e}");

        let e = Error::ConfigDirUnavailable {
            application: "danjolabs/jot".into(),
        };
        assert!(e.path().is_none());
        assert!(e.to_string().contains("danjolabs/jot"), "{e}");
    }

    /// No variant may degrade into a bare "parse error"-class message.
    #[test]
    fn no_variant_has_an_empty_or_useless_message() {
        for e in every_variant() {
            let shown = e.to_string();
            assert!(shown.len() > 10, "message too thin: `{shown}`");
            assert!(!shown.trim().is_empty(), "empty message for {e:?}");
        }
    }

    /// The mismatch error is the one place the doc names both ids explicitly; losing either makes
    /// it unactionable.
    #[test]
    fn note_id_mismatch_names_the_path_and_both_ids() {
        let e = Error::NoteIdMismatch {
            path: "vault/01a03d50-bac0-7851-bd56-683ef65923cd.md".into(),
            filename_id: uuid_a(),
            frontmatter_id: uuid_b(),
        };
        let shown = e.to_string();
        assert!(
            shown.contains("01a03d50-bac0-7851-bd56-683ef65923cd.md"),
            "{shown}"
        );
        assert!(shown.contains(A), "{shown}");
        assert!(shown.contains(B), "{shown}");
    }

    /// `open()` must be able to say plainly that the workspace is from the future, which needs both
    /// numbers.
    #[test]
    fn unsupported_schema_version_names_both_versions() {
        let e = Error::UnsupportedSchemaVersion {
            path: "v/.jot/workspace.toml".into(),
            found: 7,
            supported: 1,
        };
        let shown = e.to_string();
        assert!(shown.contains('7'), "{shown}");
        assert!(shown.contains('1'), "{shown}");
        assert!(shown.contains("newer version"), "{shown}");
    }

    /// The three required-key failures must be distinguishable, not one variant with a field name.
    #[test]
    fn each_required_key_has_its_own_variant() {
        let p = PathBuf::from("v/a.md");
        let msgs = [
            Error::MissingId { path: p.clone() }.to_string(),
            Error::MissingCreatedAt { path: p.clone() }.to_string(),
            Error::MissingRoot { path: p }.to_string(),
        ];
        assert!(msgs[0].contains("`id`"), "{}", msgs[0]);
        assert!(msgs[1].contains("`created_at`"), "{}", msgs[1]);
        assert!(msgs[2].contains("`root`"), "{}", msgs[2]);
        let mut sorted = msgs.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 3, "required-key messages are not distinct");
    }

    /// The three fence/YAML failures must likewise be distinguishable.
    #[test]
    fn each_frontmatter_shape_failure_has_its_own_variant() {
        let p = PathBuf::from("v/a.md");
        let msgs = [
            Error::MissingFrontmatterFence { path: p.clone() }.to_string(),
            Error::UnterminatedFrontmatter { path: p.clone() }.to_string(),
            Error::MalformedYaml {
                path: p.clone(),
                message: "bad".into(),
            }
            .to_string(),
            Error::FrontmatterNotAMapping { path: p }.to_string(),
        ];
        let mut sorted = msgs.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            4,
            "frontmatter shape messages are not distinct"
        );
    }

    /// U5: exactly the registry read failures may be swallowed. A registry that cannot be *written*
    /// must not be, or a `jot use` silently does nothing.
    #[test]
    fn only_registry_reads_are_recoverable() {
        let recoverable: Vec<String> = every_variant()
            .into_iter()
            .filter(Error::is_registry_recoverable)
            .map(|e| {
                format!("{e:?}")
                    .split_whitespace()
                    .next()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(recoverable, ["RegistryUnreadable", "RegistryCorrupt"]);
    }

    /// I/O-backed variants keep the underlying error reachable through `source()`, so a surface can
    /// print the chain.
    #[test]
    fn io_variants_expose_their_source() {
        for e in every_variant() {
            let is_io = matches!(
                e,
                Error::Read { .. }
                    | Error::Write { .. }
                    | Error::CreateDir { .. }
                    | Error::ReadDir { .. }
                    | Error::Rename { .. }
                    | Error::Remove { .. }
            );
            assert_eq!(
                e.source().is_some(),
                is_io,
                "source() disagrees with the variant kind for {e:?}"
            );
        }
    }

    /// `Rename` names both files: recovering a half-finished atomic write by hand needs the staged
    /// path as well as the target, and `path()` must point at the target.
    #[test]
    fn rename_names_both_paths_and_reports_the_target() {
        let e = Error::Rename {
            from: "v/.jot/tmp/staged".into(),
            to: "v/note.md".into(),
            source: io_err(),
        };
        let shown = e.to_string();
        assert!(shown.contains("staged"), "{shown}");
        assert!(shown.contains("note.md"), "{shown}");
        assert_eq!(e.path(), Some(Path::new("v/note.md")));
    }

    #[test]
    fn result_alias_is_usable() {
        fn f(ok: bool) -> Result<u8> {
            if ok {
                Ok(1)
            } else {
                Err(Error::NotUtf8 {
                    path: "v/a.md".into(),
                })
            }
        }
        assert_eq!(f(true).unwrap(), 1);
        assert!(f(false).is_err());
    }
}
