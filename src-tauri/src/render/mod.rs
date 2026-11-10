//! HTML → PDF through the operating system's own web view (FR-055, D15):
//! WebView2 `PrintToPdf` on Windows, `WKWebView` printing on macOS, WebKitGTK
//! printing to a file on Linux ([`linux`]). Runs only
//! inside the render worker ([`crate::worker::render`]), never in the app or
//! the CLI process itself, so a hostile page that crashes or wedges the web
//! view takes the worker down, not Pluryprint.
//!
//! The engine ([`crate::engine::convert::html`]) stages the page it wants
//! printed (the user's HTML, or the HTML made from Markdown or text) in the
//! job's work directory and hands the worker a [`RenderRequest`].
//!
//! **The boundary is the web view's network layer** (round 6). What a page
//! may fetch is decided on the exact URL the web view resolved, at the point
//! it would fetch it, never by a text pre-scan of the HTML (a second parser
//! always disagrees with the real one somewhere; security notes 5-7). The
//! worker loads the staged page with:
//!
//! - **no script**: the web view's own switch (`IsScriptEnabled = false`,
//!   `allowsContentJavaScript = false`), on top of the page's
//!   `script-src 'none'` policy;
//! - **an allow-list of fetches**: on Windows a WebView2
//!   `WebResourceRequested` filter on `*` for every resource kind answers
//!   **every** request itself ([`is_allowed_request`]): the private origin
//!   ([`DOC_HOST`]) from the staged page and the document's folder
//!   ([`serve`]); `data:`, `blob:` and `about:blank` load as they are;
//!   anything else (another host, `http`, `ws`, `file:`, a UNC path) gets a
//!   synthetic 403 and is written to the worker's stderr (the job's
//!   `render.log`). On macOS (round 7) the worker answers its own private
//!   scheme, [`MAC_DOC_SCHEME`] on [`DOC_HOST`], with a `WKURLSchemeHandler`
//!   that serves the staged page and the document's folder the same way
//!   ([`resolve`], [`open_in_folder`]), and a `WKContentRuleList`
//!   ([`webkit_rule_list`]) blocks every other load: every `file:` URL and
//!   every network scheme; only that origin, `data:`, `blob:` and
//!   `about:blank` pass. Linux does the same with WebKitGTK's own pieces: a
//!   URI scheme registered on the web context answers the private scheme,
//!   and a `WebKitUserContentFilter` compiled from the same rule list blocks
//!   the rest;
//! - **no navigation**: every navigation other than the worker's own load of
//!   the page (a `<meta http-equiv=refresh>`, a frame, a link target, a new
//!   window) is cancelled, and refused ones are logged;
//! - **no file share**: a reference such as `//host/x.png` or
//!   `file://host/share/x.png` would make Windows open `\\host\share` over SMB
//!   or WebDAV and hand the user's NTLM credentials to that host. On Windows
//!   the page is therefore not loaded from `file:` at all: the worker serves
//!   it, and the document's folder, itself on one private `https` origin,
//!   from which Chromium refuses every `file:` sub-resource, and a
//!   protocol-relative reference resolves to `https:`, which the filter
//!   refuses. Every path it serves is checked by the shared share predicate
//!   ([`crate::engine::convert::local_ref`]). On macOS and Linux the page is
//!   not loaded from `file:` either, and the rule list refuses every `file:`
//!   URL.
//! - **one origin**: the staged page and the document's folder share that
//!   origin, so same-folder web fonts (CORS-mode requests) and external SVG
//!   `<use>` (same-origin only) load as they do from the user's folder.
//! - **only the page's folder readable**: the worker serves the staged page
//!   and files inside the document's folder, nothing else, and checks each
//!   file on the handle it reads from: on Windows its final path
//!   (`GetFinalPathNameByHandleW`) must lie inside the folder; on macOS and
//!   Linux [`open_in_folder`] opens it component by component without
//!   following a link out of the folder (macOS then also checks its
//!   `F_GETPATH`). No web view reads a document file by itself. (Round 6 confined WebKit with a
//!   `file:` URL prefix only; round 7 showed a symbolic link inside the
//!   folder passed that prefix and WebKit followed it anywhere.)
//! - **bounded**: the worker gives up by itself after the job's conversion
//!   timeout ([`RenderRequest::timeout_secs`], none when the setting is 0),
//!   printing [`TIMEOUT_MARKER`] so the engine reports a timeout (terminal),
//!   not a render failure it would retry.
//!
//! The page size comes from the job ([`RenderRequest::paper_in`]) and is set
//! natively on the print settings, so it does not depend on the page's CSS.
//!
//! **Linux needs a display.** WebKitGTK is a GTK widget: without an X11 or
//! Wayland session (a headless `pluryprint print` over SSH, a service) the
//! worker cannot start it. It then fails at once with [`NO_DISPLAY_MARKER`]
//! on stderr instead of a GTK error; [`display_available`] tells the caller
//! beforehand.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod webkit;
#[cfg(windows)]
pub mod windows;

/// The private origin the staged page and its document's folder are served
/// from on Windows. A reserved name (RFC 2606), so it can never be a real
/// site; the worker answers every request to it itself ([`serve`]), without
/// any network or DNS.
pub const DOC_HOST: &str = "doc.pluryprint.example";

/// The folder, on [`DOC_HOST`], the staged page is served under. A name no
/// document folder is expected to have; the page wins over a file there.
pub const STAGE_PATH: &str = "/.pluryprint-stage/";

/// The private URL scheme the WebKit workers (macOS and Linux) serve the
/// staged page and the document's folder on
/// (`pluryprint-doc://`[`DOC_HOST`]`/…`), through their own scheme handler
/// (round 7; module docs). WebKit hands every request on it to the worker
/// and never reads a `file:` URL for the page. Named for the platform that
/// had it first; Linux uses the same scheme, so the page's policy
/// ([`crate::engine::convert::html::page_csp`]) names one scheme for both.
pub const MAC_DOC_SCHEME: &str = "pluryprint-doc";

/// The scheme the staged page and its document's folder are served on by
/// this platform's worker: `https` on Windows (WebView2's request filter),
/// [`MAC_DOC_SCHEME`] elsewhere (WebKit's scheme handler, on macOS and
/// Linux).
pub const fn doc_scheme() -> &'static str {
    if cfg!(windows) {
        "https"
    } else {
        MAC_DOC_SCHEME
    }
}

/// The line a render worker prints on stderr when it gave up because the
/// conversion timeout passed, so the engine reports a timeout.
pub const TIMEOUT_MARKER: &str = "pluryprint-render-timeout";

/// The line a render worker prints on stderr when it cannot start its web
/// view because there is no graphical session to start it in (Linux without
/// `DISPLAY` or `WAYLAND_DISPLAY`, or with one it cannot open). Retrying
/// does not help, so the engine can report it as its own terminal reason.
pub const NO_DISPLAY_MARKER: &str = "pluryprint-render-no-display";

/// Whether this process can start the render worker's web view: always on
/// Windows and macOS; on Linux only inside an X11 or Wayland session
/// (`DISPLAY` or `WAYLAND_DISPLAY` set and not empty). The worker inherits
/// this process' environment, so the answer holds for it too. It says
/// nothing about whether that display can actually be opened; the worker
/// reports that one itself ([`NO_DISPLAY_MARKER`]).
pub fn display_available() -> bool {
    if cfg!(target_os = "linux") {
        display_in(
            std::env::var_os("DISPLAY").as_deref(),
            std::env::var_os("WAYLAND_DISPLAY").as_deref(),
        )
    } else {
        true
    }
}

/// [`display_available`] for given values of `DISPLAY` and
/// `WAYLAND_DISPLAY` (Linux rules). Pure.
pub fn display_in(display: Option<&OsStr>, wayland: Option<&OsStr>) -> bool {
    let set = |v: Option<&OsStr>| v.is_some_and(|v| !v.is_empty());
    set(display) || set(wayland)
}

/// What one render worker run does: print `page` to `output`.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderRequest {
    /// The staged HTML page to load. It lives alone in its own folder, which
    /// is served (Windows) or readable (macOS) for the run.
    pub page: PathBuf,
    /// The folder relative references resolve against (the user's document's
    /// folder); the only other folder the page may read from.
    pub base_dir: PathBuf,
    /// Where the PDF is written.
    pub output: PathBuf,
    /// A private, throwaway folder for the web view's own data (profile,
    /// caches), inside the job's work directory.
    pub data_dir: PathBuf,
    /// Paper width × height in inches, or `None` for the web view's default
    /// (US Letter) when the job's paper is `auto` or unknown.
    pub paper_in: Option<(f64, f64)>,
    /// The job's conversion timeout, in seconds: the worker gives up by
    /// itself once it passes. `None` (the setting is 0): no limit, the
    /// job's cancel flag still stops it.
    pub timeout_secs: Option<u64>,
}

const ARG_PAGE: &str = "--page";
const ARG_BASE: &str = "--base";
const ARG_OUT: &str = "--out";
const ARG_DATA: &str = "--data";
const ARG_PAPER: &str = "--paper-in";
const ARG_TIMEOUT: &str = "--timeout-secs";

impl RenderRequest {
    /// The worker's argument vector for this request (after
    /// `--worker render-pdf`). Paths travel as `OsString`, never through a
    /// shell, so any file name survives.
    pub fn to_args(&self) -> Vec<OsString> {
        let mut args = vec![
            OsString::from(ARG_PAGE),
            self.page.clone().into_os_string(),
            OsString::from(ARG_BASE),
            self.base_dir.clone().into_os_string(),
            OsString::from(ARG_OUT),
            self.output.clone().into_os_string(),
            OsString::from(ARG_DATA),
            self.data_dir.clone().into_os_string(),
        ];
        if let Some((w, h)) = self.paper_in {
            args.push(OsString::from(ARG_PAPER));
            args.push(OsString::from(format!("{w}x{h}")));
        }
        if let Some(secs) = self.timeout_secs.filter(|s| *s > 0) {
            args.push(OsString::from(ARG_TIMEOUT));
            args.push(OsString::from(secs.to_string()));
        }
        args
    }

    /// How long the worker may run before it gives up by itself.
    pub fn limit(&self) -> Option<std::time::Duration> {
        self.timeout_secs
            .filter(|s| *s > 0)
            .map(std::time::Duration::from_secs)
    }

    /// Parse the worker's arguments back into a request. Every path is
    /// required; the paper is optional and must be two positive numbers.
    pub fn parse(args: &[OsString]) -> Result<Self, String> {
        let mut page = None;
        let mut base = None;
        let mut out = None;
        let mut data = None;
        let mut paper = None;
        let mut timeout = None;
        let mut it = args.iter();
        while let Some(flag) = it.next() {
            let flag = flag.to_string_lossy();
            let value = it
                .next()
                .ok_or_else(|| format!("{flag} needs a value"))?
                .as_os_str();
            match flag.as_ref() {
                ARG_PAGE => page = Some(PathBuf::from(value)),
                ARG_BASE => base = Some(PathBuf::from(value)),
                ARG_OUT => out = Some(PathBuf::from(value)),
                ARG_DATA => data = Some(PathBuf::from(value)),
                ARG_PAPER => paper = Some(parse_paper(value)?),
                ARG_TIMEOUT => {
                    let secs = value
                        .to_string_lossy()
                        .parse::<u64>()
                        .map_err(|_| format!("{ARG_TIMEOUT} must be a number of seconds"))?;
                    timeout = (secs > 0).then_some(secs);
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }
        let need = |v: Option<PathBuf>, name: &str| v.ok_or_else(|| format!("{name} is required"));
        Ok(Self {
            page: need(page, ARG_PAGE)?,
            base_dir: need(base, ARG_BASE)?,
            output: need(out, ARG_OUT)?,
            data_dir: need(data, ARG_DATA)?,
            paper_in: paper,
            timeout_secs: timeout,
        })
    }
}

/// Start a thread that ends the worker once `limit` passes, printing
/// [`TIMEOUT_MARKER`] first. No limit: no watchdog (the engine's cancel flag
/// still kills the worker).
pub fn start_watchdog(limit: Option<std::time::Duration>) {
    let Some(limit) = limit else { return };
    std::thread::spawn(move || {
        std::thread::sleep(limit);
        eprintln!(
            "{TIMEOUT_MARKER}: the render did not finish within {}s",
            limit.as_secs()
        );
        std::process::exit(crate::worker::EXIT_FAILED);
    });
}

fn parse_paper(value: &OsStr) -> Result<(f64, f64), String> {
    let text = value.to_string_lossy();
    let parsed = text.split_once('x').and_then(|(w, h)| {
        let w = w.parse::<f64>().ok()?;
        let h = h.parse::<f64>().ok()?;
        (w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0).then_some((w, h))
    });
    parsed.ok_or_else(|| format!("{ARG_PAPER} must be <width>x<height> in inches, got {text}"))
}

/// The `<base href>` a staged page uses so its relative references resolve
/// against `base_dir` in the web view that prints it: the private
/// [`DOC_HOST`] origin on [`doc_scheme`] (module docs), which the worker maps
/// to `base_dir` itself. Always ends with `/`.
pub fn base_href(base_dir: &Path) -> String {
    let _ = base_dir;
    format!("{}://{DOC_HOST}/", doc_scheme())
}

/// Whether `uri` is inline content the web view resolves without any fetch:
/// a `data:` or `blob:` URI, or `about:blank`. Pure.
fn is_inline(uri: &str) -> bool {
    let lower = uri.trim().to_ascii_lowercase();
    lower.starts_with("data:") || lower.starts_with("blob:") || lower == "about:blank"
}

/// Whether a request the page makes may be served: inline content (`data:`,
/// `blob:`, `about:blank`) or the private origin of this run. Everything
/// else (`http`, `https` to any other host, `ws`, `ftp`, `file:` with or
/// without a host, a UNC path, custom schemes) is refused. Case-insensitive
/// on the scheme and host. Pure; used by the Windows request filter and
/// pinned by tests on every OS.
pub fn is_allowed_request(uri: &str) -> bool {
    if is_inline(uri) {
        return true;
    }
    let lower = uri.trim().to_ascii_lowercase();
    let prefix = format!("https://{DOC_HOST}");
    lower
        .strip_prefix(&prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// The URL the staged `page` is loaded from on Windows: under
/// [`STAGE_PATH`] on [`DOC_HOST`], the document folder's own origin. Pure.
pub fn page_url(page: &Path) -> String {
    page_url_on("https", page)
}

/// [`page_url`] on `scheme` ([`doc_scheme`]). Pure.
pub fn page_url_on(scheme: &str, page: &Path) -> String {
    let name = page
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "page.html".into());
    const KEEP: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'.')
        .remove(b'-')
        .remove(b'_');
    let encoded = percent_encoding::utf8_percent_encode(&name, KEEP);
    format!("{scheme}://{DOC_HOST}{STAGE_PATH}{encoded}")
}

/// What the Windows request filter does with a request for `uri` (module
/// docs): `None` lets inline content (`data:`, `blob:`, `about:blank`) load
/// as it is; every other request is answered by the worker itself, with
/// [`serve`] on the private origin and [`Served::Refused`] (a synthetic 403)
/// for everything else. No request is ever left to the network. Pure but for
/// [`serve`]'s disk reads.
pub fn answer_request(uri: &str, page: &Path, base_dir: &Path) -> Option<Served> {
    if is_inline(uri) {
        None
    } else if is_allowed_request(uri) {
        Some(serve(uri, page, base_dir))
    } else {
        Some(Served::Refused)
    }
}

/// Most refused requests or navigations one render writes to stderr, so a
/// page that makes thousands cannot flood the job's `render.log`.
pub const MAX_LOGGED_REFUSALS: usize = 200;

/// Write one refusal (`what` is `request` or `navigation`) to stderr, the
/// job's `render.log`, up to [`MAX_LOGGED_REFUSALS`] per run.
pub fn log_refusal(what: &str, uri: &str) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static LOGGED: AtomicUsize = AtomicUsize::new(0);
    let n = LOGGED.fetch_add(1, Ordering::Relaxed);
    if n < MAX_LOGGED_REFUSALS {
        let shown: String = uri.chars().take(300).collect();
        eprintln!("pluryprint render: {what} refused: {shown}");
    } else if n == MAX_LOGGED_REFUSALS {
        eprintln!("pluryprint render: further refusals are not logged");
    }
}

/// What the Windows worker answers a request to [`DOC_HOST`] with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Served {
    /// The staged page itself.
    Page(PathBuf),
    /// A file inside the document's folder (canonical path).
    File(PathBuf),
    /// Nothing there (or a folder): 404.
    NotFound,
    /// Not this origin, or a path that tries to leave the folder or reach a
    /// share: 403.
    Refused,
}

/// Where a request on the document's private origin points, before the disk
/// is touched ([`resolve`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// The staged page.
    Page,
    /// A path inside the document's folder, as its decoded segments (none is
    /// empty, `.`, `..`, or holds a separator, `:` or a control character).
    InFolder(Vec<String>),
    /// Nothing can be there (an empty segment, a trailing `/`): 404.
    NotFound,
    /// Not the private origin on `scheme`, or a path that tries to leave the
    /// folder: 403.
    Refused,
}

/// Map a request `uri` on `scheme`://[`DOC_HOST`] to what it points at
/// (module docs): the staged `page` at [`page_url_on`], otherwise a path
/// inside the document's folder, never one that leaves it (`..`, an absolute
/// or drive path, a stream name). The query and fragment are ignored. Pure;
/// shared by the Windows filter ([`serve`]) and the macOS scheme handler.
pub fn resolve(scheme: &str, uri: &str, page: &Path) -> Resolved {
    let Ok(url) = url::Url::parse(uri.trim()) else {
        return Resolved::Refused;
    };
    let same_origin = url.scheme().eq_ignore_ascii_case(scheme)
        && url
            .host_str()
            .is_some_and(|h| h.eq_ignore_ascii_case(DOC_HOST))
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none();
    if !same_origin {
        return Resolved::Refused;
    }
    let page_path =
        url::Url::parse(&page_url_on(scheme, page)).map_or(String::new(), |u| u.path().into());
    if url.path() == page_path {
        return Resolved::Page;
    }
    let Some(segments) = url.path_segments() else {
        return Resolved::Refused;
    };
    let mut out = Vec::new();
    for raw in segments {
        let segment = percent_encoding::percent_decode_str(raw).decode_utf8_lossy();
        if segment.is_empty() {
            return Resolved::NotFound;
        }
        let bad = segment == "."
            || segment == ".."
            || segment
                .chars()
                .any(|c| matches!(c, '/' | '\\' | ':' | '\0') || c.is_control());
        if bad {
            return Resolved::Refused;
        }
        out.push(segment.into_owned());
    }
    Resolved::InFolder(out)
}

/// Map a request `uri` to what is served (module docs): the staged `page` at
/// [`page_url`], otherwise a file inside `base_dir`, never a path that
/// leaves it (`..`, an absolute or drive path, a stream name, a share by
/// [`crate::engine::convert::local_ref`]). The query and fragment are
/// ignored. Touches the disk only for a path that passed every check.
pub fn serve(uri: &str, page: &Path, base_dir: &Path) -> Served {
    use crate::engine::convert::local_ref;
    let segments = match resolve("https", uri, page) {
        Resolved::Page => return Served::Page(page.to_path_buf()),
        Resolved::NotFound => return Served::NotFound,
        Resolved::Refused => return Served::Refused,
        Resolved::InFolder(segments) => segments,
    };
    let mut candidate = base_dir.to_path_buf();
    for segment in &segments {
        candidate.push(segment);
    }
    if !local_ref::may_open(&candidate, base_dir) {
        return Served::Refused;
    }
    let (Ok(real), Ok(root)) = (candidate.canonicalize(), base_dir.canonicalize()) else {
        return Served::NotFound;
    };
    if !real.starts_with(&root) {
        return Served::Refused;
    }
    if real.is_file() {
        Served::File(real)
    } else {
        Served::NotFound
    }
}

/// Most symbolic links [`open_in_folder`] follows for one path (as the
/// kernel's own `MAXSYMLINKS`).
#[cfg(unix)]
const MAX_LINK_HOPS: usize = 32;

/// Open the file at `segments` (a [`Resolved::InFolder`] path) inside the
/// folder `root` without ever touching anything outside it (round 7; the
/// macOS and Linux workers). Each component is opened relative to the directory before it
/// (`openat` with `O_NOFOLLOW`), so no symbolic link is followed by the
/// kernel. A link found on the way is read (`readlinkat`) and followed by
/// hand only while its target stays inside `root`: a relative target that
/// climbs above `root`, or an absolute one that does not start with
/// `root`'s canonical path, is refused (`PermissionDenied`) before anything
/// it names is looked up, so a link to `/net/host/…` never mounts that
/// share. The file must be a regular file; on macOS its path, read back from
/// the open descriptor (`F_GETPATH`), must lie inside `root` too, and the
/// caller reads from that same descriptor. A hard link cannot be told from
/// the file itself, as on Windows.
#[cfg(unix)]
pub fn open_in_folder(root: &Path, segments: &[String]) -> std::io::Result<std::fs::File> {
    use std::collections::VecDeque;
    use std::ffi::{CString, OsString};
    use std::io::{Error, ErrorKind};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let outside = |what: &str| Error::new(ErrorKind::PermissionDenied, what.to_string());
    let open_at = |dir: libc::c_int, name: &CString, flags: libc::c_int| {
        // SAFETY: `dir` is a live directory descriptor and `name` a valid C
        // string; the returned descriptor is owned by the OwnedFd.
        let fd = unsafe { libc::openat(dir, name.as_ptr(), flags | libc::O_CLOEXEC) };
        if fd < 0 {
            Err(Error::last_os_error())
        } else {
            // SAFETY: a fresh descriptor nobody else owns.
            Ok(unsafe { OwnedFd::from_raw_fd(fd) })
        }
    };
    let components = |path: &Path| -> VecDeque<OsString> {
        path.components()
            .filter_map(|c| match c {
                std::path::Component::Normal(n) => Some(n.to_os_string()),
                std::path::Component::ParentDir => Some(OsString::from("..")),
                _ => None,
            })
            .collect()
    };

    let root = root.canonicalize()?;
    let root_c = CString::new(root.as_os_str().as_bytes())?;
    // SAFETY: a valid C string; the descriptor is owned below.
    let root_fd = unsafe {
        libc::open(
            root_c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if root_fd < 0 {
        return Err(Error::last_os_error());
    }
    // SAFETY: a fresh descriptor nobody else owns.
    let root_fd = unsafe { OwnedFd::from_raw_fd(root_fd) };

    // The directories below `root` the walk is in, innermost last.
    let mut dirs: Vec<OwnedFd> = Vec::new();
    let mut queue: VecDeque<OsString> = segments.iter().map(OsString::from).collect();
    let mut hops = 0;
    while let Some(name) = queue.pop_front() {
        if name.is_empty() || name == "." {
            continue;
        }
        if name == ".." {
            if dirs.pop().is_none() {
                return Err(outside("the path leaves the document's folder"));
            }
            continue;
        }
        let dir = dirs.last().map_or(root_fd.as_raw_fd(), AsRawFd::as_raw_fd);
        let c_name = CString::new(name.as_bytes())?;
        // SAFETY: plain struct the call fills.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: live descriptor, valid C string, out-pointer to a local.
        if unsafe { libc::fstatat(dir, c_name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } != 0 {
            return Err(Error::last_os_error());
        }
        if st.st_mode & libc::S_IFMT == libc::S_IFLNK {
            hops += 1;
            if hops > MAX_LINK_HOPS {
                return Err(Error::other("too many symbolic links"));
            }
            let mut buf = vec![0u8; 4096];
            // SAFETY: live descriptor, valid C string, buffer of the length
            // passed.
            let n = unsafe {
                libc::readlinkat(dir, c_name.as_ptr(), buf.as_mut_ptr().cast(), buf.len())
            };
            let n = usize::try_from(n).map_err(|_| Error::last_os_error())?;
            buf.truncate(n);
            let target = std::path::PathBuf::from(OsString::from_vec(buf));
            let mut next = if target.is_absolute() {
                let inside = target
                    .strip_prefix(&root)
                    .map_err(|_| outside("a link points outside the document's folder"))?;
                dirs.clear();
                components(inside)
            } else {
                components(&target)
            };
            next.extend(queue);
            queue = next;
            continue;
        }
        if queue.is_empty() {
            if st.st_mode & libc::S_IFMT != libc::S_IFREG {
                return Err(Error::new(ErrorKind::NotFound, "not a regular file"));
            }
            let fd = open_at(
                dir,
                &c_name,
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )?;
            let file = std::fs::File::from(fd);
            if !file.metadata()?.is_file() {
                return Err(Error::new(ErrorKind::NotFound, "not a regular file"));
            }
            // macOS: the path the kernel holds for the open file must lie
            // under the one it holds for the folder (both read the same way,
            // so the case and firmlinks agree).
            #[cfg(target_os = "macos")]
            if !fd_path(file.as_raw_fd())?.starts_with(fd_path(root_fd.as_raw_fd())?) {
                return Err(outside("the file lies outside the document's folder"));
            }
            return Ok(file);
        }
        dirs.push(open_at(
            dir,
            &c_name,
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        )?);
    }
    Err(Error::new(ErrorKind::NotFound, "no file named"))
}

/// The path the kernel holds for the open descriptor `fd` (`F_GETPATH`).
#[cfg(target_os = "macos")]
fn fd_path(fd: libc::c_int) -> std::io::Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut path = vec![0u8; libc::PATH_MAX as usize + 1];
    // SAFETY: F_GETPATH writes at most MAXPATHLEN (= PATH_MAX) bytes, NUL
    // included, into the buffer, which is larger.
    if unsafe { libc::fcntl(fd, libc::F_GETPATH, path.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let end = path.iter().position(|b| *b == 0).unwrap_or(path.len());
    Ok(PathBuf::from(OsStr::from_bytes(&path[..end])))
}

/// The `Content-Type` a served file is answered with, from its extension.
/// Pure.
pub fn content_type(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "tif" | "tiff" => "image/tiff",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "txt" | "md" => "text/plain",
        "xml" => "application/xml",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// The content rule list (WebKit's JSON) for a page served from
/// `origin_url` (the private origin, `pluryprint-doc://`[`DOC_HOST`]`/`,
/// ending in `/`): every load is blocked except one on that origin, which
/// only the worker's own scheme handler answers, a `data:` or `blob:` URI
/// and `about:blank`. Every `file:` URL (local or with a host, a share) and
/// every network scheme is refused before it is fetched. Pure.
pub fn webkit_rule_list(origin_url: &str) -> String {
    let mut folder = origin_url.to_string();
    if !folder.ends_with('/') {
        folder.push('/');
    }
    let rules = serde_json::json!([
        {"trigger": {"url-filter": ".*"}, "action": {"type": "block"}},
        {
            "trigger": {
                "url-filter": format!("^{}", webkit_regex_escape(&folder)),
                "url-filter-is-case-sensitive": true
            },
            "action": {"type": "ignore-previous-rules"}
        },
        {"trigger": {"url-filter": "^data:"}, "action": {"type": "ignore-previous-rules"}},
        {"trigger": {"url-filter": "^blob:"}, "action": {"type": "ignore-previous-rules"}},
        {"trigger": {"url-filter": "^about:blank$"}, "action": {"type": "ignore-previous-rules"}}
    ]);
    rules.to_string()
}

/// `text` with every character WebKit's `url-filter` regular expressions
/// treat specially escaped, so it matches only itself. Pure.
fn webkit_regex_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        if matches!(
            c,
            '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The text encoding a staged page is declared in, named in the
/// `Content-Type` the WebKit scheme handlers serve the page with
/// (`render/webkit.rs`): a byte-order mark first, then a
/// `charset` in the first kilobyte, then UTF-8 when the bytes are valid
/// UTF-8, else Windows-1252 (what a browser assumes for Western text). Pure.
pub fn sniff_charset(html: &[u8]) -> String {
    if html.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return "utf-8".into();
    }
    if html.starts_with(&[0xFF, 0xFE]) {
        return "utf-16le".into();
    }
    if html.starts_with(&[0xFE, 0xFF]) {
        return "utf-16be".into();
    }
    let head: Vec<u8> = html.iter().take(1024).map(u8::to_ascii_lowercase).collect();
    if let Some(at) = head.windows(8).position(|w| w == b"charset=") {
        let name: String = head[at + 8..]
            .iter()
            .skip_while(|b| matches!(b, b'"' | b'\'' | b' '))
            .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.'))
            .map(|&b| b as char)
            .collect();
        if !name.is_empty() {
            return name;
        }
    }
    if std::str::from_utf8(html).is_ok() {
        "utf-8".into()
    } else {
        "windows-1252".into()
    }
}

/// Render `request` with this platform's web view. Must run on the process'
/// main thread (every web view needs it). `Err` carries a technical message
/// for the worker's stderr.
pub fn render(request: &RenderRequest) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos::render(request)
    }
    #[cfg(windows)]
    {
        windows::render(request)
    }
    #[cfg(target_os = "linux")]
    {
        linux::render(request)
    }
    #[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
    {
        let _ = request;
        Err("rendering HTML is supported on Windows, macOS and Linux only".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(paper: Option<(f64, f64)>) -> RenderRequest {
        RenderRequest {
            page: PathBuf::from("/w/stage/page é.html"),
            base_dir: PathBuf::from("/docs/my notes"),
            output: PathBuf::from("/w/out.pdf"),
            data_dir: PathBuf::from("/w/webview"),
            paper_in: paper,
            timeout_secs: None,
        }
    }

    #[test]
    fn fr_055_render_request_round_trips_through_the_worker_args() {
        for paper in [None, Some((8.27, 11.69)), Some((11.0, 8.5))] {
            let r = req(paper);
            assert_eq!(RenderRequest::parse(&r.to_args()), Ok(r));
        }
        for timeout in [None, Some(1), Some(3600)] {
            let r = RenderRequest {
                timeout_secs: timeout,
                ..req(None)
            };
            assert_eq!(RenderRequest::parse(&r.to_args()), Ok(r.clone()));
            assert_eq!(r.limit(), timeout.map(std::time::Duration::from_secs));
        }
    }

    /// The worker's own limit is the job's conversion timeout; 0 means none.
    #[test]
    fn nfr_render_worker_honours_conversion_timeout_seconds() {
        let zero = RenderRequest {
            timeout_secs: Some(0),
            ..req(None)
        };
        assert!(!zero
            .to_args()
            .iter()
            .any(|a| a.to_str() == Some("--timeout-secs")));
        assert_eq!(zero.limit(), None);
        let mut args = req(None).to_args();
        args.extend(["--timeout-secs".into(), "0".into()]);
        assert_eq!(RenderRequest::parse(&args).unwrap().timeout_secs, None);
        let mut args = req(None).to_args();
        args.extend(["--timeout-secs".into(), "x".into()]);
        assert!(RenderRequest::parse(&args).is_err());
    }

    /// Windows serves the staged page and the document's folder from one
    /// origin; nothing outside the folder, and no share, is ever opened.
    #[test]
    fn fr_055_serve_maps_one_origin_to_the_page_and_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let doc = root.join("doc");
        std::fs::create_dir_all(doc.join("fonts")).unwrap();
        std::fs::write(doc.join("fonts").join("a b.woff2"), b"wOF2").unwrap();
        std::fs::write(root.join("secret.txt"), b"x").unwrap();
        let stage = root.join("stage");
        std::fs::create_dir_all(&stage).unwrap();
        let page = stage.join("page.html");
        std::fs::write(&page, b"<p>").unwrap();

        let url = page_url(&page);
        assert_eq!(
            url,
            "https://doc.pluryprint.example/.pluryprint-stage/page.html"
        );
        assert!(is_allowed_request(&url));
        assert_eq!(serve(&url, &page, &doc), Served::Page(page.clone()));
        assert_eq!(
            serve(
                "https://doc.pluryprint.example/fonts/a%20b.woff2?v=1#x",
                &page,
                &doc
            ),
            Served::File(doc.join("fonts").join("a b.woff2"))
        );
        assert_eq!(
            serve("https://DOC.pluryprint.example/missing.png", &page, &doc),
            Served::NotFound
        );
        assert_eq!(
            serve("https://doc.pluryprint.example/fonts", &page, &doc),
            Served::NotFound
        );
        for uri in [
            "https://doc.pluryprint.example/..%2fsecret.txt",
            "https://doc.pluryprint.example/%2e%2e/secret.txt",
            "https://doc.pluryprint.example/a%5c..%5c..%5csecret.txt",
            "https://doc.pluryprint.example/C:%5Cx",
            "https://doc.pluryprint.example/x.txt:stream",
            "https://doc.pluryprint.example:444/x",
            "https://u@doc.pluryprint.example/x",
            "https://evil.example/x",
            "http://doc.pluryprint.example/x",
            "file:///etc/passwd",
        ] {
            let served = serve(uri, &page, &doc);
            assert!(
                matches!(served, Served::Refused | Served::NotFound),
                "{uri}: {served:?}"
            );
            assert!(
                !matches!(served, Served::File(ref p) if !p.starts_with(&doc)),
                "{uri}"
            );
        }
        assert_eq!(content_type(Path::new("a.WOFF2")), "font/woff2");
        assert_eq!(content_type(Path::new("i.svg")), "image/svg+xml");
        assert_eq!(content_type(Path::new("noext")), "application/octet-stream");
    }

    /// Round 7: the macOS scheme maps its private origin like the Windows
    /// one, and nothing else.
    #[test]
    fn fr_055_resolve_maps_the_private_scheme_to_the_page_and_the_folder() {
        let page = Path::new("/w/stage/page.html");
        let url = page_url_on(MAC_DOC_SCHEME, page);
        assert_eq!(
            url,
            "pluryprint-doc://doc.pluryprint.example/.pluryprint-stage/page.html"
        );
        assert_eq!(resolve(MAC_DOC_SCHEME, &url, page), Resolved::Page);
        assert_eq!(
            resolve(
                MAC_DOC_SCHEME,
                "pluryprint-doc://DOC.pluryprint.example/fonts/a%20b.woff2?v=1#x",
                page
            ),
            Resolved::InFolder(vec!["fonts".into(), "a b.woff2".into()])
        );
        assert_eq!(
            resolve(
                MAC_DOC_SCHEME,
                "pluryprint-doc://doc.pluryprint.example/a/",
                page
            ),
            Resolved::NotFound
        );
        for uri in [
            "pluryprint-doc://doc.pluryprint.example/..%2fsecret.txt",
            "pluryprint-doc://doc.pluryprint.example/%2e%2e%2fsecret.txt",
            "pluryprint-doc://doc.pluryprint.example/a%5cb",
            "pluryprint-doc://doc.pluryprint.example:1/x",
            "pluryprint-doc://u@doc.pluryprint.example/x",
            "pluryprint-doc://other.example/x",
            "https://doc.pluryprint.example/x",
            "file:///etc/passwd",
            "file://host/share/x",
        ] {
            assert_eq!(
                resolve(MAC_DOC_SCHEME, uri, page),
                Resolved::Refused,
                "{uri}"
            );
        }
        // `..` that the URL parser folds away stays inside the origin.
        assert_eq!(
            resolve(
                MAC_DOC_SCHEME,
                "pluryprint-doc://doc.pluryprint.example/a/../../b.png",
                page
            ),
            Resolved::InFolder(vec!["b.png".into()])
        );
        assert_eq!(
            base_href(Path::new("/d")),
            format!("{}://{DOC_HOST}/", doc_scheme())
        );
        let rules = webkit_rule_list(&format!("{MAC_DOC_SCHEME}://{DOC_HOST}/"));
        assert!(
            rules.contains(r#""^pluryprint-doc://doc\\.pluryprint\\.example/""#),
            "{rules}"
        );
        assert!(!rules.contains("file:"), "{rules}");
    }

    /// Round 7 finding: a file is opened inside the document's folder
    /// without following any link out of it, whatever form the link takes;
    /// links that stay inside still work.
    #[cfg(unix)]
    #[test]
    fn nfr_security_open_in_folder_never_leaves_the_folder() {
        use std::io::{ErrorKind, Read};
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().canonicalize().unwrap();
        let doc = parent.join("doc");
        std::fs::create_dir_all(doc.join("sub")).unwrap();
        std::fs::write(parent.join("secret.txt"), b"secret").unwrap();
        std::fs::write(doc.join("a.txt"), b"a").unwrap();
        std::fs::write(doc.join("sub").join("b.txt"), b"b").unwrap();
        symlink("../secret.txt", doc.join("out.txt")).unwrap();
        symlink(parent.join("secret.txt"), doc.join("abs-out.txt")).unwrap();
        symlink("..", doc.join("up")).unwrap();
        symlink(&parent, doc.join("abs-up")).unwrap();
        symlink("/etc", doc.join("etc")).unwrap();
        symlink("hop", doc.join("hop0")).unwrap();
        symlink("sub/../../secret.txt", doc.join("hop")).unwrap();
        symlink("loop", doc.join("loop")).unwrap();
        symlink("sub/b.txt", doc.join("in.txt")).unwrap();
        symlink(doc.join("sub"), doc.join("abs-sub")).unwrap();
        symlink("../a.txt", doc.join("sub").join("up-a.txt")).unwrap();

        let read = |segments: &[&str]| -> std::io::Result<String> {
            let segments: Vec<String> = segments.iter().map(|s| s.to_string()).collect();
            let mut text = String::new();
            open_in_folder(&doc, &segments)?.read_to_string(&mut text)?;
            Ok(text)
        };
        assert_eq!(read(&["a.txt"]).unwrap(), "a");
        assert_eq!(read(&["sub", "b.txt"]).unwrap(), "b");
        assert_eq!(read(&["in.txt"]).unwrap(), "b");
        assert_eq!(read(&["abs-sub", "b.txt"]).unwrap(), "b");
        assert_eq!(read(&["sub", "up-a.txt"]).unwrap(), "a");
        for escaping in [
            &["out.txt"][..],
            &["abs-out.txt"],
            &["up", "secret.txt"],
            &["abs-up", "secret.txt"],
            &["etc", "hosts"],
            &["hop0"],
            &["up", "doc", "a.txt"],
        ] {
            let err = read(escaping).unwrap_err();
            assert_eq!(
                err.kind(),
                ErrorKind::PermissionDenied,
                "{escaping:?}: {err}"
            );
        }
        for missing in [&["nope.txt"][..], &["sub"], &[], &["loop"]] {
            let err = read(missing).unwrap_err();
            assert_ne!(err.kind(), ErrorKind::PermissionDenied, "{missing:?}");
        }
    }

    #[test]
    fn fr_055_render_request_refuses_incomplete_or_bad_args() {
        let mut args = req(None).to_args();
        args.truncate(6); // no --data
        assert!(RenderRequest::parse(&args).unwrap_err().contains("--data"));
        let mut args = req(None).to_args();
        args.push("--paper-in".into());
        args.push("0x11".into());
        assert!(RenderRequest::parse(&args).is_err());
        let mut args = req(None).to_args();
        args.push("--paper-in".into());
        assert!(RenderRequest::parse(&args).is_err(), "flag without value");
        assert!(RenderRequest::parse(&["--evil".into(), "x".into()]).is_err());
    }

    /// The security finding: no request leaves for the network or a file
    /// share, whatever form the reference takes.
    #[test]
    fn fr_055_remote_resources_blocked() {
        for uri in [
            "http://example.com/x.png",
            "https://example.com/x.png",
            "HTTPS://Example.com/",
            "ws://127.0.0.1:9/",
            "ftp://host/x",
            "file://host/share/x.png",
            "file:////host/share/x.png",
            "file:///C:/Users/me/secret.png",
            "\\\\host\\share\\x.png",
            "https://doc.pluryprint.example.evil.com/x",
            "https://doc.pluryprint.example@evil.com/x",
            "https://evil.com/https://doc.pluryprint.example/",
            "https://stage.pluryprint.example/page.html",
            "javascript:alert(1)",
            "file://10.255.255.1/s/x.png",
            "https://10.255.255.1/s/a.css",
            "wss://doc.pluryprint.example.evil/",
        ] {
            assert!(!is_allowed_request(uri), "{uri} must be refused");
        }
        for uri in [
            "https://doc.pluryprint.example/img/a.png",
            "https://DOC.pluryprint.example/a%20b.css",
            "https://doc.pluryprint.example/.pluryprint-stage/page.html",
            "https://doc.pluryprint.example",
            "data:image/png;base64,AAAA",
            "blob:https://doc.pluryprint.example/1234",
            "about:blank",
        ] {
            assert!(is_allowed_request(uri), "{uri} must be served");
        }
    }

    /// Round 6: the Windows filter answers every request itself. Inline
    /// content passes, the private origin is served, and every other form a
    /// page can spell a share or a host in gets a synthetic 403 (the forms
    /// WebView2 hands the filter after resolving them against the page).
    #[test]
    fn nfr_security_every_request_is_answered_or_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let doc = tmp.path().canonicalize().unwrap();
        std::fs::write(doc.join("a.png"), b"png").unwrap();
        let page = doc.join("stage").join("page.html");
        for inline in [
            "data:text/css,p{}",
            "blob:https://doc.pluryprint.example/x",
            "about:blank",
        ] {
            assert_eq!(answer_request(inline, &page, &doc), None, "{inline}");
        }
        assert_eq!(
            answer_request("https://doc.pluryprint.example/a.png", &page, &doc),
            Some(Served::File(doc.join("a.png")))
        );
        for refused in [
            // <img src="file://10.255.255.1/s/x.png">
            "file://10.255.255.1/s/x.png",
            // <link href="\\10.255.255.1\s\a.css"> against the https base
            "https://10.255.255.1/s/a.css",
            // @import url(//10.255.255.1/x.css)
            "https://10.255.255.1/x.css",
            // <iframe src="file:///C:/Windows/win.ini">
            "file:///C:/Windows/win.ini",
            // fetch() / WebSocket from a script, were one to run
            "http://10.255.255.1/f",
            "ws://10.255.255.1/w",
            "\\\\10.255.255.1\\s\\a.css",
        ] {
            assert_eq!(
                answer_request(refused, &page, &doc),
                Some(Served::Refused),
                "{refused}"
            );
        }
    }

    /// Round 6: WebKit may load the document's folder and inline data only;
    /// the folder's parent, another folder, a share and the network are all
    /// blocked by the rule list before any fetch.
    #[test]
    fn fr_055_webkit_rule_list_allows_only_the_document_folder_and_data() {
        let list = webkit_rule_list("file:///Users/me/My%20Docs/a.b+(c)/");
        let rules: serde_json::Value = serde_json::from_str(&list).unwrap();
        let rules = rules.as_array().unwrap();
        assert_eq!(rules[0]["trigger"]["url-filter"], ".*");
        assert_eq!(rules[0]["action"]["type"], "block");
        let allowed: Vec<&str> = rules[1..]
            .iter()
            .inspect(|r| assert_eq!(r["action"]["type"], "ignore-previous-rules"))
            .map(|r| r["trigger"]["url-filter"].as_str().unwrap())
            .collect();
        assert_eq!(
            allowed,
            [
                r"^file:///Users/me/My%20Docs/a\.b\+\(c\)/",
                "^data:",
                "^blob:",
                "^about:blank$"
            ]
        );
        assert_eq!(rules[1]["trigger"]["url-filter-is-case-sensitive"], true);
        // Nothing else is exempted: no other file: prefix, no network scheme.
        assert!(!list.contains("^file:///\""), "{list}");
        assert!(!list.contains("http"), "{list}");

        // A folder URL without its slash gets one, so `/docs` never admits
        // `/docs-evil`.
        let list = webkit_rule_list("file:///docs");
        assert!(list.contains(r#""^file:///docs/""#), "{list}");

        // Emulate WebKit's anchored match on the escaped prefix.
        let prefix = |url: &str| {
            let rules: serde_json::Value =
                serde_json::from_str(&webkit_rule_list("file:///d/a.b/")).unwrap();
            let filter = rules[1]["trigger"]["url-filter"].as_str().unwrap();
            let literal = filter[1..].replace('\\', "");
            url.starts_with(&literal)
        };
        assert!(prefix("file:///d/a.b/img/x.png"));
        for refused in [
            "file:///d/secret.png",
            "file:///d/a.b-evil/x.png",
            "file://host/d/a.b/x.png",
            "https://doc.pluryprint.example/d/a.b/x.png",
        ] {
            assert!(!prefix(refused), "{refused}");
        }
    }

    #[test]
    fn fr_055_base_href_points_at_the_document_folder() {
        let href = base_href(Path::new(if cfg!(windows) {
            r"C:\docs\my notes"
        } else {
            "/docs/my notes"
        }));
        // Never a `file:` URL (round 7): the worker maps the private origin
        // to the folder itself.
        if cfg!(windows) {
            assert_eq!(href, "https://doc.pluryprint.example/");
        } else {
            assert_eq!(href, "pluryprint-doc://doc.pluryprint.example/");
        }
    }

    /// Linux: a display is either X11 or Wayland; an empty variable is
    /// none (a session that unset it).
    #[test]
    fn fr_055_a_display_is_x11_or_wayland() {
        let s = |v: &'static str| Some(OsStr::new(v));
        assert!(display_in(s(":0"), None));
        assert!(display_in(None, s("wayland-0")));
        assert!(display_in(s(":99"), s("wayland-1")));
        assert!(!display_in(None, None));
        assert!(!display_in(s(""), s("")));
        if !cfg!(target_os = "linux") {
            assert!(display_available());
        }
    }

    #[test]
    fn fr_057_charset_sniffing() {
        assert_eq!(sniff_charset(b"\xEF\xBB\xBF<p>x"), "utf-8");
        assert_eq!(sniff_charset(&[0xFF, 0xFE, b'<', 0]), "utf-16le");
        assert_eq!(
            sniff_charset(b"<meta charset=\"ISO-8859-1\"><p>caf\xe9"),
            "iso-8859-1"
        );
        assert_eq!(
            sniff_charset(
                b"<meta http-equiv=content-type content='text/html; charset=windows-1252'>"
            ),
            "windows-1252"
        );
        assert_eq!(sniff_charset("<p>café".as_bytes()), "utf-8");
        assert_eq!(sniff_charset(b"<p>caf\xe9"), "windows-1252");
    }
}
