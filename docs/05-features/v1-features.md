# v1 Features (v0.2.0 → v1.0.0)

> **TL;DR** — Post-MVP additions that round Pluryprint into a polished v1: full Office and HTML support, more print settings, multilanguage UI, accessibility compliance, and code-signed updates with channels.

These features are sequenced into intermediate releases (`v0.2`, `v0.3`, …) before `v1.0`. The order may shift based on user feedback after MVP.

> **Already delivered ahead of schedule.** Three items below were built during the M1 screen-completeness work and are marked **Delivered** in place: GUI folder drag-drop with the recursion prompt (v0.2), profile import/export (v0.3), and the English/Spanish i18n layer with OS-locale detection and a Settings selector (v0.4). They stay in this document — moving them would lose the sequencing record — but they are not open work.

## v0.2 — Multi-format

The headline feature of v0.2 is **format breadth**. After v0.2, Pluryprint covers ~95% of the documents an office user needs to print.

### New formats — **delivered early**

All of these shipped ahead of v0.2 (FR-052..FR-057). See [supported-formats.md](../03-requirements/supported-formats.md) for how each one is run.

| Format | Converter | Notes |
|---|---|---|
| `.docx`, `.doc` | LibreOffice headless → PDF | No first-run prompt: a job that needs a missing LibreOffice pauses, and the Print Job screen offers a consented install (FR-058) |
| `.xlsx`, `.xls` | LibreOffice | |
| `.pptx`, `.ppt` | LibreOffice | |
| `.odt`, `.ods`, `.odp` | LibreOffice | |
| `.rtf` | LibreOffice (Windows, Linux); `textutil` → the system web view (macOS) | |
| `.txt` | decoded in-process → the system web view | BOM, UTF-8, else Windows-1252 |
| `.html`, `.htm` | the system web view (WebView2 / WKWebView / WebKitGTK) in a worker | Local files only; no script, no network |
| `.md`, `.markdown` | `pulldown-cmark` in-process → the system web view | No Pandoc, no LaTeX |
| `.svg`, `.svgz` | `usvg` + `svg2pdf` in a worker | Vector output |
| `.heic`, `.heif` | OS decoder (`sips` / WIC); on Linux `heif-dec` / `heif-convert` (libheif) or ImageMagick | Windows needs the Store *HEIF Image Extensions* (and *HEVC Video Extensions* for `.heic`); Linux needs one of the Linux decoders |

### Dependency detection

The Settings page gains a **Dependencies** panel showing the status of every external dependency (✓ Found / ✗ Missing) with install options and a refresh button.

Partly delivered already: the probe itself exists (`engine::deps`, reported through `app_info`) and covers SumatraPDF, LibreOffice and, on Windows, the HEIF and HEVC Store codecs; Edge/Chrome, Pandoc and ImageMagick are not used. It surfaces on the **Print Job** screen, where only the dependencies that actually gate a file the user added are shown, with remediation text and an **Install…** consent dialog (winget, Homebrew, the Store or the download page). What v0.2 still owes is the always-on Settings panel with a refresh button.

### New print settings

- **Tray** (input bin) — *delivered*: sent to CUPS as `InputSlot`/`media-source` and to SumatraPDF as the driver's bin id; the Printers screen sets a per-printer default
- **Collate** (true/false)

### Quality of life

- The "Configure print job" dialog gains a **preview** pane for PDFs (using PDFium rendering).
- ~~Drag-drop in the GUI accepts folders and recurses with a confirmation prompt.~~ **Delivered in M1.** This was never really a v0.2 item — [FR-004](../03-requirements/functional-requirements.md) and FR-007 both mark folders as MVP, so listing it here was the drift. A dropped folder with subfolders is parked until the screen asks recurse-or-top-level, then walked by the `expand_folders` command; a folder with no subfolders skips the question. Discarded files come back with a reason (FR-005/FR-059) rather than vanishing.
- ~~Recent files / recent jobs shown on the Home page.~~ **Delivered in M1** — Home's recent activity is fed by the real queue.

## v0.3 — Watermark, separator, polish

### Watermarks

- Per-profile watermark configuration: text or image, opacity, position (diagonal / header / footer / center), font size, color.
- Implemented by preprocessing the PDF with a `qpdf` overlay step.

### Separator pages

- Optional divider page between files in a multi-file job.
- Divider shows next file's name and page count.
- Configurable from per-profile settings.

### Profile import/export — **delivered in M1**

- Export a profile to a TOML file, import one or many back (`export_profile` / `import_profiles`, wired to the Profiles screen).
- Still open: importing by **drag-drop** onto the Profiles page, and the `pluryprint profiles import|export <file>` CLI verbs — neither exists.

### Per-job overrides remembered

- "Save these overrides as a new profile" button on the Configure dialog.

## v0.4 — Internationalization

### Languages — **delivered in M1**

- **Spanish** lands as the second language, alongside English. The shipped locales are exactly `en-US` and `es-ES`; there is no separate `es-MX` catalogue, so `es-MX` matches on the language subtag and resolves to `es-ES`.
- All UI strings externalized — as `src/i18n/locales/{lang}/<namespace>.json`, one file per screen, rather than the single flat `i18n/{lang}.json` originally planned.
- Language selector in Settings; first run uses OS locale if matching, then persists the choice to `config.toml`.
- **Not delivered:** ICU MessageFormat. The translator is a small in-house one with parameter substitution and a count-based plural rule; there is no `react-i18next` and no ICU. Gendered forms are unsupported. Backend (Rust) strings are not translated at all.

### RTL preparedness

- Switch to logical CSS properties (`margin-inline-start`, etc.).
- No hard-coded `left`/`right` in component styles.
- RTL itself ships in v2 with Arabic/Hebrew translations.

## v0.5 — Accessibility (WCAG 2.1 AA)

- All screens pass `axe-core` with zero violations.
- Full keyboard navigation, visible focus indicators (≥3:1 contrast).
- Screen-reader compatibility verified with NVDA (Win) and VoiceOver (Mac).
- High-contrast mode follows OS preference.
- Reduce-motion preference respected for animations/transitions.
- Color is never the sole carrier of meaning (status uses icon + label + color).

## v0.6 — Update channels

- **Stable** (default), **Beta**, **Nightly** channels.
- User can switch in Settings; downgrading channel requires confirmation. *Delivered (ADR-0011): stable reads `releases/latest/download/latest.json`, beta reads the `channel-beta` release's manifest, and nightly follows beta. A move to a more stable channel asks for confirmation.*
- ~~`https://updates.maecly.com/pluryprint/{channel}.json` per channel.~~ The manifests live on GitHub Releases.
- Nightly: not built; the channel follows beta.
- Beta and stable: cut from a `vX.Y.Z-beta.N` or `vX.Y.Z` tag by `release.yml` into a draft; installers carry no OS certificate (ADR-0011), updates are minisign-signed.

## v0.7 — N-up & custom paper

### N-up

- Paper subdivisions: 1×1, 1×2, 2×2, 2×3, 3×3.
- Implemented via `pdfjam` (Perl wrapper) preprocessing on Windows; `lpr -o number-up=N` natively on macOS.
- Per-profile setting.

### Custom paper sizes

- Width × height in millimeters.
- macOS only at first (CUPS supports natively); Windows needs more work and lands in v0.9.

## v0.8 — Quality, queue depth, history

- **Quality** setting: draft / normal / high (~150 / ~300 / ~600 dpi hint). *Delivered early: CUPS gets `print-quality`/`cupsPrintQuality`, and Windows applies Draft/High through a temporary per-user default DEVMODE (decision D16).*
- ~~**Queue concurrency**: configurable from 1–8 simultaneous jobs (default 2).~~ **Delivered in M1**, though it bounds concurrent *conversions*, not concurrent spooling: `engine.max_concurrent_conversions`, clamped to 1–8, default 2, read into the dispatcher at app start.
- **Job history** persisted indefinitely (until user clears) with full search/filter. *Today the queue holds finished jobs until `clear_done` (with an undo window); there is no separate searchable history store.*
- Export history to CSV.

## v0.9 — Polish & hardening

- All `FR-*` requirements with `MUST` priority covered by tests.
- All `NFR-*` requirements measured in CI.
- Crash reporter (Sentry-compatible, opt-in, off by default).
- First-run wizard refined based on user testing.
- Documentation site refreshed with screenshots.

## v1.0 — Windows GA

- Code signing with EV cert in production for ≥3 months: full SmartScreen reputation accumulated.
- Public release on `maecly.com` and announcement.
- Stable channel only; beta/nightly remain available.
- 14-day rollback window if critical regression discovered.
- Issue templates and SLAs for triage published.
- Metrics:
  - 5,000+ unique downloads (target)
  - <0.5% crash rate
  - 95%+ job success rate (excluding hardware errors)
  - 10+ external contributors with merged PRs

## Cross-cutting upgrades that span v0.2 → v1.0

These don't fit a single milestone; they accrue over multiple releases.

- **Test coverage** climbs from ~50% (MVP) to ≥70% (v1.0).
- **Release automation** matures: every push to a release branch produces signed installer artifacts ready for promotion.
- **Doc site** moves from raw `docs/` to a published static site (Astro or Docusaurus) on `docs.maecly.com` — but the markdown source remains in this repo.
- **Telemetry-free crash reporting** matures: opt-in, end-to-end encrypted, self-hosted Sentry-compatible.
- **Performance tracking**: Tauri's built-in instrumentation captured in CI to detect regressions.

## Explicitly **not** in v1

| Feature | Lives in |
|---|---|
| macOS support | v1.1 (M5) |
| Hot folder | v2 |
| Multi-printer load balancing | v2 |
| Email/cloud folder monitoring | v2 |
| OCR | v2 |
| CAD/Visio/EPUB | v2 |
| Plugin API | v2 |
| REST API | v2 |
| Margins override | v2 |
| Per-page settings | never |
| Booklet imposition | v2 |

## v1.0 definition of done

A v1.0 release tag is created when:

1. All `MUST` priority FRs and NFRs covered by automated tests.
2. Manual QA matrix (formats × printers × OS versions × languages) passes 100%.
3. Documentation review complete (no broken links, all decisions reflected).
4. Code-signing keys rotated and stored in HSM.
5. `updates.maecly.com` infrastructure verified, including rollback procedure.
6. CHANGELOG.md updated with full v1.0 entry.
7. Public announcement copy ready.
8. Maintainer-on-call rotation defined for the first 14 days post-release.
