//! `jot` — the command-line surface.
//!
//! # The seam
//!
//! This crate contains **no domain logic**. It parses arguments, chooses a workspace, calls
//! `jot-core`, and formats what comes back. It never opens a note file, never walks the vault, and
//! never decides what a thread is. If a command here seems to need a new rule, the rule belongs in
//! core — that is the whole reason three surfaces can exist without becoming three subtly different
//! applications.
//!
//! The one deliberate exception is [`editor`], which writes a temp file outside the vault so that
//! `$EDITOR` has something to open. What comes back still goes through core's write path.
//!
//! # Exit codes
//!
//! Fixed, because scripts depend on them:
//!
//! | Code | Meaning |
//! | --- | --- |
//! | 0 | success |
//! | 1 | runtime error |
//! | 2 | usage error (clap's own) |
//! | 3 | no such note or workspace |
//! | 4 | ambiguous id prefix |

mod compose;
mod context;
mod editor;
mod output;

use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, Duration, Utc};
use clap::{CommandFactory, Parser, Subcommand};
use context::Context;
use jot_core::note::{Note, NoteId};
use jot_core::query::{Draft, Edit, FileSort, Resolution, SearchQuery, State, TimelineQuery};
use jot_core::registry::Entry;
use jot_core::shortid;
use jot_core::workspace::Workspace;
use output::{IdWidth, MIN_ID_WIDTH, Style};
use serde_json::json;
use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use uuid::Uuid;

/// Exit code for "the thing you named does not exist".
const EXIT_NOT_FOUND: u8 = 3;
/// Exit code for "the prefix you gave matches more than one note".
const EXIT_AMBIGUOUS: u8 = 4;

/// A failure that knows which exit code it deserves.
struct Failure {
    error: anyhow::Error,
    code: u8,
}

impl Failure {
    fn runtime(error: anyhow::Error) -> Failure {
        Failure { error, code: 1 }
    }
}

impl From<anyhow::Error> for Failure {
    fn from(error: anyhow::Error) -> Failure {
        Failure::runtime(error)
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            eprintln!("jot: {:#}", failure.error);
            ExitCode::from(failure.code)
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
    long_about = "jot — a personal capture tool over a directory of markdown files.\n\n\
                  Notes reply to notes and quote notes, so an idea can grow a structure while it \
                  is being written instead of demanding a folder decision first."
)]
struct Cli {
    /// Act on the workspace at this path, overriding discovery.
    #[arg(long, global = true, value_name = "PATH")]
    workspace: Option<PathBuf>,

    /// Emit JSON instead of human-readable output.
    #[arg(long, global = true)]
    json: bool,

    /// Print full UUIDs instead of short ids.
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
    List(LsArgs),
    /// Print one note in full.
    Show(ShowArgs),
    /// Print the thread a note belongs to.
    Thread(ThreadArgs),
    /// Change a note's title or body.
    Edit(EditArgs),
    /// Move a note to the trash.
    #[command(visible_alias = "rm")]
    Remove(IdArgs),
    /// Bring a note back out of the trash.
    Restore(IdArgs),
    /// Delete a note for good. Irreversible.
    Purge(PurgeArgs),
    /// List what is in the trash.
    Trash,
    /// Search titles.
    Search(SearchArgs),
    /// Show a note's links, backlinks, and quotes.
    Links(IdArgs),
    /// Manage workspaces.
    #[command(subcommand, visible_alias = "ws")]
    Workspace(WsCommand),
    /// Inspect the vault index.
    #[command(subcommand)]
    Index(IndexCommand),
    /// Browse the vault full-screen.
    Tui,
    /// Print a shell completion script.
    Completions {
        /// The shell to generate for.
        shell: clap_complete::Shell,
    },
}

#[derive(clap::Args)]
struct NewArgs {
    /// The note's title.
    #[arg(short, long)]
    title: Option<String>,
    /// The note's body. Optional: a title on its own is a note. Without this, jot reads stdin
    /// when piped, or opens $EDITOR.
    #[arg(short = 'm', long)]
    message: Option<String>,
    /// Reply to this note, joining its thread.
    #[arg(long, value_name = "ID")]
    reply: Option<String>,
    /// Quote this note. Never changes the thread.
    #[arg(long, value_name = "ID")]
    quote: Option<String>,
    /// Put a slug derived from the title in the filename.
    #[arg(long)]
    slug: bool,
}

#[derive(clap::Args)]
struct LsArgs {
    /// Show replies as well as thread roots.
    #[arg(long)]
    flat: bool,
    /// Only notes created since then — `2d`, `3h`, or a date.
    #[arg(long, value_name = "WHEN")]
    since: Option<String>,
    /// Show at most this many.
    #[arg(short = 'n', long)]
    limit: Option<usize>,
    /// Sort by something other than creation time.
    #[arg(long, value_enum)]
    sort: Option<Sort>,
}

#[derive(clap::ValueEnum, Clone, Copy)]
enum Sort {
    /// Newest first.
    Created,
    /// Most recently written first.
    Edited,
    /// Alphabetical; untitled last.
    Title,
}

#[derive(clap::Args)]
struct ShowArgs {
    /// The note's id, or a unique prefix of it.
    id: String,
    /// Print the file's bytes exactly, frontmatter included.
    #[arg(long)]
    raw: bool,
}

#[derive(clap::Args)]
struct ThreadArgs {
    /// The note's id, or a unique prefix of it.
    id: String,
    /// Draw the whole tree. The default.
    #[arg(long, conflicts_with_all = ["path", "segments"])]
    tree: bool,
    /// Print only the line from the root to this note.
    #[arg(long, conflicts_with_all = ["tree", "segments"])]
    path: bool,
    /// Print the thread cut into chains at its branch points.
    #[arg(long, conflicts_with_all = ["tree", "path"])]
    segments: bool,
}

#[derive(clap::Args)]
struct EditArgs {
    /// The note's id, or a unique prefix of it.
    id: String,
    /// Replace the title.
    #[arg(short, long, conflicts_with = "no_title")]
    title: Option<String>,
    /// Remove the title. Refused if it would leave the note with no title and no body.
    #[arg(long)]
    no_title: bool,
    /// Replace the body. Without this or --title, jot opens $EDITOR.
    #[arg(short = 'm', long)]
    message: Option<String>,
}

#[derive(clap::Args)]
struct IdArgs {
    /// The note's id, or a unique prefix of it.
    id: String,
}

#[derive(clap::Args)]
struct PurgeArgs {
    /// The note's id, or a unique prefix of it.
    id: String,
    /// Skip the confirmation prompt.
    #[arg(short = 'y', long)]
    yes: bool,
}

#[derive(clap::Args)]
struct SearchArgs {
    /// A substring of the title.
    query: String,
    /// Only notes created since then.
    #[arg(long, value_name = "WHEN")]
    since: Option<String>,
    /// Only notes created before then.
    #[arg(long, value_name = "WHEN")]
    until: Option<String>,
    /// Search the trash too.
    #[arg(long)]
    trashed: bool,
}

/// How a command names the workspace it acts on: **by id, or by name, never both**.
///
/// # Why id is the default
///
/// A workspace name is display-only. It is seeded from the directory's basename at `init`,
/// `workspace.toml` says it is "safe to edit by hand", and **nothing enforces uniqueness** — two
/// vaults called `notes` under different parents are a normal state, as is one vault deleted and
/// remade. The id is the identity: minted once, immutable, and what the registry keys on.
///
/// So a bare `jot workspace use <ID>` means the id, and a name has to be asked for explicitly. The
/// previous behaviour — try the name, fall back to an id prefix — made the *meaning* of an argument
/// depend on what happened to be registered, which is the wrong thing to be clever about when the
/// answer decides where your notes get captured.
#[derive(clap::Args)]
#[command(group(
    clap::ArgGroup::new("selector")
        .required(true)
        .args(["target", "id", "name"])
))]
struct WorkspaceSelector {
    /// The workspace's id, or a unique prefix of it. Same as `--id`.
    #[arg(value_name = "ID")]
    target: Option<String>,

    /// Select by id, or a unique prefix of it.
    #[arg(long, value_name = "ID")]
    id: Option<String>,

    /// Select by name. Names are not unique, so this fails if more than one matches.
    #[arg(long, value_name = "NAME")]
    name: Option<String>,
}

impl WorkspaceSelector {
    /// Resolve to exactly one registered workspace.
    ///
    /// The `ArgGroup` above makes the three forms mutually exclusive and requires one, so this
    /// cannot see an empty or over-specified selector.
    fn resolve(&self, registry: &jot_core::registry::Registry) -> Result<Uuid, Failure> {
        match (&self.name, self.id.as_ref().or(self.target.as_ref())) {
            (Some(name), _) => resolve_workspace_by_name(registry, name),
            (None, Some(id)) => resolve_workspace_by_id(registry, id),
            (None, None) => unreachable!("the `selector` group requires one of the three"),
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
        /// The directory holding `.jot/`.
        path: PathBuf,
    },
    /// Create a workspace and register it.
    New {
        /// Where to create it.
        path: PathBuf,
    },
    /// Unregister a workspace. The directory and its notes are left alone.
    #[command(visible_alias = "rm")]
    Remove(WorkspaceSelector),
    /// Unregister every workspace whose directory is gone.
    Prune {
        /// Skip the confirmation prompt.
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum IndexCommand {
    /// Note counts, and any problems the scan is holding.
    Status,
    /// Discard the in-memory view and read the vault again.
    Rebuild,
}

// =============================================================================================
// Dispatch
// =============================================================================================

fn run() -> Result<(), Failure> {
    let cli = Cli::parse();
    let style = Style::new(cli.long, cli.no_color);

    // No arguments opens the browser, which is stage 5's headline: `jot` is a thing you *read* as
    // well as a thing you type at. `jot tui` is the explicit form, and both land in the same arm
    // below once a workspace has been resolved.
    //
    // Unless nobody is looking. `jot | less`, a CI step, a script capturing output — none of them
    // can drive a full-screen app, and switching to the alternate screen there produces escape
    // codes in a pipe rather than a user interface. A bare `jot` in that situation keeps its
    // stage-3 behaviour and prints help, which is also the only thing a script could have wanted.
    // `jot tui` asked explicitly, and is refused explicitly below rather than silently downgraded.
    // `jot` is a CLI first. A bare invocation prints help, as it has since stage 3 — the browser
    // is somewhere you go on purpose, via `jot tui`, not somewhere you land by typing the
    // program's name. There is deliberately no `--tui` twin: the global options are `global =
    // true`, so `jot tui --workspace ~/notes` already reads the way you would want it to, and a
    // second spelling would be a second thing to keep in step for no reach it does not have.
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
    report_problems(&context.workspace);
    // Short ids are only meaningful against the vault they have to be distinct from, so the table
    // is built once here, after the workspace is open and before anything is rendered.
    let style = style.with_abbreviations(context.workspace.abbreviations(MIN_ID_WIDTH));

    match command {
        Command::New(args) => new(&mut context.workspace, args, &cli, &style),
        Command::List(args) => list(&context.workspace, args, &cli, &style),
        Command::Show(args) => show(&context.workspace, args, &cli, &style),
        Command::Thread(args) => thread(&context.workspace, args, &cli, &style),
        Command::Edit(args) => edit(&mut context.workspace, args, &cli, &style),
        Command::Remove(args) => remove(&mut context.workspace, args, &style),
        Command::Restore(args) => restore(&mut context.workspace, args, &style),
        Command::Purge(args) => purge(&mut context.workspace, args, &style),
        Command::Trash => trash(&context.workspace, &cli, &style),
        Command::Search(args) => search(&context.workspace, args, &cli, &style),
        Command::Links(args) => links(&context.workspace, args, &cli, &style),
        Command::Index(args) => index(&mut context.workspace, args, &cli),
        // The seam in one line: the TUI is handed an already-opened workspace and owns nothing
        // else. It syncs on its own from here, because a full-screen app syncs repeatedly where a
        // command syncs once.
        Command::Tui => {
            if !std::io::stdout().is_terminal() {
                return Err(Failure::runtime(anyhow::anyhow!(
                    "`jot tui` needs a terminal; stdout is redirected"
                )));
            }
            jot_tui::run(context.workspace, &compose::Editor).map_err(Failure::runtime)
        }
        Command::Workspace(_) | Command::Completions { .. } => unreachable!("handled above"),
    }
}

/// Print the scan's problems on stderr without blocking the command.
///
/// Warnings go to stderr precisely so that `jot ls --json | jq` keeps working while a broken file
/// is still being complained about.
fn report_problems(workspace: &Workspace) {
    let problems = workspace.problems();
    if !problems.is_empty() {
        eprintln!("{}", output::problems(problems));
    }
}

// =============================================================================================
// Resolving ids
// =============================================================================================

/// Turn a user-supplied prefix into a note id, or fail with the right exit code.
///
/// Ambiguity **lists the candidates and never guesses** — picking one would be picking which of
/// your notes to edit at random.
fn resolve(workspace: &Workspace, prefix: &str, style: &Style) -> Result<NoteId, Failure> {
    match workspace.resolve(prefix) {
        Resolution::Unique(meta) => Ok(meta.id),
        Resolution::None => Err(Failure {
            error: anyhow::anyhow!("no note matches `{prefix}`"),
            code: EXIT_NOT_FOUND,
        }),
        Resolution::Ambiguous(candidates) => {
            let mut listing = format!("`{prefix}` matches {} notes:\n", candidates.len());
            for meta in &candidates {
                listing.push_str(&format!(
                    "  {}  {}\n",
                    meta.id,
                    meta.title.as_deref().unwrap_or("Untitled")
                ));
            }
            // UUIDv7 ids share a long timestamp prefix, so this happens easily among notes
            // captured together. The full id always works.
            listing.push_str("hint: use more characters, or the full id");
            let _ = style;
            Err(Failure {
                error: anyhow::anyhow!(listing),
                code: EXIT_AMBIGUOUS,
            })
        }
    }
}

/// The state a note is in, for rendering.
fn state_of(workspace: &Workspace, id: NoteId) -> State {
    workspace.state_of(id).unwrap_or(State::Active)
}

// =============================================================================================
// Commands
// =============================================================================================

/// `jot new` — the capture path, and the one that has to be fast.
fn new(workspace: &mut Workspace, args: &NewArgs, cli: &Cli, style: &Style) -> Result<(), Failure> {
    let reply = args
        .reply
        .as_deref()
        .map(|prefix| resolve(workspace, prefix, style))
        .transpose()?;
    let quote = args
        .quote
        .as_deref()
        .map(|prefix| resolve(workspace, prefix, style))
        .transpose()?;

    let mut draft = Draft {
        title: args.title.clone(),
        reply_to: reply,
        quote,
        ..Draft::default()
    };
    if args.slug {
        draft = draft.slugged();
    }

    // The three input paths, in the priority order stage 3 fixes: an explicit `-m`, then a pipe,
    // then the editor. Each exists because it removes friction from a different context.
    draft.body = match &args.message {
        Some(message) => message.clone(),
        None if !std::io::stdin().is_terminal() => read_stdin()?,
        None => {
            let seed = editor::seed(
                &seed_frontmatter(&draft),
                "\n",
                &workspace.manifest().schema,
            );
            let edited = editor::edit(workspace.schema(), &seed).map_err(Failure::runtime)?;
            if edited.is_empty() {
                // A note that was never written is the failure this app exists to prevent — but a
                // buffer left wholly untouched is how every editor-driven tool says "cancel". A
                // title with no body is not that: it is a capture, and it gets written.
                eprintln!("jot: nothing typed, nothing written");
                return Ok(());
            }

            // The buffer is authoritative for everything a new note may declare. `reply_to` is
            // included — choosing a parent is a normal thing to do while writing, and unlike an
            // *edit* it re-parents nothing. There is no root to warn about any more: it is
            // derived from `reply_to` and no note file carries one.
            draft.title = edited.frontmatter.title.clone();
            draft.quote = edited.frontmatter.quote;
            draft.reply_to = edited.frontmatter.reply_to;
            // Carries any key the schema declares that jot does not interpret, so a filled-in
            // custom field survives instead of being silently dropped.
            draft.extra = Some(edited.frontmatter.clone());
            edited.body
        }
    };

    // A note needs a title *or* a body; neither is how you cancel. The title alone is enough,
    // because a title is what a captured thought starts as.
    if draft.is_empty() {
        eprintln!("jot: no title and no body, nothing written");
        return Ok(());
    }

    let note = workspace.create(draft).map_err(anyhow::Error::from)?;
    if cli.json {
        emit(&output::note_json(&note, State::Active))?;
    } else {
        println!("{}", style.show(note.id));
    }
    Ok(())
}

/// The frontmatter an editor seed carries, so a title can be typed into the block.
fn seed_frontmatter(draft: &Draft) -> jot_core::frontmatter::Frontmatter {
    let mut frontmatter = jot_core::frontmatter::Frontmatter::new();
    frontmatter.title = draft.title.clone();
    frontmatter.reply_to = draft.reply_to;
    frontmatter.quote = draft.quote;
    frontmatter
}

fn read_stdin() -> Result<String, Failure> {
    let mut body = String::new();
    std::io::stdin()
        .read_to_string(&mut body)
        .context("cannot read the note body from stdin")
        .map_err(Failure::runtime)?;
    Ok(body)
}

/// `jot ls`.
fn list(workspace: &Workspace, args: &LsArgs, cli: &Cli, style: &Style) -> Result<(), Failure> {
    let rows = match args.sort {
        // A sort other than creation order is a *file listing*, not a timeline: paging a timeline
        // depends on ids being the sort key, and they are not once you sort by title.
        Some(sort) => {
            let mut rows = workspace.files(match sort {
                Sort::Created => FileSort::Created,
                Sort::Edited => FileSort::Edited,
                Sort::Title => FileSort::Title,
            });
            if let Some(limit) = args.limit {
                rows.truncate(limit);
            }
            rows
        }
        None => {
            let mut query = TimelineQuery {
                flat: args.flat,
                limit: args.limit,
                ..TimelineQuery::default()
            };
            if let Some(since) = &args.since {
                query.since = Some(parse_when(since).map_err(Failure::runtime)?);
            }
            workspace.timeline(&query).items
        }
    };

    if cli.json {
        emit(&json!(
            rows.iter().map(output::row_json).collect::<Vec<_>>()
        ))?;
    } else if rows.is_empty() {
        eprintln!("jot: no notes");
    } else {
        for row in &rows {
            println!("{}", output::row(row, style));
        }
    }
    Ok(())
}

/// `jot show`.
fn show(workspace: &Workspace, args: &ShowArgs, cli: &Cli, style: &Style) -> Result<(), Failure> {
    let id = resolve(workspace, &args.id, style)?;
    let note = load(workspace, id)?;
    let state = state_of(workspace, id);

    if cli.json {
        emit(&output::note_json(&note, state))?;
    } else if args.raw {
        // The file's own bytes, which is what `--raw` promises. Rendered through the schema so it
        // is exactly what jot would write, not a guess.
        print!(
            "{}",
            String::from_utf8_lossy(&note.to_bytes(&workspace.manifest().schema))
        );
    } else {
        println!("{}", output::show(&note, state, style));
    }
    Ok(())
}

fn load(workspace: &Workspace, id: NoteId) -> Result<Note, Failure> {
    workspace
        .get(id)
        .map_err(anyhow::Error::from)?
        .ok_or_else(|| Failure {
            error: anyhow::anyhow!("no note `{id}`"),
            code: EXIT_NOT_FOUND,
        })
}

/// `jot thread`.
fn thread(
    workspace: &Workspace,
    args: &ThreadArgs,
    cli: &Cli,
    style: &Style,
) -> Result<(), Failure> {
    let id = resolve(workspace, &args.id, style)?;
    let thread = workspace.thread(id).ok_or_else(|| Failure {
        error: anyhow::anyhow!("no note `{id}`"),
        code: EXIT_NOT_FOUND,
    })?;

    if cli.json {
        emit(&output::thread_json(&thread))?;
    } else if args.path {
        println!("{}", output::path(&thread, style));
    } else if args.segments {
        println!("{}", output::segments(&thread.tree.segments(), style));
    } else {
        print!("{}", output::tree(&thread, style));
    }
    Ok(())
}

/// `jot edit`.
fn edit(
    workspace: &mut Workspace,
    args: &EditArgs,
    cli: &Cli,
    style: &Style,
) -> Result<(), Failure> {
    let id = resolve(workspace, &args.id, style)?;

    let mut change = Edit::new();
    if let Some(title) = &args.title {
        change = change.title(title.clone());
    }
    if args.no_title {
        change = change.clear_title();
    }
    if let Some(message) = &args.message {
        change = change.body(message.clone());
    }

    // No flags means "open it": that is what `jot edit <id>` reads as.
    let note = load(workspace, id)?;
    if change.is_empty() {
        let seed = editor::seed(&note.frontmatter, &note.body, &workspace.manifest().schema);
        let edited = editor::edit(workspace.schema(), &seed).map_err(Failure::runtime)?;
        if edited.unchanged {
            eprintln!("jot: no changes");
            return Ok(());
        }
        warn_dropped_keys(&note, &edited);

        change = Edit {
            body: Some(edited.body),
            title: field(edited.frontmatter.title),
            quote: field(edited.frontmatter.quote),
        };
    }

    // The same rule `new` capture applies, applied to what the note would become: a title *or* a
    // body, either one on its own being enough. Emptying both is not an edit anyone means to make
    // — `jot remove` is how a note goes away — so it is refused rather than written.
    if would_be_blank(&note, &change) {
        eprintln!("jot: that would leave no title and no body; nothing written");
        eprintln!(
            "hint: `jot remove {}` is how a note goes away",
            style.show(id)
        );
        return Ok(());
    }

    let note = workspace.edit(id, change).map_err(anyhow::Error::from)?;
    if cli.json {
        emit(&output::note_json(&note, state_of(workspace, id)))?;
    } else {
        println!("{}", style.show(note.id));
    }
    Ok(())
}

/// Whether applying `change` would leave the note with neither a title nor a body.
///
/// Both halves have to be read through the change, not off the file: `--no-title` on a note whose
/// body is already blank empties it just as surely as `-m ""` on an untitled one does.
fn would_be_blank(note: &Note, change: &Edit) -> bool {
    use jot_core::query::Field;

    let title = match &change.title {
        Field::Unchanged => note.frontmatter.title.as_deref(),
        Field::Cleared => None,
        Field::Set(title) => Some(title.as_str()),
    };
    let body = change.body.as_deref().unwrap_or(&note.body);

    title.unwrap_or_default().trim().is_empty() && body.trim().is_empty()
}

/// An editor round-trip states every field, so absence means "removed", not "unchanged".
fn field<T>(value: Option<T>) -> jot_core::query::Field<T> {
    value.map_or(jot_core::query::Field::Cleared, jot_core::query::Field::Set)
}

/// Warn when the editor changed a key the [`Edit`] type cannot carry back.
///
/// Silence here would be the worst option: the key would look edited in the buffer, be preserved
/// from the file, and differ from what was typed with nothing said about it.
fn warn_dropped_keys(before: &Note, after: &editor::Edited) {
    let names = |fm: &jot_core::frontmatter::Frontmatter| {
        let mut names: Vec<(String, String)> = fm
            .unknown()
            .iter()
            .map(|key| (key.name().to_owned(), key.source().to_owned()))
            .collect();
        names.sort();
        names
    };
    if names(&before.frontmatter) != names(&after.frontmatter) {
        eprintln!(
            "jot: warning: frontmatter keys jot does not interpret were changed in the editor \
             and were not applied — they are preserved from the file as it was.\n\
             hint: edit those keys in the note file directly."
        );
    }
    if after.frontmatter.reply_to != before.frontmatter.reply_to {
        eprintln!(
            "jot: warning: `relation:reply_to` cannot be changed by an edit — re-parenting is not \
             supported — so the original was kept."
        );
    }
}

/// `jot rm`.
fn remove(workspace: &mut Workspace, args: &IdArgs, style: &Style) -> Result<(), Failure> {
    let id = resolve(workspace, &args.id, style)?;
    let replies = workspace.thread(id).map_or(0, |t| t.tree.len() - 1);
    workspace.trash(id).map_err(anyhow::Error::from)?;

    eprintln!("jot: trashed {}", style.show(id));
    if replies > 0 {
        // Trash never cascades, and someone who just trashed a note with a live subtree under it
        // should be told rather than discovering it later.
        eprintln!("jot: {replies} replies below it stay live; trash never cascades");
    }
    Ok(())
}

/// `jot restore`.
fn restore(workspace: &mut Workspace, args: &IdArgs, style: &Style) -> Result<(), Failure> {
    let id = resolve(workspace, &args.id, style)?;
    workspace.restore(id).map_err(anyhow::Error::from)?;
    eprintln!("jot: restored {}", style.show(id));
    Ok(())
}

/// `jot purge` — the only irreversible command, so it confirms.
fn purge(workspace: &mut Workspace, args: &PurgeArgs, style: &Style) -> Result<(), Failure> {
    let id = resolve(workspace, &args.id, style)?;

    if !args.yes {
        let meta = workspace.meta(id);
        let title = meta
            .and_then(|meta| meta.title.clone())
            .unwrap_or_else(|| "Untitled".into());
        if !confirm(&format!(
            "permanently delete {id} ({title})? this cannot be undone"
        ))? {
            return Ok(());
        }
    }

    workspace.purge(id).map_err(anyhow::Error::from)?;
    eprintln!("jot: purged {}", style.show(id));
    Ok(())
}

/// `jot trash`.
fn trash(workspace: &Workspace, cli: &Cli, style: &Style) -> Result<(), Failure> {
    let rows = workspace.trashed();
    if cli.json {
        emit(&json!(
            rows.iter().map(output::row_json).collect::<Vec<_>>()
        ))?;
    } else if rows.is_empty() {
        eprintln!("jot: the trash is empty");
    } else {
        for row in &rows {
            println!("{}", output::row(row, style));
        }
    }
    Ok(())
}

/// `jot search`.
fn search(
    workspace: &Workspace,
    args: &SearchArgs,
    cli: &Cli,
    style: &Style,
) -> Result<(), Failure> {
    let mut query = SearchQuery::new(&args.query);
    if args.trashed {
        query = query.include_trashed();
    }
    if let Some(since) = &args.since {
        query.since = Some(parse_when(since).map_err(Failure::runtime)?);
    }
    if let Some(until) = &args.until {
        query.until = Some(parse_when(until).map_err(Failure::runtime)?);
    }

    let rows = workspace.search(&query);
    if cli.json {
        emit(&json!(
            rows.iter().map(output::row_json).collect::<Vec<_>>()
        ))?;
    } else if rows.is_empty() {
        eprintln!("jot: nothing matches `{}`", args.query);
    } else {
        for row in &rows {
            println!("{}", output::row(row, style));
        }
    }
    Ok(())
}

/// `jot links`.
fn links(workspace: &Workspace, args: &IdArgs, cli: &Cli, style: &Style) -> Result<(), Failure> {
    let id = resolve(workspace, &args.id, style)?;
    let outgoing = workspace.links_in(id).map_err(anyhow::Error::from)?;
    let backlinks = workspace.backlinks(id);
    let quoted_by = workspace.quoted_by(id);

    if cli.json {
        emit(&json!({
            "id": id.to_string(),
            "links_out": outgoing.iter()
                .map(|(link, target)| output::link_json(link, target))
                .collect::<Vec<_>>(),
            "links_in": backlinks.iter().map(output::meta_json).collect::<Vec<_>>(),
            "quoted_by": quoted_by.iter().map(output::meta_json).collect::<Vec<_>>(),
        }))?;
    } else {
        print!(
            "{}",
            output::links(&outgoing, &backlinks, &quoted_by, style)
        );
    }
    Ok(())
}

/// `jot index`.
fn index(workspace: &mut Workspace, command: &IndexCommand, cli: &Cli) -> Result<(), Failure> {
    let report = match command {
        IndexCommand::Status => workspace.sync().map_err(anyhow::Error::from)?,
        IndexCommand::Rebuild => workspace.rebuild().map_err(anyhow::Error::from)?,
    };
    let (active, trashed) = workspace.counts();

    if cli.json {
        emit(&json!({
            "root": workspace.root().display().to_string(),
            "name": workspace.name(),
            "active": active,
            "trashed": trashed,
            "problems": report.problems.iter().map(std::string::ToString::to_string)
                .collect::<Vec<_>>(),
        }))?;
    } else {
        println!(
            "workspace  {} ({})",
            workspace.name(),
            workspace.root().display()
        );
        println!("notes      {active} active, {trashed} trashed");
        println!("problems   {}", report.problems.len());
        for problem in &report.problems {
            println!("  {problem}");
        }
    }
    Ok(())
}

/// `jot ws` — the one command group that works without an existing workspace.
fn workspaces(command: &WsCommand, cli: &Cli, style: &Style) -> Result<(), Failure> {
    let mut registry = context::load_registry()?;

    match command {
        WsCommand::List => {
            let entries: Vec<&Entry> = registry.entries().collect();
            if cli.json {
                emit(&json!(
                    entries
                        .iter()
                        .map(|entry| json!({
                            "id": entry.id().to_string(),
                            "name": entry.name(),
                            "path": entry.path().display().to_string(),
                            "current": registry.current() == Some(entry.id()),
                            "stale": entry.is_stale(),
                            "last_opened": entry.last_opened().to_rfc3339(),
                        }))
                        .collect::<Vec<_>>()
                ))?;
            } else if entries.is_empty() {
                eprintln!("jot: no registered workspaces — try `jot ws new <path>`");
            } else {
                // Workspace ids are UUIDv7 like note ids, so they carry the same timestamp prefix
                // and need the same treatment: two workspaces created in one minute would share
                // eight characters. The set to be unique within is the registry.
                let short = shortid::abbreviate(entries.iter().map(|e| e.id()), MIN_ID_WIDTH);
                for entry in entries {
                    let id_text = match style.width {
                        IdWidth::Long => entry.id().to_string(),
                        IdWidth::Abbreviated => short
                            .get(&entry.id())
                            .cloned()
                            .unwrap_or_else(|| entry.id().to_string()),
                    };
                    let current = registry.current() == Some(entry.id());
                    println!("{}", output::workspace(entry, &id_text, current, style));
                }
            }
        }

        WsCommand::Use(selector) => {
            let id = selector.resolve(&registry)?;
            registry.set_current(id);
            context::save_registry(&registry)?;
            let name = registry.get(id).map_or("?", Entry::name);
            eprintln!("jot: now using `{name}` ({id})");
        }

        WsCommand::Add { path } => {
            let workspace = Workspace::open(path)
                .with_context(|| format!("`{}`", path.display()))
                .map_err(Failure::runtime)?;
            register(&mut registry, &workspace)?;
            eprintln!("jot: registered `{}`", workspace.name());
        }

        WsCommand::Remove(selector) => {
            let id = selector.resolve(&registry)?;
            let entry = registry
                .remove(id)
                .expect("the selector returned a live id");
            // `Registry::remove` deliberately leaves a dangling `current` alone — that is its
            // documented behaviour and right for a library — so the surface is what has to notice.
            // A `current` pointing at an entry that no longer exists would make the next bare
            // `jot new` fail with nothing to act on.
            if registry.current() == Some(id) {
                registry.clear_current();
            }
            context::save_registry(&registry)?;

            eprintln!("jot: unregistered `{}` ({id})", entry.name());
            // Said every time, unprompted. `remove` is about the *registry*, and someone typing it
            // after `jot rm` — which does move a file — should not have to wonder.
            eprintln!(
                "jot: the directory is untouched: {}",
                entry.path().display()
            );
        }

        WsCommand::Prune { yes } => {
            let stale: Vec<(Uuid, String, PathBuf)> = registry
                .stale_entries()
                .map(|entry| {
                    (
                        entry.id(),
                        entry.name().to_owned(),
                        entry.path().to_path_buf(),
                    )
                })
                .collect();

            if stale.is_empty() {
                eprintln!("jot: nothing to prune — every registered workspace is present");
                return Ok(());
            }

            for (id, name, path) in &stale {
                eprintln!("  {id}  {name}  {}", path.display());
            }
            // Confirmed even though nothing on disk is deleted, because "stale" is
            // `!path.exists()` — and an external drive that is merely **unmounted** looks exactly
            // like a vault that is gone. Pruning that loses the registration for a vault whose
            // notes are perfectly fine, and re-adding it means finding the path again.
            if !yes
                && !confirm(&format!(
                    "unregister {} workspace(s) whose directory is missing? \
                     check that none is on an unmounted drive",
                    stale.len()
                ))?
            {
                return Ok(());
            }

            for (id, _, _) in &stale {
                registry.remove(*id);
                if registry.current() == Some(*id) {
                    registry.clear_current();
                }
            }
            context::save_registry(&registry)?;
            eprintln!("jot: pruned {} workspace(s)", stale.len());
        }

        WsCommand::New { path } => {
            let workspace = Workspace::init(path).map_err(anyhow::Error::from)?;
            register(&mut registry, &workspace)?;
            eprintln!(
                "jot: created `{}` at {}",
                workspace.name(),
                workspace.root().display()
            );
        }
    }
    let _ = style;
    Ok(())
}

/// Find the workspace whose id starts with `prefix`.
///
/// The default path, because the id is the identity. Matching is case-insensitive against the
/// hyphenated form, so a full id and a short prefix both work — the same rule
/// [`Snapshot::resolve`](jot_core::snapshot::Snapshot::resolve) uses for notes.
fn resolve_workspace_by_id(
    registry: &jot_core::registry::Registry,
    prefix: &str,
) -> Result<Uuid, Failure> {
    let needle = prefix.trim().to_ascii_lowercase();
    let matched: Vec<&Entry> = registry
        .entries()
        .filter(|entry| !needle.is_empty() && entry.id().to_string().starts_with(&needle))
        .collect();

    match matched.len() {
        1 => Ok(matched[0].id()),
        0 => Err(Failure {
            error: anyhow::anyhow!(
                "no registered workspace has an id starting with `{prefix}`\n\
                 hint: `jot workspace list` shows the ids, or select by name with \
                 `--name {prefix}`"
            ),
            code: EXIT_NOT_FOUND,
        }),
        _ => Err(ambiguous_workspace(prefix, &matched, "start with that id")),
    }
}

/// Find the workspace called `name`.
///
/// Only ever an exact match, and it can legitimately find several: a workspace name is display-only
/// and **nothing enforces uniqueness**, so two vaults can answer to one name. That is reported with
/// their ids rather than guessed at, which is the whole reason the id is what a bare argument means.
fn resolve_workspace_by_name(
    registry: &jot_core::registry::Registry,
    name: &str,
) -> Result<Uuid, Failure> {
    let matched: Vec<&Entry> = registry
        .entries()
        .filter(|entry| entry.name() == name)
        .collect();

    match matched.len() {
        1 => Ok(matched[0].id()),
        0 => Err(Failure {
            error: anyhow::anyhow!(
                "no registered workspace is named `{name}`\n\
                 hint: `jot workspace list` shows the names"
            ),
            code: EXIT_NOT_FOUND,
        }),
        _ => Err(ambiguous_workspace(name, &matched, "share that name")),
    }
}

/// The candidate listing both ambiguity paths print.
fn ambiguous_workspace(target: &str, candidates: &[&Entry], why: &str) -> Failure {
    let mut listing = format!(
        "`{target}` is ambiguous: {} workspaces {why}:\n",
        candidates.len()
    );
    for entry in candidates {
        listing.push_str(&format!(
            "  {}  {}  {}\n",
            entry.id(),
            entry.name(),
            entry.path().display()
        ));
    }
    listing.push_str("hint: select one by its id, or a unique prefix of it");
    Failure {
        error: anyhow::anyhow!(listing),
        code: EXIT_AMBIGUOUS,
    }
}

/// Add a workspace to the registry and make it current.
///
/// Making it current is the right default for both `add` and `new`: you asked for it by name, so
/// it is the one you meant to work in.
///
/// Any *other* entry pointing at the same directory is dropped first. The registry keys on the
/// workspace id, which is correct — that id is the vault's identity and survives the folder being
/// moved — but it means a directory that was deleted and remade carries a new id, and the old
/// entry would linger as a second row naming the same path. One path is one workspace to a person
/// looking at `jot ws ls`, so the stale row goes.
fn register(registry: &mut jot_core::registry::Registry, workspace: &Workspace) -> Result<()> {
    let id = workspace.id();
    let stale: Vec<uuid::Uuid> = registry
        .entries()
        .filter(|entry| entry.id() != id && entry.path() == workspace.root())
        .map(Entry::id)
        .collect();
    for old in stale {
        registry.remove(old);
    }

    registry.upsert(Entry::new(
        id,
        workspace.root().to_path_buf(),
        workspace.name(),
        Utc::now(),
    ));
    registry.set_current(id);
    context::save_registry(registry)
}

// =============================================================================================
// Shared helpers
// =============================================================================================

/// Ask a yes/no question on stderr. `false` means the caller should stop.
///
/// Defaults to no, and anything that is not `y`/`yes` is a no: a prompt that proceeds on a stray
/// keystroke is not a confirmation. The question goes to stderr so a piped stdout stays clean.
fn confirm(question: &str) -> Result<bool, Failure> {
    eprint!("jot: {question} [y/N] ");
    std::io::stderr().flush().map_err(anyhow::Error::from)?;

    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("cannot read a confirmation")
        .map_err(Failure::runtime)?;

    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        return Ok(true);
    }
    eprintln!("jot: cancelled");
    Ok(false)
}

/// Write a JSON value to stdout, one document, newline-terminated.
fn emit(value: &serde_json::Value) -> Result<(), Failure> {
    let text = serde_json::to_string_pretty(value)
        .context("cannot serialize the result as JSON")
        .map_err(Failure::runtime)?;
    println!("{text}");
    Ok(())
}

/// Parse a `--since` / `--until` value: a duration like `2d`, or an RFC 3339 date.
///
/// Relative first, because that is what gets typed. `3h` means "three hours ago", which reads the
/// way the flag reads and not the way a duration usually does.
fn parse_when(text: &str) -> Result<DateTime<Utc>> {
    let text = text.trim();
    if let Some(duration) = parse_duration(text) {
        return Ok(Utc::now() - duration);
    }
    if let Ok(when) = DateTime::parse_from_rfc3339(text) {
        return Ok(when.with_timezone(&Utc));
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Ok(date
            .and_hms_opt(0, 0, 0)
            .expect("midnight is a valid time")
            .and_utc());
    }
    bail!("cannot read `{text}` as a time (try `2d`, `6h`, or `2026-08-31`)")
}

/// `90m`, `6h`, `2d`, `3w` — a number and a unit.
fn parse_duration(text: &str) -> Option<Duration> {
    let split = text.find(|c: char| !c.is_ascii_digit())?;
    if split == 0 {
        return None;
    }
    let value: i64 = text[..split].parse().ok()?;
    match &text[split..] {
        "m" => Some(Duration::minutes(value)),
        "h" => Some(Duration::hours(value)),
        "d" => Some(Duration::days(value)),
        "w" => Some(Duration::weeks(value)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_tree_is_internally_consistent() {
        // clap's own assertions: conflicting flags that name missing arguments, duplicate short
        // options, and so on. Cheap, and it catches a mis-wired `conflicts_with` at test time.
        Cli::command().debug_assert();
    }

    #[test]
    fn a_relative_duration_reads_as_time_ago() {
        let two_days = parse_when("2d").unwrap();
        let elapsed = Utc::now() - two_days;
        assert!((elapsed.num_hours() - 48).abs() <= 1, "{elapsed}");
    }

    #[test]
    fn every_duration_unit_parses() {
        for (text, expected) in [
            ("90m", Duration::minutes(90)),
            ("6h", Duration::hours(6)),
            ("2d", Duration::days(2)),
            ("3w", Duration::weeks(3)),
        ] {
            assert_eq!(parse_duration(text), Some(expected), "{text}");
        }
    }

    #[test]
    fn a_plain_date_parses_as_midnight_utc() {
        let when = parse_when("2026-08-31").unwrap();
        assert_eq!(when.to_rfc3339(), "2026-08-31T00:00:00+00:00");
    }

    #[test]
    fn an_rfc_3339_timestamp_parses() {
        assert!(parse_when("2026-08-31T12:00:00Z").is_ok());
    }

    #[test]
    fn nonsense_is_refused_rather_than_silently_meaning_now() {
        for text in ["tomorrow", "", "d", "2x", "-1d"] {
            assert!(parse_when(text).is_err(), "`{text}` should not parse");
        }
    }

    #[test]
    fn an_editor_round_trip_treats_an_absent_field_as_a_removal() {
        // Every field is stated in the buffer, so a title that is gone was deleted on purpose.
        assert!(matches!(
            field(None::<String>),
            jot_core::query::Field::Cleared
        ));
        assert!(matches!(
            field(Some("t".to_owned())),
            jot_core::query::Field::Set(_)
        ));
    }
}
