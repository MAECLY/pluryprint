# Installer (Windows)

> **TL;DR.** Windows ships **Tauri's per-user NSIS installer** ([ADR-0008](../adr/0008-installer-nsis-per-user.md), which superseded the Inno Setup + Sparse MSIX plan of [ADR-0006](../adr/0006-installer-inno-setup.md)): x64 and ARM64 `setup.exe`, installed under `%LOCALAPPDATA%\Pluryprint` without elevation, in English and Spanish, with SumatraPDF bundled and the Explorer command registered under `HKCU` for the operating system's native architecture. It is unsigned (ADR-0011). There is no `.iss`, no `AppxManifest.xml` and no sparse package (decision D3). This document describes the real state first, then keeps the ADR-0006 design below as history.

## Status: the contradiction ADR-0008 resolved

| Claim | Reality |
|---|---|
| ADR-0006 (Accepted, 2026-05-03) selects Inno Setup as the primary installer, explicitly rejecting NSIS as "quirky … less polished" | `tauri.conf.json` configures `bundle.windows.nsis` and nothing else. NSIS is what a `pnpm tauri build` produces. |
| ADR-0006 requires a post-install PowerShell step registering a Sparse MSIX via `Add-AppxPackage -Register -ExternalLocation` | No sparse package exists. `installers/windows/sparse-msix/` was never created. |
| ADR-0006 requires `installers/windows/inno-setup.iss` + `build.ps1` | Neither file exists. `installers/windows/` holds a single `.gitkeep`. |
| ADR-0005 / ADR-0006 assume a C++ COM DLL named `shell-ext-win.dll` / `PluryprintShellExt.dll` | The extension is a Rust `cdylib` and the built artifact is **`shell_ext_win.dll`** (underscores — `[lib] name = "shell_ext_win"`). |
| ADR-0006 fixes the app CLSID as `{A1B2C3D4-E5F6-7890-ABCD-EF1234567890}` in draft snippets | The real CLSID, in `shell-ext-win/src/clsid.rs`, is **`{B269B8DC-044F-4D65-8F1E-8F20A2E653EA}`**. |

**Decided (2026-10-09): option 2.** [ADR-0008](../adr/0008-installer-nsis-per-user.md) supersedes ADR-0006. The Windows installer is Tauri's per-user NSIS bundle, with signed updater artifacts (`createUpdaterArtifacts`), because `tauri-plugin-updater` installs updates by running that same setup. The sparse-MSIX identity remains open and is tracked in ADR-0008. The two options as they were weighed:

1. **Implement ADR-0006** — write the `.iss`, the sparse MSIX and `build.ps1`, and stop using Tauri's NSIS bundler for release artifacts; or
2. **Supersede ADR-0006** with a new ADR that accepts NSIS (Tauri's bundler already produces a per-user NSIS installer, which satisfies the no-admin constraint) and specifies how the sparse-MSIX identity requirement from ADR-0005 is met alongside it — that requirement does not go away, because `shell-ext-win/res/shellext.manifest` already embeds an `<msix>` element that is meaningless without a matching package.

ADRs are not rewritten, they are superseded. Do not edit ADR-0006 to match the code.

Whichever way it goes, one constraint binds both options: `SumatraPDF.exe` is **not committed**. Any packaging job must run `scripts/fetch-sumatrapdf.ps1` first, or `bundle.resources` (`"resources/sumatrapdf/*": "resources/sumatrapdf/"`) copies only a `README.md` and every print job on the installed app fails with `DependencyMissing`.

## Packaging a Windows installer (current procedure)

Three steps, in this order, on a Windows machine of the installer's architecture (or with `-Arch` for the other one):

```powershell
powershell -ExecutionPolicy Bypass -File scripts\fetch-sumatrapdf.ps1 -Arch arm64   # or x64
powershell -ExecutionPolicy Bypass -File scripts\build-shell-ext.ps1 -Arch all      # both DLLs; CI requires both
pnpm tauri build                      # without the updater key: --config src-tauri/tauri.ci.conf.json
```

- `fetch-sumatrapdf.ps1` puts the SumatraPDF build for that architecture, with its licence files, in `src-tauri/resources/sumatrapdf/`.
- `build-shell-ext.ps1` builds `shell_ext_win.dll` and copies it to `src-tauri/resources/shellext/<x64|arm64>/`. The installer carries both and the hooks (`src-tauri/windows/installer-hooks.nsh`, wired through `bundle.windows.nsis.installerHooks`) register the one for the **operating system's** native architecture, so the x64 installer run on Windows on ARM still gives Explorer an ARM64 DLL. They refuse to build an installer with neither DLL, and a CI build (`CI=true`) without both; a local single-architecture build installed on the other architecture registers no menu.
- The result is `src-tauri/target/release/bundle/nsis/Pluryprint_<version>_<arch>-setup.exe`, per user (`installMode: "currentUser"`), installing to `%LOCALAPPDATA%\Pluryprint\` with its uninstall entry under HKCU. No MSI is built (`bundle.targets` has no `msi`).

What the hooks register, all under `HKCU\Software\Classes`:

| Key | Value |
|---|---|
| `CLSID\{B269B8DC-044F-4D65-8F1E-8F20A2E653EA}\InprocServer32` | `(Default)` = `<install dir>\shell_ext_win.dll`, `ThreadingModel` = `Apartment` |
| `*\shell\Pluryprint` and `Directory\shell\Pluryprint` | `ExplorerCommandHandler` = the CLSID, `MultiSelectModel` = `Player`, `Icon` = `pluryprint.exe,0` |

### Verified on Windows 11 ARM64 (24H2, 2026-10-10)

With UAC **enabled** and the installer run from a non-elevated, medium-integrity session (`whoami /groups` = *Medium Mandatory Level*, not an administrator token):

- `Pluryprint_0.0.1_arm64-setup.exe /S` exits 0 in about 7 s with no elevation; files land in `%LOCALAPPDATA%\Pluryprint\` (`pluryprint.exe`, `pluryprint-cli.exe`, `shell_ext_win.dll`, `uninstall.exe`, `resources\sumatrapdf\` with the ARM64 `SumatraPDF.exe` and its licence files); the CLSID and verbs exist under HKCU and nothing under HKLM. `uninstall.exe /S` exits 0 and removes the keys.
- The classic context menu that Explorer builds for a `.pdf` (`IContextMenu` from the file's folder) lists **Pluryprint**, and its sub-command *Print 1 file* put the file into the paused default queue as a job named `e2e-sample.pdf`.

One limit found on the way: when Explorer itself runs **elevated** (UAC turned off, as on the test VM by default), Windows ignores per-user COM registrations for shell extensions, so the HKCU-registered handler is never created and the entry does not appear. That is the documented behaviour for elevated processes, not something a per-user installer can change. Machines with UAC on, the Windows default, are not affected.

That is the Windows 10 verb shape from `shell-ext-win/README.md`: the menu entry appears on Windows 10 and in the Windows 11 classic menu (*Show more options*, Shift+F10), for any number of selected items. Uninstall deletes the three keys. The Windows 11 top-level entry needs a sparse package signed with a trusted certificate, and waits for the code-signing certificate (ADR-0008, Risks).

## What actually ships today

`bundle` in `src-tauri/tauri.conf.json`:

```json
"bundle": {
  "active": true,
  "targets": ["nsis", "app", "dmg"],
  "resources": {
    "resources/sumatrapdf/*": "resources/sumatrapdf/",
    "resources/shellext/": "resources/shellext/",
    "../LICENSE": "LICENSE",
    "../LICENSES/*": "LICENSES/",
    "../THIRD-PARTY-NOTICES.md": "THIRD-PARTY-NOTICES.md"
  },
  "publisher": "Maecly",
  "homepage": "https://pluryprint.maecly.com",
  "windows": {
    "nsis": {
      "installMode": "currentUser",
      "installerHooks": "windows/installer-hooks.nsh",
      "languages": ["English", "Spanish"],
      "displayLanguageSelector": false
    }
  },
  "createUpdaterArtifacts": true
}
```

(Abridged: icons, copyright and the macOS block are left out.)

- `installMode: "currentUser"` is what keeps the no-admin requirement (NFR-020) satisfied: a per-user install under `%LOCALAPPDATA%`, `HKCU` only.
- `targets` is an explicit list without `msi`. The bundler builds only the entries that apply to the host, so a Windows build produces the per-user NSIS `setup.exe` and nothing else. It used to be `"all"`, which also built a per-machine WiX MSI (Program Files, elevation) next to it. Installing that MSI and later updating through the NSIS-based `latest.json` left two installs and two Start-menu entries, which contradicts [ADR-0008](../adr/0008-installer-nsis-per-user.md). `fr_083_no_per_machine_msi_is_bundled` guards the list.
- The crate has two binaries, `pluryprint` (the app) and `pluryprint-cli` (`src/bin/pluryprint-cli.rs`, the console-subsystem CLI front end scripts wait on). The installer places `pluryprint-cli.exe` next to `pluryprint.exe` (verified on the ARM64 install above). Neither is added to `PATH`; Home's **Run from CLI** card copies the full path.
- **Shortcuts.** Tauri's template creates a Start-menu shortcut on every install, and a desktop shortcut from the checkbox on the last page. A silent (`/S`) or passive (`/P`) run skips that page, so it **always creates the desktop shortcut** (`%USERPROFILE%\Desktop\Pluryprint.lnk`, seen on the 0.0.1 test). That is the template's behaviour and is kept. For a managed or scripted install that wants no shortcuts, pass Tauri's `/NS` flag (`setup.exe /S /NS`): it skips **both** the desktop and the Start-menu shortcut; the template has no switch for one without the other. An update (`/UPDATE`) never creates either.
- **Uninstall entry and version information.** Tauri writes the `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Pluryprint` key (`DisplayName`, `Publisher` = `bundle.publisher`, `UninstallString`, `InstallLocation`, …). `bundle.homepage` makes it also write `URLInfoAbout`, `URLUpdateInfo` and `HelpLink` (`https://pluryprint.maecly.com`), and the post-install hook adds `QuietUninstallString` = `"<install dir>\uninstall.exe" /S`, which Tauri does not write. The setup's version resource gets `CompanyName` = `Maecly` from the hooks file (`VIAddVersionKey`): Tauri's template sets `ProductName`, `FileDescription`, `LegalCopyright` (`bundle.copyright`), `FileVersion` and `ProductVersion` but no company, and `bundle.publisher` does not map to it. `tests/installer_hooks.rs` keeps the hook's company equal to `bundle.publisher`, and `tests/bundle_identity.rs` compiles the hooks and reads `CompanyName` back from the version resource.
- **Languages.** Tauri translates its own pages for `languages` (English, Spanish), and every message the hooks add (`DetailPrint`, `MessageBox`) is a `$(PLURY_*)` language string declared in both languages in `windows/installer-strings.nsh` (`tests/installer_i18n.rs`). The setup takes the Windows display language without asking (`displayLanguageSelector: false`), and the post-install hook records it where the uninstaller reads it (`HKCU\Software\Maecly\Pluryprint`, `Installer Language`), so the uninstaller opens in the same language instead of first asking for one (`fr_093_uninstaller_reuses_the_installer_language`).
- **Uninstall.** Besides Tauri's own cleanup, the hooks remove the Explorer keys and DLL and, on every real uninstall (ticked box or not, silent or not), what is the installer's own rather than user data: `HKCU\Software\Maecly\Pluryprint` (the remembered install folder and `Installer Language`; `HKCU\Software\Maecly` too when it is then empty) and the main window's WebView2 profile `%LOCALAPPDATA%\com.maecly.pluryprint\EBWebView` (then its parent folder, if empty). The profile only holds a cache of the appearance settings, which live in `config.toml`. Before 0.0.2 both survived `uninstall.exe /S`, because Tauri removes them only with **Delete the application data**. Only when that box is ticked (never on an update or a silent run) do the hooks also remove the user data, `%APPDATA%\Maecly\Pluryprint` and `%LOCALAPPDATA%\Maecly\Pluryprint`, the `Maecly` folders when nothing else is left in them, and the stale MaeclyPrint web view profile `%LOCALAPPDATA%\com.maecly.print`.
  - **Update or uninstall.** `tauri-plugin-updater` runs the new setup with `/UPDATE` (plus `/P` for `installMode: "passive"`). The setup then installs over the old version without running its uninstaller; when Tauri's template does run the old uninstaller during an update, it appends `/UPDATE` to that command line too. Both the setup and the uninstaller set `$UpdateMode` to 1 from that flag, and every cleanup above is inside `${If} $UpdateMode <> 1`, so an update keeps the product key (the new setup reads the install folder from it), the web view profile and all user data. A manual reinstall that chooses "uninstall first" on Tauri's reinstall page runs the uninstaller without `/UPDATE`, so the key and profile go; the setup rewrites the key right after.
  - A silent uninstall closes a running Pluryprint without asking (Tauri's template). On the 0.0.1 test a job that had already been spooled was then logged as `job_unconfirmed` (`left_queue`): it had left the queue with nothing confirming that it printed.
- `plugins.updater` and `createUpdaterArtifacts` are set: the NSIS setup is signed for the updater, and `tauri-plugin-updater` installs an update by running that same setup in `passive` mode (see [auto-update-flow.md](./auto-update-flow.md)).
- `installerHooks` points at `src-tauri/windows/installer-hooks.nsh`, the one hooks file: besides registering the context menu (above), its `NSIS_HOOK_PREINSTALL` takes over an install made under the former name, MaeclyPrint (see [ADR-0009](../adr/0009-rename-to-pluryprint.md)). The app moves the user data itself on first launch.
  - **Per-user install (NSIS, HKCU).** The hook reads the old folder from the unquoted default value of `HKCU\Software\Maecly\MaeclyPrint`, falling back to the unquoted `InstallLocation` and then to the uninstaller's folder. It runs `uninstall.exe /S _?=<folder>`, so it waits for the uninstaller (user data is kept). Only when the uninstaller exits `0` and `maeclyprint.exe` is gone does it delete what registration remains: the `Uninstall\MaeclyPrint` and `HKCU\Software\Maecly\MaeclyPrint` keys, the `*\shell\MaeclyPrint` and `Directory\shell\MaeclyPrint` verbs, and the shortcuts. If the uninstaller fails, the old install keeps its registration, so it can still be removed from "Apps & features".
  - **Per-machine install (the WiX MSI that builds made before `c1466b4`, HKLM).** The setup runs without elevation, so it cannot remove this install by itself. It finds the MSI by `DisplayName`/`Publisher`, asks the user, and runs `msiexec /x <product code> /passive`, which asks for administrator approval (UAC). A silent install, a "No" or a declined UAC prompt leave the MSI install in place, and the setup log says so. The user then removes it from "Apps & features".
  - `bundle_identity.rs` checks the hook's structure and compiles it with `makensis -WX` when `makensis` is installed. **Verify the hook on the Windows VM** with a MaeclyPrint build installed, both per user and from the MSI.
  - The takeover also deletes the MaeclyPrint web view profile (`%LOCALAPPDATA%\com.maecly.print`, about 23 MB) when no MaeclyPrint install is left that could still use it. Files the old app wrote while running elevated belong to SYSTEM and Administrators; a per-user installer cannot delete them, so they can survive (16 of 276 files in the round-3 upgrade test) as wasted space.
- Nothing is code-signed, by decision ([ADR-0011](../adr/0011-release-channel.md)): SmartScreen shows "Windows protected your PC" and the release notes say to choose **More info → Run anyway**. The updater artifacts are signed with the minisign key. See [releasing.md](./releasing.md).
- **Not tested:** the x64 installer running emulated on Windows on ARM. The architecture choice is covered by the hooks' unit test and the native ARM64 install only.

Everything from here down is **the ADR-0006 design, superseded and unimplemented**. It is kept as the record of what was weighed; do not build against it.

---

## Why Inno Setup (ADR-0006 rationale)

Inno Setup 6 was chosen over alternatives for these reasons:

| Tool | Verdict | Why / why not |
|---|---|---|
| **Inno Setup** | Chosen | Free for commercial use, Pascal-like scripting (full control), single self-extracting `.exe`, mature, no admin install supported, large ecosystem of examples. |
| WiX Toolset / MSI | Rejected | MSI requires admin for system install; per-user MSI is convoluted; XML scripting is verbose; Windows Installer service interactions unpredictable for shell extensions. |
| MSIX (full) | Rejected as primary | Requires Microsoft Store association or sideloading flags; restricts what installers can do; AppContainer constraints break some Win32 print API calls. We use **Sparse** MSIX as a complement, not a replacement. |
| NSIS | Rejected | Capable but with smaller mindshare for our specific shell-extension + sparse-MSIX combo. |
| Custom PowerShell installer | Rejected | Antivirus heuristics flag PS-based installers more often; UX is worse. |

## Inno Setup script structure

The script lives at `installers/windows/inno-setup.iss`. Below is the section-by-section structure with the key directives. The full `.iss` is generated via the build script and pinned in the repo for reproducibility.

### `[Setup]` section

```iss
[Setup]
AppId={{B72E6F3A-1C4F-4C8E-9B62-3A4E5D6F7A8B}
AppName=Pluryprint
AppVersion={#MyAppVersion}
AppPublisher=Pluryprint Project
AppPublisherURL=https://maecly.com
AppSupportURL=https://maecly.com/support
AppUpdatesURL=https://maecly.com/releases
DefaultDirName={localappdata}\Programs\Pluryprint
DefaultGroupName=Pluryprint
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
DisableProgramGroupPage=yes
LicenseFile=..\..\LICENSE
OutputDir=output
OutputBaseFilename=Pluryprint-Setup-{#MyAppVersion}
SetupIconFile=..\..\src-tauri\icons\icon.ico
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64 arm64
ArchitecturesAllowed=x64 arm64
MinVersion=10.0.19041
UninstallDisplayIcon={app}\pluryprint.exe
UninstallDisplayName=Pluryprint {#MyAppVersion}
SignTool=signtool
```

Notes:

- **`PrivilegesRequired=lowest`** plus the override allowance lets the user install per-user without admin (default) or system-wide if they elevate.
- **`MinVersion=10.0.19041`** is Windows 10 20H1. Lower versions get a clear "unsupported" dialog rather than a partial install.
- **`AppId` GUID** never changes across versions; it's how Windows recognizes upgrades.
- **`SignTool=signtool`** is configured globally in Inno Setup options to invoke our `signtool` wrapper that signs every produced `.exe`.

### `[Files]` section

```iss
[Files]
Source: "..\..\src-tauri\target\release\pluryprint.exe"; DestDir: "{app}"; Flags: ignoreversion signonce
Source: "..\..\src-tauri\target\release\pluryprint-cli.exe"; DestDir: "{app}"; Flags: ignoreversion signonce
Source: "..\..\src-tauri\target\release\WebView2Loader.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\src-tauri\resources\sumatrapdf\SumatraPDF.exe"; DestDir: "{app}\sumatrapdf"; Flags: ignoreversion
Source: "..\..\src-tauri\resources\sumatrapdf\LICENSE.txt"; DestDir: "{app}\sumatrapdf"; Flags: ignoreversion
Source: "..\..\shell-ext-win\target\release\shell_ext_win.dll"; DestDir: "{app}"; Flags: ignoreversion regserver 64bit
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\..\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "sparse-msix\Pluryprint.msix"; DestDir: "{app}\sparse-msix"; Flags: ignoreversion; Check: IsWindows11
```

Notes:

- `signonce` ensures we sign each binary exactly once during install (we already signed during build, this is belt-and-suspenders for cases where Inno Setup repacks).
- The shell extension DLL gets `regserver` only as a safety net; the canonical registration is via the Sparse MSIX (Win 11) or HKCU registry entries (Win 10), set in `[Registry]`.
- `Check: IsWindows11` skips the MSIX file on Win 10. `IsWindows11` is a custom Pascal function (see [Code] section).

### `[Registry]` section

For Win 10 fallback (when Sparse MSIX is not deployed), we register a classic `IContextMenu` extension via HKCU:

```iss
[Registry]
; CLSID for the in-proc COM server
Root: HKCU; Subkey: "Software\Classes\CLSID\{{B269B8DC-044F-4D65-8F1E-8F20A2E653EA}}"; ValueType: string; ValueData: "Pluryprint Shell Extension"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\CLSID\{{B269B8DC-044F-4D65-8F1E-8F20A2E653EA}}\InprocServer32"; ValueType: string; ValueData: "{app}\shell_ext_win.dll"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\CLSID\{{B269B8DC-044F-4D65-8F1E-8F20A2E653EA}}\InprocServer32"; ValueType: string; ValueName: "ThreadingModel"; ValueData: "Apartment"

; Attach to "AllFileSystemObjects" so the verb appears for any file/folder selection
Root: HKCU; Subkey: "Software\Classes\AllFileSystemObjects\shellex\ContextMenuHandlers\Pluryprint"; ValueType: string; ValueData: "{{B269B8DC-044F-4D65-8F1E-8F20A2E653EA}}"; Flags: uninsdeletekey

; Register the application URL handler for the pluryprint:// scheme (used by IPC handoff)
Root: HKCU; Subkey: "Software\Classes\pluryprint"; ValueType: string; ValueData: "URL:Pluryprint Protocol"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\pluryprint"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\pluryprint\shell\open\command"; ValueType: string; ValueData: """{app}\pluryprint.exe"" --uri ""%1"""
```

Notes:

- All entries are `HKCU\Software\Classes\` (per-user). No `HKLM` writes; no admin needed.
- `uninsdeletekey` ensures the key is removed on uninstall.
- The CLSID GUID `{B269B8DC-044F-4D65-8F1E-8F20A2E653EA}` is fixed across versions; it's the contract with Windows. Its single source of truth is `shell-ext-win/src/clsid.rs` (`CLSID_PLURYPRINT_COMMAND`), which documents the three places it must agree in. Never regenerate it: an already-installed machine keeps the old CLSID registered and the menu entry simply stops appearing.
- Note the Win 10 fallback shape the crate documents is a **verb** under `HKCU\Software\Classes\*\shell\Pluryprint\ExplorerCommandHandler`, i.e. an `IExplorerCommand` handler, not the `shellex\ContextMenuHandlers` `IContextMenu` shape sketched above. The crate implements `IExplorerCommand` only. Whoever writes the real registration must reconcile this before shipping.

### `[Icons]` section

```iss
[Icons]
Name: "{group}\Pluryprint"; Filename: "{app}\pluryprint.exe"
Name: "{group}\Uninstall Pluryprint"; Filename: "{uninstallexe}"
Name: "{userdesktop}\Pluryprint"; Filename: "{app}\pluryprint.exe"; Tasks: desktopicon
```

```iss
[Tasks]
Name: "desktopicon"; Description: "Create a &desktop icon"; GroupDescription: "Additional shortcuts:"; Flags: unchecked
```

### `[Run]` section — Sparse MSIX deployment

```iss
[Run]
Filename: "powershell.exe"; \
  Parameters: "-NoProfile -ExecutionPolicy Bypass -Command ""Add-AppxPackage -Path '{app}\sparse-msix\Pluryprint.msix' -ExternalLocation '{app}'"""; \
  Flags: runhidden waituntilterminated; \
  Check: IsWindows11; \
  StatusMsg: "Registering modern shell integration..."

Filename: "{app}\pluryprint.exe"; Description: "{cm:LaunchProgram,Pluryprint}"; Flags: postinstall skipifsilent nowait
```

The `-ExternalLocation` flag is what makes this a **Sparse** package — the MSIX manifest declares files that live outside the package, in the main install directory. This avoids duplicating the binary while still giving us MSIX identity.

### `[UninstallRun]` section

```iss
[UninstallRun]
Filename: "powershell.exe"; \
  Parameters: "-NoProfile -ExecutionPolicy Bypass -Command ""Get-AppxPackage -Name 'Pluryprint' | Remove-AppxPackage"""; \
  Flags: runhidden waituntilterminated; \
  RunOnceId: "RemoveSparseMsix"
```

### `[Code]` section — custom Pascal logic

```pascal
[Code]
function IsWindows11(): Boolean;
var
  Major, Minor, Build: Cardinal;
begin
  GetWindowsVersionEx(Major, Minor, Build);
  Result := (Major = 10) and (Build >= 22000);
end;

function InitializeSetup(): Boolean;
begin
  Result := True;
  // Warn if optional dependencies missing
  if not CheckLibreOfficeInstalled() then
    MsgBox('LibreOffice is not installed. Office documents will be skipped. ' +
           'Install LibreOffice from https://www.libreoffice.org/ to enable Office support.',
           mbInformation, MB_OK);
end;

function CheckLibreOfficeInstalled(): Boolean;
var
  Found: Boolean;
begin
  Found := RegKeyExists(HKLM, 'SOFTWARE\LibreOffice\UNO\InstallPath') or
           RegKeyExists(HKCU, 'SOFTWARE\LibreOffice\UNO\InstallPath');
  Result := Found;
end;

procedure DeinitializeUninstall();
var
  AppDataPath: String;
begin
  // Optional: prompt user to remove their data
  AppDataPath := ExpandConstant('{userappdata}\Pluryprint');
  if DirExists(AppDataPath) then
  begin
    if MsgBox('Remove your Pluryprint profiles and logs at ' + AppDataPath + '?', mbConfirmation, MB_YESNO) = IDYES then
      DelTree(AppDataPath, True, True, True);
  end;
end;
```

## Sparse MSIX package

### Why "sparse"

A sparse MSIX package contains a manifest only — no payload. Files are declared with paths that point outside the package, into the main install directory. The package's only purpose is to give the shell extension DLL a **package identity**, which Windows 11's modern shell (the new File Explorer) requires for `IExplorerCommand` registration.

Without identity:

- Win 11's modern File Explorer ignores the extension.
- Only the legacy `IContextMenu` path works (Win 10 style), which doesn't surface in the new modern command bar.

### Package layout

```
sparse-msix/
├── AppxManifest.xml
├── Assets/
│   ├── StoreLogo.png         (50×50)
│   ├── Square150x150Logo.png
│   ├── Square44x44Logo.png
│   └── Wide310x150Logo.png
└── (no other files — payload lives in main install dir)
```

### `AppxManifest.xml`

```xml
<?xml version="1.0" encoding="utf-8"?>
<Package
  xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
  xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
  xmlns:uap3="http://schemas.microsoft.com/appx/manifest/uap/windows10/3"
  xmlns:uap10="http://schemas.microsoft.com/appx/manifest/uap/windows10/10"
  xmlns:com="http://schemas.microsoft.com/appx/manifest/com/windows10"
  xmlns:desktop="http://schemas.microsoft.com/appx/manifest/desktop/windows10"
  xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities"
  IgnorableNamespaces="uap uap3 uap10 com desktop rescap">

  <!-- These three values MUST match shell-ext-win/res/shellext.manifest
       character for character: Identity/@Publisher, Identity/@Name and
       Applications/Application/@Id. The embedded manifest already commits to
       publisher="CN=Maecly, O=Maecly, C=ES", packageName="Pluryprint",
       applicationId="Pluryprint" — so this Identity block is not free to
       choose. A mismatch surfaces as 0x80073D54, or as an extension that
       simply never appears. -->
  <Identity
    Name="Pluryprint"
    Publisher="CN=Maecly, O=Maecly, C=ES"
    Version="1.0.0.0" />

  <Properties>
    <DisplayName>Pluryprint</DisplayName>
    <PublisherDisplayName>Pluryprint Project</PublisherDisplayName>
    <Logo>Assets\StoreLogo.png</Logo>
  </Properties>

  <Resources>
    <Resource Language="en-us" />
  </Resources>

  <Dependencies>
    <TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.22000.0" MaxVersionTested="10.0.26200.0" />
  </Dependencies>

  <Capabilities>
    <rescap:Capability Name="runFullTrust" />
  </Capabilities>

  <Applications>
    <Application Id="Pluryprint"
                 Executable="pluryprint.exe"
                 uap10:TrustLevel="mediumIL"
                 uap10:RuntimeBehavior="win32App">
      <uap:VisualElements
        DisplayName="Pluryprint"
        Description="Batch print N files from File Explorer"
        BackgroundColor="transparent"
        Square150x150Logo="Assets\Square150x150Logo.png"
        Square44x44Logo="Assets\Square44x44Logo.png">
        <uap:DefaultTile Wide310x150Logo="Assets\Wide310x150Logo.png" />
      </uap:VisualElements>

      <Extensions>
        <com:Extension Category="windows.comServer">
          <com:ComServer>
            <com:SurrogateServer DisplayName="Pluryprint Shell Extension">
              <com:Class Id="B269B8DC-044F-4D65-8F1E-8F20A2E653EA" Path="shell_ext_win.dll" ThreadingModel="STA" />
            </com:SurrogateServer>
          </com:ComServer>
        </com:Extension>

        <desktop:Extension Category="windows.fileExplorerContextMenus">
          <desktop:FileExplorerContextMenus>
            <desktop:ItemType Type="*">
              <desktop:Verb Id="PluryprintVerb" Clsid="B269B8DC-044F-4D65-8F1E-8F20A2E653EA" />
            </desktop:ItemType>
            <desktop:ItemType Type="Directory">
              <desktop:Verb Id="PluryprintVerb" Clsid="B269B8DC-044F-4D65-8F1E-8F20A2E653EA" />
            </desktop:ItemType>
          </desktop:FileExplorerContextMenus>
        </desktop:Extension>
      </Extensions>
    </Application>
  </Applications>
</Package>
```

The CLSID `B269B8DC-...` matches the one in the Inno Setup `[Registry]` section. They must agree.

## Install directory layout

After install:

```
%LOCALAPPDATA%\Programs\Pluryprint\
├── pluryprint.exe              # Main app (Tauri runtime + all logic)
├── pluryprint-cli.exe          # Console CLI front end (scripts wait for it; see ipc-and-cli.md §3.1)
├── shell_ext_win.dll            # COM in-proc server
├── LICENSE                      # AGPL-3.0 (the WebView2 loader is linked into pluryprint.exe; no DLL)
├── THIRD-PARTY-NOTICES.md       # Third-party notices
├── LICENSES\                    # Apache-2.0, GPL-3.0 (SumatraPDF), OFL-1.1 (JetBrains Mono)
├── CHANGELOG.md
├── sumatrapdf\
│   ├── SumatraPDF.exe           # Bundled, GPL-3.0 (MuPDF inside: AGPL-3.0)
│   └── LICENSE.txt              # SumatraPDF's GPL-3.0 text (also COPYING.BSD.txt, AUTHORS.txt, SOURCE.txt)
├── sparse-msix\
│   ├── AppxManifest.xml
│   └── Assets\
└── unins000.exe                 # Inno Setup-generated uninstaller
```

User data lives separately:

```
%APPDATA%\Maecly\Pluryprint\config\
├── config.toml       # App settings (FR-100..103)
└── profiles.toml     # ALL custom profiles, as [[profile]] entries — one file,
                      #   not a directory of per-profile files

%LOCALAPPDATA%\Maecly\Pluryprint\
├── data\
│   ├── queue.json        # The persisted queue (NFR-010: survives a crash)
│   ├── shell-menu.json   # Explorer menu labels + profile rows (written by the app)
│   ├── handoff\          # Short-lived context-menu selections
│   └── logs\
│       ├── pluryprint-2026-05-03.jsonl
│       └── (rolls daily AND at a size ceiling; retained 14 days / 7 files,
│            whichever bound bites first — see src-tauri/src/logging/mod.rs)
└── cache\
    └── conversions\      # Converted PDFs, swept by the app
```

This separation matters: install-area is read-only after install (we never write there at runtime); user-area is everything that survives uninstall unless the user opts in to wipe.

## Registry surface

Complete list of the registry writes the designed installer would perform (all under `HKCU\Software\Classes\`). Nothing writes them today; during development the registration is manual.

| Key | Purpose |
|---|---|
| `CLSID\{B269B8DC-...}` | Friendly name of the COM server |
| `CLSID\{B269B8DC-...}\InprocServer32` | DLL path + threading model |
| `AllFileSystemObjects\shellex\ContextMenuHandlers\Pluryprint` | Hooks the verb into Win 10 File Explorer |
| `pluryprint` | URL protocol handler for `pluryprint://` IPC scheme |
| `pluryprint\shell\open\command` | Command to launch when URL is invoked |

Plus a single key under `HKCU\Software\Pluryprint`:

| Key | Purpose |
|---|---|
| `HKCU\Software\Pluryprint\Settings` | App-level settings (channel, language, etc.) — written by app at runtime, not by installer |

Inno Setup's standard `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\<AppId>_is1` key is also written automatically (for Add/Remove Programs).

## Dependency detection

> Still part of the unimplemented design. The installer that would run these
> checks does not exist. The app itself already does the runtime half: a job
> that needs a missing converter pauses with a remediation message (FR-058,
> see [supported-formats.md](../03-requirements/supported-formats.md)), and
> the Print Job screen shows it for the files the user selected.

The installer would **detect** but never **install** optional runtimes:

| Dependency | Detection method | If missing |
|---|---|---|
| WebView2 Runtime | `HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}\pv` | Hard error: WebView2 is required (preinstalled on Win 11; bootstrap link shown for Win 10). |
| LibreOffice | `HKLM` or `HKCU` `SOFTWARE\LibreOffice\UNO\InstallPath` | Soft warning: Office, OpenDocument and RTF jobs will pause until it is installed (plain text prints with the system web view and never needs it). |
| Pandoc, Edge / Chrome | Not checked | Nothing: Markdown, HTML and text print with the system web view (ADR-0012), and the headless-browser fallback was removed in round 6. An earlier draft of this table listed both. |
| HEIF Image Extensions | WIC decoder lookup (the app does this at conversion time) | Soft warning: HEIC/HEIF jobs pause with a Microsoft Store link. |

**ImageMagick is deliberately not on this list.** Image conversion is in-process (the pure-Rust `image` crate), so there is nothing to detect; an earlier draft of this table listed it.

We **never** silently install third-party software. Every install action is the user's choice.

## Uninstall flow

When the user runs the uninstaller (`unins000.exe`):

1. **Stop running app:** if `pluryprint.exe` is running, prompt the user to close it (or wait + retry).
2. **Remove sparse MSIX:** `Remove-AppxPackage -Name Pluryprint`. If this fails (rare), log and continue — orphan MSIX entries are harmless.
3. **Unregister shell extension DLL:** `regsvr32 /u shell_ext_win.dll` (no-op if Inno Setup's `regserver` flag wasn't used, but defensive).
4. **Delete registry entries:** all `HKCU\Software\Classes\` entries listed in `[Registry]` (Inno Setup handles via `uninsdeletekey`).
5. **Delete install directory:** Inno Setup default behavior.
6. **Prompt for user data:** ask whether to delete `%APPDATA%\Maecly\Pluryprint` and `%LOCALAPPDATA%\Maecly\Pluryprint`. Default is **no** (preserve user data).
7. **Refresh shell:** broadcast `WM_SETTINGCHANGE` so File Explorer re-reads context menu registrations.

After a clean uninstall (with user data wiped), `regedit` and `Process Monitor` should find zero Pluryprint references. **This is not tested anywhere.** There is no installer test job in `ci.yml`, and there cannot be one until an installer exists; today it is a manual QA item on the release checklist.

## Build script

**This file does not exist.** `installers/windows/build.ps1` would be the single entrypoint. It would:

0. Run `scripts/fetch-sumatrapdf.ps1 -Arch <x64|arm64>` for the installer's architecture (or verify a previous run left `SumatraPDF.exe`, `LICENSE.txt` and a `SOURCE.txt` naming that architecture in `src-tauri/resources/sumatrapdf/`). Skipping this is the difference between a working installer and one whose every print job fails.
1. Validate that `pluryprint.exe`, `shell_ext_win.dll` and `SumatraPDF.exe` exist in expected locations — note the DLL comes out of `shell-ext-win/target/<triple>/release/`, a *separate* target directory from the app's, because the crate is not a workspace member.
2. Read the version from `src-tauri/tauri.conf.json`.
3. Generate the sparse MSIX with `MakeAppx.exe` from the Windows SDK.
4. Sign the MSIX with the EV cert (release builds) or self-signed cert (nightly builds) — neither certificate exists yet; see [code-signing.md](./code-signing.md).
5. Invoke `iscc.exe inno-setup.iss /DMyAppVersion=<version>`.
6. Verify the produced `setup.exe` is signed: `signtool verify /pa /v setup.exe`.
7. Output the SHA-256 of the final installer for the release manifest.

```powershell
# installers/windows/build.ps1 (excerpt)
param(
  [Parameter(Mandatory)] [string]$Version,
  [ValidateSet('release', 'nightly')] [string]$Channel = 'release'
)

$ErrorActionPreference = 'Stop'

# 1. Validate prerequisites
$required = @('iscc.exe', 'MakeAppx.exe', 'SignTool.exe')
foreach ($tool in $required) {
  if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
    throw "Required tool not found in PATH: $tool"
  }
}

# 2. Build sparse MSIX
$msixPath = "sparse-msix/Pluryprint.msix"
& MakeAppx.exe pack /d "sparse-msix/" /p $msixPath /nv | Out-Host
if ($LASTEXITCODE -ne 0) { throw "MakeAppx failed" }

# 3. Sign MSIX
$cert = if ($Channel -eq 'release') { $env:EV_CERT_THUMBPRINT } else { $env:NIGHTLY_CERT_THUMBPRINT }
& SignTool.exe sign /sha1 $cert /fd SHA256 /td SHA256 /tr "http://timestamp.digicert.com" $msixPath
if ($LASTEXITCODE -ne 0) { throw "MSIX signing failed" }

# 4. Build setup.exe
& iscc.exe "inno-setup.iss" "/DMyAppVersion=$Version" "/DChannel=$Channel"
if ($LASTEXITCODE -ne 0) { throw "Inno Setup compile failed" }

# 5. Verify signature
$setupPath = "output/Pluryprint-Setup-$Version.exe"
& SignTool.exe verify /pa /v $setupPath
if ($LASTEXITCODE -ne 0) { throw "Setup verification failed" }

# 6. Emit hash
$hash = (Get-FileHash $setupPath -Algorithm SHA256).Hash
"setup.exe SHA-256: $hash" | Out-Host
$hash | Out-File "$setupPath.sha256" -Encoding ASCII -NoNewline
```

## Reproducibility

Two builds from the same commit must produce byte-identical `setup.exe`. To achieve this:

- Inno Setup's `Compression=lzma2/ultra64` is deterministic given the same input file order.
- Files are listed in `[Files]` in a fixed order.
- The build script enforces UTC timestamps via `SOURCE_DATE_EPOCH` env var (Inno Setup 6.2+ supports this).
- Sparse MSIX is built with `MakeAppx /nv` (no version stamp randomness).
- Signing introduces a timestamp; we accept that signed binaries differ in the timestamp byte range only. The unsigned binary is the bit-for-bit reproducible artifact.

**Not verified.** CI does not build twice and diff the results; NFR-064 has no automated check today. The intent is a double-build reproducibility job in the release pipeline whenever that pipeline is written.

## See also

- [installer-macos.md](./installer-macos.md) — macOS counterpart
- [code-signing.md](./code-signing.md) — signing details for Windows and macOS
- [installer-linux.md](./installer-linux.md) — the Linux AppImage and `.deb`
- [auto-update-flow.md](./auto-update-flow.md) — how installed apps receive updates
- [ci-cd.md](./ci-cd.md) — what CI really builds, and the list of release machinery that does not exist
- [../adr/0006-installer-inno-setup.md](../adr/0006-installer-inno-setup.md) — the decision this document is measured against
- [../adr/0005-shell-extension-sparse-msix.md](../adr/0005-shell-extension-sparse-msix.md) — why package identity is required at all
