//! External-dependency detection (FR-058).
//!
//! The hybrid engine (ADR-0003) shells out to programs we do not compile: the
//! final PDF printer on Windows (SumatraPDF), the best-of-breed converters the
//! architecture reserves for Office documents (LibreOffice), the HEIF/HEVC
//! codecs on Windows and, on Linux, an external HEIF decoder (libheif's tools
//! or ImageMagick). None of them is guaranteed to exist on a user's machine, so every one of them is *resolved* before it is *run*, and a
//! missing one produces a remediation message instead of an opaque
//! "failed to spawn" deep inside a job.
//!
//! # Shape
//!
//! - [`resolve`] is the whole decision and it is **pure**: it takes the
//!   candidate directories, the executable names, and an `exists` predicate,
//!   and returns where the program was found. It reads no environment variable
//!   and touches no global state, so the search order is testable against a
//!   tempdir on any OS.
//! - [`bundled_dirs`], [`path_dirs`] and [`install_dirs`] gather that input from
//!   the real machine. Only [`install_dirs`] needs environment variables, and it
//!   takes the lookup as a parameter for the same reason.
//! - [`report`] / [`report_all`] glue the two together and produce the typed
//!   [`DependencyReport`] the IPC layer ships to the UI.
//!
//! # Search order (identical for every dependency)
//!
//! 1. **Bundled** — next to our own executable, in the layout the installer
//!    produces (see `bundle.resources` in `tauri.conf.json`). A copy we ship
//!    always wins: it is the version we tested against.
//! 2. **Configured** — an explicit path the user pointed us at. There is no
//!    such setting in `config.toml` today, so callers pass `None`; the slot
//!    exists because the resolver is where the precedence has to live once one
//!    is added, and adding it later must not reshuffle the other three steps.
//! 3. **`PATH`** — the user's own install, however they manage it.
//! 4. **Well-known install directories** — the per-OS defaults the vendors'
//!    installers use.
//!
//! # Honesty
//!
//! [`Spec::blocks`] lists exactly the extensions that cannot print while this
//! dependency is missing — always a subset of
//! [`crate::engine::SUPPORTED_EXTS`], and taken from the converter routing
//! tables in [`crate::engine::convert`] so the two cannot drift. The
//! remediation text names the dependency and how to install it; the converter
//! returns the same text as `DependencyMissing` when a job needs it (FR-058).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specta::Type;

/// The operating system a resolution is being computed for.
///
/// An explicit parameter rather than `cfg!`, so the Windows search order can be
/// unit-tested from a macOS dev machine (and vice versa) — the whole point of
/// keeping [`resolve`] pure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    /// Microsoft Windows.
    Windows,
    /// Apple macOS.
    MacOs,
    /// Linux (CUPS, like macOS).
    Linux,
}

impl Os {
    /// The OS this build targets, or [`Os::Linux`] for anything else Unix-y
    /// (the engine's Unix path is CUPS either way).
    pub fn current() -> Self {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        }
    }
}

/// An external program the print pipeline can depend on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Type)]
#[serde(rename_all = "kebab-case")]
pub enum Dependency {
    /// The final PDF printer on Windows (ADR-0003). Not used elsewhere: macOS
    /// and Linux print through CUPS `lp`.
    SumatraPdf,
    /// Office → PDF converter (`soffice --headless --convert-to pdf`).
    LibreOffice,
    /// No longer used: Edge or Chrome served only the headless-browser
    /// fallback for HTML (D15), which was removed (round 6, ADR-0012); HTML,
    /// Markdown and text print through the system web view. Kept so a
    /// `queue.json` that names it still loads, and its reason stays valid. It
    /// is not in [`ALL`] and blocks nothing.
    Chromium,
    /// No longer used: Markdown converts in-process (FR-056). Kept so a
    /// `queue.json` that names it still loads, and its reason stays valid.
    Pandoc,
    /// The *HEIF Image Extensions* WIC decoder from the Microsoft Store
    /// (Windows only): without it no HEIC/HEIF photo decodes.
    HeifCodec,
    /// The *HEVC Video Extensions* decoder from the Microsoft Store (Windows
    /// only, paid unless the PC maker preinstalled it): HEIC photos are HEVC
    /// inside.
    HevcCodec,
    /// An external HEIF decoder (Linux only): libheif's `heif-dec` or
    /// `heif-convert`, or ImageMagick's `magick` or `convert`. Linux has no
    /// system decoder to ask, so the program is looked for like any other.
    HeifDecoder,
}

impl Dependency {
    /// The stable id the wire and the UI use for this dependency
    /// (`common:deps.<id>`), the same spelling serde gives it. It is also the
    /// `dependency_missing` reason a job pauses with (FR-058, FR-092).
    pub fn id(self) -> &'static str {
        match self {
            Dependency::SumatraPdf => "sumatra-pdf",
            Dependency::LibreOffice => "libre-office",
            Dependency::Chromium => "chromium",
            Dependency::Pandoc => "pandoc",
            Dependency::HeifCodec => "heif-codec",
            Dependency::HevcCodec => "hevc-codec",
            Dependency::HeifDecoder => "heif-decoder",
        }
    }
}

/// Every dependency the running app can need, in the order the UI should
/// list them. Edge/Chrome and Pandoc left this list with the system web view
/// renderer (FR-055/056, D15).
pub const ALL: &[Dependency] = &[
    Dependency::SumatraPdf,
    Dependency::LibreOffice,
    Dependency::HeifCodec,
    Dependency::HevcCodec,
    Dependency::HeifDecoder,
];

/// Which step of the search order produced the hit. Shown in diagnostics so
/// "it picked up the wrong copy" is answerable without a debugger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "kebab-case")]
pub enum FoundIn {
    /// A copy shipped with the app, next to our executable.
    Bundled,
    /// A path the user configured explicitly.
    Configured,
    /// A `PATH` entry.
    Path,
    /// A well-known install directory for this OS.
    Installed,
}

/// The outcome of a search: where the executable is, or nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// The executable exists.
    Found {
        /// Absolute path of the executable.
        path: PathBuf,
        /// Which search step found it.
        found_in: FoundIn,
    },
    /// No candidate location held the executable.
    Missing,
}

impl Resolution {
    /// The resolved path, or `None` when missing.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Resolution::Found { path, .. } => Some(path),
            Resolution::Missing => None,
        }
    }
}

/// Everything [`resolve`] is allowed to look at. Assembling one is the impure
/// half of the module; deciding from one is the pure half.
pub struct Probe<'a> {
    /// Step 1: directories a bundled copy could sit in ([`bundled_dirs`]).
    pub bundled_dirs: &'a [PathBuf],
    /// Step 2: a full path to the executable the user configured, if any.
    pub configured: Option<&'a Path>,
    /// Step 3: the entries of `PATH` ([`path_dirs`]).
    pub path_dirs: &'a [PathBuf],
    /// Step 4: vendor install directories ([`install_dirs`]).
    pub install_dirs: &'a [PathBuf],
    /// Executable file names to try inside each directory, most preferred
    /// first (Edge before Chrome, say).
    pub executables: &'a [&'a str],
    /// Whether a candidate path is an existing, usable program. Injected so
    /// tests decide with tempfiles instead of the host's real installs.
    pub exists: fn(&Path) -> bool,
}

/// The default [`Probe::exists`]: a real file on disk.
///
/// Deliberately *not* an execute-permission check. On Windows there is no such
/// bit, and on Unix a file we can see but not execute is a broken install we
/// want to report as found-but-failing at spawn time rather than silently skip
/// in favour of a different copy.
pub fn is_file(path: &Path) -> bool {
    path.is_file()
}

/// Walk the search order and return the first hit.
///
/// Pure: every input arrives through `probe`. With no executable names (a
/// dependency that does not apply to this OS) the answer is always
/// [`Resolution::Missing`], since there is nothing to look for.
pub fn resolve(probe: &Probe<'_>) -> Resolution {
    if probe.executables.is_empty() {
        return Resolution::Missing;
    }

    if let Some(found) = first_hit(probe.bundled_dirs, probe.executables, probe.exists) {
        return Resolution::Found {
            path: found,
            found_in: FoundIn::Bundled,
        };
    }

    // The configured value is a full path to the program, not a directory: the
    // user picked a specific executable, so we do not go looking for our
    // preferred names next to it.
    if let Some(configured) = probe.configured {
        if (probe.exists)(configured) {
            return Resolution::Found {
                path: configured.to_path_buf(),
                found_in: FoundIn::Configured,
            };
        }
    }

    if let Some(found) = first_hit(probe.path_dirs, probe.executables, probe.exists) {
        return Resolution::Found {
            path: found,
            found_in: FoundIn::Path,
        };
    }

    if let Some(found) = first_hit(probe.install_dirs, probe.executables, probe.exists) {
        return Resolution::Found {
            path: found,
            found_in: FoundIn::Installed,
        };
    }

    Resolution::Missing
}

/// First `dir/exe` that `exists` accepts, trying executable names in order
/// and, for each, every directory in order. The names are a preference (best
/// first), so a better program anywhere on the search path beats a worse one
/// in an earlier folder: a `magick` in `/usr/local/bin` must not shadow
/// libheif's `heif-convert` in `/usr/bin` (the HEIF decoder, whose
/// ImageMagick fallback reaches far more code). Between copies of the same
/// name the earlier folder still wins, as on a shell's `PATH`.
fn first_hit(dirs: &[PathBuf], executables: &[&str], exists: fn(&Path) -> bool) -> Option<PathBuf> {
    executables.iter().find_map(|exe| {
        dirs.iter()
            .map(|dir| dir.join(exe))
            .find(|candidate| exists(candidate))
    })
}

// ---------------------------------------------------------------------------
// Per-dependency facts
// ---------------------------------------------------------------------------

/// The static facts about one dependency on one OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spec {
    /// Which dependency these facts describe.
    pub id: Dependency,
    /// The name we print in messages, spelled the way the vendor spells it.
    pub name: &'static str,
    /// Executable file names to look for, most preferred first. Empty when the
    /// dependency does not apply to this OS, or is a codec (below).
    pub executables: &'static [&'static str],
    /// A system codec rather than a program: found by asking the OS
    /// ([`codec_present`]) instead of looking for an executable.
    pub codec: bool,
    /// The subdirectory a bundled copy lives in, under the app's resource
    /// directory (see `bundle.resources` in `tauri.conf.json`).
    pub bundle_subdir: &'static str,
    /// Extensions that cannot be printed **today** while this is missing —
    /// always a subset of [`crate::engine::SUPPORTED_EXTS`].
    pub blocks: &'static [&'static str],
    /// What the user loses, phrased for the end of
    /// "`<name>` is missing — …". Stated in the present tense about the build
    /// they are running, never about the design target.
    pub effect: &'static str,
    /// How the user can install it from Pluryprint (FR-058, D13), best first.
    /// Every one runs only after the user's explicit consent.
    pub install: &'static [InstallRecipe],
}

impl Spec {
    /// Whether this dependency is used at all on this OS. A non-applicable one
    /// is never reported as missing: SumatraPDF is irrelevant on macOS, and
    /// saying "SumatraPDF is missing" there would be a false alarm.
    pub fn applicable(&self) -> bool {
        !self.executables.is_empty() || self.codec
    }

    /// The remediation message shown when this dependency is missing. Names the
    /// dependency and states the effect, per FR-058. English: the UI shows the
    /// translated `common:deps.<id>.remediation` instead.
    pub fn remediation(&self) -> String {
        format!("{} is missing — {}", self.name, self.effect)
    }
}

/// How a dependency can be installed (FR-058, D13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[serde(rename_all = "kebab-case")]
pub enum InstallMethod {
    /// `winget install` from the community repository (Windows).
    Winget,
    /// `winget install` from the Microsoft Store source (Windows, free apps).
    WingetStore,
    /// The app's page in the Microsoft Store app (Windows).
    Store,
    /// `brew install --cask` (macOS, Homebrew).
    BrewCask,
    /// The vendor's download page in the browser: always possible.
    Web,
}

/// One way to install a dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstallRecipe {
    /// How.
    pub method: InstallMethod,
    /// What: the winget id, the Store product id, the cask, or the URL.
    pub target: &'static str,
    /// Whether the installer asks for administrator rights (a UAC prompt):
    /// LibreOffice's MSI installs for every user.
    pub needs_admin: bool,
    /// Whether it costs money (HEVC Video Extensions).
    pub paid: bool,
    /// Approximate download size in megabytes, when known.
    pub approx_mb: Option<u32>,
}

impl InstallRecipe {
    const fn new(method: InstallMethod, target: &'static str) -> Self {
        Self {
            method,
            target,
            needs_admin: false,
            paid: false,
            approx_mb: None,
        }
    }
    const fn admin(mut self) -> Self {
        self.needs_admin = true;
        self
    }
    const fn paid(mut self) -> Self {
        self.paid = true;
        self
    }
    const fn mb(mut self, mb: u32) -> Self {
        self.approx_mb = Some(mb);
        self
    }

    /// The program this recipe runs (`winget`, `brew`), or `None` when it
    /// opens a page instead.
    pub fn tool(&self) -> Option<InstallTool> {
        match self.method {
            InstallMethod::Winget | InstallMethod::WingetStore => Some(InstallTool::Winget),
            InstallMethod::BrewCask => Some(InstallTool::Brew),
            InstallMethod::Store | InstallMethod::Web => None,
        }
    }

    /// The arguments after the program, for a recipe that runs one. Never a
    /// shell string: each element is one argument. Pure.
    pub fn args(&self) -> Vec<String> {
        let common = |source: &str| {
            vec![
                "install".to_string(),
                "--id".to_string(),
                self.target.to_string(),
                "--exact".to_string(),
                "--source".to_string(),
                source.to_string(),
                "--accept-package-agreements".to_string(),
                "--accept-source-agreements".to_string(),
                "--disable-interactivity".to_string(),
            ]
        };
        match self.method {
            InstallMethod::Winget => common("winget"),
            InstallMethod::WingetStore => common("msstore"),
            InstallMethod::BrewCask => vec![
                "install".to_string(),
                "--cask".to_string(),
                self.target.to_string(),
            ],
            InstallMethod::Store | InstallMethod::Web => Vec::new(),
        }
    }

    /// The URL a recipe that opens a page opens. Pure.
    pub fn url(&self) -> Option<String> {
        match self.method {
            InstallMethod::Store => {
                Some(format!("ms-windows-store://pdp/?ProductId={}", self.target))
            }
            InstallMethod::Web => Some(self.target.to_string()),
            _ => None,
        }
    }
}

/// A program an install recipe runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallTool {
    /// Windows Package Manager.
    Winget,
    /// Homebrew.
    Brew,
}

impl InstallTool {
    /// The executable names to look for.
    pub fn executables(self) -> &'static [&'static str] {
        match self {
            InstallTool::Winget => &["winget.exe"],
            InstallTool::Brew => &["brew"],
        }
    }

    /// Where it lives when it is not on `PATH`: winget is an app execution
    /// alias under `%LOCALAPPDATA%\Microsoft\WindowsApps`; Homebrew is in
    /// `/opt/homebrew/bin` (Apple silicon) or `/usr/local/bin` (Intel), which
    /// an app started from the Finder does not have on its `PATH`.
    pub fn install_dirs(self, env: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
        match self {
            InstallTool::Winget => env("LOCALAPPDATA")
                .map(|root| vec![PathBuf::from(root).join("Microsoft").join("WindowsApps")])
                .unwrap_or_default(),
            InstallTool::Brew => vec![
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
            ],
        }
    }
}

/// Find `tool` on this machine. `None` when it is not installed: the UI then
/// offers the web page instead (no winget on some Windows 10 or LTSC
/// installs, no Homebrew on most Macs).
pub fn locate_tool(tool: InstallTool) -> Option<PathBuf> {
    let env = |name: &str| std::env::var(name).ok();
    resolve(&Probe {
        bundled_dirs: &[],
        configured: None,
        path_dirs: &path_dirs(&env),
        install_dirs: &tool.install_dirs(&env),
        executables: tool.executables(),
        exists: is_file,
    })
    .path()
    .map(Path::to_path_buf)
}

/// One way to install a dependency, as shipped to the UI: what will run (or
/// open), and what the user must know before agreeing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct InstallOption {
    /// How.
    pub method: InstallMethod,
    /// The exact command that will run (`winget install --id … --exact …`),
    /// or the address that will open. Shown verbatim in the consent dialog.
    pub command: String,
    /// Whether Windows will ask for administrator rights (UAC).
    pub needs_admin: bool,
    /// Whether it costs money.
    pub paid: bool,
    /// Approximate download size in megabytes, when known.
    pub approx_mb: Option<u32>,
    /// Whether it can run on this machine now (`winget`/`brew` found).
    pub available: bool,
}

/// The install options for `recipes`, with `available` from `has_tool`. Pure.
pub fn install_options(
    recipes: &[InstallRecipe],
    has_tool: &dyn Fn(InstallTool) -> bool,
) -> Vec<InstallOption> {
    recipes
        .iter()
        .map(|recipe| {
            let (command, available) = match recipe.tool() {
                Some(tool) => {
                    let program = match tool {
                        InstallTool::Winget => "winget",
                        InstallTool::Brew => "brew",
                    };
                    let mut words = vec![program.to_string()];
                    words.extend(recipe.args());
                    (words.join(" "), has_tool(tool))
                }
                None => (recipe.url().unwrap_or_default(), true),
            };
            InstallOption {
                method: recipe.method,
                command,
                needs_admin: recipe.needs_admin,
                paid: recipe.paid,
                approx_mb: recipe.approx_mb,
                available,
            }
        })
        .collect()
}

/// What a missing LibreOffice blocks on Windows: the Office route, RTF
/// included. Plain text no longer needs it (FR-057).
const LIBREOFFICE_BLOCKS_WINDOWS: &[&str] = &[
    "docx", "doc", "xlsx", "xls", "pptx", "ppt", "odt", "ods", "odp", "rtf",
];

/// On macOS RTF goes through the system's `textutil` (FR-057).
const LIBREOFFICE_BLOCKS_MACOS: &[&str] = &[
    "docx", "doc", "xlsx", "xls", "pptx", "ppt", "odt", "ods", "odp",
];

const SUMATRA_INSTALL: &[InstallRecipe] = &[
    InstallRecipe::new(InstallMethod::Winget, "SumatraPDF.SumatraPDF").mb(10),
    InstallRecipe::new(
        InstallMethod::Web,
        "https://www.sumatrapdfreader.org/download-free-pdf-viewer",
    ),
];

const LIBREOFFICE_INSTALL_WINDOWS: &[InstallRecipe] = &[
    InstallRecipe::new(InstallMethod::Winget, "TheDocumentFoundation.LibreOffice")
        .admin()
        .mb(350),
    InstallRecipe::new(
        InstallMethod::Web,
        "https://www.libreoffice.org/download/download-libreoffice/",
    ),
];

const LIBREOFFICE_INSTALL_MACOS: &[InstallRecipe] = &[
    InstallRecipe::new(InstallMethod::BrewCask, "libreoffice").mb(800),
    InstallRecipe::new(
        InstallMethod::Web,
        "https://www.libreoffice.org/download/download-libreoffice/",
    ),
];

/// Linux: the distribution's package is the usual route (`apt install
/// libreoffice`, `dnf install libreoffice`), which Pluryprint does not run for
/// the user, so the consent dialog offers the vendor's page. The remediation
/// text names the package.
const LIBREOFFICE_INSTALL_LINUX: &[InstallRecipe] = &[InstallRecipe::new(
    InstallMethod::Web,
    "https://www.libreoffice.org/download/download-libreoffice/",
)];

/// The program names an external HEIF decoder goes by on Linux, best first:
/// libheif's own tools (`heif-dec` from libheif 1.17, `heif-convert` before
/// it), then ImageMagick 7 (`magick`) and 6 (`convert`), which decode HEIF
/// through libheif too. [`crate::engine::convert::heif`] builds the argument
/// vector from the name it was given. A `convert` that is not ImageMagick's
/// is passed over ([`is_heif_decoder`]).
pub const HEIF_DECODERS_LINUX: &[&str] = &["heif-dec", "heif-convert", "magick", "convert"];

/// How long `convert -version` may take to say what it is
/// ([`is_heif_decoder`]).
const VERSION_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// At most this many bytes of `convert -version` are read.
#[cfg(unix)]
const VERSION_PROBE_CAP: u64 = 4096;

/// How long a `convert` that has exited may keep its output open (a helper
/// it left behind holding the pipe) before what it wrote so far is taken
/// as its answer ([`identifies_as_imagemagick`]).
#[cfg(unix)]
const VERSION_PROBE_GRACE: std::time::Duration = std::time::Duration::from_secs(1);

/// How long [`is_heif_decoder`] keeps a probe that gave no answer (it timed
/// out or the program could not be started) before running it again: long
/// enough that a `convert` that hangs does not hold every `app_info` call
/// for [`VERSION_PROBE_TIMEOUT`], short enough that a busy moment does not
/// rule ImageMagick out for the rest of the session.
const NO_ANSWER_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// What identifies one program file for [`is_heif_decoder`]'s cache: its
/// canonical path, size, modification time, inode and status-change time
/// (`st_ctime` and its nanoseconds, Unix). A package upgrade that replaces
/// the file changes at least the inode or a time; one that rewrites it in
/// place and restores its modification time (`touch -r`, `cp -p`, a
/// tarball's timestamps) still changes the status-change time, which no
/// one can set back.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProgramKey {
    canonical: PathBuf,
    len: u64,
    modified: Option<std::time::SystemTime>,
    inode: u64,
    changed: (i64, i64),
}

/// What [`is_heif_decoder`] learnt by running a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// It answered: ImageMagick or not. Kept for the life of the process.
    Known(bool),
    /// It did not answer, at this time; trusted for [`NO_ANSWER_TTL`].
    NoAnswer(std::time::Instant),
}

/// The verdicts [`is_heif_decoder`] reached by running a program.
static HEIF_DECODER_VERDICTS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<ProgramKey, Verdict>>,
> = std::sync::LazyLock::new(Default::default);

/// The [`ProgramKey`] of the file at `path`; `None` when it cannot be read.
fn program_key(path: &Path) -> Option<ProgramKey> {
    let canonical = path.canonicalize().ok()?;
    let meta = std::fs::metadata(&canonical).ok()?;
    #[cfg(unix)]
    let (inode, changed) = {
        use std::os::unix::fs::MetadataExt;
        (meta.ino(), (meta.ctime(), meta.ctime_nsec()))
    };
    #[cfg(not(unix))]
    let (inode, changed) = (0, (0, 0));
    Some(ProgramKey {
        canonical,
        len: meta.len(),
        modified: meta.modified().ok(),
        inode,
        changed,
    })
}

/// The [`Probe::exists`] for [`Dependency::HeifDecoder`]: a file, and for
/// `convert`, one that says it is ImageMagick.
///
/// `convert` is a common name. GraphicsMagick's compatibility package
/// installs one, and GraphicsMagick does not read ImageMagick's
/// `policy.xml`, so it would decode outside the policy the HEIF converter
/// runs ImageMagick under; other packages ship an unrelated `convert`. So
/// `convert -version` is run (bounded by [`VERSION_PROBE_TIMEOUT`], at most
/// [`VERSION_PROBE_CAP`] bytes read) and the program is used only if it
/// names ImageMagick. `magick` is ImageMagick 7's own name and comes first
/// in [`HEIF_DECODERS_LINUX`], so it is preferred whenever it is there.
///
/// The dependency report (`app_info`, which the UI asks on its main thread)
/// and every HEIF job run this search, so the answer is kept per program
/// file ([`ProgramKey`]) for the life of the process: each `convert` is run
/// once, and again only once it has been replaced. A probe that timed out
/// or could not start the program is a "no" kept for [`NO_ANSWER_TTL`]
/// only: a `convert` that hangs costs one [`VERSION_PROBE_TIMEOUT`] a
/// minute, not one per call, and a busy moment does not rule ImageMagick
/// out for good.
pub fn is_heif_decoder(path: &Path) -> bool {
    heif_decoder_verdict(path, VERSION_PROBE_TIMEOUT)
}

/// [`is_heif_decoder`], probing for at most `timeout`.
fn heif_decoder_verdict(path: &Path, timeout: std::time::Duration) -> bool {
    if !path.is_file() {
        return false;
    }
    if path.file_name() != Some(std::ffi::OsStr::new("convert")) {
        return true;
    }
    let Some(key) = program_key(path) else {
        return false;
    };
    let verdicts = || {
        HEIF_DECODER_VERDICTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    match verdicts().get(&key) {
        Some(Verdict::Known(verdict)) => return *verdict,
        Some(Verdict::NoAnswer(at)) if at.elapsed() < NO_ANSWER_TTL => return false,
        _ => {}
    }
    // Not under the lock: a probe can take `timeout`, and two threads
    // probing the same program at once agree anyway.
    let verdict = match imagemagick_probe(path, timeout) {
        Some(answer) => Verdict::Known(answer),
        None => Verdict::NoAnswer(std::time::Instant::now()),
    };
    verdicts().insert(key, verdict);
    verdict == Verdict::Known(true)
}

/// Whether `program -version` prints `ImageMagick` within `timeout`. A
/// program that does not start, runs past `timeout` (then killed with its
/// group), or says anything else is not ImageMagick. Only the first
/// [`VERSION_PROBE_CAP`] bytes are read; the pipe is then closed, so a
/// program that keeps writing stops.
pub fn identifies_as_imagemagick(program: &Path, timeout: std::time::Duration) -> bool {
    imagemagick_probe(program, timeout) == Some(true)
}

/// [`identifies_as_imagemagick`], or `None` when the program could not be
/// started or ran past `timeout`: no answer, rather than a "no".
///
/// The output is read on this thread, from a non-blocking pipe polled with
/// the child: no reader thread is left behind blocked on a pipe that a
/// helper of the program still holds open. Once the program has exited,
/// what it wrote within [`VERSION_PROBE_GRACE`] is its answer. Unix only:
/// the HEIF decoder is looked for on Linux alone, and elsewhere this is
/// `None`.
fn imagemagick_probe(program: &Path, timeout: std::time::Duration) -> Option<bool> {
    #[cfg(unix)]
    {
        use std::io::Read;
        use std::os::fd::AsRawFd;
        use std::process::Stdio;
        use std::time::Instant;

        use crate::engine::convert::process::{isolate_process_tree, kill_tree};

        let mut cmd = crate::util::appimage::host_command(program);
        cmd.arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        isolate_process_tree(&mut cmd);
        let mut child = cmd.spawn().ok()?;
        let started = Instant::now();
        let Some(stdout) = child.stdout.take() else {
            kill_tree(&mut child);
            return None;
        };
        let fd = stdout.as_raw_fd();
        // SAFETY: fcntl on a descriptor this process owns (`stdout`, alive
        // for both calls); no memory is passed.
        let nonblocking = unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            flags >= 0 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) == 0
        };
        if !nonblocking {
            kill_tree(&mut child);
            return None;
        }
        let mut pipe = Some(stdout);
        let mut text = Vec::new();
        let mut buf = [0u8; 1024];
        let mut exited: Option<Instant> = None;
        loop {
            let mut progressed = false;
            if let Some(out) = &mut pipe {
                let room = usize::try_from(VERSION_PROBE_CAP)
                    .unwrap_or(usize::MAX)
                    .saturating_sub(text.len())
                    .min(buf.len());
                match out.read(&mut buf[..room]) {
                    Ok(0) => pipe = None,
                    Ok(n) => {
                        text.extend_from_slice(&buf[..n]);
                        progressed = true;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => progressed = true,
                    Err(_) => pipe = None,
                }
                // At the cap the pipe is closed: a writer that goes on gets
                // SIGPIPE and stops.
                if text.len() as u64 >= VERSION_PROBE_CAP {
                    pipe = None;
                }
            }
            if exited.is_none() {
                match child.try_wait() {
                    Ok(Some(_)) => exited = Some(Instant::now()),
                    Ok(None) => {}
                    Err(_) => {
                        kill_tree(&mut child);
                        return None;
                    }
                }
            }
            match exited {
                Some(at) if pipe.is_none() || at.elapsed() >= VERSION_PROBE_GRACE => break,
                Some(_) => {}
                None if started.elapsed() >= timeout => {
                    kill_tree(&mut child);
                    return None;
                }
                None => {}
            }
            if !progressed {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        Some(String::from_utf8_lossy(&text).contains("ImageMagick"))
    }
    #[cfg(not(unix))]
    {
        let _ = (program, timeout);
        None
    }
}

/// The HEIF decoder on Linux: a package from the distribution, so only the
/// project's page can be offered (see [`LIBREOFFICE_INSTALL_LINUX`]).
const HEIF_DECODER_INSTALL_LINUX: &[InstallRecipe] = &[InstallRecipe::new(
    InstallMethod::Web,
    "https://github.com/strukturag/libheif",
)];

/// HEIF Image Extensions: free, in the Store, installable by winget.
const HEIF_INSTALL: &[InstallRecipe] = &[
    InstallRecipe::new(InstallMethod::WingetStore, "9PMMSR1CGPWG").mb(5),
    InstallRecipe::new(InstallMethod::Store, "9PMMSR1CGPWG"),
    InstallRecipe::new(
        InstallMethod::Web,
        "https://apps.microsoft.com/detail/9PMMSR1CGPWG",
    ),
];

/// HEVC Video Extensions: paid, so only the Store can sell it (winget cannot
/// buy anything).
const HEVC_INSTALL: &[InstallRecipe] = &[
    InstallRecipe::new(InstallMethod::Store, "9NMZLZ57R3T7").paid(),
    InstallRecipe::new(
        InstallMethod::Web,
        "https://apps.microsoft.com/detail/9NMZLZ57R3T7",
    )
    .paid(),
];

const BROWSER_INSTALL: &[InstallRecipe] = &[InstallRecipe::new(
    InstallMethod::Web,
    "https://www.microsoft.com/edge",
)];

/// The facts about `dep` on `os`.
pub fn spec(dep: Dependency, os: Os) -> Spec {
    let windows = os == Os::Windows;
    match dep {
        Dependency::SumatraPdf => Spec {
            id: dep,
            name: "SumatraPDF",
            // Windows-only by design: CUPS `lp` is the backend everywhere else,
            // so there is nothing to look for on macOS/Linux.
            executables: match os {
                Os::Windows => &["SumatraPDF.exe"],
                _ => &[],
            },
            codec: false,
            bundle_subdir: "sumatrapdf",
            // The final printer for *every* format on Windows, so a missing
            // copy blocks the whole supported list — this is the one dependency
            // whose absence really does stop printing.
            blocks: crate::engine::SUPPORTED_EXTS,
            effect: "no file can be printed on Windows until it is installed. \
                     Install SumatraPDF, or place SumatraPDF.exe next to Pluryprint.exe",
            install: if windows { SUMATRA_INSTALL } else { &[] },
        },
        Dependency::LibreOffice => Spec {
            id: dep,
            name: "LibreOffice",
            // Linux: the distribution packages and the TDF tarball install
            // `soffice`; the snap only has `libreoffice`, and a Flatpak is
            // reachable through its exported launcher, named after the app id.
            executables: match os {
                Os::Windows => &["soffice.exe"],
                Os::MacOs => &["soffice"],
                Os::Linux => &["soffice", "libreoffice", FLATPAK_LIBREOFFICE],
            },
            codec: false,
            bundle_subdir: "libreoffice",
            blocks: match os {
                Os::MacOs => LIBREOFFICE_BLOCKS_MACOS,
                _ => LIBREOFFICE_BLOCKS_WINDOWS,
            },
            // RTF only where LibreOffice is its route (Windows): on macOS it
            // goes through `textutil` and prints without LibreOffice.
            effect: match os {
                Os::MacOs => {
                    "Office and OpenDocument files cannot be printed until it is \
                     installed. Download it free from https://www.libreoffice.org/download"
                }
                Os::Linux => {
                    "Office, OpenDocument and RTF files cannot be printed until \
                     it is installed. Install the libreoffice package from your \
                     distribution, or download it free from https://www.libreoffice.org/download"
                }
                Os::Windows => {
                    "Office, OpenDocument and RTF files cannot be printed until \
                     it is installed. Download it free from https://www.libreoffice.org/download"
                }
            },
            install: match os {
                Os::Windows => LIBREOFFICE_INSTALL_WINDOWS,
                Os::MacOs => LIBREOFFICE_INSTALL_MACOS,
                Os::Linux => LIBREOFFICE_INSTALL_LINUX,
            },
        },
        Dependency::Chromium => Spec {
            id: dep,
            name: "Microsoft Edge or Google Chrome",
            executables: match os {
                // Edge first: it ships with Windows.
                Os::Windows => &["msedge.exe", "chrome.exe"],
                Os::MacOs => &["Microsoft Edge", "Google Chrome", "Chromium"],
                Os::Linux => &[
                    "google-chrome",
                    "chromium",
                    "chromium-browser",
                    "microsoft-edge",
                ],
            },
            codec: false,
            bundle_subdir: "chromium",
            // Nothing needs it any more (the browser fallback was removed).
            blocks: &[],
            effect: "Pluryprint no longer needs it",
            install: BROWSER_INSTALL,
        },
        Dependency::Pandoc => Spec {
            id: dep,
            name: "Pandoc",
            executables: match os {
                Os::Windows => &["pandoc.exe"],
                _ => &["pandoc"],
            },
            codec: false,
            bundle_subdir: "pandoc",
            // Markdown converts in-process now (FR-056).
            blocks: &[],
            effect: "Pluryprint no longer needs it",
            install: &[],
        },
        Dependency::HeifCodec => Spec {
            id: dep,
            name: "HEIF Image Extensions",
            executables: &[],
            codec: windows,
            bundle_subdir: "",
            blocks: if windows { &["heic", "heif"] } else { &[] },
            effect: "HEIC/HEIF photos cannot be printed until it is installed. \
                     Get it free from the Microsoft Store",
            install: if windows { HEIF_INSTALL } else { &[] },
        },
        Dependency::HevcCodec => Spec {
            id: dep,
            name: "HEVC Video Extensions",
            executables: &[],
            codec: windows,
            bundle_subdir: "",
            blocks: if windows { &["heic"] } else { &[] },
            effect: "HEIC photos cannot be printed until it is installed. \
                     Get it from the Microsoft Store (it may be paid)",
            install: if windows { HEVC_INSTALL } else { &[] },
        },
        Dependency::HeifDecoder => {
            let linux = os == Os::Linux;
            Spec {
                id: dep,
                name: "HEIF decoder (libheif or ImageMagick)",
                // macOS has `sips` and Windows asks WIC: nothing to look for.
                executables: if linux { HEIF_DECODERS_LINUX } else { &[] },
                codec: false,
                bundle_subdir: "",
                blocks: if linux { &["heic", "heif"] } else { &[] },
                effect: "HEIC/HEIF photos cannot be printed until one is installed. \
                         Install the libheif tools (libheif-examples on Debian and Ubuntu, \
                         libheif on Arch; on Fedora, libheif-tools together with \
                         libheif-freeworld from RPM Fusion, which decodes HEIC) or ImageMagick",
                install: if linux {
                    HEIF_DECODER_INSTALL_LINUX
                } else {
                    &[]
                },
            }
        }
    }
}

/// The launcher a Flatpak LibreOffice exports (`flatpak run` under the
/// app id), found in the Flatpak `exports/bin` directories.
pub const FLATPAK_LIBREOFFICE: &str = "org.libreoffice.LibreOffice";

/// The `program` folders of LibreOffice versions installed from The Document
/// Foundation's own packages, which land in `/opt/libreoffice<version>`
/// instead of on `PATH`. `entries` are the names inside `/opt`; the newest
/// version comes first, and a bare `libreoffice` (no version) last. Pure.
pub fn opt_libreoffice_dirs(entries: &[String]) -> Vec<PathBuf> {
    let version = |name: &str| -> Vec<u32> {
        name.trim_start_matches("libreoffice")
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    };
    let mut names: Vec<&String> = entries
        .iter()
        .filter(|name| name.starts_with("libreoffice"))
        .collect();
    names.sort_by(|a, b| version(b).cmp(&version(a)).then_with(|| a.cmp(b)));
    names
        .into_iter()
        .map(|name| Path::new("/opt").join(name).join("program"))
        .collect()
}

/// Whether the system codec `dep` is installed. Windows asks WIC (HEIF) and
/// Media Foundation (HEVC); if the question itself fails the answer is "yes",
/// so a broken probe never raises a false alarm. `false` for a dependency
/// that is not a codec, and everywhere but Windows.
pub fn codec_present(dep: Dependency) -> bool {
    #[cfg(windows)]
    {
        match dep {
            Dependency::HeifCodec => codec_probe::wic_decodes("heic"),
            Dependency::HevcCodec => crate::engine::convert::heif::hevc_decoder_available(),
            _ => false,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = dep;
        false
    }
}

#[cfg(windows)]
mod codec_probe {
    use windows::core::Interface;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, IWICBitmapCodecInfo, IWICImagingFactory,
        WICComponentEnumerateDefault, WICDecoder,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };

    /// Whether a WIC decoder claims the file extension `ext` (no dot).
    pub fn wic_decodes(ext: &str) -> bool {
        let wanted = format!(".{}", ext.to_ascii_lowercase());
        // SAFETY: COM is initialised for this thread and balanced below; every
        // pointer handed to WIC is a live local.
        unsafe {
            let initialised = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            let answer = (|| -> windows::core::Result<bool> {
                let factory: IWICImagingFactory =
                    CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
                let components = factory.CreateComponentEnumerator(
                    WICDecoder.0 as u32,
                    WICComponentEnumerateDefault.0 as u32,
                )?;
                loop {
                    let mut item = [None];
                    let mut fetched = 0u32;
                    if components.Next(&mut item, Some(&mut fetched)).is_err() || fetched == 0 {
                        return Ok(false);
                    }
                    let Some(unknown) = item[0].take() else {
                        return Ok(false);
                    };
                    let Ok(info) = unknown.cast::<IWICBitmapCodecInfo>() else {
                        continue;
                    };
                    let mut len = 0u32;
                    let _ = info.GetFileExtensions(&mut [], &mut len);
                    if len == 0 {
                        continue;
                    }
                    let mut buf = vec![0u16; len as usize];
                    if info.GetFileExtensions(&mut buf, &mut len).is_err() {
                        continue;
                    }
                    let list = String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)]);
                    if list
                        .to_ascii_lowercase()
                        .split(',')
                        .any(|e| e.trim() == wanted)
                    {
                        return Ok(true);
                    }
                }
            })()
            .unwrap_or(true);
            if initialised {
                CoUninitialize();
            }
            answer
        }
    }
}

// ---------------------------------------------------------------------------
// Gathering the environment (the impure half)
// ---------------------------------------------------------------------------

/// Directories a bundled copy could sit in, derived from `exe_dir` (the
/// directory holding our own executable).
///
/// Covers both installed layouts Tauri produces plus the two dev conveniences:
///
/// | Candidate                          | Why                                    |
/// |------------------------------------|----------------------------------------|
/// | `<exe>/resources/<subdir>`         | Windows/Linux bundle resource dir      |
/// | `<exe>/../Resources/<subdir>`      | macOS `.app` bundle resource dir       |
/// | `<exe>/<subdir>`, `<exe>/bin`, `<exe>` | dropped in by hand next to the app |
pub fn bundled_dirs(exe_dir: &Path, subdir: &str) -> Vec<PathBuf> {
    let mut dirs = vec![
        exe_dir.join("resources").join(subdir),
        exe_dir.join("resources"),
    ];
    if let Some(parent) = exe_dir.parent() {
        dirs.push(parent.join("Resources").join(subdir));
        dirs.push(parent.join("Resources"));
    }
    dirs.push(exe_dir.join(subdir));
    dirs.push(exe_dir.join("bin"));
    dirs.push(exe_dir.to_path_buf());
    dirs
}

/// The directory holding our own executable, when it can be determined.
pub fn current_exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// The entries of `PATH`, as read from `env`.
pub fn path_dirs(env: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    env("PATH")
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default()
}

/// Where each vendor's installer puts the program on `os`.
///
/// `env` supplies the environment variables the Windows locations expand from,
/// so the Windows table is testable from any host. Variables that are not set
/// simply contribute no candidate.
pub fn install_dirs(dep: Dependency, os: Os, env: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut push = |var: &str, tail: &[&str]| {
        if let Some(root) = env(var) {
            let mut dir = PathBuf::from(root);
            for segment in tail {
                dir.push(segment);
            }
            dirs.push(dir);
        }
    };

    match (dep, os) {
        (Dependency::SumatraPdf, Os::Windows) => {
            // The default installer is per-user; the "all users" option and the
            // 32-bit build land under Program Files.
            push("LOCALAPPDATA", &["SumatraPDF"]);
            push("ProgramFiles", &["SumatraPDF"]);
            push("ProgramFiles(x86)", &["SumatraPDF"]);
        }
        (Dependency::LibreOffice, Os::Windows) => {
            push("ProgramFiles", &["LibreOffice", "program"]);
            push("ProgramFiles(x86)", &["LibreOffice", "program"]);
        }
        (Dependency::LibreOffice, Os::MacOs) => {
            dirs.push(PathBuf::from(
                "/Applications/LibreOffice.app/Contents/MacOS",
            ));
        }
        (Dependency::Chromium, Os::Windows) => {
            push("ProgramFiles(x86)", &["Microsoft", "Edge", "Application"]);
            push("ProgramFiles", &["Microsoft", "Edge", "Application"]);
            push("ProgramFiles", &["Google", "Chrome", "Application"]);
            push("ProgramFiles(x86)", &["Google", "Chrome", "Application"]);
        }
        (Dependency::Chromium, Os::MacOs) => {
            dirs.push(PathBuf::from(
                "/Applications/Microsoft Edge.app/Contents/MacOS",
            ));
            dirs.push(PathBuf::from(
                "/Applications/Google Chrome.app/Contents/MacOS",
            ));
            dirs.push(PathBuf::from("/Applications/Chromium.app/Contents/MacOS"));
        }
        (Dependency::Pandoc, Os::Windows) => {
            push("LOCALAPPDATA", &["Pandoc"]);
            push("ProgramFiles", &["Pandoc"]);
        }
        (Dependency::Pandoc, Os::MacOs) => {
            // Homebrew, Apple Silicon then Intel.
            dirs.push(PathBuf::from("/opt/homebrew/bin"));
            dirs.push(PathBuf::from("/usr/local/bin"));
        }
        (Dependency::LibreOffice, Os::Linux) => {
            // The distribution's package puts `soffice` on `PATH` (step 3)
            // and its program folder here (Debian/Ubuntu, then Fedora). The
            // TDF packages' `/opt/libreoffice<version>` folders are listed
            // by [`resolve_on_host`], which reads `/opt`.
            dirs.push(PathBuf::from("/usr/bin"));
            dirs.push(PathBuf::from("/usr/lib/libreoffice/program"));
            dirs.push(PathBuf::from("/usr/lib64/libreoffice/program"));
            dirs.push(PathBuf::from("/opt/libreoffice/program"));
            // Sandboxed installs come last: they convert only from a folder
            // their sandbox can read, which the Office converter arranges
            // (`office::confined_work_parent`). Flatpak per user, then system
            // wide; the snap's launcher is in `/snap/bin`.
            if let Some(home) = env("HOME") {
                dirs.push(PathBuf::from(home).join(".local/share/flatpak/exports/bin"));
            }
            dirs.push(PathBuf::from("/var/lib/flatpak/exports/bin"));
            dirs.push(PathBuf::from("/snap/bin"));
        }
        (Dependency::HeifDecoder, Os::Linux) => {
            dirs.push(PathBuf::from("/usr/bin"));
            dirs.push(PathBuf::from("/usr/local/bin"));
        }
        // Linux installs land on `PATH` (step 3) via the distribution's
        // packages; `/usr/bin` is listed anyway for a `PATH`-less environment.
        (_, Os::Linux) => dirs.push(PathBuf::from("/usr/bin")),
        // macOS decodes HEIF with `sips` and Windows with WIC.
        (Dependency::HeifDecoder, _) => {}
        // SumatraPDF does not exist off Windows.
        (Dependency::SumatraPdf, _) => {}
        // Codecs are asked of the OS, not looked for on disk.
        (Dependency::HeifCodec | Dependency::HevcCodec, _) => {}
    }

    dirs
}

// ---------------------------------------------------------------------------
// The wire shape
// ---------------------------------------------------------------------------

/// One dependency's status, as shipped to the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct DependencyReport {
    /// Which dependency this row reports on.
    pub id: Dependency,
    /// Display name, e.g. `"SumatraPDF"`.
    pub name: String,
    /// Whether this dependency is used at all on the running OS. `false` means
    /// "irrelevant here", not "broken" — nothing should be flagged for it.
    pub applicable: bool,
    /// Whether an executable was found.
    pub found: bool,
    /// Where it was found, for diagnostics.
    pub path: Option<String>,
    /// Which search step found it, for diagnostics.
    pub found_in: Option<FoundIn>,
    /// Extensions that cannot be printed today because this is missing. Empty
    /// whenever the dependency is present, not applicable, or gates no format
    /// the engine currently supports.
    pub blocks: Vec<String>,
    /// FR-058's remediation message. `Some` only when the dependency is
    /// applicable and missing. English; the UI shows the translated
    /// `common:deps.<id>.remediation`.
    pub remediation: Option<String>,
    /// How the user can install it from Pluryprint, best first (D13). Empty
    /// when it is present or not applicable.
    pub install: Vec<InstallOption>,
}

/// Look for `dep` on the real machine: assembles the four candidate lists from
/// the environment and hands them to [`resolve`].
///
/// `configured` is the user's explicit override; no setting feeds it yet (see
/// the module docs), so callers pass `None`.
pub fn resolve_on_host(dep: Dependency, os: Os, configured: Option<&Path>) -> Resolution {
    let spec = spec(dep, os);
    // Nothing to look for: skip the filesystem entirely.
    if !spec.applicable() {
        return Resolution::Missing;
    }
    if spec.codec {
        return if codec_present(dep) {
            Resolution::Found {
                path: PathBuf::new(),
                found_in: FoundIn::Installed,
            }
        } else {
            Resolution::Missing
        };
    }

    let env = |name: &str| std::env::var(name).ok();
    // Only the dependencies we bundle are looked for next to our executable:
    // a HEIF decoder named `convert` must not match an unrelated file there.
    let bundled = current_exe_dir()
        .filter(|_| !spec.bundle_subdir.is_empty())
        .map(|dir| bundled_dirs(&dir, spec.bundle_subdir))
        .unwrap_or_default();
    let mut installed = install_dirs(dep, os, &env);
    if dep == Dependency::LibreOffice && os == Os::Linux {
        let opt: Vec<String> = std::fs::read_dir("/opt")
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        // After the distribution's folders, before the sandboxed launchers.
        let at = installed
            .iter()
            .position(|d| d.ends_with("flatpak/exports/bin"))
            .unwrap_or(installed.len());
        installed.splice(at..at, opt_libreoffice_dirs(&opt));
    }

    resolve(&Probe {
        bundled_dirs: &bundled,
        configured,
        path_dirs: &path_dirs(&env),
        install_dirs: &installed,
        executables: spec.executables,
        exists: if dep == Dependency::HeifDecoder {
            is_heif_decoder
        } else {
            is_file
        },
    })
}

/// Status of one dependency on the real machine, ready for the wire.
pub fn report(dep: Dependency, os: Os, configured: Option<&Path>) -> DependencyReport {
    from_resolution_with(
        &spec(dep, os),
        &resolve_on_host(dep, os, configured),
        &|tool| locate_tool(tool).is_some(),
    )
}

/// Build the wire report from an already-computed [`Resolution`]. Pure, so the
/// message and `blocks` rules can be tested without touching the host.
///
/// A dependency this OS never uses is reported as not applicable and nothing
/// else: no remediation, no blocked formats. That is the guard against the
/// loudest possible false alarm — "SumatraPDF is missing" on a Mac, where the
/// engine prints through CUPS and would not have used it either way.
pub fn from_resolution(spec: &Spec, resolution: &Resolution) -> DependencyReport {
    from_resolution_with(spec, resolution, &|_| false)
}

/// [`from_resolution`] with the install tools `has_tool` says are present.
/// Pure.
pub fn from_resolution_with(
    spec: &Spec,
    resolution: &Resolution,
    has_tool: &dyn Fn(InstallTool) -> bool,
) -> DependencyReport {
    if !spec.applicable() {
        return DependencyReport {
            id: spec.id,
            name: spec.name.to_string(),
            applicable: false,
            found: false,
            path: None,
            found_in: None,
            blocks: Vec::new(),
            remediation: None,
            install: Vec::new(),
        };
    }

    let found = matches!(resolution, Resolution::Found { .. });
    DependencyReport {
        id: spec.id,
        name: spec.name.to_string(),
        applicable: spec.applicable(),
        found,
        // A codec has no file of its own to point at.
        path: resolution
            .path()
            .filter(|_| !spec.codec)
            .map(|p| p.to_string_lossy().into_owned()),
        found_in: match resolution {
            Resolution::Found { found_in, .. } => Some(*found_in),
            Resolution::Missing => None,
        },
        blocks: if found {
            Vec::new()
        } else {
            spec.blocks.iter().map(|e| (*e).to_string()).collect()
        },
        remediation: if found {
            None
        } else {
            Some(spec.remediation())
        },
        install: if found {
            Vec::new()
        } else {
            install_options(spec.install, has_tool)
        },
    }
}

/// Status of every dependency on the running OS, in [`ALL`] order.
pub fn report_all() -> Vec<DependencyReport> {
    let os = Os::current();
    ALL.iter().map(|dep| report(*dep, os, None)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Probe` with every step empty, so a test only fills the one it is about.
    fn empty_probe<'a>(executables: &'a [&'a str]) -> Probe<'a> {
        Probe {
            bundled_dirs: &[],
            configured: None,
            path_dirs: &[],
            install_dirs: &[],
            executables,
            exists: is_file,
        }
    }

    #[test]
    fn no_executables_never_resolves() {
        assert_eq!(resolve(&empty_probe(&[])), Resolution::Missing);
    }

    #[test]
    fn everything_absent_is_missing() {
        assert_eq!(resolve(&empty_probe(&["nothing.exe"])), Resolution::Missing);
    }

    #[test]
    fn executable_names_are_tried_in_order() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("chrome.exe"), b"x").unwrap();
        std::fs::write(dir.path().join("msedge.exe"), b"x").unwrap();
        let dirs = vec![dir.path().to_path_buf()];

        let resolution = resolve(&Probe {
            path_dirs: &dirs,
            ..empty_probe(&["msedge.exe", "chrome.exe"])
        });
        assert_eq!(resolution.path().unwrap(), dir.path().join("msedge.exe"));
    }

    /// A preferred name in a later folder beats a fallback in an earlier
    /// one: `magick` in `/usr/local/bin` does not shadow libheif's
    /// `heif-convert` in `/usr/bin`. Copies of one name keep `PATH` order.
    #[test]
    fn a_preferred_name_anywhere_beats_a_fallback_earlier_on_path() {
        let local = tempfile::tempdir().unwrap();
        let usr = tempfile::tempdir().unwrap();
        std::fs::write(local.path().join("magick"), b"x").unwrap();
        std::fs::write(usr.path().join("heif-convert"), b"x").unwrap();
        std::fs::write(usr.path().join("magick"), b"x").unwrap();
        let dirs = vec![local.path().to_path_buf(), usr.path().to_path_buf()];

        let resolution = resolve(&Probe {
            path_dirs: &dirs,
            ..empty_probe(HEIF_DECODERS_LINUX)
        });
        assert_eq!(resolution.path().unwrap(), usr.path().join("heif-convert"));

        let resolution = resolve(&Probe {
            path_dirs: &dirs,
            ..empty_probe(&["magick", "convert"])
        });
        assert_eq!(resolution.path().unwrap(), local.path().join("magick"));
    }

    /// NFR-security: GraphicsMagick's `convert` (which ignores ImageMagick's
    /// `policy.xml`) or any other program of that name is never chosen as
    /// the HEIF decoder; ImageMagick's is, and `magick` beats it. A `convert`
    /// that hangs or floods its output is bounded.
    #[cfg(unix)]
    #[test]
    fn nfr_security_only_imagemagicks_convert_is_a_heif_decoder() {
        use crate::util::fake_program::script;
        let resolve_in = |dir: &Path| {
            resolve(&Probe {
                path_dirs: &[dir.to_path_buf()],
                exists: is_heif_decoder,
                ..empty_probe(HEIF_DECODERS_LINUX)
            })
        };
        let gm = tempfile::tempdir().unwrap();
        script(
            gm.path(),
            "convert",
            "echo 'GraphicsMagick 1.3.38 2022-03-26 Q16 http://www.GraphicsMagick.org/'",
        );
        assert_eq!(resolve_in(gm.path()), Resolution::Missing);

        let im = tempfile::tempdir().unwrap();
        let convert = script(
            im.path(),
            "convert",
            "echo 'Version: ImageMagick 6.9.11-60 Q16 aarch64 2021-01-25 https://imagemagick.org'",
        );
        assert_eq!(resolve_in(im.path()).path(), Some(convert.as_path()));
        // `magick` is preferred whenever it is there, and not asked.
        let magick = script(im.path(), "magick", "exit 1");
        assert_eq!(resolve_in(im.path()).path(), Some(magick.as_path()));

        let odd = tempfile::tempdir().unwrap();
        let hangs = script(odd.path(), "hangs", "sleep 30");
        let started = std::time::Instant::now();
        assert!(!identifies_as_imagemagick(
            &hangs,
            std::time::Duration::from_millis(300)
        ));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        // A flood is cut at the cap: the name past it is never seen.
        let floods = script(
            odd.path(),
            "floods",
            "yes GraphicsMagick | head -c 100000; echo ImageMagick",
        );
        assert!(!identifies_as_imagemagick(
            &floods,
            std::time::Duration::from_secs(5)
        ));
        assert!(!identifies_as_imagemagick(
            &odd.path().join("missing"),
            std::time::Duration::from_secs(5)
        ));
    }

    /// The `convert` verdict is kept per program file: the dependency report
    /// (asked on the UI thread) runs `convert -version` once, not on every
    /// call, and again once the file is replaced or its status changes. A
    /// probe that timed out is a "no" kept for [`NO_ANSWER_TTL`] only.
    #[cfg(unix)]
    #[test]
    fn nfr_perf_a_convert_is_asked_what_it_is_once_per_binary() {
        use crate::util::fake_program::script;
        let dir = tempfile::tempdir().unwrap();
        let runs = dir.path().join("runs");
        let body = |name: &str| {
            format!(
                "echo run >> '{}'\necho 'Version: {name} 6.9.11-60 Q16'",
                runs.display()
            )
        };
        let count = || {
            std::fs::read_to_string(&runs)
                .map(|r| r.lines().count())
                .unwrap_or(0)
        };
        let convert = script(dir.path(), "convert", &body("ImageMagick"));
        assert!(is_heif_decoder(&convert));
        assert!(is_heif_decoder(&convert));
        // Through another path to the same file, too.
        let link = dir.path().join("bin");
        std::fs::create_dir(&link).unwrap();
        std::os::unix::fs::symlink(&convert, link.join("convert")).unwrap();
        assert!(is_heif_decoder(&link.join("convert")));
        assert_eq!(count(), 1, "convert was run more than once");

        // Same size, time and inode, but its status changed (a chmod; a
        // rewrite whose modification time was put back): asked again.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mode = std::fs::metadata(&convert).unwrap().permissions();
        std::fs::set_permissions(&convert, mode).unwrap();
        assert!(is_heif_decoder(&convert));
        assert_eq!(count(), 2, "a changed ctime was not asked again");

        // Replaced (another size and time): asked again.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let convert = script(dir.path(), "convert", &body("GraphicsMagick"));
        assert!(!is_heif_decoder(&convert));
        assert!(!is_heif_decoder(&convert));
        assert_eq!(count(), 3);

        // No answer is a "no" kept for a while, not for good: a convert
        // that hangs is run once a minute, not on every call.
        let slow = tempfile::tempdir().unwrap();
        let hung = slow.path().join("runs");
        let hangs = script(
            slow.path(),
            "convert",
            &format!("echo run >> '{}'\nsleep 30", hung.display()),
        );
        let hung_runs = || {
            std::fs::read_to_string(&hung)
                .map(|r| r.lines().count())
                .unwrap_or(0)
        };
        let before = hung_runs();
        let quick = std::time::Duration::from_secs(1);
        assert!(!heif_decoder_verdict(&hangs, quick));
        let started = std::time::Instant::now();
        assert!(!heif_decoder_verdict(&hangs, quick));
        assert!(started.elapsed() < quick, "the second call probed again");
        assert_eq!(hung_runs(), before + 1);
        let key = program_key(&hangs).unwrap();
        assert!(matches!(
            HEIF_DECODER_VERDICTS.lock().unwrap().get(&key),
            Some(Verdict::NoAnswer(_))
        ));
        // Once it is older than the TTL, it is asked again.
        let stale = std::time::Instant::now()
            .checked_sub(NO_ANSWER_TTL + std::time::Duration::from_secs(1))
            .unwrap();
        HEIF_DECODER_VERDICTS
            .lock()
            .unwrap()
            .insert(key, Verdict::NoAnswer(stale));
        assert!(!heif_decoder_verdict(&hangs, quick));
        assert_eq!(hung_runs(), before + 2);
    }

    #[test]
    fn bundled_layout_covers_the_installed_resource_dir() {
        let dirs = bundled_dirs(Path::new("/apps/Pluryprint"), "sumatrapdf");
        assert!(dirs.contains(&PathBuf::from("/apps/Pluryprint/resources/sumatrapdf")));
        assert!(dirs.contains(&PathBuf::from("/apps/Resources/sumatrapdf")));
        assert!(dirs.contains(&PathBuf::from("/apps/Pluryprint")));
    }

    #[test]
    fn windows_install_dirs_expand_from_the_environment() {
        let env = |name: &str| match name {
            "LOCALAPPDATA" => Some(r"C:\Users\ada\AppData\Local".to_string()),
            _ => None,
        };
        let dirs = install_dirs(Dependency::SumatraPdf, Os::Windows, &env);
        // Only the variable that is set contributes a candidate.
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].ends_with("SumatraPDF"));
    }

    #[test]
    fn sumatrapdf_is_not_applicable_off_windows() {
        for os in [Os::MacOs, Os::Linux] {
            let spec = spec(Dependency::SumatraPdf, os);
            assert!(!spec.applicable(), "{os:?}");
            let report = from_resolution(&spec, &Resolution::Missing);
            assert!(!report.applicable);
            assert!(
                report.remediation.is_none(),
                "a dependency this OS never uses must not raise an alarm"
            );
            assert!(report.blocks.is_empty());
        }
    }

    #[test]
    fn fr_058_every_converter_dependency_blocks_supported_formats_only() {
        for os in [Os::Windows, Os::MacOs, Os::Linux] {
            for dep in ALL {
                let spec = spec(*dep, os);
                for ext in spec.blocks {
                    assert!(
                        crate::engine::is_supported(ext),
                        "{dep:?} blocks {ext}, which the engine does not advertise"
                    );
                }
            }
        }
    }

    #[test]
    fn fr_058_missing_converters_name_what_they_block_and_how_to_install() {
        let cases = [
            (Dependency::LibreOffice, "docx", "libreoffice.org"),
            (Dependency::LibreOffice, "rtf", "libreoffice.org"),
            (Dependency::HeifCodec, "heic", "Microsoft Store"),
            (Dependency::HevcCodec, "heic", "Microsoft Store"),
        ];
        for (dep, ext, hint) in cases {
            let report = from_resolution(&spec(dep, Os::Windows), &Resolution::Missing);
            assert!(
                report.blocks.iter().any(|b| b == ext),
                "{dep:?} must block {ext}"
            );
            let text = report.remediation.expect("missing → remediation");
            assert!(text.starts_with(&report.name), "{text}");
            assert!(text.contains(hint), "{dep:?}: {text}");
            assert!(!text.contains("changes nothing"), "stale text: {text}");
        }
    }

    /// FR-055/056/057: HTML, Markdown and text print through the system web
    /// view, so neither a browser nor Pandoc is a dependency any more, and
    /// LibreOffice no longer gates plain text (nor RTF on macOS).
    #[test]
    fn fr_058_chromium_pandoc_not_required() {
        assert!(!ALL.contains(&Dependency::Chromium));
        assert!(!ALL.contains(&Dependency::Pandoc));
        for os in [Os::Windows, Os::MacOs, Os::Linux] {
            for dep in ALL {
                let blocks = spec(*dep, os).blocks;
                if *dep == Dependency::SumatraPdf {
                    continue; // the printer itself, on Windows
                }
                for ext in ["html", "htm", "md", "markdown", "txt"] {
                    assert!(!blocks.contains(&ext), "{dep:?} blocks {ext} on {os:?}");
                }
            }
            for dep in [Dependency::Chromium, Dependency::Pandoc] {
                assert!(spec(dep, os).blocks.is_empty(), "{dep:?} on {os:?}");
            }
        }
        assert!(!spec(Dependency::LibreOffice, Os::MacOs)
            .blocks
            .contains(&"rtf"));
    }

    /// FR-058: the LibreOffice remediation names RTF only where a missing
    /// LibreOffice really blocks it (Windows), never on macOS.
    #[test]
    fn fr_058_libreoffice_effect_mentions_rtf_only_where_it_is_blocked() {
        for os in [Os::Windows, Os::MacOs] {
            let s = spec(Dependency::LibreOffice, os);
            assert_eq!(
                s.effect.contains("RTF"),
                s.blocks.contains(&"rtf"),
                "{os:?}: {}",
                s.effect
            );
        }
        assert!(spec(Dependency::LibreOffice, Os::Windows)
            .blocks
            .contains(&"rtf"));
    }

    /// The codecs are Windows-only, asked of the OS, and never a false alarm
    /// elsewhere.
    #[test]
    fn fr_058_codecs_apply_on_windows_only() {
        for dep in [Dependency::HeifCodec, Dependency::HevcCodec] {
            assert!(spec(dep, Os::Windows).applicable());
            assert!(spec(dep, Os::Windows).codec);
            for os in [Os::MacOs, Os::Linux] {
                let report = from_resolution(&spec(dep, os), &Resolution::Missing);
                assert!(!report.applicable && report.remediation.is_none(), "{os:?}");
                assert!(report.install.is_empty());
            }
        }
    }

    /// D13: every missing dependency offers at least one install option, the
    /// last one always the vendor's page, which never needs a tool.
    #[test]
    fn fr_058_every_missing_dependency_offers_an_install_and_a_web_fallback() {
        for (os, deps) in [
            (
                Os::Windows,
                &[
                    Dependency::SumatraPdf,
                    Dependency::LibreOffice,
                    Dependency::HeifCodec,
                    Dependency::HevcCodec,
                ][..],
            ),
            (Os::MacOs, &[Dependency::LibreOffice][..]),
            (
                Os::Linux,
                &[Dependency::LibreOffice, Dependency::HeifDecoder][..],
            ),
        ] {
            for dep in deps {
                let report = from_resolution(&spec(*dep, os), &Resolution::Missing);
                let last = report.install.last().unwrap_or_else(|| panic!("{dep:?}"));
                assert_eq!(last.method, InstallMethod::Web, "{dep:?} on {os:?}");
                assert!(last.available);
                assert!(last.command.starts_with("https://"), "{}", last.command);
            }
        }
    }

    #[test]
    fn fr_058_winget_absent_falls_back_web() {
        let spec = spec(Dependency::LibreOffice, Os::Windows);
        let without = from_resolution_with(&spec, &Resolution::Missing, &|_| false);
        let with = from_resolution_with(&spec, &Resolution::Missing, &|t| t == InstallTool::Winget);
        let winget = |r: &DependencyReport| {
            r.install
                .iter()
                .find(|o| o.method == InstallMethod::Winget)
                .cloned()
                .unwrap()
        };
        assert!(!winget(&without).available);
        assert!(winget(&with).available);
        assert!(without
            .install
            .iter()
            .any(|o| o.method == InstallMethod::Web && o.available));
    }

    /// The consent dialog shows exactly what will run: the argument vector,
    /// never a shell string, with LibreOffice's UAC prompt and HEVC's price
    /// flagged.
    #[test]
    fn fr_058_install_commands_are_exact_argv() {
        let office = &spec(Dependency::LibreOffice, Os::Windows).install[0];
        assert_eq!(
            office.args(),
            [
                "install",
                "--id",
                "TheDocumentFoundation.LibreOffice",
                "--exact",
                "--source",
                "winget",
                "--accept-package-agreements",
                "--accept-source-agreements",
                "--disable-interactivity",
            ]
        );
        assert!(office.needs_admin);
        let heif = &spec(Dependency::HeifCodec, Os::Windows).install[0];
        assert_eq!(heif.method, InstallMethod::WingetStore);
        assert!(heif.args().contains(&"msstore".to_string()));
        assert!(!heif.paid);
        let hevc = spec(Dependency::HevcCodec, Os::Windows).install;
        assert!(
            hevc.iter().all(|r| r.paid && r.tool().is_none()),
            "winget cannot buy"
        );
        assert_eq!(
            hevc[0].url().unwrap(),
            "ms-windows-store://pdp/?ProductId=9NMZLZ57R3T7"
        );
        let cask = &spec(Dependency::LibreOffice, Os::MacOs).install[0];
        assert_eq!(cask.args(), ["install", "--cask", "libreoffice"]);
        let options = install_options(&[*cask], &|_| true);
        assert_eq!(options[0].command, "brew install --cask libreoffice");
    }

    /// The wire id, the reason a paused job carries and the i18n key are one
    /// spelling: serde's.
    #[test]
    fn fr_058_dependency_id_is_the_serde_spelling() {
        for dep in ALL
            .iter()
            .chain(&[Dependency::Chromium, Dependency::Pandoc])
        {
            let wire = serde_json::to_value(dep).unwrap();
            assert_eq!(wire, dep.id(), "{dep:?}");
        }
    }

    /// FR-052/058 on Linux: HEIC/HEIF go through an external decoder that is
    /// looked for like a program, blocks exactly those two formats while
    /// missing, and names the distribution packages that provide it. It is
    /// never a concern on macOS (`sips`) or Windows (WIC).
    #[test]
    fn fr_058_linux_heif_decoder_is_a_program_with_package_hints() {
        let linux = spec(Dependency::HeifDecoder, Os::Linux);
        assert!(linux.applicable() && !linux.codec);
        assert_eq!(
            linux.executables,
            ["heif-dec", "heif-convert", "magick", "convert"]
        );
        assert_eq!(linux.blocks, ["heic", "heif"]);
        let text = linux.remediation();
        for package in [
            "libheif-examples",
            "libheif-tools",
            "libheif-freeworld",
            "ImageMagick",
        ] {
            assert!(text.contains(package), "{text}");
        }
        let report = from_resolution(&linux, &Resolution::Missing);
        assert_eq!(report.install.len(), 1);
        assert_eq!(report.install[0].method, InstallMethod::Web);
        for os in [Os::Windows, Os::MacOs] {
            let report = from_resolution(&spec(Dependency::HeifDecoder, os), &Resolution::Missing);
            assert!(!report.applicable && report.remediation.is_none(), "{os:?}");
        }
        // Found anywhere on PATH, and in /usr/bin for a PATH-less start.
        let dirs = install_dirs(Dependency::HeifDecoder, Os::Linux, &|_| None);
        assert!(dirs.contains(&PathBuf::from("/usr/bin")));
        assert!(install_dirs(Dependency::HeifDecoder, Os::MacOs, &|_| None).is_empty());
    }

    /// FR-053 on Linux: LibreOffice is found as the distribution installs it,
    /// as The Document Foundation's own packages install it (`/opt`), and, last,
    /// as a Flatpak or a snap. RTF goes through it, so its message says so.
    #[test]
    fn fr_053_linux_libreoffice_is_found_where_each_package_puts_it() {
        let s = spec(Dependency::LibreOffice, Os::Linux);
        assert_eq!(
            s.executables,
            ["soffice", "libreoffice", FLATPAK_LIBREOFFICE]
        );
        assert!(s.blocks.contains(&"rtf") && s.effect.contains("RTF"));
        assert!(s.effect.contains("libreoffice package"), "{}", s.effect);
        assert_eq!(s.install[0].method, InstallMethod::Web);

        let env = |name: &str| (name == "HOME").then(|| "/home/ada".to_string());
        let dirs = install_dirs(Dependency::LibreOffice, Os::Linux, &env);
        let at = |p: &str| {
            dirs.iter()
                .position(|d| d == Path::new(p))
                .unwrap_or_else(|| panic!("{p} not in {dirs:?}"))
        };
        assert!(at("/usr/bin") < at("/usr/lib/libreoffice/program"));
        assert!(
            at("/usr/lib64/libreoffice/program") < at("/home/ada/.local/share/flatpak/exports/bin")
        );
        assert!(at("/var/lib/flatpak/exports/bin") < at("/snap/bin"));
        assert_eq!(dirs.last().unwrap(), Path::new("/snap/bin"));
        // Without HOME the per-user Flatpak folder is simply not a candidate.
        let bare = install_dirs(Dependency::LibreOffice, Os::Linux, &|_| None);
        assert_eq!(bare.len(), dirs.len() - 1);
    }

    #[test]
    fn fr_053_tdf_packages_in_opt_are_searched_newest_first() {
        let entries: Vec<String> = [
            "google",
            "libreoffice7.6",
            "libreoffice",
            "libreoffice24.8",
            "libreoffice25.2",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            opt_libreoffice_dirs(&entries),
            [
                PathBuf::from("/opt/libreoffice25.2/program"),
                PathBuf::from("/opt/libreoffice24.8/program"),
                PathBuf::from("/opt/libreoffice7.6/program"),
                PathBuf::from("/opt/libreoffice/program"),
            ]
        );
        assert!(opt_libreoffice_dirs(&[]).is_empty());
    }

    #[test]
    fn a_found_converter_blocks_nothing() {
        let spec = spec(Dependency::LibreOffice, Os::MacOs);
        let report = from_resolution(
            &spec,
            &Resolution::Found {
                path: PathBuf::from("/Applications/LibreOffice.app/Contents/MacOS/soffice"),
                found_in: FoundIn::Installed,
            },
        );
        assert!(report.blocks.is_empty());
        assert!(report.remediation.is_none());
    }
}
