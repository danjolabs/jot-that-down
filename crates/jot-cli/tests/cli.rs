//! End-to-end tests: the real binary, against a real temp workspace.
//!
//! These are the tests that would catch a command wired to the wrong core call, an exit code that
//! drifted, or a `--json` key that got renamed — none of which a unit test in either crate can see,
//! because each half is individually correct in every one of those failures.
//!
//! # Isolation
//!
//! Every test gets its own workspace and passes `--workspace`, so nothing depends on the machine's
//! registry, on the working directory, or on another test. `NO_COLOR` is set for all of them so
//! assertions match plain bytes.

use assert_cmd::Command;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// A uuidv7 for notes written by hand.
const HAND_ID: &str = "01a03d4c-c708-7cbf-83c0-883cedb7f1d5";

/// A temp workspace plus a builder for commands pointed at it.
struct Vault {
    // Held so the directory outlives the test; every path goes through `root`.
    _dir: TempDir,
    // The temp dir in the registry's canonical spelling. On a Windows runner the temp dir can be an
    // 8.3 short path (`C:\Users\RUNNER~1\…`) while the registry stores the long one
    // (`runneradmin`), so comparing against the raw spelling fails for a correct registry.
    root: PathBuf,
}

impl Vault {
    fn new() -> Vault {
        let dir = tempfile::tempdir().unwrap();
        let root = jot_core::registry::canonical(dir.path());
        let vault = Vault { _dir: dir, root };
        vault.jot(&["ws", "new", &vault.path().display().to_string()]);
        vault
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn ws(&self) -> &str {
        self.path().to_str().unwrap()
    }

    /// The command builder: isolated registry, no colour, no editor, this directory.
    fn cmd(&self) -> Command {
        let mut cmd = Command::cargo_bin("jot").unwrap();
        cmd.env("NO_COLOR", "1")
            .env_remove("JOT_WORKSPACE")
            .env_remove("VISUAL")
            .env_remove("EDITOR")
            // A registry write must never touch the developer's real one, and `JOT_REGISTRY` is
            // the only thing that guarantees it: `directories` reads `FOLDERID_RoamingAppData`
            // through a syscall on Windows, so `XDG_CONFIG_HOME`/`HOME` isolate nothing there.
            .env("JOT_REGISTRY", self.root.join("registry.toml"))
            .current_dir(&self.root);
        cmd
    }

    /// Run a command that must succeed; returns stdout.
    fn jot(&self, args: &[&str]) -> String {
        let output = self.cmd().args(args).assert().success();
        String::from_utf8(output.get_output().stdout.clone()).unwrap()
    }

    /// Run a command against this workspace explicitly, with full ids.
    fn run(&self, args: &[&str]) -> String {
        let mut full = vec!["--workspace", self.ws(), "--long"];
        full.extend_from_slice(args);
        self.jot(&full)
    }

    /// Run a command against this workspace; returns stderr.
    fn stderr(&self, args: &[&str]) -> String {
        let mut full = vec!["--workspace", self.ws()];
        full.extend_from_slice(args);
        let output = self.cmd().args(&full).assert().success();
        String::from_utf8(output.get_output().stderr.clone()).unwrap()
    }

    /// Create a note and return its full id.
    fn new_note(&self, args: &[&str]) -> String {
        let mut full = vec!["new"];
        full.extend_from_slice(args);
        self.run(&full).trim().to_owned()
    }

    /// Parse a command's `--json` output.
    fn json(&self, args: &[&str]) -> Value {
        let mut full = vec!["--json"];
        full.extend_from_slice(args);
        serde_json::from_str(&self.run(&full)).expect("valid JSON on stdout")
    }

    /// The exit code and stderr of a command expected to fail.
    fn fail(&self, args: &[&str]) -> (i32, String) {
        let mut full = vec!["--workspace", self.ws()];
        full.extend_from_slice(args);
        let assert = self.cmd().args(&full).assert().failure();
        let output = assert.get_output();
        (
            output.status.code().unwrap(),
            String::from_utf8(output.stderr.clone()).unwrap(),
        )
    }

    fn code(&self, args: &[&str]) -> i32 {
        self.fail(args).0
    }

    fn note_file(&self, id: &str) -> PathBuf {
        self.path().join(format!("{id}.md"))
    }

    /// Replace the settings: `roles` is the `[roles]` table's body, `schema` the whole schema.
    fn settings(&self, roles: &str, schema: &str) {
        std::fs::write(
            self.path().join("workspace.toml"),
            format!("[id]\nformat = \"uuidv7\"\n\n[roles]\n{roles}"),
        )
        .unwrap();
        std::fs::write(self.path().join("schema.json"), schema).unwrap();
    }
}

// =============================================================================================
// Workspaces
// =============================================================================================

#[test]
fn ws_new_creates_both_settings_files_and_refuses_to_overwrite_them() {
    let vault = Vault::new();
    for expected in ["workspace.toml", "schema.json", ".jot"] {
        assert!(vault.path().join(expected).exists(), "missing `{expected}`");
    }
    let toml = std::fs::read_to_string(vault.path().join("workspace.toml")).unwrap();
    assert!(toml.contains("\"document:title\""), "{toml}");

    assert_eq!(vault.code(&["ws", "new", vault.ws()]), 1);
    assert_eq!(
        std::fs::read_to_string(vault.path().join("workspace.toml")).unwrap(),
        toml,
        "the original is untouched"
    );
}

#[test]
fn every_command_answers_to_its_full_word_and_its_short_alias() {
    let vault = Vault::new();
    vault.new_note(&["-m", "a thought"]);

    assert_eq!(vault.run(&["list"]), vault.run(&["ls"]));

    for name in ["remove", "rm"] {
        let id = vault.new_note(&["-m", "temporary"]);
        vault.run(&[name, "--yes", &id]);
        assert!(!vault.note_file(&id).exists(), "`{name}` must delete");
    }

    let expected = vault.run(&["ws", "ls"]);
    for spelling in [["workspace", "list"], ["workspace", "ls"], ["ws", "list"]] {
        assert_eq!(vault.run(&spelling), expected, "{spelling:?}");
    }

    // A group with no subcommand is a usage error, which is clap's behaviour and stays so.
    assert_eq!(vault.code(&["workspace"]), 2);
    assert_eq!(vault.code(&["field"]), 2);
}

#[test]
fn ws_remove_unregisters_without_touching_the_folder_and_clears_current() {
    let vault = Vault::new();
    let other = vault.path().join("other");
    vault.run(&["ws", "new", other.to_str().unwrap()]);
    assert_eq!(
        current(&vault)["name"],
        "other",
        "a new workspace is current"
    );

    vault.run(&["workspace", "remove", "--name", "other"]);

    let entries = vault.json(&["ws", "ls"]);
    let entries = entries.as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(
        entries.iter().all(|entry| entry["current"] == false),
        "`current` must not dangle: {entries:?}"
    );
    assert!(
        other.join("workspace.toml").is_file(),
        "the folder survives"
    );
}

#[test]
fn ws_use_selects_by_path_and_by_name_only_when_asked() {
    let vault = Vault::new();
    let other = vault.path().join("other");
    vault.run(&["ws", "new", other.to_str().unwrap()]);

    vault.run(&["ws", "use", vault.ws()]);
    assert_eq!(current(&vault)["path"], vault.ws());

    vault.run(&["ws", "use", "--name", "other"]);
    assert_eq!(current(&vault)["name"], "other");

    // A bare argument is a path, never a name.
    assert_eq!(vault.code(&["ws", "use", "no-such-folder"]), 3);
    assert_eq!(vault.code(&["ws", "use", "--name", "no-such-name"]), 3);
    assert_eq!(vault.code(&["ws", "use", "x", "--name", "y"]), 2);
}

#[test]
fn a_name_two_workspaces_share_is_refused_rather_than_guessed() {
    let vault = Vault::new();
    for parent in ["a", "b"] {
        let path = vault.path().join(parent).join("notes");
        vault.run(&["ws", "new", path.to_str().unwrap()]);
    }
    let (code, stderr) = vault.fail(&["ws", "use", "--name", "notes"]);
    assert_eq!(code, 4);
    assert!(stderr.contains("a") && stderr.contains("b"), "{stderr}");
}

#[test]
fn ws_prune_confirms_and_removes_only_the_workspaces_whose_folder_is_gone() {
    // "Stale" is a missing folder, and an unmounted drive looks exactly like a deleted one, so
    // this prompt is load-bearing rather than ceremonial.
    let vault = Vault::new();
    let doomed = vault.path().join("doomed");
    vault.run(&["ws", "new", doomed.to_str().unwrap()]);
    std::fs::remove_dir_all(&doomed).unwrap();

    vault
        .cmd()
        .args(["ws", "prune"])
        .write_stdin("n\n")
        .assert()
        .success()
        .stderr(predicates::str::contains("cancelled"));
    assert_eq!(vault.json(&["ws", "ls"]).as_array().unwrap().len(), 2);

    vault.run(&["ws", "prune", "--yes"]);
    let entries = vault.json(&["ws", "ls"]);
    assert_eq!(
        entries.as_array().unwrap().len(),
        1,
        "only the missing one goes"
    );
    assert_eq!(entries[0]["path"], vault.ws());
}

#[test]
fn the_test_suite_never_touches_the_real_registry() {
    // Guards the isolation every test registering a workspace depends on. Without `JOT_REGISTRY`
    // this passes on Linux and silently writes into the developer's `%APPDATA%` on Windows.
    let vault = Vault::new();
    let registry = vault.path().join("registry.toml");
    let text = std::fs::read_to_string(&registry).unwrap();
    assert!(text.contains(vault.ws()), "{text}");
    assert_eq!(vault.json(&["ws", "ls"]).as_array().unwrap().len(), 1);
}

#[test]
fn an_old_format_workspace_is_refused_with_what_changed() {
    let vault = Vault::new();
    let old = vault.path().join("old");
    std::fs::create_dir_all(old.join(".jot")).unwrap();
    std::fs::write(old.join(".jot/workspace.toml"), "schema_version = 1\n").unwrap();
    std::fs::write(
        old.join(format!("{HAND_ID}_a_slug.md")),
        "---\ntitle: kept\n---\n",
    )
    .unwrap();

    let assert = vault
        .cmd()
        .args(["--workspace", old.to_str().unwrap(), "ls"])
        .assert()
        .failure();
    let output = assert.get_output();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr.clone()).unwrap();
    for expected in [
        "old format",
        "workspace.toml",
        "schema.json",
        "no converter",
        "hint",
    ] {
        assert!(stderr.contains(expected), "no `{expected}` in:\n{stderr}");
    }
    // Discovery refuses it too, rather than walking past it to some other workspace.
    vault
        .cmd()
        .current_dir(&old)
        .arg("ls")
        .assert()
        .failure()
        .stderr(predicates::str::contains("old format"));
    assert!(
        !old.join("workspace.toml").exists(),
        "nothing was converted"
    );
}

// =============================================================================================
// Capture
// =============================================================================================

#[test]
fn new_writes_a_file_named_exactly_by_the_id_it_prints() {
    let vault = Vault::new();
    let id = vault.new_note(&["-m", "a thought"]);
    assert!(vault.note_file(&id).is_file(), "{id}");
}

#[test]
fn a_piped_body_becomes_a_note_and_an_empty_one_writes_nothing() {
    let vault = Vault::new();
    let output = vault
        .cmd()
        .args(["--workspace", vault.ws(), "--long", "new"])
        .write_stdin("captured from a pipe")
        .assert()
        .success();
    let id = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let file = std::fs::read_to_string(vault.note_file(id.trim())).unwrap();
    assert!(file.contains("captured from a pipe"), "{file}");

    vault
        .cmd()
        .args(["--workspace", vault.ws(), "new"])
        .write_stdin("   \n\n")
        .assert()
        .success()
        .stderr(predicates::str::contains("nothing written"));
    assert_eq!(count_notes(vault.path()), 1);
}

#[test]
fn a_title_alone_is_a_note_and_set_reads_its_value_as_yaml() {
    let vault = Vault::new();
    let id = vault.new_note(&["-t", "a thought", "--set", "n=3", "--set", "label=\"3\""]);
    let note = vault.json(&["show", &id]);
    assert_eq!(note["title"], "a thought");
    assert_eq!(note["fields"]["n"], 3);
    assert_eq!(note["fields"]["label"], "3");
    assert!(note["fields"]["created_at"].is_string(), "filled on create");
}

#[test]
fn a_note_that_fails_the_schema_is_refused_and_nothing_is_written() {
    let vault = Vault::new();
    let (code, stderr) = vault.fail(&["new", "-t", "x", "--set", "created_at=yesterday"]);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("schema.json") && stderr.contains("created_at"),
        "{stderr}"
    );
    assert_eq!(count_notes(vault.path()), 0);
}

#[test]
fn title_is_refused_clearly_when_no_title_role_is_mapped() {
    let vault = Vault::new();
    vault.settings(
        "",
        r#"{ "type": "object", "properties": { "name": { "type": "string" } } }"#,
    );
    let (code, stderr) = vault.fail(&["new", "-t", "x"]);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("document:title") && stderr.contains("--set"),
        "{stderr}"
    );
    assert_eq!(count_notes(vault.path()), 0);
}

// =============================================================================================
// Editing
// =============================================================================================

#[test]
fn edit_changes_one_field_and_keeps_the_body_and_an_undeclared_key() {
    let vault = Vault::new();
    let id = vault.new_note(&["-t", "before", "-m", "the body", "--set", "mood=calm"]);

    vault.run(&["edit", &id, "-t", "after"]);

    let note = vault.json(&["show", &id]);
    assert_eq!(note["title"], "after");
    assert_eq!(note["fields"]["mood"], "calm");
    assert!(
        note["body"].as_str().unwrap().contains("the body"),
        "{note}"
    );
}

/// The rule the redesign makes the CLI keep: jot never writes a note that fails the schema, and
/// never loses what was typed. The buffer is refused, reopened on the same text with the reasons
/// in it, and when the editor is quit without saving the draft is kept and its path printed.
#[test]
fn an_invalid_editor_buffer_is_refused_reopened_and_never_lost() {
    let vault = Vault::new();
    let id = vault.new_note(&["-t", "before"]);
    let before = std::fs::read_to_string(vault.note_file(&id)).unwrap();

    let typed =
        "---\ntitle: typed in the editor\ncreated_at: yesterday\n---\n\nwords worth keeping\n";
    let editor = ScriptedEditor::new(&vault.path().join("editor"), typed);
    let assert = vault
        .cmd()
        .env("EDITOR", &editor.program)
        .args(["--workspace", vault.ws(), "edit", &id])
        .assert()
        .failure();
    let output = assert.get_output();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr.clone()).unwrap();

    let reopened = editor.seen();
    assert!(
        reopened.contains("words worth keeping") && reopened.contains("created_at: yesterday"),
        "the editor reopened on what was typed:\n{reopened}"
    );
    assert!(
        reopened.contains("# jot:") && reopened.contains("`created_at`"),
        "with the reason in the buffer:\n{reopened}"
    );
    assert_eq!(
        std::fs::read_to_string(vault.note_file(&id)).unwrap(),
        before,
        "nothing was written"
    );

    let kept = stderr
        .split("kept at `")
        .nth(1)
        .and_then(|rest| rest.split('`').next())
        .unwrap_or_else(|| panic!("no kept draft named in:\n{stderr}"));
    let draft = std::fs::read_to_string(kept).unwrap();
    assert!(draft.contains("words worth keeping"), "{draft}");
    let _ = std::fs::remove_file(kept);
}

// =============================================================================================
// Reading
// =============================================================================================

#[test]
fn ls_and_show_json_have_the_documented_shape() {
    let vault = Vault::new();
    let id = vault.new_note(&["-t", "t", "-m", "the body", "--set", "extra=1"]);

    let listing = vault.json(&["ls"]);
    assert_eq!(listing["ordering"]["by"], "created_at");
    assert_eq!(listing["ordering"]["key"], "created_at");
    let row = &listing["notes"][0];
    assert_eq!(row["id"], id.as_str(), "always the full id");
    for key in ["id", "title", "created_at", "edited_at", "problems"] {
        assert!(row.get(key).is_some(), "row lacks `{key}`: {row}");
    }

    let note = vault.json(&["show", &id[..8]]);
    for key in [
        "id",
        "title",
        "created_at",
        "edited_at",
        "problems",
        "fields",
        "hash",
        "body",
    ] {
        assert!(note.get(key).is_some(), "note lacks `{key}`: {note}");
    }
    let keys: Vec<&str> = note["fields"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["title", "created_at", "extra"],
        "schema order, then the rest"
    );
    assert_eq!(note["problems"].as_array().unwrap().len(), 0);
}

#[test]
fn ls_says_once_that_it_is_ordered_by_mtime_when_no_created_at_role_is_mapped() {
    let vault = Vault::new();
    vault.settings(
        "\"document:title\" = \"title\"\n",
        r#"{ "type": "object", "properties": { "title": { "type": "string" } } }"#,
    );
    vault.new_note(&["-t", "one"]);
    vault.new_note(&["-t", "two"]);

    let stderr = vault.stderr(&["ls"]);
    assert_eq!(
        stderr.matches("modification time").count(),
        1,
        "said once:\n{stderr}"
    );
    let listing = vault.json(&["ls"]);
    assert_eq!(listing["ordering"]["by"], "edited_at");
    assert_eq!(listing["ordering"]["key"], Value::Null);
}

#[test]
fn show_puts_each_problem_under_its_field_and_lists_a_missing_required_one() {
    let vault = Vault::new();
    std::fs::write(vault.note_file(HAND_ID), "---\ntitle: 7\n---\nbody\n").unwrap();

    let shown = vault.run(&["show", HAND_ID]);
    let lines: Vec<&str> = shown.lines().collect();
    let at = |prefix: &str| {
        lines
            .iter()
            .position(|l| l.starts_with(prefix))
            .unwrap_or_else(|| panic!("no `{prefix}` in:\n{shown}"))
    };
    let title = at("title: 7");
    assert!(lines[title + 1].contains("! type"), "{shown}");
    let created = at("created_at: (missing)");
    assert!(lines[created + 1].contains("! required"), "{shown}");
    assert!(shown.ends_with("body\n"), "{shown}");
}

#[test]
fn check_lists_problems_and_strays_and_exits_five_only_for_problems() {
    let vault = Vault::new();
    vault.new_note(&["-t", "fine"]);
    std::fs::write(vault.path().join("README.md"), "not a note\n").unwrap();

    let assert = vault
        .cmd()
        .args(["--workspace", vault.ws(), "check"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("stray  README.md"),
        "a stray alone does not fail:\n{stdout}"
    );

    std::fs::write(
        vault.note_file(HAND_ID),
        "---\ncreated_at: yesterday\n---\n",
    )
    .unwrap();
    let assert = vault
        .cmd()
        .args(["--workspace", vault.ws(), "check"])
        .assert()
        .failure();
    assert_eq!(assert.get_output().status.code(), Some(5));
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("`created_at` (format)"), "{stdout}");
}

#[test]
fn rm_confirms_and_declining_keeps_the_note() {
    let vault = Vault::new();
    let id = vault.new_note(&["-t", "keep me"]);

    vault
        .cmd()
        .args(["--workspace", vault.ws(), "rm", &id])
        .write_stdin("n\n")
        .assert()
        .success()
        .stderr(predicates::str::contains("cancelled"));
    assert!(vault.note_file(&id).exists());

    vault
        .cmd()
        .args(["--workspace", vault.ws(), "rm", &id])
        .write_stdin("y\n")
        .assert()
        .success();
    assert!(!vault.note_file(&id).exists(), "the file is gone");
    assert_eq!(vault.code(&["show", &id]), 3);
}

// =============================================================================================
// Field batches
// =============================================================================================

#[test]
fn field_rename_rewrites_every_note_the_schema_and_the_role() {
    let vault = Vault::new();
    let a = vault.new_note(&["-t", "first"]);
    let b = vault.new_note(&["-t", "second"]);

    let stdout = vault.run(&["field", "rename", "title", "headline"]);
    assert!(
        stdout.contains(&a) && stdout.contains(&b),
        "each note named:\n{stdout}"
    );

    for id in [&a, &b] {
        let file = std::fs::read_to_string(vault.note_file(id)).unwrap();
        assert!(
            file.contains("headline: ") && !file.contains("title:"),
            "{file}"
        );
    }
    let schema = std::fs::read_to_string(vault.path().join("schema.json")).unwrap();
    assert!(
        schema.contains("\"headline\"") && !schema.contains("\"title\""),
        "{schema}"
    );
    let toml = std::fs::read_to_string(vault.path().join("workspace.toml")).unwrap();
    assert!(
        toml.contains("\"headline\""),
        "the role follows the key:\n{toml}"
    );

    // …so the title still reads, and nothing is left with a problem.
    let listing = vault.json(&["ls"]);
    let mut titles: Vec<&str> = listing["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["title"].as_str().unwrap())
        .collect();
    titles.sort_unstable();
    assert_eq!(titles, ["first", "second"]);
    vault.run(&["check"]);
}

#[test]
fn a_batch_that_would_leave_a_note_invalid_names_the_note_and_writes_nothing() {
    let vault = Vault::new();
    let fine = vault.new_note(&["-t", "fine"]);
    let before = std::fs::read_to_string(vault.note_file(&fine)).unwrap();
    // Invalid already, and still invalid once filled: the batch must not write around it.
    std::fs::write(
        vault.note_file(HAND_ID),
        "---\ncreated_at: yesterday\n---\n",
    )
    .unwrap();

    let (code, stderr) = vault.fail(&["field", "fill", "mood", "calm"]);
    assert_eq!(code, 1);
    assert!(stderr.contains(HAND_ID), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(vault.note_file(&fine)).unwrap(),
        before
    );
}

// =============================================================================================
// Exit codes and resolution
// =============================================================================================

#[test]
fn an_ambiguous_prefix_lists_the_candidates_and_exits_four() {
    let vault = Vault::new();
    // Captured together, so they share a prefix by construction: the ids are uuidv7.
    let first = vault.new_note(&["-t", "first"]);
    vault.new_note(&["-t", "second"]);

    let (code, stderr) = vault.fail(&["show", &first[..4]]);
    assert_eq!(code, 4);
    assert!(
        stderr.contains("first") && stderr.contains("second"),
        "{stderr}"
    );
    assert_eq!(vault.code(&["show", "ffffffff"]), 3, "not found");
    assert_eq!(vault.code(&["bogus"]), 2, "usage");
}

#[test]
fn a_printed_short_id_can_always_be_handed_straight_back() {
    // Ids captured together share a long prefix, so this fails immediately if a short id is cut
    // from the front of the id rather than showing its tail.
    let vault = Vault::new();
    for i in 0..12 {
        vault.new_note(&["-t", &format!("note {i}")]);
    }
    let listing = vault.jot(&["--workspace", vault.ws(), "ls"]);
    let shorts: Vec<&str> = listing
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert_eq!(shorts.len(), 12, "{listing}");
    for short in shorts {
        let (head, tail) = short
            .split_once("...")
            .unwrap_or_else(|| panic!("`{short}` is not head...tail"));
        assert_eq!(head.len(), 4, "`{short}`");
        assert!(tail.len() >= 4, "`{short}`'s tail is under the floor");
        vault
            .cmd()
            .args(["--workspace", vault.ws(), "show", short])
            .assert()
            .success();
    }
}

// =============================================================================================
// Workspace resolution
// =============================================================================================

#[test]
fn every_command_works_from_a_subdirectory_of_the_workspace() {
    let vault = Vault::new();
    let id = vault.new_note(&["-m", "a thought"]);
    let nested = vault.path().join("a/b/c");
    std::fs::create_dir_all(&nested).unwrap();

    vault
        .cmd()
        .current_dir(&nested)
        .args(["--long", "show", &id])
        .assert()
        .success()
        .stdout(predicates::str::contains("a thought"));
}

#[test]
fn the_env_var_selects_the_workspace_when_there_is_no_flag() {
    let vault = Vault::new();
    let id = vault.new_note(&["-m", "from the env"]);
    let elsewhere = tempfile::tempdir().unwrap();

    vault
        .cmd()
        .current_dir(elsewhere.path())
        .env("JOT_WORKSPACE", vault.path())
        .args(["--long", "show", &id])
        .assert()
        .success()
        .stdout(predicates::str::contains("from the env"));
}

#[test]
fn an_explicit_workspace_that_does_not_open_is_an_error_and_never_a_fallback() {
    // Silently dropping to the next rule is exactly how a note lands in the wrong workspace.
    let vault = Vault::new();
    let elsewhere = tempfile::tempdir().unwrap();

    vault
        .cmd()
        .env("JOT_WORKSPACE", elsewhere.path())
        .args(["new", "-m", "must not be captured"])
        .assert()
        .failure();
    assert_eq!(
        count_notes(vault.path()),
        0,
        "nothing landed in the workspace"
    );
}

#[test]
fn verbose_says_which_workspace_was_chosen_and_why() {
    let vault = Vault::new();
    assert!(
        vault
            .stderr(&["--verbose", "ls"])
            .contains("chosen by --workspace")
    );
}

// =============================================================================================
// Output plumbing
// =============================================================================================

#[test]
fn json_output_is_clean_even_when_a_warning_is_printed() {
    // Warnings go to stderr precisely so `jot ls --json | jq` keeps working.
    let vault = Vault::new();
    vault.new_note(&["-m", "fine"]);
    std::fs::write(vault.path().join("README.md"), "not a note\n").unwrap();

    let output = vault
        .cmd()
        .args(["--workspace", vault.ws(), "--json", "ls"])
        .assert()
        .success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    serde_json::from_str::<Value>(&stdout).expect("stdout is still valid JSON");
    let stderr = String::from_utf8(output.get_output().stderr.clone()).unwrap();
    assert!(stderr.contains("stray"), "the stray is reported: {stderr}");
}

#[test]
fn completions_generate_for_every_supported_shell() {
    let vault = Vault::new();
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let script = vault.jot(&["completions", shell]);
        assert!(!script.is_empty(), "{shell} produced nothing");
    }
}

#[test]
fn bare_jot_prints_help_rather_than_failing() {
    let vault = Vault::new();
    vault
        .cmd()
        .assert()
        .success()
        .stdout(predicates::str::contains("Usage:"));
}

// =============================================================================================
// Helpers
// =============================================================================================

/// The registry's current workspace, as JSON.
fn current(vault: &Vault) -> Value {
    vault
        .json(&["ws", "ls"])
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["current"] == true)
        .expect("a current workspace")
        .clone()
}

/// How many `*.md` files are in the workspace root.
fn count_notes(root: &Path) -> usize {
    std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "md"))
        .count()
}

/// A stand-in `$EDITOR`: its first run replaces the buffer with fixed text; every later run copies
/// the buffer it was handed to `seen.md` and saves nothing — a person reading the reasons and
/// quitting.
struct ScriptedEditor {
    dir: PathBuf,
    program: PathBuf,
}

impl ScriptedEditor {
    fn new(dir: &Path, first: &str) -> ScriptedEditor {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("first.md"), first).unwrap();
        #[cfg(windows)]
        let program = {
            let program = dir.join("editor.cmd");
            std::fs::write(
                &program,
                "@echo off\r\n\
                 if exist \"%~dp0ran\" goto again\r\n\
                 copy /y \"%~dp0first.md\" \"%~1\" >nul\r\n\
                 type nul > \"%~dp0ran\"\r\n\
                 exit /b 0\r\n\
                 :again\r\n\
                 copy /y \"%~1\" \"%~dp0seen.md\" >nul\r\n\
                 exit /b 0\r\n",
            )
            .unwrap();
            program
        };
        #[cfg(not(windows))]
        let program = {
            use std::os::unix::fs::PermissionsExt;
            let program = dir.join("editor.sh");
            std::fs::write(
                &program,
                "#!/bin/sh\n\
                 d=$(dirname \"$0\")\n\
                 if [ -e \"$d/ran\" ]; then cp \"$1\" \"$d/seen.md\"; \
                 else cp \"$d/first.md\" \"$1\"; touch \"$d/ran\"; fi\n",
            )
            .unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            program
        };
        ScriptedEditor {
            dir: dir.to_path_buf(),
            program,
        }
    }

    /// The buffer the editor was handed the second time.
    fn seen(&self) -> String {
        std::fs::read_to_string(self.dir.join("seen.md")).expect("the editor was reopened")
    }
}
