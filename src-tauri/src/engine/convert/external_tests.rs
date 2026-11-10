//! Tests for the converters that shell out (FR-052..FR-058).
//!
//! Two layers:
//!
//! - **Fake binaries** (Unix only — they are `/bin/sh` scripts): pin the
//!   argument vectors actually spawned, the "exit 0 but no output" case, the
//!   timeout kill and the scratch-folder cleanup without needing any real
//!   converter installed.
//! - **Real tools**: convert genuine fixtures generated in-test (nothing binary
//!   is committed). Each test locates its tool the same way the engine does and
//!   **skips with a note** when it is absent, so a CI runner without
//!   LibreOffice stays green while a developer machine with the tools runs the
//!   real conversion.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::*;

fn pages_of(pdf: &Path) -> usize {
    lopdf::Document::load(pdf)
        .unwrap_or_else(|e| panic!("{} must parse as a PDF: {e}", pdf.display()))
        .get_pages()
        .len()
}

/// The tool, or `None` with a printed note (the test then returns early).
fn tool_or_skip(dep: Dependency, test: &str) -> Option<PathBuf> {
    match locate(dep) {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("SKIPPED {test}: {e}");
            None
        }
    }
}

fn no_progress() -> impl Fn(f32) + Send + Sync {
    |_p: f32| {}
}

// ---------------------------------------------------------------------------
// Fake binaries
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod fakes {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Write an executable `/bin/sh` script to `dir/name`.
    pub fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// A fake `soffice` that records its argv and writes `<outdir>/<stem>.pdf`.
    fn fake_soffice(dir: &Path, argv_log: &Path) -> PathBuf {
        script(
            dir,
            "soffice",
            &format!(
                r#"printf '%s\n' "$@" > '{log}'
out=""; prev=""; last=""
for a in "$@"; do
  if [ "$prev" = "--outdir" ]; then out="$a"; fi
  prev="$a"; last="$a"
done
base=$(basename "$last"); stem="${{base%.*}}"
printf '%%PDF-1.4\n%%fake\n' > "$out/$stem.pdf""#,
                log = argv_log.display()
            ),
        )
    }

    #[test]
    fn fr_053_office_spawns_soffice_with_a_private_profile_and_returns_its_pdf() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("argv");
        let soffice = fake_soffice(dir.path(), &log);
        let input = dir.path().join("Informe $(whoami) final.docx");
        std::fs::write(&input, b"PK fake docx").unwrap();
        let output = dir.path().join("out.pdf");
        let work = WorkDir::new("job").unwrap();
        let cancel = AtomicBool::new(false);

        office::convert_with(&soffice, &input, &output, &work, &cancel, None).unwrap();

        assert!(validate_pdf_header(&output).is_ok());
        let argv = std::fs::read_to_string(&log).unwrap();
        let argv: Vec<&str> = argv.lines().collect();
        assert!(
            argv[0].starts_with("-env:UserInstallation=file://")
                && argv[0].contains("pluryprint-convert-job-"),
            "{argv:?}"
        );
        assert!(argv.contains(&"--headless"));
        // The hostile-looking original name never reaches the argv: the
        // converter works on its own staged copy.
        assert!(argv.last().unwrap().ends_with("input.docx"), "{argv:?}");
        assert!(!argv.iter().any(|a| a.contains("whoami")));
    }

    #[test]
    fn fr_053_soffice_exiting_zero_without_output_is_a_conversion_failure() {
        let dir = tempfile::tempdir().unwrap();
        let soffice = script(
            dir.path(),
            "soffice",
            "echo 'Error: source file could not be loaded' >&2; exit 0",
        );
        let input = dir.path().join("broken.odt");
        std::fs::write(&input, b"nope").unwrap();
        let work = WorkDir::new("job").unwrap();
        let cancel = AtomicBool::new(false);
        let err = office::convert_with(
            &soffice,
            &input,
            &dir.path().join("o.pdf"),
            &work,
            &cancel,
            None,
        )
        .unwrap_err();
        assert!(
            matches!(&err, EngineError::ConversionFailed(m) if m.contains("could not be loaded")),
            "{err:?}"
        );
    }

    #[test]
    fn fr_053_a_hung_converter_is_killed_at_the_timeout_and_its_folder_removed() {
        let dir = tempfile::tempdir().unwrap();
        let soffice = script(dir.path(), "soffice", "sleep 30");
        let input = dir.path().join("slow.docx");
        std::fs::write(&input, b"x").unwrap();
        let work = WorkDir::new("job").unwrap();
        let scratch = work.path().to_path_buf();
        let cancel = AtomicBool::new(false);
        let started = std::time::Instant::now();
        let err = office::convert_with(
            &soffice,
            &input,
            &dir.path().join("o.pdf"),
            &work,
            &cancel,
            Some(Duration::from_millis(300)),
        )
        .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(
            matches!(&err, EngineError::ConversionTimedOut(m) if m.contains("did not finish")),
            "{err:?}"
        );
        drop(work);
        assert!(
            !scratch.exists(),
            "the scratch folder must not outlive the job"
        );
    }

    /// A fake render worker: records its argv and the staged page it was
    /// given, and writes a PDF at `--out`.
    fn fake_worker(dir: &Path) -> PathBuf {
        let argv = dir.join("worker-argv");
        let page = dir.join("worker-page");
        script(
            dir,
            "pluryprint",
            &format!(
                r#"printf '%s\n' "$@" > '{argv}'
prev=""
for a in "$@"; do
  case "$prev" in
    --page) cp "$a" '{page}';;
    --out) printf '%%PDF-1.4\n%%%%EOF\n' > "$a";;
  esac
  prev="$a"
done"#,
                argv = argv.display(),
                page = page.display()
            ),
        )
    }

    /// FR-055: the default renderer is the hidden worker of this executable,
    /// given a staged page (CSP, base at the document folder, paper) and the
    /// paper in inches; no browser and no Pandoc are looked up.
    #[test]
    fn fr_055_html_renders_without_browser_through_the_worker() {
        let dir = tempfile::tempdir().unwrap();
        let worker = fake_worker(dir.path());
        let docs = dir.path().join("my docs");
        std::fs::create_dir_all(&docs).unwrap();
        let output = dir.path().join("out.pdf");
        let work = WorkDir::new("job").unwrap();
        let a4 = PageSetup {
            css_size: Some("210mm 297mm".into()),
        };
        html::render_system_with(
            &worker,
            b"<html><head><title>t</title></head><body><script>x()</script></body></html>",
            &docs,
            &output,
            &work,
            &AtomicBool::new(false),
            None,
            &a4,
        )
        .unwrap();
        assert!(validate_pdf_header(&output).is_ok());

        let argv: Vec<String> = std::fs::read_to_string(dir.path().join("worker-argv"))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        assert_eq!(argv[0], "--worker");
        assert_eq!(argv[1], "render-pdf");
        let value = |flag: &str| {
            let at = argv.iter().position(|a| a == flag).unwrap();
            argv[at + 1].clone()
        };
        assert_eq!(value("--base"), docs.to_string_lossy());
        assert!(value("--page").ends_with("page.html"));
        assert!(value("--data").starts_with(&*work.path().to_string_lossy()));
        let (w, h) = value("--paper-in")
            .split_once('x')
            .map(|(w, h)| (w.parse::<f64>().unwrap(), h.parse::<f64>().unwrap()))
            .unwrap();
        assert!(
            (w - 8.27).abs() < 0.01 && (h - 11.69).abs() < 0.01,
            "{w}x{h}"
        );

        let staged = std::fs::read_to_string(dir.path().join("worker-page")).unwrap();
        assert!(staged.contains("script-src 'none'"), "{staged}");
        assert!(
            staged.contains(&format!(
                "<base href=\"{}\">",
                crate::render::base_href(&docs)
            )),
            "{staged}"
        );
        assert!(staged.contains("@page { size: 210mm 297mm; }"), "{staged}");
    }

    /// A worker that fails reports `render_failed` with its stderr as the
    /// detail, and leaves no output behind.
    #[test]
    fn fr_055_a_failing_worker_is_render_failed() {
        let dir = tempfile::tempdir().unwrap();
        let worker = script(
            dir.path(),
            "pluryprint",
            "echo 'pluryprint worker render-pdf: the page did not finish loading' >&2; exit 1",
        );
        let output = dir.path().join("out.pdf");
        let work = WorkDir::new("job").unwrap();
        let err = html::render_system_with(
            &worker,
            b"<p>x",
            dir.path(),
            &output,
            &work,
            &AtomicBool::new(false),
            None,
            &PageSetup::default(),
        )
        .unwrap_err();
        match &err {
            EngineError::ConversionFailed(f) => {
                assert_eq!(f.reason, Some("render_failed"));
                assert!(f.detail.contains("did not finish loading"), "{f:?}");
            }
            other => panic!("{other:?}"),
        }
        assert!(!output.exists());
    }

    /// ADR-0014, round 7: the render worker runs under the same memory cap
    /// as the SVG worker, over its whole process tree: it is told the cap,
    /// and a helper of it that grows past the cap ends the job with the
    /// terminal `page_too_large`, naming the cap.
    #[test]
    fn nfr_security_render_worker_runs_under_a_memory_cap() {
        let dir = tempfile::tempdir().unwrap();
        let seen = dir.path().join("cap");
        let worker = script(
            dir.path(),
            "pluryprint",
            &format!(
                "echo \"$PLURYPRINT_WORKER_MEMORY_CAP\" > '{}'; \
                 perl -e '$x = q(a) x (400*1024*1024); sleep 30'; exit 0",
                seen.display()
            ),
        );
        let output = dir.path().join("out.pdf");
        let work = WorkDir::new("job").unwrap();
        let started = std::time::Instant::now();
        let err = html::render_system_capped(
            &worker,
            b"<p>x",
            dir.path(),
            &output,
            &work,
            &AtomicBool::new(false),
            Some(Duration::from_secs(60)),
            &PageSetup::default(),
            64 * 1024 * 1024,
        )
        .unwrap_err();
        match &err {
            EngineError::ConversionFailed(f) => {
                assert_eq!(f.reason, Some("page_too_large"), "{f:?}");
                assert_eq!(f.params["limit"], "64");
            }
            other => panic!("{other:?}"),
        }
        assert!(crate::util::reasons::is_terminal_conversion_reason(
            "page_too_large"
        ));
        assert!(started.elapsed() < Duration::from_secs(30));
        assert_eq!(
            std::fs::read_to_string(&seen).unwrap().trim(),
            (64u64 * 1024 * 1024).to_string()
        );
        assert!(!output.exists());
        assert_eq!(html::RENDER_WORKER_MEMORY_CAP, 2 * 1024 * 1024 * 1024);
    }

    /// A hung worker is killed at the timeout like any converter.
    #[test]
    fn fr_055_a_hung_worker_is_killed_at_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let worker = script(dir.path(), "pluryprint", "sleep 30");
        let work = WorkDir::new("job").unwrap();
        let started = std::time::Instant::now();
        let err = html::render_system_with(
            &worker,
            b"<p>x",
            dir.path(),
            &dir.path().join("out.pdf"),
            &work,
            &AtomicBool::new(false),
            Some(Duration::from_millis(300)),
            &PageSetup::default(),
        )
        .unwrap_err();
        assert!(matches!(err, EngineError::ConversionTimedOut(_)), "{err:?}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    /// FR-056: Markdown goes through the same worker as HTML: no Pandoc.
    #[test]
    fn fr_056_markdown_is_rendered_by_the_worker_not_pandoc() {
        let dir = tempfile::tempdir().unwrap();
        let worker = fake_worker(dir.path());
        let html = markdown::to_html("# Hi\n\n<script>x()</script>\n", "notes");
        let work = WorkDir::new("job").unwrap();
        html::render_system_with(
            &worker,
            html.as_bytes(),
            dir.path(),
            &dir.path().join("o.pdf"),
            &work,
            &AtomicBool::new(false),
            None,
            &PageSetup::default(),
        )
        .unwrap();
        let staged = std::fs::read_to_string(dir.path().join("worker-page")).unwrap();
        assert!(staged.contains("<h1>Hi</h1>"), "{staged}");
        assert!(!staged.contains("x()"), "{staged}");
    }

    #[test]
    fn fr_052_sips_path_turns_the_png_it_writes_into_a_one_page_pdf() {
        let dir = tempfile::tempdir().unwrap();
        // A fake `sips` that "decodes" by copying a real PNG to --out.
        let png = dir.path().join("real.png");
        ::image::RgbImage::from_pixel(4, 3, ::image::Rgb([200, 10, 10]))
            .save_with_format(&png, ::image::ImageFormat::Png)
            .unwrap();
        let sips = script(
            dir.path(),
            "sips",
            &format!(
                r#"prev=""
for a in "$@"; do
  if [ "$prev" = "--out" ]; then cp '{png}' "$a"; fi
  prev="$a"
done"#,
                png = png.display()
            ),
        );
        let heic = dir.path().join("IMG_0001.HEIC");
        std::fs::write(&heic, b"\0\0\0\x18ftypheic").unwrap();
        let output = dir.path().join("o.pdf");
        let work = WorkDir::new("job").unwrap();
        let cancel = AtomicBool::new(false);
        heif::convert_with_sips(&sips, &heic, &output, &work, &cancel, None, &no_progress())
            .unwrap();
        assert_eq!(pages_of(&output), 1);
    }

    /// A fake external HEIF decoder named `name` that records its argv and
    /// "decodes" by copying a real PNG to `target(argv)`, a shell expression.
    fn fake_heif_decoder(dir: &Path, name: &str, argv_log: &Path, target: &str) -> PathBuf {
        let png = dir.join("real.png");
        ::image::RgbImage::from_pixel(4, 3, ::image::Rgb([10, 200, 10]))
            .save_with_format(&png, ::image::ImageFormat::Png)
            .unwrap();
        script(
            dir,
            name,
            &format!(
                r#"printf '%s\n' "$@" > '{log}'
cp '{png}' {target}"#,
                log = argv_log.display(),
                png = png.display(),
            ),
        )
    }

    /// FR-052 on Linux: libheif's tool gets `<in> <out>`, ImageMagick gets
    /// the first frame and an explicit PNG output, and either PNG becomes a
    /// one-page PDF.
    #[test]
    fn fr_052_external_heif_decoders_turn_their_png_into_a_one_page_pdf() {
        for (name, target, expect_in, expect_out) in [
            ("heif-convert", r#""$2""#, "{in}", "{png}"),
            ("magick", r#""${2#png:}""#, "{in}[0]", "png:{png}"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let log = dir.path().join("argv");
            let decoder = fake_heif_decoder(dir.path(), name, &log, target);
            let heic = dir.path().join("IMG 0001.HEIC");
            std::fs::write(&heic, b"\0\0\0\x18ftypheic").unwrap();
            let output = dir.path().join("o.pdf");
            let work = WorkDir::new("job").unwrap();
            heif::convert_with_external(
                &decoder,
                &heic,
                &output,
                &work,
                &AtomicBool::new(false),
                None,
                &no_progress(),
            )
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(pages_of(&output), 1, "{name}");
            let png = work.path().join("heif.png").display().to_string();
            let input = heic.display().to_string();
            let argv: Vec<String> = std::fs::read_to_string(&log)
                .unwrap()
                .lines()
                .map(String::from)
                .collect();
            assert_eq!(
                argv,
                [
                    expect_in.replace("{in}", &input),
                    expect_out.replace("{png}", &png)
                ],
                "{name}"
            );
        }
    }

    /// A decoder that runs and writes nothing could not read the file: a
    /// conversion failure naming the tool, not a paused job.
    #[test]
    fn fr_052_an_external_decoder_that_writes_nothing_is_a_decode_failure() {
        let dir = tempfile::tempdir().unwrap();
        let decoder = script(dir.path(), "heif-convert", "echo 'bad file' >&2");
        let heic = dir.path().join("x.heic");
        std::fs::write(&heic, b"x").unwrap();
        let work = WorkDir::new("job").unwrap();
        let err = heif::convert_with_external(
            &decoder,
            &heic,
            &dir.path().join("o.pdf"),
            &work,
            &AtomicBool::new(false),
            None,
            &no_progress(),
        )
        .unwrap_err();
        assert!(
            matches!(&err, EngineError::ConversionFailed(f)
                if f.reason == Some("heif_decode_failed")
                    && f.params["tool"] == "libheif"
                    && f.contains("bad file")),
            "{err:?}"
        );
    }

    /// A decoder that vanished between being found and being started pauses
    /// the job with the `heif-decoder` remediation (FR-058).
    #[test]
    fn fr_058_a_vanished_external_decoder_pauses_with_its_remediation() {
        let dir = tempfile::tempdir().unwrap();
        let heic = dir.path().join("x.heic");
        std::fs::write(&heic, b"x").unwrap();
        let work = WorkDir::new("job").unwrap();
        let err = heif::convert_with_external(
            &dir.path().join("heif-dec"),
            &heic,
            &dir.path().join("o.pdf"),
            &work,
            &AtomicBool::new(false),
            None,
            &no_progress(),
        )
        .unwrap_err();
        assert!(
            matches!(&err, EngineError::DependencyMissing(f) if f.reason == Some("heif-decoder")),
            "{err:?}"
        );
        assert!(crate::util::reasons::is_job_reason(
            "dependency_missing",
            "heif-decoder"
        ));
    }

    /// FR-053 on Linux: a conversion folder can live outside the system temp
    /// folder (for a sandboxed LibreOffice), is created when missing, and is
    /// removed with the job like any other.
    #[test]
    fn fr_053_a_work_dir_in_a_given_folder_is_created_and_cleaned_up() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("snap").join("libreoffice").join("common");
        let work = WorkDir::new_in("job-1", &parent).unwrap();
        let path = work.path().to_path_buf();
        assert!(path.starts_with(&parent) && path.is_dir());
        drop(work);
        assert!(!path.exists());
        assert!(parent.is_dir());
    }
}

// ---------------------------------------------------------------------------
// Real tools
// ---------------------------------------------------------------------------

/// Generous: LibreOffice's first start with a fresh profile is slow, and
/// these tests run in parallel with the rest of the suite.
const REAL_TIMEOUT: Option<Duration> = Some(Duration::from_secs(180));

fn convert_real(input: &Path, cache: &Path, job: &str) -> PathBuf {
    convert_real_on(input, cache, job, &PageSetup::default())
}

fn convert_real_on(input: &Path, cache: &Path, job: &str, page: &PageSetup) -> PathBuf {
    let cancel = AtomicBool::new(false);
    convert_to_pdf(
        input,
        cache,
        job,
        &cancel,
        &no_progress(),
        REAL_TIMEOUT,
        page,
        None,
    )
    .unwrap_or_else(|e| panic!("convert {}: {e}", input.display()))
    .expect("a converted document yields a cache PDF")
}

/// RTF goes to LibreOffice where the OS has no converter of its own (on
/// macOS it is `textutil`, tested with the render worker in
/// `tests/render_worker.rs`).
#[cfg(not(target_os = "macos"))]
#[test]
fn fr_057_real_rtf_converts_through_libreoffice() {
    if tool_or_skip(Dependency::LibreOffice, "rtf").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let rtf = dir.path().join("memo.rtf");
    std::fs::write(
        &rtf,
        r"{\rtf1\ansi{\fonttbl\f0 Helvetica;}\f0\b Hello\b0  RTF.\par}",
    )
    .unwrap();
    let pdf = convert_real(&rtf, &dir.path().join("cache"), "real-rtf");
    assert_eq!(pages_of(&pdf), 1);
}

#[test]
fn fr_053_real_docx_made_by_libreoffice_converts_back_to_pdf() {
    let Some(soffice) = tool_or_skip(Dependency::LibreOffice, "docx") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let txt = dir.path().join("source.txt");
    std::fs::write(&txt, "A Word document generated for the test.\n").unwrap();

    // Build the .docx fixture with LibreOffice itself (txt → docx).
    let work = WorkDir::new("fixture").unwrap();
    let profile = work.path().join("profile");
    let mut cmd = std::process::Command::new(&soffice);
    cmd.args([
        format!("-env:UserInstallation={}", file_url(&profile)),
        "--headless".into(),
        "--norestore".into(),
        "--convert-to".into(),
        "docx:MS Word 2007 XML".into(),
        "--outdir".into(),
        dir.path().to_string_lossy().into_owned(),
        txt.to_string_lossy().into_owned(),
    ]);
    let cancel = AtomicBool::new(false);
    process::run_bounded(
        cmd,
        "LibreOffice",
        &cancel,
        REAL_TIMEOUT,
        &work.path().join("log"),
    )
    .unwrap();
    let docx = dir.path().join("source.docx");
    assert!(docx.is_file(), "fixture docx was not produced");

    let pdf = convert_real(&docx, &dir.path().join("cache"), "real-docx");
    assert_eq!(pages_of(&pdf), 1);
}

/// Security finding: a document LibreOffice converts must not fetch what it
/// links to (a linked image over HTTP, here; a `\\host\share` one on
/// Windows takes the same path). The hardened private profile
/// (`office::HARDENING_XCU`) blocks it: the listener sees no connection.
#[test]
fn fr_053_real_libreoffice_fetches_no_linked_resource() {
    let Some(soffice) = tool_or_skip(Dependency::LibreOffice, "office-links") else {
        return;
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let dir = tempfile::tempdir().unwrap();
    // A flat ODF text document with one linked (not embedded) picture.
    let fodt = dir.path().join("linked.fodt");
    std::fs::write(
        &fodt,
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
 xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"
 xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0"
 xmlns:xlink="http://www.w3.org/1999/xlink" office:version="1.3"
 office:mimetype="application/vnd.oasis.opendocument.text">
 <office:body><office:text>
  <text:p>Linked picture:
   <draw:frame svg:width="2cm" svg:height="2cm" text:anchor-type="as-char">
    <draw:image xlink:href="http://127.0.0.1:{port}/linked.png" xlink:type="simple" xlink:show="embed" xlink:actuate="onLoad"/>
   </draw:frame>
  </text:p>
 </office:text></office:body>
</office:document>"#
        ),
    )
    .unwrap();
    let work = WorkDir::new("office-links").unwrap();
    let output = dir.path().join("linked.pdf");
    office::convert_with(
        &soffice,
        &fodt,
        &output,
        &work,
        &AtomicBool::new(false),
        REAL_TIMEOUT,
    )
    .unwrap();
    assert_eq!(pages_of(&output), 1);
    match listener.accept() {
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("LibreOffice fetched the linked picture: {other:?}"),
    }
}

#[cfg(target_os = "macos")]
#[test]
fn fr_052_real_heic_made_by_sips_converts_to_one_page() {
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("photo.png");
    let mut img = ::image::RgbImage::new(64, 48);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = ::image::Rgb([(x * 4) as u8, (y * 5) as u8, 90]);
    }
    img.save_with_format(&png, ::image::ImageFormat::Png)
        .unwrap();
    let heic = dir.path().join("photo.HEIC");
    let status = std::process::Command::new("/usr/bin/sips")
        .args(["-s", "format", "heic"])
        .arg(&png)
        .arg("--out")
        .arg(&heic)
        .stdout(std::process::Stdio::null())
        .status()
        .expect("sips is part of macOS");
    if !status.success() || !heic.is_file() {
        eprintln!("SKIPPED heic: this macOS cannot encode HEIC");
        return;
    }
    let pdf = convert_real(&heic, &dir.path().join("cache"), "real-heic");
    assert_eq!(pages_of(&pdf), 1);
}

/// FR-052 on Linux with a real decoder: a HEIC encoded in-test (by
/// ImageMagick, which writes HEIF through libheif) prints as one page through
/// whichever decoder the engine finds. Skips without a decoder, or when the
/// local ImageMagick cannot encode HEIC.
#[cfg(target_os = "linux")]
#[test]
fn fr_052_real_heic_converts_through_the_linux_decoder() {
    if tool_or_skip(Dependency::HeifDecoder, "heic").is_none() {
        return;
    }
    let Some(heic) = linux_heic_fixture() else {
        return;
    };
    let pdf = convert_real(&heic.1, &heic.0.path().join("cache"), "real-heic-linux");
    assert_eq!(pages_of(&pdf), 1);
}

/// FR-052 on Linux, each decoder on its own: libheif's tool and ImageMagick
/// both turn a real HEIC into a one-page PDF. A decoder that is not installed
/// is skipped with a note.
#[cfg(target_os = "linux")]
#[test]
fn fr_052_each_real_linux_decoder_converts_a_heic() {
    let Some((dir, heic)) = linux_heic_fixture() else {
        return;
    };
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut ran = 0;
    for name in crate::engine::deps::HEIF_DECODERS_LINUX {
        let Some(decoder) = std::env::split_paths(&path)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
        else {
            eprintln!("SKIPPED {name}: not installed");
            continue;
        };
        let output = dir.path().join(format!("{name}.pdf"));
        let work = WorkDir::new(name).unwrap();
        heif::convert_with_external(
            &decoder,
            &heic,
            &output,
            &work,
            &AtomicBool::new(false),
            REAL_TIMEOUT,
            &no_progress(),
        )
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(pages_of(&output), 1, "{name}");
        ran += 1;
    }
    eprintln!("{ran} real HEIF decoder(s) exercised");
}

/// A HEIC photo made in-test with ImageMagick (`convert`/`magick`), with the
/// directory that holds it. `None`, with a note, when it cannot be made.
#[cfg(target_os = "linux")]
fn linux_heic_fixture() -> Option<(tempfile::TempDir, PathBuf)> {
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("photo.png");
    let mut img = ::image::RgbImage::new(64, 48);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = ::image::Rgb([(x * 4) as u8, (y * 5) as u8, 90]);
    }
    img.save_with_format(&png, ::image::ImageFormat::Png)
        .unwrap();
    let heic = dir.path().join("photo.HEIC");
    let made = ["magick", "convert"].iter().any(|tool| {
        std::process::Command::new(tool)
            .arg(&png)
            .arg(format!("heic:{}", heic.display()))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
            && heic.is_file()
    });
    if !made {
        eprintln!("SKIPPED heic: no ImageMagick here can encode HEIC");
        return None;
    }
    Some((dir, heic))
}

/// A damaged HEIC fails the conversion on Linux too, leaving nothing behind.
#[cfg(target_os = "linux")]
#[test]
fn fr_052_corrupt_heic_fails_on_linux_without_leaving_a_pdf() {
    if tool_or_skip(Dependency::HeifDecoder, "corrupt heic").is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let heic = dir.path().join("broken.heic");
    std::fs::write(&heic, b"\0\0\0\x18ftypheic-not-really").unwrap();
    let err = convert_to_pdf(
        &heic,
        &dir.path().join("cache"),
        "bad-heic-linux",
        &AtomicBool::new(false),
        &no_progress(),
        REAL_TIMEOUT,
        &PageSetup::default(),
        None,
    )
    .unwrap_err();
    assert!(matches!(err, EngineError::ConversionFailed(_)), "{err:?}");
    assert!(!dir.path().join("cache").join("bad-heic-linux.pdf").exists());
}

#[cfg(target_os = "macos")]
#[test]
fn fr_052_corrupt_heic_is_a_conversion_failure_not_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let heic = dir.path().join("broken.heic");
    std::fs::write(&heic, b"\0\0\0\x18ftypheic-not-really").unwrap();
    let cancel = AtomicBool::new(false);
    let err = convert_to_pdf(
        &heic,
        &dir.path().join("cache"),
        "bad-heic",
        &cancel,
        &no_progress(),
        REAL_TIMEOUT,
        &PageSetup::default(),
        None,
    )
    .unwrap_err();
    assert!(matches!(err, EngineError::ConversionFailed(_)), "{err:?}");
    assert!(
        !dir.path().join("cache").join("bad-heic.pdf").exists(),
        "no partial cache PDF"
    );
}
