# Third-Party Licenses

> **TL;DR.** Pluryprint is AGPL-3.0-only. It bundles SumatraPDF on Windows (GPL-3.0, with MuPDF inside under AGPL-3.0 — fetched at packaging time by `scripts/fetch-sumatrapdf.*`, never committed) and depends on Tauri/Rust/React (all permissive licenses, AGPL-compatible). On Linux the AppImage also carries the LGPL-2.1-or-later WebKitGTK/GTK/GLib stack as shared libraries, with a Corresponding Source pointer ([ADR-0015](../adr/0015-linux-support.md)); the `.deb` uses the distribution's. External tools we invoke as subprocesses (LibreOffice, CUPS `lp`, macOS `sips` and `textutil`, and on Linux libheif's tools or ImageMagick for HEIC) are not embedded; per FSF guidance, subprocess invocation does not create a derivative work, so their licenses do not constrain our license. The notices themselves ship in every bundle as [`THIRD-PARTY-NOTICES.md`](../../THIRD-PARTY-NOTICES.md), with `LICENSE` and the texts in `LICENSES/`.

This document inventories every third-party component Pluryprint depends on, with its license, our usage pattern (embedded vs subprocess), and the AGPL-3.0 compatibility analysis. This is the source of truth for legal compliance; the notice that ships with the app, covering every transitive crate as well, is [`THIRD-PARTY-NOTICES.md`](../../THIRD-PARTY-NOTICES.md) at the repository root.

If you find a discrepancy or a missing component, file an issue immediately.

## Master inventory

The tables below are derived from the **actual manifests**: `src-tauri/Cargo.toml`, `shell-ext-win/Cargo.toml` and `package.json`. Versions are the declared requirements, not resolved lockfile versions; the per-release inventory of every crate and package that ships, transitive ones included, is [`THIRD-PARTY-NOTICES.md`](../../THIRD-PARTY-NOTICES.md) (see "Compliance artifacts we ship"). Licenses marked *(typical)* are the customary license of that crate and must be confirmed against the generated manifest before a release.

### Rust — `src-tauri` (direct dependencies, all linked into the binary)

| Crate | Declared version | License (typical) | Used for | AGPL-3.0 compatible? |
|---|---|---|---|---|
| `tauri` | 2.12 | MIT / Apache-2.0 | App framework | Yes |
| `tauri-build` | 2.7 | MIT / Apache-2.0 | Build script (not distributed) | N/A |
| `tauri-plugin-dialog` | 2 | MIT / Apache-2.0 | Native file picker | Yes |
| `tauri-plugin-opener` | 2 | MIT / Apache-2.0 | Open URLs / reveal folders | Yes |
| `tauri-plugin-single-instance` | 2 | MIT / Apache-2.0 | Forward a second launch (NFR-014) | Yes |
| `tauri-plugin-notification` | ~2.5 | MIT / Apache-2.0 | Native OS notifications (FR-072) | Yes |
| `tauri-plugin-updater` | ~2.13 | MIT / Apache-2.0 | Signed update download, verification and install (FR-082/083) | Yes |
| `serde`, `serde_json` | 1 | MIT / Apache-2.0 | Serialization | Yes |
| `toml` | 1 | MIT / Apache-2.0 | `config.toml`, `profiles.toml` | Yes |
| `tokio` | 1.41 | MIT | Async runtime (queue dispatcher) | Yes |
| `tracing`, `tracing-subscriber`, `tracing-appender` | 0.1 / 0.3 / 0.2 | MIT | Structured logging | Yes |
| `thiserror` | **2** | MIT / Apache-2.0 | Library error types | Yes |
| `anyhow` | 1 | MIT / Apache-2.0 | Application error wrapping | Yes |
| `directories` | 6 | MIT / Apache-2.0 | Per-OS config/data/cache/log paths | Yes |
| `tempfile` | 3 | MIT / Apache-2.0 | Atomic writes, private conversion cache | Yes |
| `walkdir` | 2 | MIT / Unlicense | Folder expansion (FR-004) | Yes |
| `clap` | 4 (`derive`) | MIT / Apache-2.0 | Headless CLI surface | Yes |
| `wait-timeout` | 0.2 | MIT / Apache-2.0 | Bounded wait on the print child process | Yes |
| `semver` | 1 | MIT / Apache-2.0 | Update-check version comparison | Yes |
| `ureq` | 3.4 | MIT / Apache-2.0 | Blocking HTTPS for the GitHub release check | Yes |
| `percent-encoding` | 2.3 | MIT / Apache-2.0 | Prefilled "report issue" URL | Yes |
| `url` | 2.5 | MIT / Apache-2.0 | WHATWG URL parsing for the external-link allow-list | Yes |
| `specta` | `=2.0.0-rc.22` | MIT / Apache-2.0 | Type reflection for the IPC bindings | Yes |
| `specta-typescript` | `=0.0.9` | MIT / Apache-2.0 | TypeScript emitter | Yes |
| `tauri-specta` | `=2.0.0-rc.21` | MIT / Apache-2.0 | Typed command/event bindings | Yes |
| `printpdf` | 0.12.8 (`default-features = false`) | MIT | PDF primitives for the printer test page (writes only, never parses input) | Yes |
| `image` | 0.25 (jpeg/png/tiff/bmp/webp only) | MIT / Apache-2.0 | **In-process** image decoding | Yes |
| `lopdf` | 0.44 (`default-features = false`) | MIT | PDF parsing, hand-built image PDFs, encrypted-PDF detection and decryption (FR-059); the line printpdf 0.12 is built on | Yes |
| `svg2pdf`, `usvg` | 0.13 / `=0.45.1` | MIT / Apache-2.0 | SVG/SVGZ → vector PDF (FR-052a); pulls in `resvg`, `fontdb`, `tiny-skia` (BSD-3-Clause), `roxmltree` | Yes |
| `pdf-writer` | 0.12 | MIT / Apache-2.0 | The page around svg2pdf's drawing (FR-052a); the line svg2pdf is built on, already in the tree | Yes |
| `pulldown-cmark` | 0.13 (`html` only) | MIT | Markdown → HTML without Pandoc (FR-056) | Yes |
| `encoding_rs` | 0.8 | MIT / Apache-2.0 (+ BSD-3-Clause data) | Decoding text files that are not UTF-8 (FR-057) | Yes |
| `zeroize` | 1 | MIT / Apache-2.0 | Wiping a PDF password from memory (FR-059, NFR-082) | Yes |
| `flate2` | 1 | MIT / Apache-2.0 | Inflating an SVGZ with a size cap (FR-052a); already in the tree through `usvg`, `lopdf` and `png` | Yes |
| `libc` *(macOS/Linux targets only)* | 0.2 | MIT / Apache-2.0 | The SVG worker's memory cap: `setrlimit(RLIMIT_DATA)` and the parent's memory watchdog (`proc_pid_rusage` on macOS; on Linux the watchdog reads `/proc`); already in the tree through `tokio` and others | Yes |
| `uuid` | 1 (`v7`) | MIT / Apache-2.0 | Time-sortable job ids | Yes |
| `time` | 0.3 | MIT / Apache-2.0 | RFC 3339 timestamps | Yes |
| `windows` *(Windows targets only)* | 0.62 | MIT / Apache-2.0 | winspool + GDI `DeviceCapabilities`, PrintTicket ↔ DEVMODE, console attach, policy registry/owner checks, and `IStream` / `SHCreateMemStream` for the render worker (the line `webview2-com` and wry use) | Yes |
| `webview2-com` *(Windows targets only)* | 0.39 | MIT | WebView2 `PrintToPdf` in the render worker (FR-055) | Yes |
| `windows-core` *(Windows targets only)* | 0.62 | MIT / Apache-2.0 | COM base types (`HSTRING`, `BOOL`, `Interface`) of the line `webview2-com` is generated against | Yes |
| `objc2` *(macOS only)* | 0.6 | MIT | Objective-C runtime for the NSServices provider | Yes |
| `objc2-app-kit`, `objc2-foundation` *(macOS only)* | 0.3 | MIT | AppKit/Foundation bindings (feature-narrowed) | Yes |
| `objc2-web-kit` *(macOS only)* | 0.3 | Zlib / Apache-2.0 / MIT | WKWebView printing in the render worker (FR-055) | Yes |
| `block2` *(macOS only)* | 0.6 | MIT | Objective-C blocks for WebKit's completion handlers in the render worker (FR-055) | Yes |

### Rust — `shell-ext-win` (the Windows Explorer handler, a separate `cdylib`)

| Crate | Declared version | License (typical) | Used for |
|---|---|---|---|
| `serde`, `serde_json` | 1 | MIT / Apache-2.0 | The handoff payload |
| `uuid` | 1 (`v7`) | MIT / Apache-2.0 | Handoff file names |
| `time` | 0.3 | MIT / Apache-2.0 | `invoked_at` stamps |
| `directories` | 6 | MIT / Apache-2.0 | Resolving the handoff directory identically to the app |
| `windows`, `windows-core` *(Windows only)* | 0.58 | MIT / Apache-2.0 | COM / `IExplorerCommand` |
| `embed-resource` *(build-dependency)* | 3 | MIT | Embeds `res/shellext.rc`, including the side-by-side `<msix>` manifest |
| `tempfile` *(dev-dependency)* | 3 | MIT / Apache-2.0 | Tests only; not distributed |

This crate is deliberately **not** a workspace member (see [`../04-architecture/security-model.md`](../04-architecture/security-model.md) §3.1), so it carries its own lockfile and its own license surface.

### Frontend — `package.json` runtime dependencies (bundled into the app)

| Package | Declared version | License | Used for |
|---|---|---|---|
| `react`, `react-dom` | ^19.3.0 | MIT | UI runtime |
| `@tauri-apps/api` | ^2.12.1 | MIT / Apache-2.0 | IPC + event bridge |
| `@tauri-apps/plugin-dialog` | ^2.8.1 | MIT / Apache-2.0 | File picker binding |
| `zustand` | ^5.0.15 | MIT | Frontend state |
| `sonner` | ^2.0.8 | MIT | Toasts |
| JetBrains Mono (font, bundled `.woff2`) | — | SIL OFL 1.1 | Monospace typeface shipped offline |

Localization is **in-house** (`src/i18n/`), not `react-i18next`; there is no i18n runtime dependency.

### Frontend — build/dev only (not distributed)

`vite`, `typescript`, `tailwindcss`, `postcss`, `autoprefixer`, `eslint` (+ plugins), `prettier`, `vitest`, `@vitest/coverage-v8`, `jsdom`, `@testing-library/*`, `@wdio/*` (WebDriver e2e), `husky`, `lint-staged`, `@commitlint/*`, `@tauri-apps/cli` — all MIT or ISC.

Vite 8 builds with Rolldown and Oxc (MIT) and minifies CSS with Lightning CSS (`lightningcss`, MPL-2.0). They run only at build time: no Lightning CSS code ends up in the bundle, and the MPL does not reach the CSS it emits.

Two notes: **Tailwind CSS is MIT and its generated CSS *is* distributed** in the bundle (it is listed here because the tool is build-time; the output is ours). **WebDriverIO (`@wdio/*`) is the e2e driver**, not Playwright.

### Non-crate components

| Component | License | Used for | Embedded? | AGPL-3.0 compatible? |
|---|---|---|---|---|
| SumatraPDF (**3.6.1**, pinned) | **GPL-3.0** (MuPDF inside it: AGPL-3.0) | PDF printing on Windows | Yes — fetched at packaging time and bundled as a separate program | Yes; see the compliance note below |
| LibreOffice | MPL 2.0 | Office and OpenDocument → PDF, and RTF on Windows | No (subprocess) | Yes (separate) |
| Chromium / Microsoft Edge | BSD 3-Clause (Chromium core) | Nothing any more: the headless-browser fallback for HTML, Markdown and text was removed in round 6; pages print with the system web view | No (subprocess) | Yes (separate) |
| sips / Windows Imaging Component | OS components | HEIC/HEIF decoding (`sips` on macOS; WIC plus the Store *HEIF Image Extensions* and *HEVC Video Extensions* on Windows); macOS `textutil` likewise turns RTF into HTML | No (system tool / OS API) | Yes (separate) |
| CUPS `lp` | Apache 2.0 (system component) | Printing on macOS/Linux | No (system binary) | Yes (separate) |
| libheif tools (`heif-dec`, `heif-convert`) / ImageMagick (Linux) | libheif: LGPL-3.0 library, MIT example programs; ImageMagick License (Apache-2.0-style) | HEIC/HEIF decoding on Linux, found at run time and installed by the user | No (subprocess) | Yes (separate) |
| WebView2 loader | BSD-style Microsoft license (loader); Edge engine system-installed | Webview rendering, and printing HTML, Markdown and text in the render worker | Loader statically linked; engine system | Yes (loader); engine N/A |
| WKWebView (macOS) | Apple SLA | Webview rendering, and printing HTML, Markdown and text in the render worker | No (system framework) | N/A (system) |
| WebKitGTK 4.1, GTK 3, GLib, libsoup 3 and their dependencies (Linux) | LGPL-2.1-or-later (WebKitGTK also BSD-2-Clause in parts; some dependencies MIT or similar) | Webview rendering, and printing HTML, Markdown and text in the render worker | `.deb`: no (the distribution's shared libraries). AppImage: yes, as separate shared libraries inside the image | Yes: dynamic linking, replaceable libraries, Corresponding Source offered (see below) |
| NSIS (via `tauri build`) | zlib/libpng | Windows installer | No (build-time only) | N/A |

**Removed from this inventory because they are not used:** `reqwest` (replaced by `ureq`), `tauri-plugin-cli` (the CLI is plain `clap`), `winreg`, `which`, `mime_guess`, `infer`, `rayon`, `notify`, `react-i18next`, `@radix-ui/*`, `react-hook-form`, `@tanstack/react-query`, `lucide-react`, `clsx`, `tailwind-merge`, `playwright`, Inno Setup, create-dmg, and Pandoc (Markdown converts in-process with `pulldown-cmark` since round 3). **qpdf is not invoked at all**, and ImageMagick only on Linux as the last HEIC decoder: images are converted in-process with the `image` crate, and there is no PDF preprocessing step.

For each entry below: the license text source, how we use it, and any compliance notes.

## Embedded components (linked into our binary)

These are statically or dynamically linked into `pluryprint.exe`/`pluryprint` and form a combined work with our code.

### Tauri (MIT / Apache 2.0 dual-license)

- **Used for:** the application framework (main process, IPC, plugins, updater).
- **License text:** [https://github.com/tauri-apps/tauri/blob/dev/LICENSE_MIT](https://github.com/tauri-apps/tauri/blob/dev/LICENSE_MIT) and [https://github.com/tauri-apps/tauri/blob/dev/LICENSE_APACHE-2.0](https://github.com/tauri-apps/tauri/blob/dev/LICENSE_APACHE-2.0).
- **Compatibility:** MIT and Apache 2.0 are both permissive and compatible with GPLv3, and so with the AGPL-3.0. We can include them in an AGPL-3.0 work freely.
- **Compliance:** Tauri and its crates are listed in [`THIRD-PARTY-NOTICES.md`](../../THIRD-PARTY-NOTICES.md), which ships in every bundle; the Apache-2.0 text ships beside it in `LICENSES/Apache-2.0.txt`.

### Rust standard library + crates ecosystem

- **Used for:** language runtime + the utility crates listed in the inventory above.
- **License:** Mostly MIT/Apache 2.0 dual.
- **Compatibility:** all AGPL-3.0-compatible. Beyond MIT and Apache-2.0 the shipped crates use ISC, BSD-3-Clause, Zlib, Unicode-3.0, CDLA-Permissive-2.0 (`webpki-roots`) and MPL-2.0 (`option-ext`, no "Incompatible With Secondary Licenses" notice); none is GPL-2.0-only. The list per crate is in [`THIRD-PARTY-NOTICES.md`](../../THIRD-PARTY-NOTICES.md).
- **Audit — not yet automated.** `cargo about generate` is described below as the intended mechanism, but there is **no `src-tauri/about.toml` and no `deny.toml` in the repository today**, and neither `cargo about` nor `cargo deny` runs in `.github/workflows/ci.yml`. Until those land, the inventory above is maintained by hand against the manifests, which is exactly the failure mode that produced the previous version of this document.

### React + TypeScript ecosystem

- **Used for:** the frontend. React / React DOM (MIT), Zustand (MIT), Sonner (MIT), the `@tauri-apps/*` JS packages (MIT / Apache-2.0), and Tailwind CSS (MIT) whose *generated* CSS ships in the bundle.
- **Localization is in-house** (`src/i18n/`): a small provider plus JSON catalogues per screen. No i18n library is bundled.
- **Compatibility:** AGPL-3.0-compatible.
- **Audit:** `pnpm licenses list` produces an inventory; wiring it into the release is still open (see above).

### `windows` (windows-rs)

- **Used for:** Win32 API bindings — `EnumPrinters` / `DeviceCapabilities` / `GetDefaultPrinter` in the app, and COM / `IExplorerCommand` in the shell extension.
- **License:** MIT / Apache 2.0 dual.
- **Compatibility:** AGPL-3.0-compatible.
- **Note:** the app is on the 0.62 line (the one Tauri's WebView2 bindings use) and the shell extension on 0.58; they are separate builds because the shell extension is a separate crate.

### `objc2`, `objc2-app-kit`, `objc2-foundation` (macOS only)

- **Used for:** the "Print with Pluryprint" NSServices provider declared in `Info.plist` (FR-001).
- **License:** MIT.
- **Compatibility:** AGPL-3.0-compatible.
- **Note:** versions are constrained to the ones tao/wry already resolve, so only one copy of the objc2 tree is linked.

### WebView2 (Windows webview)

- **Loader:** `webview2-com-sys` links Microsoft's `WebView2LoaderStatic.lib` statically into `pluryprint.exe` on the MSVC targets; no `WebView2Loader.dll` is shipped. Its license is a BSD-style Microsoft license, reproduced in [`THIRD-PARTY-NOTICES.md`](../../THIRD-PARTY-NOTICES.md).
- **Edge engine:** system-installed (preinstalled on Win 11; bootstrap on Win 10). Microsoft EULA governs it. We do not redistribute the engine itself.
- **Compatibility:** the loader's license is permissive. The engine being a system component (we link to it dynamically at runtime) is analogous to linking against the OS itself; this is the [system library exception](https://www.gnu.org/licenses/gpl-faq.en.html#WindowsRuntimeAndGPL), which the AGPL-3.0 shares with the GPLv3.

### WKWebView (macOS webview)

- **Provided by macOS** as a system framework. We link against it dynamically.
- **Compatibility:** system library; same exception applies.

### WebKitGTK and the GTK stack (Linux webview)

- **Used for:** the app's own window and web view (Tauri's Linux runtime, through the gtk-rs and `webkit2gtk` bindings, which are MIT crates in the inventory's transitive notices) and the render worker's printing (FR-055), from 0.0.5 ([ADR-0015](../adr/0015-linux-support.md)).
- **`.deb`:** links dynamically against the distribution's `libwebkit2gtk-4.1-0`, `libgtk-3-0` and the rest, declared as package dependencies. Nothing of them is shipped, so this is the [system library exception](https://www.gnu.org/licenses/gpl-faq.en.html#SystemLibraryException) case again.
- **AppImage:** carries those libraries (WebKitGTK, GTK 3, GLib, libsoup 3 and what they need from Ubuntu 22.04) as separate shared objects inside the image, so they are *distributed* by us. The LGPL-2.1-or-later allows it with conditions we meet: the libraries stay dynamically linked and replaceable (`--appimage-extract` unpacks the image; any `.so` can be swapped), their licence texts and copyright notices travel with the notices, and `THIRD-PARTY-NOTICES.md` names each bundled library and points at its Corresponding Source (the matching Ubuntu 22.04 source packages). AGPL-3.0 code may be combined with LGPL-2.1-or-later libraries this way.
- **Compatibility:** yes.

## Embedded data (bundled binaries we ship)

These are not linked but shipped as separate files in our installer.

### SumatraPDF 3.6.1 (GPL-3.0) — the one third-party program we redistribute

- **Used for:** silent PDF printing on Windows (`-print-to`, `-print-settings`, `-silent`, `-exit-when-done`). Not used on macOS/Linux, which print through CUPS `lp`.
- **License:** [GPL-3.0](https://www.gnu.org/licenses/gpl-3.0.html). The MuPDF library compiled into `SumatraPDF.exe` is [AGPL-3.0](https://www.gnu.org/licenses/agpl-3.0.html) (Artifex Software).
- **How it gets into the bundle:** `scripts/fetch-sumatrapdf.ps1` (Windows) / `scripts/fetch-sumatrapdf.sh` (POSIX twin, for cross-building CI and for verifying the pins off-Windows). They download the **pinned** upstream release, version `3.6.1`, for the target architecture: `https://www.sumatrapdfreader.org/dl/rel/3.6.1/SumatraPDF-3.6.1-64.zip` for x64 and `...-arm64.zip` for ARM64 (an ARM64 installer carries the native ARM64 build, not the x64 one under emulation). Each archive is checked against a pinned SHA-256 (x64 `98b33a51...51877a544`, ARM64 `f21091ca...3667ffe`, computed on macOS and on Windows 11 ARM64 and identical to the hashes the Scoop `extras` bucket publishes for the same archives). The executable is installed as `SumatraPDF.exe` in `src-tauri/resources/sumatrapdf/`, and `tauri.conf.json` ships that directory via `bundle.resources` (`"resources/sumatrapdf/*": "resources/sumatrapdf/"`). The glob matches only the README on a macOS/Linux bundle.
- **The licence travels with it.** The portable archives contain the executable only, no licence file. The scripts therefore fetch `COPYING` (the GPL-3.0 text, byte-identical to our `LICENSES/GPL-3.0.txt`), `COPYING.BSD` and `AUTHORS` from the SumatraPDF repository at commit `16c59fde8b824ab54c56f23aef910a6fdd874ad0`, which the release tag `3.6.1rel` points to. Each file has its own pinned SHA-256, and a missing or altered file fails the script. They land next to the executable as `LICENSE.txt`, `COPYING.BSD.txt` and `AUTHORS.txt`, with a `SOURCE.txt` naming the version, architecture, archive URL and the source commit, and a `SOURCE-OFFER.txt` carrying the GPL-3.0 §6 written offer.
- **Why not committed:** ~16 MB of platform-specific artifact git would keep forever, paid for by every contributor including those who can never run it; and it is not ours to re-host. Fetching the vendor's own release at packaging time keeps provenance (and these GPL-3.0 obligations) where they belong. A pinned URL plus a pinned checksum is a stronger guarantee than a committed blob, and it is reviewable in a diff. `src-tauri/tests/fetch_scripts.rs` checks that both scripts carry the same pins and that the PowerShell one stays ASCII (Windows PowerShell 5.1 cannot parse a BOM-less UTF-8 script with typographic punctuation).
- **Source code:** [https://github.com/sumatrapdfreader/sumatrapdf](https://github.com/sumatrapdfreader/sumatrapdf), tag `3.6.1rel` (MuPDF is under `mupdf/`). Every Pluryprint release also attaches that exact tree as `SumatraPDF-3.6.1-source.tar.gz` (`.github/workflows/release.yml`).
- **Compatibility:** SumatraPDF is a separate program that Pluryprint runs as a process, so the two are an aggregate, not a combined work. Even if they were combined, section 13 of both the GPL-3.0 and the AGPL-3.0 explicitly allows linking one with the other. MuPDF is AGPL-3.0, the same license as Pluryprint.
- **Compliance (GPL-3.0 §6):** because we redistribute a GPL-3.0 binary we must offer its corresponding source. We do so by shipping the licence (`LICENSE.txt`, `COPYING.BSD.txt`, `AUTHORS.txt`) `SOURCE.txt`, which names the exact upstream commit, and `SOURCE-OFFER.txt`, the written offer, beside the executable, by attaching the source archive to every release, the GPL-3.0 text in `LICENSES/GPL-3.0.txt`, and a notice naming the version and the source tag in `THIRD-PARTY-NOTICES.md`, and by linking to the upstream repository from the About screen and from this document. **Pin the exact upstream version in the release notes** so "corresponding source" is unambiguous.

### Future bundled components

If we ever bundle Pandoc (GPLv2-or-later) or another GPL'd binary, the same compliance pattern applies: pin the version and checksum, bundle the license text, link to the source, and name the version in the release notes.

## External components (subprocess invocation)

These are tools we shell out to with `std::process::Command`. We never bundle them. CUPS `lp` (macOS/Linux) and macOS `sips` and `textutil` are part of the operating system: the app runs `sips` and `textutil` from `/usr/bin` and `lp` from `PATH`, and does not probe for any of them. LibreOffice is the one the user installs separately (or lets the app install through the system package manager after consent, D13); the app probes for it and reports when it is missing (`engine::deps`, FR-058). All of them are invoked as separate processes, each with an argument vector and never through a shell. **Per the [FSF's guidance](https://www.gnu.org/licenses/gpl-faq.html#GPLPlugins) and [Mere Aggregation FAQ](https://www.gnu.org/licenses/gpl-faq.en.html#MereAggregation), subprocess invocation does not create a combined work, so the subprocess's license does not infect ours.**

Quoting the FSF directly:

> "If two programs run at the same time and communicate using exec, fork, or pipes, then they are normally separate programs."

This is the legal foundation for using LibreOffice (MPL 2.0) in an AGPL-3.0 project without its license contaminating ours.

### LibreOffice (MPL 2.0)

- **Used for:** Office document → PDF conversion. Invoked as `soffice --headless --convert-to pdf <file>`.
- **License:** [Mozilla Public License 2.0](https://www.mozilla.org/MPL/2.0/).
- **Embedded?** No. Subprocess only.
- **Compatibility:** MPL 2.0 is a weak copyleft. Even if we did link against LibreOffice (we don't), MPL 2.0 has explicit GPL compatibility through the secondary license clause. As subprocess invocation, this is moot.
- **Compliance:** none required from us. We document the dependency; users install separately.

### Chromium / Microsoft Edge (BSD 3-Clause)

- **Used for:** nothing. The headless `[engine] html_renderer = "browser"` fallback that ran `msedge --headless --print-to-pdf` was removed in round 6 (ADR-0012); HTML, Markdown and text print with the system web view.
- **License:** [BSD 3-Clause](https://opensource.org/licenses/BSD-3-Clause) (Chromium core; Edge adds Microsoft proprietary code we don't redistribute).
- **Embedded?** No, and no longer invoked. (The WebView2 runtime that renders the app and prints pages is a system component; see the Edge engine note above.)
- **Compatibility:** BSD 3-Clause is permissive; with nothing invoked, nothing applies. The section stays so the history of the inventory is clear.

### Pandoc (GPLv2-or-later)

- **Used for:** nothing since round 3: Markdown converts in-process (`pulldown-cmark`). The dependency id stays only so an old `queue.json` that names it still loads.
- **License:** [GPLv2-or-later](https://www.gnu.org/licenses/gpl-2.0.html). Pandoc itself is GPL.
- **Embedded?** No, and no longer invoked.
- **Compatibility:** GPLv2-or-later can be used under GPLv3, which section 13 lets be combined with the AGPL-3.0. With nothing invoked, this is moot.

### CUPS `lp` (Apache 2.0, system component)

- **Used for:** spooling the final PDF on macOS and Linux (`lp -d … -n … -o …`), and cancelling by the returned job id.
- **Embedded?** No. It is part of the operating system.
- **Compatibility:** a system utility invoked as a subprocess; nothing to comply with.

### Image conversion — no third party involved

Image → PDF conversion happens **inside** Pluryprint using the `image` and `lopdf` crates (both permissive, both in the inventory above). **ImageMagick is not used for images**; on Linux it is only the last-resort HEIC decoder, run as a subprocess when libheif's tools are absent. Likewise there is **no qpdf step**: nothing linearizes or decrypts PDFs. Earlier versions of this document listed both; they were never dependencies.

## Build-time only (not distributed)

These tools are used to build Pluryprint but are not part of the distributed binary.

### Vite (MIT)

- **Used for:** frontend bundling.
- **Distributed?** No. Vite produces output JS/CSS/HTML; only the output is distributed.
- **Compliance:** none required.

### NSIS (via `tauri build`)

- **Used for:** building the Windows installer. `tauri.conf.json` sets `bundle.windows.nsis.installMode = "currentUser"`. **Inno Setup is not used**; earlier drafts said otherwise.
- **License:** zlib/libpng.
- **Distributed?** The NSIS stub is part of the produced installer; NSIS itself is not.

### Tailwind CSS / PostCSS / Autoprefixer (MIT)

- **Used for:** layout utilities only. Design tokens are CSS custom properties in `src/styles/tokens.css`; Tailwind's `preflight` is disabled.
- **Distributed?** The *generated* CSS is bundled; the toolchain is not.

### WebdriverIO (`@wdio/*`, MIT)

- **Used for:** the end-to-end lane (`pnpm test:e2e`, `e2e/wdio.conf.ts`). **Playwright is not used.**
- **Distributed?** No.

### MakeAppx, signtool, codesign, notarytool, pkgbuild, productbuild

- **Used for:** packaging and signing.
- **Distributed?** No. These are platform-vendor SDKs (Microsoft, Apple).
- **Compliance:** governed by Microsoft / Apple SDK terms, which permit normal developer use.

## Compliance artifacts we ship

Every bundle carries, via `bundle.resources` in `tauri.conf.json` (checked by `src-tauri/tests/licence_bundle.rs`):

```
<resource-dir>/
├── LICENSE                    # AGPL-3.0, Pluryprint's own license (full text)
├── THIRD-PARTY-NOTICES.md     # every shipped component, crate and package, with copyright lines
├── LICENSES/
│   ├── Apache-2.0.txt         # Apache-2.0-only crates, ring
│   ├── GPL-3.0.txt            # SumatraPDF
│   └── OFL-1.1.txt            # JetBrains Mono, with its copyright line
└── resources/sumatrapdf/      # Windows only, once fetched
    ├── SumatraPDF.exe
    └── LICENSE.txt            # SumatraPDF's own licence file, when its archive has one
```

On macOS `<resource-dir>` is `Pluryprint.app/Contents/Resources/`; on Windows it is the install folder; on Linux it is the package's folder under `/usr/lib/` (inside the AppImage, the same path in the mounted image).

`THIRD-PARTY-NOTICES.md` is generated. Its first part (SumatraPDF, the native code, the WebView2 loader, the NSIS installer stub, the font, the compatibility table) is hand-written in `scripts/licenses/notices-header.md`; the rest reproduces the licence text, with its copyright lines, of every crate compiled into the app or the Explorer DLL for the release targets (the four Windows and macOS triples and, from 0.0.5, `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`; cargo-about, `scripts/licenses/about.toml`) and of every frontend package in the bundle (`pnpm licenses list --prod`):

```bash
cargo install cargo-about --locked --features cli --version 0.9.2   # once
scripts/licenses/generate-notices.sh          # after any dependency change
scripts/licenses/generate-notices.sh --check  # release.yml's guard job
```

A crate whose licence is not in `about.toml`'s accepted list fails the generator instead of shipping unnoticed; adding a licence there needs the compatibility analysis in this document first. `src-tauri/tests/third_party_notices.rs` checks that every direct dependency has an entry and every entry is still in a lockfile.

## Compatibility check process

When adding a new dependency:

1. Identify its license (`cargo metadata` for Rust deps; `npm view <pkg> license` for frontend).
2. Cross-reference against the [FSF compatibility list](https://www.gnu.org/licenses/license-list.html) or [SPDX license list](https://spdx.org/licenses/).
3. If AGPL-3.0-compatible: proceed.
4. If AGPL-3.0-incompatible (e.g. GPL-2.0-only, CDDL, EPL, MPL 1.x without dual-license, MPL-2.0 marked "Incompatible With Secondary Licenses"): **do not add**. Either find a replacement, refactor to subprocess invocation, or skip the feature.

**`cargo deny` is not wired up yet.** There is no `deny.toml` in the repository and `.github/workflows/ci.yml` does not run it, so today step 4 above is enforced by review only. The configuration below is the intended one and should be committed as `deny.toml` when the check lands:

```toml
# deny.toml — PROPOSED, not present in the repo
[licenses]
allow = [
  "MIT",
  "Apache-2.0",
  "Apache-2.0 WITH LLVM-exception",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "ISC",
  "Unicode-3.0",
  "Zlib",
  "CDLA-Permissive-2.0",
  "MPL-2.0",
  "AGPL-3.0",
]
deny = [
  "CDDL-1.0",
  "EPL-1.0",
  "EPL-2.0",
  "GPL-2.0",  # GPL-2.0-only is incompatible with the AGPL-3.0
]
```

Once committed, a PR that introduces a denied license will fail CI. Note that `AGPL-3.0` must stay on the allow list: the project's own crates are `AGPL-3.0-only`.

## "But is X legally a derivative work?"

The case for subprocess invocation **not** being a derivative work has been argued and accepted in many contexts. Beyond the FSF FAQ:

- [Software Freedom Law Center on subprocess invocation](https://www.softwarefreedom.org/resources/2009/the-fsf-policy-on-derivatives.html): "Programs running as separate processes are not combined into a single derivative work."
- [Linux Kernel position](https://www.kernel.org/doc/Documentation/process/license-rules.rst): the syscall boundary is treated as a license boundary.
- [LibreOffice's own FAQ](https://www.libreoffice.org/about-us/licenses/) accepts subprocess invocation as a non-combining use pattern.

We are conservative: when there is doubt, we use subprocess invocation rather than embedding. The clearest example is LibreOffice (MPL 2.0) — even though MPL 2.0 has GPL/AGPL-compatibility clauses, the subprocess pattern removes any ambiguity about whether a combined work exists.

## Trademark notices for components we mention

- "Tauri" is a trademark of the Tauri Programme.
- "React" is a trademark of Meta Platforms, Inc.
- "TypeScript" is a trademark of Microsoft Corporation.
- "Rust" and the Rust logo are trademarks of the Rust Foundation.
- "LibreOffice" is a trademark of The Document Foundation.
- "Microsoft Edge", "Microsoft Print to PDF", "WebView2" are trademarks of Microsoft Corporation.
- "Apple", "macOS", "Apple Silicon", "Xcode", "Finder" are trademarks of Apple Inc.
- "Pandoc" is a trademark of John MacFarlane.
- "SumatraPDF" is the work of Krzysztof Kowalczyk and contributors.
- "MuPDF" is a trademark of Artifex Software, Inc.
- "JetBrains Mono" is a typeface by JetBrains s.r.o., used under the SIL Open Font License 1.1.

We use these names nominatively (to refer to the components themselves), not in a way that implies endorsement.

## Updating this document

This document is updated:

- Whenever a new dependency is added or a major version is bumped.
- Quarterly via a maintainer audit of `cargo about` and `pnpm licenses list` output against this table.
- Before each MAJOR or MINOR release, as part of the release checklist.

If you spot something missing or stale, file an issue tagged `legal`.

## See also

- [license.md](./license.md)
- [trademark.md](./trademark.md)
- [privacy.md](./privacy.md)
