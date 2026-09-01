//! End-to-end tests: the real binary, against a real temp vault.
//!
//! These are the tests that would catch a command wired to the wrong core call, an exit code that
//! drifted, or a `--json` key that got renamed — none of which a unit test in either crate can see,
//! because each half is individually correct in every one of those failures.
//!
//! # Isolation
//!
//! Every test gets its own vault and passes `--workspace`, so nothing depends on the machine's
//! registry, on the working directory, or on another test. `NO_COLOR` is set for all of them so
//! assertions match plain bytes.

use assert_cmd::Command;
use serde_json::Value;
use std::path::Path;
use tempfile::TempDir;

/// A temp vault plus a builder for commands pointed at it.
struct Vault {
    dir: TempDir,
}

impl Vault {
    fn new() -> Vault {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault { dir };
        vault.jot(&["ws", "new", &vault.path().display().to_string()]);
        vault
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The command builder: isolated registry, no colour, this vault.
    fn cmd(&self) -> Command {
        let mut cmd = Command::cargo_bin("jot").unwrap();
        cmd.env("NO_COLOR", "1")
            .env_remove("JOT_WORKSPACE")
            // A registry write must never touch the developer's real one, and `JOT_REGISTRY` is
            // the only thing that guarantees it. `XDG_CONFIG_HOME`/`HOME` were tried first and
            // isolate on Linux only: `directories` reads `FOLDERID_RoamingAppData` through a
            // syscall on Windows, so every test in this file was writing into the developer's
            // real `%APPDATA%` registry — and seeing the other tests' workspaces in `ws ls`.
            .env("JOT_REGISTRY", self.dir.path().join("registry.toml"))
            .current_dir(self.dir.path());
        cmd
    }

    /// Run a command that must succeed; returns stdout.
    fn jot(&self, args: &[&str]) -> String {
        let output = self.cmd().args(args).assert().success();
        String::from_utf8(output.get_output().stdout.clone()).unwrap()
    }

    /// Run a command against this vault explicitly, with full UUIDs.
    ///
    /// `--long` by default so assertions can name an id without depending on how wide the
    /// abbreviation happens to be. Use [`Vault::run_short`] to exercise the abbreviated form.
    fn run(&self, args: &[&str]) -> String {
        let mut full = vec!["--workspace", self.path().to_str().unwrap(), "--long"];
        full.extend_from_slice(args);
        self.jot(&full)
    }

    /// Run a command against this vault with ids abbreviated, as a person sees them.
    fn run_short(&self, args: &[&str]) -> String {
        let mut full = vec!["--workspace", self.path().to_str().unwrap()];
        full.extend_from_slice(args);
        self.jot(&full)
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

    /// The exit code of a command expected to fail.
    fn code(&self, args: &[&str]) -> i32 {
        let mut full = vec!["--workspace", self.path().to_str().unwrap()];
        full.extend_from_slice(args);
        self.cmd()
            .args(&full)
            .assert()
            .failure()
            .get_output()
            .status
            .code()
            .unwrap()
    }
}

// =============================================================================================
// Workspaces
// =============================================================================================

#[test]
fn a_workspace_id_is_a_uuid_v4_so_short_ids_stay_short() {
    // Unlike a note id: nothing decodes a workspace's creation time or sorts on it, and a v7's
    // timestamp prefix would make two workspaces created in one minute share eight characters.
    let vault = Vault::new();
    let id = vault.json(&["ws", "ls"])[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(&id[14..15], "4", "version nibble of {id}");
}

#[test]
fn ws_new_creates_the_documented_tree_and_nothing_else() {
    let vault = Vault::new();
    for expected in [".jot", ".jot/workspace.toml", ".jot/.trash", ".jot/tmp"] {
        assert!(vault.path().join(expected).exists(), "missing `{expected}`");
    }
    // The index is derived and disposable; creating an empty one would be a lie about that.
    assert!(!vault.path().join(".jot/index.db").exists());
}

#[test]
fn ws_new_on_an_existing_workspace_refuses_rather_than_overwriting() {
    let vault = Vault::new();
    let code = vault.code(&["ws", "new", vault.path().to_str().unwrap()]);
    assert_eq!(code, 1);
    // The original is untouched.
    assert!(vault.path().join(".jot/workspace.toml").exists());
}

#[test]
fn every_command_answers_to_its_full_word_and_its_short_alias() {
    // The short forms are what the tests above use throughout, so this is the other half: the
    // full word is the real name and must work everywhere the alias does.
    let vault = Vault::new();
    vault.new_note(&["-m", "a thought"]);

    assert_eq!(vault.run(&["list"]), vault.run(&["ls"]));

    // `remove` and its alias both trash, so they need separate notes.
    for name in ["remove", "rm"] {
        let id = vault.new_note(&["-m", "temporary"]);
        vault.run(&[name, &id]);
        assert_eq!(
            vault.json(&["show", &id])["state"],
            "trashed",
            "`{name}` must trash"
        );
    }

    // And nested, on both halves independently: `workspace`/`ws` and `list`/`ls`.
    let expected = vault.run(&["ws", "ls"]);
    for spelling in [["workspace", "list"], ["workspace", "ls"], ["ws", "list"]] {
        assert_eq!(vault.run(&spelling), expected, "{spelling:?}");
    }

    // A group with no subcommand is a usage error, which is clap's behaviour and stays so.
    assert_eq!(vault.code(&["workspace"]), 2);
}

#[test]
fn ws_remove_unregisters_without_touching_the_directory() {
    let vault = Vault::new();
    let other = vault.dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    vault.run(&["ws", "new", other.to_str().unwrap()]);
    assert_eq!(vault.json(&["ws", "ls"]).as_array().unwrap().len(), 2);

    vault.run(&["workspace", "remove", "--name", "other"]);

    assert_eq!(vault.json(&["ws", "ls"]).as_array().unwrap().len(), 1);
    // The whole point: unregistering is not deleting.
    assert!(other.join(".jot").is_dir(), "the vault survives");
}

#[test]
fn removing_the_current_workspace_clears_current_rather_than_dangling() {
    // `Registry::remove` leaves a dangling `current` by design, so the surface has to notice.
    let vault = Vault::new();
    let other = vault.dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    vault.run(&["ws", "new", other.to_str().unwrap()]);
    assert_eq!(current(&vault)["name"], "other");

    vault.run(&["ws", "rm", "--name", "other"]);

    let entries = vault.json(&["ws", "ls"]);
    assert!(
        entries
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["current"] == false),
        "no entry may still be marked current: {entries}"
    );
}

#[test]
fn ws_remove_accepts_an_id_and_rejects_an_unknown_target() {
    let vault = Vault::new();
    let id = vault.json(&["ws", "ls"])[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    assert_eq!(vault.code(&["ws", "rm", "ffffffff"]), 3);
    assert_eq!(vault.code(&["ws", "rm", "--name", "no-such-thing"]), 3);
    // A bare argument is an id, here a prefix of one.
    vault.run(&["ws", "rm", &id[..8]]);
    assert!(vault.json(&["ws", "ls"]).as_array().unwrap().is_empty());
}

#[test]
fn ws_prune_removes_only_the_workspaces_whose_directory_is_gone() {
    let vault = Vault::new();
    let doomed = vault.dir.path().join("doomed");
    std::fs::create_dir_all(&doomed).unwrap();
    vault.run(&["ws", "new", doomed.to_str().unwrap()]);
    std::fs::remove_dir_all(&doomed).unwrap();
    assert_eq!(vault.json(&["ws", "ls"]).as_array().unwrap().len(), 2);

    vault.run(&["ws", "prune", "--yes"]);

    let entries = vault.json(&["ws", "ls"]);
    let entries = entries.as_array().unwrap();
    assert_eq!(entries.len(), 1, "only the missing one goes");
    assert_eq!(entries[0]["path"], vault.path().to_str().unwrap());
}

#[test]
fn ws_prune_confirms_first_and_declining_keeps_everything() {
    // "Stale" is `!path.exists()`, and an unmounted drive looks exactly like a deleted vault, so
    // this prompt is load-bearing rather than ceremonial.
    let vault = Vault::new();
    let doomed = vault.dir.path().join("doomed");
    std::fs::create_dir_all(&doomed).unwrap();
    vault.run(&["ws", "new", doomed.to_str().unwrap()]);
    std::fs::remove_dir_all(&doomed).unwrap();

    vault
        .cmd()
        .args(["--workspace", vault.path().to_str().unwrap(), "ws", "prune"])
        .write_stdin("n\n")
        .assert()
        .success()
        .stderr(predicates::str::contains("cancelled"));

    assert_eq!(vault.json(&["ws", "ls"]).as_array().unwrap().len(), 2);
}

#[test]
fn ws_prune_on_a_healthy_registry_does_nothing_and_says_so() {
    let vault = Vault::new();
    vault
        .cmd()
        .args(["--workspace", vault.path().to_str().unwrap(), "ws", "prune"])
        .assert()
        .success()
        .stderr(predicates::str::contains("nothing to prune"));
    assert_eq!(vault.json(&["ws", "ls"]).as_array().unwrap().len(), 1);
}

#[test]
fn ws_ls_marks_the_current_workspace() {
    let vault = Vault::new();
    let listing = vault.run(&["ws", "ls"]);
    assert!(listing.contains('*'), "{listing}");
}

#[test]
fn ws_ls_leads_with_an_id_the_way_ls_does() {
    let vault = Vault::new();
    let listing = vault.run_short(&["ws", "ls"]);
    let row = listing.lines().next().unwrap();

    let id = vault.json(&["ws", "ls"])[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let short = row_id(row);
    assert!(id.starts_with(short), "`{short}` is not a prefix of {id}");
    assert!(row.contains(vault.path().to_str().unwrap()), "{row}");
    assert!(row.contains(vault.path().file_name().unwrap().to_str().unwrap()));
}

#[test]
fn two_workspaces_with_one_name_are_told_apart_by_their_ids() {
    // The registry keys on workspace id, so two vaults can share a name — a `notes` directory
    // under two parents, or one deleted and remade. Without an id the rows are indistinguishable,
    // which is the confusion this listing format exists to remove.
    let vault = Vault::new();
    let mut paths = Vec::new();
    for parent in ["a", "b"] {
        let path = vault.dir.path().join(parent).join("notes");
        std::fs::create_dir_all(&path).unwrap();
        vault.run(&["ws", "new", path.to_str().unwrap()]);
        paths.push(path);
    }

    let listing = vault.run_short(&["ws", "ls"]);
    let named: Vec<&str> = listing
        .lines()
        .filter(|line| line.contains("  notes  "))
        .collect();
    assert_eq!(named.len(), 2, "both vaults are named `notes`:\n{listing}");

    let ids: Vec<&str> = named.iter().map(|line| row_id(line)).collect();
    assert_ne!(ids[0], ids[1], "the two rows must be distinguishable");
    for (id, path) in ids.iter().zip(&paths) {
        assert!(listing.contains(path.to_str().unwrap()), "{listing}");
        assert!(!id.is_empty());
    }
}

#[test]
fn the_test_suite_never_touches_the_real_registry() {
    // Guards the isolation the two tests above depend on, and that every test registering a
    // workspace depends on. Without `JOT_REGISTRY` this passes on Linux and silently writes into
    // the developer's `%APPDATA%` on Windows.
    let vault = Vault::new();
    let registry = vault.dir.path().join("registry.toml");
    assert!(registry.is_file(), "`ws new` wrote the redirected registry");

    let text = std::fs::read_to_string(&registry).unwrap();
    assert!(text.contains(vault.path().to_str().unwrap()), "{text}");

    // And the listing shows this vault alone, not whatever else the machine has registered.
    assert_eq!(vault.json(&["ws", "ls"]).as_array().unwrap().len(), 1);
}

#[test]
fn ws_use_selects_by_id_by_default_and_by_name_only_when_asked() {
    let vault = Vault::new();
    let other = vault.dir.path().join("other");
    std::fs::create_dir_all(&other).unwrap();
    vault.run(&["ws", "new", other.to_str().unwrap()]);

    let entries = vault.json(&["ws", "ls"]);
    let first = entries.as_array().unwrap()[0].clone();
    let id = first["id"].as_str().unwrap();
    let name = first["name"].as_str().unwrap();

    let away = |v: &Vault| v.run(&["ws", "use", "--name", "other"]);

    // A bare argument is an id.
    away(&vault);
    vault.run(&["ws", "use", id]);
    assert_eq!(current(&vault)["id"], id);

    // …and so is `--id`, with a prefix.
    away(&vault);
    vault.run(&["ws", "use", "--id", &id[..8]]);
    assert_eq!(current(&vault)["id"], id);

    // A name is only ever reached through `--name`.
    away(&vault);
    vault.run(&["ws", "use", "--name", name]);
    assert_eq!(current(&vault)["id"], id);
}

#[test]
fn a_name_passed_as_a_bare_argument_is_not_looked_up_as_a_name() {
    // The point of the change: an argument's *meaning* must not depend on what is registered.
    // `notes` is a real workspace name here and still fails, because a bare argument is an id.
    let vault = Vault::new();
    let path = vault.dir.path().join("notes");
    std::fs::create_dir_all(&path).unwrap();
    vault.run(&["ws", "new", path.to_str().unwrap()]);

    assert_eq!(vault.code(&["ws", "use", "notes"]), 3);
    vault.run(&["ws", "use", "--name", "notes"]);
    assert_eq!(current(&vault)["name"], "notes");
}

#[test]
fn ws_use_refuses_both_a_name_and_an_id_and_refuses_neither() {
    let vault = Vault::new();
    // clap's own XOR group: usage errors, exit 2.
    assert_eq!(vault.code(&["ws", "use", "--id", "x", "--name", "y"]), 2);
    assert_eq!(vault.code(&["ws", "use"]), 2);
}

#[test]
fn ws_use_can_reach_a_workspace_whose_name_is_not_unique() {
    // The gap this closes: matching on name alone made the second `notes` unreachable.
    let vault = Vault::new();
    let mut ids = Vec::new();
    for parent in ["a", "b"] {
        let path = vault.dir.path().join(parent).join("notes");
        std::fs::create_dir_all(&path).unwrap();
        vault.run(&["ws", "new", path.to_str().unwrap()]);
        ids.push(current(&vault)["id"].as_str().unwrap().to_owned());
    }

    // The shared name is refused rather than guessed at.
    assert_eq!(vault.code(&["ws", "use", "--name", "notes"]), 4);
    // And as a bare argument it is an id, which nothing matches — not a silent fallback to a name.
    assert_eq!(vault.code(&["ws", "use", "notes"]), 3);

    // Either one is reachable by id.
    for id in &ids {
        vault.run(&["ws", "use", id]);
        assert_eq!(current(&vault)["id"].as_str().unwrap(), id);
    }
}

#[test]
fn ws_use_on_something_unknown_exits_not_found() {
    let vault = Vault::new();
    assert_eq!(vault.code(&["ws", "use", "ffffffff"]), 3);
    assert_eq!(vault.code(&["ws", "use", "--name", "no-such-workspace"]), 3);
}

#[test]
fn ws_ls_long_prints_full_workspace_uuids() {
    let vault = Vault::new();
    let listing = vault.run(&["ws", "ls"]);
    let id = vault.json(&["ws", "ls"])[0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(listing.contains(&id), "{listing}");
}

// =============================================================================================
// Capture
// =============================================================================================

#[test]
fn new_writes_a_file_named_by_the_id_it_prints() {
    let vault = Vault::new();
    let id = vault.new_note(&["-m", "a thought"]);
    assert!(vault.path().join(format!("{id}.md")).exists(), "{id}");
}

#[test]
fn a_piped_body_becomes_a_note() {
    let vault = Vault::new();
    let output = vault
        .cmd()
        .args([
            "--workspace",
            vault.path().to_str().unwrap(),
            "--long",
            "new",
        ])
        .write_stdin("captured from a pipe")
        .assert()
        .success();
    let id = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let id = id.trim();

    let body = std::fs::read_to_string(vault.path().join(format!("{id}.md"))).unwrap();
    assert!(body.contains("captured from a pipe"), "{body}");
}

#[test]
fn an_empty_piped_body_writes_nothing() {
    let vault = Vault::new();
    vault
        .cmd()
        .args(["--workspace", vault.path().to_str().unwrap(), "new"])
        .write_stdin("   \n\n")
        .assert()
        .success()
        .stderr(predicates::str::contains("nothing written"));

    assert_eq!(count_notes(vault.path()), 0);
}

#[test]
fn a_reply_gets_its_parents_thread_root() {
    let vault = Vault::new();
    let root = vault.new_note(&["-m", "root"]);
    let reply = vault.new_note(&["--reply", &root, "-m", "reply"]);
    let deep = vault.new_note(&["--reply", &reply, "-m", "deep"]);

    let rows = vault.json(&["ls", "--flat"]);
    let row = |id: &str| {
        rows.as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(row(&deep)["root"], root.as_str());
    assert_eq!(row(&deep)["reply_to"], reply.as_str());
}

#[test]
fn replying_to_a_note_that_does_not_exist_exits_not_found() {
    let vault = Vault::new();
    vault.new_note(&["-m", "root"]);
    assert_eq!(vault.code(&["new", "--reply", "ffffffff", "-m", "x"]), 3);
}

#[test]
fn the_slug_flag_puts_the_title_in_the_filename() {
    let vault = Vault::new();
    let id = vault.new_note(&["-t", "Hello There", "-m", "b", "--slug"]);
    assert!(vault.path().join(format!("{id}_hello_there.md")).exists());
}

// =============================================================================================
// Reading
// =============================================================================================

#[test]
fn ls_shows_roots_and_flat_shows_everything() {
    let vault = Vault::new();
    let root = vault.new_note(&["-m", "root"]);
    vault.new_note(&["--reply", &root, "-m", "reply"]);

    assert_eq!(vault.json(&["ls"]).as_array().unwrap().len(), 1);
    assert_eq!(vault.json(&["ls", "--flat"]).as_array().unwrap().len(), 2);
}

#[test]
fn an_untitled_note_reads_as_untitled() {
    let vault = Vault::new();
    vault.new_note(&["-m", "no title here"]);
    assert!(vault.run(&["ls"]).contains("Untitled"));
}

#[test]
fn show_prints_the_body_and_raw_prints_the_file() {
    let vault = Vault::new();
    let id = vault.new_note(&["-t", "A title", "-m", "the body"]);

    let human = vault.run(&["show", &id]);
    assert!(
        human.contains("A title") && human.contains("the body"),
        "{human}"
    );

    let raw = vault.run(&["show", &id, "--raw"]);
    assert!(raw.starts_with("---\n"), "{raw}");
    assert!(raw.contains("relation:root:"), "{raw}");
}

#[test]
fn thread_renders_the_worked_example_from_the_plan() {
    // `A→B`, `B→C`, `C→D`, `C→E`, `A→F` — the shape `stage3.md` draws.
    let vault = Vault::new();
    let a = vault.new_note(&["-t", "A", "-m", "a"]);
    let b = vault.new_note(&["--reply", &a, "-t", "B", "-m", "b"]);
    let c = vault.new_note(&["--reply", &b, "-t", "C", "-m", "c"]);
    let d = vault.new_note(&["--reply", &c, "-t", "D", "-m", "d"]);
    let e = vault.new_note(&["--reply", &c, "-t", "E", "-m", "e"]);
    let f = vault.new_note(&["--reply", &a, "-t", "F", "-m", "f"]);

    let json = vault.json(&["thread", &a]);
    let ids = |value: &Value| {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                row.as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_owned())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    };

    assert_eq!(
        ids(&json["paths"]),
        vec![
            vec![a.clone(), b.clone(), c.clone(), d.clone()],
            vec![a.clone(), b.clone(), c.clone(), e.clone()],
            vec![a.clone(), f.clone()],
        ]
    );
    assert_eq!(
        ids(&json["segments"]),
        vec![
            vec![a.clone(), b.clone(), c.clone()],
            vec![a.clone(), f.clone()],
            vec![c.clone(), d.clone()],
            vec![c.clone(), e.clone()],
        ]
    );

    // And the human tree draws the same shape.
    let tree = vault.run(&["thread", &a]);
    assert!(tree.contains("├─"), "{tree}");
    assert!(tree.contains("└─"), "{tree}");
}

#[test]
fn thread_path_walks_from_the_root_to_the_focus() {
    let vault = Vault::new();
    let a = vault.new_note(&["-m", "a"]);
    let b = vault.new_note(&["--reply", &a, "-m", "b"]);
    let c = vault.new_note(&["--reply", &b, "-m", "c"]);

    let path = vault.run(&["thread", &c, "--path"]);
    assert_eq!(path.trim(), format!("{a} → {b} → {c}"));
}

#[test]
fn search_matches_titles_and_says_so_when_nothing_does() {
    let vault = Vault::new();
    vault.new_note(&["-t", "Findable", "-m", "x"]);
    vault.new_note(&["-t", "Other", "-m", "y"]);

    assert_eq!(vault.json(&["search", "find"]).as_array().unwrap().len(), 1);
    assert!(
        vault
            .json(&["search", "nope"])
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn links_reports_out_in_and_quoted_by() {
    let vault = Vault::new();
    let target = vault.new_note(&["-t", "Target", "-m", "x"]);
    let linker = vault.new_note(&["-m", &format!("see [[{target}]]")]);
    let quoter = vault.new_note(&["--quote", &target, "-m", "quoting"]);

    let json = vault.json(&["links", &target]);
    assert_eq!(json["links_in"][0]["id"], linker.as_str());
    assert_eq!(json["quoted_by"][0]["id"], quoter.as_str());

    let outgoing = vault.json(&["links", &linker]);
    assert_eq!(outgoing["links_out"][0]["target"], target.as_str());
    assert_eq!(outgoing["links_out"][0]["state"], "present");
}

#[test]
fn a_link_in_a_code_fence_is_not_a_link() {
    let vault = Vault::new();
    let target = vault.new_note(&["-m", "target"]);
    let linker = vault.new_note(&["-m", &format!("```\n[[{target}]]\n```")]);

    let json = vault.json(&["links", &linker]);
    assert!(json["links_out"].as_array().unwrap().is_empty(), "{json}");
}

// =============================================================================================
// Editing
// =============================================================================================

#[test]
fn edit_changes_a_title_without_touching_the_body() {
    let vault = Vault::new();
    let id = vault.new_note(&["-t", "before", "-m", "the body"]);

    vault.run(&["edit", &id, "-t", "after"]);

    let json = vault.json(&["show", &id]);
    assert_eq!(json["title"], "after");
    assert!(json["body"].as_str().unwrap().contains("the body"));
}

#[test]
fn edit_can_remove_a_title() {
    let vault = Vault::new();
    let id = vault.new_note(&["-t", "doomed", "-m", "b"]);

    vault.run(&["edit", &id, "--no-title"]);
    assert!(vault.json(&["show", &id])["title"].is_null());
}

#[test]
fn an_unknown_frontmatter_key_survives_an_edit() {
    let vault = Vault::new();
    let id = vault.new_note(&["-t", "t", "-m", "b"]);
    let path = vault.path().join(format!("{id}.md"));

    let original = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        original.replace("title: t", "title: t\nsource: obsidian-import"),
    )
    .unwrap();

    vault.run(&["edit", &id, "-t", "retitled"]);

    let after = std::fs::read_to_string(&path).unwrap();
    assert!(after.contains("source: obsidian-import"), "{after}");
    assert!(after.contains("title: retitled"), "{after}");
}

// =============================================================================================
// The delete lifecycle
// =============================================================================================

#[test]
fn rm_hides_a_note_trash_shows_it_and_restore_brings_it_back() {
    let vault = Vault::new();
    let id = vault.new_note(&["-t", "temporary", "-m", "b"]);

    vault.run(&["rm", &id]);
    assert!(vault.json(&["ls", "--flat"]).as_array().unwrap().is_empty());
    assert_eq!(vault.json(&["trash"]).as_array().unwrap().len(), 1);
    assert!(
        vault
            .path()
            .join(".jot/.trash")
            .join(format!("{id}.md"))
            .exists()
    );

    vault.run(&["restore", &id]);
    assert_eq!(vault.json(&["ls", "--flat"]).as_array().unwrap().len(), 1);
    assert!(vault.json(&["trash"]).as_array().unwrap().is_empty());
    assert!(vault.path().join(format!("{id}.md")).exists());
}

#[test]
fn a_trash_and_restore_round_trip_leaves_the_file_byte_identical() {
    let vault = Vault::new();
    let id = vault.new_note(&["-t", "t", "-m", "b"]);
    let path = vault.path().join(format!("{id}.md"));
    let before = std::fs::read(&path).unwrap();

    vault.run(&["rm", &id]);
    vault.run(&["restore", &id]);

    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn rm_never_cascades() {
    let vault = Vault::new();
    let parent = vault.new_note(&["-m", "parent"]);
    let child = vault.new_note(&["--reply", &parent, "-m", "child"]);

    vault.run(&["rm", &parent]);

    let rows = vault.json(&["ls", "--flat"]);
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1, "the child is still live");
    assert_eq!(rows[0]["id"], child.as_str());
    assert_eq!(rows[0]["parent"]["state"], "trashed");
}

#[test]
fn purge_requires_confirmation_and_declining_keeps_the_note() {
    let vault = Vault::new();
    let id = vault.new_note(&["-m", "keep me"]);

    vault
        .cmd()
        .args(["--workspace", vault.path().to_str().unwrap(), "purge", &id])
        .write_stdin("n\n")
        .assert()
        .success()
        .stderr(predicates::str::contains("cancelled"));

    assert!(vault.path().join(format!("{id}.md")).exists());
}

#[test]
fn purge_with_yes_removes_the_file_and_leaves_the_children_grouped() {
    let vault = Vault::new();
    let root = vault.new_note(&["-m", "root"]);
    let mid = vault.new_note(&["--reply", &root, "-m", "mid"]);
    let leaf = vault.new_note(&["--reply", &mid, "-m", "leaf"]);

    vault.run(&["purge", &mid, "--yes"]);

    assert!(!vault.path().join(format!("{mid}.md")).exists());
    let rows = vault.json(&["ls", "--flat"]);
    let leaf_row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == leaf.as_str())
        .unwrap();
    assert_eq!(leaf_row["parent"]["state"], "deleted");
    assert_eq!(leaf_row["root"], root.as_str(), "still grouped");
}

#[test]
fn a_note_whose_parent_was_purged_shows_up_in_the_timeline_as_a_root() {
    let vault = Vault::new();
    let root = vault.new_note(&["-m", "root"]);
    let child = vault.new_note(&["--reply", &root, "-m", "child"]);

    vault.run(&["purge", &root, "--yes"]);

    let roots = vault.json(&["ls"]);
    assert_eq!(roots.as_array().unwrap().len(), 1);
    assert_eq!(roots[0]["id"], child.as_str());
}

// =============================================================================================
// Exit codes and resolution
// =============================================================================================

#[test]
fn exit_codes_are_the_documented_ones() {
    let vault = Vault::new();
    vault.new_note(&["-m", "one"]);

    assert_eq!(vault.code(&["show", "ffffffff"]), 3, "not found");
    assert_eq!(vault.code(&["bogus"]), 2, "usage");
    vault
        .cmd()
        .args(["--workspace", vault.path().to_str().unwrap(), "ls"])
        .assert()
        .success();
}

#[test]
fn an_ambiguous_prefix_lists_the_candidates_and_exits_four() {
    let vault = Vault::new();
    // Captured together, so they share a long timestamp prefix by construction.
    vault.new_note(&["-t", "first", "-m", "a"]);
    vault.new_note(&["-t", "second", "-m", "b"]);

    let assertion = vault
        .cmd()
        .args([
            "--workspace",
            vault.path().to_str().unwrap(),
            "show",
            "01a0",
        ])
        .assert()
        .failure();
    assert_eq!(assertion.get_output().status.code(), Some(4));

    let stderr = String::from_utf8(assertion.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("first") && stderr.contains("second"),
        "{stderr}"
    );
}

#[test]
fn a_printed_short_id_can_always_be_handed_straight_back() {
    // The property that makes short ids usable at all. Ids captured together share a long prefix,
    // so this fails immediately if the abbreviation width is fixed rather than computed.
    let vault = Vault::new();
    for i in 0..12 {
        vault.new_note(&["-t", &format!("note {i}"), "-m", "x"]);
    }

    let listing = vault.jot(&[
        "--workspace",
        vault.path().to_str().unwrap(),
        "ls",
        "--flat",
    ]);
    let shorts: Vec<&str> = listing
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    assert_eq!(shorts.len(), 12);

    for short in shorts {
        vault
            .cmd()
            .args(["--workspace", vault.path().to_str().unwrap(), "show", short])
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

    // No `--workspace`: discovery has to walk up and find `.jot/`.
    let output = vault
        .cmd()
        .current_dir(&nested)
        .args(["--long", "show", &id])
        .assert()
        .success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("a thought"), "{stdout}");
}

#[test]
fn the_env_var_selects_the_workspace_when_there_is_no_flag() {
    let vault = Vault::new();
    let id = vault.new_note(&["-m", "from the env"]);
    let elsewhere = tempfile::tempdir().unwrap();

    let output = vault
        .cmd()
        .current_dir(elsewhere.path())
        .env("JOT_WORKSPACE", vault.path())
        .args(["--long", "show", &id])
        .assert()
        .success();
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("from the env"), "{stdout}");
}

#[test]
fn an_explicit_workspace_that_does_not_open_is_an_error_and_never_a_fallback() {
    // Silently dropping to the next rule is exactly how a note lands in the wrong vault.
    let vault = Vault::new();
    let elsewhere = tempfile::tempdir().unwrap();

    vault
        .cmd()
        .env("JOT_WORKSPACE", elsewhere.path())
        .args(["new", "-m", "must not be captured"])
        .assert()
        .failure();

    assert_eq!(count_notes(vault.path()), 0, "nothing landed in the vault");
}

#[test]
fn verbose_says_which_workspace_was_chosen_and_why() {
    let vault = Vault::new();
    vault
        .cmd()
        .args([
            "--workspace",
            vault.path().to_str().unwrap(),
            "--verbose",
            "ls",
        ])
        .assert()
        .success()
        .stderr(predicates::str::contains("chosen by --workspace"));
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
        .args([
            "--workspace",
            vault.path().to_str().unwrap(),
            "--json",
            "ls",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    serde_json::from_str::<Value>(&stdout).expect("stdout is still valid JSON");
    let stderr = String::from_utf8(output.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("README.md"),
        "the problem is reported: {stderr}"
    );
}

#[test]
fn index_status_counts_notes_and_reports_problems() {
    let vault = Vault::new();
    vault.new_note(&["-m", "one"]);
    let id = vault.new_note(&["-m", "two"]);
    vault.run(&["rm", &id]);

    let json = vault.json(&["index", "status"]);
    assert_eq!(json["active"], 1);
    assert_eq!(json["trashed"], 1);
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

/// The id token of a `jot ws ls` row, skipping the `*` current-workspace marker.
fn row_id(row: &str) -> &str {
    row.split_whitespace()
        .find(|token| *token != "*")
        .expect("every row leads with an id")
}

/// How many notes are in the vault root.
fn count_notes(root: &Path) -> usize {
    std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "md"))
        .count()
}
