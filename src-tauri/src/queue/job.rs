//! Print-queue domain types and the job state machine.
//!
//! These are the wire types shared between the dispatcher, the on-disk
//! `queue.json` ([`crate::queue::persist`]) and the frontend (via
//! `specta`-generated bindings). The schema mirrors
//! `docs/04-architecture/data-model.md` §4 and the `QueueSnapshot` contract in
//! `docs/04-architecture/ipc-and-cli.md`.
//!
//! Everything here is pure data plus the [`can_transition`] guard: no I/O, no
//! process spawning. The dispatcher (a later stage) owns the actual driving of
//! jobs through the pipeline.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Print-quality preset. Re-exported from [`crate::profiles`] rather than
/// redeclared: a profile's `quality` reaches the engine unchanged, so one
/// vocabulary (`"draft"`, `"normal"`, `"high"`) covers the profile store, the
/// wire types and the backends.
pub use crate::profiles::Quality;

/// On-disk schema version for `queue.json`. Bumped only on a
/// forward-incompatible change; [`crate::queue::persist::load`] refuses to read
/// a file whose version is greater than this.
pub const SCHEMA_VERSION: u32 = 1;

/// Lifecycle state of a single print job.
///
/// The legal transitions between states are encoded in [`can_transition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    /// Created but not yet started.
    Queued,
    /// A converter process is running to produce the cache PDF.
    Converting,
    /// PDF is in the cache; waiting for a free printer.
    Converted,
    /// Handed off to the OS spooler.
    Printing,
    /// The printer reported completion (terminal).
    Done,
    /// Handed to the spooler, but nobody confirmed it printed (terminal): the
    /// job left the system queue without reporting completion, the spooler
    /// gave no id to follow, or it could not be read. Never shown as printed,
    /// and not retried automatically — paper may already have come out.
    /// Details in [`Job::error`].
    Unconfirmed,
    /// Terminal error; details in [`Job::error`] (terminal).
    Failed,
    /// Transient stop — user paused it, or it was interrupted by shutdown.
    Paused,
    /// User cancelled the job (terminal).
    Cancelled,
}

impl JobState {
    /// `true` for states from which no further work happens: [`Done`],
    /// [`Unconfirmed`], [`Failed`], [`Cancelled`].
    ///
    /// [`Done`]: JobState::Done
    /// [`Unconfirmed`]: JobState::Unconfirmed
    /// [`Failed`]: JobState::Failed
    /// [`Cancelled`]: JobState::Cancelled
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobState::Done | JobState::Unconfirmed | JobState::Failed | JobState::Cancelled
        )
    }

    /// `true` while a job is actively occupying an external resource — a
    /// converter process ([`Converting`]) or the spooler ([`Printing`]). These
    /// are exactly the states that shutdown recovery rewrites to [`Paused`].
    ///
    /// [`Converting`]: JobState::Converting
    /// [`Printing`]: JobState::Printing
    /// [`Paused`]: JobState::Paused
    pub fn is_active(self) -> bool {
        matches!(self, JobState::Converting | JobState::Printing)
    }

    /// The wire name of the state (same lowercase spelling `serde` emits), for
    /// the `status` field FR-035 requires on every job log record.
    pub fn as_str(self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Converting => "converting",
            JobState::Converted => "converted",
            JobState::Printing => "printing",
            JobState::Done => "done",
            JobState::Unconfirmed => "unconfirmed",
            JobState::Failed => "failed",
            JobState::Paused => "paused",
            JobState::Cancelled => "cancelled",
        }
    }
}

/// Duplex (double-sided) printing mode.
///
/// Note: the [`None`](Duplex::None) variant serializes to the string
/// `"none"`, matching the other lowercase enums here — it is unrelated to
/// [`Option::None`]. Refer to it by its full path (`Duplex::None`) in code to
/// avoid confusion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum Duplex {
    /// Single-sided.
    None,
    /// Double-sided, bound on the long edge.
    Long,
    /// Double-sided, bound on the short edge.
    Short,
}

/// Color vs. grayscale output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum ColorMode {
    /// Full color.
    Color,
    /// Grayscale / black-and-white.
    Monochrome,
}

/// Target paper size: any media name the printer reports — a CUPS `PageSize`
/// keyword (`A4`, `Letter`, `A6`, `4x6`, `Tabloid`, `w288h432`) or a Windows
/// paper name — or [`PaperSize::AUTO`], "whatever this printer is set to".
///
/// It used to be a closed four-value enum, so a printer's own sizes (photo
/// paper, envelopes, A5) could not be chosen at all. The wire form stays
/// backward compatible: the four classic sizes still serialize as `"a4"`,
/// `"letter"`, `"legal"`, `"a3"` and parse case-insensitively, so every stored
/// profile and `queue.json` loads unchanged. Any other name travels verbatim;
/// the engine matches it against the printer's list case-insensitively and
/// sends the printer's own spelling
/// ([`crate::engine::print::PrinterCaps::paper_keyword`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, specta::Type)]
#[specta(transparent)]
pub struct PaperSize(std::borrow::Cow<'static, str>);

impl PaperSize {
    /// ISO 216 A4 (210 × 297 mm).
    pub const A4: PaperSize = PaperSize(std::borrow::Cow::Borrowed("A4"));
    /// US Letter (8.5 × 11 in).
    pub const LETTER: PaperSize = PaperSize(std::borrow::Cow::Borrowed("Letter"));
    /// US Legal (8.5 × 14 in).
    pub const LEGAL: PaperSize = PaperSize(std::borrow::Cow::Borrowed("Legal"));
    /// ISO 216 A3 (297 × 420 mm).
    pub const A3: PaperSize = PaperSize(std::borrow::Cow::Borrowed("A3"));
    /// No explicit size: the printer's override (Printers screen) or, failing
    /// that, the printer's own default applies, and no media flag is sent.
    pub const AUTO: PaperSize = PaperSize(std::borrow::Cow::Borrowed("auto"));

    /// The sizes every build has always offered, in display order.
    pub const CLASSIC: [PaperSize; 4] = [Self::A4, Self::LETTER, Self::LEGAL, Self::A3];

    /// Parse a paper name. The classic four and `auto` are matched
    /// case-insensitively onto their canonical values; anything else is kept
    /// as written (trimmed). Blank names and control characters are refused.
    pub fn parse(name: &str) -> Result<PaperSize, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("paper size must not be empty".to_string());
        }
        if name.chars().any(char::is_control) {
            return Err(format!("'{name}' is not a paper size"));
        }
        if name.eq_ignore_ascii_case("auto") {
            return Ok(Self::AUTO);
        }
        if let Some(classic) = Self::CLASSIC
            .iter()
            .find(|c| c.as_str().eq_ignore_ascii_case(name))
        {
            return Ok(classic.clone());
        }
        Ok(PaperSize(std::borrow::Cow::Owned(name.to_string())))
    }

    /// The size's name (canonical case for the classic four).
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `true` for [`PaperSize::AUTO`].
    pub fn is_auto(&self) -> bool {
        *self == Self::AUTO
    }

    /// The wire spelling: lowercase for the classic four (the format every
    /// stored profile and `queue.json` already uses), verbatim otherwise.
    pub fn wire(&self) -> String {
        if Self::CLASSIC.contains(self) {
            self.0.to_ascii_lowercase()
        } else {
            self.0.to_string()
        }
    }
}

impl std::fmt::Display for PaperSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for PaperSize {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.wire())
    }
}

impl<'de> Deserialize<'de> for PaperSize {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        PaperSize::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// Page orientation. [`Auto`](Orientation::Auto) lets the engine pick per page
/// based on the source aspect ratio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    /// Pick per page from the source's aspect ratio.
    Auto,
    /// Force portrait.
    Portrait,
    /// Force landscape.
    Landscape,
}

/// How source pages are fitted to the target paper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum Scaling {
    /// Scale up or down to fill the printable area.
    Fit,
    /// Only shrink oversized pages; never enlarge.
    Shrink,
    /// Print at the source's actual size.
    None,
}

/// Per-job print settings. A resolved bundle: a profile plus any
/// per-invocation overrides collapse into one of these before a [`Job`] is
/// created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct JobSettings {
    /// Target printer id, or `None` to use the app/OS default at dispatch time.
    pub printer: Option<String>,
    /// Double-sided mode.
    pub duplex: Duplex,
    /// Colour or grayscale output.
    pub color_mode: ColorMode,
    /// Number of copies, `1..=999`.
    pub copies: u16,
    /// Paper size, or [`PaperSize::AUTO`] for the printer's own default.
    pub paper_size: PaperSize,
    /// Page orientation.
    pub orientation: Orientation,
    /// How pages are fitted to the paper.
    pub scaling: Scaling,
    /// Page-range expression (e.g. `"1-5,8"`), or `None` for all pages. The
    /// *syntax* is validated by the engine's parser, not here — see
    /// [`JobSettings::validate`].
    pub page_ranges: Option<String>,
    /// Print-quality preset. Reaches the spooler as CUPS's
    /// `print-quality`/`cupsPrintQuality`; on Windows, where SumatraPDF has no
    /// such switch, Draft and High go through a temporary per-user default
    /// ([`crate::engine::print::devmode_swap`]), and
    /// [`crate::engine::print::sumatra::quality_note`] says which will happen.
    ///
    /// `#[serde(default)]`: a `queue.json` written before this field existed
    /// must still load (FR-103), reading as [`Quality::Normal`].
    #[serde(default)]
    pub quality: Quality,
    /// Input tray / bin **as the driver names it** (`"Tray 2"`, `"Drawer 1"`),
    /// or `None` for whatever the printer defaults to. Bin names are
    /// model-specific, so this is a free string — it is passed through, never
    /// interpreted.
    ///
    /// `#[serde(default)]` for the same forward-migration reason as `quality`.
    #[serde(default)]
    pub tray: Option<String>,
    /// With several copies: `true` prints whole sets (1-2-3, 1-2-3), `false`
    /// groups each page (1-1, 2-2, 3-3). Irrelevant for one copy.
    /// `#[serde(default = …)]` reads an older `queue.json` as collated, the
    /// usual driver default.
    #[serde(default = "default_collate")]
    pub collate: bool,
}

/// Serde default for [`JobSettings::collate`] (and the profile field).
pub fn default_collate() -> bool {
    true
}

impl Default for JobSettings {
    fn default() -> Self {
        Self {
            printer: None,
            duplex: Duplex::None,
            color_mode: ColorMode::Color,
            copies: 1,
            paper_size: PaperSize::A4,
            orientation: Orientation::Auto,
            scaling: Scaling::Fit,
            page_ranges: None,
            quality: Quality::Normal,
            tray: None,
            collate: true,
        }
    }
}

impl JobSettings {
    /// Validate the fields this layer owns.
    ///
    /// Checks:
    /// - `copies` is in `1..=999`.
    /// - `page_ranges`, if present, is not the empty string.
    /// - `tray`, if present, is not blank — a blank bin name would reach the
    ///   driver as `InputSlot=` and silently select nothing.
    ///
    /// The page-range *grammar* is deliberately not validated here — the engine
    /// owns that parser. This only rejects an obviously empty range so the UI
    /// can surface an inline error early (FR-042).
    ///
    /// Each error carries its stable `InvalidRequest` reason (FR-092).
    pub fn validate(&self) -> Result<(), crate::util::error::LocalizedError> {
        use crate::util::error::{IpcErrorCode::InvalidRequest, LocalizedError};
        if !(1..=999).contains(&self.copies) {
            return Err(LocalizedError::new(
                InvalidRequest,
                "copies_out_of_range",
                format!("copies must be between 1 and 999, got {}", self.copies),
            ));
        }
        if let Some(ranges) = &self.page_ranges {
            if ranges.trim().is_empty() {
                return Err(LocalizedError::new(
                    InvalidRequest,
                    "page_range_empty",
                    "page range must not be empty",
                ));
            }
        }
        if let Some(tray) = &self.tray {
            if tray.trim().is_empty() {
                return Err(LocalizedError::new(
                    InvalidRequest,
                    "tray_empty",
                    "input tray must not be empty",
                ));
            }
        }
        Ok(())
    }

    /// The tray to ask the driver for: the trimmed bin name, or `None` when the
    /// job defers to the printer's own default. The backends go through this so
    /// a stray blank can never become an empty `InputSlot=`/`bin=` token.
    pub fn selected_tray(&self) -> Option<&str> {
        self.tray
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
    }
}

/// A request from the frontend/CLI to print one or more files with a shared
/// [`JobSettings`]. The dispatcher fans this out into one [`Job`] per file,
/// grouped under a single batch id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct PrintRequest {
    /// Absolute paths of the files to print.
    pub files: Vec<String>,
    /// The settings every file in the batch is printed with.
    pub settings: JobSettings,
    /// Name of the profile these settings were derived from, for the log/UI.
    pub profile_name: Option<String>,
    /// Submit every job of the batch already `Paused`, so nothing starts until
    /// the user resumes it from the Queue screen (FR-001's "add to the queue"
    /// entry). Scoped to this batch on purpose: the global pause flag would
    /// also stop every other batch that was waiting. Optional on the wire —
    /// absent means "start normally".
    #[serde(default)]
    pub hold: bool,
    /// Where the batch came from, when an integration labelled it (FR-124):
    /// every job of the batch carries it, and the Queue screen shows "From
    /// Pluryfact" on its rows. Optional and additive on the wire; absent for
    /// everything the user added by hand.
    #[serde(default)]
    pub origin: Option<JobOrigin>,
}

/// The integration a batch came from, as the queue shows it (FR-124).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct JobOrigin {
    /// Which integration (`pluryfact`, the only one labelled today).
    pub source: String,
    /// The integration's own label for the batch, when it sent one.
    #[serde(default)]
    pub label: Option<String>,
}

impl JobOrigin {
    /// Longest `source` kept, in characters.
    pub const MAX_SOURCE_CHARS: usize = 32;

    /// The origin as stored: `source` and `label` cut to their bounds
    /// ([`Self::MAX_SOURCE_CHARS`], [`crate::cli::MAX_SELECTION_LABEL_CHARS`])
    /// with control characters dropped, and an empty label as none. `None`
    /// when no source is left. The frontend sends what a validated handoff
    /// carried; this keeps a hand-written request from bloating `queue.json`.
    pub fn bounded(&self) -> Option<JobOrigin> {
        let clean = |s: &str, max: usize| -> String {
            s.chars()
                .filter(|c| !c.is_control())
                .take(max)
                .collect::<String>()
                .trim()
                .to_string()
        };
        let source = clean(&self.source, Self::MAX_SOURCE_CHARS);
        if source.is_empty() {
            return None;
        }
        let label = self
            .label
            .as_deref()
            .map(|l| clean(l, crate::cli::MAX_SELECTION_LABEL_CHARS))
            .filter(|l| !l.is_empty());
        Some(JobOrigin { source, label })
    }
}

/// A structured job error. `code` is a stable machine-readable identifier;
/// `message` is the English detail (logs, the CLI, and the UI's fallback).
///
/// FR-092: the UI never shows `message` when it can help it. It translates
/// `code` — or `code` + `reason`, when the code has variants — filling the
/// sentence with `params` (`queue:jobErrors`), exactly as it does for an
/// [`crate::util::error::IpcError`]. Both are optional on the wire, so a
/// `queue.json` written before they existed still loads.
///
/// The special code `"interrupted_by_shutdown"` is set by shutdown recovery
/// ([`crate::queue::persist::recover`]) on jobs that were mid-flight when the
/// app was last closed; the UI treats it as resumable rather than a hard
/// failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct JobError {
    /// Stable machine-readable failure family, e.g. `"conversion_failed"`.
    pub code: String,
    /// English detail for logs, the CLI and the UI's last-resort fallback.
    pub message: String,
    /// Stable variant within `code` (e.g. `"no_printer"`). Never renamed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Interpolation values for the translated sentence (`secs`, `printer`…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<BTreeMap<String, String>>,
}

impl JobError {
    /// Construct a [`JobError`] from any string-like code and message.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            reason: None,
            params: None,
        }
    }

    /// Tag the error with a stable variant of its `code`. The reason must be
    /// registered in [`crate::util::reasons::JOB_REASONS`] (checked in debug
    /// builds), so it is sure to have a sentence in every language.
    pub fn with_reason(mut self, reason: &str) -> Self {
        debug_assert!(
            crate::util::reasons::is_job_reason(&self.code, reason),
            "unregistered job reason {}.{reason}: add it to util::reasons and the catalogs",
            self.code
        );
        self.reason = Some(reason.to_string());
        self
    }

    /// Attach one interpolation value for the translated sentence.
    pub fn with_param(mut self, name: &str, value: impl ToString) -> Self {
        self.params
            .get_or_insert_with(BTreeMap::new)
            .insert(name.to_string(), value.to_string());
        self
    }
}

/// Coarse progress for the currently-running phase of a job. `phase` is a short
/// label such as `"converting"` or `"printing"`.
///
/// `percent` is in `0.0..=1.0`, or **`None` when the phase cannot report a
/// percentage** — render that as an indeterminate bar, never as 0 %. The
/// printing phase is exactly that case: the job has been handed to the OS
/// spooler, which reports no completion fraction back (SumatraPDF does not even
/// return a job id), so inventing a number would be a lie. Reading a v1
/// `queue.json` still works: a stored number deserializes into `Some`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct JobProgress {
    /// Short label of the step, e.g. `"converting"` or `"printing"`.
    pub phase: String,
    /// Fraction done in `0.0..=1.0`, or `None` when the phase cannot measure one.
    pub percent: Option<f32>,
}

impl JobProgress {
    /// Progress for a phase that reports a real fraction (conversion).
    pub fn measured(phase: impl Into<String>, percent: f32) -> Self {
        Self {
            phase: phase.into(),
            percent: Some(percent),
        }
    }

    /// Progress for a phase that runs for an unknown length with no fraction to
    /// report (printing) — the UI must show an indeterminate bar.
    pub fn indeterminate(phase: impl Into<String>) -> Self {
        Self {
            phase: phase.into(),
            percent: None,
        }
    }
}

/// One print job: a single input file tracked through the convert → print
/// pipeline. Persisted verbatim in `queue.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct Job {
    /// UUID v7 (time-sortable), assigned at creation.
    pub id: String,
    /// Groups all jobs submitted together in one [`PrintRequest`].
    pub batch_id: String,
    /// Where the job is in its lifecycle.
    pub state: JobState,
    /// Absolute path of the source file.
    pub input_path: String,
    /// Display file name (last path component of `input_path`).
    pub file_name: String,
    /// Lowercased extension without the leading dot (e.g. `"pdf"`), empty when
    /// the file has none.
    pub input_ext: String,
    /// Source size in bytes (filled by the dispatcher after `stat`).
    pub input_bytes: u64,
    /// Page count once known (after conversion / PDF parse).
    pub pages: Option<u32>,
    /// Absolute path of the converted PDF in the cache, once produced.
    pub cache_pdf: Option<String>,
    /// The resolved settings this job prints with.
    pub settings: JobSettings,
    /// Profile name these settings came from, for the log/UI.
    pub profile_name: Option<String>,
    /// Printer actually chosen at dispatch time (after resolving `<default>`).
    pub printer_resolved: Option<String>,
    /// The OS spooler's id for this job once it has been handed off (CUPS
    /// `Office-42`, a Windows spooler job number), while the dispatcher follows
    /// it to completion. `None` before spooling, or when the backend could not
    /// identify the job. `#[serde(default)]` so an older `queue.json` loads.
    #[serde(default)]
    pub os_job_id: Option<String>,
    /// RFC 3339 UTC timestamp of when this run was handed to the spooler. The
    /// print timeout counts from here, also for a print the app adopts again
    /// after a restart, so relaunching never grants a stuck print a fresh
    /// budget. `#[serde(default)]` so an older `queue.json` loads.
    #[serde(default)]
    pub spooled_at: Option<String>,
    /// Number of times this job has been started (for retry/backoff, FR-034).
    pub attempts: u32,
    /// Why the job last failed or paused, when it did.
    pub error: Option<JobError>,
    /// Live progress while converting or printing; `None` otherwise.
    pub progress: Option<JobProgress>,
    /// RFC 3339 UTC timestamp of submission.
    pub submitted_at: String,
    /// RFC 3339 UTC timestamp of first start, if started.
    pub started_at: Option<String>,
    /// RFC 3339 UTC timestamp of reaching a terminal state, if finished.
    pub finished_at: Option<String>,
    /// The integration the job's batch came from ([`PrintRequest::origin`]),
    /// shown on its Queue row. `#[serde(default)]` so an older `queue.json`
    /// loads.
    #[serde(default)]
    pub origin: Option<JobOrigin>,
}

impl Job {
    /// FR-059: whether this job is paused waiting for the password of its
    /// encrypted PDF (none given yet, or the one given was wrong). This is the
    /// state the GUI offers "Enter password" for (`job_supply_password`).
    pub fn needs_password(&self) -> bool {
        self.state == JobState::Paused
            && self.error.as_ref().is_some_and(|e| {
                e.code == "pdf_protected"
                    && matches!(
                        e.reason.as_deref(),
                        Some("password_required" | "password_incorrect")
                    )
            })
    }

    /// Create a fresh [`JobState::Queued`] job for `input_path`.
    ///
    /// `file_name` and `input_ext` are derived from the path; the `id` is a new
    /// UUID v7. `input_bytes` starts at `0` — the caller fills it after
    /// `stat`ing the file. `submitted_at` is passed in (use [`now_rfc3339`]) so
    /// every job in a batch can share one timestamp.
    pub fn new(
        batch_id: impl Into<String>,
        input_path: impl Into<String>,
        settings: JobSettings,
        profile_name: Option<String>,
        submitted_at: String,
    ) -> Job {
        let input_path = input_path.into();
        let path = std::path::Path::new(&input_path);
        let file_name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| input_path.clone());
        let input_ext = path
            .extension()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();

        Job {
            id: uuid::Uuid::now_v7().to_string(),
            batch_id: batch_id.into(),
            state: JobState::Queued,
            input_path,
            file_name,
            input_ext,
            input_bytes: 0,
            pages: None,
            cache_pdf: None,
            settings,
            profile_name,
            printer_resolved: None,
            os_job_id: None,
            spooled_at: None,
            attempts: 0,
            error: None,
            progress: None,
            submitted_at,
            started_at: None,
            finished_at: None,
            origin: None,
        }
    }
}

/// A change to a paused job, so it can be sent again without cancelling it
/// and printing the file anew: what a job paused for an unsupported setting
/// or an unusable printer needs ("Print one-sided", "Send to another
/// printer"). Every field is optional; `None` keeps what the job has.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct JobChange {
    /// Print to this printer (a queue id or display name) instead. An empty
    /// string drops the pin, so the job follows the routing rules and the
    /// default printer again.
    #[serde(default)]
    pub printer: Option<String>,
    /// Print with these sides instead (`none` is one-sided).
    #[serde(default)]
    pub duplex: Option<Duplex>,
}

/// The result of submitting a [`PrintRequest`]: the batch id and the ids of the
/// jobs it produced (one per input file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct JobBatch {
    /// Id shared by every job of the batch.
    pub batch_id: String,
    /// One job id per input file, in input order.
    pub job_ids: Vec<String>,
}

/// Serializable view of the whole queue, returned by the `queue_status` IPC
/// command and pushed via the `queue-updated` event. Also the on-disk shape of
/// `queue.json` (reused by [`crate::queue::persist`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct QueueSnapshot {
    /// Format version of `queue.json` (see [`SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Whether the whole queue is paused (FR-033).
    pub queue_paused: bool,
    /// Every job the queue knows about, in queue order.
    pub jobs: Vec<Job>,
}

impl Default for QueueSnapshot {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            queue_paused: false,
            jobs: Vec::new(),
        }
    }
}

/// Current UTC time as an RFC 3339 string, the format used for every timestamp
/// in `queue.json`.
///
/// Falls back to the Unix epoch string (rather than panicking) in the
/// impossible case that formatting fails.
pub fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// Pure guard for the job state machine: `true` iff a job may move directly
/// from `from` to `to`.
///
/// The legal transitions (from `docs/04-architecture/data-model.md` §4):
///
/// - `Queued`     → `Converting` (start) | `Cancelled` | `Paused`
/// - `Converting` → `Converted` (ok) | `Failed` | `Cancelled` | `Paused` |
///   `Queued` (retry resets to the start of the pipeline)
/// - `Converted`  → `Printing` (start) | `Cancelled` | `Paused` | `Converted`
///   (a retry that keeps the cached PDF stays in place)
/// - `Printing`   → `Done` | `Failed` | `Cancelled` | `Paused` | `Converted`
///   (a transient print failure returns to `Converted`, keeping the cached PDF
///   for the delayed retry)
/// - `Paused`     → `Queued` | `Converted` | `Cancelled` — resume returns the
///   job to whichever pre-pause stage the dispatcher decides is correct
///   (re-convert vs. re-print); this guard permits both targets.
/// - `Failed`     → `Queued` (retry). This is the only transition out of a
///   terminal state.
/// - `Done` / `Cancelled` are terminal with no successors.
///
/// The guard is intentionally permissive where the docs are silent; every
/// allowance above is deliberate.
pub fn can_transition(from: JobState, to: JobState) -> bool {
    use JobState::*;
    matches!(
        (from, to),
        (Queued, Converting)
            | (Queued, Cancelled)
            | (Queued, Paused)
            | (Converting, Converted)
            | (Converting, Failed)
            | (Converting, Cancelled)
            | (Converting, Paused)
            | (Converting, Queued)
            | (Converted, Printing)
            | (Converted, Cancelled)
            | (Converted, Paused)
            | (Converted, Converted)
            | (Printing, Done)
            | (Printing, Unconfirmed)
            | (Printing, Failed)
            | (Printing, Cancelled)
            | (Printing, Paused)
            | (Printing, Converted)
            | (Paused, Queued)
            | (Paused, Converted)
            | (Paused, Cancelled)
            | (Failed, Queued)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_and_active_predicates() {
        assert!(JobState::Done.is_terminal());
        assert!(JobState::Unconfirmed.is_terminal());
        assert!(JobState::Failed.is_terminal());
        assert!(JobState::Cancelled.is_terminal());
        assert!(!JobState::Queued.is_terminal());
        assert!(!JobState::Paused.is_terminal());

        assert!(JobState::Converting.is_active());
        assert!(JobState::Printing.is_active());
        assert!(!JobState::Queued.is_active());
        assert!(!JobState::Converted.is_active());
        assert!(!JobState::Done.is_active());
    }

    /// Stored profiles and queues name the classic sizes in lowercase; they
    /// must keep loading, and any size a printer reports must round-trip.
    #[test]
    fn paper_size_is_open_but_backward_compatible() {
        for (wire, canon) in [
            ("a4", "A4"),
            ("letter", "Letter"),
            ("legal", "Legal"),
            ("a3", "A3"),
        ] {
            let p: PaperSize = serde_json::from_value(serde_json::json!(wire)).unwrap();
            assert_eq!(p.as_str(), canon);
            assert_eq!(serde_json::to_value(&p).unwrap(), wire, "same wire form");
        }
        // Case-insensitive for the classic four, so a hand-edit still matches.
        assert_eq!(PaperSize::parse("LETTER").unwrap(), PaperSize::LETTER);

        for name in ["A6", "4x6", "Tabloid", "w288h432", "Env10", "Letter Small"] {
            let p = PaperSize::parse(name).unwrap();
            assert_eq!(p.as_str(), name);
            let v = serde_json::to_value(&p).unwrap();
            assert_eq!(v, name, "kept verbatim");
            assert_eq!(serde_json::from_value::<PaperSize>(v).unwrap(), p);
        }

        assert!(PaperSize::parse("Auto").unwrap().is_auto());
        assert!(PaperSize::parse("   ").is_err());
        assert!(serde_json::from_value::<PaperSize>(serde_json::json!("")).is_err());
        assert!(PaperSize::parse("A4\nrm").is_err());
    }

    #[test]
    fn default_settings_are_sane() {
        let s = JobSettings::default();
        assert_eq!(s.duplex, Duplex::None);
        assert_eq!(s.color_mode, ColorMode::Color);
        assert_eq!(s.copies, 1);
        assert_eq!(s.paper_size, PaperSize::A4);
        assert_eq!(s.orientation, Orientation::Auto);
        assert_eq!(s.scaling, Scaling::Fit);
        assert!(s.page_ranges.is_none());
        assert!(s.printer.is_none());
        assert_eq!(s.quality, Quality::Normal);
        assert!(s.tray.is_none(), "no tray means the printer's own default");
        s.validate().expect("defaults must validate");
    }

    #[test]
    fn fr_041_quality_and_tray_travel_with_the_job() {
        // The profile editor offers Quality and Tray, so the engine-facing
        // bundle has to be able to carry them at all — the regression this
        // guards is them being editable, persisted, and then dropped here.
        let s = JobSettings {
            quality: Quality::High,
            tray: Some("Tray 2".to_string()),
            ..JobSettings::default()
        };
        s.validate().expect("a named tray is valid");
        assert_eq!(s.selected_tray(), Some("Tray 2"));

        let v = serde_json::to_value(&s).expect("settings serialize");
        assert_eq!(v["quality"], "high");
        assert_eq!(v["tray"], "Tray 2");
    }

    #[test]
    fn selected_tray_ignores_blank_names() {
        let with_tray = |t: Option<&str>| JobSettings {
            tray: t.map(String::from),
            ..Default::default()
        };
        assert_eq!(
            with_tray(Some("  Drawer 1 ")).selected_tray(),
            Some("Drawer 1")
        );
        assert_eq!(with_tray(None).selected_tray(), None);
        // A blank name is rejected outright rather than reaching the driver.
        assert!(with_tray(Some("")).validate().is_err());
        assert!(with_tray(Some("   ")).validate().is_err());
    }

    #[test]
    fn validate_copies_bounds() {
        let with_copies = |copies| JobSettings {
            copies,
            ..Default::default()
        };
        assert!(with_copies(0).validate().is_err());
        assert!(with_copies(1000).validate().is_err());
        assert!(with_copies(1).validate().is_ok());
        assert!(with_copies(999).validate().is_ok());
    }

    #[test]
    fn validate_rejects_empty_page_ranges() {
        let with_ranges = |r: Option<&str>| JobSettings {
            page_ranges: r.map(String::from),
            ..Default::default()
        };
        assert!(with_ranges(Some("")).validate().is_err());
        assert!(with_ranges(Some("   ")).validate().is_err());
        // Syntax is NOT checked here: any non-empty string passes this layer.
        assert!(with_ranges(Some("not-a-real-range")).validate().is_ok());
        assert!(with_ranges(Some("1-5,8")).validate().is_ok());
    }

    #[test]
    fn new_job_derives_name_and_ext() {
        let job = Job::new(
            "batch-1",
            "/home/x/report.PDF",
            JobSettings::default(),
            Some("Quick BW".to_string()),
            now_rfc3339(),
        );
        assert_eq!(job.file_name, "report.PDF");
        assert_eq!(job.input_ext, "pdf");
        assert_eq!(job.state, JobState::Queued);
        assert_eq!(job.batch_id, "batch-1");
        assert_eq!(job.attempts, 0);
        assert_eq!(job.input_bytes, 0);
        assert!(job.error.is_none());
        assert!(!job.id.is_empty());
        assert_eq!(job.profile_name.as_deref(), Some("Quick BW"));
    }

    #[test]
    fn new_job_handles_extensionless_path() {
        let job = Job::new(
            "b",
            "/home/x/Makefile",
            JobSettings::default(),
            None,
            now_rfc3339(),
        );
        assert_eq!(job.file_name, "Makefile");
        assert_eq!(job.input_ext, "");
    }

    #[test]
    fn can_transition_legal_moves() {
        use JobState::*;
        assert!(can_transition(Queued, Converting));
        assert!(can_transition(Converting, Converted));
        assert!(can_transition(Converting, Queued)); // retry
        assert!(can_transition(Converted, Printing));
        assert!(can_transition(Printing, Done));
        assert!(can_transition(Printing, Unconfirmed));
        assert!(can_transition(Printing, Paused));
        assert!(can_transition(Paused, Converted)); // resume
        assert!(can_transition(Paused, Queued)); // resume
        assert!(can_transition(Failed, Queued)); // retry
    }

    #[test]
    fn can_transition_illegal_moves() {
        use JobState::*;
        assert!(!can_transition(Queued, Done));
        assert!(!can_transition(Queued, Printing));
        assert!(!can_transition(Done, Queued)); // done is terminal
        assert!(!can_transition(Unconfirmed, Queued)); // never re-sent by itself
        assert!(!can_transition(Queued, Unconfirmed));
        assert!(!can_transition(Cancelled, Queued)); // cancelled is terminal
        assert!(!can_transition(Done, Printing));
        assert!(!can_transition(Converting, Printing)); // must pass through converted
        assert!(!can_transition(Failed, Printing)); // retry only goes to queued
    }

    #[test]
    fn serialized_job_uses_lowercase_wire_format() {
        let job = Job::new(
            "batch-1",
            "/tmp/a.pdf",
            JobSettings::default(),
            None,
            "2026-07-11T10:00:00Z".to_string(),
        );
        let v = serde_json::to_value(&job).expect("job serializes");
        assert_eq!(v["state"], "queued");
        assert_eq!(v["settings"]["duplex"], "none");
        assert_eq!(v["settings"]["color_mode"], "color");
        assert_eq!(v["settings"]["paper_size"], "a4");
        assert_eq!(v["settings"]["orientation"], "auto");
        assert_eq!(v["settings"]["scaling"], "fit");
        assert_eq!(v["settings"]["quality"], "normal");
        assert!(v["settings"]["tray"].is_null());
        assert_eq!(v["input_ext"], "pdf");
        assert_eq!(v["submitted_at"], "2026-07-11T10:00:00Z");
        // Optionals present as JSON null, not omitted.
        assert!(v["error"].is_null());
        assert!(v["cache_pdf"].is_null());
    }

    #[test]
    fn fr_103_v1_queue_json_still_loads() {
        // Forward migration (FR-103): a `queue.json` written by the build that
        // predates the indeterminate-progress change stores `percent` as a bare
        // number, and its `settings` block has no `quality`/`tray` at all. It
        // must still deserialize — `percent` into `Some(_)`, the new fields into
        // their defaults — and keep every other field, so an upgrade never
        // discards a live queue.
        const V1: &str = r#"{
          "schema_version": 1,
          "queue_paused": false,
          "jobs": [
            {
              "id": "job-1",
              "batch_id": "batch-1",
              "state": "converting",
              "input_path": "/home/x/report.pdf",
              "file_name": "report.pdf",
              "input_ext": "pdf",
              "input_bytes": 1024,
              "pages": 3,
              "cache_pdf": null,
              "settings": {
                "printer": "Office",
                "duplex": "long",
                "color_mode": "color",
                "copies": 1,
                "paper_size": "a4",
                "orientation": "auto",
                "scaling": "fit",
                "page_ranges": null
              },
              "profile_name": "Photo Color",
              "printer_resolved": null,
              "attempts": 0,
              "error": null,
              "progress": { "phase": "converting", "percent": 0.5 },
              "submitted_at": "2026-07-11T10:42:00Z",
              "started_at": "2026-07-11T10:42:01Z",
              "finished_at": null
            }
          ]
        }"#;

        let snap: QueueSnapshot = serde_json::from_str(V1).expect("v1 queue.json still parses");
        assert_eq!(snap.schema_version, SCHEMA_VERSION);
        let job = &snap.jobs[0];
        assert_eq!(job.state, JobState::Converting);
        assert_eq!(job.pages, Some(3));
        assert_eq!(job.profile_name.as_deref(), Some("Photo Color"));
        assert_eq!(job.settings.quality, Quality::Normal);
        assert_eq!(job.settings.tray, None);
        assert_eq!(job.origin, None);
        let progress = job.progress.as_ref().expect("progress survives the read");
        assert_eq!(progress.phase, "converting");
        assert_eq!(progress.percent, Some(0.5));
    }

    /// FR-124: an origin is cut to its bounds, loses control characters, and
    /// is dropped when no source is left; a v1 job without one still loads
    /// (see `v1_queue_json_still_parses`).
    #[test]
    fn fr_124_origin_is_bounded() {
        let o = JobOrigin {
            source: format!(" plury\u{7}fact{} ", "s".repeat(100)),
            label: Some("  ".into()),
        }
        .bounded()
        .unwrap();
        assert!(o.source.starts_with("pluryfact"));
        assert_eq!(o.source.chars().count(), JobOrigin::MAX_SOURCE_CHARS - 1);
        assert_eq!(o.label, None);
        assert_eq!(
            JobOrigin {
                source: "\n".into(),
                label: Some("x".into())
            }
            .bounded(),
            None
        );
        let back: JobOrigin = serde_json::from_str(r#"{"source":"pluryfact"}"#).unwrap();
        assert_eq!(back.label, None);
    }

    #[test]
    fn indeterminate_progress_is_null_not_zero() {
        // A printing job must not claim 0 %: the spooler reports no fraction, so
        // the wire value is null and the UI renders an indeterminate bar.
        let p = JobProgress::indeterminate("printing");
        assert!(p.percent.is_none());
        let v = serde_json::to_value(&p).unwrap();
        assert!(v["percent"].is_null());
        assert_eq!(
            serde_json::to_value(JobProgress::measured("converting", 0.25)).unwrap()["percent"],
            0.25
        );
    }

    #[test]
    fn snapshot_default_is_empty_and_current() {
        let snap = QueueSnapshot::default();
        assert_eq!(snap.schema_version, SCHEMA_VERSION);
        assert!(!snap.queue_paused);
        assert!(snap.jobs.is_empty());
    }
}
