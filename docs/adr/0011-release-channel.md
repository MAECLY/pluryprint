# ADR-0011: Release channel on GitHub Releases, with a beta manifest and no OS code signing

## Status

Accepted.

Update (2026-10-10): the repository went public with the 0.0.1 preview instead of waiting for the first stable release, so D1 below no longer holds; everything else stands.

Update (2026-10-10): the repository was renamed from `MAECLY/maecly-print` to `MAECLY/pluryprint`. Builds from 0.0.2 on compile the new manifest URLs. Installed 0.0.1 builds still ask the old endpoint (`github.com/MAECLY/maecly-print/releases/latest/download/latest.json`) and reach the new repository through GitHub's redirect, which also covers the asset URLs in `latest.json`. That redirect lasts only while no repository named `maecly-print` exists under `MAECLY`: never create one, or every 0.0.1 install stops updating. The `MAECLY/maecly-print` mentions below describe the state on the date of this ADR.

Update (2026-11-09): D20 ("Linux is out of scope") is superseded by [ADR-0015](./0015-linux-support.md). Linux x86_64 and ARM64 ship as AppImage and `.deb` on this same channel.

## Date

2026-10-10

## Context

[ADR-0004](./0004-auto-updater-tauri-builtin.md) chose the Tauri updater and
[ADR-0008](./0008-installer-nsis-per-user.md) the per-user NSIS installer with
signed updater artifacts. What was missing was the channel those artifacts are
published on, and the client side of it:

- No workflow built, signed or published a release. The updater's endpoint
  (`releases/latest/download/latest.json`) answered 404.
- The "update available" notice asked the GitHub REST API
  (`api.github.com/repos/.../releases/latest`) while the installer read
  `latest.json`: two sources that could disagree, an anonymous limit of 60
  requests an hour per address, and nothing for the beta channel, which the
  settings recorded but no code read (FR-085 "Partial").
- The repository `MAECLY/maecly-print` is private on GitHub's free plan. An
  anonymous request for a private repository's release asset answers 404, with
  or without the API.
- There is no Apple Developer ID and no Windows code-signing certificate.
  Versorium, the owner's other app, ships without both, with release notes
  that explain Gatekeeper and SmartScreen.
- Round-3 decisions (2026-10-09): the repository stays private until the first
  stable release, then goes public (D1); releases are made exactly like
  Versorium's (D2/D4); Windows ARM64 gets its own installer (D5); a beta
  channel exists (D8); SumatraPDF 3.6.1 ships for x64 and ARM64 (D17); Linux is
  out of scope (D20).

## Decision

We will publish releases on **this repository's GitHub Releases**, built by
`.github/workflows/release.yml` into a **draft** on every `v*` tag, and published
by a person.

- **Builds:** macOS Apple silicon and macOS Intel as **separate** downloads
  (`Pluryprint_X.Y.Z_apple_silicon.*`, `..._apple_intel.*`), not a universal
  binary; Windows x64 and ARM64 NSIS installers
  (`..._windows_x64.exe`, `..._windows_arm64.exe`), the ARM64 one
  cross-compiled on `windows-latest`. Every Windows installer carries both
  Explorer DLLs and SumatraPDF for its own architecture.
- **Trust:** update artifacts are signed with the project's minisign key
  (`TAURI_SIGNING_PRIVATE_KEY`); a tag build **fails** when the secret is not
  set. The workflow verifies every signature against the public key compiled
  into the app, and publishes `SHA256SUMS` over every file. The installers are
  not Authenticode-signed and the macOS bundles are signed ad hoc; the release
  notes, in English and Spanish, say how to get past SmartScreen and
  Gatekeeper.
- **Channels (FR-085):** stable reads `releases/latest/download/latest.json`;
  beta (and nightly, which has no builds) reads the `latest.json` of a fixed
  prerelease tagged `channel-beta`, which `.github/workflows/release-channels.yml`
  overwrites when any release is published, if it is newer by SemVer
  precedence. A beta user therefore also receives the stable release of the
  same version, and beta never moves back (FR-086).
- **One source of truth:** the notice, the launch-time check and
  `pluryprint update install` all read the channel's manifest
  (`update::install::default_endpoint`). The GitHub REST API is not called.
- **Corresponding Source:** each release attaches `git archive` of its tag and
  the SumatraPDF source tree of the pinned commit; the Windows installers carry
  SumatraPDF's written offer (`SOURCE-OFFER.txt`).
- **Notices:** `THIRD-PARTY-NOTICES.md` is generated from the lockfiles
  (`scripts/licenses/generate-notices.sh`, cargo-about) and CI fails when it is
  stale.

The runbook, with the secrets and settings to add, is
[releasing.md](../07-build-and-release/releasing.md).

## Alternatives Considered

| Option | Why not |
|---|---|
| Make the repository public now | The owner's decision D1: public at the first stable release. The pipeline is built so that nothing changes on that day except visibility. |
| A separate public repository for releases (`pluryprint-releases`) with a GitHub App token | Two repositories to keep in step, a cross-repository token, issues split across them, and no benefit once the source goes public anyway. |
| Own domain or object storage (`updates.maecly.com`, R2/S3) | Costs and a second system to secure; GitHub Releases already serves files and checksums. The endpoint list can gain a second URL later. |
| A token embedded in the app to read the private repository | Anyone could extract it and read the private source. |
| Keep the REST API for the notice | Two sources of truth, a rate limit, no beta feed. |
| A universal macOS binary | Twice the download for everyone; Versorium's separate assets proved simpler. Optional later if trivial. |
| Apple Developer ID + notarization, Windows OV/EV or Azure Artifact Signing | Annual cost, and Azure Artifact Signing is not available to an individual in Spain. Deferred; NFR-021 stays unmet and the notes say so. |
| A nightly channel | No nightly builds exist; nightly follows beta until they do. |

## Consequences

### Positive

- A release is one `git tag` away once the two secrets are set, and nothing
  reaches users until a person publishes the draft.
- The same manifest drives the notice and the install, on both channels.
- Every release is checked before it can be published: four platforms in
  `latest.json`, valid signatures, the installer under 20 MB, SumatraPDF and
  both DLLs in each Windows installer, the right architecture in each Mac app.
- The licence obligations travel with the binaries: notices, licence texts,
  source archives and the written offer.

### Negative

- Until the repository is public, no installed copy can see an update: the
  manifests answer 404 and the app reports the channel as unavailable
  (`channel_unavailable`), not as up to date.
- Without OS code signing, first installs meet SmartScreen and Gatekeeper, and
  macOS may ask again for permissions after each update (ad hoc identity).
- An x64 build running emulated on Windows on ARM keeps updating to x64; the
  download page has to send ARM users to the ARM64 installer.

### Risks / Open Questions

- The four builds merge their entries into one `latest.json`; two finishing
  together can drop one. `finalise` refuses such a manifest; the fix is to
  re-run that build.
- The endpoint URLs are compiled into every copy. Renaming or moving the
  repository relies on GitHub's redirects.
- FR-086's confirmation when switching to a lower channel is in Settings
  (2026-10-10); "no downgrade" is guaranteed by the updater itself.
- `releases/latest` follows creation date: a stable hotfix for an older line
  published after a newer stable becomes "latest". The updater never
  downgrades, but such a hotfix should be published as a prerelease or with
  `--latest=false`.
