#!/usr/bin/env python3
"""Helpers for .github/workflows/release.yml and release-channels.yml.

Plain standard-library Python, so the runners need nothing installed, and
every rule the workflows depend on is a function that
src-tauri/tests/release_pipeline.rs exercises on a developer machine.

Subcommands:

  semver-gt CANDIDATE CURRENT
      Exit 0 when CANDIDATE is a higher SemVer 2.0 precedence than CURRENT,
      1 otherwise (equal included). A leading "v" is ignored, and so is build
      metadata (SemVer section 10). Used to move the beta channel's manifest
      forward only.

  stage-bundle --bundle-dir DIR --label LABEL --version V --out DIR
               [--product NAME]
      Copy what one `tauri build` produced into OUT under the name the
      release gives it, <product>_<version>_<label><ext>: the .dmg and the
      .app.tar.gz of a macOS build, the NSIS -setup.exe of a Windows one, the
      .AppImage and the .deb of a Linux one. Signature files stay behind: the
      build signs with a throwaway key, and the sign job signs the staged
      files with the real one. Fails unless every update artifact of the
      label is there and no kind of file appears twice.

  make-manifest --dir DIR --version V --assets F --repo OWNER/NAME
                [--body-file F] [--pub-date D] [--product NAME]
                [--require KEY ...] --out F
      Write the latest.json the updater reads from the staged files and their
      .sig files in DIR: for every manifest key whose update artifact is there,
      the signature (the .sig contents) and the URL in the GitHub API asset
      form (https://api.github.com/repos/<repo>/releases/assets/<id>; --assets
      is the JSON of GET /repos/<repo>/releases/<id>/assets). Fails when a
      required platform, its signature or its uploaded asset is missing. The
      release body becomes the update notes.

  check-manifest --manifest F --version V [--require KEY ...]
                 [--signed-version V]
      Fail unless the manifest names version V and every required platform
      with a url and a signature. With --signed-version, every required
      platform's signature must also be bound to that version: its trusted
      comment carries the `version:V` field `tauri signer sign --app-version`
      writes, which the app's updater (`requireSignedVersion`) insists on.

  extract-signatures --manifest F --assets F --out DIR [--require KEY ...]
      For every required platform of a finished manifest, write the decoded
      minisign signature to DIR/<platform>.minisig and print
      "<platform> <asset file name>", so the workflow can verify each
      downloaded artifact against the app's public key.
"""

from __future__ import annotations

import argparse
import base64
import binascii
import datetime
import json
import os
import re
import shutil
import sys
import urllib.parse

SEMVER = re.compile(
    r"v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
)


def semver_key(version: str):
    """A sort key with SemVer 2.0 precedence. Raises ValueError otherwise."""
    m = SEMVER.fullmatch(version.strip())
    if not m:
        raise ValueError(f"not a SemVer version: {version!r}")
    core = tuple(int(x) for x in m.group(1, 2, 3))
    pre = m.group(4)
    if pre is None:
        # A release ranks above every prerelease of the same core.
        return core, (1,)
    ids = []
    for part in pre.split("."):
        # Numeric identifiers rank below alphanumeric ones (section 11.4.3).
        ids.append((0, int(part), "") if part.isdigit() else (1, 0, part))
    return core, (0, tuple(ids))


def semver_gt(candidate: str, current: str) -> bool:
    return semver_key(candidate) > semver_key(current)


API_ASSET = "https://api.github.com/repos/{repo}/releases/assets/"

# Every latest.json key the release serves, with the label of the build that
# makes its update artifact and that artifact's extension. The labels name
# the computer, so a download says which one it is for; the keys are the ones
# the updater looks up. tauri-plugin-updater tries {os}-{arch}-{bundle} first
# and then {os}-{arch}, with the bundle type compiled into each binary: a
# Linux build makes two update artifacts, the AppImage (which replaces
# itself, served by the plain key) and the .deb (installed with
# `pkexec dpkg -i`, served by the -deb key). Without the -deb key a .deb
# install would fall back to the AppImage URL and refuse it. Every other
# build makes one, and its plain key serves it.
PLATFORMS = {
    "darwin-aarch64": ("apple_silicon", ".app.tar.gz"),
    "darwin-x86_64": ("apple_intel", ".app.tar.gz"),
    "windows-x86_64": ("windows_x64", ".exe"),
    "windows-aarch64": ("windows_arm64", ".exe"),
    "linux-x86_64": ("linux_x64", ".AppImage"),
    "linux-x86_64-deb": ("linux_x64", ".deb"),
    "linux-aarch64": ("linux_arm64", ".AppImage"),
    "linux-aarch64-deb": ("linux_arm64", ".deb"),
}

# The build labels, in the order above.
LABELS = list(dict.fromkeys(label for label, _ in PLATFORMS.values()))

# What one build leaves under target/<triple>/release/bundle/: the folder,
# the end of tauri's own file name, and the extension in the release. The
# appimage/ folder also holds the .AppDir it was made from and the .deb
# folder its unpacked tree; neither ends with the suffix, so neither is
# staged.
BUNDLE_KINDS = [
    ("dmg", ".dmg", ".dmg"),
    ("macos", ".app.tar.gz", ".app.tar.gz"),
    ("nsis", "-setup.exe", ".exe"),
    ("appimage", ".AppImage", ".AppImage"),
    ("deb", ".deb", ".deb"),
]


def update_exts(label: str) -> list[str]:
    """The extensions of the update artifacts the build `label` makes."""
    return [ext for lab, ext in PLATFORMS.values() if lab == label]


def release_name(product: str, version: str, label: str, ext: str) -> str:
    return f"{product}_{version.lstrip('v')}_{label}{ext}"


def stage_bundle(bundle_dir: str, label: str, version: str, out: str,
                 product: str = "Pluryprint") -> list[str]:
    """Copy one build's release files into `out`. Returns their names."""
    if label not in LABELS:
        raise ValueError(f"unknown label {label!r}; one of {sorted(LABELS)}")
    if os.path.isdir(out) and os.listdir(out):
        # A shared folder (Vite's dist/ cost the first 0.0.1 draft) would
        # carry foreign files into the artifact and the release.
        raise ValueError(f"{out} is not empty; stage into a fresh folder")
    os.makedirs(out, exist_ok=True)
    staged = []
    for sub, suffix, ext in BUNDLE_KINDS:
        folder = os.path.join(bundle_dir, sub)
        if not os.path.isdir(folder):
            continue
        found = sorted(
            f for f in os.listdir(folder)
            if f.endswith(suffix) and os.path.isfile(os.path.join(folder, f))
        )
        if len(found) > 1:
            raise ValueError(
                f"{folder} holds {len(found)} *{suffix} files ({', '.join(found)}); "
                "one build must make one file of each kind"
            )
        if not found:
            continue
        name = release_name(product, version, label, ext)
        shutil.copyfile(os.path.join(folder, found[0]), os.path.join(out, name))
        staged.append(name)
    for ext in update_exts(label):
        update = release_name(product, version, label, ext)
        if update not in staged:
            raise ValueError(
                f"{bundle_dir} has no update artifact for {label} ({update})"
            )
    return staged


def make_manifest(directory: str, version: str, assets: list, repo: str,
                  body: str | None, pub_date: str, required: list[str],
                  product: str = "Pluryprint") -> tuple[dict, list[str]]:
    """The latest.json for the files in `directory`, and the errors found."""
    errors = []
    ids = {a["name"]: a["id"] for a in assets}
    prefix = API_ASSET.format(repo=repo)
    platforms = {}
    for key, (label, ext) in PLATFORMS.items():
        name = release_name(product, version, label, ext)
        path = os.path.join(directory, name)
        if not os.path.isfile(path):
            continue
        sig_path = path + ".sig"
        if not os.path.isfile(sig_path):
            errors.append(f"{name} has no {name}.sig; the sign job did not sign it")
            continue
        with open(sig_path, encoding="utf-8") as f:
            signature = f.read().strip()
        if not signature:
            errors.append(f"{name}.sig is empty")
            continue
        if name not in ids:
            errors.append(f"{name} is not an asset of this release; upload it first")
            continue
        platforms[key] = {"signature": signature, "url": f"{prefix}{ids[name]}"}
    for key in required:
        if key not in PLATFORMS:
            errors.append(f"{key} is not a platform this release builds")
        elif key not in platforms:
            errors.append(
                f"latest.json has no {key} entry; re-run the build for that platform"
            )
    manifest = {
        "version": version.lstrip("v"),
        "notes": (body or "").strip(),
        "pub_date": pub_date,
        "platforms": platforms,
    }
    return manifest, errors


def signed_version(signature: str) -> str | None:
    """The version a manifest signature (base64 of the .sig file) is bound to.

    The Tauri CLI writes the trusted comment as tab-separated `key:value`
    fields, e.g. `timestamp:1700000000<TAB>file:x.exe<TAB>version:1.2.3`.
    This only reads it: minisign (the publish job, and the app) is what
    proves the comment is covered by the signature.
    """
    try:
        text = base64.b64decode(signature, validate=True).decode("utf-8")
    except (binascii.Error, UnicodeDecodeError):
        return None
    prefix = "trusted comment: "
    for line in text.splitlines():
        if line.startswith(prefix):
            for field in line[len(prefix):].split("\t"):
                if field.startswith("version:"):
                    return field[len("version:"):]
    return None


def check_manifest(manifest: dict, version: str, required: list[str],
                   signed: str | None = None) -> list[str]:
    errors = []
    if str(manifest.get("version", "")).lstrip("v") != version.lstrip("v"):
        errors.append(
            f"latest.json is for {manifest.get('version')!r}, not {version!r}"
        )
    platforms = manifest.get("platforms", {})
    for key in required:
        entry = platforms.get(key)
        if not entry:
            errors.append(f"latest.json has no {key} entry")
        elif not entry.get("url") or not entry.get("signature"):
            errors.append(f"latest.json's {key} entry lacks a url or a signature")
        elif signed is not None:
            got = signed_version(entry["signature"])
            if got is None:
                errors.append(
                    f"latest.json's {key} signature is not bound to a version; "
                    "sign with `tauri signer sign --app-version`, or the app's "
                    "updater (requireSignedVersion) refuses it"
                )
            elif got.lstrip("v") != signed.lstrip("v"):
                errors.append(
                    f"latest.json's {key} signature is bound to {got!r}, not {signed!r}"
                )
    return errors


def extract_signatures(manifest: dict, assets: list, out: str,
                       required: list[str]) -> list[tuple[str, str]]:
    names = {a["id"]: a["name"] for a in assets}
    os.makedirs(out, exist_ok=True)
    found = []
    for key in required:
        entry = manifest["platforms"][key]
        last = entry["url"].rsplit("/", 1)[-1]
        name = names.get(int(last)) if last.isdigit() else urllib.parse.unquote(last)
        if not name:
            raise ValueError(f"{key}: {entry['url']} is not an asset of this release")
        with open(os.path.join(out, f"{key}.minisig"), "wb") as f:
            f.write(base64.b64decode(entry["signature"]))
        found.append((key, name))
    return found


def main(argv: list[str]) -> int:
    # The lines this prints are read back by `while read` loops: end them
    # with LF on every platform, never the CRLF Python writes on Windows.
    sys.stdout.reconfigure(newline="\n")
    parser = argparse.ArgumentParser(prog="release_tools.py")
    sub = parser.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("semver-gt")
    p.add_argument("candidate")
    p.add_argument("current")

    p = sub.add_parser("stage-bundle")
    p.add_argument("--bundle-dir", required=True)
    p.add_argument("--label", required=True)
    p.add_argument("--version", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--product", default="Pluryprint")

    p = sub.add_parser("make-manifest")
    p.add_argument("--dir", required=True)
    p.add_argument("--version", required=True)
    p.add_argument("--assets", required=True)
    p.add_argument("--repo", required=True)
    p.add_argument("--body-file")
    p.add_argument("--pub-date")
    p.add_argument("--product", default="Pluryprint")
    p.add_argument("--require", action="append", default=[])
    p.add_argument("--out", required=True)

    p = sub.add_parser("check-manifest")
    p.add_argument("--manifest", required=True)
    p.add_argument("--version", required=True)
    p.add_argument("--require", action="append", default=[])
    p.add_argument("--signed-version")

    p = sub.add_parser("extract-signatures")
    p.add_argument("--manifest", required=True)
    p.add_argument("--assets", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--require", action="append", default=[])

    args = parser.parse_args(argv)

    if args.cmd == "semver-gt":
        try:
            return 0 if semver_gt(args.candidate, args.current) else 1
        except ValueError as e:
            print(f"::error::{e}", file=sys.stderr)
            return 2

    if args.cmd == "stage-bundle":
        try:
            for name in stage_bundle(args.bundle_dir, args.label, args.version,
                                     args.out, args.product):
                print(name)
        except (OSError, ValueError) as e:
            print(f"::error::{e}", file=sys.stderr)
            return 1
        return 0

    if args.cmd == "make-manifest":
        with open(args.assets, encoding="utf-8") as f:
            assets = json.load(f)
        body = None
        if args.body_file:
            with open(args.body_file, encoding="utf-8") as f:
                body = f.read()
        pub_date = args.pub_date or datetime.datetime.now(
            datetime.timezone.utc
        ).strftime("%Y-%m-%dT%H:%M:%SZ")
        manifest, errors = make_manifest(args.dir, args.version, assets, args.repo,
                                         body, pub_date, args.require, args.product)
        for e in errors:
            print(f"::error::{e}", file=sys.stderr)
        if errors:
            return 1
        with open(args.out, "w", encoding="utf-8") as f:
            json.dump(manifest, f, indent=2)
            f.write("\n")
        return 0

    if args.cmd == "check-manifest":
        with open(args.manifest, encoding="utf-8") as f:
            manifest = json.load(f)
        errors = check_manifest(manifest, args.version, args.require,
                                args.signed_version)
        for e in errors:
            print(f"::error::{e}", file=sys.stderr)
        return 1 if errors else 0

    if args.cmd == "extract-signatures":
        with open(args.manifest, encoding="utf-8") as f:
            manifest = json.load(f)
        with open(args.assets, encoding="utf-8") as f:
            assets = json.load(f)
        try:
            for key, name in extract_signatures(manifest, assets, args.out, args.require):
                print(f"{key} {name}")
        except (KeyError, ValueError, binascii.Error) as e:
            print(f"::error::{e}", file=sys.stderr)
            return 1
        return 0

    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
