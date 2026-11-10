# ADR-0007: Windows-first, macOS-second roadmap

## Status

Accepted

Update (2026-11-09): the "Linux (M6+)" deferral is superseded by [ADR-0015](./0015-linux-support.md); Linux ships alongside Windows and macOS from 0.0.5, as a preview.

## Date

2026-05-03

## Context

MaeclyPrint targets two desktop platforms: Windows 10/11 and macOS 13+. Per ADR-0001, the codebase is ~95% portable Rust + React; the remaining ~5% is platform-specific (shell extension, printer adapter, MSIX/notarization, installer).

We have to decide whether to:

A. Develop both platforms in parallel from M0, releasing them together.
B. Ship Windows first (M0–M4), then port to macOS (M5).
C. Ship macOS first, port to Windows.

Relevant context:

- **Target market.** Our core user is "an office worker who has dozens of files to print and gets blocked by Win Explorer's 15-file limit". This describes a Windows user. Mac users print less of this kind of bulk content (Macs are common in design / dev / education, where bulk-document printing is less of a daily pain).
- **Platform asymmetry.** The Windows shell extension story is *much* more complex than the Mac one (Sparse MSIX, IExplorerCommand, COM, manifest).  Building both shell paths simultaneously doubles cognitive overhead.
- **Team size.** The project is currently solo (one developer). Splitting attention across two platforms costs more than 2× the time of focusing on one — every bug surfaces in two contexts, every CI matrix is doubled, every release runbook is doubled.
- **CUPS vs Print Spooler.** The Mac printer adapter (`lpr`/CUPS) is dramatically simpler than the Windows side (Print Spooler API + SumatraPDF orchestration). Once Windows works, the Mac port should be days-to-weeks, not months.
- **Distribution.** Windows distribution is via direct download (Inno Setup). Mac distribution requires Apple Developer ID, notarization, and `.dmg` creation — additional infrastructure we should set up only when actually shipping.

## Decision

We will **prioritize Windows in milestones M0–M4, then port to macOS in M5**.

Specifically:

- **M0 (foundations)**: Rust core + Tauri scaffold + React shell, Windows-only, CI for Windows.
- **M1 (vertical slice)**: end-to-end print of PDFs from a CLI invocation on Windows.
- **M2 (formats)**: all converter integrations on Windows.
- **M3 (shell ext)**: Sparse MSIX + IExplorerCommand DLL on Windows, including registry fallback for Win 10.
- **M4 (polish + 1.0 GA)**: Inno Setup installer, EV signing, auto-updater wired up, beta channel, **Windows 1.0 release**.
- **M5 (macOS port)**: macOS shell glue, `lpr` printer adapter, `.dmg` packaging, notarization, **macOS 1.0 release** ~3-6 months after Windows GA.

The Rust core is written platform-agnostically from M0; trait objects (`Printer`, `Converter`) are designed so that the Mac implementations slot in without refactoring. We do **not** maintain Mac-specific code paths during M0–M4 except for the `Printer` trait shape and conditional compilation guards.

## Alternatives Considered

| Option                                  | Why rejected                                                                                              |
| --------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| **A — Both platforms parallel from M0** | Doubles surface area, doubles CI cost, splits focus. Solo dev can't sustain it. Slows down the Windows release without delivering a Mac release any sooner because the Mac shell story is downstream of the architecture decisions. |
| **C — Mac first**                       | Mac is the smaller target market for this product. Moreover, the harder shell-extension story is on Windows; tackling Mac first would mean rebuilding the engine and queue when the Windows shell-ext architecture forces refactors. We'd rather take the hit early. |
| **B' — Windows first, but skip Mac entirely** | Excludes a significant audience. Per ADR-0001 we explicitly designed for cross-platform; abandoning Mac would waste that investment. |
| **B'' — Windows first, parallel Linux** | Linux is desirable long-term but no third-party shell-extension story is consistent across distros (Nautilus / Dolphin / Thunar all differ). Defer to M6+. |

## Consequences

### Positive

- **Faster Windows 1.0.** Single-platform focus accelerates the shipping date by an estimated 30–40%.
- **Simpler CI matrix.** Win x64 + ARM64 only during M0–M4. Mac runners added in M5.
- **Lower infrastructure cost.** No need for Apple Developer Program ($99/yr), notarization workflow, or macOS runners until M5.
- **Architectural validation.** By the time we port to Mac, the Rust core and trait abstractions are battle-tested by real Windows usage — finding the rough edges before porting.
- **Tighter feedback loop on the harder platform.** Windows is the platform with the harder shell story; iterating on it without the Mac distraction means the Sparse MSIX work gets the attention it needs.
- **CUPS keeps the Mac port short.** When M5 starts, the printer adapter (`lpr`) is a 1-2 day implementation. Quick Action shell integration is another few days. We expect M5 to be 4-8 weeks total, not 6 months.

### Negative

- **Mac users wait.** Anyone who finds the project early and runs Mac will be told "M5, expected Q4 2026". Some will be disappointed; some will contribute.
- **Some Mac-specific bugs latent until M5.** If the Rust core has Windows-leaning assumptions (e.g., path separators in logs), they could hide until the port. Mitigated by `cargo test --target x86_64-apple-darwin` running on a Mac runner from M2 onward (just unit tests, not the full app).
- **Documentation has to flag "Windows only for now"** in many places. We do this consistently with a `[Windows only — macOS in M5]` admonition in the README.

### Risks / Open Questions

- **Apple silicon test hardware.** We need at least one Apple Silicon machine for M5 development. Renting from MacStadium is the planned path; bought hardware is more cost-effective if a contributor has one.
- **macOS Sequoia changes.** Apple periodically tightens Finder Sync Extension entitlements. We'll re-validate against the current macOS at M5 start.
- **Mac shell ext might surprise us.** We assume Quick Actions are sufficient for M5 and Finder Sync is post-1.0. If user feedback strongly favors Finder Sync immediately, we'll adjust.
- **Linux (M6+).** Long-term we want Nautilus / Dolphin support. The code shape we are building (small native shell glue calling the Tauri exe via CLI args) is already Linux-friendly; it's just three different shell-glue implementations.
- **Windows ARM64.** We treat Win ARM64 as part of M0–M4 (same code path as x64; just a different build target). Apple Silicon is part of M5.

## Related

- ADR-0001 (stack) — Tauri's cross-platform story is what makes the M5 port feasible.
- ADR-0005 (Sparse MSIX) — the heaviest Windows-specific work, justifying focus.
- [`../06-roadmap/`](../06-roadmap/) — milestone definitions and dates.
