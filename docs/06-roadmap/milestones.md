# Milestones

> **TL;DR.** Pluryprint ships in six milestones (M0–M5) over roughly five to six months of single-developer work, taking the project from an empty repository to a notarized macOS port. A seventh, M6, adds Linux ([ADR-0015](../adr/0015-linux-support.md)). Each milestone has explicit scope, deliverables, exit criteria, and named risks.

This document is the source of truth for what each milestone contains. Anything not listed here is out of scope for that milestone and should be deferred or filed against a later one. See [timeline.md](./timeline.md) for duration estimates and [release-strategy.md](./release-strategy.md) for how these milestones map to public versions.

## Overview

| Milestone | Theme | Public version | Status (2026-10-10, after round 3) |
|---|---|---|---|
| M0 | Foundation | `0.0.1` (repo only) | Done |
| M1 | Print Core (Win) | `0.1.0-alpha` (internal) | Scope delivered, exit criteria unverified |
| M2 | Shell Integration (Win) | `0.1.0-beta` (public) | Installer and classic-menu entry shipped; Windows 11 top-level menu waits for a certificate (D3) |
| M3 | Multi-format (Win) | `0.2.0` | Adapters delivered (every format but GIF, which is refused by decision); Settings dependency panel owed |
| M4 | Auto-update + Polish (Win) | `1.0.0` GA | Update install, channels, release pipeline and i18n delivered; no OS certificates by decision (ADR-0011); 0.0.1 preview published 2026-10-10 |
| M5 | macOS Port | `1.1.0` | Printing, Finder entry and per-architecture `.dmg` delivered; no notarization by decision |
| M6 | Linux Port | `0.0.5` (preview) | Decided 2026-11-09 (ADR-0015); in progress. Status as of that date, not the round-3 date above |

Milestones are strictly sequential. We do not start M(n+1) work until M(n) exit criteria are met. Spillover items roll forward and reduce the next milestone's headroom.

**That rule has been bent.** M2's shell handlers were written before M1 was signed off, and pieces of M4 (i18n) and M5 (CUPS printing, the Finder entry) landed alongside them. Each milestone's status block below says what is actually done rather than what the sequence implies. The overlap is real and is worth an explicit decision: either re-cut the milestone boundaries around what shipped, or accept that M1–M2 have merged into one release.

---

## M0 — Foundation

**Goal:** A browsable, lintable, buildable repository that any contributor can clone and extend.

### Scope

- Repository scaffold: `src-tauri/`, `src/`, `shell-ext-win/`, `installers/`, `scripts/`, `.github/workflows/`.
- Tauri 2 project initialized with React + TypeScript template.
- Documentation tree under `docs/` populated for all sections (this milestone is largely about the docs you are reading right now).
- Basic CI: `cargo test`, `cargo clippy`, `pnpm lint`, `pnpm test`, build (no signing).
- Licensing: `LICENSE` (GPLv3 at the time; AGPL-3.0 since [ADR-0010](../adr/0010-relicense-to-agpl-3.0.md)), `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1), `SECURITY.md`, `CONTRIBUTING.md`.
- `README.md` with badges, install instructions placeholder, status banner.

### Deliverables

| Item | Owner | Form |
|---|---|---|
| `cargo build` succeeds on `windows-latest` | Maintainer | CI green |
| `pnpm tauri dev` boots an empty window | Maintainer | Manual verify |
| All 18 architecture, roadmap, build, and dev docs landed | Maintainer | PR merged |
| `0.0.1` git tag pushed | Maintainer | GitHub Release (notes only) |

### Exit criteria

- Every CI job is green on `main`.
- A new contributor can clone, run `pnpm install && pnpm tauri dev`, and see the app shell within 10 minutes.
- All cross-document links resolve (no 404s in `docs/`).
- The pull request template asks contributors to agree to the [CLA](../../CLA.md). (M0 shipped a DCO sign-off check; it was retired when the project moved to AGPL-3.0 and the CLA, [ADR-0010](../adr/0010-relicense-to-agpl-3.0.md).)

### Risks

| Risk | Mitigation |
|---|---|
| Tauri 2 toolchain churn | Pin exact minor versions in `Cargo.toml` and `package.json`; document override path. |
| Windows SDK version drift on contributor machines | `local-setup.md` pins Visual Studio Build Tools 2022 with required components listed by ID. |
| Doc-set goes stale immediately | Each PR template requires a "Docs updated?" checkbox. |

### Dependencies

None. M0 is the bootstrap.

---

## M1 — Print Core (Win)

**Goal:** Printing works from a CLI on Windows for PDF inputs, with a profiles system and a minimal GUI.

### Status (2026-10-10) — scope delivered

Round 3 closed the items the entry below left open: the **per-user NSIS installer** bundles SumatraPDF 3.6.1 for x64 and ARM64 ([installer-windows.md](../07-build-and-release/installer-windows.md)); `tray` and `quality` **reach the printer** (CUPS options; on Windows the driver's bin id and a temporary per-user quality default, D16); the **release pipeline** signs, verifies and drafts releases on a tag, and the **channels** read their own manifests ([ADR-0011](../adr/0011-release-channel.md)). A print nobody confirmed now ends `unconfirmed`, never done. Still open: the exit criteria below (no fixture-diff print test, no `clippy::panic` gate) and no per-profile notification mute (FR-073). 0.0.1 preview was published on 2026-10-10.

### Status (2026-07-20) — superseded by the entry above

**Delivered since the 2026-07-12 entry below:**

- The **headless CLI** — the largest item that was open. `pluryprint print|printers|profiles|queue|update` parsed with `clap`, `--help` on every subcommand, `--json` output with errors as JSON on stderr, and the documented exit-code table (FR-008/009/010/112). A CLI run creates no window. `src-tauri/tests/cli_contract.rs` pins the surface.
- **Single instance** (NFR-014): a second launch forwards its selection to the first through `tauri-plugin-single-instance` and exits; the running app raises its window and drains the pending selection.
- **Folder selections** (FR-004/005/007/059): `inspect_files` classifies a path as a folder and reports whether it has subfolders, the screen asks recurse-or-top-level, and `expand_folders` walks it. Discarded files come back with a reason instead of vanishing.
- **A dependency probe** (`engine::deps`, FR-058): LibreOffice / Chromium / Pandoc located across bundled dirs, `PATH` and the usual install locations, reported through `app_info` and surfaced on the Print Job screen for the dependencies that actually gate the user's files.
- **A print timeout** and **log rotation with retention** (FR-034/035/113, NFR-012): the spooler child is waited on with a bound and killed and reaped on expiry.
- **i18n, externalized** (FR-090/091/092/093/094): EN + ES catalogues per screen, OS-locale detection on first run, a Settings selector. This is M4 scope pulled forward.
- **Profile import/export** (FR-024) over IPC and in the Profiles screen — v0.3 scope pulled forward. Not in the CLI.
- **The startup update check** (FR-080) on a detached thread after a 30 s delay, with 24 h throttling, plus **defer / skip-this-version** (FR-084).
- **App-wide keyboard shortcuts** (NFR-034) and in-app completion/failure toasts (FR-070/071).

**Still open in M1 after this pass:**

- **No installer** (`installers/` is empty) and **no bundled SumatraPDF** — `scripts/fetch-sumatrapdf.{ps1,sh}` fetch it for development only.
- *(2026-10-09)* Signed, per-user **Install now** (FR-082/083/084/087) and **native notifications** (FR-072) have since landed; see [ADR-0008](../adr/0008-installer-nsis-per-user.md). What remains is the release pipeline that signs and publishes artifacts, and per-channel feeds (the channel selector is persisted but inert). There is still no per-profile notification mute (FR-073).
- The `tray` and `quality` profile fields persist and are editable but are **never sent to a print backend**.
- The M1 **exit criteria** below are unverified: there is no integration test that prints a known-good PDF and diffs it against a fixture, and no `clippy::panic` gate in CI.

---

### Status (2026-07-12) — superseded by the entry above

**Delivered:**

- The GUI **print core**: real printer enumeration, capabilities, discovery, add-by-IP/discovery, set-default, and test page (`ipc::printers` + `platform/{cups,windows}`), plus the convert → print pipeline for PDFs (passthrough) and raster images (pure-Rust `image` + `lopdf`), spooled via CUPS `lp` / SumatraPDF.
- A **persistent print queue**: the single-writer dispatcher (concurrency, retries with backoff, pause/resume/cancel, crash recovery), `queue.json`, and typed `queue-updated` / `job-progress-event` events — wired end-to-end to the Print Job and Queue screens and Home's recent activity.
- A **persisted profiles backend** (`profiles.toml` for customs, built-ins in code, default in `config.toml`): full CRUD with an editor, clone, delete-with-undo, and set-default (`ipc::profiles`, FR-020/021/022/026/027). This closes the profiles work that was open earlier in M1.
- **App settings persistence** (`get_app_settings`/`set_app_settings`) with engine limits enforced for real — file-size at submit/inspect, concurrency + conversion timeout at the next app start — plus theme/density dual-write and a default-profile picker.
- Settings / printer-preferences persistence and an org-managed default via `policy.json`.
- An **About** surface with the read-only **update check** (GitHub `releases/latest`), honest component versions, filesystem paths + open-logs, and allow-listed external links.

Note: several scope lines below are framed Windows-first, but the enumeration/print backends are cross-platform and the CUPS path is exercised in CI.

*(The "still open" list that stood here — CLI unimplemented, shell integration not started — is obsolete. See the 2026-07-20 block above for the current one.)*

### Scope

- Rust backend modules: `engine::router`, `engine::pdf`, `printer::windows`, `queue`, `profiles`, `cli`.
- SumatraPDF integration: bundled binary in `src-tauri/resources/`, invoked via `Command` with structured args.
- CLI: `pluryprint print <files…> --printer <name> --profile <id>` parsed with `clap`.
- TOML profiles: load from `%APPDATA%\Maecly\Pluryprint\config\profiles\*.toml`, schema documented and versioned.
- GUI: bare-bones React pages for Profiles list, Queue view, Settings (printer picker).
- Print queue: Tokio task that processes jobs sequentially, reports progress over Tauri IPC.
- Logging: `tracing` with file rotation under `%LOCALAPPDATA%\Maecly\Pluryprint\data\logs\`.

### Deliverables

| Item | Acceptance |
|---|---|
| Print 50 PDFs in one batch | Wall-clock < 5 min on Microsoft Print to PDF; zero crashes. |
| Profile switching from GUI | Selected profile persists across restart and is honored by next print job. |
| Log file output | One log per app session, JSON lines format. |
| `0.1.0-alpha` internal build | Unsigned, distributed via GitHub Releases marked as pre-release. |

### Exit criteria

- Integration test prints a known-good PDF to "Microsoft Print to PDF" and the output matches a fixture (byte-equal or visual diff < 0.5%).
- All public Rust APIs in `engine` and `printer` documented with rustdoc.
- No `panic!` reachable from any user-driven code path (verified by `cargo clippy -W clippy::panic`).

### Risks

| Risk | Mitigation |
|---|---|
| SumatraPDF CLI flag changes between versions | Pin exact SumatraPDF version in `scripts/update-third-party.ps1`; add smoke test that asserts known flags exist. |
| Win32 print spooler quirks (per-printer driver bugs) | Limit M1 to "Microsoft Print to PDF" and one physical printer model in CI; track other printers as known-issues for M3. |
| Tokio + Tauri IPC backpressure | Bound channel capacity; reject new jobs with a clear error rather than buffering unbounded. |

### Dependencies

- M0 complete.
- SumatraPDF binary licensing review (covered in [third-party-licenses.md](../09-legal/third-party-licenses.md)).

---

## M2 — Shell Integration (Win)

**Goal:** Right-click in File Explorer on a selection of files surfaces a "Print all with Pluryprint" submenu that hands the selection to the running app (or starts it).

### Status (2026-10-10) — shipped in the classic menu

The installer registers the handler per user as an `ExplorerCommandHandler` verb, with the DLL of the operating system's native architecture, and removes it on uninstall ([ADR-0013](../adr/0013-explorer-command-per-user-verb.md), supersedes ADR-0005). Verified on Windows 11 24H2 ARM64 with UAC on, with real right-clicks: the full submenu under **Show more options**, and forwarded selections acted on at once (the window is raised without faked input). That registration is also what the Windows 10 menu shows, so the separate Windows 10 fallback is moot; no Windows 10 machine has run it. **Still open:** the Windows 11 top-level entry, which needs a sparse package signed with a trusted certificate (decision D3, [shell-extensions.md §2.5](../04-architecture/shell-extensions.md)); the x64 installer emulated on ARM is untested; there is no Inno Setup installer and none is planned (ADR-0008).

### Status (2026-07-20) — superseded by the entry above

**Delivered:**

- The **Explorer handler** in `shell-ext-win/`: an in-process COM DLL implementing `IExplorerCommand` with a dynamic submenu (default-profile print, one row per profile, "Configure print job…", "Add to print queue"), a localized string cache, selection enumeration with no count limit (FR-003), a supported-extension filter, and a panic guard so a fault cannot take Explorer down with it. It is a **Rust** crate, not the C++ one this scope line assumed — deliberately, and deliberately outside the `src-tauri` workspace so that crate's `panic = "abort"` cannot neuter `catch_unwind`. Built, clippy-gated and tested in CI (`shell-ext-win/tests/`).
- The **handoff contract**: the DLL writes the selection to a JSON file and launches `pluryprint --invoke=context-menu --files-from <PATH> [--action …] [--profile …]`. This is a handoff file rather than the named pipe the scope line names, because argv on Windows is capped at 32,767 characters. Both ends are pinned by round-trip tests.
- The **macOS counterpart**, pulled forward from M5: an `NSServices` declaration in `src-tauri/Info.plist` plus a provider in `platform/macos/services.rs`, so "Print with Pluryprint" appears in the Finder's Services submenu. No `.appex`, no App Group, no second signature — see the M5 note below.

**Not started — this is what still blocks the `0.1.0-beta`:**

- **Sparse MSIX.** `installers/windows/` contains only a `.gitkeep`. Without it the DLL has no package identity, so on a normal machine the entry never appears no matter how well the handler works.
- **The Inno Setup installer** and the Win 10 registry fallback path.
- Install/uninstall round-trip, reproducible builds, and the Shell Extension Best Practices audit in the exit criteria below — none of them have been run.
- The **Win 10 fallback** (`IShellExtInit` + `IContextMenu`) is not implemented and is documented as deliberately not registered. Windows 10 support is therefore an open decision, not a pending task.

### Scope

- COM DLL in `shell-ext-win/` implementing `IExplorerCommand` (Win 11) and `IShellExtInit` + `IContextMenu` (Win 10 fallback). *Implemented in Rust; the Win 10 fallback is not implemented.*
- Sparse MSIX package in `installers/windows/sparse-msix/` to grant DLL identity for `IExplorerCommand` registration on Win 11.
- Inno Setup installer (`installers/windows/inno-setup.iss`) that:
  - Installs to `%LOCALAPPDATA%\Programs\Pluryprint`.
  - Registers `HKCU\Software\Classes\` entries for the Win 10 fallback path.
  - Calls `Add-AppxPackage` to deploy the sparse MSIX during install, `Remove-AppxPackage` on uninstall.
- IPC handoff: shell extension serializes selection to JSON and hands off to running app via named pipe (or starts app with `--enqueue` CLI arg).
- Submenu items: "Print all", "Print all with profile…", "Open Pluryprint…".

### Deliverables

| Item | Acceptance |
|---|---|
| Right-click on 30 mixed files, click "Print all" | All 30 PDFs print successfully; non-PDFs are listed as skipped (multi-format is M3). |
| Submenu appears in Win 11 File Explorer (modern UI) | Verified on clean Win 11 23H2 VM. |
| Submenu appears in Win 10 File Explorer | Verified on clean Win 10 22H2 VM. |
| Installer + uninstaller round-trip | After install → use → uninstall, no Pluryprint files or registry entries remain (audited with Sysinternals Process Monitor). |
| `0.1.0-beta` public release | Signed with self-signed cert (warnings expected); SmartScreen reputation N/A yet. |

### Exit criteria

- Shell extension passes Microsoft's [Shell Extension Best Practices](https://learn.microsoft.com/windows/win32/shell/shell-extension-handlers) audit (no UI on background threads, no `MessageBox` from handler, no blocking I/O > 50 ms).
- Installer is reproducible: building twice from the same commit produces byte-identical `.exe`.
- Uninstaller deregisters MSIX, removes all registry entries, deletes install directory and `%LOCALAPPDATA%\Maecly\Pluryprint`.

### Risks

| Risk | Mitigation |
|---|---|
| Sparse MSIX requires identity that conflicts with non-MSIX install on some Win 10 builds | Detect Win version at install time; only deploy sparse MSIX on Win 11. |
| File Explorer caches shell extension DLLs | Installer kills `explorer.exe` and restarts it (with user consent) or instructs user to sign out. |
| Selection size limit (~15 files) re-imposed by Explorer | Verified workaround via verb-based registration that bypasses `IDataObject` row limits. |

### Dependencies

- M1 complete (the app must actually print before adding a way to invoke it).

---

## M3 — Multi-format (Win)

**Goal:** Office documents, web pages, Markdown, and images print transparently through format-specific adapters.

### Status (2026-10-10) — adapters delivered

Every format in [supported-formats.md](../03-requirements/supported-formats.md) prints. Round 3 replaced the browser and Pandoc routes with the system web view in a worker process ([ADR-0012](../adr/0012-system-web-view-renderer.md)), added SVG/SVGZ as vector pages, decoded text and Markdown in-process, sent RTF through `textutil` on macOS, hardened LibreOffice's profile against linked resources, and made encrypted PDFs ask for their password. Round 6 removed the hidden headless-browser fallback that round 3 had left. GIF is refused by decision (D9). Missing LibreOffice or HEIC codecs pause the job and can be installed from the Print Job screen after consent (D13). Still open: the always-on Settings dependency panel, magic-byte detection, `qpdf` preprocessing, and per-format adapter selection in profiles.

### Status (2026-07-20) — superseded by the entry above

**Dependency detection is done** (`engine::deps`, FR-058): LibreOffice, Chromium and Pandoc are located across bundled directories, `PATH` and the usual per-OS install locations, reported through `app_info`, and surfaced with remediation text on the Print Job screen — but only for the dependencies that actually gate a file the user added. The always-on Settings panel in the M3 deliverables table is still owed. ImageMagick is not probed, because nothing uses it yet.

**The adapters are done** (FR-052..FR-057, pulled forward from M3): `engine::convert::{office, html, markdown, heif}` convert Office, OpenDocument, TXT, RTF (LibreOffice), HTML (Edge, Chrome or Chromium headless), Markdown (Pandoc → HTML → browser) and HEIC (`sips` / WIC). A missing converter pauses the job with FR-058 remediation. See [supported-formats.md](../03-requirements/supported-formats.md). Still open from the scope below: magic-byte detection, ImageMagick resampling (not planned: the image path is pure Rust), `qpdf` preprocessing, the always-on dependency panel, and per-format adapter selection in profiles.

### Scope

- `engine::router` extended with format detection (extension + magic bytes).
- Adapters:
  - `engine::office` — LibreOffice headless invocation (`soffice --headless --convert-to pdf`).
  - `engine::html` — Chromium headless via `chrome --headless --print-to-pdf` (or Edge WebView2 fallback).
  - `engine::markdown` — Pandoc subprocess.
  - `engine::image` — ImageMagick `convert` for resampling, then Windows GDI+ direct print (no PDF round-trip for single images).
- Optional preprocessing: `qpdf` for PDF normalization (linearize, decrypt with empty password if applicable).
- Dependency detection at install/runtime: "LibreOffice not found, Office files will be skipped" warning.
- Profile field: per-format adapter selection (e.g. force Pandoc instead of LibreOffice for `.docx`).

### Deliverables

| Item | Acceptance |
|---|---|
| Print mixed batch: PDF, DOCX, XLSX, PNG, MD, HTML | Each prints with correct page count and orientation. |
| Missing dependencies report | UI shows green/yellow/red status per adapter on Settings page. |
| `0.2.0` public release | Signed with OV cert (SmartScreen warnings persist until reputation accumulates). |

### Exit criteria

- Integration test matrix covers PDF, DOCX, XLSX, PPTX, ODT, PNG, JPG, MD, HTML, TXT — all printing successfully to "Microsoft Print to PDF" in CI.
- Adapter timeouts enforced (default 60s/file) with clean cancellation.
- No adapter spawns more than one subprocess concurrently per file (resource discipline).

### Risks

| Risk | Mitigation |
|---|---|
| LibreOffice headless instability when invoked rapidly | Serialize Office conversions; use single LO instance with `--accept` socket. |
| Chromium version drift breaking `--print-to-pdf` flags | Moot: no browser is invoked since round 6; pages print with the system web view (ADR-0012). |
| Pandoc engine variance (LaTeX vs wkhtmltopdf) | Default to `--pdf-engine=wkhtmltopdf`; document switch in profiles. |

### Dependencies

- M2 complete (multi-format on the CLI is unhelpful without the right-click entry point).

---

## M4 — Auto-update + Polish (Win)

**Goal:** A signed, reputation-bearing 1.0.0 that updates itself silently, supports English and Spanish, and is ready for general public consumption.

### Status (2026-10-10) — delivered except the certificates

Signed in-app updates (FR-082/083/084/087), stable and beta manifests (FR-085; nightly follows beta), the release workflow that builds, signs, verifies and drafts the four targets with checksums and sources, and backend reasons translated in the UI (FR-092) are all in place. By decision ([ADR-0011](../adr/0011-release-channel.md)) there is no EV certificate and no Developer ID: SmartScreen and Gatekeeper warn, and the release notes explain both. 0.0.1 preview was published on 2026-10-10 and the repository is public. Still open: no `axe-core` gate, no silent background install (installs happen only when the user asks).

### Status (2026-07-20) — superseded by the entry above

The **i18n scope below is already delivered** (in M1): EN + ES catalogues externalized per screen, OS-locale detection on first run, a Settings selector. Two caveats against the scope text — it is a small in-house translator, not `react-i18next`, and there is no `fluent-rs`, so **backend strings are not translated**. `i18next-parser` is not in the toolchain either, so translation completeness is not machine-checked.

Everything else in M4 is untouched: no download-verify-relaunch path, no updater plugin dependency, no manifest host, no EV certificate, no per-channel feeds, no `axe-core` gate.

### Scope

- Tauri updater wired up: 24h periodic check, on-demand "Check for updates" in Settings.
- Update manifest server: static JSON at `updates.maecly.com/pluryprint/{channel}.json` (S3 or Cloudflare R2 origin, CloudFront/CF in front).
- EV code signing certificate procured and integrated into CI (key in HSM-backed GitHub Actions runner or Azure Key Vault).
- SmartScreen reputation seed: submit installer to Microsoft Defender SmartScreen analytics endpoint via Partner Center.
- i18n: full English + Spanish translations using `react-i18next` for frontend and `fluent-rs` for backend strings.
- Polish: empty states, error toasts, progress animations, accessibility audit (keyboard nav, screen reader labels, color contrast WCAG AA).
- Auto-updater install path completing the About screen's **check** (shipped in M1): download + verify + relaunch, and beta/nightly channel feeds. The About page itself (version, license, credits, update *check*) already landed in M1; M4 adds the installer and i18n/polish.

### Deliverables

| Item | Acceptance |
|---|---|
| In-app update from `0.9.x` → `1.0.0` succeeds | No user prompts beyond initial UAC for the new binary; old binary cleaned up. |
| EV signature on all installer + updater binaries | `signtool verify /pa /v setup.exe` passes; SmartScreen shows no warning on download. |
| Spanish UI verified end-to-end | Native Spanish reviewer signs off; no truncated strings; date/number formats locale-aware. |
| `1.0.0` GA release | Public announcement, GitHub Release with binaries, update manifest live. |

### Exit criteria

- Auto-update e2e test passes in CI (download → verify signature → replace binary → restart → version check).
- Translation completeness: 100% of user-facing strings in both `en.json` and `es.json`, validated by `i18next-parser`.
- Accessibility: zero `axe-core` violations on every page; keyboard reachability tested manually for all primary flows.

### Risks

| Risk | Mitigation |
|---|---|
| EV cert procurement delay (often 2-4 weeks for vetting) | Order cert at start of M3; have OV fallback in hand to keep cadence. |
| SmartScreen still warns despite EV (rare but happens) | Submit installer to MS for manual review via Partner Center; can take 1-3 weeks. |
| Translation review gates the release | Recruit Spanish reviewer at M3 start; budget 1 week for review + revision cycle. |

### Dependencies

- M3 complete.
- EV certificate purchased and provisioned (~$300/year, 1-3 week procurement).
- DNS for `updates.maecly.com` live with TLS certificate (Let's Encrypt or CDN-managed).

---

## M5 — macOS Port

**Goal:** A notarized, signed macOS application with a Finder Sync Extension or Quick Action that mirrors the Windows shell integration UX.

### Status (2026-10-10) — shipping without notarization

`release.yml` builds `aarch64-apple-darwin` and `x86_64-apple-darwin` as separate, ad-hoc-signed `.dmg` downloads with signed updater archives; HTML, Markdown and text print through WKWebView in a worker, and RTF through `textutil`. By decision there is no Developer ID, no notarization and no universal binary (ADR-0011); the release notes give the Gatekeeper step. The Finder entry is the `NSServices` provider described below.

### Status (2026-07-20) — superseded by the entry above

Delivered early, alongside M1/M2:

- **CUPS printing works.** `engine::print::cups` spools with `lp` and cancels with `cancel`, and `platform::cups` enumerates printers. It is exercised in CI on macOS runners, not merely written.
- **The Finder entry works, and it is not a Finder Sync Extension.** The scope line below offered Finder Sync or a Quick Action; the implementation took a third route — an `NSServices` provider answered inside the app process (`src-tauri/Info.plist` + `platform/macos/services.rs`). No `.appex`, no App Group, no sandbox, no second code signature, nothing for the user to enable. **`shell-ext-mac/` contains a README and no Swift project.** The "evaluate Finder Sync vs Quick Action" spike in the scope below is therefore closed by a decision neither option won; the scope text should be rewritten to say `NSServices`, and the choice deserves an ADR since ADR-0005 covers only the Windows side.

Untouched: `.pkg` / `.dmg` packaging, Developer ID signing, notarization, stapling, the universal binary, and adapter parity (which depends on M3 existing at all).

### Scope

- New crate target: `printer::macos` using `lpr` / `lp` (CUPS) via `Command`. *Delivered as `engine::print::cups` + `platform::cups`, using `lp`.*
- Adapter parity: LibreOffice (cask install), `cups-pdf` for HTML print, Pandoc, ImageMagick — all via Homebrew or bundled.
- `shell-ext-mac/` Swift project: Finder Sync Extension targeting macOS 13+ (Apple Silicon and Intel via Universal binary).
- Alternative path evaluated: macOS Quick Action (Automator-based) as fallback if Finder Sync Extension proves too constrained.
- Installer: `pkgbuild` + `productbuild` for `.pkg`, `create-dmg` for `.dmg` with drag-to-Applications layout.
- Codesign with Apple Developer ID Application certificate.
- Notarization with `xcrun notarytool` and stapling with `xcrun stapler`.
- Universal binary built with `lipo` from `aarch64-apple-darwin` and `x86_64-apple-darwin` slices.
- Auto-updater extended: Tauri updater already supports darwin-aarch64 and darwin-x86_64 platform keys.

### Deliverables

| Item | Acceptance |
|---|---|
| `Pluryprint.app` runs on macOS 13 Ventura, 14 Sonoma, 15 Sequoia | First-launch Gatekeeper allows execution without right-click bypass. |
| Right-click in Finder → "Print all with Pluryprint" | Selection passed to app, prints via CUPS. |
| `.pkg` and `.dmg` both notarized and stapled | `spctl --assess --verbose=4` reports "accepted source=Notarized Developer ID". |
| `1.1.0` macOS GA release | GitHub Release includes Win + Mac assets; update manifest covers both. |

### Exit criteria

- Notarization passes on every release build (no warnings, all entitlements minimal).
- Finder Sync Extension survives Finder restart and macOS update without re-registration.
- All Rust + frontend tests pass on `macos-14` and `macos-13` runners.

### Risks

| Risk | Mitigation |
|---|---|
| Apple notarization failures (entitlement issues, hardened runtime exceptions) | Prototype the codesign + notarize pipeline at M5 week 1 with a stub binary; do not wait until release. |
| Finder Sync Extension developer experience is poor and undocumented in places | Spike Quick Action approach in parallel; pick the better one by week 3 of M5. |
| CUPS subtleties (printer-specific PPDs) | Limit M5 launch to "Generic PostScript" and "Generic PCL" PPD families; document known-good printers; defer exotic models. |
| Universal binary inflates download size | Acceptable for v1.1; investigate per-arch downloads for v1.2. |

### Dependencies

- M4 complete (auto-updater infrastructure must exist before macOS uses it).
- Apple Developer Program membership active ($99/year).
- Access to one Apple Silicon and one Intel Mac for local QA (CI handles both).

## M6 — Linux Port

**Goal:** Linux x86_64 and ARM64 builds with the same formats, queue, profiles, CLI and updates as Windows and macOS, released with 0.0.5 and labelled **preview** until they have run on real hardware ([ADR-0015](../adr/0015-linux-support.md), which supersedes decision D20 and the "Linux (M6+)" note of ADR-0007).

### Status (2026-11-09) — decided, in progress

The CUPS backend is shared with macOS and already compiled for Linux. The work for 0.0.5 is packaging, two converters (the render worker and HEIC), the memory cap and the file-manager glue.

### Scope

- **Packages:** an AppImage and a `.deb` per architecture (`Pluryprint_<version>_linux_x64.AppImage`, `…_linux_x64.deb`, `…_linux_arm64.AppImage`, `…_linux_arm64.deb`), built on `ubuntu-22.04` and `ubuntu-22.04-arm`. No `.rpm`, no repository, Flatpak or Snap.
- **Minimum:** Ubuntu 22.04, Debian 12 or Fedora 38, or later (glibc 2.35, WebKitGTK 4.1 2.42 or later), with CUPS; X11 or Wayland.
- **Format parity:** a WebKitGTK backend for the render worker (HTML, Markdown, text); HEIC/HEIF through `heif-dec` / `heif-convert` or ImageMagick, found at run time and reported as a dependency; RTF through LibreOffice.
- **Shell integration:** "Open with" through the `.desktop` file's `MimeType=`; Dolphin and Nemo entries (**Print now**, **Configure print job**, **Add to print queue**) installed by the `.deb`.
- **Updates:** the AppImage updates itself (`linux-x86_64`, `linux-aarch64`); a `.deb` install updates in the app through `pkexec dpkg -i` (`linux-x86_64-deb`, `linux-aarch64-deb`).
- **Engine:** process-group memory accounting from `/proc`, so the memory cap covers a capped tool's children; process groups signalled without the ambiguous `kill -<sig> -<pgid>` form.
- **CI:** `ubuntu-22.04` in the pull-request gate (fmt, clippy, `--lib`); `linux.yml` with the full suite on `ubuntu-22.04-arm`, Xvfb and a `cups-pdf` queue (push to `main`, weekly, by hand).
- **Licences:** the AppImage's GPL and LGPL libraries, and the runtime's static libfuse, listed with a Corresponding Source pointer and a written offer.

### Exit criteria

- `release.yml` publishes the eight Linux files and the four Linux manifest keys, signed, and refuses a release without them.
- `linux.yml` is green on ARM64, and the pull-request gate is green on `ubuntu-22.04`.
- **To drop the "preview" label:** a manual pass on real hardware per architecture (install, print to `cups-pdf` and to a real printer, add a printer, "Open with", the Dolphin and Nemo entries, the CLI, HTML and HEIC, an in-app update from the previous Linux release) on Ubuntu 22.04 and 24.04, Debian 12 and Fedora.

### Risks

| Risk | Mitigation |
|---|---|
| The first Linux release has no earlier Linux build to update from | Required manifest keys on every release; the update path is proven by the release after it. |
| WebKitGTK traps: AppArmor and the `bubblewrap` sandbox inside an AppImage (Ubuntu 24.04), NVIDIA on Wayland, the Wayland `app_id`, 16K-page kernels | Test each on real hardware before dropping "preview"; document workarounds. |
| AppImage needs `libfuse2` | Say so in the release notes and on the landing page. |
| The `ubuntu-22.04` runner image is retired | Move the baseline (and raise the stated minimum) when GitHub announces it. |

### Dependencies

- M5's CUPS backend.
- A Linux machine or VM per architecture for the manual pass.

---

## What is explicitly out of scope (pre-1.0)

These are deferred to post-1.0 to keep the milestone path tight:

- Cloud printing / IPP-over-HTTPS (would expand attack surface; not justified yet).
- Plugin/extension API for third-party adapters.
- Per-page settings within a single document (we only do per-job and per-profile).
- OCR of scanned PDFs.
- Batch resizing/rotating beyond what ImageMagick adapter does for image inputs.
- Cloud sync of profiles (privacy posture is "no telemetry"; opt-in cloud sync would be M6+).

If a feature request matches one of the above, label it `out-of-scope/pre-1.0` and link to this section.
