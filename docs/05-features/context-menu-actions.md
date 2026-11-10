# Context Menu Actions

> **TL;DR** — UX design of the right-click submenu Pluryprint adds to Windows Explorer (and, in reduced forms, to the macOS Finder and to Linux file managers): which items appear, when, what they do, and how the layout differs by selection type.

This document is the source of truth for the **content and behavior** of the context menu integration. Implementation lives in the [shell-extensions architecture doc](../04-architecture/shell-extensions.md); UX behavior lives here.

## Submenu structure (as shipped on Windows 11)

The Pluryprint integration adds a single top-level entry to the modern context menu, expanding into a **flat** submenu. Windows Explorer does not support a submenu inside a submenu (`IExplorerCommand::EnumSubCommands`: "subcommands which themselves have subcommands are not supported"), and a handler may contribute at most 16 items, so the per-profile entries sit directly in the submenu instead of under a nested "Print with profile ▸".

```
┌─ Pluryprint submenu ──────────────────────┐
│  Print 23 files                 (bold)      │  default profile
│  ────────────────────                       │
│  Print with “Quick”                         │  one row per profile,
│  Print with “Duplex BW”                     │  at most 8
│  ────────────────────                       │
│  Configure print job…                       │
│  Add to print queue                         │
└─────────────────────────────────────────────┘
```

The menu model is `shell-ext-win/src/strings.rs` (`Strings::menu`); its labels and profile rows come from the cache the app writes (see [Localization](#localization)).

## Items in detail

### Print N files (default profile)

- **When shown:** whenever the entry is shown (see [Filtering](#filtering)).
- **Label:** `Print 1 file` / `Print {count} files`. `{count}` is the size of the **selection** as Explorer reports it — files and folders alike — not the number of supported files: counting those would mean walking the selection on Explorer's UI thread before the menu is drawn.
- **Action:** hands the selection to the app with action `print-default`; the app prints the supported files with the default profile and reports what it skipped.

### Print with “<profile>”

- **When shown:** one row per profile, built-ins first, then custom profiles alphabetically, at most 8 (`MAX_MENU_PROFILES`). Other profiles are reachable through **Configure print job…**. No rows until the app has written its cache once.
- **Action:** same as the default entry, with that profile (`print-default` + `profile_id`).

### Configure print job…

- **Action:** opens the GUI on the **Print Job** page with the selection pre-loaded (`configure`).
- **GUI start time:** must comply with [NFR-001](../03-requirements/non-functional-requirements.md#performance) (<500 ms p95).

### Add to print queue

- **Action:** hands the selection over with action `queue`: the files are queued with the default profile without starting to print, so the user can review the queue and start it.

### How the app handles each action (implementation)

- The integration hands over `action` (`configure` / `print-default` / `queue`) and an optional `profile_id`, which may be a profile **id or name** (as `--profile <NAME-OR-ID>` allows); the app re-reads `profiles.toml` before resolving it (a profile created by `pluryprint-cli profiles create` while the window is open is offered by the menu at once, so the window's cached list is not the authority), matches the id first, then the name case-insensitively. When neither matches, nothing is printed or queued: an error toast names the profile, and the files are opened on the Print Job screen so the user can pick a profile. The default profile is never substituted (`src/App.tsx` `applySelection`; tests `FR-001: a print-now selection naming an unknown profile prints nothing`, `FR-001: a profile created by the CLI after launch is honoured…`, and `tests/profile_roundtrip.rs::fr_001_profile_created_out_of_process_is_resolvable_without_restart`).
- `print-default` and `queue` never touch the Print Job screen's list or its overrides: the selection is resolved on its own (folders walked top-level, no question) and printed with the profile exactly as saved.
- `queue` submits the batch with `hold: true`: every job of **that batch** is enqueued `paused` for the user to resume from the Queue screen; the global queue pause is not touched, so other batches keep printing.
- Selections are processed strictly one at a time, in arrival order.

## Designed, not implemented

These rows were part of the original design and are **not** in the shipped menu. They stay here as the specification if they are built:

- **Open Pluryprint**, also on the Explorer background (right-click on empty space). Today the extension registers for files and folders only.
- **Supported-count labels**, e.g. `Print 18 of 23 files (5 unsupported)`, and a greyed `⚠ 5 files cannot be printed (.exe, .zip)` row. Both need the extension of every selected item at menu time, which is the per-item work the 50 ms budget below rules out for large selections; the app reports skipped files after the click instead.
- **Folder rows** `Print files in folder (top level)` / `(recursive)`. Today a folder is handed to the app like any other item, and the app applies its own folder rule ([FR-004](../03-requirements/functional-requirements.md)).
- A greyed **No printable files** row: an entry with nothing printable is hidden instead.

## Filtering

The shell extension filters the selection before displaying counts:

- The supported extensions are compiled into the DLL (`shell-ext-win/src/supported.rs`), mirroring the engine's formats.
- Filtering is by extension only (not MIME or content sniffing) for performance — the menu must appear instantly.
- Only the first items of the selection are probed. If none of them is printable (and none is a folder), the whole Pluryprint entry is hidden; a selection larger than the probe always shows it, because its unseen tail may be printable.

## Performance constraints

The shell extension code path runs synchronously in Explorer's UI thread. To stay imperceptible:

- The DLL's `EnumSubCommands` and `GetTitle` implementations must return in <50 ms total even for selections of 1000 items.
- Profile names and labels are read from a cached file the app refreshes on profile and language changes: `<data_local_dir>\shell-menu.json`, i.e. `%LOCALAPPDATA%\Maecly\Pluryprint\data\shell-menu.json`.
- The filter pass is `O(N)` over the selection but uses a hash set of supported extensions for `O(1)` lookup per file.
- No I/O beyond the cache file is permitted.

## Differences across menu environments

### Windows 11 modern menu (default) — not shipped

- The design: the submenu at the same level as Open / Share / Copy, with icons adapted to the system theme.
- It needs a sparse package signed with a certificate the machine trusts, and there is no certificate yet ([ADR-0013](../adr/0013-explorer-command-per-user-verb.md), decision D3). The entry is therefore not in the top-level menu today.

### Windows 11 classic menu (Show more options) — shipped

- The installer registers the `IExplorerCommand` handler per user as an `ExplorerCommandHandler` verb under `HKCU\Software\Classes\*\shell\Pluryprint` and `Directory\shell\Pluryprint`, so the submenu appears under **Show more options** (or Shift+F10, or Shift+right-click). Verified with real right-clicks on Windows 11 24H2 ARM64.
- The app says where it is (Home, the first-run tour, Settings).

### Windows 10

- Has only the classic menu; the same per-user registration shows the submenu there. Not yet run on a Windows 10 machine.
- Same submenu content.

### macOS Finder — shipped

- An `NSServices` declaration inside the app bundle (no `~/Library/Services` workflow, no extension to enable) adds two items to the **Services** submenu of the right-click menu: **Print with Pluryprint** (opens the print dialog with the selection) and **Print with Pluryprint (default profile)** (prints at once). No per-profile rows. See [shell-extensions.md §3](../04-architecture/shell-extensions.md).
- A Finder Sync extension (a submenu equivalent to Windows) stays deferred.
- macOS shortcuts conventionally appear under **Quick Actions** in modern Finder, which is acceptable but slightly less discoverable than Windows.

### Linux — decided for 0.0.5, preview ([ADR-0015](../adr/0015-linux-support.md))

- **Open with → Pluryprint** in every file manager, from the `.desktop` file's `MimeType=` list: the print dialog opens with the selection, like **Configure print job**.
- **Dolphin (KDE) and Nemo (Cinnamon)**, installed by the `.deb` only: a Pluryprint entry with **Print now** (the default profile, at once), **Configure print job** and **Add to print queue**. No per-profile rows: the entries are static files, not drawn from the app's profiles.
- **Not in this release:** GNOME Files (Nautilus, which would need `nautilus-python`), Thunar and Caja; and the AppImage does not register itself in any file manager. See [shell-extensions.md §4](../04-architecture/shell-extensions.md).

## Visual design notes

- Icons follow the host OS style: PNG with transparency at 16/24/32/48 px on Windows; PDF or SVG on macOS.
- Submenu uses bold for the default action ("Print N files") to draw the eye.
- Counts use the catalog's `_one` / `_other` forms (`Print 1 file` / `Print {count} files`).

## Localization

The labels are i18n keys in the app's catalog, `src/i18n/locales/{en,es}/common.json` → `shellMenu` (`print_one`, `print_other`, `printWithProfile`, `configure`, `queue`, `tooltip`). The DLL cannot load the app's catalog on Explorer's UI thread, so the app renders the labels for the current UI language into `shell-menu.json` (`src-tauri/src/shell_menu.rs`) at startup and whenever profiles, the default profile or the language change:

```json
{
  "schema_version": 1,
  "lang": "es",
  "labels": {
    "root": "Pluryprint",
    "print_one": "Imprimir 1 archivo",
    "print_many": "Imprimir {count} archivos",
    "print_with_profile": "Imprimir con «{name}»",
    "configure": "Configurar el trabajo de impresión…",
    "queue": "Añadir a la cola de impresión",
    "tooltip": "Imprime por lotes los archivos seleccionados con Pluryprint"
  },
  "profiles": [{ "id": "quick", "name": "Quick" }]
}
```

A missing, corrupt or wrong-version cache falls back to built-in English with no profile rows.

## Telemetry note

Context menu invocations are NOT counted, tracked, or reported anywhere. The shell extension touches only the cache file and spawns the main executable.

## Open questions for v1+

- Should we support a per-folder profile override? (Right-click a folder → "Always use profile X for this folder.")
- Should the cascade auto-collapse when there are very few profiles? (If only 2 profiles total, skip the submenu and inline both?)
- Should we offer a "Print and email" composite action for office workflows?

These are tracked as Discussions in the GitHub repo, not commitments.
