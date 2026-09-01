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
//! # Forward compatibility
//!
//! Keys this build does not model are **preserved through a load → save cycle**, both top-level and
//! per `[[workspace]]` table. `workspace.toml` gets this for free because `Workspace::open` never
//! writes; the registry does write, so it has to do the work.
//!
//! Without it: a newer jot writes `workspaces.toml`, an older build runs one command that saves,
//! and the newer settings are gone with no signal. The registry is a cache, so the cost is bounded
//! — but it is the same class of silent loss the note format goes to great lengths to avoid, and
//! the note format's rule ("unknown frontmatter keys are preserved verbatim on every write") reads
//! the same way here.
//!
//! They are re-emitted *after* the keys this build owns, mirroring the note format's rule for
//! frontmatter, and among themselves in sorted order rather than the file's original order —
//! `toml::Table` is a sorted map, so document order is already gone by the time the parser hands
//! the file over. Sorted is at least stable, which is what makes a save → load → save a fixed
//! point.
//!
//! Three things this deliberately does *not* do. It does not let an unknown key shadow a known one
//! — [`Registry::save_to`] always emits this build's own value. It does not keep the unknown keys
//! of an entry that was [`Registry::remove`]d, since they describe a workspace that is no longer
//! registered. And it does not survive a file that failed to load: a corrupt registry recovers to
//! an *empty* one (see "Totality"), and saving that empty registry overwrites the unreadable file,
//! unknown keys and all. Preserving keys out of a file that could not be parsed is not possible,
//! and pretending otherwise would be worse than saying so here.
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

    /// Stamps `last_opened`, as `Workspace::open` would on a successful open (stage 3 wires the
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
    /// Top-level keys the loaded file carried that this build does not know, values verbatim. See
    /// "Forward compatibility" in the module docs, including why the file's key order is not one
    /// of the things preserved.
    unknown: toml::Table,
    /// The same, per `[[workspace]]` table, keyed by the entry's id.
    unknown_entries: IndexMap<Uuid, toml::Table>,
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

        // The same text a second time, as a plain table: it still holds the keys `RegistryFile`
        // threw away, which is what makes forward compatibility possible.
        //
        // Deserializing `RegistryFile` *out of* that table instead would be one parse rather than
        // two, and is wrong: `toml::Value`'s deserializer renders a native TOML datetime as a
        // string, so `last_opened = 2026-08-30T07:08:43Z` (unquoted, which is what a hand-editor
        // writes) would quietly start loading instead of recovering as corrupt. The strictness of
        // the load path is not this change's to move. Failing here is unreachable — the document
        // just deserialized — and costs only the unknown keys if it ever happens.
        let raw: toml::Table = toml::from_str(&text).unwrap_or_default();

        match Registry::from_file(file, &raw) {
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
        let serialize_err = |source: toml::ser::Error| Error::RegistrySerialize {
            path: path.to_path_buf(),
            message: source.to_string(),
        };
        let toml_text = self.to_toml_string().map_err(serialize_err)?;

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
        // Unregistering the workspace takes its forward-compatibility keys with it: they describe
        // an entry that no longer exists, and keeping them would resurrect them on a re-add.
        self.unknown_entries.shift_remove(&id);
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
            unknown: toml::Table::new(),
            unknown_entries: IndexMap::new(),
        }
    }

    /// Converts a deserialized [`RegistryFile`] into a validated, typed [`Registry`], folding any
    /// semantic problem (an id that somehow isn't a UUID would already have failed TOML
    /// deserialization; a `last_opened` that isn't RFC 3339 has not) into a single `String` reason so
    /// the caller can report it as [`Error::RegistryCorrupt`] alongside genuine parse failures —
    /// "corrupt" covers both shapes per the totality guarantee.
    ///
    /// `table` is the same file as a plain TOML table, from which the keys `RegistryFile` does not
    /// model are lifted out and kept for the next save.
    fn from_file(file: RegistryFile, table: &toml::Table) -> std::result::Result<Registry, String> {
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
            unknown: unknown_keys(table, &TOP_LEVEL_KEYS),
            unknown_entries: unknown_entry_keys(table),
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

    /// The exact text `save_to` writes: the known keys in their fixed order, then whatever unknown
    /// keys were retained from the loaded file.
    ///
    /// Emitted through [`RegistryOut`] rather than by merging into a `toml::Table`, because
    /// `toml::Table` is sorted: routing the document through one would re-alphabetize
    /// `id, path, name, last_opened` on every save and scatter the unknown keys among them.
    fn to_toml_string(&self) -> std::result::Result<String, toml::ser::Error> {
        let file = self.to_file();
        toml::to_string_pretty(&RegistryOut {
            file: &file,
            unknown: &self.unknown,
            unknown_entries: &self.unknown_entries,
        })
    }
}

/// The registry as it is written out: [`RegistryFile`]'s keys, in their declared order, followed by
/// the unknown keys this build is only carrying.
///
/// Known keys are emitted first and unknown keys are skipped if they collide, so a retained key can
/// only ever *add* to the document, never shadow a value this build is responsible for. Ordering
/// mirrors the note format's rule for frontmatter — known keys in a fixed order, then the rest — so
/// the two files this project writes read the same way.
struct RegistryOut<'a> {
    file: &'a RegistryFile,
    unknown: &'a toml::Table,
    unknown_entries: &'a IndexMap<Uuid, toml::Table>,
}

impl serde::Serialize for RegistryOut<'_> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;

        let mut map = serializer.serialize_map(None)?;
        if let Some(current) = &self.file.current {
            map.serialize_entry("current", current)?;
        }
        for (key, value) in self.unknown {
            if !TOP_LEVEL_KEYS.contains(&key.as_str()) {
                map.serialize_entry(key, value)?;
            }
        }
        if !self.file.workspaces.is_empty() {
            let workspaces: Vec<EntryOut<'_>> = self
                .file
                .workspaces
                .iter()
                .map(|file| EntryOut {
                    // Looked up by id rather than by position, so one workspace's retained keys
                    // can never be attached to another.
                    unknown: self.unknown_entries.get(&file.id),
                    file,
                })
                .collect();
            map.serialize_entry("workspace", &workspaces)?;
        }
        map.end()
    }
}

/// One `[[workspace]]` table, written the same way: known keys, then retained unknown ones.
struct EntryOut<'a> {
    file: &'a EntryFile,
    unknown: Option<&'a toml::Table>,
}

impl serde::Serialize for EntryOut<'_> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;

        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("id", &self.file.id)?;
        map.serialize_entry("path", &self.file.path)?;
        map.serialize_entry("name", &self.file.name)?;
        map.serialize_entry("last_opened", &self.file.last_opened)?;
        for (key, value) in self.unknown.into_iter().flatten() {
            if !ENTRY_KEYS.contains(&key.as_str()) {
                map.serialize_entry(key, value)?;
            }
        }
        map.end()
    }
}

/// The top-level keys `RegistryFile` models. Anything else in the file belongs to a version of jot
/// that is not this one.
const TOP_LEVEL_KEYS: [&str; 2] = ["current", "workspace"];

/// The keys one `[[workspace]]` table models.
const ENTRY_KEYS: [&str; 4] = ["id", "path", "name", "last_opened"];

/// Every key of `table` that is not in `known`, cloned. Sorted, because `toml::Table` is: see the
/// module docs on why the file's original key order is not recoverable here.
fn unknown_keys(table: &toml::Table, known: &[&str]) -> toml::Table {
    table
        .iter()
        .filter(|(key, _)| !known.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// The unknown keys of every `[[workspace]]` table, keyed by the entry's id.
///
/// Entries whose `id` is unreadable are skipped: without an id there is nothing to attach the keys
/// to on the way out, and such an entry did not survive [`Registry::from_file`] either. Duplicate
/// ids collapse to the last, matching the entry map itself.
fn unknown_entry_keys(table: &toml::Table) -> IndexMap<Uuid, toml::Table> {
    let mut out = IndexMap::new();
    let Some(toml::Value::Array(workspaces)) = table.get("workspace") else {
        return out;
    };
    for item in workspaces {
        let Some(entry) = item.as_table() else {
            continue;
        };
        let Some(id) = entry
            .get("id")
            .and_then(toml::Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok())
        else {
            continue;
        };
        let unknown = unknown_keys(entry, &ENTRY_KEYS);
        if !unknown.is_empty() {
            out.insert(id, unknown);
        }
    }
    out
}

/// The on-disk shape of `workspaces.toml`. Private: callers only ever see [`Registry`] and
/// [`Entry`], which carry typed, validated data.
/// Deserialize only: the document is written through [`RegistryOut`], which knows about the
/// unknown keys this shape by definition does not.
#[derive(Debug, Default, serde::Deserialize)]
struct RegistryFile {
    #[serde(default)]
    current: Option<Uuid>,
    #[serde(default, rename = "workspace")]
    workspaces: Vec<EntryFile>,
}

/// The on-disk shape of one `[[workspace]]` table. `last_opened` is a `String`, not a
/// `DateTime<Utc>` — see the module docs' "the timestamp trap" section for why.
#[derive(Debug, Clone, serde::Deserialize)]
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
        // The organization component is a *platform convention*, not a property of the triple:
        // `directories` puts it in the path on Windows (`%APPDATA%\danjolabs\jot\config`) and
        // macOS, and XDG deliberately leaves it out, so on Linux the answer is `~/.config/jot`.
        // Asserting it unconditionally asserted a Windows implementation detail — found by the
        // first run of this suite on Linux, which is the only platform that could have found it.
        if cfg!(any(target_os = "windows", target_os = "macos")) {
            assert!(
                components.iter().any(|c| c == "danjolabs"),
                "path should live under the `danjolabs` qualifier/organization: {path:?}"
            );
        }
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
    /// round-trip through repeatedly (every `Workspace::open`, in stage 3) without it drifting.
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

    // ---------------------------------------------------------------- forward compatibility

    /// A newer jot writes a key this build has never heard of; this build runs one command that
    /// saves. Without preservation the newer settings are gone, silently, with the user's only
    /// clue being that their newer install has forgotten something.
    #[test]
    fn unknown_keys_survive_a_load_save_cycle_both_top_level_and_per_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspaces.toml");
        std::fs::write(
            &path,
            format!(
                "current = \"{A}\"\n\
                 theme = \"dark\"\n\n\
                 [[workspace]]\n\
                 id = \"{A}\"\n\
                 path = 'one'\n\
                 name = \"One\"\n\
                 last_opened = \"2026-08-30T07:08:43Z\"\n\
                 color = \"red\"\n\
                 pinned = true\n"
            ),
        )
        .unwrap();

        let registry = Registry::load_from(&path).expect("load is total");
        assert!(
            registry.recovered().is_none(),
            "an unknown key is not corruption"
        );
        registry.save_to(&path).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("theme = \"dark\""), "{after}");
        assert!(after.contains("color = \"red\""), "{after}");
        assert!(after.contains("pinned = true"), "{after}");
        // And the keys this build does own are still exactly right.
        assert!(after.contains(&format!("current = \"{A}\"")), "{after}");
        assert!(
            after.contains("last_opened = \"2026-08-30T07:08:43Z\""),
            "{after}"
        );

        let reloaded = Registry::load_from(&path).unwrap();
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded.get(uuid(A)).unwrap().name(), "One");
    }

    /// Preservation must survive the mutations a real session performs, not just an untouched
    /// load-save. A `jot use` stamps `last_opened` and rewrites the file.
    #[test]
    fn unknown_keys_survive_a_mutation_of_the_entry_that_carries_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspaces.toml");
        std::fs::write(
            &path,
            format!(
                "[[workspace]]\nid = \"{A}\"\npath = 'one'\nname = \"One\"\nlast_opened = \"2026-08-30T07:08:43Z\"\ncolor = \"red\"\n"
            ),
        )
        .unwrap();

        let mut registry = Registry::load_from(&path).unwrap();
        let mut entry = registry.get(uuid(A)).unwrap().clone();
        entry.touch(ts("2026-09-01T00:00:00Z"));
        entry.set_name("Renamed");
        registry.upsert(entry);
        registry.set_current(uuid(A));
        registry.save_to(&path).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("color = \"red\""), "{after}");
        assert!(after.contains("name = \"Renamed\""), "{after}");
        assert!(
            after.contains("last_opened = \"2026-09-01T00:00:00Z\""),
            "{after}"
        );
    }

    /// TOML types that a `#[serde(flatten)]`-based implementation mangles — a bare datetime, a
    /// nested table, an array — must come back the way they went in. This is why the load path
    /// parses to a `toml::Table` first instead of flattening into the struct.
    #[test]
    fn unknown_keys_of_every_toml_type_round_trip_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspaces.toml");
        std::fs::write(
            &path,
            format!(
                "last_synced = 2026-08-30T07:08:43Z\n\
                 recents = [\"a\", \"b\"]\n\n\
                 [[workspace]]\n\
                 id = \"{A}\"\n\
                 path = 'one'\n\
                 name = \"One\"\n\
                 last_opened = \"2026-08-30T07:08:43Z\"\n\
                 depth = 3\n\n\
                 [ui]\n\
                 density = \"compact\"\n"
            ),
        )
        .unwrap();

        let before: toml::Table = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        Registry::load_from(&path).unwrap().save_to(&path).unwrap();
        let after: toml::Table = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

        for key in ["last_synced", "recents", "ui"] {
            assert_eq!(after.get(key), before.get(key), "top-level `{key}` changed");
        }
        let entry_of = |t: &toml::Table| t["workspace"].as_array().unwrap()[0].clone();
        assert_eq!(
            entry_of(&after)["depth"],
            entry_of(&before)["depth"],
            "per-entry `depth` changed"
        );
    }

    /// Save -> load -> save is still a fixed point once unknown keys are in play, so a registry
    /// carrying them does not churn its own file on every command.
    #[test]
    fn a_registry_with_unknown_keys_is_still_a_save_load_save_fixed_point() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspaces.toml");
        std::fs::write(
            &path,
            format!(
                "theme = \"dark\"\n\n[[workspace]]\nid = \"{A}\"\npath = 'one'\nname = \"One\"\nlast_opened = \"2026-08-30T07:08:43Z\"\ncolor = \"red\"\n"
            ),
        )
        .unwrap();

        Registry::load_from(&path).unwrap().save_to(&path).unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        Registry::load_from(&path).unwrap().save_to(&path).unwrap();
        let second = std::fs::read_to_string(&path).unwrap();
        assert_eq!(first, second, "unknown keys must not churn the file");
    }

    /// Unregistering a workspace takes its unknown keys with it. The alternative is a registry
    /// that quietly accumulates settings for vaults the user removed, and resurrects them if the
    /// same id is ever re-added.
    #[test]
    fn removing_an_entry_drops_the_unknown_keys_that_belonged_to_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspaces.toml");
        std::fs::write(
            &path,
            format!(
                "[[workspace]]\nid = \"{A}\"\npath = 'one'\nname = \"One\"\nlast_opened = \"2026-08-30T07:08:43Z\"\ncolor = \"red\"\n"
            ),
        )
        .unwrap();

        let mut registry = Registry::load_from(&path).unwrap();
        assert!(registry.remove(uuid(A)).is_some());
        registry.upsert(Entry::new(
            uuid(A),
            dir.path().join("two"),
            "Two",
            ts("2026-09-01T00:00:00Z"),
        ));
        registry.save_to(&path).unwrap();

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(!after.contains("color"), "{after}");
    }

    /// Preserving unknown keys must not loosen the load path. The obvious one-parse implementation
    /// — deserialize `RegistryFile` out of the `toml::Table` instead of out of the text — silently
    /// does: `toml::Value`'s deserializer renders a native TOML datetime as a string, so an
    /// unquoted `last_opened` would start loading instead of recovering as corrupt. That is a
    /// strictness decision belonging to U5, not to forward compatibility.
    #[test]
    fn an_unquoted_last_opened_still_recovers_rather_than_loading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspaces.toml");
        std::fs::write(
            &path,
            format!(
                "[[workspace]]\nid = \"{A}\"\npath = 'one'\nname = \"One\"\nlast_opened = 2026-08-30T07:08:43Z\n"
            ),
        )
        .unwrap();

        let registry = Registry::load_from(&path).expect("load is total");
        assert!(registry.is_empty(), "a bare TOML datetime is not a string");
        let err = registry.recovered().expect("and it must say why");
        assert!(matches!(err, Error::RegistryCorrupt { .. }), "{err:?}");
    }

    /// The emitted document, pinned literally — the module docs' worked example, byte for byte,
    /// plus what an unknown key does to it.
    ///
    /// `workspaces.toml` is a file users open and edit. Routing the document through a
    /// `toml::Table` to merge the unknown keys in would still round-trip, would still be a fixed
    /// point, and would still pass every other test here, while re-alphabetizing every entry to
    /// `id, last_opened, name, path` and scattering the retained keys among the known ones. This
    /// is the only place that notices.
    #[test]
    fn the_emitted_registry_looks_exactly_like_the_documented_shape() {
        let mut registry = Registry::new();
        registry.upsert(Entry::new(
            uuid(A),
            PathBuf::from(r"C:\Users\jun\notes"),
            "Thoughts",
            ts("2026-08-30T07:08:43Z"),
        ));
        registry.upsert(Entry::new(
            uuid(B),
            PathBuf::from(r"C:\Users\jun\work-notes"),
            "Work",
            ts("2026-08-29T12:00:00Z"),
        ));
        registry.set_current(uuid(A));

        assert_eq!(
            registry.to_toml_string().unwrap(),
            "\
current = \"01a03d20-a54c-7977-a1f4-1a88b38855dd\"

[[workspace]]
id = \"01a03d20-a54c-7977-a1f4-1a88b38855dd\"
path = 'C:\\Users\\jun\\notes'
name = \"Thoughts\"
last_opened = \"2026-08-30T07:08:43Z\"

[[workspace]]
id = \"01a03d30-2b6b-7c22-9def-abcdef123456\"
path = 'C:\\Users\\jun\\work-notes'
name = \"Work\"
last_opened = \"2026-08-29T12:00:00Z\"
"
        );

        // And with retained keys: known keys keep their order, the rest follow.
        registry
            .unknown
            .insert("theme".into(), toml::Value::String("dark".into()));
        registry.unknown_entries.insert(uuid(A), {
            let mut unknown = toml::Table::new();
            unknown.insert("color".into(), toml::Value::String("red".into()));
            unknown
        });

        let text = registry.to_toml_string().unwrap();
        assert!(
            text.starts_with(
                "current = \"01a03d20-a54c-7977-a1f4-1a88b38855dd\"\ntheme = \"dark\"\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("last_opened = \"2026-08-30T07:08:43Z\"\ncolor = \"red\"\n"),
            "a retained entry key follows the four known ones:\n{text}"
        );
        assert!(
            !text.contains("color = \"red\"\n\nlast_opened = \"2026-08-29T12:00:00Z\""),
            "and lands on the entry it came from, not the next one:\n{text}"
        );
    }

    /// A retained unknown key must never shadow a key this build owns. Reachable only if a future
    /// version of this module forgets to list one of its own keys as known, which is exactly the
    /// mistake that would otherwise silently pin `current` to whatever the file said last.
    #[test]
    fn a_known_key_always_wins_over_a_retained_unknown_one() {
        let mut registry = Registry::new();
        registry.set_current(uuid(A));
        registry
            .unknown
            .insert("current".into(), toml::Value::String(B.into()));

        let text = registry.to_toml_string().unwrap();
        assert!(text.contains(&format!("current = \"{A}\"")), "{text}");
        assert!(
            !text.contains(B),
            "the unknown key must not shadow it: {text}"
        );
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
