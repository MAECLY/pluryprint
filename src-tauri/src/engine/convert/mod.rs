//! Input → PDF router.
//!
//! Every job's file is normalised to a single printable PDF before it reaches
//! the printer backend (ADR-0003's "convert-everything-to-PDF" strategy). This
//! module owns the routing decision ([`route`]); each converter lives in its own
//! submodule:
//!
//! | Route | Extensions | Converter |
//! |---|---|---|
//! | [`Route::Passthrough`] | `pdf` | none — header, `%%EOF` and page check, printed in place |
//! | [`Route::Image`] | [`IMAGE_EXTS`] | in-process [`image`] crate |
//! | [`Route::Heif`] | [`heif::HEIF_EXTS`] | OS decoder: `sips` (macOS) / WIC (Windows) |
//! | [`Route::Svg`] | [`svg::SVG_EXTS`] | `usvg` + `svg2pdf` in the SVG worker, under a hard memory cap (FR-052a) |
//! | [`Route::Office`] | [`office::OFFICE_EXTS`] | LibreOffice headless (`textutil` for RTF on macOS) |
//! | [`Route::Html`] | [`html::HTML_EXTS`] | the system web view (render worker) |
//! | [`Route::Markdown`] | [`markdown::MARKDOWN_EXTS`] | Markdown → HTML → the HTML route |
//! | [`Route::Text`] | [`text::TEXT_EXTS`] | decoded text → HTML → the HTML route |
//!
//! A PDF is inspected for encryption first ([`pdf_security`], FR-059): one
//! that needs a password is refused with [`EngineError::PdfProtected`] unless
//! the job carries one, in which case a decrypted copy is printed.
//!
//! External converters run in a private scratch directory ([`WorkDir`], removed
//! when the conversion ends, success or not), are bounded by the conversion
//! timeout and the job's cancel flag ([`process::run_bounded`] kills the child
//! tree), and a missing program is [`EngineError::DependencyMissing`] carrying
//! the FR-058 remediation text, which pauses the job instead of failing it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::deps::{self, Dependency};
use super::{extension_of, EngineError, Failure};

pub mod heif;
pub mod html;
pub mod html_scan;
pub mod image;
pub mod local_ref;
pub mod markdown;
pub mod office;
pub mod pdf_security;
pub mod process;
#[cfg(target_os = "macos")]
pub mod rtf_mac;
pub mod svg;
pub mod text;

#[cfg(test)]
mod external_tests;

/// Raster image extensions the in-process converter decodes.
pub const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "bmp", "webp"];

pub use heif::HEIF_EXTS;
pub use html::HTML_EXTS;
pub use markdown::MARKDOWN_EXTS;
pub use office::OFFICE_EXTS;
pub use svg::SVG_EXTS;
pub use text::TEXT_EXTS;

/// The page geometry a converter that lays pages out itself must honour.
///
/// Office documents carry their own page size, images are fitted by the
/// printer, but HTML and Markdown are *paginated by the web view*, which
/// otherwise uses US Letter whatever the job asks for: an A4 job then
/// printed Letter pages, clipped on the right at actual size. The web view
/// honours a CSS `@page { size: … }` rule, so this carries that value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageSetup {
    /// The CSS `size` value (`210mm 297mm`), or `None` to leave the
    /// browser's default (paper `auto`, or a size we cannot measure).
    pub css_size: Option<String>,
}

impl PageSetup {
    /// The page geometry `settings` asks for.
    pub fn for_settings(settings: &crate::queue::job::JobSettings) -> Self {
        Self {
            css_size: css_page_size(&settings.paper_size, settings.orientation),
        }
    }
}

/// Width × height of a paper name in portrait, as CSS lengths. `None` for
/// `auto` and for names we cannot measure (the web view's default then applies).
///
/// Knows the ISO A/B sizes, the US sizes, `4x6`-style inch sizes and CUPS'
/// self-describing `w<pt>h<pt>` keywords, matched case-insensitively; a
/// Windows name such as `A4 210 x 297 mm` is read by its first word.
fn paper_dimensions(paper: &crate::queue::job::PaperSize) -> Option<(String, String)> {
    if paper.is_auto() {
        return None;
    }
    let name = paper.as_str().trim().to_ascii_lowercase();
    let first = name.split_whitespace().next().unwrap_or("");
    let mm = |w: &str, h: &str| Some((format!("{w}mm"), format!("{h}mm")));
    let inch = |w: &str, h: &str| Some((format!("{w}in"), format!("{h}in")));
    let named = match first {
        "a3" => mm("297", "420"),
        "a4" => mm("210", "297"),
        "a5" => mm("148", "210"),
        "a6" => mm("105", "148"),
        "b4" => mm("250", "353"),
        "b5" => mm("176", "250"),
        "letter" => inch("8.5", "11"),
        "legal" => inch("8.5", "14"),
        "tabloid" | "ledger" => inch("11", "17"),
        "executive" => inch("7.25", "10.5"),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    let number = |s: &str| {
        s.parse::<f64>()
            .ok()
            .filter(|v| *v > 0.0)
            .map(|_| s.to_string())
    };
    // `4x6`, `5x7`: inches.
    if let Some((w, h)) = first.split_once('x') {
        if let (Some(w), Some(h)) = (number(w), number(h)) {
            return inch(&w, &h);
        }
    }
    // `w288h432`: points.
    if let Some((w, h)) = first.strip_prefix('w').and_then(|r| r.split_once('h')) {
        if let (Some(w), Some(h)) = (number(w), number(h)) {
            return Some((format!("{w}pt"), format!("{h}pt")));
        }
    }
    None
}

/// The CSS `@page` `size` for `paper` in `orientation` (`auto` keeps the
/// paper's own portrait shape). Pure.
pub fn css_page_size(
    paper: &crate::queue::job::PaperSize,
    orientation: crate::queue::job::Orientation,
) -> Option<String> {
    let (w, h) = paper_dimensions(paper)?;
    Some(match orientation {
        crate::queue::job::Orientation::Landscape => format!("{h} {w}"),
        _ => format!("{w} {h}"),
    })
}

/// Which converter handles a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Already a PDF: printed as is.
    Passthrough,
    /// A raster image, laid onto a PDF page in-process.
    Image,
    /// HEIC/HEIF photo, decoded by the OS's own HEIF decoder first.
    Heif,
    /// Office document, converted by LibreOffice.
    Office,
    /// HTML page, rendered to PDF by the system web view.
    Html,
    /// Markdown, turned into HTML and then rendered like an HTML page.
    Markdown,
    /// SVG/SVGZ drawing, converted to a vector PDF in-process (FR-052a).
    Svg,
    /// Plain text, decoded and laid out as an HTML page (FR-057).
    Text,
}

/// The converter for an extension (with or without a dot, any case), or
/// `None` when the engine cannot print it.
pub fn route(ext: &str) -> Option<Route> {
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    let ext = ext.as_str();
    if ext == "pdf" {
        Some(Route::Passthrough)
    } else if IMAGE_EXTS.contains(&ext) {
        Some(Route::Image)
    } else if HEIF_EXTS.contains(&ext) {
        Some(Route::Heif)
    } else if SVG_EXTS.contains(&ext) {
        Some(Route::Svg)
    } else if TEXT_EXTS.contains(&ext) {
        Some(Route::Text)
    } else if OFFICE_EXTS.contains(&ext) {
        Some(Route::Office)
    } else if HTML_EXTS.contains(&ext) {
        Some(Route::Html)
    } else if MARKDOWN_EXTS.contains(&ext) {
        Some(Route::Markdown)
    } else {
        None
    }
}

/// Whether a file of this extension always becomes exactly one page (a
/// picture or a drawing). Documents have an unknown page count until they are
/// converted.
pub fn is_single_page(ext: &str) -> bool {
    matches!(route(ext), Some(Route::Image | Route::Heif | Route::Svg))
}

/// A private scratch directory for one conversion, deleted on drop — so every
/// exit path (success, failure, cancel, timeout, unwinding) cleans up.
pub struct WorkDir(tempfile::TempDir);

impl WorkDir {
    /// Create a fresh scratch directory in the OS temp folder.
    pub fn new(job_id: &str) -> Result<Self, EngineError> {
        Self::create(job_id, None)
    }

    /// Create a fresh scratch directory inside `parent` (created if needed),
    /// for a converter that cannot read the OS temp folder.
    pub fn new_in(job_id: &str, parent: &Path) -> Result<Self, EngineError> {
        Self::create(job_id, Some(parent))
    }

    fn create(job_id: &str, parent: Option<&Path>) -> Result<Self, EngineError> {
        // Job ids are UUIDs; anything else is reduced to a safe prefix.
        let tag: String = job_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .take(40)
            .collect();
        let prefix = format!("pluryprint-convert-{tag}-");
        let mut builder = tempfile::Builder::new();
        builder.prefix(&prefix);
        match parent {
            Some(parent) => {
                std::fs::create_dir_all(parent).and_then(|()| builder.tempdir_in(parent))
            }
            None => builder.tempdir(),
        }
        .map(WorkDir)
        .map_err(|e| {
            EngineError::ConversionFailed(Failure::new(
                "workdir_failed",
                format!("cannot create a conversion folder: {e}"),
            ))
        })
    }

    /// The directory's path.
    pub fn path(&self) -> &Path {
        self.0.path()
    }
}

/// Route `input` to the right converter and produce a printable PDF.
///
/// Returns:
/// - `Ok(None)` when `input` is already a valid PDF (print the original).
/// - `Ok(Some(path))` with the freshly-written `cache_dir/<job_id>.pdf`.
/// - `Err(..)` for unreadable/corrupt files, unsupported formats, a missing
///   converter ([`EngineError::DependencyMissing`]), a timeout, or when
///   `cancel` has been set.
///
/// `cancel` is checked at each stage boundary and polled continuously while an
/// external converter runs (raising it kills the converter). `timeout` bounds
/// each external converter on its own; the dispatcher additionally raises
/// `cancel` when its conversion timeout fires. `progress` receives
/// `0.0..=1.0` updates. `page` is the paper the job prints on, which the
/// web-view converters (HTML, Markdown, text) lay their pages out to.
///
/// `password` opens an encrypted PDF (FR-059). Without one, such a PDF fails
/// with [`EngineError::PdfProtected`]`(`[`super::PdfProtection::PasswordRequired`]`)`;
/// with one, the decrypted copy is written to `cache_dir/<job_id>.pdf` and
/// returned like any converted file. It is ignored for every other input.
#[allow(clippy::too_many_arguments)]
pub fn convert_to_pdf(
    input: &Path,
    cache_dir: &Path,
    job_id: &str,
    cancel: &AtomicBool,
    progress: &(dyn Fn(f32) + Send + Sync),
    timeout: Option<Duration>,
    page: &PageSetup,
    password: Option<&super::PdfPassword>,
) -> Result<Option<PathBuf>, EngineError> {
    check_cancel(cancel)?;

    let ext = extension_of(input);
    if ext.is_empty() {
        return Err(EngineError::UnsupportedFormat(Failure::new(
            "no_extension",
            format!("{} has no file extension", input.display()),
        )));
    }
    let Some(route) = route(&ext) else {
        let reason = if ext == "gif" {
            "gif_not_supported"
        } else {
            "unsupported_type"
        };
        return Err(EngineError::UnsupportedFormat(
            Failure::new(reason, ext.clone()).param("ext", &ext),
        ));
    };

    if route == Route::Passthrough {
        validate_pdf_header(input)?;
        let security = validate_pdf_body(input)?;
        check_cancel(cancel)?;
        return match security {
            pdf_security::PdfSecurity::NeedsPassword => {
                let Some(password) = password else {
                    return Err(EngineError::PdfProtected(
                        super::PdfProtection::PasswordRequired,
                    ));
                };
                let output = cache_output(cache_dir, job_id)?;
                match pdf_security::decrypt_to(input, password, &output)
                    .and_then(|()| check_cancel(cancel))
                {
                    Ok(()) => Ok(Some(output)),
                    Err(e) => {
                        let _ = std::fs::remove_file(&output);
                        Err(e)
                    }
                }
            }
            _ => Ok(None),
        };
    }

    // Resolve the converter before touching the disk, so a missing program
    // pauses the job without leaving anything behind. The HTML, Markdown and
    // text converters resolve whatever they need themselves.
    let tools = match route {
        Route::Office if !rtf_natively(&ext) => vec![locate(Dependency::LibreOffice)?],
        // Linux has no system HEIF decoder: an external one is a program
        // like LibreOffice (macOS uses `sips`, Windows WIC).
        Route::Heif if deps::Os::current() == deps::Os::Linux => {
            vec![locate(Dependency::HeifDecoder)?]
        }
        _ => Vec::new(),
    };

    let output = cache_output(cache_dir, job_id)?;
    check_cancel(cancel)?;

    let result = run_route(
        route, &ext, &tools, input, &output, job_id, cancel, progress, timeout, page,
    )
    .and_then(|()| check_cancel(cancel));
    match result {
        Ok(()) => Ok(Some(output)),
        Err(e) => {
            // Never leave a half-written cache PDF behind.
            let _ = std::fs::remove_file(&output);
            Err(e)
        }
    }
}

/// The cache PDF a conversion of `job_id` writes, its folder created.
fn cache_output(cache_dir: &Path, job_id: &str) -> Result<PathBuf, EngineError> {
    std::fs::create_dir_all(cache_dir).map_err(|e| {
        EngineError::ConversionFailed(Failure::new(
            "workdir_failed",
            format!("cannot create cache directory {}: {e}", cache_dir.display()),
        ))
    })?;
    Ok(cache_dir.join(format!("{job_id}.pdf")))
}

/// Whether this RTF is converted by the OS (`textutil` on macOS, FR-057)
/// instead of LibreOffice.
fn rtf_natively(ext: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        ext == "rtf" && rtf_mac::available()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = ext;
        false
    }
}

#[allow(clippy::too_many_arguments)]
fn run_route(
    route: Route,
    ext: &str,
    tools: &[PathBuf],
    input: &Path,
    output: &Path,
    job_id: &str,
    cancel: &AtomicBool,
    progress: &(dyn Fn(f32) + Send + Sync),
    timeout: Option<Duration>,
    page: &PageSetup,
) -> Result<(), EngineError> {
    let tool = |i: usize| -> Result<&Path, EngineError> {
        tools.get(i).map(PathBuf::as_path).ok_or_else(|| {
            EngineError::ConversionFailed(
                "internal error: converter not resolved".to_string().into(),
            )
        })
    };
    match route {
        Route::Passthrough => Ok(()),
        Route::Image => image::to_pdf(input, output, progress),
        Route::Heif => {
            let work = WorkDir::new(job_id)?;
            let decoder = tools.first().map(PathBuf::as_path);
            heif::convert(decoder, input, output, &work, cancel, timeout, progress)
        }
        Route::Svg => svg::convert(input, output, cancel, timeout, progress),
        Route::Office => {
            // A sandboxed LibreOffice (snap, Flatpak) cannot read the system
            // temp folder: its conversion folder goes where it can.
            let work = match tools
                .first()
                .and_then(|t| office::confined_work_parent(t, home_dir().as_deref()))
            {
                Some(parent) => WorkDir::new_in(job_id, &parent)?,
                None => WorkDir::new(job_id)?,
            };
            progress(0.0);
            if rtf_natively(ext) {
                #[cfg(target_os = "macos")]
                rtf_mac::convert(input, output, &work, cancel, timeout, page)?;
            } else {
                office::convert_with(tool(0)?, input, output, &work, cancel, timeout)?;
            }
            progress(1.0);
            Ok(())
        }
        Route::Html => {
            let work = WorkDir::new(job_id)?;
            progress(0.0);
            html::convert(input, output, &work, cancel, timeout, page)?;
            progress(1.0);
            Ok(())
        }
        Route::Markdown => {
            let work = WorkDir::new(job_id)?;
            progress(0.0);
            markdown::convert(input, output, &work, cancel, timeout, page)?;
            progress(1.0);
            Ok(())
        }
        Route::Text => {
            let work = WorkDir::new(job_id)?;
            progress(0.0);
            text::convert(input, output, &work, cancel, timeout, page)?;
            progress(1.0);
            Ok(())
        }
    }
}

/// Find `dep` on this machine, or explain how to install it (FR-058).
pub fn locate(dep: Dependency) -> Result<PathBuf, EngineError> {
    let os = deps::Os::current();
    match deps::resolve_on_host(dep, os, None) {
        deps::Resolution::Found { path, .. } => Ok(path),
        // The dependency's id is the reason, so the paused job says the
        // shared `common:deps.<id>` remediation and offers its install.
        deps::Resolution::Missing => Err(EngineError::DependencyMissing(Failure::new(
            dep.id(),
            deps::spec(dep, os).remediation(),
        ))),
    }
}

/// The user's home folder, for the converters that must work inside it.
fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
}

/// Move a converter's `produced` PDF to `output`, after checking it really is
/// a PDF. `log` is the converter's stderr, quoted when it is not.
pub(crate) fn take_output(
    produced: &Path,
    output: &Path,
    label: &str,
    log: &Path,
) -> Result<(), EngineError> {
    if validate_pdf_header(produced).is_err() {
        let detail = process::log_tail(log, 400);
        let message = if detail.is_empty() {
            format!("{label} could not convert this file to PDF")
        } else {
            format!("{label} could not convert this file to PDF: {detail}")
        };
        return Err(EngineError::ConversionFailed(
            Failure::new("converter_failed", message).param("tool", label),
        ));
    }
    if std::fs::rename(produced, output).is_err() {
        // Scratch and cache can sit on different volumes; fall back to a copy.
        std::fs::copy(produced, output).map_err(|e| {
            EngineError::ConversionFailed(Failure::new(
                "cannot_write_output",
                format!("cannot write {}: {e}", output.display()),
            ))
        })?;
    }
    Ok(())
}

/// `file://` URL for a local absolute path, percent-encoded. Used for the
/// LibreOffice profile location, the page the web view prints and the
/// Markdown `<base href>`.
pub fn file_url(path: &Path) -> String {
    file_url_from(&path.to_string_lossy(), cfg!(windows))
}

/// [`file_url`] with the path flavour explicit, so both are testable anywhere.
pub fn file_url_from(path: &str, windows: bool) -> String {
    use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
    const KEEP: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'/')
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~')
        .remove(b':');
    if !windows {
        return format!("file://{}", utf8_percent_encode(path, KEEP));
    }
    let mut p = path.replace('\\', "/");
    if let Some(rest) = p.strip_prefix("//?/UNC/") {
        p = format!("//{rest}");
    } else if let Some(rest) = p.strip_prefix("//?/") {
        p = rest.to_string();
    }
    let encoded = utf8_percent_encode(&p, KEEP).to_string();
    match encoded.strip_prefix("//") {
        // UNC share: the server is the URL host.
        Some(unc) => format!("file://{unc}"),
        None => format!("file:///{encoded}"),
    }
}

/// Cheap structural check that `input` really is a PDF. We only read the first
/// bytes: a real converter would parse the whole file, but for passthrough all
/// we need is to reject a mislabelled/corrupt file before handing it to the
/// spooler.
fn validate_pdf_header(input: &Path) -> Result<(), EngineError> {
    use std::io::Read;
    let mut file = std::fs::File::open(input).map_err(|e| cannot_read(input, "open", e))?;
    let mut header = [0u8; 5];
    let read = file
        .read(&mut header)
        .map_err(|e| cannot_read(input, "read", e))?;
    if read == 5 && &header == b"%PDF-" {
        Ok(())
    } else {
        Err(EngineError::ConversionFailed(Failure::new(
            "pdf_invalid",
            "not a valid PDF",
        )))
    }
}

/// How far from the end a PDF's `%%EOF` marker may sit. The spec puts it in
/// the last line; writers that append a little padding or a signature after
/// it stay well inside this.
const PDF_EOF_WINDOW: u64 = 8 * 1024;

/// A cheap check that a PDF printed in place is a whole document, beyond its
/// header: a file that only *starts* like a PDF (a truncated download, a
/// mislabelled file) used to be spooled and reported done while the printer
/// produced nothing.
///
/// Two checks, both conservative so a real document is never refused:
/// - the `%%EOF` marker must be near the end (a truncated file has none);
/// - when the file is small enough to parse ([`crate::queue::MAX_PDF_PROBE_BYTES`])
///   and parses, it must have at least one page. A file `lopdf` cannot parse
///   is still handed to the spooler, which reads more PDF dialects than it.
///
/// Encryption is inspected in between (FR-059), **before** the page check: an
/// encrypted file loads with no readable pages, which must not read as "the
/// document has no pages". A PDF that forbids printing is refused here; one
/// that needs a password is reported back so the caller can decrypt it.
fn validate_pdf_body(input: &Path) -> Result<pdf_security::PdfSecurity, EngineError> {
    use std::io::{Read, Seek, SeekFrom};
    let fail = |reason: &'static str, why: &str| {
        EngineError::ConversionFailed(Failure::new(reason, format!("not a valid PDF: {why}")))
    };
    let mut file = std::fs::File::open(input).map_err(|e| cannot_read(input, "open", e))?;
    let size = file
        .metadata()
        .map_err(|e| cannot_read(input, "read", e))?
        .len();
    let window = size.min(PDF_EOF_WINDOW);
    let mut tail = Vec::with_capacity(window as usize);
    file.seek(SeekFrom::End(-(window as i64)))
        .and_then(|_| file.read_to_end(&mut tail))
        .map_err(|e| cannot_read(input, "read", e))?;
    if !tail.windows(5).any(|w| w == b"%%EOF") {
        return Err(fail(
            "pdf_truncated",
            "the file is truncated or damaged (it has no end-of-file marker)",
        ));
    }
    let security = pdf_security::inspect(input, size)?;
    match &security {
        pdf_security::PdfSecurity::OwnerOnly { print: false } => {
            return Err(EngineError::PdfProtected(
                super::PdfProtection::PrintingNotAllowed,
            ));
        }
        pdf_security::PdfSecurity::NeedsPassword => return Ok(security),
        _ => {}
    }
    if size <= crate::queue::MAX_PDF_PROBE_BYTES {
        match lopdf::Document::load(input) {
            Ok(doc) if doc.get_pages().is_empty() => {
                return Err(fail("pdf_no_pages", "the document has no pages"));
            }
            Ok(_) => {}
            Err(e) => {
                tracing::debug!(event = "pdf_probe_unparsed", error = %e);
            }
        }
    }
    Ok(security)
}

/// `input` could not be opened or read (`verb`), with its name for the UI.
fn cannot_read(input: &Path, verb: &str, e: std::io::Error) -> EngineError {
    let name = input
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| input.display().to_string());
    EngineError::ConversionFailed(
        Failure::new(
            "cannot_read_input",
            format!("cannot {verb} {}: {e}", input.display()),
        )
        .param("file", name),
    )
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), EngineError> {
    if cancel.load(Ordering::Relaxed) {
        Err(EngineError::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FR-055 / FR-056: the web-view converters lay pages out on the job's
    /// paper. An A4 job used to produce US Letter pages.
    #[test]
    fn fr_055_page_setup_follows_the_job_paper_and_orientation() {
        use crate::queue::job::{Orientation, PaperSize};
        let size = |p: &str, o| css_page_size(&PaperSize::parse(p).unwrap(), o);
        assert_eq!(
            size("a4", Orientation::Portrait).as_deref(),
            Some("210mm 297mm")
        );
        assert_eq!(
            size("A4", Orientation::Auto).as_deref(),
            Some("210mm 297mm")
        );
        assert_eq!(
            size("letter", Orientation::Landscape).as_deref(),
            Some("11in 8.5in")
        );
        assert_eq!(
            size("Legal", Orientation::Portrait).as_deref(),
            Some("8.5in 14in")
        );
        assert_eq!(
            size("4x6", Orientation::Portrait).as_deref(),
            Some("4in 6in")
        );
        assert_eq!(
            size("w288h432", Orientation::Portrait).as_deref(),
            Some("288pt 432pt")
        );
        assert_eq!(
            size("A4 210 x 297 mm", Orientation::Portrait).as_deref(),
            Some("210mm 297mm")
        );
        assert_eq!(size("auto", Orientation::Portrait), None, "printer default");
        assert_eq!(size("Envelope #10 odd", Orientation::Portrait), None);

        let settings = crate::queue::job::JobSettings::default();
        assert_eq!(
            PageSetup::for_settings(&settings).css_size.as_deref(),
            Some("210mm 297mm"),
            "the default job is A4"
        );
    }
    use ::image::{ImageFormat, Rgb, RgbImage};
    use std::path::Path;
    use std::sync::atomic::AtomicBool;

    /// Encode a small deterministic gradient to `dir/name` in `fmt`, returning
    /// its path. Fixtures are generated in-test; nothing binary is committed.
    fn write_image(dir: &Path, name: &str, fmt: ImageFormat) -> PathBuf {
        let mut img = RgbImage::new(8, 8);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = Rgb([(x * 32) as u8, (y * 32) as u8, 128]);
        }
        let path = dir.join(name);
        img.save_with_format(&path, fmt).expect("encode fixture");
        path
    }

    fn assert_one_page_pdf(path: &Path) {
        let doc = lopdf::Document::load(path).expect("cache PDF must parse");
        assert_eq!(doc.get_pages().len(), 1, "one page per image");
    }

    #[test]
    fn converts_every_supported_image_format() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("cache");
        let cancel = AtomicBool::new(false);
        let progress = |_p: f32| {};

        for (name, fmt) in [
            ("a.png", ImageFormat::Png),
            ("a.jpg", ImageFormat::Jpeg),
            ("a.bmp", ImageFormat::Bmp),
            ("a.tif", ImageFormat::Tiff),
            ("a.webp", ImageFormat::WebP),
        ] {
            let src = write_image(dir.path(), name, fmt);
            let out = convert_to_pdf(
                &src,
                &cache,
                "job-1",
                &cancel,
                &progress,
                None,
                &PageSetup::default(),
                None,
            )
            .unwrap_or_else(|e| panic!("convert {name}: {e}"));
            let pdf = out.expect("image conversion yields a cache PDF");
            assert!(pdf.exists(), "{name}: cache PDF should exist");
            assert_one_page_pdf(&pdf);
        }
    }

    #[test]
    fn pdf_passthrough_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("in.pdf");
        std::fs::write(&pdf, ONE_PAGE_PDF).unwrap();
        let cancel = AtomicBool::new(false);
        let progress = |_p: f32| {};
        let out = convert_to_pdf(
            &pdf,
            dir.path(),
            "job",
            &cancel,
            &progress,
            None,
            &PageSetup::default(),
            None,
        )
        .unwrap();
        assert!(out.is_none(), "a real PDF is printed in place");
    }

    /// A minimal, complete one-page PDF.
    const ONE_PAGE_PDF: &[u8] = b"%PDF-1.4\n\
        1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
        2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
        3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 595 842]>>endobj\n\
        trailer<</Root 1 0 R>>\n%%EOF\n";

    /// FR-050: a file that only starts like a PDF is refused before it is
    /// spooled. `%PDF-1.4 broken` used to be printed in place and reported
    /// done (exit 0) while nothing came out.
    #[test]
    fn fr_050_a_pdf_with_only_a_header_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let progress = |_p: f32| {};
        let run = |bytes: &[u8]| {
            let pdf = dir.path().join("broken.pdf");
            std::fs::write(&pdf, bytes).unwrap();
            convert_to_pdf(
                &pdf,
                dir.path(),
                "job",
                &cancel,
                &progress,
                None,
                &PageSetup::default(),
                None,
            )
        };

        let err = run(b"%PDF-1.4 broken").unwrap_err();
        assert!(
            matches!(&err, EngineError::ConversionFailed(m)
                if m.contains("truncated") && m.reason == Some("pdf_truncated")),
            "{err:?}"
        );

        // Well formed, but not a single page to print.
        use lopdf::dictionary;
        let mut empty = lopdf::Document::with_version("1.5");
        let pages = empty.new_object_id();
        empty.set_object(
            pages,
            dictionary! { "Type" => "Pages", "Kids" => Vec::<lopdf::Object>::new(), "Count" => 0 },
        );
        let catalog = empty.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => lopdf::Object::Reference(pages),
        });
        empty.trailer.set("Root", lopdf::Object::Reference(catalog));
        let mut bytes = Vec::new();
        empty.save_to(&mut bytes).unwrap();
        let err = run(&bytes).unwrap_err();
        assert!(
            matches!(&err, EngineError::ConversionFailed(m)
                if m.contains("no pages") && m.reason == Some("pdf_no_pages")),
            "{err:?}"
        );

        assert!(
            run(ONE_PAGE_PDF).unwrap().is_none(),
            "a real PDF still passes"
        );
    }

    /// A one-page PDF whose page points (`/Deep 4 0 R`) at object 4, `depth`
    /// levels of `open`…`close` nesting. With `encrypt`, the trailer also names
    /// object 4 as its `/Encrypt` dictionary, so the encryption inspection
    /// (FR-059) parses the file too. Offsets in the xref table are exact.
    fn nested_pdf(open: &str, close: &str, depth: usize, encrypt: bool) -> Vec<u8> {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Deep 4 0 R >>".to_owned(),
            format!("{}{}", open.repeat(depth), close.repeat(depth)),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).bytes());
        }
        let xref = pdf.len();
        pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for offset in offsets {
            pdf.extend(format!("{offset:010} 00000 n \n").bytes());
        }
        let encrypt = if encrypt { " /Encrypt 4 0 R" } else { "" };
        pdf.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R{encrypt} >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .bytes(),
        );
        pdf
    }

    /// RUSTSEC-2026-0187: lopdf before 0.42 parsed nested arrays and
    /// dictionaries recursively without a limit, so a user's PDF with deeply
    /// nested objects overflowed the stack and aborted the whole app (a stack
    /// overflow cannot be caught). Every place that parses a user PDF — the
    /// passthrough checks, the encryption inspection and the page-count probe —
    /// must now come back with an answer instead.
    ///
    /// Run on an 8 MiB thread: lopdf now stops at 100 levels, which a release
    /// build parses in well under 256 KiB of stack, but unoptimised parser
    /// frames are far bigger and a debug build needs a few MiB for it. lopdf
    /// is built without `rayon`, so it parses on this thread (and in the app on
    /// the caller's), not on a pool thread with a stack of its own. The
    /// unbounded 0.39 parser overflowed even 16 MiB in release on this input.
    #[test]
    fn rustsec_2026_0187_deeply_nested_pdf_objects_do_not_overflow_the_stack() {
        std::thread::Builder::new()
            .stack_size(8 << 20)
            .spawn(|| {
                const DEPTH: usize = 100_000;
                let dir = tempfile::tempdir().unwrap();
                let cancel = AtomicBool::new(false);
                let progress = |_p: f32| {};
                let cases = [
                    ("arrays", "[", "]", false),
                    ("dictionaries", "<< /K ", " >>", false),
                    ("encrypt-arrays", "[", "]", true),
                    ("encrypt-dictionaries", "<< /K ", " >>", true),
                ];
                for (name, open, close, encrypt) in cases {
                    let bytes = nested_pdf(open, close, DEPTH, encrypt);
                    let pdf = dir.path().join(format!("{name}.pdf"));
                    std::fs::write(&pdf, &bytes).unwrap();

                    let result = convert_to_pdf(
                        &pdf,
                        dir.path(),
                        name,
                        &cancel,
                        &progress,
                        None,
                        &PageSetup::default(),
                        None,
                    );
                    if encrypt {
                        // A trailer naming an `/Encrypt` lopdf cannot read is
                        // refused (as protected or as damaged), never printed.
                        assert!(
                            matches!(
                                result,
                                Err(EngineError::PdfProtected(_) | EngineError::ConversionFailed(_))
                            ),
                            "{name}: {result:?}"
                        );
                    } else {
                        // The nested object is not one the page needs: the
                        // file is printed in place like any other PDF.
                        assert!(matches!(result, Ok(None)), "{name}: {result:?}");
                    }

                    // Whatever the count is, it is an answer, not a crash.
                    let _ = crate::queue::probe_pages(&pdf, "pdf", bytes.len() as u64);
                    let _ = pdf_security::inspect(&pdf, bytes.len() as u64);
                }
            })
            .unwrap()
            .join()
            .expect("parsing a deeply nested PDF must not panic");
    }

    #[test]
    fn junk_bytes_named_pdf_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("in.pdf");
        std::fs::write(&pdf, b"this is definitely not a pdf").unwrap();
        let cancel = AtomicBool::new(false);
        let progress = |_p: f32| {};
        let err = convert_to_pdf(
            &pdf,
            dir.path(),
            "job",
            &cancel,
            &progress,
            None,
            &PageSetup::default(),
            None,
        )
        .unwrap_err();
        assert!(matches!(err, EngineError::ConversionFailed(_)));
    }

    #[test]
    fn corrupt_image_is_conversion_failed() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("broken.png");
        // Valid PNG signature, garbage payload — decoding must fail cleanly.
        std::fs::write(&png, b"\x89PNG\r\n\x1a\nnot actually a real png stream").unwrap();
        let cancel = AtomicBool::new(false);
        let progress = |_p: f32| {};
        let err = convert_to_pdf(
            &png,
            &dir.path().join("cache"),
            "job",
            &cancel,
            &progress,
            None,
            &PageSetup::default(),
            None,
        )
        .unwrap_err();
        assert!(matches!(err, EngineError::ConversionFailed(_)));
    }

    #[test]
    fn unsupported_extension_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("setup.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        let cancel = AtomicBool::new(false);
        let progress = |_p: f32| {};
        let err = convert_to_pdf(
            &exe,
            dir.path(),
            "job",
            &cancel,
            &progress,
            None,
            &PageSetup::default(),
            None,
        )
        .unwrap_err();
        assert!(
            matches!(&err, EngineError::UnsupportedFormat(f)
                if f.reason == Some("unsupported_type") && f.params["ext"] == "exe"),
            "{err:?}"
        );
    }

    /// Decision D9: GIF stays unsupported, and says so with its own reason
    /// (the UI's "convert it to PNG or JPEG first").
    #[test]
    fn fr_052_gif_is_refused_with_its_own_reason() {
        assert!(!crate::engine::is_supported("gif"));
        assert_eq!(route("gif"), None);
        let dir = tempfile::tempdir().unwrap();
        let gif = dir.path().join("anim.gif");
        std::fs::write(&gif, b"GIF89a").unwrap();
        let cancel = AtomicBool::new(false);
        let progress = |_p: f32| {};
        let err = convert_to_pdf(
            &gif,
            dir.path(),
            "job",
            &cancel,
            &progress,
            None,
            &PageSetup::default(),
            None,
        )
        .unwrap_err();
        assert!(
            matches!(&err, EngineError::UnsupportedFormat(f) if f.reason == Some("gif_not_supported")),
            "{err:?}"
        );
    }

    #[test]
    fn fr_052_to_057_every_supported_extension_has_a_route() {
        for ext in crate::engine::SUPPORTED_EXTS {
            assert!(route(ext).is_some(), "{ext} is advertised but not routed");
        }
        // ...and every routed extension is advertised.
        let routed = IMAGE_EXTS
            .iter()
            .chain(HEIF_EXTS)
            .chain(OFFICE_EXTS)
            .chain(HTML_EXTS)
            .chain(MARKDOWN_EXTS)
            .chain(SVG_EXTS)
            .chain(TEXT_EXTS)
            .chain(&["pdf"]);
        for ext in routed {
            assert!(
                crate::engine::is_supported(ext),
                "{ext} is routed but not advertised"
            );
        }
        assert_eq!(route("DOCX"), Some(Route::Office));
        assert_eq!(route(".htm"), Some(Route::Html));
        assert_eq!(route("markdown"), Some(Route::Markdown));
        assert_eq!(route("heif"), Some(Route::Heif));
        assert_eq!(route("SVGZ"), Some(Route::Svg));
        assert_eq!(route("txt"), Some(Route::Text));
        assert_eq!(route("rtf"), Some(Route::Office));
        assert_eq!(route("exe"), None);
    }

    #[test]
    fn only_pictures_are_known_to_be_one_page() {
        assert!(is_single_page("png"));
        assert!(is_single_page("HEIC"));
        assert!(is_single_page("svg"));
        assert!(is_single_page("svgz"));
        assert!(!is_single_page("txt"));
        assert!(!is_single_page("pdf"));
        assert!(!is_single_page("docx"));
        assert!(!is_single_page("md"));
    }

    #[test]
    fn file_urls_are_percent_encoded_for_both_path_flavours() {
        assert_eq!(
            file_url_from("/Users/ana/Mis documentos/año #1.html", false),
            "file:///Users/ana/Mis%20documentos/a%C3%B1o%20%231.html"
        );
        assert_eq!(
            file_url_from(r"C:\Users\ana\My Docs\a.html", true),
            "file:///C:/Users/ana/My%20Docs/a.html"
        );
        assert_eq!(file_url_from(r"\\?\C:\x\y.md", true), "file:///C:/x/y.md");
        assert_eq!(
            file_url_from(r"\\server\share\z.htm", true),
            "file://server/share/z.htm"
        );
    }

    #[test]
    fn take_output_rejects_a_non_pdf_and_quotes_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let produced = dir.path().join("p.pdf");
        std::fs::write(&produced, b"garbage").unwrap();
        let log = dir.path().join("log");
        std::fs::write(&log, b"Error: source file could not be loaded").unwrap();
        let err =
            take_output(&produced, &dir.path().join("o.pdf"), "LibreOffice", &log).unwrap_err();
        assert!(
            matches!(&err, EngineError::ConversionFailed(m) if m.contains("could not be loaded")),
            "{err:?}"
        );
    }

    #[test]
    fn work_dirs_are_removed_when_dropped() {
        let work = WorkDir::new("0192-abc/../evil").unwrap();
        let path = work.path().to_path_buf();
        assert!(path.is_dir());
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("pluryprint-convert-0192-abcevil-"),
            "{path:?}"
        );
        drop(work);
        assert!(!path.exists());
    }

    #[test]
    fn missing_extension_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("Makefile");
        std::fs::write(&f, b"all:").unwrap();
        let cancel = AtomicBool::new(false);
        let progress = |_p: f32| {};
        let err = convert_to_pdf(
            &f,
            dir.path(),
            "job",
            &cancel,
            &progress,
            None,
            &PageSetup::default(),
            None,
        )
        .unwrap_err();
        assert!(matches!(err, EngineError::UnsupportedFormat(_)));
    }

    #[test]
    fn cancel_flag_aborts_before_work() {
        let dir = tempfile::tempdir().unwrap();
        let png = write_image(dir.path(), "x.png", ImageFormat::Png);
        let cancel = AtomicBool::new(true);
        let progress = |_p: f32| {};
        let err = convert_to_pdf(
            &png,
            dir.path(),
            "job",
            &cancel,
            &progress,
            None,
            &PageSetup::default(),
            None,
        )
        .unwrap_err();
        assert!(matches!(err, EngineError::Cancelled));
    }
}
