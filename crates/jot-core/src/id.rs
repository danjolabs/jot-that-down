//! Note identity: the opaque [`NoteId`], the [`IdFormat`] `workspace.toml` declares, and the rule
//! that decides whether a `*.md` file is a note or a stray.
//!
//! A note is a file named **exactly** `<id>.md`, lowercase, in the declared format. Anything else
//! ending in `.md` — `README.md`, `<id> - Copy.md`, an uppercased id — is a stray: reported, never
//! indexed as a note, never renamed. Because the id *is* the filename, the filesystem guarantees it
//! is unique.
//!
//! **The id carries no meaning.** A UUIDv7 happens to embed a timestamp; nothing reads it.
//! Creation time is the `document:created_at` role's business.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::fs::NOTE_EXTENSION;

/// A note's identity: the file stem of `<id>.md`.
///
/// Opaque. It can be compared, sorted, printed and handed back; it cannot be decoded. Only
/// [`IdFormat::check`] and [`IdFormat::mint`] make one, so every `NoteId` in a workspace is in
/// that workspace's format.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NoteId(String);

impl NoteId {
    /// The id as text — exactly the file stem.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `<id>.md`.
    #[must_use]
    pub fn file_name(&self) -> String {
        format!("{}{NOTE_EXTENSION}", self.0)
    }

    /// An id built from text the caller has already validated (the index, which only ever stored
    /// checked ids).
    pub(crate) fn trusted(text: impl Into<String>) -> NoteId {
        NoteId(text.into())
    }
}

impl fmt::Display for NoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for NoteId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// `[id] format` in `workspace.toml`. Only `uuidv7` is accepted for now; `uuidv4` and snowflake
/// ids are expected later, which is why this is an enum rather than a constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum IdFormat {
    /// A UUID, version 7, hyphenated, lowercase.
    #[default]
    UuidV7,
}

impl IdFormat {
    /// The spelling in `workspace.toml`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            IdFormat::UuidV7 => "uuidv7",
        }
    }

    /// Parses the `workspace.toml` spelling.
    #[must_use]
    pub fn from_name(name: &str) -> Option<IdFormat> {
        match name {
            "uuidv7" => Some(IdFormat::UuidV7),
            _ => None,
        }
    }

    /// A fresh id in this format.
    #[must_use]
    pub fn mint(self) -> NoteId {
        match self {
            IdFormat::UuidV7 => NoteId(Uuid::now_v7().hyphenated().to_string()),
        }
    }

    /// `text` as an id of this format, or why it is not one.
    ///
    /// # Errors
    ///
    /// A short human reason, used as a stray's `reason`.
    pub fn check(self, text: &str) -> Result<NoteId, String> {
        match self {
            IdFormat::UuidV7 => {
                let Ok(uuid) = Uuid::try_parse(text) else {
                    return Err(String::from("the name is not a uuidv7 id"));
                };
                // `try_parse` accepts braced, URN and simple forms; only one spelling may name a
                // note, or one id could live in two files.
                if text.len() != 36 {
                    return Err(String::from("the id is not in hyphenated form"));
                }
                if text.bytes().any(|b| b.is_ascii_uppercase()) {
                    return Err(String::from("the id is not lowercase"));
                }
                if uuid.get_version_num() != 7 {
                    return Err(format!(
                        "the id is a version {} uuid, not a uuidv7",
                        uuid.get_version_num()
                    ));
                }
                Ok(NoteId(text.to_string()))
            }
        }
    }

    /// Whether `file_name` names a note — exactly `<id>.md` — and which.
    ///
    /// # Errors
    ///
    /// The stray reason.
    pub fn classify(self, file_name: &str) -> Result<NoteId, String> {
        let Some(stem) = file_name.strip_suffix(NOTE_EXTENSION) else {
            return Err(String::from("the extension is not lowercase `.md`"));
        };
        self.check(stem)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_lowercase_name_is_a_note() {
        let f = IdFormat::UuidV7;
        let id = "01a03d4c-c708-7cbf-83c0-883cedb7f1d5";
        assert_eq!(f.classify(&format!("{id}.md")).unwrap().as_str(), id);
        for stray in [
            "README.md".to_string(),
            format!("{id} - Copy.md"),
            format!("{}.md", id.to_uppercase()),
            format!("{id}_slug.md"),
            format!("{id}.MD"),
            // A v4 uuid in a uuidv7 workspace.
            "9b2f4c1e-3d5a-4e6f-8a7b-0c1d2e3f4a5b.md".to_string(),
        ] {
            assert!(f.classify(&stray).is_err(), "{stray} must be a stray");
        }
    }
}
