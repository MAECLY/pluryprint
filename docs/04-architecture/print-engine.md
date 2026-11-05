# Print Engine

> **TL;DR** — Pluryprint uses a **hybrid pipeline**: each input file is converted to PDF by the most appropriate converter for its format, then printed by a single back-end (SumatraPDF on Windows, CUPS `lp` on macOS/Linux). This "convert-everything-to-PDF, then print PDF" strategy is dramatically simpler than per-format printing while still producing native-quality output. **Images and SVG are converted in-process (the `image` crate; `usvg` + `svg2pdf` in a worker), not by ImageMagick.** HEIC goes through the OS decoder. Office and OpenDocument go through LibreOffice headless (RTF too on Windows). HTML, Markdown and plain text are paginated by the operating system's own web view (WebView2 / WKWebView) in a hidden worker process; Markdown and text become HTML in-process first, and RTF does so through `textutil` on macOS. Encrypted PDFs are decrypted in-process with the user's password. GIF is refused.

## Implementation status

This document describes the hybrid design. EPUB is still a target only. Everything else in the matrix is wired:

- **Supported inputs:** `pdf` (passthrough, encrypted ones after the password); the raster images `jpg`, `jpeg`, `png`, `tif`, `tiff`, `bmp`, `webp`; `heic`/`heif`; `svg`/`svgz`; Office `docx`, `doc`, `xlsx`, `xls`, `pptx`, `ppt`; OpenDocument `odt`, `ods`, `odp`; `rtf`, `txt`; `html`, `htm`; `md`, `markdown` (`src-tauri/src/engine/mod.rs::SUPPORTED_EXTS`). `gif` is refused by name (decision D9). The per-format matrix and each converter's exact command line are in [supported-formats.md](../03-requirements/supported-formats.md). **ImageMagick is not a target at all**: the image path is in-process and stays that way.
- **Conversion:** PDFs are validated by their `%PDF-` header, an `%%EOF` marker near the end and (up to 100 MB) a non-zero page count, then printed in place (no cache file). Images are decoded with the pure-Rust `image` crate and wrapped into a one-page PDF built by hand with `lopdf`, **not** `printpdf`, whose image feature is disabled. HEIC is decoded by `sips` (macOS) or WIC (Windows) into the same one-page PDF. SVG is drawn as a vector page by `usvg` + `svg2pdf` in `pluryprint --worker svg`. HTML, Markdown and text are printed by the system web view in `pluryprint --worker render-pdf`. LibreOffice, `textutil` and both workers run through `engine::convert::process` (`run_bounded` / `run_until`). It uses an argument vector (no shell) and a private scratch folder removed on every exit path, and it is bounded by the conversion timeout and the cancel flag, either of which kills the whole child process tree. Documented limitations of the image path: EXIF orientation is not applied, multi-frame TIFFs use the first frame only, and CMYK JPEGs and other sub-formats the codec cannot handle surface as a clear `ConversionFailed`.
- **Printing (M1):** CUPS `lp` on macOS/Linux; **SumatraPDF** on Windows. It is now **bundled**: `scripts/fetch-sumatrapdf.{ps1,sh}` fetches the pinned 3.6.1 release (x64 or ARM64) into `src-tauri/resources/sumatrapdf/`, which `tauri.conf.json` ships as a bundle resource, and `engine::deps::bundled_dirs` looks there first. The search order is bundled → configured → `PATH` → well-known install dirs (`%LOCALAPPDATA%`, `%ProgramFiles%`, `%ProgramFiles(x86)%` under `SumatraPDF\`), and where a binary was found is reported to the UI as `found_in`. A genuinely missing binary is still a `DependencyMissing` that pauses the job, with FR-058 remediation text. Real flags are in §3.1 / §3.6.
- **Dependency probing (FR-058):** `engine::deps` probes SumatraPDF, LibreOffice and, on Windows, the HEIF and HEVC Store codecs on every `app_info` call (a handful of `is_file` checks and two codec queries); Edge/Chrome are no longer probed (the headless-browser fallback was removed in round 6, ADR-0012), and Pandoc is no longer used. A missing dependency can be installed from the Print Job screen after the user's consent (`install_dependency`, decision D13). For each dependency it reports whether it is *applicable on this OS at all* and which extensions it blocks while missing. The applicability flag matters: SumatraPDF is irrelevant on macOS, and reporting it as "missing" there would be the loudest possible false alarm. The converters use the same resolver, and a missing one returns `DependencyMissing` with the same remediation text, which pauses the job.
- **Capability gate:** **duplex** and **paper** are hard-checked — a duplex request against a driver that reports no duplex, or a paper the printer's list does not have, pauses the job as `unsupported_setting` (the failure would otherwise be invisible at the tray); an unknown capability (a probe that failed) is not "no". Colour on a mono printer is allowed (it prints grey, which is visible). See `engine::print::check_supported` and [print-settings.md](../03-requirements/print-settings.md#driver-dependencies).
- **Orchestration lives in the queue, not the engine.** Conversion/print concurrency (default ≤ 2 conversions, 1 print), retries with backoff, cancellation, and the conversion cache are owned by the dispatcher — see [queue.md](../05-features/queue.md) and [data-model § 4](./data-model.md#4-queuejson--live-print-queue). The converter/printer functions here are pure, blocking, single-file units the dispatcher drives, with a page-range parser (`engine::pagerange`) shared by validation and the backends.
- **The headless CLI does not use the queue at all.** `pluryprint print` walks the same `engine::convert` → `engine::print::spool` path directly, into a private temp cache deleted on exit, with **no `queue.json` and no retries** — see [ipc-and-cli.md §3.3](./ipc-and-cli.md#33-flag-semantics).

## 1. Why a hybrid engine

Two extreme designs are tempting and both are wrong:

| Design                             | Why it fails                                                                                                  |
| ---------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| **Pure native per format**         | Every format requires a different printing API (Office automation, GDI+ for images, browser print for HTML). Maintenance balloons; bugs multiply per backend. |
| **Pure single-engine (Ghostscript only)** | Ghostscript can rasterize many formats but quality on Office docs is poor and licensing is GPL with edge cases. Doesn't handle DOCX or images natively without conversion anyway. |

The hybrid choice — **convert each format to PDF with the best-of-breed tool, then print PDF with one well-understood engine** — wins because:

- **Quality.** LibreOffice's DOCX → PDF is the most accurate free converter. The system web view prints HTML the way the user sees it, and is already on every machine Pluryprint runs on. (Images, SVG, Markdown and text are the exception to "best-of-breed external tool": they need no external layout engine, so converting them in-process is simpler and one fewer dependency the user must install.)
- **Simplicity.** The final printing path has one shape: `print_pdf(file, settings)`. All printer settings (duplex, color, scale, range, copies, paper size) are set once at the printer-adapter layer.
- **Testability.** The "did it print correctly?" question reduces to "did we generate a correct PDF?" — and PDFs are inspectable.
- **Replaceability.** Swapping LibreOffice for a different DOCX converter does not affect the printer code path at all.

## 2. Format → converter → final-printer matrix

| Format           | Converter           | Tool invoked                   | Final printer (Win)  | Final printer (Mac)  |
| ---------------- | ------------------- | ------------------------------ | -------------------- | -------------------- |
| `.pdf`           | (passthrough)       | —                              | SumatraPDF           | CUPS `lp`            |
| `.docx` `.doc`   | LibreOffice         | `soffice --headless --convert-to pdf` | SumatraPDF    | CUPS `lp`            |
| `.xlsx` `.xls`   | LibreOffice         | `soffice --headless --convert-to pdf` | SumatraPDF    | CUPS `lp`            |
| `.pptx` `.ppt`   | LibreOffice         | `soffice --headless --convert-to pdf` | SumatraPDF    | CUPS `lp`            |
| `.odt` `.ods` `.odp` | LibreOffice     | `soffice --headless --convert-to pdf` | SumatraPDF    | CUPS `lp`            |
| `.rtf`           | Windows: LibreOffice. macOS: `textutil` → system web view | `soffice --headless --convert-to pdf` / `textutil -convert html`, then the HTML row | SumatraPDF | CUPS `lp` |
| `.txt`           | **in-process** decode → HTML → system web view | the render worker | SumatraPDF | CUPS `lp`            |
| `.png` `.jpg` `.jpeg` `.tif` `.tiff` `.bmp` `.webp` | **in-process** (`image` + `lopdf`) | — (no subprocess) | SumatraPDF | CUPS `lp` |
| `.heic` `.heif`  | OS decoder          | `sips -s format png` (macOS) / WIC (Windows) | SumatraPDF | CUPS `lp`      |
| `.svg` `.svgz`   | `usvg` + `svg2pdf`, vector | `pluryprint --worker svg` | SumatraPDF | CUPS `lp`      |
| `.html` `.htm`   | system web view (WebView2 / WKWebView) | `pluryprint --worker render-pdf` | SumatraPDF | CUPS `lp`            |
| `.md` `.markdown` | **in-process** `pulldown-cmark` → HTML | then the HTML row | SumatraPDF | CUPS `lp`     |
| `.epub`          | *(target)* | — | — | — |

Anything not in this table (and the *target* row) is **unsupported**. It is refused at drop time with a reason, never skipped silently.

`.gif` is refused with its own reason (`gif_unsupported` / `gif_not_supported`, decision D9): the `image` crate's `gif` codec is not enabled.

Detection is by **extension**, lowercased. There is no magic-byte fallback: `infer` and `mime_guess` are not dependencies, and a file whose extension lies is rejected by the converter with a `ConversionFailed` naming the decoder's own complaint.

## 3. Concrete invocations

### 3.1 SumatraPDF (Windows, shipped)

```
SumatraPDF.exe ^
  -print-to "HP LaserJet Pro M404dn" ^
  -print-settings "1-5,duplex,monochrome,fit,paperkind=9,2x" ^
  -silent ^
  -exit-when-done ^
  "%LOCALAPPDATA%\Maecly\Pluryprint\cache\conversions\<job-id>.pdf"
```

The `-print-settings` value is a comma-separated keyword list Pluryprint emits in a fixed order (SumatraPDF is itself order-independent; the fixed order keeps the honesty preview stable):

1. **page ranges** — bare, no key (e.g. `1-5,8`); omitted when printing all pages.
2. **duplex** — `simplex` / `duplex` (long edge) / `duplexshort`.
3. **colour** — `color` / `monochrome`.
4. **scaling** — `fit` / `shrink` / `noscale`.
5. **paper** — `paperkind=<DMPAPER id>` (A4 = 9, Letter = 1): the driver's id for its own paper name, else the standard id. Never `paper=<name>`, which SumatraPDF looks up among the driver's *localized* names.
6. **tray** — `bin=<DMBIN id>` for the job's tray, else the printer's saved default tray.
7. **copies** — `<n>x` (e.g. `3x`), only when > 1. SumatraPDF silently skips an unknown `copies=<n>` token.
8. **orientation** — `landscape` / `portrait`; `auto` emits nothing.

Quality has no token: Draft and High become the printer's per-user default DEVMODE only while SumatraPDF hands the job over (`engine::print::devmode_swap`, decision D16; see [print-settings.md](../03-requirements/print-settings.md#quality)). Defaults serialise to `simplex,color,fit,paperkind=9`. Translation from the wire `JobSettings` lives in `engine/print/sumatra.rs`, and the same builder feeds the UI's honesty preview so what the user sees is exactly what runs. The child is spawned with the console window suppressed; SumatraPDF prints synchronously and reports no spooler job id. Its stdin is closed and its stdout/stderr go to a log file, never to the app's own handles: SumatraPDF logs its whole start-up to an inherited stdout, which used to land in the middle of `pluryprint-cli print --json`. The tail of that log is quoted when SumatraPDF fails. SumatraPDF exits with the number of files that failed, so there is no meaningful exit code to map: a failure is `sumatra_exit` with the code, and a PDF its log says may not be printed becomes the terminal `printing_not_allowed`. Pluryprint refuses such a PDF itself before SumatraPDF runs, because SumatraPDF 3.5.2 was seen printing it with exit 0; the bundled version is now 3.6.1. SumatraPDF has no job-title flag and names the spooler document after the file it prints, so the PDF is handed to it as a hard link (or a copy, across volumes) named after the original file (`report.pdf` for `report.docx`, the same name the CUPS backend gives a job; `sumatra::spool_file_name`) in a private `pluryprint-spool-*` temp folder that is removed once the job is spooled. The Windows print queue then shows the user's file name instead of a cache UUID or the source's full path. SumatraPDF uses the full path as the document name, so as soon as the job is found in the queue its document is set to the file name alone (`windows::retitle_job`, `GetJob`/`SetJob` level 1, queue position untouched); the job matching compares file names, so tracking and cancel still recognize it.

### 3.2 LibreOffice (cross-platform)

```
soffice ^
  -env:UserInstallation=file:///C:/Users/.../AppData/Local/Temp/pluryprint-convert-<job>-XXXX/profile ^
  --headless --invisible --nologo --nodefault --nolockcheck --norestore ^
  --convert-to pdf ^
  --outdir "%TEMP%\pluryprint-convert-<job>-XXXX\out" ^
  "%TEMP%\pluryprint-convert-<job>-XXXX\input.docx"
```

(`engine::convert::office`.) Caveats:

- `soffice` does not allow concurrent invocations against the same user profile, and without its own profile a headless run hands the file to an already-open LibreOffice window. Each conversion therefore gets a throwaway `-env:UserInstallation` profile inside its scratch folder. Conversions can then run in parallel, and while the user has LibreOffice open.
- The input is copied to `input.<ext>` first, so the output name is predictable and no lock file appears next to the user's document. Plain text no longer comes here.
- The profile is **hardened** before LibreOffice starts (`office::HARDENING_XCU`): links from untrusted documents are blocked, Writer and Calc never update links on load, macros and active content (OLE, DDE) are off, and LibreOffice's HTTP(S) goes to a dead proxy on loopback, so a document cannot make the conversion fetch `http://…` or `\\host\share\…` resources.
- `soffice` exits 0 even when it could not load the document, so success means `out/input.pdf` exists and starts with `%PDF-`. Otherwise the error quotes stderr.
- First start with a fresh profile costs a few seconds per file. The default conversion timeout is 60 s, and LibreOffice is killed (with `soffice.bin`) when it runs out. Macros are not executed. A password-protected Office document produces no PDF and fails with `ConversionFailed` (only PDFs have a password flow).

### 3.3 Images (in-process, shipped — no external tool)

There is **no ImageMagick invocation**. `engine::convert::image::to_pdf` decodes the file with the `image` crate and hand-builds a one-page PDF with `lopdf`:

- **Why `lopdf` and not `printpdf`'s image API:** `printpdf` is pulled in with `default-features = false`, which disables its `images` feature — its image-XObject serialiser is then a `panic!` stub. `lopdf` is already a direct dependency and emits a minimal, correct image XObject (`/DeviceRGB`, 8 bits per component) with no extra crates.
- **Page sizing:** the page is sized to the image's pixels at **96 DPI** (`CONVERT_DPI`), converted to PDF points at 72 pt/in. The converter is deliberately paper-agnostic — the driver does the real fitting to A4/Letter via the scaling flags (§3.1/§3.6).
- **Ceilings:** the largest emitted page dimension is 14,400 pt (200 in, the classic Acrobat limit); an enormous image is scaled down uniformly rather than producing an out-of-spec page. Decoding is bounded by `image::Limits` (max width/height 30,000 px) as a decompression-bomb guard, because the crate enforces no dimension limits by default.
- **Progress** is reported at `0.0` (before decode), `0.5` (before embedding) and `1.0` (after save).

Known limitations, documented rather than treated as bugs:

- **EXIF orientation is not applied.** The `image` crate does not auto-rotate and we do not read EXIF, so a rotated photo prints in its stored orientation. (This is the one place the old ImageMagick `-auto-orient` claim promised something we do not do.)
- **Multi-frame TIFF: first frame only.**
- **Transparency is flattened onto white**, as on paper, so a transparent PNG or WebP logo does not print as a black box.
- **CMYK JPEGs** and other sub-formats the codec cannot handle surface as `ConversionFailed` carrying the decoder's message.
- **Zero-width or zero-height images** are rejected explicitly rather than producing a degenerate page.

### 3.4 The system web view (HTML, Markdown, text; RTF on macOS)

```
pluryprint --worker render-pdf <request>    # spawned by the engine, never by the user
```

(`engine::convert::html`, `crate::render`, `crate::worker::render`; decision D15.) The engine writes a **staged copy** of the page into the job's scratch folder, headed by a Content-Security-Policy that forbids scripts, plugins, frames and connections, a `<base href>` at the document's folder and the job's paper as `@page { size }`, and hands it to a hidden worker: the same executable started with `--worker render-pdf` (the console CLI hands this to the app executable next to it). The worker is bounded by the conversion timeout and the cancel flag like any converter, so a page that hangs or crashes the web view takes the worker down, never the app. Since round 7 it also runs under a memory cap, through the same `process::run_capped` as the SVG worker (§3.5, ADR-0014), but over its whole process tree: 2 GiB (`html::RENDER_WORKER_MEMORY_CAP`) for the worker and every helper it starts together. On Windows that is a Job Object with `JOB_OBJECT_LIMIT_JOB_MEMORY` and kill-on-close, which holds the WebView2 browser, renderer and GPU processes too (they are the worker's children); the worker waits until it is inside the job before it reads the page. On macOS the parent samples the worker's process group every 20 ms; WebKit's web-content and networking processes are XPC services started by launchd, outside that group, so they are not covered. A worker stopped for memory ends the job with the terminal `page_too_large` (its `limit` is the cap in MB).

- **Windows:** a hidden window of a plugin-free Tauri app with a private WebView2 profile, whose `Local State` is seeded first so WebView2 skips Chromium's blank-password logon probe (`util::chromium`; unseeded, that probe records a failed Windows logon per run, and ten in ten minutes lock the account — the app seeds its own `EBWebView` profile at every start for the same reason, and since round 7 also any profile `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` (`--user-data-dir`, as the Edge WebDriver passes) or `WEBVIEW2_USER_DATA_FOLDER` points WebView2 at). Scripts, dialogs and dev tools are off, and `PrintToPdf` runs on the job's paper with no header or footer. A `WebResourceRequested` filter on `*` (every resource context, and every request source where `ICoreWebView2_22` exists) answers every request itself (`render::answer_request`): the private `https` origin serves the staged page and the document's folder, `data:`/`blob:`/`about:blank` load as they are, and everything else gets a synthetic 403 that is logged to `render.log`. Chromium refuses `file:` sub-resources to the `https` page, and a protocol-relative `//host/x` becomes `https:` and meets the filter. Navigations other than the page, frame navigations and new windows are cancelled. A served file is read from a handle whose final path is checked against the document's folder (security note 9). No `\\host\share` is ever opened over SMB or WebDAV.
- **macOS:** a bare AppKit process with no Dock icon, a `WKWebView` with content JavaScript off and a non-persistent data store. The page is loaded from the worker's private scheme `pluryprint-doc://doc.pluryprint.example/`, which a `WKURLSchemeHandler` of the worker answers (round 7): `render::resolve` maps the URL as on Windows, and `render::open_in_folder` opens a file inside the document's folder component by component without following a symbolic link out of it, checks the open descriptor's `F_GETPATH`, and the bytes are read from that descriptor; a link out of the folder gets a logged 403. WebKit is given no `file:` URL and no read access at all. A `WKContentRuleList` (`render::webkit_rule_list`) blocks everything but that origin, `data:`, `blob:` and `about:blank`, every `file:` URL included, and a navigation delegate cancels every navigation the worker did not start. `NSPrintOperation`, run modally for an off-screen window, writes the paginated PDF; a size watchdog stops a page that keeps producing pages.
- **Markdown** becomes HTML in-process (`pulldown-cmark`: raw HTML dropped, only local images kept, GitHub-flavoured extensions on), **text** is decoded (BOM, UTF-8, else Windows-1252) into a wrapping `<pre>` page, and **RTF on macOS** goes through `/usr/bin/textutil -convert html` on a private copy. All three then take this route.
- A worker failure is `render_failed` (out of memory: `page_too_large`), with the worker's stderr as the technical detail; `PLURYPRINT_RENDER_TRACE=1` traces every stage into the job's `render.log`.

**No browser fallback (round 6).** The hidden headless Edge/Chrome route (`[engine] html_renderer = "browser"`) was removed: it loaded the page from `file:`, so a text pre-pass (`html_scan`) was the only thing between a share reference and SMB, and it kept losing to parser differentials. The web view's network layer above is the only boundary; `html_scan` now just finds where the staged page's policy and base go and decodes the page (leniently: an unknown or empty charset falls back to UTF-8 or Windows-1252). A stored `"browser"` is ignored and removed by the app's one-time notice.

### 3.5 SVG worker

```
pluryprint --worker svg <in.svg> <out.pdf>
```

(`engine::convert::svg`, FR-052a.) `usvg` parses and `svg2pdf` writes the drawing as a form XObject (`to_chunk`), drawn by a page of the drawing's own size at 96 dpi, clamped to 200 in (not `to_pdf`, which leaves pattern matrices unscaled at that dpi and stretches gradients by 4/3); the printer's scaling fits it to the paper. A parser panic on a hostile drawing aborts the worker, not the app. Failures are `svg_invalid`, `svg_too_large` or `svg_convert_failed`. An `<image>` loads only from the SVG's own folder (no UNC, `file://host/`, `../` or URL), there is no network, script or external entity, and an `.svgz` is inflated with a 64 MB cap.

**The worker is the resource boundary** (security note 8). The parent starts it through `process::run_capped` with a 1 GiB memory cap (`svg::WORKER_MEMORY_CAP`):

- **Windows:** the worker is put in a Job Object with `JOB_OBJECT_LIMIT_PROCESS_MEMORY` (an allocation past the cap fails, and Rust aborts on it), `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` (it dies with the job handle, even if the app crashes) and an active-process limit of 1 (it cannot start helpers). The job's completion port reports `JOB_OBJECT_MSG_PROCESS_MEMORY_LIMIT`. The worker waits until it is inside a job with that limit before it reads its input, and refuses to work otherwise, so it never runs uncapped.
- **macOS / Linux:** the worker lowers `RLIMIT_DATA` to the cap before it reads its input. macOS does not enforce `RLIMIT_DATA` or `RLIMIT_AS` on `mmap`, so the parent also samples the worker's memory every 20 ms (macOS: physical footprint from `proc_pid_rusage`, compressed pages included; Linux: resident set) and kills it past the cap.

A worker stopped for memory ends the job with the terminal `svg_too_large` (its `limit` is the cap in MB); one that runs past the conversion timeout with the terminal `tool_timed_out`; a PDF over 512 MB is `svg_too_large` too. Inside the worker, cheaper checks fail fast with the same reason but are not the boundary: the byte, entity and node caps; a bounded-work walk of what usvg would expand the parsed document to (`<use>`, markers, patterns, and masks, clip paths and filters, charged in full for every reference whatever their units: usvg converts the `objectBoundingBox` ones again per reference, and svg2pdf writes even the shared `userSpaceOnUse` ones out once per use, round 7), charged step by step against one budget; at most 64 Mpx per raster `<image>` and 64 Mpx of filter rasterisation, both read from usvg's own tree; and 256 MB of image resources in all. An SVG embedded as an image (a file or a `data:` URI) goes through the same caps instead of usvg's uncapped loader. Every external reference is decided in usvg's `ImageHrefResolver`, on the exact string usvg parsed.

### 3.6 CUPS `lp` (macOS / Linux, shipped)

```
lp -d "OfficeJet_Pro" \
   -t "report.pdf" \
   -o sides=two-sided-long-edge \
   -o ColorModel=Gray \
   -n 2 \
   -o media=A4 \
   -o fit-to-page \
   -o page-ranges=1-5 \
   "~/Library/Caches/com.Maecly.Pluryprint/conversions/<job-id>.pdf"
```

Flag mapping (`engine/print/cups.rs`), in the exact order `lp_args` emits them:

| Setting | Flag |
| --- | --- |
| printer | `-d <name>` |
| title | `-t <title>` |
| duplex none / long / short | `-o sides=one-sided` / `two-sided-long-edge` / `two-sided-short-edge` |
| colour | `-o ColorModel=RGB` *only when the printer lists `RGB`; else omitted* |
| monochrome | `-o ColorModel=Gray` |
| copies > 1 | `-n <copies> -o collate=true\|false` |
| paper | `-o media=<the printer's keyword>` *(`auto` omits)* |
| tray (job's, else the printer's override) | `-o InputSlot=<bin> -o media-source=<bin>` *(none omits)* |
| quality | `-o print-quality=3\|4\|5 -o cupsPrintQuality=Draft\|Normal\|High` |
| orientation portrait / landscape | `-o orientation-requested=3` / `=4` *(`auto` omits)* |
| scaling fit **or** shrink | `-o fit-to-page` *(`none` omits)* |
| page ranges | `-o page-ranges=<r>` (whitespace stripped) |

Three things worth stating plainly:

- **`-d` and `-n`, not `lpr -P` and `-o copies=N`.** The engine invokes `lp`, whose own flags these are. Earlier drafts described the `lpr` spelling; it was never what the code emitted.
- **`sides`, `media` and quality are always emitted** (media unless the job asks for the printer's own paper, `auto`), even for the defaults, so an explicit "single-sided" or "A4" beats a driver default that says otherwise. Everything else is emitted only when it differs from the driver default or names a real choice (a tray, an orientation, several copies), so the command stays readable.
- **`fit` and `shrink` collapse to the same flag.** CUPS exposes exactly one `fit-to-page` toggle and cannot express "shrink oversized pages but never enlarge". So on macOS/Linux the two settings are indistinguishable at the spooler: a small page under `shrink` may still be scaled up. On Windows they are distinct (`fit` vs `shrink` in `-print-settings`). This is a real cross-platform behavioural difference, not a rounding error, and the honesty preview shows the user the actual flag either way.

On success `lp`'s `request id is <dest>-<n>` line yields the CUPS job id used to follow the job and to `cancel` it. `<dest>` may itself contain `-`, so the number is only ever the last segment. `lp` is run in the C locale, but macOS localizes this line from the user's language list anyway (`el id de la petición es Office-42 (1 archivos)`), so the parser does not depend on the English marker: it falls back to the token naming the destination, then to any `<name>-<digits>` token.

The job is then followed until it leaves the queue (`engine::print::job_state`): `ipptool -tv ipp://localhost/jobs/<n> <tmp>/pluryprint-ipp-XXXX.test` gives the IPP `job-state` (`pending`, `pending-held`, `processing`, `processing-stopped`, `canceled`, `aborted`, `completed`). The test is the stock `get-job-attributes` request, embedded in the app, written to a private temporary file and passed by absolute path: `ipptool` looks a bare test name up in the current directory first, so a planted `get-job-attributes.test` in the folder the CLI was started from would otherwise run. Where `ipptool` is missing `lpstat -W not-completed -o <dest>` still tells a waiting job from a finished one. It cannot tell *how* a job finished (CUPS' completed list holds printed, cancelled and aborted jobs alike), so a job that left the queue is `Gone` and ends as **unconfirmed** (`print_unconfirmed` / `left_queue`), never reported as printed. An `lpstat` that exits non-zero (cupsd restarting or unreachable) is a transient tracking failure, retried and then given up as `unconfirmed` (`tracking_failed`), never read as "no longer listed". On Windows, SumatraPDF reports no id, so the spooler's job list is read before and after spooling and the new job is followed with `GetJobW`. The recorded reference carries the spooler's own submission stamp (`12@<stamp>`): the spooler numbers jobs from low ids again after it restarts, so a job whose stamp no longer matches is reported as gone instead of followed, and a cancel deletes a job only while id and stamp still match. A bare id cannot be checked and is never acted on. The job is adopted only when exactly one new job was submitted by the current user and its document is the spooled file (SumatraPDF names the job after the full path of the staged, title-named copy; a whole path or file-name match, never a substring). There is no "it is the only new job" fallback, because on a shared `\\server\queue` that job can be a colleague's. If either job list cannot be read, the job is not tracked and ends as `unconfirmed` (`untracked`) on hand-off, never as printed. In the `JOB_STATUS_*` flags, `PRINTED`/`COMPLETE` wins over `DELETING`/`DELETED`, because the spooler deletes every job it has printed. Only a job deleted before it printed counts as cancelled, and a job the spooler no longer knows has left the queue. Polling alone cannot tell how such a job ended: Windows deletes a printed job about a millisecond after setting `PRINTED` (`0x94` → `0x84` → `0x184` on an Epson network queue), so a poll never sees the bit. Before spooling, a watcher thread for the printer therefore subscribes to the spooler's job notifications (`FindFirstPrinterChangeNotification`: status, document, user, submission time) and keeps every status bit each job carried. A job that left the queue is `Completed` when its record — matched by id **and** submission stamp — shows `PRINTED` or `COMPLETE`; anything else (no record, a deletion without `PRINTED`, notifications the spooler discarded) stays `Gone` and ends `unconfirmed`. The same records let a short job that printed and left before SumatraPDF exited still be adopted. The watcher stops after 15 minutes without a question about its printer and is restarted on the next one. The dispatcher owns the cadence and the budget — see [queue.md](../05-features/queue.md#dispatcher-semantics-what-the-ui-reflects).

## 4. The full pipeline

```
+--------------------+
| User submits N     |
| files + Profile    |
+----------+---------+
           |
           v
+--------------------+
| 1. Format detect   |  By extension, lowercased (convert::route); no magic-byte
|    per file        |  fallback. Unsupported or GIF: refused at submit.
+----------+---------+
           |
           v
+--------------------+
| 2. Router selects  |  A match on Route (§4.1): PDF passes through, the
|    a route         |  rest goes to its converter or worker.
+----------+---------+
           |
           v
+--------------------+
| 3. Submit checks   |  File exists (canonical path), <= max_file_size_mb.
|                    |  An encrypted PDF is inspected, not refused (§4.1).
+----------+---------+
           |
           v
+--------------------+
| 4. Conversions     |  Up to max_concurrent_conversions (default 2) on the
|    (parallel)      |  dispatcher's blocking tasks. Each writes
|                    |  <cache>/conversions/<job-id>.pdf (§4.2)
+----------+---------+
           |
           v
+--------------------+
| 5. Per-job result  |  Ok(pdf_path) or Err(ConvertError). Errors do not
|    captured        |  abort the queue; they are logged and reported.
+----------+---------+
           |
           v
+--------------------+
| 6. Printer dispatch|  Sequential (one print at a time). Calls SumatraPDF / `lp`
|    (sequential)    |  with the PrintSettings derived from Profile + overrides.
+----------+---------+
           |
           v
+--------------------+
| 7. Status polling  |  Spooler queried for job state; surfaced in UI.
|    + log emit      |  JSONL entry per state transition.
+--------------------+
```

### 4.1 Router selection

The router is **not** a MIME-keyed trait-object factory. `engine::convert::convert_to_pdf` is a plain function that switches on the lowercased extension and returns one of two things:

```rust
// src-tauri/src/engine/convert/mod.rs (shape, not verbatim)
// Ok(None)        -> passthrough: the input is already a printable PDF
// Ok(Some(path))  -> a converted PDF was written to cache_dir/<job-id>.pdf
// Err(Unsupported/ConversionFailed)
```

- **`pdf`** → validated cheaply and printed in place: `Ok(None)`, no cache file. The checks are the `%PDF-` header, an `%%EOF` marker in the last 8 KiB (a truncated file has none) and, for files up to 100 MB that `lopdf` can parse, at least one page. A file `lopdf` cannot parse still goes to the spooler, which reads more PDF dialects.
- **Raster images** (`jpg`, `jpeg`, `png`, `tif`, `tiff`, `bmp`, `webp`) → `image::to_pdf`, `Ok(Some(path))`.
- **`heic`/`heif`** → `heif::convert` (OS decoder), `Ok(Some(path))`.
- **Encrypted `pdf`** → `pdf_security::inspect` first: owner-only and printable is printed in place; one that needs a password is `PdfProtected(PasswordRequired)` (the job pauses) unless the job carries a password, in which case `decrypt_to` writes a decrypted copy to the cache; a no-print or unreadable one is refused.
- **`svg`/`svgz`** → the SVG worker.
- **Office / ODF** (and `rtf` on Windows) → `office::convert_with` (LibreOffice).
- **`html`/`htm`** → the render worker (the system web view; there is no browser route). **`md`/`markdown`** → `markdown`, **`txt`** → `text`, **`rtf` on macOS** → `rtf_mac`, each then the render worker.
- **`gif`** and everything else → `EngineError::UnsupportedFormat`.
`convert::route(ext)` is the table. The external program is resolved before anything is written, so a missing one costs nothing but a paused job. The trait-object design above was never implemented and there is no `JobPlan` type. The router is a match on `Route`, not a plugin registry.

### 4.2 PDF caching

Converted PDFs land in a `conversions/` subdirectory of the OS cache directory — `%LOCALAPPDATA%\Maecly\Pluryprint\cache\conversions\<job-id>.pdf` on Windows, `~/Library/Caches/com.Maecly.Pluryprint/conversions/<job-id>.pdf` on macOS (`queue::spawn` resolves `paths::cache_dir().join("conversions")`). The cache:

- Is **not** keyed on file content; each job gets its own cache file. This avoids spurious "is the file modified?" decisions.
- Holds nothing for a PDF input: a PDF passes through and is printed in place, so no cache file is produced at all.
- Is swept at app start, which is precisely why the single-instance plugin must be registered **before** any of that work runs — a second launch sweeping the cache would destroy the running instance's in-flight jobs (NFR-014).
- The **CLI does not share it.** `pluryprint print` converts into a private temp directory deleted when the process exits, so a script and a running app cannot delete each other's work.
- There is **no size-capped eviction** (`cache_max_bytes`) today. Files are removed with their jobs (`clear_done` deletes a job's cache), not by a byte budget.

### 4.3 Size and probe ceilings

Two different limits are easy to confuse:

- **`engine.max_file_size_mb`** (configurable, default and bounds in `ENGINE_LIMITS`) is a *submission* ceiling. A file over it is refused at submit with `InvalidRequest` ("… is larger than the N MB limit"), and a file found inside a folder walk is dropped with `SkipReason::TooLarge` and a human detail string rather than silently vanishing (FR-005/FR-059). A stored `0` here is treated as bricked input — it would reject every file — so it is rejected on write and sanitized back to the default.
- **`MAX_PDF_PROBE_BYTES` = 100 MB** (not configurable) is a *page-count* ceiling and has nothing to do with whether a file prints. PDFs above it are simply not parsed with `lopdf` at submit/inspect time, because that parse would stall the UI; their `pages` stays `null` until after conversion. A 300 MB PDF prints normally, it just shows no page count in the dialog.

Folder expansion has its own independent ceilings, `MAX_FOLDER_FILES = 5,000` entries and `MAX_FOLDER_DEPTH = 16` levels, reported back on the wire (`FolderExpansion.limits`, `files_truncated`, `depth_truncated`) so the UI can phrase the warning without hardcoding them.

## 5. Error handling matrix

Rows marked **(target)** are not implemented yet; the rest are shipped behaviour.

| Error | Detection | Error code | Queue effect |
| --- | --- | --- | --- |
| Print dependency missing (`lp`, `SumatraPDF.exe`) | `engine::deps` probe: bundled → configured → `PATH` → install dirs | `DependencyMissing` (CLI exit **3**) | Job paused, with FR-058 remediation text; never reported on an OS where the dependency is inapplicable |
| Conversion timeout | `conversion_timeout_seconds` elapsed | error code `"conversion_timeout"` | **Terminal — not retried** (see §5.1) |
| Print timeout | `wait-timeout` on the print child, then the wait for the OS job, one `print_timeout_seconds` budget | error code `"print_timeout"` | **Terminal — not retried** (see §5.1) |
| Image decode failure (CMYK JPEG, corrupt file, 0×0) | the `image` crate's own error | `ConversionFailed` (CLI exit **8**) | Retryable, carries the decoder's message |
| File path no longer exists | `canonical_input` at submit | `FileNotFound` (CLI exit **6**) | Rejected before enqueue |
| Relative path from IPC/drop | `canonical_input` | `InvalidRequest` (CLI exit **2**) | Rejected; only the CLI resolves relative paths |
| File is a directory | `inspect_files` reports `is_dir` | — | Not an error: the caller asks the FR-004 recurse question and calls `expand_folders` |
| Unsupported format | extension not in `SUPPORTED_EXTS` | `UnsupportedFormat` (CLI exit **7**) | Rejected before enqueue |
| File over `max_file_size_mb` | `stat` at submit / during the folder walk | `InvalidRequest`, or `SkipReason::TooLarge` in an expansion | Rejected / skipped with a reason, never silently dropped |
| Duplex or paper requested, printer reports it cannot | `engine::print::check_supported` | `UnsupportedSetting` → CLI exit **5** (a *printer* problem, not a usage one) | Job **paused**; the Queue row offers **Print one-sided** or **Send to another printer** (`adjust_job`) |
| No printer configured at all | printer resolution at dispatch | `PrintFailed` → CLI exit **5** | GUI pauses the job so the user can pick another; CLI fails immediately |
| Named printer not installed | printer resolution at dispatch | `PrintFailed` → CLI exit **5** | Same split as above |
| Spooler rejected the job | non-zero exit / stderr from `lp`/SumatraPDF | `PrintFailed` / `spooler_rejected` or `sumatra_exit` (CLI exit **9**) | Retryable; the spooler's own text is kept as the technical detail under a translated sentence |
| App was killed mid-job | recovery at next start (`queue::persist::recover`) | error code `"interrupted_by_shutdown"` | Job is `paused` and treated as **resumable**, not failed. A job already in the OS queue (it has its OS job id) stays `printing` and is tracked to its end instead, so it is never spooled twice |
| Unreadable `config.toml` in a CLI `print` run | `Config::load_reporting` | `Internal` / `config_corrupt` → CLI exit **3** | Nothing is printed; the GUI keeps degrading to defaults with a banner |
| Converter outlives the conversion timeout | the converter's own bound in `process::run_until`, or the dispatcher's timer | `conversion_timeout` | Terminal, like the dispatcher's own timeout; never retried |
| User cancellation | `cancel_job` / `cancel_all` | — | Job `cancelled` (terminal); a terminal job refuses cancellation with `InvalidState` |
| Converter binary not found (LibreOffice; on Windows the HEIF/HEVC codecs) | `engine::deps` resolution before the conversion starts; WIC's and Media Foundation's answers | `DependencyMissing` (CLI exit **3**) | Job paused with the FR-058 remediation (what is blocked and where to download it) |
| Converter exit ≠ 0, or exit 0 without a PDF (e.g. a password-protected Office document) | child exit status, then a `%PDF-` check of the expected output | `ConversionFailed` with a reason (`converter_failed`, `render_failed`, `svg_*`, …), quoting the converter's stderr as the detail | Retryable, like any conversion failure |
| Converter hangs | the conversion timeout raises the cancel flag, and `run_bounded` kills the process tree | error code `"conversion_timeout"` | Terminal (see §5.1); the scratch folder is removed |
| PDF needs a password | `/Encrypt` in the trailer, then `lopdf` cannot open it with the empty password | `PdfProtected` / `password_required` (CLI exit **13**) | Job **paused**; the Queue row offers **Enter password**. A wrong one is `password_incorrect`, paused again |
| PDF forbids printing, or uses encryption `lopdf` cannot read | its permissions / its security handler | `PdfProtected` / `printing_not_allowed`, `unsupported_encryption` (CLI exit **13**) | Terminal, not retried; never stripped (decision D12) |
| Job left the spooler without a confirmation, or could not be followed | spooler tracking (§3.6) | `print_unconfirmed` (`left_queue`, `untracked`, `tracking_failed`, `not_verifiable`) | Terminal state `unconfirmed`; nothing is re-sent automatically |

One honest gap worth naming: a printer that is offline or out of paper is shown from the spooler's own status (a held job, a printer that needs attention), not from a device-level probe.

### 5.1 Retry policy

The retry policy lives in the **dispatcher**, not the engine (`queue/dispatcher.rs`), and is the same for every failure that is retryable at all:

- **`MAX_ATTEMPTS = 4`** — the initial attempt plus **three** automatic retries.
- **`RETRY_BACKOFFS = [1 s, 5 s, 25 s]`** — the delay before retry *n*. A job that fails a fourth time is terminal (`Failed`).
- A job waiting out a backoff is held aside and skipped by the scheduler, so it does not occupy a conversion slot.

Earlier drafts said "3 attempts with 1 s/5 s backoff"; both numbers were wrong. Four attempts and three backoffs is what the code does and what its tests assert (`fr_034`-tagged tests advance tokio's virtual clock by exactly 1 s, 5 s and 25 s).

**Conversion timeouts are not retried.** A conversion that exceeds `engine.conversion_timeout_seconds` fails with the `"conversion_timeout"` error code and goes straight to `Failed` — retrying a deterministic timeout would just burn three more of them. Setting the value to `0` disables the timeout entirely; that is the one engine knob where `0` is a meaningful value rather than bricked input (`ZeroPolicy::Disables`), and the write path accepts it for exactly that reason. The **print** hop is bounded separately, by `engine.print_timeout_seconds` (default 300, `0` disables): the spooler child's wait (`wait-timeout`, killed on expiry) and the wait for the OS job to leave the queue share that one budget. A print timeout is terminal too (`"print_timeout"`, no automatic retry): re-spooling a job that wedged the spooler, or one still sitting in the OS queue, would most likely wedge again or print twice.

`retry_job` (UI/IPC) resets `attempts`, so a user can always ask for a fourth-and-fifth chance manually.

## 6. Performance characteristics

> **These figures are design-target estimates, not measurements.** The converters are wired, but nobody has benchmarked them yet. The image row also describes ImageMagick, which is not used at all. Treat the table as a budget to validate against, and replace it with real numbers. For reference, on an Apple-silicon Mac the real-tool tests convert an RTF and a DOCX (LibreOffice, a fresh profile each) and an HTML page in parallel in a few seconds; the render-worker tests print HTML, Markdown, text and RTF pages through WKWebView.

Intended envelope on a Windows 11 machine, Ryzen 7 5800X, NVMe SSD, files in `%TEMP%`:

| Scenario                               | Per-file time | Throughput     |
| -------------------------------------- | ------------- | -------------- |
| 50× small PDFs (passthrough)           | ~50 ms        | ~20 PDFs/sec spooled |
| 50× 5-page DOCX (LibreOffice, parallel ×4) | ~1.8 s avg | ~2.2 docs/sec to PDF |
| 100× JPG photos (ImageMagick parallel ×8) | ~120 ms avg | ~8 imgs/sec to PDF |
| 20× HTML pages (web view worker parallel ×4) | ~900 ms avg | ~4 pages/sec to PDF |

These are *converter* throughputs. The printer itself is the long pole for actual ink-on-paper.

## 7. Configuration knobs

The **whole** `[engine]` section is these keys, one of them hidden, in `config.toml` (see [data-model.md §1](./data-model.md#1-file-locations) for where that file lives — it is *not* `~/.config/Pluryprint/` on Windows or macOS):

```toml
[engine]
max_concurrent_conversions = 2   # 1..=8, applied at the next app start
conversion_timeout_seconds = 60  # 0 disables the timeout
print_timeout_seconds = 300      # 0 disables it
max_file_size_mb = 200           # submission ceiling, in MB
default_profile = "…"            # profile id; omitted → builtin-quick
# html_renderer = "browser"      # retired (round 6): ignored, removed with a one-time notice
```

Bounds, defaults and the meaning of a stored `0` for each knob come from a single source of truth, `ENGINE_LIMITS` (`settings/mod.rs`): the defaults, the sanitizer that clamps a hand-edited file, the write-path validator, **and** the `limits` the Settings screen validates against are all the same numbers. The UI therefore cannot drift from the server's bounds.

`max_concurrent_conversions` takes effect at the **next start** — the dispatcher reads it once at boot and changing it mid-session does not re-tune the running one.

Keys named in earlier drafts that **do not exist**: `max_file_size_bytes` (it is `max_file_size_mb`), `cache_dir`, `cache_max_bytes`, the entire `[engine.converters]` table, `[engine.printer.windows].sumatra_path`, and `[engine.printer.macos].lpr_path`. External programs are **located by probing**, not configured: `engine::deps` searches bundled → configured → `PATH` → well-known install dirs and reports where it found each one. There is likewise no `[logging]` section — log rotation and retention are fixed constants (see [data-model.md §5.2](./data-model.md#52-rotation-and-retention)).

## 8. Extensibility

Adding a new format is three changes:

1. Add the converter module under `engine/convert/` as a blocking, single-file function with the same `(input, output, progress) -> Result<(), EngineError>` shape.
2. Add the extension to `SUPPORTED_EXTS` and an arm to `convert_to_pdf`.
3. Add a row to the format table in this document, and to the §5 error matrix if it introduces a new failure mode.

If the converter shells out to a program, also add it to `engine::deps` (spec, executable names, install dirs, `blocks` list, remediation text) so a missing tool is reported as a dependency rather than as a mysterious conversion failure — and bring the subprocess hardening listed in [`security-model.md`](./security-model.md) §3.4 with it.

No printer-side change is needed because the new converter feeds the same PDF pipeline.

## 9. Related documents

- [`system-architecture.md`](./system-architecture.md) — the engine in context.
- [`data-model.md`](./data-model.md) — `PrintSettings` schema referenced above.
- [`ipc-and-cli.md`](./ipc-and-cli.md) — exposing engine controls to the UI and CLI.
- ADR [`0003-hybrid-print-engine-strategy`](../adr/0003-hybrid-print-engine-strategy.md) — why this design.
