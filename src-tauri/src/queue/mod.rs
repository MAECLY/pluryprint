//! Persistent print-job queue: domain types, on-disk state and the
//! single-writer dispatcher task that drives jobs through the
//! convert → print pipeline (see `docs/04-architecture/data-model.md` §4).
//!
//! The public surface here is small: [`QueueHandle`] is the cloneable handle the
//! IPC layer holds (via Tauri managed state) to submit work and issue control
//! commands, and [`spawn`] boots the dispatcher task and returns that handle.
//! Everything that actually mutates the queue lives behind the
//! [`dispatcher`] channel — see that module for the state-machine semantics.

pub mod job;
pub mod persist;

mod dispatcher;

// Re-exported so `crate::settings::EngineCfg`'s default can mirror the
// dispatcher's own concurrency default without exposing the whole module.
pub(crate) use dispatcher::DEFAULT_MAX_CONCURRENT_CONVERSIONS;

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::Context;
use tokio::sync::{mpsc, oneshot};

use crate::queue::job::{
    now_rfc3339, Job, JobBatch, JobOrigin, PrintRequest, QueueSnapshot, SCHEMA_VERSION,
};
use crate::util::error::{IpcError, IpcErrorCode};

/// PDFs larger than this are not parsed for a page count at submit/inspect time
/// (the parse would stall the UI); their page count stays `None` until after
/// conversion.
pub(crate) const MAX_PDF_PROBE_BYTES: u64 = 100 * 1024 * 1024;

/// The dispatcher's live, in-memory view of the queue. Written only by the
/// dispatcher task; read (cloned) by [`QueueHandle::snapshot`] and the progress
/// callback.
#[derive(Debug, Clone)]
pub struct QueueState {
    /// Whether the user has paused the whole queue (no new job starts).
    pub queue_paused: bool,
    /// Every job the dispatcher knows about, in queue order.
    pub jobs: Vec<Job>,
}

/// Cloneable handle to the running queue. Held by the IPC layer as Tauri managed
/// state; every method is cheap (a channel send, or a read-lock clone).
#[derive(Clone)]
pub struct QueueHandle {
    tx: mpsc::UnboundedSender<dispatcher::Cmd>,
    state: Arc<RwLock<QueueState>>,
}

impl QueueHandle {
    /// A consistent, serializable view of the queue for `queue_status` and the
    /// initial UI paint. Reads the live state directly — no dispatcher
    /// round-trip — so it is safe to call from any thread.
    pub fn snapshot(&self) -> QueueSnapshot {
        let st = self.state.read().unwrap();
        QueueSnapshot {
            schema_version: SCHEMA_VERSION,
            queue_paused: st.queue_paused,
            jobs: st.jobs.clone(),
        }
    }

    /// Validate a [`PrintRequest`], fan it out into one [`Job`] per file, and
    /// hand the batch to the dispatcher.
    ///
    /// Validation is synchronous and returns before the jobs are enqueued, so a
    /// bad request never reaches the queue. Each file is `stat`ed (missing →
    /// [`IpcError::FileNotFound`]), its extension checked against the engine
    /// ([`IpcError::UnsupportedFormat`]), and PDFs are probed for a page count
    /// (best effort). Settings and page-range syntax are validated up front.
    /// Performs blocking filesystem I/O, so callers run it off the UI thread.
    pub fn submit(&self, req: PrintRequest) -> Result<JobBatch, IpcError> {
        if req.files.is_empty() {
            return Err(IpcError::localized(
                IpcErrorCode::InvalidRequest,
                "no_files",
                "no files were provided to print",
            ));
        }
        req.settings.validate()?;
        if let Some(ranges) = &req.settings.page_ranges {
            crate::engine::pagerange::parse(ranges)?;
        }

        // Read the size ceiling once for the whole batch (config is loaded per
        // submit, not per file). `sanitized()` so a hand-edited `0` can't reject
        // every file.
        let max_mb = crate::settings::Config::load_or_default()
            .engine
            .sanitized()
            .max_file_size_mb;

        let submitted_at = now_rfc3339();
        let batch_id = uuid::Uuid::now_v7().to_string();
        let mut jobs = Vec::with_capacity(req.files.len());
        let mut job_ids = Vec::with_capacity(req.files.len());

        for file in &req.files {
            let path = Path::new(file);
            let meta = std::fs::metadata(path).map_err(|_| IpcError::FileNotFound(file.clone()))?;
            // The name only in the translated sentence: the full path is the
            // English detail (logs, CLI).
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| file.clone());
            if !meta.is_file() {
                return Err(IpcError::localized(
                    IpcErrorCode::InvalidRequest,
                    "not_a_file",
                    format!("{file} is not a file"),
                )
                .with_param("name", &name));
            }
            if exceeds_size_limit(meta.len(), max_mb) {
                return Err(IpcError::localized(
                    IpcErrorCode::InvalidRequest,
                    "file_too_large",
                    format!("{name} is larger than the {max_mb} MB limit"),
                )
                .with_param("name", &name)
                .with_param("limit", max_mb));
            }
            let ext = crate::engine::extension_of(path);
            if !crate::engine::is_supported(&ext) {
                // GIF is refused on purpose (decision D9) and says so.
                return Err(unsupported_format_error(file, path, &ext));
            }

            let mut job = Job::new(
                batch_id.clone(),
                file.clone(),
                req.settings.clone(),
                req.profile_name.clone(),
                submitted_at.clone(),
            );
            job.input_bytes = meta.len();
            job.pages = probe_pages(path, &ext, meta.len());
            job.origin = req.origin.as_ref().and_then(JobOrigin::bounded);
            if req.hold {
                // Paused with no cached PDF: resuming sends it back to
                // `Queued`, exactly like any job paused before conversion.
                job.state = crate::queue::job::JobState::Paused;
            }
            job_ids.push(job.id.clone());
            jobs.push(job);
        }

        self.tx
            .send(dispatcher::Cmd::Submit { jobs })
            .map_err(|_| IpcError::Internal("print queue is not running".to_string()))?;
        Ok(JobBatch { batch_id, job_ids })
    }

    /// Cancel a single job. Surfaces [`IpcError::JobNotFound`] /
    /// [`IpcError::InvalidState`] from the dispatcher.
    pub async fn cancel(&self, id: String) -> Result<(), IpcError> {
        self.request(|reply| dispatcher::Cmd::Cancel { id, reply })
            .await
    }

    /// Pause a single job. See the dispatcher docs for per-state semantics.
    pub async fn pause(&self, id: String) -> Result<(), IpcError> {
        self.request(|reply| dispatcher::Cmd::Pause { id, reply })
            .await
    }

    /// Resume a paused job back into the pipeline.
    pub async fn resume(&self, id: String) -> Result<(), IpcError> {
        self.request(|reply| dispatcher::Cmd::Resume { id, reply })
            .await
    }

    /// FR-059: hand the dispatcher the password the user typed for `id`'s
    /// encrypted PDF and resume it (and, with `apply_to_batch`, the batch's
    /// other protected PDFs). The password stays in memory only.
    pub async fn supply_password(
        &self,
        id: String,
        password: crate::engine::PdfPassword,
        apply_to_batch: bool,
    ) -> Result<(), IpcError> {
        self.request(|reply| dispatcher::Cmd::SupplyPassword {
            id,
            password,
            apply_to_batch,
            reply,
        })
        .await
    }

    /// Change a paused job's printer or sides before it is resumed.
    pub async fn adjust(
        &self,
        id: String,
        change: crate::queue::job::JobChange,
    ) -> Result<(), IpcError> {
        self.request(|reply| dispatcher::Cmd::Adjust { id, change, reply })
            .await
    }

    /// Retry a failed job from the start (attempts reset).
    pub async fn retry(&self, id: String) -> Result<(), IpcError> {
        self.request(|reply| dispatcher::Cmd::Retry { id, reply })
            .await
    }

    /// Cancel every non-terminal job; returns how many were cancelled.
    pub async fn cancel_all(&self) -> Result<u32, IpcError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(dispatcher::Cmd::CancelAll { reply: tx })
            .map_err(|_| IpcError::Internal("print queue is not running".to_string()))?;
        rx.await
            .map_err(|_| IpcError::Internal("print queue dropped the reply".to_string()))
    }

    /// Remove all terminal (done/failed/cancelled) jobs; returns how many were
    /// removed.
    pub async fn clear_done(&self) -> Result<u32, IpcError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(dispatcher::Cmd::ClearDone { reply: tx })
            .map_err(|_| IpcError::Internal("print queue is not running".to_string()))?;
        rx.await
            .map_err(|_| IpcError::Internal("print queue dropped the reply".to_string()))
    }

    /// Push changed engine settings to the running dispatcher, so the
    /// concurrency cap and both timeouts apply without a restart. Takes the
    /// config as saved; it is sanitized here, like every other consumer does.
    pub fn apply_engine_settings(&self, engine: &crate::settings::EngineCfg) {
        let limits = dispatcher::DispatcherLimits::from_engine(&engine.sanitized());
        let _ = self.tx.send(dispatcher::Cmd::SetLimits { limits });
    }

    /// Pause the whole queue (stops starting new work; in-flight continues).
    pub fn pause_queue(&self) {
        let _ = self.tx.send(dispatcher::Cmd::PauseQueue);
    }

    /// Resume the whole queue.
    pub fn resume_queue(&self) {
        let _ = self.tx.send(dispatcher::Cmd::ResumeQueue);
    }

    /// Send a reply-carrying command and await its `Result<(), IpcError>`.
    async fn request(
        &self,
        make: impl FnOnce(oneshot::Sender<Result<(), IpcError>>) -> dispatcher::Cmd,
    ) -> Result<(), IpcError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(make(tx))
            .map_err(|_| IpcError::Internal("print queue is not running".to_string()))?;
        rx.await
            .map_err(|_| IpcError::Internal("print queue dropped the reply".to_string()))?
    }
}

/// Boot the queue: recover any persisted state, sweep orphaned cache files,
/// spawn the dispatcher task, and return the [`QueueHandle`] for the IPC layer
/// to manage.
///
/// Recovery order (durable before serving): `load` → `recover` (in-flight jobs →
/// paused) → `save` (so the recovered state survives an immediate re-crash) →
/// orphan sweep of `<cache_dir>/conversions`.
pub fn spawn(app: tauri::AppHandle) -> anyhow::Result<QueueHandle> {
    let queue_file = persist::queue_file_path().context("resolving queue.json path")?;
    let cache_dir = crate::util::paths::cache_dir().join("conversions");

    let recovered = persist::recover(persist::load(&queue_file));
    if let Err(e) = persist::save(&queue_file, &recovered) {
        tracing::warn!(event = "queue_recover_persist_failed", error = %e,
            "could not persist recovered queue; continuing with in-memory state");
    }
    sweep_orphans(&cache_dir, &recovered.jobs);

    let (tx, rx) = mpsc::unbounded_channel();
    let state = Arc::new(RwLock::new(QueueState {
        queue_paused: recovered.queue_paused,
        jobs: recovered.jobs,
    }));

    let emit: Arc<dyn dispatcher::EventSink> = Arc::new(dispatcher::TauriEmit { app });
    let spawner: Arc<dyn dispatcher::BlockingSpawner> = Arc::new(dispatcher::TokioBlockingSpawner);
    let tracker: Arc<dyn dispatcher::JobTracker> = Arc::new(dispatcher::OsTracker);

    // Engine limits are read here at boot and pushed again by
    // `set_app_settings` whenever they change (`QueueHandle::set_limits`).
    // `sanitized()` clamps hand-edited values so a bricked config (0 MB / 1s
    // timeout / 99 workers) can't reach the dispatcher — every consumer reads
    // through it.
    let engine = crate::settings::Config::load_or_default()
        .engine
        .sanitized();
    let limits = dispatcher::DispatcherLimits::from_engine(&engine);

    let runner: Arc<dyn dispatcher::JobRunner> = Arc::new(dispatcher::EngineRunner { cache_dir });
    let d = dispatcher::Dispatcher::new(
        state.clone(),
        tx.clone(),
        runner,
        emit,
        spawner,
        tracker,
        queue_file,
        limits,
    );
    tauri::async_runtime::spawn(dispatcher::run(d, rx));

    Ok(QueueHandle { tx, state })
}

/// The refusal of a file whose format cannot be printed. A GIF says so by
/// name (decision D9: GIF stays unsupported, `gif_not_supported`); any other
/// type gets the plain family error. Shared with the CLI so both surfaces say
/// the same thing.
pub(crate) fn unsupported_format_error(file: &str, path: &Path, ext: &str) -> IpcError {
    if ext == "gif" {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.to_string());
        return IpcError::localized(
            crate::util::error::IpcErrorCode::UnsupportedFormat,
            "gif_not_supported",
            format!(
                "{file} is a GIF image, and GIF is not supported: convert it to PNG or JPEG first"
            ),
        )
        .with_param("name", name);
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.to_string());
    IpcError::localized(
        crate::util::error::IpcErrorCode::UnsupportedFormat,
        "not_printable",
        format!("{file} is not a printable file type"),
    )
    .with_param("name", name)
}

/// True iff `bytes` exceeds the `max_mb` megabyte ceiling. Pure `u64` math so the
/// `MB → bytes` multiplication cannot overflow for any realistic limit; the
/// settings validator keeps `max_mb` in `1..=4096`, so a legitimate limit never
/// rejects everything. Kept separate from [`MAX_PDF_PROBE_BYTES`], which gates
/// page-count probing rather than acceptance.
pub(crate) fn exceeds_size_limit(bytes: u64, max_mb: u32) -> bool {
    bytes > (max_mb as u64) * 1024 * 1024
}

/// Best-effort page count for the UI: PDFs are parsed with `lopdf` (unless
/// larger than [`MAX_PDF_PROBE_BYTES`]); pictures (images, HEIC) are always one
/// page; documents that still need converting (Office, HTML, Markdown, …) are
/// unknown until the conversion produces their PDF, at which point the
/// dispatcher counts that instead.
pub(crate) fn probe_pages(path: &Path, ext: &str, bytes: u64) -> Option<u32> {
    match ext {
        "pdf" => {
            if bytes > MAX_PDF_PROBE_BYTES {
                return None;
            }
            // A PDF that needs a password loads with no readable pages: its
            // count is unknown (FR-059), not zero.
            lopdf::Document::load(path)
                .ok()
                .filter(|d| !d.is_encrypted())
                .map(|d| d.get_pages().len() as u32)
        }
        "" => None,
        other if crate::engine::convert::is_single_page(other) => Some(1),
        _ => None,
    }
}

/// Delete cache PDFs left behind by a previous run that no live job references
/// (e.g. partial conversions interrupted by a crash). A file is orphaned iff no
/// job's `cache_pdf` points at it.
fn sweep_orphans(cache_dir: &Path, jobs: &[Job]) {
    let referenced: std::collections::HashSet<PathBuf> = jobs
        .iter()
        .filter_map(|j| j.cache_pdf.as_ref())
        .map(PathBuf::from)
        .collect();

    let entries = match std::fs::read_dir(cache_dir) {
        Ok(e) => e,
        Err(_) => return, // no cache dir yet — nothing to sweep
    };
    let mut removed = 0u32;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() && !referenced.contains(&p) && std::fs::remove_file(&p).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        tracing::info!(event = "cache_orphans_swept", removed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::job::{JobSettings, JobState, PrintRequest};
    use tempfile::TempDir;

    fn handle_with_open_channel() -> (QueueHandle, mpsc::UnboundedReceiver<dispatcher::Cmd>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let state = Arc::new(RwLock::new(QueueState {
            queue_paused: false,
            jobs: Vec::new(),
        }));
        (QueueHandle { tx, state }, rx)
    }

    fn write_file(dir: &TempDir, name: &str, bytes: &[u8]) -> String {
        let p = dir.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        p.to_string_lossy().into_owned()
    }

    fn req(files: Vec<String>, settings: JobSettings) -> PrintRequest {
        PrintRequest {
            files,
            settings,
            profile_name: None,
            hold: false,
            origin: None,
        }
    }

    /// The family and stable reason a refused submit carries (FR-092).
    fn refusal(r: Result<JobBatch, IpcError>) -> (IpcErrorCode, Option<&'static str>, IpcError) {
        let e = r.expect_err("the submit must be refused");
        (e.code(), e.reason(), e)
    }

    #[test]
    fn submit_rejects_empty_file_list() {
        let (h, _rx) = handle_with_open_channel();
        let (code, reason, _) = refusal(h.submit(req(vec![], JobSettings::default())));
        assert_eq!(code, IpcErrorCode::InvalidRequest);
        assert_eq!(reason, Some("no_files"));
    }

    #[test]
    fn submit_rejects_missing_file() {
        let (h, _rx) = handle_with_open_channel();
        assert!(matches!(
            h.submit(req(
                vec!["/no/such/file.pdf".into()],
                JobSettings::default()
            )),
            Err(IpcError::FileNotFound(_))
        ));
    }

    /// D9: a GIF is refused by name with its own reason, on every surface
    /// that uses this (the queue's submit and the CLI).
    #[test]
    fn fr_052_gif_refusal_names_the_file_and_says_why() {
        let err = unsupported_format_error("/x/anim.gif", Path::new("/x/anim.gif"), "gif");
        assert_eq!(err.reason(), Some("gif_not_supported"));
        assert!(err.to_string().contains("PNG or JPEG"), "{err}");
        let IpcError::Localized(l) = &err else {
            panic!("{err:?}")
        };
        assert_eq!(l.params["name"], "anim.gif");
        let other = unsupported_format_error("/x/a.exe", Path::new("/x/a.exe"), "exe");
        assert_eq!(other.reason(), Some("not_printable"));
    }

    #[test]
    fn submit_rejects_unsupported_extension() {
        let dir = tempfile::tempdir().unwrap();
        let f = write_file(&dir, "archive.zip", b"hi");
        let (h, _rx) = handle_with_open_channel();
        let (code, reason, e) = refusal(h.submit(req(vec![f], JobSettings::default())));
        assert_eq!(code, IpcErrorCode::UnsupportedFormat);
        assert_eq!(reason, Some("not_printable"));
        let wire = serde_json::to_value(&e).unwrap();
        assert_eq!(
            wire["params"]["name"], "archive.zip",
            "the name, not the path"
        );
    }

    /// FR-092 + decision D9: a GIF is refused with its own sentence.
    #[test]
    fn fr_092_submit_refuses_a_gif_with_its_own_reason() {
        let dir = tempfile::tempdir().unwrap();
        let f = write_file(&dir, "anim.gif", b"GIF89a");
        let (h, _rx) = handle_with_open_channel();
        let (code, reason, _) = refusal(h.submit(req(vec![f], JobSettings::default())));
        assert_eq!(code, IpcErrorCode::UnsupportedFormat);
        assert_eq!(reason, Some("gif_not_supported"));
    }

    #[test]
    fn fr_092_submit_refuses_a_folder_as_not_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let (h, _rx) = handle_with_open_channel();
        let folder = dir.path().to_string_lossy().into_owned();
        let (code, reason, _) = refusal(h.submit(req(vec![folder], JobSettings::default())));
        assert_eq!(code, IpcErrorCode::InvalidRequest);
        assert_eq!(reason, Some("not_a_file"));
    }

    #[test]
    fn submit_rejects_zero_copies() {
        let dir = tempfile::tempdir().unwrap();
        let f = write_file(&dir, "a.pdf", b"%PDF-1.4");
        let (h, _rx) = handle_with_open_channel();
        let settings = JobSettings {
            copies: 0,
            ..Default::default()
        };
        let (code, reason, _) = refusal(h.submit(req(vec![f], settings)));
        assert_eq!(code, IpcErrorCode::InvalidRequest);
        assert_eq!(reason, Some("copies_out_of_range"));
    }

    #[test]
    fn submit_rejects_bad_page_ranges() {
        let dir = tempfile::tempdir().unwrap();
        let f = write_file(&dir, "a.pdf", b"%PDF-1.4");
        let (h, _rx) = handle_with_open_channel();
        let settings = JobSettings {
            page_ranges: Some("5-1".into()),
            ..Default::default()
        };
        let (code, reason, _) = refusal(h.submit(req(vec![f], settings)));
        assert_eq!(code, IpcErrorCode::InvalidRequest);
        assert_eq!(reason, Some("page_range_backwards"));
    }

    #[test]
    fn submit_builds_batch_and_enqueues_jobs() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = write_file(&dir, "a.pdf", b"%PDF-1.4\n%stub");
        let f2 = write_file(&dir, "b.png", b"\x89PNG stub");
        let (h, mut rx) = handle_with_open_channel();
        let batch = h
            .submit(PrintRequest {
                files: vec![f1, f2],
                settings: JobSettings::default(),
                profile_name: Some("Quick".into()),
                hold: false,
                origin: None,
            })
            .unwrap();

        assert_eq!(batch.job_ids.len(), 2);
        match rx.try_recv().unwrap() {
            dispatcher::Cmd::Submit { jobs } => {
                assert_eq!(jobs.len(), 2);
                assert!(jobs.iter().all(|j| j.batch_id == batch.batch_id));
                assert!(jobs.iter().all(|j| j.input_bytes > 0));
                assert_eq!(
                    jobs.iter().map(|j| j.id.clone()).collect::<Vec<_>>(),
                    batch.job_ids
                );
            }
            _ => panic!("expected a Submit command"),
        }
    }

    /// FR-124: a batch an integration labelled carries its origin on every
    /// job (bounded), so the Queue screen can say where it came from; a batch
    /// without one stays unlabelled.
    #[test]
    fn fr_124_every_job_of_a_labelled_batch_carries_its_origin() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = write_file(&dir, "a.pdf", b"%PDF-1.4\n%stub");
        let f2 = write_file(&dir, "b.xml", b"<factura/>");
        let (h, mut rx) = handle_with_open_channel();
        h.submit(PrintRequest {
            origin: Some(JobOrigin {
                source: "pluryfact".into(),
                label: Some(format!("Recibidos · octubre 2026{}", "x".repeat(500))),
            }),
            ..req(vec![f1.clone(), f2], JobSettings::default())
        })
        .unwrap();
        match rx.try_recv().unwrap() {
            dispatcher::Cmd::Submit { jobs } => {
                assert_eq!(jobs.len(), 2);
                for j in &jobs {
                    let o = j.origin.as_ref().expect("origin on every job");
                    assert_eq!(o.source, "pluryfact");
                    let label = o.label.as_deref().unwrap();
                    assert!(label.starts_with("Recibidos · octubre 2026"));
                    assert_eq!(label.chars().count(), crate::cli::MAX_SELECTION_LABEL_CHARS);
                }
            }
            _ => panic!("expected a Submit command"),
        }
        h.submit(req(vec![f1], JobSettings::default())).unwrap();
        match rx.try_recv().unwrap() {
            dispatcher::Cmd::Submit { jobs } => assert!(jobs[0].origin.is_none()),
            _ => panic!("expected a Submit command"),
        }
    }

    /// FR-001: the context menu's "add to the queue" entry submits its batch
    /// held — every job lands `Paused`, for the user to start from the Queue —
    /// without touching the global pause flag (which would also stop every
    /// other batch already waiting).
    #[test]
    fn fr_001_a_held_request_enqueues_every_job_paused() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = write_file(&dir, "a.pdf", b"%PDF-1.4\n%stub");
        let f2 = write_file(&dir, "b.png", b"\x89PNG stub");
        let (h, mut rx) = handle_with_open_channel();
        h.submit(PrintRequest {
            hold: true,
            ..req(vec![f1, f2], JobSettings::default())
        })
        .unwrap();

        match rx.try_recv().unwrap() {
            dispatcher::Cmd::Submit { jobs } => {
                assert_eq!(jobs.len(), 2);
                assert!(
                    jobs.iter().all(|j| j.state == JobState::Paused),
                    "a held batch must not start on its own"
                );
            }
            _ => panic!("expected a Submit command"),
        }
        assert!(
            !h.snapshot().queue_paused,
            "holding one batch must not pause the whole queue"
        );
    }

    #[test]
    fn fr_001_an_unheld_request_enqueues_jobs_queued() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = write_file(&dir, "a.pdf", b"%PDF-1.4\n%stub");
        let (h, mut rx) = handle_with_open_channel();
        h.submit(req(vec![f1], JobSettings::default())).unwrap();
        match rx.try_recv().unwrap() {
            dispatcher::Cmd::Submit { jobs } => assert_eq!(jobs[0].state, JobState::Queued),
            _ => panic!("expected a Submit command"),
        }
    }

    #[test]
    fn probe_pages_classifies_by_extension() {
        // A supported image is always one page.
        assert_eq!(probe_pages(Path::new("/x/a.png"), "png", 10), Some(1));
        // Unknown / unsupported types are unknown.
        assert_eq!(probe_pages(Path::new("/x/a.txt"), "txt", 10), None);
        assert_eq!(probe_pages(Path::new("/x/a.heic"), "heic", 10), Some(1));
        // A document's page count is unknown until it is converted.
        assert_eq!(probe_pages(Path::new("/x/a.docx"), "docx", 10), None);
        assert_eq!(probe_pages(Path::new("/x/a.md"), "md", 10), None);
        // An oversized PDF is not probed.
        assert_eq!(
            probe_pages(Path::new("/x/a.pdf"), "pdf", MAX_PDF_PROBE_BYTES + 1),
            None
        );
    }

    /// FR-059: a PDF that needs a password has an unknown page count, not 0
    /// (which would make any page range look out of bounds); an owner-only
    /// one opens and is counted.
    #[test]
    fn fr_059_probe_pages_of_a_protected_pdf_is_unknown() {
        use crate::engine::convert::pdf_security::tests::{QUARTZ_OWNER, QUARTZ_USER};
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user.pdf");
        std::fs::write(&user, QUARTZ_USER).unwrap();
        assert_eq!(probe_pages(&user, "pdf", QUARTZ_USER.len() as u64), None);
        let owner = dir.path().join("owner.pdf");
        std::fs::write(&owner, QUARTZ_OWNER).unwrap();
        assert_eq!(
            probe_pages(&owner, "pdf", QUARTZ_OWNER.len() as u64),
            Some(2)
        );
    }

    #[test]
    fn exceeds_size_limit_boundaries() {
        let mb = 1024 * 1024;
        // Exactly at the limit is allowed; one byte over is rejected.
        assert!(!exceeds_size_limit(200 * mb, 200));
        assert!(exceeds_size_limit(200 * mb + 1, 200));
        assert!(!exceeds_size_limit(0, 1));
        assert!(!exceeds_size_limit(mb, 1));
        assert!(exceeds_size_limit(mb + 1, 1));
        // A large limit doesn't overflow the u64 math.
        assert!(!exceeds_size_limit(4096 * mb, 4096));
        assert!(exceeds_size_limit(4096 * mb + 1, 4096));
    }

    #[test]
    fn sweep_orphans_removes_unreferenced_files() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("conversions");
        std::fs::create_dir_all(&cache).unwrap();
        let referenced = cache.join("keep.pdf");
        let orphan = cache.join("orphan.pdf");
        std::fs::write(&referenced, b"x").unwrap();
        std::fs::write(&orphan, b"x").unwrap();

        let mut job = Job::new(
            "b",
            "/tmp/x.pdf",
            JobSettings::default(),
            None,
            now_rfc3339(),
        );
        job.cache_pdf = Some(referenced.to_string_lossy().into_owned());

        sweep_orphans(&cache, &[job]);
        assert!(referenced.exists(), "referenced cache is kept");
        assert!(!orphan.exists(), "unreferenced cache is swept");
    }
}
