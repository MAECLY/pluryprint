# Local Setup

> **TL;DR.** Install Rust stable, Node 24 LTS (22.22.2 at least) + pnpm, the platform's native build tools (Visual Studio Build Tools 2022 on Windows; Xcode CLT on macOS; the WebKitGTK and GTK development packages on Linux), then `pnpm install && pnpm tauri dev`. The app boots with hot reload in under 30 seconds on a typical dev machine.

This document is the developer onboarding checklist. Follow it once on a clean machine; afterward, daily work is `git pull && pnpm install && pnpm tauri dev`.

## Prerequisites

### Windows 10/11

| Tool | Version | Install via |
|---|---|---|
| Rust | stable; `rust-version = "1.90"` is the app crate's floor (Tauri 2.12's) | [rustup.rs](https://rustup.rs/) — accepts default options |
| Node.js | 24 LTS, what CI uses; 22.22.2 is the floor (`engines.node: ">=22.22.2"`) | [nodejs.org](https://nodejs.org/) or `winget install OpenJS.NodeJS.LTS` |
| pnpm | 9.x (`packageManager: pnpm@9.15.0`) | `npm install -g pnpm` or `corepack enable` |
| Visual Studio Build Tools 2022 | latest | [VS Build Tools download](https://visualstudio.microsoft.com/downloads/#build-tools-for-visual-studio-2022) |
| Windows SDK | 10.0.22621 or newer | Bundled with VS Build Tools |
| WebView2 Runtime | latest | Preinstalled on Win 11; for Win 10, [bootstrap installer](https://developer.microsoft.com/microsoft-edge/webview2/) |
| Git | latest | [git-scm.com](https://git-scm.com/) or `winget install Git.Git` |

You do **not** need Inno Setup. There is no `.iss` script in the repo; packaging currently goes through Tauri's own bundler (see [installer-windows.md](../07-build-and-release/installer-windows.md)).

#### Visual Studio Build Tools components

When installing VS Build Tools, select:

- **Desktop development with C++** (the workload) — needed for `link.exe`, which Rust's MSVC toolchain shells out to.
- Within that workload:
  - MSVC v143 - VS 2022 C++ x64/x86 build tools
  - Windows 11 SDK (10.0.22621 or newer)

**ATL is not required.** Earlier drafts of this document listed the C++ ATL components as mandatory for the shell extension. That was written when `shell-ext-win/` was planned as a C++ project; it is a Rust `cdylib` built with the `windows` crate, which needs no ATL. Installing ATL is harmless but buys nothing.

What the shell extension *does* need is `rc.exe` (the Windows Resource Compiler, part of the Windows SDK). Its `build.rs` embeds `res/shellext.rc`, which carries the side-by-side `<msix>` manifest without which Explorer silently never loads the DLL. Set `PLURYPRINT_REQUIRE_RESOURCES=1` to turn a missing `rc.exe` into a build failure instead of a DLL that registers fine and is never invoked — this is what CI does.

To cross-compile the ARM64 slice: `rustup target add aarch64-pc-windows-msvc`.

### macOS 13+

macOS is a fully supported development platform: the whole Rust suite, the whole Vitest suite, the CUPS backend and the portable half of the shell extension all build and run here. Only the WebdriverIO E2E lane is unavailable (WKWebView exposes no WebDriver endpoint).

| Tool | Version | Install via |
|---|---|---|
| Rust | stable | [rustup.rs](https://rustup.rs/) |
| Node.js | 24 LTS | [nodejs.org](https://nodejs.org/) or `brew install node@24` |
| pnpm | 9.x | `npm install -g pnpm` or `corepack enable` |
| Xcode Command Line Tools | latest | `xcode-select --install` |
| Xcode | 15+ | Only if you need the full IDE; the CLT alone is enough to build |
| Homebrew | latest | [brew.sh](https://brew.sh/) |

`create-dmg` is not needed — there is no `.dmg` build script in the repo yet. Neither is LibreOffice: no test shells out to it, because no converter uses it (see [supported-formats.md](../03-requirements/supported-formats.md)).

### Linux (Ubuntu 22.04 or later)

Linux builds and tests the whole app ([ADR-0015](../adr/0015-linux-support.md); Linux is a preview platform). The release builds run on Ubuntu 22.04 (glibc 2.35), so that is the baseline to develop on; Debian 12 and later works with the same packages. The system libraries come from the distribution:

```bash
sudo apt update
# Build: Tauri's WebKitGTK 4.1 / GTK 3 stack, D-Bus (tauri's default `dbus`
# feature: libdbus-sys asks pkg-config for dbus-1) and the AppImage tools.
sudo apt install -y build-essential curl file pkg-config patchelf \
  libwebkit2gtk-4.1-dev libgtk-3-dev libdbus-1-dev librsvg2-dev
# Run and test: CUPS with a PDF printer, the HEIF decoders and a virtual
# display for the render worker's tests on a headless machine.
sudo apt install -y cups cups-client cups-bsd cups-ipp-utils \
  printer-driver-cups-pdf libheif-examples imagemagick xvfb
```

| Tool | Version | Install via |
|---|---|---|
| Rust | stable (`rust-version = "1.90"`) | [rustup.rs](https://rustup.rs/) |
| Node.js | 24 LTS | [nodejs.org](https://nodejs.org/) (the distribution's `nodejs` on 22.04 is too old) |
| pnpm | 9.x | `corepack enable` |

`libwebkit2gtk-4.1-dev` must be **2.42 or later** (wry 0.57 enables WebKitGTK's `v2_40` API but also calls a 2.41.90 function); Ubuntu 22.04 has it from `jammy-updates`, so run `apt update` first. Neither `libssl-dev` nor `libxdo-dev` is needed: nothing in `Cargo.lock` links OpenSSL or libxdo. `libayatana-appindicator3-dev`, which Tauri's generic prerequisites list, is not needed either: the app does not enable Tauri's tray icon. On Fedora the matching packages are `webkit2gtk4.1-devel`, `gtk3-devel`, `dbus-devel` and `librsvg2-devel` (not tried; only Ubuntu is).

On Ubuntu 22.04 `libheif-examples` is libheif 1.12, which provides `heif-convert` but not `heif-dec` (added in libheif 1.17); either is enough for HEIC. ImageMagick 6 reads HEIC through the same libheif.

### Optional but recommended (every platform)

- **VS Code** with extensions: `rust-lang.rust-analyzer`, `tamasfe.even-better-toml`, `dbaeumer.vscode-eslint`, `esbenp.prettier-vscode`, `bradlc.vscode-tailwindcss`.
- **JetBrains RustRover** as an alternative IDE if you prefer.
- **`cargo-llvm-cov`** (`cargo install cargo-llvm-cov --locked`) if you want to measure Rust coverage (CI does not).

## Initial setup

```bash
# Clone
git clone https://github.com/MAECLY/pluryprint.git
cd pluryprint

# Install everything (frontend deps + `cargo fetch`)
make install
```

`make install` is exactly `pnpm install` followed by `cargo fetch --manifest-path src-tauri/Cargo.toml`. Without make:

```bash
pnpm install
cargo build --manifest-path src-tauri/Cargo.toml   # ~5 min first time, cached after
```

Run `make doctor` to confirm node, pnpm, rustc, cargo and the Tauri CLI are all visible.

After this you should have:

- `node_modules/` populated.
- `src-tauri/target/debug/pluryprint(.exe)` built.
- `src/ipc/bindings.ts` unchanged in `git status` — the debug build regenerates it, and a diff here means the committed copy was stale.
- No errors from either step.

> Note the `--manifest-path` form. `cd src-tauri && cargo build` also works, but every command in this repo (and in `ci.yml` and the `Makefile`) uses `--manifest-path` so it can be run from the repository root, and so it is unambiguous which of the two crates — `src-tauri/` or `shell-ext-win/` — is being addressed.

If errors appear, see [Troubleshooting](#troubleshooting) below.

## Daily dev workflow

### Hot-reload dev server

```powershell
pnpm tauri dev
```

This:

1. Starts the Vite dev server on **port 1421** (`server.port` in `vite.config.ts`, `strictPort: true`).
2. Builds the Rust backend in debug mode.
3. Launches the app, pointing the webview at `http://localhost:1421` (`build.devUrl` in `tauri.conf.json`).
4. Watches `src/` for frontend changes (instant) and `src-tauri/src/` for backend changes (full rebuild + relaunch).

Because `strictPort` is on, a stale process holding 1421 makes startup fail loudly rather than silently moving to another port and leaving the webview pointed at nothing.

Backend changes take 5-30 seconds to take effect (cargo incremental compile + relaunch). Frontend changes are sub-second.

`make dev-web` (`pnpm dev`) runs Vite alone, with no Rust build, for UI-only iteration. IPC calls degrade to the "backend unavailable" path rather than throwing.

### Running tests

```bash
make test-rust     # cargo test --manifest-path src-tauri/Cargo.toml
make test          # pnpm test  (vitest run)
make test-all      # both
make check         # EVERYTHING: lint, format:check, typecheck, vitest,
                   #   cargo fmt --check, clippy -D warnings, cargo test
```

**`pnpm check-all` is frontend-only.** It chains `pnpm lint && pnpm format:check && pnpm typecheck && pnpm test` — no cargo, no clippy, no Rust tests. Use `make check` (or run the cargo commands yourself) before pushing, or CI's `backend` lane will catch what you didn't.

The shell extension is a separate crate and none of the above touches it:

```bash
cargo test --manifest-path shell-ext-win/Cargo.toml    # works on any host OS
cargo clippy --manifest-path shell-ext-win/Cargo.toml --all-targets -- -D warnings
```

### Production build (local)

```bash
pnpm tauri build          # or: make build
```

On Windows, run `powershell -ExecutionPolicy Bypass -File scripts\fetch-sumatrapdf.ps1` first (it runs in the stock Windows PowerShell 5.1 as well as `pwsh`, and picks the host architecture unless given `-Arch x64|arm64`; POSIX twin: `scripts/fetch-sumatrapdf.sh --arch x64|arm64`). It downloads SumatraPDF from a pinned vendor URL, verifies a pinned SHA-256, fetches the licence files from the pinned upstream commit (each verified), and drops the executable plus `LICENSE.txt`, `COPYING.BSD.txt`, `AUTHORS.txt`, `SOURCE.txt` and the written offer `SOURCE-OFFER.txt` into `src-tauri/resources/sumatrapdf/`, which `bundle.resources` copies into the bundle. The binary is deliberately not committed; without this step the app builds but every print job fails with `DependencyMissing`. Then run `powershell -ExecutionPolicy Bypass -File scripts\build-shell-ext.ps1 -Arch all` to build the Explorer DLL for x64 and ARM64 into `src-tauri/resources/shellext/<arch>/`: the installer hooks refuse to build with neither DLL (and, with `CI=true`, without both).

Output:

- Windows: `src-tauri/target/release/bundle/nsis/` (NSIS is what `tauri.conf.json` configures today) and the binary at `src-tauri/target/release/pluryprint.exe`.
- macOS: `src-tauri/target/release/bundle/macos/Pluryprint.app`.

The NSIS setup in `bundle/nsis/` is the installer: it registers the Explorer entry per user through `src-tauri/windows/installer-hooks.nsh`. `installers/windows/` and `installers/macos/` are empty placeholders. See [installer-windows.md](../07-build-and-release/installer-windows.md).

### CLI usage during dev

The same binary runs headlessly when the first non-flag argument is one of `print`, `printers`, `profiles`, `queue`, `update`:

```powershell
# Files are a COMMA-SEPARATED --files list, not positional arguments
src-tauri\target\debug\pluryprint.exe print --files sample.pdf --printer "Microsoft Print to PDF"

# Several files, or a wildcard Pluryprint expands itself (so it behaves the
# same in cmd.exe and in a POSIX shell)
src-tauri\target\debug\pluryprint.exe print --files "a.pdf,b.pdf" --profile draft
src-tauri\target\debug\pluryprint.exe print --files "*.pdf" --mono --duplex long

# Inspect without printing; --json is available on every subcommand
src-tauri\target\debug\pluryprint.exe printers list --json
src-tauri\target\debug\pluryprint.exe queue status
```

This bypasses the GUI and is useful for backend debugging. Exit codes are the contract — see `src-tauri/src/cli/exit.rs` and `docs/04-architecture/ipc-and-cli.md`.

## Debugging

### Rust backend

**VS Code with CodeLLDB:**

`launch.json`:

```json
{
  "version": "0.2.0",
  "configurations": [
    {
      "name": "Debug Tauri (Rust backend)",
      "type": "lldb",
      "request": "launch",
      "cargo": {
        "args": ["build", "--manifest-path=src-tauri/Cargo.toml", "--no-default-features"],
        "filter": { "name": "pluryprint", "kind": "bin" }
      },
      "args": [],
      "cwd": "${workspaceFolder}",
      "env": {
        "RUST_LOG": "pluryprint=debug,tauri=info"
      }
    }
  ]
}
```

Set breakpoints in `src-tauri/src/`, hit F5. The debugger attaches to the Rust process; the webview runs detached.

**Logging-based debug:**

Set `RUST_LOG`:

```powershell
$env:RUST_LOG = "pluryprint=debug,tauri=info,ureq=warn"
pnpm tauri dev
```

(The HTTP client is `ureq`, not `reqwest`.) The app also writes structured JSONL to its log directory — `%LOCALAPPDATA%\Maecly\Pluryprint\data\logs\` on Windows, `~/Library/Logs/Pluryprint/` on macOS, `~/.local/share/pluryprint/logs/` on Linux. About → "Open logs folder" reveals the live path.

Module-specific filters: `pluryprint::engine=trace` for very detailed output from the engine module.

### TypeScript frontend

**Browser DevTools:**

Right-click anywhere in the app webview → "Inspect Element" (or press F12). This opens Chromium DevTools (Tauri uses WebView2 on Windows, WKWebView on macOS and WebKitGTK on Linux — DevTools work on all three; on Linux they are WebKit's Web Inspector).

In production builds, DevTools are disabled. Set `devtools = true` in `tauri.conf.json` only for dev or rebuild a debug binary.

**VS Code with Edge Tools or Chrome Debugger:**

Less reliable than browser DevTools for Tauri because the webview's HTTP server lives behind Tauri's IPC. Browser DevTools are the standard.

### Mixed Rust + TS debugging

When an IPC call fails, check both sides:

1. JS side: in browser DevTools console, look for the `invoke()` rejection.
2. Rust side: in the terminal running `pnpm tauri dev`, look for `tracing` output around the same timestamp.

The IPC layer adds a `command` field to log lines; grep for the command name.

## Connecting a real printer

For end-to-end testing, you need at least one printer. Three options:

### Microsoft Print to PDF (Windows)

Preinstalled on Win 10/11. Always available, never offline, outputs `.pdf` files. **Use this for CI and the default dev loop.**

```powershell
# Verify it's available
Get-Printer | Where-Object { $_.Name -eq "Microsoft Print to PDF" }
```

### CUPS-PDF (macOS)

Install via `brew install cups-pdf`. Outputs `.pdf` to `~/Desktop` by default. macOS equivalent of Microsoft Print to PDF.

### CUPS-PDF (Linux)

`sudo apt install printer-driver-cups-pdf` adds a queue named `PDF` that writes each job to `~/PDF/` (on some distributions `/var/spool/cups-pdf/<user>/`). If no queue appears, add one with `sudo lpadmin -p PDF -E -v cups-pdf:/ -m lsb/usr/cups-pdf/CUPS-PDF_opt.ppd` (`lpinfo -m | grep -i cups-pdf` lists the model name on your system). To test pause, cancel and job tracking without paper, a queue that holds every job is useful: `sudo lpadmin -p Maecly_Hold -E -v cups-pdf:/ -m <the same model> -o job-hold-until-default=indefinite`; `tests/real_printer.rs` and the ignored `engine::print::cups` tests submit to it.

### A real physical printer

Install the manufacturer's driver. Test with the OS's standard print dialog first to confirm the driver works; then test through Pluryprint.

For driver-quirky printers (HP, Brother), be prepared for issues that aren't Pluryprint's fault. We track known-good printers in a separate compatibility matrix doc.

## Verifying the install

Run this smoke test sequence after first setup:

```bash
# 1. Run every gate, Rust included
make check
# Expect: all green. This is the same set CI's blocking lanes run.

# 2. Boot the dev app
pnpm tauri dev
# Expect: app window opens within ~30 seconds.

# 3. From the GUI: go to Printers.
# Expect: your real queues, enumerated from the OS spooler — winspool on
# Windows, `lpstat`/CUPS on macOS and Linux. An empty list on a machine with printers
# means the platform backend failed, not that the screen is a mock.

# 4. Print a PDF or an image (JPG/PNG/TIFF/BMP/WebP — those and PDF are the
#    only formats the engine accepts today).
# Expect: the Queue screen shows the job progressing; output appears.
```

If any step fails, see Troubleshooting.

## Troubleshooting

### Windows: `link.exe not found`

VS Build Tools incomplete or not on PATH. Reinstall, ensure "Desktop development with C++" workload is selected, and **restart your shell** (build tools modify PATH on install but existing shells don't pick it up).

### Windows: cargo build fails with `windows.h not found`

Windows SDK missing. In VS Build Tools installer, add "Windows 11 SDK (10.0.22621.0)" or newer.

### Windows: the shell extension DLL builds but Explorer never loads it

Almost always a missing embedded manifest. `shell-ext-win/build.rs` embeds `res/shellext.rc`, which carries the `<msix>` side-by-side element that grants the DLL package identity; without it Explorer loads nothing and `Package.Current` is null — with no error anywhere. Build with `PLURYPRINT_REQUIRE_RESOURCES=1` so a missing `rc.exe` fails the build instead:

```powershell
$env:PLURYPRINT_REQUIRE_RESOURCES = "1"
cargo build --release --manifest-path shell-ext-win/Cargo.toml
```

The other silent failure mode is identity mismatch: `publisher`, `packageName` and `applicationId` in `res/shellext.manifest` must match `Identity/@Publisher`, `Identity/@Name` and `Application/@Id` in the sparse-MSIX `AppxManifest.xml` character for character. A mismatch surfaces as `0x80073D54`, or as an extension that simply never appears.

### Windows: `pnpm tauri dev` says WebView2 not found

Install WebView2 Runtime from [https://developer.microsoft.com/microsoft-edge/webview2/](https://developer.microsoft.com/microsoft-edge/webview2/). On Win 11 this is preinstalled and shouldn't happen.

### macOS: `xcode-select --install` opens a dialog that says "already installed"

You're fine. Proceed.

### macOS: cargo build fails with `ld: library not found for -lSystem`

Run `xcode-select --install` and accept the license: `sudo xcodebuild -license accept`.

### macOS: build slow on M-series

Ensure you have the right Rust target: `rustup target add aarch64-apple-darwin` (should be the default on M-series; verify with `rustc --print target-list | grep apple-darwin`).

### Both: `pnpm tauri dev` is "stuck" with no window

First-time builds can take 5-15 minutes. Watch for cargo progress in the terminal. If it's stuck for >20 minutes, kill it (Ctrl+C) and check `cargo build --verbose` from `src-tauri/` for actual errors.

### Both: `cargo test` fails with permission errors on a temp file

Some tests use temp files; antivirus on Windows or `tmpfs` quirks on macOS can interfere. Try setting `TMPDIR`:

```powershell
$env:TMPDIR = "$env:TEMP\pluryprint-test"
mkdir $env:TMPDIR -Force
cargo test
```

### Both: rust-analyzer is slow / unresponsive

The Rust language server indexes `target/`. Make sure `target/` is excluded from VS Code's `files.watcherExclude`:

```json
{
  "files.watcherExclude": {
    "**/target/**": true,
    "**/node_modules/**": true
  }
}
```

### Both: `pnpm install` fails on `husky` postinstall

Husky may not be set up if you're cloning a fork. Skip with `pnpm install --ignore-scripts`, or run `pnpm prepare` separately.

### Windows: shell extension registered but still not in the menu

Explorer caches shell extension DLLs aggressively. Restart `explorer.exe` (or log out and back in) after any registration change. Installing a build made with `pnpm tauri build` registers it for you (per user, under `HKCU`); a dev build run with `pnpm tauri dev` is not registered, so register by hand as `shell-ext-win/README.md` shows, or install a build. On Windows 11 the entry is under **Show more options**, and with UAC turned off Explorer ignores per-user registrations altogether. See [installer-windows.md](../07-build-and-release/installer-windows.md).

## Performance tips

- **Use a fast SSD.** Cargo and pnpm both touch many small files; HDD or slow SSD will cause minute-scale builds.
- **Increase Rust parallelism:** `export CARGO_BUILD_JOBS=8` (or whatever your CPU count). Default is "number of cores", which is usually right.
- **Use `cargo nextest`** for faster test runs: `cargo install cargo-nextest --locked && cargo nextest run`. We don't require it but it's significantly faster than `cargo test` for our suite.
- **Disable antivirus realtime scan on `target/`** if your AV is aggressive (Windows Defender by default, Symantec, etc.). On Defender: `Add-MpPreference -ExclusionPath "C:\path\to\pluryprint\src-tauri\target"`.

## Updating local environment

Periodically:

```powershell
# Update Rust
rustup update stable

# Update Node deps (review the diff before committing)
pnpm update --interactive

# Update Cargo deps
cd src-tauri
cargo update
cd ..
```

These changes go through PR review like any code change.

## See also

- [coding-standards.md](./coding-standards.md)
- [contributing.md](./contributing.md)
- [testing-strategy.md](./testing-strategy.md)
- [repo-structure.md](./repo-structure.md)
- [../07-build-and-release/installer-windows.md](../07-build-and-release/installer-windows.md)
