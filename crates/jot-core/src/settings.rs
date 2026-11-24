//! The two declarations at a workspace root: `workspace.toml` and `schema.json`, and the
//! cross-file checks between them (`docs/plans/redesign.md`, "workspace.toml" to "Roles").
//!
//! # What refuses what
//!
//! | Broken | Effect |
//! | --- | --- |
//! | `workspace.toml` (bad TOML, bad `[id] format`, a role that is not a string) | `open` refuses |
//! | `schema.json` (not JSON, not a schema, root not `"type": "object"`) | writes refused |
//! | `[roles]` disagrees with `schema.json` | writes refused |
//! | an unknown key, an unknown role, a key jot no longer reads | a warning |
//!
//! While writes are refused, reads keep working: problems are computed against the schema if it
//! still compiles (otherwise the last valid one), and roles come from the **last valid settings
//! snapshot**, which the index keeps in `index_meta`. [`Settings::is_stale`] says when that is
//! happening, so a surface can label it.
//!
//! # Meanings jot reads into a valid schema
//!
//! - **Key order** is the root `properties` order, then properties reached through `$ref`,
//!   `allOf`, `anyOf` and `oneOf` in first-reached order ([`Schema::order`]).
//! - **`default`** fills a value at creation only.
//! - **`required`** is the root's plus what `$ref` and `allOf` reach — the unconditional ones.
//!
//! `$ref` to another file is resolved relative to `schema.json` and read from disk; nothing is
//! fetched from the network.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::document::Fields;
use crate::id::IdFormat;
use crate::problem::Problem;

/// `workspace.toml`, at the workspace root.
pub const WORKSPACE_TOML: &str = "workspace.toml";
/// `schema.json`, beside it.
pub const SCHEMA_JSON: &str = "schema.json";

/// Which of the two settings files something concerns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum SettingsFile {
    /// `workspace.toml`.
    WorkspaceToml,
    /// `schema.json`.
    SchemaJson,
}

impl SettingsFile {
    /// The file name at the workspace root.
    #[must_use]
    pub fn file_name(self) -> &'static str {
        match self {
            SettingsFile::WorkspaceToml => WORKSPACE_TOML,
            SettingsFile::SchemaJson => SCHEMA_JSON,
        }
    }
}

/// A settings error or warning, naming its file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SettingsIssue {
    /// The file it is about.
    pub file: SettingsFile,
    /// What a person should be told.
    pub message: String,
}

impl SettingsIssue {
    fn new(file: SettingsFile, message: impl Into<String>) -> SettingsIssue {
        SettingsIssue {
            file,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SettingsIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.file.file_name(), self.message)
    }
}

// =============================================================================================
// Roles
// =============================================================================================

/// A meaning core gives one key of `schema.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum Role {
    /// `document:title` — listed; `Untitled` when missing or invalid.
    Title,
    /// `document:created_at` — filled on create, sorted by.
    CreatedAt,
}

impl Role {
    /// Every role this build knows.
    pub const ALL: [Role; 2] = [Role::Title, Role::CreatedAt];

    /// The spelling in `[roles]`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Title => "document:title",
            Role::CreatedAt => "document:created_at",
        }
    }

    /// Parses the `[roles]` spelling.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|role| role.as_str() == name)
    }
}

/// The `[roles]` table, resolved: which key carries each role. `None` means the role is off.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Roles {
    /// The key mapped to `document:title`.
    pub title: Option<String>,
    /// The key mapped to `document:created_at`.
    pub created_at: Option<String>,
}

impl Roles {
    /// The key mapped to `role`.
    #[must_use]
    pub fn key(&self, role: Role) -> Option<&str> {
        match role {
            Role::Title => self.title.as_deref(),
            Role::CreatedAt => self.created_at.as_deref(),
        }
    }

    fn slot(&mut self, role: Role) -> &mut Option<String> {
        match role {
            Role::Title => &mut self.title,
            Role::CreatedAt => &mut self.created_at,
        }
    }

    /// The roles mapped to `key`.
    #[must_use]
    pub fn of_key(&self, key: &str) -> Vec<Role> {
        Role::ALL
            .into_iter()
            .filter(|role| self.key(*role) == Some(key))
            .collect()
    }
}

static NO_ROLES: Roles = Roles {
    title: None,
    created_at: None,
};

// =============================================================================================
// workspace.toml
// =============================================================================================

/// The text `init` writes.
pub(crate) const DEFAULT_WORKSPACE_TOML: &str = "\
# jot workspace settings. The frontmatter schema is in schema.json, beside this file.
# Unknown keys warn and never fail; a key never changes meaning once it exists.

[id]
format = \"uuidv7\"

[roles]
\"document:title\" = \"title\"
\"document:created_at\" = \"created_at\"
";

/// The text `init` writes for `schema.json`.
///
/// Both role keys are required. `created_at` is filled on create; `title` has to be given, with
/// `-t` or in the editor.
pub(crate) const DEFAULT_SCHEMA_JSON: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "title": { "type": "string" },
    "created_at": { "type": "string", "format": "date-time" }
  },
  "required": ["title", "created_at"]
}
"#;

/// `workspace.toml`, parsed. Roles are kept as written; checking them needs the schema.
#[derive(Debug, Clone)]
pub(crate) struct WorkspaceConfig {
    pub(crate) format: IdFormat,
    /// `(role name, key)`, in file order.
    pub(crate) roles: Vec<(String, String)>,
    pub(crate) warnings: Vec<SettingsIssue>,
}

/// Top-level keys of old-format manifests: no longer read, and worth saying so.
const NO_LONGER_READ: [&str; 3] = ["schema_version", "workspace", "schema"];

pub(crate) fn parse_workspace_toml(text: &str) -> Result<WorkspaceConfig, String> {
    let table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    let warn = |message: String| SettingsIssue::new(SettingsFile::WorkspaceToml, message);
    let mut warnings = Vec::new();
    let mut format = IdFormat::default();
    let mut roles = Vec::new();

    for (key, value) in &table {
        match key.as_str() {
            "id" => {
                let Some(id) = value.as_table() else {
                    return Err(String::from("`[id]` must be a table"));
                };
                for (k, v) in id {
                    if k == "format" {
                        let name = v
                            .as_str()
                            .ok_or_else(|| String::from("`[id] format` must be a string"))?;
                        format = IdFormat::from_name(name).ok_or_else(|| {
                            format!("`[id] format = \"{name}\"` is not supported; the only accepted value is \"uuidv7\"")
                        })?;
                    } else {
                        warnings.push(warn(format!("unknown key `id.{k}` is ignored")));
                    }
                }
            }
            "roles" => {
                let Some(table) = value.as_table() else {
                    return Err(String::from("`[roles]` must be a table"));
                };
                for (role, mapped) in table {
                    let mapped = mapped.as_str().ok_or_else(|| {
                        format!("`[roles] \"{role}\"` must name a key of schema.json as a string")
                    })?;
                    roles.push((role.clone(), mapped.to_string()));
                }
            }
            other if NO_LONGER_READ.contains(&other) => warnings.push(warn(format!(
                "`{other}` is no longer read by jot (it belongs to the old workspace format)"
            ))),
            other => warnings.push(warn(format!("unknown key `{other}` is ignored"))),
        }
    }
    Ok(WorkspaceConfig {
        format,
        roles,
        warnings,
    })
}

/// `workspace.toml` with every role mapped to `from` remapped to `to`. Everything else in the
/// table survives; comments and formatting do not, because the `toml` crate does not keep them.
pub(crate) fn rename_role_key(text: &str, from: &str, to: &str) -> Result<String, String> {
    let mut table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    if let Some(toml::Value::Table(roles)) = table.get_mut("roles") {
        for (_, value) in roles.iter_mut() {
            if value.as_str() == Some(from) {
                *value = toml::Value::String(to.to_string());
            }
        }
    }
    toml::to_string_pretty(&table).map_err(|e| e.to_string())
}

// =============================================================================================
// schema.json
// =============================================================================================

/// A compiled `schema.json`, plus the meanings jot reads into it.
#[derive(Clone)]
pub struct Schema {
    raw: Value,
    validator: jsonschema::Validator,
    order: Vec<String>,
    /// Each declared key's subschema, with a chain of `$ref`s merged in.
    properties: BTreeMap<String, Value>,
    required: Vec<String>,
}

impl std::fmt::Debug for Schema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Schema")
            .field("order", &self.order)
            .field("required", &self.required)
            .finish_non_exhaustive()
    }
}

impl Schema {
    /// Compiles `raw`, read from (or standing for) the `schema.json` at `path`.
    pub(crate) fn compile(raw: Value, path: &Path) -> Result<Schema, String> {
        let Some(root) = raw.as_object() else {
            return Err(String::from("the schema must be a JSON object"));
        };
        if root.get("type") != Some(&Value::String(String::from("object"))) {
            return Err(String::from(
                "the root schema must declare `\"type\": \"object\"` — frontmatter is an object",
            ));
        }
        let validator = jsonschema::options()
            .should_validate_formats(true)
            .with_retriever(LocalFiles)
            .with_base_uri(file_uri(path))
            .build(&raw)
            .map_err(|e| e.to_string())?;

        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let mut walk = Walk {
            order: Vec::new(),
            properties: BTreeMap::new(),
            required: Vec::new(),
            seen: HashSet::new(),
        };
        walk.node(&raw, &raw, &dir, true);
        Ok(Schema {
            validator,
            order: walk.order,
            properties: walk.properties,
            required: walk.required,
            raw,
        })
    }

    /// The schema as JSON.
    #[must_use]
    pub fn raw(&self) -> &Value {
        &self.raw
    }

    /// Declared keys, in the order frontmatter is written and displayed.
    #[must_use]
    pub fn order(&self) -> &[String] {
        &self.order
    }

    /// Whether `key` is declared anywhere jot reaches.
    #[must_use]
    pub fn declares(&self, key: &str) -> bool {
        self.properties.contains_key(key)
    }

    /// Whether `key` is unconditionally required.
    #[must_use]
    pub fn is_required(&self, key: &str) -> bool {
        self.required.iter().any(|k| k == key)
    }

    /// Whether `key` is declared in the root `properties` (rather than reached through `$ref`).
    #[must_use]
    pub fn declares_at_root(&self, key: &str) -> bool {
        self.raw
            .get("properties")
            .and_then(Value::as_object)
            .is_some_and(|p| p.contains_key(key))
    }

    /// `key`'s subschema, with `$ref` chains merged in.
    #[must_use]
    pub fn property(&self, key: &str) -> Option<&Value> {
        self.properties.get(key)
    }

    /// `key`'s `default`, which fills it at creation.
    #[must_use]
    pub fn default_of(&self, key: &str) -> Option<&Value> {
        self.property(key).and_then(|p| p.get("default"))
    }

    /// `key`'s `enum`: the only values it may take.
    #[must_use]
    pub fn enum_of(&self, key: &str) -> Option<&[Value]> {
        self.property(key)
            .and_then(|p| p.get("enum"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
    }

    /// `key`'s `format`.
    #[must_use]
    pub fn format_of(&self, key: &str) -> Option<&str> {
        self.property(key)
            .and_then(|p| p.get("format"))
            .and_then(Value::as_str)
    }

    /// `fields` reordered: declared keys in schema order, then the rest as they were.
    #[must_use]
    pub fn ordered(&self, fields: &Fields) -> Fields {
        let mut out = Fields::new();
        for key in &self.order {
            if let Some(value) = fields.get(key) {
                out.insert(key.clone(), value.clone());
            }
        }
        for (key, value) in fields {
            if !out.contains_key(key) {
                out.insert(key.clone(), value.clone());
            }
        }
        out
    }

    /// Per-field problems of `fields`. `absent` are keys with a conversion problem: they are not in
    /// `fields` because jot would not invent them, and saying they are also "missing" would be
    /// the same fact twice.
    pub(crate) fn validate(&self, fields: &Fields, absent: &[&str]) -> Vec<Problem> {
        let instance = Value::Object(fields.clone());
        let mut out: Vec<Problem> = Vec::new();
        for error in self.validator.iter_errors(&instance) {
            let rule = error.kind().keyword().to_string();
            let path = error.instance_path().as_str();
            match error.kind() {
                jsonschema::error::ValidationErrorKind::Required { property }
                    if path.is_empty() =>
                {
                    let key = property.as_str().map(str::to_owned);
                    if key.as_deref().is_some_and(|k| absent.contains(&k)) {
                        continue;
                    }
                    out.push(Problem {
                        key,
                        rule,
                        message: error.to_string(),
                    });
                }
                jsonschema::error::ValidationErrorKind::AdditionalProperties { unexpected }
                | jsonschema::error::ValidationErrorKind::UnevaluatedProperties { unexpected }
                    if path.is_empty() =>
                {
                    for key in unexpected {
                        out.push(Problem {
                            key: Some(key.clone()),
                            rule: rule.clone(),
                            message: format!(
                                "`{key}` is not declared, and the schema allows no other keys"
                            ),
                        });
                    }
                }
                _ => out.push(Problem {
                    key: top_level_key(path),
                    rule,
                    message: error.to_string(),
                }),
            }
        }
        out.dedup();
        out
    }

    /// The problems `value` would have as the value of `key`, independent of any note.
    pub(crate) fn validate_value(&self, key: &str, value: &Value) -> Vec<Problem> {
        let mut fields = Fields::new();
        fields.insert(key.to_string(), value.clone());
        self.validate(&fields, &[])
            .into_iter()
            .filter(|p| p.key.as_deref() == Some(key) && p.rule != "required")
            .collect()
    }
}

/// The first segment of a JSON pointer, unescaped. `None` for the root.
fn top_level_key(pointer: &str) -> Option<String> {
    let rest = pointer.strip_prefix('/')?;
    let segment = rest.split('/').next().unwrap_or(rest);
    Some(segment.replace("~1", "/").replace("~0", "~"))
}

/// The walk that reads key order, declared properties and `required` out of a schema.
struct Walk {
    order: Vec<String>,
    properties: BTreeMap<String, Value>,
    required: Vec<String>,
    seen: HashSet<String>,
}

impl Walk {
    /// `node` lives in the document `doc`, which lives in `dir`. `unconditional` is false under
    /// `anyOf`/`oneOf`, where a `required` is not a requirement of every note.
    fn node(&mut self, node: &Value, doc: &Value, dir: &Path, unconditional: bool) {
        let Some(obj) = node.as_object() else { return };
        if let Some(Value::Object(props)) = obj.get("properties") {
            for (key, sub) in props {
                if !self.properties.contains_key(key) {
                    self.order.push(key.clone());
                    self.properties
                        .insert(key.clone(), merged(sub, doc, dir, 8));
                }
            }
        }
        if unconditional && let Some(Value::Array(required)) = obj.get("required") {
            for key in required.iter().filter_map(Value::as_str) {
                if !self.required.iter().any(|k| k == key) {
                    self.required.push(key.to_string());
                }
            }
        }
        if let Some(Value::String(reference)) = obj.get("$ref")
            && self.seen.insert(format!("{}|{reference}", dir.display()))
            && let Some((target, target_doc, target_dir)) = lookup(reference, doc, dir)
        {
            self.node(&target, &target_doc, &target_dir, unconditional);
        }
        for (keyword, keeps) in [("allOf", true), ("anyOf", false), ("oneOf", false)] {
            if let Some(Value::Array(subs)) = obj.get(keyword) {
                for sub in subs {
                    self.node(sub, doc, dir, unconditional && keeps);
                }
            }
        }
    }
}

/// `sub` with the keywords of the schemas its `$ref` chain points at merged under it — what jot
/// reads `type`, `format` and `default` from. Its own keywords win.
fn merged(sub: &Value, doc: &Value, dir: &Path, hops: u8) -> Value {
    let mut out = sub.clone();
    if hops == 0 {
        return out;
    }
    if let Some(Value::String(reference)) = sub.get("$ref")
        && let Some((target, target_doc, target_dir)) = lookup(reference, doc, dir)
        && let (Value::Object(out_obj), Value::Object(target_obj)) = (
            &mut out,
            merged(&target, &target_doc, &target_dir, hops - 1),
        )
    {
        for (k, v) in target_obj {
            out_obj.entry(k).or_insert(v);
        }
    }
    out
}

/// Resolves a `$ref` for jot's own reading: a JSON pointer into this document or into a local
/// file beside it. Anchors and remote references are not followed (the validator still resolves
/// what it can; this only affects key order and role checks).
fn lookup(reference: &str, doc: &Value, dir: &Path) -> Option<(Value, Value, PathBuf)> {
    let (file, fragment) = reference.split_once('#').unwrap_or((reference, ""));
    let (doc, dir) = if file.is_empty() {
        (doc.clone(), dir.to_path_buf())
    } else {
        if file.contains("://") {
            return None;
        }
        let path = dir.join(file);
        let text = std::fs::read_to_string(&path).ok()?;
        let value: Value = serde_json::from_str(&text).ok()?;
        (value, path.parent().unwrap_or(dir).to_path_buf())
    };
    let target = if fragment.is_empty() {
        doc.clone()
    } else if fragment.starts_with('/') {
        doc.pointer(fragment)?.clone()
    } else {
        return None;
    };
    Some((target, doc, dir))
}

/// The retriever: `file:` URIs only. Nothing is fetched from the network.
struct LocalFiles;

impl jsonschema::Retrieve for LocalFiles {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        if uri.scheme().as_str() != "file" {
            return Err(format!(
                "jot resolves `$ref` from local files beside schema.json only; not fetching `{uri}`"
            )
            .into());
        }
        let decoded = uri.path().decode().to_string_lossy().into_owned();
        #[cfg(windows)]
        let path = PathBuf::from(decoded.trim_start_matches('/').replace('/', "\\"));
        #[cfg(not(windows))]
        let path = PathBuf::from(decoded);
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("`{}`: {e}", path.display()))?;
        Ok(serde_json::from_str(&text)?)
    }
}

/// `path` as a `file:` URI, percent-encoding everything outside the unreserved set.
fn file_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !text.starts_with('/') {
        out.push('/');
    }
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/:".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

// =============================================================================================
// Settings
// =============================================================================================

/// A complete, mutually consistent set of settings. What the index keeps as the "last valid
/// settings snapshot".
#[derive(Debug, Clone)]
pub(crate) struct Valid {
    pub(crate) format: IdFormat,
    pub(crate) roles: Roles,
    pub(crate) schema: Arc<Schema>,
}

#[derive(Serialize, Deserialize)]
struct ValidOnDisk {
    format: String,
    roles: Roles,
    schema: Value,
}

impl Valid {
    pub(crate) fn to_json(&self) -> String {
        serde_json::to_string(&ValidOnDisk {
            format: self.format.as_str().to_string(),
            roles: self.roles.clone(),
            schema: self.schema.raw().clone(),
        })
        .unwrap_or_default()
    }

    pub(crate) fn from_json(text: &str, schema_path: &Path) -> Option<Valid> {
        let disk: ValidOnDisk = serde_json::from_str(text).ok()?;
        Some(Valid {
            format: IdFormat::from_name(&disk.format)?,
            roles: disk.roles,
            schema: Arc::new(Schema::compile(disk.schema, schema_path).ok()?),
        })
    }
}

/// The digests of the two files, as last read. A missing file digests as `"missing"`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Digests {
    pub(crate) workspace_toml: String,
    pub(crate) schema_json: String,
}

/// The settings files' bytes, as read once per sync.
pub(crate) struct SettingsFiles {
    pub(crate) toml: Result<String, String>,
    pub(crate) schema: Result<String, String>,
    pub(crate) digests: Digests,
}

impl SettingsFiles {
    pub(crate) fn read(root: &Path) -> SettingsFiles {
        let (toml, toml_digest) = read_text(&root.join(WORKSPACE_TOML));
        let (schema, schema_digest) = read_text(&root.join(SCHEMA_JSON));
        SettingsFiles {
            toml,
            schema,
            digests: Digests {
                workspace_toml: toml_digest,
                schema_json: schema_digest,
            },
        }
    }
}

fn read_text(path: &Path) -> (Result<String, String>, String) {
    match std::fs::read(path) {
        Ok(bytes) => {
            let digest = blake3::hash(&bytes).to_hex().to_string();
            match String::from_utf8(bytes) {
                Ok(text) => (Ok(text), digest),
                Err(_) => (Err(String::from("the file is not valid UTF-8")), digest),
            }
        }
        Err(e) => (Err(e.to_string()), String::from("missing")),
    }
}

/// A workspace's settings as of the last sync.
#[derive(Debug, Clone)]
pub struct Settings {
    pub(crate) format: IdFormat,
    /// `Some` exactly when there are no errors: what writes are checked against.
    pub(crate) current: Option<Valid>,
    pub(crate) last_valid: Option<Valid>,
    /// The schema reads validate against: the current one if it compiles, else the last valid.
    pub(crate) schema: Option<Arc<Schema>>,
    /// The roles reads use: current when valid, else the last valid snapshot's.
    pub(crate) roles: Roles,
    pub(crate) errors: Vec<SettingsIssue>,
    pub(crate) warnings: Vec<SettingsIssue>,
    /// `workspace.toml`'s own error, which refuses `open`.
    pub(crate) toml_error: Option<String>,
    /// `workspace.toml` as read, when it parsed — what `RenameField` rewrites.
    pub(crate) toml_text: Option<String>,
    /// `[roles]` as written, before the cross-file checks.
    pub(crate) declared_roles: Vec<(String, String)>,
    /// Whether this was loaded from the files at all, rather than from the index alone.
    pub(crate) loaded: bool,
    pub(crate) digests: Digests,
}

impl Settings {
    /// Settings that know nothing — before the first sync, with no snapshot in the index.
    pub(crate) fn empty(last_valid: Option<Valid>) -> Settings {
        Settings {
            format: last_valid.as_ref().map(|v| v.format).unwrap_or_default(),
            current: None,
            roles: last_valid
                .as_ref()
                .map(|v| v.roles.clone())
                .unwrap_or_default(),
            schema: last_valid.as_ref().map(|v| Arc::clone(&v.schema)),
            last_valid,
            errors: Vec::new(),
            warnings: Vec::new(),
            toml_error: None,
            toml_text: None,
            declared_roles: Vec::new(),
            loaded: false,
            digests: Digests::default(),
        }
    }

    /// Settings from the files. `previous` supplies the id format and the last valid snapshot
    /// when `workspace.toml` cannot be read; `reuse` is a compiled schema for unchanged text.
    pub(crate) fn load(
        root: &Path,
        files: &SettingsFiles,
        previous: &Settings,
        reuse: Option<Arc<Schema>>,
    ) -> Settings {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let mut toml_error = None;

        let config = match &files.toml {
            Ok(text) => match parse_workspace_toml(text) {
                Ok(config) => Some(config),
                Err(message) => {
                    toml_error = Some(message);
                    None
                }
            },
            Err(message) => {
                toml_error = Some(message.clone());
                None
            }
        };
        if let Some(message) = &toml_error {
            errors.push(SettingsIssue::new(
                SettingsFile::WorkspaceToml,
                message.clone(),
            ));
        }
        let format = config.as_ref().map_or(previous.format, |c| c.format);
        if let Some(config) = &config {
            warnings.extend(config.warnings.iter().cloned());
        }

        let schema_path = root.join(SCHEMA_JSON);
        let schema = match reuse {
            Some(schema) => Some(schema),
            None => match &files.schema {
                Ok(text) => match serde_json::from_str::<Value>(text) {
                    Ok(raw) => match Schema::compile(raw, &schema_path) {
                        Ok(schema) => Some(Arc::new(schema)),
                        Err(message) => {
                            errors.push(SettingsIssue::new(SettingsFile::SchemaJson, message));
                            None
                        }
                    },
                    Err(e) => {
                        errors.push(SettingsIssue::new(
                            SettingsFile::SchemaJson,
                            format!("not valid JSON: {e}"),
                        ));
                        None
                    }
                },
                Err(message) => {
                    errors.push(SettingsIssue::new(
                        SettingsFile::SchemaJson,
                        message.clone(),
                    ));
                    None
                }
            },
        };

        let mut roles = Roles::default();
        if let (Some(config), Some(schema)) = (&config, &schema) {
            let (resolved, role_errors, role_warnings) = check_roles(&config.roles, schema);
            roles = resolved;
            errors.extend(role_errors);
            warnings.extend(role_warnings);
        }

        let current = match (&config, &schema) {
            (Some(config), Some(schema)) if errors.is_empty() => Some(Valid {
                format: config.format,
                roles: roles.clone(),
                schema: Arc::clone(schema),
            }),
            _ => None,
        };
        let last_valid = current.clone().or_else(|| previous.last_valid.clone());
        let read_schema = schema.or_else(|| last_valid.as_ref().map(|v| Arc::clone(&v.schema)));
        let read_roles = match &current {
            Some(valid) => valid.roles.clone(),
            None => last_valid
                .as_ref()
                .map(|v| v.roles.clone())
                .unwrap_or_default(),
        };

        Settings {
            format,
            current,
            last_valid,
            schema: read_schema,
            roles: read_roles,
            errors,
            warnings,
            toml_text: config.as_ref().and(files.toml.as_ref().ok()).cloned(),
            declared_roles: config.map(|c| c.roles).unwrap_or_default(),
            toml_error,
            loaded: true,
            digests: files.digests.clone(),
        }
    }

    /// The id format `workspace.toml` declares.
    #[must_use]
    pub fn id_format(&self) -> IdFormat {
        self.format
    }

    /// The roles reads use. See [`Settings::is_stale`].
    #[must_use]
    pub fn roles(&self) -> &Roles {
        if self.current.is_none() && self.last_valid.is_none() {
            return &NO_ROLES;
        }
        &self.roles
    }

    /// The schema reads validate against, if there is one.
    #[must_use]
    pub fn schema(&self) -> Option<&Schema> {
        self.schema.as_deref()
    }

    /// Why writes are refused. Empty when they are not.
    #[must_use]
    pub fn errors(&self) -> &[SettingsIssue] {
        &self.errors
    }

    /// What is worth saying and not worth refusing over.
    #[must_use]
    pub fn warnings(&self) -> &[SettingsIssue] {
        &self.warnings
    }

    /// Whether jot may write: both files are valid and agree.
    #[must_use]
    pub fn is_writable(&self) -> bool {
        self.current.is_some()
    }

    /// Whether reads are using the last valid settings snapshot rather than the files as they
    /// stand — the label a surface should show while [`Settings::errors`] is not empty.
    #[must_use]
    pub fn is_stale(&self) -> bool {
        self.current.is_none() && self.last_valid.is_some()
    }

    /// The problems of a note with these fields and these conversion problems.
    pub(crate) fn problems(&self, fields: &Fields, conversion: &[Problem]) -> Vec<Problem> {
        let mut out: Vec<Problem> = conversion.to_vec();
        if let Some(schema) = &self.schema {
            let absent: Vec<&str> = conversion.iter().filter_map(|p| p.key.as_deref()).collect();
            out.extend(schema.validate(fields, &absent));
        }
        out
    }
}

/// The cross-file checks: every mapped key declared, of a compatible type, no key mapped twice.
pub(crate) fn check_roles(
    declared: &[(String, String)],
    schema: &Schema,
) -> (Roles, Vec<SettingsIssue>, Vec<SettingsIssue>) {
    let mut roles = Roles::default();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let err = |m: String| SettingsIssue::new(SettingsFile::WorkspaceToml, m);
    let mut claimed: BTreeMap<&str, &str> = BTreeMap::new();

    for (name, key) in declared {
        let Some(role) = Role::from_name(name) else {
            warnings.push(SettingsIssue::new(
                SettingsFile::WorkspaceToml,
                format!("unknown role `{name}` is ignored"),
            ));
            continue;
        };
        if let Some(other) = claimed.insert(key.as_str(), name.as_str()) {
            errors.push(err(format!(
                "roles `{other}` and `{name}` are both mapped to `{key}`; a key carries at most one role"
            )));
            continue;
        }
        let Some(property) = schema.property(key) else {
            errors.push(err(format!(
                "role `{name}` is mapped to `{key}`, which schema.json does not declare"
            )));
            continue;
        };
        let is_string = property.get("type") == Some(&Value::String(String::from("string")));
        let format = property.get("format").and_then(Value::as_str);
        let compatible = match role {
            Role::Title => is_string,
            Role::CreatedAt => is_string && matches!(format, Some("date" | "date-time")),
        };
        if !compatible {
            let needs = match role {
                Role::Title => "`\"type\": \"string\"`",
                Role::CreatedAt => "`\"type\": \"string\"` with `\"format\"` `date` or `date-time`",
            };
            errors.push(err(format!(
                "role `{name}` needs {needs}, and `{key}` in schema.json is not that"
            )));
            continue;
        }
        *roles.slot(role) = Some(key.clone());
    }
    (roles, errors, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(text: &str) -> Schema {
        Schema::compile(
            serde_json::from_str(text).unwrap(),
            Path::new("/w/schema.json"),
        )
        .unwrap()
    }

    #[test]
    fn roles_that_disagree_with_the_schema_are_settings_errors() {
        let s = schema(
            r#"{"type":"object","properties":{"title":{"type":"integer"},"when":{"type":"string"}}}"#,
        );
        let declared = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
            pairs
                .iter()
                .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
                .collect()
        };
        let (_, errors, warnings) = check_roles(
            &declared(&[
                ("document:title", "title"),     // incompatible type
                ("document:created_at", "when"), // string but no date format
                ("document:author", "who"),      // unknown role: a warning
            ]),
            &s,
        );
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert_eq!(warnings.len(), 1, "{warnings:?}");

        let (_, errors, _) = check_roles(&declared(&[("document:title", "missing")]), &s);
        assert!(errors[0].message.contains("does not declare"), "{errors:?}");

        let s = schema(
            r#"{"type":"object","properties":{"t":{"type":"string","format":"date-time"}}}"#,
        );
        let (roles, errors, _) = check_roles(
            &declared(&[("document:title", "t"), ("document:created_at", "t")]),
            &s,
        );
        assert!(errors[0].message.contains("both mapped"), "{errors:?}");
        assert_eq!(roles.title.as_deref(), Some("t"));
    }

    #[test]
    fn key_order_is_root_properties_then_what_ref_and_all_of_reach() {
        let s = schema(
            r##"{"type":"object",
                "properties":{"b":{"type":"string"},"a":{"$ref":"#/$defs/when"}},
                "allOf":[{"$ref":"#/$defs/extra"}],
                "$defs":{"when":{"type":"string","format":"date"},
                         "extra":{"properties":{"z":{},"b":{}},"required":["z"]}}}"##,
        );
        assert_eq!(s.order(), ["b", "a", "z"]);
        assert_eq!(s.format_of("a"), Some("date"), "a $ref chain is merged in");
        assert!(s.is_required("z"));
    }

    #[test]
    fn a_root_that_is_not_an_object_schema_is_refused() {
        let raw = serde_json::json!({"type": "array"});
        assert!(Schema::compile(raw, Path::new("/w/schema.json")).is_err());
    }

    #[test]
    fn ref_resolves_local_files_beside_the_schema_and_never_the_network() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("my notes");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(
            dir.join("defs.json"),
            r#"{"$defs":{"when":{"type":"string","format":"date"}}}"#,
        )
        .unwrap();
        let raw = serde_json::json!({"type": "object",
            "properties": {"when": {"$ref": "defs.json#/$defs/when"}}});
        let s = Schema::compile(raw, &dir.join("schema.json")).unwrap();
        let mut fields = Fields::new();
        fields.insert("when".into(), Value::String("not a date".into()));
        assert_eq!(s.validate(&fields, &[])[0].rule, "format");
        assert_eq!(
            s.format_of("when"),
            Some("date"),
            "jot reads through the file ref too"
        );

        let remote = serde_json::json!({"type": "object",
            "properties": {"x": {"$ref": "https://example.com/x.json"}}});
        let err = Schema::compile(remote, &dir.join("schema.json")).unwrap_err();
        assert!(err.contains("local files"), "{err}");
    }
}
