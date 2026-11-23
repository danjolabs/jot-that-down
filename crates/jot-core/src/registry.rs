//! The workspace registry: known workspaces and the current one, in `workspaces.toml` in the OS
//! config directory.
//!
//! ```toml
//! current = "D:/notes"
//!
//! [[workspace]]
//! path = "D:/notes"
//! name = "notes"
//! ```
//!
//! # Keyed by canonical path
//!
//! A workspace carries no id of its own any more, so the registry keys by **canonical absolute
//! path**: `.` and `..` and symlinks resolved, compared case-insensitively on Windows. `name` is
//! optional and defaults to the folder name. A moved folder leaves a stale entry, reported as
//! missing and never removed automatically.
//!
//! # Totality
//!
//! [`Registry::load_from`] never fails: a missing file is an empty registry, and an unreadable or
//! corrupt one is an empty registry plus [`Registry::recovered`]. The registry is a cache; a bad
//! one costs a re-add, never data. [`Registry::save_to`] does fail, because a save that silently
//! does nothing loses the user's action.
//!
//! # Forward compatibility
//!
//! Keys this build does not model are kept through load → save, top-level and per entry, and
//! written after the known ones.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::error::{Error, Result};

/// The registry's default location: `workspaces.toml` in the OS config directory for
/// `("", "danjolabs", "jot")`. The only call into `directories` in this crate.
///
/// # Errors
///
/// [`Error::ConfigDirUnavailable`] when the platform has no config directory.
pub fn default_path() -> Result<PathBuf> {
    directories::ProjectDirs::from("", "danjolabs", "jot")
        .map(|dirs| dirs.config_dir().join("workspaces.toml"))
        .ok_or_else(|| Error::ConfigDirUnavailable {
            application: "danjolabs/jot".into(),
        })
}

/// `path`, canonical: absolute, resolved through symlinks when it exists, lexically normal when it
/// does not. On Windows the `\\?\` prefix `canonicalize` adds is removed, so the path reads as the
/// user would write it.
#[must_use]
pub fn canonical(path: &Path) -> PathBuf {
    let absolute = crate::workspace::absolutize(path);
    match std::fs::canonicalize(&absolute) {
        Ok(resolved) => strip_verbatim(resolved),
        Err(_) => absolute,
    }
}

#[cfg(windows)]
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => path,
    }
}

#[cfg(not(windows))]
fn strip_verbatim(path: PathBuf) -> PathBuf {
    path
}

/// The comparison key for a canonical path: case-folded on Windows, where the filesystem is
/// case-insensitive.
fn key_of(path: &Path) -> String {
    let text = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

/// One known workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    path: PathBuf,
    name: Option<String>,
    stale: bool,
}

impl Entry {
    /// An entry for the workspace at `path`, canonicalised. `stale` is computed now.
    #[must_use]
    pub fn new(path: &Path, name: Option<String>) -> Entry {
        let path = canonical(path);
        Entry {
            stale: !path.is_dir(),
            path,
            name: name.filter(|n| !n.trim().is_empty()),
        }
    }

    /// The canonical path — the entry's key.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The display name: the explicit one, or the folder name.
    #[must_use]
    pub fn name(&self) -> String {
        self.name.clone().unwrap_or_else(|| {
            self.path.file_name().map_or_else(
                || self.path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        })
    }

    /// The name, only if one was set explicitly.
    #[must_use]
    pub fn explicit_name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Sets or clears the explicit name.
    pub fn set_name(&mut self, name: Option<String>) {
        self.name = name.filter(|n| !n.trim().is_empty());
    }

    /// Whether the folder was missing when this entry was loaded or made.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        self.stale
    }
}

/// The registry.
#[derive(Debug, Default)]
pub struct Registry {
    current: Option<PathBuf>,
    entries: IndexMap<String, Entry>,
    recovered: Option<Error>,
    unknown: toml::Table,
    unknown_entries: IndexMap<String, toml::Table>,
}

const TOP_LEVEL_KEYS: [&str; 2] = ["current", "workspace"];
const ENTRY_KEYS: [&str; 2] = ["path", "name"];

impl Registry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Loads the registry at `path`. Total; see the module docs.
    ///
    /// # Errors
    ///
    /// Never; the `Result` is kept so a caller's `?` reads naturally.
    pub fn load_from(path: &Path) -> Result<Registry> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Registry::new()),
            Err(e) => {
                return Ok(Registry::recovered_empty(Error::RegistryUnreadable {
                    path: path.to_path_buf(),
                    message: e.to_string(),
                }));
            }
        };
        match Registry::parse(&text) {
            Ok(registry) => Ok(registry),
            Err(message) => Ok(Registry::recovered_empty(Error::RegistryCorrupt {
                path: path.to_path_buf(),
                message,
            })),
        }
    }

    fn recovered_empty(err: Error) -> Registry {
        debug_assert!(err.is_registry_recoverable());
        Registry {
            recovered: Some(err),
            ..Registry::default()
        }
    }

    fn parse(text: &str) -> std::result::Result<Registry, String> {
        let table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut registry = Registry::new();
        if let Some(current) = table.get("current") {
            let current = current
                .as_str()
                .ok_or_else(|| String::from("`current` must be a path string"))?;
            registry.current = Some(canonical(Path::new(current)));
        }
        if let Some(workspaces) = table.get("workspace") {
            let workspaces = workspaces
                .as_array()
                .ok_or_else(|| String::from("`workspace` must be an array of tables"))?;
            for item in workspaces {
                let item = item
                    .as_table()
                    .ok_or_else(|| String::from("each `[[workspace]]` must be a table"))?;
                let path = item
                    .get("path")
                    .and_then(toml::Value::as_str)
                    .ok_or_else(|| String::from("a `[[workspace]]` has no `path` string"))?;
                let name = match item.get("name") {
                    None => None,
                    Some(v) => Some(
                        v.as_str()
                            .ok_or_else(|| format!("workspace `{path}`: `name` must be a string"))?
                            .to_string(),
                    ),
                };
                let entry = Entry::new(Path::new(path), name);
                let key = key_of(entry.path());
                let unknown: toml::Table = item
                    .iter()
                    .filter(|(k, _)| !ENTRY_KEYS.contains(&k.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                if !unknown.is_empty() {
                    registry.unknown_entries.insert(key.clone(), unknown);
                }
                registry.entries.insert(key, entry);
            }
        }
        registry.unknown = table
            .iter()
            .filter(|(k, _)| !TOP_LEVEL_KEYS.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Ok(registry)
    }

    /// Saves to `path`, atomically, staging beside it.
    ///
    /// # Errors
    ///
    /// [`Error::RegistrySerialize`], [`Error::CreateDir`], and the atomic write's errors.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let mut doc = toml::Table::new();
        if let Some(current) = &self.current {
            doc.insert(
                String::from("current"),
                toml::Value::String(current.to_string_lossy().into_owned()),
            );
        }
        for (k, v) in &self.unknown {
            doc.entry(k.clone()).or_insert_with(|| v.clone());
        }
        if !self.entries.is_empty() {
            let items = self
                .entries
                .iter()
                .map(|(key, entry)| {
                    let mut t = toml::Table::new();
                    t.insert(
                        String::from("path"),
                        toml::Value::String(entry.path.to_string_lossy().into_owned()),
                    );
                    if let Some(name) = &entry.name {
                        t.insert(String::from("name"), toml::Value::String(name.clone()));
                    }
                    for (k, v) in self.unknown_entries.get(key).into_iter().flatten() {
                        t.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                    toml::Value::Table(t)
                })
                .collect();
            doc.insert(String::from("workspace"), toml::Value::Array(items));
        }
        let text = toml::to_string_pretty(&doc).map_err(|e| Error::RegistrySerialize {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(dir).map_err(|source| Error::CreateDir {
            path: dir.to_path_buf(),
            source,
        })?;
        crate::fs::atomic_write(path, dir, text.as_bytes())
    }

    /// The current workspace's path. Not validated against the entries: a dangling `current` is
    /// a designed-for state.
    #[must_use]
    pub fn current(&self) -> Option<&Path> {
        self.current.as_deref()
    }

    /// Sets the current workspace.
    pub fn set_current(&mut self, path: &Path) {
        self.current = Some(canonical(path));
    }

    /// Clears the current workspace.
    pub fn clear_current(&mut self) {
        self.current = None;
    }

    /// The entry for the workspace at `path`.
    #[must_use]
    pub fn get(&self, path: &Path) -> Option<&Entry> {
        self.entries.get(&key_of(&canonical(path)))
    }

    /// The entry for `path`, mutably.
    pub fn get_mut(&mut self, path: &Path) -> Option<&mut Entry> {
        self.entries.get_mut(&key_of(&canonical(path)))
    }

    /// Entries whose display name is `name`.
    #[must_use]
    pub fn named(&self, name: &str) -> Vec<&Entry> {
        self.entries.values().filter(|e| e.name() == name).collect()
    }

    /// Adds `entry`, or replaces the entry for the same path.
    pub fn upsert(&mut self, entry: Entry) {
        self.entries.insert(key_of(entry.path()), entry);
    }

    /// Removes the entry for `path`. `current` is left alone.
    pub fn remove(&mut self, path: &Path) -> Option<Entry> {
        let key = key_of(&canonical(path));
        self.unknown_entries.shift_remove(&key);
        self.entries.shift_remove(&key)
    }

    /// Whether `path` is registered.
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        self.get(path).is_some()
    }

    /// How many workspaces are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether none are.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every entry, in insertion order.
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }

    /// Entries whose folder is missing.
    pub fn stale_entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values().filter(|e| e.stale)
    }

    /// `Some` when this registry is the empty result of a file that could not be read.
    #[must_use]
    pub fn recovered(&self) -> Option<&Error> {
        self.recovered.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_folder_spelled_two_ways_is_one_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("notes");
        std::fs::create_dir(&ws).unwrap();
        let mut registry = Registry::new();
        registry.upsert(Entry::new(&ws, None));
        registry.upsert(Entry::new(&ws.join("..").join("notes"), Some("n".into())));
        #[cfg(windows)]
        registry.upsert(Entry::new(
            Path::new(&ws.to_string_lossy().to_uppercase()),
            Some("n".into()),
        ));
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.entries().next().unwrap().name(), "n");
        assert!(!registry.entries().next().unwrap().is_stale());
    }

    #[test]
    fn save_and_load_round_trip_keeping_unknown_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("workspaces.toml");
        let ws = tmp.path().join("notes");
        std::fs::create_dir(&ws).unwrap();
        std::fs::write(
            &file,
            format!(
                "future = 1\ncurrent = '{0}'\n\n[[workspace]]\npath = '{0}'\ncolour = 'red'\n",
                ws.display()
            ),
        )
        .unwrap();
        let registry = Registry::load_from(&file).unwrap();
        assert!(registry.recovered().is_none());
        assert_eq!(
            registry.get(&ws).unwrap().name(),
            "notes",
            "name defaults to the folder"
        );
        registry.save_to(&file).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("future") && text.contains("colour"), "{text}");
        assert!(
            !text.contains("name"),
            "an implicit name is not written: {text}"
        );
    }

    #[test]
    fn a_corrupt_registry_recovers_to_empty_and_a_missing_folder_is_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("workspaces.toml");
        std::fs::write(&file, "this is = = not toml").unwrap();
        let registry = Registry::load_from(&file).unwrap();
        assert!(registry.is_empty() && registry.recovered().is_some());

        let entry = Entry::new(&tmp.path().join("moved-away"), None);
        assert!(entry.is_stale());
    }
}
