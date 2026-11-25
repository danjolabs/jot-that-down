//! `jot` — the command-line surface.
//!
//! # The seam
//!
//! This crate contains **no domain logic**. It parses arguments, chooses a workspace, calls
//! `jot-core`, and formats what comes back. It never opens a note file, never walks the workspace,
//! and never decides whether a note is valid: every write is a core [`Action`], and what happened
//! comes back as an [`Outcome`]. If a command here seems to need a new rule, the rule belongs in
//! core.
//!
//! The one deliberate exception is [`editor`], which writes a scratch file outside the workspace so
//! that `$EDITOR` has something to open. What comes back still goes through core's write path.
//!
//! # Exit codes
//!
//! Fixed, because scripts depend on them:
//!
//! | Code | Meaning |
//! | --- | --- |
//! | 0 | success |
//! | 1 | runtime error, including a write core refused |
//! | 2 | usage error (clap's own) |
//! | 3 | no such note or workspace |
//! | 4 | ambiguous id prefix or workspace name |
//! | 5 | `jot check` found problems |

mod context;
mod editor;
mod output;

use anyhow::{Context as _, anyhow};
use clap::{CommandFactory, Parser, Subcommand};
use context::Context;
use editor::{Composed, Verdict};
use jot_core::registry::{Entry, Registry};
use jot_core::snapshot::Resolution;
use jot_core::{
    Action, Document, Event, Failure, Fields, NoteId, Outcome, Problem, Rejection, Workspace,
};
use output::{MIN_ID_WIDTH, Style};
use serde_json::{Value, json};
use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

/// Exit code for "the thing you named does not exist".
const EXIT_NOT_FOUND: u8 = 3;
/// Exit code for "what you named matches more than one thing".
const EXIT_AMBIGUOUS: u8 = 4;
/// Exit code for "`jot check` found something wrong".
const EXIT_PROBLEMS: u8 = 5;

/// A failure that knows which exit code it deserves.
struct Exit {
    error: anyhow::Error,
    code: u8,
}

impl Exit {
    fn runtime(error: anyhow::Error) -> Exit {
        Exit { error, code: 1 }
    }

    fn not_found(error: anyhow::Error) -> Exit {
        Exit {
            error,
            code: EXIT_NOT_FOUND,
        }
    }
}

impl From<anyhow::Error> for Exit {
    fn from(error: anyhow::Error) -> Exit {
        Exit::runtime(error)
    }
}

impl From<jot_core::Error> for Exit {
    fn from(error: jot_core::Error) -> Exit {
        Exit::runtime(error.into())
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(exit) => {
            // An exit with an empty message has already said everything on stderr.
            let message = format!("{:#}", exit.error);
            if !message.is_empty() {
                eprintln!("jot: {message}");
            }
            ExitCode::from(exit.code)
        }
    }
}

// =============================================================================================
// The command tree
// =============================================================================================

#[derive(Parser)]
#[command(
    name = "jot",
    version,
    about = "Capture a thought before it goes.",
    long_about = "jot — a personal capture tool over a folder of markdown files.\n\n\
                  Each file is a note named by its id; its frontmatter is typed by the \
                  workspace's schema.json, so the folder reads like a table that works offline."
)]
struct Cli {
    /// Act on the workspace at this path, overriding discovery.
    #[arg(long, global = true, value_name = "PATH")]
    workspace: Option<PathBuf>,

    /// Emit JSON instead of human-readable output.
    #[arg(long, global = true)]
    json: bool,

    /// Print full ids instead of short ones.
    #[arg(long, global = true)]
    long: bool,

    /// Never emit colour, whatever the terminal says.
    #[arg(long, global = true)]
    no_color: bool,

    /// Explain which workspace was chosen and why.
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Write a new note.
    New(NewArgs),
    /// List notes, newest first.
    #[command(visible_alias = "ls")]
    List(ListArgs),
    /// Print one note in full, with its problems beside its fields.
    Show(ShowArgs),
    /// Change a note's frontmatter or body.
    Edit(EditArgs),
    /// Delete a note's file. Irreversible; confirms.
    #[command(visible_alias = "rm")]
    Remove(RemoveArgs),
    /// Find notes whose frontmatter text contains a string.
    Search(SearchArgs),
    /// Show a note's links and the notes linking to it.
    Links(IdArgs),
    /// Change one frontmatter key in every note at once.
    #[command(subcommand)]
    Field(FieldCommand),
    /// List notes with problems, stray files, and settings errors.
    Check,
    /// Manage workspaces.
    #[command(subcommand, visible_alias = "ws")]
    Workspace(WsCommand),
    /// Inspect the index.
    #[command(subcommand)]
    Index(IndexCommand),
    /// Print a shell completion script.
    Completions {
        /// The shell to generate for.
        shell: clap_complete::Shell,
    },
}

/// `key=value`, the value read as YAML.
type Assignment = (String, Value);

#[derive(clap::Args)]
struct NewArgs {
    /// The title: sets the key `workspace.toml` maps to `document:title`.
    #[arg(short, long)]
    title: Option<String>,
    /// Set a frontmatter key. The value is read as YAML: `n=3` is a number, `n="3"` a string.
    #[arg(long, value_name = "KEY=VALUE", value_parser = assignment)]
    set: Vec<Assignment>,
    /// The body. Without it, jot reads stdin when piped; with no field set either, it opens
    /// $EDITOR.
    #[arg(short = 'm', long)]
    message: Option<String>,
}

#[derive(clap::Args)]
struct ListArgs {
    /// Show at most this many.
    #[arg(short = 'n', long)]
    limit: Option<usize>,
}

#[derive(clap::Args)]
struct ShowArgs {
    /// The note's id, or a unique prefix of it.
    id: String,
    /// Print the file's bytes exactly.
    #[arg(long)]
    raw: bool,
}

#[derive(clap::Args)]
struct EditArgs {
    /// The note's id, or a unique prefix of it.
    id: String,
    /// Replace the title.
    #[arg(short, long)]
    title: Option<String>,
    /// Set a frontmatter key, the value read as YAML.
    #[arg(long, value_name = "KEY=VALUE", value_parser = assignment)]
    set: Vec<Assignment>,
    /// Remove a frontmatter key.
    #[arg(long, value_name = "KEY")]
    unset: Vec<String>,
    /// Replace the body. With no flag at all, jot opens $EDITOR on the whole note.
    #[arg(short = 'm', long)]
    message: Option<String>,
}

impl EditArgs {
    fn has_changes(&self) -> bool {
        self.title.is_some()
            || self.message.is_some()
            || !self.set.is_empty()
            || !self.unset.is_empty()
    }
}

#[derive(clap::Args)]
struct RemoveArgs {
    /// The note's id, or a unique prefix of it.
    id: String,
    /// Skip the confirmation prompt.
    #[arg(short = 'y', long)]
    yes: bool,
}

#[derive(clap::Args)]
struct IdArgs {
    /// The note's id, or a unique prefix of it.
    id: String,
}

#[derive(clap::Args)]
struct SearchArgs {
    /// Text to find in any frontmatter string, case-insensitively.
    query: String,
}

#[derive(Subcommand)]
enum FieldCommand {
    /// Set KEY to VALUE in every note that lacks it.
    Fill {
        /// The key.
        key: String,
        /// The value, read as YAML.
        #[arg(value_parser = yaml_value)]
        value: Value,
    },
    /// Remove KEY from every note that has it. Confirms.
    #[command(visible_alias = "rm")]
    Remove {
        /// The key.
        key: String,
        /// Skip the confirmation prompt.
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// Rename FROM to TO in every note, in schema.json, and in workspace.toml's [roles].
    Rename {
        /// The key now.
        from: String,
        /// The key after.
        to: String,
    },
}

/// How `ws use` and `ws remove` name a registered workspace: **by path, or by name, never both**.
///
/// The registry is keyed by path, so a path is the identity and a bare argument means one. Names
/// are display-only and not unique — two folders called `notes` under different parents is a normal
/// state — so a name has to be asked for with `--name`, and one matching several is refused.
#[derive(clap::Args)]
#[command(group(
    clap::ArgGroup::new("selector")
        .required(true)
        .args(["path", "name"])
))]
struct WorkspaceSelector {
    /// The workspace's folder.
    #[arg(value_name = "PATH")]
    path: Option<PathBuf>,

    /// Select by name instead. Fails if more than one workspace has it.
    #[arg(long, value_name = "NAME")]
    name: Option<String>,
}

impl WorkspaceSelector {
    fn resolve<'r>(&self, registry: &'r Registry) -> Result<&'r Entry, Exit> {
        if let Some(path) = &self.path {
            return registry.get(path).ok_or_else(|| {
                Exit::not_found(anyhow!(
                    "no registered workspace at `{}`\n\
                     hint: `jot ws list` shows them; `jot ws add <path>` registers one",
                    path.display()
                ))
            });
        }
        let name = self
            .name
            .as_deref()
            .expect("the selector group requires one");
        let matched = registry.named(name);
        match matched.as_slice() {
            [entry] => Ok(*entry),
            [] => Err(Exit::not_found(anyhow!(
                "no registered workspace is named `{name}`\nhint: `jot ws list` shows the names"
            ))),
            many => {
                let mut listing = format!("{} workspaces are named `{name}`:\n", many.len());
                for entry in many {
                    listing.push_str(&format!("  {}\n", entry.path().display()));
                }
                listing.push_str("hint: select one by its path");
                Err(Exit {
                    error: anyhow!(listing),
                    code: EXIT_AMBIGUOUS,
                })
            }
        }
    }
}

#[derive(Subcommand)]
enum WsCommand {
    /// List registered workspaces.
    #[command(visible_alias = "ls")]
    List,
    /// Make a workspace current.
    Use(WorkspaceSelector),
    /// Register an existing workspace.
    Add {
        /// The folder holding `workspace.toml`.
        path: PathBuf,
        /// A display name. Defaults to the folder name.
        #[arg(long)]
        name: Option<String>,
    },
    /// Create a workspace — `workspace.toml` and `schema.json` — and register it.
    New {
        /// Where to create it. Markdown files already there are left alone.
        path: PathBuf,
        /// A display name. Defaults to the folder name.
        #[arg(long)]
        name: Option<String>,
    },
    /// Unregister a workspace. The folder and its notes are left alone.
    #[command(visible_alias = "rm")]
    Remove(WorkspaceSelector),
    /// Unregister every workspace whose folder is gone.
    Prune {
        /// Skip the confirmation prompt.
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum IndexCommand {
    /// Note, problem and stray counts, and the state of the settings.
    Status,
    /// Throw the index away and rebuild it from the files.
    Rebuild,
}

/// Parses `key=value`, reading the value as YAML.
fn assignment(text: &str) -> Result<Assignment, String> {
    let (key, value) = text
        .split_once('=')
        .ok_or_else(|| format!("`{text}` is not KEY=VALUE"))?;
    if key.trim().is_empty() {
        return Err(format!("`{text}` names no key"));
    }
    Ok((key.to_owned(), yaml_value(value)?))
}

/// A value typed on the command line, read as one YAML value — the same YAML 1.2 core reading a
/// note's frontmatter gets, by core's own parser, so `3` here is what `3` in a file is.
fn yaml_value(text: &str) -> Result<Value, String> {
    if text.contains(['\n', '\r']) {
        return Err(String::from("a value must be on one line"));
    }
    let doc = Document::parse(&format!("---\nvalue: {text}\n---\n"));
    if let Some(problem) = doc.problems.first() {
        return Err(format!(
            "`{text}` is not a value jot can store: {}",
            problem.message
        ));
    }
    doc.fields
        .get("value")
        .cloned()
        .ok_or_else(|| format!("`{text}` is not a single YAML value"))
}

// =============================================================================================
// Dispatch
// =============================================================================================

fn run() -> Result<(), Exit> {
    let cli = Cli::parse();
    let style = Style::new(cli.long, cli.no_color);

    let Some(command) = cli.command.as_ref() else {
        Cli::command().print_help().map_err(anyhow::Error::from)?;
        return Ok(());
    };

    match command {
        // These two do not need — and must not require — an existing workspace.
        Command::Completions { shell } => {
            let mut command = Cli::command();
            let name = command.get_name().to_owned();
            clap_complete::generate(*shell, &mut command, name, &mut std::io::stdout());
            return Ok(());
        }
        Command::Workspace(ws) => return workspaces(ws, &cli, &style),
        _ => {}
    }

    let mut context = Context::open(cli.workspace.as_deref(), cli.verbose)?;
    let workspace = &mut context.workspace;
    if !matches!(command, Command::Check | Command::Index(_)) {
        report_problems(workspace);
    }
    // Short ids are only meaningful against the workspace they have to be distinct from, so the
    // table is built once here, after the workspace is open and before anything is rendered.
    let style = style.with_abbreviations(workspace.abbreviations(MIN_ID_WIDTH));

    match command {
        Command::New(args) => new(workspace, args, &cli, &style),
        Command::List(args) => list(workspace, args, &cli, &style),
        Command::Show(args) => show(workspace, args, &cli, &style),
        Command::Edit(args) => edit(workspace, args, &cli, &style),
        Command::Remove(args) => remove(workspace, args, &style),
        Command::Search(args) => search(workspace, args, &cli, &style),
        Command::Links(args) => links(workspace, args, &cli, &style),
        Command::Field(args) => field(workspace, args, &style),
        Command::Check => check(workspace, &cli, &style),
        Command::Index(args) => index(workspace, args, &cli),
        Command::Workspace(_) | Command::Completions { .. } => unreachable!("handled above"),
    }
}

/// One line on stderr when anything is wrong, pointing at `jot check`. Never blocks a command, and
/// never on stdout, so `jot ls --json | jq` keeps working while a broken file is complained about.
fn report_problems(workspace: &Workspace) {
    let mut parts = Vec::new();
    let notes = workspace.problems().len();
    if notes > 0 {
        parts.push(format!("{} with problems", output::count(notes, "note")));
    }
    let strays = workspace.strays().len();
    if strays > 0 {
        parts.push(output::count(strays, "stray file"));
    }
    if !parts.is_empty() {
        eprintln!("jot: {} — `jot check` lists them", parts.join(", "));
    }
}

// =============================================================================================
// Resolving ids, and reading outcomes
// =============================================================================================

/// Turn a user-supplied prefix into a note id, or fail with the right exit code.
///
/// Ambiguity **lists the candidates and never guesses** — picking one would be picking which of
/// your notes to edit at random.
fn resolve(workspace: &Workspace, prefix: &str) -> Result<NoteId, Exit> {
    match workspace.resolve(prefix) {
        Resolution::Unique(row) => Ok(row.id),
        Resolution::None => Err(Exit::not_found(anyhow!("no note matches `{prefix}`"))),
        Resolution::Ambiguous(candidates) => {
            let mut listing = format!("`{prefix}` matches {} notes:\n", candidates.len());
            for row in &candidates {
                listing.push_str(&format!("  {}  {}\n", row.id, row.display_title()));
            }
            listing.push_str("hint: use more characters, or the full id");
            Err(Exit {
                error: anyhow!(listing),
                code: EXIT_AMBIGUOUS,
            })
        }
    }
}

/// The key `-t` sets, or a clear refusal when `workspace.toml` maps no title.
fn title_key(workspace: &Workspace) -> Result<String, Exit> {
    workspace.settings().roles().title.clone().ok_or_else(|| {
        Exit::runtime(anyhow!(
            "-t sets the key mapped to `document:title`, and this workspace maps none\n\
             hint: map one under [roles] in workspace.toml, or set the key with --set <key>=<value>"
        ))
    })
}

/// What a person is told about a refusal: a headline, then one line per reason.
fn reasons(rejection: &Rejection) -> Vec<String> {
    match rejection {
        Rejection::Invalid { problems } | Rejection::InvalidValue { problems, .. } => {
            problems.iter().map(ToString::to_string).collect()
        }
        Rejection::Settings { errors } => errors.iter().map(ToString::to_string).collect(),
        Rejection::NotesInvalid { notes } => notes
            .iter()
            .flat_map(|(id, problems)| problems.iter().map(move |p| format!("{id}: {p}")))
            .collect(),
        other => vec![other.to_string()],
    }
}

fn refusal(rejection: &Rejection) -> anyhow::Error {
    let headline = match rejection {
        Rejection::Invalid { .. } => String::from("the note does not satisfy schema.json"),
        Rejection::Settings { .. } => {
            String::from("the workspace settings are broken, so jot will not write")
        }
        Rejection::InvalidValue { key, .. } => format!("that value is not valid for `{key}`"),
        Rejection::NotesInvalid { notes } => format!(
            "{} would not satisfy schema.json afterwards, so nothing was written; fix them first",
            output::count(notes.len(), "note")
        ),
        other => return anyhow!("{other}"),
    };
    let mut text = format!("{headline}:");
    for reason in reasons(rejection) {
        text.push_str(&format!("\n  {reason}"));
    }
    anyhow!(text)
}

/// The events of a single-note outcome, or the error to exit with.
fn done(outcome: Outcome) -> Result<Vec<Event>, Exit> {
    match outcome {
        Outcome::Done(events) => Ok(events),
        Outcome::Rejected(rejection) => Err(Exit::runtime(refusal(&rejection))),
        Outcome::Failed(failure) => Err(Exit::runtime(anyhow!("{failure}"))),
        Outcome::Partial { .. } => Err(Exit::runtime(anyhow!(
            "only part of the change was applied; run it again"
        ))),
    }
}

/// What an editor loop makes of an outcome: refusals the buffer can fix reopen it, anything else
/// stops with the buffer kept.
fn verdict(outcome: Outcome) -> Verdict<Vec<Event>> {
    match outcome {
        Outcome::Done(events) => Verdict::Accept(events),
        Outcome::Rejected(
            rejection @ (Rejection::Invalid { .. } | Rejection::Unrepresentable { .. }),
        ) => Verdict::Retry(reasons(&rejection)),
        Outcome::Rejected(rejection) => Verdict::Abort(format!("{:#}", refusal(&rejection))),
        Outcome::Failed(Failure::Changed { id }) => Verdict::Abort(format!(
            "note `{id}` changed on disk while it was open in the editor, so nothing was written \
             — run `jot edit` again and copy your changes across"
        )),
        Outcome::Failed(failure) => Verdict::Abort(failure.to_string()),
        Outcome::Partial { .. } => Verdict::Abort(String::from("only part of the change applied")),
    }
}

/// The id an outcome created.
fn created(events: &[Event]) -> Option<&NoteId> {
    events.iter().find_map(|event| match event {
        Event::NoteCreated { id } => Some(id),
        _ => None,
    })
}

/// After a write: the id on stdout, or the whole note as JSON.
fn report_note(workspace: &Workspace, id: &NoteId, cli: &Cli, style: &Style) -> Result<(), Exit> {
    if cli.json {
        let (note, row) = load(workspace, id)?;
        emit(&output::note_json(&note, &row))
    } else {
        println!("{}", style.show(id));
        Ok(())
    }
}

fn load(workspace: &Workspace, id: &NoteId) -> Result<(jot_core::Note, jot_core::Row), Exit> {
    let note = workspace.get(id)?;
    match (note, workspace.row(id)) {
        (Some(note), Some(row)) => Ok((note, row)),
        _ => Err(Exit::not_found(anyhow!("no note `{id}`"))),
    }
}

// =============================================================================================
// Commands
// =============================================================================================

/// `jot new` — the capture path, and the one that has to be fast.
fn new(workspace: &mut Workspace, args: &NewArgs, cli: &Cli, style: &Style) -> Result<(), Exit> {
    let mut fields = Fields::new();
    if let Some(title) = &args.title {
        fields.insert(title_key(workspace)?, Value::String(title.clone()));
    }
    for (key, value) in &args.set {
        fields.insert(key.clone(), value.clone());
    }

    // The input paths, in priority order: an explicit `-m`, then a pipe, then — only when nothing
    // was given at all — the editor. A field given on the command line is a capture already: a
    // title alone is a note, and it is written rather than opened.
    let body = match &args.message {
        Some(message) => message.clone(),
        None if !std::io::stdin().is_terminal() => read_stdin()?,
        None if !fields.is_empty() => String::new(),
        None => return new_in_editor(workspace, cli, style),
    };
    if fields.is_empty() && body.trim().is_empty() {
        eprintln!("jot: nothing typed, nothing written");
        return Ok(());
    }

    let events = done(workspace.apply(Action::Create { fields, body }))?;
    let id = created(&events).cloned().context("core created no note")?;
    report_note(workspace, &id, cli, style)
}

/// `jot new` with nothing given: `$EDITOR` on the workspace's template, until it is valid.
fn new_in_editor(workspace: &mut Workspace, cli: &Cli, style: &Style) -> Result<(), Exit> {
    // Before the editor opens, not after: finding out the settings are broken once the note is
    // typed would be the worst moment to.
    workspace
        .check_writable()
        .map_err(|r| Exit::runtime(refusal(&r)))?;
    let seed = workspace
        .template()
        .map_err(|r| Exit::runtime(refusal(&r)))?;
    let template = Document::parse(&seed);

    let composed = editor::compose(&seed, |text| {
        // Changed only in whitespace is the same as untouched.
        let doc = Document::parse(text);
        if doc.problems.is_empty() && doc.fields == template.fields && doc.body.trim().is_empty() {
            return Verdict::Accept(None);
        }
        match Action::create_from_text(text) {
            Ok(action) => match verdict(workspace.apply(action)) {
                Verdict::Accept(events) => Verdict::Accept(Some(events)),
                Verdict::Retry(reasons) => Verdict::Retry(reasons),
                Verdict::Abort(message) => Verdict::Abort(message),
            },
            Err(rejection) => Verdict::Retry(reasons(&rejection)),
        }
    })?;
    match composed {
        Composed::Done(Some(events)) => {
            let id = created(&events).cloned().context("core created no note")?;
            report_note(workspace, &id, cli, style)
        }
        Composed::Done(None) | Composed::Untouched => {
            eprintln!("jot: nothing typed, nothing written");
            Ok(())
        }
    }
}

fn read_stdin() -> Result<String, Exit> {
    let mut body = String::new();
    std::io::stdin()
        .read_to_string(&mut body)
        .context("cannot read the note body from stdin")?;
    Ok(body)
}

/// `jot ls`.
fn list(workspace: &Workspace, args: &ListArgs, cli: &Cli, style: &Style) -> Result<(), Exit> {
    let mut listing = workspace.list();
    if let Some(limit) = args.limit {
        listing.rows.truncate(limit);
    }
    if cli.json {
        return emit(&output::listing_json(&listing));
    }
    if listing.rows.is_empty() {
        eprintln!("jot: no notes");
        return Ok(());
    }
    if listing.ordering == jot_core::Ordering::EditedAt {
        // Said once, because the order is not the one a timeline promises.
        eprintln!(
            "jot: ordered by modification time — workspace.toml maps no `document:created_at`"
        );
    }
    print!("{}", output::listing(&listing, style));
    Ok(())
}

/// `jot show`.
fn show(workspace: &Workspace, args: &ShowArgs, cli: &Cli, style: &Style) -> Result<(), Exit> {
    let id = resolve(workspace, &args.id)?;
    let (note, row) = load(workspace, &id)?;
    if cli.json {
        emit(&output::note_json(&note, &row))
    } else if args.raw {
        print!("{}", note.text);
        Ok(())
    } else {
        print!("{}", output::show(&note, &row, style));
        Ok(())
    }
}

/// `jot edit`.
fn edit(workspace: &mut Workspace, args: &EditArgs, cli: &Cli, style: &Style) -> Result<(), Exit> {
    let id = resolve(workspace, &args.id)?;
    let (note, _) = load(workspace, &id)?;

    let events = if args.has_changes() {
        // Flags rewrite the note from its fields, and a key jot could not read is not among them:
        // writing from them would drop it without a word.
        if note.problems.iter().any(Problem::is_conversion) {
            return Err(Exit::runtime(anyhow!(
                "`{id}`'s frontmatter could not be fully read, so editing it with flags could \
                 drop what jot cannot read\nhint: `jot edit {}` opens it in $EDITOR",
                style.show(&id)
            )));
        }
        let mut fields = note.fields.clone();
        if let Some(title) = &args.title {
            fields.insert(title_key(workspace)?, Value::String(title.clone()));
        }
        for (key, value) in &args.set {
            fields.insert(key.clone(), value.clone());
        }
        for key in &args.unset {
            if fields.shift_remove(key).is_none() {
                eprintln!("jot: `{key}` is not set on this note");
            }
        }
        let body = args.message.clone().unwrap_or(note.body);
        done(workspace.apply(Action::Edit {
            id: id.clone(),
            fields,
            body,
            expected_hash: Some(note.hash),
        }))?
    } else {
        workspace
            .check_writable()
            .map_err(|r| Exit::runtime(refusal(&r)))?;
        // The hash of the bytes the editor starts from, so a file changed underneath it is a
        // refused write rather than a silent overwrite.
        let expected = note.hash;
        let composed = editor::compose(&note.text, |text| {
            match Action::edit_from_text(id.clone(), text, Some(expected)) {
                Ok(action) => verdict(workspace.apply(action)),
                Err(rejection) => Verdict::Retry(reasons(&rejection)),
            }
        })?;
        match composed {
            Composed::Done(events) => events,
            Composed::Untouched => Vec::new(),
        }
    };

    if events.is_empty() {
        eprintln!("jot: no changes");
        return Ok(());
    }
    report_note(workspace, &id, cli, style)
}

/// `jot rm` — deletes the file, so it confirms.
fn remove(workspace: &mut Workspace, args: &RemoveArgs, style: &Style) -> Result<(), Exit> {
    let id = resolve(workspace, &args.id)?;
    let title = workspace
        .row(&id)
        .map(|row| row.display_title().to_owned())
        .unwrap_or_default();
    if !args.yes
        && !confirm(&format!(
            "permanently delete {} ({title})? jot keeps no copy",
            style.show(&id)
        ))?
    {
        return Ok(());
    }

    let linked = workspace.backlinks(&id).len();
    done(workspace.apply(Action::Delete { id: id.clone() }))?;
    eprintln!("jot: deleted {}", style.show(&id));
    if linked > 0 {
        // Nothing cascades; the links dangle, which is a designed state — but worth saying.
        eprintln!("jot: {} still link to it", output::count(linked, "note"));
    }
    Ok(())
}

/// `jot search`.
fn search(workspace: &Workspace, args: &SearchArgs, cli: &Cli, style: &Style) -> Result<(), Exit> {
    let listing = workspace.search(&args.query);
    if cli.json {
        emit(&output::listing_json(&listing))
    } else if listing.rows.is_empty() {
        eprintln!("jot: nothing matches `{}`", args.query);
        Ok(())
    } else {
        print!("{}", output::listing(&listing, style));
        Ok(())
    }
}

/// `jot links`.
fn links(workspace: &Workspace, args: &IdArgs, cli: &Cli, style: &Style) -> Result<(), Exit> {
    let id = resolve(workspace, &args.id)?;
    let outgoing = workspace.links_in(&id)?;
    let backlinks = workspace.backlinks(&id);
    if cli.json {
        emit(&json!({
            "id": id.as_str(),
            "links_out": outgoing.iter()
                .map(|(link, target)| output::link_json(link, target))
                .collect::<Vec<_>>(),
            "links_in": backlinks.iter().map(output::row_json).collect::<Vec<_>>(),
        }))
    } else {
        print!("{}", output::links(&outgoing, &backlinks, style));
        Ok(())
    }
}

/// `jot field` — the three batch actions.
///
/// Core computes which notes a batch touches and refuses the whole batch, naming the notes, if any
/// of them would come out invalid; this prints what happened, from the events.
fn field(workspace: &mut Workspace, command: &FieldCommand, style: &Style) -> Result<(), Exit> {
    let (action, what) = match command {
        FieldCommand::Fill { key, value } => (
            Action::FillField {
                key: key.clone(),
                value: value.clone(),
            },
            format!("setting `{key}` to {value}"),
        ),
        FieldCommand::Remove { key, yes } => {
            let holding = workspace
                .snapshot()
                .notes()
                .filter(|record| record.fields().contains_key(key))
                .count();
            if holding > 0
                && !yes
                && !confirm(&format!(
                    "remove `{key}` and its value from {}? jot keeps no copy",
                    output::count(holding, "note")
                ))?
            {
                return Ok(());
            }
            (
                Action::RemoveField { key: key.clone() },
                format!("removing `{key}`"),
            )
        }
        FieldCommand::Rename { from, to } => (
            Action::RenameField {
                from: from.clone(),
                to: to.clone(),
            },
            format!("renaming `{from}` to `{to}`"),
        ),
    };

    let plan = workspace
        .decide(&action)
        .map_err(|r| Exit::runtime(refusal(&r)))?;
    if plan.is_noop() {
        eprintln!("jot: nothing to do — no note needs it");
        return Ok(());
    }
    let outcome = workspace.execute(plan);

    for event in outcome.events() {
        match event {
            Event::BatchStarted { total, .. } => {
                eprintln!("jot: {what} in {}", output::count(*total, "note"));
            }
            Event::NoteChanged { id, .. } => {
                let title = workspace
                    .row(id)
                    .map(|row| row.display_title().to_owned())
                    .unwrap_or_default();
                println!("{}  {title}", style.show(id));
            }
            Event::SettingsChanged { file } => eprintln!("jot: updated {}", file.file_name()),
            Event::BatchFinished { applied, .. } => {
                eprintln!("jot: {} rewritten", output::count(*applied, "note"));
            }
            _ => {}
        }
    }

    match outcome {
        Outcome::Done(_) => Ok(()),
        Outcome::Rejected(rejection) => Err(Exit::runtime(refusal(&rejection))),
        Outcome::Failed(failure) => Err(Exit::runtime(anyhow!("{failure}"))),
        Outcome::Partial {
            failed, settings, ..
        } => {
            for (id, failure) in &failed {
                eprintln!("jot: {} failed: {failure}", style.show(id));
            }
            if let Some(failure) = settings {
                eprintln!("jot: the settings were not updated: {failure}");
            }
            // Every batch step is idempotent, so a rerun picks up exactly where this stopped.
            Err(Exit::runtime(anyhow!(
                "the change was only partly applied; run the same command again to finish it"
            )))
        }
    }
}

/// `jot check` — exits 5 when a note has problems or the settings are broken. Strays are listed
/// but do not fail it: a `README.md` beside the notes is ordinary.
fn check(workspace: &Workspace, cli: &Cli, style: &Style) -> Result<(), Exit> {
    let settings = workspace.settings();
    let problems: Vec<(jot_core::Row, Vec<Problem>)> = workspace
        .problems()
        .into_iter()
        .filter_map(|(id, problems)| workspace.row(&id).map(|row| (row, problems)))
        .collect();
    let strays = workspace.strays();

    if cli.json {
        emit(&json!({
            "settings": {
                "errors": settings.errors().iter().map(ToString::to_string).collect::<Vec<_>>(),
                "warnings": settings.warnings().iter().map(ToString::to_string).collect::<Vec<_>>(),
                "stale": settings.is_stale(),
            },
            "notes": problems.iter().map(|(row, problems)| json!({
                "id": row.id.as_str(),
                "title": row.title,
                "problems": problems.iter().map(output::problem_json).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "strays": strays.iter().map(output::stray_json).collect::<Vec<_>>(),
        }))?;
    } else {
        // Settings errors were already printed on the way in, by `Context::open`.
        print!("{}", output::check(&problems, &strays, style));
        if problems.is_empty() && settings.errors().is_empty() {
            eprintln!("jot: no problems");
        }
    }

    if problems.is_empty() && settings.errors().is_empty() {
        Ok(())
    } else {
        Err(Exit {
            error: anyhow!(""),
            code: EXIT_PROBLEMS,
        })
    }
}

/// `jot index`.
fn index(workspace: &mut Workspace, command: &IndexCommand, cli: &Cli) -> Result<(), Exit> {
    // `open` already synced, so `status` has nothing to do but report.
    if let IndexCommand::Rebuild = command {
        let events = done(workspace.rebuild())?;
        if !cli.json {
            eprintln!(
                "jot: rebuilt the index; {} it had been missing",
                output::count(events.len(), "change")
            );
        }
    }
    let settings = workspace.settings();
    let notes = workspace.snapshot().len();
    let problems = workspace.problems().len();
    let strays = workspace.strays().len();

    if cli.json {
        emit(&json!({
            "root": workspace.root().display().to_string(),
            "notes": notes,
            "notes_with_problems": problems,
            "strays": strays,
            "writable": settings.is_writable(),
            "stale": settings.is_stale(),
        }))
    } else {
        println!("workspace  {}", workspace.root().display());
        println!("notes      {notes} ({problems} with problems)");
        println!("strays     {strays}");
        println!(
            "settings   {}",
            match (settings.is_writable(), settings.is_stale()) {
                (true, _) => "valid",
                (false, true) => "broken — reading with the last valid settings",
                (false, false) => "broken",
            }
        );
        Ok(())
    }
}

/// `jot ws` — the one command group that works without an existing workspace.
fn workspaces(command: &WsCommand, cli: &Cli, style: &Style) -> Result<(), Exit> {
    let mut registry = context::load_registry()?;

    match command {
        WsCommand::List => {
            let entries: Vec<&Entry> = registry.entries().collect();
            if cli.json {
                emit(&json!(
                    entries
                        .iter()
                        .map(|entry| json!({
                            "name": entry.name(),
                            "path": entry.path().display().to_string(),
                            "current": is_current(&registry, entry),
                            "stale": entry.is_stale(),
                        }))
                        .collect::<Vec<_>>()
                ))?;
            } else if entries.is_empty() {
                eprintln!("jot: no registered workspaces — try `jot ws new <path>`");
            } else {
                for entry in entries {
                    let current = is_current(&registry, entry);
                    println!("{}", output::workspace(entry, current, style));
                }
            }
        }

        WsCommand::Use(selector) => {
            let entry = selector.resolve(&registry)?.clone();
            registry.set_current(entry.path());
            context::save_registry(&registry)?;
            eprintln!(
                "jot: now using `{}` ({})",
                entry.name(),
                entry.path().display()
            );
        }

        WsCommand::Add { path, name } => {
            let workspace = Workspace::open(path)
                .map_err(|e| context::opening(e, format!("`{}`", path.display())))?;
            let entry = register(&mut registry, &workspace, name.clone())?;
            eprintln!(
                "jot: registered `{}` ({})",
                entry.name(),
                entry.path().display()
            );
        }

        WsCommand::New { path, name } => {
            let workspace = Workspace::init(path)
                .map_err(|e| context::opening(e, format!("`{}`", path.display())))?;
            let entry = register(&mut registry, &workspace, name.clone())?;
            eprintln!(
                "jot: created `{}` at {}",
                entry.name(),
                entry.path().display()
            );
        }

        WsCommand::Remove(selector) => {
            let entry = selector.resolve(&registry)?.clone();
            let was_current = is_current(&registry, &entry);
            registry.remove(entry.path());
            // `Registry::remove` leaves a dangling `current` alone, which is right for a library;
            // the surface is what has to notice, or the next bare `jot new` has nowhere to go.
            if was_current {
                registry.clear_current();
            }
            context::save_registry(&registry)?;
            eprintln!("jot: unregistered `{}`", entry.name());
            // Said every time: `remove` is about the *registry*, and someone typing it after
            // `jot rm` — which does delete a file — should not have to wonder.
            eprintln!("jot: the folder is untouched: {}", entry.path().display());
        }

        WsCommand::Prune { yes } => {
            let stale: Vec<Entry> = registry.stale_entries().cloned().collect();
            if stale.is_empty() {
                eprintln!("jot: nothing to prune — every registered workspace is present");
                return Ok(());
            }
            for entry in &stale {
                eprintln!("  {}  {}", entry.name(), entry.path().display());
            }
            // Confirmed even though nothing on disk is deleted, because "stale" is a missing
            // folder — and an external drive that is merely unmounted looks exactly like one.
            if !yes
                && !confirm(&format!(
                    "unregister {} whose folder is missing? check that none is on an unmounted drive",
                    output::count(stale.len(), "workspace")
                ))?
            {
                return Ok(());
            }
            for entry in &stale {
                if is_current(&registry, entry) {
                    registry.clear_current();
                }
                registry.remove(entry.path());
            }
            context::save_registry(&registry)?;
            eprintln!("jot: pruned {}", output::count(stale.len(), "workspace"));
        }
    }
    Ok(())
}

/// Whether `entry` is the registry's current workspace. Through `Registry::get`, so the comparison
/// is the registry's own — case-insensitive on Windows.
fn is_current(registry: &Registry, entry: &Entry) -> bool {
    registry
        .current()
        .and_then(|current| registry.get(current))
        .is_some_and(|current| current.path() == entry.path())
}

/// Add a workspace to the registry and make it current — you named it, so it is the one you meant
/// to work in. A name already set is kept unless a new one is given.
fn register(
    registry: &mut Registry,
    workspace: &Workspace,
    name: Option<String>,
) -> Result<Entry, Exit> {
    let root = workspace.root();
    let name = name.or_else(|| {
        registry
            .get(root)
            .and_then(Entry::explicit_name)
            .map(str::to_owned)
    });
    let entry = Entry::new(root, name);
    registry.upsert(entry.clone());
    registry.set_current(root);
    context::save_registry(registry)?;
    Ok(entry)
}

// =============================================================================================
// Shared helpers
// =============================================================================================

/// Ask a yes/no question on stderr. `false` means the caller should stop.
///
/// Defaults to no, and anything that is not `y`/`yes` is a no: a prompt that proceeds on a stray
/// keystroke is not a confirmation. The question goes to stderr so a piped stdout stays clean.
fn confirm(question: &str) -> Result<bool, Exit> {
    eprint!("jot: {question} [y/N] ");
    std::io::stderr().flush().map_err(anyhow::Error::from)?;

    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("cannot read a confirmation")?;

    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        return Ok(true);
    }
    eprintln!("jot: cancelled");
    Ok(false)
}

/// Write a JSON value to stdout, one document, newline-terminated.
fn emit(value: &Value) -> Result<(), Exit> {
    let text =
        serde_json::to_string_pretty(value).context("cannot serialize the result as JSON")?;
    println!("{text}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_tree_is_internally_consistent() {
        // clap's own assertions: conflicting flags that name missing arguments, duplicate short
        // options, and so on. Cheap, and it catches a mis-wired group at test time.
        Cli::command().debug_assert();
    }

    /// A value is read the way frontmatter is, so `--set` cannot store something a file holding
    /// the same text would not.
    #[test]
    fn a_set_value_is_read_as_yaml() {
        assert_eq!(assignment("n=3").unwrap(), ("n".into(), json!(3)));
        assert_eq!(assignment("n=\"3\"").unwrap().1, json!("3"));
        assert_eq!(assignment("done=true").unwrap().1, json!(true));
        assert_eq!(assignment("tags=[a, b]").unwrap().1, json!(["a", "b"]));
        assert_eq!(assignment("url=a=b").unwrap().1, json!("a=b"));
        assert!(assignment("x=.nan").is_err(), "not a number JSON can hold");
        assert!(assignment("=3").is_err());
        assert!(assignment("novalue").is_err());
    }
}
