# Security Model

> **TL;DR** — Pluryprint signs its **update** artifacts with its own minisign key (installers carry no Apple or Microsoft certificate, [ADR-0011](../adr/0011-release-channel.md)), runs without admin privileges, isolates the high-risk shell-extension code in a tiny Rust COM DLL, prints untrusted documents in worker processes with no network and no file-share access, keeps the webview's Rust-facing surface closed, and ships with telemetry **off** by default (and, in this version, inert). The only outbound network destination of its own is **`github.com`**, for the update manifest of the user's channel and, on request, the signed update it names — there is no `updates.maecly.com`.

## 1. Threat model

We start by being explicit about what we are defending against and what is out of scope.

### 1.1 In scope

| Threat                                                         | What we do about it                                                  |
| -------------------------------------------------------------- | -------------------------------------------------------------------- |
| Malicious update binary served by a compromised host           | An update is downloaded only on the user's request and installed only when its minisign signature matches the public key compiled into the app; `release.yml` verifies every signature before a draft is published (§2.3). |
| Modified installer downloaded from a tampered mirror           | Download from the GitHub release only; every release lists `SHA256SUMS`. No Authenticode or Developer ID signature today (ADR-0011), so the OS cannot check it |
| Hostile update manifest from `github.com` (or a MITM on it)       | The check parses it into two strings (version, notes) and never executes it; an install refuses any artifact whose minisign signature does not match the compiled-in key. A `200` that does not parse is an error, not "up to date". The link the user is offered is re-checked against the HTTPS allow-list before `open_external` will open it. |
| Malicious context-menu invocation tricking the user into running arbitrary code | Shell DLL only handles paths; main exe validates each path before processing |
| Webview compromise (XSS in a render path) escalating to arbitrary IPC calls | Closed command set (`collect_commands!`, no dynamic surface) + CSP + no remote content loaded |
| Local user without admin rights wanting to install / use the app | Install to `%LOCALAPPDATA%`, register COM in HKCU, no service required |
| File-content exfiltration via logs                             | Logs never include file content; paths only at INFO+ when explicitly enabled |
| User wanting to audit network behavior                         | Single outbound endpoint by default; everything else is opt-in        |
| Supply-chain attack on a Rust crate                            | Pinned `Cargo.lock`, `--locked` builds, Dependabot; no `cargo audit` workflow yet (NFR-065) |
| Supply-chain attack on an npm package                          | `pnpm audit`, lockfile-only installs in CI, dependency review on bumps |
| Privilege-escalation exploit in a converter (e.g., LibreOffice CVE) | Converters spawned as the current user, killed at timeout, never as SYSTEM |
| A document that leaks the user's IP or NTLM credentials when printed (remote picture, `\\host\share` link, tracking pixel) | Render and SVG workers refuse every request outside the document's folder; LibreOffice's profile blocks linked resources and points HTTP at a dead proxy (§3.4) |
| A hostile file that crashes a parser | SVG and HTML/Markdown/text are parsed and rendered in worker processes; a crash ends the worker, not the app |

### 1.2 Out of scope

- A user with administrator access on their own machine. (No defense against the local admin; that is the OS's job.)
- Targeted attacks against a specific user with physical access.
- Cryptanalytic attacks on Ed25519 / SHA-256 / minisign.
- Print-content interception by the local network (if the user prints to a network printer over an insecure protocol, that is the spooler's concern, not ours).
- Side-channel attacks (timing, EM, etc.).

## 2. Code signing

> **Status: §2.1 and §2.2 are not in use.** [ADR-0011](../adr/0011-release-channel.md) releases without OS certificates: the Windows installers are unsigned (SmartScreen warns; the release notes explain **More info → Run anyway**), the macOS bundles are signed ad hoc and not notarized (the notes give the `xattr` step), the Linux AppImage and `.deb` carry no GPG or distribution signature ([ADR-0015](../adr/0015-linux-support.md)), and every release attaches `SHA256SUMS`. What *is* signed is every update artifact (§2.3). §2.1/§2.2 are the plan for when certificates are bought; [code-signing.md](../07-build-and-release/code-signing.md) has the same status note.

### 2.1 Windows: EV cert + Authenticode (planned)

From day 1 (`v0.1.0` GA), all release artifacts are signed with an **Extended Validation** code-signing certificate. We use Sectigo or DigiCert depending on price; both produce certs that count toward Microsoft SmartScreen reputation immediately, removing the "Unknown publisher" warning.

Signed artifacts:

- The main executable: `Pluryprint.exe`.
- The shell-extension DLL: `shell_ext_win.dll` (the `cdylib` produced by the `pluryprint-shellext` crate).
- The Windows installer produced by `tauri build` (NSIS, `installMode: currentUser`).
- The Sparse MSIX package: `Pluryprint.msix` (signed via `signtool`).

Signing happens in the GitHub Actions release workflow on a Windows runner with the EV key residing in an Azure Key Vault HSM accessed through `signtool` and the Azure Code Signing extension. The key never lives on disk in CI.

Cost is ~$300/year for an EV cert. For nightly builds we sign with a self-signed cert (the user gets a SmartScreen warning; nightly users accept this).

### 2.2 macOS: Developer ID + notarization (planned)

From the first macOS release:

- The `.app` bundle and `.appex` (Finder Sync Extension) are signed with **Apple Developer ID Application**.
- The `.dmg` is signed with **Apple Developer ID Installer**.
- Both are notarized via `notarytool`, then stapled.

Signing keys live in macOS Keychain on the macOS runner, fetched from a 1Password / GitHub Actions secret at the start of the job, scrubbed at the end.

### 2.3 Update integrity

See [`auto-updater.md`](./auto-updater.md) for the full flow. The security-relevant summary for the **shipped** build:

- **The in-app updater installs only signed artifacts, and only on request.** `tauri-plugin-updater` downloads the artifact named by `latest.json` and verifies its minisign signature against the public key compiled in from `tauri.conf.json` (`plugins.updater.pubkey`) before a byte reaches the disk. A tampered artifact fails with `UpdateSignatureInvalid`. Install runs only after an explicit user action (About → Install now, `update install`), a major version needs a second confirmation, and release builds refuse non-`https` endpoints. The private key exists only offline and in the release secret `TAURI_SIGNING_PRIVATE_KEY` ([ADR-0008](../adr/0008-installer-nsis-per-user.md)).
- **Residual risk.** The manifest itself is not signed. Each update signature carries the version it was signed for (`tauri signer sign --app-version`, in the signed trusted comment) and the updater has `requireSignedVersion` on, so whoever can serve `latest.json` over the GitHub TLS endpoint can no longer pair a newer version number with an older, genuinely signed artifact; they can still withhold an update by serving an older but consistent manifest. A leaked private key is the critical case: rotate it and ship the new public key in a release signed with the old one.
- The update *check* is a read-only `GET` of the same `latest.json` the installer uses, from `github.com` release assets. The only thing it can influence is a version string and a markdown blob shown to the user; the link shown is built from the version (`…/releases/tag/vX.Y.Z`).
- `release.yml` signs every updater artifact on a tag build, then verifies each signature against the public key compiled into the app, and checks that a tampered copy fails, before it finishes the draft. The private key is read only from the `TAURI_SIGNING_PRIVATE_KEY` GitHub secret.
- The URL is not opened automatically. Opening it goes through `open_external`, which enforces HTTPS and an exact-host allow-list (`github.com`, `maecly.com`, `*.maecly.com`; no userinfo, no port, no lookalike suffixes).
- A first install (not an update) has no OS-verified signature today (§2 status). Its integrity rests on GitHub's TLS and the `SHA256SUMS` attached to the release, which the user can check by hand.

Artifacts are minisign-signed; the manifest is not (see the residual risk above). Key rotation: generate a new pair, ship a release signed with the **old** key that carries the **new** public key, then sign everything after it with the new key.

## 3. Sandboxing and process boundaries

### 3.1 Shell-extension DLL

The DLL runs **in the address space of `explorer.exe`**, which is the most dangerous place possible for code: a crash brings down Explorer; a vulnerability exposes the entire user session.

It is written in **Rust**, not C++: the crate `pluryprint-shellext` in `shell-ext-win/`, built as a `cdylib` implementing `IExplorerCommand` via the `windows` crate's `implement` feature.

Mitigations:

- **Memory safety by construction.** The COM boundary is the only `unsafe` surface; the path handling above it is safe Rust.
- **Tiny code surface.** ~1,800 lines across fourteen small modules, with unit tests co-located and two integration tests (`handoff_roundtrip`, `strings_cache`). Auditable in an afternoon.
- **Panics never cross the COM boundary.** Every entry point is wrapped by `guard::guard()`, which `catch_unwind`s and returns an HRESULT. This is why the crate pins `panic = "unwind"` in **both** its `dev` and `release` profiles, and why it is deliberately *not* a workspace member: `src-tauri`'s `panic = "abort"` would silently turn `catch_unwind` into a no-op and a panic would take Explorer down.
- **Bounded file I/O only.** The DLL reads paths from `IShellItemArray` and writes one handoff JSON file under the **per-user data directory** (`<data dir>/handoff`), never a world-writable temp dir — so another user cannot plant a payload. It never opens, reads, or modifies the user's documents. The consumer independently refuses any handoff path that does not canonicalize inside that directory, and any payload that is stale, empty, over-long or of the wrong schema version.
- **The handoff directory is derived, not hardcoded.** Both sides resolve it through the same `directories::ProjectDirs::from("com", "Maecly", "Pluryprint")` call, which is what guarantees producer and consumer agree on the path.
- **No networking.** The crate has no HTTP dependency; its whole dependency set is `serde`/`serde_json`, `uuid`, `time`, `directories`, and the Windows bindings.
- **Hardened release profile:** `lto = true`, `codegen-units = 1`, `opt-level = "s"`, `strip = true`.

### 3.2 Tauri main process

Runs as the user; not elevated. Has read/write access to:

- `%APPDATA%\Maecly\Pluryprint\` and `%LOCALAPPDATA%\Maecly\Pluryprint\`.
- The user's documents (because we are spawned by the user; no extra access).

Does **not**:

- Bind to any port.
- Modify `HKLM` or `%PROGRAMFILES%`.
- Write to any other user's files.
- Self-elevate.

### 3.3 Webview

The webview's Rust-facing surface is bounded two ways. **First, the closed command set:** the only commands that exist are those we register in `tauri_specta::collect_commands!` (`src-tauri/src/lib.rs`) — there is no dynamic or third-party command surface, so a compromised webview cannot reach a command we did not write. (Tauri 2's capability system gates *core and plugin* permissions — window, webview, dialog, shell — **not** app-defined `#[tauri::command]`s; `capabilities/main.json` therefore lists only `core:*` defaults plus `dialog:default`. See [`ipc-and-cli.md`](./ipc-and-cli.md) § 2.5.) **Second, and the real backstop:** there is no attacker-controlled code in the webview to abuse that surface — a strict CSP loads only the bundled SPA and forbids remote content, so a hypothetical XSS has no remote script to run.

CSP for the main window, verbatim from `src-tauri/tauri.conf.json` (`app.security.csp`):

```
default-src 'self';
img-src 'self' data: asset: http://asset.localhost;
style-src 'self' 'unsafe-inline';
font-src 'self' data:;
connect-src 'self' ipc: http://ipc.localhost
```

Notes on what this does and does not say:

- `connect-src` allows **no external origin**. The update check is made by Rust, not by the webview, so the webview never needs to reach the network — and cannot.
- `style-src 'unsafe-inline'` is required by Tailwind's generated styles; `font-src … data:` covers the inlined bundled font.
- `asset:` / `http://asset.localhost` is Tauri's own protocol for serving local files to the webview.
- There is no explicit `script-src`, `object-src` or `frame-ancestors` directive; `default-src 'self'` covers scripts, and the remaining two are a hardening gap worth closing.

We never `loadUrl()` remote content. The webview only loads the bundled SPA.

### 3.4 Converter subprocesses

In this version the only conversion that runs is **image → PDF, and it runs in-process** in pure Rust (`engine/convert/image.rs`, the `image` + `lopdf` crates). No external converter is spawned, so the converter-CVE row in §1.1 is currently vacuous — a property worth keeping in mind when the Office/HTML/Markdown converters land.

Processes the engine spawns for **printing** are — `lp` on macOS/Linux, `SumatraPDF.exe` on Windows — via `std::process::Command`:

- As the current user, never elevated.
- Bounded by a hard timeout enforced with the `wait-timeout` crate, so a hung child cannot wedge the queue; a timeout is reported as a print failure (exit code 9 on the CLI).
- With stdout/stderr captured, so the spooler's own message reaches the user rather than being invented by us.
- On Windows, with `CREATE_NO_WINDOW`, so a GUI-subsystem launch never flashes a console.

The image decoder is hardened against decompression bombs independently of any subprocess boundary: `image::Limits` caps decoded dimensions, and the emitted page is clamped to the 14,400 pt (200 in) PDF page ceiling.

The converters are a far larger attack surface than `lp`, because they parse documents someone else may have written. Since round 3 ([ADR-0012](../adr/0012-system-web-view-renderer.md)) they are LibreOffice (Office, OpenDocument, RTF on Windows), `sips` and `textutil` on macOS, and two **workers** of Pluryprint's own executable: `--worker render-pdf` (HTML, Markdown, text, RTF on macOS, through WebView2/WKWebView) and `--worker svg` (`usvg` + `svg2pdf`). All of them go through `engine::convert::process` (`run_bounded` / `run_until`):

- each gets a **dedicated scratch folder** in the OS temp directory. It holds LibreOffice's throwaway `-env:UserInstallation` profile, the staged page and the web view's private profile, and it is removed on every exit path. Every Chromium-based profile (the render worker's WebView2 and the app's own `EBWebView`) is seeded so that it does not try an empty-password logon for the user, which on Windows counts as a failed logon toward account lockout (see [print-engine.md](print-engine.md));
- each runs under a **deadline and the job's cancel flag**, and on either the **whole process tree** is killed (process group on Unix, `taskkill /T` on Windows). A parser that panics or a page that wedges the web view takes the worker down, not the app;
- **no network and no file share**, enforced by the web view's own network layer, never by a scan of the HTML: the render worker turns scripts off; on Windows a request filter on `*` answers every request itself (the document's folder from a private `https` origin, `data:`/`blob:`, a logged 403 for anything else) and served files are checked on their open handle, so no `file://host/share` or `//host` reference ever opens SMB/WebDAV and hands out the user's NTLM credentials; on macOS the worker answers its own private scheme with a `WKURLSchemeHandler` that serves the staged page and files inside the document's folder (opened without following any symbolic link out of it, checked by `F_GETPATH` on the descriptor they are read from), a content rule list admits only that origin, `data:` and `blob:` (every `file:` URL is blocked), and a navigation delegate cancels every navigation the worker did not start. Round 6 had relied on a `file:` URL prefix, which a symbolic link inside the folder passed (round 7). The headless-browser fallback, which loaded pages from `file:` behind a text pre-pass, was removed in round 6 because that pre-pass could not be made to agree with a real HTML parser (security notes 5-7); the SVG converter loads `<image>` only from the drawing's folder; LibreOffice's profile is seeded to block linked resources, never update links, disable macros and OLE/DDE, and send its own HTTP to a dead proxy;
- the render worker runs under a **memory cap** too since round 7 (2 GiB for the worker and the web view processes it starts together: a Windows Job Object with `JOB_OBJECT_LIMIT_JOB_MEMORY` and kill-on-close; on macOS a 20 ms watchdog over the worker's process group, which does not reach WebKit's own XPC web-content process; on Linux the same watchdog adds up the resident set of every process in the group, read from `/proc`). Running out of memory is the terminal `page_too_large`;
- the SVG worker additionally runs under a **hard memory cap** (1 GiB, `process::run_capped`): a Windows Job Object with `JOB_OBJECT_LIMIT_PROCESS_MEMORY`, kill-on-close and one active process; `RLIMIT_DATA` plus a 20 ms memory watchdog in the parent on macOS/Linux (macOS does not enforce `RLIMIT_DATA`/`RLIMIT_AS` on `mmap`). Running out of memory is the terminal `svg_too_large`, running out of time the terminal `tool_timed_out`, and the PDF it writes is capped at 512 MB. Its pre-checks (byte, entity and node caps, a bounded-work expansion walk that follows `<use>`, markers, patterns, masks, clip paths and filters, raster and filter pixel caps) only make an oversized drawing fail fast; the worker is the boundary. Its only door to files is usvg's `ImageHrefResolver`, which judges the exact string usvg parsed with the shared `local_ref` predicate;
- LibreOffice and `textutil` work on a private copy of the input, so a hostile file name never reaches their command line.

**Encrypted PDFs.** A user's PDF password is passed to `lopdf` in-process, held in a buffer zeroed on drop, kept in the dispatcher's memory only while the job needs it, and never written to `queue.json`, a log, a command line or an environment block (FR-059). The CLI takes it from a file or a pipe, never from `argv`. A PDF whose permissions forbid printing is refused, never stripped.

**Installing a missing dependency** (`install_dependency`, decision D13) runs `winget` or `brew` only after the user confirmed the exact command shown, with an argument vector, no shell, no console window and a 30-minute bound; Store and web options open with the OS handler. On Linux every option is a web link: the app runs no package manager and asks for no administrator password to install a dependency.

**Still a gap:** environment minimization. Converters inherit the app's environment.

## 4. Permissions

### 4.1 No admin required

Installation:

- The bundler is Tauri's **NSIS** target with `installMode: "currentUser"` (`tauri.conf.json`), i.e. a per-user install under `%LOCALAPPDATA%` — not Inno Setup.
- The Explorer command is registered per user: the COM class under `HKCU\Software\Classes\CLSID\{B269B8DC-…}` and the `ExplorerCommandHandler` verbs under `HKCU\Software\Classes\*\shell\Pluryprint` and `Directory\shell\Pluryprint` ([ADR-0013](../adr/0013-explorer-command-per-user-verb.md)). The sparse MSIX package, also per user, is not built yet.
- The one install that asks for elevation is LibreOffice's own MSI, when the user chooses to install it from the dependency dialog; the dialog says so before anything runs.
- No services, no scheduled tasks, no drivers.

Operation:

- All read/write to user-writable locations.
- No `runas` prompts during normal use.

The one exception is **uninstalling the WebView2 Runtime** — but we don't ship that; we use the system one and bootstrap it on Win 10 if missing (the bootstrapper is a Microsoft binary that handles its own elevation).

### 4.2 macOS and Linux

**macOS.**

- App installs to `/Applications` (user can drag-drop without admin if not present).
- All state in `~/Library/Application Support/com.Maecly.Pluryprint/` and friends.
- The Finder integration is an `NSServices` entry inside the app bundle (no Finder Sync extension), so there is nothing extra to enable or sign.

**Linux** (preview, [ADR-0015](../adr/0015-linux-support.md)).

- The AppImage runs from wherever the user put it, with no installation and no administrator rights; it registers nothing on its own.
- The `.deb` is installed system-wide by the package manager, which needs administrator rights (`sudo apt install ./Pluryprint_…_linux_x64.deb`). It puts the app, the `.desktop` entry and the Dolphin and Nemo entries under `/usr`, and nothing else.
- An in-app update of a `.deb` install runs `pkexec dpkg -i` on the verified package, so the system asks for an administrator password each time; an AppImage replaces its own file, which must be writable by the user.
- All state in `~/.config/pluryprint/`, `~/.local/share/pluryprint/` and `~/.cache/pluryprint/` (the XDG directories; `XDG_CONFIG_HOME`, `XDG_DATA_HOME` and `XDG_CACHE_HOME` move them).
- Printing, queue management and adding a printer go through the CUPS client tools as the user; whether `lpadmin` needs a password is the system's CUPS policy, not Pluryprint's.
- The AppImage carries its own WebKitGTK, libsoup and GnuTLS, frozen at the versions of the release that built it: the distribution's security updates do not reach them, and the render worker parses untrusted HTML with that WebKitGTK. A new Pluryprint release is the only way they are updated. The `.deb` uses the distribution's copies and gets its fixes, so on Debian and Ubuntu it is the safer package.
- Programs of the host (LibreOffice, the HEIF decoder, the CUPS tools, a printer settings panel) are started without the variables the AppImage sets to point GTK, GIO and GDK at its bundled modules (`util::appimage::host_command`); only Pluryprint's own render worker keeps them.

### 4.3 Enterprise policy (`policy.json`)

An organization can pin a small set of Pluryprint behavior with an optional, admin-managed `policy.json`. The app **reads it but never writes it** — it is intentionally an inbound-only control, so a compromised or misbehaving webview cannot alter organization settings. Today it carries a single field, the enforced default printer. Implemented in `src-tauri/src/settings/policy.rs`; surfaced to the UI via the `get_printer_policy` IPC command.

**Sources** (first hit wins). The UI calls the result *managed by your organization*, so a source is only believed when a standard user could not have written it:

| Platform | Sources, most authoritative first | Fallback (debug builds only) |
| -------- | --------------------------------- | ---------------------------- |
| Windows  | Registry `HKLM\SOFTWARE\Policies\Maecly\Pluryprint`, value `DefaultPrinter` (REG_SZ — Group Policy friendly); then `%ProgramData%\Pluryprint\policy.json`, **only if the file's owner is `BUILTIN\Administrators` or `SYSTEM`** | `<user config dir>\policy.json` |
| macOS    | `/Library/Application Support/Pluryprint/policy.json` | `~/Library/Application Support/com.Maecly.Pluryprint/policy.json` |
| Linux    | `/etc/pluryprint/policy.json` | `<XDG config dir>/pluryprint/policy.json` |

`%ProgramData%` is not admin-only: its default ACL lets any user create a folder there and own it. A `policy.json` a standard user created is therefore owned by that user and **ignored** (logged as `policy_untrusted`); otherwise any user could impose a "managed" default printer on every other account on the machine. A user cannot assign a file's ownership to Administrators without privileges they do not have.

Two more rules close the gaps around that check:

- **The folder must be admin-owned too.** `%ProgramData%\Pluryprint` must itself be owned by Administrators or SYSTEM. A standard user who created that folder first can rename and replace files inside it, so an admin-owned file in a user-owned folder is still ignored. Create the folder as an administrator.
- **One handle for the check and the read.** The file is opened once. Its owner is read from the open handle (`GetSecurityInfo`), and the content is read from that same handle. If the path were checked and read separately, someone who controls the folder could show their own file during the read and put the admin's file back before the owner check.

On macOS and Linux the location is admin/root-only, so it is the main check. The open handle must also be a regular file that is not world-writable, in a folder that is not world-writable.

The per-user config-dir fallback lets developers and testers exercise the locked state without admin rights. It is compiled into **debug builds only**, so in a release build a user cannot fake a managed state for themselves either.

**Schema** — every field optional, so an empty or partial file still deserializes:

```json
{ "default_printer": "<queue id>" }
```

**Read-only semantics.** When `default_printer` is set, the Printers screen shows a lock note and disables **Set default**; policy **wins over user preferences**, giving the effective-default precedence `policy.default_printer ?? prefs.default_printer ?? OS default`. Because the file is only ever read, the lock cannot be lifted from inside the app — only by an admin editing (or removing) the file.

**Failure behavior.** A missing file is not an error — policy is optional and absence means "unmanaged". The file may be UTF-8 (with or without a byte-order mark) or UTF-16 with its mark, which is what Notepad ("Unicode") and PowerShell 5 (`Out-File`, `Set-Content -Encoding UTF8`) write. A malformed file, or a trusted one that cannot be read as text, is **logged with a warning (`policy_parse_failed` / `policy_unreadable`) and ignored** (degrades to no-policy, and later locations are not consulted); it never blocks app startup. The loader is total: it always returns a value, so the UI can rely on a response.

Policy is read **at page load** (each time the Printers screen mounts). There is no daemon watching the file for live changes yet — a change takes effect on the next load/rescan. A background watch is a documented follow-up.

## 5. Network egress audit

On its own, Pluryprint makes exactly **one** request, an unauthenticated `GET` of the update manifest of the user's channel:

```
GET https://github.com/MAECLY/pluryprint/releases/latest/download/latest.json          # stable
GET https://github.com/MAECLY/pluryprint/releases/download/channel-beta/latest.json    # beta, nightly
```

The URLs are compiled in; only `PLURYPRINT_UPDATE_ENDPOINT` (for local testing; release builds require `https`) replaces them. Headers are `User-Agent: Pluryprint/<version>` and `Accept: application/json`. The request bodies are empty and carry no identifier of any kind. The client is `ureq` with a 10-second timeout; a system proxy is honoured from `HTTPS_PROXY`/`HTTP_PROXY` (either case) when set.

Frequency: at most once per 24 h automatically (30 s after launch, off the main thread), plus whenever the user explicitly asks. Disabling `[updater].check_on_startup` removes the automatic one entirely — and the check is skipped *before* any socket is opened, which is asserted by test.

That is everything. The app:

- Does not phone home with usage stats. **This version has no telemetry transmitter at all** — the two consent flags in `[telemetry]` are recorded and never read by any sending code (documented on `TelemetryCfg` itself).
- Does not download or install updates unless the user asks (**Install now**, `update install`); the download then goes to the artifact the same manifest names, on GitHub, and is verified before it is installed.
- Does not install converters on its own: winget, Homebrew, the Microsoft Store or the browser make their own requests only after the user confirmed the dependency dialog (on Linux, only the browser).
- Lets no printed document reach the network or a file share (§3.4).
- Does not load fonts, scripts or styles from CDNs — the CSP forbids it and the one webfont is bundled.
- Does not check for updates of converters.
- Does not communicate with printer manufacturers' clouds.
- Does not contact any Maecly-operated host. Links to `maecly.com`/`github.com` are opened in the **user's browser**, on an explicit click, through the `open_external` allow-list.

There is no `crash.maecly.com` and no metrics endpoint. If either is ever built, this section and [`../09-legal/privacy.md`](../09-legal/privacy.md) must change in the same commit.

## 6. Privacy

### 6.1 What we never collect

Nothing is collected, because nothing is transmitted. In particular the app never sends file contents, file names, printer names, or any user/host identifier — there is no code path that sends anything at all.

### 6.2 Logs

Local-only. Files are named `pluryprint-<UTC date>.jsonl` (and `pluryprint-<date>.<n>.jsonl` for each size roll within one day) under `%LOCALAPPDATA%\Maecly\Pluryprint\data\logs\` (Windows), `~/Library/Logs/Pluryprint/` (macOS) or `~/.local/share/pluryprint/logs/` (Linux, `$XDG_DATA_HOME/pluryprint/logs/`). The user can delete them at any time, and the About screen has a button that reveals the folder.

Rotation and retention are implemented by `logging::RollingJsonl`, not by `tracing-appender`'s rolling appender (which only rotates on a clock boundary and never deletes): the current file rolls when the **UTC date changes or it passes 10 MB, whichever comes first**, and a sweep at startup and on every roll deletes anything older than **14 days** and then keeps only the **7 newest** files. Both bounds apply; whichever bites first wins. There is no `[logging]` section in `config.toml` — these are the whole policy.

Logs are **never** uploaded. There is no "send logs to maintainers" upload path in this version; `report_issue` opens a prefilled GitHub issue in the browser with an environment block, and the user pastes whatever they choose.

Default log level is `info` for every crate (`logging::DEFAULT_FILTER`, overridable with `RUST_LOG`). It used to be `info,pluryprint=debug`, but an `EnvFilter` target is a string prefix, so that directive also matched `pluryprint_lib`, where every event lives, and DEBUG (with full paths) was on by default. At `info`:

- Job IDs, profile IDs, printer names, exit codes, durations.
- File extensions, and a job's **file name without its folder** (`report.docx`): FR-035 requires every job record to identify its file, and the file name is that identifier. Absolute paths and folders are **never** logged at `info`: the shell extension's `shellext.jsonl` records only the handoff file's name, and `log.file` / `log.module_path` (build-machine source paths that `tracing-log` attaches to records bridged from the `log` crate, such as the updater's) are stripped from every record before it is written (`logging::scrub_bridged_log_fields`).

At `debug`:

- File paths included.
- Converter stderr tails included.

At `trace`:

- IPC payloads (still scrubbed for any `password`/`token` fields).

### 6.3 Telemetry consent

Settings → Privacy panel:

```
[ ] Send anonymized crash reports to Pluryprint maintainers
    (off by default; you can change this any time)

[ ] Send anonymous usage metrics
    (off by default; you can read what is sent)

What you'll never send:
  - File contents or filenames
  - Personal identifiers
  - Anything outside this app

[ Read the full privacy notice ]
```

**Both switches are inert in this version.** They persist consent (`[telemetry].crash_reports`, `[telemetry].anonymous_usage_metrics`) so a future build can honour a choice already made; no code reads them to decide whether to send, because no code sends. Turning them on changes nothing about the app's network behaviour.

## 7. Dependency hygiene

- `Cargo.lock` and `pnpm-lock.yaml` are committed. CI installs are lockfile-only.
- `cargo deny check` runs in CI: blocks unmaintained crates, AGPL-incompatible licenses, advisories.
- `cargo audit` runs nightly against RustSec.
- `pnpm audit --audit-level high` runs in CI.
- Dependabot security updates are auto-merged once CI is green; non-security bumps require human review.
- `tauri-specta` / `specta` are pinned with `=` because they are pre-release; a wildcard would let a breaking rc land silently.
- The `objc2*` crates are version-constrained to the ones tao/wry already pull in. Taking a different major would build a **second** copy of the objc2 tree into the same binary — two incompatible views of the Objective-C runtime — which is a correctness and safety hazard, not a preference.

## 8. Vulnerability disclosure

A `SECURITY.md` at the repo root documents:

- The contact email: `security@maecly.com` (PGP key fingerprint published).
- A 90-day coordinated disclosure window by default.
- Acknowledgement targets: 72 h.
- A hall-of-fame credit list for reporters.
- Scope: this repo and the published binaries. Out of scope: the SumatraPDF / LibreOffice / WebView2 / WebKit projects (report upstream) and GitHub's own infrastructure.

Critical issues are patched out-of-cycle and pushed to the `stable` channel within 48 h.

## 9. Build reproducibility

- Rust release builds use `--locked`. The minimum toolchain is pinned as `rust-version` in both `Cargo.toml`s (1.90 for the app, the floor Tauri 2.12 sets; 1.78 for the shell extension); there is **no** `rust-toolchain.toml` in the repo today, so CI's Rust version is whatever the workflow installs — pinning it is an open hardening item.
- Frontend builds use `pnpm install --frozen-lockfile`.
- Build environment is documented (Windows runner image, Node version, pnpm version, Rust version).
- We do **not** claim bit-for-bit reproducibility yet — Tauri bundling has non-determinism we have not yet eliminated. Tracked as a future goal.

## 10. Hardening checklist (per release)

Before tagging a release, the maintainer verifies:

- [ ] `cargo audit` clean.
- [ ] `cargo deny check` clean.
- [ ] `pnpm audit --audit-level high` clean.
- [ ] Every updater artifact signed and verified by `release.yml` (minisign); `SHA256SUMS` attached. (Authenticode / Apple Developer ID: not used, ADR-0011.)
- [ ] The bundled SumatraPDF matches the pinned version **and checksum** in `scripts/fetch-sumatrapdf.*`, for the installer's architecture (x64 or ARM64), and its licence files (`LICENSE.txt`, `COPYING.BSD.txt`, `AUTHORS.txt`, fetched from the pinned upstream commit), `SOURCE.txt` and the written offer `SOURCE-OFFER.txt` sit next to the executable, with the GPL-3.0 text also in `LICENSES/GPL-3.0.txt` (GPL-3.0 §6 obligation — see [`../09-legal/third-party-licenses.md`](../09-legal/third-party-licenses.md)).
- [ ] CSP unchanged (or change reviewed).
- [ ] Capability allowlist unchanged (or change reviewed).
- [ ] No new outbound URLs added without privacy-doc update.
- [ ] No new logging that could include user content without `--debug`.

## 11. Related documents

- [`auto-updater.md`](./auto-updater.md) — update integrity in detail.
- [`shell-extensions.md`](./shell-extensions.md) — the DLL boundary.
- [`ipc-and-cli.md`](./ipc-and-cli.md) — capability allowlist.
- [`data-model.md`](./data-model.md) — what we persist and where.
- ADR [`0005-shell-extension-sparse-msix`](../adr/0005-shell-extension-sparse-msix.md) — the install model that keeps us out of admin.
