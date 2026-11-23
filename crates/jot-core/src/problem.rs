//! What is wrong with a note: `(key, rule, message)`.
//!
//! Reads validate per field and never auto-fix. A problem is reported, the note stays listed,
//! readable and editable. Two families share the one type:
//!
//! - **Schema problems** — `rule` is the JSON Schema keyword that failed (`type`, `required`,
//!   `format`, `enum`, `additionalProperties`, …). Derived from `notes.fields` and the schema, so
//!   they are recomputed without reading a file when `schema.json` changes.
//! - **Conversion problems** — `rule` is one of jot's own names below. They come from the file's
//!   bytes (YAML jot will not coerce into JSON), so they are carried with the note. A key with a
//!   conversion problem is **absent from `fields`**: jot never invents a value for it.

use serde::Serialize;

/// The frontmatter block is not YAML.
pub const RULE_YAML: &str = "yaml";
/// The block is not a mapping, or opens with `---` and never closes.
pub const RULE_FRONTMATTER: &str = "frontmatter";
/// A mapping key is not a string (`1: x`, `true: x`, `~: x`).
pub const RULE_KEY: &str = "key";
/// A key appears twice in one mapping.
pub const RULE_DUPLICATE_KEY: &str = "duplicate-key";
/// A YAML tag (`!foo x`). JSON has none, and jot will not guess what it meant.
pub const RULE_TAG: &str = "tag";
/// `.nan`, `.inf`, or an integer JSON cannot hold without losing precision.
pub const RULE_NUMBER: &str = "number";
/// The file is not UTF-8.
pub const RULE_UTF8: &str = "utf8";

const CONVERSION_RULES: [&str; 7] = [
    RULE_YAML,
    RULE_FRONTMATTER,
    RULE_KEY,
    RULE_DUPLICATE_KEY,
    RULE_TAG,
    RULE_NUMBER,
    RULE_UTF8,
];

/// One thing wrong with one note.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Problem {
    /// The top-level frontmatter key it concerns, from the validator's instance path. `None` when
    /// it concerns the block as a whole.
    pub key: Option<String>,
    /// The JSON Schema keyword that failed, or one of the `RULE_*` constants in this module.
    pub rule: String,
    /// What a person should be told.
    pub message: String,
}

impl Problem {
    pub(crate) fn new(key: Option<&str>, rule: &str, message: impl Into<String>) -> Problem {
        Problem {
            key: key.map(str::to_owned),
            rule: rule.to_string(),
            message: message.into(),
        }
    }

    /// Whether this came from reading the file rather than from the schema.
    #[must_use]
    pub fn is_conversion(&self) -> bool {
        CONVERSION_RULES.contains(&self.rule.as_str())
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.key {
            Some(key) => write!(f, "`{key}` ({}): {}", self.rule, self.message),
            None => write!(f, "({}): {}", self.rule, self.message),
        }
    }
}
