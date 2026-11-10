import { describe, expect, it } from 'vitest';

import {
  cliCommandForDisplay,
  cliExecutable,
  cliPrintCommand,
  cliProgram,
  distinguishingFolders,
  formatFinished,
  quoteShellArg,
} from './format';

describe('formatFinished', () => {
  const now = new Date(2026, 6, 12, 18, 0, 0);

  it('shows only the time for a job that finished today', () => {
    const today = new Date(2026, 6, 12, 14, 3, 12).toISOString();
    expect(formatFinished(today, 'en-US', now)).toMatch(/14:03:12|2:03:12/);
    expect(formatFinished(today, 'en-US', now)).not.toMatch(/2026/);
  });

  it('adds the date, in the app locale, for an earlier day', () => {
    const yesterday = new Date(2026, 6, 11, 14, 3, 12).toISOString();
    expect(formatFinished(yesterday, 'en-US', now)).toMatch(/Jul 11, 2026/);
    expect(formatFinished(yesterday, 'es-ES', now)).toMatch(/11 jul 2026/);
  });

  it('gives up on a stamp it cannot parse', () => {
    expect(formatFinished('not a date', 'en-US', now)).toBeNull();
  });
});

describe('quoteShellArg', () => {
  it('leaves shell-safe words alone', () => {
    expect(quoteShellArg('Quick', 'posix')).toBe('Quick');
    expect(quoteShellArg('builtin-quick', 'powershell')).toBe('builtin-quick');
  });

  it('FR-008: single-quotes anything a POSIX shell would split or expand', () => {
    expect(quoteShellArg('Costs $HOME', 'posix')).toBe("'Costs $HOME'");
    expect(quoteShellArg('*.pdf', 'posix')).toBe("'*.pdf'");
    // A quote inside the value closes, escapes and reopens.
    expect(quoteShellArg(`Office 'A'`, 'posix')).toBe(`'Office '\\''A'\\'''`);
    expect(quoteShellArg('Office "A"', 'posix')).toBe(`'Office "A"'`);
  });

  it('FR-008: doubles single quotes for PowerShell', () => {
    expect(quoteShellArg(`Bob's`, 'powershell')).toBe(`'Bob''s'`);
    expect(quoteShellArg('Costs $HOME', 'powershell')).toBe(`'Costs $HOME'`);
  });

  it('FR-008: doubles the typographic single quotes PowerShell also accepts', () => {
    expect(quoteShellArg('a\u2019;calc;\u2019', 'powershell')).toBe(
      "'a\u2019\u2019;calc;\u2019\u2019'",
    );
    for (const q of ['\u2018', '\u2019', '\u201A', '\u201B']) {
      expect(quoteShellArg(q, 'powershell')).toBe(`'${q}${q}'`);
    }
  });

  it('FR-008: quotes PowerShell splatting, array and expression characters', () => {
    for (const s of ['@args', 'a,b', '%x%', '1+1']) {
      expect(quoteShellArg(s, 'powershell')).toBe(`'${s}'`);
    }
    expect(quoteShellArg('C:\\Tools\\pluryprint.exe', 'powershell')).toBe(
      'C:\\Tools\\pluryprint.exe',
    );
  });

  it('FR-008: a typographic quote in a profile name cannot end the PowerShell string', () => {
    expect(quoteShellArg('Office’; Start-Process calc; ’', 'powershell')).toBe(
      "'Office’’; Start-Process calc; ’’'",
    );
    for (const q of ['‘', '‚', '‛']) {
      expect(quoteShellArg(`a${q}b`, 'powershell')).toBe(`'a${q}${q}b'`);
    }
    // POSIX does not treat them as quotes: left alone.
    expect(quoteShellArg('a’b', 'posix')).toBe("'a’b'");
  });
});

describe('cliPrintCommand', () => {
  it('FR-006/FR-008: names the real executable, because it is not on PATH', () => {
    expect(
      cliPrintCommand({
        executable: '/Applications/Pluryprint.app/Contents/MacOS/pluryprint',
        os: 'macos',
        profile: 'Quick',
      }),
    ).toBe(
      "/Applications/Pluryprint.app/Contents/MacOS/pluryprint print --files '*.pdf' --profile Quick",
    );
  });

  it('FR-008/FR-009: on Windows it runs the console pluryprint-cli.exe next to the app, with the PowerShell call operator', () => {
    // pluryprint.exe is a GUI-subsystem program: PowerShell neither waits
    // for it nor shows its output or exit code. The console twin does.
    expect(
      cliPrintCommand({
        executable:
          'C:\\Users\\Ana\\AppData\\Local\\Programs\\Pluryprint App\\pluryprint.exe',
        os: 'windows',
        profile: `Ana's "draft"`,
      }),
    ).toBe(
      `& 'C:\\Users\\Ana\\AppData\\Local\\Programs\\Pluryprint App\\pluryprint-cli.exe' print --files '*.pdf' --profile 'Ana''s "draft"'`,
    );
    // A path PowerShell can run bare still names the console binary.
    expect(
      cliPrintCommand({
        executable: 'C:\\Pluryprint\\Pluryprint.EXE',
        os: 'windows',
        profile: 'Quick',
      }),
    ).toBe(
      "C:\\Pluryprint\\pluryprint-cli.exe print --files '*.pdf' --profile Quick",
    );
  });

  it('FR-009: without a known path, Windows still names pluryprint-cli', () => {
    expect(
      cliPrintCommand({ executable: null, os: 'windows', profile: 'Quick' }),
    ).toBe("pluryprint-cli print --files '*.pdf' --profile Quick");
    expect(cliProgram('windows')).toBe('pluryprint-cli');
    expect(cliProgram('macos')).toBe('pluryprint');
  });

  it('Linux .deb: pluryprint is on PATH, so the command names it bare', () => {
    expect(
      cliPrintCommand({
        executable: '/usr/bin/pluryprint',
        os: 'linux',
        profile: 'Quick',
        linuxPackage: 'deb',
      }),
    ).toBe("pluryprint print --files '*.pdf' --profile Quick");
    expect(cliProgram('linux', { linuxPackage: 'deb' })).toBe('pluryprint');
    expect(
      cliExecutable({
        executable: '/usr/bin/pluryprint',
        os: 'linux',
        linuxPackage: 'deb',
      }),
    ).toBeNull();
  });

  it('Linux AppImage: never the temporary mount, which vanishes on exit', () => {
    const appImage = 'Pluryprint_0.0.5_linux_x64.AppImage';
    expect(
      cliPrintCommand({
        executable: '/tmp/.mount_PluryAb12/usr/bin/pluryprint',
        os: 'linux',
        profile: 'Quick',
        linuxPackage: 'appimage',
        appImage,
      }),
    ).toBe(
      "./Pluryprint_0.0.5_linux_x64.AppImage print --files '*.pdf' --profile Quick",
    );
    // Without a name to offer, a glob that matches the release asset.
    expect(cliProgram('linux', { linuxPackage: 'appimage' })).toBe(
      './Pluryprint_*.AppImage',
    );
  });

  it('Linux AppImage: the AppImage file itself, quoted, when the backend names it', () => {
    expect(
      cliPrintCommand({
        executable: '/home/ana/My Apps/Pluryprint_0.0.5_linux_arm64.AppImage',
        os: 'linux',
        profile: 'Quick',
        linuxPackage: 'appimage',
      }),
    ).toBe(
      "'/home/ana/My Apps/Pluryprint_0.0.5_linux_arm64.AppImage' print --files '*.pdf' --profile Quick",
    );
  });

  it('Linux, package unknown: the executable path, as on macOS', () => {
    expect(
      cliPrintCommand({
        executable: '/home/ana/pluryprint/target/release/pluryprint',
        os: 'linux',
        profile: 'Quick',
      }),
    ).toBe(
      "/home/ana/pluryprint/target/release/pluryprint print --files '*.pdf' --profile Quick",
    );
    expect(cliProgram('linux')).toBe('pluryprint');
  });

  it('falls back to the bare command name when the executable is unknown', () => {
    expect(
      cliPrintCommand({ executable: null, os: undefined, profile: 'Quick' }),
    ).toBe("pluryprint print --files '*.pdf' --profile Quick");
  });
});

describe('distinguishingFolders', () => {
  it('FR-007: names the folder only for files whose names collide', () => {
    const out = distinguishingFolders([
      '/Users/e2e/a/inv-001.pdf',
      '/Users/e2e/Documents/Invoices/inv-001.pdf',
      '/Users/e2e/Documents/unique.pdf',
    ]);
    expect(out.get('/Users/e2e/a/inv-001.pdf')).toBe('a');
    expect(out.get('/Users/e2e/Documents/Invoices/inv-001.pdf')).toBe(
      'Invoices',
    );
    expect(out.has('/Users/e2e/Documents/unique.pdf')).toBe(false);
  });

  it('FR-007: walks up until the folders differ, on either separator', () => {
    const out = distinguishingFolders([
      'C:\\2024\\Invoices\\INV.pdf',
      'C:\\2025\\Invoices\\inv.pdf',
    ]);
    expect(out.get('C:\\2024\\Invoices\\INV.pdf')).toBe('2024/Invoices');
    expect(out.get('C:\\2025\\Invoices\\inv.pdf')).toBe('2025/Invoices');
  });
});

describe('cliCommandForDisplay', () => {
  it('folds the executable folder into an ellipsis, keeping the quotes', () => {
    const exe = '/Applications/Pluryprint App.app/Contents/MacOS/pluryprint';
    const full = cliPrintCommand({
      executable: exe,
      os: 'macos',
      profile: 'Quick',
    });
    expect(cliCommandForDisplay(full, exe)).toBe(
      "'…/pluryprint' print --files '*.pdf' --profile Quick",
    );
  });

  it('works with Windows paths and the console program next to the app', () => {
    const exe = 'C:\\Program Files\\Pluryprint\\pluryprint.exe';
    const full = cliPrintCommand({
      executable: exe,
      os: 'windows',
      profile: 'Quick',
    });
    expect(cliCommandForDisplay(full, exe)).toBe(
      "& '…\\pluryprint-cli.exe' print --files '*.pdf' --profile Quick",
    );
  });

  it('leaves the command alone when there is no executable', () => {
    const full = cliPrintCommand({
      executable: null,
      os: 'macos',
      profile: 'Quick',
    });
    expect(cliCommandForDisplay(full, null)).toBe(full);
  });
});
