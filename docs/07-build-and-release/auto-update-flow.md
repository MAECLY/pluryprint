# Auto-Update Flow

> **TL;DR.** Pluryprint uses Tauri's built-in updater backed by a static JSON manifest per channel on this repository's GitHub Releases (see *As built* below; the `updates.maecly.com` design further down was not adopted, [ADR-0011](../adr/0011-release-channel.md)). The client checks every 24 hours, downloads signed binaries from the manifest URL, verifies the signature against an embedded public key, replaces the binary, and restarts. Channels (stable, beta, nightly) are switched in Settings.

This document covers the end-to-end auto-update flow: build, sign, host, manifest schema, client behavior, UX, rollback, and channel switching. It is the operational runbook for delivering updates.

## As built (2026-10-10)

Where this runbook and the code differ, the code wins. What actually ships ([ADR-0008](../adr/0008-installer-nsis-per-user.md)):

- **Two steps, both explicit.** The launch-time check (30 s after start, at most once per 24 h, `updater.check_on_startup`) and About's **Check for updates** read the update manifest of the user's channel, the same document the installer reads, to see whether something newer exists. The GitHub REST API is not used. Nothing is downloaded until the user picks **Install now** in About, or runs `pluryprint update install`. There is no silent or background install.
- **Manifests (FR-085).** Stable: `https://github.com/MAECLY/pluryprint/releases/latest/download/latest.json` (the newest published non-prerelease). Beta, and nightly, which has no builds: `https://github.com/MAECLY/pluryprint/releases/download/channel-beta/latest.json`, which `release-channels.yml` moves forward whenever a release is published, so beta users also get the stable release of the same version and never move back (FR-086). `PLURYPRINT_UPDATE_ENDPOINT` overrides either for local testing. A manifest that answers 404 (nothing published on that channel yet, a missing `channel-beta` pointer, or a private repository, as now) is its own outcome, `channel_unavailable`: About shows a neutral notice naming the channel, the CLI's `update check` fails with that reason, and the launch-time check logs it and stays silent. It is never reported as "up to date".
- **Verify, then install.** The artifact's minisign signature is checked against `plugins.updater.pubkey` before anything is written. A failure is reported as `UpdateSignatureInvalid` ("failed its signature check and was NOT installed"). Windows runs the per-user NSIS setup passively, and the app exits and is restarted by the installer. macOS swaps the `.app` in place and offers **Restart now**. A binary outside an `.app` bundle refuses to update itself.
- **FR-087.** A major version jump needs a second confirmation: a dialog in About, or `--yes` / a typed "yes" to a prompt that names the major update on the CLI.
- **Signing.** `TAURI_SIGNING_PRIVATE_KEY`, in one step of the release workflow's `sign` job, which builds nothing; the builds sign with a throwaway key whose signatures are discarded. See [ci-cd.md § Updater signing](./ci-cd.md#updater-signing). Each signature is bound to the release's version (`tauri signer sign --app-version`), and the app's updater has `requireSignedVersion` on: an artifact signed for another version, or for none, is refused like a bad signature (`UpdateSignatureInvalid`).
- **Release workflow:** `.github/workflows/release.yml` builds, signs and uploads the artifacts, `latest.json` and `SHA256SUMS` into a draft; the runbook is [releasing.md](./releasing.md). It has not run yet (the signing secrets are not set).
- **FR-086.** Switching to a more stable channel in Settings (nightly or beta towards stable) asks first, saying nothing is downgraded and the next update comes once that channel publishes a newer version. A move to a less stable channel applies at once. `pluryprint update channel` sets it without asking: the command is the explicit choice.
- **Not built:** staged rollout and the CDN described below.

## High-level flow

```
┌────────────┐   1. tauri build    ┌──────────────┐
│ Developer  │ ──────────────────▶ │   CI runner  │
└────────────┘                     └──────┬───────┘
                                          │ 2. sign with private key
                                          │    (GitHub Secret)
                                          ▼
                                   ┌──────────────┐
                                   │  signed bin  │
                                   │  + signature │
                                   └──────┬───────┘
                                          │ 3. upload to CDN
                                          ▼
                  ┌──────────────────────────────────────┐
                  │  updates.maecly.com (Cloudflare R2)  │
                  │   /pluryprint/stable.json           │
                  │   /pluryprint/<version>/setup.exe   │
                  │   /pluryprint/<version>/setup.exe.sig│
                  └─────────────┬────────────────────────┘
                                │ 4. periodic check (24h)
                                ▼
                       ┌────────────────┐
                       │ Installed app  │
                       │ (Tauri client) │
                       └────────┬───────┘
                                │ 5. compare versions
                                │ 6. download new bin
                                │ 7. verify signature
                                │ 8. replace + restart
```

## Build pipeline (CI)

In `.github/workflows/release.yml`, after `tauri build` completes for each platform:

1. **Tauri produces** the platform-specific update artifact:
   - Windows: `target/release/bundle/nsis/pluryprint_<version>_x64-setup.nsis.zip` (Tauri's NSIS bundle wrapped for delta update; we still ship the Inno Setup `.exe` separately for first-time install).
   - macOS: `target/release/bundle/macos/Pluryprint.app.tar.gz`.
2. **Sign with `tauri signer`:** `tauri signer sign --private-key <path> --password <env>` produces a `.sig` file (a 256-byte Ed25519 signature plus metadata).
3. **Upload** the binary, the `.sig`, and the SHA-256 to the manifest CDN.
4. **Update the manifest JSON** atomically (write `stable.json.new`, then rename).

The private key lives only in GitHub Secrets (`TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`). It is generated once per project lifetime and never rotated under normal conditions. The matching public key is hardcoded in `src-tauri/tauri.conf.json` at build time.

## Manifest schema

The manifest is a static JSON file, one per channel, served over HTTPS.

**Path:** `https://updates.maecly.com/pluryprint/{channel}.json` where `{channel}` is `stable`, `beta`, or `nightly`.

**Schema:**

```json
{
  "version": "1.2.3",
  "notes": "Bug fixes and stability improvements. See CHANGELOG for details.",
  "pub_date": "2026-06-01T12:00:00Z",
  "platforms": {
    "windows-x86_64": {
      "signature": "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVUbmRiNUJWdWY5UVk4...",
      "url": "https://updates.maecly.com/pluryprint/1.2.3/pluryprint_1.2.3_x64-setup.nsis.zip"
    },
    "windows-aarch64": {
      "signature": "...",
      "url": "https://updates.maecly.com/pluryprint/1.2.3/pluryprint_1.2.3_arm64-setup.nsis.zip"
    },
    "darwin-x86_64": {
      "signature": "...",
      "url": "https://updates.maecly.com/pluryprint/1.2.3/Pluryprint_1.2.3_x64.app.tar.gz"
    },
    "darwin-aarch64": {
      "signature": "...",
      "url": "https://updates.maecly.com/pluryprint/1.2.3/Pluryprint_1.2.3_aarch64.app.tar.gz"
    }
  }
}
```

Fields:

| Field | Type | Notes |
|---|---|---|
| `version` | string | SemVer. Tauri client compares against the running version using SemVer precedence (so `1.2.3-beta.2` < `1.2.3`). |
| `notes` | string | Plain text or Markdown. Shown to the user in the update prompt. Keep < 2 KB. |
| `pub_date` | string | ISO 8601 UTC. Informational only. |
| `platforms` | object | Keys: `<os>-<arch>`, or `<os>-<arch>-<installer>` for a platform with more than one package. Values: `{signature, url}`. Missing key means no update available for that platform. As built, the published manifest carries `windows-x86_64`, `windows-aarch64`, `darwin-x86_64`, `darwin-aarch64` and, from 0.0.5 ([ADR-0015](../adr/0015-linux-support.md)), `linux-x86_64` and `linux-aarch64` (the AppImage) plus `linux-x86_64-deb` and `linux-aarch64-deb` (the `.deb`, installed with `pkexec dpkg -i`). The client tries `<os>-<arch>-<installer>` first. |
| `platforms.*.signature` | string | Base64-encoded Ed25519 signature with Tauri's framing. |
| `platforms.*.url` | string | HTTPS URL to the binary. Must be on a domain whose certificate is trusted by the OS. |

The manifest is hand-crafted in CI (a small Rust or shell script writes it) and uploaded as the **last** step of release; until the manifest points to new URLs, no client picks up the new version.

## Hosting

Three options were evaluated. We chose Cloudflare R2.

| Option | Verdict | Notes |
|---|---|---|
| **Cloudflare R2 + custom domain** | Chosen | Egress-free, S3-compatible API, integrates with Cloudflare DNS+TLS, < $5/month at our scale. |
| AWS S3 + CloudFront | Acceptable | Egress fees add up. CloudFront necessary for HTTPS on custom domain. ~$10–30/month for moderate traffic. |
| GitHub Releases | Rejected as primary | Bandwidth caps for popular projects, no fine-grained URL control, no manifest atomic updates, no per-channel domain. |

DNS: `updates.maecly.com` → Cloudflare CNAME → R2 public bucket. TLS handled by Cloudflare automatically.

Bucket layout:

```
pluryprint/
├── stable.json
├── beta.json
├── nightly.json
└── 1.2.3/
    ├── pluryprint_1.2.3_x64-setup.nsis.zip
    ├── pluryprint_1.2.3_x64-setup.nsis.zip.sig
    ├── pluryprint_1.2.3_arm64-setup.nsis.zip
    ├── pluryprint_1.2.3_arm64-setup.nsis.zip.sig
    ├── Pluryprint_1.2.3_x64.app.tar.gz
    ├── Pluryprint_1.2.3_x64.app.tar.gz.sig
    ├── Pluryprint_1.2.3_aarch64.app.tar.gz
    └── Pluryprint_1.2.3_aarch64.app.tar.gz.sig
```

Old version directories are retained for at least 12 months (so users on stale versions can still update).

## Signing process

We use Tauri's built-in updater signing (Ed25519, not the OS code signing certs).

### One-time key generation

```bash
# Done once at project setup. Output is two files: keypair.pub and keypair.key.
tauri signer generate -w ~/.tauri/pluryprint-update.key
# Public key goes in tauri.conf.json (committed).
# Private key goes in GitHub Secret TAURI_SIGNING_PRIVATE_KEY (NEVER committed).
```

The private key is encrypted with a password (also stored as a secret, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`).

### Per-release signing

In CI:

```yaml
- name: Sign Windows updater bundle
  env:
    TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
  run: |
    tauri signer sign \
      --password "$TAURI_SIGNING_PRIVATE_KEY_PASSWORD" \
      target/release/bundle/nsis/pluryprint_*_x64-setup.nsis.zip
```

Output: a `.sig` file alongside the binary. Both are uploaded to the CDN.

### Public key in `tauri.conf.json`

```json
{
  "plugins": {
    "updater": {
      "active": true,
      "endpoints": [
        "https://updates.maecly.com/pluryprint/{{target}}-{{arch}}/{{current_version}}.json"
      ],
      "dialog": false,
      "pubkey": "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDExNDdEMzdENjkzMzg3RTUKUldUbDQ0Y3pmYWxIVjlNMTRBQTJxN1lWaW9SS3o2bUlrLXFXOWNjVVJ4WG5LU3lHaXZpaUFqUlk="
    }
  }
}
```

Notes:

- `dialog: false` because we provide our own update UI in React (better i18n + UX control).
- The `endpoints` template uses Tauri's variable substitution. We override at runtime to point at the per-channel manifest based on user preference.

This public key is what the client uses to verify downloaded binaries. It is **embedded in every installed binary**. Compromising the private key would let an attacker push a malicious update to all clients; key handling is treated as critical infrastructure (only one or two maintainers have rotation authority; rotation requires a new MAJOR release because all old clients have the old public key embedded).

## Client behavior

### Periodic check

A Tauri command, scheduled via the `tokio_cron_scheduler` crate, runs every 24 hours after app launch. It also runs once at launch if the last check was > 24h ago (the schedule is a bonus, not the only mechanism).

```rust
// src-tauri/src/updater.rs
pub async fn check_for_update(app: tauri::AppHandle) -> Result<Option<UpdateInfo>> {
    let updater = app.updater()?;
    match updater.check().await {
        Ok(Some(update)) => {
            tracing::info!(
                current = %update.current_version,
                available = %update.version,
                "update available"
            );
            Ok(Some(UpdateInfo::from(update)))
        }
        Ok(None) => {
            tracing::debug!("no update available");
            Ok(None)
        }
        Err(e) => {
            tracing::warn!(?e, "update check failed");
            Err(e.into())
        }
    }
}
```

The check sends a single HTTPS GET to the manifest URL. No identifying information is sent (no user ID, no install ID, no telemetry). The User-Agent header includes the app name and version (necessary for HTTP, no PII).

### On-demand check

The Settings page has a "Check for updates now" button. Same code path, just bypasses the 24h debounce.

### Update prompt

If an update is found, the app shows a non-blocking notification:

```
┌─────────────────────────────────────┐
│  Update available: 1.2.3            │
│                                     │
│  Bug fixes and stability            │
│  improvements. See CHANGELOG.       │
│                                     │
│  [Skip]  [Remind me later] [Install]│
└─────────────────────────────────────┘
```

Behavior:

- **Skip:** record this version as skipped; never prompt for it again.
- **Remind me later:** dismiss; re-prompt in 24h.
- **Install:** start download → verify → replace → restart.

If the user has the app open continuously for days, the prompt re-appears every 24h until acknowledged.

### Download + verify + replace

```rust
update.download_and_install(
    |chunk_length, content_length| {
        emit_progress(chunk_length, content_length);
    },
    || {
        tracing::info!("download complete, applying update");
    },
).await?;
```

Tauri internally:

1. Downloads the binary URL into a temp file.
2. Verifies the Ed25519 signature using the embedded public key. **If verification fails, abort** — the update is not applied, the temp file is deleted, and an error is logged.
3. On Windows: extracts the bundle, replaces the running executable using a small bootstrapper that survives the replace operation.
4. On macOS: replaces the `.app` bundle in place.
5. Restarts the app.

If any step fails, the app falls back to the existing version (no partial state is left behind). The user sees an error toast.

## UX details

- **Default behavior:** updates are **opt-in** in the sense that the user must click Install. We never auto-install without explicit consent. (We do auto-check, but only auto-install in the rare case the user has explicitly enabled "auto-install updates" in Settings.)
- **Background download:** download happens in a background task; user can keep using the app. Restart prompt appears when download completes.
- **Bandwidth:** downloads are full binaries (~20–40 MB), not deltas. Tauri does not yet support delta updates; we will revisit if user complaints arise.
- **Offline:** if the manifest fetch fails, the app silently logs and retries next cycle. No error shown to user. (We do not pester users about network issues that are not their problem.)
- **Metered networks:** on Windows 10/11, the app respects the metered-network flag and skips the periodic check (user can still check manually).

## Rollback strategy

Auto-update has no built-in rollback. If a release goes bad, our recovery options are:

1. **Yank the version** from the manifest (see [release-strategy.md](../06-roadmap/release-strategy.md#yanking-policy)). New installs and existing users no longer see the bad version offered, but users already on the bad version stay there.
2. **Ship a hotfix** as the next patch version. Users will receive it on next check.
3. **For critical issues** (data loss, security): publish a `YANKED` marker file at `https://updates.maecly.com/pluryprint/yanked/<version>.txt`. The app checks for this on every launch (separate from update check). If the user's current version matches a yanked version, the app refuses to start with a clear error and a one-click "Update now" button.

We do **not** support automatic downgrades. If a user wants to revert, they uninstall the bad version and manually install an older `.exe` / `.pkg` from GitHub Releases. We document this path in the troubleshooting docs.

## Channel switching

The Settings page exposes a channel selector:

```
Update channel:
( ) Stable    — released every 4–8 weeks. Most polished.
( ) Beta      — released every ~2 weeks. New features early.
( ) Nightly   — released nightly. Latest, may be unstable.
```

Behavior on switch:

- **Switching upward** (stable → beta → nightly): immediate. Next check pulls from the new channel's manifest. If the new channel has a higher version, the update prompt appears.
- **Switching downward** (nightly → beta → stable): the app warns: "Switching to Stable will not downgrade your current version. You will receive Stable updates only when a Stable version >= your current version is released." The app remembers the channel choice and applies it from the next check.

Channel preference is stored in the user-area config (`%APPDATA%\Maecly\Pluryprint\config\settings.toml` on Windows, `~/Library/Application Support/com.Maecly.Pluryprint/settings.toml` on macOS), separate from app install. So uninstalling and reinstalling preserves the channel choice if the user keeps their data.

## Failure modes

| Failure | Detection | Recovery |
|---|---|---|
| Manifest URL returns 404 | HTTP status check | Log warning, retry next cycle. |
| Manifest URL returns malformed JSON | Parse error | Log error, alert maintainer (via uptime monitor on the file), retry next cycle. |
| Binary URL returns 404 | HTTP status check | Log error, surface to user as "Update download failed", retry on next prompt. |
| Binary signature verification fails | Tauri signer rejects | Hard abort. **This is treated as a security event.** Log full details. Alert the maintainer immediately (the only paths to this are: corrupted CDN file, MITM attack, or key compromise). |
| Disk full during download | I/O error | Surface to user with clear message. |
| Restart after install fails | Bootstrapper error | Old binary remains operational. User can restart manually. |

## Monitoring

We monitor:

- **Manifest endpoint uptime:** uptime-kuma instance pinging each channel's manifest every 5 minutes. Alert on >2 consecutive failures.
- **Manifest validity:** a daily CI job parses each manifest and asserts schema validity, valid signatures (re-verifiable from the server side), and reachable binary URLs.
- **Optional: download counters.** Cloudflare R2 provides aggregate request counts. We use these only to spot anomalies (e.g. a binary URL suddenly serving 100x normal traffic might indicate a CDN cache issue).

We do **not** track individual user updates. The 24h check is anonymous from our side.

## See also

- [installer-windows.md](./installer-windows.md)
- [installer-macos.md](./installer-macos.md)
- [code-signing.md](./code-signing.md)
- [ci-cd.md](./ci-cd.md)
- [../06-roadmap/release-strategy.md](../06-roadmap/release-strategy.md)
- [../09-legal/privacy.md](../09-legal/privacy.md)
