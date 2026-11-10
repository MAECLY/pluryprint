# Non-Functional Requirements

> **TL;DR** — Quality attributes: how fast, how reliable, how secure, how accessible, how maintainable. Targets are concrete and testable; vague aspirations are not allowed here.

Non-functional requirements (NFRs) are the "ilities" that the system must satisfy across all features. Functional requirements (FRs) live in [functional-requirements.md](./functional-requirements.md).

Each NFR has a stable ID (`NFR-XXX`), a measurable target, and a verification method.

> **The "Verification" column is the *intended* method, not a claim that it runs.** Where an NFR is met, or where its stated verification does not exist, this is called out inline. The [Implementation status](#implementation-status) section at the end is the summary; it is the part to trust, because every line in it was checked against the tree.

## 1. Performance

| ID | Requirement | Target | Verification |
|---|---|---|---|
| NFR-001 | Cold start of the context menu invocation MUST complete the launch of the configuration GUI in <500 ms after right-click | p95 ≤ 500 ms on a 4-core/8 GB Win 11 machine with SSD | E2E test with stopwatch instrumentation |
| NFR-002 | Cold start of the silent print path (default profile, no GUI) MUST submit the first job to the spooler in <300 ms | p95 ≤ 300 ms | Same E2E test |
| NFR-003 | Idle RAM usage of the resident GUI MUST be <100 MB | RSS ≤ 100 MB after 60s idle | `ps`/Task Manager assertion in CI |
| NFR-004 | Peak RAM usage with 50 jobs queued MUST be <250 MB | RSS ≤ 250 MB | Load test |
| NFR-005 | The system MUST NOT add more than 50 ms of overhead to the total per-file printing time vs invoking the same backend (SumatraPDF/CUPS) directly | Δ ≤ 50 ms p50 | Microbenchmark in CI |
| NFR-006 | The Windows installer (`setup.exe`) MUST be ≤ 20 MB | size ≤ 20 MB | Size assertion in release pipeline — **in place**: `release.yml` fails a Windows build over 20 MiB (raised from 15 MiB on 2026-10-10, see below) and reports the size of every installer. Measured: the ARM64 installer is 13.48 MiB, about 8 MiB of it SumatraPDF ([releasing.md §9](../07-build-and-release/releasing.md)) |
| NFR-007 | The macOS `.dmg` MUST be ≤ 25 MB (universal binary) | size ≤ 25 MB | Same — **not gated**: releases ship one `.dmg` per architecture (`apple_silicon`, `apple_intel`), not a universal binary, and no step measures them |
| NFR-008 | The Sparse MSIX package MUST be ≤ 2 MB | size ≤ 2 MB | Same — **not applicable yet**: no sparse package is built (decision D3, [shell-extensions.md §2.5](../04-architecture/shell-extensions.md)) |

## 2. Reliability

| ID | Requirement | Target | Verification |
|---|---|---|---|
| NFR-010 | The system MUST recover unfinished queue items after an unexpected exit (crash, kill, OS reboot) | 100% of jobs not marked `printed` resume on next launch | Crash-restart integration test |
| NFR-011 | The system MUST NOT corrupt the user's source files. Print engine adapters MUST treat input files as **read-only**. | Zero hash mismatches on test corpus after a full print cycle | File hash verification in CI |
| NFR-012 | A print/convert timeout MUST NOT leak processes; orphan children MUST be killed | **Met**: the dispatcher uses `wait-timeout` for a bounded wait and kills the child, so a hung subprocess cannot wedge the queue. Covered by `src-tauri/tests/print_timeout.rs`. There is no `stress` feature. | Integration test |
| NFR-013 | The system MUST tolerate the disappearance of an external dependency mid-job (e.g., LibreOffice uninstalled) by failing the job cleanly with a typed error | Test passes with `pkill soffice` mid-conversion | Chaos test |
| NFR-014 | The system MUST support running a single instance only. A second invocation MUST send its arguments to the running instance via IPC and exit. | Pass: spawn 10 invocations in parallel, only 1 process exists | Concurrency test — **Met**: `tauri-plugin-single-instance` plus `cli/single_instance.rs`; the 10-way spawn test is not automated |

## 3. Security

| ID | Requirement | Target | Verification |
|---|---|---|---|
| NFR-020 | The application MUST install and run without administrator privileges for normal use | Install in `%LOCALAPPDATA%`, register in `HKCU` only | Manual QA on standard user account |
| NFR-021 | All released binaries MUST be code-signed with a valid certificate (EV cert preferred for Windows from day 1) | `signtool verify /pa` passes; Apple `codesign -dvv` passes; notarization passes | Release pipeline gate — **not met, by decision**: [ADR-0011](../adr/0011-release-channel.md) ships without Apple or Microsoft certificates; updates are signed with Pluryprint's own minisign key and every release lists SHA-256 checksums, and the release notes explain the Gatekeeper and SmartScreen warnings |
| NFR-022 | All update artifacts MUST be cryptographically signed; the client MUST refuse unsigned or wrongly-signed updates | Tampered update is rejected with `SignatureMismatch` error | Updater integration test |
| NFR-023 | The application MUST NOT make any unsolicited network requests except to the configured update server | Network audit shows only the update manifest's host (`github.com` release assets for the user's channel, or `PLURYPRINT_UPDATE_ENDPOINT`) | Wireshark capture in QA — not run. By construction: the converters and render workers get no network (dead proxies, request filters, a hardened LibreOffice profile), and a dependency install reaches winget, Homebrew or a web page only after the user's consent |
| NFR-024 | The shell extension COM DLL MUST be limited to: receive selection, serialize to file/stdin, spawn the main executable. It MUST NOT touch user data. | Code review + LOC budget ≤ 1500 | Code review checklist |
| NFR-025 | The application MUST validate all file paths from the shell extension as canonical absolute paths before passing to converters (no path traversal). | Fuzz test with `..\..\..\Windows\System32` payloads | Fuzz harness |
| NFR-026 | All third-party converter invocations MUST use **arg vector** APIs (`Command::new(...).args(...)`), never shell strings, to prevent command injection. | Code review; clippy lint `clippy::cmd_arg_string` enabled | Static analysis — every converter, worker and the winget/brew dependency installs use an argument vector; `clippy::cmd_arg_string` is not a real lint, so this is held by review and the `process` tests |
| NFR-027 | Secrets (signing private keys, future API tokens) MUST NEVER be committed; CI uses GitHub Secrets / OIDC. | **No gitleaks scan exists.** `ci.yml` reads no secret; `release.yml` reads the updater key from the `TAURI_SIGNING_PRIVATE_KEY` GitHub secret and stops when it is unset. The private key lives outside the repository. | — |

## 4. Usability

| ID | Requirement | Target | Verification |
|---|---|---|---|
| NFR-030 | A first-time user MUST be able to print a single PDF from the context menu in <30 seconds end-to-end after install | Usability test n=5, all complete | Recorded user test |
| NFR-031 | All defaults MUST be sane such that the application is useful without opening Settings | Verified by zero-config quick start in user test | Same |
| NFR-032 | Destructive actions (delete profile, clear history) MUST require confirmation with a clear "Undo" affordance for 5 seconds. | UI assertion in component tests | Vitest |
| NFR-033 | Error messages MUST be actionable: state what went wrong, what the user can do, and provide a link to logs. Vague "Something went wrong" is forbidden. | PR review checklist + lint rule for empty `.message` | Code review |
| NFR-034 | The system MUST provide keyboard shortcuts for all primary actions (print, cancel, switch profile, open queue). | Documented in the app | **Met** — see status section |
| NFR-035 | All UI text MUST avoid jargon when a plain-language equivalent exists; jargon allowed in Advanced Settings. | Editorial pass before each release | Release checklist |

## 5. Accessibility

| ID | Requirement | Target | Verification |
|---|---|---|---|
| NFR-040 | The GUI MUST conform to **WCAG 2.1 Level AA** | *Stated target.* **The stated verification does not exist**: there is no axe-core dependency and no Playwright — the E2E suite is WebdriverIO. | Partly covered by component tests |
| NFR-041 | The GUI MUST be fully operable with a keyboard alone (no mouse required) | Manual keyboard-only walkthrough in release checklist | Manual QA |
| NFR-042 | All interactive controls MUST have visible focus indicators with ≥3:1 contrast against background | Per-primitive Vitest assertions that a focus ring is painted (the `.mp-focus` class), plus manual contrast review. No axe-core. | Vitest + QA |
| NFR-043 | All non-decorative images and icons MUST have text alternatives (`aria-label`, `alt`, etc.) | Accessible-name assertions in the primitives' and screens' Vitest suites. No axe-core. | Vitest |
| NFR-044 | The application MUST be compatible with screen readers (NVDA on Windows, VoiceOver on macOS) | Manual smoke test with screen reader before each release | Release checklist |
| NFR-045 | Color MUST NOT be the only carrier of meaning; status uses icon + label + color. | Component review | Code review |

## 6. Internationalization

| ID | Requirement | Target | Verification |
|---|---|---|---|
| NFR-050 | All user-facing strings MUST be externalized; no string literals in JSX/Rust user-facing paths | Real layout is `src/i18n/locales/{lang}/{namespace}.json`, not a flat `i18n/{lang}.json`. **No lint enforces this**: the ESLint rule does not exist, and `#[deny(string_literal_in_user_text)]` is not a real attribute. Enforced by review only. | Code review |
| NFR-051 | The translation framework MUST support pluralization and interpolation | **Partly met, by a different mechanism.** `src/i18n/` is dependency-free — not ICU MessageFormat and not i18next. It supports `{placeholder}` interpolation and `_one`/`_other` plural keys selected by a numeric `count`, with English rules. Gendered forms and ICU date/number formatting are **not** supported. | `src/i18n/i18n.test.ts` |
| NFR-052 | The system MUST support adding a new language with no code changes | **Not met as written.** A language is a directory of eight namespace JSONs, and `src/i18n/index.ts` imports each one statically, so adding a language edits code. English is the source of truth and missing keys fall back to it. | — |
| NFR-053 | Layouts MUST handle text expansion of +50% without truncation or overlap | **Unverified.** No pseudolocale, no snapshot tests at 150%. | — |
| NFR-054 | The system SHOULD be RTL-ready by v2.0 (logical CSS properties, no hard-coded `left`/`right`) | RTL pseudolocale snapshot test | Future |

## 7. Maintainability

| ID | Requirement | Target | Verification |
|---|---|---|---|
| NFR-060 | Test coverage MUST be ≥70% for Rust code, ≥60% for TS code | **Measured locally, not gated**: `cargo llvm-cov` (not tarpaulin) and `pnpm test:coverage`; CI is kept to lint, typecheck and tests. Last measured: Rust 80.4% lines, TS 61.4% lines. | Local measurement |
| NFR-061 | All public Rust functions MUST have rustdoc with at least a one-line summary | **Enforced.** `#![deny(missing_docs)]` is set in both `src-tauri/src/lib.rs` and `shell-ext-win/src/lib.rs`, so every reachable `pub` item — functions, types, fields, variants, constants, modules — needs a doc comment or the crate does not compile. Checked on macOS and on the Windows target (the `cfg(windows)` code). | Compiler (every CI build) |
| NFR-062 | Documentation in this `docs/` folder MUST stay in sync with code; PRs that modify behavior MUST update affected docs | Reviewer checklist | Code review |
| NFR-063 | The CI MUST be green for a PR to be merged (no override without maintainer approval and reason in commit) | Branch protection rule | GitHub settings |
| NFR-064 | Build MUST be reproducible: same source + same toolchain version → byte-identical binary | `cargo build --locked --release` is reproducible | Quarterly verification |
| NFR-065 | Deps MUST be audited weekly via `cargo audit` and `pnpm audit`; high/critical vulnerabilities MUST be fixed within 7 days. | **No audit workflow exists.** No CI job runs on a schedule. Dependabot is the sole dependency automation. | — |

## 8. Portability

| ID | Requirement | Target | Verification |
|---|---|---|---|
| NFR-070 | A single codebase MUST support Windows (x64, ARM64), macOS (Apple Silicon, Intel) and, from 0.0.5, Linux (x86_64, ARM64) | **Windows and macOS: built for all four on release.** `release.yml` builds `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc` and refuses a release missing any of them. `ci.yml` tests natively on Windows x64 and macOS; the ARM64 Windows build is cross-compiled by `release.yml`. Tests run natively on Windows x64 and ARM64 (the VM) and on Apple silicon; no Intel Mac runs them. No universal binary. **Linux (decided in [ADR-0015](../adr/0015-linux-support.md), preview):** `release.yml` adds `x86_64-unknown-linux-gnu` on `ubuntu-22.04` and `aarch64-unknown-linux-gnu` on `ubuntu-22.04-arm`, each as AppImage and `.deb`, and refuses a release missing any of the eight packages and update signatures; `ci.yml` adds `ubuntu-22.04` (clippy, `--lib`; fmt runs on the Windows leg) and `linux.yml` runs the full suite natively on ARM64. No Linux build has run on real hardware yet. | CI + release |
| NFR-071 | Platform-specific code MUST be isolated to clearly named modules | **Met.** `src-tauri/src/platform/`: `windows.rs`, `cups.rs`, `macos/`. Windows is a single file rather than a directory; the isolation requirement is satisfied either way. | Code review |
| NFR-072 | On Linux the system MUST run on Ubuntu 22.04, Debian 12 and Fedora 38 or later (glibc 2.35 or later, WebKitGTK 4.1 2.42 or later), on X11 and Wayland, and print through CUPS | The builds are made on the `ubuntu-22.04` runners (glibc 2.35); wry 0.57 needs WebKitGTK 2.42 (it imports a 2.41.90 symbol; the `.deb` Depends on `libwebkit2gtk-4.1-0 (>= 2.42)`). Replaces the earlier "MUST NOT require Linux support in v1" ([ADR-0015](../adr/0015-linux-support.md)); preview until tested on real hardware | Release workflow + manual QA |

## 9. Observability

| ID | Requirement | Target | Verification |
|---|---|---|---|
| NFR-080 | All log entries MUST be structured (JSONL) with: timestamp (ISO 8601), level, target/module, message, optional fields | `tracing-subscriber` JSON formatter | Unit test on log format |
| NFR-081 | Logs MUST roll daily and by size; retention bounded | **Met, with a tightened policy**: `logging::Policy` rolls daily *and* at a size ceiling, then sweeps by `retain_days` (14) **and** `retain_files` (7) — whichever bound bites first wins. The sweep runs at startup and again on every roll, so a resident instance stays bounded without a restart. | `logging` unit tests |
| NFR-082 | Sensitive data (file contents, full paths if user opts) MUST never be logged at default level | Log audit at QA | Manual QA |
| NFR-083 | The system MAY include an opt-in crash reporter (Sentry-compatible) disabled by default | Documented in Settings | Privacy doc |

## 10. Privacy

See [privacy.md](../09-legal/privacy.md) for the full statement.

| ID | Requirement | Target | Verification |
|---|---|---|---|
| NFR-090 | The system MUST NOT collect telemetry by default. | No telemetry endpoint reachable in default config | Network audit |
| NFR-091 | File contents MUST never leave the user's machine | Integration test asserts no network egress during a print job (other than to the printer) | Wireshark in CI optional |
| NFR-092 | If a future opt-in telemetry feature is added, it MUST be **off by default** and require explicit affirmative consent. | Design review of telemetry doc | Governance |

## 11. Compliance

| ID | Requirement | Target | Verification |
|---|---|---|---|
| NFR-100 | The project MUST comply with AGPL-3.0 obligations: source disclosure, license text in distribution, copyright notices preserved. | `LICENSE`, `THIRD-PARTY-NOTICES.md` and `LICENSES/` present in the bundle (`src-tauri/tests/licence_bundle.rs`); About links the source, the license, the CLA and the notices. `THIRD-PARTY-NOTICES.md` is generated (`scripts/licenses/generate-notices.sh`, checked current by `release.yml`'s guard before any release build), SumatraPDF ships with its GPL-3.0 written source offer (`SOURCE-OFFER.txt`), and every release attaches the Pluryprint and SumatraPDF sources | Release checklist |
| NFR-101 | All bundled third-party components MUST be license-compatible with the AGPL-3.0. See [third-party-licenses.md](../09-legal/third-party-licenses.md). | License audit per release | Release checklist |
| NFR-102 | The project MUST publish a SECURITY.md with a private vulnerability disclosure channel. | File exists, channel works | Release checklist |

## Implementation status

Verified against the tree. Only NFRs whose status is non-obvious or has recently changed are listed; anything absent from this table has not been assessed and must not be assumed met.

### Met

| ID | Evidence |
|---|---|
| **NFR-012** | `wait-timeout` bounded wait + kill in the dispatcher; `src-tauri/tests/print_timeout.rs`. |
| **NFR-014** | `tauri-plugin-single-instance`; `src-tauri/src/cli/single_instance.rs`; a second launch hands its argv to the running instance instead of racing it over `queue.json`. |
| **NFR-034** | **Newly met.** `APP_SHORTCUTS` in `src/components/AppShell/MPAppShell.tsx` is a single table with three consumers that cannot disagree: `App` (owns the actions), `MPSidebar` (prints key caps), and Home (documents the full set). Six shortcuts cover the four primary actions the requirement names plus the two job entry points: `newJob` (N), `printFolder` (O), `print` (P), `switchProfile` (K), `openQueue` (J), `cancel` (`.`). `matchShortcut` requires exactly one platform modifier (⌘ on macOS, Ctrl elsewhere) with the other up, ignores auto-repeat, and refuses to fire while focus is in an `input`/`textarea`/`select`/`contenteditable`. `useAppShortcuts` also stands down on its own while **any** modal is mounted (the shared stack in `src/components/useModal.ts`: every `ConfirmDialog`, the add-printer dialog and the tour), so a screen's own confirmation owns the keyboard too. Platform-mod+P is `preventDefault`ed **always** (typing, modal open or not), so the webview's own print dialog of the app UI can never open; Ctrl/⌘+P applies the same `printBlocker` rule as the Print button, and the labels say what the handlers do (`printFolder` = "Add a folder to the job", `cancel` = "Cancel the current or next job"). Verified at two levels: component tests dispatch synthetic events at jsdom, and `e2e/specs/home.e2e.ts` asserts the layer is bound to a **real** window in the packaged app — the case jsdom cannot catch — including a test that Home documents every shortcut it implements. |
| **NFR-041** | Keyboard operability is asserted rather than assumed: quick actions are real `<button>`s (checked in `home.e2e.ts`, not decorative `div`s); every modal (`ConfirmDialog`, add-printer, first-run tour) goes through `useModalFocus` — initial focus, Tab/Shift+Tab trap, Escape (topmost modal only), focus restore — and is portalled over the whole window with the app root `inert`; disabled Settings rows use a disabled, `inert` `<fieldset>` so their controls leave the tab order; the Print Job list keeps focus across reorder and remove; and the shortcut layer above means no primary action requires a pointer. |
| **NFR-042** | Every focusable primitive paints a focus ring via `.mp-focus`, asserted per component (`Switch`, `Checkbox`, `Select` test suites) rather than left to a global stylesheet. The ≥3:1 contrast figure itself is still manual. |
| **NFR-043** | Accessible names asserted across the primitives (`Checkbox`, `Select`, `Switch`, `Input`, `Kbd` — including a spoken alternative for glyph-only keys) and at screen level: Print Job copies/page range/option groups, the add-by-IP fields, every profile-editor field (named by its caption, described by its error, `aria-invalid`), the Settings engine fields, and per-job queue actions ("Cancel report.pdf"). |
| **NFR-044** | Semantics rather than styling: `ConfirmDialog`, the add-printer dialog and the tour are labelled `role="dialog"` + `aria-modal`; `Switch`, `Checkbox` and `Kbd` expose their native semantics; `<html lang>` follows the UI locale; loading/empty notices and the About update result are live regions; Home's recent rows include the job status in their name. Screen-reader smoke testing remains manual. |
| **NFR-045** | State is never carried by colour alone: `Segmented` exposes selection through `aria-checked`, `Checkbox` reports indeterminate as `mixed`, Board cards print their status, and the selected printer/profile rows carry `aria-current`. Token contrast (text ≥4.5:1, control outlines ≥3:1, text on accent ≥4.5:1, both themes) is asserted by `src/styles/tokens.test.ts`. |
| **NFR-061** | **Newly met.** `#![deny(missing_docs)]` in `src-tauri/src/lib.rs` and `shell-ext-win/src/lib.rs`: an undocumented public item is a compile error, so the bar is held by every `cargo build`/`clippy`/`test` in CI rather than by review. Doc comments on `specta`-exported types also flow into `src/ipc/bindings.ts` as JSDoc. |
| **NFR-071** | `src-tauri/src/platform/{windows.rs, cups.rs, macos/}`. |
| **NFR-081** | `logging::Policy` — daily + size roll, swept at startup and on every roll by `retain_days` (14) and `retain_files` (7), whichever bites first. |
| **NFR-006** | `release.yml` fails a Windows installer over 20 MiB (15 MiB until 2026-10-10: the round-3 render worker, SVG stack, two Explorer DLLs and LibreOffice/HEIF install recipes took the x64 installer to 17.08 MiB and ARM64 to 15.49 MiB); the ARM64 installer measured 13.48 MiB on 2026-10-10. The x64 size is reported by every release build. |
| **NFR-020** | The per-user NSIS installer (`installMode: "currentUser"`) installs to `%LOCALAPPDATA%\Pluryprint` and registers the Explorer command under `HKCU` only. Verified on Windows 11 24H2 ARM64 from a medium-integrity session with UAC on: no elevation prompt, nothing under `HKLM`. Installing LibreOffice from the dependency dialog is the one action that asks for administrator approval, and the dialog says so first. |

### Not met, or unverified

| ID | Reality |
|---|---|
| **NFR-001 … NFR-005, NFR-007, NFR-008** | No performance measurement of any kind: no `benches/` directory, no `criterion`, no RAM assertion. The macOS `.dmg` is not measured, and there is no sparse package. |
| **NFR-021** | Nothing is code-signed (no Authenticode, no Developer ID), by decision ([ADR-0011](../adr/0011-release-channel.md)). |
| **NFR-022** | *Met on both sides, not yet exercised by a real update (0.0.1 preview, published on 2026-10-10, is the first release):* the app refuses unsigned or wrongly-signed updates (`UpdateSignatureInvalid`; unit-tested mapping, and checked by hand against a one-byte-tampered artifact served locally), and `release.yml` signs every updater artifact, then verifies each signature against the key compiled into the app and checks that a tampered copy fails before the draft is finished. |
| **NFR-027** | No gitleaks scan. The only secret in use is the updater key, read by `release.yml` from GitHub Secrets. |
| **NFR-040** | No axe-core, no Playwright. WCAG AA is approached through the per-component assertions listed above, not through an automated conformance scan. |
| **NFR-050** | Neither the ESLint rule nor the (non-existent) Rust attribute is in place; externalisation is enforced by review. |
| **NFR-051, NFR-052, NFR-053** | See the inline notes: no ICU, no gendered forms, adding a language touches code, no expansion testing. |
| **NFR-060** | Gated, but with `cargo llvm-cov` rather than tarpaulin, and the TS thresholds sit below the 60% target as a regression gate. |
| **NFR-064** | No reproducibility verification. |
| **NFR-065** | No `cargo audit` / `pnpm audit` workflow. |
| **NFR-070** | All four targets are built by `release.yml`, but no Intel Mac runs the tests, and there is no universal macOS binary. |
