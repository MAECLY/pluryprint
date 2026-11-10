# Product Vision

> **TL;DR** — Pluryprint is a free, open-source, AGPL-3.0-licensed batch printing utility for Windows (and later macOS) built with Tauri 2 + Rust + React. It replaces the friction of right-clicking a folder of files and discovering Windows hides "Print" past 15 selections, or paying $79–$149 for commercial alternatives.

## Mission Statement

Pluryprint exists to make **batch printing from the file manager feel native, instant, and unrestricted**. When a user selects a hundred PDFs, twenty Word documents, or a mixed pile of invoices and images, right-clicking and choosing "Print all" should just work — no app launches, no per-file dialogs, no artificial caps, no $149 license fee.

We believe a deep desktop integration of this quality belongs in the open-source commons. It is too small a feature to justify the recurring revenue model the incumbents rely on, and too important a daily workflow to leave behind a paywall.

## The Problem

### The 15-file ceiling

Selecting more than **15 files** in Windows Explorer causes the operating system to silently remove the "Print" verb from the context menu. This is not a command-line length limitation — `cmd.exe` accepts up to 8,191 characters per invocation — it is a deliberate UX guardrail Microsoft introduced to prevent accidental mass actions. The result is that any user who needs to print 16 or more files is funneled into one of three unpleasant routes:

1. Open each file (or batches of 15) and use File → Print.
2. Buy a commercial batch printer (Print Conductor at **$149**, 2Printer at **$149**, Batch & Print Pro at **$79.95**–**$109.95**).
3. Hand-roll a script with `ShellExecute` and the `printto` verb, then debug the application-specific quirks (Adobe Reader silently dropping files, Word DDE timeouts, ordering races) for the rest of the afternoon.

### Adobe Acrobat and Reader edge cases

When **Adobe Acrobat DC** is the default PDF handler, the "Print" verb is removed from the right-click menu entirely. When **Adobe Reader** is the default and a user selects multiple files, Reader frequently prints only a subset (for example, 5 of 8) without surfacing any error. The partial-selection workaround — disabling "Protected Mode at startup" — is itself a security regression.

### Commercial tooling is excellent but expensive and proprietary

Print Conductor, 2Printer, FolderMill, and Batch & Print Pro are mature, well-supported tools with rich feature sets. They also assume a B2B procurement budget. None of them is open source. None of them ships a true Windows 11 modern context menu integration. None of them is built with a privacy-first, telemetry-free default.

### Existing OSS alternatives do not fill the gap

- **clawPDF** is a *virtual* printer — it converts print jobs to PDF/SVG/PNG, which is the inverse of what we need.
- **Batch PDF Printer** (GitHub: stylersnico/batch-pdf-printer) is a clever set of `.bat` scripts but only handles PDFs and requires a working Adobe Reader install plus manual timer tuning.
- **DarkStorm's Batch Print Handler** is a GPLv2 .NET tool that has been abandoned since 2015.
- **PrintFile** is freeware (not OSS), supports only TXT/PostScript/EPS, and offers no modern Office or image format coverage.

The honest summary: **there is no actively maintained, open-source, multi-format batch printer integrated into the Windows shell**.

## Intended Outcome

A user installs Pluryprint. They select 200 mixed files in Windows Explorer. They right-click. They see a single, unambiguous **"Print all with Pluryprint"** entry in the modern Windows 11 menu (no "Show more options" detour). They click it. A compact, fast Tauri window opens with the file list pre-populated. They confirm printer, duplex, and copies. They click Print. The job runs to completion. The window closes.

That sequence — measured end-to-end in seconds, not minutes — is the entire product.

Everything else (hot folders, scheduling, watermarks, load balancing, CLI automation, plugins) is a logical extension built atop the same primitive: **a reliable, format-aware print engine and a deep shell integration**, both free.

## Non-Goals

What Pluryprint is explicitly **not** trying to be:

- **Not a virtual printer.** We do not convert files to PDF or capture print jobs from other applications. That space is well covered by clawPDF, Microsoft Print to PDF, and CutePDF.
- **Not a PDF editor.** No annotation, no form-filling, no signing, no OCR. Out of scope. Use a real PDF editor.
- **Not a cloud service.** No file uploads, no account requirements, no SaaS dashboard. The application runs entirely on the local machine and talks only to local printers and (optionally) printers reachable on the user's LAN.
- **Not a print spooler replacement.** We submit jobs to the existing Windows print spooler (and CUPS on macOS). We do not reimplement spooling, queue management, or driver selection.
- **Not a document conversion tool.** While we render Office and CAD formats internally to dispatch them to the printer, we do not expose that pipeline as a "Save as PDF" feature. If the output format you want is a file, use the right tool for that job.
- **Not a print-shop POS.** No invoicing, no job tracking by customer, no user accounts. Print shops can certainly use Pluryprint, but it is a tool, not a business management system.
- **Not a telemetry harvester.** No analytics SDK, no crash uploader by default. The app is privacy-first; opt-in diagnostics may be added later, never on by default.

## Success Metrics

The project is healthy if, twelve months after the v1.0 release, we can point to:

| Metric | Target | Rationale |
| --- | --- | --- |
| Cumulative downloads (GitHub Releases + winget) | 50,000+ | Indicates real-world demand reached. |
| GitHub stars | 2,500+ | A useful proxy for developer mindshare and discoverability. |
| Active contributors (>= 1 merged PR) | 25+ | Confirms the project is not a single-maintainer bottleneck. |
| Print job success rate (jobs completed without user retry) | >= 98% | The product's only real promise is reliability. |
| Time to first print (cold start, 50-file PDF batch) | < 4 seconds to spinner, < 30 seconds to first page out | Performance must beat opening Adobe and clicking Print 50 times. |
| Median installer size | < 12 MB | Lean by design; an order of magnitude smaller than Print Conductor. |
| Median resident memory footprint | < 60 MB while idle, < 200 MB during a 100-file batch | Tauri's footprint advantage must show up in practice. |
| Percentage of issues triaged within 7 days | >= 80% | Maintenance hygiene, not a vanity metric. |

We deliberately exclude metrics that would tempt scope creep — DAU, MAU, conversion funnels, in-app upsells. None of those apply to a local utility.

## Long-term Vision

The roadmap past v1.0 is additive, not directional. The core product stays the same: right-click → batch print, end of story. Future capabilities orbit that core:

- **Hot folders.** Drop files into a watched folder, Pluryprint prints them. Inspired by FolderMill but local-first and free.
- **Multi-printer load balancing.** Round-robin or least-loaded distribution across multiple printers, in the spirit of Print Conductor 11's Printer Load Balancing feature.
- **Plugins.** A stable Rust trait + WIT interface so third parties can add format renderers (additional CAD formats, niche e-book types, in-house document templates) without forking the core.
- **Scheduling.** Time-based and trigger-based jobs (e.g., print every invoice landing in `Inbox\Invoices\` after 6 PM).
- **Profiles.** Saved combinations of printer + duplex + watermark + page range, invokable from the context menu (`Print all with Pluryprint → "Letterhead Duplex"`).
- **CLI parity.** `pluryprint --src "C:\Invoices\*.pdf" --printer "HP M404" --duplex long --copies 2` for scripting and CI use, modeled after 2Printer's flag style.
- **Cross-platform.** macOS 13+ on Apple Silicon and Intel, using CUPS and a Finder Quick Action / Services menu integration. Linux x86_64 and ARM64 (Ubuntu 22.04+, Debian 12+, Fedora 38+) through the same CUPS backend, as AppImage and `.deb`, with "Open with" and Dolphin / Nemo actions ([ADR-0015](../adr/0015-linux-support.md); a preview from 0.0.5).
- **Optional cloud features.** Strictly opt-in; for example, a shared profile sync via Git or WebDAV. Never by default, never required.

Each of those is a discrete chunk of work documented separately under `../06-roadmap/`. The vision above is the rope they all hang from: **eliminate the friction between selecting files and getting paper out of a printer, for free, with no strings attached**.

## Related Documents

- [Target Users](./target-users.md)
- [Value Proposition](./value-proposition.md)
- [Competitor Analysis](../02-research/competitor-analysis.md)
- [Feature Matrix](../02-research/feature-matrix.md)
- [Market Gap](../02-research/market-gap.md)
- [Windows Context Menu Limitations](../02-research/windows-context-menu-limitations.md)
