# Value Proposition

> **TL;DR** — Pluryprint is the answer to a single, ten-word question: **What if right-click → Print just worked, for any number of files?** It is free, open source (AGPL-3.0), integrated into the Windows 11 modern context menu, and never asks you to pay $149 to bypass an arbitrary 15-file ceiling.

## The Pitch in One Paragraph

Today, batch printing on Windows costs you either time or money. You either click through the same Print dialog 15 times for an 80-file job because Windows hides the verb past 15 selections, or you spend $79 to $149 on a commercial tool and accept that it will live in its own window outside the file manager you actually work in. Pluryprint refuses that trade. It puts a single, modern context-menu entry — **Print all with Pluryprint** — directly into Windows 11's right-click menu, handles 1 or 1,000 files identically, supports the formats you actually print (PDF, Office, images, plus a growing list), and ships under AGPL-3.0 so the cost stays at zero forever.

## Why the Status Quo Is Unacceptable

### The paid path

| Tool | Single-user price | What you get | What you do not get |
| --- | --- | --- | --- |
| Print Conductor | $149 | 90+ format support, polished GUI, profiles, load balancing | Native context menu integration, source code, free updates after a year |
| 2Printer | $149 | Mature CLI, same 90+ formats | A GUI, free version, free updates |
| Batch & Print Pro | $79.95 (Standard) / $109.95 (Premium) | GUI + CLI, hot folder, email and cloud monitoring | Open source, modern context menu integration |
| FolderMill | varies (B2B quotes) | Hot folder automation as a service | A user-facing print-now experience |

These are all good tools. Print Conductor in particular is the most popular product in the category, and it earns its price through breadth and polish. But none of them solves the immediate, in-the-moment friction of *I am looking at these files in Explorer right now and I want them printed*. They all require leaving the shell.

### The free path

| Tool | Why it does not fit |
| --- | --- |
| Adobe Acrobat / Reader | Hits the 15-file cap. Silently drops files. Heavy. Not designed for batch. |
| Windows built-in `Print` verb | The 15-file cap is exactly the problem. |
| `ShellExecute` + `printto` scripts | Brittle. Format-specific. Ordering races. DDE issues with Adobe. |
| clawPDF | Virtual printer — converts to PDF, not a batch printer. |
| Batch PDF Printer (GitHub) | PDF-only. Requires Adobe Reader. Manual timer tuning. |
| DarkStorm's BPH | Abandoned since 2015. |
| PrintFile | Freeware. TXT/PostScript only. No modern formats. |

There is no actively maintained, multi-format, open-source batch printer with a real Windows shell integration. **Pluryprint exists to be that tool.**

## The Six Pillars

### 1. Free and Open Source (AGPL-3.0)

Zero dollars, forever, for any number of users. The full source is on GitHub. The license is the AGPL-3.0, which guarantees that any derivative or fork carries the same freedoms. There is no "Pro" tier waiting in the wings, no upsell, no "free for personal use only" clause.

For organizations, this collapses procurement to a software-allowlist check. For developers, it means you can read the code, fix bugs, or extend it for your specific workflow.

### 2. Native Modern Context Menu Integration

Pluryprint ships as a **Sparse MSIX** package so it can register a real Windows 11 modern context menu entry via `IExplorerCommand` — the menu item appears at the top level of the right-click menu, not behind "Show more options". The integration:

- Works with **any number of selected files** by reading the full `IShellItemArray` (which has no documented item-count limit).
- Appears in both the modern menu (Win11) and the legacy menu (Win10 / "Show more options").
- Is per-machine on install for managed environments, per-user on `--user` install for unprivileged scenarios.
- Can be disabled in Settings if a user prefers GUI-only operation.

This is the feature commercial competitors do not offer. It is the entire reason Pluryprint exists.

### 3. No Artificial Limits

There is no cap on:

- Files per job.
- Pages per file.
- Jobs per session.
- Printers configured.
- Profiles saved.

The only ceiling is what your printer can absorb. We will warn you if you queue 10,000 files at once — but we will still print them.

### 4. Multi-Format from Day One

MVP format coverage targets the formats users *actually batch-print*:

- **PDF** (printed by SumatraPDF on Windows and CUPS on macOS and Linux — never Adobe Reader), encrypted ones included after the user types the password.
- **Microsoft Office and OpenDocument**: DOCX, XLSX, PPTX (and legacy DOC/XLS/PPT), ODT, ODS, ODP via headless LibreOffice.
- **Images**: JPG, PNG, TIFF, BMP, WebP, HEIC, and SVG as vector pages. GIF is deliberately not supported.
- **Plain text, RTF and Markdown**, converted inside the app.
- **HTML**, printed by the system web view (WebView2 / WebKit) with no network access.

That is what ships today; see [supported-formats.md](../03-requirements/supported-formats.md).

A v1+ plugin interface will let third parties add CAD (DWG/DXF), e-mail (EML/MSG/PST), AutoCAD/SolidWorks, e-books, and the long tail that Print Conductor covers via its 90+ formats. We do not need to match that list on day one to be useful.

### 5. Lightweight by Design

| Metric | Target |
| --- | --- |
| Installer size | ~5–10 MB compressed (vs. Print Conductor's installer in the tens of MB). |
| Resident memory, idle | ~30–50 MB (Tauri + WebView2 share, no Electron tax). |
| Cold start to print dialog | < 4 seconds on a 2020-era laptop. |
| CPU footprint at idle | Effectively zero — no background daemon required for the basic right-click flow. |

The Tauri 2 + Rust foundation is what makes this realistic. We are not shipping Chromium, we are not shipping a JVM, and we are not shipping a .NET runtime separately from what Windows already has.

### 6. Privacy-First, Cross-Platform

- **No telemetry by default.** No analytics SDK is even compiled into release builds. Opt-in diagnostic uploads may be added later, gated behind an explicit Settings toggle.
- **No cloud dependency.** The application works fully offline. The only network traffic is print job submission to network printers on your LAN.
- **No account.** No sign-in, no license key, no activation server.
- **Cross-platform path.** Windows 10/11 (x64 and ARM64) at MVP. macOS 13+ (Apple Silicon and Intel) next, sharing the Rust core and React UI, swapping the shell integration layer for Finder Quick Actions / Services. Linux (x86_64 and ARM64; Ubuntu 22.04+, Debian 12+, Fedora 38+) from 0.0.5 as a preview, with "Open with" and Dolphin / Nemo actions as its shell integration ([ADR-0015](../adr/0015-linux-support.md)).

## Head-to-Head Pitches

### Vs. Print Conductor

> "Print Conductor is excellent and expensive. If you have $149 and you live inside Print Conductor's window all day, buy it. If you want the same 'select files, click Print, walk away' experience but starting from your Explorer right-click for free, install Pluryprint."

### Vs. Adobe Acrobat / Reader

> "Adobe is a PDF editor that happens to print. Pluryprint is a printer that happens to handle PDF (and Office, and images) without ever launching Adobe. If you have caught Reader silently skipping files in a multi-select print, you already know why this matters."

### Vs. The 'select 15 at a time' workaround

> "You are not paid by the number of times you click Print. Stop being paid by it."

### Vs. clawPDF

> "Different problem. clawPDF turns *any* print job into a PDF on your disk. Pluryprint turns a folder of files into ink on your printer. They are complementary; install both if you want both directions."

### Vs. 'I'll just write a script'

> "If you enjoy maintaining `ShellExecute` wrappers around Acrobat, Word, and SumatraPDF in PowerShell, Pluryprint is not for you. If you have ever debugged a DDE timeout at 4 PM on a Friday, it is."

## What 'Just Works' Means

A short, opinionated definition of the bar we are clearing:

1. The context menu entry is always visible when at least one printable file is selected, regardless of count.
2. The print preflight window opens in under 4 seconds, even cold.
3. Files print in the order shown in the UI. No races.
4. A failure on file 47 of 100 does not abort the job; it is logged and reported at the end with an offer to retry the failures.
5. Settings persist. The next time you batch-print to the same printer, the same duplex/copy/profile choices are pre-selected.
6. The application closes when the job completes. It does not leave a tray icon, does not hold focus, does not show advertising.

## What This Costs the User

| Cost | Amount |
| --- | --- |
| Money | $0 |
| Account creation | None |
| Subscription | None |
| Telemetry surrendered | None (default) |
| Cloud lock-in | None |
| License audits | None |

## Related Documents

- [Product Vision](./product-vision.md)
- [Target Users](./target-users.md)
- [Competitor Analysis](../02-research/competitor-analysis.md)
- [Market Gap](../02-research/market-gap.md)
