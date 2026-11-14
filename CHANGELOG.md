# Changelog

All notable changes to Pluryprint (called MaeclyPrint until October 2026) will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **PluryFact integration (in development).** `pluryprint-bridge`, a Chrome Native Messaging host installed next to the app, receives the documents the PluryFact extension downloaded from the SRI (RIDE PDFs, voucher XML, the TXT report), checks each one (size, SHA-256, type; XML without a DTD) and hands them to the app as a context-menu selection: the Print Job screen opens with a **From PluryFact** badge and PluryFact's settings filled in, or the batch is queued or printed when PluryFact asks for that; its jobs say **From PluryFact** in the Queue too. A batch that joins a job you had already started leaves that job's profile and settings alone. No port is opened and no new network traffic is made ([ADR-0016](docs/adr/0016-pluryfact-browser-bridge.md)).
- Registration for Chrome, Edge, Brave and Chromium: the Windows installer writes the per-user keys (and the uninstaller removes them); on macOS the app registers every browser it finds each time it starts. `pluryprint-cli bridge install | uninstall | status` does it by hand, with `--extra-origin` for a development build of PluryFact.
- **Settings → Browser integration** shows, per browser, whether PluryFact can reach Pluryprint, and **Reconnect**.
- Documents PluryFact sent are kept in the data folder's `inbox/` and deleted a week later, at startup.
- XML files print. An Ecuadorian SRI electronic receipt (factura, liquidación de compra, nota de crédito, nota de débito, guía de remisión, comprobante de retención; schema versions 1.0.0 to 2.1.0) prints as a RIDE-like page: issuer and document box, authorisation, access key with its Code 128 barcode, buyer, lines, taxes, payments and additional information, entirely in Spanish whatever the app language, footer included, which says Pluryprint generated it from the XML. The file the SRI portal and web service hand out (`<autorizacion>`), the web service's whole response and a bare receipt all work; a receipt without authorisation data, or one the SRI did not authorise, says so. Any other XML prints as a readable tree. Works from the window, the context menus, Open with and `pluryprint print`.
- `.xml` is offered by the Windows Explorer command, the Linux desktop entry, the KDE service menu and the Nemo actions.

### Changed

- A context-menu handoff file may carry an optional display `label` and per-batch print `overrides`; schema version 1 is unchanged, and both fields are validated like the rest of the file.
- The release workflow fails a Windows, macOS or Linux build that lacks `pluryprint-bridge`.

### Security

- XML files are read without DTDs: a `<!DOCTYPE>` is refused (`xml_dtd_refused`) and no entity is expanded or fetched, so billion-laughs and external-entity (XXE) files cannot do anything. Files are capped at 20 MiB, 200,000 nodes and 256 levels of nesting (`xml_too_large`, `xml_too_deep`).

## [0.0.5] - 2026-11-10

Pluryprint now runs on Linux, on x64 and ARM64, as an AppImage or a `.deb`, with the same file formats as on Windows and macOS. Linux is a preview until it has been tested on real hardware.

### Added

- Linux x64 and ARM64: `Pluryprint_0.0.5_linux_x64.AppImage` / `.deb` and `Pluryprint_0.0.5_linux_arm64.AppImage` / `.deb`. Ubuntu 22.04 or newer, Debian 12 or newer, Fedora 38 or newer; WebKitGTK 2.42 or newer and CUPS. The `.deb` depends on `cups-client`; the AppImage needs `fusermount3` (package `fuse3`).
- Linux printing goes through CUPS, as on macOS. **Printer settings** opens the desktop's printer panel (GNOME, system-config-printer or KDE), else the CUPS page at `localhost:631`.
- Linux prints HTML, Markdown and text through WebKitGTK in an isolated worker, as Windows and macOS do through their web views. HEIC/HEIF photos use `heif-convert` / `heif-dec` from libheif, or ImageMagick; Word, Excel, PowerPoint and RTF use LibreOffice. When one is missing, Pluryprint says which package to install.
- Linux file managers: **Open with Pluryprint** on supported files, and, with the `.deb`, a Pluryprint submenu in Dolphin (**Print now**, **Configure print job…**, **Add to print queue**) and the same three actions in Nemo.
- Linux updates: the AppImage replaces itself; the `.deb` installs the new package after asking for the administrator password.

### Security

- HEIC/HEIF on Linux: a file is checked to be HEIF before any decoder runs, copied into a private folder within the file size limit, and decoded from that copy; ImageMagick reads only HEIC and writes only PNG under a restrictive policy.
- Linux: the memory limit on converters and the render worker covers their whole process tree, helpers that leave the process group included, and the tree is frozen before it is killed.

### Changed

- Dependencies: Tauri 2.12.3 with `tauri-plugin-dialog` 2.8.4 (file dialogs on `rfd` 0.17), `tauri-plugin-single-instance` 2.5.5 (on Windows, a `|` in a path handed to the running window is no longer split, and a second launch gives up after 10 seconds if the first window stops answering), `tauri-plugin-opener` 2.7.2 and `tauri-plugin-notification` 2.8, all still on Tauri 2.12; `@tauri-apps/api` 2.12.4, `@tauri-apps/plugin-dialog` 2.8.4 and `@tauri-apps/cli` 2.12.4 on the UI side. `zeroize` 1.9.1 hardens how a PDF password is wiped; `ureq` 3.5 decodes proxy credentials as curl does (a literal `%` in a proxy password is written `%25`). Minor and patch updates of `serde_json`, `uuid`, `encoding_rs` and zustand 5.0.16.
- Development: Vite 8.3.5, `@vitejs/plugin-react` 6.2.1, Vitest 5.0.5, jsdom 30.1.3, `@testing-library/user-event` 14.6.8 and WebdriverIO 10.1 for the end-to-end suite.

## [0.0.4] - 2026-11-05

Windows prints are confirmed again: a job that printed is shown as printed instead of _Unconfirmed_. Also dependency security updates and a leaner CI.

### Security

- Dependencies: `tauri-plugin-updater` 2.14 (still on Tauri 2.12) moves its archive support to `zip` 9, fixing RUSTSEC-2026-0367, -0368, -0381 and -0429 in `zip` 4. `rustls` 0.23.46, `rustls-webpki` 0.103.16, `webpki-roots` / `webpki-root-certs` 1.0.11, `tokio` 1.54 and `pulldown-cmark` 0.13.6 fix the other advisories published in October 2026.

### Changed

- CI: pull requests run only the essential checks (lint, typecheck, format and unit tests for the frontend; fmt, clippy and the full suite on Windows; clippy and the unit tests on macOS). Dependency audits moved to a weekly `audit.yml` that also runs on lockfile changes, and to the release guard, so an advisory published against an unchanged tree no longer fails an unrelated pull request. `make check-full` runs everything locally.

### Fixed

- Windows: a job that printed is now shown as printed instead of _Unconfirmed_. Windows removes a printed job from its queue about a millisecond after marking it printed, so checking the queue every few seconds never saw it; Pluryprint now listens to the print queue's change notifications and remembers that the job was printed. A job removed from the Windows queue without printing is still _Unconfirmed_.

## [0.0.3] - 2026-10-11

Maintenance release: the Windows build compiles one copy of the Windows API bindings, the Explorer extension reports the right version, and the development tooling moves forward.

### Changed

- Dependencies: the Windows build uses `windows` 0.62 throughout, the line Tauri 2.12 and WebView2 are built on, instead of 0.58 next to it, so the app carries one copy of the Windows API bindings. Printing, printer discovery and the HTML render worker are unchanged.
- Development: TypeScript 6.0, lint-staged 17 and commitlint 21. TypeScript 6 deprecates `baseUrl`, so the `@/` import alias is now declared relative to `tsconfig.json`.
- CI: Dependabot no longer proposes, on their own, `windows` / `windows-core` lines other than the one Tauri uses, `usvg` / `pdf-writer` releases ahead of `svg2pdf`, `base64` 0.23 ahead of the rest of the tree, or `@types/node` majors ahead of the supported Node.js release (22). Patch releases still come through.

### Fixed

- Windows: the Explorer extension (`shell_ext_win.dll`) now reports the app's version in its file properties (0.0.3.0); in 0.0.2 it still said 0.0.1.0. `scripts/bump-version.sh` moves it with the app, and the release workflow refuses a tag whose extension version differs.

## [0.0.2] - 2026-10-10

Maintenance release, and the first update an installed 0.0.1 receives through the in-app updater: Tauri 2.12 and other dependency updates, update signatures bound to their version, the repository's new address, and fixes to the Home screen and the Windows uninstaller.

### Changed

- Dependencies: Tauri 2.12 (`tauri` 2.12, `@tauri-apps/api` and `@tauri-apps/cli` 2.12) with its dialog, opener, single-instance, notification and updater plugins. The HTML render worker on Windows moves with it to `webview2-com` 0.39 and `windows` 0.62, the WebView2 bindings Tauri itself uses. Tauri 2.12 drops Windows 7, which Pluryprint never supported.
- Development: building from source now needs Rust 1.90 or newer, the minimum Tauri 2.12 sets.
- Dependencies: the update check now uses `ureq` 3; its proxy (`HTTPS_PROXY` / `HTTP_PROXY` only), 10-second timeout and error handling are unchanged. Minor and patch updates of the Rust host (serde, clap, time, uuid, flate2 and others) and zustand 5.0.15 in the UI.
- Development: the frontend builds with Vite 8 and `@vitejs/plugin-react` 6 (minified by Vite's own oxc minifier, still for the same Safari 13 floor), and is formatted with Prettier 3.9. The lint and test toolchain moves to `globals` 17 and `@testing-library/user-event` 14.6.7.
- CI: Dependabot no longer proposes `lopdf` minor or major releases on their own; `lopdf` moves together with `printpdf`, which is still built on 0.44.
- The repository is now `github.com/MAECLY/pluryprint` (it was `MAECLY/maecly-print`). Update checks, About links and issue reports use the new address; 0.0.1 keeps updating through GitHub's redirect from the old one.

### Fixed

- Home: the **Run from CLI** card keeps a third of the row with a long install path. It shows the command with the executable's folder folded into an ellipsis (the full command stays in the tooltip and is still what gets copied), so the page no longer scrolls sideways or clips the command at the default window size.
- Windows uninstaller: uninstalling (also with `uninstall.exe /S`) now removes `HKCU\Software\Maecly\Pluryprint` (the remembered install folder and installer language) and the app window's WebView2 profile, `%LOCALAPPDATA%\com.maecly.pluryprint\EBWebView`, which 0.0.1 left behind. An update keeps both, and your settings, profiles and queue stay unless you tick **Delete the application data**, as before.
- Windows installer: the uninstall entry now has a silent uninstall command (`QuietUninstallString`) and the Pluryprint home page (`URLInfoAbout`, `URLUpdateInfo`, `HelpLink`), and the setup's file properties name its company (Maecly).

### Security

- Updates are now bound to their version. Every update artifact is signed for the release it belongs to (`tauri signer sign --app-version`), and the app refuses an update whose signature names another version, or none, than the update manifest announces (`requireSignedVersion`). A tampered manifest can no longer offer an older, genuinely signed release under a newer version number. The release workflow checks every signature before the draft is finished.

## [0.0.1] - 2026-10-10

First public preview of Pluryprint: batch printing of PDFs, images, Office/OpenDocument, HTML, Markdown, text, SVG and HEIC files from the app, the CLI and the Windows Explorer / macOS Finder menus, on Windows 10/11 (x64, ARM64) and macOS (Apple silicon, Intel). Installers are not signed with an Apple or Microsoft certificate; update artifacts are signed and verified by the app.

### Changed

- Dependencies: React 19.3 and sonner 2 in the UI; tauri plugins, serde_json, tokio, `directories` 6 and `toml` 1 in the Rust host. The config, cache and log folders and the `config.toml` / `profiles.toml` formats written by the pre-release builds are unchanged, and tests now pin both.
- Development: building from source now needs Node.js 22.22.2 or newer (CI uses Node.js 24 LTS). The test and lint toolchain moved to Vite 7, Vitest 5, jsdom 30 and ESLint 10 with the React Compiler hook rules.
- CI: GitHub Actions moved to their Node.js 24 releases, and Dependabot now sends one minor-and-patch pull request per ecosystem each week, with majors on their own.
- CI: every pull request now runs `cargo audit` on both Rust lockfiles and `pnpm audit` on the production dependencies, and fails on a known vulnerability. The few accepted advisories are listed with their reasons in `src-tauri/.cargo/audit.toml` and `docs/07-build-and-release/ci-cd.md`.

### Security

- A PDF with deeply nested objects could crash Pluryprint (stack overflow in `lopdf`, RUSTSEC-2026-0187) while it was checked for encryption or its pages were counted. PDFs are now read with `lopdf` 0.44, which limits nesting, and such a file is printed or refused instead.
- The updater's TLS stack moved to rustls 0.23.45 (RUSTSEC-2026-0285), and quick-xml (through plist), anyhow and event-listener to their patched releases.
- Development tooling: WebdriverIO 9.32 and pnpm overrides bring 73 dev-only npm advisories to patched releases.

### Added

- Initial repository scaffold: `package.json`, `pnpm-workspace.yaml`, `tsconfig.json`, `vite.config.ts`, `tailwind.config.ts`, ESLint 9 flat config, Prettier, Husky + lint-staged + commitlint.
- Tauri 2 backend skeleton in `src-tauri/` with utility modules (`paths`, `error`, `logging`, `settings`) and a stub IPC command (`app_info`) wired through tauri-specta.
- React + TypeScript frontend scaffold in `src/`: design tokens (CSS variables), `Brand`, `Icon`, and 15 UI primitives ported 1:1 from `claude design/maeclyprint/project/`.
- Six main screens (Home, PrintJob, Queue, Profiles, Settings, About) and onboarding flow rendered with mock data.
- GitHub Actions CI for `windows-latest`: lint, typecheck, vitest, cargo test, cargo clippy, tauri build (unsigned).
- Community files: `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, issue/PR templates, CODEOWNERS, dependabot.
- Printers: add a printer by network discovery or by IP address, installing a real spooler queue (IPP Everywhere via `lpadmin` on macOS/Linux; Microsoft IPP Class Driver on Windows, IPP/IPPS only).
- Printers: **From system** tab to choose which OS spooler queues MaeclyPrint shows, as a soft-disable preference that never modifies the underlying queue.
- Printers: missing-printer reconciliation — reassign, forget, or keep-paused references (routes, tray/paper overrides, default) to a printer the spooler no longer reports.
- Printers: organization-managed default printer via `policy.json`, which locks **Set default** in the UI and takes precedence over user preferences.
- Print Job screen: assemble a batch via the OS file picker or native drag & drop, with per-file inspection (type, size, page count), inline validation (copies, page ranges), and an honesty "resolved command" preview of the exact `lp` / SumatraPDF invocation; submit straight into the print queue.
- Persistent print queue with a single-writer dispatcher: the convert → print pipeline (PDF passthrough + image conversion), up to two concurrent conversions and one print at a time, automatic retry with backoff, per-job and whole-queue pause/resume/cancel, crash recovery, and a 200-job finished-history cap in `queue.json`.
- Queue screen wired to live `queue-updated` / `job-progress-event` events (List, Board, and Timeline views); Home "Recent activity" surfaces the last finished jobs.
- Print engine: image → PDF conversion for `jpg`/`jpeg`/`png`/`tif`/`tiff`/`bmp`/`webp` (pure-Rust `image` + `lopdf`) and PDF passthrough, spooled via CUPS `lp` (macOS/Linux) or SumatraPDF (Windows), with a duplex capability check and a shared page-range parser.
- Profiles: a persisted profile backend (`profiles.toml` for customs, three immutable built-ins in code, default id in `config.toml`) with a master-detail Profiles screen — create, clone, edit with inline validation, mark default, and delete with a 5-second **Undo** that restores the same id (NFR-032). Atomic writes; corrupt-file reads degrade to built-ins only.
- Settings: a persisted app-settings backend (`get_app_settings`/`set_app_settings`) and a Settings screen — appearance (3-way theme incl. System, density), default profile, engine limits (concurrent conversions, conversion timeout, max file size), update channel + check-on-startup, and consent-only telemetry switches. Engine limits are enforced for real: max file size at file inspection/submission; concurrency and timeouts reach the running queue on save, with no restart.
- About: an on-demand update **check** against the GitHub latest release (semver compare), honest MaeclyPrint/React/Tauri component versions with OS · arch, live filesystem paths with **Open logs folder**, and allow-listed external links (release notes, license, docs, report an issue, repository).

### Added — page-completeness campaign

Every screen was audited against its spec and completed end to end; the items below summarise what that campaign shipped.

- **Headless CLI** in the same binary (`maeclyprint print | printers | profiles | queue | update`, `--json`, frozen exit codes, `--invoke=context-menu --files-from`), driving the same cores as the GUI, with a console front end so Windows shells wait for the exit code (FR-008…FR-010).
- **Right-click integration**: a Windows Explorer `IExplorerCommand` handler (`shell-ext-win`, flat submenu with the default profile, up to eight profiles, Configure and Add to queue, labels from a localized `shell-menu.json` cache the app rewrites) and macOS Finder Services. A selection cut to the handoff cap says so (FR-005). The Windows installer does not register the handler yet.
- **Single instance**: a second launch forwards its selection into the open window; selections are drained one after another, never twice (NFR-014).
- **Folders**: dropping or right-clicking a folder asks whether to recurse, with honest counts and walk progress; skipped files are reported with a reason (FR-004/FR-005/FR-059).
- **More formats**: Office (DOCX, XLSX, PPTX, DOC, XLS, PPT), OpenDocument, TXT and RTF via LibreOffice; HTML via Edge/Chrome/Chromium with no network access; Markdown via Pandoc then the browser, with raw HTML and remote images disabled; HEIC/HEIF via the OS decoder. Converters run bounded, killable and without a shell, and page counts and page ranges apply to converted documents (FR-052…FR-057).
- **Dependency probe** (FR-058): missing SumatraPDF, LibreOffice, browser or Pandoc pauses the job with a message naming the program and how to install it; a SumatraPDF fetch script for the Windows bundle that pins the version and refuses to run until its SHA-256 is filled in.
- **Printing fidelity**: any paper size the printer reports (or **Printer default**), collate, portrait/landscape to CUPS, RGB colour only where the printer lists it, per-printer tray and paper defaults on real jobs, a warning when the printer cannot do the chosen sides or colour, and document-bounds checks on page ranges in the GUI and the CLI.
- **Queue**: `done` now means printed — each job is followed in the OS queue (CUPS, Windows spooler) until it leaves it; a separate **print timeout**; **Cancel all**; batches submitted held for review; job errors carry stable, translated reasons; a resumed job re-resolves a printer that is gone.
- **Profiles**: import/export as TOML (fresh ids, de-duplicated names, 1 MiB cap); profiles resolved by id or name everywhere (FR-024).
- **Updates**: a launch-time check 30 s after start, at most daily, with **Remind me later** and **Skip this version**; signed per-user install from About and the CLI, with a second confirmation for a major version (FR-080…FR-087).
- **Notifications**: native OS notifications for finished, failed and stuck batches while the window is in the background, with a switch in Settings (FR-072).
- **Settings**: Language (English/Español, FR-094), Start minimized, Native notifications, and a banner when `config.toml` cannot be read (saves are refused until it is fixed, FR-101).
- **i18n and accessibility**: the whole UI in English and Spanish with a typed key-parity test; IPC errors translated from their code and reason; accessible names, error wiring and real disabled states on form controls; keyboard shortcuts (NFR-034).
- **Windows**: mDNS discovery of IPP/IPPS printers through the DNS Client service; localized paper names resolved through driver paper ids.
- **Logging**: daily JSONL files with 10 MB rolls and 14-day / 7-file retention, panics and frontend errors forwarded to the log, and absolute paths kept out of the default log level (FR-113, NFR-081, NFR-082).
- **Brand**: the Abanico app icon, drawn from a generator, and the same mark inside the app.
- **Tests**: a browser E2E harness with a fake Tauri host (macOS), seven WebdriverIO specs, launch smokes in CI, opt-in real-spooler suites for a CUPS hold queue and a paused Windows queue, and a gate that every Tauri command is registered.

### Added — round 3

- **SVG and SVGZ** print as vector pages (`usvg` + `svg2pdf`) in a hidden worker process, so a hostile drawing can crash only the worker; `<image>` loads only from the drawing's own folder, and SVGZ is inflated with a 64 MB cap (FR-052a).
- **Encrypted PDFs**: a PDF that needs a password pauses its job with **Enter password**; the password decrypts the file in-process (`lopdf`), is never stored and never reaches a command line or a log. The CLI reads it from `--password-file` or `--password-stdin` (exit 13 when it is missing or wrong). A PDF whose permissions forbid printing is refused, never stripped (FR-059).
- **Guided install of a missing dependency**: the Print Job screen's **Install…** shows the exact winget/Homebrew command, Store page or download page, whether Windows asks for administrator approval, the price and the size, and runs nothing until the user confirms (FR-058). The HEIF and HEVC Store extensions are dependencies of their own on Windows.
- **Windows print quality**: Draft and High reach SumatraPDF jobs through a temporary per-user default that is restored byte for byte as soon as the job is in the spooler, journaled so a crash is recovered at the next start; the job is read back to log what it got.
- **Queue**: a job paused because its printer cannot print two-sided or is missing offers **Print one-sided** and **Send to another printer**; a print nobody confirmed ends as **unconfirmed**, never as done.
- **Windows installer**: registers the Explorer command per user (in the Windows 10 menu and, on Windows 11, under **Show more options**) with the DLL of the machine's native architecture, speaks English and Spanish, and with **Delete the application data** removes the real data folders.
- **Release pipeline**: a `vX.Y.Z` or `vX.Y.Z-beta.N` tag builds a draft release for macOS (Apple silicon and Intel) and Windows (x64 and ARM64) with signed updater artifacts, verified signatures, `SHA256SUMS`, the Pluryprint and SumatraPDF sources, and bilingual notes on the Gatekeeper and SmartScreen warnings ([ADR-0011](docs/adr/0011-release-channel.md), [releasing.md](docs/07-build-and-release/releasing.md)).
- **Update channels**: stable reads `releases/latest/download/latest.json` and beta the `channel-beta` release's manifest; the update notice reads the same manifest as the installer, not the GitHub API (FR-085/FR-086).
- **Licences**: `THIRD-PARTY-NOTICES.md` is generated from the lockfiles with every crate's and package's licence text, and CI fails when it is stale; SumatraPDF ships with its GPL written source offer.

### Changed — round 3

- **HTML, Markdown and plain text** are printed by the operating system's own web view (WebView2 / WKWebView) in a hidden worker, with scripts off and no network or file-share access. Edge, Chrome and Pandoc are no longer needed; the browser stays only as a hidden fallback (`[engine] html_renderer = "browser"`) for one version. Markdown is converted in-process, text is decoded in-process, and RTF goes through `textutil` on macOS; LibreOffice is needed only for Office, OpenDocument and (on Windows) RTF.
- **GIF** is refused by name, with a message to convert it to PNG or JPEG (decision D9).
- **SumatraPDF 3.6.1** is bundled for x64 and ARM64 (was 3.5.2), and paper and tray are sent to it as driver ids, so a Spanish driver's "Carta" no longer drops the paper size.
- Every converter, spooler, printer-management, submission and settings refusal is shown in the user's language, with the tool's own words kept as the technical detail; the printed test page follows the app's language.
- A capability the OS could not read is shown as "not reported" instead of "no".

### Changed — round 6

- **The headless Edge/Chrome fallback for HTML, Markdown and text is removed** (`[engine] html_renderer = "browser"`, `PLURYPRINT_HTML_RENDERER`). Pages always print with the system web view, whose network layer is the security boundary. A `config.toml` that still asks for the browser keeps working; the app removes the key and says so once.
- A page with an empty or unknown declared charset (`<meta charset="">`, `MacRoman`) prints, decoded as UTF-8 or Windows-1252, instead of failing with `page_not_checkable`.

### Fixed — round 7

- On Windows, a WebView2 started with a profile named in `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` or `WEBVIEW2_USER_DATA_FOLDER` (the Edge WebDriver used by the end-to-end tests does this) recorded a failed Windows logon per start, and a full test run could lock the account. That profile is now stamped too.
- **Security:** on macOS a symbolic link inside a printed page's folder (`doc/link.png -> ../secret.png`, which an extracted zip can carry) let the page load any file the user can read. WebKit now reads no file itself: the render worker serves the page and its folder through its own URL scheme handler, opens each file without following a link out of the folder and checks the open file's path, and every `file:` URL is blocked.
- **Security:** the HTML/Markdown/text render worker now runs under a 2 GiB memory cap, like the SVG worker. On Windows a Job Object holds the worker and the WebView2 processes it starts; on macOS a watchdog samples the worker's process group, which does not reach WebKit's web-content and networking processes (XPC services that launchd starts), so a page's own memory there is not capped. A page that needs more ends with the new terminal reason `page_too_large`.
- **Security:** an SVG whose clip paths or masks use the default `userSpaceOnUse` units and are nested inside each other (3.6 KB) no longer turns into a 78 MB PDF: every reference is charged, so it fails at once as `svg_too_large`.

### Fixed — round 6

- **Security:** on macOS a printed page could load a file from its folder's parent (`../x.png` or an absolute `file:` URL): the content rule list now admits only the document's own folder, `data:` and `blob:`, and a navigation delegate cancels navigations the worker did not start.
- **Security:** on Windows every web-view request is answered by the worker (a logged 403 for anything outside the document's folder), frame navigations and new windows are refused, and a served file is checked on its open handle so a junction or a swapped link cannot reach outside the folder.

### Fixed — round 3

- Transparent PNG and WebP images printed as a black box; they are now flattened onto white.
- LibreOffice conversions could fetch a document's linked pictures and objects from the network or a file share; the private profile now blocks it.
- A Windows job id reused by the spooler after a restart could be followed or cancelled as ours; job references now carry the spooler's submission stamp.
- A context-menu selection sent to the running app could stall until the next click (menu mode from a faked Alt key).
- `queue cancel` from the CLI could race an app starting at the same moment; `update install` no longer proceeds when the app lock cannot be checked; `print` reports a file held by a printer in error as held, not done.
- A `policy.json` saved as UTF-16 or with a byte-order mark was ignored.
- Unticking a printer in **From system** skipped the questions **Remove** asks and could leave the default pointing at it.

### Changed

- **Relicensed under AGPL-3.0-only; contributions now under the CLA.** Pluryprint moves from GPL-3.0-only to the GNU Affero General Public License v3.0, the model Versorium uses: contributions come in under [`CLA.md`](CLA.md) (the DCO sign-off check is gone), `TRADEMARKS.md` keeps the name and mark out of the licence under AGPL section 7(e), and [`THIRD-PARTY-NOTICES.md`](THIRD-PARTY-NOTICES.md) ships inside the app bundle with `LICENSE` and the licence texts in `LICENSES/`. About links the licence, the CLA and the notices. See [ADR-0010](docs/adr/0010-relicense-to-agpl-3.0.md).
- **Renamed to Pluryprint.** The product, binaries (`pluryprint`, `pluryprint-cli`), bundle identifier (`com.maecly.pluryprint`), data folders, log files, Finder service and Explorer verb ("Print with Pluryprint") and environment variables (`PLURYPRINT_*`) all carry the new name. An existing MaeclyPrint install upgrades in place: its settings, profiles, queue and logs move to the new folders on first launch (never overwriting), admin policies and `MAECLYPRINT_*` variables are still honoured as a fallback, and the Windows installer removes the old registration. See [ADR-0009](docs/adr/0009-rename-to-pluryprint.md). The repository URL is unchanged.
- Appearance (theme + density) now persists to `config.toml` as the source of truth, with a localStorage instant-boot cache; on divergence the backend value wins.
- The default print profile is now **Quick** (`builtin-quick`) rather than the earlier `Quick BW Duplex`; the default is stored as a profile id in `config.toml`'s `[engine].default_profile`.

### Changed — page-completeness campaign

- Windows ships only the per-user NSIS installer; the per-machine MSI is no longer built (ADR-0008).
- The repository identity is `github.com/MAECLY/maecly-print` everywhere (About links, update check, issue reports).
- `set_app_settings` never writes `engine.default_profile`, so a stale window cannot revert a default the CLI changed.

### Fixed

- Printers: **Remove** is hidden for virtual printers (e.g. Microsoft Print to PDF) so they can't be soft-disabled by mistake; **Restore** stays reachable.
- Docs: printers documentation no longer understates support — **Set default** and **Test page** have been live spooler actions, not local-only stubs.
- Context menu: **Print with “<profile>”** on a profile created after the window opened (for example with `maeclyprint profiles create`) printed with the default profile on the OS default printer. The app now re-reads profiles before resolving a selection, and a profile that matches nothing prints nothing: an error names it and the files open on the Print Job screen (FR-001).
- Security hardening from the campaign's review: CUPS option values that `lp` would split are refused, PowerShell arguments for Add-Printer travel as environment data, `open_external` parses URLs with a WHATWG parser before the allow-list, `policy.json` ownership is checked on the handle that is read, and the command preview is quoted so pasting it cannot run a file name.

### Notes

- M1 "Print Core": the desktop GUI enumerates real printers, prints every supported format end to end through the queue, and persists profiles and app settings; the CLI, SumatraPDF bundling, the signed per-user update install, the installer's Explorer registration and the release workflow have since landed. Since then, 0.0.1 preview has been published (2026-10-10) and the repository is public. Still open: there are no nightly builds (the nightly channel follows beta), the Windows 11 top-level menu needs a signed sparse package that waits for a certificate, and nothing is signed with an Apple or Microsoft certificate (ADR-0011).
- Local prerequisites for running `pnpm tauri dev`: Rust stable 1.78+, Node 20 LTS, pnpm 9, Visual Studio Build Tools 2022 with C++ workload + Windows 11 SDK + ATL. See `docs/08-development/local-setup.md`.

### Added — initial commit (2026-04-26)

- Initial commit with `LICENSE` (originally GPLv3, relicensed to AGPL-3.0-only before this release) and the documentation tree under `docs/`.
- Claude Design handoff bundle under `claude design/maeclyprint/`.
