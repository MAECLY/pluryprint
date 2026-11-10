//! CUPS backend: `lp` argument construction (pure, all platforms) and, on Unix,
//! spooling and cancellation.
//!
//! Flag mapping (`docs/03-requirements/print-settings.md` §CUPS table):
//!
//! | Setting              | `lp` flag                                             |
//! |----------------------|-------------------------------------------------------|
//! | printer              | `-d <name>`                                           |
//! | title                | `-t <title>`                                          |
//! | duplex none          | `-o sides=one-sided`                                  |
//! | duplex long          | `-o sides=two-sided-long-edge`                        |
//! | duplex short         | `-o sides=two-sided-short-edge`                       |
//! | colour               | `-o ColorModel=RGB` *only if the printer lists `RGB`* |
//! | monochrome           | `-o ColorModel=Gray`                                  |
//! | copies > 1           | `-n <copies> -o collate=true\|false`                  |
//! | paper                | `-o media=<the printer's keyword>` *(auto omitted)*   |
//! | tray                 | `-o InputSlot=<bin> -o media-source=<bin>`            |
//! | quality              | `-o print-quality=3\|4\|5 -o cupsPrintQuality=…`      |
//! | orientation          | `-o orientation-requested=3\|4` *(auto omitted)*      |
//! | scaling fit/shrink   | `-o fit-to-page` *(none omitted)*                     |
//! | page ranges          | `-o page-ranges=<r>`                                  |
//!
//! `sides` and `media` are always emitted so an explicit "none"/paper choice
//! wins over any driver default; `quality` is emitted for the same reason (a
//! user who picks "normal" must not inherit a driver default of "draft"). CUPS
//! has a single `fit-to-page` toggle and cannot distinguish "shrink only" from
//! "fit", so both map to it.
//!
//! **Why two spellings for tray and quality.** The same choice has a PPD name
//! and an IPP name, and which one a queue understands depends on how it was
//! created: PPD-backed queues take `InputSlot` / `cupsPrintQuality`, while
//! driverless (IPP Everywhere) queues take the standard job attributes
//! `media-source` / `print-quality`. Both carry the identical value, and CUPS
//! ignores a job option a queue does not support, so emitting the pair is what
//! makes the setting actually apply on both kinds of queue instead of applying
//! on one and silently vanishing on the other.

use std::path::Path;

use crate::queue::job::{ColorMode, Duplex, JobSettings, Orientation, Quality, Scaling};

use super::PrinterCaps;

#[cfg(unix)]
use anyhow::Context;

/// The option flags for a printer whose capabilities are not known — see
/// [`lp_opts_for`].
pub fn lp_opts(settings: &JobSettings) -> Vec<String> {
    lp_opts_for(settings, &PrinterCaps::unknown())
}

/// The option flags (everything after `-d <printer> -t <title>` and before the
/// PDF path) for a printer with capabilities `caps`. Shared by [`lp_args_for`]
/// and the UI preview so they never drift.
pub fn lp_opts_for(settings: &JobSettings, caps: &PrinterCaps) -> Vec<String> {
    let mut opts = Vec::new();

    // Sides — always explicit: a user's "none" must force single-sided rather
    // than inherit a duplex driver default.
    opts.push("-o".to_string());
    opts.push(format!(
        "sides={}",
        match settings.duplex {
            Duplex::None => "one-sided",
            Duplex::Long => "two-sided-long-edge",
            Duplex::Short => "two-sided-short-edge",
        }
    ));

    // Colour: grey is always forced for monochrome. Colour is asked for
    // explicitly only when the printer lists an `RGB` colour model — a queue
    // whose default is `Gray` would otherwise print a colour job in grey —
    // and sent in the printer's own spelling. A printer that does not list it
    // (driverless queues, Windows) keeps its default, which is colour on any
    // colour device.
    match settings.color_mode {
        ColorMode::Monochrome => {
            opts.push("-o".to_string());
            opts.push("ColorModel=Gray".to_string());
        }
        ColorMode::Color => {
            if let Some(rgb) = caps
                .color_models
                .iter()
                .find(|m| m.eq_ignore_ascii_case("RGB"))
            {
                opts.push("-o".to_string());
                opts.push(format!("ColorModel={rgb}"));
            }
        }
    }

    // Copies use `lp`'s own `-n` flag, and collation is stated explicitly
    // whenever it matters (more than one copy) so the driver default cannot
    // decide it.
    if settings.copies > 1 {
        opts.push("-n".to_string());
        opts.push(settings.copies.to_string());
        opts.push("-o".to_string());
        opts.push(format!("collate={}", settings.collate));
    }

    // Paper — explicit unless the job defers to the printer ("auto"), and in
    // the printer's own spelling when it lists the size.
    if !settings.paper_size.is_auto() {
        let keyword = caps
            .paper_keyword(&settings.paper_size)
            .unwrap_or(settings.paper_size.as_str());
        opts.push("-o".to_string());
        opts.push(format!("media={keyword}"));
    }

    // Tray — only when the job names one; no name means the printer's own
    // default bin, which is not ours to override.
    if let Some(tray) = settings.selected_tray() {
        opts.push("-o".to_string());
        opts.push(format!("InputSlot={tray}"));
        opts.push("-o".to_string());
        opts.push(format!("media-source={tray}"));
    }

    // Quality — always explicit, in both the PPD and the IPP spelling.
    opts.push("-o".to_string());
    opts.push(format!("print-quality={}", ipp_quality(settings.quality)));
    opts.push("-o".to_string());
    opts.push(format!(
        "cupsPrintQuality={}",
        ppd_quality(settings.quality)
    ));

    // Orientation — an explicit choice is sent explicitly, in the standard IPP
    // attribute (3 portrait, 4 landscape): a "portrait" that emitted nothing
    // would silently inherit a queue whose default is landscape. Only "auto"
    // says nothing, so each page keeps its own orientation.
    match settings.orientation {
        Orientation::Portrait => {
            opts.push("-o".to_string());
            opts.push("orientation-requested=3".to_string());
        }
        Orientation::Landscape => {
            opts.push("-o".to_string());
            opts.push("orientation-requested=4".to_string());
        }
        Orientation::Auto => {}
    }

    // Scaling — fit and shrink both collapse to CUPS's single fit-to-page.
    if matches!(settings.scaling, Scaling::Fit | Scaling::Shrink) {
        opts.push("-o".to_string());
        opts.push("fit-to-page".to_string());
    }

    // Page ranges — whitespace-stripped so the value is a single clean token.
    if let Some(ranges) = &settings.page_ranges {
        let compact = strip_ws(ranges);
        if !compact.is_empty() {
            opts.push("-o".to_string());
            opts.push(format!("page-ranges={compact}"));
        }
    }

    opts
}

/// A job-option value `lp` would not carry as the single value it looks like.
///
/// `lp -o` hands its argument to `cupsParseOptions`, which splits on whitespace
/// and honours quotes and backslashes, so `-o "InputSlot=Tray1 copies=999"` is
/// *two* options. Paper and tray names reach us from profiles (which can be
/// imported from a file someone else wrote) and per-printer overrides, so a
/// value like that would silently add job options the UI never shows. CUPS
/// keywords never contain these characters, so such a value is refused rather
/// than quoted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsafeOptionValue {
    /// The CUPS option name (`media`, `InputSlot`, ...).
    pub option: String,
    /// The offending value, verbatim.
    pub value: String,
}

impl UnsafeOptionValue {
    /// What the user knows the option as, for the error message.
    fn label(&self) -> &str {
        match self.option.as_str() {
            "media" => "paper size",
            "InputSlot" | "media-source" => "tray",
            "ColorModel" => "colour model",
            "page-ranges" => "page ranges",
            other => other,
        }
    }
}

impl std::fmt::Display for UnsafeOptionValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {:?} (a CUPS option value cannot contain spaces, quotes, '=', '\\' or \
             control characters)",
            self.label(),
            self.value
        )
    }
}

/// `true` when `value` is one non-empty CUPS option token: no whitespace, no
/// control characters, no quotes, no backslash and no `=`.
fn is_safe_option_value(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| !c.is_whitespace() && !c.is_control() && !matches!(c, '"' | '\'' | '\\' | '='))
}

/// Check one `name=value` option value; see [`UnsafeOptionValue`].
pub fn check_option_value(option: &str, value: &str) -> Result<(), UnsafeOptionValue> {
    if is_safe_option_value(value) {
        Ok(())
    } else {
        Err(UnsafeOptionValue {
            option: option.to_string(),
            value: value.to_string(),
        })
    }
}

/// Check every `-o name=value` the job would send ([`lp_opts_for`]), so a value
/// that `lp` would split into several options is refused before spooling.
pub fn check_option_values(
    settings: &JobSettings,
    caps: &PrinterCaps,
) -> Result<(), UnsafeOptionValue> {
    let opts = lp_opts_for(settings, caps);
    let mut it = opts.iter();
    while let Some(flag) = it.next() {
        if flag != "-o" {
            continue;
        }
        let Some(pair) = it.next() else { break };
        if let Some((name, value)) = pair.split_once('=') {
            check_option_value(name, value)?;
        }
    }
    Ok(())
}

/// Full `lp` argument vector for a printer whose capabilities are not known.
pub fn lp_args(printer: &str, pdf: &Path, settings: &JobSettings, title: &str) -> Vec<String> {
    lp_args_for(printer, pdf, settings, &PrinterCaps::unknown(), title)
}

/// Full `lp` argument vector: `-d <printer> -t <title>` + [`lp_opts_for`] + the
/// PDF path as the final argument.
pub fn lp_args_for(
    printer: &str,
    pdf: &Path,
    settings: &JobSettings,
    caps: &PrinterCaps,
    title: &str,
) -> Vec<String> {
    let mut args = vec![
        "-d".to_string(),
        printer.to_string(),
        "-t".to_string(),
        title.to_string(),
    ];
    args.extend(lp_opts_for(settings, caps));
    args.push(pdf.to_string_lossy().into_owned());
    args
}

/// Parse `lp`'s success line into the `<dest>-<num>` request id.
///
/// In English the line is `request id is <dest>-<num> (1 file(s))`, but
/// macOS localizes it from the user's language list whatever `LC_ALL` says
/// (`el id de la petición es Maecly_Hold-1537 (1 archivos)` on a Spanish
/// Mac), so the marker cannot be relied on. The id itself is not localized:
/// it is `<dest>-<digits>`, and `dest` is the queue we just spooled to. So the
/// English marker is tried first, then a token naming `printer`, then any
/// `<name>-<digits>` token. `<dest>` may itself contain `-`, which is why the
/// number is only ever the last segment.
pub fn parse_request_id(stdout: &str, printer: &str) -> Option<String> {
    const MARKER: &str = "request id is ";
    if let Some(start) = stdout.find(MARKER) {
        if let Some(tok) = stdout[start + MARKER.len()..].split_whitespace().next() {
            if is_request_id(tok) {
                return Some(tok.to_string());
            }
        }
    }
    let tokens: Vec<&str> = stdout
        .split_whitespace()
        .map(|t| t.trim_matches(|c: char| matches!(c, '(' | ')' | ',' | '.' | ':' | ';' | '"')))
        .collect();
    let named = format!("{printer}-");
    tokens
        .iter()
        .find(|t| t.starts_with(&named) && is_request_id(t))
        .or_else(|| tokens.iter().find(|t| is_request_id(t)))
        .map(|t| t.to_string())
}

/// `<non-empty name>-<digits>`, the shape of every CUPS request id.
fn is_request_id(token: &str) -> bool {
    match token.rsplit_once('-') {
        Some((dest, num)) => {
            !dest.is_empty() && !num.is_empty() && num.chars().all(|c| c.is_ascii_digit())
        }
        None => false,
    }
}

/// IPP `print-quality` enum value (RFC 8011: 3 draft, 4 normal, 5 high), as
/// understood by driverless queues.
fn ipp_quality(quality: Quality) -> u8 {
    match quality {
        Quality::Draft => 3,
        Quality::Normal => 4,
        Quality::High => 5,
    }
}

/// PPD `cupsPrintQuality` keyword, as understood by PPD-backed queues.
fn ppd_quality(quality: Quality) -> &'static str {
    match quality {
        Quality::Draft => "Draft",
        Quality::Normal => "Normal",
        Quality::High => "High",
    }
}

/// Remove *all* whitespace from a page-range string (`" 1-5, 8 "` → `"1-5,8"`),
/// keeping it a single shell token.
fn strip_ws(s: &str) -> String {
    s.split_whitespace().collect()
}

/// Spool `pdf` via `lp`. On success returns the CUPS job id (when the line
/// parses); a non-zero exit becomes [`EngineError::PrintFailed`] carrying the
/// trimmed stderr. Run in the C locale so CUPS's prose stays parseable.
///
/// The child is spawned rather than run through `Command::output()` so the wait
/// can be bounded by `timeout` and the process killed when it expires (see
/// [`super::wait_with_timeout`]) — a hung `lp` would otherwise hold the queue's
/// single print slot forever. `lp` writes a single short line to each pipe, so
/// draining them after the wait cannot deadlock on a full pipe buffer.
#[cfg(unix)]
pub fn spool(
    pdf: &Path,
    printer: &str,
    settings: &JobSettings,
    caps: &PrinterCaps,
    title: &str,
    timeout: Option<std::time::Duration>,
) -> Result<super::SpoolOutcome, super::PrintError> {
    use crate::engine::{EngineError, Failure};
    use std::io::Read;

    // Defence in depth: the dispatcher's capability gate already refuses
    // these, but nothing that would smuggle extra options may reach `lp`.
    check_option_values(settings, caps).map_err(|e| EngineError::UnsupportedSetting {
        setting: e.to_string(),
        printer: printer.to_string(),
    })?;
    // `lp` names the IPP document after the file it is given. A converted
    // job's file is the cache's `<uuid>.pdf`, which printers and the CUPS web
    // UI then show; a link named after the user's file fixes that. It only
    // has to live until `lp` has sent the file, which is before it returns.
    let staged = stage_named(pdf, title);
    let pdf = staged.as_ref().map_or(pdf, |(_, link)| link.as_path());
    let args = lp_args_for(printer, pdf, settings, caps, title);
    let mut child = crate::util::appimage::host_command("lp")
        .args(&args)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            EngineError::DependencyMissing(Failure::new(
                "cups",
                format!("failed to run `lp` (is CUPS installed?): {e}"),
            ))
        })?;

    let status = super::wait_with_timeout(&mut child, timeout)?;

    let read = |pipe: Option<&mut dyn Read>| {
        let mut buf = String::new();
        if let Some(p) = pipe {
            let _ = p.read_to_string(&mut buf);
        }
        buf
    };
    let stdout = read(child.stdout.as_mut().map(|p| p as &mut dyn Read));
    let stderr = read(child.stderr.as_mut().map(|p| p as &mut dyn Read));

    if status.success() {
        Ok(super::SpoolOutcome {
            os_job_id: parse_request_id(&stdout, printer),
        })
    } else {
        Err(EngineError::PrintFailed(lp_rejected(&status.to_string(), &stderr)).into())
    }
}

/// The failure `lp` exiting with `status` becomes: the translatable
/// `spooler_rejected`, with what `lp` said (or its status) as the technical
/// detail shown under it. Pure.
pub fn lp_rejected(status: &str, stderr: &str) -> crate::engine::Failure {
    let msg = stderr.trim();
    let detail = if msg.is_empty() {
        format!("lp exited with {status}")
    } else {
        msg.to_string()
    };
    crate::engine::Failure::new("spooler_rejected", detail)
}

/// The file name a spooled document should carry: the user's file name
/// (`title`) with a `.pdf` extension, since what is sent is a PDF and CUPS
/// may type a file by its name. Path separators and control characters
/// become `_`, and very long names are shortened. Pure.
pub fn document_file_name(title: &str) -> String {
    let stem = Path::new(title)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut clean: String = stem
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = clean.trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        clean = "document".to_string();
    } else {
        clean = trimmed.to_string();
    }
    // Leave room for ".pdf" under the usual 255-byte file-name limit.
    let mut end = clean.len().min(200);
    while !clean.is_char_boundary(end) {
        end -= 1;
    }
    clean.truncate(end);
    format!("{clean}.pdf")
}

/// A symlink to `pdf` named [`document_file_name`]`(title)` in a private
/// temporary folder (deleted with the returned guard), or `None` when `pdf`
/// already has that name or the link cannot be made (the original is then
/// spooled as before).
#[cfg(unix)]
fn stage_named(pdf: &Path, title: &str) -> Option<(tempfile::TempDir, std::path::PathBuf)> {
    let wanted = document_file_name(title);
    if pdf
        .file_name()
        .is_some_and(|n| n.to_string_lossy() == wanted)
    {
        return None;
    }
    let dir = tempfile::Builder::new()
        .prefix("pluryprint-spool-")
        .tempdir()
        .ok()?;
    let link = dir.path().join(&wanted);
    let target = std::fs::canonicalize(pdf).ok()?;
    std::os::unix::fs::symlink(&target, &link).ok()?;
    Some((dir, link))
}

/// The numeric CUPS job id inside an `lp` request id (`Office-42` → `42`).
/// The destination may itself contain `-`, so the number is the last segment.
pub fn job_number(os_job_id: &str) -> Option<u32> {
    os_job_id.rsplit('-').next()?.parse().ok()
}

/// Map an IPP `job-state` keyword (as `ipptool` prints it) onto
/// [`super::OsJobState`]. Unknown keywords are `None`, never guessed.
pub fn parse_ipp_job_state(keyword: &str) -> Option<super::OsJobState> {
    use super::OsJobState as S;
    Some(match keyword.trim() {
        "pending" => S::Pending,
        "pending-held" => S::Held,
        "processing" => S::Processing,
        "processing-stopped" => S::Stopped,
        "canceled" => S::Canceled,
        "aborted" => S::Aborted,
        "completed" => S::Completed,
        _ => return None,
    })
}

/// Read the job state out of `ipptool -tv … get-job-attributes.test` output:
/// a `job-state (enum) = <keyword>` line, or `client-error-not-found` when the
/// scheduler no longer knows the job.
pub fn parse_ipptool_output(out: &str) -> Option<super::OsJobState> {
    for line in out.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("job-state ") {
            if let Some((_, value)) = rest.split_once('=') {
                return parse_ipp_job_state(value);
            }
        }
    }
    if out.contains("client-error-not-found") {
        return Some(super::OsJobState::Gone);
    }
    None
}

/// Whether `lpstat -W … -o <dest>` output lists `os_job_id` (first column).
pub fn lpstat_lists(out: &str, os_job_id: &str) -> bool {
    out.lines()
        .filter_map(|l| l.split_whitespace().next())
        .any(|id| id == os_job_id)
}

/// What one `lpstat` run said: its stdout, or a [`super::TrackError`] when it
/// did not run cleanly. Pure.
///
/// `lpstat` prints "Unable to connect to server" on stderr, with an empty
/// stdout, when cupsd is restarting or unreachable. Reading that empty stdout
/// as "not listed" made a job look finished and it was marked done although
/// nothing confirmed it; a non-zero exit is a transient failure instead, so
/// the dispatcher's retry-then-unconfirmed policy applies.
pub fn lpstat_output(
    success: bool,
    stdout: &str,
    stderr: &str,
) -> Result<String, super::TrackError> {
    if success {
        Ok(stdout.to_string())
    } else {
        let why = stderr.trim();
        Err(super::TrackError::Transient(if why.is_empty() {
            "lpstat failed".to_string()
        } else {
            format!("lpstat failed: {why}")
        }))
    }
}

/// The verdict of the `lpstat -W not-completed` fallback for `os_job_id`.
/// Pure.
///
/// A job still listed is processing. One that is not has finished, but
/// `lpstat` cannot say how: CUPS' "completed" list holds printed, cancelled
/// and aborted jobs alike. So it is [`super::OsJobState::Gone`] (finished,
/// outcome unknown), never `Completed`: on a host without `ipptool`, a job
/// cancelled in the CUPS web UI must not be reported as printed.
pub fn lpstat_verdict(not_completed: &str, os_job_id: &str) -> super::OsJobState {
    if lpstat_lists(not_completed, os_job_id) {
        super::OsJobState::Processing
    } else {
        super::OsJobState::Gone
    }
}

/// The `ipptool` test Pluryprint runs to read a job's state: the same request
/// as CUPS's stock `get-job-attributes.test`, carried in the binary.
///
/// `ipptool` looks a test named by a bare file name up in the *current
/// directory first* and only then in CUPS's data folder, so passing
/// `get-job-attributes.test` let a file planted in whatever folder the CLI was
/// started from (a Downloads folder, say) run arbitrary IPP operations against
/// the local scheduler and fake the state the dispatcher trusts. The test is
/// therefore written to a private temporary file and named by its absolute
/// path, which `ipptool` never resolves against the working directory.
pub const GET_JOB_STATE_TEST: &str = r#"{
	NAME "Pluryprint get job state"
	OPERATION get-job-attributes
	GROUP operation
	ATTR charset attributes-charset utf-8
	ATTR language attributes-natural-language en
	ATTR uri job-uri $uri
	EXPECT job-uri
	EXPECT job-state
}
"#;

/// Write [`GET_JOB_STATE_TEST`] to a fresh private temporary file (created
/// exclusively, readable by this user only) whose path is absolute.
#[cfg(unix)]
fn job_state_test_file() -> std::io::Result<tempfile::NamedTempFile> {
    use std::io::Write as _;
    let mut file = tempfile::Builder::new()
        .prefix("pluryprint-ipp-")
        .suffix(".test")
        .tempfile()?;
    file.write_all(GET_JOB_STATE_TEST.as_bytes())?;
    file.flush()?;
    Ok(file)
}

/// `ipptool`'s argument vector for reading job `number`'s state with the test
/// at `test`. Pure. `test` must be absolute (see [`GET_JOB_STATE_TEST`]).
pub fn ipptool_args(number: u32, test: &Path) -> Vec<String> {
    debug_assert!(test.is_absolute(), "{}", test.display());
    vec![
        "-tv".to_string(),
        format!("ipp://localhost/jobs/{number}"),
        test.to_string_lossy().into_owned(),
    ]
}

/// Where the CUPS job `os_job_id` stands.
///
/// `ipptool get-job-attributes` is asked first because it is the only source
/// that tells a printed job from a cancelled or aborted one. Where `ipptool`
/// is missing (a minimal Linux install), `lpstat -W` still tells a waiting job
/// from a finished one, which is what keeps the print slot honest — but not
/// *how* it finished ([`lpstat_verdict`]).
#[cfg(unix)]
pub fn job_state(os_job_id: &str) -> Result<super::OsJobState, super::TrackError> {
    use super::TrackError;

    let Some(number) = job_number(os_job_id) else {
        return Err(TrackError::Unavailable(format!(
            "'{os_job_id}' is not a CUPS request id"
        )));
    };

    let test = job_state_test_file()
        .map_err(|e| TrackError::Transient(format!("cannot write the ipptool test: {e}")))?;
    // The test is named by its absolute path and the child runs in the temp
    // folder, so nothing in the caller's working directory is consulted.
    let workdir = test.path().parent().map(Path::to_path_buf);
    let mut ipptool = crate::util::appimage::host_command("ipptool");
    ipptool
        .args(ipptool_args(number, test.path()))
        .env("LC_ALL", "C")
        .env("LANG", "C");
    if let Some(dir) = &workdir {
        ipptool.current_dir(dir);
    }
    match ipptool.output() {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            if let Some(state) = parse_ipptool_output(&text) {
                return Ok(state);
            }
            // Ran but said nothing usable (no test file, scheduler busy): fall
            // through to lpstat rather than give up.
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(TrackError::Transient(format!("failed to run ipptool: {e}")));
        }
        Err(_) => {}
    }

    let dest = os_job_id
        .rsplit_once('-')
        .map(|(d, _)| d)
        .unwrap_or(os_job_id);
    let lpstat = |which: &str| -> Result<String, TrackError> {
        let out = crate::util::appimage::host_command("lpstat")
            .args(["-W", which, "-o", dest])
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    TrackError::Unavailable("neither ipptool nor lpstat is installed".to_string())
                } else {
                    TrackError::Transient(format!("failed to run lpstat: {e}"))
                }
            })?;
        lpstat_output(
            out.status.success(),
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
        )
    };
    Ok(lpstat_verdict(&lpstat("not-completed")?, os_job_id))
}

/// Best-effort cancellation of a CUPS job by its id (`cancel <id>`).
#[cfg(unix)]
pub fn cancel(os_job_id: &str) -> anyhow::Result<()> {
    let status = crate::util::appimage::host_command("cancel")
        .arg(os_job_id)
        .status()
        .with_context(|| format!("failed to run `cancel {os_job_id}`"))?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("`cancel {os_job_id}` exited with {status}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::job::PaperSize;
    use std::path::Path;

    fn strs(v: &[String]) -> Vec<&str> {
        v.iter().map(String::as_str).collect()
    }

    /// FR-092: a job `lp` refuses says so in the user's language
    /// (`spooler_rejected`); what `lp` printed stays the technical detail.
    #[test]
    fn fr_092_lp_rejection_carries_reason_and_keeps_stderr_as_detail() {
        let f = lp_rejected(
            "exit status: 1",
            "  lp: The printer or class does not exist.\n",
        );
        assert_eq!(f.reason, Some("spooler_rejected"));
        assert_eq!(f.detail, "lp: The printer or class does not exist.");
        let silent = lp_rejected("exit status: 1", "  ");
        assert_eq!(silent.reason, Some("spooler_rejected"));
        assert_eq!(silent.detail, "lp exited with exit status: 1");
    }

    /// A converted job is spooled under the user's file name, not the cache's
    /// UUID, so printers and the CUPS web UI show `report.pdf`, not
    /// `01a1…-0.pdf`.
    #[test]
    fn fr_035_the_spooled_document_is_named_after_the_users_file() {
        assert_eq!(document_file_name("report.docx"), "report.pdf");
        assert_eq!(document_file_name("Factura 03.xlsx"), "Factura 03.pdf");
        assert_eq!(document_file_name("notes"), "notes.pdf");
        assert_eq!(document_file_name("a\u{7}b.md"), "a_b.pdf");
        assert_eq!(document_file_name(""), "document.pdf");
        assert!(document_file_name(&"x".repeat(400)).len() <= 204);
    }

    #[cfg(unix)]
    #[test]
    fn fr_035_staging_links_the_cache_pdf_under_the_users_name() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("01a1-0.pdf");
        std::fs::write(&cache, b"%PDF-1.4").unwrap();

        let (guard, link) = stage_named(&cache, "report.docx").expect("a link is made");
        assert_eq!(link.file_name().unwrap(), "report.pdf");
        assert_eq!(std::fs::read(&link).unwrap(), b"%PDF-1.4");
        drop(guard);
        assert!(!link.exists(), "the link folder goes with the guard");
        assert!(cache.exists(), "the cache PDF itself is untouched");

        // A passthrough PDF already carries the user's name.
        let own = dir.path().join("report.pdf");
        std::fs::write(&own, b"%PDF-1.4").unwrap();
        assert!(stage_named(&own, "report.pdf").is_none());
    }

    #[test]
    fn defaults_produce_minimal_args() {
        let args = lp_args(
            "Office",
            Path::new("/tmp/x.pdf"),
            &JobSettings::default(),
            "Doc",
        );
        assert_eq!(
            strs(&args),
            vec![
                "-d",
                "Office",
                "-t",
                "Doc",
                "-o",
                "sides=one-sided",
                "-o",
                "media=A4",
                "-o",
                "print-quality=4",
                "-o",
                "cupsPrintQuality=Normal",
                "-o",
                "fit-to-page",
                "/tmp/x.pdf",
            ]
        );
    }

    #[test]
    fn every_setting_maps_in_stable_order() {
        let settings = JobSettings {
            printer: None,
            duplex: Duplex::Short,
            color_mode: ColorMode::Monochrome,
            copies: 3,
            paper_size: PaperSize::LEGAL,
            orientation: Orientation::Landscape,
            scaling: Scaling::Shrink,
            page_ranges: Some(" 1-5, 8 ".to_string()),
            quality: Quality::High,
            tray: Some("Tray 2".to_string()),
            collate: true,
        };
        assert_eq!(
            strs(&lp_opts(&settings)),
            vec![
                "-o",
                "sides=two-sided-short-edge",
                "-o",
                "ColorModel=Gray",
                "-n",
                "3",
                "-o",
                "collate=true",
                "-o",
                "media=Legal",
                "-o",
                "InputSlot=Tray 2",
                "-o",
                "media-source=Tray 2",
                "-o",
                "print-quality=5",
                "-o",
                "cupsPrintQuality=High",
                "-o",
                "orientation-requested=4",
                "-o",
                "fit-to-page",
                "-o",
                "page-ranges=1-5,8",
            ]
        );
    }

    #[test]
    fn long_edge_and_a3_and_no_scale() {
        let settings = JobSettings {
            duplex: Duplex::Long,
            paper_size: PaperSize::A3,
            scaling: Scaling::None,
            ..JobSettings::default()
        };
        assert_eq!(
            strs(&lp_opts(&settings)),
            vec![
                "-o",
                "sides=two-sided-long-edge",
                "-o",
                "media=A3",
                "-o",
                "print-quality=4",
                "-o",
                "cupsPrintQuality=Normal",
            ]
        );
    }

    /// FR-041: Quality and Tray are editable in the profile editor, so they have
    /// to reach `lp` — in both the IPP and the PPD spelling, since which one a
    /// queue understands depends on how it was created.
    #[test]
    fn fr_041_quality_and_tray_reach_lp() {
        let with = |quality, tray: Option<&str>| {
            lp_opts(&JobSettings {
                quality,
                tray: tray.map(String::from),
                ..JobSettings::default()
            })
        };

        for (quality, ipp, ppd) in [
            (Quality::Draft, "print-quality=3", "cupsPrintQuality=Draft"),
            (
                Quality::Normal,
                "print-quality=4",
                "cupsPrintQuality=Normal",
            ),
            (Quality::High, "print-quality=5", "cupsPrintQuality=High"),
        ] {
            let opts = with(quality, None);
            assert!(opts.iter().any(|o| o == ipp), "{quality:?}: {opts:?}");
            assert!(opts.iter().any(|o| o == ppd), "{quality:?}: {opts:?}");
        }

        let opts = with(Quality::Normal, Some("Drawer 1"));
        assert!(opts.iter().any(|o| o == "InputSlot=Drawer 1"), "{opts:?}");
        assert!(
            opts.iter().any(|o| o == "media-source=Drawer 1"),
            "{opts:?}"
        );

        // No tray chosen → nothing is said, so the printer's default bin wins.
        let opts = with(Quality::Normal, None);
        assert!(
            !opts.iter().any(|o| o.starts_with("InputSlot=")),
            "{opts:?}"
        );
        assert!(
            !opts.iter().any(|o| o.starts_with("media-source=")),
            "{opts:?}"
        );
        // A blank name is not a bin name (`validate` rejects it too).
        let opts = with(Quality::Normal, Some("   "));
        assert!(
            !opts.iter().any(|o| o.starts_with("InputSlot=")),
            "{opts:?}"
        );
    }

    /// Portrait is a choice, not the absence of one: it must reach the queue
    /// (a queue can default to landscape). Only auto stays silent.
    #[test]
    fn orientation_portrait_and_landscape_are_explicit_and_auto_is_silent() {
        let with = |orientation| {
            lp_opts(&JobSettings {
                orientation,
                ..JobSettings::default()
            })
        };
        assert!(with(Orientation::Portrait)
            .iter()
            .any(|o| o == "orientation-requested=3"));
        assert!(with(Orientation::Landscape)
            .iter()
            .any(|o| o == "orientation-requested=4"));
        let auto = with(Orientation::Auto);
        assert!(
            !auto.iter().any(|o| o.starts_with("orientation-requested")),
            "{auto:?}"
        );
        assert!(!auto.iter().any(|o| o == "landscape"), "{auto:?}");
    }

    /// Any size the printer lists reaches `lp` in the printer's own spelling;
    /// `auto` sends no media at all, so the printer's own setting applies.
    #[test]
    fn media_uses_the_printers_keyword_and_auto_sends_none() {
        let caps = PrinterCaps {
            papers: vec!["A4".into(), "Letter".into(), "4x6".into(), "Tabloid".into()],
            ..PrinterCaps::unknown()
        };
        let with = |name: &str| {
            lp_opts_for(
                &JobSettings {
                    paper_size: PaperSize::parse(name).unwrap(),
                    ..JobSettings::default()
                },
                &caps,
            )
        };
        assert!(with("tabloid").iter().any(|o| o == "media=Tabloid"));
        assert!(with("4x6").iter().any(|o| o == "media=4x6"));
        assert!(with("letter").iter().any(|o| o == "media=Letter"));
        let auto = with("auto");
        assert!(!auto.iter().any(|o| o.starts_with("media=")), "{auto:?}");
    }

    /// `lp -o` splits a value on whitespace (and honours quotes), so a tray or
    /// paper name like `Tray1 copies=999` would add options the UI never
    /// shows. Such values are refused, and `spool` refuses before running `lp`.
    #[test]
    fn fr_041_option_values_that_lp_would_split_are_refused() {
        let with = |paper: &str, tray: Option<&str>| JobSettings {
            paper_size: PaperSize::parse(paper).unwrap(),
            tray: tray.map(String::from),
            ..JobSettings::default()
        };
        let caps = PrinterCaps::unknown();
        assert_eq!(
            check_option_values(&with("A4", Some("Tray1")), &caps),
            Ok(())
        );
        assert_eq!(check_option_values(&with("w288h432", None), &caps), Ok(()));
        assert_eq!(
            check_option_values(&with("na_letter_8.5x11in", Some("auto")), &caps),
            Ok(())
        );

        let err = check_option_values(
            &with("A4", Some("Tray1 copies=999 job-hold-until=indefinite")),
            &caps,
        )
        .unwrap_err();
        assert_eq!(err.option, "InputSlot");
        let msg = err.to_string();
        assert!(msg.starts_with("tray \"Tray1 copies=999"), "{msg}");
        assert!(msg.contains("cannot contain spaces"), "{msg}");

        let err = check_option_values(&with("A4 number-up=16", None), &caps).unwrap_err();
        assert_eq!(err.option, "media");
        assert!(err.to_string().starts_with("paper size"), "{err}");

        for bad in ["A4\tx", "A4'", "A4\"", "A4\\ x", "A4=x", "A4\u{a0}x"] {
            assert!(
                check_option_values(&with("A4", Some(bad)), &caps).is_err(),
                "{bad:?} must be refused"
            );
        }
        assert!(check_option_value("InputSlot", "").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn fr_041_spool_refuses_an_unsafe_tray_before_running_lp() {
        let settings = JobSettings {
            tray: Some("Tray1 copies=999".into()),
            ..JobSettings::default()
        };
        // The printer does not even exist: the refusal must come first.
        let err = spool(
            Path::new("/nonexistent/x.pdf"),
            "No_Such_Queue_For_Tests",
            &settings,
            &PrinterCaps::unknown(),
            "t",
            Some(std::time::Duration::from_secs(5)),
        );
        let Err(err) = err else {
            panic!("an unsafe tray must not be spooled")
        };
        match err {
            super::super::PrintError::Engine(crate::engine::EngineError::UnsupportedSetting {
                setting,
                printer,
            }) => {
                assert!(setting.contains("Tray1 copies=999"), "{setting}");
                assert_eq!(printer, "No_Such_Queue_For_Tests");
            }
            other => panic!("expected UnsupportedSetting, got {other:?}"),
        }
    }

    #[test]
    fn colour_asks_for_rgb_only_where_the_printer_lists_it() {
        let colour = |models: &[&str]| {
            lp_opts_for(
                &JobSettings::default(),
                &PrinterCaps {
                    color_models: models.iter().map(|m| m.to_string()).collect(),
                    ..PrinterCaps::unknown()
                },
            )
        };
        assert!(colour(&["Gray", "RGB"])
            .iter()
            .any(|o| o == "ColorModel=RGB"));
        assert!(
            colour(&["Gray", "rgb"])
                .iter()
                .any(|o| o == "ColorModel=rgb"),
            "the printer's spelling"
        );
        for models in [&["Gray", "CMYK"][..], &[][..]] {
            let opts = colour(models);
            assert!(
                !opts.iter().any(|o| o.starts_with("ColorModel=")),
                "{models:?}: {opts:?}"
            );
        }
        // Mono stays Gray whatever the printer lists.
        let mono = lp_opts_for(
            &JobSettings {
                color_mode: ColorMode::Monochrome,
                ..JobSettings::default()
            },
            &PrinterCaps {
                color_models: vec!["RGB".into()],
                ..PrinterCaps::unknown()
            },
        );
        assert!(mono.iter().any(|o| o == "ColorModel=Gray"));
        assert!(!mono.iter().any(|o| o == "ColorModel=RGB"));
    }

    #[test]
    fn collate_is_explicit_for_several_copies_only() {
        let with = |copies, collate| {
            lp_opts(&JobSettings {
                copies,
                collate,
                ..JobSettings::default()
            })
        };
        assert!(with(3, true).iter().any(|o| o == "collate=true"));
        assert!(with(3, false).iter().any(|o| o == "collate=false"));
        assert!(!with(1, false).iter().any(|o| o.starts_with("collate=")));
    }

    #[test]
    fn job_number_is_the_last_segment_of_the_request_id() {
        assert_eq!(job_number("Office-42"), Some(42));
        assert_eq!(job_number("EPSON_L15160_Series-7"), Some(7));
        assert_eq!(job_number("Front-Desk-Laser-1522"), Some(1522));
        assert_eq!(job_number("Office"), None);
        assert_eq!(job_number(""), None);
    }

    #[test]
    fn ipp_job_states_map_one_to_one() {
        use crate::engine::print::OsJobState as S;
        for (kw, state) in [
            ("pending", S::Pending),
            ("pending-held", S::Held),
            ("processing", S::Processing),
            ("processing-stopped", S::Stopped),
            ("canceled", S::Canceled),
            ("aborted", S::Aborted),
            ("completed", S::Completed),
        ] {
            assert_eq!(parse_ipp_job_state(kw), Some(state), "{kw}");
        }
        assert_eq!(parse_ipp_job_state("something-new"), None);
    }

    /// Captured from `ipptool -tv ipp://localhost/jobs/1522
    /// get-job-attributes.test` against the `Maecly_Hold` queue.
    #[test]
    fn ipptool_output_yields_the_job_state() {
        use crate::engine::print::OsJobState as S;
        let held = r#""/usr/share/cups/ipptool/get-job-attributes.test":
    Get-Job-Attributes:
        attributes-charset (charset) = utf-8
    Get job info with get-job-attributes                                 [PASS]
        RECEIVED: 431 bytes in response
        status-code = successful-ok (successful-ok)
        job-state (enum) = pending-held
        job-state-reasons (keyword) = job-hold-until-specified
"#;
        assert_eq!(parse_ipptool_output(held), Some(S::Held));
        let done = "        job-state (enum) = completed\n";
        assert_eq!(parse_ipptool_output(done), Some(S::Completed));
        let gone = "        status-code = client-error-not-found (client-error-not-found)\n";
        assert_eq!(parse_ipptool_output(gone), Some(S::Gone));
        assert_eq!(parse_ipptool_output("garbage"), None);
    }

    /// The test file is always passed by absolute path, never a bare name that
    /// `ipptool` would first look up in the current directory.
    #[cfg(unix)]
    #[test]
    fn ipptool_reads_job_state_from_an_absolute_private_test_file() {
        let file = job_state_test_file().unwrap();
        assert!(file.path().is_absolute());
        assert_eq!(
            std::fs::read_to_string(file.path()).unwrap(),
            GET_JOB_STATE_TEST
        );
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(file.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "private to this user: {mode:o}");

        let args = ipptool_args(42, file.path());
        assert_eq!(args[..2], ["-tv", "ipp://localhost/jobs/42"]);
        assert_eq!(Path::new(&args[2]), file.path());
        assert!(!args.iter().any(|a| a == "get-job-attributes.test"));
        assert!(GET_JOB_STATE_TEST.contains("OPERATION get-job-attributes"));
    }

    /// With a real `ipptool` and scheduler, the embedded test asks the same
    /// question as CUPS's stock file: an unknown job reads as gone. Skips
    /// when either is missing.
    #[cfg(unix)]
    #[test]
    fn embedded_ipptool_test_reports_an_unknown_job_as_gone() {
        let file = job_state_test_file().unwrap();
        let out = match crate::util::appimage::host_command("ipptool")
            .args(ipptool_args(999_999_999, file.path()))
            .env("LC_ALL", "C")
            .output()
        {
            Ok(out) => String::from_utf8_lossy(&out.stdout).into_owned(),
            Err(e) => {
                eprintln!("SKIPPED: ipptool not runnable: {e}");
                return;
            }
        };
        match parse_ipptool_output(&out) {
            Some(state) => assert_eq!(state, super::super::OsJobState::Gone, "{out}"),
            None => eprintln!("SKIPPED: no scheduler answered: {out}"),
        }
    }

    /// Real-spooler check against the `Maecly_Hold` queue, which holds every
    /// job (nothing prints). Opt-in: `cargo test -- --ignored maecly_hold`.
    #[cfg(unix)]
    #[test]
    #[ignore = "needs the local CUPS queue Maecly_Hold"]
    fn maecly_hold_job_is_tracked_as_held_then_canceled() {
        use crate::engine::print::OsJobState;
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("track.pdf");
        std::fs::write(
            &pdf,
            crate::platform::testpage::render(&crate::platform::testpage::TestPageInfo {
                locale: "en".into(),
                printer: "Maecly_Hold".into(),
                paper: None,
                tray: None,
            }),
        )
        .unwrap();
        let out = spool(
            &pdf,
            "Maecly_Hold",
            &JobSettings::default(),
            &PrinterCaps::unknown(),
            "pluryprint-track-test",
            Some(std::time::Duration::from_secs(30)),
        )
        .expect("spooled");
        let id = out.os_job_id.expect("lp reports a request id");
        assert_eq!(job_state(&id), Ok(OsJobState::Held));
        cancel(&id).unwrap();
        assert_eq!(job_state(&id), Ok(OsJobState::Canceled));
    }

    /// A scheduler that does not answer is not a finished job. `lpstat`
    /// exits non-zero with an empty stdout when cupsd is unreachable, which
    /// used to read as "no longer listed" and marked the job done.
    #[test]
    fn fr_032_a_failed_lpstat_is_transient_not_a_finished_job() {
        use crate::engine::print::TrackError;
        let err = lpstat_output(false, "", "lpstat: Unable to connect to server\n").unwrap_err();
        assert!(
            matches!(&err, TrackError::Transient(m) if m.contains("Unable to connect")),
            "{err:?}"
        );
        assert_eq!(
            lpstat_output(true, "Q-1 user 1024 Mon\n", "").unwrap(),
            "Q-1 user 1024 Mon\n"
        );
    }

    /// Without `ipptool`, a job that left the queue may have printed or been
    /// cancelled or aborted: it is `Gone`, never reported as `Completed`.
    #[test]
    fn fr_032_lpstat_never_claims_a_finished_job_printed() {
        use crate::engine::print::OsJobState;
        let waiting = "Maecly_Hold-7 me 1024 Mon 10:00\n";
        assert_eq!(
            lpstat_verdict(waiting, "Maecly_Hold-7"),
            OsJobState::Processing
        );
        assert_eq!(lpstat_verdict("", "Maecly_Hold-7"), OsJobState::Gone);
    }

    #[test]
    fn lpstat_lists_matches_the_first_column_exactly() {
        let out = "Maecly_Hold-1523        ana     1024   Fri Oct  9 15:32:48 2026\n\
                   Maecly_Hold-15230       ana     1024   Fri Oct  9 15:32:48 2026\n";
        assert!(lpstat_lists(out, "Maecly_Hold-1523"));
        assert!(!lpstat_lists(out, "Maecly_Hold-152"));
        assert!(!lpstat_lists("", "Maecly_Hold-1523"));
    }

    #[test]
    fn parse_request_id_handles_hyphenated_dests() {
        assert_eq!(
            parse_request_id("request id is Office-42 (1 file(s))", "Office"),
            Some("Office-42".to_string())
        );
        assert_eq!(
            parse_request_id(
                "request id is EPSON_L15160_Series-7 (1 file(s))",
                "EPSON_L15160_Series"
            ),
            Some("EPSON_L15160_Series-7".to_string())
        );
        assert_eq!(parse_request_id("no request id present", "Office"), None);
        assert_eq!(parse_request_id("", "Office"), None);
    }

    /// macOS localizes `lp`'s line from the user's language list even under
    /// `LC_ALL=C` — captured verbatim on a Spanish Mac. Without this the job id
    /// was lost, so the job could neither be tracked nor cancelled.
    #[test]
    fn parse_request_id_survives_a_localized_lp() {
        assert_eq!(
            parse_request_id(
                "el id de la petición es Maecly_Hold-1537 (1 archivos)",
                "Maecly_Hold"
            ),
            Some("Maecly_Hold-1537".to_string())
        );
        assert_eq!(
            parse_request_id(
                "l'identifiant de requête est Front-Desk-9 (1 fichier(s))",
                "Front-Desk"
            ),
            Some("Front-Desk-9".to_string())
        );
        // A number elsewhere on the line is not mistaken for the id.
        assert_eq!(
            parse_request_id("ID 2024 de solicitud: Office-3 (1 archivos)", "Office"),
            Some("Office-3".to_string())
        );
    }
}
