# Pluryprint

> **Open source batch file printing for Windows and macOS.** Right-click → print everything, with no 15-file limit, no Adobe required, and no $149 license fee.

[![Status](https://img.shields.io/badge/status-pre--alpha-orange)](./docs/06-roadmap/milestones.md)
[![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-blue)](./LICENSE)
[![Stack](https://img.shields.io/badge/stack-Tauri%202%20%2B%20Rust%20%2B%20React-success)](./docs/04-architecture/tech-stack.md)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11%20%E2%86%92%20macOS-lightgrey)](./docs/06-roadmap/milestones.md)
[![Release](https://img.shields.io/github/v/release/MAECLY/pluryprint)](https://github.com/MAECLY/pluryprint/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/MAECLY/pluryprint/total)](https://github.com/MAECLY/pluryprint/releases)
[![Stars](https://img.shields.io/github/stars/MAECLY/pluryprint)](https://github.com/MAECLY/pluryprint/stargazers)
[![Views](https://hits.sh/github.com/MAECLY/pluryprint.svg?label=views)](https://hits.sh/github.com/MAECLY/pluryprint/)
[![Last commit](https://img.shields.io/github/last-commit/MAECLY/pluryprint)](https://github.com/MAECLY/pluryprint/commits/main)

---

## The problem

Windows hides the **Print** verb from the right-click menu when you select more than ~15 files. Adobe Acrobat doesn't help: it has the same effective limit and is heavyweight. The commercial alternatives that fix this — Print Conductor ($149), 2Printer ($149), Batch & Print Pro ($79.95) — are closed-source and pricey. The few open-source options are abandoned (last release in 2015), depend on Adobe Reader, or are virtual printers, not real batch printers.

So most people end up printing in tedious batches of 15-20 files, manually, again and again.

## What Pluryprint does

Pluryprint adds an entry to the Explorer right-click menu (on Windows 11, under **Show more options**) and to the macOS Finder's **Services** menu:

```
Right-click on any number of files → Pluryprint ▸
                                       ├─ Print 12 files          (default profile)
                                       ├─ ──────────
                                       ├─ Print with “Duplex BW”  (one row per profile, up to 8)
                                       ├─ Print with “Photo Color”
                                       ├─ ──────────
                                       ├─ Configure print job…
                                       └─ Add to print queue
```

- Select **5, 50, or 500** files. They print.
- Use **profiles** (built-in or custom) to lock in your favorite settings: duplex BW, photo color, A3 simplex, whatever.
- **PDF** (encrypted ones too: it asks for the password and never stores it), **images** (JPG, PNG, TIFF, BMP, WebP, HEIC), **SVG**, **Word, Excel, PowerPoint and OpenDocument** (through LibreOffice), **RTF, TXT, HTML and Markdown**. GIF is not supported and is refused with a clear message.
- All **local**: no cloud, no telemetry, no account. A printed page never reaches the network.

The entry sits under **Show more options** on Windows 11 because a top-level entry needs a signed package, and Pluryprint ships without code-signing certificates for now ([ADR-0013](./docs/adr/0013-explorer-command-per-user-verb.md)).

## Download

**[Pluryprint 0.0.1 preview](https://github.com/MAECLY/pluryprint/releases/tag/v0.0.1)** (pre-alpha, published 2026-10-10). Get the [latest release](https://github.com/MAECLY/pluryprint/releases/latest), or the right file from the [landing page](https://pluryprint.maecly.com/en/) ([en español](https://pluryprint.maecly.com/)):

| Computer                                                  | File                                                                                                                                     |
| --------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| Windows, Intel or AMD (x64)                               | [`Pluryprint_0.0.1_windows_x64.exe`](https://github.com/MAECLY/pluryprint/releases/download/v0.0.1/Pluryprint_0.0.1_windows_x64.exe)     |
| Windows on ARM (ARM64; the x64 file also works, emulated) | [`Pluryprint_0.0.1_windows_arm64.exe`](https://github.com/MAECLY/pluryprint/releases/download/v0.0.1/Pluryprint_0.0.1_windows_arm64.exe) |
| Mac with Apple silicon                                    | [`Pluryprint_0.0.1_apple_silicon.dmg`](https://github.com/MAECLY/pluryprint/releases/download/v0.0.1/Pluryprint_0.0.1_apple_silicon.dmg) |
| Mac with an Intel processor (tested only under Rosetta 2) | [`Pluryprint_0.0.1_apple_intel.dmg`](https://github.com/MAECLY/pluryprint/releases/download/v0.0.1/Pluryprint_0.0.1_apple_intel.dmg)     |

Check the file against [`SHA256SUMS`](https://github.com/MAECLY/pluryprint/releases/download/v0.0.1/SHA256SUMS) before opening it. Nothing is signed with an Apple or Microsoft certificate, so Windows SmartScreen (**More info → Run anyway**) and macOS Gatekeeper (**System Settings → Privacy & Security → Open Anyway**) warn the first time; the [release notes](https://github.com/MAECLY/pluryprint/releases/tag/v0.0.1) explain both, and what to do if Smart App Control blocks the installer.

## Status

**Pre-alpha.** Version 0.0.1 preview is published (see [Download](#download)) and the repository is public. What works today:

- **Printing**, end to end, of every format above: PDFs pass through untouched; images, SVG, Markdown and text are converted inside the app; HTML, Markdown and text are paginated by the system's own web view (WebView2 / WebKit) in a sandboxed worker; Office and OpenDocument go through LibreOffice. Everything is spooled through the bundled SumatraPDF on Windows or `lp` (CUPS) on macOS. A missing LibreOffice (or, on Windows, the HEIC codecs) pauses the job, and the app offers to install it after asking.
- **A persistent print queue** (`queue.json`): bounded concurrency, automatic retries with backoff, per-job and whole-queue pause/resume/cancel, crash recovery, and jobs followed in the OS queue until they really print (or end as _unconfirmed_).
- **Profiles and settings on disk**: `profiles.toml` and `config.toml`, written atomically, plus profile import/export.
- **Seven GUI screens** (Home, Print Job, Queue, Printers, Profiles, Settings, About) wired to the real backend, with keyboard shortcuts and the whole UI in English and Spanish.
- **A headless CLI**: `pluryprint print|printers|profiles|queue|update`, `--help` everywhere, `--json` output, and documented exit codes (on Windows, `pluryprint-cli.exe`).
- **A per-user Windows installer** (x64 and ARM64, English and Spanish, no administrator rights) that registers the Explorer entry, and an ad-hoc-signed `.dmg` per Mac architecture.
- **Updates**: a check at start and from About, install in place with signature verification, stable and beta channels, defer / skip-this-version.

**What is NOT here yet:**

- **No stable release.** 0.0.1 is a pre-alpha preview, built and signed by the release workflow ([releasing.md](./docs/07-build-and-release/releasing.md)).
- **No code-signing certificates**, by decision ([ADR-0011](./docs/adr/0011-release-channel.md)): expect SmartScreen and Gatekeeper warnings; the release notes say how to get past them. Updates are signed with Pluryprint's own key.
- **No top-level Windows 11 menu entry** (see above), and no nightly builds.

See the [roadmap](./docs/06-roadmap/milestones.md) for the milestone path, and the [functional requirements](./docs/03-requirements/functional-requirements.md) for per-requirement status.

## Quick start (developers)

```powershell
# Prerequisites (one-time):
#   1. Node 24 LTS (22.22.2+) + pnpm 9 -> https://nodejs.org, then `npm i -g pnpm`
#   2. Rust stable 1.78+       -> https://rustup.rs
#   3. Visual Studio Build Tools 2022 with the "Desktop development with C++"
#      workload (incl. Windows 11 SDK + ATL + Spectre Mitigations)
#   4. WebView2 Runtime (preinstalled on Windows 11)
# Detailed checklist in docs/08-development/local-setup.md.

git clone https://github.com/MAECLY/pluryprint.git
cd pluryprint
pnpm install

# Web preview only (no Tauri host, no IPC):
pnpm dev

# Full Tauri dev loop (requires Rust toolchain):
pnpm tauri dev
```

The web preview renders every screen without the Rust toolchain, but it has no Tauri host: IPC calls fail, so the screens show their empty and error states rather than real printers, jobs or profiles. Use it for layout and styling work only. `pnpm tauri dev` boots the real app against the real backend.

The Explorer context-menu DLL is a separate crate with its own target directory — build and test it with `cargo build --manifest-path shell-ext-win/Cargo.toml` (Windows only; see [`shell-ext-win/README.md`](./shell-ext-win/README.md)). An installer build registers it; [installer-windows.md](./docs/07-build-and-release/installer-windows.md) has the three packaging steps.

## Documentation

All documentation lives under [`docs/`](./docs/README.md). Recommended starting points:

- [Product Vision](./docs/01-vision/product-vision.md)
- [Value Proposition](./docs/01-vision/value-proposition.md)
- [System Architecture](./docs/04-architecture/system-architecture.md)
- [MVP Features](./docs/05-features/mvp-features.md)
- [Roadmap & Milestones](./docs/06-roadmap/milestones.md)
- [Architecture Decision Records](./docs/adr/README.md)

## Stack

| Layer                            | Technology                                                                                                  | State                          |
| -------------------------------- | ----------------------------------------------------------------------------------------------------------- | ------------------------------ |
| Application framework            | [Tauri 2](https://tauri.app)                                                                                | In use                         |
| Backend                          | Rust                                                                                                        | In use                         |
| Frontend                         | React + TypeScript + Vite + Tailwind CSS                                                                    | In use                         |
| Print engine (Win)               | [SumatraPDF](https://www.sumatrapdfreader.org/) 3.6.1 (CLI), bundled                                        | In use                         |
| Print engine (macOS)             | CUPS (`lp`)                                                                                                 | In use                         |
| Image, SVG, Markdown, text → PDF | Rust `image` + `lopdf`, `usvg` + `svg2pdf`, `pulldown-cmark` (no external binary)                           | In use                         |
| HTML, Markdown, text pagination  | The system web view: WebView2 (Windows), WebKit (macOS), in a worker process                                | In use                         |
| Office / ODF / RTF               | [LibreOffice](https://www.libreoffice.org/) (headless), installed by the user; RTF on macOS uses `textutil` | In use                         |
| Shell extension (Win)            | Rust COM DLL implementing `IExplorerCommand`, registered per user                                           | In use; sparse package planned |
| Finder integration               | `NSServices` provider inside the app (no separate `.appex`)                                                 | In use                         |
| Installer (Win)                  | Tauri's per-user NSIS bundle                                                                                | In use                         |
| Updates                          | Tauri Updater plugin, minisign-signed `latest.json` per channel on GitHub Releases                          | In use                         |

Justification in [tech-stack.md](./docs/04-architecture/tech-stack.md) and the [ADRs](./docs/adr/README.md).

## License

Pluryprint is free software under the [GNU Affero General Public License v3.0](./LICENSE).
Contributions are accepted under the [CLA](./CLA.md). The name, the wordmark and
the mark belong to MAECLY and are not covered by that licence; see
[TRADEMARKS.md](./TRADEMARKS.md). Bundled third-party licences:
[THIRD-PARTY-NOTICES.md](./THIRD-PARTY-NOTICES.md). For a commercial licence, write
to hola@maecly.com. What the licence means in practice:
[docs/09-legal/license.md](./docs/09-legal/license.md).

## Contributing

Contributions are welcome — read [CONTRIBUTING.md](./CONTRIBUTING.md) and agree to the
[Contributor License Agreement](./CLA.md); you keep the copyright in what you write.
The full guide is in [docs/08-development/contributing.md](./docs/08-development/contributing.md). Before opening a PR for non-trivial changes, please file an issue or start a discussion.

If you want to contribute a translation, see the [internationalization section](./docs/08-development/contributing.md#translations).

## Project name and trademark

"Pluryprint", its wordmark and the Abanico mark belong to the founder (MAECLY). Forks must rebrand: see [TRADEMARKS.md](./TRADEMARKS.md) and the [trademark policy](./docs/09-legal/trademark.md).

## Privacy

No telemetry by default. No cloud. Your files never leave your machine. See [privacy](./docs/09-legal/privacy.md).

## Acknowledgements

Pluryprint stands on the shoulders of these excellent open-source projects:

- [Tauri](https://tauri.app), [Rust](https://www.rust-lang.org/), [React](https://react.dev/)
- [SumatraPDF](https://www.sumatrapdfreader.org/) for fast, reliable PDF printing on Windows
- [LibreOffice](https://www.libreoffice.org/) for Office and ODF conversion
- [resvg/usvg](https://github.com/linebender/resvg), [svg2pdf](https://github.com/typst/svg2pdf), [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark), [lopdf](https://github.com/J-F-Liu/lopdf) and the [image](https://github.com/image-rs/image) crate

Full list and licenses in [third-party-licenses.md](./docs/09-legal/third-party-licenses.md).
