# ADR-0015: Linux x86_64 and ARM64 builds, shipped as AppImage and .deb

## Status

Accepted. Supersedes decision D20 ("Linux is out of scope", recorded in [ADR-0011](./0011-release-channel.md)) and the "Linux (M6+)" deferral in [ADR-0007](./0007-windows-first-mac-second.md). Linux is labelled **preview** in the docs and on the landing page until the builds have been tested on real hardware; the CI lanes below are not that test.

## Date

2026-11-09

## Context

The round-3 decisions (2026-10-09) left Linux out of scope (D20), and ADR-0007 had pushed it to "M6+" because every Linux file manager has its own extension model. Since then:

- **The print backend already runs on Linux.** `engine::print` sends every `cfg(unix)` build to `platform::cups`, the same `lp` / `lpstat` / `lpoptions` / `lpadmin` / `ippfind` code macOS uses. Discovery, add-printer, job status and cancel are CUPS command-line calls with no macOS API behind them. There is no `target_os = "linux"` gate anywhere in the crate; the Linux build is the `all(unix, not(target_os = "macos"))` combination of branches that Windows and macOS already compile.
- **Two converters had nothing behind them on Linux.** The render worker that prints HTML, Markdown and text ([ADR-0012](./0012-system-web-view-renderer.md)) had a Windows and a macOS backend only, and returned "rendering HTML is supported on Windows and macOS only". HEIC/HEIF answered `no-heif-decoder`, because the OS decoders the app relies on (`sips`, WIC) do not exist there. RTF on macOS goes through `textutil`; on Linux it falls to LibreOffice, like Windows. So 7 of the 27 accepted extensions (`txt html htm md markdown heic heif`) would have failed at conversion time.
- **One resource guarantee was weaker on Linux.** The memory cap of `process::run_capped` ([ADR-0014](./0014-svg-worker-resource-boundary.md)) counted only the process-group leader's resident set on Linux, so a child of a capped tool could grow past the cap unseen. The `procps-ng` `kill` on Ubuntu 22.04 also reads `kill -KILL -<pgid>` as a second signal option and signals the caller's own group; the engine's process-group kills had to stop relying on it.
- **Tauri 2 builds AppImage and .deb natively, and its updater installs both.** `tauri-plugin-updater` 2.14 looks for a `<os>-<arch>-<installer>` manifest key before `<os>-<arch>`, replaces an AppImage in place and installs a `.deb` with `pkexec dpkg -i`.
- **The web view sets a floor.** wry 0.57 enables `webkit2gtk` with the `v2_40` feature, but imports a 2.41.90 symbol (`webkit_cookie_manager_get_all_cookies`) and the binary binds every symbol at start, so the app needs WebKitGTK 4.1, version 2.42 or later, at run time; the `.deb` depends on `libwebkit2gtk-4.1-0 (>= 2.42)` and `libc6 (>= 2.35)`.
- **The repository is public,** so GitHub's free `ubuntu-22.04-arm` runners are available for native ARM64 builds and tests.
- Issue #66 ("Linux version (coming soon)") records the demand.

## Decision

We will build, test and publish **Linux x86_64 and ARM64** from the next stable release (0.0.5), with the same feature set as Windows and macOS.

- **Packages.** An **AppImage** and a **.deb** for each architecture, and no `.rpm`. Asset names follow the existing label scheme: `Pluryprint_<version>_linux_x64.AppImage`, `Pluryprint_<version>_linux_x64.deb`, `Pluryprint_<version>_linux_arm64.AppImage`, `Pluryprint_<version>_linux_arm64.deb` (labels `linux_x64` and `linux_arm64`).
- **Baseline.** Both architectures are built on the `ubuntu-22.04` and `ubuntu-22.04-arm` runners: glibc 2.35 and WebKitGTK 4.1 (2.42 or later). The supported minimum is **Ubuntu 22.04, Debian 12 or Fedora 38, or later**, with CUPS. The app is meant to work in X11 and Wayland sessions alike (nothing in it depends on the desktop environment except the file-manager entries below); only X11, under Xvfb, has been run so far.
- **Format parity.** The render worker gets a **WebKitGTK** backend that prints the staged page to PDF (`pluryprint --worker render-pdf`, as on the other two systems). It keeps ADR-0012's contract: scripts off, nothing loaded from the network or a file share, and every sub-resource answered by the worker itself from the document's folder. **HEIC/HEIF** is decoded by an **external decoder found at run time**: libheif's `heif-dec` (libheif 1.17 or later) or `heif-convert` (the `libheif-examples` package; Ubuntu 22.04 ships 1.12, which has only `heif-convert`), else ImageMagick. A missing decoder is a dependency like LibreOffice: the job pauses, the dependency report names it, and the app shows how to install it. **RTF** goes through LibreOffice.
- **Installing dependencies.** On Linux the install hint is a **web link** (the LibreOffice download page; for a HEIF decoder, the libheif project, while the message itself names each distribution's package); the app runs no package manager and there is no PackageKit route. The `.deb` declares what it can instead: `Depends: cups-client, libc6 (>= 2.35), libwebkit2gtk-4.1-0 (>= 2.42)` and `Recommends: cups-ipp-utils, libheif-examples`. LibreOffice would be a `Suggests`, but Tauri's `DebConfig` cannot set one, so the dependency screen's link is the only pointer to it.
- **File-manager integration.** The `.deb` installs a `.desktop` entry whose `MimeType=` lists the supported types, so **Open with → Pluryprint** works in any file manager; the AppImage carries the same entry but installs nothing, so it shows under Open with only once an AppImage integrator registers it. The `.deb` also installs a **Dolphin** (KDE) service menu and **Nemo** (Cinnamon) actions with **Print now**, **Configure print job** and **Add to print queue**. There is no Nautilus extension (it would need `nautilus-python`), and the AppImage does not register itself in the user's file manager.
- **Updates.** The AppImage updates itself through the manifest keys `linux-x86_64` and `linux-aarch64`, which point at the `.AppImage`. A `.deb` install updates in the app too, through `linux-x86_64-deb` and `linux-aarch64-deb`, which point at the `.deb`: the updater runs `pkexec dpkg -i`, so the system asks for an administrator password. All four keys are signed by the release's sign job and are required by `release.yml` and `release-channels.yml`, so a manifest without them is never published.
- **Trust.** Linux files carry no distribution or GPG signature. They are covered by `SHA256SUMS` like every other download, and updates by the project's minisign key. There is no apt or dnf repository, and no Flatpak, Snap or AUR package.
- **Memory cap.** On Linux the parent sums the resident set of **every process in the capped process group**, read from `/proc`, so ADR-0014's cap holds for a tool's children too. Process groups are signalled with `killpg` or `kill -<sig> -- -<pgid>`, never with the ambiguous `kill -<sig> -<pgid>` form.
- **CI.** The pull-request gate (`ci.yml`) adds `ubuntu-22.04` to its backend matrix with the macOS weight (clippy and `cargo test --lib`; `cargo fmt` runs on the Windows leg only, and covers every file). A separate `linux.yml` runs the full `cargo test` on `ubuntu-22.04-arm` under Xvfb with a `cups-pdf` print queue, on pushes to `main`, weekly and by hand; never on pull requests.
- **Licences.** The AppImage bundles LGPL-2.1-or-later libraries (WebKitGTK, GTK, GLib, libsoup and their dependencies), a few GPL-2.0-or-later and LGPL-3.0-or-later ones (libjbig, Nettle, libunistring, libidn2, elfutils), and the AppImage runtime with libfuse linked in statically. `THIRD-PARTY-NOTICES.md` says so, points at their Corresponding Source (the release's sources list names each Ubuntu source package and the runtime commit) and carries a written offer. The `.deb` bundles none: it depends on the distribution's copies. The `cargo audit` exceptions for the gtk-rs 0.18 advisories (RUSTSEC-2024-0429, RUSTSEC-2024-0370) are re-justified, because GTK now ships in a Pluryprint build.
- **Status.** Linux is **preview** wherever a status is given, per architecture, until each build has run on real hardware. That label is about testing, not about missing features.

## Alternatives Considered

| Option | Why rejected |
|---|---|
| Keep Linux out of scope (D20) | The print backend and most of the engine already compile and run there; the cost is packaging, two converters and the file-manager glue, not a port. |
| Ship with HTML, Markdown, text and HEIC marked unsupported on Linux | A Linux user would hit a different format list from the one the landing page and the app promise everywhere else; parity was chosen over an earlier release. |
| `.deb` only | Leaves out Fedora, Arch and others without a package; the AppImage runs on any distribution with glibc 2.35 and WebKitGTK 2.42. Avoiding the LGPL source pointer does not outweigh that. |
| Also ship `.rpm` | A third package format to test and update for no user who cannot already run the AppImage. |
| Flatpak, Snap or a distribution repository | A sandbox that would hide CUPS and the converters behind portals, or a repository to host and sign; out of proportion for a preview. |
| AppImage-only updates, with `.deb` users told to download | A `.deb` install without a `-deb` key downloads the AppImage and fails to install it. Publishing the key costs one password prompt per update. |
| A PackageKit (`pkcon`) install route | A new install method, consent flow and failure modes for one optional program per distribution; a web link and the `.deb`'s `Recommends` cover it. |
| A Nautilus extension, or AppImage self-registration | `nautilus-python` is not installed by default, and an AppImage writing into `~/.local/share` on its own is a surprise; "Open with" covers both for now. |
| `ubuntu-24.04` as the baseline | glibc 2.39 would exclude Ubuntu 22.04 and Debian 12. |
| Gate the memory-cap test to macOS | Leaves a real gap in the resource boundary on Linux. |

## Consequences

### Positive

- Every format prints on every supported system, and the same CUPS backend serves macOS and Linux.
- ARM64 Linux is built and tested natively, not cross-compiled.
- The memory cap now covers a capped tool's children on Linux as it does on Windows (Job Object) and macOS.

### Negative

- Six builds to release instead of four: each release gains ten files (four packages, their four update signatures and two AppImage source lists) and four manifest keys, and the landing page four downloads (eight in all).
- A third web engine to know: WebKitGTK paginates like neither WebView2 nor WKWebView in every case, so a page can break differently on Linux.
- A `.deb` update asks for an administrator password every time, and installing the `.deb` needs `sudo`, unlike the per-user Windows installer.
- AppImage users have no `pluryprint` on `PATH`: the command line is the AppImage file itself (`./Pluryprint_<version>_linux_x64.AppImage print …`).
- The AppImage is much larger than the other downloads, because it carries WebKitGTK and GTK.
- The AppImage's WebKitGTK, libsoup and GnuTLS are frozen at the versions of the release that built them: the distribution's security updates do not reach them, although the render worker parses untrusted HTML with that WebKitGTK and the updater talks TLS through that stack. They are only as current as the latest Pluryprint release; on Debian and Ubuntu the `.deb`, which uses the system's copies, is the safer choice.

### Risks / Open Questions

- **The first Linux release cannot prove its own updater.** There is no earlier Linux build to update from; the in-app update path is first exercised by the release after it.
- **Known WebKitGTK traps,** to check on real hardware: Ubuntu 24.04's AppArmor limit on unprivileged user namespaces can break WebKit's `bubblewrap` sandbox inside an AppImage; NVIDIA drivers on Wayland can show a blank window (`WEBKIT_DISABLE_DMABUF_RENDERER=1` works around it); the Wayland `app_id` must match the `.desktop` file name or the dock shows a generic or duplicate icon; kernels with 16K pages (Asahi) can break an AppImage that bundles Ubuntu ARM64 libraries.
- **AppImage and FUSE.** An AppImage mounts itself through FUSE. The static type2-runtime the bundler uses links libfuse 3 in and needs only the `fusermount3` program (package `fuse3`, installed by default on Ubuntu 22.04 and later, Debian 12 desktops and Fedora), not `libfuse2`; without it, `--appimage-extract-and-run` still works.
- **LibreOffice from Snap or Flatpak** may not see the conversion folder under `/tmp`; the distribution's or the official package does.
- **Runner lifetime.** GitHub has marked the `ubuntu-22.04` and `ubuntu-22.04-arm` images deprecated (reported as from 17 September 2026, with scheduled brownouts and removal on 17 April 2027; check actions/runner-images issue #14254 for the dates). A tag pushed during a brownout fails the Linux builds and blocks the draft; re-run them afterwards. Before the removal the build jobs must move to `ubuntu-24.04`/`ubuntu-24.04-arm` running an `ubuntu:22.04` container, which keeps the glibc 2.35 floor, or the minimum must rise.

## Related

- [ADR-0007](./0007-windows-first-mac-second.md) — the platform order this extends.
- [ADR-0011](./0011-release-channel.md) — the release channel the Linux files join; its D20 is superseded here.
- [ADR-0012](./0012-system-web-view-renderer.md) — the render worker contract the WebKitGTK backend keeps.
- [ADR-0014](./0014-svg-worker-resource-boundary.md) — the memory cap whose Linux half this completes.
- [`../07-build-and-release/installer-linux.md`](../07-build-and-release/installer-linux.md) — the packages, in detail.
