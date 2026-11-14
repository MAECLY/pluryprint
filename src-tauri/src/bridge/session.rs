//! One browser connection: the protocol's state machine.
//!
//! A [`Session`] owns the jobs opened on its connection. Each request is
//! answered by [`Session::handle_bytes`] with exactly one reply; the reply is
//! always a complete JSON message under the browser's 1 MB limit. Jobs that
//! are not committed when the connection ends are deleted by
//! [`Session::close`] (the documents never reach the app).
//!
//! Everything the session needs from the machine (printers, profiles, the
//! data folder, starting the app) comes through [`Host`], so the whole state
//! machine runs against fakes in unit tests and against a temporary folder
//! in the integration tests.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use sha2::Digest as _;

use super::protocol::{
    self, AbortJobReply, BeginJobReply, BeginJobRequest, ColorOverride, CommitJobReply,
    DuplexOverride, Envelope, ErrorCode, FileKind, HelloReply, HelloRequest, JobRef, Limits,
    ListPrintersReply, ListProfilesReply, OrientationOverride, PrinterEntry, ProfileEntry,
    ProtocolError, PutFileReply, PutFileRequest, Request, CAPABILITIES, MAX_CHUNK_BYTES, MAX_FILES,
    MAX_FILE_BYTES, MAX_JOB_BYTES, PROTOCOL_VERSION,
};
use super::{inbox, launch, redact, validate, SOURCE_PLURYFACT};
use crate::cli::handoff::{Handoff, HANDOFF_SCHEMA_VERSION};
use crate::cli::SelectionOverrides;

/// Most jobs one connection may have open at once.
pub const MAX_OPEN_JOBS: usize = 4;

/// Longest profile or printer id accepted in `beginJob`.
pub const MAX_REF_CHARS: usize = 256;

/// What the session needs from the machine.
pub trait Host {
    /// The printers, for `listPrinters`.
    fn printers(&self) -> Result<Vec<PrinterEntry>, String>;
    /// The profiles, for `listProfiles`.
    fn profiles(&self) -> Result<Vec<ProfileEntry>, String>;
    /// The folder holding `inbox/` and `handoff/`.
    fn data_dir(&self) -> PathBuf;
    /// Start the app on a written handoff file. `Ok` once it has started.
    fn launch(&self, args: &[String]) -> Result<(), String>;
}

/// The real machine: the OS printers, the user's profiles, the app next to
/// the bridge.
pub struct RealHost {
    data_dir: PathBuf,
    launch: bool,
}

impl RealHost {
    /// `launch` false is the test-only [`super::NO_LAUNCH_ENV`] mode.
    pub fn new(data_dir: PathBuf, launch: bool) -> Self {
        Self { data_dir, launch }
    }
}

impl Host for RealHost {
    fn printers(&self) -> Result<Vec<PrinterEntry>, String> {
        let list = crate::platform::list_printers().map_err(|e| e.to_string())?;
        Ok(list.into_iter().map(printer_entry).collect())
    }

    fn profiles(&self) -> Result<Vec<ProfileEntry>, String> {
        let paths = crate::cli::Paths::real();
        let list = crate::ipc::profiles::list_profiles_at(&paths.profiles, &paths.config)
            .map_err(|e| e.to_string())?;
        Ok(list
            .into_iter()
            .map(|p| ProfileEntry {
                id: p.id,
                name: p.name,
                is_default: p.default,
            })
            .collect())
    }

    fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    fn launch(&self, args: &[String]) -> Result<(), String> {
        if !self.launch {
            tracing::info!(event = "bridge_launch_skipped");
            return Ok(());
        }
        let me = std::env::current_exe().map_err(|e| e.to_string())?;
        let gui = launch::gui_next_to(&me);
        launch::spawn_detached(&gui, args).map_err(|e| format!("cannot start the app: {e}"))
    }
}

/// Most paper names reported per printer in `listPrinters` (keeps the reply
/// far below the 1 MB limit whatever a driver reports).
pub const MAX_PAPERS_PER_PRINTER: usize = 64;

/// Most printers reported by `listPrinters`.
pub const MAX_PRINTERS: usize = 200;

/// Above this many bytes of printers, `listPrinters` leaves out the paper
/// lists (the rest of the reply then fits in the 1 MB limit).
pub const PRINTERS_REPLY_BUDGET: usize = 900 * 1024;

/// The protocol's view of a printer.
pub fn printer_entry(p: crate::ipc::printers::Printer) -> PrinterEntry {
    PrinterEntry {
        id: p.id,
        name: p.name,
        is_default: p.is_default,
        capabilities: Some(protocol::PrinterCapabilities {
            duplex: p.duplex,
            color: p.color,
            papers: p.papers.into_iter().take(MAX_PAPERS_PER_PRINTER).collect(),
        }),
    }
}

/// A document being received.
struct Incoming {
    kind: FileKind,
    stored: String,
    size: u64,
    sha256: String,
    chunks: u32,
    next_chunk: u32,
    received: u64,
    hasher: sha2::Sha256,
    head: Vec<u8>,
    file: Option<File>,
    complete: bool,
}

impl Incoming {
    fn part_path(&self, dir: &Path) -> PathBuf {
        dir.join(format!("{}.part", self.stored))
    }
}

/// An open job.
struct Job {
    dir: PathBuf,
    label: String,
    action: protocol::JobAction,
    profile_id: Option<String>,
    overrides: SelectionOverrides,
    expected: u32,
    declared_bytes: u64,
    files: BTreeMap<u32, Incoming>,
}

/// One connection's state.
pub struct Session<H: Host> {
    host: H,
    origin_ok: bool,
    jobs: BTreeMap<String, Job>,
}

impl<H: Host> Session<H> {
    /// A session whose caller's origin has (or has not) been accepted. A
    /// refused origin gets `origin_not_allowed` for every request.
    pub fn new(host: H, origin_ok: bool) -> Self {
        Self {
            host,
            origin_ok,
            jobs: BTreeMap::new(),
        }
    }

    /// How many jobs are open (for tests).
    pub fn open_jobs(&self) -> usize {
        self.jobs.len()
    }

    /// Answer one message body with one reply body.
    pub fn handle_bytes(&mut self, body: &[u8]) -> Vec<u8> {
        let reply = match protocol::parse_request(body) {
            Err((env, e)) => protocol::error_reply(&env, &e),
            Ok((env, _)) if !self.origin_ok => protocol::error_reply(
                &env,
                &ProtocolError::new(
                    ErrorCode::OriginNotAllowed,
                    "this extension is not allowed to use Pluryprint",
                ),
            ),
            Ok((env, request)) => match self.handle(request) {
                Ok(body) => body.encode(&env),
                Err(e) => protocol::error_reply(&env, &e),
            },
        };
        if reply.len() > super::framing::MAX_OUTBOUND_BYTES {
            // Only a list could grow this far; say so instead of breaking
            // the connection with a frame the browser refuses.
            let env = protocol::parse_request(body)
                .map(|(env, _)| env)
                .unwrap_or_else(|(env, _)| env);
            return protocol::error_reply(
                &env,
                &ProtocolError::new(ErrorCode::TooLarge, "the reply would exceed 1 MB"),
            );
        }
        reply
    }

    /// The reply to a frame too large to read.
    pub fn oversized(&mut self, len: u32) -> Vec<u8> {
        protocol::error_reply(
            &Envelope::unknown(),
            &ProtocolError::new(
                ErrorCode::TooLarge,
                format!("a {len} byte message is over the 64 MiB limit"),
            ),
        )
    }

    /// The connection ended: delete every job that was not committed.
    pub fn close(&mut self) {
        let jobs = std::mem::take(&mut self.jobs);
        if !jobs.is_empty() {
            tracing::info!(event = "bridge_jobs_discarded", jobs = jobs.len());
        }
        for (_, job) in jobs {
            drop(job.files);
            let _ = inbox::remove_job_dir(&job.dir);
        }
    }

    fn handle(&mut self, request: Request) -> Result<Reply, ProtocolError> {
        match request {
            Request::Hello(h) => self.hello(h),
            Request::ListPrinters => {
                let mut printers = self
                    .host
                    .printers()
                    .map_err(|e| ProtocolError::new(ErrorCode::Internal, e))?;
                printers.truncate(MAX_PRINTERS);
                // Paper lists are the only part that can grow large; drop
                // them before the reply would brush the 1 MB limit.
                let size = serde_json::to_vec(&printers).map(|v| v.len()).unwrap_or(0);
                if size > PRINTERS_REPLY_BUDGET {
                    for p in &mut printers {
                        if let Some(c) = p.capabilities.as_mut() {
                            c.papers.clear();
                        }
                    }
                }
                Ok(Reply::Printers(ListPrintersReply { printers }))
            }
            Request::ListProfiles => {
                let profiles = self
                    .host
                    .profiles()
                    .map_err(|e| ProtocolError::new(ErrorCode::Internal, e))?;
                Ok(Reply::Profiles(ListProfilesReply { profiles }))
            }
            Request::BeginJob(b) => self.begin(b),
            Request::PutFile(p) => self.put(p),
            Request::CommitJob(j) => self.commit(j),
            Request::AbortJob(j) => self.abort(j),
        }
    }

    fn hello(&self, h: HelloRequest) -> Result<Reply, ProtocolError> {
        if !h.protocols.contains(&PROTOCOL_VERSION) {
            return Err(ProtocolError::new(
                ErrorCode::UnsupportedVersion,
                format!("this host speaks protocol version {PROTOCOL_VERSION}"),
            ));
        }
        tracing::info!(
            event = "bridge_hello",
            client = %redact::origin_for_log(&h.client),
            client_version = %redact::origin_for_log(&h.client_version),
        );
        Ok(Reply::Hello(HelloReply {
            host: "pluryprint".to_string(),
            host_version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: PROTOCOL_VERSION,
            capabilities: CAPABILITIES.iter().map(|c| c.to_string()).collect(),
            limits: Limits::current(),
        }))
    }

    fn begin(&mut self, b: BeginJobRequest) -> Result<Reply, ProtocolError> {
        if b.source != SOURCE_PLURYFACT {
            return Err(ProtocolError::bad_request("source must be \"pluryfact\""));
        }
        let label = b.label.trim().to_string();
        crate::cli::validate_label(&label).map_err(ProtocolError::bad_request)?;
        if b.expected_files == 0 {
            return Err(ProtocolError::bad_request(
                "expectedFiles must be at least 1",
            ));
        }
        if b.expected_files > MAX_FILES {
            return Err(ProtocolError::new(
                ErrorCode::TooManyFiles,
                format!("a job holds at most {MAX_FILES} documents"),
            ));
        }
        let profile_id = clean_ref(b.profile_id, "profileId")?;
        let printer_id = clean_ref(b.printer_id, "printerId")?;
        let overrides = to_selection_overrides(b.overrides.unwrap_or_default(), printer_id)?;
        if self.jobs.len() >= MAX_OPEN_JOBS {
            return Err(ProtocolError::bad_request(format!(
                "at most {MAX_OPEN_JOBS} jobs may be open at once"
            )));
        }
        let job_id = uuid::Uuid::now_v7().to_string();
        let inbox = super::inbox_dir(&self.host.data_dir());
        let dir = inbox::create_job_dir(&inbox, &job_id).map_err(io_error)?;
        tracing::info!(
            event = "bridge_job_begun",
            action = ?b.action,
            expected = b.expected_files,
            label = %redact::mask_identifiers(&label),
        );
        self.jobs.insert(
            job_id.clone(),
            Job {
                dir,
                label,
                action: b.action,
                profile_id,
                overrides,
                expected: b.expected_files,
                declared_bytes: 0,
                files: BTreeMap::new(),
            },
        );
        Ok(Reply::Begin(BeginJobReply { job_id }))
    }

    fn put(&mut self, p: PutFileRequest) -> Result<Reply, ProtocolError> {
        let job = self
            .jobs
            .get_mut(&p.job_id)
            .ok_or_else(|| unknown_job(&p.job_id))?;
        if p.index >= MAX_FILES {
            return Err(ProtocolError::new(
                ErrorCode::TooManyFiles,
                format!("a job holds at most {MAX_FILES} documents"),
            ));
        }
        if p.index >= job.expected {
            return Err(ProtocolError::new(
                ErrorCode::TooManyFiles,
                format!(
                    "index {} is past the {} announced documents",
                    p.index, job.expected
                ),
            ));
        }
        if p.size > MAX_FILE_BYTES {
            return Err(ProtocolError::new(
                ErrorCode::TooLarge,
                format!("documents are limited to {MAX_FILE_BYTES} bytes"),
            ));
        }
        if p.chunks == 0 || p.chunk >= p.chunks {
            return Err(ProtocolError::bad_request("chunk must be below chunks"));
        }
        if u64::from(p.chunks) > p.size.max(1) {
            return Err(ProtocolError::bad_request("more chunks than bytes"));
        }
        if !validate::is_sha256_hex(&p.sha256) {
            return Err(ProtocolError::bad_request(
                "sha256 must be 64 lowercase hex digits",
            ));
        }
        if validate::decoded_len_bound(p.data.len()) > MAX_CHUNK_BYTES + 2 {
            return Err(ProtocolError::new(
                ErrorCode::TooLarge,
                format!("chunks are limited to {MAX_CHUNK_BYTES} bytes"),
            ));
        }

        if p.chunk == 0 {
            match job.files.get(&p.index) {
                Some(f) if f.complete => {
                    return Err(ProtocolError::bad_request(format!(
                        "document {} was already received",
                        p.index
                    )))
                }
                // A restarted document (the extension retries after an
                // error): forget what came before.
                Some(f) => {
                    job.declared_bytes -= f.size;
                    let part = f.part_path(&job.dir);
                    job.files.remove(&p.index);
                    let _ = std::fs::remove_file(part);
                }
                None => {}
            }
            if job.declared_bytes + p.size > MAX_JOB_BYTES {
                return Err(ProtocolError::new(
                    ErrorCode::TooLarge,
                    format!("a job is limited to {MAX_JOB_BYTES} bytes"),
                ));
            }
            let stored = validate::stored_name(p.index, &validate::sanitize_name(&p.name, p.kind));
            let incoming = Incoming {
                kind: p.kind,
                stored,
                size: p.size,
                sha256: p.sha256.clone(),
                chunks: p.chunks,
                next_chunk: 0,
                received: 0,
                hasher: sha2::Sha256::new(),
                head: Vec::new(),
                file: None,
                complete: false,
            };
            let file = File::create(incoming.part_path(&job.dir)).map_err(io_error)?;
            job.declared_bytes += p.size;
            job.files.insert(
                p.index,
                Incoming {
                    file: Some(file),
                    ..incoming
                },
            );
        }

        let dir = job.dir.clone();
        let f = job
            .files
            .get_mut(&p.index)
            .ok_or_else(|| ProtocolError::bad_request("a document must start at chunk 0"))?;
        if f.complete {
            return Err(ProtocolError::bad_request(format!(
                "document {} was already received",
                p.index
            )));
        }
        if p.chunk != f.next_chunk {
            return Err(ProtocolError::bad_request(format!(
                "expected chunk {}, got {}",
                f.next_chunk, p.chunk
            )));
        }
        if p.kind != f.kind || p.size != f.size || p.sha256 != f.sha256 || p.chunks != f.chunks {
            return Err(ProtocolError::bad_request(
                "kind, size, sha256 and chunks must not change between chunks",
            ));
        }
        let bytes = validate::decode_chunk(&p.data)?;
        if bytes.len() as u64 > MAX_CHUNK_BYTES {
            return Err(ProtocolError::new(
                ErrorCode::TooLarge,
                format!("chunks are limited to {MAX_CHUNK_BYTES} bytes"),
            ));
        }
        if f.received + bytes.len() as u64 > f.size {
            return Err(ProtocolError::bad_request(
                "the document is longer than its declared size",
            ));
        }
        if f.head.len() < validate::HEAD_BYTES {
            let take = (validate::HEAD_BYTES - f.head.len()).min(bytes.len());
            f.head.extend_from_slice(&bytes[..take]);
        }
        f.hasher.update(&bytes);
        f.file
            .as_mut()
            .expect("open while incomplete")
            .write_all(&bytes)
            .map_err(io_error)?;
        f.received += bytes.len() as u64;
        f.next_chunk += 1;

        if f.next_chunk < f.chunks {
            return Ok(Reply::Put(PutFileReply {
                received: f.received,
                complete: false,
            }));
        }

        // Last chunk: the document must be whole, match its hash and look
        // like what it claims to be. Any failure discards it so the extension
        // can send it again from chunk 0.
        let mut file = f.file.take().expect("open while incomplete");
        let part = f.part_path(&dir);
        let verdict = finish(f, &mut file, &part);
        drop(file);
        match verdict {
            Ok(()) => {
                let target = dir.join(&f.stored);
                std::fs::rename(&part, &target).map_err(io_error)?;
                f.complete = true;
                Ok(Reply::Put(PutFileReply {
                    received: f.received,
                    complete: true,
                }))
            }
            Err(e) => {
                let size = f.size;
                job.files.remove(&p.index);
                job.declared_bytes -= size;
                let _ = std::fs::remove_file(&part);
                tracing::info!(
                    event = "bridge_document_refused",
                    index = p.index,
                    code = e.code.as_str()
                );
                Err(e)
            }
        }
    }

    fn commit(&mut self, j: JobRef) -> Result<Reply, ProtocolError> {
        let job = self
            .jobs
            .get(&j.job_id)
            .ok_or_else(|| unknown_job(&j.job_id))?;
        let done = job.files.values().filter(|f| f.complete).count() as u32;
        if done < job.expected {
            return Err(ProtocolError::new(
                ErrorCode::JobIncomplete,
                format!("{done} of {} documents received", job.expected),
            ));
        }
        let paths: Vec<String> = job
            .files
            .values()
            .map(|f| job.dir.join(&f.stored).to_string_lossy().into_owned())
            .collect();
        let handoff = Handoff {
            schema_version: HANDOFF_SCHEMA_VERSION,
            source: SOURCE_PLURYFACT.to_string(),
            action: Some(job.action.to_context_action()),
            profile_id: job.profile_id.clone(),
            invoked_at: crate::queue::job::now_rfc3339(),
            paths,
            truncated_from: None,
            label: Some(job.label.clone()).filter(|l| !l.is_empty()),
            overrides: Some(job.overrides.clone()).filter(|o| !o.is_empty()),
        };
        let handoff_dir = super::handoff_dir(&self.host.data_dir());
        let file = write_handoff(&handoff_dir, &j.job_id, &handoff).map_err(io_error)?;
        let args = launch::gui_args(
            &file,
            job.action.to_context_action(),
            job.profile_id.as_deref(),
        );
        if let Err(e) = self.host.launch(&args) {
            let _ = std::fs::remove_file(&file);
            tracing::warn!(event = "bridge_launch_failed", error = %e);
            return Err(ProtocolError::new(ErrorCode::LaunchFailed, e));
        }
        let files = job.expected;
        // Committed: the folder now belongs to the app (and its sweep).
        self.jobs.remove(&j.job_id);
        tracing::info!(event = "bridge_job_handed_off", files);
        Ok(Reply::Commit(CommitJobReply {
            state: "handed-off".to_string(),
            files,
        }))
    }

    fn abort(&mut self, j: JobRef) -> Result<Reply, ProtocolError> {
        let job = self
            .jobs
            .remove(&j.job_id)
            .ok_or_else(|| unknown_job(&j.job_id))?;
        drop(job.files);
        inbox::remove_job_dir(&job.dir).map_err(io_error)?;
        tracing::info!(event = "bridge_job_aborted");
        Ok(Reply::Abort(AbortJobReply {
            state: "aborted".to_string(),
        }))
    }
}

/// Verify a fully received document. `file` is the open `.part` file.
fn finish(f: &mut Incoming, file: &mut File, part: &Path) -> Result<(), ProtocolError> {
    if f.received != f.size {
        return Err(ProtocolError::bad_request(format!(
            "received {} bytes, {} were declared",
            f.received, f.size
        )));
    }
    file.flush().map_err(io_error)?;
    let digest = validate::hex(&std::mem::take(&mut f.hasher).finalize());
    if digest != f.sha256 {
        return Err(ProtocolError::new(
            ErrorCode::ChecksumMismatch,
            "the document's SHA-256 does not match",
        ));
    }
    let whole = match f.kind {
        FileKind::Pdf => None,
        FileKind::Xml | FileKind::Txt => {
            // Bounded by MAX_FILE_BYTES; XML has a tighter cap of its own.
            Some(std::fs::read(part).map_err(io_error)?)
        }
    };
    validate::check_content(f.kind, &f.head, whole.as_deref())
}

/// Write the handoff file atomically as `<dir>/pluryfact-<jobId>.json`.
pub fn write_handoff(dir: &Path, job_id: &str, handoff: &Handoff) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("pluryfact-{job_id}.json"));
    let bytes = serde_json::to_vec(handoff).map_err(std::io::Error::other)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(&bytes)?;
    tmp.persist(&path).map_err(|e| e.error)?;
    Ok(path)
}

/// A protocol id or printer id: trimmed, bounded, no control characters;
/// blank is "none".
fn clean_ref(value: Option<String>, field: &str) -> Result<Option<String>, ProtocolError> {
    let Some(v) = value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    else {
        return Ok(None);
    };
    if v.chars().count() > MAX_REF_CHARS || v.chars().any(char::is_control) {
        return Err(ProtocolError::bad_request(format!(
            "{field} is not a valid id"
        )));
    }
    Ok(Some(v))
}

/// Map the protocol's overrides (and `printerId`) onto the app's
/// [`SelectionOverrides`], with `print`'s checks.
pub fn to_selection_overrides(
    o: protocol::Overrides,
    printer: Option<String>,
) -> Result<SelectionOverrides, ProtocolError> {
    use crate::profiles::ProfileDuplex;
    use crate::queue::job::{ColorMode, Orientation, PaperSize};
    let paper = match o.paper {
        Some(name) => Some(PaperSize::parse(&name).map_err(ProtocolError::bad_request)?),
        None => None,
    };
    let out = SelectionOverrides {
        printer,
        copies: o.copies,
        duplex: o.duplex.map(|d| match d {
            DuplexOverride::Off => ProfileDuplex::Off,
            DuplexOverride::LongEdge => ProfileDuplex::LongEdge,
            DuplexOverride::ShortEdge => ProfileDuplex::ShortEdge,
        }),
        color: o.color.map(|c| match c {
            ColorOverride::Color => ColorMode::Color,
            ColorOverride::Mono => ColorMode::Monochrome,
        }),
        paper,
        orientation: o.orientation.map(|r| match r {
            OrientationOverride::Auto => Orientation::Auto,
            OrientationOverride::Portrait => Orientation::Portrait,
            OrientationOverride::Landscape => Orientation::Landscape,
        }),
        collate: o.collate,
    };
    out.validate().map_err(ProtocolError::bad_request)?;
    Ok(out)
}

fn unknown_job(job_id: &str) -> ProtocolError {
    let shown: String = job_id
        .chars()
        .take(64)
        .filter(|c| !c.is_control())
        .collect();
    ProtocolError::new(ErrorCode::UnknownJob, format!("no open job '{shown}'"))
}

fn io_error(e: std::io::Error) -> ProtocolError {
    ProtocolError::new(ErrorCode::IoError, format!("disk error: {}", e.kind()))
}

/// A successful reply, before encoding.
enum Reply {
    Hello(HelloReply),
    Printers(ListPrintersReply),
    Profiles(ListProfilesReply),
    Begin(BeginJobReply),
    Put(PutFileReply),
    Commit(CommitJobReply),
    Abort(AbortJobReply),
}

impl Reply {
    fn encode(self, env: &Envelope) -> Vec<u8> {
        match self {
            Reply::Hello(b) => protocol::ok_reply(env, b),
            Reply::Printers(b) => protocol::ok_reply(env, b),
            Reply::Profiles(b) => protocol::ok_reply(env, b),
            Reply::Begin(b) => protocol::ok_reply(env, b),
            Reply::Put(b) => protocol::ok_reply(env, b),
            Reply::Commit(b) => protocol::ok_reply(env, b),
            Reply::Abort(b) => protocol::ok_reply(env, b),
        }
    }
}

#[cfg(test)]
mod tests;
