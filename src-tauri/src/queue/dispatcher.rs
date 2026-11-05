//! The single-writer print-queue dispatcher.
//!
//! A [`Dispatcher`] owns the in-memory [`QueueState`] and is the *only* thing
//! that drives jobs through the `Queued → Converting → Converted → Printing →
//! Done` pipeline. All mutations flow through a single [`tokio`] task consuming a
//! [`Cmd`] channel, so there is no lock contention on the state machine itself:
//! the `RwLock` around [`QueueState`] exists only so read-only snapshots
//! ([`QueueHandle::snapshot`](crate::queue::QueueHandle::snapshot)) and the
//! in-flight progress callback can peek without a channel round-trip. The task
//! never holds the write guard across an `.await`.
//!
//! ## Testability
//!
//! The three side-effecting concerns are behind traits — [`JobRunner`] (the
//! engine), [`EventSink`] (Tauri event emission) and [`BlockingSpawner`] (how
//! blocking convert/print work is offloaded). Production wires the real
//! [`EngineRunner`] / [`TauriEmit`] / [`TokioBlockingSpawner`]; tests substitute
//! deterministic fakes and step the loop one [`Cmd`] at a time.
//!
//! ## Concurrency & retry decisions (documented behaviour)
//!
//! - Up to [`DispatcherLimits::max_concurrent`] conversions run at once
//!   (config-driven, defaulting to [`DEFAULT_MAX_CONCURRENT_CONVERSIONS`]);
//!   exactly one print runs at a time (spoolers serialise anyway, and it keeps
//!   job ordering legible). A conversion that outruns
//!   [`DispatcherLimits::conversion_timeout`] is failed with a
//!   `"conversion_timeout"` error (no retry). The print stage is bounded the
//!   same way by [`DispatcherLimits::print_timeout`]: the spooler child is
//!   killed and the job failed with `"print_timeout"` (also terminal), so a hung
//!   `lp`/SumatraPDF can never hold the single print slot forever.
//! - A printer that the OS no longer enumerates — or no configured printer at
//!   all — **pauses** the job with `"printer_unavailable"` instead of failing
//!   it, so the user can pick a replacement
//!   (`docs/03-requirements/print-settings.md` §printer).
//! - `queue_paused` gates *starting* new work only. Work already in flight runs
//!   to completion; a converting job that finishes while the queue is paused
//!   still reaches `Converted`, it just won't advance to `Printing`.
//! - Retry (FR-034): [`EngineError::UnsupportedSetting`] and
//!   [`EngineError::DependencyMissing`] are user-fixable, so the job is
//!   **Paused** with the error and never auto-retried — the user fixes the
//!   setting/driver and resumes. Any other error bumps `attempts`; the job gets
//!   the initial attempt plus three automatic retries (backoffs 1s/5s/25s); once
//!   `attempts >= MAX_ATTEMPTS` the job is **Failed**, otherwise it returns to
//!   the start of its stage (`Queued` for a conversion failure, `Converted` for
//!   a print failure — the cached PDF is kept) and a delayed `RetryDue` is
//!   scheduled after a [`RETRY_BACKOFFS`] backoff. Delayed jobs are held in a
//!   `delayed` set that the scheduler skips until the timer fires.
//! - Pause of a `Converting` job sets its cancel flag and moves it to `Paused`;
//!   the in-flight conversion aborts and its partial cache is discarded, so the
//!   job re-converts on resume. Pausing a `Printing` job is rejected
//!   (`InvalidState`) — the OS already owns it.
//! - Resume of a `Paused` job returns it to `Converted` when its cached PDF is
//!   still on disk, otherwise to `Queued`. The error is always cleared; a job
//!   paused by shutdown recovery (`interrupted_by_shutdown`) also has its
//!   `attempts` reset so it gets a clean run.
//! - Resume of a job paused with `printer_unavailable` **re-resolves the
//!   printer first** (`plan_printer_fix`, on the blocking pool — it reads the OS
//!   printer list). If the pinned printer is back, the
//!   job resumes onto it; if it is still gone but preferences now point
//!   somewhere usable — which is what the Printers screen's "Update references"
//!   reconciliation edits — the pin is dropped so dispatch follows them; and if
//!   nothing resolves, the resume is **refused** with what to fix instead of
//!   silently re-pausing. Without this a job pinned to a printer that
//!   disappeared was stuck forever: resuming re-ran the same failed lookup and
//!   nothing could re-point it.
//! - **A print is done when the printer is done, not when the spooler accepts
//!   it.** Once `spool` returns an OS job id the job stays `Printing` and the
//!   dispatcher polls the spooler through [`JobTracker`] with backoff
//!   ([`TRACK_BACKOFFS`], then every [`TRACK_POLL_CAP`]). `completed` (or the
//!   job leaving the queue) → `Done`; `canceled` → `Cancelled`; `aborted` →
//!   `Failed`; a held job stays `Printing` with the `"held"` progress phase and
//!   a stopped printer with `"stopped"`. The whole print — spooling plus
//!   tracking — is bounded by [`DispatcherLimits::print_timeout`]; running out
//!   fails the job with `print_timeout` and leaves the OS job alone (cancelling
//!   a job the printer may still print would lose it). A backend that cannot
//!   report job state at all, or a job id it could not recover, falls back to
//!   `Done` on hand-off, as before tracking existed.
//! - **One print slot.** It is taken when a job starts spooling and released
//!   only when its OS job leaves the queue (or the print fails), so the next
//!   job never reaches the spooler while the previous one is still printing.
//! - Cancel of a `Printing` job marks it `Cancelled` at once. If its OS job id
//!   is already known, the OS job is cancelled and the slot freed there and
//!   then; if it is still spooling, the slot stays taken until `PrintDone`
//!   arrives, which then cancels the OS job and deletes the cache — so a
//!   cancelled job can never share the printer with the next one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};

use crate::engine::print::{OsJobState, PrintError, PrinterCaps, SpoolOutcome, TrackError};
use crate::engine::{EngineError, PdfPassword};
use crate::queue::job::{
    now_rfc3339, Job, JobChange, JobError, JobProgress, JobState, QueueSnapshot, SCHEMA_VERSION,
};
use crate::queue::QueueState;
use crate::util::error::{IpcError, IpcErrorCode};

/// Default cap on conversions running concurrently, used when the config does
/// not override it (and as the value the `EngineCfg` default mirrors).
/// Conversions are CPU/IO heavy (rasterising images, parsing PDFs) so we cap the
/// pool rather than fan out. The effective cap comes from
/// [`DispatcherLimits::max_concurrent`] (config value clamped to `1..=8`).
pub(crate) const DEFAULT_MAX_CONCURRENT_CONVERSIONS: usize = 2;

/// Engine limits handed to the [`Dispatcher`]. Read from
/// [`crate::settings::EngineCfg`] when the queue spawns, and replaced at run
/// time by [`Cmd::SetLimits`] whenever the user saves new engine settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DispatcherLimits {
    /// Concurrent conversions allowed (already clamped to `1..=8`).
    pub max_concurrent: usize,
    /// Per-conversion wall-clock timeout; `None` disables it.
    pub conversion_timeout: Option<Duration>,
    /// Wall-clock budget for one print — spooling plus waiting for the OS job
    /// to leave the queue
    /// ([`crate::settings::EngineCfg::print_timeout_seconds`]); `None`
    /// disables it. Its own knob rather than the conversion budget: a print
    /// waits on a physical device, which has nothing to do with how long a
    /// rasterisation may take.
    pub print_timeout: Option<Duration>,
}

impl Default for DispatcherLimits {
    fn default() -> Self {
        Self {
            max_concurrent: DEFAULT_MAX_CONCURRENT_CONVERSIONS,
            conversion_timeout: None,
            print_timeout: None,
        }
    }
}

impl DispatcherLimits {
    /// The limits a (sanitized) engine config asks for. `0` disables a timeout.
    pub(crate) fn from_engine(engine: &crate::settings::EngineCfg) -> Self {
        let secs = |s: u32| (s > 0).then(|| Duration::from_secs(u64::from(s)));
        Self {
            max_concurrent: engine.max_concurrent_conversions.max(1) as usize,
            conversion_timeout: secs(engine.conversion_timeout_seconds),
            print_timeout: secs(engine.print_timeout_seconds),
        }
    }
}

/// Failure count after which a transiently-failing job is marked `Failed`.
/// FR-034 grants the initial attempt plus three automatic retries, so a job
/// fails permanently on its fourth failure.
pub(crate) const MAX_ATTEMPTS: u32 = 4;

/// Backoff before each delayed retry, indexed by `attempts - 1`: all three
/// entries are used before the job fails on its [`MAX_ATTEMPTS`]th failure.
pub(crate) const RETRY_BACKOFFS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(25),
];

/// Delay before each of the first OS-job polls after a successful spool. A job
/// usually finishes within seconds, so polling starts fast and backs off.
pub(crate) const TRACK_BACKOFFS: [Duration; 4] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
];

/// Poll interval once [`TRACK_BACKOFFS`] is exhausted.
pub(crate) const TRACK_POLL_CAP: Duration = Duration::from_secs(10);

/// Consecutive failed polls after which tracking gives up and the job is
/// treated as finished on hand-off (the spooler did accept it).
pub(crate) const MAX_TRACK_FAILURES: u32 = 5;

/// The delay before poll number `polls` (0-based).
fn track_delay(polls: u32) -> Duration {
    TRACK_BACKOFFS
        .get(polls as usize)
        .copied()
        .unwrap_or(TRACK_POLL_CAP)
}

/// Minimum gap between progress emissions for a single job.
const PROGRESS_THROTTLE: Duration = Duration::from_millis(100);

// ---------------------------------------------------------------------------
// Commands & wire types
// ---------------------------------------------------------------------------

/// The printer a job will go to, with what is known about it and the
/// per-printer defaults the user set for it on the Printers screen.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedPrinter {
    pub name: String,
    pub caps: PrinterCaps,
    pub overrides: crate::settings::PrinterOverride,
}

/// Successful print outcome flowing back from a blocking print task. Carries
/// the resolved printer name (so the job records which printer it actually went
/// to) alongside the spooler's own id.
pub(crate) struct PrintOk {
    pub printer: String,
    pub os_job_id: Option<String>,
}

/// Every message the dispatcher processes. User-facing commands carry a
/// [`oneshot`] reply so the async [`QueueHandle`](crate::queue::QueueHandle)
/// method can surface a per-job error; internal completions
/// (`ConversionDone`/`PrintDone`/`RetryDue`) are fire-and-forget.
pub(crate) enum Cmd {
    Submit {
        jobs: Vec<Job>,
    },
    Cancel {
        id: String,
        reply: oneshot::Sender<Result<(), IpcError>>,
    },
    CancelAll {
        reply: oneshot::Sender<u32>,
    },
    Pause {
        id: String,
        reply: oneshot::Sender<Result<(), IpcError>>,
    },
    Resume {
        id: String,
        reply: oneshot::Sender<Result<(), IpcError>>,
    },
    /// The off-loop printer re-resolution for a [`Cmd::Resume`] finished.
    ResumePlanned {
        id: String,
        fix: Result<PrinterFix, IpcError>,
        reply: oneshot::Sender<Result<(), IpcError>>,
    },
    Retry {
        id: String,
        reply: oneshot::Sender<Result<(), IpcError>>,
    },
    /// Change a paused job's printer or sides before resuming it
    /// ([`Dispatcher::on_adjust`]).
    Adjust {
        id: String,
        change: JobChange,
        reply: oneshot::Sender<Result<(), IpcError>>,
    },
    /// FR-059: the user typed the password of a job's encrypted PDF. With
    /// `apply_to_batch`, it is also tried on the batch's other protected PDFs.
    SupplyPassword {
        id: String,
        password: PdfPassword,
        apply_to_batch: bool,
        reply: oneshot::Sender<Result<(), IpcError>>,
    },
    PauseQueue,
    ResumeQueue,
    /// Replace the engine limits at run time — sent by `set_app_settings` so a
    /// changed concurrency cap or timeout applies without a restart.
    SetLimits {
        limits: DispatcherLimits,
    },
    ClearDone {
        reply: oneshot::Sender<u32>,
    },
    ConversionDone {
        id: String,
        result: Result<Option<PathBuf>, EngineError>,
        /// Page count of the converted PDF, measured off the actor (on the
        /// blocking pool) so a document whose pages were unknown before
        /// conversion (DOCX, HTML, Markdown, …) shows them afterwards.
        pages: Option<u32>,
    },
    PrintDone {
        id: String,
        result: Result<PrintOk, PrintError>,
    },
    /// A retry backoff elapsed. `generation` is the token the backoff was
    /// armed with; a timer from an earlier backoff (the job was resumed and
    /// failed again since) carries an older one and is ignored, so it cannot
    /// cut the current backoff short.
    RetryDue {
        id: String,
        generation: u64,
    },
    /// Time to ask the spooler where the tracked print `id` stands.
    TrackDue {
        id: String,
    },
    /// What the spooler said about the tracked print `id`.
    TrackResult {
        id: String,
        result: Result<OsJobState, TrackError>,
    },
    /// A per-attempt conversion timer elapsed. `flag` is the exact cancel-flag
    /// [`Arc`] the timer captured, so a stale timer (from a prior attempt whose
    /// flag was since replaced) can be distinguished by pointer identity and
    /// ignored — see [`Dispatcher::on_conversion_timeout`].
    ConversionTimeout {
        id: String,
        flag: Arc<AtomicBool>,
    },
}

// ---------------------------------------------------------------------------
// Injected side-effect traits
// ---------------------------------------------------------------------------

/// The engine seam: everything the dispatcher needs the print engine to do.
/// Real impl is [`EngineRunner`]; tests fake it.
pub(crate) trait JobRunner: Send + Sync + 'static {
    /// Convert `job`'s input to a printable PDF (or `Ok(None)` for a PDF that
    /// prints in place). `cancel` is polled by the engine at stage boundaries.
    ///
    /// `timeout` bounds each external converter process. It is the limit in
    /// force when *this* attempt started, handed over per call rather than
    /// fixed when the queue booted, so a conversion timeout changed in
    /// Settings reaches the very next conversion.
    ///
    /// `password` is the one the user supplied for this job's encrypted PDF
    /// (FR-059), held only in the dispatcher's memory.
    fn convert(
        &self,
        job: &Job,
        cancel: Arc<AtomicBool>,
        progress: Box<dyn Fn(f32) + Send + Sync>,
        timeout: Option<Duration>,
        password: Option<PdfPassword>,
    ) -> Result<Option<PathBuf>, EngineError>;

    /// Spool the job's PDF to `printer`, waiting at most `timeout` for the
    /// spooler child (`None` waits forever).
    fn print(
        &self,
        job: &Job,
        printer: &ResolvedPrinter,
        timeout: Option<Duration>,
    ) -> Result<SpoolOutcome, PrintError>;

    /// Resolve the printer to use and what it can do.
    ///
    /// `file_name` carries the input's name so a file-type routing rule can
    /// apply (see [`crate::ipc::printers::resolve_app_printer`]).
    fn resolve_printer(
        &self,
        requested: Option<&str>,
        file_name: &str,
    ) -> Result<ResolvedPrinter, PrintError>;

    /// Best-effort cancellation of an already-spooled OS job on `printer`.
    fn cancel_os_job(&self, printer: &str, os_job_id: &str);
}

/// The OS-queue seam: where a spooled job stands. Real impl is [`OsTracker`];
/// tests script the answers so the dispatcher stays hermetic.
pub(crate) trait JobTracker: Send + Sync + 'static {
    /// Ask the spooler once about `os_job_id` on `printer`. Must not block for
    /// long: the dispatcher owns the cadence and the overall budget.
    fn poll(&self, printer: &str, os_job_id: &str) -> Result<OsJobState, TrackError>;

    /// Whether `os_job_id`, read back from `queue.json`, can still be told
    /// apart from a job that later took the same id
    /// ([`crate::engine::print::os_job_id_verifiable`]).
    fn verifiable(&self, os_job_id: &str) -> bool {
        crate::engine::print::os_job_id_verifiable(os_job_id)
    }
}

/// Production [`JobTracker`] backed by [`crate::engine::print::job_state`].
pub(crate) struct OsTracker;

impl JobTracker for OsTracker {
    fn poll(&self, printer: &str, os_job_id: &str) -> Result<OsJobState, TrackError> {
        crate::engine::print::job_state(printer, os_job_id)
    }
}

/// The event seam: how queue changes reach the webview. Real impl is
/// [`TauriEmit`]; tests collect into a vector.
pub(crate) trait EventSink: Send + Sync + 'static {
    fn queue_updated(&self, snapshot: &QueueSnapshot);
    fn job_progress(&self, id: &str, phase: &str, percent: f32);
}

/// The offload seam: how blocking convert/print closures are run. Real impl
/// hands them to [`tokio::task::spawn_blocking`]; tests run them inline for
/// determinism.
pub(crate) trait BlockingSpawner: Send + Sync + 'static {
    fn spawn_blocking(&self, task: Box<dyn FnOnce() + Send + 'static>);
}

// ---------------------------------------------------------------------------
// Real implementations
// ---------------------------------------------------------------------------

/// Production [`JobRunner`] backed by [`crate::engine`] and [`crate::platform`].
pub(crate) struct EngineRunner {
    /// Directory converted PDFs are written to (`<cache_dir>/conversions`).
    pub cache_dir: PathBuf,
}

impl JobRunner for EngineRunner {
    fn convert(
        &self,
        job: &Job,
        cancel: Arc<AtomicBool>,
        progress: Box<dyn Fn(f32) + Send + Sync>,
        timeout: Option<Duration>,
        password: Option<PdfPassword>,
    ) -> Result<Option<PathBuf>, EngineError> {
        // The dispatcher's own conversion timer raises the cancel flag at the
        // same limit, which also kills the converter; passing the limit here
        // keeps the bound even if the timer message is delayed.
        crate::engine::convert::convert_to_pdf(
            Path::new(&job.input_path),
            &self.cache_dir,
            &job.id,
            &cancel,
            &*progress,
            timeout,
            &crate::engine::convert::PageSetup::for_settings(&job.settings),
            password.as_ref(),
        )
    }

    fn print(
        &self,
        job: &Job,
        printer: &ResolvedPrinter,
        timeout: Option<Duration>,
    ) -> Result<SpoolOutcome, PrintError> {
        // A converted job prints its cache PDF; a passthrough PDF prints in place.
        let pdf = match &job.cache_pdf {
            Some(p) => PathBuf::from(p),
            None => PathBuf::from(&job.input_path),
        };
        // Page ranges apply to paged documents only; an image is one page, so
        // a range that excludes page 1 would spool an empty job (see
        // `effective_settings`).
        let settings = crate::engine::print::effective_settings(&job.settings, &job.input_ext);
        crate::engine::print::spool(
            &pdf,
            &printer.name,
            &settings,
            &printer.caps,
            &job.file_name,
            timeout,
        )
    }

    /// Resolve the target printer and its duplex capability.
    ///
    /// A named printer that the OS no longer enumerates is **not** an error we
    /// let the spooler discover: the spec makes such a job paused, not failed,
    /// so the user can pick a replacement
    /// (`docs/03-requirements/print-settings.md` §printer). The check only bites
    /// when enumeration actually worked and returned something — a failed or
    /// empty enumeration (the platform backends are best-effort) keeps the old
    /// behaviour of trusting the name and letting spooling speak, so a flaky
    /// `lpstat` can never pause the whole queue.
    fn resolve_printer(
        &self,
        requested: Option<&str>,
        file_name: &str,
    ) -> Result<ResolvedPrinter, PrintError> {
        // Same order as the CLI: explicit request, then the file-type routing
        // rule, then the default (the org policy's, else the app's).
        let prefs = crate::ipc::printers::with_policy(
            crate::settings::Config::load_or_default().printers,
            &crate::settings::PrinterPolicy::load(),
        );
        let name = match crate::ipc::printers::resolve_app_printer(&prefs, requested, file_name) {
            Some(p) => p,
            None => resolve_os_default_printer()?,
        };
        // An explicit request, or the OS default, may still name a printer
        // the user disabled in Pluryprint: that job pauses with the reason.
        crate::ipc::printers::ensure_enabled(&prefs, &name)?;
        let overrides_for = |id: &str| prefs.overrides.get(id).cloned().unwrap_or_default();
        let enumerated = crate::platform::list_printers()
            .ok()
            .filter(|list| !list.is_empty());
        let Some(list) = enumerated else {
            return Ok(ResolvedPrinter {
                overrides: overrides_for(&name),
                name,
                caps: PrinterCaps::unknown(),
            });
        };
        // A display name (e.g. from a profile) is accepted but the spooler only
        // knows queue ids, so spool to the matched printer's id. A name shared
        // by several queues is not guessed at: the job pauses for a choice.
        use crate::ipc::printers::{match_installed, PrinterMatch};
        match match_installed(&list, &name) {
            PrinterMatch::Found(p) => {
                // A display name can hide a queue id the user disabled.
                crate::ipc::printers::ensure_enabled(&prefs, &p.id)?;
                Ok(ResolvedPrinter {
                    caps: crate::platform::printer_caps(p),
                    overrides: overrides_for(&p.id),
                    name: p.id.clone(),
                })
            }
            PrinterMatch::Ambiguous(candidates) => Err(PrintError::PrinterAmbiguous {
                printer: name,
                candidates,
            }),
            PrinterMatch::Missing => Err(PrintError::PrinterMissing { printer: name }),
        }
    }

    fn cancel_os_job(&self, printer: &str, os_job_id: &str) {
        if let Err(e) = crate::engine::print::cancel_os_job(printer, os_job_id) {
            tracing::warn!(event = "os_job_cancel_failed", os_job_id = %os_job_id, error = %e);
        }
    }
}

/// The OS default, used only once nothing at the app level applies. Having no
/// printer at all is user-fixable (pick one), so the caller pauses the job
/// rather than burning retries.
fn resolve_os_default_printer() -> Result<String, PrintError> {
    crate::platform::default_printer()
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
        .ok_or(PrintError::NoPrinter)
}

/// Production [`EventSink`] emitting typed `tauri-specta` events to the webview.
pub(crate) struct TauriEmit {
    pub app: tauri::AppHandle,
}

impl EventSink for TauriEmit {
    fn queue_updated(&self, snapshot: &QueueSnapshot) {
        use tauri_specta::Event;
        if let Err(e) = crate::ipc::events::QueueUpdated(snapshot.clone()).emit(&self.app) {
            tracing::warn!(event = "queue_updated_emit_failed", error = %e);
        }
    }

    fn job_progress(&self, id: &str, phase: &str, percent: f32) {
        use tauri_specta::Event;
        let _ = crate::ipc::events::JobProgressEvent {
            id: id.to_string(),
            phase: phase.to_string(),
            percent,
        }
        .emit(&self.app);
    }
}

/// Production [`BlockingSpawner`].
pub(crate) struct TokioBlockingSpawner;

impl BlockingSpawner for TokioBlockingSpawner {
    fn spawn_blocking(&self, task: Box<dyn FnOnce() + Send + 'static>) {
        tokio::task::spawn_blocking(task);
    }
}

// ---------------------------------------------------------------------------
// Post-lock side effects
// ---------------------------------------------------------------------------

/// Side effects a handler wants to run *after* releasing the state lock — file
/// deletes, OS-job cancels, delayed-retry scheduling. Collecting them keeps the
/// lock scopes tiny and avoids re-entrant locking when the inline test spawner
/// runs work synchronously.
#[derive(Default)]
struct Post {
    delete_cache: Vec<String>,
    /// `(printer, os job id)` pairs to cancel at the spooler.
    cancel_os: Vec<(String, String)>,
    remove_delayed: Vec<String>,
    retry: Option<(String, Duration)>,
}

/// What resuming a job should do about the printer it is pinned to.
///
/// Only produced for a job paused with `printer_unavailable`; see
/// [`Dispatcher::plan_printer_fix`]. The fourth outcome — nothing usable at all
/// — is not a variant: it is an [`IpcError::InvalidState`] carrying what the
/// user has to fix, so resume *refuses* instead of quietly re-pausing.
pub(crate) enum PrinterFix {
    /// Not a printer problem; resume as usual.
    NotNeeded,
    /// The job's own printer resolves again.
    Unchanged,
    /// `missing` is still gone, but the app-level chain now points at
    /// `replacement`: drop the pin so this run follows the preferences.
    Unpin {
        missing: String,
        replacement: String,
    },
}

/// The single print slot: which job holds it and, once spooled, which OS job it
/// is waiting on. See the module docs (§One print slot).
struct PrintSlot {
    job_id: String,
    /// When spooling began; the print budget counts from here.
    started: tokio::time::Instant,
    /// Set once `spool` returned an id; the slot then waits on the OS queue.
    printer: Option<String>,
    os_job_id: Option<String>,
    /// Polls answered so far (drives the backoff).
    polls: u32,
    /// Consecutive polls that failed transiently.
    failures: u32,
    /// The cache PDF of a job cleared from the queue while it was still
    /// spooling: deleting it then would pull the file out from under the
    /// spooler, so it is deleted when the print returns.
    orphan_cache: Option<String>,
}

impl PrintSlot {
    fn new(job_id: String) -> Self {
        Self {
            job_id,
            started: tokio::time::Instant::now(),
            printer: None,
            os_job_id: None,
            polls: 0,
            failures: 0,
            orphan_cache: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

/// The single-writer state machine. Constructed by
/// [`crate::queue::spawn`] and driven by [`run`].
pub(crate) struct Dispatcher {
    state: Arc<RwLock<QueueState>>,
    self_tx: mpsc::UnboundedSender<Cmd>,
    runner: Arc<dyn JobRunner>,
    emit: Arc<dyn EventSink>,
    spawner: Arc<dyn BlockingSpawner>,
    tracker: Arc<dyn JobTracker>,
    queue_file: PathBuf,
    /// Engine limits (concurrency cap and both timeouts); replaced at run time
    /// by [`Cmd::SetLimits`].
    limits: DispatcherLimits,
    /// The single print slot, held from spooling until the OS job leaves the
    /// queue.
    print_slot: Option<PrintSlot>,
    /// Cancellation flags for in-flight conversions, keyed by job id.
    converting: HashMap<String, Arc<AtomicBool>>,
    /// Jobs waiting out a retry backoff, with the generation of the backoff
    /// they wait on; the scheduler skips these.
    delayed: HashMap<String, u64>,
    /// Source of [`Cmd::RetryDue`] generations: each armed backoff gets the
    /// next value.
    retry_generation: u64,
    /// Last progress-emit instant per job, for throttling. Shared into the
    /// progress closures handed to the runner.
    progress_last: Arc<Mutex<HashMap<String, Instant>>>,
    /// Whether the last `queue.json` write failed, so persistence trouble is
    /// logged loudly once per streak instead of on every transition.
    persist_failing: bool,
    /// FR-059 / NFR-082: passwords the user supplied for encrypted PDFs, by
    /// job id. Memory only — never persisted, never logged — and dropped as
    /// soon as the job no longer needs it (decrypted, refused, finished,
    /// cleared). After a restart the job simply asks again.
    passwords: HashMap<String, PdfPassword>,
}

impl Dispatcher {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        state: Arc<RwLock<QueueState>>,
        self_tx: mpsc::UnboundedSender<Cmd>,
        runner: Arc<dyn JobRunner>,
        emit: Arc<dyn EventSink>,
        spawner: Arc<dyn BlockingSpawner>,
        tracker: Arc<dyn JobTracker>,
        queue_file: PathBuf,
        limits: DispatcherLimits,
    ) -> Self {
        Self {
            state,
            self_tx,
            runner,
            emit,
            spawner,
            tracker,
            queue_file,
            limits,
            print_slot: None,
            converting: HashMap::new(),
            delayed: HashMap::new(),
            retry_generation: 0,
            progress_last: Arc::new(Mutex::new(HashMap::new())),
            persist_failing: false,
            passwords: HashMap::new(),
        }
    }

    /// Handle one command, then (re)schedule and persist/emit the result. This
    /// is the unit the run loop and the tests both drive.
    pub(crate) fn dispatch_one(&mut self, cmd: Cmd) {
        self.handle(cmd);
        self.forget_unneeded_passwords();
        self.schedule();
        self.commit();
    }

    /// Kick the pipeline for jobs recovered from disk, then persist/emit the
    /// starting snapshot.
    ///
    /// A print that was already in the OS queue when the app closed comes back
    /// as `Printing` ([`crate::queue::persist::recover`]): it takes the print
    /// slot again and is tracked to its end, instead of being paused and
    /// spooled a second time on resume.
    pub(crate) fn bootstrap(&mut self) {
        self.adopt_handed_off_print();
        self.drop_decrypted_copies();
        self.schedule();
        self.commit();
    }

    /// FR-059 / NFR-082: a decrypted copy of a password-protected PDF never
    /// outlives the run that made it. After a restart the password is gone
    /// (memory only), so the copy is deleted and its job asks for the
    /// password again instead of printing it without one. A print that was
    /// already handed to the spooler keeps its state (the spooler holds its
    /// own copy); everything else that still needs printing pauses on
    /// `password_required`.
    fn drop_decrypted_copies(&mut self) {
        let mut post = Post::default();
        {
            let mut st = self.state.write().unwrap();
            for job in st.jobs.iter_mut().filter(|j| holds_decrypted_copy(j)) {
                if let Some(c) = job.cache_pdf.take() {
                    post.delete_cache.push(c);
                }
                tracing::info!(event = "job_decrypted_copy_dropped", job_id = %job.id);
                if job.state.is_terminal() || crate::queue::persist::is_handed_off(job) {
                    continue;
                }
                pause_needs_fix(
                    job,
                    &EngineError::PdfProtected(crate::engine::PdfProtection::PasswordRequired),
                );
            }
        }
        self.apply_post(post);
    }

    /// Re-attach the print slot to a recovered, already spooled job.
    ///
    /// Only a spooler reference that can still be told apart from a job that
    /// later took the same id is adopted ([`crate::engine::print::os_job_id_verifiable`]):
    /// the Windows spooler reuses ids after a restart, and following (or, on
    /// Cancel, deleting) somebody else's print is worse than not following
    /// ours. One that cannot be checked ends as [`JobState::Unconfirmed`]: it
    /// was handed to the spooler, so it is not sent again, and nobody can say
    /// whether it printed.
    ///
    /// The adopted print keeps the budget it had: the print timeout counts
    /// from when it was spooled (`spooled_at`), not from this launch.
    fn adopt_handed_off_print(&mut self) {
        let mut adopt: Option<(String, String, String, Duration)> = None;
        let mut post = Post::default();
        {
            let mut st = self.state.write().unwrap();
            for job in st.jobs.iter_mut() {
                if job.state != JobState::Printing {
                    continue;
                }
                let handed_off = (
                    job.printer_resolved.clone(),
                    job.os_job_id.clone(),
                    crate::queue::persist::is_handed_off(job),
                );
                match handed_off {
                    (Some(_), Some(oid), true) if !self.tracker.verifiable(&oid) => {
                        mark_unconfirmed(job, "not_verifiable", &mut post);
                    }
                    (Some(printer), Some(oid), true) if adopt.is_none() => {
                        tracing::info!(
                            event = "job_tracking_resumed",
                            job_id = %job.id,
                            file = %job.file_name,
                            printer = %printer,
                            os_job_id = %oid,
                        );
                        adopt = Some((job.id.clone(), printer, oid, spooled_for(job)));
                    }
                    // `recover` leaves at most one; anything else cannot be
                    // followed, so it waits for the user like any interrupted
                    // job.
                    _ => {
                        job.state = JobState::Paused;
                        job.progress = None;
                        job.error = Some(JobError::new(
                            "interrupted_by_shutdown",
                            "Pluryprint was closed while this job was in progress.",
                        ));
                    }
                }
            }
        }
        if let Some((id, printer, oid, spooled_for)) = adopt {
            let mut slot = PrintSlot::new(id.clone());
            let now = tokio::time::Instant::now();
            slot.started = now.checked_sub(spooled_for).unwrap_or(now);
            slot.printer = Some(printer);
            slot.os_job_id = Some(oid);
            self.print_slot = Some(slot);
            self.arm_track_timer(id, track_delay(0));
        }
        self.apply_post(post);
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Submit { jobs } => self.on_submit(jobs),
            Cmd::Cancel { id, reply } => {
                let _ = reply.send(self.on_cancel(&id));
            }
            Cmd::CancelAll { reply } => {
                let _ = reply.send(self.on_cancel_all());
            }
            Cmd::Pause { id, reply } => {
                let _ = reply.send(self.on_pause(&id));
            }
            Cmd::Resume { id, reply } => self.on_resume_request(id, reply),
            Cmd::ResumePlanned { id, fix, reply } => {
                let _ = reply.send(fix.and_then(|fix| self.on_resume(&id, fix)));
            }
            Cmd::Retry { id, reply } => {
                let _ = reply.send(self.on_retry(&id));
            }
            Cmd::SupplyPassword {
                id,
                password,
                apply_to_batch,
                reply,
            } => {
                let _ = reply.send(self.on_supply_password(&id, password, apply_to_batch));
            }
            Cmd::Adjust { id, change, reply } => {
                let _ = reply.send(self.on_adjust(&id, change));
            }
            Cmd::PauseQueue => self.set_queue_paused(true),
            Cmd::ResumeQueue => self.set_queue_paused(false),
            Cmd::SetLimits { limits } => self.set_limits(limits),
            Cmd::ClearDone { reply } => {
                let _ = reply.send(self.on_clear_done());
            }
            Cmd::ConversionDone { id, result, pages } => self.on_conversion_done(id, result, pages),
            Cmd::PrintDone { id, result } => self.on_print_done(id, result),
            Cmd::RetryDue { id, generation } => {
                // Only the timer of the backoff the job is waiting on now ends
                // it (FR-034's 1s/5s/25s schedule).
                if self.delayed.get(&id) == Some(&generation) {
                    self.delayed.remove(&id);
                }
            }
            Cmd::TrackDue { id } => self.on_track_due(id),
            Cmd::TrackResult { id, result } => self.on_track_result(id, result),
            Cmd::ConversionTimeout { id, flag } => self.on_conversion_timeout(id, flag),
        }
    }

    fn on_submit(&mut self, jobs: Vec<Job>) {
        let mut st = self.state.write().unwrap();
        for job in jobs {
            let f = JobLog::of(&job);
            // NFR-082: the absolute path never reaches the default-level log —
            // it names the user and their folders. Records carry `file` (the
            // basename) plus `job_id`, which FR-035's "file path" clause is
            // satisfied by; the full path is only available at TRACE, for a
            // user who opts in through `RUST_LOG`.
            tracing::trace!(
                event = "job_input_path",
                job_id = %job.id,
                input_path = %job.input_path
            );
            tracing::info!(
                event = "job_submitted",
                job_id = %job.id,
                file = %f.file,
                profile = %f.profile,
                printer = %f.printer,
                pages = f.pages,
                status = %f.status,
                submitted_at = %job.submitted_at
            );
            st.jobs.push(job);
        }
    }

    fn on_cancel(&mut self, id: &str) -> Result<(), IpcError> {
        let mut post = Post::default();
        let result = {
            let mut st = self.state.write().unwrap();
            match st.jobs.iter_mut().find(|j| j.id == id) {
                None => Err(IpcError::JobNotFound(id.to_string())),
                Some(job) if job.state.is_terminal() => Err(job_finished(id)),
                Some(job) => {
                    cancel_job_in_place(job, &self.converting, &mut post);
                    release_tracked_slot(&mut self.print_slot, job, &mut post);
                    post.remove_delayed.push(id.to_string());
                    Ok(())
                }
            }
        };
        self.apply_post(post);
        if result.is_ok() {
            tracing::info!(event = "job_cancelled", job_id = %id);
        }
        result
    }

    fn on_cancel_all(&mut self) -> u32 {
        let mut post = Post::default();
        let count;
        {
            let mut st = self.state.write().unwrap();
            let ids: Vec<String> = st
                .jobs
                .iter()
                .filter(|j| !j.state.is_terminal())
                .map(|j| j.id.clone())
                .collect();
            count = ids.len() as u32;
            for job in st.jobs.iter_mut().filter(|j| !j.state.is_terminal()) {
                cancel_job_in_place(job, &self.converting, &mut post);
                release_tracked_slot(&mut self.print_slot, job, &mut post);
            }
            post.remove_delayed = ids;
        }
        self.apply_post(post);
        if count > 0 {
            tracing::info!(event = "queue_cancel_all", count);
        }
        count
    }

    fn on_pause(&mut self, id: &str) -> Result<(), IpcError> {
        let result = {
            let mut st = self.state.write().unwrap();
            match st.jobs.iter_mut().find(|j| j.id == id) {
                None => Err(IpcError::JobNotFound(id.to_string())),
                Some(job) => match job.state {
                    JobState::Queued | JobState::Converted => {
                        job.state = JobState::Paused;
                        job.progress = None;
                        Ok(())
                    }
                    JobState::Converting => {
                        if let Some(flag) = self.converting.get(&job.id) {
                            flag.store(true, Ordering::Relaxed);
                        }
                        job.state = JobState::Paused;
                        job.progress = None;
                        Ok(())
                    }
                    // Idempotent: pausing an already-paused job is a no-op.
                    JobState::Paused => Ok(()),
                    JobState::Printing => Err(IpcError::localized(
                        IpcErrorCode::InvalidState,
                        "job_printing",
                        "cannot pause a job that is already printing",
                    )),
                    JobState::Done
                    | JobState::Unconfirmed
                    | JobState::Failed
                    | JobState::Cancelled => Err(job_finished(id)),
                },
            }
        };
        if result.is_ok() {
            tracing::info!(event = "job_paused", job_id = %id);
        }
        result
    }

    /// The printer question resuming `id` has to answer first, if any: the
    /// job's pinned printer and file name when it is paused because its
    /// printer could not be used. Everything else resumes unchanged.
    fn printer_question(&self, id: &str) -> Option<(Option<String>, String)> {
        let st = self.state.read().unwrap();
        let job = st.jobs.iter().find(|j| j.id == id)?;
        let stuck_on_printer = job.state == JobState::Paused
            && job
                .error
                .as_ref()
                .is_some_and(|e| e.code == "printer_unavailable" || e.code == "printer_disabled");
        stuck_on_printer.then(|| (job.settings.printer.clone(), job.file_name.clone()))
    }

    /// Resume `id`. A job stuck on its printer needs the printer re-resolved
    /// first, which reads the OS printer list (`lpstat`, the spooler) and can
    /// take seconds with slow network printers — so that runs on the blocking
    /// pool and comes back as [`Cmd::ResumePlanned`], instead of stalling every
    /// other command and completion behind it on this loop.
    fn on_resume_request(&mut self, id: String, reply: oneshot::Sender<Result<(), IpcError>>) {
        let Some((requested, file_name)) = self.printer_question(&id) else {
            let _ = reply.send(self.on_resume(&id, PrinterFix::NotNeeded));
            return;
        };
        let runner = self.runner.clone();
        let tx = self.self_tx.clone();
        self.spawner.spawn_blocking(Box::new(move || {
            let fix = plan_printer_fix(&*runner, requested, &file_name);
            let _ = tx.send(Cmd::ResumePlanned { id, fix, reply });
        }));
    }

    /// Apply a resume once its printer question (if any) is answered.
    ///
    /// A job paused with `printer_unavailable` used to resume straight back
    /// into the same failure with no way out: its printer was pinned in
    /// `settings.printer`, that queue was gone, and nothing could re-point it.
    /// Resuming now re-runs the real resolution first ([`plan_printer_fix`]),
    /// so it either succeeds, follows the reconciled preferences, or says why
    /// not.
    fn on_resume(&mut self, id: &str, fix: PrinterFix) -> Result<(), IpcError> {
        let mut post = Post::default();
        let result = {
            let mut st = self.state.write().unwrap();
            match st.jobs.iter_mut().find(|j| j.id == id) {
                None => Err(IpcError::JobNotFound(id.to_string())),
                Some(job) if job.state != JobState::Paused => Err(IpcError::localized(
                    IpcErrorCode::InvalidState,
                    "job_not_paused",
                    "only a paused job can be resumed",
                )),
                Some(job) => {
                    let cache_valid = job
                        .cache_pdf
                        .as_deref()
                        .map(|p| Path::new(p).exists())
                        .unwrap_or(false);
                    let interrupted = job
                        .error
                        .as_ref()
                        .map(|e| e.code == "interrupted_by_shutdown")
                        .unwrap_or(false);
                    job.error = None;
                    if interrupted {
                        job.attempts = 0;
                    }
                    if let PrinterFix::Unpin {
                        missing,
                        replacement,
                    } = &fix
                    {
                        // Drop the pin rather than writing `replacement` into
                        // the job: dispatch resolves the printer itself, and a
                        // name copied in here would go stale all over again.
                        // The job does show where it is going now, so the
                        // move is visible in the queue rather than only in
                        // the log (dispatch overwrites it with the real one).
                        job.settings.printer = None;
                        job.printer_resolved = Some(replacement.clone());
                        tracing::info!(
                            event = "job_printer_reassigned",
                            job_id = %job.id,
                            file = %job.file_name,
                            missing_printer = %missing,
                            printer = %replacement,
                        );
                    }
                    job.state = if cache_valid {
                        JobState::Converted
                    } else {
                        JobState::Queued
                    };
                    job.progress = None;
                    post.remove_delayed.push(id.to_string());
                    Ok(())
                }
            }
        };
        self.apply_post(post);
        if result.is_ok() {
            tracing::info!(event = "job_resumed", job_id = %id);
        }
        result
    }

    /// FR-059: take the password the user typed for `id`'s encrypted PDF and
    /// resume the job with it. Only a job that is waiting for a password
    /// ([`Job::needs_password`]) takes one. With `apply_to_batch`, the same
    /// password is also tried on every other job of the batch that is waiting
    /// for one (they resume too) or has not been converted yet (it is used if
    /// that PDF turns out to be protected).
    fn on_supply_password(
        &mut self,
        id: &str,
        password: PdfPassword,
        apply_to_batch: bool,
    ) -> Result<(), IpcError> {
        let (waiting, pending) = {
            let st = self.state.read().unwrap();
            let Some(job) = st.jobs.iter().find(|j| j.id == id) else {
                return Err(IpcError::JobNotFound(id.to_string()));
            };
            if !job.needs_password() {
                return Err(IpcError::localized(
                    IpcErrorCode::InvalidState,
                    "job_not_waiting_password",
                    format!("job {id} is not waiting for a password"),
                ));
            }
            let mut waiting = vec![id.to_string()];
            let mut pending = Vec::new();
            if apply_to_batch {
                let batch = job.batch_id.clone();
                for other in st.jobs.iter().filter(|j| j.batch_id == batch && j.id != id) {
                    if other.needs_password() {
                        waiting.push(other.id.clone());
                    } else if matches!(other.state, JobState::Queued | JobState::Paused)
                        && other.cache_pdf.is_none()
                    {
                        pending.push(other.id.clone());
                    }
                }
            }
            (waiting, pending)
        };
        for jid in waiting.iter().chain(&pending) {
            self.passwords.insert(jid.clone(), password.clone());
        }
        for jid in &waiting {
            self.on_resume(jid, PrinterFix::NotNeeded)?;
        }
        tracing::info!(
            event = "job_password_supplied",
            job_id = %id,
            resumed = waiting.len(),
            batch = apply_to_batch
        );
        Ok(())
    }

    /// Drop every stored password its job no longer needs: the job is gone or
    /// finished, or its PDF is already decrypted into the cache.
    fn forget_unneeded_passwords(&mut self) {
        if self.passwords.is_empty() {
            return;
        }
        let st = self.state.read().unwrap();
        self.passwords.retain(|id, _| {
            st.jobs.iter().any(|j| {
                j.id == *id
                    && !j.state.is_terminal()
                    && j.cache_pdf.is_none()
                    && !j.error.as_ref().is_some_and(|e| e.code == "pdf_protected")
            })
        });
    }

    /// Change a paused job's printer or sides (FR-040/FR-044).
    ///
    /// A job paused because its printer cannot do what it asks
    /// (`unsupported_setting`), or because its printer cannot be used
    /// (`printer_unavailable`, `printer_disabled`), used to have one way out:
    /// cancel it and print the file again, although the pause message says to
    /// choose one-sided or another printer. This applies that choice to the
    /// job itself. The job stays paused — resuming it is the caller's next
    /// step — so the change is visible before anything is sent. The printer
    /// it resolved to last time is forgotten, since the next run resolves
    /// afresh.
    fn on_adjust(&mut self, id: &str, change: JobChange) -> Result<(), IpcError> {
        let mut st = self.state.write().unwrap();
        let job = st
            .jobs
            .iter_mut()
            .find(|j| j.id == id)
            .ok_or_else(|| IpcError::JobNotFound(id.to_string()))?;
        if job.state != JobState::Paused {
            return Err(IpcError::InvalidState(
                "only a paused job can be changed".to_string(),
            ));
        }
        if let Some(printer) = change.printer {
            let printer = printer.trim().to_string();
            job.settings.printer = (!printer.is_empty()).then_some(printer);
            job.printer_resolved = None;
        }
        if let Some(duplex) = change.duplex {
            job.settings.duplex = duplex;
        }
        tracing::info!(
            event = "job_adjusted",
            job_id = %job.id,
            file = %job.file_name,
            printer = %job.settings.printer.as_deref().unwrap_or("<default>"),
            duplex = ?job.settings.duplex,
        );
        Ok(())
    }

    fn on_retry(&mut self, id: &str) -> Result<(), IpcError> {
        let result = {
            let mut st = self.state.write().unwrap();
            match st.jobs.iter_mut().find(|j| j.id == id) {
                None => Err(IpcError::JobNotFound(id.to_string())),
                Some(job) if job.state != JobState::Failed => Err(IpcError::localized(
                    IpcErrorCode::InvalidState,
                    "job_not_failed",
                    "only a failed job can be retried",
                )),
                Some(job) => {
                    job.state = JobState::Queued;
                    job.attempts = 0;
                    job.error = None;
                    job.finished_at = None;
                    job.progress = None;
                    // A retry is a new run: its duration starts now (the
                    // scheduler stamps `started_at` again), and the printer the
                    // failed run resolved to is not necessarily this run's.
                    job.started_at = None;
                    job.printer_resolved = None;
                    Ok(())
                }
            }
        };
        if result.is_ok() {
            tracing::info!(event = "job_retry", job_id = %id);
        }
        result
    }

    fn set_queue_paused(&mut self, paused: bool) {
        self.state.write().unwrap().queue_paused = paused;
        tracing::info!(
            event = if paused {
                "queue_paused"
            } else {
                "queue_resumed"
            }
        );
    }

    /// Apply new engine limits to the running queue.
    ///
    /// What changes when:
    /// - **Concurrency** applies at once: `schedule()` runs right after this
    ///   command, so a raised cap starts waiting conversions immediately. A
    ///   lowered cap never kills running work; it just starts nothing new until
    ///   the in-flight count drops below it.
    /// - **Conversion timeout** applies to conversions started from now on.
    ///   Each attempt arms its own timer when it starts, and re-arming running
    ///   ones would make a single attempt's budget depend on when the user
    ///   happened to save.
    /// - **Print timeout** applies to the next print, and to a print that is
    ///   being tracked: its deadline is re-read on every poll.
    fn set_limits(&mut self, limits: DispatcherLimits) {
        if limits == self.limits {
            return;
        }
        self.limits = limits;
        let secs = |d: Option<Duration>| d.map(|d| d.as_secs()).unwrap_or(0);
        tracing::info!(
            event = "queue_limits_updated",
            max_concurrent = limits.max_concurrent,
            conversion_timeout_s = secs(limits.conversion_timeout),
            print_timeout_s = secs(limits.print_timeout),
        );
    }

    fn on_clear_done(&mut self) -> u32 {
        let mut post = Post::default();
        let count;
        {
            let mut st = self.state.write().unwrap();
            let before = st.jobs.len();
            for job in st.jobs.iter().filter(|j| j.state.is_terminal()) {
                let Some(c) = &job.cache_pdf else { continue };
                match self.print_slot.as_mut() {
                    // Cancelled while still spooling: the spooler may be
                    // reading this very file. It is deleted when the print
                    // returns (`on_print_done`), not now.
                    Some(slot) if slot.job_id == job.id => slot.orphan_cache = Some(c.clone()),
                    _ => post.delete_cache.push(c.clone()),
                }
            }
            st.jobs.retain(|j| !j.state.is_terminal());
            count = (before - st.jobs.len()) as u32;
        }
        self.apply_post(post);
        count
    }

    fn on_conversion_done(
        &mut self,
        id: String,
        result: Result<Option<PathBuf>, EngineError>,
        pages: Option<u32>,
    ) {
        // The conversion is no longer in flight regardless of outcome. Dropping
        // the throttle entry too keeps `progress_last` bounded by the number of
        // conversions actually in flight.
        self.converting.remove(&id);
        self.progress_last.lock().unwrap().remove(&id);
        let mut post = Post::default();
        {
            let mut st = self.state.write().unwrap();
            match st.jobs.iter_mut().find(|j| j.id == id) {
                None => {
                    // Job was cleared while converting; discard any file it produced.
                    if let Ok(Some(p)) = &result {
                        post.delete_cache.push(p.to_string_lossy().into_owned());
                    }
                }
                Some(job) => match job.state {
                    JobState::Converting => match result {
                        Ok(cache) => {
                            job.cache_pdf =
                                cache.as_ref().map(|p| p.to_string_lossy().into_owned());
                            // The converted PDF is the truth about what will
                            // print; before it existed a document's count
                            // was unknown.
                            if pages.is_some() {
                                job.pages = pages;
                            }
                            job.state = JobState::Converted;
                            job.error = None;
                            job.progress = None;
                            tracing::info!(
                                event = "job_converted",
                                job_id = %id,
                                passthrough = cache.is_none()
                            );
                        }
                        // A cancel flag without a matching state change should
                        // not happen; requeue without penalty just in case.
                        Err(EngineError::Cancelled) => {
                            job.state = JobState::Queued;
                            job.progress = None;
                        }
                        Err(e) => record_failure(job, &e, true, &mut post),
                    },
                    // Cancelled/paused mid-conversion, or failed by a timeout
                    // that set this attempt's cancel flag: drop whatever partial
                    // cache the aborted conversion produced. The `converting`
                    // entry was already removed at the top of this handler.
                    JobState::Cancelled | JobState::Paused | JobState::Failed => {
                        if let Ok(Some(p)) = &result {
                            post.delete_cache.push(p.to_string_lossy().into_owned());
                        }
                        job.progress = None;
                    }
                    // Late/duplicate completion for a job that moved on.
                    _ => {
                        if let Ok(Some(p)) = &result {
                            post.delete_cache.push(p.to_string_lossy().into_owned());
                        }
                    }
                },
            }
        }
        self.apply_post(post);
    }

    /// A per-attempt conversion timer fired. Fail the job **only** if this timer
    /// still refers to the live conversion: the job must still be
    /// [`JobState::Converting`] and the cancel flag registered for it must be the
    /// exact [`Arc`] this timer captured. A pause/resume or a retry swaps in a
    /// fresh flag, so a stale timer from an earlier attempt is a no-op — this is
    /// what prevents a timer armed for attempt *N* from killing attempt *N+1*.
    ///
    /// On a real timeout: set the flag (so the running conversion aborts at its
    /// next stage boundary), mark the job [`JobState::Failed`] with the
    /// `"conversion_timeout"` code (terminal — no automatic retry), and drop any
    /// cached PDF. The eventual [`Cmd::ConversionDone`] for the aborted attempt
    /// finds a `Failed` job and discards its partial output (see
    /// [`Dispatcher::on_conversion_done`]).
    ///
    /// Waiting window: the job's id stays in `converting` until that
    /// stale `ConversionDone` arrives — the orphaned conversion keeps running
    /// (we can only *flag* a blocking task, not kill it). So if the user hits
    /// Retry before the orphan reports back, the job returns to `Queued` but the
    /// scheduler skips it (it's still in `converting`, see [`Dispatcher::schedule`])
    /// until the orphan finishes. The retry is delayed by that window, not lost.
    fn on_conversion_timeout(&mut self, id: String, flag: Arc<AtomicBool>) {
        // A stale flag identifies a timer whose attempt is already gone.
        let still_current = self
            .converting
            .get(&id)
            .map(|f| Arc::ptr_eq(f, &flag))
            .unwrap_or(false);
        if !still_current {
            return;
        }

        let timeout_secs = self
            .limits
            .conversion_timeout
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let mut post = Post::default();
        {
            let mut st = self.state.write().unwrap();
            if let Some(job) = st.jobs.iter_mut().find(|j| j.id == id) {
                if job.state == JobState::Converting {
                    flag.store(true, Ordering::Relaxed);
                    job.state = JobState::Failed;
                    job.error = Some(
                        JobError::new(
                            "conversion_timeout",
                            format!("conversion timed out after {timeout_secs}s"),
                        )
                        .with_param("secs", timeout_secs),
                    );
                    job.finished_at = Some(now_rfc3339());
                    job.progress = None;
                    if let Some(c) = job.cache_pdf.take() {
                        post.delete_cache.push(c);
                    }
                    let f = JobLog::of(job);
                    tracing::warn!(
                        event = "job_conversion_timeout",
                        job_id = %id,
                        file = %f.file,
                        profile = %f.profile,
                        printer = %f.printer,
                        pages = f.pages,
                        status = %f.status,
                        started_at = %f.started_at,
                        finished_at = %f.finished_at,
                        error = %f.error,
                        timeout_s = timeout_secs
                    );
                }
            }
        }
        self.apply_post(post);
    }

    fn on_print_done(&mut self, id: String, result: Result<PrintOk, PrintError>) {
        let mut post = Post::default();
        // `(printer, os job id)` when the spooler handed back an id to follow.
        let mut track: Option<(String, String)> = None;
        {
            let mut st = self.state.write().unwrap();
            match st.jobs.iter_mut().find(|j| j.id == id) {
                None => {
                    // Job cleared after spool; cancel it at the OS if it landed.
                    if let Ok(ok) = &result {
                        if let Some(oid) = &ok.os_job_id {
                            post.cancel_os.push((ok.printer.clone(), oid.clone()));
                        }
                    }
                }
                Some(job) => match job.state {
                    JobState::Printing => match result {
                        Ok(ok) => {
                            job.printer_resolved = Some(ok.printer.clone());
                            match ok.os_job_id {
                                // Handed off: the job is done when the OS job is.
                                Some(oid) => {
                                    job.os_job_id = Some(oid.clone());
                                    job.progress = Some(JobProgress::indeterminate("printing"));
                                    let f = JobLog::of(job);
                                    tracing::info!(
                                        event = "job_spooled",
                                        job_id = %id,
                                        file = %f.file,
                                        printer = %f.printer,
                                        os_job_id = %oid,
                                        status = %f.status
                                    );
                                    track = Some((ok.printer, oid));
                                }
                                // No id to follow: the hand-off is all we can
                                // know, which is not a confirmed print.
                                None => mark_unconfirmed(job, "untracked", &mut post),
                            }
                        }
                        Err(e) => record_print_failure(job, e, &mut post),
                    },
                    // Cancelled during the spool window: reconcile.
                    JobState::Cancelled => {
                        if let Ok(ok) = &result {
                            if let Some(oid) = &ok.os_job_id {
                                post.cancel_os.push((ok.printer.clone(), oid.clone()));
                            }
                        }
                        if let Some(c) = job.cache_pdf.take() {
                            post.delete_cache.push(c);
                        }
                    }
                    _ => {}
                },
            }
        }

        // Settle the print slot: it moves on to tracking, or it is free.
        if let Some(slot) = self.print_slot.as_mut().filter(|s| s.job_id == id) {
            if let Some(c) = slot.orphan_cache.take() {
                post.delete_cache.push(c);
            }
            match track {
                Some((printer, oid)) => {
                    slot.printer = Some(printer);
                    slot.os_job_id = Some(oid);
                    let delay = track_delay(0);
                    self.arm_track_timer(id, delay);
                }
                None => self.print_slot = None,
            }
        }
        self.apply_post(post);
    }

    /// Ask the spooler about the tracked print after `delay`.
    fn arm_track_timer(&self, id: String, delay: Duration) {
        let tx = self.self_tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = tx.send(Cmd::TrackDue { id });
        });
    }

    /// A tracking timer fired: poll the spooler off the loop (it shells out or
    /// calls the spooler API) and report back with [`Cmd::TrackResult`].
    fn on_track_due(&mut self, id: String) {
        let Some(slot) = self.print_slot.as_ref().filter(|s| s.job_id == id) else {
            return; // cancelled or finished since the timer was armed
        };
        let (Some(printer), Some(oid)) = (slot.printer.clone(), slot.os_job_id.clone()) else {
            return;
        };
        let tracker = self.tracker.clone();
        let tx = self.self_tx.clone();
        self.spawner.spawn_blocking(Box::new(move || {
            let result = tracker.poll(&printer, &oid);
            let _ = tx.send(Cmd::TrackResult { id, result });
        }));
    }

    /// Act on what the spooler said about the tracked print `id`.
    fn on_track_result(&mut self, id: String, result: Result<OsJobState, TrackError>) {
        let print_timeout = self.limits.print_timeout;
        let Some(slot) = self.print_slot.as_mut().filter(|s| s.job_id == id) else {
            return; // a stale answer for a print that already settled
        };
        slot.polls += 1;
        match &result {
            Ok(_) => slot.failures = 0,
            Err(TrackError::Transient(_)) => slot.failures += 1,
            Err(TrackError::Unavailable(_)) => {}
        }
        let failures = slot.failures;
        let polls = slot.polls;
        let timed_out = print_timeout.is_some_and(|t| slot.started.elapsed() >= t);

        let mut post = Post::default();
        let finished = {
            let mut st = self.state.write().unwrap();
            match st.jobs.iter_mut().find(|j| j.id == id) {
                Some(job) if job.state == JobState::Printing => {
                    let settled = settle_tracked(job, &result, failures, &mut post);
                    if !settled && timed_out {
                        fail_print_timeout(job, print_timeout, &mut post);
                        true
                    } else {
                        settled
                    }
                }
                // Gone from the queue or no longer printing: nothing to follow.
                _ => true,
            }
        };
        if finished {
            self.print_slot = None;
        } else {
            self.arm_track_timer(id, track_delay(polls));
        }
        self.apply_post(post);
    }

    /// Run post-lock side effects: file deletes, OS cancels, delayed-retry
    /// scheduling. Called with the state lock released.
    fn apply_post(&mut self, post: Post) {
        for path in post.delete_cache {
            delete_cache_file(&path);
        }
        for (printer, oid) in post.cancel_os {
            self.runner.cancel_os_job(&printer, &oid);
        }
        for id in post.remove_delayed {
            self.delayed.remove(&id);
        }
        if let Some((id, delay)) = post.retry {
            self.retry_generation += 1;
            let generation = self.retry_generation;
            self.delayed.insert(id.clone(), generation);
            let tx = self.self_tx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                let _ = tx.send(Cmd::RetryDue { id, generation });
            });
        }
    }

    /// Start as much new work as the concurrency limits and pause state allow.
    /// Mutates job states under a short lock, then spawns the blocking work with
    /// the lock released.
    fn schedule(&mut self) {
        let mut to_convert: Vec<(Job, Arc<AtomicBool>)> = Vec::new();
        let mut to_print: Option<Job> = None;
        {
            let mut st = self.state.write().unwrap();
            if !st.queue_paused {
                // Count the conversions actually in flight, not the jobs in the
                // Converting state: a paused, cancelled or timed-out conversion
                // is only *flagged* and keeps burning CPU until its next stage
                // boundary, so it holds its slot until `ConversionDone` removes
                // it from `self.converting` (FR-037).
                let mut converting = self.converting.len();
                while converting < self.limits.max_concurrent {
                    // A Queued job whose id is still in `self.converting` was
                    // paused/resumed — or retried after a conversion timeout —
                    // while its previous conversion is still aborting; starting a
                    // second conversion would race both onto the same cache path.
                    // It becomes eligible again once the stale ConversionDone
                    // arrives and removes the entry.
                    //
                    // This is the *waiting window* for a retried
                    // timed-out job. `on_conversion_timeout` marks the job Failed
                    // but leaves its orphaned conversion running (we can't kill a
                    // blocking task, only flag it). A user Retry moves the job
                    // back to Queued, yet it is skipped here until the orphan
                    // reports back and clears `converting`. It is delayed, never
                    // stuck forever — the orphan finishes (or the flag aborts it
                    // at the next stage boundary) and the retry then proceeds.
                    let Some(idx) = st.jobs.iter().position(|j| {
                        j.state == JobState::Queued
                            && !self.delayed.contains_key(&j.id)
                            && !self.converting.contains_key(&j.id)
                    }) else {
                        break;
                    };
                    let flag = Arc::new(AtomicBool::new(false));
                    {
                        let job = &mut st.jobs[idx];
                        job.state = JobState::Converting;
                        if job.started_at.is_none() {
                            job.started_at = Some(now_rfc3339());
                        }
                        job.progress = Some(JobProgress::measured("converting", 0.0));
                        job.error = None;
                        let f = JobLog::of(job);
                        tracing::info!(
                            event = "job_start_convert",
                            job_id = %job.id,
                            file = %f.file,
                            profile = %f.profile,
                            printer = %f.printer,
                            pages = f.pages,
                            status = %f.status,
                            started_at = %f.started_at
                        );
                    }
                    self.converting
                        .insert(st.jobs[idx].id.clone(), flag.clone());
                    to_convert.push((st.jobs[idx].clone(), flag));
                    converting += 1;
                }

                // The slot, not the job states: a cancelled job whose spool is
                // still running, or a printed job still in the OS queue, keeps
                // the printer busy even though no job reads `Printing`.
                if self.print_slot.is_none() {
                    if let Some(idx) = st.jobs.iter().position(|j| {
                        j.state == JobState::Converted && !self.delayed.contains_key(&j.id)
                    }) {
                        let job = &mut st.jobs[idx];
                        job.state = JobState::Printing;
                        if job.started_at.is_none() {
                            job.started_at = Some(now_rfc3339());
                        }
                        // The spooler reports no completion fraction, so the
                        // printing phase is explicitly indeterminate rather than
                        // a bar frozen at 0 %.
                        job.progress = Some(JobProgress::indeterminate("printing"));
                        job.error = None;
                        // A re-print (resume, retry) is a new OS job, with
                        // its own print budget.
                        job.os_job_id = None;
                        job.spooled_at = Some(now_rfc3339());
                        let f = JobLog::of(job);
                        tracing::info!(
                            event = "job_start_print",
                            job_id = %job.id,
                            file = %f.file,
                            profile = %f.profile,
                            printer = %f.printer,
                            pages = f.pages,
                            status = %f.status,
                            started_at = %f.started_at
                        );
                        to_print = Some(st.jobs[idx].clone());
                    }
                }
            }
        }
        for (job, flag) in to_convert {
            self.spawn_convert(job, flag);
        }
        if let Some(job) = to_print {
            self.print_slot = Some(PrintSlot::new(job.id.clone()));
            self.spawn_print(job);
        }
    }

    fn spawn_convert(&self, job: Job, flag: Arc<AtomicBool>) {
        let runner = self.runner.clone();
        let self_tx = self.self_tx.clone();
        let progress = self.make_progress(job.id.clone());
        let id = job.id.clone();
        // The limit in force now applies to this attempt, both to the timer
        // below and to the converter process itself (`set_limits` may have
        // changed it since the queue started).
        let timeout = self.limits.conversion_timeout;
        let password = self.passwords.get(&job.id).cloned();
        // Arm a timeout for THIS attempt, capturing its exact cancel-flag Arc so
        // the handler can tell it apart from a later attempt's flag by identity.
        if let Some(timeout) = timeout {
            let tx = self.self_tx.clone();
            let timer_id = id.clone();
            let timer_flag = flag.clone();
            tokio::spawn(async move {
                tokio::time::sleep(timeout).await;
                let _ = tx.send(Cmd::ConversionTimeout {
                    id: timer_id,
                    flag: timer_flag,
                });
            });
        }
        self.spawner.spawn_blocking(Box::new(move || {
            let result = runner.convert(&job, flag, progress, timeout, password);
            let pages = match &result {
                Ok(Some(pdf)) => converted_page_count(pdf),
                _ => None,
            };
            let _ = self_tx.send(Cmd::ConversionDone { id, result, pages });
        }));
    }

    fn spawn_print(&self, job: Job) {
        let runner = self.runner.clone();
        let self_tx = self.self_tx.clone();
        let id = job.id.clone();
        // The whole print stage is bounded, so a wedged spooler cannot hold the
        // queue's single print slot forever (the conversion stage has had the
        // equivalent bound since the start — see `Cmd::ConversionTimeout`).
        let timeout = self.limits.print_timeout;
        self.spawner.spawn_blocking(Box::new(move || {
            let result = (|| -> Result<PrintOk, PrintError> {
                let printer =
                    runner.resolve_printer(job.settings.printer.as_deref(), &job.file_name)?;
                // What the job leaves open (no tray, paper "auto") comes from
                // the printer's overrides on the Printers screen; explicit
                // values win. The job record keeps what the user chose.
                let mut job = job;
                job.settings = crate::engine::print::apply_printer_overrides(
                    &job.settings,
                    &printer.overrides,
                )
                .into_owned();
                // Pure capability gate (honesty policy): a duplex request against
                // a non-duplex driver, or a paper size the printer does not
                // list, is an error rather than a silent substitution.
                crate::engine::print::check_supported(&job.settings, &printer.name, &printer.caps)?;
                let outcome = runner.print(&job, &printer, timeout)?;
                Ok(PrintOk {
                    printer: printer.name,
                    os_job_id: outcome.os_job_id,
                })
            })();
            let _ = self_tx.send(Cmd::PrintDone { id, result });
        }));
    }

    /// Build the throttled progress callback for a converting job: it updates
    /// `job.progress` in memory (no persist) and emits a `JobProgressEvent`.
    fn make_progress(&self, job_id: String) -> Box<dyn Fn(f32) + Send + Sync> {
        let state = self.state.clone();
        let emit = self.emit.clone();
        let last = self.progress_last.clone();
        Box::new(move |percent: f32| {
            {
                let mut guard = last.lock().unwrap();
                let now = Instant::now();
                match guard.get(&job_id) {
                    Some(prev) if now.duration_since(*prev) < PROGRESS_THROTTLE => return,
                    _ => {
                        guard.insert(job_id.clone(), now);
                    }
                }
            }
            {
                let mut st = state.write().unwrap();
                if let Some(job) = st.jobs.iter_mut().find(|j| j.id == job_id) {
                    if job.state == JobState::Converting {
                        job.progress = Some(JobProgress::measured("converting", percent));
                    }
                }
            }
            emit.job_progress(&job_id, "converting", percent);
        })
    }

    fn commit(&mut self) {
        let snapshot = self.snapshot();
        match crate::queue::persist::save(&self.queue_file, &snapshot) {
            Ok(()) => {
                if self.persist_failing {
                    self.persist_failing = false;
                    tracing::info!(event = "queue_persist_recovered", file = %self.queue_file.display());
                }
            }
            Err(e) => {
                // Durability is silently void while this persists (full disk,
                // permissions); log loudly once per failure streak so the log
                // isn't spammed on every transition but the problem is visible.
                if self.persist_failing {
                    tracing::warn!(event = "queue_persist_failed", error = %e);
                } else {
                    self.persist_failing = true;
                    tracing::error!(
                        event = "queue_persist_failed",
                        file = %self.queue_file.display(),
                        error = %e,
                        "queue.json could not be written — job state will not survive a restart"
                    );
                }
            }
        }
        self.emit.queue_updated(&snapshot);
    }

    fn snapshot(&self) -> QueueSnapshot {
        let st = self.state.read().unwrap();
        QueueSnapshot {
            schema_version: SCHEMA_VERSION,
            queue_paused: st.queue_paused,
            jobs: st.jobs.clone(),
        }
    }
}

/// The run loop: bootstrap recovered jobs, then process commands until every
/// [`QueueHandle`](crate::queue::QueueHandle) sender is dropped.
pub(crate) async fn run(mut dispatcher: Dispatcher, mut rx: mpsc::UnboundedReceiver<Cmd>) {
    dispatcher.bootstrap();
    while let Some(cmd) = rx.recv().await {
        dispatcher.dispatch_one(cmd);
    }
    tracing::info!(event = "queue_dispatcher_stopped");
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

/// Apply cancellation to a single job in place, recording any cache file to
/// delete. `Converting` jobs get their cancel flag set and clean up their
/// partial cache when the conversion reports back; `Printing` jobs reconcile at
/// `PrintDone`.
fn cancel_job_in_place(
    job: &mut Job,
    converting: &HashMap<String, Arc<AtomicBool>>,
    post: &mut Post,
) {
    match job.state {
        JobState::Converting => {
            if let Some(flag) = converting.get(&job.id) {
                flag.store(true, Ordering::Relaxed);
            }
        }
        JobState::Queued | JobState::Converted | JobState::Paused => {
            if let Some(c) = job.cache_pdf.take() {
                post.delete_cache.push(c);
            }
        }
        // Spooling: reconciled at PrintDone. Tracking: `release_tracked_slot`.
        JobState::Printing => {}
        _ => {}
    }
    job.state = JobState::Cancelled;
    job.finished_at = Some(now_rfc3339());
    job.progress = None;
}

/// Decide what resuming a job stuck on its printer should do. `requested` is
/// the job's pinned printer, `file_name` its input (for routing rules).
///
/// The resolution asked for is the *same* one dispatch will do
/// ([`JobRunner::resolve_printer`]), so a resume can never succeed here and
/// then fail there for the same reason. Blocking (it reads the OS printer
/// list), so it runs on the blocking pool — see
/// [`Dispatcher::on_resume_request`].
fn plan_printer_fix(
    runner: &dyn JobRunner,
    requested: Option<String>,
    file_name: &str,
) -> Result<PrinterFix, IpcError> {
    match runner.resolve_printer(requested.as_deref(), file_name) {
        // The printer came back (reinstalled, renamed back, spooler restarted).
        Ok(_) => return Ok(PrinterFix::Unchanged),
        // The printer is still there but cannot be used as named. Neither is
        // a reason to send the job somewhere else: the user picked that
        // printer, so the resume is refused with what to fix.
        Err(PrintError::PrinterAmbiguous { printer, .. }) => {
            return Err(IpcError::localized(
                crate::util::error::IpcErrorCode::InvalidState,
                "resume_printer_ambiguous",
                format!(
                    "several printers are named '{printer}' — choose one on the Printers screen or remove the duplicate, then resume this job"
                ),
            )
            .with_param("printer", &printer));
        }
        Err(PrintError::PrinterDisabled { printer }) => {
            return Err(IpcError::localized(
                crate::util::error::IpcErrorCode::InvalidState,
                "resume_printer_disabled",
                format!(
                    "printer '{printer}' is disabled in Pluryprint — enable it on the Printers screen, then resume this job"
                ),
            )
            .with_param("printer", &printer));
        }
        // Only a printer the OS no longer has (or no printer at all) falls
        // through to the preferences below.
        Err(_) => {}
    }

    let Some(missing) = requested else {
        // Nothing was pinned, so there is simply no printer to print on.
        return Err(IpcError::localized(
            crate::util::error::IpcErrorCode::InvalidState,
            "resume_no_printer",
            "no printer is configured — add one, or set a default on the Printers screen, then resume this job",
        ));
    };

    // The job is pinned to a printer the OS no longer has. Ask what the
    // *preferences* now say for this file (routing rule, then app default,
    // then the OS default) — which is exactly what the Printers screen's
    // "Update references" reconciliation edits.
    match runner.resolve_printer(None, file_name) {
        Ok(ResolvedPrinter {
            name: replacement, ..
        }) => Ok(PrinterFix::Unpin {
            missing,
            replacement,
        }),
        Err(_) => Err(IpcError::localized(
            crate::util::error::IpcErrorCode::InvalidState,
            "resume_printer_missing",
            format!(
                "printer '{missing}' is still not installed and nothing else is configured — reinstall it, or reassign it on the Printers screen, then resume this job"
            ),
        )
        .with_param("printer", &missing)),
    }
}

/// Free the print slot if `job` (just cancelled) was waiting on an OS job:
/// cancel that OS job and drop the cache now. A job still spooling keeps the
/// slot — its OS id is not known yet — and `on_print_done` reconciles it.
fn release_tracked_slot(slot: &mut Option<PrintSlot>, job: &mut Job, post: &mut Post) {
    let Some(s) = slot.as_ref().filter(|s| s.job_id == job.id) else {
        return;
    };
    let (Some(printer), Some(oid)) = (s.printer.clone(), s.os_job_id.clone()) else {
        return;
    };
    post.cancel_os.push((printer, oid));
    if let Some(c) = job.cache_pdf.take() {
        post.delete_cache.push(c);
    }
    *slot = None;
}

/// Mark a printing job `Done`: the printer finished it (`how` says how we
/// know, for the log).
fn mark_done(job: &mut Job, how: &str, post: &mut Post) {
    job.state = JobState::Done;
    job.finished_at = Some(now_rfc3339());
    job.progress = None;
    job.error = None;
    if let Some(c) = job.cache_pdf.take() {
        post.delete_cache.push(c);
    }
    let f = JobLog::of(job);
    tracing::info!(
        event = "job_done",
        job_id = %job.id,
        file = %f.file,
        profile = %f.profile,
        printer = %f.printer,
        pages = f.pages,
        status = %f.status,
        started_at = %f.started_at,
        finished_at = %f.finished_at,
        completion = %how
    );
}

/// End a printing job as [`JobState::Unconfirmed`]: the spooler took it, but
/// nothing confirmed that it printed. `reason` says why not (`left_queue`,
/// `untracked`, `tracking_failed`, `not_verifiable`); it is stored with the
/// job (`print_unconfirmed`) so the UI says so instead of "printed", and it
/// is never retried automatically — paper may already have come out.
fn mark_unconfirmed(job: &mut Job, reason: &str, post: &mut Post) {
    job.state = JobState::Unconfirmed;
    job.finished_at = Some(now_rfc3339());
    job.progress = None;
    job.error = Some(
        JobError::new(
            "print_unconfirmed",
            "the job was handed to the system print queue, but it did not confirm that it printed",
        )
        .with_reason(reason),
    );
    if let Some(c) = job.cache_pdf.take() {
        post.delete_cache.push(c);
    }
    let f = JobLog::of(job);
    tracing::warn!(
        event = "job_unconfirmed",
        job_id = %job.id,
        file = %f.file,
        profile = %f.profile,
        printer = %f.printer,
        pages = f.pages,
        status = %f.status,
        started_at = %f.started_at,
        finished_at = %f.finished_at,
        reason = %reason
    );
}

/// How long ago `job` was handed to the spooler, from its persisted
/// `spooled_at` (else `started_at`, which is earlier, so the budget can only
/// shrink). Zero when neither parses or the clock went backwards.
fn spooled_for(job: &Job) -> Duration {
    use time::format_description::well_known::Rfc3339;
    job.spooled_at
        .as_deref()
        .or(job.started_at.as_deref())
        .and_then(|t| time::OffsetDateTime::parse(t, &Rfc3339).ok())
        .map(|then| time::OffsetDateTime::now_utc() - then)
        .and_then(|d| Duration::try_from(d).ok())
        .unwrap_or_default()
}

/// Apply one tracking answer to a `Printing` job. Returns `true` when the
/// print is over (the slot can be released).
fn settle_tracked(
    job: &mut Job,
    result: &Result<OsJobState, TrackError>,
    failures: u32,
    post: &mut Post,
) -> bool {
    let end = |job: &mut Job, state: JobState, code: &str, message: &str, post: &mut Post| {
        job.state = state;
        job.finished_at = Some(now_rfc3339());
        job.progress = None;
        job.error = Some(JobError::new(code, message));
        if let Some(c) = job.cache_pdf.take() {
            post.delete_cache.push(c);
        }
        let f = JobLog::of(job);
        tracing::warn!(
            event = "job_os_ended",
            job_id = %job.id,
            file = %f.file,
            profile = %f.profile,
            printer = %f.printer,
            pages = f.pages,
            status = %f.status,
            started_at = %f.started_at,
            finished_at = %f.finished_at,
            error = %f.error
        );
    };
    let phase = |job: &mut Job, phase: &str| {
        if job.progress.as_ref().map(|p| p.phase.as_str()) != Some(phase) {
            tracing::info!(event = "job_print_phase", job_id = %job.id, phase = %phase);
        }
        job.progress = Some(JobProgress::indeterminate(phase));
        false
    };
    match result {
        Ok(OsJobState::Completed) => {
            mark_done(job, "completed", post);
            true
        }
        // It left the queue without the spooler saying it printed: deleted in
        // the system queue, purged, or printed with nothing to show for it
        // (on Windows the printer's notification watcher turns a job deleted
        // after `PRINTED` into `Completed`, so this is what remains). Not
        // evidence of a print.
        Ok(OsJobState::Gone) => {
            mark_unconfirmed(job, "left_queue", post);
            true
        }
        Ok(OsJobState::Canceled) => {
            end(
                job,
                JobState::Cancelled,
                "os_cancelled",
                "the job was cancelled in the system print queue",
                post,
            );
            true
        }
        Ok(OsJobState::Aborted) => {
            end(
                job,
                JobState::Failed,
                "print_aborted",
                "the printer aborted the job — check the printer, then retry",
                post,
            );
            true
        }
        Ok(OsJobState::Held) => phase(job, "held"),
        Ok(OsJobState::Stopped) => phase(job, "stopped"),
        Ok(OsJobState::Pending | OsJobState::Processing) => phase(job, "printing"),
        // This host cannot follow jobs at all: the hand-off is all there is.
        Err(TrackError::Unavailable(e)) => {
            tracing::warn!(event = "job_tracking_unavailable", job_id = %job.id, reason = %e);
            mark_unconfirmed(job, "untracked", post);
            true
        }
        Err(TrackError::Transient(e)) if failures >= MAX_TRACK_FAILURES => {
            tracing::warn!(event = "job_tracking_failed", job_id = %job.id, reason = %e, failures);
            mark_unconfirmed(job, "tracking_failed", post);
            true
        }
        Err(TrackError::Transient(_)) => false,
    }
}

/// The print outran its budget while waiting on the OS queue. The OS job is
/// deliberately left alone: the printer may still print it, and cancelling
/// would lose a job that is merely slow. The message says so, so nobody retries
/// into a duplicate without checking.
fn fail_print_timeout(job: &mut Job, budget: Option<Duration>, post: &mut Post) {
    let secs = budget.map(|d| d.as_secs()).unwrap_or(0);
    job.state = JobState::Failed;
    job.finished_at = Some(now_rfc3339());
    job.progress = None;
    job.error = Some(
        JobError::new(
            "print_timeout",
            format!(
                "the printer did not finish within {secs}s — the job is still in the system print queue; check the printer before retrying"
            ),
        )
        .with_param("secs", secs),
    );
    if let Some(c) = job.cache_pdf.take() {
        post.delete_cache.push(c);
    }
    let f = JobLog::of(job);
    tracing::warn!(
        event = "job_print_timeout",
        job_id = %job.id,
        file = %f.file,
        profile = %f.profile,
        printer = %f.printer,
        pages = f.pages,
        status = %f.status,
        started_at = %f.started_at,
        finished_at = %f.finished_at,
        error = %f.error,
        timeout_s = secs
    );
}

/// The FR-035 fields of a job, borrowed for one `tracing` record.
///
/// FR-035 requires every job to be logged with its start time, end time, file,
/// profile, printer, page count, status and error. Those all live on [`Job`]
/// already, so this is purely the rendering: it replaces the `Option`s with the
/// placeholders the log schema uses, so a query for `printer` never has to cope
/// with a missing key. The `tracing` macros still name every field at each call
/// site (they take literals, not a spliced struct); this only guarantees the
/// *values* are derived identically everywhere.
struct JobLog<'a> {
    file: &'a str,
    profile: &'a str,
    printer: &'a str,
    /// Page count, or `-1` when it is not known yet (a numeric field stays
    /// queryable; `"unknown"` in a number column would not).
    pages: i64,
    status: &'a str,
    started_at: &'a str,
    finished_at: &'a str,
    error: &'a str,
}

impl<'a> JobLog<'a> {
    fn of(job: &'a Job) -> Self {
        Self {
            // The basename, never the absolute path: WARN/ERROR records must
            // not carry full paths (NFR-082, data-model.md §5.1).
            file: &job.file_name,
            profile: job.profile_name.as_deref().unwrap_or("<none>"),
            printer: job
                .printer_resolved
                .as_deref()
                .or(job.settings.printer.as_deref())
                .unwrap_or("<default>"),
            pages: job.pages.map(i64::from).unwrap_or(-1),
            status: job.state.as_str(),
            started_at: job.started_at.as_deref().unwrap_or(""),
            finished_at: job.finished_at.as_deref().unwrap_or(""),
            error: job.error.as_ref().map(|e| e.code.as_str()).unwrap_or(""),
        }
    }
}

/// Apply the print-stage failure policy to `job`.
///
/// Two print outcomes are not ordinary transient failures:
///
/// - a **timeout** is terminal, exactly like `conversion_timeout`: the spooler
///   child was killed, and re-spooling would most likely wedge again, so the job
///   is `Failed` with `print_timeout` and no retry; and
/// - a **missing / unconfigured printer** is user-fixable, so the job is
///   `Paused` with `printer_unavailable` for the user to choose a replacement
///   (`docs/03-requirements/print-settings.md` §printer).
///
/// Everything else is an [`EngineError`] and follows the ordinary FR-034 retry
/// policy in [`record_failure`].
fn record_print_failure(job: &mut Job, e: PrintError, post: &mut Post) {
    let (code, terminal) = match &e {
        PrintError::TimedOut { .. } => ("print_timeout", true),
        PrintError::PrinterMissing { .. }
        | PrintError::PrinterAmbiguous { .. }
        | PrintError::NoPrinter => ("printer_unavailable", false),
        PrintError::PrinterDisabled { .. } => ("printer_disabled", false),
        PrintError::Engine(engine) => return record_failure(job, engine, false, post),
    };

    let error = JobError::new(code, e.to_string());
    job.error = Some(match &e {
        PrintError::TimedOut { seconds } => {
            error.with_reason("spooler").with_param("secs", seconds)
        }
        PrintError::PrinterMissing { printer } | PrintError::PrinterDisabled { printer } => {
            error.with_param("printer", printer)
        }
        PrintError::PrinterAmbiguous {
            printer,
            candidates,
        } => error
            .with_reason("ambiguous")
            .with_param("printer", printer)
            .with_param("candidates", candidates.join(", ")),
        PrintError::NoPrinter => error.with_reason("no_printer"),
        PrintError::Engine(_) => error,
    });
    job.progress = None;
    if terminal {
        job.state = JobState::Failed;
        job.finished_at = Some(now_rfc3339());
        if let Some(c) = job.cache_pdf.take() {
            post.delete_cache.push(c);
        }
    } else {
        // Keep the cached PDF: resuming onto another printer must not re-convert.
        job.state = JobState::Paused;
    }

    let f = JobLog::of(job);
    tracing::warn!(
        event = if terminal {
            "job_print_timeout"
        } else {
            "job_paused_needs_fix"
        },
        job_id = %job.id,
        file = %f.file,
        profile = %f.profile,
        printer = %f.printer,
        pages = f.pages,
        status = %f.status,
        started_at = %f.started_at,
        finished_at = %f.finished_at,
        attempts = job.attempts,
        error = %f.error,
        message = %e
    );
}

/// Apply the FR-034 failure policy to `job`, recording any follow-up in `post`.
fn record_failure(job: &mut Job, e: &EngineError, is_conversion: bool, post: &mut Post) {
    match e {
        // User-fixable: pause and wait for the user, no automatic retry. A
        // PDF that needs a (different) password is one of these: the GUI asks
        // for it and resumes the job (`QueueHandle::supply_password`).
        EngineError::UnsupportedSetting { .. } | EngineError::DependencyMissing(_) => {
            pause_needs_fix(job, e);
        }
        EngineError::PdfProtected(p) if p.needs_password() => {
            pause_needs_fix(job, e);
        }
        // The converter itself ran out of time (its own bound, which can fire
        // a moment before the dispatcher's timer does). Terminal, exactly like
        // the dispatcher's `conversion_timeout`: a retry would only burn the
        // same budget again. So are a format the engine cannot print and a
        // PDF that forbids printing or uses unreadable encryption (FR-058,
        // FR-059): retrying never changes the file.
        // A conversion failure whose reason is deterministic (the app missing
        // next to the CLI, a page that names a share or cannot be checked)
        // fails the same way every time: terminal too
        // ([`crate::util::reasons::is_terminal_conversion_reason`]).
        EngineError::ConversionTimedOut(_)
        | EngineError::UnsupportedFormat(_)
        | EngineError::PdfProtected(_) => terminal_failure(job, e, post),
        EngineError::ConversionFailed(f)
            if f.reason
                .is_some_and(crate::util::reasons::is_terminal_conversion_reason) =>
        {
            terminal_failure(job, e, post)
        }
        // Cancellation is handled by the cancel path; defensive here.
        EngineError::Cancelled => {
            job.state = JobState::Cancelled;
            job.finished_at = Some(now_rfc3339());
            job.progress = None;
            if let Some(c) = job.cache_pdf.take() {
                post.delete_cache.push(c);
            }
        }
        _ => retry_or_fail(job, e, is_conversion, post),
    }
}

/// Fail `job` at once with `e`: a retry would fail the same way.
fn terminal_failure(job: &mut Job, e: &EngineError, post: &mut Post) {
    let event = match e {
        EngineError::ConversionTimedOut(_) => "job_conversion_timeout",
        _ => "job_failed_terminal",
    };
    job.state = JobState::Failed;
    job.error = Some(to_job_error(e));
    job.finished_at = Some(now_rfc3339());
    job.progress = None;
    if let Some(c) = job.cache_pdf.take() {
        post.delete_cache.push(c);
    }
    let f = JobLog::of(job);
    tracing::warn!(
        event,
        job_id = %job.id,
        file = %f.file,
        profile = %f.profile,
        printer = %f.printer,
        pages = f.pages,
        status = %f.status,
        started_at = %f.started_at,
        finished_at = %f.finished_at,
        attempts = job.attempts,
        error = %f.error,
        message = %e
    );
}

/// Count one more failed attempt of `job`, and schedule its FR-034 retry or
/// fail it once [`MAX_ATTEMPTS`] is reached.
fn retry_or_fail(job: &mut Job, e: &EngineError, is_conversion: bool, post: &mut Post) {
    job.attempts += 1;
    job.error = Some(to_job_error(e));
    job.progress = None;
    if job.attempts >= MAX_ATTEMPTS {
        job.state = JobState::Failed;
        job.finished_at = Some(now_rfc3339());
        if let Some(c) = job.cache_pdf.take() {
            post.delete_cache.push(c);
        }
        let f = JobLog::of(job);
        tracing::warn!(
            event = "job_failed",
            job_id = %job.id,
            file = %f.file,
            profile = %f.profile,
            printer = %f.printer,
            pages = f.pages,
            status = %f.status,
            started_at = %f.started_at,
            finished_at = %f.finished_at,
            attempts = job.attempts,
            error = %f.error,
            message = %e
        );
    } else {
        job.state = if is_conversion {
            JobState::Queued
        } else {
            JobState::Converted
        };
        let idx = (job.attempts as usize - 1).min(RETRY_BACKOFFS.len() - 1);
        post.retry = Some((job.id.clone(), RETRY_BACKOFFS[idx]));
        let f = JobLog::of(job);
        tracing::warn!(
            event = "job_retry_scheduled",
            job_id = %job.id,
            file = %f.file,
            profile = %f.profile,
            printer = %f.printer,
            pages = f.pages,
            status = %f.status,
            started_at = %f.started_at,
            attempts = job.attempts,
            backoff_ms = RETRY_BACKOFFS[idx].as_millis() as u64,
            error = %f.error,
            message = %e
        );
    }
}

/// The refusal to act on a job that has already finished (FR-092).
fn job_finished(id: &str) -> IpcError {
    IpcError::localized(
        IpcErrorCode::InvalidState,
        "job_finished",
        format!("job {id} has already finished"),
    )
}

/// Whether `job`'s cache PDF is the decrypted copy of a password-protected
/// PDF: a PDF source is printed in place, so the only cache it can have is
/// the copy `convert_to_pdf` decrypted with the user's password (FR-059).
fn holds_decrypted_copy(job: &Job) -> bool {
    job.cache_pdf.is_some()
        && crate::engine::convert::route(&crate::engine::extension_of(Path::new(&job.input_path)))
            == Some(crate::engine::convert::Route::Passthrough)
}

/// Pause `job` on a failure the user has to fix (a setting, a dependency, a
/// PDF password), with no automatic retry.
fn pause_needs_fix(job: &mut Job, e: &EngineError) {
    job.state = JobState::Paused;
    job.error = Some(to_job_error(e));
    job.progress = None;
    let f = JobLog::of(job);
    tracing::warn!(
        event = "job_paused_needs_fix",
        job_id = %job.id,
        file = %f.file,
        profile = %f.profile,
        printer = %f.printer,
        pages = f.pages,
        status = %f.status,
        started_at = %f.started_at,
        finished_at = %f.finished_at,
        attempts = job.attempts,
        error = %f.error,
        message = %e
    );
}

/// Page count of a freshly converted cache PDF, with the same size ceiling as
/// the submit-time probe. `None` when it cannot be measured.
fn converted_page_count(pdf: &Path) -> Option<u32> {
    let bytes = std::fs::metadata(pdf).ok()?.len();
    crate::queue::probe_pages(pdf, "pdf", bytes)
}

/// Map an [`EngineError`] to a persisted [`JobError`] with a stable code, plus
/// the reason and params the UI needs to say it in the user's language
/// (FR-092). `detail` is the tool's own (English) output, interpolated as is.
fn to_job_error(e: &EngineError) -> JobError {
    let code = crate::util::reasons::job_code_of(e);
    let error = JobError::new(code, e.to_string());
    match e {
        EngineError::ConversionFailed(f)
        | EngineError::PrintFailed(f)
        | EngineError::DependencyMissing(f)
        | EngineError::UnsupportedFormat(f) => with_failure(error, f),
        // The converter's own bound fired: its message names the tool and the
        // limit, so it is the detail (the dispatcher's own timer sets `secs`).
        EngineError::ConversionTimedOut(f) => match f.reason {
            Some(_) => with_failure(error, f),
            None => error
                .with_reason("converter")
                .with_param("detail", &f.detail),
        },
        EngineError::PdfProtected(p) => {
            let error = error.with_reason(p.reason());
            match p {
                crate::engine::PdfProtection::Unsupported(d) => error.with_param("detail", d),
                _ => error,
            }
        }
        EngineError::UnsupportedSetting { setting, printer } => {
            let error = error.with_param("printer", printer);
            if setting == "duplex" {
                error.with_reason("duplex")
            } else if let Some(paper) = setting.strip_prefix("paper size ") {
                error.with_reason("paper").with_param("paper", paper)
            } else {
                error.with_param("detail", setting)
            }
        }
        EngineError::Cancelled => error,
    }
}

/// Copy an engine [`crate::engine::Failure`]'s reason and params onto `error`.
/// The English detail always rides along as `params.detail`, the sentence of
/// last resort (and the technical line under a translated one).
fn with_failure(error: JobError, f: &crate::engine::Failure) -> JobError {
    let mut error = match f.reason {
        Some(reason) => error.with_reason(reason),
        None => error,
    };
    for (k, v) in &f.params {
        error = error.with_param(k, v);
    }
    if !f.params.contains_key("detail") {
        error = error.with_param("detail", &f.detail);
    }
    error
}

/// Best-effort cache-file deletion; a missing file is not an error.
fn delete_cache_file(path: &str) {
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            // The cache file's name (`<job id>.pdf`) identifies it; the full path
            // would put the user's profile directory in the log (NFR-082).
            let name = Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            tracing::warn!(event = "cache_delete_failed", file = %name, error = %e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::PdfProtection;
    use crate::queue::job::{Duplex, JobSettings};
    use std::collections::{HashSet, VecDeque};
    use std::sync::atomic::AtomicUsize;
    use tempfile::TempDir;

    // ---- Fakes -----------------------------------------------------------

    enum ConvertStep {
        Produce,
        /// Produce a real, parseable PDF with this many pages.
        ProducePages(u32),
        Passthrough,
        Fail,
        /// The converter process outlived its own bound and was killed.
        TimedOut,
        /// The engine failed with this error (any variant).
        Error(EngineError),
    }

    /// Write a minimal valid PDF with `n` empty pages.
    fn write_n_page_pdf(path: &Path, n: u32) {
        use lopdf::{dictionary, Document, Object};
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let kids: Vec<Object> = (0..n)
            .map(|_| {
                Object::Reference(doc.add_object(dictionary! {
                    "Type" => "Page",
                    "Parent" => Object::Reference(pages_id),
                    "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
                }))
            })
            .collect();
        doc.set_object(
            pages_id,
            dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => i64::from(n) },
        );
        let catalog = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => Object::Reference(pages_id),
        });
        doc.trailer.set("Root", Object::Reference(catalog));
        doc.save(path).unwrap();
    }

    enum PrintStep {
        Ok(Option<String>),
        Fail,
        /// The spooler child outlived its budget and was killed — what the real
        /// engine reports from `wait_with_timeout`.
        Timeout,
    }

    struct FakeRunner {
        cache: PathBuf,
        convert_steps: Mutex<VecDeque<ConvertStep>>,
        print_steps: Mutex<VecDeque<PrintStep>>,
        printer: String,
        duplex_supported: AtomicBool,
        resolve_ok: AtomicBool,
        /// When false, `resolve_printer` reports the requested printer as no
        /// longer installed (the missing-printer path).
        printer_installed: AtomicBool,
        /// Printer names the OS no longer enumerates. Unlike
        /// `printer_installed`, this leaves the *fallback* printer working, so a
        /// test can model "the pinned queue is gone but the app default is
        /// fine" — the situation a paused job has to be able to escape.
        missing_printers: Mutex<HashSet<String>>,
        cancelled_os: Mutex<Vec<String>>,
        convert_calls: AtomicUsize,
        print_calls: AtomicUsize,
        /// Timeouts the fake was asked to enforce, one per `print` call.
        print_timeouts: Mutex<Vec<Option<Duration>>>,
        /// Timeouts handed to each `convert` call.
        convert_timeouts: Mutex<Vec<Option<Duration>>>,
        /// The password handed to each `convert` call, exposed (tests only).
        convert_passwords: Mutex<Vec<Option<String>>>,
        /// The settings the fake was handed for each `print` call, so a test can
        /// assert what would have reached the spooler.
        print_settings: Mutex<Vec<JobSettings>>,
        /// The file name handed to each `resolve_printer` call. A file-type
        /// routing rule can only apply if the dispatcher passes it, so this is
        /// how a test proves the plumbing exists.
        resolve_file_names: Mutex<Vec<String>>,
        /// Paper sizes the fake printer reports (empty: not reported).
        papers: Mutex<Vec<String>>,
        /// Per-printer tray/paper overrides, as the Printers screen stores them.
        overrides: Mutex<HashMap<String, crate::settings::PrinterOverride>>,
        /// Printers disabled in Pluryprint.
        disabled: Mutex<HashSet<String>>,
        /// Names that match several OS queues.
        ambiguous: Mutex<HashSet<String>>,
    }

    impl FakeRunner {
        fn new(cache: PathBuf) -> Self {
            Self {
                cache,
                convert_steps: Mutex::new(VecDeque::new()),
                print_steps: Mutex::new(VecDeque::new()),
                printer: "FakePrinter".to_string(),
                duplex_supported: AtomicBool::new(true),
                resolve_ok: AtomicBool::new(true),
                printer_installed: AtomicBool::new(true),
                missing_printers: Mutex::new(HashSet::new()),
                cancelled_os: Mutex::new(Vec::new()),
                convert_calls: AtomicUsize::new(0),
                print_calls: AtomicUsize::new(0),
                print_timeouts: Mutex::new(Vec::new()),
                convert_timeouts: Mutex::new(Vec::new()),
                convert_passwords: Mutex::new(Vec::new()),
                print_settings: Mutex::new(Vec::new()),
                resolve_file_names: Mutex::new(Vec::new()),
                papers: Mutex::new(Vec::new()),
                overrides: Mutex::new(HashMap::new()),
                disabled: Mutex::new(HashSet::new()),
                ambiguous: Mutex::new(HashSet::new()),
            }
        }
    }

    impl JobRunner for FakeRunner {
        fn convert(
            &self,
            job: &Job,
            cancel: Arc<AtomicBool>,
            progress: Box<dyn Fn(f32) + Send + Sync>,
            timeout: Option<Duration>,
            password: Option<PdfPassword>,
        ) -> Result<Option<PathBuf>, EngineError> {
            self.convert_calls.fetch_add(1, Ordering::Relaxed);
            self.convert_timeouts.lock().unwrap().push(timeout);
            self.convert_passwords
                .lock()
                .unwrap()
                .push(password.map(|p| p.expose().to_string()));
            if cancel.load(Ordering::Relaxed) {
                return Err(EngineError::Cancelled);
            }
            progress(0.5);
            let step = self
                .convert_steps
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(ConvertStep::Produce);
            match step {
                ConvertStep::Produce => {
                    std::fs::create_dir_all(&self.cache).unwrap();
                    let p = self.cache.join(format!("{}.pdf", job.id));
                    std::fs::write(&p, b"%PDF-1.4 fake").unwrap();
                    Ok(Some(p))
                }
                ConvertStep::ProducePages(n) => {
                    std::fs::create_dir_all(&self.cache).unwrap();
                    let p = self.cache.join(format!("{}.pdf", job.id));
                    write_n_page_pdf(&p, n);
                    Ok(Some(p))
                }
                ConvertStep::Passthrough => Ok(None),
                ConvertStep::Fail => Err(EngineError::ConversionFailed("boom".to_string().into())),
                ConvertStep::Error(e) => Err(e),
                ConvertStep::TimedOut => Err(EngineError::ConversionTimedOut(
                    "LibreOffice did not finish within 60s and was stopped"
                        .to_string()
                        .into(),
                )),
            }
        }

        fn print(
            &self,
            job: &Job,
            _printer: &ResolvedPrinter,
            timeout: Option<Duration>,
        ) -> Result<SpoolOutcome, PrintError> {
            self.print_calls.fetch_add(1, Ordering::Relaxed);
            self.print_timeouts.lock().unwrap().push(timeout);
            // Mirror the real runner: the spooler only ever sees the settings
            // that actually apply to this input.
            self.print_settings.lock().unwrap().push(
                crate::engine::print::effective_settings(&job.settings, &job.input_ext)
                    .into_owned(),
            );
            let step = self
                .print_steps
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(PrintStep::Ok(None));
            match step {
                PrintStep::Ok(os_job_id) => Ok(SpoolOutcome { os_job_id }),
                PrintStep::Fail => Err(EngineError::PrintFailed("boom".to_string().into()).into()),
                PrintStep::Timeout => Err(PrintError::TimedOut {
                    seconds: timeout.map(|t| t.as_secs()).unwrap_or(0),
                }),
            }
        }

        fn resolve_printer(
            &self,
            requested: Option<&str>,
            file_name: &str,
        ) -> Result<ResolvedPrinter, PrintError> {
            self.resolve_file_names
                .lock()
                .unwrap()
                .push(file_name.to_string());
            if !self.resolve_ok.load(Ordering::Relaxed) {
                return Err(PrintError::NoPrinter);
            }
            let name = requested
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| self.printer.clone());
            if !self.printer_installed.load(Ordering::Relaxed)
                || self.missing_printers.lock().unwrap().contains(&name)
            {
                return Err(PrintError::PrinterMissing { printer: name });
            }
            if self.disabled.lock().unwrap().contains(&name) {
                return Err(PrintError::PrinterDisabled { printer: name });
            }
            if self.ambiguous.lock().unwrap().contains(&name) {
                return Err(PrintError::PrinterAmbiguous {
                    candidates: vec![format!("{name}_1"), format!("{name}_2")],
                    printer: name,
                });
            }
            Ok(ResolvedPrinter {
                overrides: self
                    .overrides
                    .lock()
                    .unwrap()
                    .get(&name)
                    .cloned()
                    .unwrap_or_default(),
                name,
                caps: PrinterCaps {
                    duplex: self.duplex_supported.load(Ordering::Relaxed),
                    papers: self.papers.lock().unwrap().clone(),
                    color_models: Vec::new(),
                },
            })
        }

        fn cancel_os_job(&self, _printer: &str, os_job_id: &str) {
            self.cancelled_os
                .lock()
                .unwrap()
                .push(os_job_id.to_string());
        }
    }

    /// Scripted spooler: answers each poll from `answers`, then `Completed`.
    #[derive(Default)]
    struct FakeTracker {
        answers: Mutex<VecDeque<Result<OsJobState, TrackError>>>,
        polls: Mutex<Vec<(String, String)>>,
        /// Ids that cannot be told apart from a reused one (a bare Windows id).
        unverifiable: Mutex<HashSet<String>>,
    }

    impl FakeTracker {
        fn script(&self, answers: impl IntoIterator<Item = Result<OsJobState, TrackError>>) {
            self.answers.lock().unwrap().extend(answers);
        }
    }

    impl JobTracker for FakeTracker {
        fn poll(&self, printer: &str, os_job_id: &str) -> Result<OsJobState, TrackError> {
            self.polls
                .lock()
                .unwrap()
                .push((printer.to_string(), os_job_id.to_string()));
            self.answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(OsJobState::Completed))
        }

        fn verifiable(&self, os_job_id: &str) -> bool {
            !self.unverifiable.lock().unwrap().contains(os_job_id)
        }
    }

    #[derive(Default)]
    struct FakeEmit {
        snapshots: Mutex<Vec<QueueSnapshot>>,
        progress: Mutex<Vec<(String, f32)>>,
    }

    impl EventSink for FakeEmit {
        fn queue_updated(&self, snapshot: &QueueSnapshot) {
            self.snapshots.lock().unwrap().push(snapshot.clone());
        }
        fn job_progress(&self, id: &str, _phase: &str, percent: f32) {
            self.progress
                .lock()
                .unwrap()
                .push((id.to_string(), percent));
        }
    }

    struct InlineSpawner;
    impl BlockingSpawner for InlineSpawner {
        fn spawn_blocking(&self, task: Box<dyn FnOnce() + Send + 'static>) {
            task();
        }
    }

    /// A boxed blocking task, as handed to [`BlockingSpawner::spawn_blocking`].
    type BoxedTask = Box<dyn FnOnce() + Send + 'static>;

    /// A spawner that parks blocking tasks instead of running them, so a test
    /// can keep a conversion "in flight" (job stuck in `Converting`) while it
    /// advances the virtual clock to trigger the conversion-timeout timer.
    #[derive(Default, Clone)]
    struct DeferredSpawner {
        tasks: Arc<Mutex<VecDeque<BoxedTask>>>,
    }

    impl BlockingSpawner for DeferredSpawner {
        fn spawn_blocking(&self, task: Box<dyn FnOnce() + Send + 'static>) {
            self.tasks.lock().unwrap().push_back(task);
        }
    }

    impl DeferredSpawner {
        /// Run the oldest parked task (e.g. the aborted conversion reporting
        /// back). Returns `false` when none are parked.
        fn run_next(&self) -> bool {
            let task = self.tasks.lock().unwrap().pop_front();
            match task {
                Some(t) => {
                    t();
                    true
                }
                None => false,
            }
        }
    }

    /// Where a print the fake spooler took ends by default. The fake returns
    /// no spooler id unless a test scripts one (`spool_with_ids`), and a print
    /// nobody can follow is never reported as printed: it ends `Unconfirmed`.
    const HANDED_OFF: JobState = JobState::Unconfirmed;

    // ---- Harness ---------------------------------------------------------

    struct Harness {
        d: Dispatcher,
        rx: mpsc::UnboundedReceiver<Cmd>,
        tx: mpsc::UnboundedSender<Cmd>,
        runner: Arc<FakeRunner>,
        tracker: Arc<FakeTracker>,
        emit: Arc<FakeEmit>,
        state: Arc<RwLock<QueueState>>,
        queue_file: PathBuf,
        _tmp: TempDir,
    }

    /// Default harness: inline spawner, default limits (concurrency 2, no
    /// conversion timeout).
    fn harness() -> Harness {
        harness_with(Arc::new(InlineSpawner), DispatcherLimits::default())
    }

    /// Harness with an injected spawner and explicit limits, for the
    /// concurrency-cap and conversion-timeout tests.
    fn harness_with(spawner: Arc<dyn BlockingSpawner>, limits: DispatcherLimits) -> Harness {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("conversions");
        let queue_file = tmp.path().join("queue.json");
        let (tx, rx) = mpsc::unbounded_channel();
        let runner = Arc::new(FakeRunner::new(cache));
        let tracker = Arc::new(FakeTracker::default());
        let emit = Arc::new(FakeEmit::default());
        let state = Arc::new(RwLock::new(QueueState {
            queue_paused: false,
            jobs: Vec::new(),
        }));
        let d = Dispatcher::new(
            state.clone(),
            tx.clone(),
            runner.clone(),
            emit.clone(),
            spawner,
            tracker.clone(),
            queue_file.clone(),
            limits,
        );
        Harness {
            d,
            rx,
            tx,
            runner,
            tracker,
            emit,
            state,
            queue_file,
            _tmp: tmp,
        }
    }

    impl Harness {
        fn step(&mut self) -> bool {
            match self.rx.try_recv() {
                Ok(cmd) => {
                    self.d.dispatch_one(cmd);
                    true
                }
                Err(_) => false,
            }
        }

        fn run_to_idle(&mut self) {
            while self.step() {}
        }

        fn submit(&mut self, jobs: Vec<Job>) {
            self.tx.send(Cmd::Submit { jobs }).unwrap();
        }

        fn jobs(&self) -> Vec<Job> {
            self.state.read().unwrap().jobs.clone()
        }

        fn job(&self, id: &str) -> Job {
            self.jobs().into_iter().find(|j| j.id == id).unwrap()
        }

        fn count(&self, s: JobState) -> usize {
            self.jobs().iter().filter(|j| j.state == s).count()
        }

        /// Send a command carrying a oneshot reply, then pump the loop until the
        /// reply arrives.
        async fn request<T>(&mut self, make: impl FnOnce(oneshot::Sender<T>) -> Cmd) -> T {
            let (rtx, mut rrx) = oneshot::channel();
            self.tx.send(make(rtx)).unwrap();
            loop {
                match rrx.try_recv() {
                    Ok(v) => return v,
                    Err(oneshot::error::TryRecvError::Empty) => {
                        if !self.step() {
                            tokio::task::yield_now().await;
                        }
                    }
                    Err(oneshot::error::TryRecvError::Closed) => panic!("reply dropped"),
                }
            }
        }

        /// Advance the (paused) virtual clock by `dur` and process anything that
        /// falls due. The leading yields let a freshly-spawned retry timer poll
        /// its `sleep` and register its deadline *before* the clock moves —
        /// otherwise advancing would miss a timer that hasn't been polled yet.
        async fn advance_and_settle(&mut self, dur: Duration) {
            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
            tokio::time::advance(dur).await;
            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
            self.run_to_idle();
        }
    }

    fn make_job(name: &str) -> Job {
        Job::new(
            "batch",
            format!("/tmp/{name}"),
            JobSettings::default(),
            None,
            now_rfc3339(),
        )
    }

    // ---- Tests -----------------------------------------------------------

    #[tokio::test]
    async fn happy_path_converts_prints_and_deletes_cache() {
        let mut h = harness();
        let j = make_job("a.pdf");
        let id = j.id.clone();
        let cache_path = h.runner.cache.join(format!("{id}.pdf"));
        h.submit(vec![j]);
        h.run_to_idle();

        assert_eq!(h.job(&id).state, HANDED_OFF);
        assert!(h.job(&id).cache_pdf.is_none(), "cache_pdf cleared on Done");
        assert!(!cache_path.exists(), "cache file deleted on Done");
        assert!(h.job(&id).finished_at.is_some());
        assert!(h.job(&id).started_at.is_some());
        assert_eq!(h.runner.print_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn fr_053_converted_document_reports_the_page_count_of_its_pdf() {
        let mut h = harness();
        let j = make_job("report.docx");
        assert_eq!(
            j.pages, None,
            "a document's pages are unknown before conversion"
        );
        let id = j.id.clone();
        h.runner
            .convert_steps
            .lock()
            .unwrap()
            .push_back(ConvertStep::ProducePages(3));
        h.submit(vec![j]);
        h.run_to_idle();

        let job = h.job(&id);
        assert_eq!(job.state, HANDED_OFF);
        assert_eq!(job.pages, Some(3));
    }

    #[tokio::test]
    async fn two_conversions_start_and_third_waits() {
        let mut h = harness();
        let jobs: Vec<Job> = (0..3).map(|i| make_job(&format!("f{i}.pdf"))).collect();
        let ids: Vec<String> = jobs.iter().map(|j| j.id.clone()).collect();
        h.submit(jobs);
        h.step(); // process Submit only

        assert_eq!(h.count(JobState::Converting), 2);
        assert_eq!(h.count(JobState::Queued), 1);
        assert_eq!(h.job(&ids[2]).state, JobState::Queued);

        h.run_to_idle();
        assert!(ids.iter().all(|id| h.job(id).state == HANDED_OFF));
    }

    #[tokio::test]
    async fn never_more_than_one_print_at_a_time() {
        let mut h = harness();
        let jobs: Vec<Job> = (0..3).map(|i| make_job(&format!("f{i}.pdf"))).collect();
        h.submit(jobs);
        h.run_to_idle();

        let snapshots = h.emit.snapshots.lock().unwrap();
        for s in snapshots.iter() {
            let printing = s
                .jobs
                .iter()
                .filter(|j| j.state == JobState::Printing)
                .count();
            assert!(printing <= 1, "at most one printing at a time");
            let converting = s
                .jobs
                .iter()
                .filter(|j| j.state == JobState::Converting)
                .count();
            assert!(converting <= DEFAULT_MAX_CONCURRENT_CONVERSIONS);
        }
    }

    #[tokio::test]
    async fn passthrough_pdf_prints_input_in_place() {
        let mut h = harness();
        h.runner
            .convert_steps
            .lock()
            .unwrap()
            .push_back(ConvertStep::Passthrough);
        let j = make_job("a.pdf");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();
        assert_eq!(h.job(&id).state, HANDED_OFF);
        assert_eq!(h.runner.print_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn transient_conversion_failure_retries_three_times_then_fails() {
        let mut h = harness();
        {
            let mut steps = h.runner.convert_steps.lock().unwrap();
            for _ in 0..4 {
                steps.push_back(ConvertStep::Fail);
            }
        }
        let j = make_job("a.pdf");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();

        // First failure: back to Queued, attempts=1, 1s backoff pending.
        assert_eq!(h.job(&id).state, JobState::Queued);
        assert_eq!(h.job(&id).attempts, 1);
        assert!(h.job(&id).error.is_some());

        // FR-034: three automatic retries at 1s/5s/25s, then permanent failure.
        h.advance_and_settle(Duration::from_secs(1)).await;
        assert_eq!(h.job(&id).attempts, 2);

        h.advance_and_settle(Duration::from_secs(5)).await;
        assert_eq!(h.job(&id).attempts, 3);
        assert_eq!(h.job(&id).state, JobState::Queued, "third retry pending");

        h.advance_and_settle(Duration::from_secs(25)).await;
        assert_eq!(h.job(&id).state, JobState::Failed);
        assert_eq!(h.job(&id).attempts, 4);
        assert_eq!(h.runner.convert_calls.load(Ordering::Relaxed), 4);
    }

    /// FR-034: an old retry timer must not end a later backoff. Pausing and
    /// resuming a job that waits out its 1 s backoff runs it at once; when
    /// that run fails too, the job waits 5 s — and the first timer, still
    /// pending, used to release it after about 1 s.
    #[tokio::test(start_paused = true)]
    async fn fr_034_a_stale_retry_timer_does_not_cut_a_later_backoff_short() {
        let mut h = harness();
        {
            let mut steps = h.runner.convert_steps.lock().unwrap();
            for _ in 0..3 {
                steps.push_back(ConvertStep::Fail);
            }
        }
        let j = make_job("a.docx");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();
        assert_eq!(h.job(&id).attempts, 1, "first failure, 1 s backoff armed");
        // Let the 1 s timer register its deadline before anything else runs.
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        // Pause + resume: the job runs straight away and fails again, which
        // arms a 5 s backoff while the 1 s timer is still pending.
        h.request(|reply| Cmd::Pause {
            id: id.clone(),
            reply,
        })
        .await
        .unwrap();
        h.request(|reply| Cmd::Resume {
            id: id.clone(),
            reply,
        })
        .await
        .unwrap();
        h.run_to_idle();
        assert_eq!(h.job(&id).attempts, 2, "second failure, 5 s backoff armed");

        // The stale 1 s timer fires: the job keeps waiting.
        h.advance_and_settle(Duration::from_millis(1500)).await;
        assert_eq!(
            h.job(&id).attempts,
            2,
            "the 5 s backoff must not end after 1 s"
        );
        assert_eq!(h.job(&id).state, JobState::Queued);

        // Its own timer does release it.
        h.advance_and_settle(Duration::from_secs(4)).await;
        assert_eq!(h.job(&id).attempts, 3);
    }

    #[tokio::test]
    async fn resume_during_inflight_conversion_does_not_double_start() {
        // Pause a Converting job, resume it before the aborted conversion
        // reports back: the scheduler must NOT start a second conversion for
        // the same id (both would write the same cache path) until the stale
        // ConversionDone arrives.
        let mut h = harness();
        // Use a deferring spawner? The inline spawner completes conversions
        // synchronously, so simulate the in-flight window by hand: put the job
        // into Converting with a registered flag, as schedule() would.
        let mut j = make_job("a.pdf");
        let id = j.id.clone();
        j.state = JobState::Converting;
        h.state.write().unwrap().jobs.push(j);
        let flag = Arc::new(AtomicBool::new(false));
        h.d.converting.insert(id.clone(), flag.clone());

        // Pause sets the cancel flag and parks the job.
        let r = h
            .request(|reply| Cmd::Pause {
                id: id.clone(),
                reply,
            })
            .await;
        assert!(r.is_ok());
        assert!(flag.load(Ordering::Relaxed), "cancel flag set on pause");
        assert_eq!(h.job(&id).state, JobState::Paused);

        // Resume before the stale ConversionDone: job is Queued but must be
        // skipped by the scheduler while its old conversion is unaccounted for.
        let r = h
            .request(|reply| Cmd::Resume {
                id: id.clone(),
                reply,
            })
            .await;
        assert!(r.is_ok());
        assert_eq!(h.job(&id).state, JobState::Queued);
        assert_eq!(
            h.runner.convert_calls.load(Ordering::Relaxed),
            0,
            "no second conversion while the first is in flight"
        );

        // The stale completion arrives (aborted → Cancelled): the id is
        // released and the scheduler immediately starts a fresh conversion,
        // which runs to Done.
        h.tx.send(Cmd::ConversionDone {
            id: id.clone(),
            result: Err(EngineError::Cancelled),
            pages: None,
        })
        .unwrap();
        h.run_to_idle();
        assert_eq!(h.job(&id).state, HANDED_OFF);
        assert_eq!(h.runner.convert_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn transient_print_failure_retries_and_keeps_cache() {
        let mut h = harness();
        {
            let mut steps = h.runner.print_steps.lock().unwrap();
            steps.push_back(PrintStep::Fail); // first print fails
            steps.push_back(PrintStep::Ok(None)); // retry succeeds
        }
        let j = make_job("a.pdf");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();

        // Print failure returns the job to Converted (not Queued) and keeps the
        // cached PDF so the retry does not re-convert.
        assert_eq!(h.job(&id).state, JobState::Converted);
        assert_eq!(h.job(&id).attempts, 1);
        let cache = h.job(&id).cache_pdf.clone().unwrap();
        assert!(Path::new(&cache).exists(), "cache retained for print retry");

        h.advance_and_settle(Duration::from_secs(1)).await;
        assert_eq!(h.job(&id).state, HANDED_OFF);
        assert_eq!(h.runner.print_calls.load(Ordering::Relaxed), 2);
        assert_eq!(
            h.runner.convert_calls.load(Ordering::Relaxed),
            1,
            "not re-converted on print retry"
        );
    }

    fn push_error(h: &Harness, e: EngineError) {
        h.runner
            .convert_steps
            .lock()
            .unwrap()
            .push_back(ConvertStep::Error(e));
    }

    fn password(p: &str) -> PdfPassword {
        PdfPassword::new(p.to_string()).unwrap()
    }

    /// FR-058: a file whose format the engine cannot print fails at once.
    /// Retrying never changes a file's format, so it used to burn three
    /// pointless retries first.
    #[tokio::test]
    async fn fr_058_unsupported_format_is_terminal() {
        let mut h = harness();
        push_error(&h, EngineError::UnsupportedFormat("exe".into()));
        let j = make_job("setup.exe");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();

        let job = h.job(&id);
        assert_eq!(job.state, JobState::Failed);
        assert_eq!(job.error.unwrap().code, "unsupported_format");
        assert_eq!(h.runner.convert_calls.load(Ordering::Relaxed), 1);
        assert!(!h.d.delayed.contains_key(&id), "no retry is scheduled");
    }

    /// FR-034: a conversion failure with a deterministic reason (the app
    /// missing next to the CLI, a page that names a share or cannot be
    /// checked) fails at once; it used to burn the whole retry chain first.
    /// A failure with any other reason is still retried.
    #[tokio::test]
    async fn fr_034_deterministic_conversion_failures_are_terminal() {
        for reason in crate::util::reasons::TERMINAL_CONVERSION_REASONS {
            let mut h = harness();
            push_error(
                &h,
                EngineError::ConversionFailed(crate::engine::Failure::new(reason, "x")),
            );
            let j = make_job("page.html");
            let id = j.id.clone();
            h.submit(vec![j]);
            h.run_to_idle();

            let job = h.job(&id);
            assert_eq!(job.state, JobState::Failed, "{reason}");
            let err = job.error.unwrap();
            assert_eq!(err.code, "conversion_failed");
            assert_eq!(err.reason.as_deref(), Some(*reason));
            assert_eq!(h.runner.convert_calls.load(Ordering::Relaxed), 1);
            assert!(!h.d.delayed.contains_key(&id), "{reason}: no retry");
        }

        let mut h = harness();
        push_error(
            &h,
            EngineError::ConversionFailed(crate::engine::Failure::new("render_failed", "x")),
        );
        let j = make_job("page.html");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();
        assert_eq!(h.job(&id).attempts, 1);
        assert!(h.d.delayed.contains_key(&id), "a render failure is retried");
    }

    /// FR-059: a PDF that forbids printing, or whose encryption cannot be
    /// read, fails at once with its reason; no retry and no password changes
    /// that.
    #[tokio::test]
    async fn fr_059_printing_not_allowed_is_terminal() {
        for (protection, reason) in [
            (PdfProtection::PrintingNotAllowed, "printing_not_allowed"),
            (
                PdfProtection::Unsupported("V9".into()),
                "unsupported_encryption",
            ),
        ] {
            let mut h = harness();
            push_error(&h, EngineError::PdfProtected(protection));
            let j = make_job("locked.pdf");
            let id = j.id.clone();
            h.submit(vec![j]);
            h.run_to_idle();

            let job = h.job(&id);
            assert_eq!(job.state, JobState::Failed);
            assert!(!job.needs_password());
            let err = job.error.unwrap();
            assert_eq!(err.code, "pdf_protected");
            assert_eq!(err.reason.as_deref(), Some(reason));
            assert_eq!(h.runner.convert_calls.load(Ordering::Relaxed), 1);
        }
    }

    /// FR-059: a PDF that needs a password pauses (no retry, never "no
    /// pages") in the state the GUI acts on; the password the user then
    /// supplies reaches the converter and the job resumes. It lives in memory
    /// only: once used it is forgotten.
    #[tokio::test]
    async fn fr_059_password_required_pauses_until_a_password_is_supplied() {
        let mut h = harness();
        push_error(
            &h,
            EngineError::PdfProtected(PdfProtection::PasswordRequired),
        );
        let j = make_job("secret.pdf");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();

        let job = h.job(&id);
        assert_eq!(job.state, JobState::Paused);
        assert!(job.needs_password(), "{:?}", job.error);
        assert_eq!(job.attempts, 0, "no retry for a missing password");
        assert_eq!(
            job.error.as_ref().unwrap().reason.as_deref(),
            Some("password_required")
        );

        let pid = id.clone();
        h.request(|reply| Cmd::SupplyPassword {
            id: pid,
            password: password("hunter2"),
            apply_to_batch: false,
            reply,
        })
        .await
        .unwrap();
        h.run_to_idle();

        assert_eq!(h.job(&id).state, HANDED_OFF);
        assert_eq!(
            *h.runner.convert_passwords.lock().unwrap(),
            vec![None, Some("hunter2".to_string())]
        );
        assert!(h.d.passwords.is_empty(), "the password is not kept");
        let persisted = std::fs::read_to_string(&h.d.queue_file).unwrap_or_default();
        assert!(
            !persisted.contains("hunter2"),
            "never written to queue.json"
        );
    }

    /// FR-059: a wrong password pauses the job again, asking anew, and the
    /// wrong password is forgotten.
    #[tokio::test]
    async fn fr_059_a_wrong_password_asks_again() {
        let mut h = harness();
        push_error(
            &h,
            EngineError::PdfProtected(PdfProtection::PasswordRequired),
        );
        push_error(
            &h,
            EngineError::PdfProtected(PdfProtection::PasswordIncorrect),
        );
        let j = make_job("secret.pdf");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();
        let pid = id.clone();
        h.request(|reply| Cmd::SupplyPassword {
            id: pid,
            password: password("wrong"),
            apply_to_batch: false,
            reply,
        })
        .await
        .unwrap();
        h.run_to_idle();

        let job = h.job(&id);
        assert_eq!(job.state, JobState::Paused);
        assert!(job.needs_password());
        assert_eq!(
            job.error.unwrap().reason.as_deref(),
            Some("password_incorrect")
        );
        assert!(h.d.passwords.is_empty());
    }

    /// FR-059: only a job that is waiting for a password takes one, and
    /// "apply to the batch" resumes the batch's other waiting PDFs with it.
    #[tokio::test]
    async fn fr_059_supply_password_checks_state_and_applies_to_the_batch() {
        let mut h = harness();
        let plain = make_job("plain.pdf");
        let plain_id = plain.id.clone();
        h.submit(vec![plain]);
        h.run_to_idle();
        let pid = plain_id.clone();
        let err = h
            .request(|reply| Cmd::SupplyPassword {
                id: pid,
                password: password("x"),
                apply_to_batch: false,
                reply,
            })
            .await
            .unwrap_err();
        assert_eq!(err.reason(), Some("job_not_waiting_password"));
        let missing = h
            .request(|reply| Cmd::SupplyPassword {
                id: "nope".into(),
                password: password("x"),
                apply_to_batch: false,
                reply,
            })
            .await
            .unwrap_err();
        assert!(matches!(missing, IpcError::JobNotFound(_)));

        push_error(
            &h,
            EngineError::PdfProtected(PdfProtection::PasswordRequired),
        );
        push_error(
            &h,
            EngineError::PdfProtected(PdfProtection::PasswordRequired),
        );
        let a = make_job("a.pdf");
        let b = make_job("b.pdf");
        let (a_id, b_id) = (a.id.clone(), b.id.clone());
        h.submit(vec![a, b]);
        h.run_to_idle();
        assert!(h.job(&a_id).needs_password() && h.job(&b_id).needs_password());

        let aid = a_id.clone();
        h.request(|reply| Cmd::SupplyPassword {
            id: aid,
            password: password("same"),
            apply_to_batch: true,
            reply,
        })
        .await
        .unwrap();
        h.run_to_idle();
        assert_eq!(h.job(&a_id).state, HANDED_OFF);
        assert_eq!(h.job(&b_id).state, HANDED_OFF);
    }

    /// FR-092: an engine failure's stable reason and params reach the job
    /// error, with the English detail kept as the fallback.
    #[test]
    fn fr_092_engine_failure_reason_reaches_the_job_error() {
        let e = to_job_error(&EngineError::ConversionFailed(
            crate::engine::Failure::new("cannot_read_input", "cannot open a.pdf: denied")
                .param("file", "a.pdf"),
        ));
        assert_eq!(e.code, "conversion_failed");
        assert_eq!(e.reason.as_deref(), Some("cannot_read_input"));
        assert_eq!(
            params(&e),
            [("detail", "cannot open a.pdf: denied"), ("file", "a.pdf")]
        );

        let e = to_job_error(&EngineError::PdfProtected(PdfProtection::PasswordRequired));
        assert_eq!(e.code, "pdf_protected");
        assert_eq!(e.reason.as_deref(), Some("password_required"));
    }

    #[tokio::test]
    async fn unsupported_setting_pauses_without_retry() {
        let mut h = harness();
        h.runner.duplex_supported.store(false, Ordering::Relaxed);
        let mut j = make_job("a.pdf");
        j.settings.duplex = Duplex::Long;
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();

        assert_eq!(h.job(&id).state, JobState::Paused);
        assert_eq!(h.job(&id).attempts, 0, "no retry for a user-fixable error");
        let err = h.job(&id).error.unwrap();
        assert_eq!(err.code, "unsupported_setting");
        assert_eq!(
            h.runner.print_calls.load(Ordering::Relaxed),
            0,
            "never spooled"
        );
    }

    /// FR-040: a job paused because its printer cannot print two-sided can be
    /// switched to one-sided and resumed — it no longer loops back into the
    /// same pause, and nothing has to be cancelled and printed again.
    #[tokio::test]
    async fn fr_040_a_paused_duplex_job_can_switch_to_one_sided_and_print() {
        let mut h = harness();
        h.runner.duplex_supported.store(false, Ordering::Relaxed);
        let mut j = make_job("a.pdf");
        j.settings.duplex = Duplex::Long;
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();
        assert_eq!(h.job(&id).state, JobState::Paused);

        let change = JobChange {
            duplex: Some(Duplex::None),
            ..JobChange::default()
        };
        h.request(|reply| Cmd::Adjust {
            id: id.clone(),
            change,
            reply,
        })
        .await
        .unwrap();
        assert_eq!(h.job(&id).state, JobState::Paused, "changed, not sent");
        assert_eq!(h.job(&id).settings.duplex, Duplex::None);

        h.request(|reply| Cmd::Resume {
            id: id.clone(),
            reply,
        })
        .await
        .unwrap();
        h.run_to_idle();
        assert_eq!(h.job(&id).state, HANDED_OFF);
        let sent = h.runner.print_settings.lock().unwrap();
        assert_eq!(sent.last().unwrap().duplex, Duplex::None);
    }

    /// FR-044: a job paused on a printer removed from Pluryprint can be sent
    /// to another printer instead; the old resolution is forgotten.
    #[tokio::test]
    async fn fr_044_a_paused_job_can_move_to_another_printer() {
        let mut h = harness();
        h.runner
            .disabled
            .lock()
            .unwrap()
            .insert("Office".to_string());
        let mut j = make_job("a.pdf");
        j.settings.printer = Some("Office".to_string());
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();
        assert_eq!(h.job(&id).state, JobState::Paused);
        assert_eq!(h.job(&id).error.unwrap().code, "printer_disabled");

        let change = JobChange {
            printer: Some("Front".to_string()),
            ..JobChange::default()
        };
        h.request(|reply| Cmd::Adjust {
            id: id.clone(),
            change,
            reply,
        })
        .await
        .unwrap();
        assert_eq!(h.job(&id).settings.printer.as_deref(), Some("Front"));
        assert!(h.job(&id).printer_resolved.is_none());
        h.request(|reply| Cmd::Resume {
            id: id.clone(),
            reply,
        })
        .await
        .unwrap();
        h.run_to_idle();
        assert_eq!(h.job(&id).state, HANDED_OFF);
        assert_eq!(h.job(&id).printer_resolved.as_deref(), Some("Front"));
    }

    /// Only a paused job is changed: one already printing or finished is not
    /// silently rewritten.
    #[tokio::test]
    async fn fr_040_only_a_paused_job_can_be_changed() {
        let mut h = harness();
        let j = make_job("a.pdf");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();
        let r = h
            .request(|reply| Cmd::Adjust {
                id: id.clone(),
                change: JobChange::default(),
                reply,
            })
            .await;
        assert!(matches!(r, Err(IpcError::InvalidState(_))));
        let r = h
            .request(|reply| Cmd::Adjust {
                id: "nope".to_string(),
                change: JobChange::default(),
                reply,
            })
            .await;
        assert!(matches!(r, Err(IpcError::JobNotFound(_))));
    }

    /// NFR-010 / FR-032: a job whose OS job was still in the printer queue
    /// when the app closed is followed to its end after the restart, not
    /// paused and printed a second time on resume.
    #[tokio::test(start_paused = true)]
    async fn nfr_010_a_print_in_the_os_queue_at_shutdown_is_tracked_not_reprinted() {
        let mut h = harness();
        let mut j = make_job("a.pdf");
        j.state = JobState::Printing;
        j.os_job_id = Some("P-7".to_string());
        j.printer_resolved = Some("FakePrinter".to_string());
        let id = j.id.clone();
        let recovered = crate::queue::persist::recover(QueueSnapshot {
            jobs: vec![j, make_job("b.pdf")],
            ..QueueSnapshot::default()
        });
        h.state.write().unwrap().jobs = recovered.jobs;
        h.tracker
            .script([Ok(OsJobState::Processing), Ok(OsJobState::Completed)]);

        h.d.bootstrap();
        h.run_to_idle();
        assert_eq!(h.job(&id).state, JobState::Printing);
        assert_eq!(
            h.runner.print_calls.load(Ordering::Relaxed),
            0,
            "the next job waits: the slot belongs to the recovered print"
        );

        h.advance_and_settle(Duration::from_secs(1)).await;
        h.advance_and_settle(Duration::from_secs(2)).await;
        assert_eq!(h.job(&id).state, JobState::Done);
        assert_eq!(
            h.tracker.polls.lock().unwrap()[0],
            ("FakePrinter".to_string(), "P-7".to_string())
        );
        h.run_to_idle();
        // Only the *other* job was spooled; the recovered one never again.
        assert_eq!(h.runner.print_calls.load(Ordering::Relaxed), 1);
    }

    /// FR-059 / NFR-082: the decrypted copy of a password-protected PDF does
    /// not survive a restart. A job left paused (printer offline) or
    /// converted with it asks for the password again instead of printing the
    /// plaintext copy; the file is deleted and queue.json no longer names it.
    #[tokio::test(start_paused = true)]
    async fn fr_059_decrypted_copy_never_survives_a_restart() {
        let mut h = harness();
        std::fs::create_dir_all(&h.runner.cache).unwrap();
        let mut jobs = Vec::new();
        let mut caches = Vec::new();
        for (name, state) in [
            ("paused.pdf", JobState::Paused),
            ("converted.pdf", JobState::Converted),
            ("queued.PDF", JobState::Queued),
        ] {
            let mut j = make_job(name);
            let cache = h.runner.cache.join(format!("{}.pdf", j.id));
            std::fs::write(&cache, b"%PDF plaintext").unwrap();
            j.state = state;
            j.cache_pdf = Some(cache.to_string_lossy().into_owned());
            if state == JobState::Paused {
                j.error = Some(JobError::new("printer_unavailable", "offline"));
            }
            caches.push(cache);
            jobs.push(j);
        }
        // A converted document (not a PDF) keeps its cache.
        let mut doc = make_job("letter.docx");
        let doc_cache = h.runner.cache.join(format!("{}.pdf", doc.id));
        std::fs::write(&doc_cache, b"%PDF converted").unwrap();
        doc.state = JobState::Converted;
        doc.cache_pdf = Some(doc_cache.to_string_lossy().into_owned());
        let doc_id = doc.id.clone();
        jobs.push(doc);
        let ids: Vec<String> = jobs.iter().take(3).map(|j| j.id.clone()).collect();

        // The restart: what was on disk is recovered and the dispatcher boots.
        let recovered = crate::queue::persist::recover(QueueSnapshot {
            jobs,
            queue_paused: true,
            ..QueueSnapshot::default()
        });
        h.state.write().unwrap().jobs = recovered.jobs;
        h.state.write().unwrap().queue_paused = true;
        h.d.bootstrap();
        h.run_to_idle();

        for (id, cache) in ids.iter().zip(&caches) {
            let job = h.job(id);
            assert!(job.cache_pdf.is_none(), "{id}");
            assert!(!cache.exists(), "the plaintext copy is deleted");
            assert!(job.needs_password(), "{:?}", job.error);
        }
        assert_eq!(h.runner.print_calls.load(Ordering::Relaxed), 0);
        assert_eq!(
            h.job(&doc_id).cache_pdf.as_deref(),
            Some(doc_cache.to_string_lossy().as_ref())
        );
        assert!(doc_cache.exists());
        let persisted = std::fs::read_to_string(&h.d.queue_file).unwrap();
        for cache in &caches {
            let name = cache.file_name().unwrap().to_string_lossy().into_owned();
            assert!(!persisted.contains(&name), "queue.json still names {name}");
        }
    }

    #[tokio::test]
    async fn resume_of_interrupted_shutdown_reenters_pipeline() {
        let mut h = harness();
        let mut j = make_job("a.pdf");
        let id = j.id.clone();
        std::fs::create_dir_all(&h.runner.cache).unwrap();
        let cache_file = h.runner.cache.join(format!("{id}.pdf"));
        std::fs::write(&cache_file, b"%PDF fake").unwrap();
        j.state = JobState::Paused;
        j.cache_pdf = Some(cache_file.to_string_lossy().into_owned());
        j.error = Some(JobError::new(
            "interrupted_by_shutdown",
            "closed mid-flight",
        ));
        j.attempts = 2;
        h.state.write().unwrap().jobs.push(j);

        let r = h
            .request(|reply| Cmd::Resume {
                id: id.clone(),
                reply,
            })
            .await;
        assert!(r.is_ok());
        h.run_to_idle();

        assert_eq!(h.job(&id).state, HANDED_OFF);
        assert_eq!(h.job(&id).attempts, 0, "shutdown resume resets attempts");
    }

    #[tokio::test]
    async fn resume_without_cache_requeues_for_reconversion() {
        let mut h = harness();
        let mut j = make_job("a.pdf");
        let id = j.id.clone();
        j.state = JobState::Paused; // no cache_pdf
        h.state.write().unwrap().jobs.push(j);

        let r = h
            .request(|reply| Cmd::Resume {
                id: id.clone(),
                reply,
            })
            .await;
        assert!(r.is_ok());
        // Should re-enter at Queued and run the full pipeline to Done.
        h.run_to_idle();
        assert_eq!(h.job(&id).state, HANDED_OFF);
        assert_eq!(h.runner.convert_calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn paused_queue_starts_no_new_work_then_resumes() {
        let mut h = harness();
        {
            let mut st = h.state.write().unwrap();
            st.queue_paused = true;
            st.jobs.push(make_job("a.pdf"));
            st.jobs.push(make_job("b.pdf"));
        }
        h.d.schedule();
        assert_eq!(h.count(JobState::Converting), 0);
        assert_eq!(h.count(JobState::Queued), 2);

        h.state.write().unwrap().queue_paused = false;
        h.d.schedule();
        h.run_to_idle();
        assert!(h.jobs().iter().all(|j| j.state == HANDED_OFF));
    }

    #[tokio::test]
    async fn pausing_queue_lets_inflight_conversion_finish() {
        let mut h = harness();
        let j = make_job("a.pdf");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.step(); // start convert; ConversionDone is queued but unprocessed
        assert_eq!(h.job(&id).state, JobState::Converting);

        // Pause before the pending completion is processed.
        h.state.write().unwrap().queue_paused = true;
        h.run_to_idle();

        // In-flight conversion still completes, but does not advance to printing.
        assert_eq!(h.job(&id).state, JobState::Converted);
    }

    #[tokio::test]
    async fn cancel_queued_job_marks_cancelled() {
        let mut h = harness();
        let jobs: Vec<Job> = (0..3).map(|i| make_job(&format!("f{i}.pdf"))).collect();
        let queued_id = jobs[2].id.clone();
        h.submit(jobs);
        h.step(); // 2 converting, 1 queued

        let r = h
            .request(|reply| Cmd::Cancel {
                id: queued_id.clone(),
                reply,
            })
            .await;
        assert!(r.is_ok());
        assert_eq!(h.job(&queued_id).state, JobState::Cancelled);
        h.run_to_idle();
    }

    #[tokio::test]
    async fn cancel_of_converting_job_cleans_partial_cache() {
        let mut h = harness();
        let j = make_job("a.pdf");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.step(); // convert ran inline and produced a file; ConversionDone pending
        let cache_path = h.runner.cache.join(format!("{id}.pdf"));
        assert!(cache_path.exists());
        assert_eq!(h.job(&id).state, JobState::Converting);

        let r = h
            .request(|reply| Cmd::Cancel {
                id: id.clone(),
                reply,
            })
            .await;
        assert!(r.is_ok());
        assert_eq!(h.job(&id).state, JobState::Cancelled);
        h.run_to_idle(); // process the pending ConversionDone -> cleanup
        assert!(!cache_path.exists(), "partial cache removed");
    }

    #[tokio::test]
    async fn cancel_all_counts_non_terminal_jobs() {
        let mut h = harness();
        let jobs: Vec<Job> = (0..3).map(|i| make_job(&format!("f{i}.pdf"))).collect();
        h.submit(jobs);
        h.step(); // 2 converting, 1 queued -> all 3 non-terminal

        let n = h.request(|reply| Cmd::CancelAll { reply }).await;
        assert_eq!(n, 3);
        h.run_to_idle();
        assert_eq!(h.count(JobState::Cancelled), 3);
    }

    #[tokio::test]
    async fn clear_done_removes_only_terminal_jobs() {
        let mut h = harness();
        let a = make_job("a.pdf");
        let b = make_job("b.pdf");
        let (ida, idb) = (a.id.clone(), b.id.clone());
        h.submit(vec![a, b]);
        h.run_to_idle(); // both Done

        // Pause the queue so a fresh job stays Queued (non-terminal).
        h.tx.send(Cmd::PauseQueue).unwrap();
        h.run_to_idle();
        let c = make_job("c.pdf");
        let idc = c.id.clone();
        h.submit(vec![c]);
        h.run_to_idle();
        assert_eq!(h.job(&idc).state, JobState::Queued);

        let n = h.request(|reply| Cmd::ClearDone { reply }).await;
        assert_eq!(n, 2);
        assert_eq!(h.jobs().len(), 1);
        assert!(h.jobs().iter().any(|j| j.id == idc));
        assert!(!h.jobs().iter().any(|j| j.id == ida || j.id == idb));
    }

    #[tokio::test]
    async fn print_done_for_cancelled_job_cancels_os_and_cleans_cache() {
        let mut h = harness();
        let mut j = make_job("a.pdf");
        let id = j.id.clone();
        std::fs::create_dir_all(&h.runner.cache).unwrap();
        let cache = h.runner.cache.join("leftover.pdf");
        std::fs::write(&cache, b"x").unwrap();
        j.state = JobState::Cancelled;
        j.cache_pdf = Some(cache.to_string_lossy().into_owned());
        h.state.write().unwrap().jobs.push(j);

        h.tx.send(Cmd::PrintDone {
            id: id.clone(),
            result: Ok(PrintOk {
                printer: "P".to_string(),
                os_job_id: Some("P-1".to_string()),
            }),
        })
        .unwrap();
        h.run_to_idle();

        assert_eq!(
            h.runner.cancelled_os.lock().unwrap().as_slice(),
            &["P-1".to_string()]
        );
        assert!(!cache.exists());
    }

    #[tokio::test]
    async fn events_and_persistence_track_transitions() {
        let mut h = harness();
        let j = make_job("a.pdf");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();

        // The emitted snapshots pass through Converting … Done, in that order.
        let states: Vec<JobState> = h
            .emit
            .snapshots
            .lock()
            .unwrap()
            .iter()
            .filter_map(|s| s.jobs.iter().find(|j| j.id == id).map(|j| j.state))
            .collect();
        assert!(states.contains(&JobState::Converting));
        assert!(states.contains(&JobState::Printing));
        assert_eq!(*states.last().unwrap(), HANDED_OFF);

        // Progress was emitted for the conversion.
        assert!(!h.emit.progress.lock().unwrap().is_empty());

        // The queue file on disk reflects the terminal state.
        let raw = std::fs::read_to_string(&h.queue_file).unwrap();
        let snap: QueueSnapshot = serde_json::from_str(&raw).unwrap();
        assert!(snap
            .jobs
            .iter()
            .any(|j| j.id == id && j.state == HANDED_OFF));
    }

    #[tokio::test]
    async fn retry_of_failed_job_resets_and_reruns() {
        let mut h = harness();
        let mut j = make_job("a.pdf");
        let id = j.id.clone();
        j.state = JobState::Failed;
        j.attempts = 3;
        j.error = Some(JobError::new("conversion_failed", "boom"));
        j.finished_at = Some(now_rfc3339());
        h.state.write().unwrap().jobs.push(j);

        let r = h
            .request(|reply| Cmd::Retry {
                id: id.clone(),
                reply,
            })
            .await;
        assert!(r.is_ok());
        h.run_to_idle();
        assert_eq!(h.job(&id).state, HANDED_OFF);
        assert_eq!(h.job(&id).attempts, 0);
    }

    /// A retry is a fresh run: the Timeline must not measure it from the failed
    /// attempt hours earlier, and the printer of the old run is not this one's.
    #[tokio::test]
    async fn retry_starts_a_fresh_clock_and_forgets_the_old_printer() {
        let mut h = harness_with(
            Arc::new(DeferredSpawner::default()),
            DispatcherLimits::default(),
        );
        let mut j = make_job("a.pdf");
        let id = j.id.clone();
        j.state = JobState::Failed;
        j.started_at = Some("2026-01-01T10:00:00Z".to_string());
        j.finished_at = Some("2026-01-01T10:00:05Z".to_string());
        j.printer_resolved = Some("Old Office".to_string());
        j.error = Some(JobError::new("print_failed", "boom"));
        h.state.write().unwrap().jobs.push(j);

        h.request(|reply| Cmd::Retry {
            id: id.clone(),
            reply,
        })
        .await
        .unwrap();

        let job = h.job(&id);
        assert_ne!(
            job.started_at.as_deref(),
            Some("2026-01-01T10:00:00Z"),
            "the retry restarts the clock"
        );
        assert!(job.printer_resolved.is_none());
    }

    #[tokio::test]
    async fn cancel_missing_job_is_job_not_found() {
        let mut h = harness();
        let r = h
            .request(|reply| Cmd::Cancel {
                id: "nope".to_string(),
                reply,
            })
            .await;
        assert!(matches!(r, Err(IpcError::JobNotFound(_))));
    }

    #[tokio::test]
    async fn pause_printing_job_is_invalid_state() {
        let mut h = harness();
        let mut j = make_job("a.pdf");
        let id = j.id.clone();
        j.state = JobState::Printing;
        h.state.write().unwrap().jobs.push(j);
        let r = h
            .request(|reply| Cmd::Pause {
                id: id.clone(),
                reply,
            })
            .await;
        let e = r.expect_err("pausing a printing job is refused");
        assert_eq!(e.code(), IpcErrorCode::InvalidState);
        // FR-092: the toast says exactly why, in the user's language.
        assert_eq!(e.reason(), Some("job_printing"));
    }

    // ---- Concurrency cap (DispatcherLimits::max_concurrent) ---------------

    #[tokio::test]
    async fn schedule_honors_max_concurrent_of_one() {
        let mut h = harness_with(
            Arc::new(DeferredSpawner::default()),
            DispatcherLimits {
                max_concurrent: 1,
                conversion_timeout: None,
                ..DispatcherLimits::default()
            },
        );
        let jobs: Vec<Job> = (0..3).map(|i| make_job(&format!("f{i}.pdf"))).collect();
        h.submit(jobs);
        h.step(); // process Submit → schedule
                  // Deferred spawner parks the conversions, so states reflect the cap.
        assert_eq!(h.count(JobState::Converting), 1);
        assert_eq!(h.count(JobState::Queued), 2);
    }

    #[tokio::test]
    async fn schedule_honors_max_concurrent_of_three() {
        let mut h = harness_with(
            Arc::new(DeferredSpawner::default()),
            DispatcherLimits {
                max_concurrent: 3,
                conversion_timeout: None,
                ..DispatcherLimits::default()
            },
        );
        let jobs: Vec<Job> = (0..4).map(|i| make_job(&format!("f{i}.pdf"))).collect();
        h.submit(jobs);
        h.step();
        assert_eq!(h.count(JobState::Converting), 3);
        assert_eq!(h.count(JobState::Queued), 1);
    }

    /// FR-037: pausing or cancelling a converting job only *flags* its blocking
    /// task, which keeps running until its next stage boundary. Those orphans
    /// still occupy CPU, so they count against `max_concurrent` until their
    /// `ConversionDone` arrives.
    #[tokio::test]
    async fn fr_037_orphaned_conversions_still_count_against_the_cap() {
        let spawner = Arc::new(DeferredSpawner::default());
        let mut h = harness_with(spawner.clone(), DispatcherLimits::default());
        let jobs: Vec<Job> = (0..4).map(|i| make_job(&format!("f{i}.pdf"))).collect();
        let ids: Vec<String> = jobs.iter().map(|j| j.id.clone()).collect();
        h.submit(jobs);
        h.step();
        assert_eq!(
            spawner.tasks.lock().unwrap().len(),
            2,
            "two conversions run"
        );

        // Pause one, cancel the other: both tasks are still running.
        h.request(|reply| Cmd::Pause {
            id: ids[0].clone(),
            reply,
        })
        .await
        .unwrap();
        h.request(|reply| Cmd::Cancel {
            id: ids[1].clone(),
            reply,
        })
        .await
        .unwrap();
        assert_eq!(
            spawner.tasks.lock().unwrap().len(),
            2,
            "no new conversion may start while the orphans still run"
        );
        assert_eq!(h.count(JobState::Queued), 2);

        // Once an orphan reports back, its slot frees up.
        spawner.run_next();
        h.run_to_idle();
        assert_eq!(h.count(JobState::Converting), 1);
    }

    /// Engine settings apply to the running queue without a restart: raising
    /// the cap starts the waiting conversions straight away.
    #[tokio::test]
    async fn fr_037_raising_the_cap_at_run_time_starts_waiting_conversions() {
        let spawner = Arc::new(DeferredSpawner::default());
        let mut h = harness_with(
            spawner.clone(),
            DispatcherLimits {
                max_concurrent: 1,
                ..DispatcherLimits::default()
            },
        );
        h.submit((0..3).map(|i| make_job(&format!("f{i}.pdf"))).collect());
        h.step();
        assert_eq!(h.count(JobState::Converting), 1);

        h.tx.send(Cmd::SetLimits {
            limits: DispatcherLimits {
                max_concurrent: 3,
                print_timeout: Some(Duration::from_secs(42)),
                ..DispatcherLimits::default()
            },
        })
        .unwrap();
        h.step();
        assert_eq!(h.count(JobState::Converting), 3, "no restart needed");

        // The new print budget reaches the next spool.
        while spawner.run_next() {
            h.run_to_idle();
        }
        h.run_to_idle();
        let timeouts = h.runner.print_timeouts.lock().unwrap().clone();
        assert!(!timeouts.is_empty());
        assert!(timeouts.iter().all(|t| *t == Some(Duration::from_secs(42))));
    }

    // ---- Conversion timeout ----------------------------------------------

    /// FR-034 / settings.md: a conversion timeout saved in Settings governs the
    /// next conversion. The limit used to be copied into the runner when the
    /// queue booted, so the converter process kept being killed at the old
    /// value however the setting changed.
    #[tokio::test(start_paused = true)]
    async fn fr_034_a_changed_conversion_timeout_reaches_the_converter_process() {
        let mut h = harness_with(
            Arc::new(InlineSpawner),
            DispatcherLimits {
                conversion_timeout: Some(Duration::from_secs(60)),
                ..DispatcherLimits::default()
            },
        );
        h.submit(vec![make_job("first.docx")]);
        h.run_to_idle();

        for limit in [Some(Duration::from_secs(600)), None] {
            h.tx.send(Cmd::SetLimits {
                limits: DispatcherLimits {
                    conversion_timeout: limit,
                    ..DispatcherLimits::default()
                },
            })
            .unwrap();
            h.submit(vec![make_job("next.docx")]);
            h.run_to_idle();
        }

        assert_eq!(
            *h.runner.convert_timeouts.lock().unwrap(),
            vec![
                Some(Duration::from_secs(60)),
                Some(Duration::from_secs(600)),
                None
            ],
            "each conversion runs under the limit in force when it starts"
        );
    }

    /// FR-034: a converter killed by its own deadline is a conversion timeout,
    /// which is terminal. It used to come back as a retryable
    /// `conversion_failed`, so the job was run three more times, each killed at
    /// the same limit.
    #[tokio::test]
    async fn fr_034_a_converter_timeout_fails_the_job_without_retrying() {
        let mut h = harness();
        h.runner
            .convert_steps
            .lock()
            .unwrap()
            .push_back(ConvertStep::TimedOut);
        let j = make_job("big.docx");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();

        let job = h.job(&id);
        assert_eq!(job.state, JobState::Failed);
        assert_eq!(job.error.unwrap().code, "conversion_timeout");
        assert_eq!(h.runner.convert_calls.load(Ordering::Relaxed), 1);
        assert!(!h.d.delayed.contains_key(&id), "no retry is scheduled");
    }

    #[tokio::test(start_paused = true)]
    async fn conversion_timeout_fails_job_and_sets_flag() {
        let spawner = Arc::new(DeferredSpawner::default());
        let mut h = harness_with(
            spawner.clone(),
            DispatcherLimits {
                max_concurrent: 2,
                conversion_timeout: Some(Duration::from_secs(60)),
                ..DispatcherLimits::default()
            },
        );
        let j = make_job("a.pdf");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle(); // Submit → Converting; convert parked, timer armed.
        assert_eq!(h.job(&id).state, JobState::Converting);
        let flag =
            h.d.converting
                .get(&id)
                .cloned()
                .expect("attempt flag registered");
        assert!(!flag.load(Ordering::Relaxed));

        // Advance past the timeout: the timer sends ConversionTimeout.
        h.advance_and_settle(Duration::from_secs(60)).await;
        assert_eq!(h.job(&id).state, JobState::Failed);
        assert!(
            flag.load(Ordering::Relaxed),
            "cancel flag set so the running conversion aborts"
        );
        let err = h.job(&id).error.unwrap();
        assert_eq!(err.code, "conversion_timeout");
        assert!(h.job(&id).finished_at.is_some());
        assert_eq!(h.job(&id).attempts, 0, "timeout is terminal, not a retry");

        // The parked (now-aborted) conversion reports back; it must not
        // resurrect the failed job.
        spawner.run_next();
        h.run_to_idle();
        assert_eq!(h.job(&id).state, JobState::Failed);
    }

    #[tokio::test(start_paused = true)]
    async fn conversion_timeout_after_completion_is_noop() {
        // Inline spawner completes the conversion (and the whole job) before the
        // timer ever fires; advancing past the deadline must not touch the
        // terminal job — its `converting` entry is already gone.
        let mut h = harness_with(
            Arc::new(InlineSpawner),
            DispatcherLimits {
                max_concurrent: 2,
                conversion_timeout: Some(Duration::from_secs(60)),
                ..DispatcherLimits::default()
            },
        );
        let j = make_job("a.pdf");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();
        assert_eq!(h.job(&id).state, HANDED_OFF);

        h.advance_and_settle(Duration::from_secs(60)).await;
        assert_eq!(h.job(&id).state, HANDED_OFF, "stale timer is a no-op");
    }

    #[tokio::test]
    async fn stale_conversion_timeout_does_not_kill_current_attempt() {
        // A timer armed for attempt N fires after attempt N+1 has started with a
        // fresh flag. The Arc::ptr_eq guard must ignore it.
        let mut h = harness();
        let mut j = make_job("a.pdf");
        let id = j.id.clone();
        j.state = JobState::Converting;
        h.state.write().unwrap().jobs.push(j);

        // The live attempt (N+1) registers flag B.
        let flag_b = Arc::new(AtomicBool::new(false));
        h.d.converting.insert(id.clone(), flag_b.clone());

        // A stale timer from attempt N fires with a different flag (A).
        let flag_a = Arc::new(AtomicBool::new(false));
        h.tx.send(Cmd::ConversionTimeout {
            id: id.clone(),
            flag: flag_a.clone(),
        })
        .unwrap();
        h.step();

        assert_eq!(
            h.job(&id).state,
            JobState::Converting,
            "current attempt survives a stale timer"
        );
        assert!(!flag_b.load(Ordering::Relaxed), "live flag untouched");
        assert!(!flag_a.load(Ordering::Relaxed), "stale flag not set");
    }

    // ---- Print timeout (FR-034 / NFR-012) --------------------------------

    /// Harness whose engine limits carry a print budget.
    fn harness_with_print_timeout(secs: u64) -> Harness {
        harness_with(
            Arc::new(InlineSpawner),
            DispatcherLimits {
                print_timeout: Some(Duration::from_secs(secs)),
                ..DispatcherLimits::default()
            },
        )
    }

    /// The print budget is its own setting: a conversion budget alone never
    /// reaches the spooler, and the print budget never bounds a conversion.
    #[tokio::test]
    async fn fr_034_print_budget_is_independent_of_the_conversion_budget() {
        let mut h = harness_with(
            Arc::new(InlineSpawner),
            DispatcherLimits {
                conversion_timeout: Some(Duration::from_secs(60)),
                print_timeout: None,
                ..DispatcherLimits::default()
            },
        );
        h.submit(vec![make_job("a.pdf")]);
        h.run_to_idle();
        assert_eq!(h.runner.print_timeouts.lock().unwrap().as_slice(), &[None]);

        let mut h = harness_with_print_timeout(300);
        h.submit(vec![make_job("b.pdf")]);
        h.run_to_idle();
        assert_eq!(
            h.runner.print_timeouts.lock().unwrap().as_slice(),
            &[Some(Duration::from_secs(300))]
        );
    }

    #[test]
    fn limits_follow_the_engine_settings() {
        let e = crate::settings::EngineCfg {
            max_concurrent_conversions: 3,
            conversion_timeout_seconds: 0,
            print_timeout_seconds: 120,
            ..Default::default()
        };
        let l = DispatcherLimits::from_engine(&e);
        assert_eq!(l.max_concurrent, 3);
        assert_eq!(l.conversion_timeout, None, "0 disables it");
        assert_eq!(l.print_timeout, Some(Duration::from_secs(120)));
    }

    #[tokio::test]
    async fn fr_034_print_timeout_fails_job_terminally_and_queue_keeps_going() {
        // The regression this pins: a hung spooler used to leave the job in
        // `printing` forever, and since exactly one job prints at a time, the
        // whole queue died with it. The assertion that matters is the *second*
        // job reaching Done.
        let mut h = harness_with_print_timeout(60);
        h.runner
            .print_steps
            .lock()
            .unwrap()
            .push_back(PrintStep::Timeout);

        let a = make_job("a.pdf");
        let b = make_job("b.pdf");
        let (ida, idb) = (a.id.clone(), b.id.clone());
        h.submit(vec![a, b]);
        h.run_to_idle();

        // The wedged job is terminal with a distinguishable reason and is not
        // retried — re-spooling would most likely wedge again.
        assert_eq!(h.job(&ida).state, JobState::Failed);
        let err = h.job(&ida).error.unwrap();
        assert_eq!(err.code, "print_timeout");
        assert!(
            err.message.contains("60s"),
            "reason names the budget: {err:?}"
        );
        assert_eq!(
            h.job(&ida).attempts,
            0,
            "a timeout is terminal, not a retry"
        );
        assert!(h.job(&ida).finished_at.is_some());
        assert!(h.job(&ida).cache_pdf.is_none(), "cache released");

        // The queue moved on.
        assert_eq!(h.job(&idb).state, HANDED_OFF);
        assert_eq!(h.runner.print_calls.load(Ordering::Relaxed), 2);

        // The budget really reached the engine seam.
        assert_eq!(
            h.runner.print_timeouts.lock().unwrap()[0],
            Some(Duration::from_secs(60))
        );
    }

    #[tokio::test]
    async fn print_timeout_disabled_passes_no_budget_to_the_engine() {
        let mut h = harness(); // default limits: timeout disabled
        h.submit(vec![make_job("a.pdf")]);
        h.run_to_idle();
        assert_eq!(h.runner.print_timeouts.lock().unwrap().as_slice(), &[None]);
    }

    // ---- Missing printer (print-settings.md §printer) ---------------------

    #[tokio::test]
    async fn fr_034_missing_printer_pauses_the_job_instead_of_failing_it() {
        // The spec (and the Printers screen banner) promise "paused, not
        // failed" so the user can pick a replacement.
        let mut h = harness();
        h.runner.printer_installed.store(false, Ordering::Relaxed);
        let mut j = make_job("a.pdf");
        j.settings.printer = Some("Old Office".to_string());
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();

        assert_eq!(h.job(&id).state, JobState::Paused);
        assert_eq!(
            h.job(&id).attempts,
            0,
            "no retry burn on a user-fixable error"
        );
        let err = h.job(&id).error.unwrap();
        assert_eq!(err.code, "printer_unavailable");
        assert!(err.message.contains("Old Office"), "names it: {err:?}");
        assert_eq!(h.runner.print_calls.load(Ordering::Relaxed), 0);
        assert!(
            h.job(&id).cache_pdf.is_some(),
            "cache kept so resuming onto another printer does not re-convert"
        );
    }

    /// A job pinned to a printer that disappears used to be a dead end:
    /// resuming re-ran the same lookup, failed the same way, and there was no
    /// way to re-point it. Resume now re-resolves, and once preferences point
    /// somewhere usable the job goes there instead of being stuck.
    #[tokio::test]
    async fn fr_paused_job_escapes_a_printer_that_is_gone_for_good() {
        let mut h = harness();
        h.runner
            .missing_printers
            .lock()
            .unwrap()
            .insert("Old Office".to_string());

        let mut j = make_job("a.pdf");
        j.settings.printer = Some("Old Office".to_string());
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();

        assert_eq!(h.job(&id).state, JobState::Paused);
        assert_eq!(h.job(&id).error.unwrap().code, "printer_unavailable");

        // Resuming re-resolves: "Old Office" is still gone, but the app-level
        // chain answers with the printer that is there, so the pin is dropped
        // and the job actually prints.
        h.request(|reply| Cmd::Resume {
            id: id.clone(),
            reply,
        })
        .await
        .expect("resume succeeds now that another printer resolves");

        assert_eq!(
            h.job(&id).settings.printer,
            None,
            "the dead pin is dropped so dispatch can resolve normally"
        );
        h.run_to_idle();
        assert_eq!(h.job(&id).state, HANDED_OFF);
        assert_eq!(h.job(&id).printer_resolved.as_deref(), Some("FakePrinter"));
        assert_eq!(
            h.runner.convert_calls.load(Ordering::Relaxed),
            1,
            "the cached PDF is reused — resuming must not re-convert"
        );
    }

    /// The other half of the same rule: when there is still nothing to print
    /// on, resume must say so instead of silently putting the job back in the
    /// state it was already stuck in.
    #[tokio::test]
    async fn fr_resume_refuses_while_no_printer_resolves_at_all() {
        let mut h = harness();
        h.runner.printer_installed.store(false, Ordering::Relaxed);

        let mut j = make_job("a.pdf");
        j.settings.printer = Some("Old Office".to_string());
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();
        assert_eq!(h.job(&id).state, JobState::Paused);

        let err = h
            .request(|reply| Cmd::Resume {
                id: id.clone(),
                reply,
            })
            .await
            .expect_err("nothing resolves, so the resume is refused");
        let msg = err.to_string();
        assert!(msg.contains("Old Office"), "names the printer: {msg}");
        assert!(msg.contains("Printers screen"), "says what to do: {msg}");

        // The job is untouched: still paused, still explaining why.
        assert_eq!(h.job(&id).state, JobState::Paused);
        assert_eq!(
            h.job(&id).error.unwrap().code,
            "printer_unavailable",
            "the reason survives a refused resume"
        );
        assert_eq!(h.job(&id).settings.printer.as_deref(), Some("Old Office"));

        // Once the printer is back, the very same resume works and keeps it.
        h.runner.printer_installed.store(true, Ordering::Relaxed);
        h.request(|reply| Cmd::Resume {
            id: id.clone(),
            reply,
        })
        .await
        .expect("resume works once the printer is installed again");
        assert_eq!(h.job(&id).settings.printer.as_deref(), Some("Old Office"));
        h.run_to_idle();
        assert_eq!(h.job(&id).state, HANDED_OFF);
    }

    /// A printer that is still installed but cannot be used as named — two
    /// queues share its name, or it is disabled — is not "gone": resuming
    /// used to drop the pin and print on the default printer with nothing
    /// but a log line to show for it. Resume now refuses and says what to fix.
    #[tokio::test]
    async fn fr_resume_never_moves_a_job_off_an_ambiguous_or_disabled_printer() {
        for (setup, reason) in [
            ("ambiguous", "resume_printer_ambiguous"),
            ("disabled", "resume_printer_disabled"),
        ] {
            let mut h = harness();
            let set = if setup == "ambiguous" {
                &h.runner.ambiguous
            } else {
                &h.runner.disabled
            };
            set.lock().unwrap().insert("HP LaserJet".to_string());

            let mut j = make_job("a.pdf");
            j.settings.printer = Some("HP LaserJet".to_string());
            let id = j.id.clone();
            h.submit(vec![j]);
            h.run_to_idle();
            assert_eq!(h.job(&id).state, JobState::Paused, "{setup}");

            let err = h
                .request(|reply| Cmd::Resume {
                    id: id.clone(),
                    reply,
                })
                .await
                .expect_err("resume refuses instead of re-pointing the job");
            assert_eq!(err.reason(), Some(reason), "{setup}");
            assert!(err.to_string().contains("HP LaserJet"), "{err}");
            assert_eq!(
                h.job(&id).settings.printer.as_deref(),
                Some("HP LaserJet"),
                "{setup}: the user's choice is kept"
            );
            assert_eq!(h.job(&id).state, JobState::Paused);
            assert_eq!(h.runner.print_calls.load(Ordering::Relaxed), 0);
        }
    }

    #[tokio::test]
    async fn no_configured_printer_pauses_the_job() {
        let mut h = harness();
        h.runner.resolve_ok.store(false, Ordering::Relaxed);
        let j = make_job("a.pdf");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();

        assert_eq!(h.job(&id).state, JobState::Paused);
        assert_eq!(h.job(&id).error.unwrap().code, "printer_unavailable");
    }

    // ---- Page ranges are PDF-only ----------------------------------------

    #[tokio::test]
    async fn fr_040_image_job_is_spooled_without_page_ranges() {
        let mut h = harness();
        let mut img = Job::new(
            "batch",
            "/tmp/photo.png",
            JobSettings {
                page_ranges: Some("2-3".to_string()),
                ..JobSettings::default()
            },
            None,
            now_rfc3339(),
        );
        let mut pdf = img.clone();
        img.state = JobState::Queued;
        pdf.id = uuid::Uuid::now_v7().to_string();
        pdf.input_path = "/tmp/report.pdf".to_string();
        pdf.file_name = "report.pdf".to_string();
        pdf.input_ext = "pdf".to_string();
        h.submit(vec![img, pdf]);
        h.run_to_idle();

        let spooled = h.runner.print_settings.lock().unwrap();
        assert_eq!(spooled.len(), 2);
        assert!(
            spooled[0].page_ranges.is_none(),
            "an image is one page; a 2-3 range would spool nothing"
        );
        assert_eq!(spooled[1].page_ranges.as_deref(), Some("2-3"));
    }

    // ---- File-type routing ------------------------------------------------

    /// The "Route file types here" rules on the Printers screen were persisted
    /// but never consumed by any print path. Routing is decided per file, so
    /// the dispatcher has to hand the file name to `resolve_printer`; without
    /// it the rule can never match, which is exactly how the feature was dead.
    #[tokio::test]
    async fn routing_needs_the_file_name_at_printer_resolution() {
        let mut h = harness();
        let mut pdf = Job::new(
            "batch",
            "/tmp/report.pdf",
            JobSettings::default(),
            None,
            now_rfc3339(),
        );
        pdf.state = JobState::Queued;
        pdf.file_name = "report.pdf".to_string();
        pdf.input_ext = "pdf".to_string();
        h.submit(vec![pdf]);
        h.run_to_idle();

        let seen = h.runner.resolve_file_names.lock().unwrap();
        assert_eq!(
            seen.as_slice(),
            ["report.pdf"],
            "the job's file name must reach printer resolution for a .pdf rule to apply"
        );
    }

    /// Re-resolving a printer reads the OS printer list, which can take
    /// seconds. It must not run on the dispatcher loop, where it would stall
    /// every other command and completion until it returned.
    #[tokio::test]
    async fn resume_resolves_the_printer_off_the_dispatcher_loop() {
        let spawner = Arc::new(DeferredSpawner::default());
        let mut h = harness_with(spawner.clone(), DispatcherLimits::default());
        let mut j = make_job("a.pdf");
        j.state = JobState::Paused;
        j.error = Some(JobError::new("printer_unavailable", "gone"));
        let id = j.id.clone();
        h.state.write().unwrap().jobs.push(j);

        let (rtx, mut rrx) = oneshot::channel();
        h.tx.send(Cmd::Resume {
            id: id.clone(),
            reply: rtx,
        })
        .unwrap();
        h.step();
        assert!(
            h.runner.resolve_file_names.lock().unwrap().is_empty(),
            "no printer lookup on the loop"
        );
        assert!(rrx.try_recv().is_err(), "the reply waits for the lookup");
        assert_eq!(h.job(&id).state, JobState::Paused);

        spawner.run_next(); // the lookup, on the blocking pool
        h.run_to_idle();
        assert!(rrx.try_recv().unwrap().is_ok());
        assert_ne!(h.job(&id).state, JobState::Paused);
    }

    /// A paper size the printer does not list pauses the job for the user to
    /// fix, instead of the driver substituting another size in silence.
    #[tokio::test]
    async fn fr_041_a_paper_size_the_printer_lacks_pauses_the_job() {
        let mut h = harness();
        *h.runner.papers.lock().unwrap() = vec!["A4".into(), "Letter".into()];
        let mut j = make_job("a.pdf");
        j.settings.paper_size = crate::queue::job::PaperSize::parse("Tabloid").unwrap();
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();

        assert_eq!(h.job(&id).state, JobState::Paused);
        let err = h.job(&id).error.unwrap();
        assert_eq!(err.code, "unsupported_setting");
        assert!(err.message.contains("Tabloid"), "{err:?}");
        assert_eq!(h.runner.print_calls.load(Ordering::Relaxed), 0);
    }

    /// The Printers screen's per-printer tray/paper defaults reach real jobs:
    /// they fill what the job leaves open and never beat an explicit choice.
    #[tokio::test]
    async fn fr_printer_overrides_reach_real_jobs_without_beating_explicit_values() {
        use crate::queue::job::PaperSize;
        let mut h = harness();
        h.runner.overrides.lock().unwrap().insert(
            "FakePrinter".into(),
            crate::settings::PrinterOverride {
                tray: Some("Tray2".into()),
                paper: Some("Letter".into()),
            },
        );
        let mut open = make_job("open.pdf");
        open.settings.paper_size = PaperSize::AUTO;
        let mut explicit = make_job("explicit.pdf");
        explicit.settings.paper_size = PaperSize::A3;
        explicit.settings.tray = Some("Manual".into());
        let open_id = open.id.clone();
        h.submit(vec![open, explicit]);
        h.run_to_idle();

        let sent = h.runner.print_settings.lock().unwrap().clone();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].tray.as_deref(), Some("Tray2"));
        assert_eq!(sent[0].paper_size, PaperSize::LETTER);
        assert_eq!(sent[1].tray.as_deref(), Some("Manual"));
        assert_eq!(sent[1].paper_size, PaperSize::A3);
        // The job keeps what the user asked for; only the spool is filled in.
        assert!(h.job(&open_id).settings.paper_size.is_auto());
    }

    /// A job aimed at a printer disabled in Pluryprint is paused with a
    /// reason that names it, and is never sent; once the printer is enabled
    /// again, resuming sends it there.
    #[tokio::test]
    async fn fr_a_job_for_a_disabled_printer_pauses_with_the_reason() {
        let mut h = harness();
        h.runner.disabled.lock().unwrap().insert("Office".into());
        let mut j = make_job("a.pdf");
        j.settings.printer = Some("Office".into());
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();

        assert_eq!(h.job(&id).state, JobState::Paused);
        let err = h.job(&id).error.unwrap();
        assert_eq!(err.code, "printer_disabled");
        assert!(
            err.message.contains("Office") && err.message.contains("disabled"),
            "{err:?}"
        );
        assert_eq!(h.runner.print_calls.load(Ordering::Relaxed), 0);

        h.runner.disabled.lock().unwrap().clear();
        h.request(|reply| Cmd::Resume {
            id: id.clone(),
            reply,
        })
        .await
        .unwrap();
        h.run_to_idle();
        assert_eq!(h.job(&id).state, HANDED_OFF);
        assert_eq!(h.job(&id).printer_resolved.as_deref(), Some("Office"));
    }

    // ---- OS job tracking (the print slot) --------------------------------

    /// Queue `n` spool answers that hand back OS job ids `P-1`, `P-2`, …
    fn spool_with_ids(h: &Harness, n: usize) {
        let mut steps = h.runner.print_steps.lock().unwrap();
        for i in 1..=n {
            steps.push_back(PrintStep::Ok(Some(format!("P-{i}"))));
        }
    }

    /// A job is done when the printer is done, not when `lp` returns — and the
    /// next job does not reach the spooler until then.
    #[tokio::test(start_paused = true)]
    async fn fr_tracked_print_stays_printing_until_the_os_job_completes() {
        let mut h = harness();
        spool_with_ids(&h, 2);
        h.tracker.script([
            Ok(OsJobState::Processing),
            Ok(OsJobState::Held),
            Ok(OsJobState::Completed),
        ]);
        let a = make_job("a.pdf");
        let b = make_job("b.pdf");
        let (ida, idb) = (a.id.clone(), b.id.clone());
        h.submit(vec![a, b]);
        h.run_to_idle();

        assert_eq!(h.job(&ida).state, JobState::Printing, "spooled ≠ printed");
        assert_eq!(h.job(&ida).os_job_id.as_deref(), Some("P-1"));
        assert_eq!(h.job(&idb).state, JobState::Converted, "slot still taken");
        assert_eq!(h.runner.print_calls.load(Ordering::Relaxed), 1);

        h.advance_and_settle(TRACK_BACKOFFS[0]).await; // → processing
        assert_eq!(h.job(&ida).state, JobState::Printing);
        assert_eq!(h.job(&ida).progress.unwrap().phase, "printing");

        h.advance_and_settle(TRACK_BACKOFFS[1]).await; // → held
        let p = h.job(&ida).progress.unwrap();
        assert_eq!(p.phase, "held", "a held job says so");
        assert!(p.percent.is_none());
        assert_eq!(h.job(&ida).state, JobState::Printing);

        h.advance_and_settle(TRACK_BACKOFFS[2]).await; // → completed
        assert_eq!(h.job(&ida).state, JobState::Done);
        assert!(h.job(&ida).cache_pdf.is_none());
        // Only now does the second job reach the spooler.
        assert_eq!(h.job(&idb).state, JobState::Printing);
        assert_eq!(h.runner.print_calls.load(Ordering::Relaxed), 2);
        assert_eq!(
            h.tracker.polls.lock().unwrap()[0],
            ("FakePrinter".to_string(), "P-1".to_string())
        );
    }

    #[tokio::test(start_paused = true)]
    async fn fr_os_cancel_and_abort_end_the_job_with_their_own_reason() {
        let mut h = harness();
        spool_with_ids(&h, 2);
        h.tracker
            .script([Ok(OsJobState::Canceled), Ok(OsJobState::Aborted)]);
        let a = make_job("a.pdf");
        let b = make_job("b.pdf");
        let (ida, idb) = (a.id.clone(), b.id.clone());
        h.submit(vec![a, b]);
        h.run_to_idle();
        h.advance_and_settle(TRACK_BACKOFFS[0]).await;
        assert_eq!(h.job(&ida).state, JobState::Cancelled);
        assert_eq!(h.job(&ida).error.unwrap().code, "os_cancelled");

        h.advance_and_settle(TRACK_BACKOFFS[0]).await;
        assert_eq!(h.job(&idb).state, JobState::Failed);
        assert_eq!(h.job(&idb).error.unwrap().code, "print_aborted");
        assert_eq!(h.job(&idb).attempts, 0, "never re-sent automatically");
    }

    #[tokio::test(start_paused = true)]
    async fn fr_034_print_budget_bounds_the_wait_on_the_os_queue() {
        let mut h = harness_with_print_timeout(10);
        spool_with_ids(&h, 1);
        h.tracker
            .script(std::iter::repeat_n(Ok(OsJobState::Held), 50));
        let j = make_job("a.pdf");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();

        for d in TRACK_BACKOFFS {
            h.advance_and_settle(d).await;
        }
        let job = h.job(&id);
        assert_eq!(job.state, JobState::Failed);
        let err = job.error.unwrap();
        assert_eq!(err.code, "print_timeout");
        assert!(
            err.message.contains("still in the system print queue"),
            "{err:?}"
        );
        assert!(
            h.runner.cancelled_os.lock().unwrap().is_empty(),
            "a slow job is not cancelled behind the user's back"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn fr_print_budget_off_means_tracking_waits_as_long_as_it_takes() {
        let mut h = harness(); // print_timeout: None
        spool_with_ids(&h, 1);
        h.tracker
            .script(std::iter::repeat_n(Ok(OsJobState::Stopped), 20));
        let j = make_job("a.pdf");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();
        for _ in 0..12 {
            h.advance_and_settle(TRACK_POLL_CAP).await;
        }
        assert_eq!(h.job(&id).state, JobState::Printing);
        assert_eq!(h.job(&id).progress.unwrap().phase, "stopped");
    }

    #[tokio::test(start_paused = true)]
    async fn fr_032_cancelling_a_tracked_print_cancels_the_os_job_and_frees_the_slot() {
        let mut h = harness();
        spool_with_ids(&h, 2);
        h.tracker.script([Ok(OsJobState::Processing)]);
        let a = make_job("a.pdf");
        let b = make_job("b.pdf");
        let (ida, idb) = (a.id.clone(), b.id.clone());
        h.submit(vec![a, b]);
        h.run_to_idle();
        assert_eq!(h.job(&ida).state, JobState::Printing);

        h.request(|reply| Cmd::Cancel {
            id: ida.clone(),
            reply,
        })
        .await
        .unwrap();
        assert_eq!(h.job(&ida).state, JobState::Cancelled);
        assert_eq!(
            h.runner.cancelled_os.lock().unwrap().as_slice(),
            &["P-1".to_string()]
        );
        assert_eq!(h.job(&idb).state, JobState::Printing, "slot released");

        // The stale timer for the cancelled print is harmless.
        h.advance_and_settle(TRACK_BACKOFFS[0]).await;
        assert_eq!(h.job(&ida).state, JobState::Cancelled);
    }

    /// Cancelling a job while it is still spooling must not hand the printer to
    /// the next job: the spooler child is still running.
    #[tokio::test]
    async fn fr_032_a_cancel_mid_spool_keeps_the_slot_until_the_spool_returns() {
        let spawner = Arc::new(DeferredSpawner::default());
        let mut h = harness_with(spawner.clone(), DispatcherLimits::default());
        spool_with_ids(&h, 2);
        let a = make_job("a.pdf");
        let b = make_job("b.pdf");
        let (ida, idb) = (a.id.clone(), b.id.clone());
        h.submit(vec![a, b]);
        h.run_to_idle();
        // Both conversions finish; A starts spooling (parked).
        spawner.run_next();
        spawner.run_next();
        h.run_to_idle();
        assert_eq!(h.job(&ida).state, JobState::Printing);
        assert_eq!(h.job(&idb).state, JobState::Converted);

        h.request(|reply| Cmd::Cancel {
            id: ida.clone(),
            reply,
        })
        .await
        .unwrap();
        assert_eq!(h.job(&ida).state, JobState::Cancelled);
        assert_eq!(
            h.job(&idb).state,
            JobState::Converted,
            "B must wait: A's spooler child is still running"
        );
        assert_eq!(h.runner.print_calls.load(Ordering::Relaxed), 0);

        // A's spool returns: its OS job is cancelled and B finally starts.
        spawner.run_next();
        h.run_to_idle();
        assert_eq!(
            h.runner.cancelled_os.lock().unwrap().as_slice(),
            &["P-1".to_string()]
        );
        assert_eq!(h.job(&idb).state, JobState::Printing);
    }

    /// Clearing finished jobs must not delete the PDF a cancelled job's spooler
    /// is still reading; it goes once the spool returns.
    #[tokio::test]
    async fn clear_done_keeps_the_cache_of_a_cancelled_job_still_spooling() {
        let spawner = Arc::new(DeferredSpawner::default());
        let mut h = harness_with(spawner.clone(), DispatcherLimits::default());
        let j = make_job("a.png");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();
        spawner.run_next(); // conversion → cache PDF
        h.run_to_idle();
        let cache = PathBuf::from(h.job(&id).cache_pdf.clone().expect("converted"));
        assert_eq!(h.job(&id).state, JobState::Printing);

        h.request(|reply| Cmd::Cancel {
            id: id.clone(),
            reply,
        })
        .await
        .unwrap();
        let cleared = h.request(|reply| Cmd::ClearDone { reply }).await;
        assert_eq!(cleared, 1);
        assert!(cache.exists(), "the spooler may still be reading it");

        spawner.run_next(); // the spool returns
        h.run_to_idle();
        assert!(!cache.exists(), "deleted once the spool is over");
    }

    /// The stored outcome of a print that was handed off but never confirmed.
    fn assert_unconfirmed(job: &Job, reason: &str) {
        assert_eq!(job.state, JobState::Unconfirmed, "never reported as Done");
        let err = job.error.as_ref().expect("the outcome says why");
        assert_eq!(err.code, "print_unconfirmed");
        assert_eq!(err.reason.as_deref(), Some(reason));
        assert!(job.finished_at.is_some());
        assert!(job.cache_pdf.is_none());
        assert_eq!(job.attempts, 0, "never re-sent automatically");
    }

    #[tokio::test(start_paused = true)]
    async fn fr_031_tracking_that_this_host_cannot_do_ends_unconfirmed_not_done() {
        let mut h = harness();
        spool_with_ids(&h, 1);
        h.tracker
            .script([Err(TrackError::Unavailable("no ipptool".into()))]);
        let j = make_job("a.pdf");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();
        h.advance_and_settle(TRACK_BACKOFFS[0]).await;
        assert_unconfirmed(&h.job(&id), "untracked");
    }

    /// A spool that hands back no id to follow is a hand-off, not a print.
    #[tokio::test]
    async fn fr_031_a_print_with_no_spooler_id_ends_unconfirmed() {
        let mut h = harness();
        let j = make_job("a.pdf");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();
        assert_unconfirmed(&h.job(&id), "untracked");
    }

    /// A job that vanishes from the system queue (deleted there, purged, or
    /// removed by Windows before a poll saw it print) is not a printed job,
    /// and the slot moves on to the next one.
    #[tokio::test(start_paused = true)]
    async fn fr_031_a_job_that_left_the_queue_unreported_ends_unconfirmed() {
        let mut h = harness();
        spool_with_ids(&h, 2);
        h.tracker
            .script([Ok(OsJobState::Processing), Ok(OsJobState::Gone)]);
        let a = make_job("a.pdf");
        let b = make_job("b.pdf");
        let (ida, idb) = (a.id.clone(), b.id.clone());
        h.submit(vec![a, b]);
        h.run_to_idle();
        h.advance_and_settle(TRACK_BACKOFFS[0]).await;
        assert_eq!(h.job(&ida).state, JobState::Printing);
        h.advance_and_settle(TRACK_BACKOFFS[1]).await;
        assert_unconfirmed(&h.job(&ida), "left_queue");
        assert_eq!(h.job(&idb).state, JobState::Printing, "slot released");
        assert!(
            h.runner.cancelled_os.lock().unwrap().is_empty(),
            "nothing to cancel"
        );
    }

    /// FR-032 (Windows): an id read back from `queue.json` that cannot be told
    /// apart from a job that later took the same id is not followed — its
    /// state would be somebody else's, and Cancel would delete their print.
    /// The job is not sent again either: it ends unconfirmed.
    #[tokio::test(start_paused = true)]
    async fn fr_032_an_unverifiable_spooler_id_is_not_adopted_after_a_restart() {
        let mut h = harness();
        let mut j = make_job("a.pdf");
        j.state = JobState::Printing;
        j.os_job_id = Some("12".to_string());
        j.printer_resolved = Some("FakePrinter".to_string());
        let id = j.id.clone();
        h.tracker
            .unverifiable
            .lock()
            .unwrap()
            .insert("12".to_string());
        let recovered = crate::queue::persist::recover(QueueSnapshot {
            jobs: vec![j, make_job("b.pdf")],
            ..QueueSnapshot::default()
        });
        h.state.write().unwrap().jobs = recovered.jobs;

        h.d.bootstrap();
        h.run_to_idle();
        assert_unconfirmed(&h.job(&id), "not_verifiable");
        h.advance_and_settle(TRACK_BACKOFFS[0]).await;
        assert!(h.tracker.polls.lock().unwrap().is_empty(), "never polled");
        assert!(h.runner.cancelled_os.lock().unwrap().is_empty());
        assert_eq!(
            h.runner.print_calls.load(Ordering::Relaxed),
            1,
            "only the other job is spooled; the recovered one is not re-sent"
        );
        // Nothing left to cancel: the finished job refuses.
        let r = h
            .request(|reply| Cmd::Cancel {
                id: id.clone(),
                reply,
            })
            .await;
        assert!(r.is_err());
        assert!(h.runner.cancelled_os.lock().unwrap().is_empty());
    }

    /// FR-034: relaunching the app does not hand a print stuck in the system
    /// queue a fresh budget — it keeps counting from when it was spooled.
    #[tokio::test(start_paused = true)]
    async fn fr_034_an_adopted_print_keeps_the_budget_it_had_used() {
        let mut h = harness_with_print_timeout(300);
        let mut j = make_job("a.pdf");
        j.state = JobState::Printing;
        j.os_job_id = Some("P-7".to_string());
        j.printer_resolved = Some("FakePrinter".to_string());
        let spooled = time::OffsetDateTime::now_utc() - time::Duration::seconds(290);
        j.spooled_at = Some(
            spooled
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap(),
        );
        let id = j.id.clone();
        h.state.write().unwrap().jobs = vec![j];
        h.tracker
            .script(std::iter::repeat_n(Ok(OsJobState::Held), 50));

        h.d.bootstrap();
        h.run_to_idle();
        for d in TRACK_BACKOFFS {
            h.advance_and_settle(d).await;
        }
        let job = h.job(&id);
        assert_eq!(
            job.state,
            JobState::Failed,
            "15 s after relaunch, 305 s after spooling: over the 300 s budget"
        );
        assert_eq!(job.error.unwrap().code, "print_timeout");
    }

    /// A fresh spool stamps when it was handed off, so a later restart can
    /// restore the budget it had used.
    #[tokio::test]
    async fn fr_034_a_spooled_job_records_when_it_was_spooled() {
        let spawner = Arc::new(DeferredSpawner::default());
        let mut h = harness_with(spawner.clone(), DispatcherLimits::default());
        let j = make_job("a.pdf");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();
        assert!(h.job(&id).spooled_at.is_none(), "not spooled yet");
        spawner.run_next(); // conversion
        h.run_to_idle();
        assert_eq!(h.job(&id).state, JobState::Printing);
        assert!(h.job(&id).spooled_at.is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn repeated_poll_failures_give_up_instead_of_holding_the_slot() {
        let mut h = harness();
        spool_with_ids(&h, 1);
        h.tracker.script(
            std::iter::repeat_with(|| Err(TrackError::Transient("busy".into())))
                .take(MAX_TRACK_FAILURES as usize),
        );
        let j = make_job("a.pdf");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle();
        for i in 0..MAX_TRACK_FAILURES - 1 {
            h.advance_and_settle(track_delay(i)).await;
            assert_eq!(h.job(&id).state, JobState::Printing, "poll {i}");
        }
        h.advance_and_settle(track_delay(MAX_TRACK_FAILURES - 1))
            .await;
        assert_unconfirmed(&h.job(&id), "tracking_failed");
    }

    // ---- Progress honesty -------------------------------------------------

    #[tokio::test]
    async fn printing_progress_is_indeterminate_not_zero() {
        // The spooler reports no fraction, so `percent` is null while printing
        // (the UI must draw an indeterminate bar) — never a bar stuck at 0 %.
        let spawner = Arc::new(DeferredSpawner::default());
        let mut h = harness_with(spawner.clone(), DispatcherLimits::default());
        let j = make_job("a.pdf");
        let id = j.id.clone();
        h.submit(vec![j]);
        h.run_to_idle(); // conversion parked
        spawner.run_next(); // conversion reports back
        h.run_to_idle(); // → Printing, spool parked

        assert_eq!(h.job(&id).state, JobState::Printing);
        let p = h
            .job(&id)
            .progress
            .expect("printing carries a progress phase");
        assert_eq!(p.phase, "printing");
        assert!(p.percent.is_none(), "no invented percentage");
    }

    // ---- FR-035 structured job log ---------------------------------------

    /// Collects the fields of every `tracing` event emitted while it is the
    /// thread's default subscriber, so the log record itself can be asserted on
    /// rather than the code that builds it.
    #[derive(Clone, Default)]
    struct LogCapture {
        events: Arc<Mutex<Vec<HashMap<String, String>>>>,
    }

    impl LogCapture {
        /// A capture to install with `with_default`.
        ///
        /// `tracing` caches each callsite's interest process-wide. A callsite
        /// first hit by a parallel test thread with no subscriber installed can
        /// be cached as `never`, and then this thread's scoped subscriber never
        /// sees the record. Installing an always-interested global registry once
        /// keeps every callsite live. A drain loop does not fix that: the
        /// harness spawner is inline, so nothing is ever left parked.
        fn install() -> Self {
            static GLOBAL: std::sync::Once = std::sync::Once::new();
            GLOBAL.call_once(|| {
                let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
            });
            Self::default()
        }

        /// The fields of the first record with this `event` name.
        fn record(&self, event: &str) -> HashMap<String, String> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .find(|e| e.get("event").map(String::as_str) == Some(event))
                .cloned()
                .unwrap_or_else(|| panic!("no {event} record was emitted"))
        }
    }

    #[derive(Default)]
    struct FieldGrab(HashMap<String, String>);

    impl tracing::field::Visit for FieldGrab {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            // Debug-quoting is uniform here, so strip it and compare values.
            self.0.insert(
                field.name().to_string(),
                format!("{value:?}").trim_matches('"').to_string(),
            );
        }
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for LogCapture {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut grab = FieldGrab::default();
            event.record(&mut grab);
            self.events.lock().unwrap().push(grab.0);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn fr_035_job_records_carry_start_end_file_profile_printer_pages_status() {
        use tracing_subscriber::layer::SubscriberExt;

        let capture = LogCapture::install();
        let subscriber = tracing_subscriber::registry().with(capture.clone());

        let mut h = harness();
        let mut j = make_job("report.pdf");
        j.profile_name = Some("Photo Color".to_string());
        j.pages = Some(3);
        j.settings.printer = Some("Office".to_string());
        let id = j.id.clone();

        // The spooler hands back an id and then reports the job completed:
        // the one way a job is `Done`.
        spool_with_ids(&h, 1);
        let guard = tracing::subscriber::set_default(subscriber);
        h.submit(vec![j]);
        h.run_to_idle();
        h.advance_and_settle(TRACK_BACKOFFS[0]).await;
        drop(guard);
        assert_eq!(h.job(&id).state, JobState::Done);

        // NFR-082: the absolute path stays out of the default-level record.
        let submitted = capture.record("job_submitted");
        assert!(
            !submitted.contains_key("input_path"),
            "job_submitted must not carry the absolute path: {submitted:?}"
        );
        assert_eq!(submitted["file"], "report.pdf");
        assert_eq!(submitted["profile"], "Photo Color");
        assert_eq!(submitted["pages"], "3");

        // Start time (FR-035) is on the stage-start records.
        for event in ["job_start_convert", "job_start_print"] {
            let rec = capture.record(event);
            assert!(!rec["started_at"].is_empty(), "{event} lacks a start time");
            assert_eq!(rec["profile"], "Photo Color");
            assert_eq!(rec["printer"], "Office");
            assert_eq!(rec["pages"], "3");
        }

        // Completion carries every FR-035 field, and no absolute path.
        let done = capture.record("job_done");
        for key in [
            "job_id",
            "file",
            "profile",
            "printer",
            "pages",
            "status",
            "started_at",
            "finished_at",
        ] {
            assert!(
                done.contains_key(key),
                "job_done is missing {key}: {done:?}"
            );
        }
        assert_eq!(done["status"], "done");
        assert_eq!(done["printer"], "Office");
        assert!(!done["finished_at"].is_empty());
        assert!(
            !done.contains_key("input_path"),
            "terminal records must not carry absolute paths"
        );
    }

    #[tokio::test]
    async fn fr_035_failure_record_carries_the_error_and_the_job_context() {
        use tracing_subscriber::layer::SubscriberExt;

        let capture = LogCapture::install();
        let subscriber = tracing_subscriber::registry().with(capture.clone());

        let mut h = harness_with_print_timeout(60);
        h.runner
            .print_steps
            .lock()
            .unwrap()
            .push_back(PrintStep::Timeout);
        let mut j = make_job("report.pdf");
        j.profile_name = Some("Quick BW".to_string());
        j.pages = Some(7);

        tracing::subscriber::with_default(subscriber, || {
            h.submit(vec![j]);
            h.run_to_idle();
        });

        let rec = capture.record("job_print_timeout");
        assert_eq!(rec["status"], "failed");
        assert_eq!(rec["error"], "print_timeout");
        assert_eq!(rec["profile"], "Quick BW");
        assert_eq!(rec["pages"], "7");
        assert_eq!(rec["file"], "report.pdf");
        assert!(!rec["started_at"].is_empty());
        assert!(!rec["finished_at"].is_empty());
    }

    // ---- FR-092: job errors carry what the UI needs to translate them -----

    fn bare_job() -> Job {
        Job::new(
            "batch",
            "/tmp/report.pdf",
            JobSettings::default(),
            None,
            now_rfc3339(),
        )
    }

    fn params(e: &JobError) -> Vec<(&str, &str)> {
        e.params
            .iter()
            .flatten()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }

    #[test]
    fn fr_092_engine_errors_carry_reason_and_params() {
        let e = to_job_error(&EngineError::UnsupportedSetting {
            setting: "duplex".into(),
            printer: "Front".into(),
        });
        assert_eq!(e.code, "unsupported_setting");
        assert_eq!(e.reason.as_deref(), Some("duplex"));
        assert_eq!(params(&e), [("printer", "Front")]);

        let e = to_job_error(&EngineError::UnsupportedSetting {
            setting: "paper size A6".into(),
            printer: "Front".into(),
        });
        assert_eq!(e.reason.as_deref(), Some("paper"));
        assert_eq!(params(&e), [("paper", "A6"), ("printer", "Front")]);

        let e = to_job_error(&EngineError::ConversionFailed("bad zip".into()));
        assert_eq!(e.reason, None);
        assert_eq!(params(&e), [("detail", "bad zip")]);
        // The English detail is kept for logs and the CLI.
        assert_eq!(e.message, "conversion failed: bad zip");

        let e = to_job_error(&EngineError::ConversionTimedOut(
            "LibreOffice did not finish within 9s and was stopped".into(),
        ));
        assert_eq!(e.code, "conversion_timeout");
        assert_eq!(e.reason.as_deref(), Some("converter"));
        assert_eq!(
            params(&e),
            [(
                "detail",
                "LibreOffice did not finish within 9s and was stopped"
            )]
        );
    }

    #[test]
    fn fr_092_print_failures_carry_reason_and_params() {
        let cases = [
            (
                PrintError::TimedOut { seconds: 30 },
                "print_timeout",
                Some("spooler"),
                vec![("secs", "30")],
            ),
            (
                PrintError::PrinterMissing {
                    printer: "Lab".into(),
                },
                "printer_unavailable",
                None,
                vec![("printer", "Lab")],
            ),
            (
                PrintError::NoPrinter,
                "printer_unavailable",
                Some("no_printer"),
                vec![],
            ),
            (
                PrintError::PrinterAmbiguous {
                    printer: "Office".into(),
                    candidates: vec!["Office_A".into(), "Office_B".into()],
                },
                "printer_unavailable",
                Some("ambiguous"),
                vec![("candidates", "Office_A, Office_B"), ("printer", "Office")],
            ),
            (
                PrintError::PrinterDisabled {
                    printer: "Lab".into(),
                },
                "printer_disabled",
                None,
                vec![("printer", "Lab")],
            ),
        ];
        for (err, code, reason, want) in cases {
            let mut job = bare_job();
            record_print_failure(&mut job, err, &mut Post::default());
            let e = job.error.expect("error recorded");
            assert_eq!(e.code, code);
            assert_eq!(e.reason.as_deref(), reason, "{code}");
            assert_eq!(params(&e), want, "{code}");
        }

        let mut job = bare_job();
        fail_print_timeout(
            &mut job,
            Some(Duration::from_secs(90)),
            &mut Post::default(),
        );
        let e = job.error.unwrap();
        assert_eq!(
            (e.code.as_str(), e.reason.as_deref()),
            ("print_timeout", None)
        );
        assert_eq!(params(&e), [("secs", "90")]);
    }

    #[test]
    fn fr_092_job_error_wire_shape_is_backward_compatible() {
        // A queue.json from before reason/params still loads...
        let old: JobError =
            serde_json::from_str(r#"{"code":"print_failed","message":"x"}"#).unwrap();
        assert_eq!(old, JobError::new("print_failed", "x"));
        // ...and an error without them is written exactly as before.
        assert_eq!(
            serde_json::to_string(&old).unwrap(),
            r#"{"code":"print_failed","message":"x"}"#
        );
        let new = JobError::new("print_timeout", "x").with_param("secs", 5);
        assert_eq!(
            serde_json::to_string(&new).unwrap(),
            r#"{"code":"print_timeout","message":"x","params":{"secs":"5"}}"#
        );
    }
}
