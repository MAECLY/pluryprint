# Installer (Linux)

> **TL;DR.** From 0.0.5, every release ships Linux x64 and ARM64 as a **preview**: an AppImage and a `.deb` per architecture (`Pluryprint_X.Y.Z_linux_x64.AppImage`, `…_linux_x64.deb`, `…_linux_arm64.AppImage`, `…_linux_arm64.deb`), built natively on GitHub's Ubuntu 22.04 runners. They run on Ubuntu 22.04, Debian 12, Fedora 38 or later (glibc 2.35 and WebKitGTK 2.42 or later). The `.deb` depends on the CUPS client tools and installs **Open with Pluryprint** plus the Dolphin and Nemo actions; the AppImage runs anywhere without installing. Both update in the app: the AppImage replaces its own file, the `.deb` runs `pkexec dpkg -i`. No `.rpm`, no apt repository, no Flatpak or Snap. "Preview" means built and checked in CI, not yet tested on real hardware.

This is the Linux counterpart of [installer-windows.md](./installer-windows.md) and [installer-macos.md](./installer-macos.md). The release runbook is [releasing.md](./releasing.md); the decision to ship Linux at all is the ADR that supersedes decision D20.

## Targets

| Distribution | Architecture | Package | Built and checked in CI | Status |
|---|---|---|---|---|
| Ubuntu 22.04 LTS, 24.04 LTS and later | x86_64, aarch64 | `.deb` or AppImage | Yes (`ubuntu-22.04`, `ubuntu-22.04-arm`) | Preview |
| Debian 12 and later, Linux Mint 21 and later, Pop!\_OS 22.04 and later | x86_64, aarch64 | `.deb` or AppImage | Built on Ubuntu, not run | Preview |
| Fedora 38 and later, openSUSE, Arch and others | x86_64, aarch64 | AppImage | Built on Ubuntu, not run | Preview |
| Ubuntu 20.04, Debian 11, RHEL 8 and older | any | none | No | Not supported |

The floor comes from the build machine, not from a setting:

- **glibc 2.35.** The binaries are linked on Ubuntu 22.04, so they need its glibc or a newer one. Building on 24.04 would raise the floor to 2.39 (and make linuxdeploy's `strip` fail on its `.relr.dyn` sections).
- **WebKitGTK 2.42.** wry 0.57 enables the `webkit2gtk/v2_40` feature, but it also imports `webkit_cookie_manager_get_all_cookies`, new in 2.41.90, and the binary binds every symbol at start, so 2.42 is the real floor: an older WebKitGTK stops the app before it opens a window. Ubuntu 22.04 has 2.42 or later through `jammy-updates` (the runner image had 2.50.4 in November 2026), and so do Debian 12 and Fedora 38 through their updates. The `.deb` says so: `Depends: cups-client, libc6 (>= 2.35), libwebkit2gtk-4.1-0 (>= 2.42)` (tauri adds the unversioned WebKitGTK and GTK entries too).
- **WebKitGTK 4.1 API** (`libwebkit2gtk-4.1-0`, the libsoup 3 build), the one Tauri 2 uses.

## What ships

### The `.deb`

`pnpm tauri build --bundles deb` makes `Pluryprint_<version>_<arch>.deb` (`amd64` or `arm64`); `release_tools.py stage-bundle` renames it `Pluryprint_<version>_linux_<x64|arm64>.deb`. Measured on ARM64 with 0.0.4 sources: 12.67 MiB, 26 MB installed.

```
Package: pluryprint
Section: utils
Depends: cups-client, libwebkit2gtk-4.1-0, libgtk-3-0
Recommends: cups-ipp-utils, libheif-examples
Description: Batch file printing
```

| Path | What |
|---|---|
| `/usr/bin/pluryprint` | the app; also the CLI (`pluryprint print …`) |
| `/usr/bin/pluryprint-cli` | the same CLI under the name Windows uses for its console front end (on macOS the app binary itself is the CLI) |
| `/usr/lib/Pluryprint/` | `LICENSE`, `LICENSES/`, `THIRD-PARTY-NOTICES.md` (`bundle.resources`), and the `resources/sumatrapdf` and `resources/shellext` READMEs, which every non-Windows bundle carries |
| `/usr/share/applications/Pluryprint.desktop` | from `src-tauri/linux/pluryprint.desktop.hbs` (`bundle.linux.deb.desktopTemplate`): the menu entry and **Open with Pluryprint** (`MimeType=`, `Exec=pluryprint %F`) |
| `/usr/share/icons/hicolor/{32x32,64x64,128x128,512x512}/apps/pluryprint.png` | `bundle.icon` |
| `/usr/share/kio/servicemenus/pluryprint.desktop` | the Dolphin service menu (KDE Frameworks 6, and KF5 from 5.85) |
| `/usr/share/nemo/actions/pluryprint-{print-now,configure,queue}.nemo_action` | the Nemo (Cinnamon) actions |

The dependencies are `bundle.linux.deb` in `tauri.conf.json`; tauri adds the WebKitGTK and GTK libraries to `Depends` on its own:

- **`cups-client`** (Depends): every print runs `lp`, `lpstat`, `lpoptions` and `cancel`. Without them the printer list is empty.
- **`cups-ipp-utils`** (Recommends): `ipptool` and `ippfind`, for the IPP queries. apt installs recommended packages by default; a user who turned that off still prints.
- **`libheif-examples`** (Recommends): `heif-convert`, the HEIC/HEIF decoder. On Ubuntu 22.04 it is libheif 1.12 and has no `heif-dec` (that arrived in 1.17); the app looks for either, and ImageMagick as a fallback.
- **LibreOffice** would be a `Suggests`, for Word, Excel, PowerPoint and OpenDocument files. Tauri 2's `DebConfig` has no `suggests` field (only `depends`, `recommends`, `provides`, `conflicts`, `replaces`), so the package does not name it; the app's dependency screen links to the download instead, as it does on Windows and macOS.

No maintainer scripts: the system's own dpkg triggers update the desktop, MIME and icon caches.

### The AppImage

`--bundles appimage` makes `Pluryprint_<version>_<x86_64|aarch64>.AppImage`, renamed `Pluryprint_<version>_linux_<x64|arm64>.AppImage`. Measured on ARM64: 84.68 MiB. It is large because linuxdeploy and its GTK plug-in copy WebKitGTK, GTK, GLib, libsoup, GStreamer's base libraries and the rest of their tree (124 Ubuntu packages on ARM64) into it, so it runs on distributions whose own versions differ. `bundleMediaFramework` is off: nothing plays audio or video, so the extra GStreamer plug-ins (15-35 MB) stay out. It carries no Dolphin or Nemo files (`deb.files` is for the `.deb` only) and does not register itself with the desktop in this release.

Running it needs FUSE, but not `libfuse2`: the bundler now starts every AppImage with the static type2-runtime, which has libfuse 3 linked in and needs only a `fusermount3` (or `fusermount`) program on `PATH`. That is the `fuse3` package, installed by default on Ubuntu 22.04 and later, Debian 12 desktops and Fedora. Without it the runtime says "No suitable fusermount binary found on the $PATH", and `--appimage-extract-and-run` or `APPIMAGE_EXTRACT_AND_RUN=1` runs it unpacked.

The CLI inside an AppImage is the AppImage itself: `./Pluryprint_X.Y.Z_linux_x64.AppImage print …`. `pluryprint-cli` inside it is not reachable from outside.

### Release assets

Per architecture: the `.AppImage` and `.deb`, a `.sig` for each (the minisign signature the updater checks), and `Pluryprint_X.Y.Z_linux_<arch>_appimage-sources.txt` (below). `SHA256SUMS` covers them all. The packages carry no GPG signature: with no apt repository to hold a key, one would check nothing. The release notes tell users to check `SHA256SUMS` instead (`sha256sum -c SHA256SUMS --ignore-missing`).

## Desktop integration

| Integration | How | Where it comes from |
|---|---|---|
| Applications menu, dock icon | `Pluryprint.desktop`, `Icon=pluryprint`, `StartupWMClass` | the desktop template |
| **Open with Pluryprint** (every file manager) | `MimeType=` lists the printable types, `Exec=pluryprint %F` | the desktop template |
| Dolphin (KDE): *Print now*, *Configure print job*, *Add to print queue* | KIO service menu | `src-tauri/linux/kio-servicemenu.desktop` → `/usr/share/kio/servicemenus/` |
| Nemo (Cinnamon): the same three | three `.nemo_action` files | `src-tauri/linux/nemo/` → `/usr/share/nemo/actions/` |
| Nautilus (GNOME Files) | **Open with** only: its context menu needs `nautilus-python`, which is not used | not in this release |

The files under `src-tauri/linux/` are written and documented with the context-menu code ([shell-extensions.md](../04-architecture/shell-extensions.md)); this document only covers how they are packaged. A service menu in a system directory needs no executable bit (KDE asks for one only in the user's own directories).

`src-tauri/tests/tauri_conf_contract.rs` (`fr_001_deb_installs_the_file_manager_actions`) pins the template and the four destinations, and checks that every source file exists, so a renamed file fails the tests instead of the release build.

## Updates

The updater picks its manifest entry by the bundle type tauri patches into each binary while bundling: `linux-<arch>-deb` for the `.deb`'s binary, then the plain `linux-<arch>` key, which is the AppImage's.

| Installed as | Manifest key | What the update does | Asks for |
|---|---|---|---|
| AppImage | `linux-x86_64`, `linux-aarch64` | downloads the new AppImage and replaces the running file (`$APPIMAGE`) | nothing, but the file and its folder must be writable |
| `.deb` | `linux-x86_64-deb`, `linux-aarch64-deb` | downloads the new `.deb` and runs `pkexec dpkg -i` (or `sudo`) | the administrator password |

A `.deb` install without its `-deb` key would fall back to the AppImage URL and fail (`InvalidUpdaterFormat`), so `release.yml` (`make-manifest`, `check-manifest`, `extract-signatures`) and `release-channels.yml` require all four Linux keys. No Linux release before 0.0.5 exists: the first in-app Linux update that can be tried is from 0.0.5 to the next one.

A beta `.deb` (`0.2.0-beta.1`) sorts above the stable `0.2.0` for dpkg, which reads `-beta.1` as a Debian revision; `dpkg -i` installs the stable one anyway, with a downgrade warning ([releasing.md](./releasing.md) §4).

## Licences

The `.deb` carries only Pluryprint and its notices; the distribution provides WebKitGTK and the rest. The AppImage **conveys** those libraries, most of them LGPL-2.1-or-later. What the release does about it:

- `THIRD-PARTY-NOTICES.md` (generated for all six targets, so the gtk-rs, webkit2gtk and zbus crates are listed) has a section, *Libraries bundled in the Linux AppImage*, that names them, where their licence texts are, and how to get their source, with a written offer for three years.
- Each Linux build runs `scripts/release/appimage-sources.sh` on the extracted AppImage: it traces every file linuxdeploy copied to the Ubuntu package it came from and writes the package, version and **source package and version** to `Pluryprint_X.Y.Z_linux_<arch>_appimage-sources.txt`, which the release attaches. The source is then `apt-get source <source>=<version>` or `https://launchpad.net/ubuntu/+source/<source>/<version>`. The script fails if a file cannot be traced.
- The libraries are unmodified and can be replaced (`--appimage-extract`), as LGPL-2.1 section 6 requires.

`src-tauri/.cargo/audit.toml` keeps ignoring RUSTSEC-2024-0429 (glib 0.18, unsound `VariantStrIter`), now with the reason that holds when GTK ships: nothing in tao, wry, muda, tauri or `src-tauri/src` calls it.

## The release build

Two rows of the `build` matrix in `release.yml`:

| Row | Runner | Target | Label | `machine` (readelf) |
|---|---|---|---|---|
| Linux (x64) | `ubuntu-22.04` | `x86_64-unknown-linux-gnu` | `linux_x64` | `Advanced Micro Devices X86-64` |
| Linux (ARM64) | `ubuntu-22.04-arm` | `aarch64-unknown-linux-gnu` | `linux_arm64` | `AArch64` |

1. **System libraries (Linux)**: `apt-get install libwebkit2gtk-4.1-dev libgtk-3-dev libdbus-1-dev librsvg2-dev patchelf file desktop-file-utils`. D-Bus because tauri's default `dbus` feature builds `libdbus-sys`, which panics without `dbus-1.pc`; librsvg for linuxdeploy's GTK plug-in. No `libssl-dev` (rustls, no `openssl-sys`), no `libxdo-dev`, no appindicator (no tray icon), no CUPS headers (printing runs the CUPS tools).
2. **Build**: `pnpm tauri build --target <triple> --bundles appimage,deb --ci`, with `APPIMAGE_EXTRACT_AND_RUN=1` (linuxdeploy is itself an AppImage and the runners have no FUSE). The bundler downloads linuxdeploy, its GTK and AppImage plug-ins and the AppImage runtime from GitHub at this point, unpinned: the one download in the release that is not pinned.
3. **Architecture and package checks (Linux)**: unpacks both packages (`dpkg-deb -x`, `--appimage-extract`) and fails unless every binary in `usr/bin` is the row's `machine`; the `.deb`'s `Depends` names `cups-client`; `usr/lib/Pluryprint/` holds `LICENSE`, `THIRD-PARTY-NOTICES.md` and `LICENSES/`; no `SumatraPDF.exe` or `shell_ext_win.dll` is inside; the application entries pass `desktop-file-validate` and carry a `MimeType`; the Dolphin and Nemo files are installed; and `pluryprint --version` runs from the `.deb` tree and through the AppImage's `AppRun`. `--version` is answered before GTK starts, so no display is needed. The sizes go to the job summary; no size budget applies to Linux.
4. **Stage the release files**, then **Record the AppImage's system libraries (Linux)** (`appimage-sources.sh`) into the same `unsigned-<label>` artifact.

The `.desktop` checks skip the Dolphin service menu: it is `Type=Service` with `MimeType=all/allfiles`, which KDE reads and `desktop-file-validate` rejects by design.

## Building and checking locally

Any Ubuntu 22.04 machine, container or VM with the packages of step 1, Rust stable and Node 24 with pnpm:

```bash
pnpm install --frozen-lockfile
pnpm tauri build --bundles appimage,deb --config src-tauri/tauri.ci.conf.json   # no updater key needed
B=src-tauri/target/release/bundle
dpkg-deb -I $B/deb/*.deb && dpkg-deb -c $B/deb/*.deb
$B/appimage/*.AppImage --appimage-extract && ./squashfs-root/AppRun --version
desktop-file-validate squashfs-root/*.desktop
scripts/release/appimage-sources.sh squashfs-root /tmp/appimage-sources.txt
```

`tauri.ci.conf.json` turns the updater artifacts off, so the build needs no signing key. In a container without FUSE, set `APPIMAGE_EXTRACT_AND_RUN=1`. The first AppImage build downloads linuxdeploy (network needed).

Checked on 2026-11-09 in an Ubuntu 22.04 arm64 container (WebKitGTK 2.50.4, GTK 3.24.33): both packages built, every check of step 3 passed, `stage-bundle` staged exactly the two release files, and `appimage-sources.sh` traced the AppImage to 124 packages. The x64 row and the hosted runners have not run yet.

## Known issues and things to test on real hardware

- **FUSE.** The AppImage's static runtime needs only `fusermount3` (package `fuse3`), not `libfuse2`/`libfuse2t64`; this was read from the runtime itself (static-pie, libfuse 3 linked in), not yet tried on a clean desktop without `libfuse2`.
- **AppArmor and user namespaces.** Ubuntu 24.04 restricts unprivileged user namespaces, which WebKitGTK's bubblewrap sandbox uses. A blank window from the AppImage there is the first thing to check (`WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1` confirms it; it is not a fix).
- **NVIDIA on Wayland** can show an empty window with WebKitGTK's DMA-BUF renderer; `WEBKIT_DISABLE_DMABUF_RENDERER=1` is the usual workaround.
- **Wayland app id.** The dock icon matches the window only when the app id equals the desktop file's name (`Pluryprint.desktop`); check for a duplicated or generic icon on GNOME and KDE.
- **16 KiB pages.** Asahi Linux and some other ARM64 kernels use 16 KiB pages, which libraries built for 4 KiB pages may not load in; the AppImage carries Ubuntu's.
- **Icons.** The PNGs are the macOS tile with its inset ([icons/README.md](../../src-tauri/icons/README.md)), not a full-bleed Linux icon. tauri installs `128x128@2x.png` as `hicolor/256x256@2/`, a folder icon themes do not read; the 512 px icon covers that size.
- **Distribution channels.** No apt or dnf repository, Flathub, Snap or AUR. A `.deb` user gets updates through the app or a new download.
- **Runner lifetime.** GitHub has deprecated the `ubuntu-22.04` and `ubuntu-22.04-arm` images (reported as from 17 September 2026, with scheduled brownouts and removal on 17 April 2027; check actions/runner-images issue #14254). A tag pushed during a brownout fails the Linux builds and blocks the draft until they are re-run. The way out that keeps the glibc 2.35 floor is to run the build jobs on `ubuntu-24.04`/`ubuntu-24.04-arm` inside an `ubuntu:22.04` container (installing `sudo`, `curl`, `git` and the toolchain in it); moving the baseline itself to 24.04 raises the glibc floor and needs `NO_STRIP=true` for linuxdeploy. Neither is done for 0.0.5.
- **Unpinned AppImage tooling.** linuxdeploy, its GTK and AppImage plug-ins and the type2-runtime are fetched from GitHub's "continuous" releases at build time, and the runtime ends up inside the shipped AppImage; every other release input is pinned. `appimage-sources.sh` now records the runtime commit each release shipped. Pinning means pre-seeding `~/.cache/tauri/` with checked copies (`sha256sum -c`) before `tauri build`; not done for 0.0.5.

## See also

- [releasing.md](./releasing.md): the runbook, the asset and manifest tables
- [ci-cd.md](./ci-cd.md): `ci.yml`'s Linux lane and `linux.yml`
- [auto-update-flow.md](./auto-update-flow.md)
- [shell-extensions.md](../04-architecture/shell-extensions.md)
