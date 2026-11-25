//! Finding the workspace a command should act on, and saying so out loud.
//!
//! # Why this is its own module
//!
//! `stage3.md` names it: **a note captured into the wrong workspace is quietly lost.** It is the
//! worst bug this surface can have, because nothing reports it — the command succeeds, prints an
//! id, and the note is somewhere you will not look. Every other CLI failure is loud.
//!
//! So resolution is one function with one documented order, it records *which* rule fired, and
//! `--verbose` prints it. Ambiguity about where a note landed is never resolved silently.

use anyhow::{Context as _, Result};
use jot_core::Error;
use jot_core::registry::{self, Registry};
use jot_core::workspace::Workspace;
use std::path::{Path, PathBuf};

/// The environment variable consulted between `--workspace` and discovery.
pub const ENV_VAR: &str = "JOT_WORKSPACE";

/// The environment variable that moves the workspace registry somewhere else.
///
/// The escape hatch `registry::default_path`'s own documentation anticipates. Two uses:
///
/// * **Tests.** `default_path` resolves through `directories`, which on Windows reads
///   `FOLDERID_RoamingAppData` through `SHGetKnownFolderPath` — a syscall, not an environment
///   variable. `XDG_CONFIG_HOME` and `HOME` therefore isolate nothing there, and a test suite that
///   registers workspaces would write into the developer's real registry. This is the only way to
///   redirect it on every platform.
/// * **People.** A portable install, or keeping work and personal workspaces in separate
///   registries.
pub const REGISTRY_ENV_VAR: &str = "JOT_REGISTRY";

/// What is said under an old-format refusal. Core's message says what changed; this says what to
/// do about it, which is a surface's business.
const OLD_FORMAT_HINT: &str = "hint: nothing was changed. Keep using the jot that wrote this \
                               workspace, or create a new one with `jot ws new <path>` and move \
                               the notes across by hand";

/// Which rule chose the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The `--workspace` flag.
    Flag,
    /// The `JOT_WORKSPACE` environment variable.
    Env,
    /// A `workspace.toml` found by walking up from the working directory.
    Discovered,
    /// The registry's current workspace.
    Registry,
}

impl Source {
    /// How `--verbose` describes this choice.
    pub const fn explain(self) -> &'static str {
        match self {
            Source::Flag => "--workspace",
            Source::Env => "the JOT_WORKSPACE environment variable",
            Source::Discovered => "a workspace.toml found from the working directory",
            Source::Registry => "the registry's current workspace",
        }
    }
}

/// An opened workspace and the reason it was the one opened.
pub struct Context {
    /// The workspace, already synced by `open`.
    pub workspace: Workspace,
    /// Which rule chose it.
    pub source: Source,
}

impl Context {
    /// Open the workspace this invocation should act on.
    ///
    /// The order is fixed:
    ///
    /// 1. `--workspace <path>`
    /// 2. `JOT_WORKSPACE`
    /// 3. [`Workspace::discover`] walking up from the working directory
    /// 4. the registry's current workspace
    ///
    /// An explicit choice (1 or 2) that does not open is an **error**, never a fallback: silently
    /// dropping to the next rule is exactly how a note lands in the wrong workspace. Discovery
    /// finding nothing is not an error, because "not inside a workspace" is the normal state that
    /// makes rule 4 useful — but discovery finding something it cannot open (an old-format
    /// workspace, a broken `workspace.toml`) is, for the same reason as 1 and 2.
    pub fn open(flag: Option<&Path>, verbose: bool) -> Result<Context> {
        let context = Context::resolve(flag)?;
        if verbose {
            eprintln!(
                "jot: using workspace `{}`, chosen by {}",
                context.workspace.root().display(),
                context.source.explain()
            );
        }
        report_settings(&context.workspace);
        Ok(context)
    }

    fn resolve(flag: Option<&Path>) -> Result<Context> {
        if let Some(path) = flag {
            let workspace = Workspace::open(path)
                .map_err(|e| opening(e, format!("--workspace `{}`", path.display())))?;
            return Ok(Context {
                workspace,
                source: Source::Flag,
            });
        }

        if let Some(path) = std::env::var_os(ENV_VAR).filter(|value| !value.is_empty()) {
            let path = PathBuf::from(path);
            let workspace = Workspace::open(&path)
                .map_err(|e| opening(e, format!("{ENV_VAR}=`{}`", path.display())))?;
            return Ok(Context {
                workspace,
                source: Source::Env,
            });
        }

        let cwd = std::env::current_dir().context("cannot read the working directory")?;
        match Workspace::discover(&cwd) {
            Ok(workspace) => {
                return Ok(Context {
                    workspace,
                    source: Source::Discovered,
                });
            }
            Err(Error::WorkspaceNotFound { .. }) => {}
            Err(e) => return Err(opening(e, String::from("the working directory"))),
        }

        // Last resort. A corrupt registry recovers to an empty one rather than failing, so this
        // reads as "no current workspace" instead of taking the command down.
        let registry = load_registry()?;
        let current = registry.current().ok_or_else(|| {
            anyhow::anyhow!(
                "not inside a jot workspace, and no current workspace is set\n\
                 hint: run `jot ws new <path>` to make one, or `jot ws use <path>` to pick one"
            )
        })?;
        let workspace = Workspace::open(current).map_err(|e| {
            opening(
                e,
                format!("the registry's current workspace `{}`", current.display()),
            )
        })?;
        Ok(Context {
            workspace,
            source: Source::Registry,
        })
    }
}

/// A failure to open a workspace, as the user should read it: `what` names how it was chosen, and
/// an old-format workspace gets a hint about what to do next.
pub fn opening(error: Error, what: String) -> anyhow::Error {
    let old = matches!(error, Error::OldFormat { .. });
    let error = anyhow::Error::from(error).context(what);
    if old {
        anyhow::anyhow!("{error:#}\n{OLD_FORMAT_HINT}")
    } else {
        error
    }
}

/// Settings warnings and errors, on stderr, without blocking the command.
///
/// A broken `schema.json` refuses writes, not reads, so a read command still runs — and says so,
/// because a listing computed from the last valid settings is not what the files say right now.
fn report_settings(workspace: &Workspace) {
    let settings = workspace.settings();
    for warning in settings.warnings() {
        eprintln!("jot: warning: {warning}");
    }
    if settings.errors().is_empty() {
        return;
    }
    for error in settings.errors() {
        eprintln!("jot: settings error: {error}");
    }
    if settings.is_stale() {
        eprintln!("jot: writes are refused until that is fixed; reads use the last valid settings");
    } else {
        eprintln!("jot: writes are refused until that is fixed");
    }
}

/// Where the registry lives for this invocation: [`REGISTRY_ENV_VAR`], else the OS config
/// directory.
pub fn registry_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(REGISTRY_ENV_VAR).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    registry::default_path().context("cannot locate the workspace registry")
}

/// Load the registry.
///
/// A registry that cannot be read or parsed recovers to an empty one — that is the crate's
/// documented behavior and the right one here: a corrupt list of workspaces must not stop you
/// capturing a note into the workspace you are standing in.
pub fn load_registry() -> Result<Registry> {
    let path = registry_path()?;
    let registry = Registry::load_from(&path)?;
    if let Some(err) = registry.recovered() {
        eprintln!("jot: warning: {err}; starting from an empty registry");
    }
    Ok(registry)
}

/// Save the registry back where [`registry_path`] says it lives.
pub fn save_registry(registry: &Registry) -> Result<()> {
    registry.save_to(&registry_path()?)?;
    Ok(())
}
