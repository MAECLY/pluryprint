# Pluryprint — Documentation

> Open source batch file printing for Windows, macOS and Linux (Linux in preview). Right-click → print everything, with no 15-file limit, no Adobe required, and no $149 license fee.

This directory contains the full documentation for the **Pluryprint** project: vision, research, requirements, architecture, features, roadmap, build/release pipelines, development guidelines, legal information, and Architecture Decision Records (ADRs).

If you are reading this on GitHub, every link below is clickable.

---

## Quick start by audience

| You are… | Read first |
|---|---|
| A user wanting to know what this is | [Product Vision](./01-vision/product-vision.md) → [Value Proposition](./01-vision/value-proposition.md) |
| A contributor / developer | [Local Setup](./08-development/local-setup.md) → [Contributing](./08-development/contributing.md) → [Coding Standards](./08-development/coding-standards.md) |
| A reviewer / decision-maker | [Tech Stack](./04-architecture/tech-stack.md) → [System Architecture](./04-architecture/system-architecture.md) → [ADRs](./adr/README.md) |
| A packager / sysadmin | [Installer Windows](./07-build-and-release/installer-windows.md) → [Releasing](./07-build-and-release/releasing.md) → [Auto-Update Flow](./07-build-and-release/auto-update-flow.md) |
| Curious about prior art | [Competitor Analysis](./02-research/competitor-analysis.md) → [Market Gap](./02-research/market-gap.md) |

---

## Locked project decisions

These decisions are not up for re-debate during the MVP cycle. They are recorded individually in the [ADR section](./adr/README.md).

| Item | Choice | ADR |
|---|---|---|
| Product name | **Pluryprint** | — |
| Application stack | **Tauri 2 + Rust + React (TypeScript)** | [ADR-0001](./adr/0001-stack-tauri-rust-react.md) |
| License | **AGPL-3.0-only**, contributions under a CLA (was GPLv3 until 2026-10-09) | [ADR-0010](./adr/0010-relicense-to-agpl-3.0.md) (supersedes [ADR-0002](./adr/0002-license-gplv3.md)) |
| Print engine strategy | **Hybrid: SumatraPDF / CUPS as final hop, format-specific converters upstream** | [ADR-0003](./adr/0003-hybrid-print-engine-strategy.md) |
| HTML, Markdown and text | **The system web view (WebView2 / WebKit) in a worker process** | [ADR-0012](./adr/0012-system-web-view-renderer.md) |
| Auto-update | **Tauri Updater plugin (built-in)**, one manifest per channel on GitHub Releases | [ADR-0004](./adr/0004-auto-updater-tauri-builtin.md), [ADR-0011](./adr/0011-release-channel.md) |
| Release signing | **No OS certificates; updates signed with minisign** | [ADR-0011](./adr/0011-release-channel.md) |
| Windows shell extension | **IExplorerCommand COM DLL (Rust), registered per user; sparse package when a certificate exists** | [ADR-0013](./adr/0013-explorer-command-per-user-verb.md) (supersedes [ADR-0005](./adr/0005-shell-extension-sparse-msix.md)) |
| Windows installer | **Tauri's per-user NSIS bundle** | [ADR-0008](./adr/0008-installer-nsis-per-user.md) (supersedes [ADR-0006](./adr/0006-installer-inno-setup.md)) |
| Platform priority | **Windows first, macOS second** | [ADR-0007](./adr/0007-windows-first-mac-second.md) |
| Documentation language | English | — |
| Initial supported OS | Windows 10/11 (x64 + ARM64) | — |
| Next supported OS | macOS 13+ (Apple Silicon + Intel) | — |

---

## Table of contents

### 01 — Vision

- [Product Vision](./01-vision/product-vision.md) — mission, problem, intended outcome
- [Target Users](./01-vision/target-users.md) — personas
- [Value Proposition](./01-vision/value-proposition.md) — why Pluryprint vs alternatives

### 02 — Research

- [Competitor Analysis](./02-research/competitor-analysis.md) — Print Conductor, 2Printer, Batch & Print Pro, FolderMill, PrintFile, clawPDF, etc.
- [Feature Matrix](./02-research/feature-matrix.md) — side-by-side comparison table
- [Market Gap](./02-research/market-gap.md) — why no good OSS alternative exists
- [Windows Context Menu Limitations](./02-research/windows-context-menu-limitations.md) — 15-file limit, Adobe DC bugs, ShellExecute "PrintTo" issues

### 03 — Requirements

- [Functional Requirements](./03-requirements/functional-requirements.md) — FR-001 onward
- [Non-Functional Requirements](./03-requirements/non-functional-requirements.md) — performance, accessibility, i18n
- [Supported Formats](./03-requirements/supported-formats.md) — per-format engine and conversion path
- [Print Settings](./03-requirements/print-settings.md) — full settings matrix

### 04 — Architecture

- [Tech Stack](./04-architecture/tech-stack.md) — Tauri 2 + Rust + React justification
- [System Architecture](./04-architecture/system-architecture.md) — C4-style diagrams
- [Shell Extensions](./04-architecture/shell-extensions.md) — Windows IExplorerCommand + macOS Finder Sync
- [Print Engine](./04-architecture/print-engine.md) — hybrid strategy
- [Auto-Updater](./04-architecture/auto-updater.md) — Tauri updater config
- [Data Model](./04-architecture/data-model.md) — profiles, queue, logs
- [IPC and CLI](./04-architecture/ipc-and-cli.md) — Tauri commands + headless CLI
- [Printer Enumeration](./04-architecture/printer-enumeration.md) — Windows spooler + macOS CUPS detection
- [Security Model](./04-architecture/security-model.md) — code signing, MSIX identity

### 05 — Features

- [MVP Features](./05-features/mvp-features.md) — v0.1 scope
- [v1 Features](./05-features/v1-features.md) — v0.2–v1.0 roadmap
- [Future Features](./05-features/future-features.md) — v2+
- [Context Menu Actions](./05-features/context-menu-actions.md) — per-file-type submenu items
- [Printers Management](./05-features/printers-management.md) — printer list, status, capabilities, add/discover
- [Print Job](./05-features/print-job.md) — the batch composer, settings, resolved-command preview
- [Queue](./05-features/queue.md) — live job queue, states, retries, views
- [Profiles Management](./05-features/profiles-management.md) — built-in + custom profiles, editor, default, undo
- [Settings](./05-features/settings.md) — appearance, engine limits, updates, privacy
- [About](./05-features/about.md) — version, update check, components, paths, links

### 06 — Roadmap

- [Milestones](./06-roadmap/milestones.md) — M0 through M5
- [Timeline](./06-roadmap/timeline.md) — single-dev estimates
- [Release Strategy](./06-roadmap/release-strategy.md) — semver, channels, branching

### 07 — Build & Release

- [Releasing](./07-build-and-release/releasing.md) — the runbook: signing key, channels, cutting and verifying a release
- [Installer (Windows)](./07-build-and-release/installer-windows.md) — per-user NSIS, Explorer registration
- [Installer (macOS)](./07-build-and-release/installer-macos.md) — ad-hoc-signed `.dmg` per architecture (and the Developer ID plan)
- [Auto-Update Flow](./07-build-and-release/auto-update-flow.md) — end-to-end
- [Code Signing](./07-build-and-release/code-signing.md) — what is signed today (updates, minisign) and the certificate plan
- [CI / CD](./07-build-and-release/ci-cd.md) — GitHub Actions matrix

### 08 — Development

- [Repo Structure](./08-development/repo-structure.md) — monorepo layout
- [Coding Standards](./08-development/coding-standards.md) — Rust + TS conventions
- [Local Setup](./08-development/local-setup.md) — prerequisites, dev env
- [Contributing](./08-development/contributing.md) — PR flow, code of conduct
- [Testing Strategy](./08-development/testing-strategy.md) — unit / integration / e2e
- [Release Checklist](./08-development/release-checklist.md) — pre-release manual QA

### 09 — Legal

- [License](./09-legal/license.md) — what the AGPL-3.0 and the CLA mean in practice
- [Third-Party Licenses](./09-legal/third-party-licenses.md) — SumatraPDF, LibreOffice, Tauri, etc. (the shipped notice is [`THIRD-PARTY-NOTICES.md`](../THIRD-PARTY-NOTICES.md))
- [Trademark](./09-legal/trademark.md) — name and logo usage policy
- [Privacy](./09-legal/privacy.md) — telemetry, crash reports, logs

### ADRs

- [ADR Index](./adr/README.md)
- [0001 — Stack: Tauri + Rust + React](./adr/0001-stack-tauri-rust-react.md)
- [0002 — License: GPLv3](./adr/0002-license-gplv3.md) (superseded by 0010)
- [0003 — Hybrid print engine strategy](./adr/0003-hybrid-print-engine-strategy.md)
- [0004 — Auto-updater: Tauri built-in](./adr/0004-auto-updater-tauri-builtin.md)
- [0005 — Shell extension: Sparse MSIX](./adr/0005-shell-extension-sparse-msix.md)
- [0006 — Installer: Inno Setup](./adr/0006-installer-inno-setup.md)
- [0007 — Platform priority: Windows first, macOS second](./adr/0007-windows-first-mac-second.md)
- [0010 — Relicense to AGPL-3.0](./adr/0010-relicense-to-agpl-3.0.md)

---

## How this documentation is organized

We follow a **numbered-folder** convention for ordering and a flat hierarchy inside each folder. Cross-references use relative links so the docs work both on disk and on GitHub. ADRs follow the [Michael Nygard ADR template](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions): `Status`, `Context`, `Decision`, `Consequences`.

For changes to architectural decisions, **add a new ADR** that supersedes the old one rather than rewriting history.

## Status

This project is **pre-alpha · M1 (Print Core) largely delivered**. There is real production code behind most of these pages: a print engine, a persistent queue, profiles, settings, a headless CLI, and the Explorer/Finder context-menu handlers. There is also work that is still specification only — notably the installer and the download-and-install half of the updater.

[**Roadmap → Milestones**](./06-roadmap/milestones.md) is the single source of truth for what is delivered and what is open; per-requirement status lives in the `Status` column of [Functional Requirements](./03-requirements/functional-requirements.md). Those two documents are deliberately the only place that tracks status, so it cannot drift in three directions at once.

Expect rough edges; expect change. The locked decisions in the table above are stable.

## License of this documentation

The documentation in this folder is released under the same license as the code: **[AGPL-3.0](../LICENSE)**.
