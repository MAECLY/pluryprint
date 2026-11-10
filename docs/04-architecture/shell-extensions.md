# Shell Extensions

> **TL;DR** — On Windows we ship a small Rust COM DLL implementing `IExplorerCommand`, registered per user as an `ExplorerCommandHandler` verb: it shows in the Windows 10 menu and in the Windows 11 classic menu ("Show more options"). The Windows 11 top-level menu needs a signed sparse MSIX package, which waits on a code-signing certificate (§2.5). On macOS the integration ships **inside the app bundle** as an `NSServices` declaration answered by a Rust/`objc2` provider — no `.appex`, no sandbox, no separate signature and nothing for the user to enable (see §3). On Linux (a preview platform, [ADR-0015](../adr/0015-linux-support.md)) there is no extension at all: a `.desktop` file puts Pluryprint under **Open with**, and the `.deb` adds a Dolphin service menu and Nemo actions (see §4). In every case the shell integration does *no real work*: it hands over the user's selection and starts the main Pluryprint binary.

## 1. The problem we are solving

Right-clicking files in Windows Explorer and picking "Print" calls `IContextMenu::InvokeCommand` with the print verb. Windows then enumerates the selected items, opens each one in its registered application's print handler, and dispatches one print at a time. Two things go wrong with this default path:

1. **The ~15-file limit.** When you select more than about 15 files and right-click, Explorer hides or disables the Print verb. (The exact threshold depends on the file types and registry policy, but it is real and well-known.)
2. **No batching, no profile, no queue.** Each file is printed independently, with whatever default printer + settings are active, and there is no progress UI.

Pluryprint replaces this with a single context-menu entry — **"Print all with Pluryprint"** — that:

- Receives the *full* selection (no 15-file ceiling).
- Opens a unified dialog with a profile selector and per-job overrides.
- Enqueues all files, runs converters in parallel where safe, and prints sequentially with progress reporting.

To do this properly on Windows 11 we need a real shell extension, not a registry-only command.

## 2. Windows: an Explorer command, registered per user

**Status: implemented (registry verb); the sparse-package route for the Windows 11 top-level menu is designed but not shipped** (decision D3 of round 3, see §2.5).

Windows 11 has two context menus:

- the **modern menu**, shown by default;
- the **classic menu**, reached through **Show more options**, Shift+F10 or Shift+right-click. It is the Windows 10 menu.

A third-party entry reaches the modern menu only as an `IExplorerCommand` declared by a **packaged** app (a full MSIX, or a *sparse* package that gives an externally installed app an identity), and Windows registers a sparse package only when it is signed with a certificate the machine trusts. Without that certificate (NFR-021, D2/D4: no code signing this round) the entry lives in the classic menu, where it works on every Windows version with no elevation.

### 2.1 The DLL

`shell_ext_win.dll` is a Rust crate, [`shell-ext-win/`](../../shell-ext-win/README.md), outside the app's workspace so its release profile can keep `panic = "unwind"` (a panic must be caught at the COM boundary, not take Explorer down). It is an in-process COM server with one class, `{B269B8DC-044F-4D65-8F1E-8F20A2E653EA}`, implementing `IExplorerCommand`, `IObjectWithSelection` and `IEnumExplorerCommand`:

- the root entry, **Pluryprint**, has sub-commands: *Print N files* (default profile), one *Print with "<profile>"* per profile (at most 8), *Configure print job…* and *Add to print queue*;
- the labels and profiles come from `<data_local_dir>\shell-menu.json`, which the app writes in the UI language (`src-tauri/src/shell_menu.rs`), so no file is read while Explorer draws the menu. Until the app has written it (installed but never started) the menu uses its built-in labels in the Windows display language, English or Spanish; on a first run the app and the CLI take that same language before writing anything (`settings::locale`), and the built-in profiles are listed under their names in the app language;
- the entry hides itself (`ECS_HIDDEN`) when none of the first 16 selected items is a supported file or a folder (FR-005); the app is the authority on what prints;
- on click it writes the selection to a JSON handoff file and starts `pluryprint.exe --invoke=context-menu --files-from <file> --action <action> [--profile <id>]`. A command line has a fixed size whatever the selection, so there is no 15-item limit (FR-003).

The DLL never prints, never reads file contents and never talks to the running app: that is the app's job (§5).

### 2.2 What the installer registers

The per-user NSIS installer ([ADR-0008](../adr/0008-installer-nsis-per-user.md)) does it in [`src-tauri/windows/installer-hooks.nsh`](../../src-tauri/windows/installer-hooks.nsh), all under `HKCU\Software\Classes`, so neither install nor update asks for elevation:

| Key | Values |
|---|---|
| `CLSID\{B269B8DC-…}\InprocServer32` | `(Default)` = `<install dir>\shell_ext_win.dll`, `ThreadingModel` = `Apartment` |
| `*\shell\Pluryprint` and `Directory\shell\Pluryprint` | `ExplorerCommandHandler` = the CLSID, `MultiSelectModel` = `Player`, `Icon` = `pluryprint.exe,0` |

**Architecture.** The installer carries the DLL for both architectures (`resources\shellext\x64` and `resources\shellext\arm64`) and copies the one for the **operating system's** native architecture next to `pluryprint.exe`. Explorer on Windows on ARM is a native ARM64 process and cannot load an x64 DLL, even when the x64 installer runs there under emulation. CI builds must carry both DLLs (the hooks refuse to compile otherwise); a local single-architecture installer run on the other architecture registers nothing rather than a CLSID that points at no file.

**Updates.** Explorer keeps the DLL loaded once a menu has been shown. A loaded DLL cannot be overwritten but can be renamed, so the old copy is moved aside as `shell_ext_win.dll.old<N>` and removed by the next update or the uninstaller.

**Uninstall** deletes the keys, the DLL (if Explorer still holds it, it is moved to `%TEMP%\pluryprint-shellext-<N>.dll` and deleted at the next logon by an HKCU `RunOnce` value: `Delete /REBOOTOK` needs HKLM, which a per-user uninstaller cannot write) and the SumatraPDF settings file. With **Delete the application data** ticked it also removes `%APPDATA%\Maecly\Pluryprint` and `%LOCALAPPDATA%\Maecly\Pluryprint` (settings, profiles, queue, logs, cache): Tauri's own checkbox only knows the `%APPDATA%\<identifier>` folders, which hold the web view profile.

**Verified** on Windows 11 24H2 ARM64 with UAC on, from a medium-integrity session (round 3): the arm64 installer installs in 8 s with no elevation prompt, copies the arm64 DLL next to `pluryprint.exe` and writes the keys above; "Show more options" shows Pluryprint ▸ with every entry; ten "Print 1 file" clicks with the app already open were each submitted within 0.2 s to a paused queue; the GUI uninstall with **Delete the application data** ticked left no install folder, no data folders, no keys and no pending renames.

**UAC off.** With UAC disabled (EnableLUA = 0) Explorer runs elevated and ignores COM classes registered under HKCU, so the entry does not appear. That is a Windows rule for every per-user shell extension.

### 2.3 Where the entry appears

| Surface | Entry |
|---|---|
| Windows 10 menu | Pluryprint ▸ submenu |
| Windows 11, **Show more options** (click or keyboard) | Pluryprint ▸ submenu |
| Windows 11, Shift+right-click / Shift+F10 | Pluryprint ▸ submenu |
| Windows 11 top-level menu | not shown (needs the signed sparse package, §2.5) |

**A test trap, not a product defect.** If **Show more options** is activated through UI Automation (`InvokePattern`) instead of a real click or the keyboard, Explorer builds the classic menu without the owner-draw handling its cascades need: every submenu opens as an empty box about 20 px wide, Windows' own *Send to* included, and Explorer calls `EnumSubCommands` but never `Next`. The round-3 upgrade test reported that as an empty Pluryprint submenu (row 7b). The VM test ([`scripts/vm/r3-contextmenu-e2e.ps1`](../../scripts/vm/r3-contextmenu-e2e.ps1)) therefore uses real mouse input. A classic `IContextMenu` handler, which builds its whole submenu in `QueryContextMenu`, was tried as an alternative and shows the same empty box under `InvokePattern` and the same full submenu with a real click, so it brought nothing and was not kept.

### 2.4 Handing the selection to a running app

When the app is already open, the process the DLL starts is a second instance. `tauri-plugin-single-instance` hands its argv to the running one (a `WM_COPYDATA` message) and exits; the running instance reads the handoff file, parks the selection and rings the window (`selection-forwarded`), which drains it (`take_pending_selections`).

The running window then comes to the front **without faked input** (`src-tauri/src/cli/foreground.rs`). Tauri's `set_focus` was used before; on Windows, when `SetForegroundWindow` is refused (a background app may not take the foreground), it injects an Alt key press. The lone Alt puts the window into menu mode, a modal loop that left the selection undelivered until the user's next click: the intermittent stall of the upgrade test (row 7c). Now the second instance, which Explorer started in response to the user's click and which may therefore set the foreground, passes that right on with `AllowSetForegroundWindow(ASFW_ANY)` before it forwards, and the running instance calls plain `SetForegroundWindow`; if Windows still refuses, the taskbar button flashes. Before raising, the callback also ends menu mode (`EndMenu`) whoever caused it: it runs on the window's thread, inside the modal loop, which makes it the one place that can.

Measured on Windows 11 24H2 ARM64: with the window put in menu mode by a lone Alt, the pre-fix build did not act on a forwarded selection within 40 s (3 of 3) and did at once after Escape; the fixed build acted in 0.2 to 0.3 s (3 of 3), and in 0.2 to 0.3 s for 10 of 10 selections sent through the real Explorer menu, each time with the window raised (`window_raised`, `outcome: raised` in the log).

### 2.5 The sparse package (designed, not shipped)

The top-level Windows 11 entry needs a sparse package registered with `Add-AppxPackage -Path <msix> -ExternalLocation <install dir>`, whose manifest declares the same CLSID as a `windows.comServer` and a `windows.fileExplorerContextMenus` verb (`desktop5:ItemType Type="*"` and `"Directory"`), and the DLL and `pluryprint.exe` must embed the matching side-by-side `<msix>` manifest (`shell-ext-win/res/shellext.manifest`, embedded by `build.rs`). Windows installs such a package only when it is signed by a certificate the machine trusts, which outside a test machine means a code-signing certificate. Per decision D3 the installer registers the HKCU verb always and will add the sparse package only once a trusted-signed `.msix` exists; with both registered, the packaged verb serves the top-level menu and the HKCU verb must then be dropped from the classic menu to avoid a duplicate entry.

Whatever registers the package must first remove any package registered under the former identity, `MaeclyPrint` ([ADR-0009](../adr/0009-rename-to-pluryprint.md)), which would claim the same CLSID:

```powershell
Get-AppxPackage -Name MaeclyPrint | Remove-AppxPackage
```

## 3. macOS: an NSService inside the app

**Status: implemented.** The Finder integration ships in the app bundle itself, as an
[NSService](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/SysServices/Articles/properties.html).
There is no `.appex`, no Automator workflow and no second bundle to sign. The
alternatives — a Finder Sync extension, an Action extension, an Automator Quick
Action — and the reasoning that rejected them are tabulated in
[`shell-ext-mac/README.md`](../../shell-ext-mac/README.md); this section
describes what exists.

### 3.1 What the user sees

Selecting any number of files or folders in the Finder, right-clicking and
opening the **Services** submenu offers:

- **Print with Pluryprint** — opens the print dialog pre-loaded with the
  selection (`action = configure`);
- **Print with Pluryprint (default profile)** — prints straight away with the
  default profile (`action = print-default`).

The entry is one level deeper than the Windows 11 top-level menu. That is the
price of not shipping a sandboxed extension the user has to enable, and it is
the only mechanism of the four that keeps working under an MDM policy that
blocks ExtensionKit. A user who wants it faster can bind a shortcut in
**System Settings → Keyboard → Keyboard Shortcuts → Services**.

### 3.2 The declaration

[`src-tauri/Info.plist`](../../src-tauri/Info.plist) carries an `NSServices`
array with one dictionary per menu entry. The Tauri CLI (v2) looks for
`Info.plist` next to `tauri.conf.json` and merges it over the plist it
generates, so no bundler script and no `bundle.macOS.infoPlist` setting is
needed — the file's presence is the whole mechanism.

```xml
<key>NSServices</key>
<array>
  <dict>
    <key>NSMenuItem</key>     <dict><key>default</key><string>Print with Pluryprint</string></dict>
    <key>NSMessage</key>      <string>printWithPluryprint</string>
    <key>NSPortName</key>     <string>Pluryprint</string>
    <key>NSUserData</key>     <string>configure</string>
    <key>NSSendTypes</key>    <array><string>public.file-url</string></array>
    <key>NSSendFileTypes</key><array><string>public.item</string></array>
    <key>NSRequiredContext</key><dict/>
    <key>NSTimeout</key>      <string>10000</string>
  </dict>
  <!-- second entry: same NSMessage, NSUserData = print-default -->
</array>
```

Four keys carry the load:

| Key | Why it is what it is |
| --- | --- |
| `NSMessage` | Names the provider method minus the `:userData:error:` tail AppKit appends: `printWithPluryprint` ⇒ `-printWithPluryprint:userData:error:`. Changing one without the other silently produces a menu entry that does nothing. |
| `NSPortName` | Must be the app's `CFBundleName`, i.e. `Pluryprint`. |
| `NSUserData` | The official way to let two menu entries share one `NSMessage`. Its values are the CLI's `--action` vocabulary (`configure`, `print-default`), so the Finder and the command line cannot drift apart. |
| `NSSendFileTypes` | `public.item` covers both files and folders, which is what FR-004 needs. |

`NSRequiredContext` is declared empty ("no restriction"). Apple's guide treats
it as required; several macOS releases are reported to filter out services that
omit it entirely, and an empty dictionary costs nothing.

**Localization (FR-092).** The `NSMenuItem` `default` strings are the English
titles. macOS translates them through `Resources/<lang>.lproj/ServicesMenu.strings`,
keyed by the English title, so a Spanish system shows "Imprimir con Pluryprint"
and "Imprimir con Pluryprint (perfil predeterminado)". The sources live in
`src-tauri/macos/{en,es}.lproj/ServicesMenu.strings`, are copied into the bundle
by `bundle.macOS.files` in `tauri.conf.json`, and `CFBundleLocalizations` in
`Info.plist` declares both languages. The Services menu follows the **system**
language, not Pluryprint's own language setting, because the Finder draws it
before the app is involved. `tests/macos_services_l10n.rs` fails if a title is
added to `Info.plist` without its translations or if the bundle mapping is lost.

**Deliberately not declared: `CFBundleDocumentTypes`.** It would add a free
"Open With → Pluryprint" entry, but Launch Services delivers those files
through `application:openURLs:`, an AppKit delegate callback the app does not
implement today. Declaring it would advertise a capability that silently drops
the user's files. It is a follow-up, not a freebie.

### 3.3 The provider

[`src-tauri/src/platform/macos/services.rs`](../../src-tauri/src/platform/macos/services.rs)
defines `PluryprintServicesProvider`, an Objective-C class built with `objc2`
(the same `objc2 0.6` / `objc2-*` `0.3` versions `tao` already pulls in, so the
integration adds **zero** new crates to the dependency graph). Tauri's `setup`
hook registers it:

```rust
#[cfg(target_os = "macos")]
if let Err(e) = crate::platform::macos::services::register(app.handle()) {
    tracing::warn!(event = "macos_services_registration_failed", error = %e);
}
```

`register` calls `-[NSApplication setServicesProvider:]` and then
`NSUpdateDynamicServices()`, which forces the system to re-read this bundle's
declarations — without it a freshly built app inherits the previous build's (or
no) entries from the Services cache.

When the user picks the menu entry, AppKit calls
`-printWithPluryprint:userData:error:` on the main thread. The handler:

1. maps `userData` onto a `ContextAction` (unknown ⇒ `configure`: never print
   something the user did not ask for);
2. reads the selection with
   `-[NSPasteboard readObjectsForClasses:options:]` for `NSURL`, which returns
   the **whole** selection — there is no documented ceiling, which is what
   FR-003 needs — and drops anything that is not a file URL, the same way the
   Windows side drops shell items with no `SIGDN_FILESYSPATH`;
3. writes the selection as a handoff file and hands it to the app (§3.4);
4. runs inside `catch_unwind`: a panic unwinding into an Objective-C frame is
   undefined behaviour, and in a release build (`panic = "abort"`) it would take
   the app down mid-print.

One behaviour worth knowing: `NSURL` reports paths in the filesystem's
**decomposed** form (NFD), so `ñ` arrives as `n` + U+0303, not the precomposed
character a user types. The path still opens the file; string comparisons
against a path from any other source do not match. Anything that deduplicates
across sources must compare canonicalized paths, never raw strings. There is a
test pinning this.

### 3.4 Handoff, and why there is no relaunch

The Windows extension is a DLL inside Explorer, so it *must* start or signal the
app: it writes a handoff file and launches
`pluryprint --invoke=context-menu --files-from <PATH>`. On macOS the service is
answered by the app itself — Launch Services starts it if it is not running and
reuses it if it is, which is the same single-instance guarantee
`tauri-plugin-single-instance` gives on Windows. Relaunching our own binary from
inside our own process would add a spawn, a window race and an untested path,
so the selection is delivered in-process.

It still travels through the handoff file
([`src-tauri/src/platform/macos/handoff.rs`](../../src-tauri/src/platform/macos/handoff.rs)
writes it; [`src-tauri/src/cli/handoff.rs`](../../src-tauri/src/cli/handoff.rs)
reads it), for three reasons: the selection then passes the *same* validation
gate a Windows invocation passes (schema version, freshness window, directory
containment, path and byte caps); the file is an artefact tests can assert on
without AppKit; and the bytes are identical to what a `--files-from` relaunch
would carry, so the schema cannot fork per platform. The write is a temp file
plus `rename`, and the handoff directory is `chmod 0700`.

After validation the selection is parked in `PendingSelections` and announced
with the `SelectionForwarded` event — exactly what the single-instance callback
does on Windows — and the main window is unminimized and focused.

```
Finder selection
      │  (Services menu)
      ▼
NSApplication ──► PluryprintServicesProvider.printWithPluryprint:userData:error:
                        │  NSPasteboard → Vec<PathBuf>
                        ▼
              platform::macos::handoff::write   ──►  <data_dir>/handoff/sel-<uuid>.json
                        │
                        ▼
              cli::handoff::{is_within_dir, consume, is_fresh}   ← the shared gate
                        │
                        ▼
              PendingSelections + SelectionForwarded  ──►  print dialog
```

### 3.5 Verifying it

`cargo test` covers everything that does not need the Finder: payload writing
and its caps, the pasteboard→paths decoding (built from real `NSURL`s), hostile
file names, the NFD behaviour above, and the round trip through the reader.

Making the menu entry actually *appear* needs a real bundle registered with
Launch Services; the commands (`lsregister -f`, `pbs -flush`,
`pbs -dump_pboard | grep`, `killall Finder`) are in
[`shell-ext-mac/README.md`](../../shell-ext-mac/README.md). CI can assert two of
those steps — that `NSServices` survived the bundler, and that `pbs` discovered
the service — but that the entry renders in the Finder's menu is not
automatable.

## 4. Linux: desktop entries, no extension

**Status: decided for 0.0.5 ([ADR-0015](../adr/0015-linux-support.md)); Linux is a preview platform and these entries have not yet been tried on real desktops.**

Linux file managers share no extension API. GNOME Files (Nautilus) loads Python extensions only through `nautilus-python`, which is rarely installed; Dolphin reads KDE *service menus*; Nemo reads its own *actions*; Thunar and Caja have other formats again. All of them can run a program from a desktop-entry-style text file, and that is the only kind of integration Pluryprint ships on Linux: nothing loads into the file manager's process, and the file only names the program to start, as the Windows DLL and the macOS service do.

- **Open with (every file manager).** The application's `.desktop` file lists the supported types in `MimeType=` and passes the selected files on its `Exec=` line, so **Open with → Pluryprint** appears for a supported file in any file manager that follows the freedesktop.org specifications. The selection arrives as an "open with" launch: the print dialog opens with those files, and a running instance receives them through the single-instance forward (§2.4 describes the same path on Windows).
- **Dolphin (KDE) and Nemo (Cinnamon).** The `.deb` also installs a Dolphin service menu and Nemo actions with three entries, the same actions as the Windows submenu: **Print now** (the default profile, at once), **Configure print job** (the print dialog) and **Add to print queue**. Each entry starts `pluryprint` with its action and the selected paths. There are no per-profile rows.
- **Not shipped.** No Nautilus extension, no Thunar or Caja actions, and the AppImage registers nothing: it never writes into `~/.local/share` on its own, so its "Open with" entry appears only if the user integrates the AppImage with a tool of their choice. Only the `.deb` installs the Dolphin and Nemo entries.

Checking the files needs no desktop: `desktop-file-validate` covers the `.desktop` entry, and the `.deb`'s contents can be listed with `dpkg -c`. Whether an entry renders in Dolphin's or Nemo's menu, and starts the app with the right selection, is a manual check on a real KDE Plasma and Cinnamon session.

## 5. Flow: User right-click → DLL → spawn → Tauri renders

```
+-----------+      +-------------------+      +----------------------+      +---------------+
| Explorer  |      | Pluryprint       |      | Pluryprint.exe      |      | WebView2      |
| (or       | ---> | shell ext DLL     | ---> | (Rust core)          | ---> | print dialog  |
| Finder)   |      | (Rust)            |      |                      |      | (React UI)    |
+-----------+      +-------------------+      +----------------------+      +---------------+
                          |                            |                            |
                          | 1. enumerate items         |                            |
                          | 2. resolve full paths      |                            |
                          | 3. dedupe                  |                            |
                          | 4. serialize JSON          |                            |
                          | 5. spawn Tauri exe         |                            |
                          | 6. exit                    |                            |
                          X                            |                            |
                                                       | A. read --files-from file  |
                                                       | B. single-instance mutex   |
                                                       |    (or forward argv)       |
                                                       | C. boot Tauri runtime      |
                                                       | D. open print dialog       |
                                                       |--------------------------> |
                                                       |                            |
                                                       |       user picks profile,  |
                                                       |       hits "Print"         |
                                                       |<-------------------------- |
                                                       | E. enqueue jobs            |
                                                       | F. progress events to UI   |
```

### Why this shape?

- **Cheap DLL.** Anything beyond serialization in the DLL risks crashing Explorer.
- **Single source of truth.** The Tauri exe is the only place that knows about profiles, supported formats, the queue, etc. The DLL doesn't need to be redeployed when a converter is added.
- **Singleton.** A second right-click while the app is open routes the new selection into the running instance: `tauri-plugin-single-instance` forwards the argv (`WM_COPYDATA`), and the window is raised without faked input (§2.4).

## 6. Logging and diagnostics

The DLL logs only to `%LOCALAPPDATA%\Maecly\Pluryprint\data\logs\shellext.jsonl`: one JSON line per invocation (action, item count, handoff file) and per caught panic, nothing while the menu is drawn, capped at 1 MB.

The Tauri exe logs in JSONL to its own log file (see [`data-model.md`](./data-model.md)).

## 7. Testing the shell extension

- **Unit (DLL).** `cargo test` in `shell-ext-win/` covers the handoff payload, the menu model and the label cache on any host; the app reads the same fixtures (`tests/handoff_roundtrip.rs`, `tests/strings_cache.rs`).
- **Installer.** `src-tauri/tests/installer_hooks.rs` and `bundle_identity.rs` pin what the hooks register and remove, and compile them with `makensis -WX`.
- **VM.** [`scripts/vm/r3-contextmenu-e2e.ps1`](../../scripts/vm/r3-contextmenu-e2e.ps1) opens the classic menu with real mouse input, prints the Pluryprint submenu, clicks an entry and takes a screenshot (§2.3 explains why UI Automation's `InvokePattern` must not be used for "Show more options").

On macOS everything that does not need the Finder is a plain `cargo test` in `platform::macos::*` — handoff writing and its caps, `NSPasteboard`→paths decoding built from real `NSURL`s, hostile file names, the action vocabulary. Whether the system *discovered* the service is assertable in CI (`plutil -extract NSServices` on the bundled `Info.plist`, then `lsregister -f` + `pbs -flush` + `pbs -dump_pboard | grep`); whether the entry renders in the Finder's menu is not automatable (see `shell-ext-mac/README.md`).

## 8. Related documents

- [`system-architecture.md`](./system-architecture.md) — where this fits in the overall layout.
- [`ipc-and-cli.md`](./ipc-and-cli.md) — exact `--invoke` flag semantics.
- ADR [`0005-shell-extension-sparse-msix`](../adr/0005-shell-extension-sparse-msix.md) — why Sparse MSIX over the alternatives.
- ADR [`0008-installer-nsis-per-user`](../adr/0008-installer-nsis-per-user.md) — the per-user installer that registers the verb (supersedes ADR-0006).
