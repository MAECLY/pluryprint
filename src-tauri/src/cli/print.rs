//! `pluryprint print …` (FR-008).
//!
//! The headless print hop. It resolves a profile plus the per-invocation
//! overrides into one [`JobSettings`], applies exactly the acceptance rules
//! [`crate::queue::QueueHandle::submit`] applies (existence, size ceiling,
//! supported format, settings and page-range validity), then walks the files
//! one at a time through the same engine the dispatcher uses:
//! [`crate::engine::convert`] → [`crate::engine::print::spool`].
//!
//! Two deliberate differences from the GUI queue, both because a CLI run is a
//! *foreground* operation that ends when the process ends:
//!
//! - **No `queue.json`, no persistence.** A headless run never touches the
//!   queue file, so it cannot race a running app for it (NFR-014). Its jobs are
//!   ephemeral by definition — the process holding them is about to exit.
//! - **Its own conversion cache.** Converted PDFs go to a private temp
//!   directory that is deleted on exit, rather than the shared conversions
//!   cache the GUI sweeps at boot. Neither process can delete the other's work.
//!
//! There is no retry/backoff either: a script wants an exit code, not a job
//! that quietly retries for half a minute.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use serde::Serialize;

use crate::cli::args::{PrintArgs, SettingFlags};
use crate::cli::exit::{CliError, ExitCode};
use crate::cli::output::{table, Emitter};
use crate::cli::{files, handoff, profiles, Paths};
use crate::profiles::{Profile, ProfileDuplex};
use crate::queue::job::{Duplex, JobError, JobSettings};
use crate::util::error::IpcError;

/// `config.toml` for a print run, refusing one that exists but cannot be read.
///
/// The GUI's read paths degrade to defaults and flag the file
/// ([`crate::settings::Config::load_reporting`]) because a window still has to
/// render. A headless print has nobody watching a banner, and acting on the
/// defaults would quietly drop the user's disabled printers, routing rules and
/// per-printer overrides, so it stops with the documented exit 3 (`Config`)
/// instead. A *missing* file is the normal first-run case and loads as the
/// defaults.
pub(crate) fn load_config(path: &Path) -> Result<crate::settings::Config, CliError> {
    let load = crate::settings::Config::load_reporting(path);
    if load.unreadable {
        return Err(CliError::new(
            ExitCode::Config,
            IpcError::localized(
                crate::util::error::IpcErrorCode::Internal,
                "config_corrupt",
                format!(
                    "{} could not be read — fix or remove it; nothing was printed",
                    path.display()
                ),
            ),
        ));
    }
    Ok(load.config)
}

/// What happened to one file.
#[derive(Debug, Clone, Serialize)]
pub struct FileOutcome {
    /// The input path as given (after wildcard and folder expansion).
    pub path: String,
    /// Where this file actually went. Per file because a file-type routing rule
    /// can split one batch across printers.
    pub printer: String,
    /// `done` (the spooler accepted the job), `held` (accepted, but the
    /// printer is paused, offline or stopped on an error, or the spooler holds
    /// the job itself — CUPS `pending-held`, a paused Windows job — so nothing
    /// prints until someone acts), `unconfirmed` (accepted, but the printers or
    /// the job's own state could not be read, so whether it prints now is
    /// unknown), or `failed`.
    pub state: String,
    /// The spooler's own job id, when the backend reports one (CUPS does).
    pub os_job_id: Option<String>,
    /// Why the file failed, when `state` is `failed`.
    pub error: Option<JobError>,
}

/// The document `print --json` emits: one batch, one resolved settings bundle,
/// one row per file.
#[derive(Debug, Clone, Serialize)]
pub struct PrintReport {
    /// Id of the queue batch this run created.
    pub batch_id: String,
    /// The batch target: the shared printer, or `<per file type>` when routing
    /// rules split the batch. The authoritative per-file value lives in
    /// [`FileOutcome::printer`].
    pub printer: String,
    /// Name of the profile the settings came from, if one was used.
    pub profile_name: Option<String>,
    /// The settings every file was printed with.
    pub settings: JobSettings,
    /// One row per input file, in input order.
    pub files: Vec<FileOutcome>,
    /// The exit code the process will return (the worst among the files).
    pub exit_code: i32,
}

/// Run `print`, returning the exit code for the batch.
///
/// Anything that invalidates the *whole* invocation (a missing file, an
/// unusable setting, no printer) is an `Err` and nothing is printed at all —
/// the same all-or-nothing acceptance the GUI has. Once the batch is accepted,
/// each file's outcome is recorded and the worst code wins.
pub fn run(paths: &Paths, emitter: Emitter, args: PrintArgs) -> Result<ExitCode, CliError> {
    // One read of `config.toml` for the whole run. A file that exists but
    // cannot be parsed stops the run with exit 3 (`Config`): printing on the
    // defaults would ignore the user's disabled printers, routes and
    // per-printer overrides without a word.
    let config = load_config(&paths.config)?;
    // Engine limits come from the same (sanitized) config the queue reads at
    // boot, so a headless run accepts exactly the files the app would.
    let engine = config.engine.clone().sanitized();

    // The password (FR-059) is read before anything else touches the batch,
    // so a bad `--password-file` stops the run with nothing printed.
    let password = read_password(&args)?;

    let inputs = collect_inputs(&args)?;
    let expanded = files::expand_inputs(&inputs, engine.max_file_size_mb)?;

    let profile = match &args.profile {
        Some(name_or_id) => profiles::resolve(paths, name_or_id)?,
        None => profiles::default_profile(paths)?,
    };
    let settings = resolve_settings(&profile, args.printer.clone(), &args.settings)?;

    let files = accept_files(&expanded, engine.max_file_size_mb)?;
    check_ranges(&settings, &files)?;

    // Resolve and validate every file up front. A file-type routing rule can
    // send a `.pdf` and a `.jpg` of the same batch to different printers, and
    // duplex support is a property of the printer, so both the resolution and
    // the capability gate are per file. Doing the whole pass before printing
    // anything preserves the all-or-nothing acceptance described above: one
    // unusable target means nothing is spooled.
    let targets: Vec<(String, crate::engine::print::PrinterCaps, JobSettings)> = files
        .iter()
        .map(|file| {
            let name = Path::new(file)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| file.clone());
            let (printer, caps) = resolve_printer(&config, settings.printer.as_deref(), &name)?;
            // The printer's own tray/paper defaults fill what the job leaves
            // open, as in the queue; the check is against what will be sent.
            let over = config
                .printers
                .overrides
                .get(&printer)
                .cloned()
                .unwrap_or_default();
            let effective = crate::engine::print::apply_printer_overrides(&settings, &over);
            crate::engine::print::check_supported(&effective, &printer, &caps)?;
            Ok((printer, caps, effective.into_owned()))
        })
        .collect::<Result<_, CliError>>()?;
    // What the batch reports as its target: the common printer when routing did
    // not split it, otherwise a marker rather than one file's printer standing
    // in for the rest.
    let printer = match targets.first() {
        Some((first, _, _)) if targets.iter().all(|(p, _, _)| p == first) => first.clone(),
        Some(_) => "<per file type>".to_string(),
        None => String::new(),
    };
    // Two budgets, exactly as the queue uses them: the converter is bounded by
    // the conversion timeout, the spool wait by the print timeout. `0`
    // disables either.
    let timeouts = Timeouts::from_engine(&engine);

    // Private, self-deleting conversion cache — see the module docs.
    let cache = tempfile::Builder::new()
        .prefix("pluryprint-cli-")
        .tempdir()
        .map_err(|e| {
            CliError::from(IpcError::Internal(format!(
                "could not create a conversion cache: {e}"
            )))
        })?;

    let batch_id = uuid::Uuid::now_v7().to_string();
    let mut batch = BatchOutcomes::with_capacity(files.len());

    for (index, file) in files.iter().enumerate() {
        let job_id = format!("{batch_id}-{index}");
        let (target, caps, effective) = targets
            .get(index)
            .cloned()
            .unwrap_or_else(|| (String::new(), Default::default(), settings.clone()));
        let outcome = print_one(
            Path::new(file),
            cache.path(),
            &job_id,
            &target,
            &caps,
            &effective,
            timeouts,
            password.as_ref(),
        );
        batch.record(file, target, outcome);
    }

    // "done" is "the spooler took it". When the target queue is paused,
    // offline or stopped on an error nothing comes out until someone fixes it,
    // and a script must be able to tell (the spooler itself still calls such a
    // job "normal"). When the printers cannot be read at all, nobody can say,
    // and "done" would be a guess.
    if batch.has_done() {
        match crate::platform::list_printers() {
            Ok(printers) => batch.mark_held(&not_printing(&printers)),
            Err(e) => {
                tracing::warn!(event = "cli_printer_state_unknown", error = %e);
                batch.mark_unconfirmed();
            }
        }
    }
    // A ready queue can still hold the job itself: a CUPS queue with a default
    // `job-hold-until` parks every job as `pending-held`, and a Windows job can
    // be paused on its own. So each job still `done` is looked up by its id.
    if batch.has_done() {
        batch.mark_held_jobs(crate::engine::print::job_state);
    }

    let worst = batch.worst();
    let report = PrintReport {
        batch_id,
        printer,
        profile_name: Some(profile.name.clone()),
        settings,
        files: batch.into_files(),
        exit_code: worst.code(),
    };
    emitter.emit(&report, || render(&report));
    Ok(worst)
}

/// The printers that will not print a job right now: paused or offline
/// (`Offline`), or stopped on an error someone must clear (`Error`: out of
/// paper, a jam, a door open, a CUPS queue that stopped). Pure.
pub fn not_printing(
    printers: &[crate::ipc::printers::Printer],
) -> std::collections::HashSet<String> {
    use crate::ipc::printers::PrinterStatus;
    printers
        .iter()
        .filter(|p| matches!(p.status, PrinterStatus::Offline | PrinterStatus::Error))
        .map(|p| p.id.clone())
        .collect()
}

/// One batch's per-file outcomes plus the code the process will return.
///
/// Split out of [`run`] because it is the whole of FR-009's batch rule — every
/// file is recorded whatever happened to it, and the *worst* code wins, so one
/// failure in fifty is never reported as success — and inside `run` that rule
/// could only be exercised by spooling to a real printer.
#[derive(Debug)]
pub struct BatchOutcomes {
    files: Vec<FileOutcome>,
    worst: ExitCode,
}

impl BatchOutcomes {
    /// An empty batch, which reports [`ExitCode::Success`]: nothing failed.
    pub fn with_capacity(files: usize) -> Self {
        Self {
            files: Vec::with_capacity(files),
            worst: ExitCode::Success,
        }
    }

    /// Record what [`print_one`] made of one file: the spooler's job id on
    /// success, or the failure — which also raises the batch's code.
    pub fn record(
        &mut self,
        path: &str,
        printer: String,
        outcome: Result<Option<String>, CliError>,
    ) {
        let file = match outcome {
            Ok(os_job_id) => FileOutcome {
                path: path.to_string(),
                printer,
                state: "done".to_string(),
                os_job_id,
                error: None,
            },
            Err(err) => {
                self.worst = self.worst.worst(err.exit);
                let error = job_error(&err.error);
                // The queue logs `job_failed`; a headless run must leave the
                // same trace, or a failed scripted print cannot be explained
                // afterwards. The file name, never its folder, and the stable
                // code, never the message: messages quote absolute paths and
                // SumatraPDF's own log, which names the staged copy (NFR-082).
                // The message is in the report the caller receives.
                let name = Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                tracing::warn!(
                    event = "cli_job_failed",
                    file = %name,
                    ext = %crate::engine::extension_of(Path::new(path)),
                    printer = %printer,
                    code = %error.code,
                    reason = %err.error.reason().unwrap_or_default(),
                    exit_code = err.exit.code(),
                );
                FileOutcome {
                    path: path.to_string(),
                    printer,
                    state: "failed".to_string(),
                    os_job_id: None,
                    error: Some(error),
                }
            }
        };
        self.files.push(file);
    }

    /// Whether any file was accepted by a spooler.
    pub fn has_done(&self) -> bool {
        self.files.iter().any(|f| f.state == "done")
    }

    /// Turn `done` into `unconfirmed` for every file: the spooler took them,
    /// but the printers could not be read, so whether they print now is
    /// unknown. The exit code is unchanged.
    pub fn mark_unconfirmed(&mut self) {
        for f in &mut self.files {
            if f.state == "done" {
                f.state = "unconfirmed".to_string();
            }
        }
    }

    /// Turn `done` into `held` for every file spooled to a queue in
    /// `not_printing` ([`not_printing`]: paused, offline or stopped on an
    /// error). The exit code is unchanged: the job is safely queued and prints
    /// once the printer is back.
    pub fn mark_held(&mut self, not_printing: &std::collections::HashSet<String>) {
        for f in &mut self.files {
            if f.state == "done" && not_printing.contains(&f.printer) {
                f.state = "held".to_string();
            }
        }
    }

    /// Ask the spooler about each file still `done` that has a job id, through
    /// `read` ([`crate::engine::print::job_state`] in production), and turn it
    /// into `held` when the spooler itself holds the job: `pending-held` on
    /// CUPS (a queue whose default `job-hold-until` parks every job, while the
    /// queue itself reads as ready), a job paused in the Windows queue, or one
    /// stopped on a device problem. A state that cannot be read makes the file
    /// `unconfirmed` rather than a guessed `done`. A file without a job id has
    /// nothing to ask about and keeps `done`. The exit code is unchanged.
    pub fn mark_held_jobs(
        &mut self,
        mut read: impl FnMut(
            &str,
            &str,
        ) -> Result<
            crate::engine::print::OsJobState,
            crate::engine::print::TrackError,
        >,
    ) {
        use crate::engine::print::OsJobState;
        for f in &mut self.files {
            if f.state != "done" {
                continue;
            }
            let Some(id) = f.os_job_id.as_deref() else {
                continue;
            };
            match read(&f.printer, id) {
                Ok(OsJobState::Held | OsJobState::Stopped) => f.state = "held".to_string(),
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(event = "cli_job_state_unknown", printer = %f.printer, error = %e);
                    f.state = "unconfirmed".to_string();
                }
            }
        }
    }

    /// The exit code the batch reports so far.
    pub fn worst(&self) -> ExitCode {
        self.worst
    }

    /// The recorded rows, in the order the files were printed.
    pub fn into_files(self) -> Vec<FileOutcome> {
        self.files
    }
}

/// Bounds-check the resolved page range against the documents it will apply to,
/// before anything is spooled.
///
/// This calls the *same* [`crate::ipc::jobs::check_ranges_fit_documents`] the
/// print dialog calls, deliberately: `--range 500-600` on an 8-page report used
/// to be accepted here, spooled, and print nothing at all, while the GUI refused
/// it with a message naming the file. A second implementation would drift again,
/// so there is only one.
fn check_ranges(settings: &JobSettings, files: &[String]) -> Result<(), CliError> {
    let Some(ranges) = settings.page_ranges.as_deref() else {
        return Ok(());
    };
    crate::ipc::jobs::check_ranges_fit_documents(ranges, files)
        .map_err(|e| CliError::from(IpcError::from(e)))
}

/// The two wall-clock budgets of a headless print, read from `[engine]`.
///
/// They are separate knobs in the app (`conversion_timeout_seconds` bounds a
/// converter, `print_timeout_seconds` the spooler), and the CLI must honour
/// both the same way: bounding a LibreOffice run with the print budget killed
/// conversions the user had allowed to take longer, and left them unbounded
/// whenever the print budget was disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    /// Budget for one conversion (`None` = unbounded).
    pub conversion: Option<std::time::Duration>,
    /// Budget for one spooler submission (`None` = unbounded).
    pub print: Option<std::time::Duration>,
}

impl Timeouts {
    /// The budgets a (sanitized) engine config asks for; `0` disables one.
    pub fn from_engine(engine: &crate::settings::EngineCfg) -> Self {
        let secs = |s: u32| (s > 0).then(|| std::time::Duration::from_secs(u64::from(s)));
        Self {
            conversion: secs(engine.conversion_timeout_seconds),
            print: secs(engine.print_timeout_seconds),
        }
    }
}

/// Convert one file (PDFs print in place) and hand it to the spooler.
///
/// "Done" means the OS accepted the job, not that paper came out: no spooler
/// tells us the latter synchronously, and claiming otherwise would be the kind
/// of dishonesty the print-settings policy forbids.
///
/// `password` opens an encrypted PDF (FR-059): the decrypted copy lands in
/// the run's private cache and is deleted with it. A protected PDF without
/// it, or with a wrong one, fails this file only (exit 13).
#[allow(clippy::too_many_arguments)]
fn print_one(
    input: &Path,
    cache_dir: &Path,
    job_id: &str,
    printer: &str,
    caps: &crate::engine::print::PrinterCaps,
    settings: &JobSettings,
    timeouts: Timeouts,
    password: Option<&crate::engine::PdfPassword>,
) -> Result<Option<String>, CliError> {
    // The CLI has no cancel channel (Ctrl-C kills the process), so the flag the
    // engine polls is always false here.
    let cancel = AtomicBool::new(false);
    let converted = crate::engine::convert::convert_to_pdf(
        input,
        cache_dir,
        job_id,
        &cancel,
        &|_| {},
        timeouts.conversion,
        &crate::engine::convert::PageSetup::for_settings(settings),
        password,
    )?;

    let pdf = converted.unwrap_or_else(|| input.to_path_buf());
    let title = input
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| input.display().to_string());

    // Page ranges only apply to paged documents, so the engine narrows them per input
    // exactly as the dispatcher does — an image would otherwise spool empty.
    let settings =
        crate::engine::print::effective_settings(settings, &crate::engine::extension_of(input));
    let outcome =
        crate::engine::print::spool(&pdf, printer, &settings, caps, &title, timeouts.print)?;
    // The basename (`title`), never the absolute path (NFR-082).
    tracing::info!(event = "cli_job_printed", file = %title, printer = %printer);
    Ok(outcome.os_job_id)
}

/// The password for the batch's encrypted PDFs, from `--password-file` or
/// `--password-stdin` (FR-059, NFR-082). Never from argv, never logged.
///
/// `--password-stdin` refuses a terminal: reading a typed password there
/// would echo it on screen. Pipe it in instead.
fn read_password(args: &PrintArgs) -> Result<Option<crate::engine::PdfPassword>, CliError> {
    use std::io::{IsTerminal, Read};
    let text = if let Some(path) = &args.password_file {
        let path = &crate::cli::from_caller(path);
        let mut text = zeroize::Zeroizing::new(String::new());
        std::fs::File::open(path)
            .and_then(|mut f| f.read_to_string(&mut text))
            .map_err(|e| {
                CliError::usage(format!(
                    "cannot read the password file {}: {e}",
                    path.display()
                ))
            })?;
        text
    } else if args.password_stdin {
        let stdin = std::io::stdin();
        if stdin.is_terminal() {
            return Err(CliError::usage(
                "--password-stdin reads a piped password; pipe it in (for example \
                 `printf %s \"$PW\" | pluryprint print …`) or use --password-file",
            ));
        }
        let mut text = zeroize::Zeroizing::new(String::new());
        stdin
            .lock()
            .read_to_string(&mut text)
            .map_err(|e| CliError::usage(format!("cannot read the password from stdin: {e}")))?;
        text
    } else {
        return Ok(None);
    };
    password_from_text(&text).map(Some)
}

/// The password in `text`: its first line, without the line ending. An empty
/// one is refused (`password_empty`). Pure.
pub fn password_from_text(text: &str) -> Result<crate::engine::PdfPassword, CliError> {
    let line = text.split('\n').next().unwrap_or_default();
    let line = line.strip_suffix('\r').unwrap_or(line);
    crate::engine::PdfPassword::new(line.to_string()).ok_or_else(|| {
        CliError::from(IpcError::localized(
            crate::util::error::IpcErrorCode::InvalidRequest,
            "password_empty",
            "the password file or stdin holds no password",
        ))
    })
}

/// The file list, from `--files` or from a `--files-from` handoff payload.
/// Exactly one of the two is present (clap enforces the group).
fn collect_inputs(args: &PrintArgs) -> Result<Vec<String>, CliError> {
    match &args.files_from {
        Some(path) => Ok(handoff::read_file(&crate::cli::from_caller(path))?.paths),
        None => Ok(args.files.clone()),
    }
}

/// Fold a profile and the CLI overrides into one settings bundle, then validate
/// it the way the queue would.
///
/// `--printer` beats the profile's printer; every other flag overrides its
/// profile counterpart and leaves the rest alone.
pub fn resolve_settings(
    profile: &Profile,
    printer: Option<String>,
    flags: &SettingFlags,
) -> Result<JobSettings, CliError> {
    let mut settings = from_profile(profile);

    if let Some(printer) = printer.filter(|p| !p.trim().is_empty()) {
        settings.printer = Some(printer);
    }
    if let Some(mode) = flags.color_mode() {
        settings.color_mode = mode;
    }
    if let Some(duplex) = flags.duplex {
        settings.duplex = duplex.to_engine();
    }
    if let Some(copies) = flags.copies {
        settings.copies = copies;
    }
    if let Some(paper) = &flags.paper {
        settings.paper_size = paper.clone();
    }
    if let Some(scale) = flags.scale {
        settings.scaling = scale.to_engine();
    }
    if let Some(orientation) = flags.orientation {
        settings.orientation = orientation.to_engine();
    }
    if let Some(range) = &flags.range {
        settings.page_ranges = Some(range.clone());
    }
    if let Some(collate) = flags.collate() {
        settings.collate = collate;
    }

    settings
        .validate()
        .map_err(|e| CliError::from(IpcError::from(e)))?;
    if let Some(ranges) = &settings.page_ranges {
        crate::engine::pagerange::parse(ranges).map_err(|e| CliError::from(IpcError::from(e)))?;
    }
    Ok(settings)
}

/// A profile's engine-facing settings. The `<default>` printer sentinel becomes
/// `None`, which the dispatch step resolves against the app/OS default, and so
/// does the `auto` tray: [`JobSettings::tray`] names a bin the driver knows, and
/// "auto" is the absence of a choice rather than a bin name to pass through.
pub fn from_profile(profile: &Profile) -> JobSettings {
    let printer = profile.printer.trim();
    let tray = profile.tray.trim();
    JobSettings {
        printer: (!printer.is_empty() && printer != "<default>").then(|| printer.to_string()),
        duplex: match profile.duplex {
            ProfileDuplex::Off => Duplex::None,
            ProfileDuplex::LongEdge => Duplex::Long,
            ProfileDuplex::ShortEdge => Duplex::Short,
        },
        color_mode: profile.color_mode,
        copies: profile.copies,
        paper_size: profile.paper_size.clone(),
        orientation: profile.orientation,
        scaling: profile.scaling,
        page_ranges: profile.page_range.clone(),
        quality: profile.quality,
        tray: (!tray.is_empty() && !tray.eq_ignore_ascii_case("auto")).then(|| tray.to_string()),
        collate: profile.collate,
    }
}

/// Apply the queue's acceptance rules to every file before anything is
/// printed — it must resolve to an existing regular file, fit the configured
/// size ceiling and be a format the engine can print — and return the
/// canonical paths to print. One bad file rejects the batch, exactly as
/// `print_files` does in the UI.
///
/// Resolution goes through [`crate::util::paths::canonical_input`], the same
/// gate every externally-supplied path passes (NFR-025), so a `..` segment or a
/// symlink names the same job here as it would in the app.
fn accept_files(files: &[String], max_mb: u32) -> Result<Vec<String>, CliError> {
    let mut accepted = Vec::with_capacity(files.len());

    for file in files {
        let path = crate::util::paths::canonical_input(file)?;
        let meta = std::fs::metadata(&path)
            .map_err(|_| CliError::from(IpcError::FileNotFound(file.clone())))?;
        // The same reasons the app's submit gives (FR-092), naming the file.
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.clone());
        let invalid = |reason: &'static str, message: String| {
            IpcError::localized(
                crate::util::error::IpcErrorCode::InvalidRequest,
                reason,
                message,
            )
            .with_param("name", &name)
        };
        if !meta.is_file() {
            let error = invalid("not_a_file", format!("{file} is not a file"));
            return Err(CliError::new(ExitCode::Usage, error));
        }
        if crate::queue::exceeds_size_limit(meta.len(), max_mb) {
            let error = invalid(
                "file_too_large",
                format!("{file} is larger than the {max_mb} MB limit"),
            )
            .with_param("limit", max_mb);
            return Err(CliError::new(ExitCode::Usage, error));
        }
        let ext = crate::engine::extension_of(&path);
        if !crate::engine::is_supported(&ext) {
            return Err(CliError::from(crate::queue::unsupported_format_error(
                file, &path, &ext,
            )));
        }
        accepted.push(path.to_string_lossy().into_owned());
    }
    Ok(accepted)
}

/// Resolve the target printer and whether it can duplex, mirroring the
/// dispatcher's [`crate::engine::print::PrintError`] semantics: the requested
/// name, else Pluryprint's app default, else the OS default; a name the OS
/// does not enumerate is refused rather than handed to the spooler.
///
/// Both failures are [`ExitCode::Printer`], so a script can tell "this host is
/// misconfigured" from "the job itself failed". Where the GUI *pauses* such a
/// job for the user to pick a replacement, the CLI has no one to ask and says
/// so immediately.
fn resolve_printer(
    config: &crate::settings::Config,
    requested: Option<&str>,
    file_name: &str,
) -> Result<(String, crate::engine::print::PrinterCaps), CliError> {
    // Same order as the queue: explicit request, then the file-type routing
    // rule, then the default (org policy, else app) — one definition, in
    // `ipc::printers`.
    let prefs = crate::ipc::printers::with_policy(
        config.printers.clone(),
        &crate::settings::PrinterPolicy::load(),
    );
    let name = match crate::ipc::printers::resolve_app_printer(&prefs, requested, file_name) {
        Some(p) => p,
        None => crate::platform::default_printer()
            .ok()
            .flatten()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                CliError::printer("no printer is configured; pass --printer or set a default")
            })?,
    };
    // A printer disabled in Pluryprint is refused by name, like the queue does.
    crate::ipc::printers::ensure_enabled(&prefs, &name)
        .map_err(|e| CliError::printer(e.to_string()))?;

    // A failed or empty enumeration is not evidence of absence (the platform
    // backends are best effort), so it keeps trusting the name and lets the
    // spooler speak — the dispatcher makes the same call.
    let enumerated = crate::platform::list_printers()
        .ok()
        .filter(|list| !list.is_empty());
    let Some(list) = enumerated else {
        return Ok((name, crate::engine::print::PrinterCaps::unknown()));
    };
    // `--printer` accepts the id or the display name `printers list` shows,
    // but `lp -d` only knows the queue id — so spool to the matched id.
    use crate::ipc::printers::{match_installed, PrinterMatch};
    match match_installed(&list, &name) {
        PrinterMatch::Found(p) => {
            // A display name can hide a queue id the user disabled.
            crate::ipc::printers::ensure_enabled(&prefs, &p.id)
                .map_err(|e| CliError::printer(e.to_string()))?;
            Ok((p.id.clone(), crate::platform::printer_caps(p)))
        }
        PrinterMatch::Ambiguous(ids) => Err(CliError::printer(format!(
            "printer name '{name}' matches several queues ({}) — pass the queue id",
            ids.join(", ")
        ))),
        PrinterMatch::Missing => Err(CliError::printer(format!(
            "printer '{name}' is not installed — choose another printer"
        ))),
    }
}

/// An [`IpcError`] as the `{code, message}` pair the queue stores on a job, so
/// a CLI report and a queue entry describe a failure identically. A localized
/// error also keeps its stable `reason` and `params`, so a script can tell,
/// say, `password_required` from `printing_not_allowed` (both exit 13).
fn job_error(err: &IpcError) -> JobError {
    let value = serde_json::to_value(err).ok();
    let field = |name: &str| value.as_ref().map(|v| v[name].clone());
    let code = field("code")
        .and_then(|c| c.as_str().map(str::to_string))
        .unwrap_or_else(|| "Internal".to_string());
    let mut error = JobError::new(code, err.to_string());
    // Set directly: these are IPC-family reasons (already registered under
    // their IPC code), not the queue's job-code reasons `with_reason` checks.
    error.reason = err.reason().map(str::to_string);
    error.params = field("params")
        .and_then(|p| serde_json::from_value(p).ok())
        .filter(|p: &std::collections::BTreeMap<String, String>| !p.is_empty());
    error
}

/// Human-mode rendering: the resolved target, then one row per file.
fn render(report: &PrintReport) -> String {
    let rows: Vec<Vec<String>> = report
        .files
        .iter()
        .map(|f| {
            vec![
                f.state.clone(),
                f.path.clone(),
                f.error
                    .as_ref()
                    .map(|e| e.message.clone())
                    .unwrap_or_default(),
            ]
        })
        .collect();
    format!(
        "Printer: {}\nProfile: {}\n{}",
        report.printer,
        report.profile_name.as_deref().unwrap_or("<none>"),
        table(&["STATE", "FILE", "ERROR"], &rows)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::{CliDuplex, CliScale};
    use crate::queue::job::{ColorMode, PaperSize, Scaling};

    fn quick() -> Profile {
        crate::profiles::builtins().remove(0)
    }

    /// FR-010 / FR-059: a failed file in the JSON report keeps the stable
    /// reason (and params) of a localized error, so a script can tell the
    /// exit-13 cases apart; a plain error has none.
    #[test]
    fn fr_059_report_error_keeps_the_reason() {
        use crate::engine::{EngineError, PdfProtection};
        let err = IpcError::from(EngineError::PdfProtected(PdfProtection::PrintingNotAllowed));
        let row = job_error(&err);
        assert_eq!(row.code, "PdfProtected");
        assert_eq!(row.reason.as_deref(), Some("printing_not_allowed"));

        let gif = crate::queue::unsupported_format_error(
            "/x/a.gif",
            std::path::Path::new("/x/a.gif"),
            "gif",
        );
        let row = job_error(&gif);
        assert_eq!(row.reason.as_deref(), Some("gif_not_supported"));
        assert_eq!(row.params.unwrap()["name"], "a.gif");

        let plain = job_error(&IpcError::PrintFailed("lp exploded".into()));
        assert_eq!((plain.reason, plain.params), (None, None));
    }

    /// FR-059: the password is the first line, without its line ending (a
    /// file written by `echo` or Notepad works); an empty one is refused.
    #[test]
    fn fr_059_password_from_text_takes_the_first_line() {
        for text in ["s3cret", "s3cret\n", "s3cret\r\n", "s3cret\nignored\n"] {
            assert_eq!(password_from_text(text).unwrap().expose(), "s3cret");
        }
        // Spaces are part of a password.
        assert_eq!(password_from_text(" a b \n").unwrap().expose(), " a b ");
        for empty in ["", "\n", "\r\n"] {
            let err = password_from_text(empty).unwrap_err();
            assert_eq!(err.exit, ExitCode::Usage);
            assert_eq!(err.error.reason(), Some("password_empty"));
        }
    }

    /// FR-059: `--password-file` is read before the batch starts; an
    /// unreadable one stops the run (usage, exit 2) without echoing anything
    /// it may have held.
    #[test]
    fn fr_059_password_file_is_read_or_refused() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pw.txt");
        std::fs::write(&file, "s3cret\n").unwrap();
        let args = |password_file: Option<std::path::PathBuf>| PrintArgs {
            files: vec!["a.pdf".into()],
            files_from: None,
            profile: None,
            printer: None,
            settings: SettingFlags::default(),
            no_dialog: false,
            wait: false,
            password_file,
            password_stdin: false,
        };
        let pw = read_password(&args(Some(file))).unwrap().unwrap();
        assert_eq!(pw.expose(), "s3cret");
        assert!(read_password(&args(None)).unwrap().is_none());
        let err = read_password(&args(Some(dir.path().join("missing")))).unwrap_err();
        assert_eq!(err.exit, ExitCode::Usage);
    }

    #[test]
    fn fr_008_profile_supplies_the_baseline_settings() {
        let duplex_bw = crate::profiles::builtins().remove(1);
        let settings = resolve_settings(&duplex_bw, None, &SettingFlags::default()).unwrap();
        assert_eq!(settings.duplex, Duplex::Long);
        assert_eq!(settings.color_mode, ColorMode::Monochrome);
        // "<default>" never leaks into the engine as a printer name.
        assert_eq!(settings.printer, None);
    }

    #[test]
    fn fr_008_flags_override_the_profile() {
        let flags = SettingFlags {
            mono: true,
            duplex: Some(CliDuplex::ShortEdge),
            copies: Some(3),
            paper: Some(PaperSize::LETTER),
            scale: Some(CliScale::Actual),
            range: Some("1-5,8".to_string()),
            ..Default::default()
        };
        let settings = resolve_settings(&quick(), Some("Front Desk".to_string()), &flags).unwrap();
        assert_eq!(settings.printer.as_deref(), Some("Front Desk"));
        assert_eq!(settings.color_mode, ColorMode::Monochrome);
        assert_eq!(settings.duplex, Duplex::Short);
        assert_eq!(settings.copies, 3);
        assert_eq!(settings.paper_size, PaperSize::LETTER);
        assert_eq!(settings.scaling, Scaling::None);
        assert_eq!(settings.page_ranges.as_deref(), Some("1-5,8"));
    }

    #[test]
    fn fr_008_collate_flags_override_the_profile() {
        use crate::cli::args::{Cli, Command};
        use clap::Parser;

        let profile = quick();
        assert!(
            resolve_settings(&profile, None, &SettingFlags::default())
                .unwrap()
                .collate
        );

        let parsed =
            Cli::try_parse_from(["pluryprint", "print", "--files", "a.pdf", "--no-collate"])
                .expect("--no-collate parses");
        let Some(Command::Print(args)) = parsed.command else {
            panic!("expected print");
        };
        assert!(
            !resolve_settings(&profile, None, &args.settings)
                .unwrap()
                .collate
        );

        let parsed = Cli::try_parse_from(["pluryprint", "print", "--files", "a.pdf", "--collate"])
            .expect("--collate parses");
        let Some(Command::Print(args)) = parsed.command else {
            panic!("expected print");
        };
        assert!(
            resolve_settings(&profile, None, &args.settings)
                .unwrap()
                .collate
        );

        assert!(
            Cli::try_parse_from([
                "pluryprint",
                "print",
                "--files",
                "a.pdf",
                "--collate",
                "--no-collate"
            ])
            .is_err(),
            "the two flags conflict"
        );
    }

    /// FR-008 / FR-041: `--paper` takes the same names as the GUI — the
    /// classic four, `auto`, and any size a printer reports — not only
    /// `a4|letter|legal|a3`.
    #[test]
    fn fr_041_paper_accepts_printer_sizes_and_auto() {
        use crate::cli::args::{Cli, Command};
        use clap::Parser;

        let paper_of = |value: &str| {
            let cli =
                Cli::try_parse_from(["pluryprint", "print", "--files", "a.pdf", "--paper", value])
                    .unwrap_or_else(|e| panic!("--paper {value}: {e}"));
            let Some(Command::Print(args)) = cli.command else {
                panic!("expected print");
            };
            resolve_settings(&quick(), None, &args.settings)
                .unwrap()
                .paper_size
        };
        assert_eq!(paper_of("LETTER"), PaperSize::LETTER);
        assert_eq!(paper_of("auto"), PaperSize::AUTO);
        assert_eq!(paper_of("4x6").as_str(), "4x6");
        assert_eq!(paper_of("A6").as_str(), "A6");

        // A printer that does not list the size refuses it before spooling.
        let caps = crate::engine::print::PrinterCaps {
            papers: vec!["A4".into(), "Letter".into()],
            ..crate::engine::print::PrinterCaps::unknown()
        };
        let settings = JobSettings {
            paper_size: paper_of("4x6"),
            ..JobSettings::default()
        };
        let err = crate::engine::print::check_supported(&settings, "Office", &caps).unwrap_err();
        assert_eq!(CliError::from(err).exit, ExitCode::Printer);

        // Blank is still refused by the parser.
        assert!(
            Cli::try_parse_from(["pluryprint", "print", "--files", "a.pdf", "--paper", " "])
                .is_err()
        );
    }

    #[test]
    fn fr_009_bad_settings_are_usage_errors() {
        let too_many = SettingFlags {
            copies: Some(0),
            ..Default::default()
        };
        assert_eq!(
            resolve_settings(&quick(), None, &too_many)
                .unwrap_err()
                .exit,
            ExitCode::Usage
        );

        let bad_range = SettingFlags {
            range: Some("5-1".to_string()),
            ..Default::default()
        };
        assert_eq!(
            resolve_settings(&quick(), None, &bad_range)
                .unwrap_err()
                .exit,
            ExitCode::Usage
        );
    }

    /// FR-009 / FR-101: a `config.toml` that exists but does not parse used to
    /// be replaced by the defaults without a word, so a printer the user had
    /// disabled could be printed to and routing rules were ignored. The run now
    /// stops with exit 3 before anything is spooled.
    #[test]
    fn fr_009_a_corrupt_config_stops_the_print_run_with_exit_3() {
        use clap::Parser;

        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            config: dir.path().join("config.toml"),
            profiles: dir.path().join("profiles.toml"),
            queue: dir.path().join("queue.json"),
        };
        std::fs::write(&paths.config, "schema_version = = broken\n").unwrap();
        let pdf = one_page_pdf(dir.path(), "doc.pdf");

        let cli = crate::cli::args::Cli::try_parse_from([
            "pluryprint",
            "print",
            "--printer",
            "Nowhere",
            "--files",
            pdf.to_str().unwrap(),
        ])
        .unwrap();
        let Some(crate::cli::args::Command::Print(args)) = cli.command else {
            panic!("expected print");
        };
        let err = run(&paths, Emitter::new(true), args).unwrap_err();
        assert_eq!(err.exit, ExitCode::Config);
        assert_eq!(err.error.reason(), Some("config_corrupt"));

        // A missing file is the ordinary first run, not an error.
        std::fs::remove_file(&paths.config).unwrap();
        assert!(load_config(&paths.config).is_ok());
    }

    /// FR-034 / NFR-012: the converter is bounded by the conversion budget and
    /// the spool by the print budget, as in the queue. The CLI used to bound
    /// both with the print budget.
    #[test]
    fn fr_034_cli_conversions_use_the_conversion_timeout() {
        use std::time::Duration;
        let engine = crate::settings::EngineCfg {
            conversion_timeout_seconds: 600,
            print_timeout_seconds: 300,
            ..Default::default()
        };
        let t = Timeouts::from_engine(&engine);
        assert_eq!(t.conversion, Some(Duration::from_secs(600)));
        assert_eq!(t.print, Some(Duration::from_secs(300)));

        // Disabling one budget leaves the other in force.
        let engine = crate::settings::EngineCfg {
            conversion_timeout_seconds: 60,
            print_timeout_seconds: 0,
            ..Default::default()
        };
        let t = Timeouts::from_engine(&engine);
        assert_eq!(t.conversion, Some(Duration::from_secs(60)));
        assert_eq!(t.print, None);
    }

    #[test]
    fn fr_009_job_error_keeps_the_ipc_code() {
        let err = job_error(&IpcError::UnsupportedFormat("a.docx".into()));
        assert_eq!(err.code, "UnsupportedFormat");
        assert!(err.message.contains("a.docx"));
        assert_eq!(err.reason, None);
    }

    /// FR-092: a failed file's row in `print --json` keeps the reason and
    /// params of its error, like the CLI's top-level JSON error does.
    #[test]
    fn fr_092_job_error_keeps_the_reason_and_params() {
        let engine = crate::engine::EngineError::ConversionFailed(crate::engine::Failure::new(
            "pdf_truncated",
            "not a valid PDF: truncated",
        ));
        let err = job_error(&IpcError::from(engine));
        assert_eq!(err.code, "ConversionFailed");
        assert_eq!(err.reason.as_deref(), Some("pdf_truncated"));

        let err = job_error(
            &IpcError::localized(
                crate::util::error::IpcErrorCode::UnsupportedFormat,
                "gif_not_supported",
                "/x/a.gif is not a printable file type",
            )
            .with_param("name", "a.gif"),
        );
        assert_eq!(err.reason.as_deref(), Some("gif_not_supported"));
        assert_eq!(err.params.unwrap()["name"], "a.gif");
    }

    /// A real 1-page PDF, written by the engine's own image converter so the
    /// page count the bounds check reads is the one production reads.
    fn one_page_pdf(dir: &Path, name: &str) -> std::path::PathBuf {
        use ::image::{ImageFormat, Rgb, RgbImage};
        let mut img = RgbImage::new(4, 4);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = Rgb([(x * 60) as u8, (y * 60) as u8, 10]);
        }
        let src = dir.join("src.png");
        img.save_with_format(&src, ImageFormat::Png).unwrap();
        let out = dir.join(name);
        crate::engine::convert::image::to_pdf(&src, &out, &|_p| {}).unwrap();
        out
    }

    #[test]
    fn fr_042_a_range_past_the_document_is_refused_before_spooling() {
        // The defect: the GUI refused "500-600" on a short PDF while the CLI
        // accepted it, spooled, and printed nothing. Both now go through
        // `ipc::jobs::check_ranges_fit_documents`.
        let dir = tempfile::tempdir().unwrap();
        let pdf = one_page_pdf(dir.path(), "report.pdf");
        let files = vec![pdf.to_string_lossy().into_owned()];

        let flags = SettingFlags {
            range: Some("500-600".to_string()),
            ..Default::default()
        };
        // The grammar is fine, so nothing catches this before the bounds check.
        let settings = resolve_settings(&quick(), None, &flags).unwrap();

        let err = check_ranges(&settings, &files).unwrap_err();
        assert_eq!(err.exit, ExitCode::Usage);
        let message = err.error.to_string();
        assert!(
            message.contains("report.pdf"),
            "the message names the file: {message}"
        );

        // A range that fits, and no range at all, both pass.
        let fits = SettingFlags {
            range: Some("1".to_string()),
            ..Default::default()
        };
        assert!(check_ranges(&resolve_settings(&quick(), None, &fits).unwrap(), &files).is_ok());
        assert!(check_ranges(
            &resolve_settings(&quick(), None, &SettingFlags::default()).unwrap(),
            &files
        )
        .is_ok());
    }

    /// FR-008: a job spooled into a paused or offline queue is reported as
    /// held, not done, without changing the exit code.
    #[test]
    fn fr_008_a_job_on_a_paused_queue_is_held_not_done() {
        let mut batch = BatchOutcomes::with_capacity(3);
        batch.record("/d/a.pdf", "Hold".to_string(), Ok(Some("7".to_string())));
        batch.record("/d/b.pdf", "Office".to_string(), Ok(Some("8".to_string())));
        batch.record(
            "/d/c.pdf",
            "Hold".to_string(),
            Err(CliError::from(IpcError::PrintFailed("x".into()))),
        );
        assert!(batch.has_done());
        let worst = batch.worst();
        batch.mark_held(&["Hold".to_string()].into_iter().collect());
        let states: Vec<_> = batch.into_files().into_iter().map(|f| f.state).collect();
        assert_eq!(states, ["held", "done", "failed"]);
        assert_eq!(worst.code(), ExitCode::PrintFailed.code());
    }

    fn printer(
        id: &str,
        status: crate::ipc::printers::PrinterStatus,
    ) -> crate::ipc::printers::Printer {
        use crate::ipc::printers::{Printer, PrinterKind, Supplies};
        Printer {
            id: id.into(),
            name: id.into(),
            kind: PrinterKind::Laser,
            color: false,
            duplex: true,
            color_unknown: false,
            duplex_unknown: false,
            status,
            is_default: false,
            location: String::new(),
            connection: String::new(),
            driver: String::new(),
            shared: false,
            queued_jobs: 0,
            max_dpi: None,
            papers: vec![],
            trays: vec![],
            supplies: Supplies::default(),
            default_tray: None,
            default_paper: None,
        }
    }

    /// FR-008: a printer stopped on an error (out of paper, a jam, a stopped
    /// CUPS queue) prints nothing until someone fixes it, like an offline one:
    /// the file is `held`, not `done`.
    #[test]
    fn fr_008_a_job_on_a_printer_in_error_is_held_not_done() {
        use crate::ipc::printers::PrinterStatus;
        let printers = vec![
            printer("Jammed", PrinterStatus::Error),
            printer("Away", PrinterStatus::Offline),
            printer("Fine", PrinterStatus::Ready),
            printer("Working", PrinterStatus::Busy),
        ];
        let stopped = not_printing(&printers);
        assert!(stopped.contains("Jammed"));
        assert!(stopped.contains("Away"));
        assert!(!stopped.contains("Fine"));
        assert!(!stopped.contains("Working"));

        let mut batch = BatchOutcomes::with_capacity(2);
        batch.record("/d/a.pdf", "Jammed".to_string(), Ok(None));
        batch.record("/d/b.pdf", "Fine".to_string(), Ok(None));
        batch.mark_held(&stopped);
        let states: Vec<_> = batch.into_files().into_iter().map(|f| f.state).collect();
        assert_eq!(states, ["held", "done"]);
    }

    /// FR-008: a job the spooler itself holds — a CUPS queue whose default
    /// `job-hold-until` parks every job (`pending-held`), a job paused in the
    /// Windows queue, or one stopped on a device problem — is `held` even
    /// though its queue reads as ready. A job whose state cannot be read is
    /// `unconfirmed`, never guessed; one without a spooler id keeps `done`
    /// (nothing to ask about); a failure stays failed and is never queried.
    #[test]
    fn fr_008_a_job_the_spooler_holds_is_held_not_done() {
        use crate::engine::print::{OsJobState, TrackError};
        let mut batch = BatchOutcomes::with_capacity(8);
        batch.record("/d/held.pdf", "Q".to_string(), Ok(Some("Q-1".into())));
        batch.record("/d/stopped.pdf", "Q".to_string(), Ok(Some("Q-2".into())));
        batch.record("/d/pending.pdf", "Q".to_string(), Ok(Some("Q-3".into())));
        batch.record("/d/gone.pdf", "Q".to_string(), Ok(Some("Q-4".into())));
        batch.record("/d/busy.pdf", "Q".to_string(), Ok(Some("Q-5".into())));
        batch.record("/d/notool.pdf", "Q".to_string(), Ok(Some("Q-6".into())));
        batch.record("/d/noid.pdf", "Q".to_string(), Ok(None));
        batch.record(
            "/d/failed.pdf",
            "Q".to_string(),
            Err(CliError::from(IpcError::PrintFailed("x".into()))),
        );
        let worst = batch.worst();
        let mut asked = Vec::new();
        batch.mark_held_jobs(|printer, id| {
            asked.push((printer.to_string(), id.to_string()));
            match id {
                "Q-1" => Ok(OsJobState::Held),
                "Q-2" => Ok(OsJobState::Stopped),
                "Q-3" => Ok(OsJobState::Pending),
                "Q-4" => Ok(OsJobState::Gone),
                "Q-5" => Err(TrackError::Transient("busy".into())),
                _ => Err(TrackError::Unavailable("no ipptool".into())),
            }
        });
        assert_eq!(asked.len(), 6, "only accepted files with an id: {asked:?}");
        assert_eq!(asked[0], ("Q".to_string(), "Q-1".to_string()));
        assert_eq!(batch.worst(), worst, "the exit code is unchanged");
        let states: Vec<_> = batch.into_files().into_iter().map(|f| f.state).collect();
        assert_eq!(
            states,
            [
                "held",
                "held",
                "done",
                "done",
                "unconfirmed",
                "unconfirmed",
                "done",
                "failed"
            ]
        );
    }

    /// FR-008: a file already `held` by its queue's state is not asked about
    /// again, and the job check never undoes it.
    #[test]
    fn fr_008_job_check_only_asks_about_files_still_done() {
        use crate::engine::print::OsJobState;
        let mut batch = BatchOutcomes::with_capacity(2);
        batch.record("/d/a.pdf", "Paused".to_string(), Ok(Some("P-1".into())));
        batch.record("/d/b.pdf", "Fine".to_string(), Ok(Some("F-1".into())));
        batch.mark_held(&["Paused".to_string()].into_iter().collect());
        let mut asked = Vec::new();
        batch.mark_held_jobs(|_, id| {
            asked.push(id.to_string());
            Ok(OsJobState::Pending)
        });
        assert_eq!(asked, ["F-1"]);
        let states: Vec<_> = batch.into_files().into_iter().map(|f| f.state).collect();
        assert_eq!(states, ["held", "done"]);
    }

    /// FR-008: when the printer list cannot be read, "done" would be a guess:
    /// the accepted files are `unconfirmed`, failures stay failed, and the
    /// exit code is unchanged.
    #[test]
    fn fr_008_an_unreadable_printer_list_makes_accepted_files_unconfirmed() {
        let mut batch = BatchOutcomes::with_capacity(2);
        batch.record("/d/a.pdf", "P".to_string(), Ok(Some("1".into())));
        batch.record(
            "/d/b.pdf",
            "P".to_string(),
            Err(CliError::from(IpcError::PrintFailed("x".into()))),
        );
        let worst = batch.worst();
        batch.mark_unconfirmed();
        assert_eq!(batch.worst(), worst);
        let states: Vec<_> = batch.into_files().into_iter().map(|f| f.state).collect();
        assert_eq!(states, ["unconfirmed", "failed"]);
    }

    /// NFR-082: the WARN record of a failed headless print names the file and
    /// the stable code, never the message, which can quote the absolute path
    /// or SumatraPDF's log of the staged copy.
    #[test]
    fn nfr_082_cli_job_failed_does_not_log_the_message_or_folder() {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::layer::SubscriberExt;

        #[derive(Clone, Default)]
        struct Grab(Arc<Mutex<Vec<String>>>);
        struct Fields<'a>(&'a mut Vec<String>);
        impl tracing::field::Visit for Fields<'_> {
            fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
                self.0.push(format!("{}={v:?}", f.name()));
            }
        }
        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Grab {
            fn on_event(
                &self,
                event: &tracing::Event<'_>,
                _: tracing_subscriber::layer::Context<'_, S>,
            ) {
                let mut out = Vec::new();
                event.record(&mut Fields(&mut out));
                self.0.lock().unwrap().push(out.join(" "));
            }
        }

        let grab = Grab::default();
        let subscriber = tracing_subscriber::registry().with(grab.clone());
        let secret = "/Users/ana/Confidencial/nómina.docx";
        tracing::subscriber::with_default(subscriber, || {
            let mut batch = BatchOutcomes::with_capacity(1);
            batch.record(
                secret,
                "P".to_string(),
                Err(CliError::from(IpcError::PrintFailed(format!(
                    "SumatraPDF exited with 1: cannot open {secret}"
                )))),
            );
        });
        let records = grab.0.lock().unwrap().clone();
        let rec = records
            .iter()
            .find(|r| r.contains("cli_job_failed"))
            .expect("the failure is logged");
        assert!(rec.contains("code=PrintFailed"), "{rec}");
        assert!(!rec.contains("Confidencial"), "no folder: {rec}");
        assert!(!rec.contains("SumatraPDF exited"), "no message: {rec}");
    }

    #[test]
    fn fr_009_a_batch_records_every_file_and_reports_the_worst_code() {
        let mut batch = BatchOutcomes::with_capacity(3);
        assert_eq!(batch.worst(), ExitCode::Success, "an empty batch succeeded");

        batch.record(
            "/tmp/a.pdf",
            "Front Desk".to_string(),
            Ok(Some("42".into())),
        );
        assert_eq!(batch.worst(), ExitCode::Success);

        batch.record(
            "/tmp/b.pdf",
            "Front Desk".to_string(),
            Err(CliError::from(IpcError::PrintFailed("spooler".into()))),
        );
        batch.record(
            "/tmp/c.pdf",
            "Front Desk".to_string(),
            Err(CliError::from(IpcError::FileNotFound("/tmp/c.pdf".into()))),
        );

        // PrintFailed (9) outranks FileNotFound (6): the worst code wins, not
        // the last one.
        assert_eq!(batch.worst(), ExitCode::PrintFailed);

        let files = batch.into_files();
        assert_eq!(files.len(), 3, "a failure never drops a row");
        assert_eq!(files[0].state, "done");
        assert_eq!(files[0].os_job_id.as_deref(), Some("42"));
        assert_eq!(files[1].state, "failed");
        assert_eq!(files[1].error.as_ref().unwrap().code, "PrintFailed");
        assert_eq!(files[2].error.as_ref().unwrap().code, "FileNotFound");
    }
}
