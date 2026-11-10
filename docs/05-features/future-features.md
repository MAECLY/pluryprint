# Future Features (v2 and beyond)

> **TL;DR** — A wishlist of capabilities that **could** make Pluryprint useful for power users, IT departments, and print shops, once the v1 core is rock solid. None of these are committed; they exist to give shape to the long-term direction.

The post-v1 roadmap is intentionally generous. We list ideas to attract contributors and align the architecture today with where we might go. Anything here can be cut, deferred indefinitely, or replaced based on what users actually want.

## Automation & monitoring

### Hot folder / watch folder

Monitor a directory; new files matching a pattern are automatically printed with a configured profile.

- Multiple watch rules.
- Per-rule: directory, glob, profile, post-action (move/delete/leave).
- Runs as a Windows Service or macOS LaunchAgent so it works without the GUI open.
- Replicates Print Conductor's *Foldermill* and Batch & Print Pro's hot-folder feature.

### Email-to-print

Listen on a POP3/IMAP mailbox; emails matching rules trigger printing of attachments.

- IMAP polling or IDLE.
- Sender / subject / attachment-type filtering.
- Per-rule profile mapping.
- Reply on completion (optional).

### Cloud folder watching

Same as hot folder but for Dropbox / Google Drive / OneDrive folders.

- Use each provider's notification API where available; fall back to polling.
- Per-folder rule and profile.
- Useful for distributed teams and print-on-arrival workflows.

### Scheduler

Print at a scheduled time (e.g., nightly invoice batch).

- Cron-like schedule in the GUI.
- Combines with hot folder ("snapshot the folder at 2am, print everything new since yesterday").

## Print management

### Multi-printer load balancing

Distribute jobs across multiple printers.

- Strategies: round-robin, least-loaded, fastest-completing.
- Per-job preferred printer with failover to a pool.
- Mirrors Print Conductor 11's load balancing feature.

### Failover

If a printer goes offline (out of paper, jammed, network down), automatically reroute pending jobs to a configured backup printer.

### Quotas and reporting

- Per-user / per-printer page counts.
- Monthly CSV reports.
- Useful for shared print environments.

### Page text matching

Print only pages of a document that contain specific text — replicates Batch & Print Pro's feature, useful for filtering large reports.

### Blank page removal

Detect and skip blank pages before printing.

### Letterhead overlay

Apply a PDF letterhead/template as background overlay on every printed page.

## Format support expansion

### CAD

- AutoCAD `.dwg`, `.dxf` via LibreCAD or the proprietary ODA SDK.
- SolidWorks, Inventor: too proprietary; likely never.

### Visio

- `.vsdx` via LibreOffice (fragile; depends on LibreOffice's Visio import quality).

### EPUB

- For users who want to print e-books or ARC review copies.

### Email files

- `.eml`, `.msg`, `.mbox`, `.pst` rendered to PDF then printed.
- Tricky because email rendering is non-trivial; possibly via a headless Thunderbird embed.

### Postscript and legacy print formats

- `.ps`, `.eps`, `.pcl`, `.zpl` — needs Ghostscript / driver-specific tooling.

### Code with syntax highlighting

- Pretty-print source code (`.py`, `.rs`, `.ts`, `.go`, …) with syntax coloring before printing.
- Replicates PrintFile's old niche.

## Intelligence & ML

### OCR

- Run Tesseract on raster files before printing so the printed copy has selectable/searchable text.
- Useful when printing scanned documents.

### Auto-orientation

- Detect text orientation in images and rotate before printing.

### Auto-grayscale

- Detect that a document is effectively grayscale and switch color mode silently to save toner (with user opt-in default).

### Smart paper sizing

- Detect oversized documents and warn the user before they print 100 letter-size pages from an A3 source.

## Extensibility

### Plugin API

- Loadable Rust crates or WebAssembly plugins for new format converters.
- Manifest schema for declaring supported extensions, conversion command, and capabilities.
- Plugin registry on a central server (opt-in install).

### REST API

- Local HTTP server (default off, opt-in) for scripting and integration.
- Endpoints: submit job, list profiles, query queue, get logs.
- OpenAPI documented.

### Webhooks

- POST to a configured URL on job completion or failure.
- Useful for Slack notifications, ticketing system integration.

### CLI scripting cookbook

- Documented recipes for common scripts (PowerShell, Bash) that drive the CLI for batch operations.

## Distributed & cloud

### "Print Anywhere"

- Send a print job to a printer at the office from a laptop at home.
- Requires a small relay service (self-hosted optional, hosted as opt-in paid service).
- Encrypted end-to-end so Pluryprint can never see the document.

### Mobile companion

- Web-based GUI accessible on phone for monitoring and triggering jobs.
- Read-only by default; write access requires PIN.

### Multi-user mode

- Multiple users on a workstation each have their own profiles, queue, and history.
- Shared profiles available to all users (admin-managed).

## Enterprise / compliance

### Audit trail

- Tamper-evident log of every print job (signed, append-only).
- Useful for regulated environments.

### Active Directory / SSO

- Profile assignment by AD group membership.
- SAML/OIDC SSO for the management GUI.

### Policy engine

- Centrally enforced rules: "All HR documents must be duplex monochrome", "PDFs over 100 pages require manager approval".

### MDM / Group Policy templates

- ADMX/ADML templates for Windows Group Policy administration.
- Configuration profiles for macOS MDM.

## Operating system reach

### Linux

- Shipped as a preview in 0.0.5 ([ADR-0015](../adr/0015-linux-support.md)): x86_64 and ARM64, AppImage and `.deb`, "Open with" everywhere and Dolphin / Nemo actions from the `.deb`.
- Still future: a GNOME Files (Nautilus) entry, Thunar and Caja actions, per-profile rows in the file-manager menus, an opt-in way for the AppImage to register itself, and a package repository, Flatpak or `.rpm` if there is demand.

### Windows on ARM

- Already in scope from MVP (ARM64 target).

### Headless server installs

- Run as a Windows Service or systemd unit.
- All settings via config files; GUI-less.

## UI/UX upgrades

### Preview improvements

- Live thumbnail preview of every page before printing (currently just the first page).
- Draggable page reordering.

### Per-page settings

- Apply different paper / orientation / scaling per page.
- Niche but powerful for specific workflows.

### Booklet imposition

- 2-up / 4-up booklet layout with proper imposition (so you fold and staple).

### Stapling and finishing

- Driver-specific finishing options (staple, hole-punch, fold).
- Surface in UI when driver supports.

### Accessibility v2

- Voice control integration (Windows Voice Access, macOS Voice Control).
- High-contrast theme variants beyond OS-driven.

## Telemetry that respects privacy

- Anonymous, aggregated, opt-in: format counts, printer model counts, error categories.
- No file contents, no file paths, no user identifiers, no IP addresses retained.
- Self-hosted backend so there is no third-party trust required.
- Quarterly anonymized reports published openly on the website.

## Things we will likely never do

| Idea | Why not |
|---|---|
| Built-in PDF editor | Out of scope; pair with a real PDF tool |
| Cloud-only or subscription model | Against project ethos |
| Closed-source plugins | License-incompatible with the AGPL-3.0 |
| Bundled converter binaries (Office, Chromium) | Bloats installer, license risk; users install separately |
| Sponsored content / ads in UI | Never |
| Automatic anonymous telemetry without opt-in | Never |

## How features get prioritized

When choosing what to build next post-v1, we weigh:

1. **User demand** (issue upvotes, recurring requests, surveys).
2. **Strategic fit** (does it strengthen the core thesis or dilute it?).
3. **Implementation cost** vs **maintenance cost over time**.
4. **License/legal risk** (especially for new bundled deps).
5. **Contributor interest** (a feature with a willing contributor leaps the queue).

Anything in this document can be promoted to a roadmap milestone via a GitHub Discussion + maintainer approval.
