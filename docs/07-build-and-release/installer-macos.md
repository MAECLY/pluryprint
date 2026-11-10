# Installer (macOS)

> **TL;DR.** **Today:** releases ship an ad-hoc-signed `.dmg` per architecture (Apple silicon, Intel), not notarized, for macOS 13+ (see the status section). **The design below, for when a Developer ID exists:** macOS distribution is a Universal binary `.app` bundle, packaged into both a `.pkg` (via `pkgbuild` + `productbuild`) and a `.dmg` (via `create-dmg`). Both artifacts are codesigned with an Apple Developer ID Application certificate and notarized through `xcrun notarytool`. Targets macOS 13+ on Apple Silicon and Intel.

## Status: what ships today

The design below (Universal binary, `.pkg`, Developer ID, notarization) is **not what releases produce**. What does:

| Design in this document | What `release.yml` builds |
|---|---|
| One Universal `.app` (`lipo`) | Two builds, `aarch64-apple-darwin` and `x86_64-apple-darwin`, published as separate `Pluryprint_X.Y.Z_apple_silicon.dmg` and `Pluryprint_X.Y.Z_apple_intel.dmg`, each with its `.app.tar.gz` updater archive and `.sig` |
| `.pkg` + `.dmg` | `.dmg` only (Tauri's `app,dmg` bundles) |
| Developer ID + notarization | Ad hoc signature (`signingIdentity: "-"`), no notarization ([ADR-0011](../adr/0011-release-channel.md)); the release notes give the `xattr -dr com.apple.quarantine` step Gatekeeper needs |
| macOS 13+ | Unchanged: `bundle.macOS.minimumSystemVersion` is `13.0` |

The right-click integration is the in-bundle `NSServices` declaration (*Services › Print with Pluryprint*), not a Finder Sync extension; see [shell-extensions.md §3](../04-architecture/shell-extensions.md). The runbook is [releasing.md](./releasing.md). Read the rest of this document as the plan for a Developer ID build.

This document covers the M5 macOS port. Code signing and notarization details that overlap with Windows are in [code-signing.md](./code-signing.md); this file focuses on the bundle structure, the installer scripts, and the macOS-specific quirks.

## Targets

| OS | Architecture | Tested in CI? | Status |
|---|---|---|---|
| macOS 13 Ventura | x86_64 (Intel) | Yes (`macos-13` runner) | Primary support |
| macOS 13 Ventura | aarch64 (Apple Silicon) | Yes (`macos-14` runner with deployment target 13.0) | Primary support |
| macOS 14 Sonoma | x86_64 + aarch64 | Yes | Primary support |
| macOS 15 Sequoia | x86_64 + aarch64 | Yes | Primary support |
| macOS 12 Monterey or older | any | No | Not supported |

We require macOS 13 because:

- It's the oldest version currently receiving Apple security updates.
- Finder Sync Extensions and `notarytool` workflows are stable from 13 onward.
- Tauri 2's macOS minimum is 10.13, but our backend uses APIs (e.g. `os_log` modern variants) that benefit from 13+.

## App bundle structure

The `Pluryprint.app` bundle, post-build, looks like:

```
Pluryprint.app/
└── Contents/
    ├── Info.plist
    ├── PkgInfo
    ├── MacOS/
    │   └── pluryprint                # Universal binary (lipo-ed)
    ├── Resources/
    │   ├── icon.icns
    │   ├── en.lproj/
    │   │   ├── InfoPlist.strings
    │   │   └── ServicesMenu.strings   # Finder Services titles (FR-001)
    │   ├── es.lproj/
    │   │   ├── InfoPlist.strings
    │   │   └── ServicesMenu.strings   # "Imprimir con Pluryprint", …
    │   ├── LICENSE                    # AGPL-3.0
    │   ├── THIRD-PARTY-NOTICES.md
    │   └── LICENSES/                  # Apache-2.0, GPL-3.0, OFL-1.1
    ├── Frameworks/
    │   └── (any embedded frameworks; usually empty for Tauri)
    ├── PlugIns/
    │   └── PluryprintFinderExt.appex/   # Finder Sync Extension
    │       └── Contents/
    │           ├── Info.plist
    │           ├── MacOS/
    │           │   └── PluryprintFinderExt
    │           └── Resources/
    └── _CodeSignature/
        └── CodeResources
```

Highlights:

- `MacOS/pluryprint` is a Universal Mach-O binary. Built with `lipo -create` from `target/aarch64-apple-darwin/release/pluryprint` and `target/x86_64-apple-darwin/release/pluryprint`.
- `PlugIns/PluryprintFinderExt.appex` is the Finder Sync Extension, which is itself a small `.appex` bundle with its own `Info.plist` and Mach-O.
- `Frameworks/` is empty in the M5 baseline. We avoid bundling third-party `.framework` directories; SumatraPDF is Windows-only, and the Mac path uses `lpr` (CUPS) which is system-provided.

## `Info.plist` essentials

Key entries in `Contents/Info.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDisplayName</key>
  <string>Pluryprint</string>
  <key>CFBundleExecutable</key>
  <string>pluryprint</string>
  <key>CFBundleIdentifier</key>
  <string>com.maecly.pluryprint</string>
  <key>CFBundleName</key>
  <string>Pluryprint</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>1.1.0</string>
  <key>CFBundleVersion</key>
  <string>1.1.0.42</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.utilities</string>
  <key>NSHumanReadableCopyright</key>
  <string>Copyright (C) 2026 Maecly. Licensed under AGPL-3.0-only.</string>
  <key>NSAppTransportSecurity</key>
  <dict>
    <key>NSAllowsArbitraryLoads</key>
    <false/>
    <key>NSExceptionDomains</key>
    <dict>
      <key>updates.maecly.com</key>
      <dict>
        <key>NSIncludesSubdomains</key>
        <true/>
        <key>NSRequiresCertificateTransparency</key>
        <true/>
      </dict>
    </dict>
  </dict>
  <key>NSPrintingUsageDescription</key>
  <string>Pluryprint sends documents to printers you choose.</string>
</dict>
</plist>
```

Notes:

- `CFBundleIdentifier` must match the Developer ID provisioning profile.
- `LSMinimumSystemVersion` enforces the macOS 13 floor; older versions get a clear "won't open" error from Launch Services.
- `NSAppTransportSecurity` whitelists only `updates.maecly.com`; everything else is denied.

## Universal binary with `lipo`

Build steps for the Universal Mach-O:

```bash
#!/usr/bin/env bash
set -euo pipefail

# Build both slices
cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-apple-darwin

# Combine
lipo -create \
  -output target/universal-apple-darwin/release/pluryprint \
  target/aarch64-apple-darwin/release/pluryprint \
  target/x86_64-apple-darwin/release/pluryprint

# Verify
lipo -info target/universal-apple-darwin/release/pluryprint
# Expected: Architectures in the fat file: pluryprint are: x86_64 arm64
```

The same flow runs for the Finder Sync Extension's executable.

## Building the `.pkg` installer

`installers/macos/pkgbuild.sh`:

```bash
#!/usr/bin/env bash
# Builds Pluryprint.pkg from a fully-built Pluryprint.app
set -euo pipefail

VERSION="${1:?Usage: pkgbuild.sh <version>}"
APP_PATH="build/Pluryprint.app"
PKG_PATH="dist/Pluryprint-${VERSION}.pkg"
COMPONENT_PKG="dist/Pluryprint-component-${VERSION}.pkg"

mkdir -p dist

# 1. Build the component package (the app payload)
pkgbuild \
  --component "${APP_PATH}" \
  --install-location "/Applications" \
  --identifier "com.maecly.pluryprint" \
  --version "${VERSION}" \
  --sign "${APPLE_DEVELOPER_ID_INSTALLER}" \
  "${COMPONENT_PKG}"

# 2. Build the distribution package (wraps the component, adds welcome/license/conclusion)
productbuild \
  --distribution "installers/macos/distribution.xml" \
  --package-path "dist" \
  --resources "installers/macos/resources" \
  --sign "${APPLE_DEVELOPER_ID_INSTALLER}" \
  "${PKG_PATH}"

# 3. Cleanup component package; only distribution package ships
rm "${COMPONENT_PKG}"

echo "Built: ${PKG_PATH}"
```

`installers/macos/distribution.xml`:

```xml
<?xml version="1.0" encoding="utf-8"?>
<installer-gui-script minSpecVersion="2">
  <title>Pluryprint</title>
  <organization>com.maecly</organization>
  <welcome file="welcome.html" mime-type="text/html"/>
  <license file="LICENSE.html" mime-type="text/html"/>
  <conclusion file="conclusion.html" mime-type="text/html"/>
  <options customize="never" require-scripts="false" hostArchitectures="x86_64,arm64"/>
  <volume-check>
    <allowed-os-versions>
      <os-version min="13.0"/>
    </allowed-os-versions>
  </volume-check>
  <choices-outline>
    <line choice="default">
      <line choice="com.maecly.pluryprint"/>
    </line>
  </choices-outline>
  <choice id="default"/>
  <choice id="com.maecly.pluryprint" visible="false">
    <pkg-ref id="com.maecly.pluryprint"/>
  </choice>
  <pkg-ref id="com.maecly.pluryprint" version="1.1.0" auth="root">Pluryprint-component-1.1.0.pkg</pkg-ref>
</installer-gui-script>
```

The `auth="root"` is required because the install destination (`/Applications`) needs admin write. Users see the standard macOS authentication dialog.

## Building the `.dmg`

`installers/macos/create-dmg.sh`:

```bash
#!/usr/bin/env bash
# Builds Pluryprint.dmg using create-dmg (Homebrew: brew install create-dmg)
set -euo pipefail

VERSION="${1:?Usage: create-dmg.sh <version>}"
APP_PATH="build/Pluryprint.app"
DMG_PATH="dist/Pluryprint-${VERSION}.dmg"

mkdir -p dist

create-dmg \
  --volname "Pluryprint ${VERSION}" \
  --volicon "installers/macos/resources/volume.icns" \
  --background "installers/macos/resources/dmg-bg.png" \
  --window-pos 200 120 \
  --window-size 600 400 \
  --icon-size 100 \
  --icon "Pluryprint.app" 175 200 \
  --hide-extension "Pluryprint.app" \
  --app-drop-link 425 200 \
  --no-internet-enable \
  --codesign "${APPLE_DEVELOPER_ID_APPLICATION}" \
  "${DMG_PATH}" \
  "${APP_PATH}"

echo "Built: ${DMG_PATH}"
```

The `--codesign` flag invokes `codesign` on the produced `.dmg` itself (not just the contained `.app`). DMGs are signed because Gatekeeper checks the container, not just the contents, when downloaded.

## Codesigning the `.app`

Before `pkgbuild` or `create-dmg`, the `.app` and its embedded `.appex` must be signed. Order matters: sign inside-out.

```bash
#!/usr/bin/env bash
set -euo pipefail

APP="build/Pluryprint.app"
SIGN_ID="${APPLE_DEVELOPER_ID_APPLICATION}"
ENTITLEMENTS="installers/macos/resources/entitlements.plist"

# 1. Sign the Finder Sync Extension first (deepest first)
codesign --force --options runtime --timestamp \
  --entitlements "installers/macos/resources/finder-ext-entitlements.plist" \
  --sign "${SIGN_ID}" \
  "${APP}/Contents/PlugIns/PluryprintFinderExt.appex"

# 2. Sign any frameworks (none in M5; placeholder for future)
# for fw in "${APP}/Contents/Frameworks/"*.framework; do
#   codesign --force --options runtime --timestamp --sign "${SIGN_ID}" "$fw"
# done

# 3. Sign the main app
codesign --force --options runtime --timestamp \
  --entitlements "${ENTITLEMENTS}" \
  --sign "${SIGN_ID}" \
  "${APP}"

# 4. Verify
codesign --verify --deep --strict --verbose=2 "${APP}"
spctl --assess --type execute --verbose "${APP}"
```

`entitlements.plist` (main app):

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>com.apple.security.app-sandbox</key>
  <false/>
  <key>com.apple.security.cs.allow-jit</key>
  <false/>
  <key>com.apple.security.cs.allow-unsigned-executable-memory</key>
  <false/>
  <key>com.apple.security.cs.disable-library-validation</key>
  <false/>
  <key>com.apple.security.network.client</key>
  <true/>
  <key>com.apple.security.print</key>
  <true/>
  <key>com.apple.security.files.user-selected.read-only</key>
  <true/>
</dict>
</plist>
```

Minimal entitlements: network for the updater, print for printer access, user-selected read-only files (the user picks files to print in Finder; we read them).

We **do not** enable App Sandbox in M5. Sandboxing complicates Finder Sync Extensions and CUPS interactions. We may revisit for a Mac App Store edition later, but the Developer ID distribution does not require sandbox.

## Notarization

Notarization is mandatory for macOS 10.15+ (and we target 13+). After signing:

```bash
#!/usr/bin/env bash
# installers/macos/notarize.sh
set -euo pipefail

ARTIFACT="${1:?Usage: notarize.sh <path-to-pkg-or-dmg>}"
APPLE_ID="${APPLE_ID:?APPLE_ID env var required}"
APPLE_TEAM_ID="${APPLE_TEAM_ID:?APPLE_TEAM_ID env var required}"
APPLE_APP_PASSWORD="${APPLE_APP_PASSWORD:?APPLE_APP_PASSWORD env var required}"

# 1. Submit and wait for notarization
xcrun notarytool submit "${ARTIFACT}" \
  --apple-id "${APPLE_ID}" \
  --team-id "${APPLE_TEAM_ID}" \
  --password "${APPLE_APP_PASSWORD}" \
  --wait

# 2. Staple the notarization ticket to the artifact
xcrun stapler staple "${ARTIFACT}"

# 3. Verify
xcrun stapler validate "${ARTIFACT}"
spctl --assess --type install --verbose "${ARTIFACT}"
```

Notes:

- `notarytool` typically returns within 5–15 minutes; in busy periods, up to a few hours.
- `--wait` blocks until Apple finishes; alternative is `submit --output-format json`, capture submission ID, poll separately.
- Stapling embeds the notarization ticket in the artifact so Gatekeeper can verify offline.
- Both `.pkg` and `.dmg` must be notarized and stapled separately; notarization is per-artifact, not per-app.

The `App Specific Password` (`APPLE_APP_PASSWORD`) is generated in [appleid.apple.com](https://appleid.apple.com) under "App-Specific Passwords". Stored in GitHub Actions secrets as `APPLE_APP_PASSWORD`.

## Finder Sync Extension

`shell-ext-mac/PluryprintFinderExt/` is a Swift target. Built with `xcodebuild`:

```bash
xcodebuild -project shell-ext-mac/PluryprintFinderExt/PluryprintFinderExt.xcodeproj \
  -scheme PluryprintFinderExt \
  -configuration Release \
  -destination "generic/platform=macOS" \
  ARCHS="x86_64 arm64" \
  ONLY_ACTIVE_ARCH=NO \
  CODE_SIGN_IDENTITY="${APPLE_DEVELOPER_ID_APPLICATION}" \
  build
```

Output is copied into `Pluryprint.app/Contents/PlugIns/PluryprintFinderExt.appex/`.

The extension's `Info.plist` declares it's a Finder Sync Extension:

```xml
<key>NSExtension</key>
<dict>
  <key>NSExtensionPointIdentifier</key>
  <string>com.apple.FinderSync</string>
  <key>NSExtensionPrincipalClass</key>
  <string>$(PRODUCT_MODULE_NAME).FinderSync</string>
</dict>
```

Users must enable the extension in **System Settings → Extensions → Added Extensions → Pluryprint** after first launch. The app prompts the user to do this on first run with a deep link.

If Finder Sync Extension proves too restrictive (it has known limitations around right-click menus on multi-selections in some Finder versions), the fallback is a **Quick Action** (Automator workflow) installed at `~/Library/Services/`. Decision is made by week 3 of M5.

## Gatekeeper user experience

After notarization, on first launch:

- Downloaded `.dmg` → user double-clicks → Finder shows: "Pluryprint is an app downloaded from the Internet. Are you sure you want to open it?" → user clicks Open → app runs.
- Downloaded `.pkg` → user double-clicks → Installer.app launches → standard install flow → app runs.

No right-click bypass needed. No "developer cannot be verified" dialog.

If the user disables Gatekeeper or the notarization ticket is missing, they get the legacy "unidentified developer" dialog. We do not support this path.

## Universal binary on M-series CI

`macos-14` is Apple Silicon. Building the x86_64 slice from there requires Rosetta 2 only at runtime (not at build time, since Rust cross-compiles natively). The `x86_64-apple-darwin` target is added via `rustup target add x86_64-apple-darwin`. No special tooling required.

We could alternatively build x86_64 on `macos-13` (Intel) and aarch64 on `macos-14`, then combine in a third job. The single-runner approach is simpler and ~5 minutes faster end-to-end.

## Reproducibility notes

macOS reproducibility is harder than Windows due to:

- `codesign` embeds timestamps (mitigated by `--timestamp` argument which uses Apple's TSA).
- Notarization staples are timestamps embedded in the artifact.
- `pkgbuild` and `productbuild` embed timestamps in their archive metadata.

We accept that **signed + notarized** artifacts are not bit-reproducible. The **unsigned** intermediate `.app` bundle is reproducible given fixed `SOURCE_DATE_EPOCH`.

## See also

- [installer-windows.md](./installer-windows.md) — Windows counterpart
- [code-signing.md](./code-signing.md) — full code signing details for Windows and macOS
- [installer-linux.md](./installer-linux.md) — the Linux AppImage and `.deb`
- [auto-update-flow.md](./auto-update-flow.md) — how installed apps receive updates
- [ci-cd.md](./ci-cd.md) — where this installer is built in CI
