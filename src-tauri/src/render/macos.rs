//! `WKWebView` renderer (FR-055).
//!
//! The worker is a bare AppKit process (no Tauri, no window on screen, no Dock
//! icon: the activation policy is `Prohibited`):
//!
//! 1. A content rule list ([`super::webkit_rule_list`]) is compiled into a
//!    store inside the job's work folder; it blocks every load that is not on
//!    the private origin `pluryprint-doc://`[`super::DOC_HOST`], `data:`,
//!    `blob:` or `about:blank`: the network, and every `file:` URL, local or
//!    with a host (a share).
//! 2. A `WKWebView` with content JavaScript off, a non-persistent website data
//!    store (no cookies, caches or storage written anywhere) and that rule
//!    list loads the staged page from that origin. **WebKit never reads a
//!    file itself** (round 7): the worker registers its own
//!    `WKURLSchemeHandler` ([`DocSchemeHandler`]) for
//!    [`super::MAC_DOC_SCHEME`] and answers every request on it, as the
//!    Windows worker does with its request filter: the staged page, and files
//!    inside the document's folder opened by [`super::open_in_folder`], which
//!    follows no symbolic link out of the folder and checks the open file's
//!    own path before the bytes are read from it. The round-6 design granted
//!    WebKit read access to the folder and confined it with the rule list's
//!    URL prefix; a symbolic link inside the folder (one an extracted zip can
//!    carry) passed the prefix and WebKit followed it to any file on disk.
//!    A navigation delegate ([`NavigationGuard`]) cancels every navigation
//!    the worker did not start itself (a refresh, a frame, a link target),
//!    writing each one it refused to stderr (the job's `render.log`), and so
//!    do refused requests.
//! 3. When loading ends, an `NSPrintOperation` from
//!    `printOperationWithPrintInfo:` writes the paginated PDF straight to the
//!    output file (`NSPrintSaveJob`), with no panel. It runs modally for the
//!    web view's off-screen window, as WebKit's printing view expects; run
//!    synchronously it never ends its pagination.
//!
//! `createPDF` is not used: it returns the whole page as one tall PDF page,
//! ignoring the paper and CSS page breaks.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{
    define_class, msg_send, sel, AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSPrintInfo,
    NSPrintJobSavingURL, NSPrintOperation, NSPrintSaveJob, NSPrintingPaginationMode, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{
    ns_string, NSData, NSDate, NSDefaultRunLoopMode, NSDictionary, NSError, NSHTTPURLResponse,
    NSObject, NSObjectProtocol, NSPoint, NSRect, NSRunLoop, NSSize, NSString, NSURLRequest, NSURL,
};
use objc2_web_kit::{
    WKContentRuleList, WKContentRuleListStore, WKNavigationAction, WKNavigationActionPolicy,
    WKNavigationDelegate, WKURLSchemeHandler, WKURLSchemeTask, WKWebView, WKWebViewConfiguration,
    WKWebpagePreferences, WKWebsiteDataStore,
};

use super::webkit::{answer, paper_points, remaining, timed_out, MARGIN_PT, MAX_OUTPUT_BYTES};
use super::RenderRequest;

/// State of the [`DocSchemeHandler`].
struct SchemeIvars {
    page: PathBuf,
    base_dir: PathBuf,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the class adds no
    // Drop and implements WKURLSchemeHandler's two methods with their
    // declared signatures.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PluryprintRenderSchemeHandler"]
    #[ivars = SchemeIvars]
    struct DocSchemeHandler;

    unsafe impl NSObjectProtocol for DocSchemeHandler {}

    unsafe impl WKURLSchemeHandler for DocSchemeHandler {
        // SAFETY: matches WebKit's
        // - (void)webView:(WKWebView *)webView
        //   startURLSchemeTask:(id<WKURLSchemeTask>)urlSchemeTask
        #[unsafe(method(webView:startURLSchemeTask:))]
        fn start_task(&self, _web_view: &WKWebView, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            // SAFETY: plain property reads of a live task.
            let Some(url) = (unsafe { task.request() }).URL() else {
                return;
            };
            let uri = url
                .absoluteString()
                .map(|u| u.to_string())
                .unwrap_or_default();
            let ivars = self.ivars();
            let answer = answer(&uri, &ivars.page, &ivars.base_dir);
            let keys: Vec<Retained<NSString>> = answer
                .headers
                .iter()
                .map(|(k, _)| NSString::from_str(k))
                .collect();
            let values: Vec<Retained<NSString>> = answer
                .headers
                .iter()
                .map(|(_, v)| NSString::from_str(v))
                .collect();
            let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
            let value_refs: Vec<&NSString> = values.iter().map(|v| &**v).collect();
            let headers = NSDictionary::from_slices(&key_refs, &value_refs);
            let response = NSHTTPURLResponse::initWithURL_statusCode_HTTPVersion_headerFields(
                NSHTTPURLResponse::alloc(),
                &url,
                isize::try_from(answer.status).unwrap_or(500),
                Some(ns_string!("HTTP/1.1")),
                Some(&headers),
            );
            let Some(response) = response else {
                return;
            };
            // SAFETY: the task is live and has not been answered or
            // stopped: it is answered once, here, synchronously.
            unsafe {
                task.didReceiveResponse(&response);
                if !answer.body.is_empty() {
                    task.didReceiveData(&NSData::with_bytes(&answer.body));
                }
                task.didFinish();
            }
        }

        // SAFETY: matches WebKit's
        // - (void)webView:(WKWebView *)webView
        //   stopURLSchemeTask:(id<WKURLSchemeTask>)urlSchemeTask
        #[unsafe(method(webView:stopURLSchemeTask:))]
        fn stop_task(&self, _web_view: &WKWebView, _task: &ProtocolObject<dyn WKURLSchemeTask>) {
            // Every task is answered in full when it starts; nothing runs on.
        }
    }
);

impl DocSchemeHandler {
    fn new(mtm: MainThreadMarker, page: PathBuf, base_dir: PathBuf) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(SchemeIvars { page, base_dir });
        // SAFETY: NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// Spin the main run loop until `done` says so or `limit` passes.
fn pump_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    let run_loop = NSRunLoop::currentRunLoop();
    while !done() {
        if started.elapsed() > limit {
            return false;
        }
        let until = NSDate::dateWithTimeIntervalSinceNow(0.02);
        // SAFETY: the default mode is a valid run loop mode constant.
        unsafe { run_loop.runMode_beforeDate(NSDefaultRunLoopMode, &until) };
    }
    true
}

fn file_url(path: &Path, is_dir: bool) -> Retained<NSURL> {
    NSURL::fileURLWithPath_isDirectory(&NSString::from_str(&path.to_string_lossy()), is_dir)
}

/// Compile the rule list that keeps the page on `origin_url`, its private
/// origin (module docs).
fn compile_rules(
    mtm: MainThreadMarker,
    store_dir: &Path,
    origin_url: &str,
) -> Result<Retained<WKContentRuleList>, String> {
    std::fs::create_dir_all(store_dir)
        .map_err(|e| format!("cannot create the web view folder: {e}"))?;
    // SAFETY: a file URL to a folder we own is what the store expects.
    let store =
        unsafe { WKContentRuleListStore::storeWithURL(Some(&file_url(store_dir, true)), mtm) }
            .ok_or("WebKit could not open a content rule list store")?;
    type Slot = Rc<RefCell<Option<Result<Retained<WKContentRuleList>, String>>>>;
    let slot: Slot = Rc::new(RefCell::new(None));
    let sink = slot.clone();
    let handler = RcBlock::new(move |list: *mut WKContentRuleList, error: *mut NSError| {
        // SAFETY: WebKit hands either a list or an error, both valid for the
        // duration of the call; retaining keeps the list alive past it.
        let result = match unsafe { Retained::retain(list) } {
            Some(list) => Ok(list),
            None => Err(unsafe { error.as_ref() }
                .map(|e| e.localizedDescription().to_string())
                .unwrap_or_else(|| "unknown error".into())),
        };
        *sink.borrow_mut() = Some(result);
    });
    // SAFETY: identifier and JSON are valid strings; WebKit copies the block.
    unsafe {
        store.compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler(
            Some(ns_string!("pluryprint-offline")),
            Some(&NSString::from_str(&super::webkit_rule_list(origin_url))),
            Some(&handler),
        );
    }
    if !pump_until(Duration::from_secs(30), || slot.borrow().is_some()) {
        return Err("WebKit did not compile the content rule list in time".into());
    }
    let result = slot.borrow_mut().take().expect("set above");
    result.map_err(|e| format!("WebKit refused the content rule list: {e}"))
}

/// Render `request` (see [`super::render`]).
pub fn render(request: &RenderRequest) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("the renderer must run on the main thread")?;
    // The job's conversion timeout bounds the whole render (none when the
    // setting is 0); the watchdog also covers a main thread stuck in WebKit.
    let deadline = request.limit().map(|limit| Instant::now() + limit);
    super::start_watchdog(request.limit());
    let app = NSApplication::sharedApplication(mtm);
    // No Dock icon, no menu bar, never frontmost.
    app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);

    let (width, height) = paper_points(request.paper_in);

    let origin_url = super::base_href(&request.base_dir);
    let page_url = super::page_url_on(super::MAC_DOC_SCHEME, &request.page);
    let rules = compile_rules(mtm, &request.data_dir.join("rules"), &origin_url)?;
    let page_nsurl =
        NSURL::URLWithString(&NSString::from_str(&page_url)).ok_or("the staged page has no URL")?;
    let guard = NavigationGuard::new(
        mtm,
        page_nsurl
            .absoluteString()
            .map_or(page_url.clone(), |u| u.to_string()),
    );
    let handler = DocSchemeHandler::new(mtm, request.page.clone(), request.base_dir.clone());

    let view_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height));
    // SAFETY: plain WebKit object construction and configuration on the main
    // thread.
    let webview = unsafe {
        let config = WKWebViewConfiguration::new(mtm);
        let prefs = WKWebpagePreferences::new(mtm);
        prefs.setAllowsContentJavaScript(false);
        config.setDefaultWebpagePreferences(Some(&prefs));
        config.setWebsiteDataStore(&WKWebsiteDataStore::nonPersistentDataStore(mtm));
        config.userContentController().addContentRuleList(&rules);
        config.setURLSchemeHandler_forURLScheme(
            Some(ProtocolObject::from_ref(&*handler)),
            &NSString::from_str(super::MAC_DOC_SCHEME),
        );
        WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), view_frame, &config)
    };
    // SAFETY: the guard outlives the web view (both live to the end of this
    // function; the view only keeps a weak reference).
    unsafe { webview.setNavigationDelegate(Some(ProtocolObject::from_ref(&*guard))) };

    // An off-screen, never-shown window: printing needs the view in a window.
    let frame = NSRect::new(
        NSPoint::new(-20_000.0, -20_000.0),
        NSSize::new(width, height),
    );
    // SAFETY: standard NSWindow construction on the main thread.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: the window is owned by this function's Retained handle.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setContentView(Some(&webview));

    // The page comes from the worker's own scheme handler; WebKit is never
    // given a file URL or read access to any folder.
    guard.expect_load();
    // SAFETY: a valid request for the private origin.
    unsafe {
        webview.loadRequest(&NSURLRequest::requestWithURL(&page_nsurl));
    }
    // SAFETY: isLoading is a plain property read on the main thread.
    let loaded = pump_until(remaining(deadline), || unsafe { !webview.isLoading() });
    if !loaded {
        return Err(timed_out("loading the page"));
    }
    // Let layout settle (images decoded, fonts applied) before printing.
    pump_until(Duration::from_millis(150), || false);

    let printed = print_to_file(
        mtm,
        &webview,
        &window,
        &request.output,
        (width, height),
        deadline,
    );
    window.close();
    // SAFETY: plain property write on the main thread.
    unsafe { webview.setNavigationDelegate(None) };
    drop(handler);
    printed?;
    match std::fs::metadata(&request.output) {
        Ok(meta) if meta.len() > 0 => Ok(()),
        _ => Err("the web view printed nothing".into()),
    }
}

/// State of the [`NavigationGuard`].
struct GuardIvars {
    /// The staged page's URL on the private origin: the only URL the worker
    /// itself loads.
    page_url: String,
    /// Navigations the worker started and that have not been decided yet.
    expected: Cell<u32>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the class adds no
    // Drop and only implements optional WKNavigationDelegate methods with
    // their declared signatures.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PluryprintRenderNavigationGuard"]
    #[ivars = GuardIvars]
    struct NavigationGuard;

    unsafe impl NSObjectProtocol for NavigationGuard {}

    unsafe impl WKNavigationDelegate for NavigationGuard {
        // SAFETY: matches WebKit's
        // - (void)webView:(WKWebView *)webView
        //   decidePolicyForNavigationAction:(WKNavigationAction *)action
        //   decisionHandler:(void (^)(WKNavigationActionPolicy))handler
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_policy(
            &self,
            _web_view: &WKWebView,
            action: &WKNavigationAction,
            handler: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            // SAFETY: plain property reads of a live navigation action.
            let url = unsafe { action.request() }
                .URL()
                .and_then(|u| u.absoluteString())
                .map(|u| u.to_string())
                .unwrap_or_default();
            let policy = if self.allows(&url) {
                WKNavigationActionPolicy::Allow
            } else {
                eprintln!("pluryprint render: navigation refused: {url}");
                WKNavigationActionPolicy::Cancel
            };
            handler.call((policy,));
        }
    }
);

impl NavigationGuard {
    fn new(mtm: MainThreadMarker, page_url: String) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(GuardIvars {
            page_url,
            expected: Cell::new(0),
        });
        // SAFETY: NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }

    /// The worker is about to load the page itself.
    fn expect_load(&self) {
        let ivars = self.ivars();
        ivars.expected.set(ivars.expected.get() + 1);
    }

    /// Whether a navigation to `url` is one the worker started: the page's
    /// own URL, while a load of ours is pending. Anything else, and the
    /// page URL once our load is done (a refresh to it), is refused.
    fn allows(&self, url: &str) -> bool {
        let ivars = self.ivars();
        let ours = ivars.expected.get() > 0 && url == ivars.page_url;
        if ours {
            ivars.expected.set(ivars.expected.get() - 1);
        }
        ours
    }
}

struct DelegateIvars {
    /// `Some(success)` once the print operation has run.
    done: Cell<Option<bool>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the class adds no
    // Drop and only the one callback below.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PluryprintRenderPrintDelegate"]
    #[ivars = DelegateIvars]
    struct PrintDelegate;

    impl PrintDelegate {
        // SAFETY: matches AppKit's didRunSelector contract:
        // - (void)printOperationDidRun:(NSPrintOperation *)op
        //        success:(BOOL)success contextInfo:(void *)info
        #[unsafe(method(printOperationDidRun:success:contextInfo:))]
        fn did_run(&self, _op: &NSPrintOperation, success: Bool, _info: *mut c_void) {
            self.ivars().done.set(Some(success.as_bool()));
        }
    }
);

impl PrintDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars {
            done: Cell::new(None),
        });
        // SAFETY: NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// Print the loaded `webview` (shown in `window`) into `output` on a
/// `width` × `height` pt page, by `deadline` when there is one.
fn print_to_file(
    mtm: MainThreadMarker,
    webview: &WKWebView,
    window: &NSWindow,
    output: &Path,
    (width, height): (f64, f64),
    deadline: Option<Instant>,
) -> Result<(), String> {
    let _ = std::fs::remove_file(output);
    let info = NSPrintInfo::new();
    info.setPaperSize(NSSize::new(width, height));
    info.setTopMargin(MARGIN_PT);
    info.setBottomMargin(MARGIN_PT);
    info.setLeftMargin(MARGIN_PT);
    info.setRightMargin(MARGIN_PT);
    info.setHorizontalPagination(NSPrintingPaginationMode::Fit);
    info.setVerticalPagination(NSPrintingPaginationMode::Automatic);
    info.setHorizontallyCentered(false);
    info.setVerticallyCentered(false);
    // SAFETY: the job disposition and saving URL keys are AppKit constants;
    // the dictionary is the print info's own, typed `NSString -> id`.
    unsafe {
        info.setJobDisposition(NSPrintSaveJob);
        let url = file_url(output, false);
        let object: &AnyObject = &url;
        info.dictionary()
            .setObject_forKey(object, ProtocolObject::from_ref(NSPrintJobSavingURL));
    }
    // SAFETY: the web view is loaded and lives in `window`.
    let op = unsafe { webview.printOperationWithPrintInfo(&info) };
    op.setShowsPrintPanel(false);
    op.setShowsProgressPanel(false);
    op.setCanSpawnSeparateThread(true);
    // WebKit lays the page out for print at the printing view's size; a view
    // without a frame prints blank pages.
    if let Some(view) = op.view() {
        view.setFrame(webview.bounds());
    }
    let delegate = PrintDelegate::new(mtm);
    // SAFETY: the delegate implements the selector with AppKit's signature
    // and outlives the operation (kept until the callback fired).
    unsafe {
        op.runOperationModalForWindow_delegate_didRunSelector_contextInfo(
            window,
            Some(&delegate),
            Some(sel!(printOperationDidRun:success:contextInfo:)),
            std::ptr::null_mut(),
        );
    }
    let mut too_big = false;
    let finished = pump_until(remaining(deadline), || {
        if std::fs::metadata(output).is_ok_and(|m| m.len() > MAX_OUTPUT_BYTES) {
            too_big = true;
            return true;
        }
        delegate.ivars().done.get().is_some()
    });
    if too_big {
        let _ = std::fs::remove_file(output);
        return Err("the page did not paginate (the PDF kept growing)".into());
    }
    match (finished, delegate.ivars().done.get()) {
        (true, Some(true)) => Ok(()),
        (true, _) => Err("the print operation failed".into()),
        (false, _) => Err(timed_out("printing")),
    }
}
