//! HTML → PDF (FR-055), and the page renderer Markdown ([`super::markdown`])
//! and plain text ([`super::text`]) share.
//!
//! ## The system web view (default)
//!
//! The page is printed by the operating system's own web view — WebView2 on
//! Windows, WebKit on macOS — in a hidden worker process of this same
//! executable (`pluryprint --worker render-pdf …`, [`crate::worker::render`],
//! [`crate::render`]). Nothing has to be installed: Pluryprint needs WebView2
//! to run at all, and WebKit is part of macOS. Edge, Chrome and Pandoc are no
//! longer dependencies.
//!
//! - The engine never hands the worker the user's file. It writes a **staged
//!   copy** to the job's work directory ([`stage_page`]) carrying, at the start
//!   of its head, a Content-Security-Policy that forbids scripts, plugins,
//!   frames and connections ([`page_csp`]), a `<base href>` that points
//!   relative references at the document's folder ([`crate::render::base_href`]),
//!   and the job's paper as CSS `@page { size }`. The paper is also set
//!   natively on the print settings.
//! - The worker loads it with scripts off and **no network and no file
//!   share**: see [`crate::render`] (a security finding: a page could make
//!   Windows open `\\host\share` over SMB/WebDAV, handing the user's NTLM
//!   credentials to that host, through `file://host/…` or a protocol-relative
//!   `//host/…` reference).
//! - The worker is bounded by the conversion timeout, the job's cancel flag
//!   and a memory cap, like the SVG worker ([`super::process::run_capped`]
//!   over the worker's whole process tree, [`RENDER_WORKER_MEMORY_CAP`];
//!   ADR-0014): a page that hangs, crashes the web view or needs too much
//!   memory kills the worker, never the app. Out of memory is the terminal
//!   `page_too_large`. On Windows the cap holds the WebView2 processes too;
//!   on macOS it covers only the worker's process group, not WebKit's
//!   web-content and networking processes (XPC services launchd starts), so
//!   memory a page uses there is not capped.
//!
//! - Text pre-scanning is **not** the security boundary. An HTML parser that
//!   is not the web view's own will always read some page differently from
//!   it, so the guarantees above are enforced where the web view fetches:
//!   its request filter and navigation hooks ([`crate::render`]). The staged
//!   page's policy and base ([`inject_head`]) are defence in depth behind
//!   them.
//!
//! The headless Edge/Chrome fallback (`[engine] html_renderer = "browser"`,
//! D15) is gone: it loaded the page from `file:` with nothing but a text
//! pre-pass between a share reference and SMB, and that pre-pass kept losing
//! to parser differentials (security notes 5-7). A stored `"browser"` is
//! ignored, and the app says so once ([`crate::settings::EngineCfg`]).
//!
//! ## The staged page
//!
//! The staged page is always UTF-8 with a byte-order mark
//! ([`super::html_scan::decode_page`]), and the policy must land as its very
//! first element: a page that leaves no safe place for the policy (a doctype
//! that never closes) is refused with `page_not_checkable` ([`inject_head`])
//! rather than rendered without it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::process::{run_capped, CapScope, CappedError};
use super::{PageSetup, WorkDir};
use crate::engine::{EngineError, Failure};

/// Extensions rendered as HTML directly.
pub const HTML_EXTS: &[&str] = &["html", "htm"];

/// Environment variable naming the executable that runs the render worker.
/// Unset in the shipped app (the worker is the app itself); tests point it at
/// the freshly built `pluryprint` binary.
pub const WORKER_EXE_ENV: &str = "PLURYPRINT_WORKER_EXE";

/// Label of the render worker in messages.
const RENDERER_LABEL: &str = "The page renderer";

/// Most memory the render worker and the web view processes it starts may
/// use together (ADR-0014, round 7): 2 GiB. A web view printing an ordinary
/// page uses a few hundred MB; a page that needs more is stopped and the job
/// fails with the terminal `page_too_large`.
pub const RENDER_WORKER_MEMORY_CAP: u64 = 2 * 1024 * 1024 * 1024;

/// `page_too_large`, naming the cap in MB as its `limit` param.
fn page_too_large(cap: u64) -> EngineError {
    let limit = (cap / (1024 * 1024)).to_string();
    EngineError::ConversionFailed(
        Failure::new(
            "page_too_large",
            format!("the page needed more memory than the renderer may use ({limit} MB)"),
        )
        .param("limit", limit),
    )
}

/// Convert the HTML page at `input` into a PDF at `output`, laid out on the
/// job's paper. The route's entry point.
pub fn convert(
    input: &Path,
    output: &Path,
    work: &WorkDir,
    cancel: &AtomicBool,
    timeout: Option<Duration>,
    setup: &PageSetup,
) -> Result<(), EngineError> {
    let html = std::fs::read(input).map_err(|e| {
        EngineError::ConversionFailed(
            Failure::new(
                "cannot_read_input",
                format!("cannot read {}: {e}", input.display()),
            )
            .param("file", input.display().to_string()),
        )
    })?;
    let base_dir = input.parent().unwrap_or_else(|| Path::new("."));
    render_document(&html, base_dir, output, work, cancel, timeout, setup)
}

/// Print the HTML document `html`, whose relative references resolve against
/// `base_dir`, into `output` with the system web view. The shared back half
/// of the HTML, Markdown, text and RTF routes.
pub fn render_document(
    html: &[u8],
    base_dir: &Path,
    output: &Path,
    work: &WorkDir,
    cancel: &AtomicBool,
    timeout: Option<Duration>,
    setup: &PageSetup,
) -> Result<(), EngineError> {
    // One decoding for the staging and for the renderer (the staged page is
    // UTF-8 with a byte-order mark, which the renderer has to obey).
    let html = super::html_scan::decode_page(html);
    render_system_with(
        &worker_exe()?,
        &html,
        base_dir,
        output,
        work,
        cancel,
        timeout,
        setup,
    )
}

/// The `page_not_checkable` refusal: no safe place for the page's policy, so
/// it is not printed (fail closed).
fn not_checkable(detail: impl Into<String>) -> EngineError {
    tracing::warn!(event = "html_page_not_checkable");
    EngineError::ConversionFailed(Failure::new("page_not_checkable", detail))
}

/// The executable that runs the render worker: [`WORKER_EXE_ENV`], else this
/// program, except that the console CLI (`pluryprint-cli`) hands it to the
/// app next to it, which owns the worker branch.
pub fn worker_exe() -> Result<PathBuf, EngineError> {
    if let Some(exe) = std::env::var_os(WORKER_EXE_ENV).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(exe));
    }
    let me = std::env::current_exe().map_err(|e| {
        EngineError::ConversionFailed(Failure::new(
            "render_failed",
            format!("cannot locate Pluryprint to start the page renderer: {e}"),
        ))
    })?;
    worker_exe_for(&me)
}

/// [`worker_exe`] for a given running executable. From the console CLI the
/// worker is the app next to it, which must be there: without it the job
/// fails with `render_worker_missing` (the app has to be reinstalled), not
/// with a dependency the remediation screen cannot name or install.
pub fn worker_exe_for(me: &Path) -> Result<PathBuf, EngineError> {
    let stem = me
        .file_stem()
        .map(|s| s.to_string_lossy().to_ascii_lowercase());
    if stem.as_deref() != Some("pluryprint-cli") {
        return Ok(me.to_path_buf());
    }
    let gui = if cfg!(windows) {
        "pluryprint.exe"
    } else {
        "pluryprint"
    };
    let sibling = me.with_file_name(gui);
    if sibling.is_file() {
        Ok(sibling)
    } else {
        Err(EngineError::ConversionFailed(Failure::new(
            "render_worker_missing",
            format!(
                "the page renderer is the Pluryprint app, which is not next to the CLI at {}",
                sibling.display()
            ),
        )))
    }
}

/// Paper width × height in inches for a CSS `size` value such as
/// `210mm 297mm`, `8.5in 11in` or `288pt 432pt`. `None` for anything else.
/// Pure.
pub fn paper_inches(css_size: &str) -> Option<(f64, f64)> {
    const UNITS: [(&str, f64); 4] = [
        ("mm", 1.0 / 25.4),
        ("cm", 1.0 / 2.54),
        ("in", 1.0),
        ("pt", 1.0 / 72.0),
    ];
    let mut parts = css_size.split_whitespace().map(|v| {
        let (number, factor) = UNITS
            .iter()
            .find_map(|(unit, factor)| v.strip_suffix(unit).map(|n| (n, *factor)))?;
        number
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite() && *n > 0.0)
            .map(|n| n * factor)
    });
    let w = parts.next()??;
    let h = parts.next()??;
    parts.next().is_none().then_some((w, h))
}

/// Stage `html` and print it with the render worker run by `exe` (module
/// docs), under [`RENDER_WORKER_MEMORY_CAP`]. [`render_document`] passes
/// [`worker_exe`]; tests pass a fake.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_system_with(
    exe: &Path,
    html: &[u8],
    base_dir: &Path,
    output: &Path,
    work: &WorkDir,
    cancel: &AtomicBool,
    timeout: Option<Duration>,
    setup: &PageSetup,
) -> Result<(), EngineError> {
    render_system_capped(
        exe,
        html,
        base_dir,
        output,
        work,
        cancel,
        timeout,
        setup,
        RENDER_WORKER_MEMORY_CAP,
    )
}

/// [`render_system_with`] under a memory cap of `memory_cap` bytes for the
/// worker's whole process tree.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_system_capped(
    exe: &Path,
    html: &[u8],
    base_dir: &Path,
    output: &Path,
    work: &WorkDir,
    cancel: &AtomicBool,
    timeout: Option<Duration>,
    setup: &PageSetup,
    memory_cap: u64,
) -> Result<(), EngineError> {
    let page = stage_page(html, &crate::render::base_href(base_dir), setup, work)?;
    let produced = work.path().join("out.pdf");
    let request = crate::render::RenderRequest {
        page,
        base_dir: base_dir.to_path_buf(),
        output: produced.clone(),
        data_dir: work.path().join("webview"),
        paper_in: setup.css_size.as_deref().and_then(paper_inches),
        // The worker's own watchdog: the same limit as the engine's, so a
        // worker left alone (a dead parent) still ends; none when the
        // setting is 0.
        timeout_secs: timeout.map(|t| t.as_secs().max(1)),
    };
    let mut cmd = Command::new(exe);
    cmd.arg(crate::worker::WORKER_FLAG)
        .arg(crate::worker::WorkerKind::RenderPdf.as_arg())
        .args(request.to_args())
        .current_dir(worker_cwd(
            work.path(),
            std::env::var_os("APPDIR").as_deref(),
            cfg!(target_os = "linux"),
        ));
    let log = work.path().join("render.log");
    match run_capped(
        cmd,
        RENDERER_LABEL,
        cancel,
        timeout,
        &log,
        memory_cap,
        CapScope::Tree,
    ) {
        Ok(()) => {}
        Err(CappedError::OverMemory) => {
            let _ = std::fs::remove_file(&produced);
            return Err(page_too_large(memory_cap));
        }
        Err(CappedError::Engine(EngineError::ConversionFailed(failure))) => {
            // The worker gave up on its own watchdog: a timeout, terminal
            // like the engine's own, never a render failure to retry.
            let full = std::fs::read_to_string(&log).unwrap_or_default();
            if crate::render::log_reports(&full, crate::render::TIMEOUT_MARKER) {
                return Err(super::process::timed_out_error(RENDERER_LABEL, timeout));
            }
            // Linux without a graphical session: no retry can start the web
            // view, so it is terminal and says why instead of "render failed".
            if crate::render::log_reports(&full, crate::render::NO_DISPLAY_MARKER) {
                return Err(EngineError::ConversionFailed(Failure::new(
                    "render_no_display",
                    super::process::log_tail(&log, 400),
                )));
            }
            let detail = super::process::log_tail(&log, 400);
            tracing::warn!(event = "render_worker_failed", detail = %detail, error = %failure.detail);
            return Err(EngineError::ConversionFailed(Failure::new(
                "render_failed",
                if detail.is_empty() {
                    failure.detail
                } else {
                    detail
                },
            )));
        }
        Err(CappedError::Engine(other)) => return Err(other),
    }
    super::take_output(&produced, output, RENDERER_LABEL, &log)
}

/// The folder the render worker starts in: the job's private work folder,
/// except inside a Linux AppImage. There linuxdeploy rewrites the paths
/// WebKitGTK spawns its helper processes from (`WebKitNetworkProcess`,
/// `WebKitWebProcess`) to `././/lib/<triplet>/webkit2gtk-4.1/…`, relative to
/// the current folder, and the AppImage's AppRun starts the app in
/// `$APPDIR/usr` so they resolve. A worker started anywhere else aborts with
/// "Unable to spawn a new child process" before it loads the page, so it
/// starts in `$APPDIR/usr` too. Every path the worker uses is absolute, so the
/// folder changes nothing else. Pure: `appdir` is the `APPDIR` the AppImage
/// runtime sets, `linux` whether this is Linux.
pub(crate) fn worker_cwd(work: &Path, appdir: Option<&std::ffi::OsStr>, linux: bool) -> PathBuf {
    if linux {
        if let Some(usr) = appdir
            .filter(|d| !d.is_empty())
            .map(|d| Path::new(d).join("usr"))
            .filter(|usr| usr.is_dir())
        {
            return usr;
        }
    }
    work.to_path_buf()
}

/// The Content-Security-Policy every staged page carries: no script, plugin,
/// frame, worker, form or connection; styles, images, fonts and media only
/// from the renderer's private origin (the page's own folder, served by the
/// worker: `https://` on Windows, `pluryprint-doc://` on macOS) or `data:`;
/// never `file:` (round 7: no platform loads the page from `file:` any
/// more). Defence in depth: the renderer already turns scripts off and
/// refuses every other request natively. It is the first thing in the staged
/// page, and also a response header.
pub fn page_csp() -> String {
    let host = crate::render::DOC_HOST;
    let hosts = format!("https://{host} {}://{host}", crate::render::MAC_DOC_SCHEME);
    format!(
        "default-src 'self' data: 'unsafe-inline' {hosts}; script-src 'none'; \
         object-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; \
         connect-src 'none'; form-action 'none'; base-uri 'self' {hosts}"
    )
}

/// Write the staged copy of `html` the renderer prints (module docs) to
/// `<work>/stage/page.html`, alone in its folder, and return its path.
fn stage_page(
    html: &[u8],
    base_href: &str,
    setup: &PageSetup,
    work: &WorkDir,
) -> Result<PathBuf, EngineError> {
    let staged_html = inject_head(html, base_href, setup.css_size.as_deref())?;
    let dir = work.path().join("stage");
    let page = dir.join("page.html");
    std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(&page, staged_html))
        .map_err(|e| {
            EngineError::ConversionFailed(Failure::new(
                "workdir_failed",
                format!("cannot prepare the conversion folder: {e}"),
            ))
        })?;
    Ok(page)
}

/// The base the staged page resolves its references against: the page's own
/// `<base href>` (its first real one, [`super::html_scan::own_base_href`])
/// resolved against `base_href`, the document folder's URL, when it stays on
/// that origin and is no share; else `base_href` itself. A relative
/// `<base href="assets/">` therefore means the document's `assets` folder on
/// every renderer, never the private stage folder the page is served from.
/// Pure.
pub fn effective_base(html: &[u8], base_href: &str) -> String {
    let Some(own) = super::html_scan::own_base_href(html) else {
        return base_href.to_string();
    };
    let Ok(ours) = url::Url::parse(base_href) else {
        return base_href.to_string();
    };
    let Ok(resolved) = ours.join(own.trim()) else {
        return base_href.to_string();
    };
    let same_origin = resolved.scheme() == ours.scheme()
        && resolved.host_str() == ours.host_str()
        && resolved.port() == ours.port();
    if same_origin && !super::local_ref::is_share_ref(resolved.as_str()) {
        resolved.to_string()
    } else {
        base_href.to_string()
    }
}

/// `html` with the renderer's head inserted first: the [`page_csp`] meta, the
/// `<base href>` of [`effective_base`], and the job's paper as
/// `@page { size }` when `css_size` names one. Pure.
///
/// The insertion point ([`super::html_scan::insertion_point`]) is just after
/// a leading doctype, else the very start: never before the doctype (quirks
/// mode), never inside a comment (a page that begins `<!-- <head> -->` used to
/// get its policy spliced into the comment), and before any script or
/// `<base>` of the page, so the policy covers the whole page and the injected
/// base is the one that counts (the first `<base href>` wins). The page is
/// decoded by its real encoding first and re-encoded as UTF-8 with a BOM
/// ([`super::html_scan::decode_page`]), so ASCII can be spliced into it and
/// the renderer reads it the way it was checked.
///
/// **Fails closed** with `page_not_checkable`: when there is no safe
/// insertion point (a doctype that never closes), or when the result does
/// not start, for the HTML tokenizer, with the policy as its very first
/// element. Any declared encoding is decoded leniently and never refuses
/// the page (a round-6 finding).
pub fn inject_head(
    html: &[u8],
    base_href: &str,
    css_size: Option<&str>,
) -> Result<Vec<u8>, EngineError> {
    let decoded = super::html_scan::decode_page(html);
    let html = &decoded[..];
    let escape = |s: &str| {
        s.replace('&', "&amp;")
            .replace('"', "&quot;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let mut snippet = format!(
        "<meta http-equiv=\"Content-Security-Policy\" content=\"{}\">",
        escape(&page_csp())
    );
    snippet.push_str(&format!(
        "<base href=\"{}\">",
        escape(&effective_base(html, base_href))
    ));
    if let Some(size) = css_size {
        let css: String = size
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ' '))
            .collect();
        snippet.push_str(&format!("<style>@page {{ size: {css}; }}</style>"));
    }

    let at = super::html_scan::insertion_point(html)
        .ok_or_else(|| not_checkable("the page's doctype is never closed"))?;
    let mut out = Vec::with_capacity(html.len() + snippet.len());
    out.extend_from_slice(&html[..at]);
    out.extend_from_slice(snippet.as_bytes());
    out.extend_from_slice(&html[at..]);
    // The policy must be the first element the tokenizer meets.
    let first = super::html_scan::tags(&out[..(at + snippet.len()).min(out.len())])
        .into_iter()
        .next();
    let csp_first = first.is_some_and(|t| {
        !t.is_end
            && t.name == "meta"
            && t.span.start == at
            && t.attr("http-equiv")
                .is_some_and(|v| v.eq_ignore_ascii_case("content-security-policy"))
            && t.attr("content") == Some(page_csp().as_str())
    });
    if !csp_first {
        return Err(not_checkable(
            "the security policy could not be placed at the start of the page",
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`inject_head`]'s staged page without its UTF-8 byte-order mark.
    fn inject(html: &[u8], base: &str, size: Option<&str>) -> Vec<u8> {
        let out = inject_head(html, base, size).unwrap();
        assert!(out.starts_with(b"\xEF\xBB\xBF"), "staged with a BOM");
        out[3..].to_vec()
    }

    fn reason(err: &EngineError) -> Option<&'static str> {
        match err {
            EngineError::ConversionFailed(f) => f.reason,
            _ => None,
        }
    }

    /// Security note 6: the default renderer's injection fails closed when
    /// the policy cannot be placed first.
    #[test]
    fn nfr_security_injection_fails_closed() {
        let err =
            inject_head(b"<!DOCTYPE html, no end to the doctype", "file:///d/", None).unwrap_err();
        assert_eq!(reason(&err), Some("page_not_checkable"));
        // ... and the system route refuses such a page before the worker runs.
        let work = WorkDir::new("inject-closed").unwrap();
        let err = render_system_with(
            Path::new("/nonexistent/worker"),
            b"<!DOCTYPE html",
            Path::new("/tmp"),
            Path::new("/tmp/never.pdf"),
            &work,
            &AtomicBool::new(false),
            None,
            &PageSetup::default(),
        )
        .unwrap_err();
        assert_eq!(reason(&err), Some("page_not_checkable"));
    }

    /// Round-6 finding: a page with an unknown or empty declared charset
    /// was refused as `page_not_checkable` on the default route, which the
    /// web view would have printed. With the web view's network layer as
    /// the boundary, it is decoded leniently (UTF-8, else Windows-1252) and
    /// printed.
    #[test]
    fn fr_055_an_unknown_or_empty_charset_still_prints() {
        for page in [
            &b"<meta charset=\"\"><p>caf\xC3\xA9"[..],
            b"<meta charset=MacRoman><p>caf\xE9",
            b"<meta http-equiv=content-type content=\"text/html; charset=\"><p>x",
            b"<meta charset=iso-2022-kr><p>x",
        ] {
            let out = String::from_utf8(inject(page, "file:///d/", None)).unwrap();
            assert!(out.starts_with("<meta http-equiv=\"Content-Security-Policy\""));
        }
        let out = String::from_utf8(inject(
            b"<meta charset=MacRoman><p>caf\xE9",
            "file:///d/",
            None,
        ))
        .unwrap();
        assert!(out.ends_with("café"), "{out}");
    }

    #[test]
    fn fr_055_the_head_is_injected_right_after_the_doctype() {
        let html = b"<!DOCTYPE html><html lang=en><HEAD><title>x</title></head><body><header>h</header></body></html>";
        let out = String::from_utf8(inject(html, "file:///docs/", Some("210mm 297mm"))).unwrap();
        assert!(
            out.starts_with("<!DOCTYPE html><meta http-equiv=\"Content-Security-Policy\""),
            "{out}"
        );
        assert!(
            out.contains(
                "<base href=\"file:///docs/\"><style>@page { size: 210mm 297mm; }</style><html lang=en>"
            ),
            "{out}"
        );
        assert!(out.ends_with("<body><header>h</header></body></html>"));
    }

    #[test]
    fn fr_055_injection_keeps_the_doctype_first() {
        // No head or html tag: after the doctype, never before it.
        let out = String::from_utf8(inject(
            b"<!doctype html><p>hi",
            "file:///d/",
            Some("8.5in 11in"),
        ))
        .unwrap();
        assert!(out.starts_with("<!doctype html><meta "), "{out}");
        assert!(out.contains("size: 8.5in 11in"));

        // No paper: no @page rule at all.
        let out = String::from_utf8(inject(b"<p>x", "file:///d/", None)).unwrap();
        assert!(!out.contains("@page"), "{out}");
        assert!(out.ends_with("<p>x"));
    }

    /// Security finding: a `<head>` (or `<base>`) written inside a comment
    /// used to receive the policy, inside the comment, where it does
    /// nothing. The policy now comes first, before any comment or script of
    /// the page, and a commented-out `<base>` is not the page's.
    #[test]
    fn nfr_security_csp_is_never_spliced_into_a_comment() {
        let page = b"<!-- <head> <base href='//evil/'> --><script>x()</script><html><head><title>t</title>";
        let out = String::from_utf8(inject(page, "file:///d/", None)).unwrap();
        assert!(
            out.starts_with("<meta http-equiv=\"Content-Security-Policy\""),
            "{out}"
        );
        assert!(
            out.contains("<base href=\"file:///d/\"><!-- <head>"),
            "{out}"
        );

        let page = b"<!DOCTYPE html><!-- <head> --><head><title>t</title>";
        let out = String::from_utf8(inject(page, "file:///d/", None)).unwrap();
        assert!(
            out.starts_with("<!DOCTYPE html><meta http-equiv=\"Content-Security-Policy\""),
            "{out}"
        );
        let csp_at = out.find("Content-Security-Policy").unwrap();
        assert!(csp_at < out.find("<!--").unwrap(), "{out}");
    }

    /// The page's own relative `<base href>` means the document's folder on
    /// every renderer (it used to resolve against the private stage folder on
    /// Windows); an absolute, remote or share base is not followed.
    #[test]
    fn fr_055_a_relative_own_base_resolves_against_the_document_folder() {
        let base = |page: &[u8], ours: &str| effective_base(page, ours);
        let page = b"<html><head><base href=\"assets/\"></head>";
        assert_eq!(base(page, "file:///docs/"), "file:///docs/assets/");
        assert_eq!(
            base(page, "https://doc.pluryprint.example/"),
            "https://doc.pluryprint.example/assets/"
        );
        assert_eq!(
            base(b"<base href='./'>", "https://doc.pluryprint.example/"),
            "https://doc.pluryprint.example/"
        );
        for other in [
            "<base href='https://x/'>",
            "<base href='//host/share/'>",
            "<base href='file://host/share/'>",
            "<base href='\\\\host\\share\\'>",
            "<base href='/\\host/share/'>",
        ] {
            assert_eq!(
                base(other.as_bytes(), "file:///docs/"),
                "file:///docs/",
                "{other}"
            );
            assert_eq!(
                base(other.as_bytes(), "https://doc.pluryprint.example/"),
                "https://doc.pluryprint.example/",
                "{other}"
            );
        }
        // The injected base comes first, so it is the one the page uses.
        let out = String::from_utf8(inject(page, "https://doc.pluryprint.example/", None)).unwrap();
        let ours = out
            .find("<base href=\"https://doc.pluryprint.example/assets/\">")
            .unwrap();
        assert!(ours < out.find("<base href=\"assets/\">").unwrap(), "{out}");
    }

    /// The worker's own watchdog is a timeout (terminal), never a render
    /// failure the queue would retry; the worker gets the job's limit.
    #[cfg(unix)]
    #[test]
    fn nfr_render_worker_timeout_is_terminal_with_reason() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("fake-worker.sh");
        let args_log = dir.path().join("args.txt");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\necho \"$@\" > '{}'\necho '{}: the render did not finish within 7s' >&2\nexit 1\n",
                args_log.display(),
                crate::render::TIMEOUT_MARKER
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let work = WorkDir::new("timeout-test").unwrap();
        let cancel = AtomicBool::new(false);
        let err = render_system_with(
            &fake,
            b"<p>x",
            dir.path(),
            &dir.path().join("o.pdf"),
            &work,
            &cancel,
            Some(Duration::from_secs(7)),
            &PageSetup::default(),
        )
        .unwrap_err();
        assert!(
            matches!(&err, EngineError::ConversionTimedOut(f) if f.reason == Some("tool_timed_out")),
            "{err:?}"
        );
        let args = std::fs::read_to_string(&args_log).unwrap();
        assert!(args.contains("--timeout-secs 7"), "{args}");

        // No limit (the setting is 0): no watchdog argument at all.
        let _ = render_system_with(
            &fake,
            b"<p>x",
            dir.path(),
            &dir.path().join("o.pdf"),
            &work,
            &cancel,
            None,
            &PageSetup::default(),
        );
        let args = std::fs::read_to_string(&args_log).unwrap();
        assert!(!args.contains("--timeout-secs"), "{args}");
    }

    #[test]
    fn fr_055_injection_paper_rule() {
        let out = String::from_utf8(inject(
            b"<html><head></head></html>",
            "file:///d/",
            Some("4in 6in"),
        ))
        .unwrap();
        assert_eq!(out.matches("<base").count(), 1, "{out}");
        assert!(out.contains("size: 4in 6in"));

        // No paper: no @page rule at all.
        let out = String::from_utf8(inject(b"<p>x", "file:///d/", None)).unwrap();
        assert!(!out.contains("@page"), "{out}");
        assert!(out.ends_with("<p>x"));
    }

    #[test]
    fn fr_055_a_utf16_page_is_reencoded_before_the_injection() {
        let mut page = vec![0xFF, 0xFE];
        for unit in "<html><head></head><p>Ñandú</p>".encode_utf16() {
            page.extend_from_slice(&unit.to_le_bytes());
        }
        let out = inject_head(&page, "file:///d/", None).unwrap();
        assert!(out.starts_with(&[0xEF, 0xBB, 0xBF]));
        let text = String::from_utf8(out[3..].to_vec()).unwrap();
        assert!(text.starts_with("<meta http-equiv"), "{text}");
        assert!(text.contains("Ñandú"));
    }

    /// The CSP forbids everything active and every connection; the page's
    /// own policies can only narrow it further (CSPs intersect).
    #[test]
    fn fr_055_the_page_policy_forbids_script_frames_and_connections() {
        let csp = page_csp();
        for directive in [
            "script-src 'none'",
            "object-src 'none'",
            "frame-src 'none'",
            "connect-src 'none'",
            "worker-src 'none'",
            "form-action 'none'",
        ] {
            assert!(csp.contains(directive), "{directive}: {csp}");
        }
        assert!(!csp.contains("http:"), "{csp}");
        assert!(!csp.contains("file:"), "no file: source (round 7): {csp}");
        assert!(!csp.contains("https: "), "no blanket https: {csp}");
        assert!(!csp.contains('*'), "{csp}");
    }

    #[test]
    fn fr_055_page_size_from_pagesetup() {
        let a4 = paper_inches("210mm 297mm").unwrap();
        assert!((a4.0 - 8.2677).abs() < 1e-3 && (a4.1 - 11.6929).abs() < 1e-3);
        assert_eq!(paper_inches("11in 8.5in"), Some((11.0, 8.5)));
        assert_eq!(paper_inches("288pt 432pt"), Some((4.0, 6.0)));
        assert_eq!(paper_inches("auto"), None);
        assert_eq!(paper_inches("210mm"), None);
        assert_eq!(paper_inches("0mm 297mm"), None);
        assert_eq!(paper_inches("1in 2in 3in"), None);
        // Every size the job settings can produce is understood.
        for paper in ["A3", "A4", "A5", "Letter", "Legal", "4x6", "w288h432"] {
            for orientation in [
                crate::queue::job::Orientation::Portrait,
                crate::queue::job::Orientation::Landscape,
            ] {
                let css = super::super::css_page_size(
                    &crate::queue::job::PaperSize::parse(paper).unwrap(),
                    orientation,
                )
                .unwrap();
                assert!(paper_inches(&css).is_some(), "{paper}: {css}");
            }
        }
    }

    #[test]
    fn nfr_worker_exe_is_the_app_even_from_the_console_cli() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let (cli, gui) = if cfg!(windows) {
            ("pluryprint-cli.exe", "pluryprint.exe")
        } else {
            ("pluryprint-cli", "pluryprint")
        };
        std::fs::write(dir.join(gui), b"app").unwrap();
        assert_eq!(worker_exe_for(&dir.join(cli)).unwrap(), dir.join(gui));
        assert_eq!(worker_exe_for(&dir.join(gui)).unwrap(), dir.join(gui));
    }

    /// Inside a Linux AppImage the worker starts in `$APPDIR/usr`, where
    /// WebKitGTK's relative helper paths resolve; everywhere else (and when
    /// `APPDIR` is unset, empty or not an AppImage) in the job's work folder.
    #[test]
    fn fr_055_render_worker_starts_where_the_appimage_webkit_finds_its_helpers() {
        let work = tempfile::tempdir().unwrap();
        let appdir = tempfile::tempdir().unwrap();
        let usr = appdir.path().join("usr");
        std::fs::create_dir(&usr).unwrap();
        let ad = Some(appdir.path().as_os_str());
        assert_eq!(worker_cwd(work.path(), ad, true), usr);
        assert_eq!(worker_cwd(work.path(), ad, false), work.path());
        assert_eq!(worker_cwd(work.path(), None, true), work.path());
        assert_eq!(
            worker_cwd(work.path(), Some(std::ffi::OsStr::new("")), true),
            work.path()
        );
        let bare = tempfile::tempdir().unwrap();
        assert_eq!(
            worker_cwd(work.path(), Some(bare.path().as_os_str()), true),
            work.path()
        );
    }

    /// A worker with no graphical session to start its web view in (Linux)
    /// fails with its own terminal reason, not a retryable render failure.
    #[cfg(unix)]
    #[test]
    fn fr_055_render_worker_without_a_display_is_terminal_with_reason() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("fake-worker.sh");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\necho '{}: no X11 or Wayland display' >&2\nexit 1\n",
                crate::render::NO_DISPLAY_MARKER
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let work = WorkDir::new("no-display-test").unwrap();
        let cancel = AtomicBool::new(false);
        let err = render_system_with(
            &fake,
            b"<p>x",
            dir.path(),
            &dir.path().join("o.pdf"),
            &work,
            &cancel,
            None,
            &PageSetup::default(),
        )
        .unwrap_err();
        assert!(
            matches!(&err, EngineError::ConversionFailed(f) if f.reason == Some("render_no_display")),
            "{err:?}"
        );
        assert!(crate::util::reasons::is_terminal_conversion_reason(
            "render_no_display"
        ));
    }

    /// The CLI without the app next to it fails with a registered reason the
    /// queue can explain, not a reasonless "dependency missing" pause.
    #[test]
    fn nfr_render_worker_missing_is_a_registered_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let cli = if cfg!(windows) {
            "pluryprint-cli.exe"
        } else {
            "pluryprint-cli"
        };
        let err = worker_exe_for(&tmp.path().join(cli)).unwrap_err();
        assert!(
            matches!(&err, EngineError::ConversionFailed(f) if f.reason == Some("render_worker_missing")),
            "{err:?}"
        );
        assert!(crate::util::reasons::JOB_REASONS
            .iter()
            .any(|(code, reason, _)| *code == "conversion_failed"
                && *reason == "render_worker_missing"));
    }
}
