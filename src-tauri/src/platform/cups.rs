//! CUPS printer enumeration for Unix (macOS, Linux).
//!
//! macOS localizes `lpstat`'s human-readable prose even under `LC_ALL=C`, so we
//! avoid parsing it and read locale-independent IPP keyword output instead:
//!
//! - `lpstat -e` — bare queue names (not localized).
//! - `lpstat -d` — default; we keep the trailing token only if it is a known queue.
//! - `lpoptions -p N` — IPP metadata (`device-uri`, `printer-state`, `printer-make-and-model`, `marker-colors`/`marker-levels`).
//! - `lpoptions -p N -l` — capability lists (`ColorModel`, `Duplex`, `PageSize`, `InputSlot`, `Resolution`).
//! - `ippfind` — mDNS/Bonjour discovery.
//!
//! Per-queue enumeration is best-effort: missing tools or fields degrade to
//! empty/`None`, because a half-readable printer is still worth showing.
//! Two exceptions return `Err` so the UI can say so instead of showing an
//! empty list: a printer list without `lpstat` at all (`cups_missing`: on
//! Linux the CUPS client tools are a package of their own), and a network
//! scan that could not run.

use std::collections::HashMap;
use std::process::Command;

use anyhow::Context;

use crate::ipc::printers::{DiscoveredPrinter, Printer, PrinterStatus, Supplies, Tray};
use crate::platform::infer_kind;
use crate::util::error::{IpcErrorCode, LocalizedError};

/// Run a CLI in the C locale and return its stdout (lossy UTF-8).
fn lc_run(program: &str, args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .output()
        .with_context(|| format!("failed to spawn `{program}`"))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run a CLI and succeed only on a zero exit status (macOS settings panes;
/// the tests' queue clean-up).
#[cfg(any(test, target_os = "macos"))]
fn run_ok(program: &str, args: &[&str]) -> anyhow::Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("failed to spawn `{program}`"))?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("`{program}` exited with {status}");
    }
}

/// Run a CLI and, on failure, report it as `reason` (FR-092) with the tool's
/// captured stderr verbatim as the English message and the `detail` param.
///
/// The UI says `reason` in the user's language and shows the tool's own
/// diagnostic (e.g. lpadmin's "Unable to connect to ...") under it as the
/// technical detail. Keeps the `LC_ALL=C` locale so that detail is stable.
/// A tool that cannot even be started means CUPS itself is not answering
/// (`cups_unavailable`).
fn run_ok_capture(program: &str, args: &[&str], reason: &'static str) -> anyhow::Result<()> {
    let out = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .output()
        .map_err(|e| {
            LocalizedError::new(
                IpcErrorCode::PrintFailed,
                "cups_unavailable",
                format!("failed to spawn `{program}`: {e}"),
            )
        })?;
    if out.status.success() {
        return Ok(());
    }
    Err(tool_failure(
        reason,
        program,
        &out.status.to_string(),
        &String::from_utf8_lossy(&out.stderr),
    )
    .into())
}

/// The `PrintFailed` error a CUPS tool that exited with `status` becomes:
/// `reason`, with what the tool printed (or its status) as the message and
/// the `detail` param. Pure.
fn tool_failure(reason: &'static str, program: &str, status: &str, stderr: &str) -> LocalizedError {
    let msg = stderr.trim();
    let detail = if msg.is_empty() {
        format!("`{program}` exited with {status}")
    } else {
        msg.to_string()
    };
    LocalizedError::new(IpcErrorCode::PrintFailed, reason, detail.clone()).param("detail", detail)
}

/// Set the CUPS *user* default — writes `~/.cups/lpoptions`, no privileges.
///
/// Like every user-triggered spooler action here, failures carry the tool's
/// own stderr ([`run_ok_capture`]) so the toast can say *why*.
pub fn set_os_default(name: &str) -> anyhow::Result<()> {
    run_ok_capture("lpoptions", &["-d", name], "set_default_failed")
}

/// Print the branded Pluryprint test page, applying the user's configured
/// default paper/tray for this printer (from `PrinterPrefs`).
pub fn print_test_page(name: &str) -> anyhow::Result<()> {
    use std::io::Write as _;

    // The app's saved defaults for this printer.
    let config = crate::settings::Config::load_or_default();
    // The sheet and its job title are in the app's language (FR-092).
    let locale = config.app.locale.clone();
    let prefs = config.printers;
    let over = prefs.overrides.get(name).cloned().unwrap_or_default();

    // A friendlier display name for the page.
    let opts = parse_kv(&lc_run("lpoptions", &["-p", name]).unwrap_or_default());
    let display = opts
        .get("printer-info")
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| name.to_string());

    let pdf = super::testpage::render(&super::testpage::TestPageInfo {
        locale: locale.clone(),
        printer: display,
        paper: over.paper.clone(),
        tray: over.tray.clone(),
    });

    let tmp = tempfile::Builder::new()
        .prefix("pluryprint-testpage-")
        .suffix(".pdf")
        .tempfile()
        .and_then(|mut tmp| {
            tmp.write_all(&pdf)?;
            tmp.flush()?;
            Ok(tmp)
        })
        .map_err(|e| {
            LocalizedError::new(
                IpcErrorCode::PrintFailed,
                "test_page_failed",
                format!("failed to create the test page PDF: {e}"),
            )
        })?;
    let path = tmp.path().to_string_lossy().into_owned();

    // `lp` spools (copies) the file before returning, so the temp file can be
    // dropped afterwards.
    let mut args = vec![
        "-d".to_string(),
        name.to_string(),
        "-t".to_string(),
        super::testpage::Strings::for_locale(&locale).title,
    ];
    // The saved names go to `lp -o`, which splits a value on whitespace into
    // extra options; refuse anything that is not a single CUPS keyword.
    use crate::engine::print::cups::check_option_value;
    let invalid_option = |e: String| {
        LocalizedError::new(
            IpcErrorCode::InvalidRequest,
            "printer_option_invalid",
            format!("invalid {e}"),
        )
    };
    if let Some(paper) = &over.paper {
        check_option_value("media", paper).map_err(|e| invalid_option(e.to_string()))?;
        args.push("-o".into());
        args.push(format!("media={paper}"));
    }
    if let Some(tray) = &over.tray {
        check_option_value("InputSlot", tray).map_err(|e| invalid_option(e.to_string()))?;
        args.push("-o".into());
        args.push(format!("InputSlot={tray}"));
    }
    args.push(path);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_ok_capture("lp", &arg_refs, "spooler_rejected")
}

/// Open the OS "Printers" settings so the user can add one manually.
/// macOS 13+ deep link, with a legacy-pane fallback.
#[cfg(target_os = "macos")]
pub fn open_printer_settings() -> anyhow::Result<()> {
    const URL: &str = "x-apple.systempreferences:com.apple.Print-Scan-Settings.extension";
    if run_ok("open", &[URL]).is_ok() {
        return Ok(());
    }
    run_ok(
        "open",
        &["/System/Library/PreferencePanes/PrintAndFax.prefPane"],
    )
}

/// Open the desktop's printer settings so the user can add one manually
/// (Linux and other non-macOS Unix).
///
/// Every desktop has its own tool, so the candidates of
/// [`printer_settings_candidates`] are tried in turn, the running desktop's
/// first, ending with the CUPS web interface in the browser. A tool that is
/// not installed, or that fails at once, moves on to the next. When none can
/// be started the error says where CUPS can still be managed
/// (`printer_settings_unavailable`).
#[cfg(not(target_os = "macos"))]
pub fn open_printer_settings() -> anyhow::Result<()> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let appdir = std::env::var_os("APPDIR").map(std::path::PathBuf::from);
    let mut tried = Vec::new();
    for (program, args) in printer_settings_candidates(&desktop) {
        match launch_detached(program, args, appdir.as_deref()) {
            Ok(()) => return Ok(()),
            Err(e) => tried.push(format!("{program}: {e}")),
        }
    }
    Err(LocalizedError::new(
        IpcErrorCode::Internal,
        "printer_settings_unavailable",
        format!(
            "no printer settings tool could be opened ({}); manage printers at \
             http://localhost:631/ in a browser",
            tried.join("; ")
        ),
    )
    .into())
}

/// The CUPS web interface's printer page, the last resort of
/// [`open_printer_settings`].
#[cfg(any(test, not(target_os = "macos")))]
const CUPS_WEB_PRINTERS: &str = "http://localhost:631/printers/";

/// The printer-settings tools to try on a Linux desktop, in order. Pure.
///
/// `desktop` is `XDG_CURRENT_DESKTOP` (a `:`-separated list such as
/// `ubuntu:GNOME`). KDE Plasma's own module comes first on KDE, GNOME
/// Settings first on GNOME and its relatives, and the desktop-neutral
/// `system-config-printer` first everywhere else (Xfce, Cinnamon, MATE...).
/// `xdg-open` on the CUPS web interface is always last.
#[cfg(any(test, not(target_os = "macos")))]
fn printer_settings_candidates(desktop: &str) -> Vec<(&'static str, &'static [&'static str])> {
    const GNOME: (&str, &[&str]) = ("gnome-control-center", &["printers"]);
    const SCP: (&str, &[&str]) = ("system-config-printer", &[]);
    const KDE6: (&str, &[&str]) = ("kcmshell6", &["kcm_printer_manager"]);
    const KDE5: (&str, &[&str]) = ("kcmshell5", &["kcm_printer_manager"]);
    let is = |name: &str| {
        desktop
            .split(':')
            .any(|d| d.trim().eq_ignore_ascii_case(name))
    };
    let mut list = if is("KDE") {
        vec![KDE6, KDE5, SCP, GNOME]
    } else if ["GNOME", "Unity", "Budgie", "Pantheon"]
        .iter()
        .any(|d| is(d))
    {
        vec![GNOME, SCP, KDE6, KDE5]
    } else {
        vec![SCP, GNOME, KDE6, KDE5]
    };
    list.push(("xdg-open", &[CUPS_WEB_PRINTERS]));
    list
}

/// How long a settings tool may take to fail before it counts as started.
#[cfg(not(target_os = "macos"))]
const LAUNCH_GRACE: std::time::Duration = std::time::Duration::from_millis(1500);

/// Start a GUI tool without waiting for its window to close: it counts as
/// started unless it cannot be spawned or exits with a failure within
/// [`LAUNCH_GRACE`] (an unknown settings panel, no display). A tool still
/// running is reaped on a background thread when it exits.
///
/// Inside an AppImage the environment points GTK and GIO at the libraries
/// bundled with Pluryprint; a system tool must not load them, so those
/// variables are cleaned first ([`host_env`]).
#[cfg(not(target_os = "macos"))]
fn launch_detached(
    program: &str,
    args: &[&str],
    appdir: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    use std::process::Stdio;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(appdir) = appdir {
        for (key, value) in host_env(appdir, std::env::vars_os()) {
            match value {
                Some(value) => cmd.env(key, value),
                None => cmd.env_remove(key),
            };
        }
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn `{program}`"))?;
    let started = std::time::Instant::now();
    while started.elapsed() < LAUNCH_GRACE {
        match child.try_wait()? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => anyhow::bail!("`{program}` exited with {status}"),
            None => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    }
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// The environment changes that keep a host program from loading what an
/// AppImage mounted at `appdir` bundles. Pure.
///
/// A list variable (`PATH`, `LD_LIBRARY_PATH`, `XDG_DATA_DIRS`...) loses its
/// entries inside `appdir` and is removed when nothing is left; any other
/// variable whose value points inside `appdir` (`GTK_PATH`,
/// `GDK_PIXBUF_MODULE_FILE`, `GIO_MODULE_DIR`...) is removed, and so are
/// `APPDIR`, `APPIMAGE` and `ARGV0` themselves. `None` means "remove".
#[cfg(any(test, not(target_os = "macos")))]
fn host_env(
    appdir: &std::path::Path,
    vars: impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) -> Vec<(std::ffi::OsString, Option<std::ffi::OsString>)> {
    let inside = |p: &std::path::Path| !appdir.as_os_str().is_empty() && p.starts_with(appdir);
    let mut changes = Vec::new();
    for (key, value) in vars {
        if ["APPDIR", "APPIMAGE", "ARGV0"].iter().any(|k| key == *k) {
            changes.push((key, None));
            continue;
        }
        let entries: Vec<std::path::PathBuf> = std::env::split_paths(&value).collect();
        if !entries.iter().any(|e| inside(e)) {
            continue;
        }
        let kept: Vec<&std::path::PathBuf> = entries.iter().filter(|e| !inside(e)).collect();
        let joined = std::env::join_paths(kept.iter().filter(|e| !e.as_os_str().is_empty()))
            .ok()
            .filter(|v| !v.is_empty());
        changes.push((key, if entries.len() > 1 { joined } else { None }));
    }
    changes
}

// ----------------------------------------------------------------------------
// Adding a printer (`lpadmin`)
// ----------------------------------------------------------------------------

/// Install a CUPS print queue for `uri`, labelled with the human `name`.
///
/// `lpadmin -p EXISTING` silently *modifies* an existing queue, so we derive a
/// legal queue id from `name` and then de-duplicate it against the installed
/// queues before creating anything. Driverless IPP URIs use the `everywhere`
/// model; raw sockets/LPD use the `raw` model; anything else is rejected with a
/// message the UI can show.
pub fn add_printer(name: &str, uri: &str) -> anyhow::Result<()> {
    let base = sanitize_queue_name(name);
    let queue = unique_queue_name(&base, &queue_names());
    // The display/description keeps the user's original text; fall back to the
    // queue id if the name was blank.
    let trimmed = name.trim();
    let display = if trimmed.is_empty() {
        queue.clone()
    } else {
        trimmed.to_string()
    };
    let args = lpadmin_args(&queue, uri, &display)?;
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_ok_capture("lpadmin", &arg_refs, "add_printer_failed")
}

/// Turn a free-form display name into a legal CUPS queue id.
///
/// Mirrors `lpadmin`'s own `validate_name` so nothing the form accepts can be
/// refused by the spooler: every character that is not printable ASCII, and
/// each of `/ \ ? ' " # @` (`@` ends a CUPS name), becomes `_`. Accented
/// letters are therefore replaced too — the human name survives untouched as
/// the queue's description (`-D`). The result is capped at 127 **bytes** (CUPS
/// requires fewer than 128 octets) and falls back to `"printer"` when nothing
/// usable is left (empty, all whitespace, all punctuation).
fn sanitize_queue_name(name: &str) -> String {
    const MAX_BYTES: usize = 127;
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && !matches!(c, '/' | '\\' | '?' | '\'' | '"' | '#' | '@') {
                c
            } else {
                '_'
            }
        })
        // Every char is ASCII at this point, so one char is one byte.
        .take(MAX_BYTES)
        .collect();
    if cleaned.chars().all(|c| c == '_') {
        "printer".to_string()
    } else {
        cleaned
    }
}

/// Return `base` if no installed queue already uses it, otherwise the first
/// free `base_2`, `base_3`, … variant. This is what keeps `add` from silently
/// overwriting an existing queue.
///
/// CUPS resolves queue names **case-insensitively** (`lpadmin -p office`
/// modifies an existing `Office`), so the comparison ignores ASCII case; the
/// sanitized id is pure ASCII, so that is a complete case fold.
fn unique_queue_name(base: &str, existing: &[String]) -> String {
    let taken = |name: &str| existing.iter().any(|e| e.eq_ignore_ascii_case(name));
    if !taken(base) {
        return base.to_string();
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{base}_{n}");
        if !taken(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Build the `lpadmin` argument vector, selecting the PPD-less model by scheme:
/// `ipp`/`ipps`/`dnssd` are driverless (`-m everywhere`); `socket`/`lpd` are
/// raw byte streams (`-m raw`). Any other scheme is rejected.
fn lpadmin_args(queue: &str, uri: &str, display: &str) -> anyhow::Result<Vec<String>> {
    let model = match scheme(uri) {
        "ipp" | "ipps" | "dnssd" => "everywhere",
        "socket" | "lpd" => "raw",
        other => {
            return Err(LocalizedError::new(
                IpcErrorCode::InvalidRequest,
                "add_printer_unsupported_scheme",
                format!(
                    "Pluryprint can't add a printer over '{other}'. Add it from your \
                     system's printer settings instead."
                ),
            )
            .param("scheme", other)
            .into())
        }
    };
    Ok(vec![
        "-p".into(),
        queue.into(),
        "-E".into(),
        "-v".into(),
        uri.into(),
        "-m".into(),
        model.into(),
        "-D".into(),
        display.into(),
    ])
}

/// Every installed CUPS queue with its options, capabilities and pending-job
/// count. An empty CUPS setup is `Ok(vec![])`, not an error; a system without
/// the CUPS client tools is `cups_missing`.
pub fn list_printers() -> anyhow::Result<Vec<Printer>> {
    let names = queue_names_or_missing(|| lc_run("lpstat", &["-e"]))?;
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let default = default_printer().unwrap_or(None);
    let jobs = parse_jobs(&lc_run("lpstat", &["-o"]).unwrap_or_default());

    Ok(names
        .iter()
        .map(|name| detail(name, default.as_deref(), &jobs))
        .collect())
}

/// The name of the CUPS default queue, or `None` when no default is set (or
/// `lpstat -d` names something that is not an installed queue).
pub fn default_printer() -> anyhow::Result<Option<String>> {
    let names = queue_names();
    let out = lc_run("lpstat", &["-d"]).unwrap_or_default();
    // The line is "<localized label>: NAME"; queue names have no spaces, so the
    // trailing token is the name — but only trust it if it is a real queue.
    let token = out.split_whitespace().last().unwrap_or("").to_string();
    Ok(if names.iter().any(|n| n == &token) {
        Some(token)
    } else {
        None
    })
}

/// Run `ippfind`, telling "the scan found nothing" apart from "the scan could
/// not run".
///
/// This deliberately does NOT go through [`lc_run`]/[`run_ok`]: `ippfind` exits
/// **1** when it matched no service, which is an empty network — the normal,
/// successful outcome of scanning a room with no printers in it. Only a spawn
/// failure (the CUPS client tools are not installed), a signal, or a non-0/1
/// status is a real error, and that error must reach the UI instead of being
/// painted as "no printers found" (which is what `unwrap_or_default()` used to
/// do).
fn run_ippfind(args: &[&str]) -> anyhow::Result<String> {
    let out = match Command::new("ippfind")
        .args(args)
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .output()
    {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(LocalizedError::new(
                IpcErrorCode::DependencyMissing,
                "ippfind_missing",
                "`ippfind` is not installed — network discovery needs the CUPS client tools",
            )
            .into())
        }
        Err(e) => return Err(discovery_failed(format!("failed to run `ippfind`: {e}")).into()),
    };
    classify_ippfind(
        out.status.code(),
        &out.status.to_string(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        &String::from_utf8_lossy(&out.stderr),
    )
}

/// Pure exit-status policy for [`run_ippfind`], split out so it is unit-testable
/// without spawning anything: exit **0** (matches) and **1** (no matches) are
/// both successful scans; anything else surfaces the tool's own stderr.
fn classify_ippfind(
    code: Option<i32>,
    status_text: &str,
    stdout: String,
    stderr: &str,
) -> anyhow::Result<String> {
    if matches!(code, Some(0) | Some(1)) {
        return Ok(stdout);
    }
    let msg = stderr.trim();
    let detail = if msg.is_empty() {
        format!("`ippfind` exited with {status_text}")
    } else {
        msg.to_string()
    };
    Err(discovery_failed(detail).into())
}

/// A network scan that could not run (FR-092 `discovery_failed`), with the
/// tool's own words as the technical detail.
fn discovery_failed(detail: String) -> LocalizedError {
    LocalizedError::new(IpcErrorCode::Internal, "discovery_failed", detail.clone())
        .param("detail", detail)
}

/// Scan the local network for IPP/IPPS printers via `ippfind` (about four
/// seconds). An empty network is `Ok(vec![])`; a scan that could not run is
/// an error so the UI can say so instead of showing "no printers found".
pub fn discover_printers() -> anyhow::Result<Vec<DiscoveredPrinter>> {
    // `-T 4` bounds the scan. `-x printf …` prints one tab-separated record
    // per service: URI, the TXT `UUID`, the Bonjour service name and the TXT
    // `ty` (make and model) — the UUID is what lets us recognise a printer we
    // already installed under a different URI. A failure propagates: the UI
    // renders a scan error, not an empty network.
    let out = run_ippfind(&[
        "_ipp._tcp",
        "_ipps._tcp",
        "-T",
        "4",
        "-x",
        "/usr/bin/printf",
        "%s\\t%s\\t%s\\t%s\\n",
        "{service_uri}",
        "{txt_UUID}",
        "{service_name}",
        "{txt_ty}",
        ";",
    ])?;
    let installed: Vec<String> = queue_names()
        .iter()
        .filter_map(|n| {
            parse_kv(&lc_run("lpoptions", &["-p", n]).unwrap_or_default()).remove("device-uri")
        })
        .collect();
    Ok(parse_discovery(&out, &installed))
}

/// One `ippfind` hit, before de-duplication.
struct Found {
    uri: String,
    uuid: String,
    name: String,
    model: String,
}

/// Turn `ippfind` records into the "Scan network" rows, pure for tests.
///
/// The same physical printer usually advertises itself several times (once
/// per `_ipp`/`_ipps` service), and the copy CUPS installed is stored under a
/// different URI again (`dnssd://Service%20Name._ipps._tcp.local./?uuid=…`). So
/// devices are keyed by the printer **UUID** when it is advertised, else by the
/// host without its port, and:
///
/// - a device whose UUID, Bonjour service name or host matches an installed
///   queue's `device-uri` is skipped — it is already set up;
/// - the `ipp` and `ipps` records of one device collapse into one row, which
///   prefers `ipps`.
///
/// A bare-URI line (no tab fields) still works, keyed by host.
fn parse_discovery(stdout: &str, installed_uris: &[String]) -> Vec<DiscoveredPrinter> {
    let installed: Vec<(String, String, String)> = installed_uris
        .iter()
        .map(|u| (uri_uuid(u), uri_service_name(u).to_lowercase(), uri_host(u)))
        .collect();

    let mut rows: Vec<(String, Found)> = Vec::new();
    for line in stdout.lines() {
        let mut fields = line.split('\t').map(str::trim);
        let uri = fields.next().unwrap_or("");
        if !uri.contains("://") {
            continue;
        }
        let found = Found {
            uri: uri.to_string(),
            uuid: fields.next().unwrap_or("").to_lowercase(),
            name: fields.next().unwrap_or("").to_string(),
            model: fields.next().unwrap_or("").to_string(),
        };
        let host = uri_host(uri);
        let already_installed = installed.iter().any(|(uuid, service, ihost)| {
            (!found.uuid.is_empty() && *uuid == found.uuid)
                || (!found.name.is_empty() && *service == found.name.to_lowercase())
                || (!host.is_empty() && *ihost == host)
        });
        if already_installed {
            continue;
        }
        let key = if found.uuid.is_empty() {
            host
        } else {
            found.uuid.clone()
        };
        match rows.iter_mut().find(|(k, _)| *k == key) {
            // Same device again: keep the secure transport if this is it.
            Some((_, kept)) => {
                if scheme(&found.uri) == "ipps" && scheme(&kept.uri) != "ipps" {
                    *kept = found;
                }
            }
            None => rows.push((key, found)),
        }
    }

    rows.into_iter()
        .map(|(_, f)| {
            let name = if f.name.is_empty() {
                strip_port(&uri_service_name(&f.uri)).to_string()
            } else {
                f.name
            };
            DiscoveredPrinter {
                kind: infer_kind(&f.model, &name, Some(&f.uri)),
                connection: pretty_conn(Some(&f.uri)),
                via: format!("mDNS · {}", scheme(&f.uri).to_uppercase()),
                id: f.uri,
                name,
            }
        })
        .collect()
}

/// The `uuid=` query parameter of a device URI (lowercased), or `""`.
fn uri_uuid(uri: &str) -> String {
    uri.split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix("uuid="))
        .unwrap_or("")
        .to_lowercase()
}

/// The authority of a URI without port or trailing dot, lowercased — the
/// "same machine" key for `ipp://Host.local:631/…` vs `ipps://host.local./…`.
fn uri_host(uri: &str) -> String {
    let rest = uri.split("://").nth(1).unwrap_or("");
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    strip_port(authority).trim_end_matches('.').to_lowercase()
}

/// `host:631` → `host`; a bracketed IPv6 literal keeps its colons.
fn strip_port(authority: &str) -> &str {
    if authority.starts_with('[') {
        return authority
            .split_once(']')
            .map_or(authority, |(h, _)| h)
            .trim_start_matches('[');
    }
    authority.split(':').next().unwrap_or(authority)
}

// ----------------------------------------------------------------------------
// Per-printer assembly
// ----------------------------------------------------------------------------

fn queue_names() -> Vec<String> {
    parse_queue_names(&lc_run("lpstat", &["-e"]).unwrap_or_default())
}

/// `lpstat -e`'s output, one queue per line.
fn parse_queue_names(out: &str) -> Vec<String> {
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

/// The queue names `lpstat` (run by `run`) reports, telling "no printers"
/// apart from "no `lpstat`".
///
/// macOS always has the CUPS tools; Linux has them only with the CUPS client
/// package (`cups-client` on Debian and Ubuntu, `cups-client` or `cups` on
/// Fedora), and without it the Printers screen would claim there are no
/// printers. A program that is not there is `cups_missing`; any other failure
/// keeps the best-effort empty list.
fn queue_names_or_missing(
    run: impl FnOnce() -> anyhow::Result<String>,
) -> anyhow::Result<Vec<String>> {
    match run() {
        Ok(out) => Ok(parse_queue_names(&out)),
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) =>
        {
            Err(LocalizedError::new(
                IpcErrorCode::DependencyMissing,
                "cups_missing",
                format!(
                    "the CUPS client tools are not installed ({e:#}) — install cups-client \
                     (Debian, Ubuntu) or cups (Fedora) to list and use printers"
                ),
            )
            .into())
        }
        Err(_) => Ok(Vec::new()),
    }
}

fn detail(name: &str, default: Option<&str>, jobs: &HashMap<String, u32>) -> Printer {
    let opts = parse_kv(&lc_run("lpoptions", &["-p", name]).unwrap_or_default());
    let listing = listing_caps(capability_probe(name).as_deref());
    let caps = &listing.caps;

    let driver = opts
        .get("printer-make-and-model")
        .cloned()
        .unwrap_or_default();
    let uri = opts.get("device-uri").map(String::as_str);
    let display = opts
        .get("printer-info")
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| name.to_string());

    Printer {
        id: name.to_string(),
        name: display,
        kind: infer_kind(&driver, name, uri),
        color: listing.color,
        duplex: listing.duplex,
        color_unknown: listing.color_unknown,
        duplex_unknown: listing.duplex_unknown,
        status: status_from(
            opts.get("printer-state").map(String::as_str),
            opts.get("printer-state-reasons").map(String::as_str),
        ),
        is_default: default == Some(name),
        location: opts.get("printer-location").cloned().unwrap_or_default(),
        connection: pretty_conn(uri),
        driver,
        shared: opts
            .get("printer-is-shared")
            .map(|v| v == "true")
            .unwrap_or(false),
        queued_jobs: jobs.get(name).copied().unwrap_or(0),
        max_dpi: max_dpi(caps),
        papers: papers(caps),
        trays: trays(caps),
        supplies: parse_supplies(
            opts.get("marker-colors").map(String::as_str),
            opts.get("marker-levels").map(String::as_str),
        ),
        default_tray: caps.get("InputSlot").and_then(|c| c.default.clone()),
        default_paper: caps.get("PageSize").and_then(|c| c.default.clone()),
    }
}

// ----------------------------------------------------------------------------
// Parsers (pure — unit-tested against captured real output)
// ----------------------------------------------------------------------------

/// Parse a single `lpoptions -p NAME` line of space-separated `key=value`
/// pairs, honoring single-quoted values (which may contain spaces) and bare
/// keys (no `=`, meaning empty). Backslash-escaped spaces are unescaped.
fn parse_kv(line: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for tok in tokenize(line) {
        let (k, v) = match tok.split_once('=') {
            Some((k, v)) => (k, unquote(v)),
            None => (tok.as_str(), String::new()),
        };
        if !k.is_empty() {
            map.insert(k.to_string(), v);
        }
    }
    map
}

/// Split on whitespace, but keep single-quoted runs together.
fn tokenize(line: &str) -> Vec<String> {
    let mut toks = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in line.chars() {
        match ch {
            '\'' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    toks.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        toks.push(cur);
    }
    toks
}

fn unquote(v: &str) -> String {
    v.trim_matches('\'').replace("\\ ", " ")
}

#[derive(Default)]
struct Cap {
    values: Vec<String>,
    default: Option<String>,
}

/// Parse `lpoptions -p NAME -l` lines:
/// `Keyword/Human label: val1 *defaultVal val3`.
fn parse_caps(text: &str) -> HashMap<String, Cap> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let Some((left, right)) = line.split_once(':') else {
            continue;
        };
        let keyword = left.split('/').next().unwrap_or("").trim();
        if keyword.is_empty() {
            continue;
        }
        let mut cap = Cap::default();
        for raw in right.split_whitespace() {
            if let Some(def) = raw.strip_prefix('*') {
                cap.default = Some(def.to_string());
                cap.values.push(def.to_string());
            } else {
                cap.values.push(raw.to_string());
            }
        }
        map.insert(keyword.to_string(), cap);
    }
    map
}

/// The `ColorModel` choices `lpoptions -p <id> -l` lists for queue `id`.
pub fn color_models(id: &str) -> Vec<String> {
    parse_caps(&lc_run("lpoptions", &["-p", id, "-l"]).unwrap_or_default())
        .remove("ColorModel")
        .map(|c| c.values)
        .unwrap_or_default()
}

/// What the print engine may assume about queue `p` when it spools to it.
///
/// Read with its own `lpoptions -p <id> -l`, whose exit status is checked:
/// the listing's `duplex: false` cannot tell "this driver has no Duplex
/// option" from "the probe failed" (a driverless queue whose device is
/// briefly unreachable, a busy cupsd), and the capability gate would pause a
/// duplex job as unsupported on the strength of a failed probe.
pub fn engine_caps(p: &Printer) -> crate::engine::print::PrinterCaps {
    caps_from_probe(capability_probe(&p.id).as_deref(), &p.papers)
}

/// `lpoptions -p <name> -l`, or `None` when it could not run or failed.
fn capability_probe(name: &str) -> Option<String> {
    Command::new("lpoptions")
        .args(["-p", name, "-l"])
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// What the printer listing says about colour and duplex, from the
/// capability probe. Pure.
#[derive(Default)]
struct ListingCaps {
    caps: HashMap<String, Cap>,
    color: bool,
    color_unknown: bool,
    duplex: bool,
    duplex_unknown: bool,
}

/// The listing's reading of a capability probe, consistent with what the
/// engine assumes ([`caps_from_probe`]): a probe that failed or listed
/// nothing says nothing, so colour and duplex are *unknown* — the Printers
/// and Print Job screens then do not warn that the printer cannot do them,
/// while the engine prints duplex anyway. A real listing without a
/// `ColorModel` option leaves colour unknown too: vendor drivers name their
/// colour option differently, and "grey only" would be a guess.
fn listing_caps(probe: Option<&str>) -> ListingCaps {
    let caps = probe.map(parse_caps).unwrap_or_default();
    if caps.is_empty() {
        return ListingCaps {
            color_unknown: true,
            duplex_unknown: true,
            ..ListingCaps::default()
        };
    }
    ListingCaps {
        color: color_capable(&caps),
        color_unknown: !caps.contains_key("ColorModel"),
        duplex: duplex_capable(&caps),
        duplex_unknown: false,
        caps,
    }
}

/// [`engine_caps`]' decision, pure: a probe that failed or listed nothing is
/// [`crate::engine::print::PrinterCaps::unknown`] (duplex trusted, colour not
/// checked), never "supports nothing". `listed_papers` are the sizes the
/// Printers screen shows, which the job is checked against either way.
pub fn caps_from_probe(
    probe: Option<&str>,
    listed_papers: &[String],
) -> crate::engine::print::PrinterCaps {
    let caps = probe.map(parse_caps).unwrap_or_default();
    if caps.is_empty() {
        return crate::engine::print::PrinterCaps {
            papers: listed_papers.to_vec(),
            ..crate::engine::print::PrinterCaps::unknown()
        };
    }
    crate::engine::print::PrinterCaps {
        duplex: duplex_capable(&caps),
        papers: listed_papers.to_vec(),
        color_models: caps
            .get("ColorModel")
            .map(|c| c.values.clone())
            .unwrap_or_default(),
    }
}

fn is_grayscale(v: &str) -> bool {
    matches!(
        v.to_ascii_lowercase().as_str(),
        "gray" | "grayscale" | "fastgray" | "mono" | "monochrome" | "black" | "kgray" | "bw"
    )
}

fn color_capable(caps: &HashMap<String, Cap>) -> bool {
    caps.get("ColorModel")
        .map(|c| c.values.iter().any(|v| !is_grayscale(v)))
        .unwrap_or(false)
}

fn duplex_capable(caps: &HashMap<String, Cap>) -> bool {
    caps.get("Duplex")
        .map(|c| c.values.iter().any(|v| !v.eq_ignore_ascii_case("None")))
        .unwrap_or(false)
}

fn papers(caps: &HashMap<String, Cap>) -> Vec<String> {
    caps.get("PageSize")
        .map(|c| {
            c.values
                .iter()
                .filter(|v| !v.contains("Custom"))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn trays(caps: &HashMap<String, Cap>) -> Vec<Tray> {
    caps.get("InputSlot")
        .map(|c| {
            c.values
                .iter()
                .map(|name| Tray {
                    name: name.clone(),
                    paper: None,
                    level: None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn max_dpi(caps: &HashMap<String, Cap>) -> Option<u32> {
    let cap = caps.get("Resolution")?;
    cap.values
        .iter()
        .filter_map(|v| {
            // values like "600dpi" or "1200x1200dpi" — take the leading number.
            let digits: String = v.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<u32>().ok()
        })
        .max()
}

/// Map IPP `printer-state` + `printer-state-reasons` onto the four UI statuses.
///
/// - Any reason with the `-error` severity suffix (`media-jam-error`,
///   `cover-open-error`, `toner-empty-error`, …) is a device fault → `Error`,
///   whatever the state says. `-report`/`-warning` reasons are informational.
/// - `offline`/`shutdown` → `Offline`.
/// - A queue the user **paused** (`cupsdisable`: state 5, reason `paused`) is
///   not a fault — nothing is wrong with the device, it just won't print, which
///   is what `Offline` means to the user.
/// - Otherwise state 4 (processing) → `Busy`, state 5 (stopped) → `Error`.
fn status_from(state: Option<&str>, reasons: Option<&str>) -> PrinterStatus {
    let reasons = reasons.unwrap_or("").to_ascii_lowercase();
    let list: Vec<&str> = reasons
        .split(',')
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .collect();
    if list.iter().any(|r| r.ends_with("-error")) {
        return PrinterStatus::Error;
    }
    if reasons.contains("offline") || reasons.contains("shutdown") {
        return PrinterStatus::Offline;
    }
    if list.contains(&"paused") {
        return PrinterStatus::Offline;
    }
    match state {
        Some("4") => PrinterStatus::Busy,
        Some("5") => PrinterStatus::Error,
        _ => PrinterStatus::Ready,
    }
}

fn parse_supplies(colors: Option<&str>, levels: Option<&str>) -> Supplies {
    let mut s = Supplies::default();
    let (Some(colors), Some(levels)) = (colors, levels) else {
        return s;
    };
    for (hex, lvl) in colors.split(',').zip(levels.split(',')) {
        let Ok(pct) = lvl.trim().parse::<f64>() else {
            continue;
        };
        // IPP reserves negative levels for "not a level": -1 unavailable,
        // -2 unknown, -3 "some remaining". Leaving the channel `None` hides it
        // instead of painting an empty red bar for a cartridge that is fine.
        if pct < 0.0 {
            continue;
        }
        let frac = (pct / 100.0).clamp(0.0, 1.0);
        match hex.trim().to_ascii_uppercase().as_str() {
            "#000000" => s.black = Some(frac),
            "#00FFFF" => s.cyan = Some(frac),
            "#FF00FF" => s.magenta = Some(frac),
            "#FFFF00" => s.yellow = Some(frac),
            _ => {}
        }
    }
    s
}

/// Count queued jobs per destination from `lpstat -o`. Job ids are
/// `<dest>-<num>`; the destination may itself contain `-`.
fn parse_jobs(text: &str) -> HashMap<String, u32> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let Some(job_id) = line.split_whitespace().next() else {
            continue;
        };
        if let Some((dest, num)) = job_id.rsplit_once('-') {
            if !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()) {
                *map.entry(dest.to_string()).or_insert(0) += 1;
            }
        }
    }
    map
}

// ----------------------------------------------------------------------------
// URI / kind helpers
// ----------------------------------------------------------------------------

fn scheme(uri: &str) -> &str {
    uri.split("://").next().unwrap_or("")
}

fn percent_decode(s: &str) -> String {
    s.replace("%20", " ").replace("%2F", "/")
}

/// Service/host name extracted from a printer URI, for display.
fn uri_service_name(uri: &str) -> String {
    let rest = uri.split("://").nth(1).unwrap_or(uri);
    let head = rest
        .split(['/', '?'])
        .next()
        .unwrap_or(rest)
        .split("._ipp")
        .next()
        .unwrap_or(rest);
    percent_decode(head.trim_end_matches('.'))
}

fn pretty_conn(uri: Option<&str>) -> String {
    let Some(uri) = uri.filter(|u| !u.is_empty()) else {
        return "Local".to_string();
    };
    match scheme(uri) {
        "usb" => "USB".to_string(),
        "dnssd" | "ipp" | "ipps" => format!("{} (IPP)", uri_service_name(uri)),
        "socket" => format!("{} (Raw 9100)", uri_service_name(uri)),
        "lpd" => format!("{} (LPD)", uri_service_name(uri)),
        other if !other.is_empty() => {
            format!("{} ({})", uri_service_name(uri), other.to_uppercase())
        }
        _ => uri.to_string(),
    }
}

// ----------------------------------------------------------------------------
// Tests — fixtures are verbatim from real macOS CUPS output.
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {

    /// A failed or empty capability probe is "unknown", not "cannot duplex":
    /// a duplex job used to pause as unsupported whenever `lpoptions -l` came
    /// back empty for a moment.
    #[test]
    fn fr_040_a_failed_caps_probe_does_not_refuse_duplex() {
        let papers = vec!["A4".to_string()];
        for probe in [None, Some("")] {
            let caps = caps_from_probe(probe, &papers);
            assert!(caps.duplex, "{probe:?}");
            assert_eq!(caps.papers, papers);
            assert!(caps.color_models.is_empty());
        }

        let simplex = caps_from_probe(Some("ColorModel/Color Model: Gray *RGB\n"), &papers);
        assert!(!simplex.duplex, "a real probe without Duplex is believed");
        assert_eq!(simplex.color_models, vec!["Gray", "RGB"]);

        let duplex = caps_from_probe(
            Some("Duplex/2-Sided Printing: *None DuplexNoTumble DuplexTumble\n"),
            &papers,
        );
        assert!(duplex.duplex);
    }
    use super::*;
    use crate::ipc::printers::PrinterKind;

    /// The listing agrees with the engine: a failed probe is "unknown", not
    /// "no duplex, grey only", so the UI does not warn about a printer whose
    /// probe merely failed; a real listing is believed, except that a
    /// missing colour option is unknown rather than "black and white".
    #[test]
    fn fr_040_a_failed_listing_probe_reports_unknown_not_unsupported() {
        for probe in [None, Some("")] {
            let l = listing_caps(probe);
            assert!(l.duplex_unknown && l.color_unknown, "{probe:?}");
            assert!(!l.duplex && !l.color);
        }
        let l = listing_caps(Some(LPOPTIONS_L));
        assert!(l.duplex && l.color);
        assert!(!l.duplex_unknown && !l.color_unknown);

        let simplex = listing_caps(Some("PageSize/Media Size: *A4 Letter\n"));
        assert!(!simplex.duplex && !simplex.duplex_unknown, "believed");
        assert!(simplex.color_unknown, "no ColorModel option: unknown");
        let grey = listing_caps(Some("ColorModel/Color Mode: *Gray\n"));
        assert!(
            !grey.color && !grey.color_unknown,
            "a real grey-only driver"
        );
    }

    const LPOPTIONS_P: &str = "copies=1 device-uri=dnssd://EPSON%20L15160%20Series._ipps._tcp.local./?uuid=cfe92100 finishings=3 marker-colors=#000000,#00FFFF,#FF00FF,#FFFF00 marker-levels=38,52,51,55 marker-names='Black\\ ink,Cyan\\ ink,Magenta\\ ink,Yellow\\ ink' printer-info='EPSON L15160 Series' printer-is-shared=false printer-location printer-make-and-model='EPSON L15160 Series-AirPrint' printer-state=3 printer-state-reasons=media-empty-report";

    const LPOPTIONS_L: &str = "ColorModel/Color Mode: Gray *RGB\nDuplex/2-Sided Printing: None *DuplexNoTumble DuplexTumble\nPageSize/Media Size: A3 A4 *Letter Legal Custom.WIDTHxHEIGHT\nInputSlot/Media Source: auto main alternate rear\ncupsPrintQuality/Quality: *Normal High";

    #[test]
    fn parse_kv_handles_quotes_and_bare_keys() {
        let kv = parse_kv(LPOPTIONS_P);
        assert_eq!(kv["printer-info"], "EPSON L15160 Series");
        assert_eq!(kv["printer-make-and-model"], "EPSON L15160 Series-AirPrint");
        assert_eq!(kv["printer-is-shared"], "false");
        assert_eq!(kv["printer-state"], "3");
        // bare key → empty value
        assert_eq!(kv["printer-location"], "");
        // value with embedded '=' keeps the tail
        assert!(kv["device-uri"].contains("uuid=cfe92100"));
        // escaped spaces unescaped inside quotes
        assert_eq!(
            kv["marker-names"],
            "Black ink,Cyan ink,Magenta ink,Yellow ink"
        );
    }

    #[test]
    fn supplies_map_cmyk_by_hex() {
        let kv = parse_kv(LPOPTIONS_P);
        let s = parse_supplies(
            kv.get("marker-colors").map(String::as_str),
            kv.get("marker-levels").map(String::as_str),
        );
        assert_eq!(s.black, Some(0.38));
        assert_eq!(s.cyan, Some(0.52));
        assert_eq!(s.magenta, Some(0.51));
        assert_eq!(s.yellow, Some(0.55));
    }

    #[test]
    fn caps_color_duplex_papers_trays_dpi() {
        let caps = parse_caps(LPOPTIONS_L);
        assert!(color_capable(&caps)); // RGB present
        assert!(duplex_capable(&caps)); // DuplexNoTumble present
        let p = papers(&caps);
        assert!(p.contains(&"A4".to_string()));
        assert!(!p.iter().any(|v| v.contains("Custom"))); // custom filtered
        assert_eq!(caps["PageSize"].default.as_deref(), Some("Letter"));
        assert_eq!(trays(&caps).len(), 4);
        // this fixture has no Resolution line — degrade to None
        assert_eq!(max_dpi(&caps), None);
    }

    #[test]
    fn status_mapping() {
        assert!(matches!(
            status_from(Some("3"), Some("none")),
            PrinterStatus::Ready
        ));
        assert!(matches!(status_from(Some("4"), None), PrinterStatus::Busy));
        assert!(matches!(status_from(Some("5"), None), PrinterStatus::Error));
        assert!(matches!(
            status_from(Some("3"), Some("offline-report")),
            PrinterStatus::Offline
        ));
    }

    /// A jam or an open door is an error even while CUPS still says "idle";
    /// a queue the user paused is not an error at all.
    #[test]
    fn fr_regression_status_reads_error_reasons_and_paused_queues() {
        for reason in [
            "media-jam-error",
            "cover-open-error",
            "none,toner-empty-error",
        ] {
            assert!(
                matches!(status_from(Some("3"), Some(reason)), PrinterStatus::Error),
                "{reason}"
            );
        }
        assert!(matches!(
            status_from(Some("5"), Some("paused")),
            PrinterStatus::Offline
        ));
        // Informational reasons keep the printer Ready.
        assert!(matches!(
            status_from(Some("3"), Some("media-empty-report,toner-low-warning")),
            PrinterStatus::Ready
        ));
        // A stopped queue that is not merely paused is still an error.
        assert!(matches!(
            status_from(Some("5"), Some("none")),
            PrinterStatus::Error
        ));
    }

    /// Negative IPP marker levels mean "unknown", not "empty".
    #[test]
    fn fr_regression_unknown_marker_levels_are_not_reported_as_empty() {
        let s = parse_supplies(Some("#000000,#00FFFF,#FF00FF,#FFFF00"), Some("-1,-2,-3,40"));
        assert_eq!(s.black, None);
        assert_eq!(s.cyan, None);
        assert_eq!(s.magenta, None);
        assert_eq!(s.yellow, Some(0.40));
    }

    #[test]
    fn jobs_counted_per_dest() {
        let out = "EPSON_L15160_Series-7 user 1024 Mon\nEPSON_L15160_Series-8 user 2048 Mon";
        let jobs = parse_jobs(out);
        assert_eq!(jobs["EPSON_L15160_Series"], 2);
    }

    #[test]
    fn pretty_conn_variants() {
        assert_eq!(pretty_conn(Some("usb://EPSON/L15160")), "USB");
        assert_eq!(
            pretty_conn(Some("dnssd://EPSON%20L15160%20Series._ipps._tcp.local./")),
            "EPSON L15160 Series (IPP)"
        );
        assert_eq!(pretty_conn(None), "Local");
    }

    #[test]
    fn sanitize_queue_name_replaces_illegal_chars() {
        assert_eq!(sanitize_queue_name("Front Desk"), "Front_Desk");
        assert_eq!(sanitize_queue_name("HP/LaserJet"), "HP_LaserJet");
        assert_eq!(sanitize_queue_name("Lab#3"), "Lab_3");
    }

    #[test]
    fn sanitize_queue_name_falls_back_when_empty() {
        assert_eq!(sanitize_queue_name(""), "printer");
        assert_eq!(sanitize_queue_name("   "), "printer");
    }

    #[test]
    fn sanitize_queue_name_caps_length() {
        let out = sanitize_queue_name(&"a".repeat(300));
        assert_eq!(out.chars().count(), 127);
    }

    /// Every character `lpadmin`'s `validate_name` refuses must be mapped,
    /// otherwise a name the form accepted fails at the spooler.
    #[test]
    fn fr_regression_sanitize_queue_name_maps_every_char_lpadmin_rejects() {
        assert_eq!(sanitize_queue_name("Ana's printer"), "Ana_s_printer");
        assert_eq!(sanitize_queue_name("Printer?2"), "Printer_2");
        assert_eq!(sanitize_queue_name(r#"A\B"C"#), "A_B_C");
        assert_eq!(sanitize_queue_name("lab@host"), "lab_host");
        assert_eq!(sanitize_queue_name("tab\there\u{7f}"), "tab_here_");
        assert_eq!(sanitize_queue_name("Impresora España"), "Impresora_Espa_a");
        assert_eq!(sanitize_queue_name("@@@"), "printer");
        for c in sanitize_queue_name("x'\"?#@/\\ é\u{1}").chars() {
            assert!(c.is_ascii_graphic(), "{c:?} would be refused by lpadmin");
        }
    }

    /// The cap is in bytes: a long accented name must stay under 128 octets.
    #[test]
    fn fr_regression_sanitize_queue_name_caps_by_bytes() {
        let out = sanitize_queue_name(&"ñ".repeat(120));
        assert!(out.len() <= 127, "{} bytes", out.len());
    }

    /// CUPS queue names are case-insensitive: `office` must not reach
    /// `lpadmin -p` when `Office` exists, or the existing queue is rewritten.
    #[test]
    fn fr_regression_unique_queue_name_is_case_insensitive() {
        let existing = vec!["Office".to_string(), "OFFICE_2".to_string()];
        assert_eq!(unique_queue_name("office", &existing), "office_3");
        assert_eq!(unique_queue_name("Lab", &existing), "Lab");
    }

    #[test]
    fn unique_queue_name_passes_through_and_suffixes() {
        let existing = vec!["Office".to_string(), "Office_2".to_string()];
        // no collision → unchanged
        assert_eq!(unique_queue_name("Lab", &existing), "Lab");
        // collides with Office and Office_2 → first free suffix is _3
        assert_eq!(unique_queue_name("Office", &existing), "Office_3");
    }

    #[test]
    fn lpadmin_args_selects_model_by_scheme() {
        // Value that follows the `-m` flag in an argument vector.
        fn model_of(args: &[String]) -> Option<&str> {
            let i = args.iter().position(|a| a == "-m")?;
            args.get(i + 1).map(String::as_str)
        }
        let ipp = lpadmin_args("Q", "ipp://host/ipp/print", "Disp").unwrap();
        assert_eq!(model_of(&ipp), Some("everywhere"));
        let dnssd = lpadmin_args("Q", "dnssd://Printer._ipp._tcp.local./", "Disp").unwrap();
        assert_eq!(model_of(&dnssd), Some("everywhere"));
        let sock = lpadmin_args("Q", "socket://10.0.0.5:9100", "Disp").unwrap();
        assert_eq!(model_of(&sock), Some("raw"));
        let lpd = lpadmin_args("Q", "lpd://10.0.0.5/queue", "Disp").unwrap();
        assert_eq!(model_of(&lpd), Some("raw"));
        // unknown scheme is rejected
        assert!(lpadmin_args("Q", "usb://HP/Printer", "Disp").is_err());
    }

    /// FR-092: the unsupported-scheme refusal reaches the UI as a stable
    /// reason (+ the scheme), so it is translated instead of shown in English.
    #[test]
    fn fr_092_unsupported_scheme_is_a_localized_error() {
        let err = lpadmin_args("Q", "usb://HP/Printer", "Disp").unwrap_err();
        let ipc = crate::util::error::IpcError::from(err);
        assert_eq!(ipc.code(), IpcErrorCode::InvalidRequest);
        assert_eq!(ipc.reason(), Some("add_printer_unsupported_scheme"));
        let v = serde_json::to_value(&ipc).unwrap();
        assert_eq!(v["params"]["scheme"], "usb");
    }

    /// Real `lpadmin` round-trip. Ignored by default so it never runs in CI;
    /// exercise it manually with `cargo test -- --ignored add_and_remove`.
    #[test]
    #[ignore]
    fn add_and_remove_real_queue() {
        // A raw socket queue can be created without a reachable device.
        add_printer("Pluryprint Test Queue", "socket://127.0.0.1:9100").expect("add should work");
        // Best-effort cleanup of the queue id sanitize_queue_name would produce.
        let _ = run_ok("lpadmin", &["-x", "Pluryprint_Test_Queue"]);
    }

    /// Verbatim `ippfind -x printf` output from the dev Mac: one EPSON
    /// advertised twice (ipp + ipps) under the same UUID.
    const IPPFIND_EPSON: &str = "ipp://EPSON7E7F58.local:631/ipp/print\tcfe92100-67c4-11d4-a45f-dccd2f7e7f58\tEPSON L15160 Series\tEPSON L15160 Series\nipps://EPSON7E7F58.local:631/ipp/print\tcfe92100-67c4-11d4-a45f-dccd2f7e7f58\tEPSON L15160 Series\tEPSON L15160 Series\n";

    /// The installed EPSON queue's `device-uri` (a `dnssd://` URI, not the
    /// `ipp://` one `ippfind` reports): it must still be recognised.
    #[test]
    fn fr_regression_scan_skips_a_printer_installed_under_a_dnssd_uri() {
        let installed = vec![
            "dnssd://EPSON%20L15160%20Series._ipps._tcp.local./?uuid=cfe92100-67c4-11d4-a45f-dccd2f7e7f58"
                .to_string(),
        ];
        assert!(parse_discovery(IPPFIND_EPSON, &installed).is_empty());

        // Matched by UUID alone (service renamed since install).
        let renamed = vec![
            "dnssd://Old%20Name._ipps._tcp.local./?uuid=CFE92100-67C4-11D4-A45F-DCCD2F7E7F58"
                .to_string(),
        ];
        assert!(parse_discovery(IPPFIND_EPSON, &renamed).is_empty());

        // Matched by host alone (installed by IP with another port).
        let by_host = vec!["ipp://epson7e7f58.local./ipp/print".to_string()];
        assert!(parse_discovery(IPPFIND_EPSON, &by_host).is_empty());
    }

    /// ipp + ipps of one device is ONE row, on ipps, named after the service.
    #[test]
    fn fr_regression_scan_collapses_ipp_and_ipps_of_one_device() {
        let rows = parse_discovery(IPPFIND_EPSON, &[]);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.id, "ipps://EPSON7E7F58.local:631/ipp/print");
        assert_eq!(r.name, "EPSON L15160 Series");
        assert_eq!(r.via, "mDNS · IPPS");

        // Without TXT fields: keyed by host, port stripped from the name.
        let bare = "ipp://lab.local:631/ipp/print\nipps://lab.local:631/ipp/print\n";
        let rows = parse_discovery(bare, &[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "lab.local");
        assert!(rows[0].id.starts_with("ipps://"));
    }

    #[test]
    fn uri_host_and_uuid_helpers() {
        assert_eq!(uri_host("ipp://Host.Local.:631/ipp/print"), "host.local");
        assert_eq!(uri_host("ipp://[fe80::1]:631/ipp"), "fe80::1");
        assert_eq!(uri_uuid("dnssd://x._ipp._tcp.local./?uuid=ABC&x=1"), "abc");
        assert_eq!(uri_uuid("ipp://host/ipp"), "");
    }

    /// Set default and Test page go through this runner so the toast carries
    /// `lp`'s own reason ("The printer or class does not exist.") instead of
    /// a bare "exited with exit status: 1".
    #[test]
    fn fr_regression_spooler_failures_surface_the_tool_stderr() {
        let e = run_ok_capture(
            "sh",
            &["-c", "echo 'lp: no such printer' >&2; exit 1"],
            "spooler_rejected",
        )
        .expect_err("a non-zero exit is a failure");
        // FR-092: the toast's sentence is translated from the reason, and the
        // tool's own words ride along as the technical detail under it.
        let e = crate::util::error::IpcError::from(e);
        assert_eq!(e.code(), IpcErrorCode::PrintFailed);
        assert_eq!(e.reason(), Some("spooler_rejected"));
        assert_eq!(e.message(), "lp: no such printer");
        let wire = serde_json::to_value(&e).unwrap();
        assert_eq!(wire["params"]["detail"], "lp: no such printer");
        assert!(run_ok_capture("sh", &["-c", "exit 0"], "spooler_rejected").is_ok());
    }

    /// FR-092: a CUPS tool that is not there says CUPS is not answering.
    #[test]
    fn fr_092_a_cups_tool_that_cannot_start_is_cups_unavailable() {
        let e = run_ok_capture("/nonexistent/lpoptions", &[], "set_default_failed")
            .expect_err("spawn fails");
        let e = crate::util::error::IpcError::from(e);
        assert_eq!(e.code(), IpcErrorCode::PrintFailed);
        assert_eq!(e.reason(), Some("cups_unavailable"));
        let silent = tool_failure("add_printer_failed", "lpadmin", "exit status: 1", " ");
        assert_eq!(silent.reason, "add_printer_failed");
        assert_eq!(
            silent.params["detail"],
            "`lpadmin` exited with exit status: 1"
        );
    }

    #[test]
    fn kind_inference() {
        // The keyword table itself lives in `platform::mod` and is tested
        // there; this only pins the CUPS call shape (model + queue + URI).
        assert!(matches!(
            infer_kind("Zebra ZD420 ZPL", "labels", None),
            PrinterKind::Label
        ));
        assert!(matches!(
            infer_kind("Generic", "Save_as_PDF", Some("cups-pdf:/")),
            PrinterKind::Virtual
        ));
        assert!(matches!(
            infer_kind("HP LaserJet M404", "office", Some("usb://HP/M404")),
            PrinterKind::Laser
        ));
    }

    /// `ippfind` exits 1 on an empty network; that must stay a successful,
    /// empty scan rather than becoming an error the UI paints red.
    #[test]
    fn fr_classify_ippfind_no_matches_is_an_empty_scan_not_a_failure() {
        assert_eq!(
            classify_ippfind(Some(1), "exit status: 1", String::new(), "").unwrap(),
            ""
        );
        assert_eq!(
            classify_ippfind(Some(0), "exit status: 0", "ipp://a/\n".into(), "").unwrap(),
            "ipp://a/\n"
        );
    }

    /// A broken scan must NOT be reported as "no printers on the network".
    #[test]
    fn fr_classify_ippfind_surfaces_real_failures() {
        let e = classify_ippfind(Some(2), "exit status: 2", String::new(), "bad argument")
            .expect_err("a non-0/1 exit is a failed scan");
        let e = crate::util::error::IpcError::from(e);
        assert_eq!(e.reason(), Some("discovery_failed"));
        assert_eq!(e.message(), "bad argument");

        // Killed by a signal (no exit code) with nothing on stderr.
        let e = classify_ippfind(None, "signal: 9", String::new(), "  ")
            .expect_err("a signalled scan is a failed scan");
        assert!(e.to_string().contains("signal: 9"), "{e}");
    }

    /// Real-environment smoke test: enumeration must not error and must yield
    /// well-formed records (tolerates zero printers on a headless CI runner).
    ///
    /// Ignored by default — it shells out to the host's CUPS install, so its
    /// result depends on the developer's real queues. `docs/08-development/
    /// testing-strategy.md` § "When to skip subprocess tests" requires exactly
    /// this gate.
    ///
    /// CI never runs it, so a failure here never blocks a merge. Run it
    /// locally with `cargo test --manifest-path src-tauri/Cargo.toml --
    /// --ignored list_printers_smoke`.
    /// A missing `lpstat` is `cups_missing`, not an empty printer list; a
    /// CUPS with no queues is still an empty list.
    #[test]
    fn fr_092_no_lpstat_is_cups_missing_not_an_empty_list() {
        let missing = queue_names_or_missing(|| lc_run("/nonexistent/lpstat", &["-e"]))
            .expect_err("no lpstat");
        let e = crate::util::error::IpcError::from(missing);
        assert_eq!(e.code(), IpcErrorCode::DependencyMissing);
        assert_eq!(e.reason(), Some("cups_missing"));
        assert!(e.message().contains("cups-client"), "{}", e.message());

        assert_eq!(
            queue_names_or_missing(|| Ok(" Office\nPDF \n\n".to_string())).unwrap(),
            ["Office", "PDF"]
        );
        assert!(queue_names_or_missing(|| Ok(String::new()))
            .unwrap()
            .is_empty());
        // Any other failure keeps the best-effort empty list.
        assert!(queue_names_or_missing(|| anyhow::bail!("odd"))
            .unwrap()
            .is_empty());
    }

    /// Linux printer settings: the running desktop's own tool first, the
    /// desktop-neutral one next, and the CUPS web interface always last.
    #[test]
    fn printer_settings_follow_the_desktop() {
        let first = |desktop: &str| printer_settings_candidates(desktop)[0].0;
        assert_eq!(first("KDE"), "kcmshell6");
        assert_eq!(first("ubuntu:GNOME"), "gnome-control-center");
        assert_eq!(first("Budgie:GNOME"), "gnome-control-center");
        assert_eq!(first("XFCE"), "system-config-printer");
        assert_eq!(first("X-Cinnamon"), "system-config-printer");
        assert_eq!(first(""), "system-config-printer");
        for desktop in ["KDE", "GNOME", ""] {
            let list = printer_settings_candidates(desktop);
            assert_eq!(list.len(), 5, "{desktop}");
            assert_eq!(
                list.last().unwrap(),
                &("xdg-open", &[CUPS_WEB_PRINTERS][..])
            );
            let kde = list.iter().find(|(p, _)| *p == "kcmshell5").unwrap();
            assert_eq!(kde.1, ["kcm_printer_manager"]);
        }
    }

    /// Inside an AppImage, a host program loses every variable that points
    /// into the mounted image, and keeps the rest.
    #[test]
    fn host_programs_do_not_inherit_the_appimage_libraries() {
        use std::ffi::OsString;
        let appdir = std::path::Path::new("/tmp/.mount_PluryX");
        let vars = [
            ("PATH", "/tmp/.mount_PluryX/usr/bin:/usr/bin:/bin"),
            ("GTK_PATH", "/tmp/.mount_PluryX/usr/lib/gtk-3.0"),
            ("LD_LIBRARY_PATH", "/tmp/.mount_PluryX/usr/lib"),
            ("HOME", "/home/ada"),
            ("APPDIR", "/tmp/.mount_PluryX"),
            ("APPIMAGE", "/home/ada/Pluryprint.AppImage"),
        ]
        .map(|(k, v)| (OsString::from(k), OsString::from(v)));
        let changes = host_env(appdir, vars.into_iter());
        let get = |k: &str| {
            changes
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("PATH"), Some(Some(OsString::from("/usr/bin:/bin"))));
        assert_eq!(get("GTK_PATH"), Some(None));
        assert_eq!(get("LD_LIBRARY_PATH"), Some(None));
        assert_eq!(get("APPDIR"), Some(None));
        assert_eq!(get("APPIMAGE"), Some(None));
        assert_eq!(get("HOME"), None, "untouched");
    }

    #[test]
    #[ignore = "requires a real CUPS install; run with -- --ignored"]
    fn list_printers_smoke() {
        let printers = list_printers().expect("list_printers should not error");
        for p in &printers {
            assert!(!p.id.is_empty(), "every printer needs an id");
            for (ch, lvl) in [
                p.supplies.black,
                p.supplies.cyan,
                p.supplies.magenta,
                p.supplies.yellow,
            ]
            .iter()
            .filter_map(|v| v.map(|x| (x, x)))
            {
                let _ = ch;
                assert!((0.0..=1.0).contains(&lvl), "supply levels are normalized");
            }
        }
        // default, if any, must be one of the listed printers
        if let Some(def) = default_printer().unwrap() {
            assert!(printers.iter().any(|p| p.id == def));
        }
    }
}
