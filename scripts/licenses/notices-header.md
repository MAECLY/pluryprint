# Third-party notices

Pluryprint is licensed under the GNU Affero General Public License v3.0 (see
`LICENSE`; it was GPL-3.0-only until 2026-10-09). It links third-party code
into its binaries and its webview, its Windows installers carry one
third-party program (SumatraPDF), and its Linux AppImages carry the system
libraries it runs on (WebKitGTK, GTK and their dependencies). Their licences
require the notices below to travel with the program, so this file ships
inside every application bundle (`bundle.resources` in
`src-tauri/tauri.conf.json`), together with `LICENSE` and the licence texts in
`LICENSES/`, not only in the repository.

`docs/09-legal/third-party-licenses.md` is the human-readable inventory of
direct dependencies, how each is used, and why its licence is compatible with
the AGPL-3.0. This file is the notice that ships.

## How this file is made

The first part, up to [Rust crates](#rust-crates), is written by hand
(`scripts/licenses/notices-header.md`): the bundled program, the native code,
the installer, the font and the licence compatibility table. Everything from
[Rust crates](#rust-crates) on is **generated** by
`scripts/licenses/generate-notices.sh` and must not be edited by hand:

- **Rust:** `cargo about` (configuration in `scripts/licenses/about.toml`) on
  `src-tauri` (the `pluryprint` and `pluryprint-cli` binaries) and on
  `shell-ext-win` (the Explorer handler DLL), for the six release targets
  (`aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc`,
  `aarch64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`,
  `aarch64-unknown-linux-gnu`), from `Cargo.lock` and the crates' own published
  sources, offline. For every crate it reproduces the licence text the crate
  ships, which carries its copyright lines; crates offered under a choice
  ("MIT OR Apache-2.0") are listed under the licence Pluryprint uses. Build
  scripts and test-only crates are left out; a procedural macro that is a
  normal dependency is listed even though its code runs only at build time,
  so the list errs on the side of more notices, not fewer.
- **Frontend:** the production dependencies of `package.json` (from
  `pnpm licenses list --prod`), plus Tailwind CSS, whose generated stylesheet
  is in the bundle although the package is a development dependency. Type-only
  packages (`@types/*`) contain no shipped code and are left out. Each entry
  reproduces the package's own licence file.

The release workflow (`release.yml`, job `guard`) regenerates the file and
refuses to build when the committed copy differs, so a dependency change
cannot reach a release without its notice.

---

## Program shipped beside the app (Windows only)

### SumatraPDF 3.6.1

Pluryprint's Windows build prints PDFs through `SumatraPDF.exe`, which it runs
as a separate process. It is not linked into Pluryprint and is not modified.
`scripts/fetch-sumatrapdf.ps1` (or its POSIX twin,
`scripts/fetch-sumatrapdf.sh`) downloads the pinned upstream release for the
installer's architecture (`SumatraPDF-3.6.1-64.zip` or
`SumatraPDF-3.6.1-arm64.zip` from sumatrapdfreader.org), checks its SHA-256,
and puts `SumatraPDF.exe` in `src-tauri/resources/sumatrapdf/`, which
`bundle.resources` ships. The archives carry no licence, so the scripts also
fetch `COPYING`, `COPYING.BSD` and `AUTHORS` from the pinned source commit and
put them beside the executable as `LICENSE.txt`, `COPYING.BSD.txt` and
`AUTHORS.txt`, with a `SOURCE.txt` naming the version and source commit and a
`SOURCE-OFFER.txt` carrying the written offer of GPL-3.0 section 6. The macOS
bundle does not contain it.

| Component                              | Licence  | Copyright                                                                                   |
| -------------------------------------- | -------- | ------------------------------------------------------------------------------------------- |
| SumatraPDF                             | GPL-3.0  | Copyright Krzysztof Kowalczyk and the SumatraPDF contributors (see `AUTHORS` in its source) |
| MuPDF (compiled into `SumatraPDF.exe`) | AGPL-3.0 | Copyright Artifex Software, Inc.                                                            |

The GPL-3.0 text is in `LICENSES/GPL-3.0.txt` and the AGPL-3.0 text is
`LICENSE`; both ship in every bundle. The corresponding source is commit
`16c59fde8b824ab54c56f23aef910a6fdd874ad0` (tag `3.6.1rel`) of
https://github.com/sumatrapdfreader/sumatrapdf, which includes MuPDF under
`mupdf/`. Every Pluryprint release also attaches that tree as
`SumatraPDF-3.6.1-source.tar.gz`, next to the installers.

---

## Native code compiled into the binaries

### ring

`ring`, the cryptography under `rustls` (reached through `ureq` and through
`tauri`'s `reqwest`), compiles C and assembly derived from BoringSSL on every
target. Apache-2.0 AND ISC; the licence is given at the top of each source
file:

| Part                                                | Licence    | Copyright                                                                   |
| --------------------------------------------------- | ---------- | --------------------------------------------------------------------------- |
| ring's own code                                     | ISC        | Copyright 2015-2025 Brian Smith                                             |
| C and assembly taken from OpenSSL through BoringSSL | Apache-2.0 | Copyright 1995-2020 The OpenSSL Project Authors (year ranges vary per file) |
| C taken from BoringSSL                              | Apache-2.0 | Copyright 2014-2024 The BoringSSL Authors (year ranges vary per file)       |
| Other BoringSSL-derived files                       | ISC        | Copyright (c) 2014, Google Inc.; Copyright (c) 2014, Intel Corporation      |
| fiat-crypto field arithmetic (`third_party/fiat`)   | Apache-2.0 | Copyright 2015-2020 the fiat-crypto authors                                 |

The texts are reproduced under [Rust crates](#rust-crates); the Apache-2.0
text is also in `LICENSES/Apache-2.0.txt`.

### Objective-C helpers (macOS only)

`mac-notification-sys` compiles a small Objective-C file for native
notifications, and `objc2-exception-helper` a small C file. Both are covered
by their crate entries under [Rust crates](#rust-crates).

### Microsoft WebView2 loader (Windows only)

`webview2-com-sys` links Microsoft's `WebView2LoaderStatic.lib` statically
when the target is MSVC, and both Windows release targets are MSVC builds.
`wry` calls it to start the webview, so the loader is part of
`pluryprint.exe`; no `WebView2Loader.dll` is shipped. The text below is the
`LICENSE.txt` of the `Microsoft.Web.WebView2` NuGet package:

```text
Copyright (C) Microsoft Corporation. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

   * Redistributions of source code must retain the above copyright
notice, this list of conditions and the following disclaimer.
   * Redistributions in binary form must reproduce the above
copyright notice, this list of conditions and the following disclaimer
in the documentation and/or other materials provided with the
distribution.
   * The name of Microsoft Corporation, or the names of its contributors
may not be used to endorse or promote products derived from this
software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

The WebView2 _runtime_ (the Edge engine) is part of Windows and is not shipped;
the installer downloads Microsoft's bootstrapper only when it is missing.

---

## The Windows installer

The `.exe` setup programs are built with NSIS (Nullsoft Scriptable Install
System) by the Tauri bundler, which also compiles in its `nsis_tauri_utils`
plug-in (MIT OR Apache-2.0, Copyright the Tauri Programme within The
Commons Conservancy). The installer stub and its LZMA decompressor are part
of the setup program, not of Pluryprint, which the setup program unpacks and
installs. NSIS's licence notice:

```text
{{NSIS_COPYING}}
```

---

## Libraries bundled in the Linux AppImage

The `.deb` packages carry only Pluryprint: WebKitGTK, GTK and the rest are
the distribution's own packages, which the `.deb` depends on. An AppImage has
to run without them, so the Tauri bundler (through linuxdeploy and its GTK
plug-in) copies the shared libraries Pluryprint loads, and theirs, from the
Ubuntu 22.04 machine that builds it into the AppImage, unmodified: among them
WebKitGTK and JavaScriptCore, GTK 3, GDK-Pixbuf, GLib and GIO, libsoup 3,
Pango, ATK and AT-SPI, GStreamer and its base plug-ins, cairo, HarfBuzz,
libsecret, Enchant, libgcrypt, GnuTLS, Nettle, libidn2, libunistring,
librsvg, ICU, libxml2 and libxslt, the X11 and Wayland client libraries and
the GObject introspection data that comes with them. Each keeps its own
licence. Most of the GNOME and freedesktop.org libraries are under the GNU
Lesser General Public License, version 2.1 or later (WebKitGTK also has
BSD-licensed parts). Some are under the GNU General Public License or LGPL
version 3: JBIG-KIT's `libjbig` (which libtiff loads) is GPL-2.0-or-later;
Nettle and `libhogweed`, `libunistring`, `libidn2` and elfutils' `libdw` and
`libelf` are LGPL-3.0-or-later or GPL-2.0-or-later, at the recipient's choice;
Enchant and systemd's `libsystemd` carry some files under LGPL-3.0-or-later
or GPL-2.0. The rest are under permissive licences (MIT, BSD, ISC,
Apache-2.0, the ICU and libpng licences). The full text of each library's
licence and copyright notices is in its Debian package's `copyright` file
(`/usr/share/doc/<package>/copyright` on any Debian or Ubuntu system, or the
source package named below). The licence texts are at
https://www.gnu.org/licenses/old-licenses/lgpl-2.1.html,
https://www.gnu.org/licenses/lgpl-3.0.html,
https://www.gnu.org/licenses/old-licenses/gpl-2.0.html and
https://www.gnu.org/licenses/gpl-3.0.html.

Every AppImage also starts with the AppImage project's static type2-runtime
(https://github.com/AppImage/type2-runtime, MIT), which mounts the image. It
has these libraries linked into it: libfuse (LGPL-2.1), squashfuse
(BSD-2-Clause), zstd (BSD-3-Clause) and the musl C library (MIT). `AppRun`,
which then starts Pluryprint, is MIT-licensed (Copyright the AppImage project
contributors), and so is linuxdeploy's GTK plug-in hook script (Copyright the
linuxdeploy contributors).

**Corresponding Source.** Every Pluryprint release attaches, for each Linux
architecture, `Pluryprint_<version>_linux_x64_appimage-sources.txt` and
`Pluryprint_<version>_linux_arm64_appimage-sources.txt`
(`scripts/release/appimage-sources.sh`, run on the build machine right after
the AppImage is made). They name every package the AppImage carries files
from, with the exact version and the Ubuntu source package and version it was
built from, and the type2-runtime commit the AppImage starts with. That
source is published by Ubuntu, at
`https://launchpad.net/ubuntu/+source/<source>/<source version>` or with
`apt-get source <source>=<source version>`, and the runtime's (libfuse
included) at `https://github.com/AppImage/type2-runtime/commit/<commit>`. In
addition, for at least three years after each release, Maecly will give
anyone who asks a copy of the complete corresponding source of every GPL- or
LGPL-licensed component in that release's AppImages, the runtime included, at
the versions its list names, on a medium customarily used for software
interchange, for no more than the cost of providing it; ask through
https://github.com/MAECLY/pluryprint/issues.

Pluryprint uses these libraries only through their public interfaces, and
an AppImage can be unpacked (`--appimage-extract`), so a user can replace any
of them with a modified version, as LGPL-2.1 section 6 requires. The
runtime's libfuse is linked statically: its object files are not shipped
separately, so the runtime's complete source (above) is what lets a user
rebuild it with a modified libfuse.

---

## Font

Three weights of JetBrains Mono (`JetBrainsMono-Regular.woff2`, `-Medium` and
`-SemiBold`, in `src/assets/fonts/`) are copied into the bundle and used as the
monospace face. SIL Open Font License 1.1. Copyright 2020 The JetBrains Mono
Project Authors (https://github.com/JetBrains/JetBrainsMono). The copyright
line and the full licence text are in `LICENSES/OFL-1.1.txt`. The fonts are
not modified.

---

## System components the app uses but does not ship

- **macOS:** WebKit (`WKWebView`) and the other system frameworks, the CUPS
  `lp` command, which prints, and `textutil`.
- **Windows:** the WebView2 runtime. The WebView2 _loader_ is linked in; see
  [Microsoft WebView2 loader](#microsoft-webview2-loader-windows-only).
- **Linux (`.deb`):** WebKitGTK, GTK 3, GLib and the rest of their stack, the
  CUPS command-line tools (`lp`, `lpstat`, `lpoptions`, `cancel`, `ipptool`,
  `ippfind`), which print, and `heif-convert` (libheif) for HEIC pictures:
  the distribution's own packages, which the `.deb` depends on or recommends.
  The AppImage carries the WebKitGTK and GTK stack itself (see
  [Libraries bundled in the Linux AppImage](#libraries-bundled-in-the-linux-appimage))
  and uses the system's CUPS tools and libheif.
- **Programs the user installs, if they want those formats:** LibreOffice and
  the operating system's image codecs, run or loaded as separate components.
  None of them is shipped.

---

## Licence compatibility

Every licence in the shipped set lets its code be combined with, or shipped
beside, code under the AGPL-3.0:

| Licence                                                                                       | Where                                                                           | Why it is compatible                                                                                                                         |
| --------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| MIT, MIT-0, ISC, BSD-2-Clause, BSD-3-Clause, Zlib, 0BSD, Unlicense, CC0-1.0, BSL-1.0, OpenSSL | most crates, the frontend packages                                              | Permissive; ask only for a notice                                                                                                            |
| Apache-2.0 (alone, or with LLVM-exception)                                                    | many crates, `ring`, choices used under Apache                                  | Compatible with GPLv3 and so with AGPLv3 (its patent terms are what GPLv2 could not accept)                                                  |
| Unicode-3.0, Unicode-DFS-2016, CDLA-Permissive-2.0                                            | the ICU4X crates, `webpki-roots` data                                           | Permissive data licences; ask only for a notice                                                                                              |
| MPL-2.0                                                                                       | `option-ext`, `cssparser`, `selectors`                                          | Section 3.3 allows combination with the GNU licences; none carries an Exhibit B notice                                                       |
| OFL-1.1                                                                                       | JetBrains Mono                                                                  | A font licence; the fonts are shipped unmodified with their licence                                                                          |
| BSD-style Microsoft licence                                                                   | the WebView2 loader                                                             | Permissive; asks for the notice reproduced above                                                                                             |
| GPL-3.0                                                                                       | SumatraPDF                                                                      | A separate program, shipped beside Pluryprint; AGPL-3.0 section 13 and GPL-3.0 section 13 also allow the two to be combined                  |
| AGPL-3.0                                                                                      | MuPDF, inside SumatraPDF                                                        | The same licence as Pluryprint                                                                                                               |
| LGPL-2.0, LGPL-2.1 or LGPL-3.0, each "or later"                                               | WebKitGTK, GTK, GLib and others, in the AppImage                                | Lets a work that uses the library be conveyed under its own terms; the libraries ship unmodified, replaceable, with their source named above |
| GPL-2.0-or-later; LGPL-3.0-or-later or GPL-2.0-or-later                                       | `libjbig`; Nettle, `libunistring`, `libidn2`, `libdw`/`libelf`, in the AppImage | Usable under GPL-3.0, which AGPL-3.0 section 13 allows to be combined with Pluryprint; shipped unmodified, with their source named above     |
| zlib/libpng, bzip2, CPL-1.0                                                                   | the NSIS installer stub                                                         | The setup program that carries Pluryprint, not part of it: an aggregate, which the AGPL-3.0 (section 5) permits                              |

The MPL-2.0 crates are used unmodified; their source is available from
crates.io at the versions listed below. The GPL and LGPL libraries ship
only in the Linux AppImages, unmodified, with their Corresponding Source as
described above. No shipped component is GPL-2.0-only, EPL, CDDL, SSPL or proprietary.

---
