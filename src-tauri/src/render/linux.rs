//! WebKitGTK renderer (FR-055): the Linux counterpart of the macOS
//! `WKWebView` worker ([`super::macos`]), with WebKitGTK's own pieces.
//!
//! The worker is a bare GTK process (no Tauri, no window on screen, no
//! IPC): the web view lives in a `GtkOffscreenWindow` that is never mapped
//! on the display. It does need a display to start GTK at all (X11 or
//! Wayland); without one it fails at once with [`super::NO_DISPLAY_MARKER`].
//!
//! 1. A content filter ([`super::webkit_rule_list`], the same JSON rule list
//!    macOS compiles) is compiled into a store inside the job's work folder
//!    and added to the web view's user content manager. It blocks every load
//!    that is not on the private origin `pluryprint-doc://`[`super::DOC_HOST`],
//!    `data:`, `blob:` or `about:blank`: the network, and every `file:` URL,
//!    local or with a host (a share).
//! 2. The web view has an ephemeral web context (no cookies, caches or
//!    storage written anywhere) whose network proxy is a dead port on
//!    loopback, so a request that got past the filter would still go
//!    nowhere; JavaScript is off (`enable-javascript` and
//!    `enable-javascript-markup`), and so are WebGL, media, local
//!    storage, DNS prefetching and the page cache. **WebKit never reads a
//!    file itself**: the worker registers its own URI scheme,
//!    [`super::MAC_DOC_SCHEME`], on the web context and answers every
//!    request on it with [`super::webkit::answer`], as the macOS worker does:
//!    the staged page (with its policy as a header), files inside the
//!    document's folder opened by [`super::open_in_folder`], which follows no
//!    symbolic link out of the folder, a 404, or a logged 403. A
//!    `decide-policy` handler ignores every navigation the worker did not
//!    start itself (a refresh, a frame, a link target, a new window),
//!    writing each one it refused to stderr (the job's `render.log`).
//! 3. When loading ends, a `WebKitPrintOperation` prints the page with no
//!    dialog to GTK's own print-to-file printer, which writes the PDF
//!    straight to the output file on the job's paper and margins
//!    ([`super::webkit::MARGIN_PT`]), backgrounds included. Its `finished`
//!    or `failed` signal ends the render.
//!
//! The worker's web process is not run in WebKit's bubblewrap sandbox: that
//! needs `bwrap` and unprivileged user namespaces, which neither the
//! AppImage nor every distribution provides, and a sandbox that fails to
//! start ends the render. The boundary is the one the other platforms rely
//! on: no script, the content filter, the scheme handler and the dead proxy.

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;
use webkit2gtk::glib::translate::{FromGlibPtrFull, ToGlibPtr};
use webkit2gtk::{
    gio, glib, HardwareAccelerationPolicy, LoadEvent, NavigationPolicyDecision,
    NavigationPolicyDecisionExt, NetworkProxyMode, NetworkProxySettings, PolicyDecisionExt,
    PolicyDecisionType, PrintOperation, PrintOperationExt, SettingsExt, URIRequestExt,
    URISchemeRequest, URISchemeRequestExt, URISchemeResponse, URISchemeResponseExt,
    UserContentManager, WebContext, WebContextExt, WebProcessTerminationReason, WebView,
    WebViewExt, WebsiteDataManager, WebsiteDataManagerExt,
};

use super::webkit::{answer, paper_points, remaining, timed_out, MARGIN_PT, MAX_OUTPUT_BYTES};
use super::{RenderRequest, Resolved};

/// Set to `1`, the worker writes each stage to stderr (the job's
/// `render.log`), as the Windows worker does.
const TRACE_ENV: &str = "PLURYPRINT_RENDER_TRACE";

/// A proxy on a loopback port nothing listens on (the discard port): the
/// web context's every network request would end there.
const DEAD_PROXY: &str = "http://127.0.0.1:9";

fn trace(what: impl std::fmt::Display) {
    if std::env::var_os(TRACE_ENV).is_some_and(|v| v == "1") {
        eprintln!("pluryprint render: {what}");
    }
}

/// The environment the worker sets for itself before GTK and WebKit start,
/// each only when the user has not set it: the web view is never shown, so
/// WebKit composites nothing and shares no GPU buffers (both fail, or show
/// nothing, on some drivers and under Xvfb), and GTK neither reaches for the
/// accessibility bus nor starts GVfs daemons for a process with no UI. The
/// web process WebKit starts inherits it.
const QUIET_ENV: [(&str, &str); 4] = [
    ("WEBKIT_DISABLE_COMPOSITING_MODE", "1"),
    ("WEBKIT_DISABLE_DMABUF_RENDERER", "1"),
    ("NO_AT_BRIDGE", "1"),
    ("GIO_USE_VFS", "local"),
];

/// Spin the default GLib main context until `done` says so or `limit`
/// passes.
fn pump_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    let context = glib::MainContext::default();
    while !done() {
        if started.elapsed() > limit {
            return false;
        }
        if !context.iteration(false) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    true
}

/// A compiled `WebKitUserContentFilter`, released when dropped.
struct ContentFilter(*mut webkit2gtk::ffi::WebKitUserContentFilter);

impl Drop for ContentFilter {
    fn drop(&mut self) {
        // SAFETY: the pointer is the full reference `save_finish` returned,
        // released once.
        unsafe { webkit2gtk::ffi::webkit_user_content_filter_unref(self.0) };
    }
}

type FilterSlot = Rc<RefCell<Option<Result<ContentFilter, String>>>>;

/// `GAsyncReadyCallback` of the filter store's `save`: finishes it and puts
/// the filter (or the error) in the slot `data` points at.
unsafe extern "C" fn filter_saved(
    source: *mut glib::gobject_ffi::GObject,
    result: *mut gio::ffi::GAsyncResult,
    data: glib::ffi::gpointer,
) {
    // SAFETY: `data` is the boxed slot `compile_rules` leaked for this one
    // callback; it is taken back exactly once, here.
    let slot = unsafe { Box::from_raw(data.cast::<FilterSlot>()) };
    let mut error = std::ptr::null_mut();
    // SAFETY: `source` is the store the call was made on and `result` the
    // result GIO hands this callback.
    let filter = unsafe {
        webkit2gtk::ffi::webkit_user_content_filter_store_save_finish(
            source.cast(),
            result,
            &mut error,
        )
    };
    let outcome = if filter.is_null() {
        Err(if error.is_null() {
            "unknown error".to_string()
        } else {
            // SAFETY: a GError the call transferred to us.
            unsafe { glib::Error::from_glib_full(error) }.to_string()
        })
    } else {
        Ok(ContentFilter(filter))
    };
    *slot.borrow_mut() = Some(outcome);
}

/// Compile the content filter that keeps the page on `origin_url`, its
/// private origin (module docs), in a store inside `store_dir`.
fn compile_rules(store_dir: &Path, origin_url: &str) -> Result<ContentFilter, String> {
    std::fs::create_dir_all(store_dir)
        .map_err(|e| format!("cannot create the web view folder: {e}"))?;
    let path = CString::new(store_dir.as_os_str().as_encoded_bytes())
        .map_err(|_| "the web view folder's path holds a NUL byte")?;
    // SAFETY: a valid C string; the store is a new GObject we own (full
    // reference), wrapped so it is released when this function returns,
    // after the save it was used for has finished.
    let store: glib::Object = unsafe {
        let raw = webkit2gtk::ffi::webkit_user_content_filter_store_new(path.as_ptr());
        if raw.is_null() {
            return Err("WebKit could not open a content filter store".into());
        }
        glib::Object::from_glib_full(raw.cast::<glib::gobject_ffi::GObject>())
    };
    let source = glib::Bytes::from_owned(super::webkit_rule_list(origin_url).into_bytes());
    let slot: FilterSlot = Rc::new(RefCell::new(None));
    let data = Box::into_raw(Box::new(slot.clone()));
    // SAFETY: the store, identifier and bytes are valid for the call (GIO
    // keeps its own references while the save runs); `data` is taken back
    // by `filter_saved`, which GIO calls exactly once.
    unsafe {
        let store_ptr: *mut glib::gobject_ffi::GObject = store.to_glib_none().0;
        webkit2gtk::ffi::webkit_user_content_filter_store_save(
            store_ptr.cast(),
            c"pluryprint-offline".as_ptr(),
            source.to_glib_none().0,
            std::ptr::null_mut(),
            Some(filter_saved),
            data.cast(),
        );
    }
    if !pump_until(Duration::from_secs(30), || slot.borrow().is_some()) {
        return Err("WebKit did not compile the content filter in time".into());
    }
    let result = slot.borrow_mut().take().expect("set above");
    result.map_err(|e| format!("WebKit refused the content filter: {e}"))
}

/// Answer one request on the private scheme (module docs).
fn respond(request: &URISchemeRequest, page: &Path, base_dir: &Path) {
    let uri = request.uri().map(|u| u.to_string()).unwrap_or_default();
    let answer = answer(&uri, page, base_dir);
    let mime = answer
        .content_type()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    let length = i64::try_from(answer.body.len()).unwrap_or(i64::MAX);
    let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(answer.body));
    let response = URISchemeResponse::new(&stream, length);
    response.set_status(u32::from(answer.status), None);
    response.set_content_type(&mime);
    let headers = soup::MessageHeaders::new(soup::MessageHeadersType::Response);
    for (name, value) in &answer.headers {
        headers.append(name, value);
    }
    response.set_http_headers(headers);
    request.finish_with_response(&response);
}

/// Navigations the worker allows (module docs): the staged page itself,
/// while a load the worker started has not been decided yet.
struct NavigationGuard {
    page: PathBuf,
    expected: Cell<u32>,
}

impl NavigationGuard {
    fn allows(&self, url: &str) -> bool {
        let ours = self.expected.get() > 0 && is_page(url, &self.page);
        if ours {
            self.expected.set(self.expected.get() - 1);
        }
        ours
    }
}

/// Whether `url` is the staged `page` on the private origin, by the same
/// rules the scheme handler serves it by ([`super::resolve`]: the host in
/// any case, the query and fragment ignored). Pure.
fn is_page(url: &str, page: &Path) -> bool {
    super::resolve(super::MAC_DOC_SCHEME, url, page) == Resolved::Page
}

/// How the page load went.
#[derive(Default)]
struct LoadState {
    finished: bool,
    error: Option<String>,
}

/// The name GTK gives its print-to-file printer: "Print to File" in GTK's
/// own translation for the process' language (GTK binds its `gtk30` text
/// domain when it starts), which is the name WebKit looks the printer up by.
fn file_printer_name() -> String {
    // SAFETY: two valid C strings; g_dgettext returns a static string
    // (the translation or the message itself) that is copied at once.
    unsafe {
        let text = glib::ffi::g_dgettext(c"gtk30".as_ptr(), c"Print to File".as_ptr());
        if text.is_null() {
            "Print to File".to_string()
        } else {
            CStr::from_ptr(text).to_string_lossy().into_owned()
        }
    }
}

/// Render `request` (see [`super::render`]).
pub fn render(request: &RenderRequest) -> Result<(), String> {
    if !super::display_available() {
        return Err(format!(
            "{}: printing HTML, Markdown or text needs a graphical session \
             (neither DISPLAY nor WAYLAND_DISPLAY is set)",
            super::NO_DISPLAY_MARKER
        ));
    }
    for (name, value) in QUIET_ENV {
        if std::env::var_os(name).is_none() {
            std::env::set_var(name, value);
        }
    }
    // The job's conversion timeout bounds the whole render (none when the
    // setting is 0); the watchdog also covers a main thread stuck in WebKit.
    let deadline = request.limit().map(|limit| Instant::now() + limit);
    super::start_watchdog(request.limit());
    gtk::init().map_err(|e| {
        format!(
            "{}: GTK could not open the display ({e})",
            super::NO_DISPLAY_MARKER
        )
    })?;
    trace("GTK started");

    let (width, height) = paper_points(request.paper_in);
    let origin_url = super::base_href(&request.base_dir);
    let page_url = super::page_url_on(super::MAC_DOC_SCHEME, &request.page);
    let filter = compile_rules(&request.data_dir.join("rules"), &origin_url)?;
    trace("content filter compiled");

    let data_manager = WebsiteDataManager::new_ephemeral();
    let mut proxy = NetworkProxySettings::new(Some(DEAD_PROXY), &[]);
    data_manager.set_network_proxy_settings(NetworkProxyMode::Custom, Some(&mut proxy));
    let context = WebContext::with_website_data_manager(&data_manager);
    context.set_cache_model(webkit2gtk::CacheModel::DocumentViewer);
    let (page, base_dir) = (request.page.clone(), request.base_dir.clone());
    context.register_uri_scheme(super::MAC_DOC_SCHEME, move |req| {
        respond(req, &page, &base_dir);
    });

    let content = UserContentManager::new();
    // SAFETY: both pointers are live; the manager takes its own reference
    // to the filter.
    unsafe {
        let manager: *mut webkit2gtk::ffi::WebKitUserContentManager = content.to_glib_none().0;
        webkit2gtk::ffi::webkit_user_content_manager_add_filter(manager, filter.0);
    }
    drop(filter);

    let settings = webkit2gtk::Settings::new();
    settings.set_enable_javascript(false);
    settings.set_enable_javascript_markup(false);
    settings.set_enable_webgl(false);
    settings.set_enable_webaudio(false);
    settings.set_enable_media(false);
    settings.set_enable_html5_local_storage(false);
    settings.set_enable_html5_database(false);
    settings.set_enable_offline_web_application_cache(false);
    settings.set_enable_dns_prefetching(false);
    settings.set_enable_page_cache(false);
    settings.set_enable_developer_extras(false);
    settings.set_allow_file_access_from_file_urls(false);
    settings.set_auto_load_images(true);
    settings.set_print_backgrounds(true);
    settings.set_hardware_acceleration_policy(HardwareAccelerationPolicy::Never);

    let webview = WebView::builder()
        .web_context(&context)
        .user_content_manager(&content)
        .settings(&settings)
        .build();

    let guard = Rc::new(NavigationGuard {
        page: request.page.clone(),
        expected: Cell::new(0),
    });
    let decide = guard.clone();
    webview.connect_decide_policy(move |_, decision, kind| {
        if !matches!(
            kind,
            PolicyDecisionType::NavigationAction | PolicyDecisionType::NewWindowAction
        ) {
            return false;
        }
        let url = decision
            .downcast_ref::<NavigationPolicyDecision>()
            .and_then(NavigationPolicyDecisionExt::navigation_action)
            .and_then(|action| action.request())
            .and_then(|req| req.uri())
            .map(|u| u.to_string())
            .unwrap_or_default();
        if kind == PolicyDecisionType::NavigationAction && decide.allows(&url) {
            decision.use_();
        } else {
            eprintln!("pluryprint render: navigation refused: {url}");
            decision.ignore();
        }
        true
    });

    let load = Rc::new(RefCell::new(LoadState::default()));
    let on_change = load.clone();
    webview.connect_load_changed(move |_, event| {
        if event == LoadEvent::Finished {
            on_change.borrow_mut().finished = true;
        }
    });
    let on_fail = load.clone();
    let failed_page = request.page.clone();
    webview.connect_load_failed(move |_, _, uri, error| {
        // A navigation the guard ignored also ends here; only the page's
        // own load failing is an error.
        if is_page(uri, &failed_page) {
            on_fail.borrow_mut().error = Some(format!("{uri}: {error}"));
        }
        false
    });
    let crashed: Rc<Cell<Option<WebProcessTerminationReason>>> = Rc::new(Cell::new(None));
    let on_crash = crashed.clone();
    webview.connect_web_process_terminated(move |_, reason| on_crash.set(Some(reason)));

    // An off-screen, never-mapped window: printing needs a realised view.
    let window = gtk::OffscreenWindow::new();
    window.set_default_size(width.round() as i32, height.round() as i32);
    window.add(&webview);
    window.show_all();

    // The page comes from the worker's own scheme handler; WebKit is never
    // given a file URL.
    guard.expected.set(guard.expected.get() + 1);
    trace(format_args!("loading {page_url}"));
    webview.load_uri(&page_url);
    let loaded = pump_until(remaining(deadline), || {
        load.borrow().finished || crashed.get().is_some()
    });
    if let Some(reason) = crashed.get() {
        return Err(terminated(reason));
    }
    if !loaded {
        return Err(timed_out("loading the page"));
    }
    if let Some(error) = load.borrow_mut().error.take() {
        return Err(format!("the web view could not load the page: {error}"));
    }
    trace("page loaded");
    // Let layout settle (images decoded, fonts applied) before printing.
    pump_until(Duration::from_millis(150), || false);

    let printed = print_to_file(
        &webview,
        &request.output,
        (width, height),
        deadline,
        &crashed,
    );
    // SAFETY: the window is ours and no longer used.
    unsafe { window.destroy() };
    printed?;
    match std::fs::metadata(&request.output) {
        Ok(meta) if meta.len() > 0 => {
            trace(format_args!("printed {} bytes", meta.len()));
            Ok(())
        }
        _ => Err("the web view printed nothing".into()),
    }
}

/// The error for a web process that ended before the render did. Running
/// out of memory says so in the words the engine reads as one
/// ([`crate::engine::convert::process`]), so the job fails as
/// `page_too_large`, not as a renderer failure to retry.
fn terminated(reason: WebProcessTerminationReason) -> String {
    match reason {
        WebProcessTerminationReason::ExceededMemoryLimit => {
            "the web view's memory allocation of the page failed: \
             it exceeded the memory limit"
                .into()
        }
        WebProcessTerminationReason::Crashed => "the web view's content process crashed".into(),
        other => format!("the web view's content process ended ({other:?})"),
    }
}

/// Print the loaded `webview` into `output` on a `width` × `height` pt page,
/// by `deadline` when there is one.
fn print_to_file(
    webview: &WebView,
    output: &Path,
    (width, height): (f64, f64),
    deadline: Option<Instant>,
    crashed: &Rc<Cell<Option<WebProcessTerminationReason>>>,
) -> Result<(), String> {
    let _ = std::fs::remove_file(output);
    let uri = url::Url::from_file_path(output)
        .map_err(|()| format!("the output path is not absolute: {}", output.display()))?;

    let paper =
        gtk::PaperSize::new_custom("pluryprint", "Pluryprint", width, height, gtk::Unit::Points);
    let setup = gtk::PageSetup::new();
    setup.set_orientation(gtk::PageOrientation::Portrait);
    setup.set_paper_size(&paper);
    setup.set_top_margin(MARGIN_PT, gtk::Unit::Points);
    setup.set_bottom_margin(MARGIN_PT, gtk::Unit::Points);
    setup.set_left_margin(MARGIN_PT, gtk::Unit::Points);
    setup.set_right_margin(MARGIN_PT, gtk::Unit::Points);

    let printer = file_printer_name();
    let settings = gtk::PrintSettings::new();
    settings.set_printer(&printer);
    settings.set(gtk::PRINT_SETTINGS_OUTPUT_FILE_FORMAT, Some("pdf"));
    settings.set(gtk::PRINT_SETTINGS_OUTPUT_URI, Some(uri.as_str()));
    settings.set_orientation(gtk::PageOrientation::Portrait);
    settings.set_paper_size(&paper);
    settings.set_n_copies(1);
    settings.set_scale(100.0);

    let op = PrintOperation::new(webview);
    op.set_page_setup(&setup);
    op.set_print_settings(&settings);
    let done: Rc<RefCell<Option<Result<(), String>>>> = Rc::new(RefCell::new(None));
    let on_fail = done.clone();
    op.connect_failed(move |_, error| {
        on_fail
            .borrow_mut()
            .get_or_insert_with(|| Err(format!("the print operation failed: {error}")));
    });
    let on_finish = done.clone();
    op.connect_finished(move |_| {
        on_finish.borrow_mut().get_or_insert(Ok(()));
    });
    trace(format_args!("printing to \"{printer}\""));
    op.print();

    let mut too_big = false;
    let finished = pump_until(remaining(deadline), || {
        if std::fs::metadata(output).is_ok_and(|m| m.len() > MAX_OUTPUT_BYTES) {
            too_big = true;
            return true;
        }
        done.borrow().is_some() || crashed.get().is_some()
    });
    if too_big {
        let _ = std::fs::remove_file(output);
        return Err("the page did not paginate (the PDF kept growing)".into());
    }
    if let Some(reason) = crashed.get() {
        return Err(terminated(reason));
    }
    if !finished {
        return Err(timed_out("printing"));
    }
    let outcome = done.borrow_mut().take().expect("set above");
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard lets through exactly the loads the worker started, once
    /// each, and nothing else: not another URL, not a second load (a
    /// refresh to the page itself).
    #[test]
    fn nfr_security_only_the_workers_own_navigation_is_allowed() {
        let page = "pluryprint-doc://doc.pluryprint.example/.pluryprint-stage/page.html";
        let guard = NavigationGuard {
            page: PathBuf::from("/w/stage/page.html"),
            expected: Cell::new(0),
        };
        assert!(!guard.allows(page), "nothing expected yet");
        guard.expected.set(1);
        assert!(!guard.allows("http://127.0.0.1:9/refresh"));
        assert!(!guard.allows("pluryprint-doc://doc.pluryprint.example/other.html"));
        assert!(guard.allows("pluryprint-doc://DOC.pluryprint.example/.pluryprint-stage/page.html"));
        assert!(!guard.allows(page), "a refresh to the page is refused");
    }

    /// Running out of memory reads as one to the engine; a crash does not.
    #[test]
    fn nfr_render_a_web_process_over_its_memory_is_reported_as_such() {
        let oom = terminated(WebProcessTerminationReason::ExceededMemoryLimit);
        assert!(oom.contains("memory allocation of") && oom.contains("failed"));
        let crash = terminated(WebProcessTerminationReason::Crashed);
        assert!(!crash.contains("memory allocation of"));
    }
}
