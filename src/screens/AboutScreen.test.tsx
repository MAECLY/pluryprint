import {
  cleanup,
  render,
  screen,
  fireEvent,
  waitFor,
  within,
} from '@testing-library/react';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { version as REACT_VERSION } from 'react';
import tauriPkg from '@tauri-apps/api/package.json';

import { AboutScreen } from './AboutScreen';
import { MPSidebar } from '@/components/AppShell/MPSidebar';
import { Toaster } from '@/components/AppShell/Toaster';
import { I18nProvider } from '@/i18n/I18nProvider';
import {
  APP_LICENSE,
  CLA_URL,
  DOCS_URL,
  LICENSE_URL,
  NOTICES_URL,
  RELEASES_URL,
  REPO_URL,
  UNKNOWN_VERSION,
} from '@/lib/repo';
import {
  fixtureAppInfo,
  fixtureAppSettings,
  fixtureUpdateCheckAvailable,
  fixtureUpdateCheckUpToDate,
} from '@/test/fixtures';
import type { AppSettings, UpdateCheck } from '@/lib/ipc';

// A few assertions read files instead of rendering: they guard the repository
// slug and the "no hardcoded version" rule at the source level, which no amount
// of rendering can catch. Vitest runs from the project root.
const REPO_ROOT = process.cwd();

// Mock the settings store so these tests own only the About screen. The
// channel suffix reads `settings.updater.channel`, so we drive `settings`
// directly rather than through the real store's IPC path.
const { settingsState } = vi.hoisted(() => ({
  settingsState: {
    settings: null as AppSettings | null,
    init: () => Promise.resolve(),
    // `I18nProvider` writes the detected locale back through `patch`; the
    // Spanish tests below mount it, so the mock has to answer that selector.
    patch: () => Promise.resolve(),
  },
}));

vi.mock('@/stores/settingsStore', () => ({
  useSettingsStore: (selector: (s: typeof settingsState) => unknown) =>
    selector(settingsState),
}));

interface Call {
  cmd: string;
  args: Record<string, unknown>;
}

interface MockOpts {
  /** Override the `app_info` payload. */
  appInfo?: unknown;
  /** Return value for `check_for_updates`. */
  update?: unknown;
  /** When set, `check_for_updates` throws with this message. */
  updateError?: string;
  /** When set, `check_for_updates` returns this command-level IpcError. */
  updateIpcError?: Record<string, unknown>;
  /** Version already recorded as skipped in the persisted update state. */
  skippedVersion?: string | null;
  /** Return value for `install_update`. */
  installResult?: unknown;
  /** When set, `install_update` rejects with this IPC error. */
  installError?: { code: string; message: string };
}

/** Install IPC mocks (which also flags the Tauri host) and record the calls. */
function mockAbout(opts: MockOpts = {}): Call[] {
  const calls: Call[] = [];
  // Mutable so `set_skipped_update_version` is observable by a later read,
  // which is what makes the persistence assertions meaningful.
  let skipped: string | null = opts.skippedVersion ?? null;
  mockIPC(
    (cmd, args) => {
      calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> });
      if (cmd === 'app_info') return opts.appInfo ?? fixtureAppInfo;
      if (cmd === 'get_update_state') {
        return { last_check_at: null, skipped_version: skipped };
      }
      if (cmd === 'set_skipped_update_version') {
        skipped = (args as { version: string | null }).version;
        return null;
      }
      if (cmd === 'check_for_updates') {
        if (opts.updateError) throw new Error(opts.updateError);
        // A plain object is what a command-returned IpcError looks like on the
        // raw invoke path (the bindings turn it into the error branch).
        if (opts.updateIpcError) throw opts.updateIpcError;
        return opts.update ?? fixtureUpdateCheckUpToDate;
      }
      if (cmd === 'install_update') {
        // A plain object, not an Error: that is how a command-returned IpcError
        // reaches the bindings, which turn it into `{ status: 'error' }`.
        if (opts.installError) throw opts.installError;
        return (
          opts.installResult ?? { installed: '1.1.0', restart_required: true }
        );
      }
      // open_external / open_logs_folder / report_issue resolve with null.
      return null;
      // The screen listens for update-download progress; mocking the event
      // plugin gives `listen` a real unlisten instead of an unhandled rejection
      // on unmount.
    },
    { shouldMockEvents: true },
  );
  return calls;
}

/** Render with a mounted Toaster so any error-path toast has somewhere to land. */
function renderAbout() {
  return render(
    <>
      <AboutScreen />
      <Toaster theme="light" />
    </>,
  );
}

/** Render inside the real i18n provider, which reads the mocked settings. */
function renderTranslatedAbout() {
  return render(
    <I18nProvider>
      <AboutScreen />
      <Toaster theme="light" />
    </I18nProvider>,
  );
}

/**
 * The version line only shows once `app_info` has resolved. Matched on the
 * build date alone so the helper works in every locale (the surrounding
 * "build"/"compilación" wording is translated).
 */
function waitForAppInfo() {
  return screen.findByText(/2026-07-01/);
}

describe('AboutScreen', () => {
  beforeEach(() => {
    settingsState.settings = null;
    // clearMocks() only strips the invoke fn but leaves the __TAURI_INTERNALS__
    // shell on window, which keeps isTauri() true. Remove the key so a test
    // that installs no IPC mock sees a genuinely host-less window.
    delete (window as unknown as Record<string, unknown>)[
      '__TAURI_INTERNALS__'
    ];
  });
  afterEach(() => {
    // Unmount before the mocks go: the screen's update-progress listener
    // unregisters itself on unmount, which needs the mocked event plugin.
    cleanup();
    clearMocks();
  });

  it('FR-110: shows the version and opens the license and docs links', async () => {
    const calls = mockAbout();
    renderAbout();

    // Version line is sourced from app_info (version · build date).
    expect(await waitForAppInfo()).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /license agpl-3\.0/i }));
    fireEvent.click(screen.getByRole('button', { name: /documentation/i }));

    await waitFor(() => {
      const urls = calls
        .filter((c) => c.cmd === 'open_external')
        .map((c) => c.args.url);
      expect(urls).toContain(LICENSE_URL);
      expect(urls).toContain(DOCS_URL);
    });
  });

  it('opens the third-party notices and the contributor agreement', async () => {
    const calls = mockAbout();
    renderAbout();
    expect(await waitForAppInfo()).toBeInTheDocument();

    fireEvent.click(
      screen.getByRole('button', { name: /third-party notices/i }),
    );
    fireEvent.click(
      screen.getByRole('button', { name: /contributor agreement \(cla\)/i }),
    );

    await waitFor(() => {
      const urls = calls
        .filter((c) => c.cmd === 'open_external')
        .map((c) => c.args.url);
      expect(urls).toContain(NOTICES_URL);
      expect(urls).toContain(CLA_URL);
    });
  });

  it('names the product with its maker: Pluryprint by Maecly', async () => {
    mockAbout();
    renderAbout();

    expect(await waitForAppInfo()).toBeInTheDocument();
    expect(screen.getByText('by Maecly')).toBeInTheDocument();
    expect(screen.getByText('by Maecly').parentElement).toHaveTextContent(
      /^Pluryprint by Maecly$/,
    );
  });

  it('renders the standalone fallback version line and disables update actions outside Tauri', async () => {
    // No mockIPC → not a Tauri host: fetchAppInfo rejects, actions are disabled.
    renderAbout();

    expect(
      await screen.findByText(/standalone web preview/),
    ).toBeInTheDocument();
    expect(await screen.findByText(/IPC unavailable/i)).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: /check for updates/i }),
    ).toBeDisabled();
  });

  it('checks for updates: shows a checking state, then an up-to-date card with the checked time', async () => {
    mockAbout({ update: fixtureUpdateCheckUpToDate });
    renderAbout();
    await waitForAppInfo();

    fireEvent.click(screen.getByRole('button', { name: /check for updates/i }));

    // Transient checking state (spinner + disabled button).
    expect(screen.getByRole('button', { name: /checking/i })).toBeDisabled();

    // Resolves to a real up-to-date card with the formatted checked_at time.
    expect(await screen.findByText(/up to date/i)).toBeInTheDocument();
    expect(screen.getByText(/checked \d{2}:\d{2}/)).toBeInTheDocument();
    expect(screen.getByText(/version 1\.0\.0 · checked/i)).toBeInTheDocument();
  });

  it('shows an update-available card and opens the specific release notes URL', async () => {
    const calls = mockAbout({ update: fixtureUpdateCheckAvailable });
    renderAbout();
    await waitForAppInfo();

    fireEvent.click(screen.getByRole('button', { name: /check for updates/i }));

    const card = await screen.findByRole('group', {
      name: /update available/i,
    });
    expect(
      within(card).getByText(/version 9\.9\.9 is available/i),
    ).toBeInTheDocument();

    fireEvent.click(
      within(card).getByRole('button', { name: /release notes/i }),
    );

    await waitFor(() => {
      const ext = calls.find((c) => c.cmd === 'open_external');
      expect(ext?.args.url).toBe(fixtureUpdateCheckAvailable.url);
    });
  });

  it('surfaces the thrown update-check error inline, and retry re-runs the check', async () => {
    const calls = mockAbout({
      updateError: 'GitHub rate limit reached — try again in a bit.',
    });
    renderAbout();
    await waitForAppInfo();

    fireEvent.click(screen.getByRole('button', { name: /check for updates/i }));
    expect(
      await screen.findByText(/github rate limit reached/i),
    ).toBeInTheDocument();

    const before = calls.filter((c) => c.cmd === 'check_for_updates').length;
    fireEvent.click(screen.getByRole('button', { name: /retry/i }));
    await waitFor(() =>
      expect(
        calls.filter((c) => c.cmd === 'check_for_updates').length,
      ).toBeGreaterThan(before),
    );
  });

  it('treats "no releases yet" (available:false, latest:null) as an honest up-to-date state', async () => {
    const noReleases: UpdateCheck = {
      current: '1.0.0',
      latest: null,
      available: false,
      url: null,
      notes: null,
      checked_at: '2026-07-11T10:00:00Z',
    };
    mockAbout({ update: noReleases });
    renderAbout();
    await waitForAppInfo();

    fireEvent.click(screen.getByRole('button', { name: /check for updates/i }));

    expect(await screen.findByText(/up to date/i)).toBeInTheDocument();
    expect(screen.queryByText(/available/i)).not.toBeInTheDocument();
    expect(
      screen.queryByText(/rate limit|could not reach/i),
    ).not.toBeInTheDocument();
  });

  it('FR-085: a channel with no published manifest is a neutral notice, not "up to date" and not an error', async () => {
    settingsState.settings = {
      ...fixtureAppSettings,
      updater: { ...fixtureAppSettings.updater, channel: 'beta' },
    };
    const calls = mockAbout({
      updateIpcError: {
        code: 'UpdateCheckFailed',
        reason: 'channel_unavailable',
        message:
          'no update has been published on this channel (its manifest answered 404)',
      },
    });
    renderAbout();
    await waitForAppInfo();

    fireEvent.click(screen.getByRole('button', { name: /check for updates/i }));

    const notice = await screen.findByRole('status');
    expect(notice).toHaveTextContent(
      'No updates published on the beta channel',
    );
    expect(notice).toHaveTextContent(/releases are not public/);
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.queryByText(/up to date/i)).toBeNull();
    // No toast either: it is not a failure.
    expect(screen.queryByText(/answered 404/)).toBeNull();

    // The release page and a fresh check are one click away.
    fireEvent.click(within(notice).getByRole('button', { name: /retry/i }));
    await waitFor(() =>
      expect(calls.filter((c) => c.cmd === 'check_for_updates').length).toBe(2),
    );
  });

  it('files an issue and opens the logs folder through IPC', async () => {
    const calls = mockAbout();
    renderAbout();
    await waitForAppInfo();

    fireEvent.click(screen.getByRole('button', { name: /report an issue/i }));
    fireEvent.click(screen.getByRole('button', { name: /open logs folder/i }));

    await waitFor(() => {
      expect(calls.some((c) => c.cmd === 'report_issue')).toBe(true);
      expect(calls.some((c) => c.cmd === 'open_logs_folder')).toBe(true);
    });
  });

  it('lists the real React and Tauri versions in the components card', async () => {
    mockAbout();
    renderAbout();
    await waitForAppInfo();

    // Sourced from react's runtime `version` export and @tauri-apps/api's
    // own package.json — asserted against the very values the app imports.
    expect(screen.getByText(REACT_VERSION)).toBeInTheDocument();
    expect(screen.getByText(tauriPkg.version)).toBeInTheDocument();
    // The removed bundled-tooling rows must be gone until they are real.
    expect(screen.queryByText(/SumatraPDF|LibreOffice|ImageMagick/)).toBeNull();
  });

  it('does not render the removed hardcoded "12 minutes ago" placeholder', async () => {
    mockAbout();
    renderAbout();
    await waitForAppInfo();

    expect(screen.queryByText(/12 minutes/i)).toBeNull();
    expect(screen.queryByText(/last checked/i)).toBeNull();
  });

  it('appends the update channel from settings to the version line', async () => {
    settingsState.settings = fixtureAppSettings; // channel: 'stable'
    mockAbout();
    renderAbout();

    expect(await screen.findByText(/stable channel/i)).toBeInTheDocument();
  });

  it('opens the releases page from the persistent Release notes link', async () => {
    const calls = mockAbout();
    renderAbout();
    await waitForAppInfo();

    fireEvent.click(screen.getByRole('button', { name: /release notes/i }));
    await waitFor(() =>
      expect(
        calls.some(
          (c) => c.cmd === 'open_external' && c.args.url === RELEASES_URL,
        ),
      ).toBe(true),
    );
  });

  it('opens the source repository from the footer link', async () => {
    const calls = mockAbout();
    renderAbout();
    await waitForAppInfo();

    fireEvent.click(
      screen.getByRole('button', {
        name: /github\.com\/MAECLY\/pluryprint/i,
      }),
    );
    await waitFor(() =>
      expect(
        calls.some((c) => c.cmd === 'open_external' && c.args.url === REPO_URL),
      ).toBe(true),
    );
  });

  it('FR-110: renders the version app_info reports, not a frontend literal', async () => {
    // A version deliberately unlike anything written anywhere in the tree: it
    // can only reach the screen through the backend payload.
    mockAbout({ appInfo: { ...fixtureAppInfo, version: '4.5.6' } });
    renderAbout();

    expect(await screen.findByText(/^4\.5\.6 · build/)).toBeInTheDocument();
    // …and the components card echoes the same value.
    expect(screen.getByText('4.5.6')).toBeInTheDocument();
    // The retired hardcoded fallbacks must not reappear.
    expect(screen.queryByText(/0\.0\.1/)).toBeNull();
  });

  // -- FR-084 (defer / install / skip) ------------------------------------

  it('FR-084: "Remind me later" defers the update card without losing the check', async () => {
    mockAbout({ update: fixtureUpdateCheckAvailable });
    renderAbout();
    await waitForAppInfo();

    fireEvent.click(screen.getByRole('button', { name: /check for updates/i }));
    const card = await screen.findByRole('group', {
      name: /update available/i,
    });

    fireEvent.click(
      within(card).getByRole('button', { name: /remind me later/i }),
    );

    // The nagging card is gone…
    await waitFor(() =>
      expect(
        screen.queryByRole('group', { name: /update available/i }),
      ).toBeNull(),
    );
    // …replaced by an honest note, not by pretending no check ever ran.
    expect(screen.getByText(/reminder postponed/i)).toBeInTheDocument();
    expect(screen.queryByText(/up to date/i)).toBeNull();
  });

  it('FR-084: a fresh check after deferring brings the update card back', async () => {
    mockAbout({ update: fixtureUpdateCheckAvailable });
    renderAbout();
    await waitForAppInfo();

    fireEvent.click(screen.getByRole('button', { name: /check for updates/i }));
    fireEvent.click(
      within(
        await screen.findByRole('group', { name: /update available/i }),
      ).getByRole('button', { name: /remind me later/i }),
    );
    await waitFor(() => expect(screen.getByText(/postponed/i)).toBeVisible());

    // Deferring is not "skip forever": the button is still live and the next
    // check surfaces the same update again.
    fireEvent.click(screen.getByRole('button', { name: /check for updates/i }));
    expect(
      await screen.findByRole('group', { name: /update available/i }),
    ).toBeInTheDocument();
  });

  it('FR-084: skipping a version persists it and silences the card', async () => {
    const calls = mockAbout({ update: fixtureUpdateCheckAvailable });
    renderAbout();
    await waitForAppInfo();

    fireEvent.click(screen.getByRole('button', { name: /check for updates/i }));
    fireEvent.click(
      within(
        await screen.findByRole('group', { name: /update available/i }),
      ).getByRole('button', { name: /skip this version/i }),
    );

    // The version travels to the backend, which is what makes it survive a
    // restart — unlike "remind me later", which is session-scoped by design.
    await waitFor(() => {
      const call = calls.find((c) => c.cmd === 'set_skipped_update_version');
      expect(call?.args.version).toBe('9.9.9');
    });
    expect(
      screen.queryByRole('group', { name: /update available/i }),
    ).toBeNull();
    expect(screen.getByText(/skipping 9\.9\.9/i)).toBeInTheDocument();
  });

  it('FR-084: a version skipped in a previous run is still skipped on open', async () => {
    mockAbout({ skippedVersion: '9.9.9' });
    renderAbout();
    await waitForAppInfo();

    // No check has run in this session; the state comes from disk.
    expect(await screen.findByText(/skipping 9\.9\.9/i)).toBeInTheDocument();
  });

  it('FR-084: undoing a skip clears it and offers the release again', async () => {
    const calls = mockAbout({
      skippedVersion: '9.9.9',
      update: fixtureUpdateCheckAvailable,
    });
    renderAbout();
    await waitForAppInfo();
    await screen.findByText(/skipping 9\.9\.9/i);

    fireEvent.click(screen.getByRole('button', { name: /undo/i }));

    await waitFor(() => {
      const cleared = calls.filter(
        (c) =>
          c.cmd === 'set_skipped_update_version' && c.args.version === null,
      );
      expect(cleared).toHaveLength(1);
    });
    expect(
      await screen.findByRole('group', { name: /update available/i }),
    ).toBeInTheDocument();
  });

  /** A minor update from the running 1.0.0. */
  const MINOR_UPDATE: UpdateCheck = {
    ...fixtureUpdateCheckAvailable,
    latest: '1.1.0',
    url: 'https://github.com/MAECLY/pluryprint/releases/tag/v1.1.0',
  };

  async function openUpdateCard() {
    fireEvent.click(screen.getByRole('button', { name: /check for updates/i }));
    return screen.findByRole('group', { name: /update available/i });
  }

  it('FR-084: "Install now" installs the update and offers a restart', async () => {
    const calls = mockAbout({ update: MINOR_UPDATE });
    renderAbout();
    await waitForAppInfo();

    const card = await openUpdateCard();
    // FR-082/FR-083 are real now, and the card says so.
    expect(within(card).getByText(/release signature/i)).toBeInTheDocument();
    fireEvent.click(within(card).getByRole('button', { name: 'Install now' }));

    expect(await screen.findByText('1.1.0 is installed')).toBeInTheDocument();
    const install = calls.find((c) => c.cmd === 'install_update');
    // A minor update needs no extra opt-in.
    expect(install?.args.allowMajor).toBe(false);

    fireEvent.click(screen.getByRole('button', { name: 'Restart now' }));
    await waitFor(() =>
      expect(calls.some((c) => c.cmd === 'restart_app')).toBe(true),
    );
  });

  it('FR-083: a .deb install warns that the system will ask for an administrator password', async () => {
    mockAbout({
      update: MINOR_UPDATE,
      appInfo: {
        ...fixtureAppInfo,
        os: 'linux',
        arch: 'x86_64',
        executable: '/usr/bin/pluryprint',
      },
    });
    renderAbout();
    await waitForAppInfo();

    const card = await openUpdateCard();
    expect(
      within(card).getByText(/asks for an administrator password/i),
    ).toBeInTheDocument();
    expect(
      within(card).queryByText(/no administrator rights are needed/i),
    ).toBeNull();
    // The platform line names the package, which decides how updates work.
    expect(
      screen.getByText('linux · x86_64 · .deb package'),
    ).toBeInTheDocument();
  });

  it('FR-083: an AppImage is replaced in place, with no administrator rights', async () => {
    mockAbout({
      update: MINOR_UPDATE,
      appInfo: {
        ...fixtureAppInfo,
        os: 'linux',
        arch: 'aarch64',
        executable: '/tmp/.mount_PluryQ1/usr/bin/pluryprint',
      },
    });
    renderAbout();
    await waitForAppInfo();

    const card = await openUpdateCard();
    expect(
      within(card).getByText(/replaces this AppImage file where it is/i),
    ).toBeInTheDocument();
    expect(screen.getByText('linux · aarch64 · AppImage')).toBeInTheDocument();
  });

  it('FR-087: a major update is installed only after a second, explicit confirmation', async () => {
    const calls = mockAbout({
      update: fixtureUpdateCheckAvailable, // 1.0.0 → 9.9.9
      installResult: { installed: '9.9.9', restart_required: true },
    });
    renderAbout();
    await waitForAppInfo();

    const card = await openUpdateCard();
    fireEvent.click(within(card).getByRole('button', { name: 'Install now' }));

    const dialog = await screen.findByRole('dialog');
    expect(
      within(dialog).getByText('Install a major update?'),
    ).toBeInTheDocument();
    // Nothing is downloaded on the first click.
    expect(calls.some((c) => c.cmd === 'install_update')).toBe(false);

    fireEvent.click(
      within(dialog).getByRole('button', { name: 'Install 9.9.9' }),
    );
    await waitFor(() => {
      const install = calls.find((c) => c.cmd === 'install_update');
      expect(install?.args.allowMajor).toBe(true);
      // The confirmation is bound to the version it named.
      expect(install?.args.confirmedVersion).toBe('9.9.9');
    });
  });

  it('FR-087: dismissing the major-update question installs nothing', async () => {
    const calls = mockAbout({ update: fixtureUpdateCheckAvailable });
    renderAbout();
    await waitForAppInfo();

    const card = await openUpdateCard();
    fireEvent.click(within(card).getByRole('button', { name: 'Install now' }));
    const dialog = await screen.findByRole('dialog');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));

    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(calls.some((c) => c.cmd === 'install_update')).toBe(false);
  });

  it('speaks to users, not developers: no implementation language or milestones', async () => {
    mockAbout({ update: MINOR_UPDATE });
    renderAbout();
    await waitForAppInfo();

    expect(screen.getByText('File locations')).toBeInTheDocument();
    const text = document.body.textContent ?? '';
    expect(text).not.toMatch(/\bRust\b/);
    expect(text).not.toMatch(/\bM\d\+?/);
  });

  it('FR-092: an install failure states its prefix once', async () => {
    mockAbout({
      update: MINOR_UPDATE,
      installError: {
        code: 'UpdateInstallFailed',
        message: 'disk full',
      },
    });
    renderAbout();
    await waitForAppInfo();

    const card = await openUpdateCard();
    fireEvent.click(within(card).getByRole('button', { name: 'Install now' }));

    const msg = await screen.findByText(/could not be installed/i);
    expect(msg.textContent).toBe(
      'The update could not be installed: disk full',
    );
  });

  it('FR-082: a tampered download is rejected with a clear message and a way to the logs', async () => {
    const calls = mockAbout({
      update: MINOR_UPDATE,
      installError: {
        code: 'UpdateSignatureInvalid',
        message: 'the downloaded update failed its signature check',
      },
    });
    renderAbout();
    await waitForAppInfo();

    const card = await openUpdateCard();
    fireEvent.click(within(card).getByRole('button', { name: 'Install now' }));

    expect(
      await screen.findByText(
        /failed its signature check and was not installed/i,
      ),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/Nothing on this computer was changed/),
    ).toBeVisible();
    fireEvent.click(
      screen.getAllByRole('button', { name: /open (the )?logs folder/i })[0]!,
    );
    await waitFor(() =>
      expect(calls.some((c) => c.cmd === 'open_logs_folder')).toBe(true),
    );
  });

  it('FR-084: a platform without an installable artifact says so instead of pretending', async () => {
    mockAbout({
      update: MINOR_UPDATE,
      installResult: { installed: null, restart_required: false },
    });
    renderAbout();
    await waitForAppInfo();

    const card = await openUpdateCard();
    fireEvent.click(within(card).getByRole('button', { name: 'Install now' }));
    expect(
      await screen.findByText(/no installable update for this platform/i),
    ).toBeInTheDocument();
  });

  // -- i18n (FR-092) ------------------------------------------------------

  it('FR-092: renders the screen in Spanish when the locale is es-ES', async () => {
    settingsState.settings = {
      ...fixtureAppSettings,
      app: { ...fixtureAppSettings.app, locale: 'es-ES' },
    };
    mockAbout();
    renderTranslatedAbout();

    expect(
      await screen.findByRole('button', { name: /buscar actualizaciones/i }),
    ).toBeInTheDocument();
    expect(screen.getByText(/impresión por lotes/i)).toBeInTheDocument();
    expect(screen.getByText('Componentes')).toBeInTheDocument();
    expect(screen.getByText('Enlaces')).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: /informar de un problema/i }),
    ).toBeInTheDocument();
    // …and none of the English originals leaked through.
    expect(screen.queryByText('Check for updates')).toBeNull();
    expect(screen.queryByText('Report an issue')).toBeNull();
  });

  it('FR-092/NFR-044: a failed check is announced, in Spanish, from its stable reason', async () => {
    settingsState.settings = {
      ...fixtureAppSettings,
      app: { ...fixtureAppSettings.app, locale: 'es-ES' },
    };
    mockAbout({
      updateIpcError: {
        code: 'UpdateCheckFailed',
        reason: 'offline',
        message: 'could not reach github.com — check your connection',
      },
    });
    renderTranslatedAbout();
    await waitForAppInfo();

    fireEvent.click(
      screen.getByRole('button', { name: /buscar actualizaciones/i }),
    );
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(
      'No se pudo conectar con github.com. Comprueba tu conexión.',
    );
    expect(screen.queryByText(/check your connection/)).toBeNull();
  });

  it('FR-085: the unavailable-channel notice is translated', async () => {
    settingsState.settings = {
      ...fixtureAppSettings,
      app: { ...fixtureAppSettings.app, locale: 'es-ES' },
      updater: { ...fixtureAppSettings.updater, channel: 'stable' },
    };
    mockAbout({
      updateIpcError: {
        code: 'UpdateCheckFailed',
        reason: 'channel_unavailable',
        message: 'no update has been published on this channel',
      },
    });
    renderTranslatedAbout();
    await waitForAppInfo();

    fireEvent.click(
      screen.getByRole('button', { name: /buscar actualizaciones/i }),
    );
    const notice = await screen.findByRole('status');
    expect(notice).toHaveTextContent(
      'No hay actualizaciones publicadas en el canal estable',
    );
  });

  it('FR-092: translates the update-available card, including its aria-label', async () => {
    settingsState.settings = {
      ...fixtureAppSettings,
      app: { ...fixtureAppSettings.app, locale: 'es-ES' },
    };
    mockAbout({ update: fixtureUpdateCheckAvailable });
    renderTranslatedAbout();
    await waitForAppInfo();

    fireEvent.click(
      screen.getByRole('button', { name: /buscar actualizaciones/i }),
    );

    const card = await screen.findByRole('group', {
      name: /actualización disponible/i,
    });
    expect(
      within(card).getByText(/la versión 9\.9\.9 está disponible/i),
    ).toBeInTheDocument();
    expect(
      within(card).getByRole('button', { name: /recordármelo más tarde/i }),
    ).toBeInTheDocument();
  });

  it('shows the unknown-version placeholder, not a version number, without a backend', async () => {
    // No mockIPC → fetchAppInfo rejects; there is no version to report.
    renderAbout();

    expect(
      await screen.findByText(`${UNKNOWN_VERSION} · standalone web preview`),
    ).toBeInTheDocument();
    // The Pluryprint row of the components card falls back to the same
    // placeholder rather than repeating a stale number.
    expect(screen.queryByText(/\d+\.\d+\.\d+-dev/)).toBeNull();
  });
});

/**
 * The sidebar footer used to print a hand-written `v0.0.1`, which would have
 * lied from the first release bump onwards. These live here, next to the About
 * assertions, because both surfaces read the same authoritative `app_info`.
 */
describe('MPSidebar version footer', () => {
  beforeEach(() => {
    delete (window as unknown as Record<string, unknown>)[
      '__TAURI_INTERNALS__'
    ];
  });
  afterEach(() => clearMocks());

  it('renders the version reported by app_info', async () => {
    mockIPC((cmd) => (cmd === 'app_info' ? fixtureAppInfo : null));
    render(<MPSidebar active="home" />);

    expect(await screen.findByText(`v${fixtureAppInfo.version}`)).toBeVisible();
    expect(screen.getByText(/connected/)).toBeInTheDocument();
  });

  it('shows the placeholder while app_info is still in flight', () => {
    mockIPC((cmd) => (cmd === 'app_info' ? fixtureAppInfo : null));
    render(<MPSidebar active="home" />);

    // Synchronously after mount the invoke has not resolved: no version yet,
    // and the status dot says so instead of claiming a live backend.
    expect(screen.getByText(UNKNOWN_VERSION)).toBeInTheDocument();
    expect(screen.getByText(/connecting/)).toBeInTheDocument();
  });

  it('falls back to the placeholder outside the Tauri host', async () => {
    render(<MPSidebar active="home" />);

    expect(await screen.findByText(/no backend/)).toBeInTheDocument();
    expect(screen.getByText(UNKNOWN_VERSION)).toBeInTheDocument();
  });

  it('regression: no version number is written by hand in the sidebar source', () => {
    const source = readFileSync(
      join(REPO_ROOT, 'src/components/AppShell/MPSidebar.tsx'),
      'utf8',
    );
    // Any `x.y.z` literal in this file is a version being duplicated: the only
    // authoritative source is `app_info().version` (Rust `CARGO_PKG_VERSION`).
    expect(source).not.toMatch(/\d+\.\d+\.\d+/);
  });
});

/**
 * Repository identity. The slug lives in exactly two places — `repository` in
 * `src-tauri/Cargo.toml` (what Rust reads for the update check and the issue
 * URL) and `REPO_URL` here — so these assertions keep them from drifting and
 * keep the derived links pointing at paths that really exist.
 */
describe('project identity (src/lib/repo.ts)', () => {
  it('mirrors the license declared in both manifests', () => {
    // `APP_LICENSE` is rendered twice on the screen (the Components row and the
    // footer). The manifests declare the precise SPDX id (`AGPL-3.0-only`); the
    // UI shows the short family name, so this pins the prefix rather than an
    // exact string, and still fails the day the project relicenses.
    const cargo = readFileSync(join(REPO_ROOT, 'src-tauri/Cargo.toml'), 'utf8');
    const cargoLicense = /^license\s*=\s*"([^"]+)"/m.exec(cargo)?.[1];
    const pkgLicense = (
      JSON.parse(
        readFileSync(join(REPO_ROOT, 'package.json'), 'utf8'),
      ) as Record<string, string>
    ).license;

    expect(cargoLicense?.startsWith(APP_LICENSE)).toBe(true);
    expect(pkgLicense?.startsWith(APP_LICENSE)).toBe(true);
  });

  it('mirrors the repository configured in src-tauri/Cargo.toml', () => {
    const cargo = readFileSync(join(REPO_ROOT, 'src-tauri/Cargo.toml'), 'utf8');
    const configured = /^repository\s*=\s*"([^"]+)"/m.exec(cargo)?.[1];
    expect(configured).toBe(REPO_URL);
    // …and it is the real remote, not a repository that does not exist.
    expect(REPO_URL).toBe('https://github.com/MAECLY/pluryprint');
  });

  it('derives every link from the one slug', () => {
    for (const url of [
      RELEASES_URL,
      LICENSE_URL,
      CLA_URL,
      NOTICES_URL,
      DOCS_URL,
    ]) {
      expect(url.startsWith(`${REPO_URL}/`)).toBe(true);
    }
  });

  it('points the blob/tree links at paths that exist on the default branch', () => {
    // `git symbolic-ref refs/remotes/origin/HEAD` → refs/remotes/origin/main.
    for (const url of [LICENSE_URL, CLA_URL, NOTICES_URL, DOCS_URL]) {
      const [, kind, branch, ...rest] = url.slice(REPO_URL.length).split('/');
      expect(kind).toMatch(/^(blob|tree)$/);
      expect(branch).toBe('main');
      expect(existsSync(join(REPO_ROOT, rest.join('/')))).toBe(true);
    }
  });
});

/**
 * Third-party inventory (`docs/09-legal/third-party-licenses.md`).
 *
 * The About screen derives its Components card at runtime, but the *legal*
 * inventory — the document that calls itself "the source of truth for legal
 * compliance" — is a hand-written markdown table, and a hand-written list of
 * dependencies rots silently: it grew a `reqwest` and a `tauri-plugin-updater`
 * we never depended on, while every crate added since (ureq, semver, walkdir,
 * clap, wait-timeout, the Tauri plugins) and the webfonts we actually
 * redistribute never made it in.
 *
 * These assertions are the missing feedback loop: the truth is the manifests
 * (`src-tauri/Cargo.toml`, `package.json`) and the files we ship, and any
 * divergence has to be listed *here*, explicitly, to pass. They live next to
 * the About tests because this is the screen that reports what the app is made
 * of; there is no other frontend test that reads the manifests.
 */
describe('third-party inventory (docs/09-legal/third-party-licenses.md)', () => {
  const INVENTORY = join(REPO_ROOT, 'docs/09-legal/third-party-licenses.md');
  const FONT_DIR = join(REPO_ROOT, 'src/assets/fonts');

  /**
   * Inventory rows that are deliberately not a Cargo/npm package: bundled
   * binaries, system frameworks, subprocess tools and packaging utilities.
   * Their presence is a legal fact, not a dependency-graph fact, so they are
   * exempt from the "does this package exist?" direction of the check.
   */
  const NON_PACKAGE_ROWS = [
    'Chromium / Edge',
    'Chromium / Microsoft Edge',
    'CUPS lp',
    // The Linux web view stack, one combined row (system libraries, bundled
    // only inside the AppImage), and the HEIC decoders run as subprocesses.
    'GLib',
    'GTK 3',
    'ImageMagick',
    'Inno Setup',
    'JetBrains Mono',
    'LibreOffice',
    'libheif tools / ImageMagick',
    'libsoup 3 and their dependencies',
    'NSIS',
    'Pandoc',
    'Rust standard library',
    'SumatraPDF',
    'SumatraPDF 3.5.2',
    'TypeScript runtime helpers',
    'WKWebView',
    'WebKit / WKWebView (macOS)',
    'WebKitGTK 4.1',
    'WebView2 (loader stub)',
    'WebView2 loader',
    'create-dmg',
    'qpdf',
    'sips / Windows Imaging Component',
  ];

  /** Inventory rows named after the project rather than the package. */
  const ROW_ALIASES: Record<string, string> = { 'windows-rs': 'windows' };

  /**
   * KNOWN DRIFT — the divergence that already existed when this check was
   * written (finding `about-third-party-inventory-stale`). It is a ratchet, not
   * an excuse list: the assertions below fail both when a NEW divergence
   * appears and when a listed one is fixed without deleting its entry, so the
   * list can only shrink. Emptying it is the docs task; nothing here may be
   * added to without a matching change to the manifests.
   */
  const KNOWN_MISSING_FROM_INVENTORY: string[] = [];

  /** Inventory rows claiming a dependency this project does not have. */
  const KNOWN_PHANTOM_ROWS: string[] = [];

  /** Compare names the way a human would: case- and punctuation-insensitive. */
  const norm = (name: string) =>
    (ROW_ALIASES[name] ?? name).toLowerCase().replace(/[^a-z0-9]/g, '');

  /** The body of one `[section]` of a TOML file. */
  function tomlSection(toml: string, header: string): string {
    const start = toml.indexOf(header);
    if (start < 0) return '';
    const rest = toml.slice(start + header.length);
    const end = rest.search(/^\[/m);
    return end < 0 ? rest : rest.slice(0, end);
  }

  const tomlKeys = (body: string) =>
    [...body.matchAll(/^([A-Za-z0-9_-]+)\s*=/gm)].map((m) => m[1] ?? '');

  /**
   * Everything compiled into, bundled with, or shipped alongside the binary —
   * i.e. everything we redistribute and therefore have to account for. Build-
   * and test-only tooling is excluded on purpose: it never leaves this repo.
   */
  function redistributedComponents(): string[] {
    const cargo = readFileSync(join(REPO_ROOT, 'src-tauri/Cargo.toml'), 'utf8');
    const pkg = JSON.parse(
      readFileSync(join(REPO_ROOT, 'package.json'), 'utf8'),
    ) as { dependencies: Record<string, string> };
    const fonts = readdirSync(FONT_DIR)
      .filter((f) => f.endsWith('.woff2'))
      .map((f) => f.split('-')[0] ?? f);

    // The shell extension ships as its own DLL, so its crates are
    // redistributed too even though they never enter the Tauri binary.
    const shellExt = readFileSync(
      join(REPO_ROOT, 'shell-ext-win/Cargo.toml'),
      'utf8',
    );

    return [
      ...tomlKeys(tomlSection(cargo, '[dependencies]')),
      ...tomlKeys(tomlSection(cargo, "[target.'cfg(windows)'.dependencies]")),
      ...tomlKeys(tomlSection(cargo, "[target.'cfg(unix)'.dependencies]")),
      ...tomlKeys(
        tomlSection(cargo, `[target.'cfg(target_os = "macos")'.dependencies]`),
      ),
      ...tomlKeys(
        tomlSection(cargo, `[target.'cfg(target_os = "linux")'.dependencies]`),
      ),
      ...tomlKeys(tomlSection(shellExt, '[dependencies]')),
      ...Object.keys(pkg.dependencies),
      ...new Set(fonts),
    ];
  }

  /**
   * Every component named in the first column of a markdown table.
   *
   * The inventory is written for humans, so a cell can carry markdown emphasis
   * (`**3.5.2**`), a parenthetical note (`(macOS only)`) or several packages at
   * once (`` `serde`, `serde_json` ``). All three are stripped or split here
   * rather than banned from the document.
   */
  function inventoryRows(): string[] {
    const lines = readFileSync(INVENTORY, 'utf8').split('\n');
    const isSeparator = (line: string) => /^\|[\s:|-]+$/.test(line);
    const out: string[] = [];

    lines.forEach((line, i) => {
      if (!line.startsWith('|') || isSeparator(line)) return;
      // The row above a separator is the header, not an entry.
      if (isSeparator(lines[i + 1] ?? '')) return;

      const cell = (line.split('|')[1] ?? '')
        .replace(/[`*]/g, '')
        .replace(/\([^)]*\)/g, '')
        .trim();
      for (const name of cell.split(',')) {
        const trimmed = name.trim();
        if (trimmed) out.push(trimmed);
      }
    });
    return out;
  }

  it('lists every dependency and asset we redistribute', () => {
    const listed = new Set(inventoryRows().map(norm));
    const missing = redistributedComponents()
      .filter((dep) => !listed.has(norm(dep)))
      .sort();

    expect(
      missing,
      'a component we ship is missing from the legal inventory (or a fixed ' +
        'entry is still listed in KNOWN_MISSING_FROM_INVENTORY — delete it)',
    ).toEqual(KNOWN_MISSING_FROM_INVENTORY);
  });

  it('does not claim dependencies the project does not have', () => {
    const cargo = readFileSync(join(REPO_ROOT, 'src-tauri/Cargo.toml'), 'utf8');
    const pkg = JSON.parse(
      readFileSync(join(REPO_ROOT, 'package.json'), 'utf8'),
    ) as {
      dependencies: Record<string, string>;
      devDependencies: Record<string, string>;
    };
    // Dev/build tooling counts here: an inventory row for Vite or Tailwind is
    // legitimate even though neither is redistributed as a package.
    const shellExt = readFileSync(
      join(REPO_ROOT, 'shell-ext-win/Cargo.toml'),
      'utf8',
    );
    const known = new Set(
      [
        ...tomlKeys(tomlSection(cargo, '[dependencies]')),
        ...tomlKeys(tomlSection(cargo, '[dev-dependencies]')),
        ...tomlKeys(tomlSection(cargo, "[target.'cfg(windows)'.dependencies]")),
        ...tomlKeys(tomlSection(cargo, "[target.'cfg(unix)'.dependencies]")),
        ...tomlKeys(
          tomlSection(
            cargo,
            `[target.'cfg(target_os = "macos")'.dependencies]`,
          ),
        ),
        ...tomlKeys(
          tomlSection(
            cargo,
            `[target.'cfg(target_os = "linux")'.dependencies]`,
          ),
        ),
        ...tomlKeys(tomlSection(cargo, '[build-dependencies]')),
        ...tomlKeys(tomlSection(shellExt, '[dependencies]')),
        ...tomlKeys(tomlSection(shellExt, '[build-dependencies]')),
        ...tomlKeys(
          tomlSection(shellExt, "[target.'cfg(windows)'.dependencies]"),
        ),
        ...Object.keys(pkg.dependencies),
        ...Object.keys(pkg.devDependencies),
      ].map(norm),
    );
    const exempt = new Set(NON_PACKAGE_ROWS.map(norm));

    const phantom = inventoryRows()
      .filter((row) => !exempt.has(norm(row)) && !known.has(norm(row)))
      .sort();

    expect(
      phantom,
      'the legal inventory names a package that is not in any manifest (or a ' +
        'removed entry is still listed in KNOWN_PHANTOM_ROWS — delete it)',
    ).toEqual(KNOWN_PHANTOM_ROWS);
  });

  it('regression: the webfonts we redistribute are real files, so the OFL notice is owed', () => {
    // The one omission above with a real legal consequence: JetBrains Mono is
    // SIL OFL 1.1 and we ship the `.woff2` files themselves, which obliges us
    // to reproduce its notice. This asserts the premise (we do redistribute
    // them) so the KNOWN_MISSING entry can never be "fixed" by pretending the
    // fonts are external.
    const fonts = readdirSync(FONT_DIR).filter((f) => f.endsWith('.woff2'));
    expect(fonts.length).toBeGreaterThan(0);

    const css = readFileSync(join(REPO_ROOT, 'src/styles/globals.css'), 'utf8');
    expect(css).toMatch(/@font-face/);
  });
});
