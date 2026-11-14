# Testing Strategy

> **TL;DR.** Test pyramid: many fast Rust unit tests, a thin layer of Rust integration binaries that exercise real on-disk paths, and a small WebdriverIO end-to-end suite driving the built GUI (by hand on Windows; non-blocking on Linux ARM64 in `linux.yml`). Coverage is measured locally, not in CI: `cargo llvm-cov --fail-under-lines 70` on the Rust side and `pnpm test:coverage` on the Vitest side. Pull requests run the lean gate in `ci.yml` (Windows, macOS and Linux x64); `linux.yml` runs the full Linux suite, with a display, CUPS queues and the converters, after every merge to `main`. Requirement-tied tests carry an `FR-###` / `NFR-###` tag in their name.

This document covers what we test, how, with which tools, and what the acceptance bar looks like.

**Scope note.** Sections below distinguish what exists today from what is planned. The current suite prints nothing: there is no virtual-printer harness, no PDF-output comparison and no benchmark suite. The converters are tested where they are: `engine::convert::external_tests` pins the subprocess converters (LibreOffice, `sips`, `textutil`) with fake binaries and runs the real tools when they are installed, and `tests/render_worker.rs` and `tests/svg_worker.rs` drive the worker processes. Pandoc and a headless browser are not converters any more (ADR-0012; the browser fallback was removed in round 6). Everything marked *Planned* is a design commitment, not a description of the tree.

## The pyramid

```
                ┌──────────────┐
                │  Manual QA   │  Pre-release smoke; humans only
                ├──────────────┤
                │     E2E      │  WebdriverIO + tauri-driver (Win/Linux only)
                ├──────────────┤
                │ Integration  │  ~90 test fns across 8 binaries in src-tauri/tests/
                ├──────────────┤
                │     Unit     │  ~340 Rust fns co-located + ~405 Vitest cases
                └──────────────┘
```

(Counts are `#[test]`/`#[tokio::test]` functions under `src-tauri/src` and `src-tauri/tests`, plus `it(...)` cases across the 24 `*.test.ts(x)` files under `src/`. The standalone `shell-ext-win/` crate adds ~27 more Rust tests in its own lane.)

Most bugs are caught by unit tests (cheap, fast). A smaller layer of integration tests catches the ones that span process and filesystem boundaries. E2E tests catch GUI integration issues. Manual QA handles things that resist automation (does it look right? does it feel right?).

## Rust unit tests

### Tooling

- `cargo test` (the standard). This is the whole toolchain today — the crate has no test-only dependency beyond `tempfile` (a normal dependency, also used at runtime) and `tokio`'s `test-util` feature, which unlocks the virtual clock (`start_paused` / `time::advance`) the dispatcher's retry-backoff tests need to stay deterministic and instant.
- Optional: `cargo nextest` for faster local runs (3–5x speedup). Not used in CI.
- **Not used:** `insta` (snapshots), `proptest` (property tests), `mockall`, `wiremock`. None is in `src-tauri/Cargo.toml`. Adding one is a normal dependency decision, not a documented default.

### Conventions

- Co-locate with source: `#[cfg(test)] mod tests { ... }` at the bottom of the module file.
- One test per behavior. Don't pack 5 assertions into one test if you can split.
- Test name: `test_<scenario>_<expected_result>`. Examples:
  - `test_parse_profile_minimal_succeeds`
  - `test_parse_profile_missing_required_field_errors`
  - `test_enqueue_when_full_returns_full_error`
- Avoid `should_panic` tests except for verifying explicit `panic!()` messages. Prefer `assert!(matches!(...))` on `Result::Err`.

### Mocks vs fakes

We prefer **hand-rolled fakes** over mocking libraries.

- A **fake** is a real implementation of a trait that's simpler than production (e.g. an in-memory file store).
- A **mock** is a test-time stub of arbitrary call sequences (e.g. `mockall`).

Fakes are easier to debug, easier to maintain, and force good interface design. We use mocking libraries only when the fake would be larger than 200 LOC.

Example:

```rust
// engine/router.rs
trait PrinterAdapter: Send + Sync {
    async fn print(&self, file: &Path, opts: &PrintOpts) -> Result<()>;
}

// In tests:
struct FakePrinter {
    captured: Mutex<Vec<(PathBuf, PrintOpts)>>,
}

impl PrinterAdapter for FakePrinter {
    async fn print(&self, file: &Path, opts: &PrintOpts) -> Result<()> {
        self.captured.lock().unwrap().push((file.to_owned(), opts.clone()));
        Ok(())
    }
}

#[tokio::test]
async fn test_router_dispatches_pdf_to_pdf_adapter() {
    let printer = Arc::new(FakePrinter::default());
    let router = Router::new(printer.clone());
    router.print("test.pdf").await.unwrap();
    assert_eq!(printer.captured.lock().unwrap().len(), 1);
}
```

### Coverage target

**70% line coverage** on Rust code. It is measured locally, not in CI (the pull-request gate is kept lean, see [ci-cd.md](../07-build-and-release/ci-cd.md)); measure on macOS so the CUPS backend is compiled and exercised:

```bash
cargo llvm-cov --manifest-path src-tauri/Cargo.toml \
  --lcov --output-path lcov.info \
  --fail-under-lines 70
```

Locally, `cargo install cargo-llvm-cov --locked && cargo llvm-cov --html` gives the same numbers with a browsable report.

Last measured (macOS, when CI still ran it): **80.4% lines** (82.4% regions, 67.8% functions). The thinnest modules are `queue/mod.rs` (≈66%) and `update/mod.rs` (≈78%).

Coverage is a sanity check, not a goal. We don't write tests purely to chase coverage; we write tests for behaviors that matter.

Excluded from coverage:

- Win32 / Apple FFI calls (hard to unit-test; covered by integration tests).
- `unreachable!()` arms.
- Trivial getters / setters.
- Generated code (e.g. from `build.rs`).

## Rust integration tests

### Location

`src-tauri/tests/`. Each `.rs` file is a separate test binary, which means each gets its own clean environment. Twenty-eight binaries exist today:

```
src-tauri/tests/
├── bundle_identity.rs       # ADR-0009 bundle identity; the NSIS hooks compile with makensis
├── bridge_binary.rs         # FR-120/125/126: the real pluryprint-bridge over framed pipes, temp data dir, no launch
├── bridge_handoff_e2e.rs    # FR-124/125: the app's half of the cross-repo E2E with Pluryfact (ignored, opt-in)
├── bridge_bundle_contract.rs # FR-127: the bridge ships next to the app; the NSIS manifest and HKCU keys match bridge::register
├── bridge_protocol_contract.rs # FR-120: protocol v1 fixtures (shared with Pluryfact) round-trip and replay
├── cli_console_binary.rs    # FR-009: pluryprint-cli.exe returns the documented exit codes
├── cli_contract.rs          # The headless surface: subcommands, flags, exit codes (FR-008/009/112)
├── config_files_0_0_1.rs    # FR-100/026: config.toml and profiles.toml as 0.0.1 wrote them still load
├── engine_deps.rs           # FR-058: resolution order, remediation, install options
├── fetch_scripts.rs         # The SumatraPDF fetch scripts agree on version, hashes, licences
├── folder_expansion.rs      # Walking a dropped directory: limits, skip reasons (FR-004/005/059)
├── installer_hooks.rs       # FR-001/083: what the installer ships and registers, DLL per architecture
├── installer_i18n.rs        # FR-093: every hook message in English and Spanish, ASCII files
├── ipc_surface.rs           # Every Tauri command is registered; settings/profiles IPC
├── licence_bundle.rs        # ADR-0010: LICENSE, LICENSES/ and the notices are in the bundle
├── macos_services_l10n.rs   # Finder Services titles localized
├── print_quality_windows.rs # D16 on paused Windows queues (ignored, opt-in)
├── print_timeout.rs         # A hung child must not wedge the queue (NFR-012)
├── profile_roundtrip.rs     # FR-020…027: CRUD, built-ins, atomic save, 500 profiles
├── real_printer.rs          # CUPS option mapping on the Maecly_Hold queue (ignored, opt-in)
├── real_printer_windows.rs  # SumatraPDF spool, tracking and cancel on a paused queue (ignored, opt-in)
├── release_pipeline.rs      # release.yml / ci.yml contract: guard, signatures by asset id, size budget
├── render_worker.rs         # FR-055/056/057 through the built `--worker render-pdf`
├── settings_persist.rs      # FR-100…103: TOML, corrupt-file fallback, atomic write, schema_version
├── svg_worker.rs            # FR-052a through the built `--worker svg`
├── tauri_conf_contract.rs   # tauri.conf.json and the main capability
├── third_party_notices.rs   # THIRD-PARTY-NOTICES.md covers every direct dependency
└── update_startup.rs        # The startup update check, offline and error paths
```

There is **no** `pdf_print.rs`, `queue_concurrency.rs` or `shell_handoff.rs`. Print-output verification is unbuilt (see *Print harness — planned* below); queue concurrency and the dispatcher are covered in-module under `src-tauri/src/queue/`; the shell handoff schema is covered by `shell-ext-win/tests/handoff_roundtrip.rs` on the producer side and `src-tauri/src/cli/handoff.rs` on the consumer side.

Test functions that pin a requirement are **named after it** — `fr_020_crud_roundtrip`, `fr_102_atomic_write` — so a failure names the requirement it breaks. Domain logic is unit-tested in-module (`#[cfg(test)]`); these integration binaries exercise the real on-disk read/write paths through a tempdir.

### Tooling

- Standard `cargo test`.
- `tempfile` for ephemeral directories.
- Nothing else. In particular there is **no HTTP mock server**: the update client's parsing, comparison and error paths are unit-tested against fixture payloads, and the one test that really talks to `api.github.com` is `#[ignore]`d (see *Ignored tests* below).

### Virtual printer — planned, not built

**Nothing in the suite prints today.** The default suite spawns neither SumatraPDF nor `lp`; the print backends are covered as *pure argument construction* — `sumatra::print_settings_string`, `sumatra::sumatra_args`, `cups::lp_opts`, `cups::lp_args` are pure functions with co-located unit tests, and only `spool`/`cancel` touch a process, `cfg`-gated per platform.

The one exception is opt-in: **`src-tauri/tests/real_printer.rs`** (Unix only, every test `#[ignore]`d *and* gated on `MAECLY_REAL_PRINTER=1`) spools through the engine's public `engine::print::spool` to a CUPS queue that **holds every job** (`MAECLY_HOLD_QUEUE`, default `Maecly_Hold`, created with `-o job-hold-until-default=indefinite`). It reads each job back with `ipptool … get-job-attributes.test`, asserts the IPP attributes the mapping should produce (`copies`, `sides`, `media`, `ColorModel`/`print-color-mode`, `page-ranges`, `landscape`/`orientation-requested`, `media-source`/`InputSlot`, `print-quality`/`cupsPrintQuality`, `fit-to-page`) and cancels the job, also on panic. It refuses to submit unless `lpoptions -p <queue>` shows `job-hold-until=indefinite`, and asserts every job is `pending-held` before reading it. Nothing reaches paper. Without the env var the tests return immediately, so a plain `cargo test -- --ignored` passes them as no-ops. Each job is spooled with the queue's real capabilities (`platform::queue_of`), as the dispatcher does. Orientation goes out as `orientation-requested=3|4`; the check accepts that or an older `landscape` boolean. Observed on CUPS 2.x (macOS): when both quality spellings are sent, the queue keeps `cupsPrintQuality` and drops `print-quality`. `linux.yml` runs this suite, and the ignored `engine::print::cups` tests, on every merge against a `Maecly_Hold` queue on the cups-pdf backend (CUPS 2.4, Ubuntu 22.04 ARM64), so a held job that were ever released would land as a PDF, not on paper. `fr_032_spool_reports_the_cups_job_id` pins the request-id parse on a host whose `lp` prints its success line in Spanish.

The design when we do build it:

- **Windows:** "Microsoft Print to PDF" (preinstalled), output path set via the job's `OutputFile` parameter.
- **macOS:** `cups-pdf` (Homebrew), configured to write `.pdf` to a known location.
- **Linux:** `printer-driver-cups-pdf`. `linux.yml` already creates such a queue, `PDF`, as the runner's default destination (output under `~/PDF`), next to the `Maecly_Hold` hold queue; nothing compares its output yet.

Verification would be page count + text layer (deterministic, fast), with an opt-in visual diff at 150 DPI behind an env var. That needs a PDF-reading crate in `dev-dependencies` (`lopdf` is already a normal dependency and can read page counts) and a fixtures tree — neither of which exists yet. **Do not** cite this section as if the harness were available.

### Subprocess fixtures — not applicable yet

Only `linux.yml` installs converters: LibreOffice (`libreoffice-writer-nogui`), ImageMagick and libheif's `heif-convert` (`libheif-examples`; Ubuntu 22.04 has libheif 1.12, which ships `heif-convert` but not `heif-dec`). `ci.yml` installs none. The converters that shell out (LibreOffice, `sips`, and the render worker) are covered at three levels:

- **Fake binaries** (`external_tests::fakes`, Unix only): `/bin/sh` scripts standing in for `soffice`, the browser, the render worker and `sips`. They pin the argument vector that is actually spawned, the "exit 0 but no PDF" failure, the timeout kill of the whole process tree, the browser profile seeding, scratch-folder cleanup, and that HTML and Markdown go to the worker rather than a browser or Pandoc. They need nothing installed.
- **Real tools** (`external_tests::fr_05x_real_*`): convert fixtures generated in-test (an RTF, a DOCX made by LibreOffice, and on macOS a HEIC made by `sips`), plus a LibreOffice document that must not reach a local listener. Each test locates its tool with the engine's own resolver and **returns early with a `SKIPPED` note** when the tool is absent, instead of using `#[ignore]`. A developer machine with the tools runs them on every `cargo test`, and a bare CI runner stays green.

- **The real workers** (`tests/render_worker.rs`, `tests/svg_worker.rs`): the built binary's `--worker render-pdf` and `--worker svg`, with pagination, paper sizes, relative images, a hostile page, Markdown, UTF-16 text, RTF, every SRI XML fixture and parallel workers. They need only the OS web view. `PLURYPRINT_XML_SAMPLES=<dir>` keeps the PDFs the XML test prints, for review.

### XML fixtures and HTML snapshots

`tests/fixtures/sri-xml/` holds fictitious SRI receipts — issuer RUC `0999999999001`, invented names, access keys with a valid check digit that describe nothing real — of the six types in their published versions, in the three file shapes (`<autorizacion>`, SOAP response, bare), plus a generic XML. Never commit a real receipt: regenerate them with `python3 scripts/fixtures/sri_xml_fixtures.py src-tauri/tests/fixtures/sri-xml`. `engine::convert::xml::tests` compares the page each one produces with `tests/fixtures/sri-xml/snapshots/*.html`; after an intended change, rerun with `PLURYPRINT_BLESS=1 cargo test --lib xml` and review the diff like code. The same module holds the hostile-input table (billion laughs, external DTD and entities, 10,000 levels, control characters, broken CDATA, markup in every field) and a mutation test that must never panic; `code128` is checked against reference encodings from an independent implementation.

On Linux ARM64 the real-tool tests run on every merge (`linux.yml`). Installing LibreOffice in CI on Windows and macOS as well (`choco install libreoffice-fresh`, `brew install --cask libreoffice`) would turn their remaining skips into real coverage. That step is still owed.

### Ignored tests

These tests are `#[ignore]`d because they reach outside the process:

| Test | Why | How to run |
|---|---|---|
| `update::tests::live_manifest_check` | Reads the published update manifests on `github.com`; network trouble must not fail a merge | `cargo test -- --ignored live_manifest_check` |
| `platform::cups::tests::add_and_remove_real_queue` | Creates a real CUPS queue via `lpadmin` | `cargo test -- --ignored add_and_remove` |
| `platform::cups::tests::list_printers_smoke` | Shells out to the host's CUPS install; the result depends on the developer's real queues | `cargo test -- --ignored list_printers_smoke` |
| `engine::print::cups::tests::maecly_hold_job_is_tracked_as_held_then_canceled` | Needs the local CUPS hold queue `Maecly_Hold` | `cargo test -- --ignored maecly_hold` |
| `platform::windows::tests::discover_printers_smoke`, `platform::windows_dnssd::tests::browse_smoke` | Wait for the DNS-SD browse window on a real Windows network | on the Windows VM, `cargo test -- --ignored smoke` |
| `engine_deps::fr_058_print_this_hosts_report` | Diagnostic: prints the host's dependency report | `cargo test --test engine_deps -- --ignored --nocapture` |
| `tests/real_printer.rs` (6 tests) | Submits held jobs to a real CUPS queue and reads their IPP attributes back (see *Virtual printer* above); a no-op unless `MAECLY_REAL_PRINTER=1` | `MAECLY_REAL_PRINTER=1 cargo test --test real_printer -- --ignored --test-threads=1` |
| `tests/bridge_handoff_e2e.rs` | Reads the inbox and handoff a real Pluryfact run left behind (see *Cross-repo E2E with Pluryfact* below); a no-op unless `PLURYPRINT_E2E_DATA_DIR` is set | run by Pluryfact's `bun run e2e:pluryprint`, or `PLURYPRINT_E2E_DATA_DIR=<dir> cargo test --test bridge_handoff_e2e -- --ignored --nocapture` within 60 s of the handoff |
| `tests/real_printer_windows.rs` (2 tests), `tests/print_quality_windows.rs` (10 tests) | Spool to **paused** Windows queues and read the jobs back; a no-op unless `MAECLY_REAL_PRINTER=1` | see [e2e/README.md](../../e2e/README.md#real-spooler-suites-opt-in-never-print); `scripts/vm/print-quality-e2e.ps1` sets up the queues |

`ci.yml` does not run them: they need the network, real queues or a real printer, and must never fail a pull request. The CUPS ones are the exception after a merge: `linux.yml` has a hold queue, and runs `cargo test --lib -- --ignored cups` and `cargo test --test real_printer -- --ignored --test-threads=1` with `MAECLY_REAL_PRINTER=1`. Run the rest by hand with the commands above; `cargo test -- --ignored` runs *only* the ignored ones (the rest already ran in CI), **not** `--include-ignored`.

## Frontend unit tests

### Tooling

- **Vitest** (Vite-native test runner; faster than Jest for our setup), configured under `test:` in `vite.config.ts` — jsdom environment, globals on, `src/test/setup.ts` as the setup file, `include: ['src/**/*.{test,spec}.{ts,tsx}']` and `exclude: [..., 'e2e/**']`.
- **React Testing Library** (`@testing-library/react`) plus `@testing-library/jest-dom` matchers.
- **`@testing-library/user-event`** for simulating user interactions.
- **`mockIPC` from `@tauri-apps/api/mocks`** for the backend boundary — see *Mocking Tauri IPC* below.
- **Not used: MSW.** The frontend makes no HTTP requests of its own; everything crosses the Tauri IPC bridge, which `mockIPC` intercepts directly. There is no `msw` dependency in `package.json` and none is needed.

### Conventions

- Test files co-located: `Button.tsx` → `Button.test.tsx`.
- One `describe` block per component or function.
- Test names in plain English: `it("disables itself when isLoading is true", ...)`.
- Avoid testing implementation details. Test what the user (or component consumer) sees.

```tsx
// src/components/Button.test.tsx
import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Button } from './Button';

describe('Button', () => {
  it('calls onClick when clicked', async () => {
    const onClick = vi.fn();
    render(<Button onClick={onClick}>Print</Button>);
    await userEvent.click(screen.getByRole('button', { name: 'Print' }));
    expect(onClick).toHaveBeenCalledOnce();
  });

  it('is disabled when loading', () => {
    render(<Button isLoading>Print</Button>);
    expect(screen.getByRole('button')).toBeDisabled();
  });
});
```

### Mocking Tauri IPC

`src/test/setup.ts` is deliberately thin — it wires `@testing-library/jest-dom` and shims `localStorage` for jsdom's opaque origin. It does **not** globally stub IPC. Instead, each test that needs the bridge installs a **per-test `mockIPC`** from `@tauri-apps/api/mocks` and tears it down with `clearMocks()` in `afterEach`. The handler is usually stateful, so a store's reload-after-write sees a realistic post-mutation snapshot:

```ts
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks';

afterEach(() => clearMocks());

it('FR-022: the default flag moves after set_default_profile', async () => {
  let list = fixtureProfiles;
  mockIPC((cmd, args) => {
    switch (cmd) {
      case 'list_profiles': return list;
      case 'set_default_profile':
        list = list.map((p) => ({ ...p, default: p.id === (args as any).id }));
        return null;
    }
  });
  // …drive the store, assert the reloaded list…
});
```

Event-driven stores need one extra step. The tauri-specta `events` object drives the event plugin over IPC, which `mockIPC` would also intercept, so those tests `vi.mock('@/ipc/bindings')` to swap **just `events`** for a controllable fake (keeping the real `commands` so `mockIPC` still serves command invokes). `vi.hoisted` makes the fake registry available inside the hoisted `vi.mock` factory. This is the pattern in `queueStore.test.ts`.

### Coverage target

**60% line coverage** on TypeScript is the target. Lower than Rust because UI code has more that is hard to test (rendering details, Tailwind utility usage). We don't chase the number; we cover behaviors.

Measured by `pnpm test:coverage` (`vitest run --coverage`) with the `@vitest/coverage-v8` provider — reporters `text`/`html`/`lcov`, output under `coverage/`, non-product files (generated `bindings.ts`, `src/test/**`, `src/main.tsx`, configs) excluded from the denominator (`vite.config.ts`).

**Coverage is informational.** CI runs `pnpm test` without coverage; `pnpm test:coverage` measures it locally. Last measured: lines/statements 61.4%, functions 71.4%, branches 79.8%.

## End-to-end tests

### Tooling

- **WebdriverIO** as the test framework (`@wdio/cli` + the Mocha framework adapter), configured in `e2e/wdio.conf.ts`.
- **`tauri-driver`** (a cargo binary, `cargo install tauri-driver --locked`; `linux.yml` pins 2.1.0) as the WebDriver bridge to the Tauri webview. On Windows it proxies to the Edge WebDriver (`msedgedriver.exe`) matching the installed WebView2, passed via the `NATIVE_DRIVER` env var. On Linux it proxies to WebKitGTK's own `WebKitWebDriver` (Debian/Ubuntu package `webkit2gtk-driver`), found on `PATH`; `NATIVE_DRIVER` is not needed.
- Tests run against an actual built `pluryprint(.exe)` binary (`pnpm tauri build --debug`), not the dev server. Run locally with `pnpm test:e2e`.

### Platform support — Windows/Linux only

`tauri-driver` supports **Windows and Linux only**; **macOS WKWebView exposes no WebDriver endpoint**, so the WebdriverIO suite cannot run on macOS. macOS is covered instead by the Rust unit/integration suite, the Vitest suite, the browser harness below, and manual QA.

### Browser harness (macOS, or any host without a built binary)

`pnpm dev:e2e` (`vite --mode e2e`; `PLURY_E2E_STUB=1 pnpm dev` is equivalent) serves the real frontend at `http://localhost:1421` with a **fake Tauri host** injected ahead of the app. The `pluryE2eStub` plugin in `vite.config.ts` (`e2e/browser/vite-plugin.ts`) is `apply: 'serve'` and inert without the flag, so neither `pnpm dev` nor any production build ever contains it. `e2e/browser/ipc-stub.ts` installs `window.__TAURI_INTERNALS__` (invoke, callbacks, the event plugin's listen/unlisten/emit, the dialog plugin, `convertFileSrc`, webview metadata), so `isTauri()` is true and the screens run their real code paths. Every command lands on `e2e/browser/fake-backend.ts`: in-memory printers, profiles (CRUD, default guard), settings (with the server's engine limits), folders and a queue seeded from `src/test/fixtures.ts`. Submitted jobs advance on a timer (queued → converting → converted → printing → done, a file named `*fail*` fails), emitting `queue-updated` and `job-progress-event`. A command it does not model rejects with `Internal`, never resolves `undefined`.

Drive it with any browser automation (Chrome DevTools MCP, Playwright) through `window.__PLURY_E2E__`: `backend` (state, `tick()`, `openPaths` for the next file picker), `forwardSelection(sel)`, `dropFiles(paths)`, `emit(event, payload)`, `calls()`. URL flags: `?e2e-seed=1` (fixture queue with every job state), `?e2e-tick=0` (manual ticks), `?e2e-onboarded=1` (skip onboarding). It proves GUI ↔ IPC-contract behaviour, not the backend; the harness's own contract tests live in `e2e/browser/ipc-stub.test.ts` and run with `pnpm test`.

### Cross-repo E2E with Pluryfact (macOS, Linux)

The browser bridge (FR-120…127, ADR-0016) is tested end to end from the Pluryfact repository, against this repository's real binaries: Pluryfact's `bun run e2e:pluryprint` (`scripts/e2e-pluryprint.ts` there) drives the extension in Playwright's Chromium, and `src-tauri/tests/bridge_handoff_e2e.rs` here checks what the app would do with the result. Nothing is printed and no window opens: the bridge runs with `PLURYPRINT_BRIDGE_NO_LAUNCH=1`, which writes the inbox and the handoff and stops there.

```sh
cd src-tauri && cargo build --bin pluryprint-bridge --bin pluryprint-cli
cd ../../pluryfact
# Manifest written by the script, bridge data in a temporary folder:
PLURYPRINT_BRIDGE_BIN=../pluryprint/src-tauri/target/debug/pluryprint-bridge \
  PLURYPRINT_BRIDGE_NO_LAUNCH=1 bun run e2e:pluryprint
# Real registration: `pluryprint-cli bridge install` under a temporary HOME:
PLURYPRINT_REGISTRO=cli PLURYPRINT_BRIDGE_BIN=../pluryprint/src-tauri/target/debug/pluryprint-bridge \
  PLURYPRINT_BRIDGE_NO_LAUNCH=1 bun run e2e:pluryprint
```

What one run covers:

1. **Registration** (`PLURYPRINT_REGISTRO=cli`): the CLI and the bridge are copied next to each other into a temporary folder, `pluryprint-cli bridge install --browser chrome --extra-origin chrome-extension://<test id>/ --json` runs with `HOME=<temp>/home`, and `bridge status --json` must report Chrome registered, pointing at that bridge, binary present. Chromium's profile is that HOME's Chrome folder, so the browser reads the manifest exactly where the CLI wrote it, and it hands that HOME to the bridge, which accepts the test origin only because the installed manifest lists it. The user's real manifests and data folder are compared before and after and must be unchanged. Without the variable the script writes the manifest itself and uses `PLURYPRINT_BRIDGE_DATA_DIR`.
2. **Protocol**: "not installed" before registration, `hello`, `listProfiles`, then two "Print a ZIP…" jobs of a fictitious download (RUC `0999999999001`): RIDE PDFs with the XML of the receipt that has no RIDE plus the TXT report, and "XML only" (four SRI XML, one of them a withholding receipt, plus the report).
3. **Inbox**: per job, every file in `inbox/<jobId>/` is byte for byte its entry in the ZIP the page was given, in print order, and the handoff says `source: "pluryfact"`, `action: "configure"`, a label without the RUC and the inbox paths in order.
4. **The app's side** (`bridge_handoff_e2e`, run by the script right after each job because a handoff is only honoured for 60 s): the argv `launch::gui_args` builds classifies as a context-menu launch and passes `cli::selection_from_at`; under the temporary HOME the app's own handoff folder is the bridge's (`PLURYPRINT_E2E_SAME_HOME=1`); `ipc::jobs::inspect_files` reports every file printable; and `convert_to_pdf`, with the real render worker, passes the PDFs through and turns each XML into its RIDE-like page and the report into a text page. `PLURYPRINT_E2E_OUT` keeps those PDFs.

Not covered: the app actually opening the Print Job screen from the handoff (`PLURYPRINT_BRIDGE_NO_LAUNCH=0` with an installed app), spooling, Windows (registry registration and the NSIS hooks; checked by `bridge_bundle_contract.rs` and `installer_hooks.rs` only), and Edge/Brave. Last run: 2026-11-14 on macOS, both modes green.

### Coverage scope

E2E specs live in `e2e/specs/` and are named **`*.e2e.ts`**, not `*.spec.ts`. The extension is load-bearing: Vitest's default `{test,spec}` glob would otherwise pick them up and try to run a WebdriverIO suite inside jsdom. `wdio.conf.ts` matches them with `specs: [path.resolve(here, 'specs', '**', '*.e2e.ts')]`.

There is one spec per screen — seven files plus a shared `helpers.ts` (dismiss onboarding in either language, navigate the sidebar, and `forceEnglishUi`, which the `before` hook in `wdio.conf.ts` runs once per session because the first launch follows the OS language, FR-093):

| Spec | Covers |
|---|---|
| `home.e2e.ts` | Home advertises only formats the engine really accepts; the global shortcut layer is bound to a real window (NFR-034); quick actions are focusable buttons (NFR-041); recent activity is fed by a live `queue_status` |
| `print-job.e2e.ts` | Add a file, submit the batch, see it in the Queue (the queue is paused around the spec so the job never reaches the host's real default printer, then cancelled and resumed); an unsupported file is refused with a visible reason (FR-005); a dropped folder asks before recursing (FR-004) |
| `queue.e2e.ts` | The live queue renders instead of an "unavailable" notice; switching List / Board / Timeline views; the queue controls, including an honest "Clear done" |
| `printers.e2e.ts` | Enumeration comes from the host spooler, not mock data; the add-printer methods are tabs and honour the Windows transport limit |
| `profiles.e2e.ts` | Create a custom profile and make it the default; unsaved edits survive leaving and returning to the screen |
| `settings.e2e.ts` | Theme switch applies the dark class; a changed setting survives a relaunch with the local cache cleared (FR-100); language switching re-renders and persists (FR-094) |
| `about.e2e.ts` | Semver version string; license/release/docs/issue links (FR-110); the live log directory (FR-113); React/Tauri versions and host platform; the update check is enabled inside the real host and the IPC-unavailable fallback never shows |

There is no `launch.e2e.ts` or `navigation.e2e.ts`: "the binary starts and paints" is what every other spec's first step proves, and navigation is exercised by `helpers.navigateTo` in every other spec.

What E2E tests don't cover (we have lower-level tests for these): component-level UI behavior (Vitest), print correctness (Rust unit tests over the flag builders), and the IPC contract (`src-tauri/tests/ipc_surface.rs`).

### Test structure

```ts
// e2e/specs/about.e2e.ts — WebdriverIO + Mocha globals
import { $, expect } from '@wdio/globals';
import { dismissOnboarding, navigateTo } from './helpers.js';

describe('About', () => {
  beforeEach(async () => {
    await dismissOnboarding();
    await navigateTo('About'); // takes the sidebar label
  });

  it('shows a semver version string', async () => {
    await expect($('body')).toHaveText(/\d+\.\d+\.\d+/);
  });
});
```

### CI integration

Not in the pull-request gate: `ci.yml` stays lean, because driving a real GUI on a hosted runner is slow and flake-prone. `linux.yml`'s `e2e` job runs the whole suite after every merge to `main`, weekly and by hand, on `ubuntu-22.04-arm`: it builds a debug binary with the frontend embedded (`pnpm tauri build --debug --no-bundle --config src-tauri/tauri.ci.conf.json`), installs `tauri-driver` and `webkit2gtk-driver`, creates a cups-pdf queue for the printers spec, and runs `pnpm test:e2e` under `xvfb-run` and its own session bus; failure screenshots are uploaded as the `e2e-linux-artifacts` artifact. The job is **`continue-on-error`** for now: the specs were written and verified against WebView2 on Windows, and WebKitWebDriver has gaps of its own, so its first runs are expected to find spec assumptions rather than app bugs. Measured before it first ran, in an Ubuntu 22.04 ARM64 container with WebKitGTK 2.50: the app starts, renders and answers WebDriver; `home`, `print-job`, `profiles` and `settings` pass; `about`, `printers` and `queue` time out (the 60 s test limit) and are still to be triaged. Two Linux-only fixes made the sessions open at all: `forceEnglishUi` waits for the onboarding tour, which appears after the sidebar on a slow host, and `afterSession` kills the app, which WebKitWebDriver leaves running (the single-instance plugin then swallowed the next session's launch). It becomes blocking once it has passed on `main` for a few weeks. On Windows, run the suite locally with `pnpm test:e2e` (see [e2e/README.md](../../e2e/README.md)); the Windows VM scripts under `scripts/vm/` cover the installed app.

### The full job list in `ci.yml`

| Job | Runner | What it does |
|---|---|---|
| `frontend` | ubuntu | `pnpm lint`, `typecheck`, `format:check`, `test` |
| `backend` | matrix `windows-latest`, `macos-latest`, `ubuntu-22.04` | `cargo fmt --check` (Windows only, both crates) and `cargo clippy --all-targets -- -D warnings` everywhere; on Windows the full `cargo test` (which is also the `src/ipc/bindings.ts` drift gate) and clippy + `cargo test` for `shell-ext-win/`; on macOS and Linux `cargo test --lib`, the unit tests that hold their cfg-gated code (CUPS, Finder, `/proc`). Linux first installs `libwebkit2gtk-4.1-dev libgtk-3-dev libdbus-1-dev librsvg2-dev patchelf file` |

Both are blocking. Triggers: `pull_request`, `push` to `main`, `workflow_dispatch`. The Linux leg runs on `ubuntu-22.04`, the image the Linux packages are built on (glibc 2.35, WebKitGTK 2.42 or later from `jammy-updates`), x64 only; its unit tests need no display. The cargo cache is keyed on `runner.os` and `runner.arch`.

### `linux.yml`: the full Linux run

| Job | Runner | What it does |
|---|---|---|
| `test` | `ubuntu-22.04-arm` | Installs the build libraries, Xvfb, CUPS with `printer-driver-cups-pdf`, LibreOffice (`libreoffice-writer-nogui`), ImageMagick and `libheif-examples`; creates the `PDF` (default) and `Maecly_Hold` queues; then clippy, the full unfiltered `cargo test` under `dbus-run-session -- xvfb-run`, and the real-spooler suites (`--lib -- --ignored cups`, `--test real_printer -- --ignored --test-threads=1`) with `MAECLY_REAL_PRINTER=1` |
| `e2e` | `ubuntu-22.04-arm` | The WebdriverIO suite through `tauri-driver` and `WebKitWebDriver` (see *CI integration* above); `continue-on-error` |

Triggers: `push` to `main`, every Monday, `workflow_dispatch`; **never** `pull_request`, so a pull request waits only for `ci.yml`. ARM64 is the architecture `ci.yml` does not cover, so between them both Linux architectures are compiled and tested. The workflow builds no package and reads no secret. WebKitGTK runs with `WEBKIT_DISABLE_COMPOSITING_MODE=1` and `LIBGL_ALWAYS_SOFTWARE=1`, since Xvfb has no GPU.

### Linux traps the suite already hit

- **procps-ng `kill` and process groups.** On Ubuntu 22.04 (procps-ng 3.3.17), `kill -KILL -<pgid>` is read as a second signal option and signals the *caller's* group: the first converter killed at its timeout took the test runner, and cargo, with it, and `kill -0 -<pgid>` said every group was alive. The engine always writes `kill -KILL -- -<pgid>` and `kill -0 -- -<pgid>` (`engine::convert::process`); new code (and tests) that signal a group must do the same. `process::reason_tests::kill_tree_ends_the_childs_group_and_the_probe_sees_it_gone` pins it.
- **Process-group memory.** Linux has no call that lists a process group, so `group_memory_in_use` scans `/proc/*/stat` for members (the fifth field, read after the last `)`) and sums their resident sets. A `CapScope::Tree` helper that leaves the group (`setsid`) is not counted.
- **Zombies count as group members.** `kill -0 -- -<pgid>` succeeds while a killed helper is an unreaped zombie. The hosted runners have an init that reaps orphans; a container needs `docker run --init`, or the `fr_058` linger and cancel tests fail. `shell-ext-win/` gets its own commands because it is deliberately **not** a Cargo workspace member (its release profile must keep `panic = "unwind"` so `catch_unwind` at the COM boundary works, which `src-tauri`'s `panic = "abort"` would destroy), so `--manifest-path src-tauri/Cargo.toml` never touches it. [ci-cd.md](../07-build-and-release/ci-cd.md) lists what moved to `release.yml` and what is local only.

## Print mock harness — planned

> **Status: not implemented.** Nothing described in this section exists in the tree. There is no `src-tauri/tests/fixtures/`, no `src-tauri/tests/common/`, and no PDF-comparison helper. It is kept as the design to build against when print-output verification lands; treat every path and crate name below as a proposal.

### What it would do

- Provides a deterministic virtual printer (Microsoft Print to PDF on Windows, CUPS-PDF on macOS).
- Captures every print job as a `.pdf` file in a known location.
- Provides helper functions to compare output against fixtures.

### Fixtures (proposed layout)

`src-tauri/tests/fixtures/` **does not exist**. It would contain:

```
fixtures/
├── inputs/
│   ├── single-page.pdf
│   ├── multi-page.pdf
│   ├── encrypted.pdf
│   ├── small.docx
│   ├── chart.xlsx
│   ├── slide.pptx
│   ├── readme.md
│   ├── photo.jpg
│   └── webpage.html
└── expected/
    ├── single-page.pdf       # What "Microsoft Print to PDF" produces from inputs/single-page.pdf
    ├── single-page.png       # Rendered at 150 DPI for visual diff
    └── ...
```

Fixtures would be committed (Git LFS for files > 1 MB). Note that the input list above spans Office, HTML and Markdown, which the engine cannot print at all today (see [supported-formats.md](../03-requirements/supported-formats.md)) — the fixture set arrives with the converters, not before them.

### Comparison strategy (proposed)

```rust
// src-tauri/tests/common/mod.rs
pub fn assert_pdf_matches(actual: &Path, expected: &Path) -> Result<()> {
    // 1. Page count
    let actual_pages = pdf_extract::extract_pages(actual)?.len();
    let expected_pages = pdf_extract::extract_pages(expected)?.len();
    assert_eq!(actual_pages, expected_pages, "page count mismatch");

    // 2. Text layer
    let actual_text = pdf_extract::extract_text(actual)?;
    let expected_text = pdf_extract::extract_text(expected)?;
    assert_eq!(actual_text, expected_text, "text mismatch");

    // 3. Visual diff (optional, expensive)
    if std::env::var("RUN_VISUAL_DIFF").is_ok() {
        let actual_png = render_pdf_to_png(actual, 150)?;
        let expected_png_path = expected.with_extension("png");
        let diff_pct = image_compare(&actual_png, &expected_png_path)?;
        assert!(diff_pct < 0.5, "visual diff {}% exceeds 0.5%", diff_pct);
    }
    Ok(())
}
```

We default to **text + page count** comparison because it's deterministic and fast. Visual diff is opt-in via env var because it's slower and can flag minor rendering differences (anti-aliasing, font hinting) that don't matter functionally.

## Performance / regression tests

### Benchmarks — none exist

There is **no `src-tauri/benches/` directory and no `criterion` dependency.** The performance NFRs (NFR-001…NFR-005) are therefore unverified by automation today.

If and when benchmarks are added, the plan is Criterion under `src-tauri/benches/`, run locally before any change to the queue or engine and reported in the PR — not in CI, where shared runners are too noisy to compare against a baseline. Until then, do not claim a bench "confirms" anything.

### Regression detection

For correctness regressions, the Rust suite plus the bindings-drift gate is the safety net. There is no performance regression net at all — this is a known gap, not a policy.

## Manual QA

Manual QA is for things that resist automation:

- "Does the print job feel responsive?"
- "Is the Spanish translation natural?"
- "Does the UI look right at 200% Windows display scaling?"
- "Does the right-click menu appear in the right position in File Explorer?"
- "Does an EV-signed installer trigger no SmartScreen warning?"

We have a manual QA checklist (in [release-checklist.md](./release-checklist.md)) that runs pre-release on:

- Windows 10 22H2.
- Windows 11 23H2.
- macOS 13, 14, 15 (M5+).
- Linux (preview): Ubuntu 22.04 and 24.04 (GNOME, Wayland and X11), Debian 12 and Fedora 38 or later, the `.deb` where it applies and the AppImage everywhere, x64 and ARM64 (a VM is enough for ARM64).
- At least 3 different physical printers (laser, inkjet, network).

The checklist covers ~30 minutes of focused testing per platform.

## Generated-artifact drift gates

We do not use snapshot testing (`insta` is not a dependency, and `src-tauri/tests/snapshots/` does not exist). What we do have is one **generated-file drift gate**, which serves a similar purpose for the artifact that matters most:

`src-tauri/tests/ipc_surface.rs` regenerates `src/ipc/bindings.ts` from the tauri-specta command surface during `cargo test` and fails when the committed copy is stale. Regenerate deliberately with:

```bash
UPDATE_BINDINGS=1 cargo test --manifest-path src-tauri/Cargo.toml
git add src/ipc/bindings.ts
```

CI's `backend` job runs the full, unfiltered `cargo test` on Windows (macOS and Linux run `--lib` only), and `linux.yml` runs it on Linux ARM64 after every merge, so this test always runs there. A filtered local run that never reaches `ipc_surface` regenerates nothing; run the full suite before committing a command change.

If a future need for real string snapshots appears (log line format, manifest JSON), adding `insta` is a normal dependency decision — but it must be made before this section can describe it in the present tense.

## What we deliberately don't test

- **Tauri itself.** We trust upstream's tests.
- **Win32 / CUPS APIs.** We trust the OS.
- **Bundled subprocess output.** SumatraPDF is GPL'd third-party; if its output changes between versions, we update expectations, not bug them.
- **Network layer for the updater.** We trust `ureq` for HTTP and TLS. The update client's own logic (semver comparison, release-payload parsing, the 404/no-release and error paths) *is* unit-tested against fixture payloads, and `src-tauri/tests/update_startup.rs` covers the startup check. The transport is covered against loopback servers only (status mapping, headers, the timeout and an unreachable server reading as `offline`, a `CONNECT` proxy, the proxy variable order); the one test that reads the published manifests over the network is `#[ignore]`d.
- **Rendering output of WebView2 / WKWebView / WebKitGTK.** Browser rendering is tested by upstream.

## Running tests locally

| Command | Runs |
|---|---|
| `cargo test --manifest-path src-tauri/Cargo.toml` | Unit + integration tests not marked `#[ignore]` (or `make test-rust`) |
| `cargo test --manifest-path src-tauri/Cargo.toml -- --ignored` | **Only** the ignored ones (network + real CUPS); `ci.yml` never runs them, `linux.yml` runs the CUPS ones |
| `dbus-run-session -- xvfb-run -a cargo test --manifest-path src-tauri/Cargo.toml` | The full suite on a Linux machine without a desktop session, as `linux.yml` runs it (needs `xvfb` and `dbus`) |
| `cargo test --manifest-path src-tauri/Cargo.toml -- --include-ignored` | Everything, ignored included |
| `cargo test <name>` | Tests matching `<name>` substring |
| `cargo test --manifest-path shell-ext-win/Cargo.toml` | The shell extension's portable half; works on any host OS |
| `pnpm test` | Frontend unit tests (`vitest run`) |
| `pnpm test:coverage` | Frontend tests with V8 coverage (report under `coverage/`); no threshold locally |
| `pnpm test:e2e` | E2E tests (Windows/Linux; requires a built binary + `tauri-driver`, plus `NATIVE_DRIVER` on Windows or `WebKitWebDriver` on Linux) |
| `pnpm dev:e2e` | Frontend with the fake Tauri host on `:1421`, for browser-driven E2E on any OS |
| `MAECLY_REAL_PRINTER=1 cargo test --manifest-path src-tauri/Cargo.toml --test real_printer -- --ignored --test-threads=1` | Real-CUPS mapping checks against a hold queue (Unix) |
| `pnpm check-all` | `lint`, `format:check`, `typecheck`, `test` — **frontend only, no Rust** |
| `make check` | Everything: the four above plus `cargo fmt --check`, `clippy -D warnings` and `cargo test` |

## Test naming

| Layer | Convention |
|---|---|
| Rust unit | `test_<scenario>_<expected>`, or `<scenario>_<expected>` in a co-located `mod tests` |
| Rust integration | `fr_<NNN>_<scenario>` when pinning a requirement (e.g. `fr_102_atomic_write`), else `<feature>_<scenario>` |
| Vitest | Plain English in `it(...)`, prefixed `FR-###:` / `NFR-###:` when it pins a requirement |
| WebdriverIO | Plain English in `it(...)`, grouped by `describe(...)` |

Names should read as a sentence describing what's true if the test passes. Bad: `test_queue_1`. Good: `test_enqueue_when_full_returns_full_error`. The `FR`/`NFR` tag is a real, searchable convention — `it('FR-021: built-in profiles are read-only …')`, `fr_020_crud_roundtrip` — so you can grep a requirement id to its tests.

## See also

- [coding-standards.md](./coding-standards.md)
- [contributing.md](./contributing.md)
- [release-checklist.md](./release-checklist.md)
- [../07-build-and-release/ci-cd.md](../07-build-and-release/ci-cd.md)
