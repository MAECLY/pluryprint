# Architecture Decision Records (ADRs)

> **TL;DR** — This folder contains the architectural decisions for Pluryprint, recorded in **Michael Nygard's ADR format**. Each file captures one decision: the context that forced it, the alternatives considered, the choice made, and the consequences. ADRs are append-only — once accepted, an ADR is never edited; it is **superseded** by a new ADR if the decision changes.

## Index

| ID   | Title                                                                          | Status   | Date       |
| ---- | ------------------------------------------------------------------------------ | -------- | ---------- |
| 0001 | [Stack: Tauri 2 + Rust + React (TypeScript)](./0001-stack-tauri-rust-react.md) | Accepted | 2026-05-03 |
| 0002 | [License: GPLv3](./0002-license-gplv3.md)                                       | Superseded by ADR-0010 | 2026-05-03 |
| 0003 | [Hybrid print-engine strategy](./0003-hybrid-print-engine-strategy.md)          | Accepted; HTML/Markdown/text converters superseded by ADR-0012 | 2026-05-03 |
| 0004 | [Auto-updater: Tauri Updater plugin](./0004-auto-updater-tauri-builtin.md)      | Accepted | 2026-05-03 |
| 0005 | [Shell extension: Sparse MSIX + IExplorerCommand](./0005-shell-extension-sparse-msix.md) | Superseded by ADR-0013 | 2026-05-03 |
| 0006 | [Installer: Inno Setup (+ Sparse MSIX for shell ext)](./0006-installer-inno-setup.md) | Superseded by ADR-0008 | 2026-05-03 |
| 0007 | [Windows-first, macOS-second roadmap](./0007-windows-first-mac-second.md)       | Accepted; Linux deferral superseded by ADR-0015 | 2026-05-03 |
| 0008 | [Installer: per-user NSIS with signed updater artifacts](./0008-installer-nsis-per-user.md) | Accepted | 2026-10-09 |
| 0009 | [Rename the product from MaeclyPrint to Pluryprint](./0009-rename-to-pluryprint.md) | Accepted | 2026-10-09 |
| 0010 | [Relicense to AGPL-3.0](./0010-relicense-to-agpl-3.0.md)                        | Accepted | 2026-10-09 |
| 0011 | [Release channel on GitHub Releases, beta manifest, no OS code signing](./0011-release-channel.md) | Accepted; D20 (Linux out of scope) superseded by ADR-0015 | 2026-10-10 |
| 0012 | [The system web view renders HTML, Markdown and text, in a worker process](./0012-system-web-view-renderer.md) | Accepted | 2026-10-10 |
| 0013 | [The Explorer command ships as a per-user registry verb](./0013-explorer-command-per-user-verb.md) | Accepted | 2026-10-10 |
| 0014 | [The SVG worker process is the resource boundary](./0014-svg-worker-resource-boundary.md) | Accepted | 2026-10-10 |
| 0015 | [Linux x86_64 and ARM64 builds, shipped as AppImage and .deb](./0015-linux-support.md) | Accepted | 2026-11-09 |
| 0017 | [Print SRI XML as a RIDE-like representation](./0017-print-sri-xml-as-ride.md) | Accepted | 2026-11-14 |

## Format

We use the format proposed by Michael Nygard in [*Documenting Architecture Decisions*](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions). Each ADR has these sections:

1. **Title** — short noun phrase, prefixed with the four-digit ID.
2. **Status** — one of: `Proposed`, `Accepted`, `Superseded by ADR-XXXX`, `Deprecated`, `Rejected`.
3. **Date** — ISO 8601 date the decision was last changed.
4. **Context** — the situation, the forces at play, the constraints. Plain technical English; no decisions yet.
5. **Decision** — what we chose, in active voice. ("We will use X.")
6. **Alternatives Considered** — a table or list of other options, with a one-line reason each was not picked.
7. **Consequences** — what becomes easier and what becomes harder. Three sub-sections recommended: **Positive**, **Negative**, **Risks / Open Questions**.

## Conventions

- IDs are sequential and zero-padded to four digits (`0001`, `0002`, ...). Once assigned, an ID is permanent.
- Filenames are `NNNN-kebab-case-title.md`.
- ADRs are written in present tense for the decision (you are documenting what we *will* do as of today) and past tense for context.
- We avoid product-marketing language. ADRs are technical artifacts read by future contributors trying to understand "why on earth did they choose this?"
- Keep them short. Most ADRs should be 80–180 lines; longer means the decision should probably be split into multiple ADRs.

## Lifecycle

```
   ┌────────────┐      accept       ┌──────────────┐    new ADR     ┌────────────────┐
   │  Proposed  │ ────────────────> │   Accepted   │ ─────────────> │  Superseded by │
   └────────────┘                   └──────────────┘                │    ADR-XXXX    │
         │                                                           └────────────────┘
         │ reject
         v
   ┌────────────┐
   │  Rejected  │
   └────────────┘
```

### Superseding

When a decision changes, **do not edit the original ADR**. Instead:

1. Write a new ADR explaining the new decision and its context.
2. Reference the old ADR in the new one's *Context* section: "ADR-0003 originally chose X; new evidence shows Y, so we are revisiting."
3. Update the old ADR's `Status:` line to `Superseded by ADR-XXXX` and add a one-line note at the top pointing to the new ADR. **No other change to the old ADR.**

This preserves the historical record of why we used to do things differently — invaluable for future maintainers and for evaluating whether the new decision was right.

### Deprecating

If a decision is no longer relevant but is not being replaced (e.g., we removed a subsystem), mark its `Status:` as `Deprecated` and add a one-line note explaining what happened. Again, leave the body intact.

## Contributing a new ADR

1. Pick the next free ID. Check the index above.
2. Copy `template.md` to `NNNN-kebab-case-title.md` (template provided alongside this README).
3. Fill in all sections. **Do not skip Alternatives Considered** — an ADR without alternatives is not an ADR, it is a press release.
4. Open a PR titled `ADR-NNNN: <title>`. The PR is the discussion forum.
5. On merge, update this index.

## Why we keep ADRs

- **Onboarding.** New contributors can read `adr/` cover-to-cover and understand the shape of the codebase faster than reading code.
- **Avoiding revisits.** When someone asks "why don't we use Electron?", the answer is `ADR-0001` and the conversation is over in three seconds.
- **Honest history.** The Alternatives sections record what we genuinely considered, not what we want to claim we considered.
- **Constraint propagation.** A new ADR can call out that it is *constrained* by an earlier one (e.g., "ADR-0002 means we cannot use proprietary X").

## Related

- [`../04-architecture/`](../04-architecture/) — the *what* and *how*. ADRs are the *why*.
- [`../06-roadmap/`](../06-roadmap/) — what we plan to build, milestone by milestone.
