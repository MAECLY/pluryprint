# CI/CD

> **TL;DR.** `ci.yml` is the lean pull-request gate: frontend lint, typecheck, format, tests and `pnpm audit` on ubuntu, and Rust fmt, clippy and the full test suite on Windows and macOS, plus `cargo audit` on macOS. It builds no bundle and reads no secret. `release.yml` builds a signed release into a **draft** on every `v*` tag (after checking the signing secret, the versions and `THIRD-PARTY-NOTICES.md`), and `release-channels.yml` moves the beta channel when a person publishes it: see [releasing.md](./releasing.md), the runbook, and [ADR-0011](../adr/0011-release-channel.md). Nothing is notarized or Authenticode-signed (decision D2), there is no nightly workflow, and the first release, 0.0.1 preview, was published on 2026-10-10. `pages.yml` redeploys the landing with the latest published release's version and downloads on every publish ([below](#the-landing-follows-the-release)). There is no sign-off check: contributions come in under the [CLA](../../CLA.md), which the pull request template asks for.

This document specifies what CI does; [releasing.md](./releasing.md) covers the release workflows.

## Workflows that exist

| Workflow | File | Trigger | Signs? | Publishes? |
|---|---|---|---|---|
| **CI** | `.github/workflows/ci.yml` | `pull_request`, `push` → `main`, `workflow_dispatch` | No | No |
| **release** | `.github/workflows/release.yml` | `push` of a `v*` tag; `workflow_dispatch` (build only) | Update artifacts (minisign); no OS code signing | A **draft** release, published by a person |
| **release-channels** | `.github/workflows/release-channels.yml` | `release: published` | No | Overwrites `latest.json` on the `channel-beta` prerelease, forward only |
| **Pages** | `.github/workflows/pages.yml` | `push` → `main` touching `site/`, `scripts/site/` or itself; `release: published`; `workflow_dispatch` | No | The landing page, filled with the latest published release ([below](#the-landing-follows-the-release)) |

Only `ci.yml` runs on a pull request. There is no scheduled workflow.

```yaml
concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true
```

If a new commit lands on a PR, the in-flight run for the previous commit is cancelled.

## `ci.yml` job by job

Two jobs, both blocking, and nothing else. `src-tauri/tests/release_pipeline.rs` (`ci_is_the_lean_pull_request_gate`) pins this shape, so a lane added back by accident fails the Rust tests.

### `frontend` — ubuntu-latest

`pnpm install --frozen-lockfile --ignore-scripts`, then:

1. `pnpm lint` (ESLint, including `eslint-plugin-jsx-a11y`)
2. `pnpm typecheck` (`tsc --noEmit`)
3. `pnpm format:check` (Prettier)
4. `pnpm test` (Vitest)
5. `pnpm audit --prod --audit-level high`: a high or critical advisory in a production dependency fails the job (see [Dependency audits](#dependency-audits))

> Note that `e2e/tsconfig.json` is standalone: `pnpm typecheck` does **not** type-check the E2E specs. They are checked when `pnpm test:e2e` compiles them, which CI does not run (see [testing-strategy.md](../08-development/testing-strategy.md)).

### `backend` — matrix `windows-latest` + `macos-latest`

The two platforms Pluryprint ships for. The Rust code is heavily `cfg`-gated — WinSpool, SumatraPDF and the Explorer handoff on Windows, the CUPS backend (`platform/cups.rs`, `engine/print/cups.rs`) on macOS — so each half only compiles, and its tests only run, on its own OS. `fail-fast: false` keeps one platform's failure from hiding the other's. `PLURYPRINT_SKIP_MIGRATION=1` is set for the job, so a test run never moves a MaeclyPrint-era data directory ([ADR-0009](../adr/0009-rename-to-pluryprint.md)).

1. macOS only: `cargo audit` in `src-tauri/` and in `shell-ext-win/`, with cargo-audit 0.22.2 installed by `taiki-e/install-action` (see [Dependency audits](#dependency-audits)); the lockfiles do not depend on the runner
2. `cargo fmt --check` for `src-tauri` and `shell-ext-win` — Windows only; formatting does not depend on the platform
3. `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`
4. `cargo test --manifest-path src-tauri/Cargo.toml` — the full suite, never filtered
5. Windows only: `cargo clippy` and `cargo test` for `shell-ext-win/`

Step 4 is also the **bindings drift gate**: `tests/ipc_surface.rs` regenerates `src/ipc/bindings.ts` from the tauri-specta command surface and fails when the committed copy is stale. Its failure message says what to do (`UPDATE_BINDINGS=1 cargo test …` and commit the result). It only runs in a full test run, which is why the step takes no filter.

On Windows, `src-tauri/build.rs` links the Common-Controls v6 manifest into **every** artifact (`/MANIFEST:EMBED /MANIFESTINPUT:windows-app-manifest.xml`), test harnesses included. With Tauri's default (a manifest resource linked into `[[bin]]` targets only) the lib's test binary died with `STATUS_ENTRYPOINT_NOT_FOUND` (0xc0000139) before running a test.

The Explorer context-menu DLL is its own crate: `shell-ext-win/` is deliberately **not** a Cargo workspace member, because its release profile must keep `panic = "unwind"` (so `catch_unwind` at the COM boundary contains a panic instead of taking Explorer down), which `src-tauri`'s `panic = "abort"` would override. `--manifest-path src-tauri/Cargo.toml` never touches it, hence steps 1 and 5. Its COM layer only builds for Windows. No `src-tauri` test needs the built DLL: the installer-hook and bundle tests read the NSIS script and use a placeholder file.

### What CI does not run, and where it went

| Check | Where it lives now |
|---|---|
| Installers, the NFR-006 size budget, both Explorer DLLs in each bundle | `release.yml` `build` (every Windows target) |
| COM exports of the shipped DLLs (`DllGetClassObject`, `DllCanUnloadNow`) | `release.yml` `build`, step *Explorer DLLs export the COM entry points* |
| `THIRD-PARTY-NOTICES.md` is current (`scripts/licenses/generate-notices.sh --check`) | `release.yml` `guard`, so no release is built from stale notices; run it by hand after a dependency change |
| Coverage (Vitest V8, `cargo llvm-cov`) | Local only: `pnpm test:coverage`, `cargo llvm-cov` |
| `#[ignore]`d tests (live update manifest, real CUPS queue) | Local only: `cargo test -- --ignored` |
| E2E (WebdriverIO + tauri-driver) and smoke launches | Local only: `pnpm test:e2e` on Windows; the VM scripts under `scripts/vm/` |
| Windows on ARM | `release.yml` cross-compiles the ARM64 app and DLL on every tag |

The COM export check that an earlier `shell-ext-win` CI lane ran was itself broken: it tested `$exports -notmatch $symbol` with `$exports` being `dumpbin`'s output as an **array** of lines, and on an array PowerShell's `-notmatch` returns the non-matching lines instead of a yes or no, so it threw on every DLL. The DLL was fine: `lib.rs` exports both functions with `#[no_mangle] pub unsafe extern "system"`, and the linker's `LNK4104 ... should be PRIVATE` warning in the same log names them in the export table. The release check joins the output into one string first.

### Dependency audits

Both jobs fail on a known vulnerability, and the only exceptions are written down with a reason, next to the configuration that applies them.

- **Rust (RustSec).** `cargo audit` fails on any vulnerability in `src-tauri/Cargo.lock` or `shell-ext-win/Cargo.lock`. Informational advisories (unmaintained, unsound) are reported but do not fail it. `src-tauri/.cargo/audit.toml` is the ignore list; `cargo audit` reads it from the directory it runs in, so run it from `src-tauri/` locally too. Ignores are by advisory ID, so a new advisory against the same crate still fails. `ci_is_the_lean_pull_request_gate` checks that every ignored ID has a comment above it.
- **npm.** CI audits production dependencies at `high` and above. A full audit (`pnpm audit`, dev dependencies included) is clean apart from the advisories in `package.json` → `pnpm.auditConfig.ignoreGhsas`, listed below. Patched releases inside the major the tree already uses are forced with `pnpm.overrides`.

Accepted advisories (2026-10-18):

| Advisory | Package | Why it is accepted | Exit |
|---|---|---|---|
| RUSTSEC-2024-0429 (unsound) | glib 0.18 | Linux only: Tauri's GTK/WebKitGTK stack, never built for Windows or macOS | A Tauri release off gtk-rs 0.18 |
| RUSTSEC-2024-0370 (unmaintained) | proc-macro-error 1.0 | Linux only and build time only: proc-macro of glib-macros | Same |
| RUSTSEC-2024-0436 (unmaintained) | paste 1.0 | Build time only: proc-macro of specta, pinned by tauri-specta | specta without it |
| RUSTSEC-2026-0192 (unmaintained) | ttf-parser 0.25 | Through usvg/fontdb for SVG text; reads the system's fonts, not the printed file. svg2pdf 0.13 is the latest | svg2pdf on a newer usvg |
| RUSTSEC-2026-0206 (unmaintained) | rustybuzz 0.20 | Text shaping in usvg, same path as ttf-parser | Same |
| RUSTSEC-2026-0367, 0368, 0369 (vulnerabilities) | zip 4.6 | Windows only, through tauri-plugin-updater, to unpack a zip update package; ours are NSIS `.exe` installers, and a package is unpacked only after its minisign signature has been checked. The flaws are SUID/SGID bits and Unix symlink targets, which do not apply on Windows. zip 9 has the fix; tauri-plugin-updater 2.14 still requires zip ^4 | A tauri-plugin-updater on zip 9 |
| GHSA-5c6j-r48x-rmvq, GHSA-qj8w-gfj5-8c6v (high, moderate) | serialize-javascript 6.0.2 | Dev only: mocha 10, required by `@wdio/mocha-framework` 9, uses it only in its parallel mode (worker pool), to pass our own test results between processes. The fix is 7.x, a new major | A mocha on serialize-javascript 7 |
| GHSA-jmr9-qjv8-65gv, GHSA-7pqw-9j4j-h8q3 (high) | extract-zip 2.0.1 | Dev only: `@puppeteer/browsers` 2 in `@wdio/utils` 9 unpacks downloaded browsers and drivers, which the E2E suite never asks for (it drives the built app through tauri-driver). No patched release exists | WebdriverIO 10, whose `@puppeteer/browsers` 3 drops extract-zip |
| GHSA-c475-qrg2-pj4r (high) | basic-ftp 5.3.1 | Dev only: `get-uri` in the proxy support of the same `@puppeteer/browsers` 2, reached only when it downloads a browser through an `ftp:` PAC URL. Fixed in 6.2.1, a new major that `get-uri` (latest included) does not accept | WebdriverIO 10 (`@puppeteer/browsers` 3 has no proxy-agent) |
| GHSA-vfj7-8cjw-p6xm (high) | braces 3.0.3 | Dev only: micromatch/chokidar in Tailwind 3, lint-staged and mocha, matching our own glob patterns. No patched release exists | A patched braces |
| GHSA-rj75-hqrm-r3gf (moderate) | postcss-selector-parser 6.1.4 | Dev only: Tailwind 3 parses our own stylesheets with it. The fix is 7.1.6, a major Tailwind 3 does not accept (Tailwind stays on 3.x, see `.github/dependabot.yml`) | Tailwind on postcss-selector-parser 7 |

Fixed rather than accepted in October 2026: RUSTSEC-2026-0187 (lopdf 0.39, stack overflow on deeply nested PDF objects, which a user's PDF could trigger: lopdf 0.44 and printpdf 0.12, with a regression test in `engine/convert`), RUSTSEC-2026-0285 (rustls 0.23.45), RUSTSEC-2026-0346 to 0349 (rustls 0.23.46), RUSTSEC-2026-0194/0195 (quick-xml 0.42 through plist 1.10), RUSTSEC-2026-0190 (anyhow 1.0.104), RUSTSEC-2026-0221 (event-listener 5.4.2), and 73 npm advisories through WebdriverIO 9.32 and the overrides.

## The landing follows the release

`site/` names no version, release date, download link or file size. Those places carry markers (`{{release.version}}`, `{{release.date}}`, `{{release.date_es}}`, `{{release.date_en}}`, `{{release.notes_url}}`, `{{release.sha256sums_url}}`, and `{{release.<file>.url}}`, `.mib_en`, `.mib_es` for `windows_x64`, `windows_arm64`, `apple_silicon` and `apple_intel`) in both landings, their JSON-LD and meta descriptions, the two `site.webmanifest` files, `llms.txt` and `agents.txt`.

`pages.yml` fetches `GET /repos/MAECLY/pluryprint/releases/latest` with the job's `GITHUB_TOKEN` (`contents: read`), and `scripts/site/apply_release.py` (standard-library Python, run with `python3 -I`) copies `site/` to `$RUNNER_TEMP/site` with every marker filled. That copy is what Pages publishes; nothing generated is committed. The script fails the deploy, with nothing published, when the release is a draft or pre-release, its tag is not `v<SemVer>`, any of the four installers or `SHA256SUMS` is missing, empty, not fully uploaded or not at `releases/download/<tag>/<name>`, a marker is unknown or malformed, a download marker is used nowhere, or a JSON-LD block or manifest no longer parses. It has no switch to skip a check.

The workflow runs on `release: published`, so publishing a release redeploys the landing. The `github-pages` environment only deploys from `main` and a release event runs on the tag, so that run's only job (`redeploy`) starts `pages.yml` on `main` with `workflow_dispatch`, and the `main` run deploys. GitHub does not start workflows from events caused by `GITHUB_TOKEN`, so a release published by a workflow would not; ours are published by a person, and **Run workflow** redeploys by hand. Pre-releases are never "latest", so a beta leaves the landing on the stable release.

To preview the filled landing locally:

```bash
gh api repos/MAECLY/pluryprint/releases/latest > /tmp/release.json
python3 -I scripts/site/apply_release.py --release-json /tmp/release.json \
  --repo MAECLY/pluryprint --site site --out /tmp/site
python3 -m http.server -d /tmp/site 8000
```

`src-tauri/tests/site_release.rs` runs the script against a fixture release, checks that no file in `site/` hardcodes a release link or version, that a missing installer or `SHA256SUMS` is refused, and pins the trigger and the step order in `pages.yml`.

## Caching

The `backend` job uses `actions/cache@v6` directly (not `Swatinem/rust-cache`) over `~/.cargo/{registry/index,registry/cache,git/db}` plus `src-tauri/target/` and `shell-ext-win/target/`, keyed on `${{ runner.os }}-cargo-` and both lockfiles. pnpm modules are cached by `actions/setup-node@v7` with `cache: pnpm`. The release jobs use no cache: their `setup-node` steps set `package-manager-cache: false`.

## What does **not** exist yet

Everything in this section is **unimplemented**. It is recorded as design intent. Do not cite it as a description of the repository.

| Missing | Status |
|---|---|
| `.github/workflows/nightly.yml` | Not written. There are no nightly builds: the app's **nightly** channel setting follows the beta manifest (FR-085). |
| OS code signing | No Authenticode certificate, no Apple Developer ID, no notarization, no stapling (decision D2). macOS bundles are signed ad hoc. Update artifacts **are** signed, by `release.yml`. |
| Artifact publication outside GitHub (Cloudflare R2, own domain) | Not planned for now: releases and manifests live on GitHub Releases ([ADR-0011](../adr/0011-release-channel.md)). |
| `installers/windows/build.ps1`, `installers/macos/*.sh` | The `installers/` tree contains only `.gitkeep` files. |
| `scripts/sign-*.ps1`, `scripts/upload-to-r2.sh`, `scripts/update-manifest.sh`, `scripts/build-mac-universal.sh`, `scripts/setup-apple-certs.sh`, `scripts/verify-windows-signatures.ps1` | None exist. What `scripts/` does hold for releases: `fetch-sumatrapdf.{ps1,sh}`, `build-shell-ext.ps1`, `bump-version.sh`, `licenses/generate-notices.sh` `release/release_tools.py` (the manifest and signature helper `release.yml` runs) and `site/apply_release.py` (fills the landing from the latest release for `pages.yml`). |
| Universal macOS binary | Not built: `release.yml` builds Apple silicon and Intel as separate downloads (Versorium's layout). |
| Scheduled `cargo audit` / `pnpm audit` (NFR-065) | Both run on every pull request and push to `main` ([Dependency audits](#dependency-audits)), but nothing re-audits an unchanged tree when a new advisory is published. Dependabot (`.github/dependabot.yml`) opens the update PRs. |
| `gitleaks` secret scanning (NFR-027) | Not configured. |
| Reproducible-build verification (NFR-064) | Not automated. |

### Updater signing

Releases are signed so the installed app can verify an update before installing it (FR-082, [ADR-0008](../adr/0008-installer-nsis-per-user.md)). The key pair is minisign, generated once with `pnpm tauri signer generate`:

- **Public key** — committed, in `src-tauri/tauri.conf.json` → `plugins.updater.pubkey`. Every build embeds it and accepts only artifacts signed by the matching private key.
- **Private key** — **never committed**. A release build reads it from the `TAURI_SIGNING_PRIVATE_KEY` secret (the key file's contents), plus `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` if the key has a password. `release.yml` keeps it away from the builds: they sign with a throwaway key, and the `sign` job runs `tauri signer sign` with the real one on the NSIS setup (Windows) and `Pluryprint.app.tar.gz` (macOS), writing the `.sig` files the release carries ([releasing.md](./releasing.md) §5). Keep an offline backup. If the key is lost, installed copies cannot be updated in place. If it leaks, rotate it and ship the new public key through a release signed with the old one.
- **The private key must have a password.** Tauri asks for the password whenever `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` is not set, and a passwordless key can only be used by setting that variable to an empty string. Windows PowerShell cannot do that: `$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""` *deletes* the variable (verified with `Test-Path env:...` on 5.1), so `pnpm tauri build` finishes both bundles and then waits forever at the password prompt, with no `.sig` written. In a non-interactive job (a scheduled task, CI) that is a hang, not an error. Generate release keys with a non-empty password (`pnpm tauri signer generate -w <file>` asks for one), and run signing builds with `CI=true` and a job timeout so a missing password fails instead of hanging. If an existing key has no password, sign the artifact explicitly instead, from `cmd.exe`: `pnpm tauri signer sign -f <key file> -p "" --app-version <version> <artifact>`. Windows PowerShell 5.1 drops empty arguments to native programs, so the same line typed there loses the `""`; `cmd.exe` passes it through.
- **Signed version** — every update signature is bound to its release: `tauri signer sign --app-version <version>` writes `version:<version>` into the signature's trusted comment, which the signature covers. The app's updater has `requireSignedVersion` on, so an artifact signed by hand without `--app-version`, or for another version than the manifest announces, is refused. `release.yml` signs with the tag's version and `release_tools.py check-manifest --signed-version` checks every platform before the draft is finished.
- **Manifest** — the release uploads `latest.json` as a release asset: `version`, `notes`, `pub_date`, and a `platforms` map (`windows-x86_64`, `windows-aarch64`, `darwin-aarch64`, `darwin-x86_64`; the updater falls back from `{os}-{arch}-{installer}` to these) of `{ url, signature }`, where `signature` is the contents of the matching `.sig`. The stable channel reads it from `https://github.com/MAECLY/pluryprint/releases/latest/download/latest.json`, the beta channel from the `channel-beta` release (see [releasing.md](./releasing.md) §3).

PR and `main` builds have no key and must not get one; `ci.yml` builds no bundle at all. With `createUpdaterArtifacts` on, Tauri refuses to bundle without a private key, so an unsigned local build uses `--config src-tauri/tauri.ci.conf.json`, which turns updater artifacts off for that build only.

To test an update end to end without publishing, serve a `latest.json` and a signed artifact locally, then point a **debug** build at it with `PLURYPRINT_UPDATE_ENDPOINT=http://127.0.0.1:<port>/latest.json`. Release builds only accept `https`. The variable also redirects `update check`, the About screen's check and the launch-time check, which read that manifest's `version` and `notes` exactly as they read the channel's published manifest (`update::fetch_configured_latest`), so the whole flow can be exercised against one test server. A copy of the artifact with one byte changed must fail with "failed its signature check and was NOT installed" (exit code 11 from `update install`).

`BUILD_DATE` (About, issue reports) is the build script's run date, re-stamped on every source change. A reproducible release pins it with `SOURCE_DATE_EPOCH`.

### Required secrets

**None** for `ci.yml`, which reads no `secrets.*` value. `release.yml` needs `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (release keys must have a password; see above), and its `guard` job fails a tag build when the key is not set. Only one step of its `sign` job reads them; the four builds get no secret and a read-only token. The full list of secrets and repository settings is in [releasing.md](./releasing.md) §2.

### Constraints the release pipeline keeps

- **Tag-triggered** on `v*`, with the channel derived from the tag (any `-` suffix → prerelease, beta channel; otherwise stable).
- **Manual approval gate.** The workflow only ever builds a **draft**; a person publishes it after the checks in [releasing.md](./releasing.md) §6. On the free plan with a private repository, Environments with required reviewers are not available; the draft is the gate.
- **No signing in CI.** PR and `main` builds must never have access to signing material; forks and contributors cannot accidentally sign.
- **`scripts/fetch-sumatrapdf.ps1` runs before any Windows packaging step.** `SumatraPDF.exe` is not committed (see `src-tauri/resources/sumatrapdf/README.md`), so a packaging job that skips this produces an installer whose every print job fails with `DependencyMissing`.
- **Least privilege and pinned actions.** Every workflow states its permissions (`ci.yml` reads only; in `release.yml` only `publish` has `contents: write`), every action is pinned to a full commit SHA with its version in a comment, and every checkout sets `persist-credentials: false`. `pnpm install` runs with `--ignore-scripts` everywhere. The release jobs use no cache. `src-tauri/tests/release_pipeline.rs` fails when any of this regresses; `actionlint` and `zizmor` report nothing on the workflows (2026-10-10).
- **No self-hosted runners.** GitHub-hosted runners are sufficient at our scale, and a self-hosted runner with access to signing secrets is a supply-chain liability. If cost becomes a concern, narrow the PR matrix rather than moving off hosted runners.
- **Notices before a release.** The `guard` job regenerates `THIRD-PARTY-NOTICES.md` with cargo-about 0.9.2 and fails the tag build when the committed copy differs.

## Artifacts

`ci.yml` uploads none. The release builds' artifacts are described in [releasing.md](./releasing.md).

## Status checks required for merge

Branch protection is a GitHub repository setting, not something this file can assert. Both jobs are designed to gate: `frontend`, and `backend` on both matrix entries (`Backend (windows-latest)`, `Backend (macos-latest)`).

## See also

- [installer-windows.md](./installer-windows.md)
- [installer-macos.md](./installer-macos.md)
- [code-signing.md](./code-signing.md)
- [auto-update-flow.md](./auto-update-flow.md)
- [../06-roadmap/release-strategy.md](../06-roadmap/release-strategy.md)
- [../08-development/release-checklist.md](../08-development/release-checklist.md)
- [../08-development/testing-strategy.md](../08-development/testing-strategy.md)
