# Tech Stack

> **TL;DR** — Pluryprint is built on **Tauri 2 + Rust + React (TypeScript)**. This combination gives us a small installer, fast cold start, low idle RAM, and a near-fully shared codebase across Windows and macOS — which is exactly what a context-menu-invoked utility needs. Updates install through Tauri's updater plugin with a minisign-signed manifest per channel on GitHub Releases; see [`auto-updater.md`](./auto-updater.md).

## 1. Stack at a glance

Everything in this table is declared in `src-tauri/Cargo.toml`, `shell-ext-win/Cargo.toml`, `package.json` or `tauri.conf.json`.

| Layer                  | Choice                                                  |
| ---------------------- | ------------------------------------------------------- |
| Application shell      | **Tauri 2.12** (Rust host + system webview)             |
| Backend / core logic   | **Rust 1.90+** (stable, edition 2021)                   |
| Frontend / UI          | **React 19 + TypeScript 5 + Vite 7 + Tailwind CSS 3** (layout utilities only; design tokens are CSS custom properties, Tailwind `preflight` disabled) |
| Frontend state         | `zustand` 5; toasts via `sonner`                        |
| Localization           | **In-house** (`src/i18n/`, JSON catalogues per screen) — no i18n library |
| Typed IPC              | `tauri-specta` + `specta` (pinned `=` pre-release) → generated `src/ipc/bindings.ts` |
| Async runtime          | `tokio` 1.41 (queue dispatcher). The print/convert hops are **blocking** `std::process` / pure functions the dispatcher drives |
| Serialization          | `serde`, `serde_json`, `toml`                           |
| Logging / tracing      | `tracing`, `tracing-subscriber` (JSON), `tracing-appender` (non-blocking writer only — rotation/retention is our own `RollingJsonl`) |
| HTTP (update check)    | `ureq` 3.4, blocking, 10 s timeout                      |
| Windows API bindings   | `windows` 0.62, the line tauri 2.12 / webview2-com 0.39 use (`Win32_Graphics_Printing`, `Win32_Storage_Xps`, …) |
| macOS API bindings     | `objc2` 0.6 + `objc2-app-kit` / `objc2-foundation` 0.3, feature-narrowed |
| Image → PDF            | `image` 0.25 (jpeg/png/tiff/bmp/webp) + `lopdf` 0.44, **in-process** |
| SVG → PDF              | `usvg` + `svg2pdf` (vector), `flate2` for SVGZ, in the `--worker svg` process |
| Markdown / text → HTML | `pulldown-cmark` 0.13 (`html` only); text decoded in-process |
| HTML → PDF             | The system web view in the `--worker render-pdf` process: WebView2 through `webview2-com` / `windows-core` (Windows), `WKWebView` through `objc2` + `block2` (macOS) |
| Encrypted PDFs         | `lopdf` decryption in-process; the password held in a `zeroize`d buffer |
| PDF inspection         | `lopdf`; `printpdf` 0.12 with `default-features = false` (the test page) |
| CLI                    | `clap` 4 (`derive`), plus `wait-timeout` for bounded child waits and `walkdir` for folder expansion |
| Shell extension (Win)  | **Rust** `cdylib` implementing `IExplorerCommand` (`shell-ext-win/`), built with `windows` + `embed-resource` |
| Shell extension (Mac)  | NSServices provider declared in `Info.plist`, backed by `objc2` |
| Installer (Win)        | Tauri's **NSIS** target, `installMode: currentUser`, English and Spanish, registering the Explorer command per user (no sparse package yet, ADR-0013) |
| Installer (Mac)        | `.dmg` per architecture, `.app` signed ad hoc, not notarized (ADR-0011) |
| Update flow            | `tauri-plugin-updater`: check and install read the same `latest.json` per channel on GitHub Releases; minisign signatures; install only on request |
| Tauri plugins in use   | `tauri-plugin-dialog`, `tauri-plugin-opener`, `tauri-plugin-single-instance`, `tauri-plugin-notification`, `tauri-plugin-updater` |
| Tests                  | `vitest` + `@testing-library/react` (frontend), `cargo test` incl. `src-tauri/tests/*` (backend), **WebdriverIO** for e2e |
| CI/CD                  | GitHub Actions: `ci.yml`, `release.yml` (draft releases on a tag), `release-channels.yml`, `landing.yml` (redeploys the landing, a separate repository on Vercel) |
| Code signing           | None for the OS (ADR-0011); updates signed with minisign |

## 2. Why Tauri 2

Tauri 2 was selected after benchmarking five competing frameworks. The full comparison appears in [`../02-research/`](../02-research/), but the headline numbers driving the choice are:

| Stack             | Installer  | RAM idle    | Cold start | Auto-update           | Cross-platform        | Score |
| ----------------- | ---------- | ----------- | ---------- | --------------------- | --------------------- | ----- |
| **Tauri 2**       | 5-10 MB    | 30-50 MB    | < 500 ms   | Built-in (MIT)        | ✓ Win/Mac/Linux       | 9/10  |
| Electron          | 80-150 MB  | 200-300 MB  | 1-2 s      | `autoUpdater` module  | ✓ but overkill        | 5/10  |
| .NET MAUI / Avalonia | ~50 MB  | 80-150 MB   | Fast       | Velopack / NetSparkle | ✓ Avalonia only       | 6/10  |
| Wails (Go)        | 5-15 MB    | 20-40 MB    | < 500 ms   | None built-in         | ✓ smaller community   | 6.5/10|
| Flutter Desktop   | 50-100 MB  | 100-150 MB  | Fast       | None built-in         | ✓ shell ext. awkward  | 4/10  |
| Qt / PySide6      | 100+ MB    | 80-150 MB   | Fast       | None built-in         | ✓ mature but verbose  | 5/10  |

### Why each metric matters for Pluryprint

- **Installer size.** Pluryprint is invoked by people who right-click a file and expect the dialog to appear instantly. Bundle bloat hurts the install experience and slows updates.
- **RAM idle.** The app may sit in the tray (or be re-launched on every right-click). Electron's 250 MB baseline is unacceptable for a utility.
- **Cold start.** Right-click latency is perceived directly by users. Anything over ~700 ms feels broken.
- **Auto-update.** A polished updater is non-negotiable for a desktop app shipped outside an app store. Tauri ships one as a first-party MIT plugin; competitors require third-party glue.
- **Cross-platform.** macOS is on the roadmap (M5). A 95% shared codebase means the Mac port is a port, not a rewrite.

### Concrete Tauri 2 advantages

1. **Updater plugin, in use.** Tauri's signed-manifest updater installs updates per user once the user asks; the check reads the same manifest. Releases live on GitHub, so there is no update server to run ([ADR-0011](../adr/0011-release-channel.md)).
2. **System webview.** Uses WebView2 on Windows and WKWebView on macOS — no Chromium bundled. This is the source of the small installer and low RAM, and since round 3 the same web view also prints HTML, Markdown and text documents in a worker process ([ADR-0012](../adr/0012-system-web-view-renderer.md)).
3. **Rust backend.** Memory-safe, zero garbage-collection pauses, fast startup, excellent FFI for the COM DLL boundary, and a strong ecosystem (`tokio`, `serde`, `tracing`).
4. **Type-safe IPC.** `tauri::command` macros plus generated TypeScript bindings (`tauri-specta` or hand-rolled) make the React ↔ Rust boundary safe.
5. **First-class Windows support.** Tauri 2 fixed several Windows pain points from v1 (better installer story, MSIX support, mobile, capabilities system).
6. **Capabilities security model.** Tauri 2's permission system lets us narrow the webview's reachable IPC commands to a minimum.

## 3. Why Rust for the core

The Rust core handles file enumeration, format detection, conversion orchestration, queue management, IPC, the CLI, and the updater logic. It was chosen because:

- **Memory safety** in a process that ingests untrusted file paths and shells out to converters. A use-after-free in the print queue would be embarrassing.
- **Performance.** Enumerating thousands of files, dispatching conversions in parallel, and maintaining an IPC stream to the webview is throughput-bound. Rust + `tokio` excels here.
- **Single binary deployment.** Statically linked Rust binaries ship without a runtime — important for a utility that should never make the user "install Java" or ".NET Framework".
- **Ecosystem maturity.** Every crate the design needs exists and is mature — see the table below, which is the real list.
- **One language across the OS boundary.** The Windows shell extension is a Rust `cdylib` too, so the handoff contract (`shell-ext-win/src/handoff.rs` ↔ `src-tauri/src/cli/handoff.rs`) is written once in one language, and both sides resolve the handoff directory through the same `directories` call.

### Rust dependencies (direct, `src-tauri/Cargo.toml`)

| Crate                                  | Purpose |
| -------------------------------------- | ------- |
| `tauri` 2.1                            | App framework |
| `tauri-plugin-dialog`                  | Native file picker |
| `tauri-plugin-opener`                  | Open URLs / reveal folders (Rust-side only) |
| `tauri-plugin-single-instance`         | Forward a second launch's argv (NFR-014) |
| `tauri-plugin-notification`            | Native OS notifications (FR-072) |
| `tauri-plugin-updater`                 | Download, minisign verification and per-user install of an update (FR-082/083) |
| `tauri-specta`, `specta`, `specta-typescript` | Typed IPC bindings; pinned `=` because they are pre-release |
| `tokio` 1.41                           | Async runtime for the queue dispatcher |
| `serde`, `serde_json`, `toml`          | Config / profiles / queue / IPC serialization |
| `tracing`, `tracing-subscriber`, `tracing-appender` | Structured JSONL logging |
| `clap` 4 (`derive`)                    | The headless CLI surface. `env` is deliberately **off** so exit codes stay the only contract |
| `wait-timeout` 0.2                     | Bounded wait on the print child, so a hung process cannot wedge the queue |
| `walkdir` 2                            | Folder expansion with bounded depth and symlink-loop handling (FR-004) |
| `image` 0.25                           | Pure-Rust image decoding (jpeg/png/tiff/bmp/webp) |
| `lopdf` 0.44                           | PDF page counts, hand-built single-page image PDFs, and decrypting a protected PDF with the user's password |
| `usvg`, `svg2pdf`, `flate2`            | SVG/SVGZ → vector PDF in the SVG worker |
| `pulldown-cmark` 0.13                  | Markdown → HTML in-process |
| `zeroize` 1                            | Zeroes a PDF password when it is dropped |
| `webview2-com`, `windows-core` *(Windows only)* | WebView2 `PrintToPdf` in the render worker |
| `block2` *(macOS only)*                | WebKit completion handlers in the render worker |
| `printpdf` 0.12.8                      | PDF primitives for the test page (`default-features = false`; writes only) |
| `ureq` 3.4                             | Blocking HTTPS for the update check (the channel's `latest.json`) |
| `semver` 1                             | Version comparison by SemVer *precedence* |
| `percent-encoding` 2.3                 | Prefilled "report issue" URL |
| `uuid` 1 (`v7`)                        | Time-sortable job ids — queue ordering falls out of the id |
| `time` 0.3                             | RFC 3339 timestamps |
| `thiserror` 2 / `anyhow` 1             | Library / application error types |
| `directories` 6                        | Per-OS config, data, cache paths |
| `tempfile` 3                           | Atomic writes and the CLI's private conversion cache |
| `windows` 0.58 *(Windows only)*        | winspool `EnumPrinters` + GDI `DeviceCapabilities` |
| `objc2` + `objc2-app-kit` / `objc2-foundation` *(macOS only)* | NSServices provider (FR-001) |

Crates named in earlier drafts and **not** used: `tauri-plugin-cli`, `reqwest`, `winreg`, `which`, `mime_guess`, `infer`, `rayon`, `notify`, `minisign-verify`.

## 4. Why React + TypeScript for the UI

The frontend is a small SPA: a print dialog, a profile editor, a queue viewer, and a settings page. We chose React + TypeScript because:

- **Productivity.** Building rich form UIs in React with hooks and component libraries is fast.
- **TypeScript** catches IPC mismatches at compile time when paired with generated bindings.
- **Small, deliberate dependency set.** The runtime frontend has five packages, not fifty: React, React DOM, the two `@tauri-apps` bridges, `zustand` and `sonner`. UI primitives, forms, icons and IPC caching are hand-written, which is affordable at this app's size and keeps the bundle honest.
- **Testability.** `vitest` + `@testing-library/react` is a well-trodden path.

### Frontend dependencies

**Runtime (bundled):**

| Package                      | Purpose |
| ---------------------------- | ------- |
| `react`, `react-dom` 18.3    | UI runtime |
| `@tauri-apps/api` 2.1        | `invoke` + event bridge (used through the generated bindings) |
| `@tauri-apps/plugin-dialog`  | Native file picker |
| `zustand` 5                  | Cross-route state (queue, profiles, settings stores) |
| `sonner` 1.7                 | Toasts |

**Build / dev only:** `vite` 7, `typescript` 5.7, `tailwindcss` 3.4 + `postcss` + `autoprefixer`, `eslint` 10 (+ react-hooks 7 with the React Compiler rules, jsx-a11y, react-refresh plugins), `prettier`, `vitest` 5 + `@vitest/coverage-v8` + `jsdom` 30, `@testing-library/{react,jest-dom,user-event}`, `@wdio/*` 9 for e2e, `husky` + `lint-staged` + `@commitlint/*`, `@tauri-apps/cli`.

Packages named in earlier drafts and **not** used: `@radix-ui/*`, `react-hook-form`, `@tanstack/react-query`, `lucide-react`, `clsx`, `tailwind-merge`, `react-i18next`, `playwright`.

### Frontend conventions

- Styling is Tailwind for layout plus design tokens as CSS custom properties in `src/styles/tokens.css` (`--mp-*`), imported by `src/styles/globals.css`. Tailwind's `preflight` is disabled so the tokens own the base look. Fonts are bundled, never fetched.
- Localization lives in `src/i18n/` — a provider plus one JSON catalogue per screen, in `en` and `es`. No i18n runtime dependency.
- IPC goes through the generated `src/ipc/bindings.ts` (never hand-edited) and the `src/lib/ipc.ts` wrapper. No raw `invoke()` in components.
- State that crosses route boundaries lives in `zustand`. Anything component-local stays in `useState`.

## 5. Stacks we considered and rejected

### 5.1 Electron

**Pros:** ubiquitous, huge community, mature auto-updater (`electron-updater`), batteries-included.
**Cons:**
- Bundles Chromium → 80-150 MB installer.
- 200-300 MB RAM idle.
- Cold start in the 1-2 s range; unacceptable for a context-menu utility.
- Node.js in main process: extra GC pauses, weaker isolation story than Rust.
- Repeated security advisories for popular Electron apps suggest the surface area is large.

**Verdict:** rejected on installer size, RAM, and cold start.

### 5.2 .NET MAUI / Avalonia (C#)

**Pros:** strong tooling (Visual Studio, Rider), mature .NET BCL, native Win32 interop, decent cross-platform via Avalonia.
**Cons:**
- Self-contained .NET runtime adds 30-50 MB even after trimming.
- No first-party auto-updater. Velopack is good, but third-party.
- COM interop is doable but more friction than C++/Rust.
- macOS story (Avalonia) is less polished than Tauri's WKWebView path.

**Verdict:** rejected because no built-in updater and a bigger footprint than Tauri.

### 5.3 Wails (Go)

**Pros:** very small binaries, fast cold start, simple Go syntax, similar webview model to Tauri.
**Cons:**
- No built-in updater.
- Smaller community; fewer plugins, less battle-tested for shell extension integration.
- Go's GC is fine but adds latency variability that Rust avoids.

**Verdict:** rejected. Tauri is structurally similar but more featureful (updater, capabilities, mobile).

### 5.4 Flutter Desktop

**Pros:** beautiful UI, fast iteration, single codebase across mobile + desktop.
**Cons:**
- Renders its own widgets (Skia) → larger bundle, no native look-and-feel.
- No built-in auto-updater.
- Shell extension integration on Windows is awkward (no clean FFI to a C++ COM DLL).
- Dart ecosystem outside Google is thin for desktop concerns.

**Verdict:** rejected. Wrong fit for a "small native utility" target.

### 5.5 Qt / PySide6

**Pros:** mature, professional, exhaustive widgets, cross-platform.
**Cons:**
- LGPL dynamic linking adds compliance complexity.
- 100+ MB installer when bundled.
- Verbose to develop in compared to React.
- No built-in updater.

**Verdict:** rejected on installer size, LGPL friction, and verbosity.

### 5.6 Rust + egui / Slint (no webview)

**Pros:** smallest possible binary, fastest startup, no webview dependency.
**Cons:**
- Custom-rendered UIs feel non-native on Windows; accessibility (NVDA, narrator) is weaker.
- The print profile editor and dialog need rich form controls; building them in egui is a slog.
- Theming and dark-mode tracking is more work.

**Verdict:** rejected. Webview UX wins for form-heavy desktop utilities.

## 6. Toolchain and supported versions

| Tool          | Required version    |
| ------------- | ------------------- |
| Rust          | 1.90 stable+        |
| Node.js       | 24 LTS (CI); 22.22.2+ minimum |
| pnpm          | 9.x (preferred over npm/yarn) |
| Tauri CLI     | 2.x                 |
| Visual Studio Build Tools | 2022 (Win)         |
| Windows SDK   | 10.0.22621.0+ (for WebView2 + COM)  |
| WebView2 Runtime | Evergreen (auto-installed by Win 11; bootstrapper for Win 10) |
| Xcode         | 15+ (Mac)           |

`package.json` pins the package manager (`pnpm@9.15.0`) and declares `engines.node >= 22.22.2` (the floor jsdom 30 and Vitest 5 set for the test toolchain); `src-tauri/Cargo.toml` declares `rust-version = "1.90"` (the floor Tauri 2.12 and its plugins set) and `shell-ext-win/Cargo.toml` `rust-version = "1.78"`. There is **no `rust-toolchain.toml`** in the repo, so the Rust version CI uses is whatever `.github/workflows/ci.yml` installs — adding one is an open item (see [`security-model.md`](./security-model.md) §9). Inno Setup is not part of the toolchain; the Windows installer comes from Tauri's NSIS target.

## 7. Where this decision is recorded

- ADR [`0001-stack-tauri-rust-react`](../adr/0001-stack-tauri-rust-react.md) — the canonical record of this decision, with full alternatives and consequences.
- ADR [`0010-relicense-to-agpl-3.0`](../adr/0010-relicense-to-agpl-3.0.md) — license choice (AGPL-3.0, superseding [`0002-license-gplv3`](../adr/0002-license-gplv3.md)) that constrains some dependencies.
- ADR [`0004-auto-updater-tauri-builtin`](../adr/0004-auto-updater-tauri-builtin.md) — updater choice tied to Tauri. **The shipped build does not follow it**; the disagreement is tracked in [`auto-updater.md`](./auto-updater.md) §8 and needs a superseding ADR or an implementation.

## 8. Related documents

- [`system-architecture.md`](./system-architecture.md) — how the layers fit together.
- [`shell-extensions.md`](./shell-extensions.md) — the glue at the OS boundary.
- [`auto-updater.md`](./auto-updater.md) — what the update path actually does.
- [`security-model.md`](./security-model.md) — what we sign, what we sandbox, and why.
