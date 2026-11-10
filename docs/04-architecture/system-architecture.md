# System Architecture

> **TL;DR** — Pluryprint is a Tauri 2 app made of (a) a small **Rust** COM DLL that Windows Explorer loads for the right-click entry (on macOS, an `NSServices` entry inside the app bundle; on Linux, desktop-entry files the file manager reads), (b) **one Rust executable** that is the GUI, the headless CLI, and — started with `--worker` — the hidden worker that renders documents, and (c) the processes it runs for a job: LibreOffice, `sips`/`textutil` on macOS, a HEIF decoder on Linux, and the final printer (SumatraPDF on Windows, CUPS `lp` on macOS and Linux). Linux is a preview platform from 0.0.5 ([ADR-0015](../adr/0015-linux-support.md)). The DLL hands the selection over in a JSON file named on the command line; the webview talks to Rust through typed Tauri commands; Rust drives every child process with an argument vector, a timeout and the job's cancel flag.

This page is the map. The details live in [print-engine.md](./print-engine.md), [shell-extensions.md](./shell-extensions.md), [ipc-and-cli.md](./ipc-and-cli.md), [auto-updater.md](./auto-updater.md) and [security-model.md](./security-model.md).

## 1. Context

```
                    End user
     (right-clicks files in Explorer / Finder / Dolphin / Nemo,
      or opens the app, or runs `pluryprint print …` from a script)
                        |
                        v
+-------------------------------------------------------------+
|                         Pluryprint                          |
|   Explorer DLL / Finder service  ·  app + CLI  ·  workers   |
+------+---------------+-------------------+------------------+
       |               |                   |
       v               v                   v
+-------------+  +--------------+  +--------------------------+
| OS spooler  |  | LibreOffice  |  | github.com releases      |
| (Windows    |  | (installed   |  | latest.json per channel, |
|  spooler /  |  |  by the user)|  | signed update files      |
|  CUPS)      |  +--------------+  +--------------------------+
+------+------+
       |
       v
   Printer
```

- **OS print spooler.** The Windows Print Spooler or CUPS. Pluryprint never talks to a driver directly: SumatraPDF or `lp` hands the PDF to the spooler, and the job is followed there until it prints or ends unconfirmed.
- **External programs.** SumatraPDF is bundled with the Windows installer. LibreOffice (Office, OpenDocument, RTF on Windows and Linux) and a HEIF decoder (the HEIF/HEVC Store codecs on Windows; libheif's tools or ImageMagick on Linux) are installed by the user, by hand or from the app's consent dialog (on Linux, a web link). Everything else is in the app or the OS: images, SVG, Markdown and text are converted in-process; HTML, Markdown and text are paginated by the system web view (WebView2 / WKWebView / WebKitGTK) in a worker; `sips` and `textutil` ship with macOS; on Linux the CUPS client tools come with the distribution.
- **GitHub releases.** The only host Pluryprint contacts on its own: the update manifest of the user's channel, and, when the user asks, the signed update it names ([ADR-0011](../adr/0011-release-channel.md)). There is no update server, no crash reporter and no telemetry endpoint.

## 2. Processes

```
 explorer.exe                          pluryprint.exe (GUI, single instance)
+---------------------------+          +-------------------------------------+
| shell_ext_win.dll (Rust)  |          |  Webview (React + TS)               |
|  IExplorerCommand         |  spawn   |   WebView2 / WKWebView               |
|  writes handoff JSON  ----+--------> |        ^  typed Tauri commands      |
+---------------------------+  (a 2nd  |        v                            |
                               launch  |  Rust core: queue dispatcher,       |
 pluryprint-cli.exe / `pluryprint      |  engine, profiles, settings,        |
 print …` (headless, no window)  forwards it)  printers, updater           |
                                       +---------+---------------+----------+
                                                 | spawn         | spawn
                                                 v               v
                              +--------------------------+  +------------------+
                              | pluryprint --worker      |  | soffice, sips,   |
                              |   render-pdf | svg       |  | textutil,        |
                              | (hidden, one per file)   |  | SumatraPDF / lp  |
                              +--------------------------+  +------------------+
```

- **Explorer DLL** (`shell-ext-win/`, Rust `cdylib`) runs inside `explorer.exe`. It only reads the selection, writes it to a JSON handoff file and starts `pluryprint.exe --invoke=context-menu --files-from <file> --action <action> [--profile <id>]`; it never prints and never reads file contents. It is registered per user as an `ExplorerCommandHandler` verb ([ADR-0013](../adr/0013-explorer-command-per-user-verb.md)). On macOS the same entry point is the in-bundle Finder service. On Linux it is a desktop entry: the `.desktop` file's `MimeType=` list puts Pluryprint under **Open with** in every file manager, and the `.deb` adds a Dolphin service menu and Nemo actions (**Print now**, **Configure print job**, **Add to print queue**) that start `pluryprint` with the selected paths.
- **The app** is one executable. `main` decides from `argv` alone, in this order: a `--worker` request runs the worker and exits; a CLI subcommand runs headless with no window; otherwise the GUI starts. The GUI is single-instance (`tauri-plugin-single-instance`): a second launch forwards its selection, which the window drains once with `take_pending_selections`, and exits.
- **The console CLI** `pluryprint-cli.exe` (Windows) exists so shells wait for the exit code; on macOS and Linux the app binary is the CLI (`pluryprint` on `PATH` from the `.deb`; for an AppImage, the AppImage file itself). The CLI runs the same engine directly, with a private cache, no queue and no retries.
- **Workers** are the same executable started with `--worker render-pdf` (HTML, Markdown, text, RTF on macOS, through the system web view) or `--worker svg`. Each is bounded by the conversion timeout and the job's cancel flag; a hostile file that crashes or wedges it ends the worker, not the app ([ADR-0012](../adr/0012-system-web-view-renderer.md)).
- **Other children** (LibreOffice, `sips`, `textutil`, SumatraPDF, `lp`) are short-lived, run as the user, without a shell, in a private scratch folder, and are killed with their process tree on timeout or cancel.

## 3. The Rust core

The real module layout of `src-tauri/src/`:

```
main.rs              entry: worker → CLI → GUI
lib.rs               Tauri builder, plugins, the 47 registered commands and 6 events
bin/pluryprint-cli.rs  console-subsystem front end (Windows)
cli/                 clap surface, exit codes, handoff, single instance, pending selections,
                     foreground hand-off for a forwarded selection
ipc/                 tauri::command handlers: diag, printers, jobs, pdf_password,
                     profiles, settings, update, deps_install, notify, events
queue/               job model, single-writer dispatcher, queue.json persistence
engine/              SUPPORTED_EXTS, dependency resolver (deps.rs), page ranges
  convert/           router + one module per converter: image, heif, svg, office,
                     html, markdown, text, rtf_mac, pdf_security, process (bounded runner)
  print/             cups.rs, sumatra.rs, windows.rs (spooler tracking),
                     devmode_swap.rs (Windows quality and colour, D16)
render/              web view printing for the render worker (windows.rs, macos.rs;
                     a WebKitGTK backend on Linux)
worker/              the --worker dispatch: render, svg
platform/            printer listing, defaults, add printer, test page, DNS-SD,
                     macOS Finder service and handoff
profiles/ settings/  profiles.toml, config.toml, policy.json
update/              manifest check, install policy around tauri-plugin-updater
logging/             JSONL logs with daily and size rolls
shell_menu.rs        writes the localized shell-menu.json the DLL reads
util/                errors and reasons, paths, MaeclyPrint migration, Chromium profile seeding
```

There are no `Converter` or `Printer` traits and no plugin registry: `engine::convert::route` is a match on the extension, and each converter is a blocking function the dispatcher runs off the async runtime. That keeps every step testable as a plain function; the Windows-only FFI is kept thin and the logic around it is pure and tested on every platform.

## 4. Sequence — right-click → print

```
User       Explorer      DLL              pluryprint (running)        engine / children       spooler
 | right-click |          |                       |                          |                    |
 |------------>| Invoke   |                       |                          |                    |
 |             |--------->| write handoff JSON,   |                          |                    |
 |             |          | start pluryprint.exe  |                          |                    |
 |             |          |---- 2nd launch ------>| selection forwarded,     |                    |
 |             |          X   (exits at once)     | window raised, drained   |                    |
 |             |                                  | once                     |                    |
 |   Print N files (default profile): submitted at once; Configure…: the Print Job screen    |
 |                                                | print_files ------------>| per file:          |
 |                                                |                          | convert (worker /  |
 |                                                |                          | LibreOffice / in-  |
 |                                                |                          | process) → PDF     |
 |                                                |                          | SumatraPDF / lp -->|
 |                                                |<-- queue-updated --------| follow the OS job  |
 | progress, toasts, OS notifications             |                          | until it leaves    |
```

- The DLL is gone before the window comes forward, so Explorer stays responsive. The forwarding process passes its foreground right to the running window instead of faking input, and the window leaves menu mode before it acts on the selection ([shell-extensions.md](./shell-extensions.md)).
- A selection of any size travels as a file, so there is no 15-item or command-line-length limit (FR-003).

## 5. Concurrency

- **`tokio` runtime** for the GUI's core. The queue is a single-writer **dispatcher task** fed by an `mpsc` channel: IPC handlers send commands, the dispatcher owns the state and persists `queue.json` after each transition.
- **Conversions** run on `spawn_blocking`, at most `engine.max_concurrent_conversions` at a time (default 2, 1–8). **Printing** is one job at a time.
- A conversion or print that overruns its timeout is killed and fails without retry; other failures retry up to three times with 1 s / 5 s / 25 s backoff; a failure the user must fix (missing dependency, PDF password, unsupported setting, missing printer) **pauses** the job.
- The headless CLI does not start the dispatcher; it holds the app's lock only while it edits `queue.json`.

## 6. What crosses each boundary

| Boundary | Format | Notes |
| --- | --- | --- |
| Explorer → DLL | `IShellItemArray` | Native COM |
| DLL → app | `--files-from <handoff.json>` plus `--action` / `--profile` | At most 10,000 paths; a larger selection says how many were cut (`truncated_from`) |
| Second launch → running app | single-instance plugin, then `take_pending_selections` | Drained once, never twice |
| Webview ↔ Rust | Tauri IPC, typed by `src/ipc/bindings.ts` | Errors carry a stable `code` + `reason` + `params`, translated in the UI |
| Rust → converters and workers | argument vector, private scratch folder | Output is a PDF file; stderr is kept as the technical detail |
| Rust → printer (Windows) | `SumatraPDF.exe -print-to … -print-settings …` | Paper and tray as driver ids; quality, and colour for PrintTicket drivers, through the per-user default swap |
| Rust → printer (macOS) | `lp -d … -o …` | CUPS options; the job id from `lp`'s reply |
| Rust → GitHub | HTTPS GET of `latest.json`; on request, the signed update | minisign verified before install |
| Rust → disk | JSONL logs, `queue.json`, `config.toml`, `profiles.toml` | Atomic writes (temp file + rename) |

## 7. Failure isolation

| Failure | Effect | Mitigation |
| --- | --- | --- |
| Explorer DLL panics | Caught at the COM boundary | `panic = "unwind"` in its own release profile; no file contents read |
| App killed mid-job | Jobs resume on the next start | `queue.json` written after each transition; a job already in the OS queue is followed, never spooled twice |
| A hostile HTML page or SVG crashes its parser | The worker dies | The job fails with `render_failed` / `svg_*`; the app keeps running |
| Converter hangs | Killed at the timeout with its process tree | Terminal `conversion_timeout`, no retry |
| Printer offline, out of paper, or the job is held | Shown from the spooler's status | The job keeps waiting until the print timeout, or ends `unconfirmed` |
| Process killed during the Windows quality swap | The user's default DEVMODE stays swapped | A journal restores it on the next swap or start |
| Update download fails or is tampered with | Nothing is installed | Signature checked before install |

## 8. Related documents

- [`tech-stack.md`](./tech-stack.md) — the technology choices.
- [`shell-extensions.md`](./shell-extensions.md) — the DLL, the Finder service and their registration.
- [`print-engine.md`](./print-engine.md) — the converter and printer pipeline.
- [`ipc-and-cli.md`](./ipc-and-cli.md) — every IPC command, event and CLI flag.
- [`data-model.md`](./data-model.md) — config, profile, queue and log schemas.
- [`security-model.md`](./security-model.md) — trust boundaries and network egress.
