# Releasing Pluryprint

Pluryprint updates itself from this repository's GitHub Releases. Every update
artifact is signed with the project's own minisign key, and the app refuses an
update it cannot verify, so a release built without that key is worse than no
release: it installs, and then no copy of it can ever update.

The pipeline follows Versorium's (`versorium-app/.github/workflows/release.yml`
and `docs/project/RELEASING.md`), with the decisions of round 3 (ADR-0011):
draft release on tag, no Apple Developer ID and no Windows code-signing
certificate, separate Apple silicon and Intel downloads, Windows x64 and ARM64
installers, a beta channel.

Where things stand on 2026-10-10:

- **0.0.1 preview is published** (2026-10-10), and the repository is
  **public**: it went public with this first preview rather than waiting for
  the first stable release (decision D1, revised). Release files and the
  `latest.json` manifests download anonymously.
- **The pipeline has run**: `.github/workflows/release.yml` (build, sign,
  draft with checksums) built the 0.0.1 draft, and
  `.github/workflows/release-channels.yml` moves the beta channel when a draft
  is published.
- **Verified by hand**: the ARM64 installer built natively on Windows 11 ARM
  (13.48 MiB), installed silently, carried SumatraPDF 3.6.1 ARM64 with its
  licence and source offer, printed to a paused queue, and uninstalled clean;
  the Intel macOS build ran under Rosetta (CLI, GUI start, a print to the CUPS
  hold queue). See section 9.

## 1. The signing key

Tauri signs update artifacts with minisign (Ed25519). The public key, key id
`46C121ACF2A79915`, is compiled into the app from `src-tauri/tauri.conf.json`
(`plugins.updater.pubkey`). The private half is `~/.maecly/updater.key` on the
maintainer's machine. **Do not generate a new pair**: every installed copy
trusts only the public key it was built with, so rotating it after a release
strands every install. Before the first release, rotating costs nothing; after
it, treat the pair as permanent.

**The private key must never be committed, pasted into an issue or read by a
tool.** Anyone holding it can sign an update that every installed copy accepts
and runs. Keep the file, and a backup of it offline: a GitHub secret cannot be
read back.

### Prove the pair before setting the secret

The workflow can check that a key is set, not that it is the right one. If the
private key is not the partner of the public key in `tauri.conf.json`, every
release it produces is refused by every installed copy. Prove the pair first,
on your own machine:

```bash
echo probe > /tmp/probe.txt
pnpm exec tauri signer sign -f ~/.maecly/updater.key /tmp/probe.txt   # asks for its password

# The public key exactly as the app carries it:
python3 -c "import json,base64,sys;sys.stdout.buffer.write(base64.b64decode(json.load(open('src-tauri/tauri.conf.json'))['plugins']['updater']['pubkey']))" > /tmp/updater.pub
python3 -c "import base64,sys;sys.stdout.buffer.write(base64.b64decode(open('/tmp/probe.txt.sig').read()))" > /tmp/probe.txt.minisig
minisign -V -p /tmp/updater.pub -m /tmp/probe.txt -x /tmp/probe.txt.minisig   # brew install minisign
```

It must print `Signature and comment signature verified`. Then append a byte to
`/tmp/probe.txt` and run the last command again: it must fail. (The release
workflow repeats both checks on every artifact, against the same public key.)

## 2. Secrets and settings to add

Settings → Secrets and variables → Actions → **Repository secrets**:

| Secret | Value | Needed |
|---|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | the **contents** of `~/.maecly/updater.key`, not its path | for every tag build. The `guard` job stops one without it. Only one step of the `sign` job reads it (section 5) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | the key's password | yes. ADR-0008: release keys have a password, because a passwordless key can leave a Windows build waiting at Tauri's password prompt (Windows has no empty environment variables). Versorium's empty password did build on `windows-latest` through tauri-action, but this project's rule stands |

`GITHUB_TOKEN` is provided by Actions; do not create it. No other secret is
used: there is no Apple or Microsoft certificate (decision D2). Adding them
later is a workflow change, described in `code-signing.md`; it does not replace
the minisign key.

Repository settings:

| Setting | Value | Why |
|---|---|---|
| Actions → General → Workflow permissions | **read** (the default for new repositories) | every workflow states its own permissions. In `release.yml` only the `publish` job asks for `contents: write`, to create the draft and upload its files |
| Visibility | **public, at the first stable release** (D1) | until then anonymous downloads of `latest.json` and the installers answer 404, so no installed copy can update and the landing page cannot link the files |
| Rules → Rulesets → tag rule for `v*` (restrict creation and deletion to maintainers) | recommended | a pushed tag starts a release build. On the free plan, rulesets on a private repository are not available; they are once it is public |
| Environments | none needed | the draft is the human gate: nothing is published until a person publishes it. A `release` environment that holds the two signing secrets and is required by the `sign` job would also keep them from any other workflow; it is optional |

## 3. The channels

| Channel | Manifest the app reads | What it gets |
|---|---|---|
| stable (default) | `https://github.com/MAECLY/pluryprint/releases/latest/download/latest.json` | the newest published release that is not a prerelease. GitHub resolves `latest` |
| beta | `https://github.com/MAECLY/pluryprint/releases/download/channel-beta/latest.json` | every published release, prerelease or stable, moving forward only |
| nightly | the beta manifest | no nightly builds are published |

The endpoints are compiled into the app (`update::install::default_endpoint`),
and the notice in About, the launch-time check and `pluryprint update install`
all read the same manifest. Nothing calls the GitHub REST API.

`channel-beta` is a fixed prerelease whose only asset is `latest.json`.
`release-channels.yml` creates it the first time and overwrites its manifest
whenever a release is published, if that release is newer (SemVer precedence)
than what beta has. So a beta user on `0.2.0-beta.3` is offered `0.2.0` when
it is published, and a stable hotfix `0.1.5` published later does not move beta
back. The updater itself only installs a strictly newer version, so switching
from beta back to stable never downgrades.

A tag with a suffix (`v0.2.0-beta.1`, `v0.2.0-rc.1`) is published as a GitHub
prerelease: beta only. A tag without one is stable.

**Repository rename (2026-10-10).** The repository was renamed from
`MAECLY/maecly-print` to `MAECLY/pluryprint`. Builds from 0.0.2 on compile the
URLs above. Installed 0.0.1 builds still read
`https://github.com/MAECLY/maecly-print/releases/latest/download/latest.json`;
GitHub redirects it, and the asset URLs in the manifest, to the new repository,
so they keep updating. Never create a repository named `maecly-print` in the
`MAECLY` organisation: it would take over the old URL and cut every 0.0.1
install off from updates (ADR-0011).

## 4. Cutting a release

`src-tauri/Cargo.toml` is the only version the app reads: `tauri.conf.json` has
no `version`, so the bundles, the updater and `CARGO_PKG_VERSION` cannot
disagree. `package.json` is kept equal. The workflow's `guard` job refuses a
tag that differs from either.

```bash
scripts/bump-version.sh 0.2.0            # or 0.2.0-beta.1 for the beta channel
# add a "## [0.2.0]" section to CHANGELOG.md
git commit -s -am "chore(release): v0.2.0"
git tag -a v0.2.0 -m "Pluryprint 0.2.0"
git push origin main v0.2.0
```

`bump-version.sh` writes `src-tauri/Cargo.toml`, the `pluryprint` entry of
`src-tauri/Cargo.lock` and `package.json`, and the Explorer extension's
version: `shell-ext-win/Cargo.toml`, its `Cargo.lock` entry and the
VERSIONINFO in `shell-ext-win/res/shellext.rc` (`X.Y.Z.0`, the version Windows
shows for `shell_ext_win.dll`; the pre-release suffix is dropped there). It
refuses a version that is not SemVer, and the `guard` job refuses a tag whose
extension version differs. NSIS accepts any pre-release suffix (no MSI is built), so `-beta.1`
works.

If a dependency changed since the last release, regenerate the notices first
(section 8): CI fails while `THIRD-PARTY-NOTICES.md` is stale.

## 5. What the workflow does

`release.yml`, on a pushed `v*` tag. Every action is pinned to a full commit
SHA (the version is in a comment beside it), every checkout keeps no git
credentials (`persist-credentials: false`), no job uses a cache, and every
`pnpm install` runs with `--ignore-scripts`. The workflow grants nothing by
default (`permissions: {}`); each job asks for what it needs:

| Job | Runner | Token | Secrets | What it runs |
|---|---|---|---|---|
| `guard` | ubuntu | `contents: read` | tests that the key is set | shell checks |
| `build` ×4 | macOS / Windows | `contents: read` | **none** | the whole build: every crate's build script and proc macro, the npm packages, the bundler |
| `sign` | ubuntu | `contents: read` | the signing key, in **one step** | `tauri signer sign` on the finished files, nothing else |
| `publish` | ubuntu | `contents: write` | `GITHUB_TOKEN` | `gh`, `python3 -I scripts/release/release_tools.py`, `minisign`; no third-party build code |

1. **guard**: stops when `TAURI_SIGNING_PRIVATE_KEY` is empty, or when the
   tag, `Cargo.toml` and `package.json` disagree, or `tauri.conf.json` pins a
   version.
2. **build**, four jobs, `fail-fast: false`:

   | Job | Runner | Target | Bundles | Label |
   |---|---|---|---|---|
   | macOS (Apple silicon) | `macos-latest` | `aarch64-apple-darwin` | `app`, `dmg` | `apple_silicon` |
   | macOS (Intel) | `macos-latest` | `x86_64-apple-darwin` | `app`, `dmg` | `apple_intel` |
   | Windows (x64) | `windows-latest` | `x86_64-pc-windows-msvc` | `nsis` | `windows_x64` |
   | Windows (ARM64) | `windows-latest`, cross-compiled | `aarch64-pc-windows-msvc` | `nsis` | `windows_arm64` |

   The Windows jobs first fetch SumatraPDF for their architecture
   (`scripts/fetch-sumatrapdf.ps1`, pinned 3.6.1 with its licence and
   `SOURCE-OFFER.txt`) and build the Explorer DLL for **both** architectures
   (`scripts/build-shell-ext.ps1 -Arch all`) into
   `src-tauri/resources/shellext/<arch>/`, because Explorer on Windows on ARM
   is ARM64 and cannot load the x64 DLL even when the x64 installer ran
   there. The NSIS hooks refuse a CI build that lacks either DLL. CI's
   `installers` lane builds the same two installers the same way.

   With `createUpdaterArtifacts` on, `tauri build` signs the update artifacts
   and will not bundle without a key (`--no-sign` would also drop the macOS
   ad hoc signature). So each build generates a **throwaway key** with a
   random password, builds with it and deletes it. Those signatures never
   leave the job: `release_tools.py stage-bundle` copies the `.dmg`, the
   `.app.tar.gz` or the NSIS installer to `dist/` under its release name
   (`Pluryprint_<version>_<label><ext>`) without them, and `dist/` is uploaded
   as the workflow artifact `unsigned-<label>`.

   Then the checks: the Windows installer against NFR-006's 20 MB (an error),
   and that the bundle carries SumatraPDF, its licence, the offer and both
   DLLs; on macOS, that every binary is the architecture its name says, that
   no `SumatraPDF.exe` slipped in, and that the Intel build starts under
   Rosetta.
3. **sign**, only when all four builds passed: downloads the four
   `unsigned-*` artifacts and runs `tauri signer sign --app-version <version>`
   (the CLI version the lockfile pins) on the four update artifacts, with the
   key and its password in the environment of that step only. The version
   goes into each signature's trusted comment; the app's updater
   (`requireSignedVersion`) refuses a signature without it. It fails unless it signed exactly
   four, and uploads the `.sig` files as `update-signatures`.
4. **publish**, only when signing passed:
   - creates the draft (`gh release create --draft --verify-tag`, a
     prerelease when the tag has a suffix) with the notes from
     `.github/release-notes.md`, the version filled in. A re-run reuses the
     draft; a tag whose release is already published is refused;
   - uploads the installers, the update artifacts and their `.sig` files;
   - writes `latest.json` (`release_tools.py make-manifest`): for each of
     `darwin-aarch64`, `darwin-x86_64`, `windows-x86_64` and
     `windows-aarch64`, the `.sig` contents and the API asset URL
     (`https://api.github.com/repos/<repo>/releases/assets/<id>`) of the file
     just uploaded. A platform without its file, its signature or its upload
     fails the job, and so does a signature not bound to the tag's version
     (`check-manifest --signed-version`). The release body becomes `notes`
     (the update dialog shows it);
   - verifies each platform's signature with `minisign` against the public
     key in `tauri.conf.json`, and that a one-byte change fails. This also
     catches a throwaway signature that slipped through;
   - attaches the Corresponding Source: `pluryprint-X.Y.Z-source.tar.gz`
     (`git archive` of the tag) and `SumatraPDF-3.6.1-source.tar.gz` (the
     commit the fetch script pins);
   - writes `SHA256SUMS` over every file and uploads it with `latest.json`.

`workflow_dispatch` (Actions → release → Run workflow) runs only the four
builds and leaves their `unsigned-*` artifacts (kept 7 days). It needs no
secret: the builds never have one.

The labels, the names they give the files and the `latest.json` key each
serves are one table, `PLATFORMS` in `scripts/release/release_tools.py`.
`src-tauri/tests/release_pipeline.rs` checks that the matrix uses them, that
no two builds can produce the same file name, and the permission, pinning and
secret rules above.

Checked on a Mac on 2026-10-10, with stand-in keys: the throwaway-key build of
the Apple silicon app, `stage-bundle`, `tauri signer sign`, `make-manifest`,
`extract-signatures` and `minisign -V` against the stand-in public key, in
that order. The bundle kept its ad hoc signature. The workflow itself has not
run.

## 6. Verify the draft before you publish

1. **The files are there, under these names** (version `X.Y.Z`):

   | Build | Files |
   |---|---|
   | macOS (Apple silicon) | `Pluryprint_X.Y.Z_apple_silicon.dmg`, `.app.tar.gz`, `.app.tar.gz.sig` |
   | macOS (Intel) | `Pluryprint_X.Y.Z_apple_intel.dmg`, `.app.tar.gz`, `.app.tar.gz.sig` |
   | Windows (x64) | `Pluryprint_X.Y.Z_windows_x64.exe`, `.exe.sig` |
   | Windows (ARM64) | `Pluryprint_X.Y.Z_windows_arm64.exe`, `.exe.sig` |
   | publish | `latest.json`, `SHA256SUMS`, `pluryprint-X.Y.Z-source.tar.gz`, `SumatraPDF-3.6.1-source.tar.gz` |

   A name missing is a build that failed. A name in tauri's own form
   (`_aarch64.dmg`, `_x64-setup.exe`) means staging was skipped.

2. **`latest.json`** has `version` X.Y.Z and these keys, each with a `url` on
   `api.github.com` and a `signature`:

   | Key | File |
   |---|---|
   | `darwin-aarch64` | `Pluryprint_X.Y.Z_apple_silicon.app.tar.gz` |
   | `darwin-x86_64` | `Pluryprint_X.Y.Z_apple_intel.app.tar.gz` |
   | `windows-x86_64` | `Pluryprint_X.Y.Z_windows_x64.exe` |
   | `windows-aarch64` | `Pluryprint_X.Y.Z_windows_arm64.exe` |

   The updater looks up `{os}-{arch}-{installer}` first and then
   `{os}-{arch}`, so the four plain keys serve every build.

   The updater resolves its target at build time, so an x64 build running
   emulated on Windows on ARM keeps updating to x64: the download page must
   send ARM users to the ARM64 installer.

3. **Checksums**:

   ```bash
   gh release download vX.Y.Z --repo MAECLY/pluryprint --dir /tmp/rel --clobber
   (cd /tmp/rel && shasum -a 256 -c SHA256SUMS)
   ```

4. **Install it yourself** on at least one platform, following the notices in
   section 7, and print a page to a paused or held queue.

5. **Edit the notes** if you want to say what changed (the body starts with
   the install notices). If you edit them after `publish` ran, the update
   dialog still shows the old text: re-run the `publish` job, which reuses
   the draft.

Then publish, in the web UI or:

```bash
gh release edit vX.Y.Z --repo MAECLY/pluryprint --draft=false
```

Publishing runs `release-channels.yml`, which points the beta channel at the
release (section 3), and `landing.yml`, which calls the Vercel Deploy Hook
that redeploys https://pluryprint.maecly.com from
[`MAECLY/pluryprint-landing`](https://github.com/MAECLY/pluryprint-landing)
with the new version, date, download links, file sizes, `SHA256SUMS` and
release-notes links in both languages, `llms.txt` and `agents.txt`.
**Nothing on the landing is edited by hand for a release**: Vercel's build
fills it in from the latest published release
([ci-cd.md](./ci-cd.md#the-landing-follows-the-release)). If the **Landing**
run fails, the hook was not called: check the `LANDING_DEPLOY_HOOK` secret.
If the Vercel deployment fails (for instance because the release lacks one
of the four installers or `SHA256SUMS`), its build log in the Vercel project
says why, and the landing keeps serving the previous deployment; fix the
release and use **Run workflow** on Landing. A pre-release (beta) leaves the
landing on the stable release.

## 7. The install notices

`.github/release-notes.md` is the body of every release (`publish` fills in
the version). It writes these into every release, in English and
Spanish, with a table of which file is for which computer. They are repeated
here so the wording stays the same wherever it is quoted (the landing page,
support answers).

> **macOS will say the app is damaged. It is not.**
> The app carries no Apple Developer ID signature, so macOS quarantines it and
> Gatekeeper reports the most misleading message it has. After dragging
> Pluryprint to Applications, clear the quarantine flag once, in Terminal:
>
> ```
> xattr -dr com.apple.quarantine /Applications/Pluryprint.app
> ```
>
> Do it only with a copy you downloaded from this release page: that flag is
> the check that protects you from a tampered download.

> **SmartScreen will interrupt the installer.**
> The installer is not signed with a Microsoft code-signing certificate, so
> Windows shows the blue "Windows protected your PC" screen. Choose **More
> info**, then **Run anyway**. It installs for your user only and asks for no
> administrator rights.

The older advice, right-click the app and choose **Open**, is wrong on current
macOS: a quarantined bundle without a Developer ID signature is reported as
damaged, and right-click → Open does not get past it.

The macOS bundles are signed ad hoc (`signingIdentity: "-"`), which Apple
silicon requires to run them at all. Ad hoc signatures cannot be notarized,
and each update changes the code identity, so macOS may ask again for
permissions it granted the previous version.

## 8. Licences and Corresponding Source

- **Third-party notices.** `THIRD-PARTY-NOTICES.md` ships in every bundle and
  reproduces every copyright and licence notice of the crates, frontend
  packages, SumatraPDF, the WebView2 loader and the NSIS installer stub. The
  crate and package part is generated:

  ```bash
  cargo install cargo-about --locked --features cli --version 0.9.2   # once
  scripts/licenses/generate-notices.sh          # after any dependency change
  scripts/licenses/generate-notices.sh --check  # what release.yml's guard runs
  ```

- **Source.** Pluryprint is AGPL-3.0 and SumatraPDF is GPL-3.0: whoever
  conveys the binaries must make the Corresponding Source available. Every
  release attaches both source archives, and the Windows installers carry
  `resources/sumatrapdf/SOURCE-OFFER.txt`, the written offer that names the
  SumatraPDF commit and points to the release's archive. Once the repository
  is public, the tag itself is the Pluryprint source too.

- **SumatraPDF pin.** 3.6.1, x64 and ARM64. The archive hashes in
  `scripts/fetch-sumatrapdf.{ps1,sh}` were computed on macOS and on the
  Windows 11 ARM VM on 2026-10-10 and match the hashes the Scoop `extras`
  bucket publishes for the same archives. To move to a new version, update
  the version, both archive hashes, the source commit of the `<version>rel`
  tag and the licence hashes in both scripts together
  (`src-tauri/tests/fetch_scripts.rs` compares them), then regenerate the
  notices.

## 9. Installer size (NFR-006)

NFR-006 caps the Windows installer at 20 MB (15 MB until 2026-10-10; the first full release build measured x64 17.08 MiB and ARM64 15.49 MiB, about 8 MiB of each being SumatraPDF, so the budget was raised rather than dropping a feature). Measured:

| Installer | Size | How |
|---|---|---|
| `Pluryprint_0.0.1_arm64-setup.exe` | 15.49 MiB | release workflow dry run, 2026-10-10 |
| `Pluryprint_0.0.1_x64-setup.exe` | 17.08 MiB | release workflow dry run, 2026-10-10 |

About 8 MiB of it is SumatraPDF (18 MB uncompressed for ARM64). `release.yml`
fails a Windows build over the budget; `ci.yml` builds no installer.

## 10. Troubleshooting

**The tag build stops at "Signing key must be configured".**
`TAURI_SIGNING_PRIVATE_KEY` is not set. Set it (section 2), delete the remote
tag (`git push origin :refs/tags/vX.Y.Z`) and push it again.

**The tag build stops at "Tag must match the app version".** Run
`scripts/bump-version.sh` with the tag's version, commit, and re-tag.

**`publish` says "latest.json has no windows-aarch64 entry".** That build did
not stage its update artifact, or the file was not uploaded. Re-run the
failed jobs; `publish` reuses the draft.

**`sign` says "signed 3 update artifacts, expected 4".** One build uploaded no
`.app.tar.gz` or `.exe`. Look at that build's "Stage the release files" step.

**`publish` fails at "Verify the update signatures".** A signature does not
verify against the key in `tauri.conf.json`: the secret is not the partner of
that public key (section 1), or a throwaway signature reached the draft. Do
not publish; fix the secret and re-run from `sign`.

**A Windows job fails at "Installer size budget".** The installer passed
20 MB. Look at what grew (`cargo bloat`, the SumatraPDF version) before raising
the budget, which is a requirement change (NFR-006).

**Clients never see the update.** In order of likelihood: the repository is
still private; the release is still a draft; it is a prerelease and the
client is on stable; the version was not bumped; a platform key is missing
from `latest.json`.

**A client downloads but refuses to install.** The signature did not verify:
the build was signed with a key that is not the partner of the `pubkey` in
`tauri.conf.json`. Section 1's check exists to prevent this.

**The macOS release build fails locally with "mis-aligned LINKEDIT string pool"
for a proc-macro dylib.** Seen on a macOS 27 host when cross-building
`--target x86_64-apple-darwin`: the release profile strips the host
proc-macro dylibs. Build with `CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_STRIP=false`.
