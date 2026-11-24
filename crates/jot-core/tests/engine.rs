//! The engine against `tests/fixtures/workspace/`, through the public API only.

use std::path::{Path, PathBuf};

use jot_core::action::{Action, Event, Failure, FieldChange, Outcome, Rejection};
use jot_core::settings::SettingsFile;
use jot_core::snapshot::Ordering;
use jot_core::{Error, NoteId, Workspace};
use serde_json::{Value, json};

const FIRST: &str = "01a03d4c-c708-7cbf-83c0-883cedb7f1d5";
const PROBLEMS: &str = "01a03d4d-5790-7855-9af5-c362987fc91e";
const NEWEST: &str = "01a03d4e-78a0-76bc-be78-8ae41b38eefa";

/// A fresh copy of the fixture. Opening creates `.jot/`, so the fixture is never opened in place.
fn fixture() -> (tempfile::TempDir, PathBuf) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/workspace");
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("ws");
    std::fs::create_dir(&root).unwrap();
    for entry in std::fs::read_dir(&src).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), root.join(entry.file_name())).unwrap();
        }
    }
    (tmp, root)
}

fn id(text: &str) -> NoteId {
    jot_core::IdFormat::UuidV7.check(text).unwrap()
}

fn fields(value: Value) -> jot_core::Fields {
    value.as_object().unwrap().clone()
}

fn done(outcome: Outcome) -> Vec<Event> {
    match outcome {
        Outcome::Done(events) => events,
        other => panic!("expected Done, got {other:?}"),
    }
}

fn read(root: &Path, id: &str) -> String {
    std::fs::read_to_string(root.join(format!("{id}.md"))).unwrap()
}

#[test]
fn an_invalid_note_is_refused_and_nothing_is_written() {
    let (_tmp, root) = fixture();
    let mut ws = Workspace::open(&root).unwrap();
    let before = read(&root, FIRST);

    let outcome = ws.apply(Action::Edit {
        id: id(FIRST),
        fields: fields(
            json!({"title": "x", "created_at": "2026-11-01T09:00:00Z", "counter": "two"}),
        ),
        body: String::from("new body"),
        expected_hash: None,
    });
    let Outcome::Rejected(Rejection::Invalid { problems }) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(problems[0].key.as_deref(), Some("counter"));
    assert_eq!(read(&root, FIRST), before, "a refused edit writes nothing");

    // Format assertion is on: `yesterday` is not a date-time.
    let count = std::fs::read_dir(&root).unwrap().count();
    let outcome = ws.apply(Action::Create {
        fields: fields(json!({"created_at": "yesterday"})),
        body: String::new(),
    });
    assert!(
        matches!(outcome, Outcome::Rejected(Rejection::Invalid { .. })),
        "{outcome:?}"
    );
    assert_eq!(
        std::fs::read_dir(&root).unwrap().count(),
        count,
        "no file was created"
    );

    // An `$EDITOR` buffer whose YAML cannot become JSON is refused the same way.
    let rejected = Action::edit_from_text(id(FIRST), "---\nn: .nan\n---\n", None).unwrap_err();
    assert!(matches!(rejected, Rejection::Invalid { .. }));
}

#[test]
fn reads_report_problems_per_field_and_keep_the_note_and_its_value() {
    let (_tmp, root) = fixture();
    let ws = Workspace::open(&root).unwrap();
    let problems = &ws.problems();
    assert_eq!(problems.len(), 1, "{problems:?}");
    let (who, list) = &problems[0];
    assert_eq!(who, &id(PROBLEMS));
    let keyed: Vec<(Option<&str>, &str)> = list
        .iter()
        .map(|p| (p.key.as_deref(), p.rule.as_str()))
        .collect();
    assert!(keyed.contains(&(Some("counter"), "type")), "{keyed:?}");
    assert!(
        keyed.contains(&(Some("created_at"), "required")),
        "{keyed:?}"
    );

    let note = ws.get(&id(PROBLEMS)).unwrap().unwrap();
    assert_eq!(note.fields["counter"], json!("two"), "never auto-fixed");

    let listing = ws.list();
    assert_eq!(
        listing.ordering,
        Ordering::CreatedAt {
            key: String::from("created_at")
        }
    );
    let order: Vec<&str> = listing.rows.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(
        order,
        [NEWEST, FIRST, PROBLEMS],
        "newest first, missing created_at last"
    );
    assert_eq!(listing.rows[0].display_title(), "Untitled");
}

#[test]
fn strays_are_reported_and_never_indexed() {
    let (_tmp, root) = fixture();
    let ws = Workspace::open(&root).unwrap();
    let mut names: Vec<String> = ws
        .strays()
        .iter()
        .map(|s| s.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [format!("{FIRST} - Copy.md"), String::from("README.md")]
    );
    assert_eq!(ws.snapshot().len(), 3);
}

#[test]
fn roles_that_disagree_with_the_schema_refuse_writes_and_reads_keep_the_last_valid_roles() {
    let (_tmp, root) = fixture();
    let mut ws = Workspace::open(&root).unwrap();
    let toml = std::fs::read_to_string(root.join("workspace.toml")).unwrap();
    std::fs::write(
        root.join("workspace.toml"),
        toml.replace(
            "\"document:title\" = \"title\"",
            "\"document:title\" = \"counter\"",
        ),
    )
    .unwrap();

    let events = done(ws.sync());
    assert!(events.contains(&Event::SettingsChanged {
        file: SettingsFile::WorkspaceToml
    }));
    let settings = ws.settings();
    assert!(!settings.is_writable() && settings.is_stale());
    assert!(
        settings.errors()[0].message.contains("counter"),
        "{:?}",
        settings.errors()
    );
    assert_eq!(
        settings.roles().title.as_deref(),
        Some("title"),
        "last valid roles"
    );
    assert_eq!(ws.row(&id(FIRST)).unwrap().display_title(), "First note");

    assert!(ws.check_writable().is_err());
    let outcome = ws.apply(Action::Create {
        fields: fields(json!({"title": "t"})),
        body: String::new(),
    });
    assert!(
        matches!(outcome, Outcome::Rejected(Rejection::Settings { .. })),
        "{outcome:?}"
    );
}

#[test]
fn a_write_puts_keys_in_schema_order_and_undeclared_keys_last() {
    let (_tmp, root) = fixture();
    let mut ws = Workspace::open(&root).unwrap();
    let events = done(ws.apply(Action::Create {
        fields: fields(json!({"zeta": 1, "counter": 2, "title": "Ordered", "alpha": true})),
        body: String::from("body"),
    }));
    let Event::NoteCreated { id } = &events[0] else {
        panic!("{events:?}")
    };
    let text = read(&root, id.as_str());
    let keys: Vec<&str> = text
        .lines()
        .skip(1)
        .take_while(|l| *l != "---")
        .map(|l| l.split(':').next().unwrap())
        .collect();
    // `created_at` is filled on create because the role is mapped.
    assert_eq!(
        keys,
        ["title", "created_at", "counter", "zeta", "alpha"],
        "{text}"
    );
}

#[test]
fn sync_reports_precise_events_and_stays_quiet_about_its_own_writes() {
    let (_tmp, root) = fixture();
    let mut ws = Workspace::open(&root).unwrap();
    let rx = ws.subscribe();

    let created = done(ws.apply(Action::Create {
        fields: fields(json!({"title": "Mine"})),
        body: String::new(),
    }));
    assert_eq!(
        rx.try_iter().collect::<Vec<_>>(),
        created,
        "subscribers get a copy"
    );
    assert!(
        done(ws.sync()).is_empty(),
        "core's own write is not reported again"
    );

    let first = root.join(format!("{FIRST}.md"));
    let text = std::fs::read_to_string(&first).unwrap();
    std::fs::write(
        &first,
        text.replace("title: First note", "title: Renamed by hand"),
    )
    .unwrap();
    std::fs::remove_file(root.join(format!("{NEWEST}.md"))).unwrap();
    std::fs::write(root.join("notes.md"), "a stray").unwrap();

    let events = done(ws.sync());
    assert_eq!(
        events,
        [
            Event::NoteChanged {
                id: id(FIRST),
                fields: vec![FieldChange {
                    key: String::from("title"),
                    before: Some(json!("First note")),
                    after: Some(json!("Renamed by hand")),
                }],
            },
            Event::NoteDeleted { id: id(NEWEST) },
            Event::StrayAppeared {
                path: root.join("notes.md")
            },
        ]
    );
    assert_eq!(rx.try_iter().count(), events.len());
}

#[test]
fn rename_field_renames_every_note_the_schema_and_the_role() {
    let (_tmp, root) = fixture();
    let mut ws = Workspace::open(&root).unwrap();
    let rename = || Action::RenameField {
        from: String::from("title"),
        to: String::from("heading"),
    };

    // jot never writes a note that fails the schema, batches included: a note the rename would
    // touch that is already invalid blocks the whole batch, by name, until it is fixed.
    let Outcome::Rejected(Rejection::NotesInvalid { notes }) = ws.apply(rename()) else {
        panic!("an invalid note must block the batch");
    };
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].0, id(PROBLEMS));
    done(ws.apply(Action::Edit {
        id: id(PROBLEMS),
        fields: fields(
            json!({"title": "Fixed", "created_at": "2026-11-02T00:00:00Z", "counter": 2}),
        ),
        body: String::from("fixed"),
        expected_hash: None,
    }));

    let events = done(ws.apply(rename()));
    assert!(
        matches!(events[0], Event::BatchStarted { total: 2, .. }),
        "{events:?}"
    );
    assert!(read(&root, FIRST).contains("heading: First note"));
    assert!(!read(&root, FIRST).contains("title:"));

    let schema: Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("schema.json")).unwrap()).unwrap();
    let keys: Vec<&String> = schema["properties"].as_object().unwrap().keys().collect();
    assert_eq!(keys[0], "heading", "renamed in place");
    let toml = std::fs::read_to_string(root.join("workspace.toml")).unwrap();
    assert!(toml.contains("\"document:title\" = \"heading\""), "{toml}");

    assert!(ws.settings().is_writable(), "{:?}", ws.settings().errors());
    assert_eq!(ws.row(&id(FIRST)).unwrap().display_title(), "First note");

    // Idempotent: running it again finds nothing left to do.
    assert!(ws.decide(&rename()).unwrap().is_noop());
}

#[test]
fn an_edit_against_a_stale_hash_fails_and_keeps_the_external_change() {
    let (_tmp, root) = fixture();
    let mut ws = Workspace::open(&root).unwrap();
    let note = ws.get(&id(FIRST)).unwrap().unwrap();

    let external = note.text.replace("First note", "Edited elsewhere");
    std::fs::write(root.join(format!("{FIRST}.md")), &external).unwrap();

    let outcome = ws.apply(Action::Edit {
        id: id(FIRST),
        fields: note.fields.clone(),
        body: String::from("mine"),
        expected_hash: Some(note.hash),
    });
    assert_eq!(outcome, Outcome::Failed(Failure::Changed { id: id(FIRST) }));
    assert_eq!(read(&root, FIRST), external);
}

#[test]
fn an_old_format_vault_is_refused_with_a_message_saying_the_format_changed() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join(".jot")).unwrap();
    std::fs::write(
        tmp.path().join(".jot/workspace.toml"),
        "schema_version = 2\n",
    )
    .unwrap();
    let err = Workspace::open(tmp.path()).unwrap_err();
    assert!(matches!(err, Error::OldFormat { .. }));
    assert!(err.to_string().contains("format changed"), "{err}");
}

#[test]
fn the_new_note_template_lists_every_declared_key_and_says_what_it_needs() {
    let (_tmp, root) = fixture();
    let schema = std::fs::read_to_string(root.join("schema.json")).unwrap();
    let schema = schema.replace(
        r#""required": ["created_at"]"#,
        r#""required": ["title", "created_at"]"#,
    );
    std::fs::write(root.join("schema.json"), schema).unwrap();
    let mut ws = Workspace::open(&root).unwrap();

    let template = ws.template().unwrap();
    let lines: Vec<&str> = template.lines().collect();
    assert_eq!(lines[..2], ["---", "title: # required"], "{template}");
    assert!(
        lines[2].starts_with("created_at: 20") && lines[2].ends_with("Z # required"),
        "{template}"
    );
    assert_eq!(
        lines[3..],
        [
            "status: # seed | growing | done",
            "counter:",
            "tags:",
            "---",
            "",
        ],
        "{template}"
    );

    // Saved with only a title typed: the empty keys are dropped, not written as null, and the
    // comments are gone.
    let text = template.replacen("title: # required", "title: a title", 1);
    let events = done(ws.apply(Action::create_from_text(&text).unwrap()));
    let Event::NoteCreated { id } = &events[0] else {
        panic!("{events:?}")
    };
    let written = read(&root, id.as_str());
    assert!(
        written.starts_with("---\ntitle: a title\ncreated_at: "),
        "{written}"
    );
    for absent in ["#", "status", "counter", "tags", "null"] {
        assert!(!written.contains(absent), "`{absent}` in {written}");
    }

    // Saved untouched: the title is still missing, so it is refused.
    assert!(matches!(
        ws.apply(Action::create_from_text(&template).unwrap()),
        Outcome::Rejected(Rejection::Invalid { .. })
    ));
}

#[test]
fn init_writes_both_settings_files_and_refuses_to_overwrite() {
    let tmp = tempfile::tempdir().unwrap();
    let mut ws = Workspace::init(tmp.path()).unwrap();
    assert!(ws.settings().is_writable(), "{:?}", ws.settings().errors());
    // The default schema requires a title; `created_at` is required too, and filled.
    assert!(matches!(
        ws.apply(Action::Create {
            fields: jot_core::Fields::new(),
            body: String::from("a thought"),
        }),
        Outcome::Rejected(Rejection::Invalid { .. })
    ));
    done(ws.apply(Action::Create {
        fields: fields(json!({ "title": "a title" })),
        body: String::from("a thought"),
    }));
    assert!(matches!(
        Workspace::init(tmp.path()),
        Err(Error::WorkspaceExists { .. })
    ));
}
