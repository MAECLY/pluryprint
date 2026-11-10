//! Diagnostic & "About" IPC commands.
//!
//! [`app_info`] is the canary that proves the React ↔ Rust round-trip works and
//! that filesystem paths resolve on the host OS; it also carries the
//! external-dependency probe (FR-058), so a screen that needs to know whether
//! SumatraPDF is installed asks the same command it already asks for the
//! version. The rest back the About/Help
//! surface: opening the logs folder, prefilling a GitHub issue, and opening an
//! allow-listed external link. All three shell out via the `opener` plugin
//! Rust-side only, so no JS `opener` capability is needed.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_plugin_opener::OpenerExt;

use crate::engine::deps::{self, DependencyReport};
use crate::util::{
    error::{IpcError, IpcErrorCode},
    paths,
};

/// What this build is and what it sees on this host: version, the resolved
/// app directories and the external-dependency probe. Backs the About page
/// and the diagnostics copy.
#[derive(Debug, Clone, Serialize, Type)]
pub struct AppInfo {
    /// The app version from `Cargo.toml`.
    pub version: String,
    /// Date the binary was built (set by `build.rs`).
    pub build_date: String,
    /// Resolved config directory (`config.toml`, `profiles.toml`).
    pub config_dir: String,
    /// Resolved log directory (the JSONL logs).
    pub log_dir: String,
    /// Resolved data directory (the persisted queue and its spool).
    pub data_dir: String,
    /// Target OS, e.g. `"macos"`, `"windows"`, `"linux"` (`std::env::consts::OS`).
    pub os: String,
    /// Target architecture, e.g. `"x86_64"`, `"aarch64"` (`std::env::consts::ARCH`).
    pub arch: String,
    /// FR-058: every external program the print pipeline can shell out to,
    /// with where it was found (or the remediation message for a missing one).
    ///
    /// It rides on `app_info` rather than a command of its own because this is
    /// exactly what `app_info` is for — "what does this build see on this
    /// host?" — and because the screens that care already fetch it once on
    /// mount. The probe is mostly `is_file` calls (see [`deps::report_all`]).
    /// The one exception is a `convert` met while looking for the Linux HEIF
    /// decoder: it is run (`convert -version`, up to five seconds) to tell
    /// ImageMagick's from GraphicsMagick's ([`deps::is_heif_decoder`]). Its
    /// answer is kept per program file for the life of the process; no
    /// answer (it hung past five seconds or could not start) counts as
    /// "not ImageMagick" for a minute and is then asked again. So the
    /// first call on such a machine can take five seconds, and with a
    /// `convert` that hangs one call a minute does; every other costs a few
    /// `stat`s, and a program replaced or changed is asked again, so the
    /// list is never stale.
    ///
    /// `#[serde(default)]` — as on `AppSettings::meta`, and for the same
    /// reason: the server always sends it, but a test building an `AppInfo` by
    /// hand should not have to enumerate the machine's dependencies to talk
    /// about a version string. Consumers treat an absent list as "not probed",
    /// which raises no warning.
    #[serde(default)]
    pub dependencies: Vec<DependencyReport>,
    /// Absolute path of the running executable — which is also the headless
    /// CLI (FR-008). The installers do not put it on `PATH`, so a command the
    /// UI offers to copy has to name this path for the paste to work.
    /// `None` when the OS will not say (the UI then explains the requirement
    /// instead of offering a command that cannot run). Inside a Linux
    /// AppImage it is the AppImage file itself (`$APPIMAGE`), wherever it
    /// was saved and whatever it is called now, never the temporary mount
    /// the binary runs from.
    #[serde(default)]
    pub executable: Option<String>,
    /// The package this build was bundled as, from Tauri's own stamp
    /// (`tauri::utils::platform::bundle_type`): `"appimage"`, `"deb"`,
    /// `"rpm"`, `"nsis"`, `"msi"` or `"app"`; `None` for a build that was
    /// not bundled (`cargo run`). The updater reads the same stamp.
    #[serde(default)]
    pub bundle_type: Option<String>,
}

/// The executable `app_info` reports: the AppImage file when `appimage`
/// (`$APPIMAGE`) names one, else `exe`. Pure.
pub fn reported_executable(
    exe: Option<String>,
    appimage: Option<&std::ffi::OsStr>,
) -> Option<String> {
    let image = appimage
        .map(std::path::Path::new)
        .filter(|p| p.is_absolute())
        .map(|p| p.to_string_lossy().into_owned());
    image.or(exe)
}

/// FR-058 canary: report the version, resolved paths and dependency probe.
/// Never fails in practice; the `Result` keeps the command shape uniform.
#[tauri::command]
#[specta::specta]
pub fn app_info() -> Result<AppInfo, IpcError> {
    Ok(AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        build_date: env!("BUILD_DATE").to_string(),
        config_dir: paths::config_dir().display().to_string(),
        log_dir: paths::log_dir().display().to_string(),
        data_dir: paths::data_dir().display().to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        dependencies: deps::report_all(),
        executable: reported_executable(
            std::env::current_exe()
                .ok()
                .map(|p| p.canonicalize().unwrap_or(p))
                .map(|p| without_verbatim_prefix(&p.to_string_lossy())),
            if cfg!(target_os = "linux") {
                std::env::var_os("APPIMAGE")
            } else {
                None
            }
            .as_deref(),
        ),
        bundle_type: tauri::utils::platform::bundle_type()
            .map(|kind| format!("{kind:?}").to_ascii_lowercase()),
    })
}

/// `path` without the `\\?\` prefix `canonicalize` adds on Windows. Pure.
///
/// The prefix is valid but alien: the Home screen's "Run from CLI" command
/// showed `& '\\?\C:\Users\…\pluryprint.exe'`, which nobody would type and
/// some tools reject. `\\?\UNC\server\share` becomes `\\server\share`.
pub fn without_verbatim_prefix(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        path.to_string()
    }
}

/// Reveal the app's log directory in the OS file browser (creating it first if
/// it does not exist yet, so the command never no-ops on a fresh install).
#[tauri::command]
#[specta::specta]
pub fn open_logs_folder(app: tauri::AppHandle) -> Result<(), IpcError> {
    let dir = paths::log_dir();
    paths::ensure_dir(&dir)?;
    app.opener()
        .open_path(dir.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|e| {
            IpcError::localized(
                IpcErrorCode::Internal,
                "open_logs_failed",
                format!("could not open the logs folder: {e}"),
            )
        })
}

/// Open a prefilled "new issue" page on the project's GitHub, seeding the body
/// with the app version, build date, and OS/arch followed by the user's text.
#[tauri::command]
#[specta::specta]
pub fn report_issue(app: tauri::AppHandle, text: String) -> Result<(), IpcError> {
    let url = issue_url(
        env!("CARGO_PKG_REPOSITORY"),
        env!("CARGO_PKG_VERSION"),
        env!("BUILD_DATE"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        &text,
    );
    app.opener().open_url(url, None::<&str>).map_err(|e| {
        IpcError::localized(
            IpcErrorCode::Internal,
            "open_link_failed",
            format!("could not open the issue page: {e}"),
        )
    })
}

/// Open an external link in the default browser, restricted to an allow-list of
/// project hosts (github.com, maecly.com, and `*.maecly.com` over HTTPS). Any
/// other URL is rejected with [`IpcError::InvalidRequest`] rather than opened.
#[tauri::command]
#[specta::specta]
pub fn open_external(app: tauri::AppHandle, url: String) -> Result<(), IpcError> {
    if !allowed_external(&url) {
        return Err(IpcError::localized(
            IpcErrorCode::InvalidRequest,
            "url_not_allowed",
            format!("refusing to open a non-allowlisted URL: {url}"),
        ));
    }
    app.opener().open_url(url, None::<&str>).map_err(|e| {
        IpcError::localized(
            IpcErrorCode::Internal,
            "open_link_failed",
            format!("could not open the link: {e}"),
        )
    })
}

/// An error raised in the webview, forwarded so it lands in the log file the
/// error screens point the user at (NFR-033, FR-113).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Type)]
pub struct FrontendErrorReport {
    /// Where it was caught: `"error-boundary"`, `"window.error"`,
    /// `"unhandledrejection"`, or a short call-site label.
    pub source: String,
    /// The error's message.
    pub message: String,
    /// The JS stack and/or React component stack, when there is one.
    pub stack: Option<String>,
}

/// Upper bounds on what one report may write, so a runaway error loop (or a
/// megabyte-long stack) cannot fill the log on its own.
const FRONTEND_SOURCE_MAX: usize = 64;
const FRONTEND_MESSAGE_MAX: usize = 2_000;
const FRONTEND_STACK_MAX: usize = 8_000;

/// Truncate `s` to at most `max` bytes on a char boundary, marking the cut.
fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [truncated]", &s[..end])
}

/// Bound every field of a report. Pure, for testing.
fn sanitize_frontend_error(report: FrontendErrorReport) -> FrontendErrorReport {
    let source = clip(report.source.trim(), FRONTEND_SOURCE_MAX);
    FrontendErrorReport {
        source: if source.is_empty() {
            "unknown".to_string()
        } else {
            source
        },
        message: clip(&report.message, FRONTEND_MESSAGE_MAX),
        stack: report
            .stack
            .filter(|s| !s.trim().is_empty())
            .map(|s| clip(&s, FRONTEND_STACK_MAX)),
    }
}

/// Write a webview error into the structured log.
///
/// Before this existed the error screen said "the logs folder has the details"
/// while the details only ever reached the devtools console. Infallible by
/// design: the caller is already handling a failure.
#[tauri::command]
#[specta::specta]
pub fn log_frontend_error(report: FrontendErrorReport) -> Result<(), IpcError> {
    let r = sanitize_frontend_error(report);
    tracing::error!(
        event = "frontend_error",
        source = %r.source,
        stack = r.stack.as_deref().unwrap_or(""),
        "{}",
        r.message
    );
    Ok(())
}

/// Build the prefilled GitHub issue URL. `repo` is the `https://github.com/o/r`
/// base; the body is percent-encoded (`NON_ALPHANUMERIC`) so newlines, spaces
/// and `#` survive intact in the query string. Pure, for testing.
fn issue_url(
    repo: &str,
    version: &str,
    build_date: &str,
    os: &str,
    arch: &str,
    text: &str,
) -> String {
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};

    let body =
        format!("- Pluryprint: {version}\n- Build date: {build_date}\n- OS: {os} {arch}\n\n{text}");
    let enc = |s: &str| utf8_percent_encode(s, NON_ALPHANUMERIC).to_string();
    format!("{repo}/issues/new?title={}&body={}", enc(""), enc(&body))
}

/// Whether `url` is an allow-listed external link: HTTPS to exactly
/// `github.com`, `maecly.com`, or any `*.maecly.com` subdomain.
///
/// The URL is parsed with the WHATWG-compliant `url` crate, so the host we
/// compare is the host a browser would visit. Hand-parsing the authority was
/// not enough: browsers treat `\` as a path separator for `https`, so
/// `https://evil.example\.maecly.com/` looks like a `*.maecly.com` authority
/// to a naive split but visits `evil.example`. Belt and braces, any backslash,
/// whitespace or control character is rejected before parsing, as are
/// userinfo (`https://github.com@evil.example`) and explicit ports. Pure, for
/// testing.
fn allowed_external(raw: &str) -> bool {
    if raw
        .chars()
        .any(|c| c == '\\' || c.is_whitespace() || c.is_control())
    {
        return false;
    }
    let Ok(parsed) = url::Url::parse(raw) else {
        return false;
    };
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
    {
        return false;
    }
    let Some(url::Host::Domain(host)) = parsed.host() else {
        return false;
    };
    // The parser already lower-cases and IDNA-normalises the host.
    host == "github.com" || host == "maecly.com" || host.ends_with(".maecly.com")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inside an AppImage the CLI command must name the AppImage file, not
    /// the temporary mount the binary runs from (which vanishes on exit and
    /// whose name the UI could only guess).
    /// Unix only: an AppImage exists on Linux alone, and these are Unix paths
    /// (on Windows `/tmp/...` is not even absolute).
    #[cfg(unix)]
    #[test]
    fn fr_008_an_appimage_reports_its_own_file_as_the_executable() {
        use std::ffi::OsStr;
        let mount = Some("/tmp/.mount_PluryX/usr/bin/pluryprint".to_string());
        assert_eq!(
            reported_executable(
                mount.clone(),
                Some(OsStr::new("/home/u/Apps/plury.AppImage"))
            ),
            Some("/home/u/Apps/plury.AppImage".to_string())
        );
        assert_eq!(reported_executable(mount.clone(), None), mount);
        assert_eq!(
            reported_executable(mount.clone(), Some(OsStr::new(""))),
            mount
        );
        assert_eq!(
            reported_executable(mount.clone(), Some(OsStr::new("x.AppImage"))),
            mount
        );
    }

    /// FR-008: the command the UI offers to copy names the executable the way
    /// a user would type it.
    #[test]
    fn fr_008_the_offered_executable_has_no_verbatim_prefix() {
        assert_eq!(
            without_verbatim_prefix(r"\\?\C:\Users\a\AppData\Local\Pluryprint\pluryprint.exe"),
            r"C:\Users\a\AppData\Local\Pluryprint\pluryprint.exe"
        );
        assert_eq!(
            without_verbatim_prefix(r"\\?\UNC\srv\share\m.exe"),
            r"\\srv\share\m.exe"
        );
        assert_eq!(
            without_verbatim_prefix("/Applications/M.app/x"),
            "/Applications/M.app/x"
        );
    }

    #[test]
    fn issue_url_encodes_spaces_newlines_and_hash() {
        let url = issue_url(
            env!("CARGO_PKG_REPOSITORY"),
            "0.0.1",
            "2026-07-12",
            "macos",
            "aarch64",
            "crash on #3 with two files",
        );
        assert!(url.starts_with("https://github.com/MAECLY/pluryprint/issues/new?title=&body="));
        // Spaces → %20, newlines → %0A, '#' → %23 (NON_ALPHANUMERIC encodes all).
        assert!(url.contains("%20"), "spaces encoded: {url}");
        assert!(url.contains("%0A"), "newlines encoded: {url}");
        assert!(url.contains("%23"), "'#' encoded: {url}");
        // The environment block is present (percent-encoded).
        assert!(url.contains("Pluryprint"));
        assert!(!url.contains(' '), "no raw spaces in the URL");
    }

    fn report(source: &str, message: &str, stack: Option<&str>) -> FrontendErrorReport {
        FrontendErrorReport {
            source: source.into(),
            message: message.into(),
            stack: stack.map(str::to_string),
        }
    }

    #[test]
    fn nfr_033_frontend_error_report_is_bounded() {
        let huge = "é".repeat(10_000); // multi-byte: the cut must respect chars
        let r = sanitize_frontend_error(report("error-boundary", &huge, Some(&huge)));
        assert!(
            r.message.len() <= FRONTEND_MESSAGE_MAX + 20,
            "{}",
            r.message.len()
        );
        assert!(r.message.ends_with("[truncated]"));
        assert!(r.stack.unwrap().len() <= FRONTEND_STACK_MAX + 20);
    }

    #[test]
    fn nfr_033_frontend_error_report_keeps_small_reports_verbatim() {
        let r = sanitize_frontend_error(report(" window.error ", "boom", Some("at x")));
        assert_eq!(r, report("window.error", "boom", Some("at x")));
        // An empty source or a blank stack is normalised, not logged as noise.
        let r = sanitize_frontend_error(report("", "boom", Some("   ")));
        assert_eq!(r, report("unknown", "boom", None));
    }

    #[test]
    fn nfr_033_log_frontend_error_never_fails() {
        assert!(log_frontend_error(report("test", "boom", None)).is_ok());
    }

    #[test]
    fn allowed_external_accepts_project_hosts() {
        assert!(allowed_external("https://github.com/MAECLY/pluryprint"));
        assert!(allowed_external("https://github.com"));
        assert!(allowed_external("https://maecly.com/"));
        assert!(allowed_external("https://sub.maecly.com/path?q=1#frag"));
        assert!(allowed_external("https://GitHub.com/MAECLY")); // case-insensitive host
    }

    /// Every link the About screen can open is derived from the configured
    /// repository URL (mirrored frontend-side in `src/lib/repo.ts`), so the
    /// allow-list has to accept it exactly as configured — capitalized owner
    /// segment included. The rule is host-only and the host is lowercased before
    /// comparing, so the path's case never matters.
    #[test]
    fn allowed_external_accepts_the_configured_repository_links() {
        let repo = env!("CARGO_PKG_REPOSITORY");
        assert!(allowed_external(repo));
        assert!(allowed_external(&format!("{repo}/releases")));
        assert!(allowed_external(&format!("{repo}/blob/main/LICENSE")));
        assert!(allowed_external(&format!("{repo}/tree/main/docs")));
        assert!(allowed_external(&format!("{repo}/releases/tag/v9.9.9")));
        assert!(allowed_external(&issue_url(
            repo,
            "0.0.1",
            "2026-07-12",
            "windows",
            "x86_64",
            "hello"
        )));
    }

    #[test]
    fn allowed_external_rejects_everything_else() {
        let repo = env!("CARGO_PKG_REPOSITORY");
        // The real repository over plain http, or on a look-alike host, is still
        // refused: fixing the slug must not widen the rule.
        assert!(!allowed_external(&repo.replace("https://", "http://")));
        assert!(!allowed_external(
            &repo.replace("github.com", "github.com.evil.com")
        ));
        assert!(!allowed_external(&repo.replace(
            "https://github.com",
            "https://github.com@evil.example"
        )));
        assert!(!allowed_external("http://github.com")); // not https
        assert!(!allowed_external("https://evil-github.com"));
        assert!(!allowed_external("https://github.com.evil.com"));
        assert!(!allowed_external("https://user@github.com")); // userinfo trick
        assert!(!allowed_external("https://github.com:8080")); // port
        assert!(!allowed_external("https://notmaecly.com"));
        assert!(!allowed_external("ftp://maecly.com"));
        assert!(!allowed_external("https://"));
        // Only the two project hosts are allowed — not the API host the update
        // check talks to, and not raw.githubusercontent.com.
        assert!(!allowed_external("https://api.github.com/repos/MAECLY"));
        assert!(!allowed_external(
            "https://raw.githubusercontent.com/MAECLY"
        ));
    }

    #[test]
    fn allowed_external_rejects_backslash_and_whitespace_authority_tricks() {
        // Browsers read `\` as `/` for https, so this visits evil.example.
        assert!(!allowed_external("https://evil.example\\.maecly.com/"));
        assert!(!allowed_external("https://evil.example\\@github.com/"));
        assert!(!allowed_external("https://github.com/\\evil"));
        // Whitespace and control characters never reach the opener.
        assert!(!allowed_external("https://github.com /x"));
        assert!(!allowed_external("https://github.com\t.evil.example/"));
        assert!(!allowed_external("https://github.com\n"));
        assert!(!allowed_external(" https://github.com"));
        assert!(!allowed_external("https://github.com:8443/"));
        // IP literals are never project hosts.
        assert!(!allowed_external("https://127.0.0.1/"));
        assert!(!allowed_external("https://[::1]/"));
    }
}
