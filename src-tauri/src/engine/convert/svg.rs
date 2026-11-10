//! SVG / SVGZ → vector PDF (FR-052a).
//!
//! The drawing is parsed by `usvg` and written by `svg2pdf` (both from the
//! resvg/Typst projects, MIT OR Apache-2.0): shapes, gradients and text stay
//! vector; only filter effects are rasterised. The page is the drawing's own
//! size at 96 dpi (CSS px → 0.75 pt, the same scale as the raster image
//! route), clamped to [`MAX_PAGE_PT`]; fitting it to the paper is left to the
//! printer's scaling flags, exactly like an image.
//!
//! **Isolation and resource caps (decision D10, security note 8).** A hostile
//! drawing can make a parser panic (the release profile aborts on panic) or
//! ask for unbounded memory and time, so the conversion runs in a hidden
//! worker process (`pluryprint --worker svg <in> <out>`, see
//! [`crate::worker::svg`]), and **the worker is the boundary**: it runs under
//! a hard memory cap ([`WORKER_MEMORY_CAP`]: a Windows Job Object with
//! `JOB_OBJECT_LIMIT_PROCESS_MEMORY`, kill-on-close and one active process; on
//! Unix `RLIMIT_DATA` plus a resident-memory watchdog in the parent, because
//! macOS does not enforce `RLIMIT_DATA`/`RLIMIT_AS` on `mmap`), the
//! conversion timeout and the job's cancel flag
//! ([`super::process::run_capped`]). A worker stopped for memory ends the job
//! with the terminal `svg_too_large`; one stopped for time with the terminal
//! `tool_timed_out`. The PDF it writes is capped too ([`MAX_PDF_BYTES`]).
//! When no worker executable sits next to the current one (unit tests), the
//! conversion runs on a thread instead, without those caps.
//!
//! **References.** Nothing is fetched from the network and no script runs:
//! usvg has no network code, ignores `<script>`, `<foreignObject>` and
//! animation, and its XML parser (`roxmltree`) has no external entities, so
//! XXE cannot read a file. Every external reference usvg follows goes through
//! its `ImageHrefResolver` (`<image>` and `<feImage>`), and that is where the
//! decision is taken, on the exact string usvg parsed ([`resolver`]): a
//! `data:` URI, or a file inside the SVG's own folder by [`local_only`] (the
//! shared [`super::local_ref`] predicate on the href and again on the resolved
//! path, then `canonicalize` and a containment check). Anything else (a share,
//! which would leak NTLM credentials over SMB on Windows, a `file://host/` URL,
//! a URL, a `../` outside the folder) resolves to nothing and simply does not
//! draw. An embedded SVG goes through the same caps as the drawing itself, and
//! a raster's pixel count is checked before svg2pdf decodes it. Fonts come
//! only from the system font database, loaded once per process and only when
//! the parsed drawing has text or embeds an image; usvg never loads a font
//! from a URL or a path in the drawing.
//!
//! **Cheap caps, defence in depth.** The document is read with a bounded
//! reader, and an `.svgz` is inflated through one too, so neither a huge file
//! nor a gzip bomb is ever held past [`SVG_MAX_BYTES`]. The text is handed to
//! usvg's XML parser directly (never to `usvg::Tree::from_data`, which would
//! inflate a second, nested gzip layer without any cap) with a node limit
//! ([`MAX_NODES`]), after [`entity_expansion`] has checked that the
//! document's internal entities cannot blow it up past the same byte cap.
//! Then, on that same parsed document, [`expansion::check_expansion`] bounds
//! the work usvg would expand it to, and on the tree usvg built
//! [`check_raster_work`] bounds the pixels svg2pdf would rasterise for
//! filters. None of these is the security boundary; they make an oversized
//! drawing fail fast with the right reason.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::engine::{EngineError, Failure};

mod expansion;

/// Extensions this converter handles. `svgz` is gzip-compressed SVG.
pub const SVG_EXTS: &[&str] = &["svg", "svgz"];

/// Largest SVG document accepted, after inflating an `.svgz`.
pub const SVG_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Most memory the SVG worker process may use (security note 8): 1 GiB. A
/// worker that needs more is stopped and the job fails with `svg_too_large`.
pub const WORKER_MEMORY_CAP: u64 = 1024 * 1024 * 1024;

/// Largest PDF the conversion may produce.
pub const MAX_PDF_BYTES: u64 = 512 * 1024 * 1024;

/// Most bytes all of a drawing's `<image>` resources may add up to, read from
/// disk or decoded from `data:` URIs.
const MAX_RESOURCE_BYTES: u64 = 256 * 1024 * 1024;

/// Most pixels one raster `<image>` may have: svg2pdf decodes it whole (up to
/// four bytes a pixel, plus a copy for its alpha channel).
const MAX_IMAGE_PIXELS: u64 = 64 * 1024 * 1024;

/// Most pixels svg2pdf may rasterise for filter effects, all filters together
/// (each one is rendered into an RGBA pixmap of its region, then encoded).
const MAX_RASTER_PIXELS: u64 = 64 * 1024 * 1024;

/// svg2pdf's `raster_scale` (its default): the factor a filter region is
/// rasterised at.
const RASTER_SCALE: f32 = 1.5;

/// The drawing's CSS pixels per inch.
const SVG_DPI: f32 = 96.0;

/// Largest page side, in points (the image route's ceiling, 200 in).
const MAX_PAGE_PT: f32 = 14_400.0;

/// Most XML nodes (elements and text) a drawing may have; usvg itself refuses
/// more than a million elements, after `<use>` expansion.
const MAX_NODES: u32 = 1_000_000;

/// The caps the SVG worker process runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerCaps {
    /// Most memory the worker may use, in bytes.
    pub memory: u64,
}

impl Default for WorkerCaps {
    fn default() -> Self {
        Self {
            memory: WORKER_MEMORY_CAP,
        }
    }
}

/// Convert the SVG at `input` into a one-page PDF at `output`.
pub fn convert(
    input: &Path,
    output: &Path,
    cancel: &AtomicBool,
    timeout: Option<Duration>,
    progress: &(dyn Fn(f32) + Send + Sync),
) -> Result<(), EngineError> {
    progress(0.0);
    match worker_exe() {
        Some(exe) => convert_with_worker(&exe, input, output, cancel, timeout)?,
        None => {
            let (input, output) = (input.to_path_buf(), output.to_path_buf());
            super::heif::run_abandonable("svg", cancel, timeout, move || {
                convert_in_process(&input, &output)
            })?;
        }
    }
    progress(1.0);
    Ok(())
}

/// The executable that runs `--worker svg`: the app itself. From the
/// console CLI (`pluryprint-cli`) it is the app sitting next to it. `None`
/// when neither is this process (a test harness).
fn worker_exe() -> Option<PathBuf> {
    let me = std::env::current_exe().ok()?;
    let stem = me.file_stem()?.to_string_lossy().to_ascii_lowercase();
    if stem == "pluryprint" {
        return Some(me);
    }
    let sibling = me.with_file_name(if cfg!(windows) {
        "pluryprint.exe"
    } else {
        "pluryprint"
    });
    (stem == "pluryprint-cli" && sibling.is_file()).then_some(sibling)
}

/// Convert in the worker process `exe` (`<exe> --worker svg <in> <out>`)
/// under the default [`WorkerCaps`], killing it on cancel or timeout. See
/// [`convert_with_worker_capped`].
pub fn convert_with_worker(
    exe: &Path,
    input: &Path,
    output: &Path,
    cancel: &AtomicBool,
    timeout: Option<Duration>,
) -> Result<(), EngineError> {
    convert_with_worker_capped(exe, input, output, cancel, timeout, WorkerCaps::default())
}

/// Convert in the worker process `exe` under `caps`. A failure the worker
/// reports keeps its stable reason ([`report_failure`]); a worker stopped
/// for memory is `svg_too_large`, one that ran out of time `tool_timed_out`
/// (both terminal), and one that died otherwise (a panic aborts) is
/// `svg_convert_failed`. A PDF over [`MAX_PDF_BYTES`] is `svg_too_large`.
pub fn convert_with_worker_capped(
    exe: &Path,
    input: &Path,
    output: &Path,
    cancel: &AtomicBool,
    timeout: Option<Duration>,
    caps: WorkerCaps,
) -> Result<(), EngineError> {
    use super::process::{run_capped, CapScope, CappedError};
    let tag = output
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let work = super::WorkDir::new(&tag)?;
    let log = work.path().join("svg-worker.log");
    let mut cmd = std::process::Command::new(exe);
    cmd.arg(crate::worker::WORKER_FLAG)
        .arg(crate::worker::WorkerKind::Svg.as_arg())
        .arg(input)
        .arg(output)
        .current_dir(work.path());
    match run_capped(
        cmd,
        "SVG converter",
        cancel,
        timeout,
        &log,
        caps.memory,
        CapScope::Process,
    ) {
        Ok(()) => match std::fs::metadata(output) {
            Ok(meta) if meta.is_file() && meta.len() <= MAX_PDF_BYTES => Ok(()),
            Ok(meta) if meta.is_file() => {
                let _ = std::fs::remove_file(output);
                Err(pdf_too_large())
            }
            _ => Err(convert_failed("the SVG worker wrote no PDF")),
        },
        Err(CappedError::OverMemory) => {
            let _ = std::fs::remove_file(output);
            Err(over_memory(caps.memory))
        }
        Err(CappedError::Engine(EngineError::ConversionFailed(f))) => {
            let reported = std::fs::read_to_string(&log)
                .ok()
                .and_then(|text| parse_failure(&text));
            Err(EngineError::ConversionFailed(reported.unwrap_or_else(
                || Failure::new("svg_convert_failed", f.detail),
            )))
        }
        Err(CappedError::Engine(e)) => Err(e),
    }
}

/// The marker that starts the failure line a worker prints on stderr.
const FAILURE_MARKER: &str = "pluryprint-failure ";

/// The line the worker prints on stderr for `failure`, so the parent keeps
/// its reason and params ([`parse_failure`]).
pub fn report_failure(failure: &Failure) -> String {
    let json = serde_json::json!({
        "reason": failure.reason,
        "params": failure.params,
        "detail": failure.detail,
    });
    format!("{FAILURE_MARKER}{json}")
}

/// The failure a worker reported in `stderr`, if it reported one. Only
/// reasons of this route are accepted (they are `&'static str`). Pure.
fn parse_failure(stderr: &str) -> Option<Failure> {
    let line = stderr
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix(FAILURE_MARKER))?;
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let reason = value.get("reason")?.as_str()?;
    let reason = ["svg_invalid", "svg_too_large", "svg_convert_failed"]
        .into_iter()
        .chain(["cannot_read_input", "cannot_write_output"])
        .find(|r| *r == reason)?;
    let detail = value.get("detail")?.as_str().unwrap_or_default();
    let mut failure = Failure::new(reason, detail);
    if let Some(params) = value.get("params").and_then(|p| p.as_object()) {
        for (name, v) in params {
            if let Some(v) = v.as_str() {
                failure = failure.param(name, v);
            }
        }
    }
    Some(failure)
}

/// Convert in this process. The worker's whole job; also the fallback when
/// there is no worker executable.
pub fn convert_in_process(input: &Path, output: &Path) -> Result<(), EngineError> {
    let pdf = render(input)?;
    std::fs::write(output, pdf).map_err(|e| {
        EngineError::ConversionFailed(Failure::new(
            "cannot_write_output",
            format!("cannot write {}: {e}", output.display()),
        ))
    })
}

/// The PDF bytes of the drawing at `input`.
fn render(input: &Path) -> Result<Vec<u8>, EngineError> {
    let data = read_svg(input)?;
    let dir = input
        .parent()
        .and_then(|d| d.canonicalize().ok())
        .unwrap_or_default();
    let budget = Arc::new(Budget::default());
    let text = svg_text(&data)?;
    let doc = parse_document(text)?;
    budget.spend_expansion(&doc)?;

    let mut opt = usvg::Options {
        // Every file usvg reads goes through `resolver`, which resolves
        // relative hrefs against `dir` itself.
        resources_dir: None,
        ..usvg::Options::default()
    };
    if needs_fonts(&doc) {
        let fonts = system_fonts();
        if cfg!(target_os = "linux") {
            // Text without a font-family uses usvg's default, "Times New
            // Roman", which a Linux desktop rarely has: use the serif family
            // the database settled on instead (see `use_installed_generics`).
            opt.font_family = fonts.family_name(&usvg::fontdb::Family::Serif).to_owned();
        }
        opt.fontdb = fonts;
    }
    opt.image_href_resolver = resolver(dir, budget.clone());
    let tree = build_tree(&doc, &opt)?;
    budget.check()?;
    check_raster_work(&tree)?;

    let pdf = page_pdf(&tree)?;
    if pdf.len() as u64 > MAX_PDF_BYTES {
        return Err(pdf_too_large());
    }
    Ok(pdf)
}

/// One page of the drawing at [`page_dpi`]: svg2pdf writes the drawing as a
/// form XObject (`to_chunk`) and the page draws it scaled to its size.
///
/// Not `svg2pdf::to_pdf`: with a dpi other than 72 it scales the page
/// content but leaves every pattern's /Matrix in CSS px, and a pattern
/// matrix is relative to the page's default space, not the current
/// transform, so each gradient came out 4/3 too large and offset (round 8:
/// an A4 bar's middle stop printed at ~70 % of the bar). Inside a form, a
/// pattern is relative to the form's space, which the `cm` before `Do`
/// scales together with everything else.
fn page_pdf(tree: &usvg::Tree) -> Result<Vec<u8>, EngineError> {
    use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref};

    let size = tree.size();
    let scale = 72.0 / page_dpi(size.width(), size.height());
    let (width, height) = (size.width() * scale, size.height() * scale);
    let options = svg2pdf::ConversionOptions {
        raster_scale: RASTER_SCALE,
        ..svg2pdf::ConversionOptions::default()
    };
    let (chunk, drawing) = svg2pdf::to_chunk(tree, options)
        .map_err(|e| convert_failed(&format!("the SVG could not be converted: {e:?}")))?;

    // The page's own four objects come first; the chunk's ids (from 1) are
    // shifted past them.
    let (catalog, pages, page, content) = (Ref::new(1), Ref::new(2), Ref::new(3), Ref::new(4));
    let shift = |r: Ref| Ref::new(r.get() + 4);
    let chunk = chunk.renumber(shift);
    let drawing = shift(drawing);
    let name = Name(b"D");

    let mut pdf = Pdf::new();
    pdf.catalog(catalog).pages(pages);
    pdf.pages(pages).kids([page]).count(1);
    let mut p = pdf.page(page);
    p.media_box(Rect::new(0.0, 0.0, width, height));
    p.parent(pages);
    p.contents(content);
    p.resources().x_objects().pair(name, drawing);
    // As `to_pdf` does: blend the drawing's transparency in an isolated
    // RGB group.
    p.group()
        .transparency()
        .isolated(true)
        .knockout(false)
        .color_space()
        .device_rgb();
    p.finish();

    // The form maps its drawing onto the unit square.
    let mut c = Content::new();
    c.save_state()
        .transform([width, 0.0, 0.0, height, 0.0, 0.0])
        .x_object(name)
        .restore_state();
    pdf.stream(content, &c.finish());
    pdf.extend(&chunk);
    Ok(pdf.finish())
}

/// The dpi that maps the drawing onto the page: 96, or more for a drawing
/// whose longer side would exceed [`MAX_PAGE_PT`] (shrinking it uniformly).
/// Pure.
fn page_dpi(width_px: f32, height_px: f32) -> f32 {
    let longest_pt = width_px.max(height_px) * 72.0 / SVG_DPI;
    if longest_pt > MAX_PAGE_PT {
        SVG_DPI * longest_pt / MAX_PAGE_PT
    } else {
        SVG_DPI
    }
}

/// Read the SVG text, inflating an `.svgz` (sniffed, not trusted from the
/// extension) — either way at most [`SVG_MAX_BYTES`].
fn read_svg(input: &Path) -> Result<Vec<u8>, EngineError> {
    let cannot_read = |e: std::io::Error| {
        let name = input
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        EngineError::ConversionFailed(
            Failure::new(
                "cannot_read_input",
                format!("cannot read {}: {e}", input.display()),
            )
            .param("file", name),
        )
    };
    let file = std::fs::File::open(input).map_err(cannot_read)?;
    let mut raw = Vec::new();
    file.take(SVG_MAX_BYTES + 1)
        .read_to_end(&mut raw)
        .map_err(cannot_read)?;
    inflate_svg(raw)
}

/// `raw`, inflated when it is gzip, at most [`SVG_MAX_BYTES`] either way.
fn inflate_svg(raw: Vec<u8>) -> Result<Vec<u8>, EngineError> {
    let data = if raw.starts_with(&[0x1f, 0x8b]) {
        let mut inflated = Vec::new();
        flate2::read::GzDecoder::new(raw.as_slice())
            .take(SVG_MAX_BYTES + 1)
            .read_to_end(&mut inflated)
            .map_err(|e| {
                EngineError::ConversionFailed(Failure::new(
                    "svg_invalid",
                    format!("not a valid SVGZ: {e}"),
                ))
            })?;
        inflated
    } else {
        raw
    };
    if data.len() as u64 > SVG_MAX_BYTES {
        return Err(too_large(
            "the SVG is larger than the limit once decompressed",
        ));
    }
    Ok(data)
}

/// `svg_too_large`, with the limit in MB as its `limit` param (the sentence
/// adds the unit).
fn too_large(why: &str) -> EngineError {
    too_large_with(why, SVG_MAX_BYTES)
}

/// `svg_too_large` naming `limit` bytes (as MB).
fn too_large_with(why: &str, limit: u64) -> EngineError {
    let limit = (limit / (1024 * 1024)).to_string();
    EngineError::ConversionFailed(
        Failure::new("svg_too_large", format!("{why} ({limit} MB)")).param("limit", limit),
    )
}

/// The worker needed more than `cap` bytes of memory and was stopped.
fn over_memory(cap: u64) -> EngineError {
    too_large_with(
        "converting the SVG needs more memory than the limit; it was stopped",
        cap,
    )
}

/// The PDF would be larger than [`MAX_PDF_BYTES`].
fn pdf_too_large() -> EngineError {
    too_large_with("the PDF of the SVG is larger than the limit", MAX_PDF_BYTES)
}

/// The document as text: UTF-8, and not a second gzip layer (usvg would
/// inflate that again with no cap; no real SVGZ is compressed twice).
fn svg_text(data: &[u8]) -> Result<&str, EngineError> {
    if data.starts_with(&[0x1f, 0x8b]) {
        return Err(invalid("a compressed SVG inside a compressed SVG".into()));
    }
    std::str::from_utf8(data).map_err(|_| invalid("not a valid SVG: not UTF-8 text".into()))
}

fn invalid(why: String) -> EngineError {
    EngineError::ConversionFailed(Failure::new("svg_invalid", why))
}

/// Parse the (already inflated) text into the XML document usvg is handed,
/// within the entity and node caps of the module docs.
fn parse_document(text: &str) -> Result<usvg::roxmltree::Document<'_>, EngineError> {
    if entity_expansion(text) > SVG_MAX_BYTES {
        return Err(too_large("the SVG's entities expand past the limit"));
    }
    let xml_opt = usvg::roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: MAX_NODES,
    };
    usvg::roxmltree::Document::parse_with_options(text, xml_opt).map_err(|e| match e {
        usvg::roxmltree::Error::NodesLimitReached => too_large("the SVG has too many elements"),
        e => invalid(format!("not a valid SVG: {e}")),
    })
}

/// usvg's tree of `doc`.
fn build_tree(
    doc: &usvg::roxmltree::Document,
    opt: &usvg::Options,
) -> Result<usvg::Tree, EngineError> {
    usvg::Tree::from_xmltree(doc, opt).map_err(|e| match e {
        usvg::Error::ElementsLimitReached
        | usvg::Error::ParsingFailed(usvg::roxmltree::Error::NodesLimitReached) => {
            too_large("the SVG has too many elements")
        }
        e => invalid(format!("not a valid SVG: {e}")),
    })
}

/// What one conversion may still spend, shared by the drawing and every
/// resource its resolver loads (usvg calls the resolver from inside
/// `from_xmltree`, once per `<image>` copy).
#[derive(Default)]
struct Budget {
    /// Expansion work spent ([`expansion::check_expansion`] units).
    weight: AtomicU64,
    /// Resource bytes read or decoded.
    bytes: AtomicU64,
    /// Why a resource was refused for its size, if one was: the conversion
    /// then fails with `svg_too_large` rather than drawing without it.
    refused: Mutex<Option<EngineError>>,
}

impl Budget {
    /// Charge the expansion of `doc` against what is left.
    fn spend_expansion(&self, doc: &usvg::roxmltree::Document) -> Result<(), EngineError> {
        let left =
            expansion::MAX_EXPANDED_WEIGHT.saturating_sub(self.weight.load(Ordering::SeqCst));
        let spent = expansion::check_expansion(doc, left)?;
        self.weight.fetch_add(spent, Ordering::SeqCst);
        Ok(())
    }

    /// Charge `n` resource bytes; `false` (and the refusal recorded) past
    /// [`MAX_RESOURCE_BYTES`].
    fn spend_bytes(&self, n: u64) -> bool {
        let total = self.bytes.fetch_add(n, Ordering::SeqCst).saturating_add(n);
        if total > MAX_RESOURCE_BYTES {
            self.refuse(too_large_with(
                "the SVG's images add up to more than the limit",
                MAX_RESOURCE_BYTES,
            ));
            return false;
        }
        true
    }

    /// Record why a resource was refused (the first reason wins).
    fn refuse(&self, why: EngineError) {
        if let Ok(mut refused) = self.refused.lock() {
            refused.get_or_insert(why);
        }
    }

    /// The recorded refusal, if any.
    fn check(&self) -> Result<(), EngineError> {
        match self.refused.lock().ok().and_then(|mut r| r.take()) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

/// The `<image>` / `<feImage>` resolver: the only place an external reference
/// of the drawing is decided, on the exact string usvg parsed. `data:` URIs
/// are decoded by usvg and handed to `resolve_data`; every other href goes to
/// `resolve_string`, which loads only a file inside `dir` ([`local_only`]).
/// Whatever is loaded is checked by [`image_kind`].
fn resolver(dir: PathBuf, budget: Arc<Budget>) -> usvg::ImageHrefResolver<'static> {
    let data_budget = budget.clone();
    usvg::ImageHrefResolver {
        resolve_data: Box::new(move |mime, data, opts| {
            if !data_budget.spend_bytes(data.len() as u64) {
                return None;
            }
            // usvg's own rule: an SVG by its type, or text that is no raster.
            let svg_hint = match mime {
                "image/svg+xml" => true,
                "text/plain" => image::guess_format(&data).is_err(),
                "image/jpg" | "image/jpeg" | "image/png" | "image/gif" | "image/webp" => false,
                _ => return None,
            };
            image_kind(data, svg_hint, opts, &data_budget)
        }),
        resolve_string: Box::new(move |href, opts| {
            let path = local_only(href, &dir)?;
            let data = read_resource(&path, &budget)?;
            let svg_hint = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| SVG_EXTS.contains(&e.to_ascii_lowercase().as_str()));
            image_kind(Arc::new(data), svg_hint, opts, &budget)
        }),
    }
}

/// The bytes of the resource at `path`, read through a bounded reader and
/// charged to `budget`.
fn read_resource(path: &Path, budget: &Budget) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut data = Vec::new();
    file.take(SVG_MAX_BYTES + 1).read_to_end(&mut data).ok()?;
    if data.len() as u64 > SVG_MAX_BYTES {
        budget.refuse(too_large("an image of the SVG is larger than the limit"));
        return None;
    }
    budget.spend_bytes(data.len() as u64).then_some(data)
}

/// What usvg draws for a loaded resource: an SVG (when `svg_hint`, as usvg
/// decides it) through the same caps as the drawing, else a PNG, JPEG, GIF or
/// WebP by its magic bytes whose pixel count is within [`MAX_IMAGE_PIXELS`].
fn image_kind(
    data: Arc<Vec<u8>>,
    svg_hint: bool,
    opts: &usvg::Options,
    budget: &Budget,
) -> Option<usvg::ImageKind> {
    if svg_hint {
        return sub_svg(&data, opts, budget);
    }
    let format = image::guess_format(&data).ok()?;
    let (w, h) = raster_size(&data, format)?;
    if u64::from(w) * u64::from(h) > MAX_IMAGE_PIXELS {
        budget.refuse(too_large_with(
            "an image of the SVG has more pixels than the limit",
            MAX_IMAGE_PIXELS * 4,
        ));
        return None;
    }
    Some(match format {
        image::ImageFormat::Png => usvg::ImageKind::PNG(data),
        image::ImageFormat::Jpeg => usvg::ImageKind::JPEG(data),
        image::ImageFormat::Gif => usvg::ImageKind::GIF(data),
        image::ImageFormat::WebP => usvg::ImageKind::WEBP(data),
        _ => return None,
    })
}

/// The width and height a raster header declares, read without decoding.
fn raster_size(data: &[u8], format: image::ImageFormat) -> Option<(u32, u32)> {
    if format == image::ImageFormat::Gif {
        // The logical screen size, right after `GIF87a`/`GIF89a` (no GIF
        // decoder is built into the `image` crate here).
        let b = data.get(6..10)?;
        return Some((
            u32::from(u16::from_le_bytes([b[0], b[1]])),
            u32::from(u16::from_le_bytes([b[2], b[3]])),
        ));
    }
    image::ImageReader::with_format(std::io::Cursor::new(data), format)
        .into_dimensions()
        .ok()
}

/// An SVG embedded as an image, through the same caps as the drawing (bounded
/// inflate, entities, nodes, expansion charged to the same budget), built as
/// usvg builds one: with no images of its own. `None` (and the refusal
/// recorded) when it is too large; `None` when it is not a valid SVG.
fn sub_svg(data: &[u8], opts: &usvg::Options, budget: &Budget) -> Option<usvg::ImageKind> {
    let parsed = inflate_svg(data.to_vec()).and_then(|data| {
        let text = svg_text(&data)?;
        let doc = parse_document(text)?;
        budget.spend_expansion(&doc)?;
        let sub_opt = usvg::Options {
            resources_dir: None,
            dpi: opts.dpi,
            font_size: opts.font_size,
            languages: opts.languages.clone(),
            shape_rendering: opts.shape_rendering,
            text_rendering: opts.text_rendering,
            image_rendering: opts.image_rendering,
            default_size: opts.default_size,
            // The referenced SVG image cannot have any images of its own
            // (usvg's rule for sub-images).
            image_href_resolver: usvg::ImageHrefResolver {
                resolve_data: Box::new(|_, _, _| None),
                resolve_string: Box::new(|_, _| None),
            },
            fontdb: opts.fontdb.clone(),
            ..usvg::Options::default()
        };
        build_tree(&doc, &sub_opt)
    });
    match parsed {
        Ok(tree) => Some(usvg::ImageKind::SVG(tree)),
        Err(EngineError::ConversionFailed(f)) if f.reason == Some("svg_too_large") => {
            budget.refuse(EngineError::ConversionFailed(f));
            None
        }
        Err(_) => None,
    }
}

/// Bound the pixels svg2pdf would rasterise for filter effects: each group
/// with a filter is rendered into an RGBA pixmap of its layer bounding box
/// (in its parent's space) times [`RASTER_SCALE`], exactly as svg2pdf sizes
/// it. Clip paths, masks, patterns, `<feImage>` content and embedded SVGs are
/// walked too, each shared one once.
fn check_raster_work(tree: &usvg::Tree) -> Result<(), EngineError> {
    fn walk(
        group: &usvg::Group,
        seen: &mut std::collections::HashSet<*const usvg::Group>,
        pixels: &mut f64,
    ) {
        if !seen.insert(group as *const _) {
            return;
        }
        if !group.filters().is_empty() {
            if let Some(bbox) = group.layer_bounding_box().transform(group.transform()) {
                let scale = f64::from(RASTER_SCALE);
                *pixels += f64::from(bbox.width()) * scale * f64::from(bbox.height()) * scale;
            }
        }
        for child in group.children() {
            if let usvg::Node::Group(g) = child {
                walk(g, seen, pixels);
            }
            child.subroots(|root| walk(root, seen, pixels));
        }
    }
    let mut pixels = 0.0;
    let mut seen = std::collections::HashSet::new();
    walk(tree.root(), &mut seen, &mut pixels);
    if pixels > MAX_RASTER_PIXELS as f64 {
        return Err(too_large_with(
            "the SVG's filter effects need more pixels than the limit",
            MAX_RASTER_PIXELS * 4,
        ));
    }
    Ok(())
}

/// An upper bound of how many bytes `text` becomes once its general entities
/// are expanded: the text itself plus, for every `&name;` reference anywhere
/// in it, the expanded size of every `<!ENTITY name "value">` declared
/// anywhere in it (the largest, when a name is declared twice). Scanning the
/// whole text rather than parsing the DOCTYPE keeps it an over-estimate that
/// a crafted DTD cannot hide a declaration from. A self-referencing entity
/// counts as unbounded; character and predefined references count as
/// written. Pure; saturating.
pub(crate) fn entity_expansion(text: &str) -> u64 {
    use std::collections::HashMap;
    let base = text.len() as u64;
    let entities = entity_declarations(text);
    if entities.is_empty() {
        return base;
    }
    fn size_of<'a>(
        name: &'a str,
        entities: &HashMap<&'a str, Vec<&'a str>>,
        sizes: &mut HashMap<&'a str, u64>,
        depth: u32,
    ) -> u64 {
        if let Some(&n) = sizes.get(name) {
            return n;
        }
        let Some(values) = entities.get(name) else {
            return 0;
        };
        if depth > 16 {
            return u64::MAX;
        }
        // In progress: a loop back to this entity reads as unbounded.
        sizes.insert(name, u64::MAX);
        let mut largest = 0u64;
        for value in values {
            let mut total = value.len() as u64;
            for r in references(value) {
                total = total.saturating_add(size_of(r, entities, sizes, depth + 1));
            }
            largest = largest.max(total);
        }
        sizes.insert(name, largest);
        largest
    }
    let mut sizes = HashMap::new();
    let mut total = base;
    for r in references(text) {
        total = total.saturating_add(size_of(r, &entities, &mut sizes, 0));
        if total > SVG_MAX_BYTES {
            break;
        }
    }
    total
}

/// The `&name;` references in `s` (not `&#...;` character references).
fn references(s: &str) -> impl Iterator<Item = &str> {
    s.split('&').skip(1).filter_map(|after| {
        let end = after.find(';')?;
        let name = &after[..end];
        let valid = !name.is_empty()
            && !name.starts_with('#')
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | '-' | '.'));
        valid.then_some(name)
    })
}

/// Every `<!ENTITY name "value">` (or `'value'`) declared anywhere in
/// `text`, by name. Parameter (`%`) and external (`SYSTEM`/`PUBLIC`)
/// entities are skipped: neither is expanded into the drawing.
fn entity_declarations(text: &str) -> std::collections::HashMap<&str, Vec<&str>> {
    let mut entities: std::collections::HashMap<&str, Vec<&str>> = Default::default();
    let mut rest = text;
    while let Some(at) = rest.find("<!ENTITY") {
        rest = rest[at + "<!ENTITY".len()..].trim_start();
        if rest.starts_with('%') {
            continue;
        }
        let name_end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
        let name = &rest[..name_end];
        let after = rest[name_end..].trim_start();
        let Some(quote) = after.chars().next().filter(|c| matches!(c, '"' | '\'')) else {
            continue;
        };
        let Some(len) = after[1..].find(quote) else {
            break;
        };
        entities.entry(name).or_default().push(&after[1..1 + len]);
        rest = &after[1 + len..];
    }
    entities
}

/// Whether the parsed drawing may need fonts: it has a text element, or an
/// image (an embedded SVG shares the drawing's font database). Decided on the
/// document usvg is handed, never on a text scan. Pure.
fn needs_fonts(doc: &usvg::roxmltree::Document) -> bool {
    doc.descendants()
        .any(|n| matches!(n.tag_name().name(), "text" | "image" | "feImage"))
}

/// The system fonts, loaded once per process. The only fonts the conversion
/// uses: usvg has no `@font-face` support and never loads a font from a URL
/// or a path named in the drawing.
fn system_fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: std::sync::OnceLock<Arc<usvg::fontdb::Database>> = std::sync::OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            if cfg!(target_os = "linux") {
                use_installed_generics(&mut db);
            }
            Arc::new(db)
        })
        .clone()
}

/// Linux: fontdb maps the generic families to Windows and macOS fonts (Times
/// New Roman, Arial, Courier New), which a Linux desktop rarely has, and usvg
/// silently drops text whose family it cannot find. Point each generic family
/// that is missing at the first common Linux family that is installed, so
/// `serif`, `sans-serif` and `monospace` text prints as it does elsewhere.
fn use_installed_generics(db: &mut usvg::fontdb::Database) {
    use usvg::fontdb::Family;
    fn installed(db: &usvg::fontdb::Database, name: &str) -> bool {
        db.faces()
            .any(|face| face.families.iter().any(|(family, _)| family == name))
    }
    const SERIF: &[&str] = &[
        "DejaVu Serif",
        "Liberation Serif",
        "Noto Serif",
        "FreeSerif",
    ];
    const SANS: &[&str] = &["DejaVu Sans", "Liberation Sans", "Noto Sans", "FreeSans"];
    const MONO: &[&str] = &[
        "DejaVu Sans Mono",
        "Liberation Mono",
        "Noto Sans Mono",
        "FreeMono",
    ];
    for (generic, candidates) in [
        (Family::Serif, SERIF),
        (Family::SansSerif, SANS),
        (Family::Monospace, MONO),
    ] {
        if installed(db, db.family_name(&generic)) {
            continue;
        }
        let Some(name) = candidates.iter().find(|name| installed(db, name)) else {
            continue;
        };
        match generic {
            Family::Serif => db.set_serif_family(*name),
            Family::SansSerif => db.set_sans_serif_family(*name),
            _ => db.set_monospace_family(*name),
        }
    }
}

fn convert_failed(why: &str) -> EngineError {
    EngineError::ConversionFailed(Failure::new("svg_convert_failed", why))
}

/// The local file an `<image href>` may load: one that exists inside `dir`
/// (the SVG's own folder, canonical) once symlinks and `..` are resolved.
/// `None` for everything else, which then simply does not draw.
///
/// The href must be local by [`super::local_ref`] (decoded, slashes
/// normalised), and the path it resolves to is checked again by the same
/// predicate *before* it is opened: `canonicalize` on a share would already
/// have connected to it.
pub(crate) fn local_only(href: &str, dir: &Path) -> Option<PathBuf> {
    use super::local_ref::{self, RefClass};
    if local_ref::classify(href) != RefClass::Local {
        tracing::debug!(event = "svg_resource_refused", reason = "not_local");
        return None;
    }
    if dir.as_os_str().is_empty() {
        return None;
    }
    let path = href_path(href, cfg!(windows));
    let candidate = if path.is_absolute() || path.has_root() {
        path
    } else {
        dir.join(path)
    };
    if !local_ref::may_open(&candidate, dir) {
        tracing::debug!(event = "svg_resource_refused", reason = "not_local");
        return None;
    }
    let real = candidate.canonicalize().ok()?;
    if real.starts_with(dir) && real.is_file() {
        Some(real)
    } else {
        tracing::debug!(event = "svg_resource_refused", reason = "outside_folder");
        None
    }
}

/// The path a local href names (path rules of Windows when `windows`): the
/// decoded path of a host-less `file:` URL (`file:///C:/x` → `C:/x` on
/// Windows, `/x` elsewhere), else the href as written. Pure.
fn href_path(href: &str, windows: bool) -> PathBuf {
    let href = href.trim();
    let is_file_url = href
        .get(..5)
        .is_some_and(|head| head.eq_ignore_ascii_case("file:"));
    if !is_file_url {
        return PathBuf::from(href);
    }
    let rest = &href[5..];
    let rest = match rest.get(..11) {
        Some(head) if head.eq_ignore_ascii_case("//localhost") => &rest[11..],
        _ => rest,
    };
    let decoded = percent_encoding::percent_decode_str(rest.trim_start_matches('/'))
        .decode_utf8_lossy()
        .into_owned();
    let is_drive = decoded.as_bytes().get(1) == Some(&b':');
    if windows && is_drive {
        PathBuf::from(decoded)
    } else {
        PathBuf::from(format!("/{decoded}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn mediabox(pdf: &Path) -> Vec<f32> {
        let doc = lopdf::Document::load(pdf).unwrap();
        assert_eq!(doc.get_pages().len(), 1, "one page per drawing");
        let page = *doc.get_pages().values().next().unwrap();
        let dict = doc.get_dictionary(page).unwrap();
        dict.get(b"MediaBox")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o.as_float().unwrap())
            .collect()
    }

    fn convert_file(path: &Path) -> Result<PathBuf, EngineError> {
        let out = path.with_extension("out.pdf");
        convert_in_process(path, &out).map(|()| out)
    }

    const RECT: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
        <rect x="10" y="10" width="180" height="80" fill="#c33"/></svg>"##;

    /// FR-052a: a sized SVG becomes one vector page of its size at 96 dpi
    /// (200 × 100 px → 150 × 75 pt).
    #[test]
    fn fr_052a_svg_rect_mediabox_075() {
        let dir = tempfile::tempdir().unwrap();
        let svg = write(dir.path(), "r.svg", RECT);
        let pdf = convert_file(&svg).unwrap();
        let mb = mediabox(&pdf);
        assert!((mb[2] - mb[0] - 150.0).abs() < 0.01, "{mb:?}");
        assert!((mb[3] - mb[1] - 75.0).abs() < 0.01, "{mb:?}");
        // Vector, not a raster: no image XObject in the file.
        let bytes = std::fs::read(&pdf).unwrap();
        assert!(!bytes.windows(13).any(|w| w == b"/Subtype /Ima"));
    }

    /// FR-052a: a drawing with only a viewBox takes the viewBox size; with
    /// neither, the size of what it draws.
    #[test]
    fn fr_052a_svg_viewbox_default() {
        let dir = tempfile::tempdir().unwrap();
        let vb = write(
            dir.path(),
            "vb.svg",
            br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 400 300"><circle cx="200" cy="150" r="100"/></svg>"#,
        );
        let mb = mediabox(&convert_file(&vb).unwrap());
        assert!(
            (mb[2] - 300.0).abs() < 0.01 && (mb[3] - 225.0).abs() < 0.01,
            "{mb:?}"
        );
        let bare = write(
            dir.path(),
            "bare.svg",
            br#"<svg xmlns="http://www.w3.org/2000/svg"><circle cx="50" cy="50" r="40"/></svg>"#,
        );
        // usvg sizes such a drawing from its content's bounding box (here
        // 10..90 px): a small page, never an empty or giant one.
        let mb = mediabox(&convert_file(&bare).unwrap());
        assert!(
            (mb[2] - 67.5).abs() < 0.01 && (mb[3] - 67.5).abs() < 0.01,
            "{mb:?}"
        );
    }

    /// FR-052a: an enormous drawing is shrunk onto the largest page.
    #[test]
    fn fr_052a_svg_page_clamped_to_max() {
        assert_eq!(page_dpi(200.0, 100.0), 96.0);
        let dpi = page_dpi(40_000.0, 10_000.0);
        assert!((40_000.0 * 72.0 / dpi - MAX_PAGE_PT).abs() < 0.5);
        let dir = tempfile::tempdir().unwrap();
        let big = write(
            dir.path(),
            "big.svg",
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="40000" height="10000"><rect width="40000" height="10000"/></svg>"#,
        );
        let mb = mediabox(&convert_file(&big).unwrap());
        assert!((mb[2] - MAX_PAGE_PT).abs() < 1.0, "{mb:?}");
        assert!((mb[3] - MAX_PAGE_PT / 4.0).abs() < 1.0, "{mb:?}");
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(bytes).unwrap();
        gz.finish().unwrap()
    }

    /// FR-052a: `.svgz` (gzip) converts like the plain drawing — and the
    /// format is sniffed, so a gzipped file named `.svg` works too.
    #[test]
    fn fr_052a_svgz_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["r.svgz", "gz-named.svg"] {
            let svgz = write(dir.path(), name, &gzip(RECT));
            let mb = mediabox(&convert_file(&svgz).unwrap());
            assert!((mb[2] - 150.0).abs() < 0.01, "{name}: {mb:?}");
        }
    }

    /// FR-052a / NFR (security): a gzip bomb stops at the cap with
    /// `svg_too_large`, without inflating the rest.
    #[test]
    fn fr_052a_svgz_bomb_is_capped() {
        let dir = tempfile::tempdir().unwrap();
        // 65 MiB of spaces compresses to ~65 KiB.
        let mut big = b"<svg xmlns=\"http://www.w3.org/2000/svg\">".to_vec();
        big.resize((SVG_MAX_BYTES + 1024 * 1024) as usize, b' ');
        let bomb = write(dir.path(), "bomb.svgz", &gzip(&big));
        let err = convert_file(&bomb).unwrap_err();
        assert!(
            matches!(&err, EngineError::ConversionFailed(f)
                if f.reason == Some("svg_too_large") && f.params["limit"] == "64"),
            "{err:?}"
        );
    }

    /// NFR (security): a gzip stream inside the gzip stream is refused, never
    /// handed to usvg's own (uncapped) inflater. The inner layer here is a
    /// 100 MiB bomb that survives the outer cap at a few hundred KiB.
    #[test]
    fn nfr_security_svgz_nested_gzip_bomb_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut big = b"<svg xmlns=\"http://www.w3.org/2000/svg\">".to_vec();
        big.resize(100 * 1024 * 1024, b' ');
        let inner = gzip(&big);
        assert!((inner.len() as u64) < SVG_MAX_BYTES, "inner fits the cap");
        let nested = write(dir.path(), "nested.svgz", &gzip(&inner));
        let err = convert_file(&nested).unwrap_err();
        assert!(
            matches!(&err, EngineError::ConversionFailed(f) if f.reason == Some("svg_invalid")),
            "{err:?}"
        );
    }

    /// NFR (security): a small plain SVG whose entities expand past the cap
    /// (few large entities, which the XML parser's own reference-count guard
    /// lets through) is `svg_too_large`, refused before parsing.
    #[test]
    fn nfr_security_svg_entity_expansion_is_capped() {
        let dir = tempfile::tempdir().unwrap();
        let chunk = "x".repeat(1024 * 1024);
        let refs = "&a;".repeat(200);
        let doc = format!(
            "<?xml version=\"1.0\"?>\n<!DOCTYPE svg [ <!ENTITY a \"{chunk}\"> ]>\n\
             <svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"><text>{refs}</text></svg>"
        );
        assert!(entity_expansion(&doc) > SVG_MAX_BYTES);
        let path = write(dir.path(), "laughs.svg", doc.as_bytes());
        let err = convert_file(&path).unwrap_err();
        assert!(
            matches!(&err, EngineError::ConversionFailed(f) if f.reason == Some("svg_too_large")),
            "{err:?}"
        );

        // Nested and looping entities are bounded too; a plain drawing and
        // the usual Illustrator namespace entities cost nothing extra.
        let nested = "<!DOCTYPE s [<!ENTITY a \"xxxxxxxxxx\"><!ENTITY b \"&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;\">\
                      <!ENTITY c \"&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;\"><!ENTITY d \"&c;&c;&c;&c;&c;&c;&c;&c;&c;&c;\">\
                      <!ENTITY e \"&d;&d;&d;&d;&d;&d;&d;&d;&d;&d;\"><!ENTITY f \"&e;&e;&e;&e;&e;&e;&e;&e;&e;&e;\">\
                      <!ENTITY g \"&f;&f;&f;&f;&f;&f;&f;&f;&f;&f;\"><!ENTITY h \"&g;&g;&g;&g;&g;&g;&g;&g;&g;&g;\">]><s>&h;&h;</s>";
        assert!(entity_expansion(nested) > SVG_MAX_BYTES);
        let looped = "<!DOCTYPE s [<!ENTITY a \"&b;\"><!ENTITY b \"&a;\">]><s>&a;</s>";
        assert!(entity_expansion(looped) > SVG_MAX_BYTES);
        assert_eq!(
            entity_expansion(std::str::from_utf8(RECT).unwrap()),
            RECT.len() as u64
        );
        let ns = "<!DOCTYPE svg [<!ENTITY ns_svg \"http://www.w3.org/2000/svg\">]><svg xmlns=\"&ns_svg;\"/>";
        assert!(entity_expansion(ns) < 200);
    }

    /// NFR (security): too many elements is `svg_too_large`, not a parse of
    /// unbounded size.
    #[test]
    fn nfr_security_svg_node_limit() {
        let dir = tempfile::tempdir().unwrap();
        let mut doc =
            String::from("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\">");
        doc.push_str(&"<g/>".repeat(MAX_NODES as usize + 10));
        doc.push_str("</svg>");
        let path = write(dir.path(), "many.svg", doc.as_bytes());
        let err = convert_file(&path).unwrap_err();
        assert!(
            matches!(&err, EngineError::ConversionFailed(f) if f.reason == Some("svg_too_large")),
            "{err:?}"
        );
    }

    fn expansion_of(svg: &str) -> Result<(), EngineError> {
        let doc = usvg::roxmltree::Document::parse_with_options(
            svg,
            usvg::roxmltree::ParsingOptions {
                allow_dtd: true,
                nodes_limit: MAX_NODES,
            },
        )
        .unwrap();
        expansion::check_expansion(&doc, expansion::MAX_EXPANDED_WEIGHT).map(|_| ())
    }

    fn is_too_large(r: Result<(), EngineError>) -> bool {
        matches!(&r, Err(EngineError::ConversionFailed(f)) if f.reason == Some("svg_too_large"))
    }

    /// Security note 4: the caps hold on what usvg expands the parsed
    /// document to, not only on the document: a `<use>` pyramid, markers on
    /// every vertex (nested), patterns cloned per shape, and image copies
    /// are each refused small, before usvg builds them.
    #[test]
    fn nfr_security_svg_expansion_is_capped_on_the_parsed_tree() {
        const NS: &str =
            r#"xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink""#;
        // `<use>` pyramid: 10 levels of 10 copies, under 2 KB of source.
        let mut defs = String::from(r#"<rect id="l0" width="1" height="1"/>"#);
        for level in 1..=10 {
            defs.push_str(&format!("<g id=\"l{level}\">"));
            for _ in 0..10 {
                defs.push_str(&format!("<use href=\"#l{}\"/>", level - 1));
            }
            defs.push_str("</g>");
        }
        let pyramid = format!("<svg {NS} width=\"9\" height=\"9\"><defs>{defs}</defs><use xlink:href=\"#l10\"/></svg>");
        assert!(pyramid.len() < 2048);
        assert!(is_too_large(expansion_of(&pyramid)));

        // Markers that hold marked paths, on long paths.
        let points = "1 1 ".repeat(200);
        let mut markers = String::from(r#"<marker id="m0"><rect width="1" height="1"/></marker>"#);
        for level in 1..=4 {
            markers.push_str(&format!(
                "<marker id=\"m{level}\"><polyline points=\"{points}\" marker-mid=\"url(#m{})\"/></marker>",
                level - 1
            ));
        }
        let marked = format!(
            "<svg {NS} width=\"9\" height=\"9\"><defs>{markers}</defs><polyline points=\"{points}\" style=\"marker-mid: url(#m4)\"/></svg>"
        );
        assert!(is_too_large(expansion_of(&marked)));

        // A large pattern painting many shapes (set by a style sheet).
        let content = "<rect width=\"1\" height=\"1\" x=\"0\"/>".repeat(2000);
        let shapes = "<rect width=\"1\" height=\"1\"/>".repeat(2000);
        let painted = format!(
            "<svg {NS} width=\"9\" height=\"9\"><style>rect {{ fill: url(#p) }}</style><pattern id=\"p\" width=\"1\" height=\"1\">{content}</pattern>{shapes}</svg>"
        );
        assert!(is_too_large(expansion_of(&painted)));

        // One image, copied by `<use>` past the image cap.
        let mut copies = String::new();
        for _ in 0..(expansion::MAX_EXPANDED_IMAGES + 1) {
            copies.push_str("<use href=\"#i\"/>");
        }
        let images = format!(
            "<svg {NS} width=\"9\" height=\"9\"><defs><image id=\"i\" href=\"a.png\" width=\"1\" height=\"1\"/></defs>{copies}</svg>"
        );
        assert!(is_too_large(expansion_of(&images)));

        // Ordinary drawings pass: a few uses, a marker on a line, a pattern.
        let fine = format!(
            "<svg {NS} width=\"9\" height=\"9\"><defs><marker id=\"a\"><path d=\"M0 0L1 1\"/></marker>\
             <pattern id=\"p\" width=\"2\" height=\"2\"><rect width=\"1\" height=\"1\"/></pattern>\
             <circle id=\"c\" r=\"1\"/></defs><use href=\"#c\"/><use href=\"#c\" x=\"3\"/>\
             <line x1=\"0\" y1=\"0\" x2=\"5\" y2=\"5\" marker-end=\"url(#a)\" stroke=\"black\"/>\
             <rect width=\"5\" height=\"5\" fill=\"url(#p)\"/></svg>"
        );
        assert!(expansion_of(&fine).is_ok());
        assert!(expansion_of(std::str::from_utf8(RECT).unwrap()).is_ok());
        // A self-referencing `<use>` is skipped, as usvg skips it.
        let looped = format!("<svg {NS}><g id=\"g\"><use href=\"#g\"/></g></svg>");
        assert!(expansion_of(&looped).is_ok());

        // End to end: the pyramid is refused as too large.
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "pyramid.svg", pyramid.as_bytes());
        let err = convert_file(&path).unwrap_err();
        assert!(
            matches!(&err, EngineError::ConversionFailed(f) if f.reason == Some("svg_too_large")),
            "{err:?}"
        );
    }

    const NS: &str =
        r#"xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink""#;

    /// The finding's mask bomb: `levels` masks, each holding ten rects masked
    /// by the previous one (default `objectBoundingBox` units, so usvg
    /// converts each again per reference), one visible rect on top.
    fn mask_bomb(levels: usize, tag: &str, attr: &str) -> String {
        let mut defs = format!("<{tag} id=\"m0\"><rect width=\"1\" height=\"1\"/></{tag}>");
        for level in 1..=levels {
            defs.push_str(&format!("<{tag} id=\"m{level}\">"));
            for i in 0..10 {
                defs.push_str(&format!(
                    "<rect x=\"{i}\" width=\"1\" height=\"1\" {attr}=\"url(#m{})\"/>",
                    level - 1
                ));
            }
            defs.push_str(&format!("</{tag}>"));
        }
        format!(
            "<svg {NS} width=\"20\" height=\"20\"><defs>{defs}</defs>\
             <rect width=\"10\" height=\"10\" {attr}=\"url(#m{levels})\"/></svg>"
        )
    }

    /// Round-6 finding (security note 4): masks and clip paths are followed.
    /// The 3.7 KB six-level mask bomb (a million masks in usvg, 156 s) and
    /// the same with `objectBoundingBox` clip paths are refused at once as
    /// `svg_too_large`, end to end. Round 7: so is the same with the default
    /// (`userSpaceOnUse`) clip paths or masks, which usvg shares but svg2pdf
    /// writes out once per use (a 3.6 KB drawing made a 78 MB PDF).
    #[test]
    fn nfr_security_svg_mask_and_clip_bombs_are_refused() {
        let bomb = mask_bomb(6, "mask", "mask");
        assert!(bomb.len() < 4096, "{}", bomb.len());
        assert!(is_too_large(expansion_of(&bomb)));
        let clips = mask_bomb(
            6,
            "clipPath clipPathUnits=\"objectBoundingBox\"",
            "clip-path",
        )
        .replace(
            "</clipPath clipPathUnits=\"objectBoundingBox\">",
            "</clipPath>",
        );
        assert!(is_too_large(expansion_of(&clips)));
        // In a style attribute and a style sheet, too.
        let styled = bomb.replace("mask=\"url(#m", "style=\"mask: url(#m");
        assert!(is_too_large(expansion_of(&styled)));
        let mut sheet = bomb.clone();
        let mut rules = String::new();
        for level in 0..=6 {
            sheet = sheet.replace(
                &format!("mask=\"url(#m{level})\""),
                &format!("class=\"k{level}\""),
            );
            rules.push_str(&format!(
                "g .k{level}, .k{level} {{ mask: url(#m{level}) }}\n"
            ));
        }
        let sheet = sheet.replace("<defs>", &format!("<style>{rules}</style><defs>"));
        assert!(!sheet.contains("mask=\""));
        assert!(is_too_large(expansion_of(&sheet)));

        // Round 7: shared units do not make a bomb cheap.
        let shared = mask_bomb(6, "clipPath", "clip-path");
        assert!(is_too_large(expansion_of(&shared)));
        let shared = mask_bomb(6, "mask maskUnits=\"userSpaceOnUse\"", "mask")
            .replace("</mask maskUnits=\"userSpaceOnUse\">", "</mask>");
        assert!(is_too_large(expansion_of(&shared)));
        // One shallower level of the same nesting (ten thousand clip paths)
        // still passes: references are charged, not refused.
        assert!(expansion_of(&mask_bomb(4, "clipPath", "clip-path")).is_ok());

        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "maskbomb.svg", bomb.as_bytes());
        let started = std::time::Instant::now();
        let err = convert_file(&path).unwrap_err();
        assert!(
            matches!(&err, EngineError::ConversionFailed(f) if f.reason == Some("svg_too_large")),
            "{err:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    /// Round-6 finding: the expansion check is bounded work. A style sheet
    /// with a huge number of `url()` references (to a gradient, which adds
    /// nothing usvg multiplies) and many shapes no longer costs references
    /// × shapes: every reference read is charged, so it ends fast.
    #[test]
    fn nfr_security_svg_style_url_refs_are_bounded_work() {
        let refs = "fill:url(#g);".repeat(200_000);
        let shapes = "<path d=\"M0 0h1\"/>".repeat(100_000);
        let svg = format!(
            "<svg {NS} width=\"9\" height=\"9\"><style>path {{ {refs} }}</style>\
             <linearGradient id=\"g\"/>{shapes}</svg>"
        );
        let started = std::time::Instant::now();
        assert!(is_too_large(expansion_of(&svg)));
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "{:?}",
            started.elapsed()
        );
        // The walk never spends more than its budget, whatever it is given.
        let doc = usvg::roxmltree::Document::parse(&svg).unwrap();
        assert!(expansion::check_expansion(&doc, 1_000).is_err());
    }

    /// A map-like drawing of about 2 MB (thousands of paths with long `d`,
    /// classes from a style sheet, gradients, shared clip paths, a pattern, a
    /// mask and markers) passes the expansion check and converts.
    #[test]
    fn fr_052a_complex_map_like_drawing_still_converts() {
        let mut body = String::new();
        let mut d = String::from("M0 0");
        for i in 0..60 {
            d.push_str(&format!(" L{}.{} {}.{}", i * 3, i % 7, (i * 7) % 50, i % 3));
        }
        d.push_str(" Z");
        for region in 0..3_000 {
            body.push_str(&format!(
                "<path id=\"r{region}\" class=\"land c{}\" d=\"{d}\" transform=\"translate({} {})\" clip-path=\"url(#frame)\"/>",
                region % 12,
                (region % 60) * 10,
                (region / 60) * 10
            ));
        }
        let mut css = String::from(".land { stroke: #555; stroke-width: .2 }\n");
        for c in 0..12 {
            css.push_str(&format!(".c{c} {{ fill: url(#grad{}) }}\n", c % 4));
        }
        let mut defs = String::new();
        for g in 0..4 {
            defs.push_str(&format!(
                "<linearGradient id=\"grad{g}\"><stop offset=\"0\" stop-color=\"#9c{g}\"/><stop offset=\"1\" stop-color=\"#3a{g}\"/></linearGradient>"
            ));
        }
        let svg = format!(
            "<svg {NS} width=\"600\" height=\"500\"><style>{css}</style><defs>{defs}\
             <clipPath id=\"frame\"><rect width=\"600\" height=\"500\"/></clipPath>\
             <pattern id=\"sea\" width=\"8\" height=\"8\" patternUnits=\"userSpaceOnUse\"><path d=\"M0 8L8 0\" stroke=\"#9cf\"/></pattern>\
             <mask id=\"fade\"><rect width=\"600\" height=\"500\" fill=\"white\"/></mask>\
             <marker id=\"dot\"><circle r=\"1\"/></marker></defs>\
             <rect width=\"600\" height=\"500\" fill=\"url(#sea)\"/>\
             <g mask=\"url(#fade)\">{body}</g>\
             <polyline points=\"0 0 10 10 20 0 30 10\" marker-mid=\"url(#dot)\" fill=\"none\" stroke=\"red\"/></svg>"
        );
        assert!(svg.len() > 1_500_000, "{}", svg.len());
        assert!(expansion_of(&svg).is_ok());
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "map.svg", svg.as_bytes());
        mediabox(&convert_file(&path).unwrap());
    }

    fn svg_with_data_image(mime: &str, bytes: &[u8]) -> Vec<u8> {
        use base64::Engine as _;
        let data = base64::engine::general_purpose::STANDARD.encode(bytes);
        svg_with_image(&format!("data:{mime};base64,{data}"))
    }

    /// Security note 8: an SVG embedded as an image (a `data:` URI, or a file
    /// next to the drawing) goes through the same caps as the drawing: a
    /// `<use>` pyramid inside one is `svg_too_large`, not handed to usvg's
    /// uncapped loader; a plain one draws.
    #[test]
    fn nfr_security_svg_embedded_svg_is_capped() {
        let mut defs = String::from(r#"<rect id="l0" width="1" height="1"/>"#);
        for level in 1..=8 {
            defs.push_str(&format!("<g id=\"l{level}\">"));
            for _ in 0..10 {
                defs.push_str(&format!("<use href=\"#l{}\"/>", level - 1));
            }
            defs.push_str("</g>");
        }
        let pyramid = format!(
            "<svg {NS} width=\"9\" height=\"9\"><defs>{defs}</defs><use href=\"#l8\"/></svg>"
        );
        let dir = tempfile::tempdir().unwrap();
        for (name, svg) in [
            (
                "data.svg",
                svg_with_data_image("image/svg+xml", pyramid.as_bytes()),
            ),
            ("file.svg", {
                write(dir.path(), "inner.svg", pyramid.as_bytes());
                svg_with_image("inner.svg")
            }),
            ("svgz.svg", {
                write(dir.path(), "inner.svgz", &gzip(pyramid.as_bytes()));
                svg_with_image("inner.svgz")
            }),
        ] {
            let path = write(dir.path(), name, &svg);
            let err = convert_file(&path).unwrap_err();
            assert!(
                matches!(&err, EngineError::ConversionFailed(f) if f.reason == Some("svg_too_large")),
                "{name}: {err:?}"
            );
        }
        // A plain embedded SVG and a data: PNG still draw.
        let ok = write(
            dir.path(),
            "ok.svg",
            &svg_with_data_image("image/svg+xml", RECT),
        );
        convert_file(&ok).unwrap();
        let png_svg = write(
            dir.path(),
            "png.svg",
            &svg_with_data_image("image/png", &png()),
        );
        assert!(embeds_an_image(&convert_file(&png_svg).unwrap()));
    }

    /// A PNG header declaring `w` × `h` pixels (one byte a pixel, grey), its
    /// rows all zero: a few hundred bytes for any size.
    fn png_bomb(w: u32, h: u32) -> Vec<u8> {
        use std::io::Write;
        fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
            out.extend((data.len() as u32).to_be_bytes());
            let mut crc = flate2::Crc::new();
            crc.update(kind);
            crc.update(data);
            out.extend(kind);
            out.extend(data);
            out.extend(crc.sum().to_be_bytes());
        }
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend(w.to_be_bytes());
        ihdr.extend(h.to_be_bytes());
        ihdr.extend([8, 0, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &ihdr);
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        let row = vec![0u8; w as usize + 1];
        for _ in 0..h.min(64) {
            z.write_all(&row).unwrap();
        }
        chunk(&mut out, b"IDAT", &z.finish().unwrap());
        chunk(&mut out, b"IEND", &[]);
        out
    }

    /// Security note 8: a raster `<image>` with more pixels than the cap is
    /// `svg_too_large` before svg2pdf decodes it.
    #[test]
    fn nfr_security_svg_raster_pixel_bomb_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "huge.png", &png_bomb(30_000, 30_000));
        let path = write(dir.path(), "bomb.svg", &svg_with_image("huge.png"));
        let err = convert_file(&path).unwrap_err();
        assert!(
            matches!(&err, EngineError::ConversionFailed(f) if f.reason == Some("svg_too_large")),
            "{err:?}"
        );
    }

    /// Security note 8: filter effects whose rasterised region exceeds the
    /// pixel cap are `svg_too_large`, judged on usvg's tree (the region
    /// svg2pdf would allocate), before anything is rasterised.
    #[test]
    fn nfr_security_svg_filter_region_bomb_is_refused() {
        let svg = format!(
            "<svg {NS} width=\"40000\" height=\"40000\"><filter id=\"b\"><feFlood flood-color=\"red\"/></filter>\
             <rect width=\"40000\" height=\"40000\" filter=\"url(#b)\"/></svg>"
        );
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "region.svg", svg.as_bytes());
        let err = convert_file(&path).unwrap_err();
        assert!(
            matches!(&err, EngineError::ConversionFailed(f) if f.reason == Some("svg_too_large")),
            "{err:?}"
        );
        // A small blur converts.
        let small = format!(
            "<svg {NS} width=\"100\" height=\"100\"><filter id=\"b\"><feGaussianBlur stdDeviation=\"2\"/></filter>\
             <rect x=\"10\" y=\"10\" width=\"80\" height=\"80\" filter=\"url(#b)\"/></svg>"
        );
        let path = write(dir.path(), "blur.svg", small.as_bytes());
        mediabox(&convert_file(&path).unwrap());
    }

    /// Security note 8: the only external references usvg follows are the
    /// ones its resolver returns: a share, a URL or a file outside the
    /// folder resolves to nothing, through `<image>` and `<feImage>` alike.
    #[test]
    fn nfr_security_svg_resolver_is_the_only_door() {
        let root = tempfile::tempdir().unwrap();
        let inside = root.path().join("doc");
        std::fs::create_dir(&inside).unwrap();
        write(root.path(), "secret.png", &png());
        write(&inside, "ok.png", &png());
        for href in [
            "../secret.png",
            r"\\attacker\share\x.png",
            "file://attacker/share/x.png",
            "https://example.com/x.png",
        ] {
            let fe = format!(
                "<svg {NS} width=\"20\" height=\"20\"><filter id=\"f\"><feImage xlink:href=\"{href}\"/></filter>\
                 <rect width=\"20\" height=\"20\" filter=\"url(#f)\"/></svg>"
            );
            let path = write(&inside, "fe.svg", fe.as_bytes());
            // Converts (the reference simply does not draw), or is refused;
            // never reads the secret.
            if let Ok(pdf) = convert_file(&path) {
                let _ = std::fs::read(pdf).unwrap();
            }
            let path = write(&inside, "img.svg", &svg_with_image(href));
            assert!(!embeds_an_image(&convert_file(&path).unwrap()), "{href}");
        }
        let path = write(&inside, "ok.svg", &svg_with_image("ok.png"));
        assert!(embeds_an_image(&convert_file(&path).unwrap()));
    }

    /// NFR (security): the `<image>` resolver refuses every share spelling
    /// (mixed slashes, escapes, `\\?\UNC\`, devices, `file://host/`) before
    /// anything is opened, and refuses a resolved path on a share too.
    #[test]
    fn nfr_security_svg_image_share_bypass_vectors() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().canonicalize().unwrap();
        for href in [
            r"\\attacker\share\x.png",
            "//attacker/share/x.png",
            r"/\attacker\share\x.png",
            r"\/attacker/share/x.png",
            r"\\?\UNC\attacker\share\x.png",
            r"\\.\pipe\x",
            r"\??\UNC\attacker\share\x.png",
            "file://attacker/share/x.png",
            "file:////attacker/share/x.png",
            "file:///%5C%5Cattacker%5Cshare%5Cx.png",
            "%5C%5Cattacker%5Cshare%5Cx.png",
            "http://example.com/x.png",
            "smb://host/x.png",
            "",
        ] {
            assert!(local_only(href, &dir).is_none(), "{href}");
        }
        // The predicate is applied to the resolved path as well.
        assert!(!super::super::local_ref::may_open(
            Path::new(r"\\attacker\share\x.png"),
            &dir
        ));
    }

    /// A tiny 2 × 2 PNG for `<image>` tests.
    fn png() -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(2, 2, image::Rgb([0, 128, 255]))
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    fn svg_with_image(href: &str) -> Vec<u8> {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="20" height="20"><image width="20" height="20" xlink:href="{href}"/></svg>"#
        )
        .into_bytes()
    }

    fn embeds_an_image(pdf: &Path) -> bool {
        let bytes = std::fs::read(pdf).unwrap();
        bytes.windows(13).any(|w| w == b"/Subtype /Ima")
    }

    /// FR-052a: an `<image>` next to the drawing (relative href) is drawn.
    #[test]
    fn fr_052a_relative_image() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("img")).unwrap();
        write(&dir.path().join("img"), "dot.png", &png());
        let svg = write(dir.path(), "with-image.svg", &svg_with_image("img/dot.png"));
        assert!(embeds_an_image(&convert_file(&svg).unwrap()));
    }

    /// NFR (security): images outside the SVG's folder, on a share or on the
    /// network never load; the drawing still converts without them.
    #[test]
    fn nfr_security_svg_local_only_table() {
        // How a local href becomes a path, both path flavours.
        assert_eq!(href_path("img/a.png", false), PathBuf::from("img/a.png"));
        assert_eq!(
            href_path(r"C:\pics\a.png", true),
            PathBuf::from(r"C:\pics\a.png")
        );
        assert_eq!(
            href_path("file:///C:/pics/a%20b.png", true),
            PathBuf::from("C:/pics/a b.png")
        );
        assert_eq!(
            href_path("file:///tmp/a.png", false),
            PathBuf::from("/tmp/a.png")
        );
        assert_eq!(
            href_path("FILE://localhost/tmp/a.png", false),
            PathBuf::from("/tmp/a.png")
        );

        // On disk: inside the folder only, symlinks and `..` resolved.
        let root = tempfile::tempdir().unwrap();
        let inside = root.path().join("doc");
        std::fs::create_dir(&inside).unwrap();
        let dir = inside.canonicalize().unwrap();
        write(root.path(), "secret.png", &png());
        write(&inside, "ok.png", &png());
        assert!(local_only("ok.png", &dir).is_some());
        // An absolute path as an author writes it (not the `\\?\` form a
        // canonical Windows path takes, which reads like a share and is
        // refused).
        assert!(local_only(&inside.join("ok.png").to_string_lossy(), &dir).is_some());
        assert!(local_only("../secret.png", &dir).is_none(), "../ escapes");
        let outside_abs = root.path().join("secret.png");
        assert!(local_only(&outside_abs.to_string_lossy(), &dir).is_none());
        assert!(local_only("missing.png", &dir).is_none());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside_abs, inside.join("link.png")).unwrap();
            assert!(local_only("link.png", &dir).is_none(), "symlink escapes");
        }

        // End to end: the outside image is simply not drawn.
        let svg = write(&inside, "leak.svg", &svg_with_image("../secret.png"));
        assert!(!embeds_an_image(&convert_file(&svg).unwrap()));
        let svg = write(
            &inside,
            "abs.svg",
            &svg_with_image(&outside_abs.to_string_lossy()),
        );
        assert!(!embeds_an_image(&convert_file(&svg).unwrap()));
    }

    /// NFR (security): an external entity cannot pull a file into the PDF.
    #[test]
    fn fr_052a_xxe_no_leak() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "secret.txt", b"TOP-SECRET-CONTENT");
        let secret = dir.path().join("secret.txt");
        let svg = format!(
            r#"<?xml version="1.0"?>
<!DOCTYPE svg [ <!ENTITY xxe SYSTEM "file://{}"> ]>
<svg xmlns="http://www.w3.org/2000/svg" width="100" height="40"><text x="0" y="20">&xxe;</text></svg>"#,
            secret.display()
        );
        let path = write(dir.path(), "xxe.svg", svg.as_bytes());
        // Either refused as invalid, or converted without the entity: never
        // with the file's contents.
        if let Ok(pdf) = convert_file(&path) {
            let bytes = std::fs::read(pdf).unwrap();
            assert!(!bytes.windows(10).any(|w| w == b"TOP-SECRET"));
        }
    }

    /// FR-052a: a file that is not SVG is `svg_invalid`, not a crash.
    #[test]
    fn fr_052a_malformed_conversion_failed() {
        let dir = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            ("junk.svg", &b"definitely not xml"[..]),
            ("html.svg", &b"<html><body>hi</body></html>"[..]),
            ("broken.svgz", &[0x1f, 0x8b, 0x08, 0x00, 0x01][..]),
        ] {
            let path = write(dir.path(), name, bytes);
            let err = convert_file(&path).unwrap_err();
            assert!(
                matches!(&err, EngineError::ConversionFailed(f) if f.reason == Some("svg_invalid")),
                "{name}: {err:?}"
            );
        }
    }

    /// FR-052a: the route honours the cancel flag.
    #[test]
    fn fr_052a_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let svg = write(dir.path(), "r.svg", RECT);
        let cancel = AtomicBool::new(true);
        let err = convert(&svg, &dir.path().join("o.pdf"), &cancel, None, &|_| {}).unwrap_err();
        assert!(matches!(err, EngineError::Cancelled), "{err:?}");
    }

    /// FR-052a: text is drawn with the system fonts (embedded, vector).
    #[test]
    fn fr_052a_text_svg_produces_a_parseable_pdf() {
        let dir = tempfile::tempdir().unwrap();
        let svg = write(
            dir.path(),
            "t.svg",
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="300" height="60"><text x="10" y="40" font-size="30" font-family="sans-serif">Pluryprint</text></svg>"#,
        );
        let parse = |b: &[u8]| {
            needs_fonts(&usvg::roxmltree::Document::parse(std::str::from_utf8(b).unwrap()).unwrap())
        };
        assert!(parse(&std::fs::read(&svg).unwrap()));
        assert!(!parse(RECT));
        let pdf = convert_file(&svg).unwrap();
        mediabox(&pdf);
    }

    /// FR-052a on Linux: text in a generic family, or with no family at all,
    /// still prints when the Windows and macOS fonts fontdb names by default
    /// are not installed (the drawing used to come out without its text).
    #[cfg(target_os = "linux")]
    #[test]
    fn fr_052a_generic_family_text_prints_on_linux() {
        let dir = tempfile::tempdir().unwrap();
        for family in [
            "",
            " font-family=\"serif\"",
            " font-family=\"sans-serif\"",
            " font-family=\"monospace\"",
        ] {
            let body = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="300" height="60"><text x="10" y="40" font-size="30"{family}>Pluryprint</text></svg>"#
            );
            let svg = write(dir.path(), "g.svg", body.as_bytes());
            let pdf = std::fs::read(convert_file(&svg).unwrap()).unwrap();
            assert!(
                pdf.windows(5).any(|w| w == b"/Font"),
                "no font in the PDF for{family:?}"
            );
        }
    }

    /// A PDF matrix `[a b c d e f]`, composed as PDF does (row vectors).
    type Matrix = [f32; 6];

    /// `a` then `b`: the matrix that applies `a` first.
    fn then(a: Matrix, b: Matrix) -> Matrix {
        [
            a[0] * b[0] + a[1] * b[2],
            a[0] * b[1] + a[1] * b[3],
            a[2] * b[0] + a[3] * b[2],
            a[2] * b[1] + a[3] * b[3],
            a[4] * b[0] + a[5] * b[2] + b[4],
            a[4] * b[1] + a[5] * b[3] + b[5],
        ]
    }

    fn apply(m: Matrix, x: f32, y: f32) -> (f32, f32) {
        (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
    }

    fn matrix_of(obj: &lopdf::Object) -> Matrix {
        let v: Vec<f32> = obj
            .as_array()
            .unwrap()
            .iter()
            .map(|o| o.as_float().unwrap())
            .collect();
        v.try_into().unwrap()
    }

    fn dict_at<'a>(doc: &'a lopdf::Document, obj: &'a lopdf::Object) -> &'a lopdf::Dictionary {
        match doc.dereference(obj).unwrap().1 {
            lopdf::Object::Dictionary(d) => d,
            lopdf::Object::Stream(s) => &s.dict,
            other => panic!("not a dictionary: {other:?}"),
        }
    }

    /// Every pattern a content stream (and the forms it draws) paints with,
    /// as the matrix that maps pattern space onto the page. A pattern's
    /// /Matrix is relative to the default space of the stream whose
    /// resources hold it (`base`: identity for the page, the form's matrix
    /// times the CTM at its `Do` for a form), not to the CTM at the fill.
    fn patterns_in(
        doc: &lopdf::Document,
        resources: &lopdf::Dictionary,
        content: &[u8],
        base: Matrix,
        out: &mut Vec<Matrix>,
    ) {
        if let Ok(patterns) = resources.get(b"Pattern") {
            for (_, p) in dict_at(doc, patterns).iter() {
                let m = dict_at(doc, p)
                    .get(b"Matrix")
                    .map(matrix_of)
                    .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
                out.push(then(m, base));
            }
        }
        let mut ctm = base;
        let mut stack = Vec::new();
        for op in lopdf::content::Content::decode(content).unwrap().operations {
            match op.operator.as_str() {
                "q" => stack.push(ctm),
                "Q" => ctm = stack.pop().unwrap(),
                "cm" => {
                    let m: Vec<f32> = op.operands.iter().map(|o| o.as_float().unwrap()).collect();
                    ctm = then(m.try_into().unwrap(), ctm);
                }
                "Do" => {
                    let name = op.operands[0].as_name().unwrap();
                    let xobjects = dict_at(doc, resources.get(b"XObject").unwrap());
                    let id = xobjects.get(name).unwrap().as_reference().unwrap();
                    let form = doc.get_object(id).unwrap().as_stream().unwrap();
                    let matrix = form
                        .dict
                        .get(b"Matrix")
                        .map(matrix_of)
                        .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
                    let inner = form
                        .dict
                        .get(b"Resources")
                        .map(|r| dict_at(doc, r).clone())
                        .unwrap_or_default();
                    let bytes = form
                        .decompressed_content()
                        .unwrap_or_else(|_| form.content.clone());
                    patterns_in(doc, &inner, &bytes, then(matrix, ctm), out);
                }
                _ => {}
            }
        }
    }

    /// FR-052a: an `objectBoundingBox` gradient spans exactly the shape it
    /// fills on the 96 dpi page. svg2pdf's `to_pdf` scales the page content
    /// by 72/96 but leaves the pattern matrices in CSS px, so the gradient
    /// came out 4/3 too large and offset (on an A4 drawing, the bar's
    /// mid-stop drifted from the middle to ~70 % of the bar, round 8 print).
    #[test]
    fn fr_052a_svg_gradient_spans_its_shape() {
        let dir = tempfile::tempdir().unwrap();
        let svg = write(
            dir.path(),
            "g.svg",
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100">
            <defs><linearGradient id="g"><stop offset="0" stop-color="#0cc"/>
            <stop offset="0.5" stop-color="#c0c"/><stop offset="1" stop-color="#ee0"/></linearGradient></defs>
            <rect x="20" y="10" width="160" height="80" fill="url(#g)"/></svg>"##,
        );
        let pdf = convert_file(&svg).unwrap();
        let doc = lopdf::Document::load(&pdf).unwrap();
        let page = *doc.get_pages().values().next().unwrap();
        let page_dict = doc.get_dictionary(page).unwrap();
        let resources = dict_at(&doc, page_dict.get(b"Resources").unwrap()).clone();
        let content = doc.get_page_content(page);
        let mut found = Vec::new();
        let identity = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        patterns_in(&doc, &resources, &content, identity, &mut found);
        assert_eq!(found.len(), 1, "one gradient pattern: {found:?}");
        // The bounding box (unit square) lands on the rect: 20..180 ×
        // 10..90 px → 15..135 × 7.5..67.5 pt, the y axis flipped.
        let close = |(x, y): (f32, f32), (ex, ey): (f32, f32)| {
            assert!(
                (x - ex).abs() < 0.01 && (y - ey).abs() < 0.01,
                "({x}, {y}) != ({ex}, {ey}); pattern {:?}",
                found[0]
            )
        };
        close(apply(found[0], 0.0, 0.0), (15.0, 67.5));
        close(apply(found[0], 1.0, 1.0), (135.0, 7.5));
        close(apply(found[0], 0.5, 0.0), (75.0, 67.5));
        assert_eq!(mediabox(&pdf), vec![0.0, 0.0, 150.0, 75.0]);
    }

    /// The worker's failure line round-trips its reason and params; a line
    /// with a reason of another route is ignored.
    #[test]
    fn worker_failure_line_round_trips() {
        let failure = Failure::new("svg_too_large", "too big").param("limit", "64");
        let stderr = format!("some noise\n{}\n", report_failure(&failure));
        assert_eq!(parse_failure(&stderr), Some(failure));
        assert_eq!(
            parse_failure(r#"pluryprint-failure {"reason":"no_printer","detail":"x"}"#),
            None
        );
        assert_eq!(parse_failure("panicked at ..."), None);
    }

    /// Through the full router: an SVG job yields a one-page cache PDF.
    #[test]
    fn fr_052a_svg_through_convert_to_pdf() {
        let dir = tempfile::tempdir().unwrap();
        let svg = write(dir.path(), "r.svg", RECT);
        let cancel = AtomicBool::new(false);
        let out = super::super::convert_to_pdf(
            &svg,
            &dir.path().join("cache"),
            "job",
            &cancel,
            &|_| {},
            None,
            &super::super::PageSetup::default(),
            None,
        )
        .unwrap()
        .expect("a cache PDF");
        mediabox(&out);
    }
}
