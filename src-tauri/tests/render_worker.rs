//! FR-055/056/057 with the real render worker: the freshly built `pluryprint`
//! binary prints HTML, Markdown, text (and RTF on macOS) through the system
//! web view, with no Edge, Chrome, Pandoc or LibreOffice.
//!
//! Runs on Windows, macOS and Linux (the platforms with a system web view the
//! worker drives). On Linux the web view needs a display: under CI (`CI` set)
//! a missing one fails the run, so `xvfb-run` cannot be forgotten; on a
//! developer machine without one (an SSH session) the tests that render say
//! so and return early.

#![cfg(any(windows, target_os = "macos", target_os = "linux"))]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Once;
use std::time::{Duration, Instant};

use pluryprint_lib::engine::convert::{convert_to_pdf, PageSetup};
use pluryprint_lib::engine::EngineError;

/// Point the engine at the binary cargo built for this test run.
fn setup() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        std::env::set_var("PLURYPRINT_WORKER_EXE", env!("CARGO_BIN_EXE_pluryprint"));
    });
}

/// Whether this run can render at all: always on Windows and macOS; on
/// Linux only with a display (module docs). Under CI a missing display is
/// a failure, not a skip.
fn can_render() -> bool {
    if pluryprint_lib::render::display_available() {
        return true;
    }
    assert!(
        std::env::var_os("CI").is_none(),
        "the render worker needs a display: run the tests under xvfb-run"
    );
    eprintln!("no DISPLAY or WAYLAND_DISPLAY: skipping a real render");
    false
}

const TIMEOUT: Option<Duration> = Some(Duration::from_secs(120));

fn a4() -> PageSetup {
    PageSetup {
        css_size: Some("210mm 297mm".into()),
    }
}

fn convert(input: &Path, page: &PageSetup) -> Result<PathBuf, EngineError> {
    setup();
    let cache = input.parent().unwrap().join("cache");
    let job = format!(
        "job-{}",
        input
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .replace(' ', "-")
    );
    convert_to_pdf(
        input,
        &cache,
        &job,
        &AtomicBool::new(false),
        &|_| {},
        TIMEOUT,
        page,
        None,
    )
    .map(|out| out.expect("a converted document yields a cache PDF"))
}

fn pages(pdf: &Path) -> usize {
    lopdf::Document::load(pdf).unwrap().get_pages().len()
}

fn first_page_size(pdf: &Path) -> (i64, i64) {
    let doc = lopdf::Document::load(pdf).unwrap();
    let (_, id) = doc.get_pages().into_iter().next().unwrap();
    let page = doc.get_dictionary(id).unwrap();
    let mb = page
        .get(b"MediaBox")
        .or_else(|_| {
            // Inherited from the page tree.
            let parent = page.get(b"Parent").unwrap().as_reference().unwrap();
            doc.get_dictionary(parent).unwrap().get(b"MediaBox")
        })
        .unwrap()
        .as_array()
        .unwrap()
        .clone();
    let n = |o: &lopdf::Object| {
        o.as_float()
            .map(f64::from)
            .or_else(|_| o.as_i64().map(|v| v as f64))
            .unwrap()
    };
    (
        (n(&mb[2]) - n(&mb[0])).round() as i64,
        (n(&mb[3]) - n(&mb[1])).round() as i64,
    )
}

/// Count image XObjects in the document.
fn images(pdf: &Path) -> usize {
    let doc = lopdf::Document::load(pdf).unwrap();
    doc.objects
        .values()
        .filter(|o| {
            o.as_stream()
                .ok()
                .and_then(|s| s.dict.get(b"Subtype").ok())
                .and_then(|v| v.as_name().ok())
                == Some(b"Image".as_slice())
        })
        .count()
}

fn red_png(path: &Path) {
    let img = image::RgbImage::from_pixel(40, 40, image::Rgb([220, 20, 20]));
    img.save_with_format(path, image::ImageFormat::Png).unwrap();
}

/// A listener that must never see a connection.
struct Tripwire(std::net::TcpListener);

impl Tripwire {
    fn new() -> Self {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        Self(l)
    }
    fn port(&self) -> u16 {
        self.0.local_addr().unwrap().port()
    }
    fn assert_untouched(&self) {
        match self.0.accept() {
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            other => panic!("the page reached the network: {other:?}"),
        }
    }
}

#[test]
fn fr_055_html_renders_without_browser() {
    if !can_render() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("img")).unwrap();
    red_png(&dir.path().join("img").join("red dot.png"));
    std::fs::write(dir.path().join("style.css"), "h1 { color: #036; }").unwrap();
    let paragraphs: String = (1..=200)
        .map(|i| format!("<p>Paragraph {i} of a long page.</p>"))
        .collect();
    let html = dir.path().join("página larga.html");
    std::fs::write(
        &html,
        format!(
            "<!doctype html><html><head><meta charset=utf-8><title>t</title>\
             <link rel=stylesheet href=style.css></head><body><h1>Título</h1>\
             <img src=\"img/red%20dot.png\" alt=dot>{paragraphs}</body></html>"
        ),
    )
    .unwrap();
    let pdf = convert(&html, &a4()).unwrap();
    assert!(
        pages(&pdf) > 1,
        "200 paragraphs span pages: {}",
        pages(&pdf)
    );
    assert_eq!(first_page_size(&pdf), (595, 842), "A4 from the job");
    assert!(images(&pdf) >= 1, "the relative image loads");
    // Nothing is lost at the page breaks: the first and the last paragraph
    // are both in the text.
    if let Some(text) = pdf_text(&pdf) {
        for needle in ["Título", "Paragraph 1 of", "Paragraph 200 of"] {
            assert!(text.contains(needle), "{needle} is printed: {text}");
        }
    }
}

/// Security finding: nothing on the page reaches the network or a file
/// share — sub-resources, a protocol-relative reference, a refresh and a
/// frame all point at a listener that must stay untouched.
#[test]
fn fr_055_remote_resources_blocked() {
    if !can_render() {
        return;
    }
    let wire = Tripwire::new();
    let port = wire.port();
    let dir = tempfile::tempdir().unwrap();
    let html = dir.path().join("hostile.html");
    std::fs::write(
        &html,
        format!(
            "<!doctype html><html><head><meta charset=utf-8>\
             <meta http-equiv=refresh content=\"0;url=http://127.0.0.1:{port}/refresh\">\
             <link rel=stylesheet href=\"http://localhost:{port}/x.css\">\
             <style>@import url(http://127.0.0.1:{port}/import.css); \
             body {{ background: url(//127.0.0.1:{port}/bg.png); }}</style></head>\
             <body><p>static text</p>\
             <img src=\"http://127.0.0.1:{port}/beacon.png\">\
             <img src=\"//127.0.0.1:{port}/proto.png\">\
             <iframe src=\"http://127.0.0.1:{port}/frame\"></iframe>\
             <object data=\"http://127.0.0.1:{port}/obj\"></object>\
             <script>fetch('http://127.0.0.1:{port}/fetch');</script>\
             </body></html>"
        ),
    )
    .unwrap();
    let pdf = convert(&html, &PageSetup::default()).unwrap();
    assert_eq!(pages(&pdf), 1);
    // Give a late request (the refresh) the chance to show up.
    std::thread::sleep(Duration::from_millis(300));
    wire.assert_untouched();
}

#[test]
fn fr_055_page_size_from_pagesetup() {
    if !can_render() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let html = dir.path().join("letter.html");
    std::fs::write(&html, "<p>letter landscape</p>").unwrap();
    let letter_landscape = PageSetup {
        css_size: Some("11in 8.5in".into()),
    };
    let pdf = convert(&html, &letter_landscape).unwrap();
    assert_eq!(first_page_size(&pdf), (792, 612));
}

#[test]
fn fr_056_markdown_no_pandoc_real_render() {
    if !can_render() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    red_png(&dir.path().join("pic.png"));
    let md = dir.path().join("README.md");
    std::fs::write(
        &md,
        "# Título\n\nSome *emphasis* and a table:\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n\
         ![local](pic.png)\n\n<script>document.write('ran')</script>\n",
    )
    .unwrap();
    let pdf = convert(&md, &a4()).unwrap();
    assert_eq!(pages(&pdf), 1);
    assert_eq!(first_page_size(&pdf), (595, 842));
    assert!(images(&pdf) >= 1, "the local picture is printed");
}

#[test]
fn fr_057_txt_real_render_utf16_and_long() {
    if !can_render() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let txt = dir.path().join("notas año.txt");
    let body: String = (1..=300)
        .map(|i| format!("Línea {i}: ñandú, acción, café\r\n"))
        .collect();
    let mut utf16 = vec![0xFF, 0xFE];
    for unit in body.encode_utf16() {
        utf16.extend_from_slice(&unit.to_le_bytes());
    }
    std::fs::write(&txt, utf16).unwrap();
    let pdf = convert(&txt, &PageSetup::default()).unwrap();
    assert!(pages(&pdf) > 1, "300 lines span pages");
    assert_eq!(first_page_size(&pdf), (612, 792), "Letter when auto");
    if let Some(text) = pdf_text(&pdf) {
        for needle in ["Línea 1: ñandú", "Línea 300: ñandú, acción, café"] {
            assert!(text.contains(needle), "{needle} is printed: {text}");
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn fr_057_rtf_macos_textutil() {
    if !can_render() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let rtf = dir.path().join("memo.rtf");
    std::fs::write(
        &rtf,
        r"{\rtf1\ansi{\fonttbl\f0 Helvetica;}\f0\b Hello\b0  RTF \u241?and\u250?.\par}",
    )
    .unwrap();
    let pdf = convert(&rtf, &a4()).unwrap();
    assert_eq!(pages(&pdf), 1);
    assert_eq!(first_page_size(&pdf), (595, 842));
}

/// The content streams of every page, decompressed.
fn page_content(pdf: &Path) -> String {
    let doc = lopdf::Document::load(pdf).unwrap();
    doc.get_pages()
        .values()
        .map(|id| String::from_utf8_lossy(&doc.get_page_content(*id)).into_owned())
        .collect()
}

/// A TrueType font that exists on this OS, to serve as a web font.
fn some_font() -> Option<(PathBuf, &'static str)> {
    let candidates: &[(&str, &str)] = if cfg!(windows) {
        &[
            (r"C:\Windows\Fonts\consola.ttf", "Consolas"),
            (r"C:\Windows\Fonts\cour.ttf", "Courier"),
        ]
    } else if cfg!(target_os = "linux") {
        &[
            (
                "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
                "DejaVuSansMono",
            ),
            (
                "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf",
                "DejaVuSerif",
            ),
        ]
    } else {
        &[
            (
                "/System/Library/Fonts/Supplemental/Courier New.ttf",
                "CourierNew",
            ),
            (
                "/System/Library/Fonts/Supplemental/Arial Black.ttf",
                "Arial-Black",
            ),
        ]
    };
    candidates
        .iter()
        .map(|(p, n)| (PathBuf::from(p), *n))
        .find(|(p, _)| p.is_file())
}

/// Round 4: same-folder web fonts and an external SVG `<use>` load (on
/// Windows they used to come from a second, CORS-denied origin), and a page's
/// own relative `<base href>` points into the document's folder, not into
/// the private stage folder.
#[test]
fn fr_055_same_folder_fonts_use_and_relative_base_load() {
    if !can_render() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("fonts")).unwrap();
    std::fs::create_dir_all(dir.path().join("assets")).unwrap();
    red_png(&dir.path().join("assets").join("red.png"));
    std::fs::write(
        dir.path().join("assets").join("icons.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><symbol id=\"r\" viewBox=\"0 0 10 10\">\
         <rect width=\"10\" height=\"10\" fill=\"#00ff00\"/></symbol></svg>",
    )
    .unwrap();
    let font = some_font();
    if let Some((path, _)) = &font {
        std::fs::copy(path, dir.path().join("assets").join("face.ttf")).unwrap();
    }
    let html = dir.path().join("based.html");
    std::fs::write(
        &html,
        "<!doctype html><html><head><meta charset=utf-8><base href=\"assets/\">\
         <style>@font-face { font-family: PluryTest; src: url(face.ttf); }\
         p { font-family: PluryTest; font-size: 20pt; }</style></head><body>\
         <p>Web font text</p><img src=\"red.png\">\
         <svg width=\"40\" height=\"40\"><use href=\"icons.svg#r\"/></svg></body></html>",
    )
    .unwrap();
    let pdf = convert(&html, &PageSetup::default()).unwrap();
    assert!(
        images(&pdf) >= 1,
        "the image under the page's own base loads"
    );
    // WebKit does not draw an external `<use>` document (also not from its
    // private scheme, round 7); WebView2, serving one origin, does.
    if cfg!(windows) {
        let content = page_content(&pdf);
        assert!(
            content.contains("0 1 0 rg"),
            "the external <use> symbol is drawn: {content}"
        );
    }
    if let Some((_, name)) = font {
        let bytes = std::fs::read(&pdf).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            text.contains(name),
            "the same-folder web font {name} is embedded"
        );
    }
}

/// Round 4 security finding: a `<head>` inside a leading comment no longer
/// swallows the page's policy, and a share reference is never followed.
#[test]
fn nfr_security_commented_head_and_share_refs_render_safely() {
    if !can_render() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let html = dir.path().join("commented.html");
    std::fs::write(
        &html,
        "<!-- <head> --><!doctype html><html><head><title>t</title></head><body>\
         <p>ok</p><img src=\"/\\127.0.0.1\\share\\x.png\"><img src=\"file:///%5C%5C127.0.0.1%5Cs%5Cx.png\">\
         </body></html>",
    )
    .unwrap();
    // A marker picture of an odd size, planted wherever a share reference
    // could be mis-resolved to: if any of them is printed, a share
    // reference was followed.
    let mut planted = vec![
        dir.path().join("127.0.0.1").join("share").join("x.png"),
        dir.path().join("127.0.0.1").join("s").join("x.png"),
        dir.path().join("share").join("x.png"),
        dir.path().join("s").join("x.png"),
        dir.path().join("x.png"),
    ];
    // Off Windows a backslash is an ordinary file name character, so the
    // reference's literal spelling can name a file in the folder too.
    let backslashed: &[&str] = if cfg!(windows) {
        &[]
    } else {
        &["\\127.0.0.1\\share\\x.png", "\\\\127.0.0.1\\s\\x.png"]
    };
    planted.extend(backslashed.iter().map(|name| dir.path().join(name)));
    for path in &planted {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        marker_png(path);
    }
    let pdf = convert(&html, &PageSetup::default()).unwrap();
    assert_eq!(pages(&pdf), 1);
    // The marker is recognisable in a PDF when it does load.
    let sanity = dir.path().join("sanity.html");
    std::fs::write(
        &sanity,
        "<!doctype html><html><head><title>t</title></head><body>\
         <p>ok</p><img src=\"x.png\"></body></html>",
    )
    .unwrap();
    let sanity_pdf = convert(&sanity, &PageSetup::default()).unwrap();
    assert!(
        image_sizes(&sanity_pdf).contains(&MARKER_SIZE),
        "a loaded marker is found by its size: {:?}",
        image_sizes(&sanity_pdf)
    );
    assert!(
        !image_sizes(&pdf).contains(&MARKER_SIZE),
        "no share reference reaches a planted picture: {:?}",
        image_sizes(&pdf)
    );
    // Whatever stops a share reference, it prints at most what a missing
    // picture prints. Which layer stops it decides whether the web view
    // draws its broken-image glyph: WebView2 draws one for every refusal,
    // WebKit draws one for a 403/404 from the worker's scheme handler and
    // none for a load its content rule list blocks, and which of the two
    // stops `/\host\share` differs between WebKit releases (macOS 26 vs
    // 27). So this is an upper bound, not an equality.
    let control = dir.path().join("control.html");
    std::fs::write(
        &control,
        "<!doctype html><html><head><title>t</title></head><body>\
         <p>ok</p><img src=\"missing-1.png\"><img src=\"missing-2.png\"></body></html>",
    )
    .unwrap();
    let control_pdf = convert(&control, &PageSetup::default()).unwrap();
    assert!(
        images(&pdf) <= images(&control_pdf),
        "no share image is loaded: {} images, a page of missing ones has {}",
        images(&pdf),
        images(&control_pdf)
    );
}

/// The odd pixel size of [`marker_png`], which no broken-image glyph has.
const MARKER_SIZE: (i64, i64) = (37, 23);

fn marker_png(path: &Path) {
    let (w, h) = MARKER_SIZE;
    let img = image::RgbImage::from_pixel(w as u32, h as u32, image::Rgb([20, 160, 220]));
    img.save_with_format(path, image::ImageFormat::Png).unwrap();
}

/// The pixel size of every image in a PDF.
fn image_sizes(pdf: &Path) -> Vec<(i64, i64)> {
    let doc = lopdf::Document::load(pdf).unwrap();
    doc.objects
        .values()
        .filter_map(|o| o.as_stream().ok())
        .filter(|s| {
            s.dict.get(b"Subtype").ok().and_then(|v| v.as_name().ok()) == Some(b"Image".as_slice())
        })
        .filter_map(|s| {
            let w = s.dict.get(b"Width").ok()?.as_i64().ok()?;
            let h = s.dict.get(b"Height").ok()?.as_i64().ok()?;
            Some((w, h))
        })
        .collect()
}

/// D10: the worker runs as its own process and exits; it never forwards to
/// a running instance or waits on one, and a second one in parallel works.
#[test]
fn nfr_worker_not_single_instance_and_no_ipc() {
    if !can_render() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.html");
    let b = dir.path().join("b.html");
    std::fs::write(&a, "<p>a</p>").unwrap();
    std::fs::write(&b, "<p>b</p>").unwrap();
    let (ra, rb) = std::thread::scope(|s| {
        let ta = s.spawn(|| convert(&a, &PageSetup::default()));
        let tb = s.spawn(|| convert(&b, &PageSetup::default()));
        (ta.join().unwrap(), tb.join().unwrap())
    });
    assert_eq!(pages(&ra.unwrap()), 1);
    assert_eq!(pages(&rb.unwrap()), 1);
}

/// Informative: how long one render takes end to end (worker start, load,
/// print). Printed, and only bounded loosely.
#[test]
fn nfr_001_worker_startup_time_is_measured() {
    if !can_render() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let html = dir.path().join("tiny.html");
    std::fs::write(&html, "<p>tiny</p>").unwrap();
    let started = Instant::now();
    convert(&html, &PageSetup::default()).unwrap();
    let took = started.elapsed();
    eprintln!("NFR-001 render worker, tiny page: {took:?}");
    assert!(took < Duration::from_secs(60), "{took:?}");
}

/// Round 6: the web view's own network layer is the boundary, not a text
/// pre-pass. Every way a page has been seen to reach out (a share named by
/// `file://host/`, a UNC path, a protocol-relative `@import`, an SVG
/// `<style>` whose `@import` is spelled with character references, a frame
/// on a local system file, a script `fetch`) points at a listener that must
/// stay untouched, and the page still prints.
#[test]
fn nfr_security_network_layer_blocks_every_reference_form() {
    if !can_render() {
        return;
    }
    let wire = Tripwire::new();
    let port = wire.port();
    let dir = tempfile::tempdir().unwrap();
    let html = dir.path().join("vectors.html");
    let system_file = if cfg!(windows) {
        "file:///C:/Windows/win.ini"
    } else {
        "file:///etc/hosts"
    };
    std::fs::write(
        &html,
        format!(
            "<!doctype html><html><head><meta charset=utf-8>\
             <link rel=stylesheet href=\"\\\\127.0.0.1:{port}\\s\\a.css\">\
             <link rel=preload as=image href=\"http://127.0.0.1:{port}/preload.png\">\
             <style>@import url(//127.0.0.1:{port}/import.css);</style></head>\
             <body><p>vectors</p>\
             <img src=\"file://127.0.0.1:{port}/s/x.png\">\
             <img srcset=\"http://127.0.0.1:{port}/set.png 1x\">\
             <video poster=\"http://127.0.0.1:{port}/poster.png\"></video>\
             <svg><style>@import url(&#47;&#47;127.0.0.1:{port}/svg.css);\
             rect{{fill:url(&#x2F;&#x2F;127.0.0.1:{port}/p.svg#a)}}</style>\
             <rect width=9 height=9/></svg>\
             <iframe src=\"{system_file}\"></iframe>\
             <a href=\"http://127.0.0.1:{port}/link\" target=_blank>link</a>\
             <script>fetch('http://127.0.0.1:{port}/fetch');\
             new Image().src='//127.0.0.1:{port}/js.png';</script>\
             </body></html>"
        ),
    )
    .unwrap();
    let pdf = convert(&html, &PageSetup::default()).unwrap();
    assert_eq!(pages(&pdf), 1);
    std::thread::sleep(Duration::from_millis(300));
    wire.assert_untouched();
}

/// Round 6: only the document's folder is readable. A picture in the
/// folder's parent (by `../`, by an absolute `file:` URL, and through a
/// stylesheet that sits in the parent) never loads, while one inside the
/// folder does.
#[test]
fn nfr_security_only_the_document_folder_is_readable() {
    if !can_render() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().canonicalize().unwrap();
    let doc = parent.join("doc");
    std::fs::create_dir_all(&doc).unwrap();
    red_png(&parent.join("secret.png"));
    red_png(&doc.join("inside.png"));
    std::fs::write(
        parent.join("outside.css"),
        "body { background: url(secret.png); }",
    )
    .unwrap();
    let secret_url = url::Url::from_file_path(parent.join("secret.png"))
        .unwrap()
        .to_string();
    let page = doc.join("page.html");
    std::fs::write(
        &page,
        format!(
            "<!doctype html><html><head><meta charset=utf-8>\
             <link rel=stylesheet href=\"../outside.css\"></head><body>\
             <img src=\"inside.png\"><img src=\"../secret.png\">\
             <img src=\"{secret_url}\"></body></html>"
        ),
    )
    .unwrap();
    let pdf = convert(&page, &PageSetup::default()).unwrap();
    // The control: the same page with the outside references pointing at
    // files that do not exist (WebView2 draws its broken-image icon for
    // either).
    let control = doc.join("control.html");
    std::fs::write(
        &control,
        "<!doctype html><html><head><meta charset=utf-8></head><body>\
         <img src=\"inside.png\"><img src=\"missing-1.png\">\
         <img src=\"missing-2.png\"></body></html>",
    )
    .unwrap();
    let control_pdf = convert(&control, &PageSetup::default()).unwrap();
    assert!(images(&pdf) >= 1, "the picture inside the folder loads");
    assert_eq!(
        images(&pdf),
        images(&control_pdf),
        "nothing from the parent folder is loaded"
    );
}

#[cfg(unix)]
fn png_of(path: &Path, rgb: [u8; 3]) {
    let img = image::RgbImage::from_pixel(40, 40, image::Rgb(rgb));
    img.save_with_format(path, image::ImageFormat::Png).unwrap();
}

/// Round 7 finding: a symbolic link inside the document's folder that
/// points outside it (a file link, a folder link, an absolute one, a chain)
/// never loads, on the real WebKit worker; a link that stays inside the
/// folder, and a plain same-folder picture, still load. The round-6 rule
/// list matched URLs only, and WebKit followed `doc/link.png ->
/// ../secret.png` to the parent folder.
#[cfg(unix)]
#[test]
fn nfr_security_a_symlink_in_the_document_folder_cannot_escape_it() {
    if !can_render() {
        return;
    }
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().canonicalize().unwrap();
    let doc = parent.join("doc");
    std::fs::create_dir_all(doc.join("sub")).unwrap();
    png_of(&parent.join("secret.png"), [20, 20, 220]);
    png_of(&doc.join("inside.png"), [220, 20, 20]);
    png_of(&doc.join("sub").join("inside2.png"), [20, 220, 20]);
    // Out of the folder: relative, through a folder link, absolute, and a
    // chain whose second hop leaves.
    symlink("../secret.png", doc.join("link.png")).unwrap();
    symlink("..", doc.join("up")).unwrap();
    symlink(parent.join("secret.png"), doc.join("abs.png")).unwrap();
    symlink("hop2.png", doc.join("hop1.png")).unwrap();
    symlink("../secret.png", doc.join("hop2.png")).unwrap();
    // Inside the folder: a relative and an absolute link to the same file.
    symlink("sub/inside2.png", doc.join("alias.png")).unwrap();
    symlink(
        doc.join("sub").join("inside2.png"),
        doc.join("alias-abs.png"),
    )
    .unwrap();

    let page = doc.join("page.html");
    std::fs::write(
        &page,
        "<!doctype html><html><head><meta charset=utf-8></head><body>\
         <img src=\"inside.png\"><img src=\"alias.png\"><img src=\"alias-abs.png\">\
         <img src=\"link.png\"><img src=\"up/secret.png\"><img src=\"abs.png\">\
         <img src=\"hop1.png\"><img src=\"up/doc/link.png\"></body></html>",
    )
    .unwrap();
    let pdf = convert(&page, &PageSetup::default()).unwrap();
    let control = doc.join("control.html");
    std::fs::write(
        &control,
        "<!doctype html><html><head><meta charset=utf-8></head><body>\
         <img src=\"inside.png\"><img src=\"sub/inside2.png\"><img src=\"sub/inside2.png\">\
         <img src=\"m1.png\"><img src=\"m2.png\"><img src=\"m3.png\">\
         <img src=\"m4.png\"><img src=\"m5.png\"></body></html>",
    )
    .unwrap();
    let control_pdf = convert(&control, &PageSetup::default()).unwrap();
    let only_parent = doc.join("only-parent.html");
    std::fs::write(
        &only_parent,
        "<!doctype html><html><head><meta charset=utf-8></head><body>\
         <img src=\"link.png\"><img src=\"up/secret.png\"><img src=\"abs.png\">\
         <img src=\"hop1.png\"></body></html>",
    )
    .unwrap();
    let only_parent_pdf = convert(&only_parent, &PageSetup::default()).unwrap();

    assert!(
        images(&control_pdf) >= 2,
        "same-folder pictures load ({})",
        images(&control_pdf)
    );
    assert_eq!(
        images(&pdf),
        images(&control_pdf),
        "links inside the folder load, links out of it do not"
    );
    // The control: the same number of pictures that do not exist (a web
    // view may draw its broken-image icon for each).
    let missing = doc.join("missing.html");
    std::fs::write(
        &missing,
        "<!doctype html><html><head><meta charset=utf-8></head><body>\
         <img src=\"m1.png\"><img src=\"m2.png\"><img src=\"m3.png\">\
         <img src=\"m4.png\"></body></html>",
    )
    .unwrap();
    let missing_pdf = convert(&missing, &PageSetup::default()).unwrap();
    assert_eq!(
        images(&only_parent_pdf),
        images(&missing_pdf),
        "no picture behind an escaping link loads"
    );
}

/// The text of `pdf` as `pdftotext` (Poppler) extracts it, or `None` when
/// the tool is not installed (the text checks are then skipped; the page
/// counts and sizes still run).
fn pdf_text(pdf: &Path) -> Option<String> {
    // `-layout` keeps each printed line together. Without it pdftotext
    // guesses a reading order, and on WebView2's PDFs (every word placed on
    // its own) it reads them column by column: "Línea Línea … 1: 2: …".
    // The runs of spaces `-layout` pads with are folded to one, so a needle
    // matches whatever the column widths.
    let out = std::process::Command::new("pdftotext")
        .args(["-layout", "-enc", "UTF-8"])
        .arg(pdf)
        .arg("-")
        .output()
        .ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// The size of every page, in points.
fn page_sizes(pdf: &Path) -> Vec<(i64, i64)> {
    let doc = lopdf::Document::load(pdf).unwrap();
    doc.get_pages()
        .values()
        .map(|id| {
            let mut dict = doc.get_dictionary(*id).unwrap();
            let mb = loop {
                if let Ok(mb) = dict.get(b"MediaBox") {
                    break mb.as_array().unwrap().clone();
                }
                let parent = dict.get(b"Parent").unwrap().as_reference().unwrap();
                dict = doc.get_dictionary(parent).unwrap();
            };
            let n = |o: &lopdf::Object| {
                o.as_float()
                    .map(f64::from)
                    .or_else(|_| o.as_i64().map(|v| v as f64))
                    .unwrap()
            };
            (
                (n(&mb[2]) - n(&mb[0])).round() as i64,
                (n(&mb[3]) - n(&mb[1])).round() as i64,
            )
        })
        .collect()
}

/// Text in scripts other than Latin prints in all three web-view formats:
/// the words come back out of the PDF as text (not as pictures of glyphs),
/// so the fonts the web view picked cover them and are embedded. Cyrillic
/// and Greek are checked word for word (avoiding μ, which some fonts map to
/// the micro sign, U+00B5, so it extracts as another character); right-to-left scripts and CJK are
/// printed too, but extraction tools reorder or decompose them, so they are
/// only required not to break the render (CJK is checked when this system
/// has a font for it).
#[test]
fn fr_055_non_latin_text_prints_as_text_in_html_markdown_and_txt() {
    if !can_render() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let words = "Привет мир · Καλησπέρα Ελλάδα · ñandú çà · مرحبا بالعالم · שלום עולם · 你好世界 · こんにちは";
    let html = dir.path().join("scripts.html");
    std::fs::write(
        &html,
        format!(
            "<!doctype html><html lang=und><head><meta charset=utf-8><title>t</title></head>\
             <body><h1>Ελληνικά</h1><p>{words}</p><p dir=rtl>مرحبا بالعالم</p></body></html>"
        ),
    )
    .unwrap();
    let md = dir.path().join("scripts.md");
    std::fs::write(
        &md,
        format!("# Ελληνικά\n\n{words}\n\n| Кириллица | Ελληνικά |\n|---|---|\n| да | ναι |\n"),
    )
    .unwrap();
    let txt = dir.path().join("scripts.txt");
    std::fs::write(&txt, format!("Ελληνικά\n{words}\n")).unwrap();

    let cjk_font = cfg!(not(target_os = "linux"))
        || std::process::Command::new("fc-list")
            .arg(":lang=zh")
            .output()
            .is_ok_and(|o| !o.stdout.is_empty());
    for input in [&html, &md, &txt] {
        let pdf = convert(input, &a4()).unwrap();
        assert_eq!(pages(&pdf), 1, "{}", input.display());
        assert_eq!(first_page_size(&pdf), (595, 842), "{}", input.display());
        let Some(text) = pdf_text(&pdf) else {
            continue;
        };
        for needle in ["Привет", "мир", "Καλησπέρα", "Ελλάδα", "Ελληνικά", "ñandú"]
        {
            assert!(
                text.contains(needle),
                "{}: {needle} is printed as text: {text}",
                input.display()
            );
        }
        if cjk_font {
            assert!(
                text.contains("你好世界"),
                "{}: CJK is printed as text: {text}",
                input.display()
            );
        }
    }
}

/// A page that declares no charset prints in the encoding its bytes are
/// in: UTF-8 when they are valid UTF-8, Windows-1252 otherwise. The staged
/// copy is re-encoded as UTF-8 with a BOM, and the private scheme answers
/// with the sniffed charset as well; this pins the end result on every
/// platform (a review fed the bare Linux worker such a page and got Latin-1
/// mojibake, because the scheme's answer then dropped the charset).
#[test]
fn fr_057_a_page_without_meta_charset_prints_in_its_own_encoding() {
    if !can_render() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let utf8 = dir.path().join("utf8.html");
    std::fs::write(
        &utf8,
        "<!doctype html><p>café Ελληνικά naïve</p>".as_bytes(),
    )
    .unwrap();
    let latin = dir.path().join("cp1252.html");
    std::fs::write(&latin, b"<!doctype html><p>caf\xe9 na\xefve \x80uro</p>").unwrap();
    for (input, needles) in [
        (&utf8, &["café", "Ελληνικά", "naïve"][..]),
        (&latin, &["café", "naïve", "€uro"][..]),
    ] {
        let pdf = convert(input, &a4()).unwrap();
        let Some(text) = pdf_text(&pdf) else {
            continue;
        };
        for needle in needles {
            assert!(
                text.contains(needle),
                "{}: {needle} is printed as text: {text}",
                input.display()
            );
        }
    }
}

/// A long document of each web-view format paginates onto many pages of
/// the job's paper, every page the same size, with its last line printed.
#[test]
fn fr_055_long_documents_paginate_on_the_jobs_paper() {
    if !can_render() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let html = dir.path().join("long.html");
    let paragraphs: String = (1..=600)
        .map(|i| format!("<p>Párrafo número {i} de un documento largo.</p>"))
        .collect();
    std::fs::write(
        &html,
        format!("<!doctype html><html><head><meta charset=utf-8></head><body>{paragraphs}</body></html>"),
    )
    .unwrap();
    let md = dir.path().join("long.md");
    let sections: String = (1..=120)
        .map(|i| {
            format!("## Sección {i}\n\nTexto de la sección {i}, con *énfasis*.\n\n- uno\n- dos\n\n")
        })
        .collect();
    std::fs::write(&md, sections).unwrap();
    let txt = dir.path().join("long.txt");
    let lines: String = (1..=1000).map(|i| format!("Renglón {i}\n")).collect();
    std::fs::write(&txt, lines).unwrap();

    for (input, last) in [
        (&html, "Párrafo número 600 de"),
        (&md, "Sección 120"),
        (&txt, "Renglón 1000"),
    ] {
        let pdf = convert(input, &a4()).unwrap();
        let sizes = page_sizes(&pdf);
        assert!(
            sizes.len() >= 5,
            "{}: {} pages",
            input.display(),
            sizes.len()
        );
        assert!(
            sizes.iter().all(|s| *s == (595, 842)),
            "{}: every page is A4: {sizes:?}",
            input.display()
        );
        if let Some(text) = pdf_text(&pdf) {
            assert!(
                text.contains(last),
                "{}: {last} is printed",
                input.display()
            );
        }
    }
}

/// Linux: with no X11 or Wayland session the worker does not try to start
/// GTK; it fails at once, with the usage-free failure code and the marker
/// the engine can tell a missing display by.
#[cfg(target_os = "linux")]
#[test]
fn fr_055_without_a_display_the_worker_fails_fast_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("page.html");
    std::fs::write(&page, "<p>x</p>").unwrap();
    let started = Instant::now();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_pluryprint"))
        .args(["--worker", "render-pdf", "--page"])
        .arg(&page)
        .arg("--base")
        .arg(dir.path())
        .arg("--out")
        .arg(dir.path().join("out.pdf"))
        .arg("--data")
        .arg(dir.path().join("webview"))
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(pluryprint_lib::render::NO_DISPLAY_MARKER),
        "{stderr}"
    );
    assert!(!dir.path().join("out.pdf").exists());
    assert!(started.elapsed() < Duration::from_secs(10));
}

/// A 1-bit greyscale PNG of `side` × `side` blank pixels: a few dozen KB on
/// disk, `side`² × 4 bytes once the web view decodes it.
#[cfg(target_os = "linux")]
fn png_bomb(path: &Path, side: u32) {
    use std::io::Write;
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(data);
        out.extend_from_slice(&crc.sum().to_be_bytes());
    }
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&side.to_be_bytes());
    ihdr.extend_from_slice(&side.to_be_bytes());
    ihdr.extend_from_slice(&[1, 0, 0, 0, 0]); // 1-bit greyscale
    let row = vec![0u8; 1 + (side as usize).div_ceil(8)];
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    for _ in 0..side {
        z.write_all(&row).unwrap();
    }
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &z.finish().unwrap());
    chunk(&mut png, b"IEND", &[]);
    std::fs::write(path, png).unwrap();
}

/// The page every memory test prints: a 20 000 px square image, which the
/// web process decodes into 1.6 GB and holds twice over while it paints
/// (measured: 3.2 GB of resident set, under Xvfb). Scripts are off in the
/// worker, so a page cannot allocate from JavaScript; an image is how a page
/// takes memory without one.
#[cfg(target_os = "linux")]
fn memory_bomb_page(dir: &Path) -> PathBuf {
    png_bomb(&dir.join("bomb.png"), 20_000);
    let page = dir.join("bomb.html");
    std::fs::write(
        &page,
        "<!doctype html><meta charset=\"utf-8\"><p>bomb</p>\
         <img src=\"bomb.png\" style=\"width:20000px;height:20000px\">",
    )
    .unwrap();
    page
}

/// ADR-0014, round 7, on Linux, with the real worker: a page whose web
/// process outgrows the render cap is stopped by the engine's watchdog over
/// the worker's tree (the web process is the worker's child, not the worker
/// itself) and the job fails with the terminal `page_too_large`, naming the
/// cap, in well under the conversion timeout.
#[cfg(target_os = "linux")]
#[test]
fn nfr_security_a_page_over_the_render_cap_is_stopped_as_page_too_large() {
    if !can_render() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let page = memory_bomb_page(dir.path());
    let started = Instant::now();
    let err = convert(&page, &a4()).unwrap_err();
    match &err {
        EngineError::ConversionFailed(f) => {
            assert_eq!(f.reason, Some("page_too_large"), "{f:?}");
            assert_eq!(f.params["limit"], "2048", "{f:?}");
        }
        other => panic!("{other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(60));
}

/// The same page, with the worker started by hand under a 1 GB cap and no
/// engine watching it (a crashed app): WebKit's own limit, which the worker
/// sets to its cap, kills the web process, and the worker says it ran out of
/// memory in the words the engine reads as `page_too_large`.
#[cfg(target_os = "linux")]
#[test]
fn nfr_security_webkits_own_limit_holds_the_web_process_without_a_watchdog() {
    if !can_render() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let page = memory_bomb_page(dir.path());
    // The bare worker serves the page from its stage path: the image needs
    // the document folder as its base, which the engine's staging adds.
    let html = std::fs::read_to_string(&page).unwrap().replace(
        "<p>bomb",
        &format!(
            "<base href=\"{}\"><p>bomb",
            pluryprint_lib::render::base_href(dir.path())
        ),
    );
    std::fs::write(&page, html).unwrap();
    let started = Instant::now();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_pluryprint"))
        .args(["--worker", "render-pdf", "--page"])
        .arg(&page)
        .arg("--base")
        .arg(dir.path())
        .arg("--out")
        .arg(dir.path().join("out.pdf"))
        .arg("--data")
        .arg(dir.path().join("webview"))
        .env(
            pluryprint_lib::engine::convert::process::MEMORY_CAP_ENV,
            (1024u64 * 1024 * 1024).to_string(),
        )
        .env("PLURYPRINT_RENDER_TRACE", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("memory allocation of") && stderr.contains("failed"),
        "{stderr}"
    );
    assert!(
        stderr.contains("web process memory limit: 1024 MB"),
        "{stderr}"
    );
    assert!(!dir.path().join("out.pdf").exists());
    assert!(started.elapsed() < Duration::from_secs(60));
}

// ---------------------------------------------------------------------------
// FR-057a / FR-057b: XML through the real web view
// ---------------------------------------------------------------------------

/// A fictitious SRI receipt of `tests/fixtures/sri-xml/` and what its print
/// must look like.
struct XmlCase {
    name: &'static str,
    /// The exact number of A4 pages: a pagination regression fails here
    /// even without `pdftotext`.
    pages: usize,
    /// Words the printed pages must carry.
    needles: &'static [&'static str],
    /// Whether the page ends with the RIDE footer (only on the last page).
    footer: bool,
    /// A word of the closing blocks (totals or additional information) that
    /// must share the last page with the footer, which is never alone on
    /// it; `None` when the receipt has no closing block.
    closing: Option<&'static str>,
}

const SRI_FIXTURES: &[XmlCase] = &[
    XmlCase {
        name: "factura-1.1.0-autorizacion.xml",
        pages: 1,
        needles: &["Factura", "001-002-000000123", "207.88", "Subtotal 15%"],
        footer: true,
        closing: Some("Valor total"),
    },
    XmlCase {
        name: "factura-1.0.0-respuesta-soap.xml",
        pages: 1,
        needles: &["Factura", "IVA 12%", "11.20"],
        footer: true,
        closing: Some("Valor total"),
    },
    XmlCase {
        name: "factura-2.0.0-no-autorizado.xml",
        pages: 1,
        needles: &["NO AUTORIZADO"],
        footer: true,
        closing: Some("Valor total"),
    },
    XmlCase {
        name: "factura-2.1.0.xml",
        pages: 1,
        needles: &["IVA 5%", "ICE", "46.73"],
        footer: true,
        closing: Some("Valor total"),
    },
    XmlCase {
        name: "nota-credito-1.1.0.xml",
        pages: 1,
        needles: &["Nota de crédito", "41.69"],
        footer: true,
        closing: Some("Valor de la modificación"),
    },
    XmlCase {
        name: "nota-credito-1.0.0.xml",
        pages: 1,
        needles: &["Nota de crédito", "11.50"],
        footer: true,
        closing: Some("Valor de la modificación"),
    },
    XmlCase {
        name: "nota-debito-1.0.0.xml",
        pages: 1,
        needles: &["Nota de débito", "13.80", "Gastos de cobranza"],
        footer: true,
        closing: Some("Valor total"),
    },
    XmlCase {
        name: "retencion-1.0.0.xml",
        pages: 1,
        needles: &["Comprobante de retención", "62.50", "001-001-000004567"],
        footer: true,
        closing: Some("Total retenido (calculado)"),
    },
    XmlCase {
        name: "retencion-2.0.0.xml",
        pages: 1,
        needles: &["Comprobante de retención", "40.38"],
        footer: true,
        closing: Some("Total retenido (calculado)"),
    },
    // Two recipients overflow the first page: the closing blocks and the
    // footer move to the second one together (8530b29).
    XmlCase {
        name: "guia-remision-1.1.0.xml",
        pages: 2,
        needles: &["Guía de remisión", "GAA-0000", "Guayaquil - Cuenca"],
        footer: true,
        closing: Some("Información adicional"),
    },
    XmlCase {
        name: "guia-remision-1.0.0.xml",
        pages: 1,
        needles: &["Guía de remisión", "GBB-1111"],
        footer: true,
        closing: None,
    },
    XmlCase {
        name: "liquidacion-compra-1.1.0.xml",
        pages: 1,
        needles: &["Liquidación de compra", "300.00"],
        footer: true,
        closing: Some("Valor total"),
    },
    XmlCase {
        name: "liquidacion-compra-1.0.0.xml",
        pages: 1,
        needles: &["Liquidación de compra", "50.00"],
        footer: true,
        closing: Some("Valor total"),
    },
    XmlCase {
        name: "catalogo-generico.xml",
        pages: 1,
        needles: &["cat:catalogo", "Taladro percutor"],
        footer: false,
        closing: None,
    },
];

/// The text of page `page` (1-based) of `pdf`, as [`pdf_text`] reads it.
fn pdf_page_text(pdf: &Path, page: usize) -> Option<String> {
    let n = page.to_string();
    let out = std::process::Command::new("pdftotext")
        .args(["-layout", "-enc", "UTF-8", "-f", &n, "-l", &n])
        .arg(pdf)
        .arg("-")
        .output()
        .ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// Whether `text` carries the RIDE footer, in either app language.
fn has_ride_footer(text: &str) -> bool {
    text.contains("Representación impresa generada por Pluryprint")
        || text.contains("Printed representation generated by Pluryprint")
}

/// Where `PLURYPRINT_XML_SAMPLES` asks the sample PDFs to be kept (for the
/// docs and the PR), if anywhere.
fn keep_sample(pdf: &Path, name: &str) {
    if let Some(dir) = std::env::var_os("PLURYPRINT_XML_SAMPLES") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(pdf, dir.join(name.replace(".xml", ".pdf"))).unwrap();
    }
}

#[test]
fn fr_057b_sri_receipts_print_as_ride_pages_on_a4() {
    if !can_render() {
        return;
    }
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sri-xml");
    let dir = tempfile::tempdir().unwrap();
    let mut text_checked = true;
    for case in SRI_FIXTURES {
        let name = case.name;
        let input = dir.path().join(name);
        std::fs::copy(fixtures.join(name), &input).unwrap();
        // Paper `auto`: an XML page is laid out on A4.
        let pdf = convert(&input, &PageSetup::default()).unwrap();
        assert_eq!(pages(&pdf), case.pages, "{name}: page count");
        assert_eq!(first_page_size(&pdf), (595, 842), "{name}: A4 by default");
        let Some(text) = pdf_text(&pdf) else {
            text_checked = false;
            keep_sample(&pdf, name);
            continue;
        };
        for needle in case.needles {
            assert!(
                text.contains(needle),
                "{name}: {needle} is printed:\n{text}"
            );
        }
        if case.footer {
            // The footer is on the last page, with the closing blocks.
            let last = pdf_page_text(&pdf, case.pages).unwrap();
            assert!(
                has_ride_footer(&last),
                "{name}: footer on the last page:\n{last}"
            );
            if let Some(closing) = case.closing {
                assert!(
                    last.contains(closing),
                    "{name}: {closing} shares the last page with the footer:\n{last}"
                );
            }
            for page in 1..case.pages {
                let earlier = pdf_page_text(&pdf, page).unwrap();
                assert!(
                    !has_ride_footer(&earlier),
                    "{name}: footer only once, at the end"
                );
            }
        }
        keep_sample(&pdf, name);
    }
    if !text_checked {
        // Page counts were still checked; the words need Poppler.
        eprintln!("pdftotext not found: printed words and footer placement not checked");
    }
}

#[test]
fn fr_057b_an_xml_page_follows_the_jobs_paper_and_fetches_nothing() {
    if !can_render() {
        return;
    }
    let wire = Tripwire::new();
    let port = wire.port();
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("hostil.xml");
    std::fs::write(
        &input,
        format!(
            "<?xml-stylesheet href=\"http://127.0.0.1:{port}/x.xsl\"?>\
             <r xmlns:xi=\"http://www.w3.org/2001/XInclude\">\
             <xi:include href=\"http://127.0.0.1:{port}/inc\"/>\
             <img src=\"http://127.0.0.1:{port}/img.png\"/>\
             <t>&lt;img src=\"http://127.0.0.1:{port}/esc.png\"&gt;</t></r>"
        ),
    )
    .unwrap();
    let letter = PageSetup {
        css_size: Some("8.5in 11in".into()),
    };
    let pdf = convert(&input, &letter).unwrap();
    assert_eq!(first_page_size(&pdf), (612, 792), "the job's paper wins");
    std::thread::sleep(Duration::from_millis(300));
    wire.assert_untouched();
}

#[test]
fn fr_057a_a_hostile_xml_fails_with_its_reason_before_rendering() {
    setup();
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("lolz.xml");
    std::fs::write(
        &input,
        "<!DOCTYPE l [<!ENTITY a \"aaaa\"><!ENTITY b \"&a;&a;&a;&a;\">]><l>&b;</l>",
    )
    .unwrap();
    let err = convert(&input, &a4()).unwrap_err();
    assert!(
        matches!(&err, EngineError::ConversionFailed(f) if f.reason == Some("xml_dtd_refused")),
        "{err:?}"
    );
    assert!(!dir.path().join("cache").join("job-lolz.pdf").exists());
}
