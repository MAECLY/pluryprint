//! What the two WebKit renderers share: `WKWebView` on macOS
//! ([`super::macos`]) and WebKitGTK on Linux ([`super::linux`]).
//!
//! Both load the staged page from the private origin
//! `pluryprint-doc://`[`super::DOC_HOST`] and answer every request on it
//! themselves, through their web view's own scheme handler, with
//! [`answer`]: the staged page, files inside the document's folder opened by
//! [`super::open_in_folder`] (no symbolic link is followed out of it), a 404,
//! or a logged 403. Both print on the same paper and margins and give up on
//! the same deadline, so a page prints alike on either platform.

use std::path::Path;
use std::time::{Duration, Instant};

use super::Resolved;

/// US Letter, in points, for a job whose paper is `auto`.
pub(crate) const LETTER_PT: (f64, f64) = (612.0, 792.0);

/// Page margins, in points (0.4 in, Chromium's default print margin), so a
/// page prints alike on every platform.
pub(crate) const MARGIN_PT: f64 = 28.8;

/// The largest PDF a render may write. A page WebKit cannot paginate keeps
/// producing pages; this stops it before it fills the disk.
pub(crate) const MAX_OUTPUT_BYTES: u64 = 512 * 1024 * 1024;

/// Largest file the worker serves to the page, as on Windows. A bigger one
/// is answered 404.
const MAX_SERVED_BYTES: u64 = 256 * 1024 * 1024;

/// The page size in points for `paper_in` (inches), US Letter when the job
/// left it to the renderer. Pure.
pub(crate) fn paper_points(paper_in: Option<(f64, f64)>) -> (f64, f64) {
    paper_in
        .map(|(w, h)| (w * 72.0, h * 72.0))
        .unwrap_or(LETTER_PT)
}

/// Time left until `deadline` (the job's conversion timeout, counted from
/// the worker's start), or no limit when there is none.
pub(crate) fn remaining(deadline: Option<Instant>) -> Duration {
    deadline.map_or(Duration::MAX, |d| {
        d.saturating_duration_since(Instant::now())
    })
}

/// The error of a stage that ran past the conversion timeout: it carries
/// [`super::TIMEOUT_MARKER`], so the engine reports a timeout.
pub(crate) fn timed_out(stage: &str) -> String {
    format!(
        "{}: {stage} did not finish within the conversion timeout",
        super::TIMEOUT_MARKER
    )
}

/// What the scheme handler answers one request with.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Answer {
    /// The HTTP status: 200, 403 or 404.
    pub status: u16,
    /// `(name, value)` response headers.
    pub headers: Vec<(&'static str, String)>,
    /// The response body (empty for a 403 or 404).
    pub body: Vec<u8>,
}

impl Answer {
    fn empty(status: u16) -> Self {
        Self {
            status,
            headers: vec![("Content-Type", "text/plain".into())],
            body: Vec::new(),
        }
    }

    /// The `Content-Type` header's value (macOS hands the headers to
    /// `NSHTTPURLResponse` as they are; WebKitGTK takes the type apart).
    #[cfg(any(test, target_os = "linux"))]
    pub fn content_type(&self) -> &str {
        self.headers
            .iter()
            .find(|(k, _)| *k == "Content-Type")
            .map_or("application/octet-stream", |(_, v)| v.as_str())
    }
}

/// The answer to a request for `uri` on the private origin (module docs):
/// the staged `page` (with its policy as a header too), a file inside
/// `base_dir` opened by [`super::open_in_folder`] and read from that same
/// descriptor, a 404, or a logged 403 for anything that is not this origin
/// or would leave the folder.
pub(crate) fn answer(uri: &str, page: &Path, base_dir: &Path) -> Answer {
    use std::io::Read;
    let read_capped = |file: std::fs::File| -> Option<Vec<u8>> {
        let mut bytes = Vec::new();
        file.take(MAX_SERVED_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        (bytes.len() as u64 <= MAX_SERVED_BYTES).then_some(bytes)
    };
    match super::resolve(super::MAC_DOC_SCHEME, uri, page) {
        Resolved::Page => match std::fs::File::open(page).ok().and_then(read_capped) {
            Some(body) => Answer {
                status: 200,
                headers: vec![
                    (
                        "Content-Type",
                        format!("text/html; charset={}", super::sniff_charset(&body)),
                    ),
                    (
                        "Content-Security-Policy",
                        crate::engine::convert::html::page_csp(),
                    ),
                ],
                body,
            },
            None => Answer::empty(404),
        },
        Resolved::InFolder(segments) => match super::open_in_folder(base_dir, &segments) {
            Ok(file) => match read_capped(file) {
                Some(body) => {
                    let name = segments.last().map(String::as_str).unwrap_or_default();
                    Answer {
                        status: 200,
                        headers: vec![(
                            "Content-Type",
                            super::content_type(Path::new(name)).to_string(),
                        )],
                        body,
                    }
                }
                None => Answer::empty(404),
            },
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                super::log_refusal("file", uri);
                Answer::empty(403)
            }
            Err(_) => Answer::empty(404),
        },
        Resolved::NotFound => Answer::empty(404),
        Resolved::Refused => {
            super::log_refusal("request", uri);
            Answer::empty(403)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round 7: the scheme handler serves the staged page with its policy,
    /// files inside the folder with their type, and refuses a link out of
    /// the folder (403), another origin (403) and a missing file (404).
    #[test]
    fn nfr_security_scheme_handler_answers_only_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let doc = root.join("doc");
        let stage = root.join("stage");
        std::fs::create_dir_all(&doc).unwrap();
        std::fs::create_dir_all(&stage).unwrap();
        let page = stage.join("page.html");
        std::fs::write(&page, "\u{feff}<p>hi").unwrap();
        std::fs::write(doc.join("a.css"), "p{}").unwrap();
        std::fs::write(root.join("secret.css"), "x").unwrap();
        std::os::unix::fs::symlink("../secret.css", doc.join("link.css")).unwrap();
        let origin = format!(
            "{}://{}",
            super::super::MAC_DOC_SCHEME,
            super::super::DOC_HOST
        );

        let served = answer(
            &super::super::page_url_on(super::super::MAC_DOC_SCHEME, &page),
            &page,
            &doc,
        );
        assert_eq!(served.status, 200);
        assert!(served
            .headers
            .iter()
            .any(|(k, v)| *k == "Content-Security-Policy" && v.contains("script-src 'none'")));
        assert_eq!(served.content_type(), "text/html; charset=utf-8");

        let css = answer(&format!("{origin}/a.css"), &page, &doc);
        assert_eq!((css.status, css.body.as_slice()), (200, b"p{}".as_slice()));
        assert_eq!(css.headers, vec![("Content-Type", "text/css".to_string())]);

        assert_eq!(
            answer(&format!("{origin}/link.css"), &page, &doc).status,
            403
        );
        assert_eq!(
            answer(&format!("{origin}/..%2fsecret.css"), &page, &doc).status,
            403
        );
        assert_eq!(answer("file:///etc/hosts", &page, &doc).status, 403);
        assert_eq!(
            answer(&format!("{origin}/nope.css"), &page, &doc).status,
            404
        );
        assert!(answer(&format!("{origin}/link.css"), &page, &doc)
            .body
            .is_empty());
    }

    /// The paper the job asked for, in points; Letter when it left it open.
    #[test]
    fn fr_055_paper_points_follow_the_job() {
        assert_eq!(paper_points(None), LETTER_PT);
        assert_eq!(paper_points(Some((11.0, 8.5))), (792.0, 612.0));
        let (w, h) = paper_points(Some((8.27, 11.69)));
        assert_eq!((w.round(), h.round()), (595.0, 842.0));
    }
}
