#!/usr/bin/env bash
#
# List the system packages an extracted AppImage carries libraries from.
#
#   scripts/release/appimage-sources.sh SQUASHFS_ROOT OUT
#
# linuxdeploy copies WebKitGTK, GTK, GLib, libsoup and the rest of their
# dependency tree from the build machine into the AppImage, unmodified. Most
# of them are LGPL-2.1-or-later, whose section 6 asks whoever conveys the
# object code to make the Corresponding Source available. This writes, for
# every package a bundled file came from, the binary package, its version and
# architecture, and the source package and version Ubuntu built it from, so
# the exact source can be fetched (`apt-get source <source>=<version>`, or
# https://launchpad.net/ubuntu/+source/<source>/<version>). release.yml runs
# it on the build machine, right after the AppImage is made, while dpkg still
# knows those versions, and attaches the list to the release
# (THIRD-PARTY-NOTICES.md, "Libraries bundled in the Linux AppImage").
#
# Fails when a bundled library cannot be traced to a package: the list would
# then be incomplete.

set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 SQUASHFS_ROOT OUT" >&2
  exit 2
fi
root="$1"
out="$2"
[[ -d "$root/usr/lib" ]] || { echo "$root has no usr/lib; extract the AppImage first" >&2; exit 1; }

# The package that installed the file whose path ends with `rel`. Development
# packages own the unversioned .so symlinks, never the library itself.
owner() {
  local rel="$1"
  dpkg -S "*/$rel" 2>/dev/null | cut -d: -f1 | tr ',' '\n' | sed 's/^ *//' |
    { grep -v -- '-dev$' || true; } | head -n1
}

packages=$(mktemp)
missing=0
# Every file linuxdeploy copied (libraries, WebKit's helper processes, GTK
# and GStreamer modules, typelibs), except Pluryprint's own files
# (usr/lib/Pluryprint) and the module caches linuxdeploy writes itself.
while IFS= read -r -d '' f; do
  rel="${f#"$root"/usr/lib/}"
  pkg=$(owner "$rel")
  # linuxdeploy flattens most libraries into usr/lib: fall back to the name.
  [[ -n "$pkg" ]] || pkg=$(owner "$(basename "$f")")
  # A library it reached through a development symlink (libgio-2.0.so) is a
  # copy of the versioned file that symlink points at on this machine.
  if [[ -z "$pkg" ]]; then
    for link in /usr/lib/*/"$(basename "$f")" /lib/*/"$(basename "$f")"; do
      [[ -e "$link" ]] || continue
      pkg=$(owner "$(basename "$(readlink -f "$link")")")
      [[ -n "$pkg" ]] && break
    done
  fi
  if [[ -z "$pkg" ]]; then
    echo "::error::no package owns $rel" >&2
    missing=1
    continue
  fi
  echo "$pkg" >> "$packages"
done < <(find "$root/usr/lib" -path "$root/usr/lib/Pluryprint" -prune -o \
           -type f ! -name '*.cache' -print0)

{
  echo "# Packages the Pluryprint AppImage carries libraries from, as installed on"
  # shellcheck source=/dev/null # the build machine's, read at run time
  echo "# the build machine ($(. /etc/os-release && echo "$PRETTY_NAME"), $(dpkg --print-architecture))."
  echo "# Columns: package, version, architecture, source package, source version."
  echo "# The source of each: apt-get source <source>=<source version>, or"
  echo "# https://launchpad.net/ubuntu/+source/<source>/<source version>"
  sort -u "$packages" | while read -r pkg; do
    dpkg-query -W -f='${binary:Package} ${Version} ${Architecture} ${source:Package} ${source:Version}\n' "$pkg"
  done
} > "$out"
rm -f "$packages"
[[ "$missing" -eq 0 ]]
echo "$(grep -vc '^#' "$out") packages listed in $out"
