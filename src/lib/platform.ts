/**
 * Which operating system, and on Linux which package, this build runs on.
 *
 * Copy and behaviour differ per platform in a handful of places: where the
 * right-click entry lives (Explorer, Finder › Services, a Linux file
 * manager), what the command-line program is called, what installing an
 * update asks of the user, and how a missing dependency is installed. Every
 * one of those choices goes through here, so a third platform cannot quietly
 * fall into another one's wording again (before Linux shipped, every branch
 * was `isMacPlatform() ? mac : windows`, and Linux would have read about
 * Explorer and `pluryprint-cli`).
 *
 * The authority is the backend: `app_info.os` is Rust's
 * `std::env::consts::OS`, and every `app_info` answer is remembered here
 * ({@link rememberHostInfo}, called by `fetchAppInfo`). The sidebar asks for
 * it as soon as the shell mounts, so in the app the answer is known before
 * anyone reads a tip. Until then (and in the standalone web preview, which has
 * no backend) the webview's own platform string stands in: it names the
 * webview rather than the host, but WebView2, WKWebView and WebKitGTK each
 * report their own OS truthfully, and a guess beats copy for the wrong one.
 */

import { useSyncExternalStore } from 'react';

import type { AppInfo } from '@/ipc/bindings';

/** The three desktop platforms Pluryprint ships for. */
export type OsId = 'windows' | 'macos' | 'linux';

/**
 * How Pluryprint was installed on Linux, which decides how it updates and
 * where its command-line program lives:
 * - `appimage`: a single file the user runs from wherever they saved it. It
 *   updates itself in place, nothing puts it on `PATH`, and no file manager
 *   menu is registered unless the user integrates it themselves.
 * - `deb`: the Debian/Ubuntu package. It installs `pluryprint` into
 *   `/usr/bin` (on `PATH`), the "Open with" entry and the Dolphin and Nemo
 *   menus, and an update asks for the administrator password (`pkexec`).
 */
export type LinuxPackage = 'appimage' | 'deb';

/** What the UI needs to know about the host. */
export interface HostPlatform {
  /** The operating system. */
  os: OsId;
  /** On Linux, the package that installed this build, when known. */
  linuxPackage: LinuxPackage | null;
  /** Whether `os` came from the backend rather than the webview's guess. */
  fromBackend: boolean;
}

/**
 * The fields of `app_info` this module reads. `bundle_type` is optional on
 * purpose: it is Tauri's `bundle_type()` (`"deb"`, `"appimage"`, `"nsis"`,
 * `"app"`, …), which an older backend does not send; the executable path is
 * the fallback then.
 */
export interface HostInfo {
  os: AppInfo['os'];
  executable?: string | null | undefined;
  version?: string | undefined;
  arch?: string | undefined;
  bundle_type?: string | null | undefined;
}

/** `app_info.os` as an {@link OsId}, or `null` for anything else. */
export function toOsId(os: string | null | undefined): OsId | null {
  const value = os?.toLowerCase();
  return value === 'windows' || value === 'macos' || value === 'linux'
    ? value
    : null;
}

/**
 * The webview's platform string, read the same way `isMacPlatform` reads it
 * (`userAgentData`, then the deprecated but universal `navigator.platform`).
 * WebKitGTK reports `Linux x86_64` / `Linux aarch64`; Android is excluded
 * because it says "Linux" too and is not a platform this app runs on.
 */
export function webviewOs(): OsId {
  if (typeof navigator === 'undefined') return 'windows';
  const withData = navigator as Navigator & {
    userAgentData?: { platform?: string };
  };
  const platform =
    withData.userAgentData?.platform ??
    navigator.platform ??
    navigator.userAgent;
  if (/mac|iphone|ipad/i.test(platform)) return 'macos';
  if (/linux|x11/i.test(platform) && !/android/i.test(platform)) {
    return 'linux';
  }
  return 'windows';
}

/**
 * The Linux package `info` describes, or `null` when it is not Linux or the
 * package cannot be told.
 *
 * `bundle_type` is the backend's own answer and wins. Without it the
 * executable path decides: an AppImage runs from a FUSE mount
 * (`/tmp/.mount_<random>/…`), or is named by its own file when the backend
 * reports `$APPIMAGE`; the .deb installs into `/usr/bin`. Anything else (a
 * build run from `target/`, a package someone repacked) stays `null`, and
 * the copy then avoids claims that depend on the package.
 */
export function linuxPackageOf(info: HostInfo): LinuxPackage | null {
  if (toOsId(info.os) !== 'linux') return null;
  const kind = info.bundle_type?.toLowerCase();
  if (kind === 'appimage' || kind === 'deb') return kind;
  if (kind) return null;
  const exe = info.executable ?? '';
  if (isAppImageMount(exe) || /\.appimage$/i.test(exe)) return 'appimage';
  if (/^\/usr\/bin\/[^/]+$/.test(exe)) return 'deb';
  return null;
}

/**
 * Whether `path` lies inside the read-only mount an AppImage runs from. That
 * path disappears the moment the app exits, so a command naming it would
 * fail: the CLI command must name the AppImage file instead.
 */
export function isAppImageMount(path: string | null | undefined): boolean {
  return !!path && /^\/tmp\/\.mount_[^/]+\//.test(path);
}

/** Release asset architecture labels, as on the download page. */
const ARCH_LABEL: Readonly<Record<string, string>> = {
  x86_64: 'x64',
  aarch64: 'arm64',
};

/**
 * The AppImage's file name as the release publishes it
 * (`Pluryprint_<version>_linux_<x64|arm64>.AppImage`), or a shell glob that
 * matches it when the version or architecture is unknown. Used to tell an
 * AppImage user how to call the command-line program: nothing puts an
 * AppImage on `PATH`, so it is run by its own file name.
 */
export function appImageFileName(
  version?: string | null,
  arch?: string | null,
): string {
  const label = arch ? ARCH_LABEL[arch] : undefined;
  if (!version || !label) return 'Pluryprint_*.AppImage';
  return `Pluryprint_${version}_linux_${label}.AppImage`;
}

// ---------------------------------------------------------------------------
// The remembered host (fed by every `app_info` answer)
// ---------------------------------------------------------------------------

let remembered: HostInfo | null = null;
let snapshot: HostPlatform | null = null;
/**
 * The guessed snapshots, one per OS, so the same guess is the same object:
 * `useSyncExternalStore` re-renders forever on a snapshot that changes
 * identity between calls. The guess itself is not cached, because the
 * webview's platform is what tests stub.
 */
const guesses: Partial<Record<OsId, HostPlatform>> = {};
const listeners = new Set<() => void>();

/**
 * Remember what the backend said about the host. `fetchAppInfo` calls this on
 * every answer, so whichever screen asked first, the rest of the UI (tips,
 * notes, remediation sentences) switches to the backend's word.
 */
export function rememberHostInfo(info: HostInfo): void {
  if (!toOsId(info.os)) return;
  const next: HostInfo = {
    os: info.os,
    executable: info.executable ?? null,
    bundle_type: info.bundle_type ?? null,
    version: info.version,
    arch: info.arch,
  };
  const same =
    remembered !== null &&
    remembered.os === next.os &&
    remembered.executable === next.executable &&
    remembered.bundle_type === next.bundle_type &&
    remembered.version === next.version &&
    remembered.arch === next.arch;
  if (same) return;
  remembered = next;
  snapshot = null;
  for (const listener of listeners) listener();
}

/** Forget the remembered host (tests: every test starts from the webview). */
export function resetHostPlatform(): void {
  remembered = null;
  snapshot = null;
  for (const listener of listeners) listener();
}

/** What the backend last said about the host, if it has said anything. */
export function rememberedHostInfo(): HostInfo | null {
  return remembered;
}

/** The host platform: the backend's answer once known, the webview's guess before. */
export function hostPlatform(): HostPlatform {
  if (snapshot) return snapshot;
  const os = toOsId(remembered?.os);
  if (remembered && os) {
    snapshot = {
      os,
      linuxPackage: linuxPackageOf(remembered),
      fromBackend: true,
    };
    return snapshot;
  }
  const guess = webviewOs();
  return (guesses[guess] ??= {
    os: guess,
    linuxPackage: null,
    fromBackend: false,
  });
}

/** The host OS ({@link hostPlatform}`().os`). */
export function hostOs(): OsId {
  return hostPlatform().os;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/**
 * {@link hostPlatform} as a hook: a component re-renders when the backend's
 * answer arrives, so a tip first worded from the webview's guess is corrected
 * as soon as `app_info` lands.
 */
export function useHostPlatform(): HostPlatform {
  return useSyncExternalStore(subscribe, hostPlatform, hostPlatform);
}

/**
 * The variant of a per-platform catalog key: `<key>Mac` on macOS,
 * `<key>Linux` on Linux, `<key>` itself on Windows. Callers fall back to the
 * plain key when the variant has no sentence.
 */
export function platformKeyVariant(key: string, os: OsId): string {
  if (os === 'macos') return `${key}Mac`;
  if (os === 'linux') return `${key}Linux`;
  return key;
}

/**
 * The `home` sentence that says where Pluryprint is in a Linux file manager,
 * per package: the .deb registers "Open with" and the Dolphin and Nemo
 * actions; an AppImage registers nothing until the user integrates it; an
 * unknown package gets the sentence that explains both.
 */
export function linuxMenuKey(
  pkg: LinuxPackage | null,
): 'linuxMenu.deb' | 'linuxMenu.appimage' | 'linuxMenu.unknown' {
  if (pkg === 'deb') return 'linuxMenu.deb';
  if (pkg === 'appimage') return 'linuxMenu.appimage';
  return 'linuxMenu.unknown';
}
