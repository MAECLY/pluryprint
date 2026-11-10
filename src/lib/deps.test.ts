import { describe, expect, it } from 'vitest';

import { CATALOGS, translate } from '@/i18n';
import { missingDependency, remediationKey } from '@/lib/deps';
import { rememberHostInfo } from '@/lib/platform';

describe('remediationKey (FR-058)', () => {
  it('FR-058: LibreOffice names RTF on Windows only — macOS prints RTF without it', () => {
    for (const locale of ['en-US', 'es-ES'] as const) {
      const catalog = CATALOGS[locale];
      const win = translate(
        catalog,
        'common',
        remediationKey('libre-office', { mac: false, locale }),
      );
      const mac = translate(
        catalog,
        'common',
        remediationKey('libre-office', { mac: true, locale }),
      );
      expect(win).toContain('RTF');
      expect(mac).not.toContain('RTF');
      expect(mac).toContain('OpenDocument');
    }
  });

  it('FR-058: on Linux, LibreOffice comes from the distribution and still covers RTF', () => {
    for (const locale of ['en-US', 'es-ES'] as const) {
      const key = remediationKey('libre-office', { os: 'linux', locale });
      expect(key).toBe('deps.libre-office.remediationLinux');
      const text = translate(CATALOGS[locale], 'common', key);
      expect(text).toContain('RTF');
      expect(text).toContain('apt install libreoffice');
    }
  });

  it('FR-058: the host platform decides when no OS is given', () => {
    rememberHostInfo({ os: 'linux' });
    expect(remediationKey('libre-office')).toBe(
      'deps.libre-office.remediationLinux',
    );
    expect(remediationKey('cups')).toBe('deps.cups.remediationLinux');
    // No Linux variant: the one sentence.
    expect(remediationKey('sumatra-pdf')).toBe('deps.sumatra-pdf.remediation');
    rememberHostInfo({ os: 'macos' });
    expect(remediationKey('libre-office')).toBe(
      'deps.libre-office.remediationMac',
    );
  });

  it('FR-058: the Linux HEIF decoder names the packages that provide it', () => {
    for (const locale of ['en-US', 'es-ES'] as const) {
      const text = translate(
        CATALOGS[locale],
        'common',
        remediationKey('heif-decoder', { os: 'linux', locale }),
      );
      expect(text).toContain('libheif-freeworld');
      expect(text).toContain('libheif-examples');
      expect(text).toContain('ImageMagick');
      expect(text).toMatch(/resumed|reanudar/);
    }
  });

  it('FR-058: a dependency without a macOS variant keeps its one sentence', () => {
    expect(remediationKey('sumatra-pdf', { mac: true })).toBe(
      'deps.sumatra-pdf.remediation',
    );
  });

  it('FR-058: the codec remediation points at Resume, the action a paused job has', () => {
    for (const locale of ['en-US', 'es-ES'] as const) {
      for (const id of ['heif-codec', 'hevc-codec']) {
        const text = translate(
          CATALOGS[locale],
          'common',
          remediationKey(id, { mac: false, locale }),
        );
        expect(text).not.toMatch(/retry|vuelve a intentar/i);
        expect(text).toMatch(/resumed|reanudar/);
      }
    }
  });

  it('FR-058: missingDependency reads the dependency of a paused job only', () => {
    const errorInfo = {
      code: 'dependency_missing',
      message: 'x',
      reason: 'libre-office',
    };
    expect(missingDependency({ status: 'paused', errorInfo })).toBe(
      'libre-office',
    );
    expect(missingDependency({ status: 'failed', errorInfo })).toBeNull();
    expect(
      missingDependency({
        status: 'paused',
        errorInfo: { ...errorInfo, code: 'printer_unavailable' },
      }),
    ).toBeNull();
  });
});
