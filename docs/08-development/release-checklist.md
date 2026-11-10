# Release Checklist

> **TL;DR.** Pre-release: bump version, update CHANGELOG, run all tests, complete QA matrix. Release: tag, let CI build + sign + notarize, manually approve publish, verify the live download. Post-release: announce, watch for issues, prepare for next cycle.

> **Status (2026-10-10).** The release process that exists is the runbook [releasing.md](../07-build-and-release/releasing.md): a tag builds a **draft** with `release.yml`, and §6 there is the checklist for verifying it before publishing. Releases carry no Authenticode, Developer ID or notarization ([ADR-0011](../adr/0011-release-channel.md)), and there is no `installers/windows/build.ps1`, so the items below that name `signtool`, `codesign --deep`, `spctl`, `stapler` or that script do not apply; the QA matrix and the post-release steps still do.

This is the operational runbook a maintainer follows to ship a release. Print it, work through it top-to-bottom, do not skip steps. Each item has a checkbox; do not consider the release complete until every box is checked.

## Pre-release (T-7 to T-1 days)

### Version planning

- [ ] Decide the new version per [release-strategy.md](../06-roadmap/release-strategy.md). Confirm whether this is `MAJOR`, `MINOR`, or `PATCH` and on which channel (`stable`, `beta`).
- [ ] Confirm no breaking changes are sneaking in unannounced. If a breaking change is included, this MUST be a `MAJOR` bump.
- [ ] Announce intent on the project communication channels: "Cutting `<version>` on `<date>`. Soft freeze on new features starts now."

### Code freeze and final commits

- [ ] Open a "release prep" PR (or commit directly if maintainer-only): bump version in `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, `package.json`. All three must agree.
- [ ] Update `CHANGELOG.md`: rename the `## [Unreleased]` section to `## [<version>] - <YYYY-MM-DD>`. Add a new empty `## [Unreleased]` section above it.
- [ ] Write `RELEASE_NOTES.md` (in repo root, gitignored after release) summarizing user-facing changes. This file is referenced by the release workflow and becomes the GitHub Release body.
- [ ] Verify `RELEASE_NOTES.md` reads well to a non-developer: highlight what's new, mention upgrade notes if any, link to full CHANGELOG.

### Tests

- [ ] All CI checks green on `main` for the head commit.
- [ ] Local `pnpm check-all` green.
- [ ] Run `cargo test --include-ignored` locally. All tests pass (this catches subprocess-dependent tests that may be skipped in regular dev).
- [ ] Run E2E suite (`pnpm test:e2e`) against a fresh build: green.
- [ ] Run `cargo audit`. Zero unaddressed advisories. (Address with explicit allowlist + comment if waiver justified.)
- [ ] Re-check `THIRD-PARTY-NOTICES.md` with the commands in its "How this list was made" section (`cargo tree` on the four release targets, a production `vite build`). Every new crate or package is named, and every new licence is compatible with the AGPL-3.0 (see [third-party-licenses.md](../09-legal/third-party-licenses.md)).
- [ ] The release notes name the SumatraPDF version the Windows installer ships, so its "corresponding source" is unambiguous.

### Build verification

- [ ] Local production build succeeds: `pnpm tauri build`.
- [ ] Local installer build succeeds: `scripts\fetch-sumatrapdf.ps1`, `scripts\build-shell-ext.ps1 -Arch all`, then `pnpm tauri build` (see [installer-windows.md](../07-build-and-release/installer-windows.md)); on macOS `pnpm tauri build --bundles app,dmg`; on Linux `pnpm tauri build --bundles appimage,deb` (see [local-setup.md](./local-setup.md) for the packages).
- [ ] Reproducibility check (Windows): build the installer twice from the same commit; unsigned `setup.exe` files are byte-identical.

### Manual QA matrix

For a stable release, run the manual QA suite on **every** combination below. For a beta, run on at least one Windows + one macOS combination. Linux is a preview ([ADR-0015](../adr/0015-linux-support.md)): until a full pass on real hardware has been recorded for an architecture, its rows below are what removes the label, not a release blocker.

| OS | Architecture | Display scaling | Test sequence |
|---|---|---|---|
| Windows 10 22H2 | x64 | 100% | Smoke + print to PDF |
| Windows 10 22H2 | x64 | 200% | Smoke (UI scaling check) |
| Windows 11 23H2 | x64 | 100% | Full QA suite |
| Windows 11 23H2 | x64 | 200% | Smoke (UI scaling check) |
| Windows 11 23H2 | ARM64 | 100% | Full QA suite |
| macOS 13 Ventura | x64 (Intel) | Default | Full QA suite (M5+ only) |
| macOS 14 Sonoma | aarch64 (M-series) | Default | Full QA suite (M5+ only) |
| macOS 15 Sequoia | aarch64 (M-series) | Default | Smoke (M5+ only) |
| Ubuntu 22.04, GNOME on X11 | x86_64 | 100% | Full QA suite, AppImage and `.deb` |
| Ubuntu 24.04, GNOME on Wayland | x86_64 | 100% | Full QA suite, AppImage (AppArmor user namespaces, `libfuse2t64`) |
| Debian 12, KDE Plasma | x86_64 | 100% | Smoke, `.deb`, the Dolphin entries |
| Fedora (current), GNOME | x86_64 | 100% | Smoke, AppImage |
| Linux Mint (Cinnamon) or Ubuntu with Nemo | x86_64 | 100% | The Nemo entries, `.deb` |
| Ubuntu 24.04 ARM64 (VM) | aarch64 | 100% | Full QA suite, AppImage and `.deb` |

**Format coverage** (each platform): print at least one of each format end-to-end:

- [ ] PDF (single page)
- [ ] PDF (multi-page)
- [ ] DOCX
- [ ] XLSX
- [ ] PPTX
- [ ] ODT
- [ ] PNG
- [ ] JPG
- [ ] HTML
- [ ] MD
- [ ] TXT

**Printer coverage** (cycle through these per platform):

- [ ] Microsoft Print to PDF (Windows) / cups-pdf (macOS, Linux) — virtual sink
- [ ] At least one **physical laser** printer (we test with a Brother HL-L2350DW)
- [ ] At least one **physical inkjet** printer (we test with an HP DeskJet)
- [ ] At least one **network** printer (we test with a Canon over IP)

**Smoke checklist** (each platform combination, ~10 min):

- [ ] Installer launches and completes with no errors.
- [ ] First-run flow appears (welcome message, default profile created).
- [ ] App launches in <5 seconds on a typical machine.
- [ ] Default printer is detected.
- [ ] Single-PDF print works.
- [ ] About page shows correct version.
- [ ] Settings page shows correct channel.
- [ ] Right-click in File Explorer shows Pluryprint submenu.
- [ ] Uninstaller runs cleanly; no leftover files in install dir.

**Full QA suite** (~30 min): the smoke checklist plus:

- [ ] Print 30-file batch; all complete; queue progress UI accurate.
- [ ] Switch between profiles; settings persist across restart.
- [ ] Trigger "Check for updates" with a stub manifest pointing to a fake newer version; update prompt appears with correct content.
- [ ] Switch language EN ↔ ES; UI updates immediately, dates/numbers reformatted.
- [ ] Disable a required dependency (e.g. uninstall LibreOffice); print attempt shows clear error and skips Office files.
- [ ] Check accessibility: tab through every page using keyboard only; all interactive elements reachable.
- [ ] Check accessibility: enable screen reader (NVDA on Windows, VoiceOver on Mac); navigate first 3 pages; all elements announced correctly.

### Update site staging

- [ ] Build the release manifest JSON (don't publish yet). Save as `staging-<channel>.json`.
- [ ] Manually verify each `signature` field decodes as valid Ed25519 against the embedded public key.
- [ ] Manually verify each `url` field resolves (fetch HEAD; expect 200).
- [ ] On a test machine running the previous version, point its updater at the staging manifest. Verify update prompt appears, download succeeds, signature verifies, app restarts on the new version.

### Signing verification

- [ ] `release.yml`'s signature check passed: every updater artifact verifies against the public key compiled into the app, and the tampered copy fails (job log, `sigcheck/`).
- [ ] `SHA256SUMS` lists every asset and matches a downloaded copy.
- [ ] *(Only once certificates exist, ADR-0011.)* Each Windows binary in the release artifact set passes `signtool verify /pa /v <file>`.
- [ ] Each macOS artifact passes `codesign --verify --deep --strict --verbose=2 <artifact>`.
- [ ] Each macOS `.pkg` and `.dmg` passes `spctl --assess --verbose <artifact>`.
- [ ] Each macOS artifact passes `stapler validate <artifact>`.
- [ ] Linux has no OS signature to check. Each `.deb` lists the expected contents and dependencies (`dpkg-deb --info` / `--contents`: `Depends: cups-client, libc6 (>= 2.35), libwebkit2gtk-4.1-0 (>= 2.42)` plus the unversioned WebKitGTK and GTK entries Tauri adds, `Recommends: cups-ipp-utils, libheif-examples`, no `Suggests` (Tauri cannot set one), the `.desktop` entry, the Dolphin and Nemo entries), and each AppImage starts with `--version` on its own architecture.

## Release day (T-0)

### The push

- [ ] All pre-release checkboxes complete. (Re-verify any items that aren't explicitly persisted.)
- [ ] Final `RELEASE_NOTES.md` is committed to the release branch.
- [ ] Tag the release: `git tag -a v<version> -m "<version>"` (annotated tag with version as message).
- [ ] Push the tag: `git push origin v<version>`.
- [ ] CI's release workflow starts. Watch the run; do not multitask.

### Manual approval gate

When CI reaches the `publish` job, GitHub will request approval (via the `production` Environment).

- [ ] Verify all build jobs in the run succeeded.
- [ ] Verify the artifacts attached to the workflow look right (filenames, sizes ~ expected).
- [ ] Approve the deploy.

### Post-publish verification

After CI completes:

- [ ] Manifest at `https://updates.maecly.com/pluryprint/<channel>.json` reflects the new version. Open in a browser; check JSON.
- [ ] Each binary URL in the manifest is reachable. Run `./scripts/verify-published-manifest.sh <channel>` which fetches all URLs.
- [ ] GitHub Release for `v<version>` is published with the correct binaries attached and notes match `RELEASE_NOTES.md`.
- [ ] `CHANGELOG.md` on `main` is correctly updated.

### Updater dry-run

On a fresh test machine running a previous version:

- [ ] Trigger "Check for updates" via the GUI.
- [ ] Update prompt appears with correct version + notes.
- [ ] Download starts, completes (note download time for benchmarking).
- [ ] App restarts on new version.
- [ ] About page shows new version number.
- [ ] Functionality smoke check (open app, navigate to Settings, print one PDF) succeeds.

If any updater step fails, **immediately**:

- [ ] Yank the version (see [release-strategy.md](../06-roadmap/release-strategy.md#yanking-policy)). Update the GitHub Release notes with `[YANKED]` prefix and reason.
- [ ] Remove the version from the channel's manifest. Push corrected manifest.
- [ ] Investigate root cause. Fix in `main`. Cut a new patch.

## Post-release (T+1 to T+7)

### Announcements

- [ ] Post on project social channels (X/Twitter, Mastodon if active). Use a template:
  ```
  Pluryprint <version> is out!

  • <highlight 1>
  • <highlight 2>
  • <highlight 3>

  Full notes: <link to GitHub Release>
  Download: <link to website>
  ```
- [ ] If MAJOR or significant MINOR: post a blog entry on the project site.
- [ ] Send to opt-in email list (if `MAJOR` or significant `MINOR`).
- [ ] Update project website's "Latest version" callout.

### Monitoring (T+1 to T+7)

- [ ] Watch issue tracker daily for new bug reports tagged with the new version.
- [ ] Watch the manifest endpoint uptime monitor.
- [ ] Spot-check Cloudflare R2 download counts; anomalies (e.g. one binary getting 100x normal traffic) may indicate a CDN cache issue.

### Triage

For any bug report:

- [ ] Triage severity (critical, high, medium, low) within 24 hours.
- [ ] Critical (data loss, security, crash on launch): consider yanking + emergency hotfix.
- [ ] High (major feature broken): plan a patch within 1 week.
- [ ] Medium / low: schedule for next regular release.

## Cleanup

- [ ] Delete `RELEASE_NOTES.md` from the working tree (gitignored anyway, just hygiene).
- [ ] Move incomplete release-prep changes (if any) back to feature branches for the next cycle.
- [ ] Open a tracking issue for the next planned release: "Cut `<next-version>` around `<date>`".

## Hotfix release (abbreviated)

For an urgent patch (security, critical regression):

1. Create `hotfix/<version>` branch from the previous tag (not `main` — don't pull in unfinished work).
2. Cherry-pick the fix commit from `main`.
3. Bump version in `Cargo.toml`, `tauri.conf.json`, `package.json`.
4. Update `CHANGELOG.md`.
5. Run minimal QA: smoke checklist on the affected platform.
6. Tag, let CI build + sign + notarize.
7. Approve publish.
8. Verify updater delivers the fix.
9. Announce on social with `[Security]` or `[Hotfix]` prefix.

Hotfixes skip the manual QA matrix breadth in exchange for speed. The bar: the fix must not introduce a new bug worse than the one being fixed.

## Failure recovery

If something fails mid-release:

| Failure | Action |
|---|---|
| CI build fails | Investigate, fix, push fix to `main`, re-tag (delete and recreate the tag locally and remotely). |
| Notarization fails | Read the notarization log, fix entitlement / signing issue, repush tag. |
| Manual QA finds a critical bug after tag | Delete the tag, do not approve publish, fix the bug, re-tag (with the same version since the bad tag never published). |
| Manifest update fails after binaries are uploaded | Manually upload the corrected manifest. R2 has versioned objects so rollback is possible. |
| Bad release reaches users | Yank immediately. Hotfix within 24-48h. |

## See also

- [../06-roadmap/release-strategy.md](../06-roadmap/release-strategy.md)
- [../07-build-and-release/ci-cd.md](../07-build-and-release/ci-cd.md)
- [../07-build-and-release/auto-update-flow.md](../07-build-and-release/auto-update-flow.md)
- [../07-build-and-release/code-signing.md](../07-build-and-release/code-signing.md)
- [testing-strategy.md](./testing-strategy.md)
