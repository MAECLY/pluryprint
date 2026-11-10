# About

> **TL;DR** — The **About** screen: app identity and version (authoritative from Rust), an on-demand **Check for updates** that reads the update manifest of the user's channel and reports whether a newer one exists, with **Install now** (signature-verified, per-user), **Remind me later** and **Skip this version**, a Components card with the real Pluryprint / React / Tauri versions and the running OS · arch, the live filesystem paths with an **Open logs folder** button, and a row of allow-listed external links (release notes, license, docs, report an issue, repository).

This document is the source of truth for the **content and behavior** of the About screen (`src/screens/AboutScreen.tsx`). The update-check logic lives in `src-tauri/src/update/mod.rs`; the diagnostic/link commands in `src-tauri/src/ipc/diag.rs`. It satisfies FR-110 (an About surface showing version, a license link, and a documentation link).

## Screen layout

A single scrolling column: an identity header, the update-check action and its result card, then Components, Filesystem paths, and Links.

```
┌─ About ──────────────────────────────────────────────────────────────────────┐
│  ▣  Pluryprint                                                                │
│     0.1.0 · build 2026-07-12 · stable channel                                 │
│     Open source batch printing for Windows, Mac and Linux. No 15-file limit…  │
│                                                                                │
│  [ ⟳ Check for updates ]                                                       │
│  ┌ Updates ───────────────────────────────────────────────────────────────┐  │
│  │ ✓ You're up to date        version 0.1.0 · checked 14:22                │  │
│  └─────────────────────────────────────────────────────────────────────────┘  │
│  COMPONENTS                                                                     │
│  │ Pluryprint   0.1.0     AGPL-3.0         │                                  │
│  │ React         18.x      MIT              │                                  │
│  │ Tauri         2.x       MIT/Apache-2.0   │  Platform: macos · aarch64       │
│  FILE LOCATIONS                                                                 │
│  │ config: …  data: …  logs: … │            [ 🗀 Open logs folder ]            │
│  LINKS   [Release notes] [License AGPL-3.0] [Third-party notices]             │
│          [Contributor agreement (CLA)] [Documentation] [Report an issue]       │
└───────────────────────────────────────────────────────────────────────────────┘
```

- **Identity header** — the app icon, `Pluryprint`, and a version line `{version} · build {build_date} · {channel} channel` (channel from the settings store). Outside the Tauri host (or until `app_info` answers) there is no version to show, so the line reads `— · standalone web preview` (`UNKNOWN_VERSION`); no frontend version literal exists to go stale.
- **Update check** — the primary **Check for updates** button (a spinner while checking), followed by a result card once a check runs.
- **Components / File locations / Links** — described below. User-facing copy names no implementation language or milestone: the paths section is titled *File locations*.

## App info (live from Rust)

`app_info()` returns the authoritative identity, so version/paths reflect the real build, not a hardcoded frontend value:

| Field                          | Source                                                    |
| ------------------------------ | --------------------------------------------------------- |
| `version`                      | `CARGO_PKG_VERSION`                                        |
| `build_date`                   | `BUILD_DATE` (stamped at build)                            |
| `config_dir` / `data_dir` / `log_dir` | `paths::*` (per-OS — see [data model § 1](../04-architecture/data-model.md#1-file-locations)) |
| `os` / `arch`                  | `std::env::consts::OS` / `ARCH` (e.g. `macos` / `aarch64`) |

If the call fails (the expected case in the standalone Vite dev server, which has no Tauri host), the screen shows an explanatory `IPC unavailable` card rather than breaking.

## Update check

**Check for updates** runs `check_for_updates`, an explicit state machine — `idle` (no card) → `checking` (spinner) → `done` (result card) or `error` (message + Retry). The button is disabled outside the Tauri host.

`check_for_updates` is the **check**: it reads the updater manifest (`latest.json`) of the user's channel from GitHub Releases via `ureq` (10-second timeout) — the same manifest **Install now** uses — and folds its `version` against the running version with a semver compare into an `UpdateCheck { current, latest, available, url, notes, checked_at }`. Nothing is downloaded by a check.

| Result                                   | Card shown                                                                                  |
| ---------------------------------------- | ------------------------------------------------------------------------------------------- |
| Newer release (`available: true`)        | Accent "Version {latest} is available", noting that the download is signature-checked and needs no admin rights, with **Install now**, **Release notes** (opens `url`), **Remind me later** and **Skip this version**. |
| Up to date (`available: false`, a release found) | Success "You're up to date — version {current} · checked {HH:MM}".                    |
| No releases published on the channel (manifest `404`, also while the repository is private) | `UpdateCheckFailed` / `channel_unavailable`: a neutral status card, "No updates published on the {channel} channel", with the release page and Retry. Not the up-to-date card and not the error card. |
| Network / rate-limit / offline           | Error card with the short user-facing message and a **Retry** button.                       |

The result card is one state of a single machine (the `update` state in `AboutScreen.tsx`): `idle` → `checking` → `done` / `error`, plus `deferred`, `skipped`, `installing`, `installed` and `installError`. Each replaces the card in place inside an `aria-live="polite"` region, so the change is announced (NFR-044).

- **Remind me later** (`deferred`, FR-084) — session-scoped on purpose: the card is replaced by the line *Reminder postponed. Check again whenever you like.* Nothing is persisted, so the next launch-time check (or a manual one) offers the release again.
- **Skip this version** (`skipped`, FR-084) — persisted; see below. Offered only when the check named a version.

### Install now (FR-082, FR-083, FR-084, FR-087)

**Install now** calls `install_update`, which downloads the artifact that `latest.json` names for this platform, verifies its minisign signature against the key in `tauri.conf.json`, and installs it per-user ([ADR-0008](../adr/0008-installer-nsis-per-user.md)). The card shows **Downloading and verifying {version}…** with a percentage from the `update-download-progress` event, then one of:

| Outcome | Card |
| --- | --- |
| Installed (macOS) | "{version} is installed — Restart Pluryprint to start using it." with **Restart now** (`restart_app`). |
| Installed (Windows) | Nothing: the app exits into the per-user installer, which restarts it. |
| Bad signature (`UpdateSignatureInvalid`) | "The downloaded update failed its signature check and was not installed. Nothing on this computer was changed…", with **Open logs folder** and **Release notes**. |
| Nothing installable (`installed: null`) or any other failure | The reason, with the same two buttons. |

**FR-087:** when `latest` is a major jump from `current` (`1.x` → `2.x`), Install now first opens a `ConfirmDialog` ("Install a major update?"). Only confirming it passes `allowMajor: true` together with the confirmed version, and the backend refuses a major install without it; if the manifest has meanwhile moved to a *different* major version, it refuses with `major_update_changed` instead of installing on a confirmation that was not for it.

**Skip this version** persists the tag (`set_skipped_update_version`) in `update-state.json` and shows *Skipping {version}. A newer release will still be offered.* with **Undo**. The screen opens on that line on every later visit (`get_update_state` on mount), and the launch-time check stays silent about that release; a *newer* one prompts again (version-aware compare, not string compare). **Undo** clears the skip (`set_skipped_update_version(null)`) and re-runs the check. `get_update_state` drops a skip that is at or below the running version, so the line goes away once the user has moved past it.

Rate-limit (`403`/`429`) and transport failures map to friendly strings (`GitHub rate limit reached — try again later`, `could not reach github.com — check your connection`); an unparseable version on either side never spuriously prompts an update. `url` and `notes` are populated only when an update is actually available.

## Components

A static-plus-live card. Pluryprint's version comes from `app_info`; React from its runtime `version` export; Tauri from `@tauri-apps/api/package.json` (the resolved dependency, so it stays honest as the lockfile bumps). A footer row shows `Platform: {os} · {arch}` from `app_info`.

| Component     | Value source                     | License         |
| ------------- | -------------------------------- | --------------- |
| Pluryprint   | `app_info().version`             | AGPL-3.0        |
| React         | `react`'s `version` export       | MIT             |
| Tauri         | `@tauri-apps/api` package version | MIT/Apache-2.0 |

## Filesystem paths & logs

An `info`-gated card lists the live **config**, **data**, and **logs** directories from `app_info`. An **Open logs folder** button calls `open_logs_folder`, which creates the log directory first (so it never no-ops on a fresh install) and reveals it in the OS file browser. The button is disabled outside the Tauri host.

## Links

A row of buttons, each opening an allow-listed external URL through `open_external` (or, for issues, `report_issue`). All are disabled outside the Tauri host.

| Link                | Command / target                                                     |
| ------------------- | -------------------------------------------------------------------- |
| **Release notes**   | `open_external(RELEASES_URL)` — `…/releases`                         |
| **License AGPL-3.0** | `open_external(LICENSE_URL)` — `…/blob/main/LICENSE`                |
| **Third-party notices** | `open_external(NOTICES_URL)` — `…/blob/main/THIRD-PARTY-NOTICES.md` (the same file ships in the bundle) |
| **Contributor agreement (CLA)** | `open_external(CLA_URL)` — `…/blob/main/CLA.md`        |
| **Documentation**   | `open_external(DOCS_URL)` — `…/tree/main/docs`                       |
| **Report an issue** | `report_issue("")` — opens a prefilled GitHub new-issue page         |
| repository (footer) | `open_external(REPO_URL)` — `github.com/MAECLY/pluryprint`         |

`report_issue` seeds the issue body with the app version, build date, and OS/arch (percent-encoded so newlines and `#` survive), followed by the user's text. `open_external` is restricted to an **HTTPS allow-list**: exactly `github.com`, `maecly.com`, or any `*.maecly.com` subdomain — a bare host, no `userinfo@` and no `:port`, so `https://github.com@evil.example` or `https://github.com.evil.com` are rejected. Any other URL returns `InvalidRequest` rather than opening. All the URLs above live under `github.com/MAECLY/pluryprint` (`src/lib/repo.ts`), which the allow-list permits. `REPO_URL` must match the crate's `repository` in `src-tauri/Cargo.toml` (what Rust reads for the update check and the issue URL); `AboutScreen.test.tsx` fails if they drift apart. The footer under the links reads *Pluryprint is free software licensed under GPL-3.0. No telemetry, no cloud, no account. Source at* followed by the repository link.

## Per-platform notes

The screen is platform-neutral. The three shell-outs (`open_logs_folder`, `open_external`, `report_issue`) go through the `opener` plugin on the Rust side, so no JS `opener` capability is needed. Only the reported values differ per OS: the filesystem paths (Windows `%APPDATA%`/`%LOCALAPPDATA%` vs macOS `~/Library/…`) and the `os`/`arch` line. The update check is an OS-neutral network call.

## Edge & empty states

- **No Tauri host (standalone preview)** — every backend-backed control is disabled; the version line falls back to `— · standalone web preview`; an `IPC unavailable` card explains that `pnpm tauri dev` is needed for the real bridge.
- **Update check before it runs** — no card at all (`idle`); the card appears only after a check.
- **A release with no notes** — the update-available card still shows; `notes` is simply absent (`body` tolerated as missing/empty upstream).

## IPC mapping

| UI surface              | IPC command                          | Notes                                                        |
| ----------------------- | ------------------------------------ | ------------------------------------------------------------ |
| Identity + paths        | `app_info() => AppInfo`              | Version, build date, config/data/log dirs, os, arch.          |
| Check for updates       | `check_for_updates() => UpdateCheck` | Reads the channel's `latest.json`; 404 = `channel_unavailable`. |
| Install now / Restart   | `install_update(allowMajor, confirmedVersion) => UpdateInstallResult`, `restart_app()` | Signed download + per-user install; `update-download-progress` events. |
| Skip / Undo             | `get_update_state()`, `set_skipped_update_version(v)` | Persisted in `update-state.json`; stale skips are pruned. |
| Open logs folder        | `open_logs_folder() => void`         | Creates the dir, reveals it in the OS file browser.           |
| Release notes / License / Docs / repo | `open_external(url) => void`  | HTTPS allow-list (github.com / maecly.com / *.maecly.com).    |
| Report an issue         | `report_issue(text) => void`         | Prefilled GitHub issue with the environment block.            |

See [ipc-and-cli.md § diagnostics/update](../04-architecture/ipc-and-cli.md) for the request/response types.

## Implementation status

- **M0** — front-end only; version and paths were placeholders (superseded).
- **M1 (current)** — **the About surface is real.** Rust: `src-tauri/src/ipc/diag.rs` (`app_info`, `open_logs_folder`, `report_issue`, `open_external`) + `src-tauri/src/update/mod.rs` / `src-tauri/src/ipc/update.rs` (`check_for_updates`, `install_update`, `get_update_state`, `set_skipped_update_version`, `restart_app`). Frontend: `AboutScreen.tsx`, `src/lib/repo.ts`. The update logic is almost entirely pure functions (`repo_slug` / `parse_release` / `is_newer` / `evaluate`) with co-located tests; only `fetch_latest` touches the network. Frontend coverage: `src/screens/AboutScreen.test.tsx` (FR-110).
- **Follow-ups**
  - **Release pipeline** — install works end to end against a signed `latest.json`, but no workflow publishes one yet.
  - **Channel feeds (beta/nightly)** — the configured channel is read but every check hits `releases/latest`; per-channel feeds land with the updater.
  - **Converter tooling versions** — not listed in the Components card; add them there if the converters are ever bundled.

## Open questions for v1+

- Should the update card also render the release notes markdown inline, or keep linking out to the release page?
- Should a background startup check (Settings → *Check on startup*: 30 s after launch, at most once per 24 h) surface its result here too? Today it raises a toast only, and About finds the same release again on demand; the launch-time result itself is not kept.

These are tracked as design discussions, not commitments.
