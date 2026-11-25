//! The crate-wide error taxonomy and `Result` alias.
//!
//! These are the failures of *calls*: opening a workspace, reading a file, talking to the index.
//! The outcome of an action is not an [`Error`] — it is an [`Outcome`](crate::action::Outcome),
//! whose [`Rejection`](crate::action::Rejection) and [`Failure`](crate::action::Failure) say what
//! was refused and what IO declined. A note that fails its schema is not an error either: it is a
//! [`Problem`](crate::problem::Problem) on the note.
//!
//! Every variant names the file or the thing it concerns. There is no `Other(String)` and no
//! blanket `From<io::Error>`: a bare conversion would discard the path at the moment it matters.

use std::path::{Path, PathBuf};

/// The result type used throughout `jot-core`.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything a `jot-core` call can fail with.
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

    /// The rename step of an atomic write failed. Both files are named, because recovering by
    /// hand needs both.
    #[error("cannot rename `{from}` onto `{to}`: {source}")]
    Rename {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A file could not be removed.
    #[error("cannot remove `{path}`: {source}")]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A settings file is not valid UTF-8.
    #[error("`{path}` is not valid UTF-8")]
    NotUtf8 { path: PathBuf },

    // ---------------------------------------------------------------- workspace
    /// The path is not a workspace: it has no `workspace.toml`.
    #[error("`{path}` is not a jot workspace: no `workspace.toml`")]
    NotAWorkspace { path: PathBuf },

    /// The path holds a workspace in the format before the schema-first redesign:
    /// `.jot/workspace.toml`, and no `workspace.toml` at the root.
    #[error(
        "`{path}` is a jot workspace in the old format (`.jot/workspace.toml`, `<id>_<slug>.md`, `[[schema.frontmatter]]`). The format changed: a workspace is now a folder with `workspace.toml` and `schema.json` at its root. This version of jot cannot open the old format, and there is no converter yet"
    )]
    OldFormat { path: PathBuf },

    /// `init` was pointed at a directory that already has a `workspace.toml` or `schema.json`.
    /// Never a silent overwrite.
    #[error("`{path}` already exists; refusing to overwrite it")]
    WorkspaceExists { path: PathBuf },

    /// `discover` walked up to the filesystem root without finding a `workspace.toml`.
    #[error("no jot workspace found in `{from}` or any parent directory")]
    WorkspaceNotFound { from: PathBuf },

    /// `workspace.toml` is broken. Refuses to open the workspace, naming the error.
    #[error("`{path}` is broken: {message}")]
    WorkspaceToml { path: PathBuf, message: String },

    /// A settings file could not be emitted.
    #[error("cannot serialize `{path}`: {message}")]
    SettingsSerialize { path: PathBuf, message: String },

    // ----------------------------------------------------------------- registry
    /// The OS config directory could not be located, so the registry has no home.
    #[error("cannot locate the OS config directory for `{application}`")]
    ConfigDirUnavailable { application: String },

    /// The registry file exists but could not be read. **Recoverable** — see
    /// [`Error::is_registry_recoverable`].
    #[error("cannot read the workspace registry `{path}`: {message}")]
    RegistryUnreadable { path: PathBuf, message: String },

    /// The registry file was read but is not a registry. **Recoverable**.
    #[error("the workspace registry `{path}` is corrupt: {message}")]
    RegistryCorrupt { path: PathBuf, message: String },

    /// The registry could not be emitted.
    #[error("cannot serialize the workspace registry `{path}`: {message}")]
    RegistrySerialize { path: PathBuf, message: String },

    // -------------------------------------------------------------------- the index
    //
    // The index is derived and disposable, so every message says that deleting it is safe.
    /// The index database could not be opened, created, or migrated.
    #[error(
        "cannot open the index `{path}`: {message} (the index is derived — deleting it is safe)"
    )]
    IndexOpen { path: PathBuf, message: String },

    /// A read or a write against the index failed.
    #[error("index `{path}`: {message} (the index is derived — deleting it is safe)")]
    IndexQuery { path: PathBuf, message: String },

    /// The filesystem watcher could not be started. Never fatal to a surface: a workspace with no
    /// watcher is one you sync by hand.
    #[error("cannot watch `{path}` for changes: {message}")]
    Watch { path: PathBuf, message: String },
}

impl Error {
    /// The filesystem path this error is about, if it is about one. For [`Error::Rename`] this
    /// is the target.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Error::Read { path, .. }
            | Error::Write { path, .. }
            | Error::CreateDir { path, .. }
            | Error::ReadDir { path, .. }
            | Error::Remove { path, .. }
            | Error::NotUtf8 { path }
            | Error::NotAWorkspace { path }
            | Error::OldFormat { path }
            | Error::WorkspaceExists { path }
            | Error::WorkspaceToml { path, .. }
            | Error::SettingsSerialize { path, .. }
            | Error::RegistryUnreadable { path, .. }
            | Error::RegistryCorrupt { path, .. }
            | Error::RegistrySerialize { path, .. }
            | Error::IndexOpen { path, .. }
            | Error::IndexQuery { path, .. }
            | Error::Watch { path, .. } => Some(path),
            Error::Rename { to, .. } => Some(to),
            Error::WorkspaceNotFound { from } => Some(from),
            Error::ConfigDirUnavailable { .. } => None,
        }
    }

    /// Whether the registry's load path may swallow this error and continue with an empty
    /// registry: exactly the two registry *read* failures. The registry is a cache; a bad one
    /// costs one re-add and never data.
    pub fn is_registry_recoverable(&self) -> bool {
        matches!(
            self,
            Error::RegistryUnreadable { .. } | Error::RegistryCorrupt { .. }
        )
    }
}
