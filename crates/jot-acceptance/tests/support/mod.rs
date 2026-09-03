#![cfg(feature = "stage4")]
#![allow(dead_code)]
//! Shared scaffolding for the stage 4 suites.
//!
//! Two things live here that neither suite could own alone:
//!
//! * [`describe`] — the whole of a workspace's **publicly observable** state, rendered as lines.
//!   Stage 4's first acceptance criterion is "reproduces every query result exactly", and
//!   `stage4.md` insists the rebuild invariant be compared over whole `Record`s rather than note
//!   rows. `Record` is not reachable from outside `jot-core` (`Workspace::snapshot` is
//!   `#[cfg(test)]`-private, deliberately, because there is no `&Snapshot` to hand back once
//!   SQLite is behind the seam), so this reconstructs every field of it from the public API and
//!   compares that instead. The mapping is spelled out on [`describe`].
//! * Vault builders, so a test says what a vault *is* rather than how to write one.
//!
//! Nothing here calls [`jot_core::workspace::Workspace::get`] or `links_in`: both re-read the
//! file, so an answer that came from them would prove nothing about the index.

use jot_core::note::{NoteId, NoteMeta};
use jot_core::query::{FileSort, Page, Ref, Resolution, Row, SearchQuery, State, TimelineQuery};
use jot_core::thread::{Thread, TreeNode};
use jot_core::workspace::Workspace;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

// =============================================================================================
// Building vaults
// =============================================================================================

/// A note file, described rather than spelled.
pub struct Spec {
    pub id: String,
    pub filename: Option<String>,
    pub keys: Vec<String>,
    pub body: String,
    pub trashed: bool,
}

impl Spec {
    pub fn new(id: &str) -> Self {
        Spec {
            id: id.to_string(),
            filename: None,
            keys: Vec::new(),
            body: String::new(),
            trashed: false,
        }
    }
    pub fn title(mut self, title: &str) -> Self {
        self.keys.insert(0, format!("title: {title}"));
        self
    }
    pub fn key(mut self, line: &str) -> Self {
        self.keys.push(line.to_string());
        self
    }
    pub fn reply_to(self, parent: &str) -> Self {
        self.key(&format!("relation:reply_to: {parent}"))
    }
    pub fn quote(self, quoted: &str) -> Self {
        self.key(&format!("relation:quote_to: {quoted}"))
    }
    pub fn body(mut self, body: &str) -> Self {
        self.body = body.to_string();
        self
    }
    pub fn filename(mut self, name: &str) -> Self {
        self.filename = Some(name.to_string());
        self
    }
    pub fn trashed(mut self) -> Self {
        self.trashed = true;
        self
    }

    pub fn file_name(&self) -> String {
        self.filename
            .clone()
            .unwrap_or_else(|| format!("{}.md", self.id))
    }

    pub fn text(&self) -> String {
        let mut block = String::new();
        for line in &self.keys {
            block.push_str(line);
            block.push('\n');
        }
        format!("---\n{block}---\n\n{}\n", self.body)
    }

    /// Write this note into `root`, in the directory its state implies.
    pub fn write(&self, root: &Path) -> PathBuf {
        let dir = if self.trashed {
            root.join(".jot").join(".trash")
        } else {
            root.to_path_buf()
        };
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(self.file_name());
        std::fs::write(&path, self.text()).unwrap();
        path
    }
}

/// An initialised workspace at `<tmp>/v` holding exactly `specs`, already synced.
pub fn vault_of(tmp: &Path, specs: &[Spec]) -> (PathBuf, Workspace) {
    let root = tmp.join("v");
    Workspace::init(&root).expect("init");
    for spec in specs {
        spec.write(&root);
    }
    let mut ws = Workspace::open(&root).expect("open");
    ws.sync().expect("sync");
    (root, ws)
}

/// A manifest declaring `entries`, each `(key, type)`, written over whatever `init` wrote.
///
/// `key` is omitted from the file when it equals the type string, which is the shape
/// `Manifest::to_toml` itself produces.
pub fn write_manifest(root: &Path, entries: &[(&str, &str)]) {
    let mut text = String::from(
        "schema_version = 2\n\n[workspace]\nid = \"b4b4856a-e5db-4f9b-bd87-658b0be50741\"\n\
         name = \"V\"\n",
    );
    for (key, field_type) in entries {
        text.push_str("\n[[schema.frontmatter]]\n");
        if key != field_type {
            let _ = writeln!(text, "key = \"{key}\"");
        }
        let _ = writeln!(text, "type = \"{field_type}\"");
    }
    std::fs::create_dir_all(root.join(".jot")).unwrap();
    std::fs::write(root.join(".jot").join("workspace.toml"), text).unwrap();
}

// =============================================================================================
// The publicly observable state of a workspace
// =============================================================================================

/// Every question the public API can be asked about a vault, rendered as sorted, comparable lines.
///
/// # Why lines
///
/// The two things this is used for — "deleting the index reproduces every query result exactly"
/// and the rebuild invariant — both fail as *one field somewhere is different*, and a line diff
/// names that field. A derived `PartialEq` over a nested struct would say `false`.
///
/// # The `Record` mapping
///
/// `stage4.md`: "the comparison is over whole `Record`s, not note rows". `Record` is
/// `{ meta, path, state, edited_at, links, undeclared }`. Reconstructed here as:
///
/// | `Record` field | Observed through |
/// | --- | --- |
/// | `meta` | [`Workspace::meta`] — id, created_at, title, root, reply_to, quote |
/// | `path` | [`Workspace::note_path`] (live notes only; see the note below) |
/// | `state` | [`Workspace::state_of`] |
/// | `edited_at` | **deliberately omitted** — `overview.md` exempts mtime from the invariant |
/// | `links` | inverted [`Workspace::backlinks`] over every id, so a link edge that the index
/// |          | forgot vanishes from this listing |
/// | `undeclared` | [`Workspace::problems`]'s `UndeclaredKey` entries |
///
/// A **trashed** note's path is not publicly observable — `note_path` searches live notes only —
/// so the path line reads `-` for one. That gap is recorded in the phase A report rather than
/// papered over by reaching into the trash directory, which would be this suite asserting against
/// the filesystem instead of against the index.
pub fn describe(ws: &Workspace) -> Vec<String> {
    let mut out = Vec::new();
    let (active, trashed) = ws.counts();
    out.push(format!("counts active={active} trashed={trashed}"));

    for problem in ws.problems() {
        out.push(format!("problem {problem}"));
    }

    push_page(&mut out, "timeline", &ws.timeline(&TimelineQuery::new()));
    push_page(
        &mut out,
        "timeline_flat",
        &ws.timeline(&TimelineQuery::new().flat()),
    );
    push_page(
        &mut out,
        "timeline_limit2",
        &ws.timeline(&TimelineQuery::new().limit(2)),
    );
    push_rows(&mut out, "files_created", &ws.files(FileSort::Created));
    push_rows(&mut out, "files_edited", &ws.files(FileSort::Edited));
    push_rows(&mut out, "files_title", &ws.files(FileSort::Title));
    push_rows(&mut out, "trash", &ws.trashed());
    push_rows(
        &mut out,
        "search_all",
        &ws.search(&SearchQuery::new("").include_trashed()),
    );
    push_rows(&mut out, "search_active", &ws.search(&SearchQuery::new("")));

    let abbreviations = ws.abbreviations(1);
    let ids: Vec<NoteId> = abbreviations.keys().copied().collect();

    for &id in &ids {
        let prefix = &abbreviations[&id];
        out.push(format!("note {id} abbrev={prefix}"));
        out.push(format!(
            "note {id} meta={}",
            ws.meta(id).map_or("-".to_string(), render_meta)
        ));
        out.push(format!(
            "note {id} state={}",
            ws.state_of(id).map_or("-".to_string(), |s| s.to_string())
        ));
        out.push(format!(
            "note {id} path={}",
            ws.note_path(id)
                .expect("note_path may not fail on a healthy vault")
                .map_or("-".to_string(), |p| relative(ws.root(), &p))
        ));
        out.push(format!("note {id} ref={}", render_ref(&ws.reference(id))));
        out.push(format!(
            "note {id} resolve_exact={}",
            render_resolution(&ws.resolve(&id.to_string()))
        ));
        out.push(format!(
            "note {id} resolve_prefix={}",
            render_resolution(&ws.resolve(prefix))
        ));
        out.push(format!(
            "note {id} thread={}",
            ws.thread(id).map_or("-".to_string(), |t| render_thread(&t))
        ));
        out.push(format!(
            "note {id} backlinks={}",
            render_ids(ws.backlinks(id).iter().map(|m| m.id))
        ));
        out.push(format!(
            "note {id} quoted_by={}",
            render_ids(ws.quoted_by(id).iter().map(|m| m.id))
        ));
    }

    // `Record::links`, inverted. The index's link edges are only reachable from outside the crate
    // through `backlinks`, so this is what a lost `links` row looks like from here.
    for &from in &ids {
        let targets: Vec<NoteId> = ids
            .iter()
            .copied()
            .filter(|&to| ws.backlinks(to).iter().any(|m| m.id == from))
            .collect();
        out.push(format!(
            "note {from} links_to={}",
            render_ids(targets.into_iter())
        ));
    }

    out
}

/// [`describe`] plus the ids that do not exist, so a dangling reference's *resolution* is part of
/// the comparison too. `extra` is the set of ids a test knows the vault no longer holds.
pub fn describe_with_dangling(ws: &Workspace, extra: &[NoteId]) -> Vec<String> {
    let mut out = describe(ws);
    for &id in extra {
        out.push(format!(
            "dangling {id} ref={}",
            render_ref(&ws.reference(id))
        ));
        out.push(format!(
            "dangling {id} thread={}",
            ws.thread(id).map_or("-".to_string(), |t| render_thread(&t))
        ));
        out.push(format!(
            "dangling {id} backlinks={}",
            render_ids(ws.backlinks(id).iter().map(|m| m.id))
        ));
    }
    out
}

/// Fail with a line-by-line diff. `assert_eq!` on two 300-line vectors prints two walls.
pub fn assert_views_eq(actual: &[String], expected: &[String], context: &str) {
    if actual == expected {
        return;
    }
    let mut report = String::new();
    let _ = writeln!(
        report,
        "{context}\n{} lines vs {} lines",
        actual.len(),
        expected.len()
    );
    let max = actual.len().max(expected.len());
    let mut shown = 0;
    for i in 0..max {
        let a = actual.get(i).map(String::as_str).unwrap_or("<missing>");
        let b = expected.get(i).map(String::as_str).unwrap_or("<missing>");
        if a != b {
            let _ = writeln!(report, "  line {i}:\n    actual   {a}\n    expected {b}");
            shown += 1;
            if shown == 20 {
                let _ = writeln!(report, "  … and more");
                break;
            }
        }
    }
    panic!("{report}");
}

// ------------------------------------------------------------------------------- rendering

fn push_page(out: &mut Vec<String>, label: &str, page: &Page<Row>) {
    push_rows(out, label, &page.items);
    out.push(format!(
        "{label}.next={}",
        page.next.map_or("-".to_string(), |id| id.to_string())
    ));
}

fn push_rows(out: &mut Vec<String>, label: &str, rows: &[Row]) {
    for (i, row) in rows.iter().enumerate() {
        out.push(format!("{label}[{i}] {}", render_row(row)));
    }
    out.push(format!("{label}.len={}", rows.len()));
}

/// A row without its `edited_at`. The exemption is `overview.md`'s and is the only one.
fn render_row(row: &Row) -> String {
    format!(
        "{} state={} replies={} descendants={} root={} parent={}",
        render_meta(&row.note),
        row.state,
        row.replies,
        row.descendants,
        row.is_root(),
        row.parent.as_ref().map_or("-".to_string(), render_ref),
    )
}

fn render_meta(meta: &NoteMeta) -> String {
    format!(
        "id={} created_at={} title={:?} root={} reply_to={} quote={}",
        meta.id,
        meta.created_at.map_or("-".to_string(), |t| t.to_rfc3339()),
        meta.title,
        opt_id(meta.root),
        opt_id(meta.reply_to),
        opt_id(meta.quote),
    )
}

fn render_ref(r: &Ref) -> String {
    match r {
        Ref::Present(meta) => format!("present({})", meta.id),
        Ref::Trashed(meta) => format!("trashed({})", meta.id),
        Ref::Deleted(id) => format!("deleted({id})"),
    }
}

fn render_resolution(r: &Resolution) -> String {
    match r {
        Resolution::Unique(meta) => format!("unique({})", meta.id),
        Resolution::Ambiguous(metas) => {
            format!("ambiguous({})", render_ids(metas.iter().map(|m| m.id)))
        }
        Resolution::None => "none".to_string(),
    }
}

fn render_thread(thread: &Thread) -> String {
    format!(
        "focus={} ancestors={} tree={}",
        thread.focus,
        render_ids(thread.ancestors.iter().map(|m| m.id)),
        render_tree(&thread.tree),
    )
}

fn render_tree(node: &TreeNode) -> String {
    if node.children.is_empty() {
        return node.id().to_string();
    }
    let kids: Vec<String> = node.children.iter().map(render_tree).collect();
    format!("{}({})", node.id(), kids.join(","))
}

fn render_ids(ids: impl Iterator<Item = NoteId>) -> String {
    let v: Vec<String> = ids.map(|id| id.to_string()).collect();
    format!("[{}]", v.join(","))
}

fn opt_id(id: Option<NoteId>) -> String {
    id.map_or("-".to_string(), |id| id.to_string())
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

// =============================================================================================
// Small conveniences
// =============================================================================================

pub fn nid(s: &str) -> NoteId {
    s.parse()
        .unwrap_or_else(|e| panic!("{s} is not a note id: {e}"))
}

/// The rows a timeline returns, as ids.
pub fn timeline_ids(ws: &Workspace) -> Vec<NoteId> {
    ws.timeline(&TimelineQuery::new())
        .items
        .iter()
        .map(|row| row.note.id)
        .collect()
}

pub fn state_words(ws: &Workspace, id: NoteId) -> Option<State> {
    ws.state_of(id)
}
