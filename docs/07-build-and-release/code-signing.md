# Code Signing

> **TL;DR.** **Not in use:** releases carry no Authenticode or Developer ID signature (ADR-0011); updates are signed with Pluryprint's own minisign key, see the status section below. The design for when certificates exist: Windows binaries are signed with an Extended Validation (EV) certificate (~$300/year) for instant SmartScreen reputation; macOS uses Apple Developer ID Application certs ($99/year, mandatory) plus Apple notarization. Private keys live in HSMs or in CI secrets behind tight access controls. Self-signed certs are used only for nightly builds.

## Status: no OS code signing (ADR-0011)

**Nothing described below the next heading is in use.** [ADR-0011](../adr/0011-release-channel.md) (2026-10-10) releases Pluryprint the way Versorium is released, without Apple or Microsoft certificates:

| What | How it is done today |
|---|---|
| Windows installers | **Not Authenticode-signed.** SmartScreen shows "Windows protected your PC"; the release notes (English and Spanish) say to choose **More info → Run anyway**. The installer is per user and asks for no administrator rights. |
| macOS bundles | **Signed ad hoc** (`bundle.macOS.signingIdentity: "-"` in `tauri.conf.json`), which Apple silicon needs to run the app at all. Not notarized: ad hoc signatures cannot be. The release notes give the `xattr -dr com.apple.quarantine` step that Gatekeeper requires. |
| Linux packages | **No signature of any kind** beyond the two rows below ([ADR-0015](../adr/0015-linux-support.md)): the AppImage carries no embedded signature and the `.deb` no `debsig` or GPG signature. Linux has no Gatekeeper or SmartScreen equivalent, so nothing warns; a GPG signature would only matter with an apt repository, which the project does not run. |
| Updates | Every updater artifact is signed with Pluryprint's own **minisign** key (`TAURI_SIGNING_PRIVATE_KEY`, a GitHub secret); the app verifies it against the public key compiled in (`plugins.updater.pubkey`) and refuses anything else. `release.yml` verifies every signature, and that a tampered copy fails, before the draft is finished. |
| Integrity of downloads | `SHA256SUMS` is attached to every release. |

The full procedure is the runbook [releasing.md](./releasing.md). The rest of this document is the certificate-based design, kept for the day certificates are bought (it would also unblock the signed sparse package of [shell-extensions.md §2.5](../04-architecture/shell-extensions.md)). Treat its statements as a plan, not a description.

This document covers code signing on Windows and macOS (Linux has no certificate scheme to plan for): cert types, costs, procurement, signing tools, CI integration, and common pitfalls. It complements the OS-specific build documentation in [installer-windows.md](./installer-windows.md), [installer-macos.md](./installer-macos.md) and [installer-linux.md](./installer-linux.md).

## Why we sign

| Without signing | With signing |
|---|---|
| Windows SmartScreen blocks downloads with red warning. | EV cert: green/no warning from day 1. OV cert: warning fades after ~3000 reputable downloads. |
| macOS Gatekeeper refuses to launch downloaded binaries from unidentified developer. | Signed + notarized: standard "downloaded from internet" prompt only. |
| Antivirus heuristics flag unsigned binaries more aggressively. | Signed binaries pass most AV heuristics; signature is part of trust. |
| Auto-updater cannot replace running binary on Windows (UAC complications). | Signed binaries replace cleanly with proper installer hooks. |

Signing is **not** optional for any public release. Nightly builds get a self-signed fallback; everything else gets a real cert.

## Windows code signing

### EV vs OV vs self-signed

| Cert type | Cost / year | SmartScreen reputation | Procurement time | Use case |
|---|---|---|---|---|
| **EV (Extended Validation)** | $250–500 | Instant (day 1, no warnings) | 1–4 weeks (extensive vetting) | Public stable + beta |
| **OV (Organization Validation)** | $80–200 | Earned over ~3000 downloads (~weeks to months) | 1–2 weeks (org vetting) | Fallback if EV not yet procured |
| **Self-signed** | Free | None — always shows warning | Minutes | Nightly builds only |

**We use EV for stable + beta and self-signed for nightly.** The cost gap is justified: SmartScreen warnings are a near-100% download abandonment trigger for new users.

### EV cert acquisition

EV certs come on a **physical USB token** (e.g. SafeNet eToken) or in a cloud HSM (e.g. Azure Key Vault Premium tier with Managed HSM). We use the cloud HSM path because:

- USB tokens are a single point of failure (lost token = no signing until re-issued).
- USB tokens require physical presence on the signing machine — bad fit for cloud CI.
- Cloud HSM signing is more expensive operationally but compatible with GitHub Actions runners.

**Vendor selection:** Sectigo, DigiCert, GlobalSign, SSL.com all offer EV certs with cloud HSM support. We selected SSL.com (~$300/year) based on the best Cloud Signing Service experience and clear documentation.

**Procurement steps:**

1. Create an organization profile with the CA (legal name, address, phone).
2. Submit organizational documents (incorporation certificate, business license).
3. CA conducts an in-person or video verification call.
4. CA issues the cert and provisions it in their cloud HSM.
5. Configure CI runner with API credentials to sign via the CA's API.

Total: 1–4 weeks. We start procurement at the beginning of M3 to ensure the cert is ready by M4.

### `signtool.exe` (the canonical signer)

`signtool.exe` ships with the Windows SDK. Standard usage:

```powershell
signtool sign `
  /sha1 <thumbprint> `
  /fd SHA256 `
  /td SHA256 `
  /tr "http://timestamp.sectigo.com" `
  /d "Pluryprint" `
  /du "https://maecly.com" `
  setup.exe
```

| Flag | Purpose |
|---|---|
| `/sha1 <thumbprint>` | Selects the cert by its SHA-1 thumbprint. Alternative: `/n "Pluryprint Project"` to select by subject name. |
| `/fd SHA256` | File digest algorithm (signed hash of file contents). |
| `/td SHA256` | Timestamp digest algorithm. |
| `/tr <url>` | Timestamp authority URL. **Critical**: without a timestamp, the signature expires when the cert does, and existing signed binaries become "invalid" overnight. With a timestamp, the signature remains valid forever (the timestamp proves the signature was made while the cert was valid). |
| `/d "Pluryprint"` | Description shown in UAC dialog. |
| `/du <url>` | Description URL (clickable in some dialogs). |

We sign every `.exe` and every `.dll` we ship: the main app, the shell extension DLL, the installer, and the bundled SumatraPDF (re-signing it does not violate its license; we ship it as-is and add our signature on top).

### Cloud HSM signing via SSL.com CodeSignTool

For our cloud HSM cert, we use SSL.com's `CodeSignTool.jar`:

```powershell
java -jar CodeSignTool.jar sign `
  -username "$env:SSL_COM_USERNAME" `
  -password "$env:SSL_COM_PASSWORD" `
  -credential_id "$env:SSL_COM_CREDENTIAL_ID" `
  -totp_secret "$env:SSL_COM_TOTP_SECRET" `
  -input_file_path setup.exe `
  -output_dir_path signed/
```

The TOTP secret automates the 2FA challenge that would otherwise require human interaction. It is stored as a GitHub Secret and never logged.

### `osslsigncode` (Linux/Mac alternative)

For developers signing locally on non-Windows machines (e.g. testing the signing pipeline before pushing), `osslsigncode` (open source) can sign Authenticode binaries:

```bash
osslsigncode sign \
  -pkcs12 cert.p12 \
  -pass "$CERT_PASSWORD" \
  -t "http://timestamp.digicert.com" \
  -in setup.exe \
  -out setup-signed.exe
```

We use this for local testing only. Production signing always goes through the cloud HSM via `CodeSignTool.jar` to ensure key custody.

### SmartScreen reputation

Even with an EV cert, SmartScreen looks at:

- The cert publisher's reputation across all binaries signed with it.
- The download URL's reputation.
- The binary's hash reputation (built up over downloads).

EV certs **bypass** the per-binary reputation requirement (this is the key benefit). OV certs do not; they require accumulating ~3000 reputable downloads before the warning disappears.

If despite EV signing we see warnings (rare but possible for the first few releases), we submit the binary to Microsoft for manual review via the [Microsoft Defender SmartScreen Reputation Submission portal](https://www.microsoft.com/wdsi/filesubmission). Manual review takes 1–3 weeks.

### Verification

After every signing, verify before shipping:

```powershell
signtool verify /pa /v setup.exe
```

`/pa` uses the default authentication policy (any trusted CA). Output should include:

```
Successfully verified: setup.exe
Number of files successfully Verified: 1
```

If verification fails, the binary is **not shipped**. The CI release job halts.

## macOS code signing

### Apple Developer ID Application cert

| Cert type | Cost / year | Required for | Use case |
|---|---|---|---|
| **Developer ID Application** | $99 (Apple Developer Program membership covers all certs) | Distributing outside Mac App Store | All our macOS builds |
| **Developer ID Installer** | Same membership | Signing `.pkg` installers | Our `.pkg` files |
| **Apple Distribution** | Same membership | Mac App Store distribution | Not used (we don't ship to MAS) |

Apple Developer Program membership is a flat $99/year and grants all needed cert types.

### Cert acquisition

1. Enroll in Apple Developer Program (one-time application, ~24h approval).
2. In Apple Developer portal, create a new certificate of type "Developer ID Application" and "Developer ID Installer".
3. Download the cert files; install into the macOS Keychain on the signing machine.
4. Export the certs (with private keys) as `.p12` files for CI usage.

Cert files in CI:

- `developer-id-application.p12` — for `codesign` of `.app` bundles.
- `developer-id-installer.p12` — for `pkgbuild` / `productbuild` of `.pkg`.

Both `.p12` files are stored as GitHub Secrets (`APPLE_DEVELOPER_ID_APPLICATION_P12_BASE64`, `APPLE_DEVELOPER_ID_INSTALLER_P12_BASE64`) and decoded into a temporary keychain at the start of the CI run.

### Importing certs in CI (macOS runner)

```bash
#!/usr/bin/env bash
set -euo pipefail

# Decode .p12 files from base64-encoded secrets
echo "$APPLE_DEVELOPER_ID_APPLICATION_P12_BASE64" | base64 --decode > /tmp/app.p12
echo "$APPLE_DEVELOPER_ID_INSTALLER_P12_BASE64" | base64 --decode > /tmp/inst.p12

# Create a temporary keychain
KEYCHAIN_PATH="$RUNNER_TEMP/build.keychain"
KEYCHAIN_PASSWORD=$(openssl rand -base64 32)
security create-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN_PATH"
security set-keychain-settings -lut 21600 "$KEYCHAIN_PATH"
security unlock-keychain -p "$KEYCHAIN_PASSWORD" "$KEYCHAIN_PATH"
security list-keychains -d user -s "$KEYCHAIN_PATH" $(security list-keychains -d user | tr -d '"')

# Import certs
security import /tmp/app.p12 -k "$KEYCHAIN_PATH" -P "$APPLE_P12_PASSWORD" -T /usr/bin/codesign
security import /tmp/inst.p12 -k "$KEYCHAIN_PATH" -P "$APPLE_P12_PASSWORD" -T /usr/bin/productsign -T /usr/bin/pkgbuild
security set-key-partition-list -S apple-tool:,apple: -s -k "$KEYCHAIN_PASSWORD" "$KEYCHAIN_PATH"

# Cleanup .p12 files
rm /tmp/app.p12 /tmp/inst.p12
```

The keychain is destroyed at the end of the CI run. Certs never persist on the runner.

### `codesign` command

Standard signing of an `.app`:

```bash
codesign --force --options runtime --timestamp \
  --entitlements installers/macos/resources/entitlements.plist \
  --sign "Developer ID Application: Pluryprint Project (TEAMID)" \
  Pluryprint.app
```

| Flag | Purpose |
|---|---|
| `--force` | Replace existing signature if any. |
| `--options runtime` | Enable Hardened Runtime — required for notarization. |
| `--timestamp` | Use Apple's timestamp authority. Same rationale as Windows: timestamp keeps signature valid after cert expiry. |
| `--entitlements <plist>` | Apply entitlements (capabilities the binary requests). |
| `--sign <identity>` | Cert subject. Quote because of spaces. |

**Sign inside-out:** sign all embedded frameworks and `.appex` bundles before signing the outer `.app`. `codesign --deep` exists but is deprecated and unreliable for complex bundles.

### Notarization with `notarytool`

Apple's notarization service scans the binary for malware and known security issues. As of macOS 10.15+, it is **mandatory** for Gatekeeper to allow apps from outside the Mac App Store.

```bash
xcrun notarytool submit Pluryprint-1.1.0.pkg \
  --apple-id "$APPLE_ID" \
  --team-id "$APPLE_TEAM_ID" \
  --password "$APPLE_APP_PASSWORD" \
  --wait
```

| Flag | Purpose |
|---|---|
| `--apple-id` | Your Apple Developer account email. |
| `--team-id` | The 10-character team ID from your Developer profile. |
| `--password` | An **App-Specific Password** (not your real Apple ID password) generated at appleid.apple.com. |
| `--wait` | Block until Apple finishes (5–30 min typically; up to several hours in queue spikes). |

After notarization succeeds, **staple** the ticket to the artifact:

```bash
xcrun stapler staple Pluryprint-1.1.0.pkg
xcrun stapler validate Pluryprint-1.1.0.pkg
```

Stapling embeds the notarization ticket in the artifact so Gatekeeper can verify offline. Without stapling, Gatekeeper has to phone home on first launch, which fails on offline machines.

Both `.pkg` and `.dmg` must be notarized and stapled separately. The `.app` inside does not need its own notarization if the wrapping installer is notarized.

### Verification

```bash
# Verify signature
codesign --verify --deep --strict --verbose=2 Pluryprint.app

# Verify Gatekeeper acceptance
spctl --assess --type execute --verbose Pluryprint.app
spctl --assess --type install --verbose Pluryprint-1.1.0.pkg

# Verify notarization stapled
stapler validate Pluryprint-1.1.0.pkg
```

All four commands must succeed before shipping.

## Cert and key storage

| Storage | Used for | Access control |
|---|---|---|
| **SSL.com Cloud HSM** | Windows EV cert private key | Username + password + TOTP, all in GitHub Secrets. Two maintainers know the recovery codes. |
| **Apple Keychain (CI runner)** | macOS Developer ID certs (decoded `.p12`) | Ephemeral keychain, destroyed per run. `.p12` source is GitHub Secret. |
| **GitHub Actions Secrets** | All passwords, TOTP secrets, base64-encoded `.p12` blobs, Apple ID credentials | Repo-scoped secrets, rotation requires manual maintainer action. Audit log via GitHub. |
| **1Password (offline)** | Recovery copies of all secrets | Two maintainers have access. Used only for recovery if GitHub Secrets are lost. |
| **Self-signed cert** | Nightly builds | Generated locally, stored in repo at `installers/windows/nightly-cert.pfx` (encrypted with a password also in GitHub Secrets). |

### Why not Azure Key Vault?

We considered Azure Key Vault Managed HSM ($1500+/month for HSM Premium tier) and rejected it on cost grounds. SSL.com's per-cert cloud HSM is much cheaper for our scale (one cert).

If the project grows large enough to warrant multiple signing identities (e.g. per-product), revisit Azure Key Vault or AWS CloudHSM.

## CI integration

In `.github/workflows/release.yml`:

```yaml
jobs:
  build-and-sign-windows:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v7
      - name: Build
        run: ./scripts/build-release.ps1
      - name: Sign with EV cert via SSL.com Cloud HSM
        env:
          SSL_COM_USERNAME: ${{ secrets.SSL_COM_USERNAME }}
          SSL_COM_PASSWORD: ${{ secrets.SSL_COM_PASSWORD }}
          SSL_COM_CREDENTIAL_ID: ${{ secrets.SSL_COM_CREDENTIAL_ID }}
          SSL_COM_TOTP_SECRET: ${{ secrets.SSL_COM_TOTP_SECRET }}
        run: ./scripts/sign-windows.ps1
      - name: Verify signature
        run: signtool verify /pa /v dist/Pluryprint-Setup.exe
      - uses: actions/upload-artifact@v7
        with:
          name: windows-x64-signed
          path: dist/

  build-and-sign-macos:
    runs-on: macos-14
    steps:
      - uses: actions/checkout@v7
      - name: Setup Apple certs
        env:
          APPLE_DEVELOPER_ID_APPLICATION_P12_BASE64: ${{ secrets.APPLE_DEVELOPER_ID_APPLICATION_P12_BASE64 }}
          APPLE_DEVELOPER_ID_INSTALLER_P12_BASE64: ${{ secrets.APPLE_DEVELOPER_ID_INSTALLER_P12_BASE64 }}
          APPLE_P12_PASSWORD: ${{ secrets.APPLE_P12_PASSWORD }}
        run: ./scripts/setup-apple-certs.sh
      - name: Build universal binary
        run: ./scripts/build-release-mac.sh
      - name: Sign and notarize
        env:
          APPLE_ID: ${{ secrets.APPLE_ID }}
          APPLE_TEAM_ID: ${{ secrets.APPLE_TEAM_ID }}
          APPLE_APP_PASSWORD: ${{ secrets.APPLE_APP_PASSWORD }}
        run: |
          ./installers/macos/codesign.sh
          ./installers/macos/notarize.sh dist/Pluryprint-1.1.0.pkg
          ./installers/macos/notarize.sh dist/Pluryprint-1.1.0.dmg
      - uses: actions/upload-artifact@v7
        with:
          name: macos-universal-signed
          path: dist/
```

Manual approval gate: before the upload-to-CDN step, the workflow pauses for a human approval (configured via GitHub Environments). This prevents accidental release pushes from a broken CI script.

## Self-signed cert for nightlies

Nightly builds use a self-signed cert generated locally. SmartScreen warnings are expected for these builds; nightly users are explicitly told to expect them.

Generation (one-time):

```powershell
$cert = New-SelfSignedCertificate `
  -Type CodeSigningCert `
  -Subject "CN=Pluryprint Nightly, O=Pluryprint Project, C=US" `
  -KeyAlgorithm RSA `
  -KeyLength 4096 `
  -HashAlgorithm SHA256 `
  -NotAfter (Get-Date).AddYears(10) `
  -CertStoreLocation Cert:\CurrentUser\My

$pwd = ConvertTo-SecureString -String "<password>" -Force -AsPlainText
Export-PfxCertificate -Cert $cert -FilePath nightly-cert.pfx -Password $pwd
```

The `.pfx` is committed (encrypted) to the repo at `installers/windows/nightly-cert.pfx`. The password is in GitHub Secrets as `NIGHTLY_CERT_PASSWORD`.

We **never** use the self-signed cert for stable or beta. Doing so would erode trust.

## Common pitfalls

### Windows

- **Missing timestamp:** signature expires when cert does, all distributed binaries break overnight. Always use `/tr` and verify the signature includes a counter-signature from the timestamp authority.
- **Wrong file digest:** `/fd SHA1` is deprecated and rejected by modern SmartScreen. Always use `/fd SHA256`.
- **Re-signing without `/as`:** by default `signtool sign` replaces existing signatures. Use `/as` (append signature) if you need to keep an existing signature (rare; we don't do this).
- **EV cert revocation without backup:** if the cert is revoked (compromise, CA error), all binaries signed with it become invalid. Mitigation: maintain an OV cert as warm spare.

### macOS

- **Forgetting `--options runtime`:** notarization fails. Hardened Runtime is required.
- **Forgetting to staple:** Gatekeeper works online but fails on offline machines. Always staple.
- **Using your real Apple ID password instead of an App-Specific Password:** authentication fails. Always use App-Specific Password.
- **Notarization rejection due to entitlement issues:** common ones include forgetting the network entitlement or having an entitlement that doesn't match the actual binary capabilities. Read the notarization log carefully (`xcrun notarytool log <submission-id>`).
- **Architecture mismatch:** signing an x86_64 binary with a key configured for arm64 (or vice versa) — codesign accepts but verification later fails. Always sign Universal binaries with `codesign` directly on the Universal artifact, not on individual slices.

## Cost summary

| Item | Annual cost | Notes |
|---|---|---|
| Windows EV cert (SSL.com Cloud HSM) | ~$300 | Required for stable + beta. |
| Apple Developer Program | $99 | Covers all macOS cert types. |
| Cloudflare R2 hosting (signed binaries) | ~$50 | Storage + zero egress. |
| **Total** | **~$450/year** | Covers all signing infrastructure. |

This is the floor. We may add an OV cert as warm spare ($100/year) once the project is mature enough that an EV revocation would be catastrophic.

## See also

- [installer-windows.md](./installer-windows.md)
- [installer-macos.md](./installer-macos.md)
- [auto-update-flow.md](./auto-update-flow.md)
- [ci-cd.md](./ci-cd.md)
