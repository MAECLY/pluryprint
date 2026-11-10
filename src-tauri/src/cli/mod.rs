//! Headless CLI and the argv contract (FR-008/009/010/112, NFR-014).
//!
//! The binary has three possible starts, decided by [`startup`] from argv
//! **before** anything else happens — no Tauri builder, no webview, no queue:
//!
//! | argv                                        | mode         | window? |
//! | ------------------------------------------- | ------------ | ------- |
//! | `--invoke=context-menu --files-from <PATH>`  | context menu | yes, pre-loaded |
//! | `--invoke=context-menu … -- <PATHS>` (Linux) | context menu | yes, pre-loaded |
//! | `print …` / `printers …` / … / `--cli` / `--help` | headless | **no** |
//! | anything else                                | GUI          | yes     |
//!
//! Order matters and matches `docs/04-architecture/ipc-and-cli.md` §3.1:
//! `--invoke=context-menu` wins over everything (a shell extension must never
//! be reinterpreted as a subcommand), then a known subcommand or `--cli`, then
//! the GUI.
//!
//! ## Why the decision lives here and not in `run()`
//!
//! `pluryprint print --files a.pdf` must not create a window, so the split
//! has to happen in `main` before [`crate::run_with`] is even called.
//! [`startup`] therefore returns either "the work is done, exit with this
//! code" or "boot the GUI, and here is the selection to pre-load".
//!
//! ## What the CLI is allowed to be
//!
//! A facade. Every subcommand calls the same path-injected core the matching
//! Tauri command calls (`ipc::profiles::*_at`, `ipc::settings::*_at`,
//! `platform::*`, `update::*`, `queue::persist`), so there is exactly one
//! implementation of every rule. The only logic that lives here is what only
//! a command line needs: argument vocabulary, name→id lookup, file expansion,
//! output formatting and exit codes.

pub mod args;
pub mod console;
pub mod exit;
pub mod files;
pub mod foreground;
pub mod handoff;
pub mod instance;
pub mod output;
pub mod pending;
pub mod print;
pub mod printers;
pub mod profiles;
pub mod queue;
pub mod single_instance;
pub mod update;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::cli::args::{Cli, Command, ContextAction, InvokeSource, SUBCOMMANDS};
use crate::cli::exit::{CliError, ExitCode};
use crate::cli::output::Emitter;

/// A file selection handed to the app by a shell extension, a forwarded second
/// launch, or a context-menu-style command line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct Selection {
    /// Absolute paths, exactly as the integration reported them.
    pub paths: Vec<String>,
    /// What the user asked for (open the dialog, print with the default
    /// profile, just queue it).
    pub action: ContextAction,
    /// Profile id/name to apply, when the integration specified one.
    pub profile_id: Option<String>,
    /// Set when the integration had to cut a larger selection down to the
    /// handoff cap: the number of items the user actually selected. `paths`
    /// then holds only the first ones, and the UI says so.
    #[serde(default)]
    pub truncated_from: Option<u32>,
}

/// What `main` should do next.
///
/// Internal to the process: unlike [`Selection`] this never crosses the IPC
/// boundary, which is why it can carry the handoff path the frontend has no
/// business knowing about.
#[derive(Debug)]
pub enum Startup {
    /// The run already happened headlessly; exit with this code.
    Exit(i32),
    /// Boot the GUI, pre-loading `selection` when there is one.
    Gui {
        /// The parsed context-menu selection, or `None` for a plain launch.
        selection: Option<Selection>,
        /// The handoff file `selection` was read out of, still on disk.
        ///
        /// It is deleted by whichever process ends up parking the selection —
        /// this one once its window is up, or the already-running instance the
        /// single-instance plugin forwards our argv to. See
        /// [`handoff::discard`]; `None` whenever there is nothing to delete
        /// (a plain launch, or a selection we refused).
        handoff: Option<PathBuf>,
    },
}

/// The on-disk files every command reads through. Injected rather than resolved
/// inside each command so tests can point a whole subcommand at a tempdir
/// instead of the developer's real configuration.
#[derive(Debug, Clone)]
pub struct Paths {
    /// `config.toml`.
    pub config: PathBuf,
    /// `profiles.toml`.
    pub profiles: PathBuf,
    /// `queue.json`.
    pub queue: PathBuf,
}

impl Paths {
    /// The real per-user locations (see [`crate::util::paths`]).
    pub fn real() -> Self {
        Self {
            config: crate::settings::Config::config_path(),
            profiles: crate::profiles::profiles_path(),
            queue: crate::util::paths::data_dir().join("queue.json"),
        }
    }
}

/// Which of the three starts an argv asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// Open the app with a selection from a shell extension.
    ContextMenu,
    /// Run a subcommand and exit; no window.
    Headless,
    /// Plain launch.
    Gui,
}

/// Classify an argv without parsing it.
///
/// Deliberately cheap and total: it must reach a verdict for *any* argv,
/// including the platform noise a GUI launch carries (macOS passes
/// `-psn_0_12345` to bundled apps). The rule is the documented one — the first
/// **non-flag** argument decides — so an unknown leading flag can never turn a
/// double-click into a failed CLI run.
pub fn classify<S: AsRef<str>>(args: &[S]) -> Intent {
    let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();

    // 1. A context-menu invocation outranks everything else.
    if args.contains(&"--invoke=context-menu")
        || args
            .windows(2)
            .any(|w| w[0] == "--invoke" && w[1] == "context-menu")
    {
        return Intent::ContextMenu;
    }

    // 2. An explicit `--cli`, a help/version request, or a known subcommand.
    for arg in &args {
        match *arg {
            "--cli" | "--help" | "-h" | "--version" | "-V" => return Intent::Headless,
            a if a.starts_with('-') => continue,
            a if SUBCOMMANDS.contains(&a) => return Intent::Headless,
            // A bare path is an "open with" launch: the shell handed us a file
            // to show, not a command to run.
            a if Path::new(a).exists() => return Intent::Gui,
            // Anything else is a typo. Routing it to the CLI lets clap answer
            // with "unrecognized subcommand" and a non-zero code; routing it to
            // the GUI would silently open a window for `pluryprint frobnicate`.
            _ => return Intent::Headless,
        }
    }

    // 3. Anything else is a plain launch.
    Intent::Gui
}

/// End the process with `code` (FR-009) after flushing stdout and stderr.
///
/// `std::process::exit` runs no destructors, so a `print!` without a trailing
/// newline would still sit in stdout's line buffer and be lost — exactly the
/// output a script piping `--json` is waiting for.
pub fn exit_process(code: i32) -> ! {
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(code)
}

/// Decide what this process does, and — in headless mode — do it.
///
/// `argv` includes the program name, exactly as [`std::env::args_os`] yields
/// it. This is the single entry point `main` calls.
pub fn startup<I, T>(argv: I) -> Startup
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let argv: Vec<OsString> = argv.into_iter().map(Into::into).collect();
    let tail: Vec<String> = argv
        .iter()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();

    match classify(&tail) {
        Intent::Gui => {
            // At launch the working directory *is* the caller's, so a relative
            // path typed in a terminal can be anchored here. A launch forwarded
            // to an already running instance is anchored there instead, against
            // the cwd the single-instance plugin reports for it.
            let cwd = std::env::current_dir().ok();
            Startup::Gui {
                selection: open_with_selection(&anchor_relative(&tail, cwd.as_deref())),
                handoff: None,
            }
        }

        Intent::ContextMenu => {
            // A malformed shell-extension invocation must not leave the user
            // staring at nothing: report it and open the app empty, which is
            // still a usable outcome (they can pick files themselves).
            // Paths after `--` (Linux) are anchored like an "open with"
            // launch's: against this process' working directory, which at
            // launch is the caller's.
            let cwd = std::env::current_dir().ok();
            let parsed = Cli::parse_context_menu(&argv)
                .map_err(context_menu_parse_error)
                .and_then(|cli| {
                    let handoff = cli.files_from.clone();
                    selection_from(&cli, cwd.as_deref())
                        .map(|selection| (selection, handoff))
                        .map_err(|e| e.error.to_string())
                });
            match parsed {
                Ok((selection, handoff)) => Startup::Gui {
                    selection: Some(selection),
                    // Only a selection we accepted carries its file along:
                    // a stale or foreign payload is not ours to delete.
                    handoff,
                },
                Err(message) => {
                    eprintln!("pluryprint: ignoring the context-menu selection: {message}");
                    Startup::Gui {
                        selection: None,
                        handoff: None,
                    }
                }
            }
        }

        Intent::Headless => {
            console::attach_parent();
            match Cli::try_parse_from(&argv) {
                Ok(cli) => Startup::Exit(execute(cli)),
                Err(err) => {
                    // `--json` promises JSON errors on stderr (§3.3), so a
                    // usage error is enveloped like any other failure. Help
                    // and version are not errors and print as clap renders
                    // them; so does everything without `--json`.
                    match parse_error_json(&err).filter(|_| wants_json(&tail)) {
                        Some(json) => eprintln!("{json}"),
                        None => {
                            let _ = err.print();
                        }
                    }
                    Startup::Exit(clap_exit_code(err.kind()).code())
                }
            }
        }
    }
}

/// Make every relative, non-flag argument absolute against `cwd` (the
/// launching process' working directory). Without a `cwd` the arguments are
/// returned as they are, and [`open_with_selection`] drops the relative ones.
pub fn anchor_relative<S: AsRef<str>>(args: &[S], cwd: Option<&Path>) -> Vec<String> {
    args.iter()
        .map(AsRef::as_ref)
        .map(|a| match cwd {
            Some(cwd) if !a.starts_with('-') && Path::new(a).is_relative() => {
                cwd.join(a).to_string_lossy().into_owned()
            }
            _ => a.to_string(),
        })
        .collect()
}

/// The selection an "open with" launch carries (`pluryprint C:\docs\a.pdf`,
/// or a file dropped on the executable): every non-flag argument that resolves
/// to an existing absolute path, as a `configure` selection — the app opens on
/// the Print Job screen with the files loaded, and nothing prints by itself.
///
/// Paths go through [`crate::util::paths::canonical_input`] (NFR-025), which
/// refuses relative ones: callers anchor them first with [`anchor_relative`]
/// against the *launching* process' working directory — its own at startup,
/// the one the single-instance plugin reports for a forwarded launch — never
/// against the folder of whichever process happens to interpret them. `None` when nothing qualifies (a
/// plain launch, macOS' `-psn_…` noise).
pub fn open_with_selection<S: AsRef<str>>(args: &[S]) -> Option<Selection> {
    let paths: Vec<String> = args
        .iter()
        .map(AsRef::as_ref)
        .filter(|a| !a.starts_with('-'))
        .filter_map(|a| crate::util::paths::canonical_input(a).ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    (!paths.is_empty()).then_some(Selection {
        paths,
        action: ContextAction::Configure,
        profile_id: None,
        // Argv is never capped, so an open-with launch is never truncated.
        truncated_from: None,
    })
}

/// Run a parsed headless command line and return its exit code. Every failure
/// is reported through the emitter, so `--json` errors land on stderr as JSON
/// and human errors as one `error: …` line.
fn execute(cli: Cli) -> i32 {
    let emitter = Emitter::new(cli.json);
    let paths = Paths::real();

    // Same log file as the GUI, never on stdout (`--json` parses stdout). A
    // run that cannot open its log still runs: the log explains failures, it
    // must never become one — so the problem goes to stderr and we carry on.
    // A CLI run can be the first launch after the rename: move MaeclyPrint's
    // files first, exactly as the GUI does (ADR-0009).
    // Its lock is held until the move is logged; the CLI never claims the
    // instance lock, and a GUI starting meanwhile waits for this.
    let crate::util::legacy::Migration {
        report: migration,
        guard: migration_guard,
    } = crate::util::legacy::migrate_current();
    if let Err(e) = crate::logging::init_cli() {
        eprintln!("pluryprint: logging disabled for this run: {e}");
    }
    crate::util::legacy::log_report(&migration);
    drop(migration_guard);
    // FR-093: a CLI run can be the first one too: the profile names it lists
    // and the menu cache it writes follow the Windows display language.
    crate::settings::locale::settle_first_run();
    // Put back a printer default a killed print left swapped (D16).
    #[cfg(windows)]
    crate::engine::print::devmode_swap::recover_stale_journals();
    tracing::info!(
        event = "cli_invoked",
        command = cli.command.as_ref().map(command_name).unwrap_or("none"),
        json = cli.json,
    );

    let result = match cli.command {
        Some(Command::Print(args)) => print::run(&paths, emitter, args),
        Some(Command::Printers { cmd }) => printers::run(emitter, cmd).map(|_| ExitCode::Success),
        Some(Command::Profiles { cmd }) => {
            profiles::run(&paths, emitter, cmd).map(|_| ExitCode::Success)
        }
        Some(Command::Queue { cmd }) => queue::run(&paths, emitter, cmd).map(|_| ExitCode::Success),
        Some(Command::Update { cmd }) => {
            update::run(&paths, emitter, cmd).map(|_| ExitCode::Success)
        }
        // `--cli` (or a bare `--invoke`) with nothing to do: show the help and
        // report invalid usage, the same as any other incomplete command line.
        None => {
            use clap::CommandFactory;
            let _ = Cli::command().print_help();
            Err(CliError::usage("no subcommand was given"))
        }
    };

    let code = match result {
        Ok(code) => {
            tracing::info!(event = "cli_finished", exit_code = code.code());
            code.code()
        }
        Err(err) => {
            // The error *code*, not its message: messages can name absolute
            // paths, which NFR-082 keeps out of WARN records.
            let code = serde_json::to_value(&err.error)
                .ok()
                .and_then(|v| v.get("code").and_then(|c| c.as_str()).map(str::to_owned))
                .unwrap_or_default();
            tracing::warn!(
                event = "cli_failed",
                exit_code = err.exit.code(),
                error_code = %code,
            );
            emitter.fail(&err);
            err.exit.code()
        }
    };
    // `main` exits with `process::exit`, which skips destructors.
    crate::logging::flush();
    code
}

/// The subcommand's name for the log (never its arguments: those can carry
/// absolute paths, which NFR-082 keeps out of routine records).
fn command_name(cmd: &Command) -> &'static str {
    match cmd {
        Command::Print(_) => "print",
        Command::Printers { .. } => "printers",
        Command::Profiles { .. } => "profiles",
        Command::Queue { .. } => "queue",
        Command::Update { .. } => "update",
    }
}

/// Build the [`Selection`] a context-menu invocation carries, reading the
/// handoff file from the real handoff directory. `cwd` is the launching
/// process' working directory, which anchors relative paths given after `--`
/// (see [`argv_selection`]).
pub fn selection_from(cli: &Cli, cwd: Option<&Path>) -> Result<Selection, CliError> {
    selection_from_at_cwd(&handoff::handoff_dir(), cli, cwd)
}

/// Whether this build honours a context-menu selection given as paths after
/// `--`. Only Linux does: its file-manager actions are static `.desktop` and
/// `.nemo_action` lines (`src-tauri/linux/`) that can expand `%F` but cannot
/// write a handoff file. The Windows and macOS integrations keep the handoff
/// file as their only transport, so their contract is unchanged.
///
/// Taking the selection from argv opens no new door: whoever can run
/// `pluryprint --invoke=context-menu --action print-default -- a.pdf` can run
/// `pluryprint print --files a.pdf` just as well. The handoff file's
/// containment and freshness checks exist because *its path* is the input,
/// and a stale or planted file must not be replayed; a path on the command
/// line is checked like every other input path ([`argv_selection`]).
pub const ARGV_SELECTION: bool = cfg!(target_os = "linux");

/// Build the [`Selection`] a context-menu invocation carries, out of a handoff
/// file that must live inside `dir`.
///
/// The handoff file is the only transport (see [`handoff`]), and it is treated
/// as untrusted input: it must sit inside the handoff directory and be recent,
/// or it is refused. The payload is authoritative for `action`/`profile_id`;
/// the `--action`/`--profile` flags are the fallback for a payload that omits
/// them (or for a hand-typed invocation).
///
/// The file is **read, never deleted**. The same argv is interpreted twice
/// whenever a launch is forwarded to an already-running instance — once by the
/// launching process, once by the instance that actually owns the window — so
/// consuming it here would leave the second reader with a missing file and the
/// user with nothing (NFR-014). [`handoff::discard`] does the deleting, in the
/// process that parked the selection.
///
/// `dir` is injected so the whole gate can be exercised against a tempdir
/// instead of the developer's real data directory.
pub fn selection_from_at(dir: &Path, cli: &Cli) -> Result<Selection, CliError> {
    selection_from_at_cwd(dir, cli, None)
}

/// [`selection_from_at`], with the launching process' working directory to
/// anchor relative paths given after `--` (Linux, [`ARGV_SELECTION`]). A
/// selection read from a handoff file ignores `cwd`: its paths are absolute
/// by contract.
pub fn selection_from_at_cwd(
    dir: &Path,
    cli: &Cli,
    cwd: Option<&Path>,
) -> Result<Selection, CliError> {
    if cli.invoke != Some(InvokeSource::ContextMenu) {
        return Err(CliError::usage("not a context-menu invocation"));
    }
    // `Cli::parse_context_menu` refuses `--files-from` with paths after `--`, so
    // at most one of the two transports is present here.
    if !cli.paths.is_empty() {
        if !ARGV_SELECTION {
            return Err(CliError::usage(
                "--invoke=context-menu requires --files-from <PATH>; \
                 paths after -- are only accepted on Linux",
            ));
        }
        return argv_selection(&cli.paths, cli.action, cli.profile.clone(), cwd).ok_or_else(|| {
            CliError::usage("none of the paths after -- names an existing file or folder")
        });
    }
    let path = cli.files_from.as_ref().ok_or_else(|| {
        CliError::usage(if ARGV_SELECTION {
            "--invoke=context-menu requires --files-from <PATH> or paths after --"
        } else {
            "--invoke=context-menu requires --files-from <PATH>"
        })
    })?;

    if !handoff::is_within_dir(dir, path) {
        return Err(CliError::usage(format!(
            "{} is not inside the handoff directory",
            path.display()
        )));
    }

    let payload = handoff::read_file(path)?;
    if !handoff::is_fresh(
        &payload.invoked_at,
        time::OffsetDateTime::now_utc(),
        handoff::MAX_HANDOFF_AGE,
    ) {
        return Err(CliError::usage(
            "handoff file is stale; the selection was not honoured",
        ));
    }

    Ok(selection_from_payload(
        payload,
        cli.action,
        cli.profile.clone(),
    ))
}

/// The selection a Linux file-manager action carries on its command line
/// (`pluryprint --invoke=context-menu --action print-default -- %F`): the
/// paths after `--`, with the action and profile from the flags.
///
/// Every path goes through [`crate::util::paths::canonical_input`] like an
/// "open with" launch's ([`open_with_selection`]): a relative one is first
/// anchored against `cwd` (the *launching* process' folder, never this
/// one's), and one that does not exist is dropped. Unlike an open-with
/// argument, nothing after `--` is a flag, so a file named `-notes.txt` is
/// still a file. Folders pass, as they do in the Windows menu; the app
/// expands them the same way.
///
/// Portable on purpose, so every platform's tests exercise it; whether a
/// command line may use it is [`ARGV_SELECTION`]'s call. `None` when no path
/// survives: a menu entry that resolves to nothing is refused, not turned
/// into an empty dialog.
pub fn argv_selection(
    paths: &[PathBuf],
    action: ContextAction,
    profile: Option<String>,
    cwd: Option<&Path>,
) -> Option<Selection> {
    let given = paths.len();
    let paths: Vec<String> = paths
        .iter()
        .map(|p| match cwd {
            Some(cwd) if p.is_relative() => cwd.join(p),
            _ => p.clone(),
        })
        .filter_map(|p| crate::util::paths::canonical_input(&p.to_string_lossy()).ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    if paths.len() < given {
        // The count, never the paths: NFR-082 keeps them out of WARN records.
        tracing::warn!(
            event = "context_menu_paths_dropped",
            dropped = given - paths.len(),
            kept = paths.len(),
        );
    }
    (!paths.is_empty()).then_some(Selection {
        paths,
        action,
        profile_id: profile,
        // Argv is not capped on Linux the way it is on Windows, so the whole
        // selection always arrives.
        truncated_from: None,
    })
}

/// Fold a validated handoff payload and the argv fallbacks into a [`Selection`].
///
/// Split out from [`selection_from`] because it is the whole decision — what
/// the app will actually do — with none of the I/O, so it can be asserted
/// directly. The payload wins wherever it has an opinion; `--action`/`--profile`
/// cover a payload that omits one (or a hand-typed invocation).
pub fn selection_from_payload(
    payload: handoff::Handoff,
    action: ContextAction,
    profile: Option<String>,
) -> Selection {
    Selection {
        paths: payload.paths,
        action: payload.action.unwrap_or(action),
        profile_id: payload.profile_id.or(profile),
        truncated_from: payload.truncated_from,
    }
}

/// Turn a clap failure on the context-menu path into a message. Help/version
/// requests are impossible here (they would have classified as headless), so
/// every kind is a real error.
fn context_menu_parse_error(err: clap::Error) -> String {
    err.kind()
        .as_str()
        .unwrap_or("invalid arguments")
        .to_string()
}

/// Exit code for a clap outcome: printing `--help`/`--version` succeeded;
/// everything else is invalid usage (FR-009's code 2).
/// Whether the command line asked for JSON output: `--json` anywhere before
/// a `--` end-of-options marker. Read from the raw arguments because a
/// command line clap refused never becomes a parsed [`Cli`].
fn wants_json<S: AsRef<str>>(args: &[S]) -> bool {
    args.iter()
        .map(AsRef::as_ref)
        .take_while(|a| *a != "--")
        .any(|a| a == "--json")
}

/// A refused command line as the `{"error":{"code":"InvalidRequest",…}}`
/// envelope `--json` errors use, or `None` for help and version output, which
/// are not errors. The message is clap's own first line, without its
/// `error: ` prefix and the usage block that follows it.
fn parse_error_json(err: &clap::Error) -> Option<String> {
    use clap::error::ErrorKind;
    if matches!(
        err.kind(),
        ErrorKind::DisplayHelp
            | ErrorKind::DisplayVersion
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    ) {
        return None;
    }
    let rendered = err.render().to_string();
    let message = rendered
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("invalid command line");
    let message = message.strip_prefix("error:").unwrap_or(message).trim();
    Some(output::error_json(
        &crate::util::error::IpcError::InvalidRequest(message.to_string()),
    ))
}

fn clap_exit_code(kind: clap::error::ErrorKind) -> ExitCode {
    use clap::error::ErrorKind;
    match kind {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => ExitCode::Success,
        _ => ExitCode::Usage,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ipc-and-cli.md §3.3: `--json` turns errors into JSON on stderr — usage
    /// errors included. An unknown flag or a bad value used to come back as
    /// clap's plain text.
    #[test]
    fn fr_008_json_usage_errors_are_json() {
        for argv in [
            vec![
                "pluryprint",
                "print",
                "--files",
                "a.pdf",
                "--bogus",
                "--json",
            ],
            vec![
                "pluryprint",
                "--json",
                "print",
                "--copies",
                "many",
                "--files",
                "a.pdf",
            ],
            vec!["pluryprint", "print", "--json"],
        ] {
            let err = Cli::try_parse_from(&argv).unwrap_err();
            assert!(wants_json(&argv), "{argv:?}");
            let json = parse_error_json(&err).expect("an error, not help");
            let value: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(value["error"]["code"], "InvalidRequest", "{json}");
            let message = value["error"]["message"].as_str().unwrap();
            assert!(
                !message.is_empty() && !message.starts_with("error:"),
                "{message}"
            );
            assert!(!message.contains("Usage:"), "{message}");
            assert_eq!(clap_exit_code(err.kind()), ExitCode::Usage);
        }

        // Help stays help, and a `--json` after `--` is a value, not the flag.
        let help = Cli::try_parse_from(["pluryprint", "print", "--help"]).unwrap_err();
        assert!(parse_error_json(&help).is_none());
        assert!(!wants_json(&["print", "--files", "--", "--json"]));
    }

    #[test]
    fn fr_008_subcommands_switch_to_headless() {
        for sub in SUBCOMMANDS {
            assert_eq!(classify(&[*sub]), Intent::Headless, "{sub} is headless");
        }
        assert_eq!(classify(&["print", "--files", "a.pdf"]), Intent::Headless);
        assert_eq!(classify(&["--cli"]), Intent::Headless);
        assert_eq!(classify(&["--json", "queue", "status"]), Intent::Headless);
    }

    #[test]
    fn fr_112_help_and_version_are_headless() {
        for arg in ["--help", "-h", "--version", "-V"] {
            assert_eq!(classify(&[arg]), Intent::Headless);
        }
        assert_eq!(classify(&["print", "--help"]), Intent::Headless);
    }

    #[test]
    fn nfr_014_context_menu_outranks_everything() {
        assert_eq!(
            classify(&["--invoke=context-menu", "--files-from", "/tmp/s.json"]),
            Intent::ContextMenu
        );
        assert_eq!(
            classify(&["--invoke", "context-menu", "--files-from", "/tmp/s.json"]),
            Intent::ContextMenu
        );
        // Even paired with a subcommand: the shell extension always wins.
        assert_eq!(
            classify(&["print", "--invoke=context-menu"]),
            Intent::ContextMenu
        );
    }

    #[test]
    fn fr_008_anything_else_opens_the_gui() {
        let empty: [&str; 0] = [];
        assert_eq!(classify(&empty), Intent::Gui);
        // macOS hands a bundled app its process serial number.
        assert_eq!(classify(&["-psn_0_1234"]), Intent::Gui);
    }

    /// An "open with" launch must open the app *on the file*, not empty: the
    /// path becomes a `configure` selection, exactly like the context menu's
    /// "Configure print job…".
    #[test]
    fn fr_008_an_open_with_launch_preloads_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("report.pdf");
        std::fs::write(&file, b"%PDF-1.4\n").expect("write");
        // Same normalization as production: on Windows `canonicalize` yields the
        // verbatim `\\?\C:\…` form, which the app deliberately strips.
        let canonical = crate::util::paths::canonical_input(&file.to_string_lossy())
            .unwrap()
            .to_string_lossy()
            .into_owned();

        match startup([
            "pluryprint".to_string(),
            file.to_string_lossy().into_owned(),
        ]) {
            Startup::Gui { selection, handoff } => {
                let selection = selection.expect("the file is pre-loaded");
                assert_eq!(selection.paths, vec![canonical]);
                assert_eq!(selection.action, ContextAction::Configure);
                assert_eq!(selection.profile_id, None);
                assert!(handoff.is_none(), "no handoff file is involved");
            }
            other => panic!("an open-with launch opens the GUI, got {other:?}"),
        }
    }

    #[test]
    fn fr_008_a_plain_launch_preloads_nothing() {
        match startup(["pluryprint".to_string(), "-psn_0_1234".to_string()]) {
            Startup::Gui { selection, .. } => assert!(selection.is_none()),
            other => panic!("a plain launch opens the GUI, got {other:?}"),
        }
    }

    #[test]
    fn fr_008_an_existing_path_is_an_open_with_launch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("report.pdf");
        std::fs::write(&file, b"%PDF-1.4\n").expect("write");
        assert_eq!(
            classify(&[file.to_string_lossy().as_ref()]),
            Intent::Gui,
            "the shell handing us a real file must open the app on it"
        );
    }

    /// Regression: `pluryprint frobnicate` used to open a window instead of
    /// reporting an unknown subcommand, so a typo silently spawned a GUI.
    #[test]
    fn fr_009_an_unknown_word_is_a_cli_error_not_a_window() {
        assert_eq!(classify(&["frobnicate"]), Intent::Headless);
        // A path-looking argument that does not exist is still a typo, not a
        // launch: opening an empty window would hide the mistake.
        assert_eq!(classify(&["Documents/report.pdf"]), Intent::Headless);
    }

    /// Reading a handoff file must leave it on disk. The launching process
    /// reads it here and may then be forwarded to a running instance, which has
    /// to read the same file; consuming it would hand that instance a missing
    /// path and lose the user's selection (NFR-014).
    #[test]
    fn nfr_014_selection_from_reads_the_handoff_without_consuming_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sel.json");
        std::fs::write(
            &path,
            format!(
                r#"{{"schema_version":1,"source":"windows-explorer","action":"queue",
                    "invoked_at":"{}","paths":["/tmp/a.pdf"]}}"#,
                crate::queue::job::now_rfc3339()
            ),
        )
        .expect("write");

        let cli = Cli::try_parse_from([
            "pluryprint",
            "--invoke=context-menu",
            "--files-from",
            path.to_string_lossy().as_ref(),
        ])
        .expect("a well-formed context-menu argv");

        let selection = selection_from_at(dir.path(), &cli).expect("payload is honoured");
        assert_eq!(selection.paths, vec!["/tmp/a.pdf"]);
        assert_eq!(selection.action, ContextAction::Queue);
        assert!(path.exists(), "the file is the next reader's only copy");

        // And it keeps resolving until someone parks the selection.
        assert!(selection_from_at(dir.path(), &cli).is_ok());
        handoff::discard(&path);
        assert!(selection_from_at(dir.path(), &cli).is_err());
    }

    /// A payload we refused is not ours to delete, so nothing travels back to
    /// `main` that would make it delete one.
    #[test]
    fn nfr_014_a_refused_context_menu_launch_carries_no_handoff() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("sel.json");
        std::fs::write(&path, "{}").expect("write");

        // Outside the real handoff directory, so the containment gate refuses
        // it — the same verdict a planted payload gets.
        let startup = startup([
            "pluryprint".to_string(),
            "--invoke=context-menu".to_string(),
            "--files-from".to_string(),
            path.to_string_lossy().into_owned(),
        ]);
        match startup {
            Startup::Gui { selection, handoff } => {
                assert!(selection.is_none());
                assert!(handoff.is_none(), "a refused payload is left alone");
            }
            other => panic!("a context-menu launch always opens the GUI, got {other:?}"),
        }
        assert!(path.exists());
    }

    /// A Linux file-manager action's selection: existing paths, relative
    /// ones anchored at the caller's folder, missing ones dropped, and a file
    /// whose name starts with `-` still a file.
    #[test]
    fn fr_004_argv_selection_resolves_paths_like_an_open_with_launch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let canonical = |name: &str| {
            crate::util::paths::canonical_input(&dir.path().join(name).to_string_lossy())
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };
        std::fs::write(dir.path().join("a report.pdf"), b"%PDF-1.4\n").unwrap();
        std::fs::write(dir.path().join("-notes.txt"), b"hi").unwrap();
        std::fs::create_dir(dir.path().join("scans")).unwrap();

        let selection = argv_selection(
            &[
                dir.path().join("a report.pdf"),
                PathBuf::from("-notes.txt"),
                PathBuf::from("scans"),
                dir.path().join("gone.pdf"),
            ],
            ContextAction::PrintDefault,
            Some("Quick".into()),
            Some(dir.path()),
        )
        .expect("three of the four exist");
        assert_eq!(
            selection.paths,
            [
                canonical("a report.pdf"),
                canonical("-notes.txt"),
                canonical("scans")
            ]
        );
        assert_eq!(selection.action, ContextAction::PrintDefault);
        assert_eq!(selection.profile_id.as_deref(), Some("Quick"));
        assert_eq!(selection.truncated_from, None);

        // Without the caller's folder a relative path is never guessed, and
        // a selection with nothing left is no selection.
        assert!(argv_selection(
            &[PathBuf::from("-notes.txt")],
            ContextAction::Configure,
            None,
            None
        )
        .is_none());
    }

    /// Paths after `--` are a Linux transport. Windows and macOS keep the
    /// handoff file as the only one, so the same command line is refused
    /// there, exactly as it was before Linux existed.
    #[test]
    fn fr_004_argv_selections_are_honoured_on_linux_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("a.pdf");
        std::fs::write(&file, b"%PDF-1.4\n").unwrap();
        let argv = [
            "pluryprint".to_string(),
            "--invoke=context-menu".to_string(),
            "--action=queue".to_string(),
            "--".to_string(),
            file.to_string_lossy().into_owned(),
        ];
        let cli = Cli::parse_context_menu(&argv).expect("parses everywhere");
        let resolved = selection_from_at_cwd(dir.path(), &cli, None);

        match startup(argv.clone()) {
            Startup::Gui { selection, handoff } => {
                assert!(handoff.is_none(), "no handoff file is involved");
                if cfg!(target_os = "linux") {
                    let selection = selection.expect("the files are pre-loaded");
                    assert_eq!(selection.action, ContextAction::Queue);
                    assert_eq!(selection.paths.len(), 1);
                    assert_eq!(resolved.unwrap(), selection);
                } else {
                    assert!(selection.is_none(), "refused, and the app opens empty");
                    assert!(resolved.is_err());
                }
            }
            other => panic!("a context-menu launch always opens the GUI, got {other:?}"),
        }
        assert_eq!(ARGV_SELECTION, cfg!(target_os = "linux"));
    }

    /// A menu entry whose files are all gone opens the app empty rather than
    /// with an empty selection.
    #[test]
    fn fr_004_an_argv_selection_of_missing_files_is_refused() {
        let cli = Cli::parse_context_menu([
            "pluryprint",
            "--invoke=context-menu",
            "--",
            "/nonexistent/pluryprint/a.pdf",
        ])
        .unwrap();
        assert!(selection_from_at_cwd(Path::new("/nonexistent"), &cli, None).is_err());
    }

    /// The CLI's log records name the subcommand and nothing it was given:
    /// arguments carry absolute paths (NFR-082).
    #[test]
    fn fr_035_cli_log_names_the_subcommand_not_its_arguments() {
        let cli = Cli::try_parse_from(["pluryprint", "print", "--files", "/secret/a.pdf"])
            .expect("a well-formed print argv");
        assert_eq!(cli.command.as_ref().map(command_name), Some("print"));
        let cli = Cli::try_parse_from(["pluryprint", "queue", "status"]).unwrap();
        assert_eq!(cli.command.as_ref().map(command_name), Some("queue"));
    }

    #[test]
    fn fr_009_clap_outcomes_map_to_exit_codes() {
        use clap::error::ErrorKind;
        assert_eq!(clap_exit_code(ErrorKind::DisplayHelp), ExitCode::Success);
        assert_eq!(clap_exit_code(ErrorKind::DisplayVersion), ExitCode::Success);
        assert_eq!(clap_exit_code(ErrorKind::UnknownArgument), ExitCode::Usage);
        assert_eq!(
            clap_exit_code(ErrorKind::MissingRequiredArgument),
            ExitCode::Usage
        );
    }
}
