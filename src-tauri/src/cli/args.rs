//! The clap surface: subcommands, flags and their value vocabularies.
//!
//! This module is *declaration only* — no I/O, no business rules — so the whole
//! command surface can be parsed and asserted in tests without touching the
//! machine. It mirrors `docs/04-architecture/ipc-and-cli.md` §3.2/§3.3 plus the
//! entry points the shell integrations need: `--invoke=context-menu` with
//! either `--files-from` (the Windows and macOS extensions, see
//! [`super::handoff`]) or, on Linux, the paths after `--` (the Dolphin and Nemo
//! menus, see `src-tauri/linux/`).
//!
//! Flag names are public surface (§5 of the same doc): add, deprecate, but do
//! not rename.

use std::path::PathBuf;

use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

use crate::profiles::{ProfileDuplex, Quality};
use crate::queue::job::{ColorMode, Duplex, Orientation, PaperSize, Scaling};

/// Subcommand names that switch the binary into headless mode when they appear
/// as the first non-flag argument (§3.1). Kept as data so the argv classifier
/// and the parser can never disagree.
pub const SUBCOMMANDS: &[&str] = &["print", "printers", "profiles", "queue", "update", "bridge"];

/// `pluryprint` — the whole command line.
#[derive(Debug, Parser, PartialEq, Eq)]
#[command(
    name = "pluryprint",
    version,
    about = "Batch file printing for Windows, macOS and Linux.",
    long_about = "Batch file printing for Windows, macOS and Linux.\n\nRun without arguments to open the app; run a subcommand to work headlessly \
(no window is created). Exit codes are documented in docs/04-architecture/ipc-and-cli.md."
)]
pub struct Cli {
    /// How this process was started. `context-menu` opens the app with the
    /// supplied selection instead of running headlessly.
    #[arg(long, value_name = "SOURCE")]
    pub invoke: Option<InvokeSource>,

    /// Read the file selection from a JSON handoff file instead of argv. This
    /// is how the shell extensions pass a large selection (argv is capped at
    /// 32,767 characters on Windows).
    #[arg(long, value_name = "PATH")]
    pub files_from: Option<PathBuf>,

    /// What to do with a `--invoke=context-menu` selection.
    #[arg(long, value_name = "ACTION", default_value_t = ContextAction::Configure)]
    pub action: ContextAction,

    /// Profile (name or id) applied to a `--invoke=context-menu` selection.
    #[arg(long, value_name = "NAME-OR-ID")]
    pub profile: Option<String>,

    /// The files of a `--invoke=context-menu` selection given after `--`
    /// (`--invoke=context-menu --action=queue -- a.pdf b.pdf`), filled only
    /// by [`Cli::parse_context_menu`]. Linux only: a file-manager action is a
    /// static `.desktop`/`.nemo_action` line that can expand `%F` but cannot
    /// write a handoff file. Elsewhere such a selection is refused.
    ///
    /// Not a clap positional on purpose: one would turn `pluryprint
    /// frobnicate` from "unrecognized subcommand" into "unexpected argument"
    /// and add `-- <PATHS>` to every platform's usage line.
    #[arg(skip)]
    pub paths: Vec<PathBuf>,

    /// Force headless mode even without a subcommand.
    #[arg(long)]
    pub cli: bool,

    /// Emit machine-readable JSON on stdout (errors as JSON on stderr).
    #[arg(long, global = true)]
    pub json: bool,

    /// The headless subcommand; `None` means "open the app".
    #[command(subcommand)]
    pub command: Option<Command>,
}

impl Cli {
    /// Parse a `--invoke=context-menu` command line, taking everything after
    /// the first `--` as the selection's paths ([`Cli::paths`]) and the rest
    /// through clap as usual. Only the context-menu route calls this; every
    /// other command line goes through plain `try_parse_from`, so `--` there
    /// means what clap says it means.
    ///
    /// A selection has one transport: paths after `--` together with
    /// `--files-from` are refused as a conflict.
    pub fn parse_context_menu<I, T>(argv: I) -> Result<Cli, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        let mut head: Vec<std::ffi::OsString> = argv.into_iter().map(Into::into).collect();
        let paths: Vec<PathBuf> = match head.iter().position(|a| a == "--") {
            Some(at) => head
                .split_off(at)
                .into_iter()
                .skip(1)
                .map(PathBuf::from)
                .collect(),
            None => Vec::new(),
        };
        let mut cli = Cli::try_parse_from(head)?;
        if !paths.is_empty() && cli.files_from.is_some() {
            use clap::CommandFactory;
            return Err(Cli::command().error(
                clap::error::ErrorKind::ArgumentConflict,
                "--files-from cannot be used with paths after --",
            ));
        }
        cli.paths = paths;
        Ok(cli)
    }
}

/// Entry points that are not a plain terminal invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InvokeSource {
    /// Launched by the Windows Explorer / macOS Finder integration, or by a
    /// Linux file manager's action (Dolphin, Nemo).
    ContextMenu,
}

/// What a context-menu invocation asks for.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum, Serialize, Deserialize, specta::Type,
)]
#[serde(rename_all = "kebab-case")]
pub enum ContextAction {
    /// Open the print dialog pre-loaded with the selection.
    #[default]
    Configure,
    /// Print immediately with the default (or `--profile`) profile.
    PrintDefault,
    /// Add the selection to the queue without starting it.
    Queue,
}

impl std::fmt::Display for ContextAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ContextAction::Configure => "configure",
            ContextAction::PrintDefault => "print-default",
            ContextAction::Queue => "queue",
        };
        f.write_str(s)
    }
}

/// The headless subcommands (§3.2). Each runs to completion and exits
/// without creating a window.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum Command {
    /// Print one or more files.
    Print(PrintArgs),
    /// Inspect the printers installed on this machine.
    Printers {
        /// Which printers action to run.
        #[command(subcommand)]
        cmd: PrintersCmd,
    },
    /// Manage print profiles.
    Profiles {
        /// Which profiles action to run.
        #[command(subcommand)]
        cmd: ProfilesCmd,
    },
    /// Inspect and control the persisted print queue.
    Queue {
        /// Which queue action to run.
        #[command(subcommand)]
        cmd: QueueCmd,
    },
    /// Check for a newer Pluryprint release.
    Update {
        /// Which update action to run.
        #[command(subcommand)]
        cmd: UpdateCmd,
    },
    /// Register the browser bridge Pluryfact prints through (Chrome, Edge,
    /// Brave, Chromium).
    Bridge {
        /// Which bridge action to run.
        #[command(subcommand)]
        cmd: BridgeCmd,
    },
}

/// `pluryprint print …` — exactly one input source is required.
#[derive(Debug, Args, PartialEq, Eq)]
#[command(group(ArgGroup::new("input").required(true).args(["files", "files_from"])))]
pub struct PrintArgs {
    /// Files to print: a comma-separated list of paths, directories or
    /// wildcard patterns (`"*.pdf"`). Patterns are expanded by Pluryprint, so
    /// they work identically in cmd.exe and in a POSIX shell.
    #[arg(long, value_name = "FILES", value_delimiter = ',', num_args = 1..)]
    pub files: Vec<String>,

    /// Read the file list from a JSON handoff file (see `--invoke`).
    #[arg(long, value_name = "PATH")]
    pub files_from: Option<PathBuf>,

    /// Profile (name or id) to base the settings on; defaults to the app's
    /// default profile.
    #[arg(long, value_name = "NAME-OR-ID")]
    pub profile: Option<String>,

    /// Target printer; defaults to the app printer, then the OS default.
    #[arg(long, value_name = "NAME")]
    pub printer: Option<String>,

    /// Per-job overrides layered on top of the chosen profile.
    #[command(flatten)]
    pub settings: SettingFlags,

    /// Accepted for compatibility: a headless `print` never opens a window, so
    /// this is already the behaviour.
    #[arg(long)]
    pub no_dialog: bool,

    /// Accepted for compatibility: a headless `print` always runs each file to
    /// completion before exiting.
    #[arg(long)]
    pub wait: bool,

    /// Password for the encrypted PDFs of this batch, read from the first
    /// line of a file (FR-059). It is never accepted on the command line,
    /// where other users and the shell history could see it. Files that are
    /// not encrypted ignore it.
    #[arg(long, value_name = "PATH", conflicts_with = "password_stdin")]
    pub password_file: Option<PathBuf>,

    /// Read the password for the encrypted PDFs of this batch from standard
    /// input (its first line), e.g. `printf %s "$PW" | pluryprint print …`.
    #[arg(long)]
    pub password_stdin: bool,
}

/// The print-setting overrides shared by `print` and `profiles create`.
#[derive(Debug, Args, PartialEq, Eq, Default)]
pub struct SettingFlags {
    /// Print in colour.
    #[arg(long, conflicts_with = "mono")]
    pub color: bool,

    /// Print in grayscale.
    #[arg(long)]
    pub mono: bool,

    /// Double-sided mode.
    #[arg(long, value_name = "MODE", ignore_case = true)]
    pub duplex: Option<CliDuplex>,

    /// Number of copies (1-999).
    #[arg(long, value_name = "N")]
    pub copies: Option<u16>,

    /// Paper size: `a4`, `letter`, `legal`, `a3`, `auto` (the printer's own
    /// default), or any size the printer reports (`A6`, `4x6`, `Tabloid`…,
    /// see `printers list`). The four classic names and `auto` are
    /// case-insensitive; a printer size is checked against the target
    /// printer before anything is spooled.
    #[arg(long, value_name = "SIZE", value_parser = parse_paper)]
    pub paper: Option<PaperSize>,

    /// Page range, e.g. `1-5,8`.
    #[arg(long, value_name = "SPEC")]
    pub range: Option<String>,

    /// How pages are fitted to the paper.
    #[arg(long, value_name = "MODE", ignore_case = true)]
    pub scale: Option<CliScale>,

    /// Page orientation.
    #[arg(long, value_name = "MODE", ignore_case = true)]
    pub orientation: Option<CliOrientation>,

    /// Collate multiple copies (whole sets: 1-2-3, 1-2-3).
    #[arg(long, conflicts_with = "no_collate")]
    pub collate: bool,

    /// Do not collate multiple copies (each page grouped: 1-1, 2-2, 3-3).
    #[arg(long)]
    pub no_collate: bool,
}

/// `pluryprint printers …` actions.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum PrintersCmd {
    /// List every printer the OS spooler knows about.
    List,
}

/// `pluryprint profiles …` actions.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum ProfilesCmd {
    /// List every profile, built-ins first.
    List,
    /// Show one profile.
    Show {
        /// Profile name or id.
        name_or_id: String,
    },
    /// Create a custom profile.
    Create {
        /// Name of the new profile (must be unique).
        #[arg(long, value_name = "NAME")]
        name: String,
        /// Existing profile (name or id) to copy the settings from.
        #[arg(long, value_name = "NAME-OR-ID")]
        from: Option<String>,
        /// Target printer for the profile.
        #[arg(long, value_name = "NAME")]
        printer: Option<String>,
        /// Print-quality preset.
        #[arg(long, value_name = "LEVEL", ignore_case = true)]
        quality: Option<CliQuality>,
        /// Input tray.
        #[arg(long, value_name = "TRAY")]
        tray: Option<String>,
        /// Settings stored in the new profile, on top of `--from`'s.
        #[command(flatten)]
        settings: SettingFlags,
    },
    /// Delete a custom profile.
    Delete {
        /// Profile name or id.
        name_or_id: String,
    },
    /// Make a profile the app default.
    SetDefault {
        /// Profile name or id.
        name_or_id: String,
    },
}

/// `pluryprint queue …` actions.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum QueueCmd {
    /// Show the persisted queue.
    Status {
        /// Re-print the queue whenever it changes (Ctrl-C to stop).
        #[arg(long)]
        watch: bool,
    },
    /// Cancel one job.
    Cancel {
        /// Job id.
        id: String,
    },
    /// Cancel every job that has not reached a terminal state.
    CancelAll,
}

/// `pluryprint update …` actions.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum UpdateCmd {
    /// Ask whether a newer release exists (GitHub releases, or the manifest
    /// PLURYPRINT_UPDATE_ENDPOINT names).
    Check,
    /// Download, verify and install the newest release (asks first unless --yes).
    Install {
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Show or set the update channel.
    Channel {
        /// New channel; omit to print the current one.
        #[arg(value_name = "CHANNEL", ignore_case = true)]
        channel: Option<CliChannel>,
    },
}

/// `pluryprint bridge …` actions (FR-127).
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum BridgeCmd {
    /// Write the native messaging manifest so the browser can start the
    /// bridge (on Windows also the registry key that names it).
    Install {
        /// Which browser to register with.
        #[arg(
            long,
            value_name = "BROWSER",
            ignore_case = true,
            default_value = "all"
        )]
        browser: CliBrowser,
        /// Also allow this extension origin (`chrome-extension://<id>/`), for
        /// a development build of Pluryfact. Repeatable. Kept when the bridge
        /// is registered again, except on Windows, where installing or
        /// updating Pluryprint resets the manifest to Pluryfact's origin:
        /// add it again afterwards.
        #[arg(long = "extra-origin", value_name = "ORIGIN")]
        extra_origin: Vec<String>,
    },
    /// Remove the manifest (and, on Windows, the registry key).
    Uninstall {
        /// Which browser to unregister from.
        #[arg(
            long,
            value_name = "BROWSER",
            ignore_case = true,
            default_value = "all"
        )]
        browser: CliBrowser,
    },
    /// Show, per browser, whether the bridge is registered and where.
    Status,
}

/// `--browser` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliBrowser {
    /// Google Chrome.
    Chrome,
    /// Microsoft Edge.
    Edge,
    /// Brave.
    Brave,
    /// Chromium.
    Chromium,
    /// Every browser above.
    All,
}

impl CliBrowser {
    /// The browsers this value names.
    pub fn browsers(self) -> Vec<crate::bridge::register::Browser> {
        use crate::bridge::register::Browser;
        match self {
            CliBrowser::Chrome => vec![Browser::Chrome],
            CliBrowser::Edge => vec![Browser::Edge],
            CliBrowser::Brave => vec![Browser::Brave],
            CliBrowser::Chromium => vec![Browser::Chromium],
            CliBrowser::All => Browser::ALL.to_vec(),
        }
    }
}

// ---------------------------------------------------------------------------
// Value vocabularies
//
// These exist because the CLI spelling and the wire spelling differ on purpose:
// `--duplex off|long-edge|short-edge` is the profile vocabulary users already
// see in the UI, while the engine's `Duplex` is `none|long|short`; `--scale
// actual` reads better than the engine's `none`. Each enum owns its mapping so
// the translation lives in exactly one place.
// ---------------------------------------------------------------------------

/// `--duplex` values, in the profile vocabulary the UI shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliDuplex {
    /// Single-sided.
    Off,
    /// Two-sided, flipped on the long edge (book style).
    LongEdge,
    /// Two-sided, flipped on the short edge (notepad style).
    ShortEdge,
}

impl CliDuplex {
    /// The engine's spelling of this mode.
    pub fn to_engine(self) -> Duplex {
        match self {
            CliDuplex::Off => Duplex::None,
            CliDuplex::LongEdge => Duplex::Long,
            CliDuplex::ShortEdge => Duplex::Short,
        }
    }

    /// The profile's spelling of this mode.
    pub fn to_profile(self) -> ProfileDuplex {
        match self {
            CliDuplex::Off => ProfileDuplex::Off,
            CliDuplex::LongEdge => ProfileDuplex::LongEdge,
            CliDuplex::ShortEdge => ProfileDuplex::ShortEdge,
        }
    }
}

/// `--paper`'s parser: the engine's own [`PaperSize::parse`], so the CLI
/// accepts exactly the names the Print Job screen and the profile editor do.
/// It used to be a closed `a4|letter|legal|a3` list, so `--paper A6` or
/// `--paper auto` failed even for a printer that lists the size.
pub fn parse_paper(name: &str) -> Result<PaperSize, String> {
    PaperSize::parse(name)
}

/// `--scale` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliScale {
    /// Scale every page to fill the printable area.
    Fit,
    /// Shrink pages that do not fit; leave smaller pages at their size.
    Shrink,
    /// The engine's `none`: print at the source's actual size.
    Actual,
}

impl CliScale {
    /// The engine's spelling of this mode.
    pub fn to_engine(self) -> Scaling {
        match self {
            CliScale::Fit => Scaling::Fit,
            CliScale::Shrink => Scaling::Shrink,
            CliScale::Actual => Scaling::None,
        }
    }
}

/// `--orientation` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliOrientation {
    /// Pick per page from its aspect ratio.
    Auto,
    /// Force portrait.
    Portrait,
    /// Force landscape.
    Landscape,
}

impl CliOrientation {
    /// The engine's spelling of this orientation.
    pub fn to_engine(self) -> Orientation {
        match self {
            CliOrientation::Auto => Orientation::Auto,
            CliOrientation::Portrait => Orientation::Portrait,
            CliOrientation::Landscape => Orientation::Landscape,
        }
    }
}

/// `--quality` values for `profiles create`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliQuality {
    /// Fastest, lowest ink use.
    Draft,
    /// The driver's standard quality.
    Normal,
    /// Best quality the driver offers.
    High,
}

impl CliQuality {
    /// The profile's spelling of this quality.
    pub fn to_profile(self) -> Quality {
        match self {
            CliQuality::Draft => Quality::Draft,
            CliQuality::Normal => Quality::Normal,
            CliQuality::High => Quality::High,
        }
    }
}

/// `update channel` values (FR-085). Persisted only: today every channel
/// resolves to the same release feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliChannel {
    /// Final releases (the default).
    Stable,
    /// Beta pre-releases.
    Beta,
    /// Nightly builds.
    Nightly,
}

impl CliChannel {
    /// The settings' spelling of this channel.
    pub fn to_settings(self) -> crate::settings::UpdateChannel {
        use crate::settings::UpdateChannel;
        match self {
            CliChannel::Stable => UpdateChannel::Stable,
            CliChannel::Beta => UpdateChannel::Beta,
            CliChannel::Nightly => UpdateChannel::Nightly,
        }
    }
}

impl SettingFlags {
    /// The colour override these flags express, if any. `--color` and `--mono`
    /// are mutually exclusive (enforced by clap), so at most one is set.
    pub fn color_mode(&self) -> Option<ColorMode> {
        match (self.color, self.mono) {
            (true, _) => Some(ColorMode::Color),
            (_, true) => Some(ColorMode::Monochrome),
            _ => None,
        }
    }

    /// The collation override, if any. `--collate` and `--no-collate` are
    /// mutually exclusive (enforced by clap).
    pub fn collate(&self) -> Option<bool> {
        match (self.collate, self.no_collate) {
            (true, _) => Some(true),
            (_, true) => Some(false),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// clap's own consistency checks (duplicate flags, bad groups, …). Failing
    /// this means the declaration above is malformed, which would otherwise
    /// only surface as a runtime panic in front of a user.
    #[test]
    fn fr_112_command_surface_is_well_formed() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    /// The Linux file-manager form: the selection after `--`, the action as
    /// `--action=<a>`. Paths there are values even when they look like flags.
    #[test]
    fn fr_004_context_menu_paths_follow_the_end_of_options_marker() {
        let cli = Cli::parse_context_menu([
            "pluryprint",
            "--invoke=context-menu",
            "--action=queue",
            "--",
            "/home/u/a report.pdf",
            "-notes.txt",
        ])
        .expect("the Dolphin/Nemo command line parses");
        assert_eq!(cli.invoke, Some(InvokeSource::ContextMenu));
        assert_eq!(cli.action, ContextAction::Queue);
        assert_eq!(
            cli.paths,
            [
                PathBuf::from("/home/u/a report.pdf"),
                PathBuf::from("-notes.txt")
            ]
        );
        assert!(cli.files_from.is_none() && cli.command.is_none());
    }

    /// The paths are only a context-menu transport: never with a handoff
    /// file, and never part of the ordinary command line, whose errors stay
    /// what they were.
    #[test]
    fn fr_004_context_menu_paths_exclude_files_from_and_leave_clap_alone() {
        use clap::error::ErrorKind;
        let both = Cli::parse_context_menu([
            "pluryprint",
            "--invoke=context-menu",
            "--files-from",
            "/tmp/handoff/sel.json",
            "--",
            "/a.pdf",
        ])
        .unwrap_err();
        assert_eq!(both.kind(), ErrorKind::ArgumentConflict);

        // Outside the context-menu route `--` is clap's: no positional
        // takes what follows, and a typo is still an unknown subcommand.
        assert!(Cli::try_parse_from(["pluryprint", "--", "/a.pdf"]).is_err());
        let typo = Cli::try_parse_from(["pluryprint", "frobnicate"]).unwrap_err();
        assert_eq!(typo.kind(), ErrorKind::InvalidSubcommand);
        let print = Cli::try_parse_from(["pluryprint", "print", "--files", "a.pdf"]).unwrap();
        assert!(print.paths.is_empty() && matches!(print.command, Some(Command::Print(_))));
    }

    #[test]
    fn fr_008_vocabularies_map_onto_the_engine() {
        assert_eq!(CliDuplex::Off.to_engine(), Duplex::None);
        assert_eq!(CliDuplex::LongEdge.to_engine(), Duplex::Long);
        assert_eq!(CliDuplex::ShortEdge.to_engine(), Duplex::Short);
        // `actual` is the engine's `none` — the one deliberate rename.
        assert_eq!(CliScale::Actual.to_engine(), Scaling::None);
        assert_eq!(parse_paper("a4"), Ok(PaperSize::A4));
        assert_eq!(CliOrientation::Auto.to_engine(), Orientation::Auto);
    }

    #[test]
    fn fr_008_color_flags_are_exclusive_and_optional() {
        assert_eq!(SettingFlags::default().color_mode(), None);
        let color = SettingFlags {
            color: true,
            ..Default::default()
        };
        assert_eq!(color.color_mode(), Some(ColorMode::Color));
        let mono = SettingFlags {
            mono: true,
            ..Default::default()
        };
        assert_eq!(mono.color_mode(), Some(ColorMode::Monochrome));
    }
}
