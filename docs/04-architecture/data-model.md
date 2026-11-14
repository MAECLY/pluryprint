# Data Model

> **TL;DR** — Pluryprint persists three kinds of state: human-editable **TOML** (`config.toml`, `profiles.toml`), machine-managed **JSON** (`queue.json`, `update-state.json`, and the transient shell-extension handoff files), and append-only **JSONL** (logs). Files live under XDG-equivalent paths on each OS. The two files that need it — `config.toml` and `queue.json` — carry a `schema_version` (both `1` today); `update-state.json` deliberately does not, because it is disposable bookkeeping that must degrade to "start over" rather than to a parse refusal. Its shape is documented in [auto-updater.md §4.1](./auto-updater.md#41-persisted-state).

## 1. File locations

| File                                | Windows                                                          | macOS                                                          |
| ----------------------------------- | ---------------------------------------------------------------- | -------------------------------------------------------------- |
| Config                              | `%APPDATA%\Maecly\Pluryprint\config\config.toml`                              | `~/Library/Application Support/com.Maecly.Pluryprint/config.toml`        |
| Profiles                            | `%APPDATA%\Maecly\Pluryprint\config\profiles.toml`                            | `~/Library/Application Support/com.Maecly.Pluryprint/profiles.toml`      |
| Queue (live state)                  | `%LOCALAPPDATA%\Maecly\Pluryprint\data\queue.json`                          | `~/Library/Application Support/com.Maecly.Pluryprint/queue.json`         |
| Update bookkeeping                  | `%LOCALAPPDATA%\Maecly\Pluryprint\data\update-state.json`                   | `~/Library/Application Support/com.Maecly.Pluryprint/update-state.json`  |
| Shell-extension handoff             | `%LOCALAPPDATA%\Maecly\Pluryprint\data\handoff\`                            | `~/Library/Application Support/com.Maecly.Pluryprint/handoff/`           |
| PluryFact bridge inbox (FR-126) | `%LOCALAPPDATA%\Maecly\Pluryprint\data\inbox\<jobId>\` | `~/Library/Application Support/com.Maecly.Pluryprint/inbox/<jobId>/` |
| Explorer menu labels and profiles   | `%LOCALAPPDATA%\Maecly\Pluryprint\data\shell-menu.json` (written by the app, read by the DLL) | — |
| Print-quality swap journal (D16)    | `%LOCALAPPDATA%\Maecly\Pluryprint\data\devmode-swap\devmode-swap-<printer hash>.json` and `.lock`; present only while a swap is in flight or after a crash | — |
| Logs (see §5.2 for rotation)        | `%LOCALAPPDATA%\Maecly\Pluryprint\data\logs\pluryprint-YYYY-MM-DD.jsonl`   | `~/Library/Logs/Pluryprint/pluryprint-YYYY-MM-DD.jsonl`      |
| Cache (converted PDFs)              | `%LOCALAPPDATA%\Maecly\Pluryprint\cache\conversions\<job-id>.pdf`            | `~/Library/Caches/com.Maecly.Pluryprint/conversions/<job-id>.pdf`        |
| Enterprise policy (**read-only**)   | `%ProgramData%\Pluryprint\policy.json`                          | `/Library/Application Support/Pluryprint/policy.json`         |

On **Linux** (a preview platform, [ADR-0015](../adr/0015-linux-support.md)) `ProjectDirs` follows the XDG Base Directory specification and uses the application name in lower case, so every per-user file sits under one of three folders, each moved by its `XDG_*_HOME` variable:

| File                                | Linux                                                            |
| ----------------------------------- | ---------------------------------------------------------------- |
| Config, Profiles                    | `~/.config/pluryprint/config.toml`, `profiles.toml` (`$XDG_CONFIG_HOME/pluryprint/`) |
| Queue, update bookkeeping           | `~/.local/share/pluryprint/queue.json`, `update-state.json` (`$XDG_DATA_HOME/pluryprint/`) |
| Logs                                | `~/.local/share/pluryprint/logs/pluryprint-YYYY-MM-DD.jsonl`      |
| Cache (converted PDFs)              | `~/.cache/pluryprint/conversions/<job-id>.pdf` (`$XDG_CACHE_HOME/pluryprint/`) |
| Enterprise policy (**read-only**)   | `/etc/pluryprint/policy.json`                                    |

The Linux paths were checked against `directories` 6.0.0 on Ubuntu 22.04 (`ProjectDirs::from("com", "Maecly", "Pluryprint")` gives `~/.config/pluryprint`, `~/.local/share/pluryprint` and `~/.cache/pluryprint`); the AppImage and the `.deb` use the same folders, so switching between them keeps the user's settings, profiles and queue.

Note the **`conversions/` subdirectory** — converted PDFs are not written directly into the cache root; a protected PDF's decrypted copy is the job's cache PDF too, deleted with the job. Pluryprint keeps no updater staging directory of its own: `tauri-plugin-updater` downloads an update only when the user asks and hands it to the installer (see [auto-updater.md](./auto-updater.md)). Each conversion's scratch folder (`pluryprint-convert-<job>-*`) lives in the OS temp directory and is removed when the conversion ends.

`update-state.json`, `handoff/` and the PluryFact bridge's `inbox/` (documents it received, one folder per job, deleted after 7 days; [ADR-0016](../adr/0016-pluryfact-browser-bridge.md)) live under the **data** directory, alongside `queue.json`, because all three are live machine-managed state rather than user configuration. On Windows the data and cache directories are the `data\` and `cache\` subfolders of `%LOCALAPPDATA%\Maecly\Pluryprint\`, and config is the `config\` subfolder of the roaming `%APPDATA%\Maecly\Pluryprint\`; on macOS config and data share `com.Maecly.Pluryprint` under Application Support and the cache is a separate tree; on Linux config, data and cache are three separate XDG trees, and the logs are the `logs/` subfolder of the data folder, as on Windows. The layout is `directories::ProjectDirs::from("com", "Maecly", "Pluryprint")`, which the shell extension uses too.

We do not write under `%PROGRAMFILES%`, `/Applications` or `/usr` after install — the user does not need admin rights at any time after setup, and uninstall does not orphan user-state.

**Upgrading from MaeclyPrint.** Before the rename ([ADR-0009](../adr/0009-rename-to-pluryprint.md)) the application segment was `MaeclyPrint`, so every path above had `MaeclyPrint` where it now has `Pluryprint` (`com.Maecly.MaeclyPrint`, `~/Library/Logs/MaeclyPrint`, `maeclyprint-<date>.jsonl`). On launch, before logging starts, the GUI and the CLI move what is still in those folders into the new ones (`src-tauri/src/util/legacy.rs`). An entry whose name already exists at the new location is never overwritten; it stays where it was and is logged (`legacy_migration_kept`). The exception is a file holding only what a first run writes for itself (default settings apart from the detected locale, no custom profiles, an empty queue, no skipped update, the menu cache): it is renamed to `<name>.first-run.bak` and the old file takes its place, so a first launch made while MaeclyPrint was still running does not strand the user's data. A copy across volumes goes through a hidden temporary and is renamed into place only when whole. Old log files are renamed to the `pluryprint-` prefix so retention prunes them. The move holds `migration.lock` in the new data folder (a second launch waits for it; the GUI keeps it until it holds its own `instance.lock`), is deferred while a Pluryprint app runs, and holds MaeclyPrint's `instance.lock` for its whole duration; a lock that cannot be checked counts as held. A run without failures writes `legacy-migration.done` next to `queue.json`, after which launches skip the step. An upgrade also sets `[app].onboarding_complete` in `config.toml`, and `[app].locale_detected` when it moved a `config.toml`, because the old app kept those flags in the webview's storage, which the new bundle identifier starts empty. `PLURYPRINT_SKIP_MIGRATION=1` turns the step off. The enterprise policy is not moved (a user process cannot): the MaeclyPrint registry key and `policy.json` locations are read after every Pluryprint one, with the same trust rules.

Unlike every file above, `policy.json` is **read-only to Pluryprint**: an administrator provisions it and the app only ever reads it. On Windows the registry policy `HKLM\SOFTWARE\Policies\Maecly\Pluryprint` (`DefaultPrinter`) wins, and the `%ProgramData%` file is honored only when Administrators or SYSTEM own it; a per-user copy at `<config dir>/policy.json` is honored in debug builds only. Its schema, precedence, and malformed-file behavior are documented in [security-model.md §4.3](./security-model.md#43-enterprise-policy-policyjson); the shape is a single optional field, `{ "default_printer": "<printer id>" }`.

These paths are computed by the `directories` crate (XDG-conformant on Linux, `KnownFolderId` on Windows, `~/Library/...` on macOS) wrapped in a `paths` module that is the only place hardcoded names appear.

```rust
// src/util/paths.rs
use directories::ProjectDirs;
use std::path::PathBuf;

const QUALIFIER: &str = "com";
const ORG:       &str = "Maecly";
const APP:       &str = "Pluryprint";

pub fn config_dir() -> PathBuf { dirs().config_dir().to_path_buf() }
pub fn data_dir()   -> PathBuf { dirs().data_local_dir().to_path_buf() }   // note: *local*
pub fn cache_dir()  -> PathBuf { dirs().cache_dir().to_path_buf() }
pub fn log_dir()    -> PathBuf {
    #[cfg(target_os = "macos")]
    { /* ~/Library/Logs/Pluryprint, falling back to data_dir()/logs */ }
    #[cfg(not(target_os = "macos"))]
    { data_dir().join("logs") }
}

fn dirs() -> ProjectDirs {
    ProjectDirs::from(QUALIFIER, ORG, APP).expect("home directory must be resolvable")
}
```

`data_dir()` uses `data_local_dir()`, **not** `data_dir()`: on Windows that is the difference between `%LOCALAPPDATA%` (machine-local, not roamed) and `%APPDATA%` (roamed). A print queue and a conversion cache must not follow a user onto another machine.

The same module owns **`canonical_input`**, the single gate every externally-supplied path passes through before it is stored or printed (NFR-025):

- A **relative** path is refused, never resolved — it would silently land on the process' working directory, which is wherever the OS happened to launch the app from (an Explorer window, `/`, the shell extension's cwd) and is never what the user pointed at. The CLI is the one exception and resolves relative paths itself, *before* calling in.
- `.`/`..` segments and symlinks are resolved, so the same file always gets the same identity in the queue no matter how it was named. A symlink resolving to its target is deliberate: a link to a real document prints that document.
- A path that does not exist is refused up front, so the error names the file instead of surfacing later from inside a converter.
- On Windows the verbatim `\\?\C:\…` form that `canonicalize` returns is simplified back to a drive-letter path, because the external tools we spawn and the UI both dislike it. Verbatim **UNC** paths (`\\?\UNC\…`) are left alone, since there the prefix carries meaning.

## 2. `config.toml` — global settings

### 2.1 Schema

```toml
schema_version = 1

[app]
locale = "en-US"
theme  = "system"            # "light" | "dark" | "system"
launch_minimized = false
density = "comfortable"      # "comfortable" | "compact"

[printers]                   # printer preferences — see § 2.3

[engine]
max_concurrent_conversions = 2   # 1..=8; clamped and applied at next app start
conversion_timeout_seconds = 60  # 0 disables the timeout, else 10..=3600
print_timeout_seconds = 300      # spool + wait for the OS job; 0 disables, else 10..=3600
max_file_size_mb           = 200 # 1..=4096; enforced at inspect/submit time
default_profile = "builtin-quick"  # profile *id*; omitted → builtin-quick
# html_renderer = "browser"     # RETIRED (round 6, ADR-0012): read but never used; removed by the
                                # app's one-time notice (take_retired_renderer_notice); not on the wire

[updater]
channel = "stable"           # "stable" | "beta" | "nightly"
check_on_startup = true

[telemetry]
crash_reports = false            # consent only; nothing is transmitted in this version
anonymous_usage_metrics = false  # consent only
```

Every section beyond `schema_version` is `#[serde(default)]`, so a file written by an older build (missing whole sections) still loads with those sections filled from their defaults. Two fields **skip-serialize when unset**, so a fresh `config.toml` omits them: `engine.default_profile` (absent means `builtin-quick`) and — being an `Option` — is only written once a non-default is chosen. The `[app].density` field also `#[serde(default)]`s to `"comfortable"` for configs written before it existed.

**Dropped for the MVP.** Earlier drafts of this schema listed `[shell_extension]`, `[engine.converters]`, and `[logging]` tables plus `engine.max_file_size_bytes` / `engine.auto_retry_timeouts` and `updater.startup_delay_seconds` / `updater.include_prerelease`. None of those ship today: shell-integration config lands with **M2**, converter paths with the multi-format work (**M3**), and the file-size limit is expressed in **megabytes** (`max_file_size_mb`), not bytes. `default_profile` holds a profile **id** (e.g. `builtin-quick`), not a display name like the old `"Quick BW Duplex"`.

### 2.2 Rust binding

```rust
// src/settings/mod.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)] pub app:       AppCfg,
    #[serde(default)] pub printers:  PrinterPrefs,   // see § 2.3
    #[serde(default)] pub engine:    EngineCfg,
    #[serde(default)] pub updater:   UpdaterCfg,
    #[serde(default)] pub telemetry: TelemetryCfg,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AppCfg {
    pub locale: String,               // default "en-US"
    pub theme:  Theme,                // default System
    pub launch_minimized: bool,
    #[serde(default)] pub density: Density,   // default Comfortable
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum Theme { Light, Dark, System }

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum Density { #[default] Comfortable, Compact }

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct EngineCfg {
    pub max_concurrent_conversions: u32,   // default 2, clamped 1..=8 at boot
    pub conversion_timeout_seconds: u32,   // default 60; 0 disables
    pub print_timeout_seconds: u32,        // default 300; 0 disables
    pub max_file_size_mb: u32,             // default 200
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,   // profile id; None → builtin-quick
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct UpdaterCfg {
    pub channel: UpdateChannel,            // default Stable
    pub check_on_startup: bool,            // default true
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel { #[default] Stable, Beta, Nightly }

#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct TelemetryCfg {
    #[serde(default)] pub crash_reports: bool,
    #[serde(default)] pub anonymous_usage_metrics: bool,
}
```

The `Default` impl provides the values shown above. Reads go through `Config::load_from(path)` (`toml::from_str`), which degrades to `Config::default()` on any missing/unreadable/unparseable file (FR-101); writes go through `save_to(path)` — a tempfile-plus-rename atomic write (FR-102) serialized by a process-wide `CONFIG_LOCK` so concurrent section-writers can't clobber each other. Both are path-injected so integration tests point at a tempdir (`src-tauri/tests/settings_persist.rs`). The frontend sees the four-section `AppSettings` subset (`app`/`engine`/`updater`/`telemetry`) via `get_app_settings` / `set_app_settings` — `printers` and `schema_version` are excluded and preserved server-side. **Env-var overrides and `notify` hot-reload are not implemented** (an earlier draft described a `figment`/`PLURYPRINT_*` layering that does not ship); the file is read on demand under the lock.

### 2.3 `[printers]` — printer preferences (M1)

Pluryprint-level printer state that must survive restarts, persisted as a `[printers]` table inside `config.toml` and exposed to the UI via the `get_printer_prefs` / `set_printer_prefs` IPC commands (see [ipc-and-cli.md](./ipc-and-cli.md) and [printers-management.md](../05-features/printers-management.md)). These are **preferences only — they never mutate the OS spooler**: "removing" a printer here soft-disables it in the app while leaving the OS queue installed.

```toml
[printers]
default_printer = "EPSON_L15160"      # app default printer id; absent → use the OS default
disabled = ["Old_Fax"]                # printer ids hidden in Pluryprint (still installed in the OS)

[printers.routes]
".pdf"  = "EPSON_L15160"              # file extension → printer id
".docx" = "HP_LaserJet"

[printers.overrides.EPSON_L15160]     # per-printer default tray/paper, keyed by printer id
tray  = "Tray 1"
paper = "A4"
```

Rust binding (`src/settings/mod.rs`):

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct PrinterPrefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_printer: Option<String>,               // app default printer id
    #[serde(default)]
    pub disabled: Vec<String>,                          // soft-disabled printer ids
    #[serde(default)]
    pub routes: BTreeMap<String, String>,               // extension → printer id
    #[serde(default)]
    pub overrides: BTreeMap<String, PrinterOverride>,   // printer id → tray/paper
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct PrinterOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tray: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paper: Option<String>,
}
```

Every field defaults, so a `config.toml` with no `[printers]` table still loads. The Printers screen reconciles these ids against the live spooler on each load: a route target, override key, or default that the spooler no longer reports drives the missing-printer banner ([printers-management.md](../05-features/printers-management.md#edge-states)). Because `overrides` keys are tray/media names specific to one model, they are **dropped, not carried over**, when references are reassigned to a different printer. The effective default printer resolves as `policy.default_printer ?? printers.default_printer ?? OS default` — the org policy (§1, `policy.json`) wins when present.

## 3. `profiles.toml` — print profiles

A profile is a named bundle of print settings. The user can create, rename, clone, edit, and delete profiles. One is marked as the default (the one used when invoked from the context menu without a chooser).

**Two things are important about how profiles persist.** First, `profiles.toml` holds **only the user-created customs**; the three built-ins (`builtin-quick`, `builtin-duplex-bw`, `builtin-photo-color`) live in code (`profiles::builtins()`) and are never written to disk — they cannot be edited or deleted, only cloned. Second, **which profile is default is not stored here** — it lives in `config.toml` as `[engine].default_profile` (a profile **id**). The on-disk profile therefore carries neither a `builtin` nor a `default` flag; both are server-computed on every read and stamped onto the wire [`Profile`] the frontend sees. See [profiles-management.md](../05-features/profiles-management.md).

### 3.1 Schema

```toml
schema_version = 1

# Only user-created customs are stored; the three built-ins are in code.
[[profile]]
id           = "018f9c2e-6a10-7b3a-9c4d-2f1e0a8b7c6d"  # UUID v7, stable for life
name         = "Press A"
printer      = "<default>"             # "<default>" sentinel or printer id
color_mode   = "monochrome"            # "color" | "monochrome"
duplex       = "long-edge"             # "off" | "long-edge" | "short-edge"
copies       = 1
paper_size   = "A4"                    # ISO name or "Letter", "Legal", "A3"
orientation  = "auto"                  # "portrait" | "landscape" | "auto"
scaling      = "fit"                   # "fit" | "shrink" | "none"
quality      = "normal"                # "draft" | "normal" | "high"
tray         = "auto"                  # printer-tray name or "auto"
# page_range is OMITTED for "all pages"; present only for a real expression:

[[profile]]
id           = "018f9c2e-6a10-7b3a-9c4d-1a2b3c4d5e6f"
name         = "Color Single-sided"
printer      = "HP LaserJet Pro M404dn"
color_mode   = "color"
duplex       = "off"
copies       = 1
paper_size   = "Letter"
orientation  = "auto"
scaling      = "fit"
quality      = "high"
tray         = "auto"
page_range   = "1-5,8,11-13"           # engine grammar; absent means all pages
```

### 3.2 Rust binding

There are two shapes. `StoredProfile` is the on-disk `[[profile]]` entry; the wire `Profile` the frontend sees is `StoredProfile` plus the two server-computed flags (`builtin`, `default`).

```rust
// The on-disk entry (profiles/mod.rs). No margins/notes — those are
// deferred; no builtin/default flag — those are computed, not stored.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredProfile {
    pub id: String,                       // UUID v7 for customs
    pub name: String,
    pub printer: String,                  // "<default>" sentinel or printer id
    pub color_mode: ColorMode,
    pub duplex: ProfileDuplex,            // "off" | "long-edge" | "short-edge"
    pub copies: u16,                      // 1..=999
    pub paper_size: PaperSize,
    pub orientation: Orientation,
    pub scaling: Scaling,                 // "fit" | "shrink" | "none"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_range: Option<String>,       // None == all pages (omitted from TOML)
    pub quality: Quality,                 // "draft" | "normal" | "high"
    pub tray: String,
    #[serde(default = "default_collate")]
    pub collate: bool,                    // true == whole sets; missing reads as true
}

// The wire shape adds the computed flags (ignored on input, recomputed on read).
pub struct Profile { /* …StoredProfile fields…, */ pub builtin: bool, pub default: bool }

// The whole file: a version plus the customs. The field is named `profile`
// (singular) so TOML renders it as `[[profile]]`.
pub struct ProfilesFile { pub schema_version: u32, pub profile: Vec<StoredProfile> }
```

The `<default>` printer sentinel is a plain string preserved verbatim on round-trip (not a tagged enum), so the file stays human-readable. `page_range` is `None`/omitted for "all pages" — there is no `"all"` sentinel. `collate` is stored (missing in an older file reads as `true`); `margins_mm` and `notes` from earlier drafts are **deferred** and not persisted.

### 3.3 Profile invariants

- `id` is set on creation and never changed. Customs get a **UUID v7** (time-sortable); built-ins use fixed slugs. Renaming changes `name` only.
- `name` must be non-empty, ≤ 100 chars, and unique **case-insensitively** across built-ins and customs. On load, a hand-edited file with duplicate names keeps the first and drops the rest with a warning.
- `copies` is in `1..=999`; validated on save.
- `page_range`, when present, must parse against the engine grammar (`engine::pagerange::parse`) at save time; a blank/whitespace value collapses to `None` (all pages).
- Writes are **atomic** (tempfile + rename, FR-026) and serialized by `PROFILES_LOCK`. Reads tolerate a corrupt file by degrading to built-ins only; *mutations* refuse to touch a corrupt file so user data is never clobbered.
- The **default** is not a per-profile flag: it is `config.toml`'s `[engine].default_profile` id, resolved to `builtin-quick` when it names no known profile.

## 4. `queue.json` — live print queue

JSON, not TOML, because:

- It changes very frequently (per-job state transitions).
- It is machine-managed; not intended for hand-editing.
- It must be readable in partial form by other tools (e.g., a tray-icon companion).

The file lives at `<data_dir>/queue.json` (`paths::data_dir()`); its wire shape is the `QueueSnapshot` reused verbatim by the `queue_status` IPC command and the `queue-updated` event (see [ipc-and-cli.md](./ipc-and-cli.md)). A resolved `JobSettings` bundle is embedded in each job — there is no `profile_id`/`overrides` split (settings are resolved at submit time; jobs never reference a profile id) and no persisted OS spool id.

### 4.1 Schema

```json
{
  "schema_version": 1,
  "queue_paused": false,
  "jobs": [
    {
      "id": "018f9c2e-6a10-7b3a-9c4d-2f1e0a8b7c6d",   // UUID v7 (time-sortable)
      "batch_id": "018f9c2e-6a10-7b3a-9c4d-000000000001",
      "state": "converting",
      "input_path": "C:\\Users\\Miguel\\Pictures\\chart.png",
      "file_name": "chart.png",
      "input_ext": "png",
      "input_bytes": 491520,
      "pages": 1,
      "cache_pdf": "…\\Pluryprint\\cache\\conversions\\018f9c2e-…-7b3a.pdf",
      "settings": {
        "printer": null,
        "duplex": "none",
        "color_mode": "color",
        "copies": 1,
        "paper_size": "a4",
        "orientation": "auto",
        "scaling": "fit",
        "page_ranges": null
      },
      "profile_name": "Photo Color",
      "printer_resolved": null,
      "attempts": 0,
      "error": null,
      "progress": { "phase": "converting", "percent": 0.5 },
      "submitted_at": "2026-07-11T10:42:00Z",
      "started_at": "2026-07-11T10:42:01Z",
      "finished_at": null
    }
  ]
}
```

Field notes: `input_ext` is the lowercased extension without a dot; `pages` is best-effort (PDFs parsed with `lopdf` when ≤ 100 MB, images always `1`, else `null`); `cache_pdf` is set only for converted inputs (a PDF prints in place, so it stays `null`); `printer_resolved` records the printer actually chosen once a job reaches `printing`/`done`; `os_job_id` is the spooler's id for the job once spooled (`Office-42` on CUPS; on Windows the spooler job number plus the spooler's submission stamp, `12@20261010T101500123`, because the Windows spooler reuses job numbers after a restart and only the stamp tells a job from a later one with the same number), `null` before that or when it could not be identified, and missing in files written before it existed; `spooled_at` is when the current run was handed to the spooler (the print timeout counts from it, also after a restart); `origin` (`{ "source": "pluryfact", "label": "Recibidos · octubre 2026" }`) names the integration the job's batch came from, for the Queue row's **From PluryFact** (FR-124), and is missing for everything else and in files written before it existed; all timestamps are RFC 3339 UTC. The full TS shapes are in [ipc-and-cli.md § 2.2](./ipc-and-cli.md#22-type-definitions).

States (`state` enum, lowercase):

- `queued` — created but not started.
- `converting` — a converter is producing the cache PDF.
- `converted` — cache PDF ready, waiting for a free printer.
- `printing` — handed to the OS spooler and not yet finished there (exactly one job prints at a time). `os_job_id` carries the spooler's id while the dispatcher follows it; `progress.phase` is `printing`, `held` (the OS queue is holding it) or `stopped` (the printer needs attention).
- `done` — the spooler reported the OS job completed (terminal); the cache PDF is deleted.
- `unconfirmed` — handed to the spooler, but nothing confirmed it printed (terminal): the job left the system queue without reporting completion (deleted or purged there, or a printed Windows job removed before a poll saw it), the spooler gave no id to follow, tracking kept failing, or, after a restart, a Windows id could not be told apart from a reused one. `error` is `print_unconfirmed` with the reason (`left_queue`, `untracked`, `tracking_failed`, `not_verifiable`). Never shown as printed and never re-sent automatically; the cache PDF is deleted.
- `failed` — terminal error after retries; details in `error`.
- `paused` — user-paused, interrupted by shutdown (before the spooler accepted it), or stopped on a user-fixable error (`unsupported_setting` / `dependency_missing` / `printer_unavailable` / `printer_disabled`); resumable, **not** auto-retried.
- `cancelled` — user cancelled (terminal).

Transient conversion/print failures do not go straight to `paused`: the dispatcher retries them (up to 3 attempts, 1 s then 5 s backoff) by returning the job to `queued` (re-convert) or `converted` (re-print, cache kept), and only marks `failed` once attempts are exhausted. The full state machine (`can_transition`) and retry policy live in `src-tauri/src/queue/{job,dispatcher}.rs` and the [print engine](./print-engine.md).

### 4.2 Atomic writes

The queue file is rewritten on every state transition. To avoid corruption from a crash mid-write:

```rust
use std::io::Write;
use tempfile::NamedTempFile;

fn write_queue(path: &Path, q: &Queue) -> std::io::Result<()> {
    let dir = path.parent().unwrap();
    let mut tmp = NamedTempFile::new_in(dir)?;
    serde_json::to_writer(&mut tmp, q)?;
    tmp.flush()?;
    tmp.persist(path)?;            // atomic rename on same filesystem
    Ok(())
}
```

`tempfile::NamedTempFile::persist` is `MoveFileEx` with `MOVEFILE_REPLACE_EXISTING` on Windows and `rename` on Unix; both are atomic across the same volume.

### 4.3 Load, recovery, and history cap

On launch the core loads `queue.json` and runs shutdown recovery before serving the queue: any job left `converting` or `printing` when the app last closed is moved to `paused` with `error.code = "interrupted_by_shutdown"` and its transient progress cleared. The exception is a `printing` job the spooler had already accepted (`os_job_id` and `printer_resolved` set): it stays `printing`, and the dispatcher re-attaches the print slot to it and polls its OS job, because resuming it would print a second copy. At most one such job exists (one print slot). Such a job is surfaced as resumable rather than failed; resuming it clears the error and resets its attempt count. The recovered state is immediately re-persisted (so it survives an immediate re-crash), and orphaned cache PDFs — files in `<cache_dir>/conversions` that no live job's `cache_pdf` references — are swept.

**Corruption / forward-incompatibility.** A missing file loads as an empty queue with no side effects. An unreadable file, invalid JSON, or a `schema_version` greater than this build supports is renamed to `queue.json.bak` (best-effort), logged, and replaced with an empty queue — the file is taken whole or discarded whole, never partial-loaded.

**History cap.** On every save the terminal (done/unconfirmed/failed/cancelled) jobs are pruned to the 200 most-recent (ranked by `finished_at`, falling back to `submitted_at`); every non-terminal job is always kept, so the file cannot grow without bound.

## 5. Logs — JSONL

Logs are *structured* JSON Lines, one event per line, newline-terminated. This is friendly to `jq`, `Sentry`, log shippers, and humans (`less`).

### 5.1 Schema

Each line:

```json
{"ts":"2026-05-03T10:42:01.123Z","lvl":"info","module":"engine::router","event":"converter_selected","job":"01HX5KAB","mime":"application/pdf","converter":"passthrough"}
{"ts":"2026-05-03T10:42:02.501Z","lvl":"warn","module":"engine::converters::libreoffice","event":"timeout","job":"01HX5KCD","timeout_s":60}
{"ts":"2026-05-03T10:42:03.000Z","lvl":"info","module":"engine::printer::windows","event":"job_spooled","job":"01HX5KAB","spool_id":42,"printer":"HP LaserJet"}
```

Mandatory fields:

- `ts`  — RFC 3339 with millisecond precision, always UTC.
- `lvl` — `trace` | `debug` | `info` | `warn` | `error`.
- `module` — `tracing` target.
- `event` — short identifier for the event type (snake_case).

Other fields are event-specific. We do not log file *contents*, and absolute file **paths** stay out of every level the default filter enables (`info`, and `debug` for our own crate — see §5.3), per NFR-082: records name a file by its basename (`file`) and the job by `job_id`. The one record that carries the absolute input path, `job_input_path`, is emitted at `trace`, so it only appears for a user who raises the filter through `RUST_LOG`. There is no `logging.include_paths_in_errors` switch — `config.toml` has no `[logging]` section at all.

### 5.2 Rotation and retention

Rotation and retention are implemented by **`logging::RollingJsonl`**, not by `tracing-appender`'s rolling appender. That appender only rotates on a clock boundary and never deletes anything, so logs grew without bound; `tracing-appender` is still used, but only for its non-blocking writer thread.

`RollingJsonl` is a plain `Write`, so the JSON layer and the non-blocking writer are wired exactly as they would be otherwise. It does both halves of the requirement:

- **Roll** when the **UTC date changes or the file passes `max_bytes` (10 MB), whichever comes first.** The day's first file is `pluryprint-<date>.jsonl`; each size roll after it is `pluryprint-<date>.<n>.jsonl` (n ≥ 2). So the documented daily name is what a user finds unless a single day is unusually chatty.
- **Sweep** at startup, *before* the writer opens today's file, and again **after every roll** (the app is resident and can run for weeks, so retention cannot wait for a relaunch). The roll-time sweep runs once the new part is open, so the file about to be appended to is always the newest and is never dropped. It deletes anything older than `retain_days` (**14**) and then keeps only the newest `retain_files` (**7**). **Both** bounds apply; whichever bites first wins.

Dates are **UTC** for both file names and the retention cutoff, so a machine crossing a timezone or a DST boundary never re-opens yesterday's file.

These are the spec's numbers (NFR-081, FR-113), not tunables: there is no `[logging]` section in `config.toml`. They live in a `Policy` struct rather than three constants so tests can shrink them (a 10 MB rotation test would be absurd) and so wiring them to settings later is a one-liner.

### 5.3 Tracing setup

```rust
// src-tauri/src/logging/mod.rs (shape)
pub fn init() -> io::Result<()> { init_with(true) }      // GUI
pub fn init_cli() -> io::Result<()> { init_with(false) } // headless CLI: no stdout layer

fn init_with(stdout: bool) -> io::Result<()> {
    let log_dir = paths::log_dir();
    paths::ensure_dir(&log_dir)?;

    let policy = Policy::default();          // 10 MB / 14 days / 7 files
    let swept = sweep(&log_dir, today_utc(), &policy);   // before opening today's file

    let writer = RollingJsonl::new(log_dir, policy);
    let (non_blocking, guard) = tracing_appender::non_blocking(writer);
    let _ = GUARD.set(Mutex::new(Some(guard)));  // dropped by `flush()` (CLI exit)

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,pluryprint=debug"));

    let console = stdout.then(|| fmt::layer().compact().with_target(false));
    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer().json().with_writer(non_blocking))  // JSONL to disk
        .with(console)                                        // GUI only: human-readable to stdout
        .try_init()
        .map_err(io::Error::other)?;
    // ...
}
```

Consequences worth knowing: the default filter is **`info,pluryprint=debug`** (overridable with `RUST_LOG`), so our own crate logs at debug — including paths — out of the box; the GUI adds a `compact` layer on stdout for `pnpm tauri dev`. A **headless CLI run logs too**, to the same file with the same schema (`cli_invoked` / `cli_finished` / `cli_failed` records, plus whatever the shared cores log), but with **no stdout layer**: stdout is the CLI's output channel and `--json` callers parse it. `cli::execute` calls `logging::flush()` before returning because the process then ends in `std::process::exit`, which runs no destructors.

### 5.4 Panics and frontend errors

Release builds use `panic = "abort"`, and the Windows GUI binary has no console, so a panic would otherwise end the process without a trace. `logging::install_panic_hook` (installed in `run_with` right after `logging::init`) writes each panic **synchronously** as one record in the same JSON shape — `level: "ERROR"`, `target: "pluryprint::panic"`, `fields.event: "panic"`, plus `location`, `payload` and `thread` — to the newest file for the day. It bypasses the non-blocking writer on purpose: the abort that follows would discard that writer's buffer.

Errors raised in the webview reach the same file through the `log_frontend_error` command (`event: "frontend_error"`): the React error boundary, `window` `error`/`unhandledrejection`, and the listener failures that used to go only to the devtools console.

## 6. Schema migration policy

Every persisted file carries a `schema_version: u32` at the top.

### 6.1 Forward compatibility

When Pluryprint sees a file with `schema_version` *higher* than it knows about, it:

1. Logs a warning.
2. Refuses to write to that file (would otherwise downgrade it).
3. Continues to function with in-memory defaults where possible (e.g., for `config.toml` it falls back to `Config::default()`).

Specifically, profiles and queue with future versions block the relevant feature with a clear "this file was written by a newer Pluryprint" error rather than silently dropping fields.

### 6.2 Backward migrations

When we read a file with an *older* version, we run a chain of migration functions:

```rust
// src/util/migrate.rs
pub fn migrate_config(value: &mut toml::Value) -> Result<()> {
    let mut v = value["schema_version"].as_integer().unwrap_or(1) as u32;
    while v < CURRENT_CONFIG_VERSION {
        match v {
            1 => { migrate_config_1_to_2(value)?; v = 2; }
            2 => { migrate_config_2_to_3(value)?; v = 3; }
            _ => break,
        }
    }
    value["schema_version"] = toml::Value::Integer(v as i64);
    Ok(())
}
```

Each `migrate_X_to_Y` is small, pure, and unit-tested with golden files.

### 6.3 Pre-migration backup

Before running migrations, the original file is copied to `<filename>.v<old>.bak`. If the migration succeeds the backup stays for one major version cycle, then is GC'd.

## 7. Concurrency rules

- **Config and profiles:** read on demand (each command loads the file), not watched — `notify` hot-reload is deferred. Writes are atomic (tempfile + rename) and serialized by a process-wide mutex per file: `CONFIG_LOCK` for `config.toml`, `PROFILES_LOCK` for `profiles.toml`. When a command needs both (e.g. `set_default_profile`), the lock order is always `PROFILES_LOCK` → `CONFIG_LOCK` so they can't deadlock.
- **Queue:** owned by the dispatcher task; persisted on every transition (atomic write). The UI reads via `invoke('queue_status')` which returns a serializable snapshot, not a borrow.
- **Logs:** `tracing-appender` `non_blocking` queues writes off the hot path.

## 8. Testing the data model

- **Round-trip tests.** For each schema, `serde` round-trip from a golden TOML/JSON file and assert byte-identical output (with stable formatting).
- **Migration tests.** For each old-version golden file, run the migration chain and assert the result equals the corresponding current-version golden.
- **Forward-version tests.** Load a file with `schema_version = u32::MAX` and assert the loader returns `Err(SchemaTooNew)`.
- **Crash-resistance tests.** Simulate process kill mid-write of `queue.json` and verify the on-disk file is either the old or new value, never partial.

## 9. Related documents

- [`system-architecture.md`](./system-architecture.md) — how these files fit into the running system.
- [`ipc-and-cli.md`](./ipc-and-cli.md) — IPC commands that read/write profiles and queue.
- [`security-model.md`](./security-model.md) — file permissions and what we never log.
