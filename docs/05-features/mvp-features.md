# MVP Features (v0.1.0)

> **TL;DR** — The minimum cut of Pluryprint that is genuinely useful: right-click → print PDFs and common images, with a small set of profiles, on Windows 10/11. No Office, no cloud, no fancy automation.

The MVP exists to **prove the core thesis**: that a free, open-source tool can integrate cleanly into the Windows context menu and print N files in batch without the friction of Adobe Reader or the price of Print Conductor. Everything else can wait.

> **Scope vs. status.** This page defines the v0.1.0 **scope**. Sections that describe something not yet built say so inline. The authoritative per-milestone status is [milestones.md](../06-roadmap/milestones.md); the authoritative per-requirement status is the `Status` column of [functional-requirements.md](../03-requirements/functional-requirements.md).

## In scope

### 1. Native Windows 11 context menu integration

- **Submenu** appears in the **modern** context menu (not relegated to "Show more options").
- Items, in order:
  - **Print N files** — the default profile, silent, fastest path
  - **Print with "<profile>"** — one row per profile, up to a bounded number
  - **Configure print job…** — opens the GUI with the selection pre-loaded
  - **Add to print queue** — enqueues without opening the composer
- Works for selections of any size (no 15-file limit).
- Works for files, folders (with recursion prompt), and mixed selections.
- Filters unsupported file types automatically and lists what was skipped.

Implementation: a COM DLL implementing `IExplorerCommand`, written in Rust rather than C++, registered per user as a registry verb by the installer. Details in [shell-extensions.md](../04-architecture/shell-extensions.md) and [ADR-0013](../adr/0013-explorer-command-per-user-verb.md), which supersedes the Sparse MSIX plan of [ADR-0005](../adr/0005-shell-extension-sparse-msix.md).

**Status:** the DLL is implemented and tested in `shell-ext-win/`, and the app accepts the handoff (`--invoke=context-menu --files-from <PATH>`). The macOS counterpart is an `NSServices` provider declared in `src-tauri/Info.plist`. The per-profile rows and the translated labels come from `<data_local_dir>\shell-menu.json`, which the app writes at startup and on every profile or language change (`src-tauri/src/shell_menu.rs`). The per-user installer registers it, with the DLL of the machine's native architecture, and it was verified with real right-clicks on Windows 11 24H2 ARM64. One thing is missing:

- **The Windows 11 top-level menu.** The entry is in the Windows 10 menu and, on Windows 11, under **Show more options**. The top-level menu needs a sparse package signed with a trusted certificate, which waits for a certificate (decision D3).

### 2. Format support

| Format | Engine | Notes |
|---|---|---|
| **PDF** (`.pdf`) | Passthrough → SumatraPDF (Win) / `lp` (macOS, Linux) | Native, no conversion |
| **JPG/JPEG, PNG** | Rust `image` → one-page PDF (`lopdf`) → spooler | Standard image batch |
| **TIFF, BMP, WebP** | Rust `image` → one-page PDF (`lopdf`) → spooler | Less common but cheap to include |
| **HEIC/HEIF** | OS decoder (`sips` on macOS, WIC on Windows; on Linux libheif's tools or ImageMagick) → one-page PDF | Windows needs the Store *HEIF Image Extensions* (and *HEVC Video Extensions* for `.heic`), Linux one of its decoders. Without them the job pauses and offers the install (on Linux, a web link) |
| **SVG, SVGZ** | `usvg` + `svg2pdf` → one vector page, in a worker process | Nothing to install |
| **DOCX, XLSX, PPTX, DOC, XLS, PPT, ODT, ODS, ODP** (and RTF on Windows and Linux) | LibreOffice headless → PDF | Needs LibreOffice. Pulled forward from v0.2 |
| **HTML** | The system web view (WebView2 / WKWebView / WebKitGTK) in a worker process | Nothing to install. Pulled forward from v0.2 |
| **Markdown, TXT** (and RTF on macOS) | In-process to HTML (`textutil` for RTF), then the HTML row | Nothing to install. Pulled forward from v0.2 |
| **GIF** | — | Refused with its own message (decision D9) |

The image path is **pure Rust**: the `image` crate decodes, and `lopdf` embeds the bitmap as an XObject in a one-page PDF. There is no ImageMagick dependency. Office and OpenDocument files need LibreOffice. A missing one pauses the job with an install hint and an **Install…** button that runs the shown command only after the user confirms (FR-058), instead of skipping the file. Encrypted PDFs ask for their password (FR-059). The engine's supported-extension list is `SUPPORTED_EXTS` in `src-tauri/src/engine/mod.rs`. See [supported-formats.md](../03-requirements/supported-formats.md) for the full matrix and how each converter is run.

### 3. Print settings

The user can configure (per profile or per job override):

- **Printer** (system default or any installed printer)
- **Duplex**: none / long-edge / short-edge
- **Color mode**: color / monochrome
- **Copies**: 1–999
- **Page ranges**: e.g., `1-5,10` (PDFs and converted documents; images are single-page)
- **Paper size**: A4 / Letter / Legal / A3
- **Orientation**: portrait / landscape / auto
- **Scaling**: fit / shrink / none

Settings not exposed in MVP: n-up, collate, watermark, separator pages, custom paper, margins. These come in v0.2 / v1.

A profile also carries `tray` and `quality`. Both reach the printer: CUPS gets them as options; on Windows the tray goes to SumatraPDF as the driver's bin id and Draft/High through a temporary per-user default (decision D16). A tray or quality the driver does not offer is noted on the job instead of applied. See [print-settings.md](../03-requirements/print-settings.md).

### 4. Profiles

- 3 built-in (immutable, can be cloned):
  - **Quick** — system default printer, color, simplex, fit, A4, portrait
  - **Duplex BW** — system default printer, monochrome, long-edge, fit, A4, portrait
  - **Photo Color** — system default printer, color, simplex, none scaling, A4, portrait, high quality
- User can create unlimited custom profiles.
- One profile is marked **Default** for context menu invocations.
- Profiles persist in `%APPDATA%\Maecly\Pluryprint\config\profiles.toml` (atomic writes).

### 5. GUI

A single window built with React + Tailwind, embedded via Tauri. Seven pages, in sidebar order:

- **Home** — drag-drop area, quick actions, recent activity
- **Print Job** — pre-loaded with the selection, settings panel on the right, resolved-command preview, "Start printing" button
- **Queue** — current and recent jobs with status (pending / converting / spooling / printed / failed), per-job pause/resume/cancel, retry, whole-queue pause
- **Printers** — enumerated printers with status and capabilities, set-default, test page, add by IP / discovery
- **Profiles** — CRUD list of profiles with an editor, clone, delete-with-undo, set-default, import/export
- **Settings** — appearance, language, engine limits, updates, privacy
- **About** — version, license, components, paths, "Check for updates"

GUI MUST start in <500 ms cold from a context menu invocation.

### 6. CLI

The same binary, headless mode:

```
pluryprint print --files <list> [--profile <name>] [--printer <name>] [...overrides]
pluryprint printers list
pluryprint profiles list
pluryprint queue status
pluryprint update check
```

`--help` for every subcommand. Stable exit codes (documented in [ipc-and-cli.md](../04-architecture/ipc-and-cli.md)).

Delivered, and slightly wider than this list: the five subcommand roots are `print`, `printers`, `profiles`, `queue` and `update`, each with the sub-verbs the CLI contract names (`profiles list|show|create|…`, `queue status|…`). `--json` emits machine-readable output on stdout with errors as JSON on stderr, and works as a global flag. Two extra entry points exist for the shell handlers: `--invoke=context-menu` and `--files-from <PATH>` (argv on Windows is capped at 32,767 characters, so a large selection travels in a handoff file). A headless run creates no window; a second launch forwards its selection to the first through the single-instance plugin.

`profiles import` / `profiles export` are **not** in the CLI — import/export exists only over IPC and in the Profiles screen.

### 7. Queue & lifecycle

- Persistent queue (`queue.json`) survives crashes and restarts.
- Concurrent **conversions**: configurable 1–8 (default 2), clamped in both the IPC command and the settings loader, and read into the dispatcher at app start. Spooling itself is sequential.
- Automatic retry: the initial attempt plus 3 retries with exponential backoff (1s, 5s, 25s) before marking failed.
- Structured JSONL logs in `%LOCALAPPDATA%\Maecly\Pluryprint\data\logs\`, size-capped with rotation and retention.

### 8. Notifications

- **In-app toast** on batch complete and on batch failure, raised from the queue store when every job of a batch reaches a terminal state. A failed batch reports the error and offers the logs folder.
- **A job that stops for the user** (printer gone, SumatraPDF missing, a setting the driver refuses) gets its own toast with an **Open queue** action — its batch cannot complete until it is fixed.
- **Native OS notifications** (Windows toast / macOS Notification Center) for the same three events, only while the window is in the background — see [FR-072](../03-requirements/functional-requirements.md). **Settings → Native notifications** turns them off; per-profile mute stays post-MVP.

### 9. Auto-update (basic)

Delivered:

- On startup (delayed 30 s) and at most once per 24 h, check for a newer release. The check runs in Rust on a detached thread, so a cold start never waits on the network. It reads the updater manifest (`latest.json`) of the user's channel from GitHub Releases, the same file the installer uses; there is no `updates.maecly.com`.
- Manual "Check for updates now" from About.
- If a newer version is available: an in-app toast (with the release notes) and an About-screen card offering **Install now**, **defer** (say nothing until the next check window) or **skip this version** (silence exactly that tag; a later release prompts again). The skipped tag is persisted.
- **Install now** downloads the artifact named by `latest.json`, verifies its signature, installs it per-user and offers a restart ([FR-082](../03-requirements/functional-requirements.md), FR-083). A major version needs a second confirmation (FR-087). The CLI equivalent is `pluryprint update install [--yes]`. See [ADR-0008](../adr/0008-installer-nsis-per-user.md).

Not delivered — this is the honest part:

- **No update has shipped yet.** 0.0.1 preview, the first release built by `release.yml` ([releasing.md](../07-build-and-release/releasing.md)), was published on 2026-10-10, so the in-app install path has not yet run against a real newer release.
- Stable and beta read their own manifests; there are no nightly builds, so nightly follows beta. Switching to a lower channel asks for confirmation (FR-086).

### 10. Installer

**Shipped** as Tauri's per-user NSIS bundle ([ADR-0008](../adr/0008-installer-nsis-per-user.md)), not Inno Setup:

- `Pluryprint_<version>_x64-setup.exe` and `…_arm64-setup.exe`, about 15.5 MiB for ARM64 and 17.1 MiB for x64, under the 20 MB budget (NFR-006).
- Installs to `%LOCALAPPDATA%\Pluryprint\` with no administrator rights; English and Spanish.
- Bundles SumatraPDF 3.6.1 for its architecture, with its licence files and written source offer.
- Registers the Explorer command per user (no sparse package, ADR-0013).
- Creates a Start-menu shortcut, and a desktop shortcut from the last page's checkbox.
- **Not code-signed** (ADR-0011): SmartScreen warns, and the release notes explain **More info → Run anyway**. Updates are signed with Pluryprint's own key.

macOS ships an ad-hoc-signed `.dmg` per architecture. See [installer-windows.md](../07-build-and-release/installer-windows.md) and [releasing.md](../07-build-and-release/releasing.md).

### 11. Localization

Planned as **English only**, but **Spanish landed early**. The i18n layer shipped with the screen-completeness work rather than waiting for v0.4: every user-facing string is externalized under `src/i18n/locales/{en,es}/<namespace>.json`, the first run picks the OS language (`navigator.language`, narrowed to a supported locale; on Windows the app and the CLI take the Windows display language before any window opens, so the Explorer menu and the built-in profile names follow it too) and persists it, and Settings has a language selector. Backend errors arrive as a stable reason with parameters and are translated by the UI; the installer and the printed test page are bilingual too.

This is scope pulled forward, not scope added — the v0.4 line in [v1-features.md](./v1-features.md) is marked delivered accordingly.

### 12. Documentation

- README.md, all `docs/` content, CONTRIBUTING.md, CODE_OF_CONDUCT.md, SECURITY.md, LICENSE all present and **kept in sync with the code**. A doc that describes behaviour the code does not have is a defect, not a wish list; status belongs in [milestones.md](../06-roadmap/milestones.md) and the `Status` column of [functional-requirements.md](../03-requirements/functional-requirements.md), and nowhere else.

## Out of scope for MVP (explicitly NOT in v0.1.0)

| Feature | Defer to | Note |
|---|---|---|
| ~~Office (DOCX/XLSX/PPTX)~~ | ~~v0.2~~ | **Shipped early** — LibreOffice headless (FR-053). |
| ~~ODF (ODT/ODS/ODP)~~ | ~~v0.2~~ | **Shipped early** (FR-054). |
| ~~HTML printing~~ | ~~v0.2~~ | **Shipped early** — the system web view in a worker (FR-055). |
| ~~Markdown printing~~ | ~~v0.2~~ | **Shipped early** — in-process to HTML, then the web view (FR-056). |
| ~~TXT/RTF~~ | ~~v0.2~~ | **Shipped early** — text in-process; RTF through `textutil` (macOS) or LibreOffice (Windows) (FR-057). |
| ~~SVG~~ | ~~v2~~ | **Shipped early** — vector PDF in a worker (FR-052a). |
| ~~HEIC/HEIF~~ | ~~v0.2+~~ | **Shipped** — OS decoder (FR-052). |
| ~~Tray selection~~ | ~~v0.2~~ | **Shipped** — sent to CUPS and, as the driver's bin id, to SumatraPDF. The Print Job screen does not override it per job. |
| N-up | v1.0 | |
| Custom paper sizes | v1.0 | |
| Watermark | v1.0 | |
| Separator pages | v1.0 | |
| Profile import/export | ~~v1.0~~ | **Shipped early** — GUI + IPC (`export_profile`, `import_profiles`). Not in the CLI. |
| Profile keyboard shortcuts | v1.0 | App-wide shortcuts exist; per-profile ones do not. |
| Multi-language UI (Spanish) | ~~v1.0~~ | **Shipped early** — EN + ES, OS-locale detection, Settings selector. |
| Update channels (beta/nightly) | v1.0 | Selector and persistence exist; the feeds do not. |
| macOS support | v1.1 (M5) | Partially early: CUPS printing, Finder `NSServices` and the macOS path layout all work; packaging and notarization do not. |
| Hot folder / watch folder | v2.0 | |
| Multi-printer load balancing | v2.0 | |
| Email-to-print | v2.0 | |
| Cloud folder watching | v2.0 | |
| OCR | v2.0 | |
| CAD support | v2.0 | |
| Plugin API | v2.0 | |

## Success criteria for MVP

The MVP is a success if, three months after release:

- A first-time user can install and complete a 50-file batch print in under 5 minutes.
- 95% of jobs in real-world use complete without errors (excluding driver/printer hardware failures).
- Cold context-menu invocation reliably under 500 ms p95.
- Crash rate under 0.5% of jobs (telemetry is opt-in; we measure via opt-in crash reporter and GitHub issues).
- At least 3 external contributors with merged PRs.
- A documentation site (`docs/`) that lets a new contributor set up a dev environment in under 30 minutes.

## What MVP is NOT

- Not a PDF editor.
- Not a virtual printer (we don't pretend to be a printer; we drive real printers).
- Not a cloud service.
- Not an Office replacement.
- Not a print-shop ERP.
- Not a notification daemon for non-print events.

We resist scope creep aggressively. Every feature added before v1.0 must serve the core thesis: **right-click N files, they print correctly, fast, and free.**
