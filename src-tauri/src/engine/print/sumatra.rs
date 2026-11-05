//! SumatraPDF backend: `-print-settings` string construction (pure, all
//! platforms), locating the executable via [`crate::engine::deps`], and, on
//! Windows, spooling.
//!
//! Flag mapping (`docs/03-requirements/print-settings.md` §SumatraPDF table).
//! The `-print-settings` value is a comma-separated keyword list; SumatraPDF is
//! order-independent, but we emit a fixed order for a stable, testable preview.
//!
//! | Setting               | Token(s)                                   |
//! |-----------------------|--------------------------------------------|
//! | page ranges           | bare `1-5,10` (prepended, no key)          |
//! | duplex none/long/short| `simplex` / `duplex` / `duplexshort`       |
//! | colour / monochrome   | `color` / `monochrome`                     |
//! | scaling fit/shrink/none| `fit` / `shrink` / `noscale`              |
//! | paper                 | `paperkind=<DMPAPER id>`                   |
//! | tray                  | `bin=<DMBIN id>`                           |
//! | copies > 1            | `<n>x` (e.g. `3x`)                         |
//! | orientation land/port | `landscape` / `portrait` *(auto omitted)*  |
//! | **quality**           | **no token — a temporary per-user default, see below** |
//!
//! The doc table lists only `duplex`/`duplexshort`; SumatraPDF also accepts
//! `simplex` and `noscale`, which we use for the "none" cases so every setting
//! is stated explicitly. Copies use `<n>x`, the only spelling SumatraPDF's
//! parser accepts; an unknown token such as `copies=N` is skipped without a
//! warning, which is how an earlier version printed one copy of everything.
//!
//! ## Paper and tray go by id
//!
//! SumatraPDF's `paper=<name>` and `bin=<name>` look the name up in the
//! **driver's** list (`DC_PAPERNAMES`, `DC_BINNAMES`), which is localized: on
//! Spanish Windows Letter is "Carta", so `paper=Letter` matched nothing and
//! the job silently printed on the size of the document's first page. Both
//! also accept the numeric id, and that is all we send: `paperkind=<id>` with
//! the `DMPAPER_*` id (the driver's own id for a driver paper name, the
//! standard one for a size Pluryprint names), `bin=<id>` with the driver's
//! `DMBIN_*` id for the bin name. A tray name with a comma in it now works
//! too, since the name never reaches the comma-separated list. A paper or
//! tray the driver does not list cannot be sent, and [`unsupported_notes`]
//! says so.
//!
//! ## Quality
//!
//! SumatraPDF has no print-quality switch, but it starts every job from the
//! printer's per-user default DEVMODE. For Draft and High,
//! [`super::devmode_swap`] makes a DEVMODE with that quality the user's
//! default only while SumatraPDF hands the job over, then restores it and
//! reads the job back to log what it really got. Normal is never forced: it is
//! what the driver does anyway. [`quality_note`] tells the user which of
//! those will happen before printing.
//!
//! ## Colour
//!
//! `color` only sets the public `dmColor`, which a PrintTicket driver may
//! not read as colour: the Microsoft IPP Class Driver spooled such a job as
//! `psk:PageOutputColor = psk:AutoSelect` (the printer got
//! `print-color-mode=auto`). So a colour job also goes through the swap,
//! selecting `psk:Color`, when the driver offers it and the user's default
//! does not already select it ([`super::devmode_swap::needed`]).
//! `monochrome` needs nothing: `dmColor = DMCOLOR_MONOCHROME` reaches the
//! same driver as `psk:Grayscale`.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use super::devmode_swap::QualitySupport;
use crate::engine::{deps, EngineError, Failure, PdfProtection};
use crate::queue::job::{ColorMode, Duplex, JobSettings, Orientation, Quality, Scaling};

#[cfg(windows)]
use std::process::Command;

/// The driver ids that stand for a job's paper and tray on one printer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DriverIds {
    /// `DMPAPER_*` for `paperkind=`, or `None` (auto, or not resolvable).
    pub paper: Option<u16>,
    /// `DMBIN_*` for `bin=`, or `None` (no tray, or not in the driver's list).
    pub bin: Option<u16>,
}

impl DriverIds {
    /// Resolve `settings`' paper and tray to ids. `paper_by_name` and
    /// `bin_by_name` ask the driver (its localized names → ids); a paper the
    /// driver does not name falls back to the standard id of a size
    /// Pluryprint names itself ([`super::windows::dmpaper_for_size`]).
    pub fn resolve(
        settings: &JobSettings,
        paper_by_name: impl Fn(&str) -> Option<u16>,
        bin_by_name: impl Fn(&str) -> Option<u16>,
    ) -> Self {
        let paper = if settings.paper_size.is_auto() {
            None
        } else {
            paper_by_name(settings.paper_size.as_str())
                .or_else(|| super::windows::dmpaper_for_size(&settings.paper_size))
        };
        Self {
            paper,
            bin: settings.selected_tray().and_then(bin_by_name),
        }
    }

    /// [`DriverIds::resolve`] with no driver to ask: standard paper ids only.
    pub fn without_driver(settings: &JobSettings) -> Self {
        Self::resolve(settings, |_| None, |_| None)
    }
}

/// Build SumatraPDF's `-print-settings` value from `settings` and the ids
/// they resolved to on the target printer.
pub fn print_settings_string(settings: &JobSettings, ids: &DriverIds) -> String {
    let mut parts: Vec<String> = Vec::new();

    // Page ranges first, as bare comma-separated tokens (SumatraPDF takes them
    // with no key). Whitespace-stripped so the join stays clean.
    if let Some(ranges) = &settings.page_ranges {
        let compact = strip_ws(ranges);
        if !compact.is_empty() {
            parts.push(compact);
        }
    }

    parts.push(
        match settings.duplex {
            Duplex::None => "simplex",
            Duplex::Long => "duplex",
            Duplex::Short => "duplexshort",
        }
        .to_string(),
    );

    parts.push(
        match settings.color_mode {
            ColorMode::Color => "color",
            ColorMode::Monochrome => "monochrome",
        }
        .to_string(),
    );

    parts.push(
        match settings.scaling {
            Scaling::Fit => "fit",
            Scaling::Shrink => "shrink",
            Scaling::None => "noscale",
        }
        .to_string(),
    );

    // Paper and tray by id (see the module docs); nothing when there is none,
    // so the printer's own setting applies.
    if let Some(id) = ids.paper {
        parts.push(format!("paperkind={id}"));
    }
    if let Some(id) = ids.bin {
        parts.push(format!("bin={id}"));
    }

    // SumatraPDF spells copies `<n>x` (`3x`). It silently skips tokens it does
    // not know, so the `copies=N` spelling some docs use prints one copy.
    if settings.copies > 1 {
        parts.push(format!("{}x", settings.copies));
    }

    match settings.orientation {
        Orientation::Landscape => parts.push("landscape".to_string()),
        Orientation::Portrait => parts.push("portrait".to_string()),
        Orientation::Auto => {}
    }

    parts.join(",")
}

/// Full SumatraPDF argument vector:
/// `-print-to <printer> -print-settings <string> -silent -exit-when-done <pdf>`.
pub fn sumatra_args(
    printer: &str,
    settings: &JobSettings,
    ids: &DriverIds,
    pdf: &Path,
) -> Vec<String> {
    vec![
        "-print-to".to_string(),
        printer.to_string(),
        "-print-settings".to_string(),
        print_settings_string(settings, ids),
        "-silent".to_string(),
        "-exit-when-done".to_string(),
        pdf.to_string_lossy().into_owned(),
    ]
}

/// Fill in the per-printer default tray (Printers → *Defaults for this
/// printer*) when the job itself names none.
///
/// `docs/05-features/printers-management.md` says those defaults "seed any
/// profile that targets this printer without an explicit override". A job's
/// own tray always wins; a blank override is no override. Pure, so the rule is
/// tested on every host; [`resolve_for_printer`] feeds it the saved value.
pub fn apply_printer_tray<'a>(
    settings: &'a JobSettings,
    printer_tray: Option<&str>,
) -> Cow<'a, JobSettings> {
    let fallback = printer_tray.map(str::trim).filter(|t| !t.is_empty());
    match (settings.selected_tray(), fallback) {
        (None, Some(tray)) => Cow::Owned(JobSettings {
            tray: Some(tray.to_string()),
            ..settings.clone()
        }),
        _ => Cow::Borrowed(settings),
    }
}

/// The settings SumatraPDF is actually given for `printer`, and their ids.
pub struct Resolved<'a> {
    /// The job's settings plus the printer's saved default tray.
    pub settings: Cow<'a, JobSettings>,
    /// What the paper and tray resolve to on the printer's driver.
    pub ids: DriverIds,
}

/// The job's own settings plus the printer's saved default tray when the job
/// names none, resolved to the driver's ids (on Windows, through
/// `DeviceCapabilities`). Used by both [`spool`] and the honesty preview, so
/// the preview shows the same `paperkind=` and `bin=`.
pub fn resolve_for_printer<'a>(settings: &'a JobSettings, printer: &str) -> Resolved<'a> {
    let settings = if settings.selected_tray().is_some() {
        Cow::Borrowed(settings)
    } else {
        let prefs = crate::settings::Config::load_or_default().printers;
        let tray = prefs.overrides.get(printer).and_then(|o| o.tray.clone());
        apply_printer_tray(settings, tray.as_deref())
    };
    #[cfg(windows)]
    let ids = DriverIds::resolve(
        &settings,
        |name| crate::platform::driver_paper_id(printer, name),
        |name| crate::platform::driver_bin_id(printer, name),
    );
    #[cfg(not(windows))]
    let ids = {
        let _ = printer;
        DriverIds::without_driver(&settings)
    };
    Resolved { settings, ids }
}

/// The `printJob` catalogs the preview notes are written from, compiled in so
/// the backend renders them in the UI's language with no frontend round trip
/// (`printJob:preview.notes.<reason>`).
const EN_PRINT_JOB: &str = include_str!("../../../../src/i18n/locales/en/printJob.json");
const ES_PRINT_JOB: &str = include_str!("../../../../src/i18n/locales/es/printJob.json");

/// One thing the honesty preview says about a setting that will not reach the
/// printer as asked (or about how it is reached): a stable `reason`, its
/// parameters, and a sentence per language from the shared catalog
/// (`printJob:preview.notes.<reason>`, registered in
/// [`crate::util::reasons::PREVIEW_NOTES`]).
///
/// Rendered as a `#` comment so the preview (a PowerShell line) stays
/// copy-pasteable; the parameters (a driver's paper or tray name) cannot end
/// the comment ([`super::comment_text`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewNote {
    /// Stable sub-code, e.g. `quality_not_offered`.
    pub reason: &'static str,
    /// Interpolation values, by placeholder name. A `quality` value is a
    /// quality keyword (`draft`, `high`) translated at render time.
    pub params: Vec<(&'static str, String)>,
}

impl PreviewNote {
    fn new(reason: &'static str) -> Self {
        debug_assert!(
            crate::util::reasons::is_preview_note(reason),
            "unregistered preview note {reason}: add it to util::reasons and the catalogs"
        );
        Self {
            reason,
            params: Vec::new(),
        }
    }

    fn param(mut self, name: &'static str, value: impl ToString) -> Self {
        self.params.push((name, value.to_string()));
        self
    }

    /// The note as a `#` comment line in `locale` (`es-ES` → Spanish; any
    /// language without a catalog is English, as in the UI).
    pub fn render(&self, locale: &str) -> String {
        let catalog = preview_catalog(locale);
        let english = preview_catalog("en");
        let lookup = |path: &[&str]| -> Option<String> {
            [catalog, english].iter().find_map(|doc| {
                path.iter()
                    .try_fold(*doc, |v, k| v.get(*k))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            })
        };
        let mut sentence =
            lookup(&["preview", "notes", self.reason]).unwrap_or_else(|| self.reason.to_string());
        for (name, value) in &self.params {
            let value = if *name == "quality" {
                lookup(&["preview", "qualityWords", value]).unwrap_or_else(|| value.clone())
            } else {
                value.clone()
            };
            sentence = sentence.replace(&format!("{{{{{name}}}}}"), &value);
        }
        let prefix = lookup(&["preview", "notePrefix"]).unwrap_or_else(|| "note".into());
        super::comment_text(&format!("# {prefix}: {sentence}"))
    }

    /// The note in English, for the job log and the CLI.
    pub fn english(&self) -> String {
        self.render("en")
    }
}

/// The parsed `printJob` catalog for `locale`.
fn preview_catalog(locale: &str) -> &'static serde_json::Value {
    use std::sync::OnceLock;
    static EN: OnceLock<serde_json::Value> = OnceLock::new();
    static ES: OnceLock<serde_json::Value> = OnceLock::new();
    let parse = |raw: &str| serde_json::from_str(raw).unwrap_or(serde_json::Value::Null);
    let lang = locale
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match lang.as_str() {
        "es" => ES.get_or_init(|| parse(ES_PRINT_JOB)),
        _ => EN.get_or_init(|| parse(EN_PRINT_JOB)),
    }
}

/// What the honesty preview says about the job's quality, or `None` for
/// Normal (the driver's own default, never forced).
pub fn quality_preview_note(
    settings: &JobSettings,
    support: QualitySupport,
) -> Option<PreviewNote> {
    let word = match settings.quality {
        Quality::Normal => return None,
        Quality::Draft => "draft",
        Quality::High => "high",
    };
    let reason = match support {
        QualitySupport::Offered => "quality_offered",
        QualitySupport::Unknown => "quality_unknown",
        QualitySupport::NotOffered => "quality_not_offered",
        QualitySupport::PendingRestore => "quality_pending_restore",
    };
    Some(PreviewNote::new(reason).param("quality", word))
}

/// [`quality_preview_note`] in English: the sentence the job log carries.
pub fn quality_note(settings: &JobSettings, support: QualitySupport) -> Option<String> {
    quality_preview_note(settings, support).map(|n| n.english())
}

/// Every setting that will not reach the printer as asked, as notes for the
/// honesty preview ([`PreviewNote`]): a quality the driver does not offer (or
/// how it is applied), collation, and a paper or tray the driver has no id
/// for.
///
/// `quality` is what the driver said about the job's quality; `None` leaves
/// quality out (the spool path logs what really happened instead).
pub fn preview_notes(
    settings: &JobSettings,
    ids: &DriverIds,
    quality: Option<QualitySupport>,
) -> Vec<PreviewNote> {
    let mut notes: Vec<PreviewNote> = quality
        .and_then(|support| quality_preview_note(settings, support))
        .into_iter()
        .collect();
    if settings.copies > 1 {
        let reason = if settings.collate {
            "collated_not_applied"
        } else {
            "uncollated_not_applied"
        };
        notes.push(PreviewNote::new(reason).param("copies", settings.copies));
    }
    if !settings.paper_size.is_auto() && ids.paper.is_none() {
        notes.push(PreviewNote::new("paper_not_listed").param("paper", &settings.paper_size));
    }
    if let (Some(tray), None) = (settings.selected_tray(), ids.bin) {
        notes.push(PreviewNote::new("tray_not_listed").param("tray", tray));
    }
    notes
}

/// [`preview_notes`] in English, as `#` comment lines for the job log.
pub fn unsupported_notes(
    settings: &JobSettings,
    ids: &DriverIds,
    quality: Option<QualitySupport>,
) -> Vec<String> {
    preview_notes(settings, ids, quality)
        .iter()
        .map(PreviewNote::english)
        .collect()
}
/// What the driver of `printer` says about `settings`' quality, for the
/// preview. Normal needs no answer; off Windows nobody can be asked.
pub fn quality_support_for(printer: &str, settings: &JobSettings) -> QualitySupport {
    let Some(target) = super::devmode_swap::Target::for_quality(settings.quality) else {
        return QualitySupport::Offered;
    };
    #[cfg(windows)]
    {
        // Not cached: a restore that finally succeeds must show at once.
        let dir = super::devmode_swap::journal_dir();
        if super::devmode_swap::pending_restore(&dir, printer) {
            return QualitySupport::PendingRestore;
        }
        cached_quality_support(printer, target)
    }
    #[cfg(not(windows))]
    {
        let _ = (printer, target);
        QualitySupport::Unknown
    }
}

/// The preview is rebuilt on every settings change; asking the driver's
/// PrintTicket provider each time would be wasted work, so answers are kept a
/// minute.
#[cfg(windows)]
fn cached_quality_support(printer: &str, target: super::devmode_swap::Target) -> QualitySupport {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};
    type Cache = Mutex<HashMap<(String, &'static str), (Instant, QualitySupport)>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = (printer.to_lowercase(), target.label());
    if let Some((at, s)) = cache.lock().ok().and_then(|c| c.get(&key).copied()) {
        if at.elapsed() < Duration::from_secs(60) {
            return s;
        }
    }
    let s = super::devmode_swap::quality_support(printer, target);
    if let Ok(mut c) = cache.lock() {
        c.insert(key, (Instant::now(), s));
    }
    s
}

/// What SumatraPDF logs when it refuses a document whose permissions forbid
/// printing (`PrintFile2` in its 3.5.2 and 3.6.1 sources: `AllowsPrinting()`
/// is false, so the engine is dropped). It then exits 1 like any other
/// failure — its exit code is the number of files that failed, nothing more —
/// so the log is the only thing that tells this case apart.
///
/// The official 3.5.2 build does **not** refuse such a file: on a paused
/// Windows 11 queue it printed one with exit 0. The engine's own permission
/// check (`engine::convert`, FR-059) is therefore what keeps them from the
/// printer; this only maps a build that does refuse.
const NO_PRINT_MARKER: &str = "PrintFile2: engine is null";

/// The error for a SumatraPDF run that exited with `code` after writing
/// `log`. Pure.
///
/// A PDF that forbids printing is [`PdfProtection::PrintingNotAllowed`]:
/// terminal, never retried (FR-059). The engine's own check refuses such a
/// file before it gets here; this maps a SumatraPDF refusal of one that check
/// could not read (see [`NO_PRINT_MARKER`]).
pub fn failure_from_exit(code: Option<i32>, log: &str) -> EngineError {
    if log.contains(NO_PRINT_MARKER) {
        return EngineError::PdfProtected(PdfProtection::PrintingNotAllowed);
    }
    let status = code.map_or_else(|| "?".to_string(), |c| c.to_string());
    let tail = log.trim();
    let detail = if tail.is_empty() {
        format!("SumatraPDF exited with code {status}")
    } else {
        format!("SumatraPDF exited with code {status}: {tail}")
    };
    EngineError::PrintFailed(Failure::new("sumatra_exit", detail).param("status", status))
}

fn strip_ws(s: &str) -> String {
    s.split_whitespace().collect()
}

/// Locate `SumatraPDF.exe` through the shared dependency resolver
/// ([`crate::engine::deps`]): a bundled copy next to the app first, then a
/// configured path, then `PATH`, then the usual install directories.
///
/// The failure carries the same remediation text the UI shows for a missing
/// dependency ([`deps::Spec::remediation`]), so the queue's job error and the
/// Print Job banner say the same thing.
///
/// On non-Windows platforms SumatraPDF is not used at all (CUPS handles
/// printing), and the resolver has no executable to look for there, so this is
/// always a `DependencyMissing` — see the `applicable` flag on the report.
pub fn locate() -> Result<PathBuf, EngineError> {
    let os = deps::Os::current();
    let spec = deps::spec(deps::Dependency::SumatraPdf, os);

    if let deps::Resolution::Found { path, .. } =
        deps::resolve_on_host(deps::Dependency::SumatraPdf, os, None)
    {
        return Ok(path);
    }

    Err(EngineError::DependencyMissing(if spec.applicable() {
        spec.remediation().into()
    } else {
        "SumatraPDF is only used on Windows".to_string().into()
    }))
}

/// The file name a job is spooled under, from its `title` (the original
/// file's name). Pure.
///
/// SumatraPDF has no job-title flag and names the spooler document after the
/// file it prints, so the PDF is handed to it under this name
/// ([`stage_for_spool`]): the Windows print queue then shows `report.pdf` for
/// `report.docx` instead of a cache UUID. It is the CUPS backend's name
/// ([`super::cups::document_file_name`], so both platforms title a job the
/// same way) with the characters Windows forbids in a file name replaced by
/// `_` and no trailing dot or space before the extension.
pub fn spool_file_name(title: &str) -> String {
    // Replaced first: on Windows `a:b.png` parses as drive `a:` plus `b.png`,
    // and the name would lose its first part.
    let safe: String = title
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = super::cups::document_file_name(&safe);
    let stem = name.strip_suffix(".pdf").unwrap_or(&name);
    let clean = stem.trim_end_matches(['.', ' ']);
    if clean.is_empty() {
        "document.pdf".to_string()
    } else {
        format!("{clean}.pdf")
    }
}

/// A private folder holding `pdf` under [`spool_file_name`]`(title)`: a hard
/// link when the volume allows it, a copy otherwise. The folder (and the
/// link) disappears when the returned guard drops, after SumatraPDF has
/// handed the job to the spooler, which keeps its own copy of the data.
pub fn stage_for_spool(pdf: &Path, title: &str) -> std::io::Result<(tempfile::TempDir, PathBuf)> {
    let dir = tempfile::Builder::new()
        .prefix("pluryprint-spool-")
        .tempdir()?;
    let staged = dir.path().join(spool_file_name(title));
    if std::fs::hard_link(pdf, &staged).is_err() {
        std::fs::copy(pdf, &staged)?;
    }
    Ok((dir, staged))
}

/// At most this many bytes of SumatraPDF's output are quoted in an error.
#[cfg_attr(not(windows), allow(dead_code))]
const LOG_TAIL: usize = 600;

/// SumatraPDF's own output: the whole of it (to look for
/// [`NO_PRINT_MARKER`]) and its last [`LOG_TAIL`] bytes, for an error message.
#[cfg_attr(not(windows), allow(dead_code))]
fn read_log(log: &Path) -> (String, String) {
    let Ok(bytes) = std::fs::read(log) else {
        return (String::new(), String::new());
    };
    let start = bytes.len().saturating_sub(LOG_TAIL);
    (
        String::from_utf8_lossy(&bytes).into_owned(),
        String::from_utf8_lossy(&bytes[start..]).trim().to_string(),
    )
}

/// Spool `pdf` via SumatraPDF, hiding the child's console window. SumatraPDF
/// reports no OS job id itself, so the spooler's job list is compared before
/// and after ([`super::windows::find_spooled_job`]); `os_job_id` is `None` when
/// either list could not be read, or no new job of this user carries the
/// spooled file's name (it already left, or it cannot be told apart from
/// another).
///
/// SumatraPDF has no job-title flag, so the job is titled by printing the PDF
/// under `title`'s name ([`stage_for_spool`]); it names the job after that
/// file's full path, so once the job is in the queue its document is cut down
/// to the file name ([`super::windows::retitle_job`]). Its diagnostic output (it logs
/// its whole start-up to an inherited stdout) goes to a file next to that
/// copy, never to our own stdout, where it would corrupt `print --json`; the
/// tail of it is quoted when it fails.
///
/// The wait is bounded by `timeout` and the child killed when it expires (see
/// [`super::wait_with_timeout`]): SumatraPDF blocks until the spooler accepts
/// the job, so a driver dialog or a stalled spooler would otherwise hold the
/// queue's single print slot forever.
///
/// A Draft or High quality goes through [`super::devmode_swap`]: the user's
/// default for the printer carries it only until the job shows up in the
/// queue, and the job is then read back. Anything not sent as asked is logged
/// against the job ([`unsupported_notes`]), as is what the quality check
/// found.
///
/// The printer's saved default tray is applied when the job names none
/// ([`resolve_for_printer`]).
#[cfg(windows)]
pub fn spool(
    pdf: &Path,
    printer: &str,
    settings: &JobSettings,
    title: &str,
    timeout: Option<std::time::Duration>,
) -> Result<super::SpoolOutcome, super::PrintError> {
    let resolved = resolve_for_printer(settings, printer);
    spool_with(
        pdf,
        printer,
        &resolved.settings,
        &resolved.ids,
        title,
        timeout,
    )
}

/// [`spool`] with the settings and their driver ids taken as final.
#[cfg(windows)]
pub fn spool_with(
    pdf: &Path,
    printer: &str,
    settings: &JobSettings,
    ids: &DriverIds,
    title: &str,
    timeout: Option<std::time::Duration>,
) -> Result<super::SpoolOutcome, super::PrintError> {
    use std::os::windows::process::CommandExt;
    use std::time::{Duration, Instant};

    use super::devmode_swap::{self as swap, QualityOutcome, Request, SwapError};

    // Suppress the transient console window when spawning the child.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // How often the queue is looked at while the user's default is swapped.
    const SWAP_POLL: Duration = Duration::from_millis(50);

    let request = Request::for_job(settings.quality, settings.color_mode);
    let target = request.quality;
    let exe = locate()?;
    let (stage, staged) = stage_for_spool(pdf, title).map_err(|e| {
        EngineError::PrintFailed(format!("cannot prepare the file for the print queue: {e}").into())
    })?;
    let log_path = stage.path().join("sumatrapdf.log");
    let (log_out, log_err) = std::fs::File::create(&log_path)
        .and_then(|f| Ok((f.try_clone()?, f)))
        .map_err(|e| {
            EngineError::PrintFailed(format!("cannot create the print log: {e}").into())
        })?;
    let args = sumatra_args(printer, settings, ids, &staged);
    // SumatraPDF reports no job id, so take the spooler's job list before and
    // after: the new job carrying this file's path is the one to track. If the
    // "before" list cannot be read there is nothing to compare against, so the
    // job is not tracked at all rather than guessed.
    //
    // The printer's notifications are listened to first: Windows deletes a
    // printed job about a millisecond after marking it printed, so only they
    // can tell a print from a deletion once the job has left the queue.
    let seen_mark = super::windows::watch_printer(printer);
    let before = match super::windows::job_ids(printer) {
        Ok(ids) => Some(ids),
        Err(e) => {
            tracing::warn!(
                event = "os_job_untracked",
                printer = %printer,
                reason = "the job list could not be read before spooling",
                error = %e,
            );
            None
        }
    };

    // The quality and colour swap, made last so the user's default is
    // changed for as short a time as possible. A swap that cannot be made is
    // not a failure: the job prints at the driver's default and says so.
    let mut quality_swap = None;
    if !request.is_empty() {
        match swap::Swap::begin(
            swap::SpoolerDefaults,
            &swap::journal_dir(),
            printer,
            request,
            swap::LOCK_TIMEOUT,
            || swap::build_job_devmode(printer, request),
        ) {
            Ok(s) => quality_swap = Some(s),
            // Colour alone, and the driver needs no change for it (it
            // already selects colour, or reads SumatraPDF's `dmColor`).
            Err(SwapError::NotOffered(_)) if target.is_none() => {}
            Err(e) => {
                tracing::warn!(
                    event = "print_setting_not_applied",
                    setting = if target.is_some() { "quality" } else { "color" },
                    backend = "sumatra",
                    printer = %printer,
                    selects = request.label(),
                    message = %e,
                );
            }
        }
    }
    let swapped = quality_swap.is_some();
    for note in unsupported_notes(settings, ids, None) {
        tracing::warn!(
            event = "print_setting_not_applied",
            backend = "sumatra",
            printer = %printer,
            message = %note,
        );
    }

    let document = staged.to_string_lossy().into_owned();
    let mut child = match Command::new(exe)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(log_out))
        .stderr(std::process::Stdio::from(log_err))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            // `quality_swap` restores on drop.
            return Err(EngineError::PrintFailed(Failure::new(
                "sumatra_start_failed",
                format!("failed to run SumatraPDF: {e}"),
            ))
            .into());
        }
    };

    // While the default is swapped: restore it the moment the job is in the
    // queue (SumatraPDF has its own copy of the settings by then), or when
    // SumatraPDF exits, or at the deadline.
    let started = Instant::now();
    let mut seen_job = None;
    if let Some(s) = quality_swap.as_mut() {
        loop {
            if let Some(before) = &before {
                seen_job = super::windows::new_job_of(printer, before, seen_mark, &document);
            }
            let exited = matches!(child.try_wait(), Ok(Some(_)) | Err(_));
            let late = timeout.is_some_and(|t| started.elapsed() >= t);
            if seen_job.is_some() || exited || late {
                break;
            }
            std::thread::sleep(SWAP_POLL);
        }
        if let Err(e) = s.restore() {
            tracing::error!(event = "devmode_swap_restore_failed", printer = %printer, error = %e);
        }
    }
    // Releases the printer's lock: the next job's swap need not wait for
    // this SumatraPDF to finish rendering.
    drop(quality_swap);
    // The queue shows the file's name, not the temporary folder SumatraPDF
    // printed it from; done as soon as the job is known, before it prints.
    if let Some(job) = &seen_job {
        super::windows::retitle_job(printer, job);
    }

    let remaining = timeout.map(|t| t.saturating_sub(started.elapsed()));
    let status = super::wait_with_timeout(&mut child, remaining).map_err(|e| match e {
        super::PrintError::TimedOut { .. } => super::PrintError::TimedOut {
            seconds: timeout.map_or(0, |t| t.as_secs()),
        },
        other => other,
    })?;

    if !status.success() {
        let (log, tail) = read_log(&log_path);
        return Err(match failure_from_exit(status.code(), &log) {
            EngineError::PrintFailed(_) => failure_from_exit(status.code(), &tail),
            refused => refused,
        }
        .into());
    }

    let retitled = seen_job.is_some();
    let os_job_id = seen_job.or_else(|| {
        before.as_deref().and_then(|before| {
            super::windows::find_spooled_job(printer, before, seen_mark, &document)
        })
    });
    if let (Some(job), false) = (&os_job_id, retitled) {
        super::windows::retitle_job(printer, job);
    }

    if let (Some(target), true) = (target, swapped) {
        let outcome = match &os_job_id {
            Some(job) => swap::job_quality(printer, job.id, target),
            None => QualityOutcome::Unverified("the job could not be found in the queue".into()),
        };
        match outcome {
            QualityOutcome::Applied => tracing::info!(
                event = "print_setting_applied",
                setting = "quality",
                backend = "sumatra",
                printer = %printer,
                quality = target.label(),
            ),
            QualityOutcome::NotApplied(why) => tracing::warn!(
                event = "print_setting_not_applied",
                setting = "quality",
                backend = "sumatra",
                printer = %printer,
                quality = target.label(),
                message = %why,
            ),
            QualityOutcome::Unverified(why) => tracing::info!(
                event = "print_setting_unverified",
                setting = "quality",
                backend = "sumatra",
                printer = %printer,
                quality = target.label(),
                message = %why,
            ),
        }
    }

    if request.color && swapped {
        let found = os_job_id
            .as_ref()
            .map(|job| swap::job_color(printer, job.id));
        match found {
            Some(Some(c)) if c == swap::COLOR_KEYWORD => tracing::info!(
                event = "print_setting_applied",
                setting = "color",
                backend = "sumatra",
                printer = %printer,
            ),
            Some(Some(other)) => tracing::warn!(
                event = "print_setting_not_applied",
                setting = "color",
                backend = "sumatra",
                printer = %printer,
                message = %format!("the job's settings carry \"{other}\" instead of \"Color\""),
            ),
            _ => tracing::info!(
                event = "print_setting_unverified",
                setting = "color",
                backend = "sumatra",
                printer = %printer,
            ),
        }
    }

    Ok(super::SpoolOutcome {
        os_job_id: os_job_id.map(|id| id.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::job::PaperSize;
    use std::path::Path;

    fn strs(v: &[String]) -> Vec<&str> {
        v.iter().map(String::as_str).collect()
    }

    /// The settings string with no driver to ask (standard paper ids only).
    fn plain(settings: &JobSettings) -> String {
        print_settings_string(settings, &DriverIds::without_driver(settings))
    }

    /// The Windows queue shows the user's file name, as CUPS does, never the
    /// cache path or a UUID, and the name is always a legal Windows file name.
    #[test]
    fn fr_035_the_spooled_document_is_named_after_the_users_file() {
        assert_eq!(spool_file_name("report.pdf"), "report.pdf");
        assert_eq!(spool_file_name("letter.docx"), "letter.pdf");
        assert_eq!(spool_file_name("Factura 03.xlsx"), "Factura 03.pdf");
        assert_eq!(spool_file_name("a:b*c?.png"), "a_b_c_.pdf");
        assert_eq!(spool_file_name("  "), "document.pdf");
        assert_eq!(spool_file_name("dots...txt"), "dots.pdf");
        let long = "x".repeat(300) + ".docx";
        let name = spool_file_name(&long);
        assert!(name.len() <= 210 && name.ends_with(".pdf"), "{name}");
    }

    #[test]
    fn fr_035_staging_links_the_pdf_under_its_title_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("0192-uuid.pdf");
        std::fs::write(&pdf, b"%PDF-1.4\n%%EOF\n").unwrap();
        let (guard, staged) = stage_for_spool(&pdf, "Quarterly report.docx").unwrap();
        assert_eq!(staged.file_name().unwrap(), "Quarterly report.pdf");
        assert_eq!(
            std::fs::read(&staged).unwrap(),
            std::fs::read(&pdf).unwrap()
        );
        let folder = guard.path().to_path_buf();
        drop(guard);
        assert!(!folder.exists());
        assert!(pdf.exists(), "the source file is never touched (NFR-011)");
    }

    #[test]
    fn defaults_string_is_explicit() {
        assert_eq!(
            plain(&JobSettings::default()),
            "simplex,color,fit,paperkind=9"
        );
    }

    #[test]
    fn full_string_order_and_tokens() {
        let settings = JobSettings {
            printer: None,
            duplex: Duplex::Long,
            color_mode: ColorMode::Monochrome,
            copies: 2,
            paper_size: PaperSize::A3,
            orientation: Orientation::Landscape,
            scaling: Scaling::None,
            page_ranges: Some("1-5, 10".to_string()),
            quality: Quality::High,
            tray: Some("Drawer 1".to_string()),
            collate: true,
        };
        let ids = DriverIds {
            paper: Some(8),
            bin: Some(260),
        };
        assert_eq!(
            print_settings_string(&settings, &ids),
            "1-5,10,duplex,monochrome,noscale,paperkind=8,bin=260,2x,landscape"
        );
    }

    /// The driver's paper names are localized: "Carta" on Spanish Windows.
    /// SumatraPDF's `paper=` looks a name up in that list, so `paper=Letter`
    /// matched nothing there; the id is what reaches it now.
    #[test]
    fn print_settings_paper_localized_name_uses_paperkind() {
        let driver_paper = |name: &str| match name {
            "Carta" => Some(1u16),
            "Sobre n.º 10" => Some(20),
            "A4 210 x 297 mm" => Some(9),
            _ => None,
        };
        let job = |paper: &str| JobSettings {
            paper_size: PaperSize::parse(paper).unwrap(),
            ..JobSettings::default()
        };
        let ids = |s: &JobSettings| DriverIds::resolve(s, driver_paper, |_| None);

        // A driver name, whatever its language or decoration.
        for (paper, id) in [("Carta", 1), ("Sobre n.º 10", 20), ("A4 210 x 297 mm", 9)] {
            let s = job(paper);
            let string = print_settings_string(&s, &ids(&s));
            assert!(string.contains(&format!("paperkind={id}")), "{string}");
            assert!(!string.contains("paper="), "{string}");
            assert!(unsupported_notes(&s, &ids(&s), None).is_empty());
        }
        // A size Pluryprint names, on a driver that spells it otherwise.
        let letter = job("Letter");
        assert_eq!(ids(&letter).paper, Some(1));
        // Neither the driver nor Pluryprint knows it: not sent, and said.
        let photo = job("4x6");
        assert_eq!(ids(&photo).paper, None);
        assert!(!print_settings_string(&photo, &ids(&photo)).contains("paper"));
        let notes = unsupported_notes(&photo, &ids(&photo), None);
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("4x6") && notes[0].contains("NOT applied"));
        // `auto`: nothing sent, nothing to say, the driver is not asked.
        let auto = job("auto");
        let never = |_: &str| -> Option<u16> { panic!("must not be asked") };
        assert_eq!(
            DriverIds::resolve(&auto, never, never),
            DriverIds::default()
        );
        assert!(unsupported_notes(&auto, &DriverIds::default(), None).is_empty());
    }

    /// FR-041: the tray reaches SumatraPDF as the driver's bin id, so a
    /// localized or comma-carrying bin name works; a name the driver does not
    /// list is said, not sent.
    #[test]
    fn print_settings_paper_bin_numeric() {
        let bins = |name: &str| match name {
            "Bandeja 2" => Some(259u16),
            "Tray 2, Legal" => Some(262),
            _ => None,
        };
        let job = |tray: Option<&str>| JobSettings {
            tray: tray.map(String::from),
            ..JobSettings::default()
        };
        let ids = |s: &JobSettings| DriverIds::resolve(s, |_| None, bins);

        let s = job(Some("Bandeja 2"));
        assert_eq!(
            print_settings_string(&s, &ids(&s)),
            "simplex,color,fit,paperkind=9,bin=259"
        );
        let comma = job(Some("Tray 2, Legal"));
        assert!(print_settings_string(&comma, &ids(&comma)).ends_with(",bin=262"));
        assert!(unsupported_notes(&comma, &ids(&comma), None).is_empty());

        let unknown = job(Some("Drawer 9"));
        assert!(!print_settings_string(&unknown, &ids(&unknown)).contains("bin="));
        let notes = unsupported_notes(&unknown, &ids(&unknown), None);
        assert!(notes
            .iter()
            .any(|n| n.contains("Drawer 9") && n.contains("NOT applied")));

        // No tray, or a blank one: no token, no note, the driver is not asked.
        let never = |_: &str| -> Option<u16> { panic!("must not be asked") };
        for s in [job(None), job(Some(" "))] {
            assert_eq!(DriverIds::resolve(&s, |_| None, never).bin, None);
            assert!(unsupported_notes(&s, &DriverIds::without_driver(&s), None).is_empty());
        }
    }

    /// D16: Draft and High are carried by a temporary per-user default, and
    /// the preview says how — or that the driver has no such quality. They
    /// are never faked as a command-line token.
    #[test]
    fn print_settings_quality_note_says_what_really_happens() {
        let with = |quality| JobSettings {
            quality,
            ..JobSettings::default()
        };
        for quality in [Quality::Draft, Quality::Normal, Quality::High] {
            let s = plain(&with(quality));
            assert!(!s.contains("quality") && !s.contains("resolution"), "{s}");
        }
        for support in [
            QualitySupport::Offered,
            QualitySupport::NotOffered,
            QualitySupport::Unknown,
            QualitySupport::PendingRestore,
        ] {
            assert!(quality_note(&with(Quality::Normal), support).is_none());
        }
        let pending = quality_note(&with(Quality::Draft), QualitySupport::PendingRestore).unwrap();
        assert!(pending.contains("NOT applied") && pending.contains("\"draft\""));
        let offered = quality_note(&with(Quality::Draft), QualitySupport::Offered).unwrap();
        assert!(offered.starts_with("# ") && offered.contains("\"draft\" is applied"));
        assert!(!offered.contains("NOT applied"));
        let unknown = quality_note(&with(Quality::High), QualitySupport::Unknown).unwrap();
        assert!(unknown.contains("\"high\" is tried") && unknown.contains("checked"));
        let not = quality_note(&with(Quality::High), QualitySupport::NotOffered).unwrap();
        assert!(not.contains("NOT applied") && not.contains("\"high\""));

        // The preview carries it; the spool path logs the outcome instead.
        let s = with(Quality::Draft);
        let ids = DriverIds::without_driver(&s);
        assert_eq!(
            unsupported_notes(&s, &ids, Some(QualitySupport::Offered)).len(),
            1
        );
        assert!(unsupported_notes(&s, &ids, None).is_empty());
        assert_eq!(
            quality_support_for("P", &with(Quality::Normal)),
            QualitySupport::Offered
        );
    }

    /// FR-092 / D16: a Spanish UI gets the preview notes in Spanish, the job
    /// log keeps them in English, and a driver's tray name cannot end the
    /// comment line in either.
    #[test]
    fn fr_092_preview_notes_follow_the_ui_language() {
        let s = JobSettings {
            quality: Quality::High,
            copies: 2,
            collate: true,
            tray: Some("Bandeja\n2".into()),
            ..JobSettings::default()
        };
        let ids = DriverIds::without_driver(&s);
        let notes = preview_notes(&s, &ids, Some(QualitySupport::NotOffered));
        let reasons: Vec<&str> = notes.iter().map(|n| n.reason).collect();
        assert_eq!(
            reasons,
            [
                "quality_not_offered",
                "collated_not_applied",
                "tray_not_listed"
            ]
        );

        let es: Vec<String> = notes.iter().map(|n| n.render("es-ES")).collect();
        assert!(
            es[0].starts_with("# nota: ") && es[0].contains("«alta»"),
            "{}",
            es[0]
        );
        assert!(es[0].contains("NO se aplica"), "{}", es[0]);
        assert!(es[1].contains("2 copias"), "{}", es[1]);
        assert!(
            es[2].contains("«Bandeja?2»") && !es[2].contains('\n'),
            "{}",
            es[2]
        );
        assert!(es.iter().all(|n| !n.contains("{{")), "{es:?}");

        let en = unsupported_notes(&s, &ids, Some(QualitySupport::NotOffered));
        assert!(
            en[0].starts_with("# note: ") && en[0].contains("\"high\""),
            "{}",
            en[0]
        );
        assert_eq!(
            en,
            notes.iter().map(|n| n.render("fr-FR")).collect::<Vec<_>>()
        );
        assert_eq!(en[0], notes[0].english());
    }

    #[test]
    fn collation_is_declared_not_applied_for_several_copies() {
        let with = |copies, collate| JobSettings {
            copies,
            collate,
            ..JobSettings::default()
        };
        let notes = |s: &JobSettings| unsupported_notes(s, &DriverIds::without_driver(s), None);
        assert!(notes(&with(1, false)).is_empty());
        assert!(notes(&with(3, false))
            .iter()
            .any(|n| n.contains("uncollated") && n.contains("NOT applied")));
        assert!(!plain(&with(3, false)).contains("collate"));
    }

    /// FR-041: SumatraPDF's `-print-settings` only understands copies as
    /// `<n>x`; it skips an unknown `copies=N` token without a word, which
    /// printed exactly one copy whatever the user asked for.
    #[test]
    fn fr_041_copies_use_sumatras_nx_token() {
        let with = |copies| JobSettings {
            copies,
            ..JobSettings::default()
        };
        assert_eq!(plain(&with(3)), "simplex,color,fit,paperkind=9,3x");
        assert!(!plain(&with(3)).contains("copies"));
        // One copy is the default and is not stated at all.
        assert_eq!(plain(&with(1)), "simplex,color,fit,paperkind=9");
    }

    /// The printer's saved default tray fills in when the job names no tray,
    /// and never overrides a tray the job did name.
    #[test]
    fn fr_041_printer_default_tray_fills_in_only_when_the_job_has_none() {
        let none = JobSettings::default();
        let filled = apply_printer_tray(&none, Some("Manual Feed"));
        assert_eq!(filled.tray.as_deref(), Some("Manual Feed"));

        let own = JobSettings {
            tray: Some("Tray 2".into()),
            ..JobSettings::default()
        };
        assert_eq!(
            apply_printer_tray(&own, Some("Manual Feed"))
                .tray
                .as_deref(),
            Some("Tray 2")
        );
        // Blank and absent overrides change nothing.
        assert!(apply_printer_tray(&none, Some("  ")).tray.is_none());
        assert!(apply_printer_tray(&none, None).tray.is_none());
    }

    /// With no paper preference at all (`auto`) the token is left out, so the
    /// driver's own default paper is used instead of a forced A4.
    #[test]
    fn auto_paper_sends_no_paper_token() {
        let auto = JobSettings {
            paper_size: PaperSize::AUTO,
            ..JobSettings::default()
        };
        assert_eq!(plain(&auto), "simplex,color,fit");
    }

    #[test]
    fn short_edge_portrait_shrink() {
        let settings = JobSettings {
            duplex: Duplex::Short,
            orientation: Orientation::Portrait,
            scaling: Scaling::Shrink,
            paper_size: PaperSize::LETTER,
            ..JobSettings::default()
        };
        assert_eq!(
            plain(&settings),
            "duplexshort,color,shrink,paperkind=1,portrait"
        );
    }

    #[test]
    fn args_wrap_the_settings_string() {
        let settings = JobSettings::default();
        let args = sumatra_args(
            "HP LaserJet",
            &settings,
            &DriverIds::without_driver(&settings),
            Path::new("C:/x.pdf"),
        );
        assert_eq!(
            strs(&args),
            vec![
                "-print-to",
                "HP LaserJet",
                "-print-settings",
                "simplex,color,fit,paperkind=9",
                "-silent",
                "-exit-when-done",
                "C:/x.pdf",
            ]
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn locate_is_dependency_missing_off_windows() {
        assert!(matches!(locate(), Err(EngineError::DependencyMissing(_))));
    }

    /// FR-059: SumatraPDF exits 1 for any failed file (its exit code is the
    /// number of files that failed), so a PDF whose permissions forbid
    /// printing is recognized by what it logs, and becomes the terminal
    /// `printing_not_allowed` the dispatcher never retries.
    #[test]
    fn fr_059_sumatra_no_print_refusal_is_terminal() {
        let log =
            "PrintFile: file: 'C:\\t\\locked.pdf', printer: 'Q'\nPrintFile2: engine is null\n";
        let e = failure_from_exit(Some(1), log);
        assert!(matches!(
            e,
            EngineError::PdfProtected(PdfProtection::PrintingNotAllowed)
        ));
        assert_eq!(crate::util::reasons::job_code_of(&e), "pdf_protected");

        // Anything else is an ordinary, retryable print failure with its code.
        let other = failure_from_exit(Some(1), "PrintToDevice: failed\n");
        let EngineError::PrintFailed(f) = &other else {
            panic!("{other:?}")
        };
        assert_eq!(f.reason, Some("sumatra_exit"));
        assert_eq!(f.params.get("status").map(String::as_str), Some("1"));
        assert!(f.detail.contains("PrintToDevice"));
        let killed = failure_from_exit(None, "");
        assert!(matches!(&killed, EngineError::PrintFailed(f) if f.params["status"] == "?"));
    }
}
