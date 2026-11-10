import { describe, expect, it, vi } from 'vitest';

import {
  appImageFileName,
  appImageNameOf,
  hostOs,
  hostPlatform,
  isAppImageMount,
  linuxMenuKey,
  linuxPackageOf,
  platformKeyVariant,
  rememberHostInfo,
  rememberedHostInfo,
  resetHostPlatform,
  toOsId,
  webviewOs,
} from '@/lib/platform';

/** Pretend the webview reports `value` as its platform for one callback. */
function withWebviewPlatform<T>(value: string, fn: () => T): T {
  const spy = vi
    .spyOn(window.navigator, 'platform', 'get')
    .mockReturnValue(value);
  try {
    return fn();
  } finally {
    spy.mockRestore();
  }
}

describe('toOsId', () => {
  it('accepts the three values Rust reports, in any case', () => {
    expect(toOsId('windows')).toBe('windows');
    expect(toOsId('macos')).toBe('macos');
    expect(toOsId('linux')).toBe('linux');
    expect(toOsId('Linux')).toBe('linux');
  });

  it('rejects anything else instead of guessing', () => {
    expect(toOsId('freebsd')).toBeNull();
    expect(toOsId('')).toBeNull();
    expect(toOsId(undefined)).toBeNull();
    expect(toOsId(null)).toBeNull();
  });
});

describe('webviewOs', () => {
  it('reads each engine’s own platform string', () => {
    expect(withWebviewPlatform('Win32', webviewOs)).toBe('windows');
    expect(withWebviewPlatform('MacIntel', webviewOs)).toBe('macos');
    // WebKitGTK reports the kernel and the machine.
    expect(withWebviewPlatform('Linux x86_64', webviewOs)).toBe('linux');
    expect(withWebviewPlatform('Linux aarch64', webviewOs)).toBe('linux');
  });

  it('does not take Android for a Linux desktop', () => {
    expect(withWebviewPlatform('Linux armv8l Android', webviewOs)).toBe(
      'windows',
    );
  });
});

describe('linuxPackageOf', () => {
  it('trusts the backend’s bundle type first', () => {
    expect(
      linuxPackageOf({
        os: 'linux',
        bundle_type: 'deb',
        executable: '/tmp/.mount_abc/usr/bin/pluryprint',
      }),
    ).toBe('deb');
    expect(linuxPackageOf({ os: 'linux', bundle_type: 'AppImage' })).toBe(
      'appimage',
    );
    // A package this copy has no wording for (rpm) is not mistaken for one.
    expect(
      linuxPackageOf({
        os: 'linux',
        bundle_type: 'rpm',
        executable: '/usr/bin/pluryprint',
      }),
    ).toBeNull();
  });

  it('falls back to the executable path', () => {
    // The AppImage's FUSE mount, or the AppImage file itself.
    expect(
      linuxPackageOf({
        os: 'linux',
        executable: '/tmp/.mount_PluryXa1b2c/usr/bin/pluryprint',
      }),
    ).toBe('appimage');
    expect(
      linuxPackageOf({
        os: 'linux',
        executable: '/home/ana/Apps/Pluryprint_0.0.5_linux_x64.AppImage',
      }),
    ).toBe('appimage');
    // The .deb installs the binary into /usr/bin.
    expect(
      linuxPackageOf({ os: 'linux', executable: '/usr/bin/pluryprint' }),
    ).toBe('deb');
    // A development build says nothing about a package.
    expect(
      linuxPackageOf({
        os: 'linux',
        executable: '/home/ana/pluryprint/target/debug/pluryprint',
      }),
    ).toBeNull();
    expect(linuxPackageOf({ os: 'linux' })).toBeNull();
  });

  it('is null off Linux, whatever the path says', () => {
    expect(
      linuxPackageOf({ os: 'macos', executable: '/usr/bin/pluryprint' }),
    ).toBeNull();
    expect(linuxPackageOf({ os: 'windows', bundle_type: 'deb' })).toBeNull();
  });
});

describe('isAppImageMount', () => {
  it('spots the temporary mount an AppImage runs from', () => {
    expect(isAppImageMount('/tmp/.mount_Pluryp1/usr/bin/pluryprint')).toBe(
      true,
    );
    expect(isAppImageMount('/home/ana/Pluryprint.AppImage')).toBe(false);
    expect(isAppImageMount('/usr/bin/pluryprint')).toBe(false);
    expect(isAppImageMount(null)).toBe(false);
  });
});

describe('appImageNameOf', () => {
  const linux = { os: 'linux', version: '0.0.6', arch: 'x86_64' };
  it('names the AppImage file the backend reports, even after an in-place update', () => {
    expect(
      appImageNameOf({
        ...linux,
        bundle_type: 'appimage',
        executable: '/home/u/Apps/Pluryprint_0.0.5_linux_x64.AppImage',
      }),
    ).toBe('Pluryprint_0.0.5_linux_x64.AppImage');
    expect(
      appImageNameOf({
        ...linux,
        bundle_type: 'appimage',
        executable: '/home/u/Applications/pluryprint',
      }),
    ).toBe('pluryprint');
  });
  it('falls back to the published name for a mount, a shell-unsafe name or no info', () => {
    expect(
      appImageNameOf({
        ...linux,
        executable: '/tmp/.mount_abc/usr/bin/pluryprint',
      }),
    ).toBe('Pluryprint_0.0.6_linux_x64.AppImage');
    expect(
      appImageNameOf({
        ...linux,
        bundle_type: 'appimage',
        executable: '/home/u/My Apps/Pluryprint (1).AppImage',
      }),
    ).toBe('Pluryprint_0.0.6_linux_x64.AppImage');
    expect(appImageNameOf(null)).toBe('Pluryprint_*.AppImage');
  });
});

describe('appImageFileName', () => {
  it('matches the release asset names', () => {
    expect(appImageFileName('0.0.5', 'x86_64')).toBe(
      'Pluryprint_0.0.5_linux_x64.AppImage',
    );
    expect(appImageFileName('0.0.5', 'aarch64')).toBe(
      'Pluryprint_0.0.5_linux_arm64.AppImage',
    );
  });

  it('falls back to a glob when the version or architecture is unknown', () => {
    expect(appImageFileName()).toBe('Pluryprint_*.AppImage');
    expect(appImageFileName('0.0.5', 'riscv64')).toBe('Pluryprint_*.AppImage');
  });
});

describe('the remembered host', () => {
  it('starts from the webview’s guess and switches to the backend’s word', () => {
    withWebviewPlatform('Win32', () => {
      expect(hostPlatform()).toEqual({
        os: 'windows',
        linuxPackage: null,
        fromBackend: false,
      });
    });
    rememberHostInfo({
      os: 'linux',
      arch: 'aarch64',
      version: '0.0.5',
      executable: '/usr/bin/pluryprint',
    });
    // The backend wins over whatever the webview says from now on.
    withWebviewPlatform('Win32', () => {
      expect(hostPlatform()).toEqual({
        os: 'linux',
        linuxPackage: 'deb',
        fromBackend: true,
      });
    });
    expect(rememberedHostInfo()?.arch).toBe('aarch64');
    resetHostPlatform();
    expect(rememberedHostInfo()).toBeNull();
    expect(withWebviewPlatform('MacIntel', hostOs)).toBe('macos');
  });

  it('keeps one snapshot per state, as useSyncExternalStore requires', () => {
    expect(hostPlatform()).toBe(hostPlatform());
    rememberHostInfo({ os: 'macos' });
    const first = hostPlatform();
    // The same answer again is not a change.
    rememberHostInfo({ os: 'macos' });
    expect(hostPlatform()).toBe(first);
  });

  it('ignores an OS it does not know rather than forgetting the last one', () => {
    rememberHostInfo({ os: 'linux' });
    rememberHostInfo({ os: 'haiku' });
    expect(hostOs()).toBe('linux');
  });
});

describe('platformKeyVariant / linuxMenuKey', () => {
  it('names the macOS and Linux variants, and Windows by the plain key', () => {
    expect(platformKeyVariant('a.remediation', 'macos')).toBe(
      'a.remediationMac',
    );
    expect(platformKeyVariant('a.remediation', 'linux')).toBe(
      'a.remediationLinux',
    );
    expect(platformKeyVariant('a.remediation', 'windows')).toBe(
      'a.remediation',
    );
  });

  it('picks the file-manager sentence by package', () => {
    expect(linuxMenuKey('deb')).toBe('linuxMenu.deb');
    expect(linuxMenuKey('appimage')).toBe('linuxMenu.appimage');
    expect(linuxMenuKey(null)).toBe('linuxMenu.unknown');
  });
});
