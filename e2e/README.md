# End-to-end tests

Two harnesses live here:

- **WebdriverIO specs** (`specs/`) that drive the **real built Pluryprint
  binary** through [`tauri-driver`](https://v2.tauri.app/develop/tests/webdriver/),
  the WebDriver bridge Tauri ships. Windows and Linux only.
- **A browser harness** (`browser/`) that serves the real frontend from the
  Vite dev server with a **fake Tauri host**, for driving the UI from any
  browser automation on any OS. This is how the UI gets end-to-end coverage
  on macOS, where WebDriver cannot reach WKWebView.

Both cover flows, not features (feature-level coverage lives in the Rust and
Vitest unit tests); see
[`docs/08-development/testing-strategy.md`](../docs/08-development/testing-strategy.md).

## Layout

```
e2e/
├── wdio.conf.ts            # runner config; spawns/kills tauri-driver, forces an English UI, screenshots on failure
├── tsconfig.json           # standalone TS config for the wdio specs (not part of `pnpm typecheck`)
├── artifacts/              # failure screenshots (gitignored, with wdio-logs/)
├── specs/
│   ├── helpers.ts          # dismiss onboarding (EN/ES), navigate the sidebar, forceEnglishUi, restore settings
│   ├── home.e2e.ts         # formats advertised (GIF never), shortcut layer (NFR-034), focusable quick actions, live recent activity
│   ├── print-job.e2e.ts    # drop a file, submit (queue paused, never spooled), see it in the Queue; FR-005 (.xyz and a GIF refused); FR-004
│   ├── queue.e2e.ts        # live queue, List/Board/Timeline views, queue controls and an honest "Clear done"
│   ├── printers.e2e.ts     # enumeration from the host spooler; add-printer tabs and the Windows transport limit
│   ├── profiles.e2e.ts     # create a profile and make it default; unsaved edits survive navigation; removes what it created
│   ├── settings.e2e.ts     # dark theme class; a setting survives relaunch (FR-100); language switch (FR-094)
│   └── about.e2e.ts        # semver, links (FR-110), log directory (FR-113), versions, live update check
└── browser/
    ├── vite-plugin.ts      # injects ipc-stub.ts into index.html, dev server only, only in e2e mode
    ├── ipc-stub.ts         # installs window.__TAURI_INTERNALS__ and window.__PLURY_E2E__
    ├── fake-backend.ts     # in-memory backend seeded from src/test/fixtures.ts
    └── ipc-stub.test.ts    # the harness's own contract tests (run by `pnpm test`)
```

The wdio specs are named `*.e2e.ts` (not `*.spec.ts`) so Vitest never picks
them up: `pnpm test` stays unit-only (plus `browser/*.test.ts`, which are plain
Vitest), and `pnpm test:e2e` is the sole WebdriverIO entry point.

## WebdriverIO on Windows

`tauri-driver` supports **Windows and Linux only**. The pull-request gate
(`ci.yml`) does not run this suite; `linux.yml` runs it on Linux ARM64 after
every merge, without blocking (see [below](#webdriverio-on-linux)). On
Windows, run it on a Windows machine or the VM. Install `tauri-driver`
(`cargo install tauri-driver --locked`), read the installed WebView2 runtime
version from its EdgeUpdate registry key
(`HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}`,
value `pv`) and download the `msedgedriver` of exactly that version, exported
as `NATIVE_DRIVER`. Build a debug binary with
`pnpm tauri build --debug --no-bundle --config src-tauri/tauri.ci.conf.json`
(frontend embedded, no installers), then `pnpm test:e2e`. Failure screenshots
land in `e2e/artifacts/`. The `about` and `queue` specs are the quickest check
that a session opens at all.

Three things make WebDriver reach the app's WebView2 safely, verified on
Windows 11 ARM64 with WebView2 154:

- `msedgedriver` asks for its debugging port through
  `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`, which WebView2 ignores once wry
  passes explicit arguments. The app folds that variable into its windows'
  arguments at startup (`util::chromium::merge_browser_args`); without it every
  session failed with `DevToolsActivePort file doesn't exist`.
- `wdio.conf.ts` sets `wdio:enforceWebDriverClassic`: WebDriver BiDi commands
  target a browsing context msedgedriver does not have for a WebView2, and the
  specs then read a blank page.
- `msedgedriver` also hands WebView2 a throwaway `--user-data-dir` in that
  variable, and WebView2 keeps its profile in that folder's `EBWebView`. An
  unstamped profile runs Chromium's blank-password logon probe, which records
  a failed Windows logon (Security event 4625) per session, and ten in ten
  minutes lock the account. The app stamps every profile the variable (or
  `WEBVIEW2_USER_DATA_FOLDER`) names before its web view starts
  (`util::chromium::extra_profiles_to_stamp`). Measured in the VM in round 7:
  the full suite (7 sessions) recorded 0 failed logons; the same `about` spec
  on the previous build recorded 1 per session.

```powershell
# 1. Install the WebDriver bridge (once).
cargo install tauri-driver --locked

# 2. Build the app so there is a binary to drive.
pnpm tauri build --debug

# 3. Point tauri-driver at the Edge WebDriver that matches your WebView2, then
#    run the suite. msedgedriver.exe ships with the `edgedriver` npm package
#    (a WebdriverIO dependency) or with the Selenium tools on CI runners.
$env:NATIVE_DRIVER = "C:\path\to\msedgedriver.exe"
pnpm test:e2e
```

| Var              | Purpose                                                                       |
| ---------------- | ----------------------------------------------------------------------------- |
| `TAURI_APP_PATH` | Override the binary path (default `src-tauri/target/debug/pluryprint[.exe]`). |
| `NATIVE_DRIVER`  | Path to `msedgedriver.exe` (Windows) passed to `tauri-driver`.                |

Selectors match visible text with the helpers in `specs/helpers.ts`: `byText` (an
XPath on the element's own text — WebdriverIO's bare `*=text` is _partial link
text_ and only matches `<a>`), `visibleText` (lower-cased, since `getText()`
returns CSS-uppercased headings as rendered) and `replaceInputText` (select
all and type, because `setValue` bypasses React). The suite does not need an
interactive desktop: it passes in the Windows test VM from a session-0
scheduled task.

Things to know before running it on your own machine:

- **It writes your real config.** The specs drive the real binary, so they
  change the per-user `config.toml` (theme, language, a test profile) and
  restore the defaults afterwards. There is no config-path override.
- **The UI is switched to English first.** Every spec finds controls by their
  English labels, and the first launch follows the OS language (FR-093), so
  the `before` hook in `wdio.conf.ts` selects `en-US` whatever the starting
  language is. Your language setting ends up as English.
- **Nothing is printed.** `print-job.e2e.ts` pauses the whole queue before it
  submits, then cancels the job and resumes. If a run is killed in between,
  resume the queue from the Queue screen and clear the cancelled job.
- **If the run seems stuck,** it is usually a lookup waiting out its timeout
  (10–15 s per step, 60 s per test). Check `e2e/artifacts/` for the failure
  screenshot, and confirm `NATIVE_DRIVER` matches the installed WebView2
  version: a mismatched `msedgedriver` makes session creation retry three
  times over two minutes before failing.

## WebdriverIO on Linux

On Linux `tauri-driver` proxies to WebKitGTK's own `WebKitWebDriver`, which
it finds on `PATH`: no `NATIVE_DRIVER`, and no version to match by hand
(Debian and Ubuntu ship it as `webkit2gtk-driver`, built from the same
WebKitGTK as the library the app links). Without a desktop session, run it
under a virtual display and a session bus:

```sh
sudo apt-get install webkit2gtk-driver xvfb dbus-x11
cargo install tauri-driver --version 2.1.0 --locked
pnpm tauri build --debug --no-bundle --config src-tauri/tauri.ci.conf.json
dbus-run-session -- xvfb-run -a pnpm test:e2e
```

`linux.yml`'s `e2e` job does exactly this on `ubuntu-22.04-arm`, with a
cups-pdf queue as the default printer for the printers spec, and uploads
`e2e/artifacts/` when it fails. It is `continue-on-error` for now: the specs
were written and verified against WebView2, so the first Linux runs are
expected to surface spec assumptions (timings, the printers screen's Windows
transport note) rather than app bugs. Under Xvfb, WebKitGTK renders in
software: set `WEBKIT_DISABLE_COMPOSITING_MODE=1` if pages stay blank.

## Launch smoke (by hand)

The pull-request gate never launches the app (on Linux, `linux.yml`'s e2e
job does). To check that a packaged binary boots at all, build it with `pnpm tauri build --debug --config src-tauri/tauri.ci.conf.json`
(unsigned: no updater key), launch it, and look in the real per-user log
folder (`%LOCALAPPDATA%\Maecly\Pluryprint\data\logs\` on Windows,
`~/Library/Logs/Pluryprint/` on macOS) for `app_started` and then
`main_window_resolved` in `pluryprint-*.jsonl`. That catches missing runtime
dependencies and startup panics a compile-only build would miss.

## Browser harness (macOS and anywhere else)

```sh
pnpm dev:e2e     # = vite --mode e2e  (PLURY_E2E_STUB=1 pnpm dev is equivalent)
# then open http://localhost:1421/?e2e-onboarded=1 in any browser
```

`vite.config.ts` registers the `pluryE2eStub` plugin, which is
`apply: 'serve'` and inert unless the server runs in `e2e` mode or with
`PLURY_E2E_STUB=1`, so `pnpm dev`, `pnpm tauri dev` and every production build
are untouched. In e2e mode it prepends `browser/ipc-stub.ts` to `index.html`,
which installs `window.__TAURI_INTERNALS__` before the app boots. The app then
believes it runs inside Tauri: every command, event, file dialog and the
webview's drag-and-drop go to `browser/fake-backend.ts`.

The fake backend mirrors the IPC contract (command names, argument names, wire
shapes, `IpcError` rejections), not the Rust implementation:

- printers, discovered printers, printer prefs and policy from the fixtures;
- profiles with create/update/delete, the default flag and its delete guard;
- settings with the server's engine bounds (out-of-range values are refused);
- `inspect_files` by extension (`*missing*` is not found, unknown extensions
  are unsupported) and two fake folders, `/Users/e2e/Documents/Invoices` (has a
  subfolder) and `/Users/e2e/Documents/Flat`;
- a queue whose jobs advance on a timer (queued → converting → converted →
  printing → done, one printing at a time; a file named `*fail*` fails),
  emitting `queue-updated` and `job-progress-event`, honouring pause/resume,
  cancel, retry, clear and `adjust_job` (a paused job's printer or sides);
- `install_dependency` with the same consent contract as the app: without
  `consent: true` it is refused, with it the "page opens" and the dependency
  stays missing (nothing is installed from a browser).
- the browser bridge's state for Settings > Browser integration
  (`bridge_status`: Chrome registered, Edge and Brave found but not
  registered, Chromium absent) and `bridge_reconnect`, which registers every
  browser found (FR-127).

A command it does not model rejects with `Internal: not stubbed …`.

Drive it from the page (DevTools, Playwright, the console):

```js
__PLURY_E2E__.backend.openPaths = ['/Users/e2e/a.pdf']; // next file picker result
__PLURY_E2E__.dropFiles(['/Users/e2e/b.pdf']); // a native drop
__PLURY_E2E__.forwardSelection({
  paths: ['/x.pdf'],
  action: 'print-default',
  profile_id: null,
});
__PLURY_E2E__.backend.tick(); // advance the queue by hand
__PLURY_E2E__.calls(); // every command the app invoked
```

| URL flag           | Effect                                                |
| ------------------ | ----------------------------------------------------- |
| `?e2e-onboarded=1` | Skip the onboarding overlay                           |
| `?e2e-seed=1`      | Start from the fixture queue (one job in every state) |
| `?e2e-tick=0`      | Stop the timer; advance with `backend.tick()`         |

### A batch from PluryFact (FR-123/FR-124)

What the browser bridge hands the app is an ordinary selection with
`source: 'pluryfact'`, PluryFact's label and the print settings it asked
for. Park it and ring the doorbell, as the backend would:

```js
const h = __PLURY_E2E__;
h.backend.pendingSelections.push({
  paths: [
    '/Users/e2e/Library/Application Support/com.Maecly.Pluryprint/inbox/019a/0000-RIDE_001-001-000000123.pdf',
  ],
  action: 'configure',
  profile_id: 'builtin-duplex-bw',
  source: 'pluryfact',
  label: 'Recibidos · octubre 2026',
  overrides: {
    copies: 2,
    color: 'monochrome',
    duplex: 'long-edge',
    paper: 'a4',
  },
});
h.forwardSelection({ paths: [], action: 'configure', profile_id: null });
```

The Print Job screen opens with the file, a "From PluryFact" badge and the
label next to the summary, Duplex BW selected, and two copies, black and
white, long edge and A4 pre-filled; nothing is printed. Settings > Browser
integration lists the four browsers, and Reconnect turns Edge and Brave to
Connected. Checked by hand with Playwright on 2026-11-14.

What it does **not** prove: anything below the IPC boundary. Printing,
spooling and conversion are covered by the Rust suites, including the two
opt-in real-spooler suites below.

## Real-spooler suites (opt-in, never print)

All three are `#[ignore]`d **and** gated on `MAECLY_REAL_PRINTER=1`, go through the
engine's real `spool` path, and refuse to submit to any queue that would print:

| Suite                                      | Platform      | Queue (`MAECLY_HOLD_QUEUE`)                                                   | What it proves                                                                                                                                                                                                                                                                                         |
| ------------------------------------------ | ------------- | ----------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `src-tauri/tests/real_printer.rs`          | macOS / Linux | `Maecly_Hold`, a CUPS queue with `job-hold-until-default=indefinite`          | CUPS turns the `lp` vector into the intended IPP job attributes; every job stays `pending-held` and is cancelled on the way out                                                                                                                                                                        |
| `src-tauri/tests/real_printer_windows.rs`  | Windows       | `Maecly_Hold_Win`, a **paused** queue on a file port                          | SumatraPDF spools, the job is found by its document among the user's new jobs, `GetJob` reads its state, and the cancel removes it                                                                                                                                                                     |
| `src-tauri/tests/print_quality_windows.rs` | Windows       | `PLURYPRINT_QUALITY_QUEUES` (default `Maecly_Hold_Win`), every one **paused** | Draft/High reach the job's own DEVMODE through the per-user default swap, the user's default comes back byte for byte (or is cleared again), a killed process is recovered, two processes keep their own quality, `paperkind=`/`bin=` ids land in the job, a no-print PDF is refused before SumatraPDF |

```sh
MAECLY_REAL_PRINTER=1 cargo test --test real_printer -- --ignored              # macOS, Linux
$env:MAECLY_REAL_PRINTER='1'; cargo test --test real_printer_windows -- --ignored --test-threads=1   # Windows
```

The doc comment at the top of each file has the one-time command that creates
the hold queue. Never point `MAECLY_HOLD_QUEUE` or `PLURYPRINT_QUALITY_QUEUES`
at a real device.

## Windows VM scripts

Two PowerShell scripts in `scripts/vm/` run checks that need a real Windows
desktop or spooler, and were run on the Windows 11 ARM64 test VM in round 3:

- `print-quality-e2e.ps1` creates the paused queues (a file-port queue on the
  Microsoft IPP Class Driver, optionally one bound to an `ippeveprinter` URL)
  and runs `print_quality_windows`.
- `r3-contextmenu-e2e.ps1` opens Explorer on a file, right-clicks it and
  reaches the classic menu through **Show more options** with **real mouse
  input**, prints the Pluryprint submenu as Explorer built it, can click an
  entry, and takes a screenshot with the menus. Real input is the point:
  activating "Show more options" through UI Automation builds a menu whose
  submenus all open empty, Windows' own included
  ([shell-extensions.md](../docs/04-architecture/shell-extensions.md)).

## Worker binaries

`src-tauri/tests/render_worker.rs` and `src-tauri/tests/svg_worker.rs` drive the
freshly built `pluryprint --worker render-pdf` and `--worker svg` the way the
engine does: HTML, Markdown, text and RTF pages through the system web view
(pagination, paper, relative images, a hostile page that must not reach a local
listener, two workers in parallel) and SVG drawings to vector PDF. They run in
the normal `cargo test`.
