# Privacy Policy

> **TL;DR.** Pluryprint does not collect telemetry, does not phone home, does not transmit your file contents, and does not have user accounts. The only network call it makes by default is an update **check** that reads a small manifest (`latest.json`) from Pluryprint's GitHub releases — no binary, no identifiers, no payload. An update is downloaded only when you choose **Install now** (or run `pluryprint update install`). **This version transmits nothing else at all: there is no crash reporter and no usage-metrics reporter in the shipped code**, opt-in or otherwise. Logs stay on your machine.

This document describes what data Pluryprint collects, transmits, stores, and shares — which is nearly nothing. It is current as of **2026-11-09** and applies to all installed versions of Pluryprint on Windows and macOS, and on Linux (a preview platform from 0.0.5).

## Our principles

1. **Minimum necessary collection.** We do not collect data we do not need.
2. **No telemetry by default.** Anything we do collect is opt-in.
3. **File contents never leave your machine.** Period.
4. **Local-first.** Logs, profiles, and settings stay on your device.
5. **No user accounts.** We don't know who you are. We don't want to know.

## What we do **not** collect

The list of things we explicitly do not do:

- We do not have analytics. No Google Analytics, no Mixpanel, no Amplitude, no self-hosted equivalent.
- We do not track installs. We don't ping a server when you install or uninstall.
- We do not generate per-install IDs or fingerprints.
- We do not track which features you use.
- We do not track which printers you have.
- We do not track which file types you print.
- We do not transmit file contents anywhere, ever.
- We do not transmit file names.
- We do not transmit profile names or contents.
- We do not collect crash data. This version has no crash reporter at all (see below).
- We do not have user accounts. There is no sign-in, no email collection.
- We do not use cookies (the app is not a website; the webview only loads our local bundle).
- We do not embed third-party tracking scripts in the app.
- We do not transmit information to third-party advertisers, brokers, or analytics providers.
- We do not sell or rent any data because we do not collect any data to sell or rent.

## Network calls we **do** make (by default)

There is exactly one kind of outbound network call Pluryprint makes by default, and it goes to GitHub — **not** to any Maecly-operated server. Pluryprint does not run an update server; there is no `updates.maecly.com`.

### Update check

- **Endpoint:** the updater manifest of your update channel, compiled into the binary: `https://github.com/MAECLY/pluryprint/releases/latest/download/latest.json` for **Stable**, `https://github.com/MAECLY/pluryprint/releases/download/channel-beta/latest.json` for **Beta** and **Nightly**. GitHub answers with a redirect to its own download host. That is the only URL the automatic check requests; it no longer calls the GitHub REST API (`api.github.com`).
- **Frequency:** at most once every 24 hours, and only 30 seconds after launch — never during startup. Plus on demand when you press "Check for updates" or run `pluryprint update check`.
- **Request payload:** none. It is a plain unauthenticated HTTPS GET. No account, no token, no install id.
- **Request headers:** `User-Agent: Pluryprint/<version>` and `Accept: application/json`. The user agent carries the app version and nothing else — no OS string, no identifiers.
- **Proxy:** if `HTTPS_PROXY` / `HTTP_PROXY` (or their lowercase forms) are set in your environment, the request goes through that proxy. Nothing else about your environment is read.
- **What is downloaded:** only the manifest — the version, the release notes, and for each platform the address and signature of its update file. The check never downloads a binary and never installs anything.
- **What GitHub sees:** whatever any unauthenticated HTTPS request to `github.com` exposes — your IP address, the user agent above, and the requested URL. GitHub's privacy policy governs that; we operate no server in this path and therefore keep no logs of it.
- **What you can disable:** the automatic check, via Settings → Updates (`[updater].check_on_startup`). The on-demand button still works when you want it.

### Installing an update (only when you ask)

- **When:** only after you press **Install now** on the About screen or run `pluryprint update install`. Nothing is installed automatically, and a major version asks you a second time.
- **Endpoint:** the same manifest as the check, then the installer file it names, both served by GitHub. The request carries the updater's own user agent and no identifiers.
- **Verification:** the file is checked against Pluryprint's release signature before it is installed. A file that fails the check is discarded and nothing on your computer changes.

### Things this version does **not** do over the network

- It does not download or install updates unless you ask it to (see above).
- It does not fetch fonts, scripts, styles or images from any CDN — the webview loads only the bundled app, and the one font it uses ships inside the bundle.
- It does not check for updates of external converters.
- It does not install a missing converter on its own. **Install…** on the Print Job screen shows the exact command (`winget`, `brew`), Store page or download page first, and runs or opens it only after you confirm; winget, Homebrew, the Microsoft Store or your browser then make their own requests. On Linux it only ever offers a web page, which your browser opens.
- A file being printed cannot reach the network: HTML, Markdown, text and XML pages are printed with scripts off and every request refused (an XML file's DTD is never read, so it cannot point at a file or a server either), SVG drawings load nothing remote, and LibreOffice's private profile blocks linked pictures and objects and points its own HTTP at a dead proxy. On Windows none of them opens a file share either, so printing a document cannot hand your Windows credentials to another machine.
- It does not contact printer manufacturers' cloud services.
- It does not contact `maecly.com` on its own. The **About** screen and the "report an issue" button open links in your normal browser when *you* click them; that is your browser making the request, not the app, and only `github.com`, `maecly.com` and `*.maecly.com` over HTTPS are accepted as link targets.

## Telemetry settings that record consent but send nothing

Settings → Privacy exposes two switches, `Send anonymized crash reports` and `Send anonymous usage metrics`. Both default to **off**, and in this version **both are inert**: the code that would transmit anything does not exist. There is no crash reporter, no Sentry integration, and no metrics ping in the shipped binary — the flags exist only so your choice survives restarts and a future build can honour it. This is stated in the code itself, on the `TelemetryCfg` type.

If either is ever wired up, it will be:

- Off by default.
- Clearly disclosed in this document.
- Toggleable in Settings.
- Anonymous and aggregated where possible.

We will **never** flip a default from off to on without a major version bump and clear communication.

## Data we store on your machine

These are the real file names and locations, as resolved by `src-tauri/src/util/paths.rs`. There is no `settings.toml`, no `profiles/` folder and no `update-cache/` — earlier drafts of this document named files that never existed.

| Data | Location (Windows) | Location (macOS) | Contents |
|---|---|---|---|
| Settings | `%APPDATA%\Maecly\Pluryprint\config\config.toml` | `~/Library/Application Support/com.Maecly.Pluryprint/config.toml` | Language, theme, engine limits, update channel, printer preferences, the two consent flags. |
| Profiles | `%APPDATA%\Maecly\Pluryprint\config\profiles.toml` | `~/Library/Application Support/com.Maecly.Pluryprint/profiles.toml` | One TOML file holding the print presets **you** created. The three built-in presets live in code and are never written to disk. |
| Print queue | `%LOCALAPPDATA%\Maecly\Pluryprint\data\queue.json` | `~/Library/Application Support/com.Maecly.Pluryprint/queue.json` | The live queue, so it survives a restart. Contains the **paths and names of files you queued**, their sizes and page counts. |
| Update bookkeeping | `%LOCALAPPDATA%\Maecly\Pluryprint\data\update-state.json` | `~/Library/Application Support/com.Maecly.Pluryprint/update-state.json` | Two fields: when the last update check ran, and which version you chose to skip. |
| Converted PDFs | `%LOCALAPPDATA%\Maecly\Pluryprint\cache\conversions\` | `~/Library/Caches/com.Maecly.Pluryprint/conversions/` | Temporary PDFs produced while printing non-PDF files. Swept by the app; deleting them by hand is safe. |
| Shell-extension handoff | `%LOCALAPPDATA%\Maecly\Pluryprint\data\handoff\` | `~/Library/Application Support/com.Maecly.Pluryprint/handoff/` | Short-lived JSON files listing the paths you right-clicked. Consumed immediately; leftovers are swept at the next launch. Kept under your own user directory rather than a shared temp folder so another user cannot read or plant one. |
| Logs | `%LOCALAPPDATA%\Maecly\Pluryprint\data\logs\pluryprint-<date>.jsonl` | `~/Library/Logs/Pluryprint/pluryprint-<date>.jsonl` | JSON-lines tracing output. **May include file paths** of jobs you printed. Rolls on a new UTC day or at 10 MB, whichever comes first; a sweep at startup and on every roll deletes anything older than 14 days and then keeps only the 7 newest files. |
| Enterprise policy (read-only) | `%ProgramData%\Pluryprint\policy.json` | `/Library/Application Support/Pluryprint/policy.json` | Provisioned by an administrator; the app only ever reads it. |

On **Linux** the same files follow the XDG Base Directory specification (each folder moves with its `XDG_CONFIG_HOME`, `XDG_DATA_HOME` or `XDG_CACHE_HOME` variable):

| Data | Linux |
|---|---|
| Settings, profiles | `~/.config/pluryprint/config.toml`, `profiles.toml` |
| Print queue, update bookkeeping | `~/.local/share/pluryprint/queue.json`, `update-state.json` |
| Converted PDFs | `~/.cache/pluryprint/conversions/` |
| Logs | `~/.local/share/pluryprint/logs/pluryprint-<date>.jsonl` |
| Enterprise policy (read-only) | `/etc/pluryprint/policy.json` |
| The window's web view data | `~/.local/share/com.maecly.pluryprint/` (WebKitGTK's own storage, cache and HSTS list for the app's window; Pluryprint loads only its own pages there) |

The AppImage and the `.deb` use the same folders, and removing either package leaves them in place; delete them by hand to remove every trace.

If you used the app under its former name, MaeclyPrint, the same files lived in folders named `MaeclyPrint` (for example `~/Library/Application Support/com.Maecly.MaeclyPrint/`). The first launch of Pluryprint moves them to the locations above, on your machine, without sending anything anywhere, and never overwrites a file that is already there.

This data stays on your device. We never transmit it. You can inspect or delete any of it manually at any time. On Windows, uninstalling the app prompts you to delete this data; the default is to preserve it (in case you reinstall). Removing the macOS app or a Linux package never deletes it.

## File contents

When Pluryprint prints a file:

- The file is read from disk by Pluryprint.
- If it is not already a PDF, it is converted to one — images are converted **inside Pluryprint itself** (no external tool involved). HEIC photos are decoded by your operating system (`sips` on macOS, Windows Imaging Component on Windows) or, on Linux, by a decoder you installed (libheif's `heif-dec` or `heif-convert`, or ImageMagick), run as a normal subprocess under your user account. SVG drawings, Markdown and plain text are also converted inside Pluryprint. HTML, Markdown and text pages are paginated by your operating system's own web view (WebView2 on Windows, WebKit on macOS, WebKitGTK on Linux) in a hidden Pluryprint worker process. Office and OpenDocument files (and RTF on Windows and Linux) are handed to LibreOffice, installed on your machine, which runs as a normal subprocess under your user account with a private, temporary profile that is deleted afterwards.
- A **PDF password** you type is used only to decrypt that file for that print job. It is never written to disk, to a log or to a command line, and is forgotten when the job ends; the decrypted copy lives in Pluryprint's cache and is deleted with the job.
- The resulting PDF is handed to the printing backend: SumatraPDF on Windows, the CUPS `lp` command on macOS/Linux.
- The rendered output is sent to your printer (via the OS print spooler).

At no point is the file content transmitted over the network. The print spooler may keep the rendered output in a temp folder briefly (this is OS behavior, not ours), then deletes it after the job completes.

For network printers: the rendered output is sent **to the printer**, over your local network or via the printer's own network connection. This is necessary for printing to work. We do not also send it to anyone else.

## Children's privacy

Pluryprint is a desktop utility for printing files. We do not knowingly collect data from children under 13 because we do not knowingly collect data from anyone. If you are a parent and have concerns about your child using Pluryprint, the answer is: there is no data flow that reaches us at all.

## GDPR posture

The General Data Protection Regulation (GDPR) governs the processing of personal data of individuals in the European Union. Pluryprint:

- Does not process personal data, because we do not collect any. We operate no server that Pluryprint contacts.
- Does not require user accounts or identifiers.
- Does not transfer personal data outside the EU because we don't have any to transfer.
- Does not use personal data for profiling, automated decision-making, or marketing.

The one network call the app makes goes to GitHub, a third party we do not control and receive nothing from. GitHub is the data controller for its own access logs; its privacy policy applies. We hold no copy of them.

If you have a GDPR data subject request, contact the email in `SECURITY.md`. In practice there is nothing for us to produce or erase, because we hold no data about you; we will say so in writing within 30 days.

## CCPA / state-level US privacy laws

The California Consumer Privacy Act (CCPA) and similar state laws (Virginia VCDPA, Colorado CPA, Connecticut CTDPA, Utah UCPA) grant rights regarding personal information. Same posture as GDPR: we collect no personal information at all, so the rights to access, delete, opt out of sale, etc. all have nothing to act on. We do not sell data; we don't have data to sell.

## Third-party services

We use these services in the operation of the project. None of them receive user data from Pluryprint runtime by default; they are part of our infrastructure or development pipeline.

| Service | What for | Data they see |
|---|---|---|
| GitHub | Source hosting, CI/CD, releases — **and the update check the app performs** | Source code, CI logs (no user data). For the update check and for any release you download, GitHub sees the standard HTTP request including your IP and the `Pluryprint/<version>` user agent. |
| Apple (during macOS use) | Notarization checks (Gatekeeper phones home on first launch) | Apple sees the app bundle hash + your IP. This is OS behavior, not ours. Stapled tickets allow offline verification once notarized. |
| Microsoft (during Windows use) | SmartScreen reputation lookup at install time | Microsoft sees the installer hash + your IP. This is OS behavior, not ours. |

We do not operate a CDN, an update server, or a crash-report endpoint. Earlier drafts of this document listed Cloudflare R2 and a self-hosted Sentry instance; neither is used by the shipped app.

We do **not** use Google Analytics, Facebook Pixel, ad networks, marketing platforms, or any other consumer-tracking services.

## Security

Although unrelated to privacy directly, security is the foundation of any privacy claim. Brief notes:

- Update files are signed with Pluryprint's own key, and the app installs only a file whose signature matches the public key built into it. The installers themselves carry no Apple or Microsoft code-signing certificate (see [code-signing.md](../07-build-and-release/code-signing.md)); each release lists the SHA-256 of every file so you can check a download.
- The app replaces its own code only when you choose **Install now** (or run `pluryprint update install`), and only with a signed update — see [auto-updater.md](../04-architecture/auto-updater.md).
- We disclose vulnerabilities responsibly per `SECURITY.md`.
- We do not bundle remote-code-execution tools or scripting engines that would let an update silently change app behavior beyond what the new version does.

## Logging on our infrastructure

Pluryprint contacts no server we operate, so there is no application traffic for us to log. Any web-server logging that applies to the project website (`maecly.com`) is a property of that website and not of the app; the app never requests it.

Release downloads and the update check are served by GitHub, whose logging practices are governed by GitHub's own privacy policy. We have no access to per-request logs for them.

## Children and accessibility

Privacy considerations for accessibility tools: if you use a screen reader, eye tracker, or other assistive technology with Pluryprint, that interaction is between you and the assistive technology. Pluryprint does not log, transmit, or store information about the use of assistive technology.

## Changes to this policy

If we change this policy in any way that adds collection or sharing, we will:

1. Announce the change in the release notes for the version that introduces it.
2. Bump to a new MAJOR version (per SemVer rules; this is a behavioral change to a documented promise).
3. Show a clear in-app notification on first launch of the new version asking the user to acknowledge.
4. Update this document with a "Changes" section dating the previous version.

We will **never** make a privacy-affecting change quietly. The principle: if you're surprised by something Pluryprint did with your data, we failed.

For changes that *reduce* collection (e.g. removing an opt-in feature, reducing log retention), we may make the change in a normal MINOR or PATCH release without ceremony.

## Contact

For privacy questions or concerns:

- Open an issue on GitHub tagged `privacy` for general questions.
- Email the security address in `SECURITY.md` for sensitive matters or formal requests (GDPR, CCPA, etc.).

## Version history of this policy

| Version | Date | Changes |
|---|---|---|
| 1.2 | 2026-10-10 | The update check reads the channel's `latest.json` manifest on `github.com` instead of `api.github.com`; documents the consented dependency install, the no-network printing of documents, the PDF password, and signed in-app updates in place of "no in-app updater". |
| 1.1 | 2026-07-21 | Corrected to match the shipped code: the update check targets `api.github.com`, not `updates.maecly.com`; removed the description of a crash reporter and a metrics ping that do not exist (the consent flags are inert); replaced the local-storage table with the real file names and paths; removed Cloudflare and Sentry from third-party services. |
| 1.0 | 2026-05-03 | Initial policy. |

## See also

- [license.md](./license.md)
- [third-party-licenses.md](./third-party-licenses.md)
- [trademark.md](./trademark.md)
- [../07-build-and-release/auto-update-flow.md](../07-build-and-release/auto-update-flow.md)
