# Update Check and Install

> **TL;DR** — Two steps, one manifest. The **check** reads the updater manifest (`latest.json`) of the user's channel from GitHub Releases — stable: `releases/latest/download/latest.json`; beta and nightly: the `channel-beta` release's copy — and compares its `version` to the running one with semver precedence. The **install** runs only when the user asks for it (About → **Install now**, or `pluryprint update install`): `tauri-plugin-updater` downloads the artifact named by that same manifest, verifies its minisign signature against the public key in `tauri.conf.json`, and installs it per-user (§10, [ADR-0008](../adr/0008-installer-nsis-per-user.md), [ADR-0011](../adr/0011-release-channel.md)). The GitHub REST API and `updates.maecly.com` are not contacted.

## 0. Status and scope

| Capability                                | Shipped today | Where |
| ----------------------------------------- | ------------- | ----- |
| Check whether a newer release exists      | ✅            | `src-tauri/src/update/mod.rs` |
| Launch-time check (delayed, throttled)    | ✅            | `src-tauri/src/lib.rs` `setup` |
| Manual check from the About screen        | ✅            | `check_for_updates` IPC command |
| Manual check from a script                | ✅            | `pluryprint update check [--json]` |
| "Skip this version"                       | ✅            | `set_skipped_update_version`, FR-084 |
| Download an update                        | ✅            | `update::install::find_update` / `install`, on explicit request only |
| Install / replace the running binary      | ✅            | `install_update` IPC command, `pluryprint update install [--yes]` |
| Signature verification (minisign/Ed25519) | ✅            | `tauri-plugin-updater`, against `plugins.updater.pubkey` |
| Major version needs explicit opt-in (FR-087) | ✅         | `update::install::is_major_bump` + `allowMajor` / `--yes` |
| Stable and beta feeds (FR-085)            | ✅            | `update::install::default_endpoint`; nightly follows beta (no nightly builds) |
| A release pipeline that publishes signed artifacts | ✅ (not yet run) | `.github/workflows/release.yml`, `release-channels.yml`; see [releasing.md](../07-build-and-release/releasing.md) |

§1–§7 describe the check. §10 describes the install.

## 1. One source of truth

The check answers the only question a user actually asks — "am I running the current version?" — from the **same file the installer uses**. Reading the updater manifest rather than the GitHub REST API means the version the About screen announces is the version `update install` installs, the beta channel works for both, there is no anonymous 60-requests-an-hour limit, and nothing depends on `api.github.com`.

Everything except the one network call is a pure function with co-located tests (`parse_manifest` / `interpret_manifest_response` / `is_newer` / `evaluate` / `due_for_check` / `should_check_on_startup` / `should_notify`). `fetch_manifest` is the only part that touches the network and is kept tiny so the rest is reasoned about — and tested — entirely offline.

## 2. The request

```
GET https://github.com/MAECLY/pluryprint/releases/latest/download/latest.json          # stable
GET https://github.com/MAECLY/pluryprint/releases/download/channel-beta/latest.json    # beta, nightly
User-Agent: Pluryprint/<version>
Accept: application/json
```

- The channel comes from `updater.channel` (`update::install::default_endpoint`). `PLURYPRINT_UPDATE_ENDPOINT` replaces it for local end-to-end testing (release builds still require `https`), and then drives the check, the launch-time check and the install alike (`fetch_configured_latest`).
- The client is **`ureq`** 3 (blocking, rustls with the bundled webpki roots, a 10-second budget for the whole request: DNS, connect, TLS, redirects and body). There is no `reqwest` and no async runtime in the check. HTTP error statuses come back as responses (`http_status_as_error(false)`), so every status reaches `interpret_manifest_response`; only a failure to reach the server is `offline`.
- A system proxy is honoured from the standard environment variables — the first of `HTTPS_PROXY` / `https_proxy` / `HTTP_PROXY` / `http_proxy` whose value parses as an HTTP `CONNECT` proxy (`update::proxy_from`). The agent's proxy is always set explicitly, so ureq's own environment lookup (which would also read `ALL_PROXY` and `NO_PROXY`) does not apply. Only HTTP `CONNECT` proxies are supported (the `socks-proxy` feature is off), so a SOCKS value is skipped like an empty or unparseable one; with no usable proxy the connection is direct.
- **No request body, no identifiers, no telemetry.** GitHub sees a plain unauthenticated GET.

### 2.1 Status → outcome

`interpret_manifest_response(status, body, page)` owns the whole mapping, so the decision table is unit-tested without a network:

| Status | Outcome |
| ------ | ------- |
| `200` + a manifest with a `version` | `Ok(Some(ReleaseInfo))` |
| `200` + unparseable body or no version | **Error** (`bad_response`). Reporting "you're up to date" off an unreadable payload would be a lie. |
| `404`                     | `UpdateCheckFailed` with reason `channel_unavailable` — nothing published on this channel yet, its pointer release is missing, or the repository is private. Never "up to date". About shows it as a neutral notice; the launch-time check logs it, stays silent and counts it for the 24 h throttle. |
| `403` / `429`             | Error (`rate_limited`): "the update server is limiting requests — try again later" |
| anything else             | Error (`bad_status`, with the status) |
| transport failure         | Error (`offline`): "could not reach the update server — check your connection" |

### 2.2 Parsed payload

Only two fields are read from the manifest:

| Field     | Required | Used for |
| --------- | -------- | -------- |
| `version` | yes      | The version compared against the running one |
| `notes`   | no       | Release notes shown in the prompt |

The link shown with an available update is the release page built from the version (`release_page`: `…/releases/tag/vX.Y.Z`), the tag `release.yml` creates.

## 3. Version comparison

`is_newer(current, tag)` strips a single leading `v`/`V` from both sides and compares with **`semver::Version::cmp_precedence`**, not `Ord`. This matters: `Ord` totally-orders versions and therefore ranks `0.0.1+build` *above* `0.0.1`, which would flag a build-metadata-only "release" as an update. SemVer §10 excludes build metadata from precedence, and `cmp_precedence` implements exactly that.

An unparseable version on either side yields `false` and logs a warning, so a weird tag never spuriously prompts.

## 4. The launch-time check (FR-080)

The check lives in `run()`'s Tauri `setup`, not in a frontend effect, for three reasons: the single-instance plugin guarantees `setup` runs exactly once per process, so a second launch cannot double-check; the webview can be reloaded or its window closed and reopened, neither of which should re-run a network call; and the preference, the throttle and the skip rule already live in Rust next to the state file.

```
T = 0        App launches; window mounts. Nothing network happens.
T = 30 s     STARTUP_CHECK_DELAY elapses on a detached thread.
             run_startup_check():
               - updater.check_on_startup off?          -> None (no request)
               - last check < MIN_CHECK_INTERVAL_HOURS? -> None (no request)
               - fetch fails / rate-limited / offline?  -> None
               - not strictly newer?                    -> None (state stamped)
               - tag == skipped_version?                -> None (state stamped)
               - otherwise                              -> Some(UpdateCheck)
T = ~30 s+   Some(_) is emitted as the `update-available` event.
```

The whole thread — the sleep included — is detached, so a cold start never waits on the network and a user who quits before the delay elapses simply takes the thread down with the process, having persisted nothing.

`STARTUP_CHECK_DELAY` is 30 s and `MIN_CHECK_INTERVAL_HOURS` is 24 (`update/mod.rs`). Because every uninteresting outcome collapses to `None`, the frontend never has to filter the event — receiving `update-available` *is* the decision to tell the user.

### 4.1 Persisted state

`update-state.json` lives in the per-user **data** directory next to `queue.json` — it is live bookkeeping, not configuration, so it is deliberately *not* part of `config.toml`: a corrupt or deleted file must degrade to "check again", never to a settings-parse refusal.

```json
{
  "last_check_at": "2026-07-20T09:12:44Z",
  "skipped_version": "v1.3.0"
}
```

| Field             | Meaning |
| ----------------- | ------- |
| `last_check_at`   | RFC 3339 stamp of the last **completed** check, manual or automatic |
| `skipped_version` | The tag the user chose to skip (FR-084) |

Writes are atomic (temp file in the same directory, then rename). An unreadable file is logged and ignored; the worst case is one extra check.

**Throttle edge cases** (`due_for_check`): never having checked is due; an unparseable timestamp is due (a hand-edited or older-build file must not wedge updates forever); a timestamp in the *future* is due (the clock moved backwards, or the file was copied between machines). In all three, refusing to check would be the worse failure.

The state is stamped whenever a check **completed**, including one that found nothing — the throttle counts checks, not findings, or an up-to-date app would re-check on every launch. A manual check from the About screen stamps it too, so pressing the button postpones the next automatic check rather than adding to it.

### 4.2 Skipping a version (FR-084)

`should_notify` silences the launch-time prompt while `skipped_version` names the same release as `latest`. Skipping is **version-scoped**: skipping `1.2.0` silences that release only — `1.3.0` prompts again. Comparison ignores a leading `v` and build metadata, falling back to a case-insensitive string compare for non-semver tags. A *manual* check does not consult this: the user asked, so they get the answer either way.

## 5. Surfaces

| Surface | How |
| ------- | --- |
| React (About screen) | `commands.checkForUpdates()`, `commands.getUpdateState()`, `commands.setSkippedUpdateVersion(v)`; passive notification via the `update-available` event |
| CLI | `pluryprint update check [--json]` — the same `evaluate()` over the same manifest |
| CLI | `pluryprint update channel [stable\|beta\|nightly]` — reads/writes `updater.channel` in `config.toml` |
| React (About screen) | `commands.installUpdate(allowMajor)`, `commands.restartApp()`; progress via the `update-download-progress` event |
| CLI | `pluryprint update install [--yes]` — the same install core; exit 11 on failure, 12 when the prompt is declined; on Windows refused (exit 1) while the app is open, and the result printed at installer hand-off (see [ipc-and-cli.md](./ipc-and-cli.md#update-install)) |

The wire shape is `UpdateCheck` (`current`, `latest`, `available`, `url`, `notes`, `checked_at`); `url` and `notes` are populated **only** when `available` is true. See [ipc-and-cli.md](./ipc-and-cli.md) §2.2.

## 6. Channels

```toml
[updater]
channel = "stable"        # "stable" | "beta" | "nightly"
check_on_startup = true
```

Those are the only two `[updater]` keys `settings::UpdaterCfg` defines. There is no `startup_delay_seconds` and no `include_prerelease`; the delay is the `STARTUP_CHECK_DELAY` constant.

**The channel picks the manifest** (FR-085, [ADR-0011](../adr/0011-release-channel.md)):

| Channel | Manifest | Receives |
| --- | --- | --- |
| `stable` (default) | `releases/latest/download/latest.json` | Final releases only |
| `beta` | the `latest.json` asset of the fixed `channel-beta` prerelease | Every prerelease **and** every stable release: `release-channels.yml` moves that manifest forward on every publish, so a beta user is offered the stable release of the same version (FR-086) |
| `nightly` | same as beta | No nightly builds are published |

Switching back to stable never offers an older version (semver precedence decides, §3). Switching to a lower channel in Settings asks first (FR-086): the dialog says nothing is downgraded and the next update comes once that channel publishes a newer version. The Settings note says what each channel receives.

## 7. Failure modes

| Failure | Handling |
| ------- | -------- |
| DNS / connection failure | `offline`. Automatic check: silent (`None`). Manual check: shown as `UpdateCheckFailed`; CLI exits 10. |
| `403` / `429` | `rate_limited`. Same silent/loud split. |
| `200` with an unreadable body | Error, never "up to date". |
| `404` on the manifest | `channel_unavailable`: nothing published on this channel (or the repository is still private). A neutral notice in About, a logged line at launch. |
| Unparseable running version or tag | Logged; treated as "not newer". |
| Corrupt `update-state.json` | Logged and ignored; degrades to "check again". |

There is no backoff ladder: the 24 h throttle *is* the rate limiter for the automatic path, and the manual path is user-initiated.

## 8. Relation to ADR-0004

*Resolved (2026-10-09):* the plugin path from ADR [`0004-auto-updater-tauri-builtin`](../adr/0004-auto-updater-tauri-builtin.md) is implemented, with two differences recorded in [ADR-0008](../adr/0008-installer-nsis-per-user.md). The manifest is a GitHub release asset rather than `updates.maecly.com`, and nothing installs without an explicit user action.

## 10. Installing an update

`src-tauri/src/update/install.rs` holds the policy, as pure functions with tests. The plugin does the download, signature check and platform install.

1. **Find.** `find_update` builds an updater for the endpoint from `resolve_endpoint` (release builds refuse plain `http`). The endpoint is the channel's manifest (§6), the same one the check reads, or `PLURYPRINT_UPDATE_ENDPOINT` when it is set. A manifest that cannot be downloaded is `download_failed`; one that offers nothing for this platform is `no_platform_update` (`classify_error`). No newer version gives `installed: null`.
2. **Gate.** `install` refuses a major jump (`is_major_bump`) unless `allow_major` is set, which only the About confirmation dialog or the CLI's `--yes` / typed "yes" provide (FR-087). The confirmation is bound to its version (`check_confirmation`): the About screen asks about the release the check named and passes that version along, and `install` reads the manifest again, so a major jump to any *other* version (a newer major published in between) is refused with `major_update_changed` until the user confirms it. A typed "yes" in the CLI pins the version the prompt named; `--yes` is a blanket opt-in. Minor and patch updates are not compared. It refuses a binary that is not inside an installed `.app` bundle on macOS (`check_installable_location`), so a dev build never replaces its build directory. One install runs at a time per process.
3. **Download and verify.** `Update::download` streams the artifact (progress goes out as `update-download-progress`) and verifies the minisign signature against `plugins.updater.pubkey` before returning any bytes. Every signature failure maps to `IpcError::UpdateSignatureInvalid`, whose message says the update was **not** installed and nothing changed (`classify_error`).
4. **Install.** Windows runs the per-user NSIS setup passively, and the app exits so the installer can restart it. macOS swaps the `.app` in place and returns `restart_required: true`, and About offers **Restart now** (`restart_app`). Linux (a preview platform, [ADR-0015](../adr/0015-linux-support.md)) also returns `restart_required: true`. The plugin looks for the manifest key of the running package first (`linux-x86_64-deb` / `linux-aarch64-deb` for a `.deb` install) and then the plain `linux-x86_64` / `linux-aarch64` key, which names the AppImage. An AppImage replaces its own file in place, with no administrator rights; a `.deb` install downloads the `.deb` and runs `pkexec dpkg -i` on it, so the system asks for an administrator password. The release workflow requires all four Linux keys in every manifest, because a `.deb` install that found only the AppImage key would download the AppImage and fail to install it.

The CLI runs the same steps through `headless_updater_app()`, a windowless Tauri app with only the updater plugin, because the plugin needs an `AppHandle`. Each step is logged (`update_install_started`, `update_download_rejected`, `update_installed`).

Signing, the `latest.json` format and the CI setup are in [ci-cd.md § Updater signing](../07-build-and-release/ci-cd.md#updater-signing).

## 9. Related documents

- [`security-model.md`](./security-model.md) — network egress audit and signing context.
- [`ipc-and-cli.md`](./ipc-and-cli.md) — `UpdateCheck` / `UpdateState` wire types, the `update-available` event, and the `update` subcommand.
- [`../09-legal/privacy.md`](../09-legal/privacy.md) — what this one network call means for privacy.
- ADR [`0004-auto-updater-tauri-builtin`](../adr/0004-auto-updater-tauri-builtin.md) — the superseded-in-practice decision (see §8).
- ADR [`0011-release-channel`](../adr/0011-release-channel.md) — the channels and the manifest per channel.
