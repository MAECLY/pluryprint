//! Print engine: format router, converters (everything → PDF) and per-OS
//! printer adapters (see `docs/04-architecture/print-engine.md` and ADR-0003).
//!
//! The engine has three pure-ish concerns, one per submodule:
//!
//! - [`pagerange`] — parse/validate `"1-5,10"` page-range expressions.
//! - folder expansion ([`expand_folder`]) — turn a dropped/selected directory
//!   into the list of printable files inside it (FR-004/FR-007), keeping the
//!   discarded ones with a reason (FR-005/FR-059).
//! - [`convert`] — turn a supported input file into a printable PDF (PDFs pass
//!   through; images are rasterised in-process; HEIC goes through the OS
//!   decoder; documents through LibreOffice or the system web view).
//! - [`print`] — translate [`crate::queue::job::JobSettings`] into the
//!   SumatraPDF (Windows) / CUPS `lp` (Unix) invocation and spool it.
//! - [`deps`] — locate the external programs the pipeline shells out to
//!   (SumatraPDF and the documented converters) and say what a missing one
//!   costs the user (FR-058).
//!
//! Everything is small and testable: the flag builders and parsers are pure
//! functions with co-located unit tests; only the `spool`/`cancel` entry points
//! actually spawn a process, and those are `cfg`-gated per platform.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use specta::Type;
use walkdir::WalkDir;

pub mod convert;
pub mod deps;
pub mod pagerange;
pub mod print;

/// File extensions the engine can turn into a printable PDF, grouped by
/// converter (see [`convert::route`]): PDF passthrough; raster images (the
/// in-process `image` codecs); HEIC/HEIF (the OS decoder); SVG/SVGZ (vector,
/// in-process); Office, OpenDocument and RTF (LibreOffice); TXT, HTML and
/// Markdown (the system web view); XML (an SRI receipt as a RIDE-like page,
/// anything else as a tree, then the web view). GIF is deliberately absent (decision D9):
/// it is refused with its own reason, `gif_not_supported`. A format whose converter is not installed is accepted:
/// its job pauses with the FR-058 remediation instead of being refused at drop
/// time. The UI mirrors this list in `src/lib/formats.ts` (a test diffs them).
pub const SUPPORTED_EXTS: &[&str] = &[
    "pdf", "jpg", "jpeg", "png", "tif", "tiff", "bmp", "webp", "heic", "heif", "svg", "svgz",
    "docx", "doc", "xlsx", "xls", "pptx", "ppt", "odt", "ods", "odp", "rtf", "txt", "html", "htm",
    "md", "markdown", "xml",
];

/// Whether `ext` (with or without a leading dot, any case) names a format the
/// engine can print.
pub fn is_supported(ext: &str) -> bool {
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    SUPPORTED_EXTS.contains(&ext.as_str())
}

/// The lowercased extension of `path` without the leading dot, or an empty
/// string when the path has none. Shared by the router and callers that need to
/// classify a file the same way the engine does.
pub fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Folder expansion (FR-004 / FR-005 / FR-007 / FR-059)
// ---------------------------------------------------------------------------

/// How deep a folder selection is walked (FR-004: the user is asked, the
/// backend never guesses).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum FolderScan {
    /// Only the files directly inside the folder; subfolders are reported but
    /// not entered.
    TopLevel,
    /// The whole subtree, down to [`ExpansionLimits::max_depth`].
    Recursive,
}

/// Safety ceiling on one folder expansion. Dropping a disk root must bound the
/// walk rather than wedge the app, so both the number of entries considered and
/// the depth are capped; whichever bites is reported back to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
pub struct ExpansionLimits {
    /// Most directory entries a single expansion may look at.
    pub max_files: u32,
    /// Deepest level a recursive walk descends to, relative to the root (`1` =
    /// the files directly inside it).
    pub max_depth: u32,
}

/// Default entry ceiling: comfortably above any real print batch, far below a
/// disk root.
pub const MAX_FOLDER_FILES: u32 = 5_000;

/// Default depth ceiling for a recursive walk. Deeper trees are truncated with
/// [`FolderExpansion::depth_truncated`] set.
pub const MAX_FOLDER_DEPTH: u32 = 16;

impl Default for ExpansionLimits {
    fn default() -> Self {
        Self {
            max_files: MAX_FOLDER_FILES,
            max_depth: MAX_FOLDER_DEPTH,
        }
    }
}

/// Why a file found inside a folder did not make it into the print selection.
/// FR-005/FR-059: a discarded file is never dropped silently, it comes back
/// with the reason so the caller can show it and write it to the job log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// The extension is not one [`is_supported`] accepts.
    UnsupportedFormat,
    /// Bigger than the configured `max_file_size_mb` ceiling.
    TooLarge,
    /// Could not be `stat`ed or walked (permissions, broken or looping
    /// symlink, disappeared mid-walk).
    Unreadable,
}

/// One discarded entry, with a human explanation ready for the job log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct SkippedFile {
    /// Full path of the discarded entry.
    pub path: String,
    /// File name, or the whole path when it has none.
    pub name: String,
    /// Why it was discarded, for the UI to translate.
    pub reason: SkipReason,
    /// User-facing detail (`"Unsupported file type (.docx)"`).
    pub detail: String,
}

/// The result of walking one folder: what will print, what will not and why,
/// plus whether a safety ceiling cut the walk short.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct FolderExpansion {
    /// The folder that was walked (already canonical when it came through IPC).
    pub root: String,
    /// How deep the walk went.
    pub scan: FolderScan,
    /// Printable files, sorted by full path so the print order is stable.
    pub files: Vec<String>,
    /// Everything discarded, same ordering rule.
    pub skipped: Vec<SkippedFile>,
    /// The entry ceiling was reached; the folder holds more than this.
    pub files_truncated: bool,
    /// Non-empty folders existed below the depth that was walked — expected in
    /// [`FolderScan::TopLevel`], a truncation warning in
    /// [`FolderScan::Recursive`].
    pub depth_truncated: bool,
    /// The ceilings that were applied, so the UI can phrase the warning without
    /// hardcoding them.
    pub limits: ExpansionLimits,
}

impl FolderExpansion {
    /// An empty result for `root`, used as the base of a walk and when the root
    /// itself cannot be read.
    fn empty(root: &Path, scan: FolderScan, limits: ExpansionLimits) -> Self {
        Self {
            root: root.to_string_lossy().into_owned(),
            scan,
            files: Vec::new(),
            skipped: Vec::new(),
            files_truncated: false,
            depth_truncated: false,
            limits,
        }
    }
}

/// Walk `root` and split its contents into printable files and discarded ones.
///
/// Never fails: an unreadable root or entry becomes a [`SkipReason::Unreadable`]
/// row, because a folder drop must degrade into "here is what I could use"
/// rather than an error dialog.
///
/// Symlinks are followed (a symlinked documents folder is a legitimate
/// selection), which walkdir does with loop detection — a link that points at
/// one of its own ancestors comes back as an error entry and is reported as
/// unreadable instead of spinning forever.
///
/// Ordering is deterministic twice over: the walk is sorted by file name (so
/// which entries the ceiling cuts is stable) and the two output lists are then
/// sorted by full path (so the print order is stable).
pub fn expand_folder(
    root: &Path,
    scan: FolderScan,
    limits: ExpansionLimits,
    max_file_size_mb: u32,
) -> FolderExpansion {
    let mut out = FolderExpansion::empty(root, scan, limits);
    let max_depth = match scan {
        FolderScan::TopLevel => 1,
        FolderScan::Recursive => limits.max_depth.max(1) as usize,
    };
    let max_entries = limits.max_files as usize;

    let walker = WalkDir::new(root)
        .min_depth(1)
        .max_depth(max_depth)
        .follow_links(true)
        .sort_by_file_name();

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                out.skipped.push(skipped_from_walk_error(&err));
                continue;
            }
        };

        if entry.file_type().is_dir() {
            // A directory sitting *at* the ceiling is one we will not enter:
            // say so, but only when it actually holds something.
            if entry.depth() >= max_depth && has_entries(entry.path()) {
                out.depth_truncated = true;
            }
            continue;
        }

        if out.files.len() + out.skipped.len() >= max_entries {
            out.files_truncated = true;
            break;
        }

        match classify_entry(entry.path(), max_file_size_mb) {
            Ok(path) => out.files.push(path.to_string_lossy().into_owned()),
            Err(skipped) => out.skipped.push(skipped),
        }
    }

    out.files.sort();
    out.skipped.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// Accept one file, or explain why not. Format is checked before size so a
/// folder full of huge videos reads as "unsupported file type" rather than the
/// misleading "too large"; the file-list surface
/// (`crate::ipc::jobs::inspect_files`) only ever sees files that passed both,
/// so the two orderings can never contradict each other in the UI.
fn classify_entry(path: &Path, max_file_size_mb: u32) -> Result<PathBuf, SkippedFile> {
    let ext = extension_of(path);
    if !is_supported(&ext) {
        let detail = if ext.is_empty() {
            "Unsupported file type (no extension)".to_string()
        } else {
            format!("Unsupported file type (.{ext})")
        };
        return Err(skipped(path, SkipReason::UnsupportedFormat, detail));
    }
    match std::fs::metadata(path) {
        Err(e) => Err(skipped(
            path,
            SkipReason::Unreadable,
            format!("Could not be read ({e})"),
        )),
        Ok(m) if crate::queue::exceeds_size_limit(m.len(), max_file_size_mb) => Err(skipped(
            path,
            SkipReason::TooLarge,
            format!("Larger than the {max_file_size_mb} MB limit"),
        )),
        Ok(_) => Ok(path.to_path_buf()),
    }
}

/// Build a [`SkippedFile`] for `path`, deriving the display name the same way
/// the rest of the app does.
fn skipped(path: &Path, reason: SkipReason, detail: String) -> SkippedFile {
    let full = path.to_string_lossy().into_owned();
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| full.clone());
    SkippedFile {
        path: full,
        name,
        reason,
        detail,
    }
}

/// Turn a walkdir error into a reported skip. A symlink loop is called out by
/// name: it is the one case where the *link* is fine and only the cycle is the
/// problem, and users deserve to know which one bit them.
fn skipped_from_walk_error(err: &walkdir::Error) -> SkippedFile {
    let path = err.path().unwrap_or_else(|| Path::new("")).to_path_buf();
    let detail = match err.loop_ancestor() {
        Some(ancestor) => format!(
            "Skipped: symbolic link loops back to {}",
            ancestor.display()
        ),
        None => format!("Could not be read ({err})"),
    };
    skipped(&path, SkipReason::Unreadable, detail)
}

/// Whether `dir` holds at least one entry. Used only to avoid reporting an
/// empty directory at the depth ceiling as truncated content; an unreadable
/// directory answers `false` rather than raising a second error.
fn has_entries(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|mut it| it.next().is_some())
        .unwrap_or(false)
}

/// The payload of every message-carrying [`EngineError`] variant (FR-092).
///
/// `detail` is the English sentence for logs, the CLI and the UI's last-resort
/// fallback. `reason` is the stable machine sub-code the UI translates (with
/// `params` filled in): it is registered in [`crate::util::reasons`] under the
/// job error code of its variant, and like every reason it is part of the
/// contract (`queue.json`, the CLI's JSON) and is never renamed.
///
/// A plain string still converts (`"…".into()`, `format!(..).into()`): the
/// failure then has no reason and the UI shows the family sentence with the
/// detail, exactly as before reasons existed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Failure {
    /// Stable, translatable sub-code within the variant's family.
    pub reason: Option<&'static str>,
    /// Interpolation values for the translated sentence.
    pub params: std::collections::BTreeMap<String, String>,
    /// English detail: logs, the CLI and the UI's fallback.
    pub detail: String,
}

impl Failure {
    /// A failure with a registered `reason` and its English `detail`.
    pub fn new(reason: &'static str, detail: impl Into<String>) -> Self {
        Self {
            reason: Some(reason),
            params: Default::default(),
            detail: detail.into(),
        }
    }

    /// Attach one interpolation value (builder style).
    pub fn param(mut self, name: &str, value: impl ToString) -> Self {
        self.params.insert(name.to_string(), value.to_string());
        self
    }
}

impl From<String> for Failure {
    fn from(detail: String) -> Self {
        Self {
            detail,
            ..Self::default()
        }
    }
}

impl From<&str> for Failure {
    fn from(detail: &str) -> Self {
        detail.to_string().into()
    }
}

/// Existing callers read a failure as its detail (`m.contains(..)`).
impl std::ops::Deref for Failure {
    type Target = str;
    fn deref(&self) -> &str {
        &self.detail
    }
}

impl PartialEq<str> for Failure {
    fn eq(&self, other: &str) -> bool {
        self.detail == other
    }
}

impl PartialEq<&str> for Failure {
    fn eq(&self, other: &&str) -> bool {
        self.detail == *other
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

/// Why a PDF cannot be printed as it is (FR-059). Never stripped: a PDF whose
/// author forbade printing is refused, and a password is only ever supplied by
/// the user, per job, and kept in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfProtection {
    /// The PDF is encrypted with a user (open) password and none was given.
    /// The job **needs a password**: it pauses, and the GUI asks for one
    /// (`job_supply_password`) or the CLI takes `--password-file`/stdin.
    PasswordRequired,
    /// A password was given but it does not open the PDF. Also actionable:
    /// the job pauses so the user can try again.
    PasswordIncorrect,
    /// The PDF opens, but its permissions forbid printing. Terminal.
    PrintingNotAllowed,
    /// The PDF uses a security handler the engine cannot read (detail says
    /// which). Terminal.
    Unsupported(String),
}

impl PdfProtection {
    /// The stable reason of this protection under the `pdf_protected` job code
    /// and the `PdfProtected` IPC family. Never renamed.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::PasswordRequired => "password_required",
            Self::PasswordIncorrect => "password_incorrect",
            Self::PrintingNotAllowed => "printing_not_allowed",
            Self::Unsupported(_) => "unsupported_encryption",
        }
    }

    /// Whether the user can unblock the job by supplying a (different)
    /// password, as opposed to a refusal no password changes.
    pub fn needs_password(&self) -> bool {
        matches!(self, Self::PasswordRequired | Self::PasswordIncorrect)
    }
}

impl std::fmt::Display for PdfProtection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PasswordRequired => f.write_str("the PDF is password-protected"),
            Self::PasswordIncorrect => f.write_str("the password does not open the PDF"),
            Self::PrintingNotAllowed => f.write_str("the PDF does not allow printing"),
            Self::Unsupported(d) => write!(f, "the PDF uses unsupported encryption ({d})"),
        }
    }
}

/// A password the user typed to open an encrypted PDF (FR-059, NFR-082).
///
/// Lives only in memory: it is not `Serialize`, its `Debug` is redacted, and
/// its buffer is wiped when the last copy is dropped. It reaches the engine
/// per job (the GUI's `job_supply_password`, the CLI's `--password-file` /
/// `--password-stdin`), never through `queue.json`, a profile, argv or a log.
#[derive(Clone)]
pub struct PdfPassword(zeroize::Zeroizing<String>);

impl PdfPassword {
    /// Wrap a typed password. `None` for an empty one, which is never a
    /// password the user meant to give.
    pub fn new(password: String) -> Option<Self> {
        let password = zeroize::Zeroizing::new(password);
        (!password.is_empty()).then_some(Self(password))
    }

    /// The password itself, for the one call that opens the PDF.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for PdfPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PdfPassword(<redacted>)")
    }
}

/// Every way the engine can fail. Messages are user-facing: they surface in the
/// queue UI and the job log, so they are written for a person, not a developer.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// A converter could not turn the input into a valid PDF (bad/corrupt input,
    /// unreadable file, unsupported image sub-format such as CMYK JPEG).
    #[error("conversion failed: {0}")]
    ConversionFailed(Failure),

    /// A converter ran past the conversion timeout and was killed. Separate
    /// from [`EngineError::ConversionFailed`] because it is terminal: the queue
    /// fails the job with `conversion_timeout` instead of retrying a run that
    /// already used its whole budget (FR-034).
    #[error("conversion timed out: {0}")]
    ConversionTimedOut(Failure),

    /// The OS spooler/printer backend rejected the job.
    #[error("printing failed: {0}")]
    PrintFailed(Failure),

    /// A requested setting the driver cannot honour and that would otherwise
    /// silently degrade the output (currently only duplex — see
    /// [`print::check_supported`]).
    #[error("the driver for '{printer}' does not support {setting}")]
    UnsupportedSetting {
        /// The setting that cannot be honoured, e.g. `"duplex"`.
        setting: String,
        /// The printer whose driver lacks it.
        printer: String,
    },

    /// A required external program (CUPS `lp`, SumatraPDF, LibreOffice, the
    /// HEIF codec) is not installed or could not be located. The detail is
    /// the FR-058 remediation text; the reason, when set, is the dependency id
    /// (`common:deps.<id>.remediation`).
    #[error("missing dependency: {0}")]
    DependencyMissing(Failure),

    /// The file's format is not one the engine can print at all. Terminal:
    /// retrying never changes a file's format.
    #[error("unsupported file format: {0}")]
    UnsupportedFormat(Failure),

    /// The PDF is encrypted or forbids printing (FR-059). A missing or wrong
    /// password pauses the job until the user supplies one; the other cases
    /// are terminal.
    #[error("protected PDF: {0}")]
    PdfProtected(PdfProtection),

    /// The job's cancellation flag was set; work stopped at a stage boundary.
    #[error("job cancelled")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn is_supported_is_case_insensitive_and_dot_tolerant() {
        assert!(is_supported("pdf"));
        assert!(is_supported("PDF"));
        assert!(is_supported("JpEg"));
        assert!(is_supported(".png"));
        assert!(is_supported("TIFF"));
        assert!(is_supported("DOCX"));
        assert!(is_supported(".md"));
        assert!(is_supported("SVG"));
        assert!(is_supported(".svgz"));
        assert!(!is_supported("gif")); // D9: GIF is refused on purpose
        assert!(!is_supported("exe"));
        assert!(!is_supported(""));
    }

    #[test]
    fn fr_052_to_057_supported_exts_are_the_expected_set() {
        assert_eq!(
            SUPPORTED_EXTS,
            &[
                "pdf", "jpg", "jpeg", "png", "tif", "tiff", "bmp", "webp", "heic", "heif", "svg",
                "svgz", "docx", "doc", "xlsx", "xls", "pptx", "ppt", "odt", "ods", "odp", "rtf",
                "txt", "html", "htm", "md", "markdown",
            ]
        );
    }

    /// FR-092: a failure keeps its stable reason and params next to the
    /// English detail, and still reads as that detail.
    #[test]
    fn fr_092_failure_carries_reason_params_and_detail() {
        let f = Failure::new("cannot_read_input", "cannot open a.pdf").param("file", "a.pdf");
        assert_eq!(f.reason, Some("cannot_read_input"));
        assert_eq!(f.params.get("file").map(String::as_str), Some("a.pdf"));
        assert!(f.contains("cannot open"));
        assert_eq!(f.to_string(), "cannot open a.pdf");

        let plain: Failure = "boom".into();
        assert_eq!(plain.reason, None);
        assert!(plain == "boom");
        let e = EngineError::ConversionFailed(format!("x{}", 1).into());
        assert_eq!(e.to_string(), "conversion failed: x1");
    }

    /// FR-059: the protections that a password fixes are the ones the GUI
    /// and the CLI act on; their reasons are the stable wire names.
    #[test]
    fn fr_059_pdf_protection_reasons_and_needs_password() {
        use PdfProtection as P;
        let all = [
            (P::PasswordRequired, "password_required", true),
            (P::PasswordIncorrect, "password_incorrect", true),
            (P::PrintingNotAllowed, "printing_not_allowed", false),
            (P::Unsupported("V9".into()), "unsupported_encryption", false),
        ];
        for (p, reason, needs) in all {
            assert_eq!(p.reason(), reason);
            assert_eq!(p.needs_password(), needs, "{p:?}");
            assert!(crate::util::reasons::is_job_reason("pdf_protected", reason));
        }
        assert_eq!(
            EngineError::PdfProtected(P::PasswordRequired).to_string(),
            "protected PDF: the PDF is password-protected"
        );
    }

    /// NFR-082: a supplied password never shows up in a log line or a debug
    /// dump, and an empty one is not a password.
    #[test]
    fn fr_059_pdf_password_is_redacted_and_never_empty() {
        let p = PdfPassword::new("s3cret".into()).unwrap();
        assert_eq!(p.expose(), "s3cret");
        assert!(!format!("{p:?}").contains("s3cret"));
        assert!(PdfPassword::new(String::new()).is_none());
    }

    #[test]
    fn extension_of_lowercases_and_handles_missing() {
        assert_eq!(extension_of(Path::new("/a/b/report.PDF")), "pdf");
        assert_eq!(extension_of(Path::new("/a/b/photo.JPeG")), "jpeg");
        assert_eq!(extension_of(Path::new("/a/b/Makefile")), "");
    }
}
