//! The workspace registry: known workspaces and the current one, persisted in the OS config
//! directory. Landed by T3.3 (stage 1, wave 3), resolving U5.
//!
//! # Shape (U5)
//!
//! - **Location.** `directories::ProjectDirs::from("", "danjolabs", "jot")`'s `config_dir()`. On
//!   Windows this is `%APPDATA%\danjolabs\jot\config` (verified by T2.1). [`default_path`] is the
//!   *only* place this crate calls into `directories` — every test injects its own path instead, so
//!   nothing here ever touches the real OS config directory.
//! - **File.** `workspaces.toml`, joined onto that directory.
//! - **Format.** TOML, matching `workspace.toml` so the project has one config format.
//! - **Keying.** Entries are keyed by the workspace's minted `id`, never by path. `path` is a mutable
//!   field on the entry: moving a vault updates the field rather than orphaning the entry. This is
//!   the whole point of a self-identifying directory.
//! - **Fields per entry.** `id`, `path`, `name`, `last_opened` (RFC 3339, UTC).
//! - **"The current one"** is a single global `current` key holding an id. It is never validated
//!   against the entry list — a `current` that names a since-removed id is a dangling reference, not
//!   an error, matching this project's "no cascading anything" convention.
//!
//! A worked example — this is the literal output of `Registry::save_to` given two entries and a
//! current workspace (verified against a real save in this task's report; `toml`'s pretty printer
//! chooses TOML literal strings, single-quoted, for the Windows paths, so no backslash escaping is
//! needed at all):
//!
//! ```toml
//! current = "01a03d20-a54c-7977-a1f4-1a88b38855dd"
//!
//! [[workspace]]
//! id = "01a03d20-a54c-7977-a1f4-1a88b38855dd"
//! path = 'C:\Users\jun\notes'
//! name = "Thoughts"
//! last_opened = "2026-08-30T07:08:43Z"
//!
//! [[workspace]]
//! id = "01a03d30-2b6b-7c22-9def-abcdef123456"
//! path = 'C:\Users\jun\work-notes'
//! name = "Work"
//! last_opened = "2026-08-29T12:00:00Z"
//! ```
//!
//! # Stale entries
//!
//! "Stale" means the entry's `path` no longer exists on disk. A stale entry is **retained**, never
//! removed automatically, and never an error (U5). It is made observable by attaching a `stale: bool`
//! snapshot to each [`Entry`], computed with [`Path::exists`] at the moment the entry is loaded from
//! disk (in [`Registry::load_from`]) or constructed fresh (in [`Entry::new`]). A caller reads it with
//! [`Entry::is_stale`], or filters the whole registry with [`Registry::stale_entries`]. It is a
//! snapshot, not a live check re-run on every access — re-checking staleness after any filesystem
//! change requires a fresh `load_from` (or `Entry::set_path`, which recomputes it), which keeps the
//! semantics simple and matches "reported by the load path" from the ruling.
//!
//! # Totality
//!
//! [`Registry::load_from`] never returns an `Err` for a missing or corrupt file — a bad registry
//! costs one re-add, never data, and must never propagate into `Workspace::open` (U5, U7). Two of the
//! three failure shapes are folded into the same outcome:
//!
//! - **Missing file** → an empty [`Registry`], indistinguishable from a fresh install. Not a
//!   degraded state, so [`Registry::recovered`] is `None`.
//! - **Present but unreadable** (permissions, or the path is a directory) or **present but
//!   corrupt** (malformed TOML, or well-formed TOML that is not a valid registry — a `last_opened`
//!   that is not RFC 3339, say) → an empty [`Registry`] plus a **recoverable signal**:
//!   [`Registry::recovered`] returns `Some(&Error)`, carrying an [`Error::RegistryUnreadable`] or
//!   [`Error::RegistryCorrupt`]. Both variants satisfy [`Error::is_registry_recoverable`], which is
//!   asserted (debug-only) at the one place this module constructs a recovered, empty registry,
//!   rather than inventing a second signalling mechanism.
//!
//! `save_to` is **not** total: a write failure loses the user's most recent registration or `use`, so
//! it propagates as an ordinary `Err`.
//!
//! # Save: staging directory
//!
//! [`Registry::save_to`] stages through [`crate::fs::atomic_write`] — it is not reimplemented here.
//! The registry has no `.jot/tmp/` to stage into, because it lives outside any vault, so the chosen
//! staging directory is **the target file's own parent directory** (the config dir itself),
//! `create_dir_all`'d first if it does not yet exist (a fresh install has no config directory at
//! all). Staging beside the target guarantees the same volume, which is what makes the following
//! rename atomic in the first place; a separate `tmp/` subdirectory would need to be created and
//! would not obviously be safer.
//!
//! # Path encoding
//!
//! `Entry::path` is stored as a native [`PathBuf`], serialized through `serde`'s own
//! `Serialize`/`Deserialize` impls for `PathBuf` (a plain string under the hood, escaped by the TOML
//! writer as needed). No lossy conversion, no forced forward-slash normalization — unlike index
//! paths (`overview.md`'s "relative to the workspace root, forward slashes" convention), a registry
//! path is absolute by necessity and points anywhere on disk, so it is kept in its native OS form:
//! backslashes, drive letters, and UNC prefixes all round-trip unchanged.
//!
//! # The timestamp trap (U2)
//!
//! `chrono`'s derived `serde` impl emits `DateTime<Utc>` with subsecond precision
//! (`2026-08-30T07:08:43.250180Z`), not the second-precision RFC 3339 this format wants. `last_opened`
//! is therefore **not** given directly to `serde` — [`EntryFile`] carries it as a plain `String`, and
//! [`Registry`] converts explicitly with `to_rfc3339_opts(SecondsFormat::Secs, true)` on write and
//! `DateTime::parse_from_rfc3339` on read.

use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use indexmap::IndexMap;
use uuid::Uuid;

use crate::error::{Error, Result};

/// Resolves the registry's default location: the OS config directory for `("", "danjolabs",
/// "jot")`, joined with `workspaces.toml`.
///
/// This is the **only** function in this module (or, per U5's ownership note, in the crate) that
/// calls into `directories`. Every other entry point here takes an explicit path, precisely so tests
/// — and callers who want a `--registry-path` escape hatch later — never have to touch this.
///
/// No test may call this and then read or write at the returned path; a test may only assert the
/// path's shape (see the module's test suite).
pub fn default_path() -> Result<PathBuf> {
    directories::ProjectDirs::from("", "danjolabs", "jot")
        .map(|dirs| dirs.config_dir().join("workspaces.toml"))
        .ok_or_else(|| Error::ConfigDirUnavailable {
            application: "danjolabs/jot".into(),
        })
}

/// One known workspace: where it lives, its display name, and when it was last opened.
///
/// `stale` is a snapshot, not a live property — see the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    id: Uuid,
    path: PathBuf,
    name: String,
    last_opened: DateTime<Utc>,
    stale: bool,
}

impl Entry {
    /// Builds a new entry, computing `stale` immediately from whether `path` exists right now.
    pub fn new(
        id: Uuid,
        path: PathBuf,
        name: impl Into<String>,
        last_opened: DateTime<Utc>,
    ) -> Entry {
        let stale = !path.exists();
        Entry {
            id,
            path,
            name: name.into(),
            last_opened,
            stale,
        }
    }

    /// The workspace's minted id — the entry's key in the registry.
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// Where the workspace currently lives, as last recorded.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The display name, editable by hand in `workspace.toml` and copied here on registration.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// When this workspace was last opened, RFC 3339 UTC, second precision.
    pub fn last_opened(&self) -> DateTime<Utc> {
        self.last_opened
    }

    /// Whether `path` failed an existence check as of the last load or mutation of this entry.
    pub fn is_stale(&self) -> bool {
        self.stale
    }

    /// Records that the workspace moved, and recomputes staleness against the new path.
    pub fn set_path(&mut self, path: PathBuf) {
        self.stale = !path.exists();
        self.path = path;
    }

    /// Updates the display name.
    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }

    /// Stamps `last_opened`, as `Workspace::open` would on a successful open (stage 4 wires the
    /// call; this module only provides the mutation).
    pub fn touch(&mut self, when: DateTime<Utc>) {
        self.last_opened = when;
    }
}

/// The workspace registry: known workspaces keyed by id, plus which one is current.
///
/// See the module docs for the on-disk shape, the totality guarantee on [`Registry::load_from`], and
/// how staleness and registry corruption are surfaced.
#[derive(Debug, Default)]
pub struct Registry {
    current: Option<Uuid>,
    entries: IndexMap<Uuid, Entry>,
    recovered: Option<Error>,
}

impl Registry {
    /// An empty registry with nothing current and no recovered-from signal — what a fresh install
    /// looks like before anything is ever registered.
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Loads the registry from `path`. Total: see the module docs' "Totality" section for exactly
    /// what each failure shape produces. Never returns `Err`.
    pub fn load_from(path: &Path) -> Result<Registry> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Registry::new());
            }
            Err(source) => {
                let err = Error::RegistryUnreadable {
                    path: path.to_path_buf(),
                    message: source.to_string(),
                };
                return Ok(Registry::recovered_empty(err));
            }
        };

        let file: RegistryFile = match toml::from_str(&text) {
            Ok(file) => file,
            Err(source) => {
                let err = Error::RegistryCorrupt {
                    path: path.to_path_buf(),
                    message: source.to_string(),
                };
                return Ok(Registry::recovered_empty(err));
            }
        };

        match Registry::from_file(file) {
            Ok(registry) => Ok(registry),
            Err(message) => {
                let err = Error::RegistryCorrupt {
                    path: path.to_path_buf(),
                    message,
                };
                Ok(Registry::recovered_empty(err))
            }
        }
    }

    /// Saves the registry to `path`, staging through [`crate::fs::atomic_write`] beside the target
    /// (see the module docs' "Save: staging directory" section). Not total: a write failure is a
    /// real `Err`, because a save that silently loses the user's action is worse than a crash.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let toml_text =
            toml::to_string_pretty(&self.to_file()).map_err(|source| Error::RegistrySerialize {
                path: path.to_path_buf(),
                message: source.to_string(),
            })?;

        let tmp_dir = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(tmp_dir).map_err(|source| Error::CreateDir {
            path: tmp_dir.to_path_buf(),
            source,
        })?;

        crate::fs::atomic_write(path, tmp_dir, toml_text.as_bytes())
    }

    /// The id of the current workspace, if one is set. Not validated against `entries` — a dangling
    /// `current` is a designed-for state, not corruption.
    pub fn current(&self) -> Option<Uuid> {
        self.current
    }

    /// Sets the current workspace by id.
    pub fn set_current(&mut self, id: Uuid) {
        self.current = Some(id);
    }

    /// Clears the current workspace.
    pub fn clear_current(&mut self) {
        self.current = None;
    }

    /// Looks up an entry by id.
    pub fn get(&self, id: Uuid) -> Option<&Entry> {
        self.entries.get(&id)
    }

    /// Inserts a new entry or replaces the existing one with the same id.
    pub fn upsert(&mut self, entry: Entry) {
        self.entries.insert(entry.id, entry);
    }

    /// Removes an entry by id, returning it if it was present. Does not touch `current`, even if it
    /// named the id just removed — a dangling `current` is not an error (see [`Registry::current`]).
    pub fn remove(&mut self, id: Uuid) -> Option<Entry> {
        self.entries.shift_remove(&id)
    }

    /// Whether an id is registered.
    pub fn contains(&self, id: Uuid) -> bool {
        self.entries.contains_key(&id)
    }

    /// The number of known workspaces.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the registry has no known workspaces.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// All known workspaces, in insertion order.
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }

    /// Known workspaces whose recorded path failed an existence check as of load time.
    pub fn stale_entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values().filter(|entry| entry.stale)
    }

    /// `Some` if this registry is the empty result of swallowing an unreadable-or-corrupt file
    /// during [`Registry::load_from`] rather than a genuinely fresh or freshly-loaded one. The
    /// carried [`Error`] is the diagnostic that was swallowed; it always satisfies
    /// [`Error::is_registry_recoverable`].
    pub fn recovered(&self) -> Option<&Error> {
        self.recovered.as_ref()
    }

    /// Builds the empty, "recovered from a bad file" registry, asserting (debug builds only) that
    /// the error being swallowed is actually one U5 allows to be swallowed. This is the one call
    /// site for that assertion, per the instruction to use `Error::is_registry_recoverable` rather
    /// than a parallel mechanism.
    fn recovered_empty(err: Error) -> Registry {
        debug_assert!(
            err.is_registry_recoverable(),
            "registry load swallowed a non-recoverable error: {err}"
        );
        Registry {
            current: None,
            entries: IndexMap::new(),
            recovered: Some(err),
        }
    }

    /// Converts a deserialized [`RegistryFile`] into a validated, typed [`Registry`], folding any
    /// semantic problem (an id that somehow isn't a UUID would already have failed TOML
    /// deserialization; a `last_opened` that isn't RFC 3339 has not) into a single `String` reason so
    /// the caller can report it as [`Error::RegistryCorrupt`] alongside genuine parse failures —
    /// "corrupt" covers both shapes per the totality guarantee.
    fn from_file(file: RegistryFile) -> std::result::Result<Registry, String> {
        let mut entries = IndexMap::new();
        for raw in file.workspaces {
            let last_opened = DateTime::parse_from_rfc3339(&raw.last_opened)
                .map_err(|source| {
                    format!(
                        "workspace `{}`: last_opened `{}` is not RFC 3339: {source}",
                        raw.id, raw.last_opened
                    )
                })?
                .with_timezone(&Utc);
            let stale = !raw.path.exists();
            entries.insert(
                raw.id,
                Entry {
                    id: raw.id,
                    path: raw.path,
                    name: raw.name,
                    last_opened,
                    stale,
                },
            );
        }

        Ok(Registry {
            current: file.current,
            entries,
            recovered: None,
        })
    }

    /// The inverse of [`Registry::from_file`]: builds the plain, serde-friendly shape that gets
    /// written to disk.
    fn to_file(&self) -> RegistryFile {
        RegistryFile {
            current: self.current,
            workspaces: self
                .entries
                .values()
                .map(|entry| EntryFile {
                    id: entry.id,
                    path: entry.path.clone(),
                    name: entry.name.clone(),
                    last_opened: entry.last_opened.to_rfc3339_opts(SecondsFormat::Secs, true),
                })
                .collect(),
        }
    }
}

/// The on-disk shape of `workspaces.toml`. Private: callers only ever see [`Registry`] and
/// [`Entry`], which carry typed, validated data.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct RegistryFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current: Option<Uuid>,
    #[serde(default, rename = "workspace", skip_serializing_if = "Vec::is_empty")]
    workspaces: Vec<EntryFile>,
}

/// The on-disk shape of one `[[workspace]]` table. `last_opened` is a `String`, not a
/// `DateTime<Utc>` — see the module docs' "the timestamp trap" section for why.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct EntryFile {
    id: Uuid,
    path: PathBuf,
    name: String,
    last_opened: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn uuid(s: &str) -> Uuid {
        Uuid::parse_str(s).unwrap()
    }

    const A: &str = "01a03d20-a54c-7977-a1f4-1a88b38855dd";
    const B: &str = "01a03d30-2b6b-7c22-9def-abcdef123456";

    // ---------------------------------------------------------------- default_path

    /// `default_path` must resolve to a `workspaces.toml` under jot's own OS config dir (the
    /// `("", "danjolabs", "jot")` qualifier triple from U5), but this test never reads or writes
    /// there — only the shape of the returned path is asserted. Catches a wrong file name, a path
    /// that forgot to join the file on at all, or a qualifier triple that resolved to someone else's
    /// config directory.
    #[test]
    fn default_path_ends_in_workspaces_toml() {
        let path = default_path().expect("directories should resolve on any supported platform");
        assert_eq!(path.file_name().unwrap(), "workspaces.toml");
        assert!(path.is_absolute(), "{path:?} should be absolute");
        let components: Vec<String> = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect();
        assert!(
            components.iter().any(|c| c == "jot"),
            "path should live under a `jot` config directory: {path:?}"
        );
        assert!(
            components.iter().any(|c| c == "danjolabs"),
            "path should live under the `danjolabs` qualifier/organization: {path:?}"
        );
    }

    // ---------------------------------------------------------------- totality: load

    /// A registry file that has never been written yields an empty registry with no recovered
    /// signal — the normal, non-degraded "nothing registered yet" state. Catches a load path that
    /// treats "missing" the same as "corrupt".
    #[test]
    fn missing_file_is_an_empty_registry_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspaces.toml");
        assert!(!path.exists());

        let registry = Registry::load_from(&path).expect("load_from must be total");
        assert!(registry.is_empty());
        assert_eq!(registry.current(), None);
        assert!(registry.recovered().is_none());
    }

    /// `load_from` must be read-only: loading a registry that has never been saved must not create
    /// the file as a side effect. A load path that "helpfully" writes an empty file on first
    /// encounter would make every subsequent `Path::exists()` check on that file lie.
    #[test]
    fn missing_file_load_does_not_create_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspaces.toml");
        assert!(!path.exists());

        let _registry = Registry::load_from(&path).expect("load_from must be total");
        assert!(
            !path.exists(),
            "load_from must not create the file as a side effect"
        );
    }

    /// Malformed TOML must not propagate as `Err` — it must degrade to an empty registry carrying a
    /// recoverable signal. This is the acceptance property U5 exists to guarantee: a bad registry
    /// costs one re-add, never a crash.
    #[test]
    fn malformed_toml_recovers_to_an_empty_registry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspaces.toml");
        std::fs::write(&path, "this is not [valid toml").unwrap();

        let registry = Registry::load_from(&path).expect("load_from must be total");
        assert!(registry.is_empty());
        let err = registry
            .recovered()
            .expect("should carry the recovered error");
        assert!(matches!(err, Error::RegistryCorrupt { .. }));
        assert!(err.is_registry_recoverable());
    }

    /// A well-formed TOML file that is not a valid registry (a `last_opened` that fails RFC 3339)
    /// must degrade the same way as syntactically malformed TOML — "corrupt" covers both shapes.
    #[test]
    fn semantically_invalid_registry_recovers_to_an_empty_registry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspaces.toml");
        std::fs::write(
            &path,
            format!(
                "[[workspace]]\nid = \"{A}\"\npath = \"C:\\\\notes\"\nname = \"Thoughts\"\nlast_opened = \"not a timestamp\"\n"
            ),
        )
        .unwrap();

        let registry = Registry::load_from(&path).expect("load_from must be total");
        assert!(registry.is_empty());
        let err = registry
            .recovered()
            .expect("should carry the recovered error");
        assert!(matches!(err, Error::RegistryCorrupt { .. }));
    }

    /// Reading a directory as if it were the registry file is the "present but unreadable" shape —
    /// distinct from "missing" (`NotFound`). It must also recover rather than propagate.
    #[test]
    fn unreadable_path_recovers_to_an_empty_registry() {
        let dir = tempfile::tempdir().unwrap();
        // `dir.path()` exists and is a directory, not a file: reading it as a file fails with an
        // error other than `NotFound`.
        let registry = Registry::load_from(dir.path()).expect("load_from must be total");
        assert!(registry.is_empty());
        let err = registry
            .recovered()
            .expect("should carry the recovered error");
        assert!(matches!(err, Error::RegistryUnreadable { .. }));
        assert!(err.is_registry_recoverable());
    }

    // ---------------------------------------------------------------- save / load round trip

    /// The core contract: save, then load from the same path, and get back equivalent data —
    /// keying by id, the `current` key, and every field on the entry.
    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspaces.toml");
        let vault_dir = dir.path().join("vault");
        std::fs::create_dir_all(&vault_dir).unwrap();

        let mut registry = Registry::new();
        registry.upsert(Entry::new(
            uuid(A),
            vault_dir.clone(),
            "Thoughts",
            ts("2026-08-30T07:08:43Z"),
        ));
        registry.set_current(uuid(A));
        registry.save_to(&path).expect("save should succeed");

        let loaded = Registry::load_from(&path).expect("load_from must be total");
        assert!(loaded.recovered().is_none());
        assert_eq!(loaded.current(), Some(uuid(A)));
        assert_eq!(loaded.len(), 1);

        let entry = loaded.get(uuid(A)).expect("entry should round-trip");
        assert_eq!(entry.id(), uuid(A));
        assert_eq!(entry.path(), vault_dir.as_path());
        assert_eq!(entry.name(), "Thoughts");
        assert_eq!(entry.last_opened(), ts("2026-08-30T07:08:43Z"));
        assert!(!entry.is_stale(), "vault_dir exists on disk");
    }

    /// `save -> load -> save` must be a fixed point: re-saving a registry that was just loaded
    /// produces byte-identical output to the first save. This is what makes the registry safe to
    /// round-trip through repeatedly (every `Workspace::open`, in stage 4) without it drifting.
    #[test]
    fn save_load_save_is_a_fixed_point() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspaces.toml");
        let vault_a = dir.path().join("a");
        let vault_b = dir.path().join("b");
        std::fs::create_dir_all(&vault_a).unwrap();
        // vault_b is deliberately left stale, so the fixed point holds for stale entries too.

        let mut registry = Registry::new();
        registry.upsert(Entry::new(
            uuid(A),
            vault_a,
            "A",
            ts("2026-08-30T07:08:43Z"),
        ));
        registry.upsert(Entry::new(
            uuid(B),
            vault_b,
            "B",
            ts("2026-08-29T12:00:00Z"),
        ));
        registry.set_current(uuid(A));
        registry.save_to(&path).unwrap();
        let first_save = std::fs::read_to_string(&path).unwrap();

        let loaded = Registry::load_from(&path).unwrap();
        loaded.save_to(&path).unwrap();
        let second_save = std::fs::read_to_string(&path).unwrap();

        assert_eq!(
            first_save, second_save,
            "save -> load -> save must be a fixed point"
        );
    }

    /// A path with backslashes, as every Windows path has, must survive the round trip unchanged —
    /// the module deliberately avoids any forward-slash normalization for registry paths.
    #[test]
    fn windows_style_path_round_trips_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspaces.toml");
        let vault_dir = dir.path().join("my vault");
        std::fs::create_dir_all(&vault_dir).unwrap();

        let mut registry = Registry::new();
        registry.upsert(Entry::new(
            uuid(A),
            vault_dir.clone(),
            "Vault",
            ts("2026-08-30T00:00:00Z"),
        ));
        registry.save_to(&path).unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        // On Windows this must contain an escaped backslash somewhere in the path string; on any
        // platform, the byte content of the path must reappear verbatim once escaping is undone.
        assert!(contents.contains(vault_dir.file_name().unwrap().to_str().unwrap()));

        let loaded = Registry::load_from(&path).unwrap();
        assert_eq!(loaded.get(uuid(A)).unwrap().path(), vault_dir.as_path());
    }

    /// The trap the docs warn about: chrono's derived serde would emit subsecond precision. The
    /// saved file must contain a whole-second, `Z`-suffixed timestamp instead.
    #[test]
    fn saved_timestamp_has_no_subsecond_precision() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspaces.toml");

        let mut registry = Registry::new();
        // A timestamp that, if chrono's default serde emitter were used, would carry microseconds.
        let with_micros = DateTime::parse_from_rfc3339("2026-08-30T07:08:43.250180Z")
            .unwrap()
            .with_timezone(&Utc);
        registry.upsert(Entry::new(
            uuid(A),
            dir.path().to_path_buf(),
            "V",
            with_micros,
        ));
        registry.save_to(&path).unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        let line = contents
            .lines()
            .find(|l| l.starts_with("last_opened"))
            .expect("last_opened line");
        assert!(line.ends_with("Z\""), "{line}");
        assert!(!line.contains('.'), "subsecond precision leaked: {line}");
    }

    // ---------------------------------------------------------------- keying and mutation

    /// Entries are keyed by id, not path: upserting the same id with a different path updates in
    /// place rather than creating a second entry. This is the property that makes moving a vault a
    /// field update instead of an orphaned registration.
    #[test]
    fn upsert_by_id_replaces_not_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let old_path = dir.path().join("old");
        let new_path = dir.path().join("new");
        std::fs::create_dir_all(&old_path).unwrap();
        std::fs::create_dir_all(&new_path).unwrap();

        let mut registry = Registry::new();
        registry.upsert(Entry::new(
            uuid(A),
            old_path,
            "V",
            ts("2026-08-30T00:00:00Z"),
        ));
        assert_eq!(registry.len(), 1);

        registry.upsert(Entry::new(
            uuid(A),
            new_path.clone(),
            "V",
            ts("2026-08-30T01:00:00Z"),
        ));
        assert_eq!(registry.len(), 1, "same id must replace, not add");
        assert_eq!(registry.get(uuid(A)).unwrap().path(), new_path.as_path());
    }

    /// `remove` does not touch a `current` that named the removed id — a dangling `current` is a
    /// designed-for state (see the module docs), not something this module repairs.
    #[test]
    fn remove_leaves_a_dangling_current_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = Registry::new();
        registry.upsert(Entry::new(
            uuid(A),
            dir.path().to_path_buf(),
            "V",
            ts("2026-08-30T00:00:00Z"),
        ));
        registry.set_current(uuid(A));

        registry.remove(uuid(A));
        assert!(registry.get(uuid(A)).is_none());
        assert_eq!(
            registry.current(),
            Some(uuid(A)),
            "current is a dangling reference now, not silently cleared"
        );
    }

    // ---------------------------------------------------------------- staleness

    /// An entry whose path no longer exists is retained and reported stale, never dropped and never
    /// an error — the U5 stale-path behavior, exercised end to end through a real load.
    #[test]
    fn stale_entry_is_retained_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspaces.toml");
        let vanished = dir.path().join("gone");
        // Deliberately never created: `vanished` does not exist on disk.

        let mut registry = Registry::new();
        registry.upsert(Entry::new(
            uuid(B),
            vanished.clone(),
            "Gone",
            ts("2026-08-30T00:00:00Z"),
        ));
        registry.save_to(&path).unwrap();

        let loaded = Registry::load_from(&path).unwrap();
        assert_eq!(loaded.len(), 1, "stale entries are retained, not dropped");
        let entry = loaded.get(uuid(B)).unwrap();
        assert!(entry.is_stale());
        assert_eq!(entry.path(), vanished.as_path());

        let stale: Vec<_> = loaded.stale_entries().collect();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].id(), uuid(B));
    }

    /// The counterpart: an entry whose path exists is never reported stale.
    #[test]
    fn existing_path_is_not_stale() {
        let dir = tempfile::tempdir().unwrap();
        let entry = Entry::new(
            uuid(A),
            dir.path().to_path_buf(),
            "V",
            ts("2026-08-30T00:00:00Z"),
        );
        assert!(!entry.is_stale());
    }

    /// `set_path` recomputes staleness rather than leaving a now-wrong flag behind.
    #[test]
    fn set_path_recomputes_staleness() {
        let dir = tempfile::tempdir().unwrap();
        let mut entry = Entry::new(
            uuid(A),
            dir.path().join("nope"),
            "V",
            ts("2026-08-30T00:00:00Z"),
        );
        assert!(entry.is_stale());

        entry.set_path(dir.path().to_path_buf());
        assert!(!entry.is_stale());
    }

    // ---------------------------------------------------------------- no test touches the real OS
    // config directory: enforced by construction above (every test builds its own tempdir path and
    // passes it explicitly to `load_from` / `save_to`), and `default_path_ends_in_workspaces_toml`
    // never opens the path it resolves.
}
