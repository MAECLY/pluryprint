import {
  act,
  render,
  screen,
  fireEvent,
  waitFor,
  within,
} from '@testing-library/react';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks';

import type { AppSettings, EngineLimits, SettingsMeta } from '@/ipc/bindings';
import { fixtureAppSettings } from '@/test/fixtures';
import { I18nProvider } from '@/i18n/I18nProvider';
import { Toaster } from '@/components/AppShell/Toaster';
import {
  applyDensity,
  type Density,
  type ResolvedTheme,
  type ThemeMode,
} from '@/lib/theme';

// The Default-profile row reads the profiles store; mock it so this suite
// stays decoupled from that file. The mock is a plain selector over a static
// state object — enough for the screen, which only reads id/name/default.
const profilesMock = vi.hoisted(() => {
  const setDefault = vi.fn((_id: string) => Promise.resolve(true));
  const init = vi.fn(() => Promise.resolve());
  const state = {
    profiles: [
      { id: 'builtin-quick', name: 'Quick', default: true },
      { id: 'builtin-duplex-bw', name: 'Duplex BW', default: false },
      { id: 'builtin-photo-color', name: 'Photo Color', default: false },
    ],
    status: 'ready' as 'ready' | 'loading' | 'error' | 'unavailable',
    setDefault,
    init,
  };
  return { setDefault, init, state };
});

vi.mock('@/stores/profilesStore', () => ({
  useProfilesStore: <T,>(selector: (s: typeof profilesMock.state) => T): T =>
    selector(profilesMock.state),
}));

// Imported after the mock is registered (vi.mock hoists regardless).
import { SettingsScreen } from './SettingsScreen';
import { useSettingsStore } from '@/stores/settingsStore';
import { rememberHostInfo } from '@/lib/platform';

/** The engine bounds a real backend ships in `meta.limits` (ENGINE_LIMITS). */
const LIMITS: EngineLimits = {
  max_concurrent_conversions: { min: 1, max: 8, default: 2, zero: 'clamp' },
  conversion_timeout_seconds: {
    min: 10,
    max: 3600,
    default: 60,
    zero: 'disables',
  },
  max_file_size_mb: { min: 1, max: 4096, default: 200, zero: 'default' },
  print_timeout_seconds: {
    min: 10,
    max: 3600,
    default: 300,
    zero: 'disables',
  },
};

const CONFIG_PATH = '/home/u/.config/Pluryprint/config.toml';

function meta(over: Partial<SettingsMeta> = {}): SettingsMeta {
  return {
    config_unreadable: false,
    config_path: CONFIG_PATH,
    limits: LIMITS,
    ...over,
  };
}

/** The wire payload a current backend returns: settings + server annotations. */
function settingsWith(over: Partial<AppSettings> = {}): AppSettings {
  return { ...fixtureAppSettings, meta: meta(), ...over };
}

interface Call {
  cmd: string;
  args: Record<string, unknown>;
}

/** Install settings IPC mocks and return the list of commands invoked. */
function mockSettings(
  opts: { initial?: AppSettings; fail?: string } = {},
): Call[] {
  const calls: Call[] = [];
  mockIPC((cmd, args) => {
    calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> });
    if (cmd === opts.fail) throw new Error('boom');
    if (cmd === 'get_app_settings') return opts.initial ?? settingsWith();
    if (cmd === 'set_app_settings')
      return (args as { settings: AppSettings }).settings;
    return null;
  });
  return calls;
}

interface RenderOver {
  theme?: ResolvedTheme;
  density?: Density;
  mode?: ThemeMode;
  setMode?: (m: ThemeMode) => void;
}

/**
 * Render inside the i18n provider so every assertion runs against the real
 * catalog rather than literals baked into the component.
 */
function renderSettings(over: RenderOver = {}) {
  const setTheme = vi.fn();
  const setDensity = vi.fn();
  render(
    <I18nProvider>
      <SettingsScreen
        theme={over.theme ?? 'light'}
        setTheme={setTheme}
        density={over.density ?? 'comfortable'}
        setDensity={setDensity}
        {...(over.mode !== undefined ? { mode: over.mode } : {})}
        {...(over.setMode ? { setMode: over.setMode } : {})}
      />
      <Toaster theme="light" />
    </I18nProvider>,
  );
  return { setTheme, setDensity };
}

async function waitReady() {
  await waitFor(() => expect(useSettingsStore.getState().status).toBe('ready'));
}

/** The control cell (right column) of the Row whose label is `labelText`. */
function controlOf(labelText: string): HTMLElement {
  const labelDiv = screen.getByText(labelText);
  const rowRoot = labelDiv.parentElement!.parentElement!;
  return rowRoot.children[1] as HTMLElement;
}

/** Assert `set_app_settings` was called, and return the settings it was sent. */
async function savedSettings(calls: Call[]): Promise<AppSettings> {
  let sent: AppSettings | undefined;
  await waitFor(() => {
    const save = calls.find((c) => c.cmd === 'set_app_settings');
    expect(save).toBeTruthy();
    sent = save!.args.settings as AppSettings;
  });
  return sent!;
}

describe('SettingsScreen', () => {
  beforeEach(() => {
    useSettingsStore.setState({
      settings: null,
      status: 'idle',
      error: undefined,
    });
    profilesMock.setDefault.mockClear();
    profilesMock.init.mockClear();
    profilesMock.state.status = 'ready';
  });
  afterEach(() => clearMocks());

  // -- smoke --------------------------------------------------------------

  it('FR-090: smoke — renders every section of the persisted configuration', async () => {
    mockSettings();
    renderSettings();
    await waitReady();

    for (const section of [
      'Appearance',
      'Printing',
      'Engine',
      'Updates',
      'Privacy',
    ]) {
      expect(screen.getByText(section)).toBeInTheDocument();
    }
    // One control per documented setting, none of them missing.
    for (const label of [
      'Theme',
      'Density',
      'Language',
      'Start minimized',
      'Default profile',
      'Concurrent conversions',
      'Conversion timeout',
      'Max file size',
      'Channel',
      'Check on startup',
      'Crash reports',
      'Anonymous usage metrics',
    ]) {
      expect(screen.getByText(label)).toBeInTheDocument();
    }
  });

  // -- appearance ---------------------------------------------------------

  it('FR: persists a 3-way theme change (System included) and drives setMode', async () => {
    // Backend theme matches the passed mode so the load reconcile is a no-op.
    const calls = mockSettings({
      initial: settingsWith({
        app: { ...fixtureAppSettings.app, theme: 'light' },
      }),
    });
    const setMode = vi.fn();
    renderSettings({ mode: 'light', setMode });
    await waitReady();

    fireEvent.click(screen.getByText('System'));

    expect(setMode).toHaveBeenCalledWith('system');
    const sent = await savedSettings(calls);
    expect(sent.app.theme).toBe('system');
  });

  it('persists a 2-way theme change via setTheme when no mode is supplied', async () => {
    const calls = mockSettings();
    const { setTheme } = renderSettings({ theme: 'light' });
    await waitReady();

    fireEvent.click(screen.getByText('Dark'));

    expect(setTheme).toHaveBeenCalledWith('dark');
    const sent = await savedSettings(calls);
    expect(sent.app.theme).toBe('dark');
  });

  it('dual-writes a density change (setter + backend)', async () => {
    const calls = mockSettings();
    const { setDensity } = renderSettings({ density: 'comfortable' });
    await waitReady();

    fireEvent.click(screen.getByText('Compact'));

    expect(setDensity).toHaveBeenCalledWith('compact');
    const sent = await savedSettings(calls);
    expect(sent.app.density).toBe('compact');
  });

  // -- FR-094 language ----------------------------------------------------

  it('FR-094: the language control offers every supported locale and persists the choice', async () => {
    const calls = mockSettings();
    renderSettings();
    await waitReady();

    const select = within(controlOf('Language')).getByRole('combobox');
    expect(
      within(select)
        .getAllByRole('option')
        .map((o) => o.textContent),
    ).toEqual(['English', 'Español']);

    fireEvent.change(select, { target: { value: 'es-ES' } });

    const sent = await savedSettings(calls);
    expect(sent.app.locale).toBe('es-ES');
    // A language picked here is final: first-run detection never overrides it.
    expect(sent.app.locale_detected).toBe(true);
  });

  it('FR-094: integration — choosing Spanish re-renders the UI in Spanish', async () => {
    mockSettings();
    renderSettings();
    await waitReady();
    expect(screen.getByText('Settings')).toBeInTheDocument();

    fireEvent.change(within(controlOf('Language')).getByRole('combobox'), {
      target: { value: 'es-ES' },
    });

    // The optimistic write lands in the store, which is what I18nProvider
    // reads — no reload, no second round-trip.
    expect(await screen.findByText('Ajustes')).toBeInTheDocument();
    expect(screen.getByText('Apariencia')).toBeInTheDocument();
    expect(screen.getByText('Idioma')).toBeInTheDocument();
  });

  it('FR-092: renders in Spanish when the persisted locale is es-ES', async () => {
    mockSettings({
      initial: settingsWith({
        app: { ...fixtureAppSettings.app, locale: 'es-ES' },
      }),
    });
    renderSettings();
    await waitReady();

    expect(await screen.findByText('Ajustes')).toBeInTheDocument();
    expect(screen.getByText('Motor')).toBeInTheDocument();
    expect(screen.getByText('Privacidad')).toBeInTheDocument();
    expect(screen.getByText('Iniciar minimizada')).toBeInTheDocument();
    expect(screen.getByText('Comprobar al iniciar')).toBeInTheDocument();
    expect(
      screen.getByText(/solo registran tu consentimiento/),
    ).toBeInTheDocument();
  });

  // -- launch_minimized ---------------------------------------------------

  it('FR-090: persists the start-minimized switch (the flag the backend reads at launch)', async () => {
    const calls = mockSettings();
    renderSettings();
    await waitReady();

    fireEvent.click(
      within(controlOf('Start minimized')).getByRole('switch', {
        name: 'Start minimized',
      }),
    );

    const sent = await savedSettings(calls);
    expect(sent.app.launch_minimized).toBe(true);
  });

  it('FR-072: the native-notifications switch is on by default and persists when turned off', async () => {
    const calls = mockSettings();
    renderSettings();
    await waitReady();

    const toggle = within(controlOf('Native notifications')).getByRole(
      'switch',
      { name: 'Native notifications' },
    );
    expect(toggle).toBeChecked();
    fireEvent.click(toggle);

    const sent = await savedSettings(calls);
    expect(sent.app.native_notifications).toBe(false);
  });

  // -- printing / updates / privacy ---------------------------------------

  it('FR: the default-profile select calls setDefault on the profiles store', async () => {
    mockSettings();
    renderSettings();
    await waitReady();

    const select = within(controlOf('Default profile')).getByRole('combobox');
    fireEvent.change(select, { target: { value: 'builtin-duplex-bw' } });

    expect(profilesMock.setDefault).toHaveBeenCalledWith('builtin-duplex-bw');
  });

  it('persists an update channel change', async () => {
    const calls = mockSettings();
    renderSettings();
    await waitReady();

    fireEvent.click(screen.getByText('Beta'));

    const sent = await savedSettings(calls);
    expect(sent.updater.channel).toBe('beta');
  });

  it('FR-086: switching to a lower channel asks first, and Cancel keeps the channel', async () => {
    const calls = mockSettings({
      initial: settingsWith({
        updater: { ...fixtureAppSettings.updater, channel: 'beta' },
      }),
    });
    renderSettings();
    await waitReady();

    fireEvent.click(screen.getByText('Stable'));

    const dialog = await screen.findByRole('dialog');
    expect(dialog).toHaveTextContent('Switch to Stable?');
    // It says nothing is downgraded and when the next update comes.
    expect(dialog).toHaveTextContent(/never installs an older one/);
    expect(dialog).toHaveTextContent(
      /only when the Stable channel publishes a version newer than this one/,
    );
    expect(calls.some((c) => c.cmd === 'set_app_settings')).toBe(false);

    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(calls.some((c) => c.cmd === 'set_app_settings')).toBe(false);
    expect(useSettingsStore.getState().settings?.updater.channel).toBe('beta');
  });

  it('FR-086: confirming the switch to a lower channel saves it', async () => {
    const calls = mockSettings({
      initial: settingsWith({
        updater: { ...fixtureAppSettings.updater, channel: 'nightly' },
      }),
    });
    renderSettings();
    await waitReady();

    fireEvent.click(screen.getByText('Beta'));
    const dialog = await screen.findByRole('dialog');
    fireEvent.click(
      within(dialog).getByRole('button', { name: 'Switch to Beta' }),
    );

    const sent = await savedSettings(calls);
    expect(sent.updater.channel).toBe('beta');
  });

  it('FR-086: a move to a less stable channel applies without a question', async () => {
    const calls = mockSettings({
      initial: settingsWith({
        updater: { ...fixtureAppSettings.updater, channel: 'stable' },
      }),
    });
    renderSettings();
    await waitReady();

    fireEvent.click(screen.getByText('Nightly'));

    const sent = await savedSettings(calls);
    expect(sent.updater.channel).toBe('nightly');
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('FR-086: the lower-channel question is translated', async () => {
    mockSettings({
      initial: settingsWith({
        app: { ...fixtureAppSettings.app, locale: 'es-ES' },
        updater: { ...fixtureAppSettings.updater, channel: 'beta' },
      }),
    });
    renderSettings();
    await waitReady();

    fireEvent.click(screen.getByText('Estable'));
    const dialog = await screen.findByRole('dialog');
    expect(dialog).toHaveTextContent('¿Cambiar a Estable?');
    expect(dialog).toHaveTextContent(/nunca instala una más antigua/);
    expect(
      within(dialog).getByRole('button', { name: 'Cambiar a Estable' }),
    ).toBeInTheDocument();
  });

  it('persists the check-on-startup toggle', async () => {
    const calls = mockSettings();
    renderSettings();
    await waitReady();

    fireEvent.click(controlOf('Check on startup').querySelector('label')!);

    const sent = await savedSettings(calls);
    expect(sent.updater.check_on_startup).toBe(false);
  });

  it('FR-080: the updates section describes the startup check it actually performs', async () => {
    mockSettings();
    renderSettings();
    await waitReady();

    // This assertion has been inverted once already. It first guarded the
    // opposite claim, because the toggle was persisted and nothing acted on
    // it; FR-080 is implemented now, so the honest text is the one that states
    // the timing. Naming both numbers matters: this is the sentence a
    // privacy-minded user reads before deciding.
    expect(
      screen.getByText(/30 seconds after launch, at most once every 24 hours/i),
    ).toBeInTheDocument();
    expect(screen.queryByText(/no launch-time check/i)).not.toBeInTheDocument();
  });

  it('persists the crash-reports consent toggle', async () => {
    const calls = mockSettings();
    renderSettings();
    await waitReady();

    fireEvent.click(controlOf('Crash reports').querySelector('label')!);

    const sent = await savedSettings(calls);
    expect(sent.telemetry.crash_reports).toBe(true);
  });

  it('regression: the crash-reports hint does not promise a transmission', async () => {
    mockSettings();
    renderSettings();
    await waitReady();

    expect(
      screen.queryByText(/Sent only when toggled on/),
    ).not.toBeInTheDocument();
    expect(
      screen.getByText(/Nothing is sent in this version/),
    ).toBeInTheDocument();
  });

  it('persists the anonymous-usage-metrics consent toggle', async () => {
    const calls = mockSettings();
    renderSettings();
    await waitReady();

    fireEvent.click(
      controlOf('Anonymous usage metrics').querySelector('label')!,
    );

    const sent = await savedSettings(calls);
    expect(sent.telemetry.anonymous_usage_metrics).toBe(true);
  });

  // -- engine fields, validated against the SERVER's limits ----------------

  it('FR: an in-bounds engine value commits on blur and persists', async () => {
    const calls = mockSettings();
    renderSettings();
    await waitReady();

    const input = within(controlOf('Concurrent conversions')).getByRole(
      'textbox',
    );
    fireEvent.change(input, { target: { value: '4' } });
    fireEvent.focusOut(input);

    const sent = await savedSettings(calls);
    expect(sent.engine.max_concurrent_conversions).toBe(4);
  });

  it('FR: an out-of-bounds engine value shows an inline error and does NOT persist', async () => {
    const calls = mockSettings();
    renderSettings();
    await waitReady();

    const input = within(controlOf('Concurrent conversions')).getByRole(
      'textbox',
    );
    fireEvent.change(input, { target: { value: '99' } });
    fireEvent.focusOut(input);

    expect(
      await screen.findByText('Enter a whole number from 1 to 8.'),
    ).toBeInTheDocument();
    expect(calls.some((c) => c.cmd === 'set_app_settings')).toBe(false);
  });

  it('regression: the inline bounds come from the server, not from a hard-coded copy', async () => {
    // A backend whose ceiling moved to 4: the client must reject 5 and say
    // "1 to 4" without anyone editing the screen.
    const calls = mockSettings({
      initial: settingsWith({
        meta: meta({
          limits: {
            ...LIMITS,
            max_concurrent_conversions: {
              min: 1,
              max: 4,
              default: 2,
              zero: 'clamp',
            },
          },
        }),
      }),
    });
    renderSettings();
    await waitReady();

    const input = within(controlOf('Concurrent conversions')).getByRole(
      'textbox',
    );
    fireEvent.change(input, { target: { value: '5' } });
    fireEvent.focusOut(input);

    expect(
      await screen.findByText('Enter a whole number from 1 to 4.'),
    ).toBeInTheDocument();
    expect(calls.some((c) => c.cmd === 'set_app_settings')).toBe(false);
  });

  it('accepts 0 for the conversion timeout (its bounds mark 0 as "disables")', async () => {
    const calls = mockSettings();
    renderSettings();
    await waitReady();

    const input = within(controlOf('Conversion timeout')).getByRole('textbox');
    fireEvent.change(input, { target: { value: '0' } });
    fireEvent.focusOut(input);

    const sent = await savedSettings(calls);
    expect(sent.engine.conversion_timeout_seconds).toBe(0);
    // …while the timeout error text still advertises the 0 escape hatch.
    fireEvent.change(input, { target: { value: '5' } });
    fireEvent.focusOut(input);
    expect(
      await screen.findByText('Enter 0, or a whole number from 10 to 3600.'),
    ).toBeInTheDocument();
  });

  it('FR-034: saves the print timeout as its own setting, separate from the conversion timeout', async () => {
    const calls = mockSettings();
    renderSettings();
    await waitReady();

    const input = within(controlOf('Print timeout')).getByRole('textbox');
    fireEvent.change(input, { target: { value: '600' } });
    fireEvent.focusOut(input);

    const sent = await savedSettings(calls);
    expect(sent.engine.print_timeout_seconds).toBe(600);
    expect(sent.engine.conversion_timeout_seconds).toBe(60);
  });

  it('falls back to server-side validation when the backend ships no limits', async () => {
    // An older backend (no `meta`): the client must not invent bounds of its
    // own — it forwards the value and lets `set_app_settings` decide.
    const calls = mockSettings({ initial: { ...fixtureAppSettings } });
    renderSettings();
    await waitReady();

    const input = within(controlOf('Concurrent conversions')).getByRole(
      'textbox',
    );
    fireEvent.change(input, { target: { value: '99' } });
    fireEvent.focusOut(input);

    const sent = await savedSettings(calls);
    expect(sent.engine.max_concurrent_conversions).toBe(99);
  });

  // -- FR-101: corrupt config -------------------------------------------

  it('FR-101: warns that the shown values are defaults when config.toml is unreadable', async () => {
    mockSettings({
      initial: settingsWith({ meta: meta({ config_unreadable: true }) }),
    });
    renderSettings();
    await waitReady();

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('Your settings file could not be read');
    expect(alert).toHaveTextContent(/not your saved settings/);
    // The path is what makes it recoverable: the user has to fix that file.
    expect(alert).toHaveTextContent(CONFIG_PATH);
  });

  it('FR-101: shows no warning when the config is healthy', async () => {
    mockSettings();
    renderSettings();
    await waitReady();

    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  // -- notes & edge states -------------------------------------------------

  it('renders the channel, M2 milestone and consent notes', async () => {
    mockSettings();
    renderSettings();
    await waitReady();

    expect(
      screen.getByText(/No nightly builds are published/),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/these switches record consent only/),
    ).toBeInTheDocument();
    // FR-001: off macOS the installer registers the Explorer entry; Windows
    // 11 keeps it under "Show more options".
    expect(
      screen.getByText(
        /installer adds Pluryprint to the Explorer right-click menu/,
      ),
    ).toBeInTheDocument();
    expect(screen.getByText(/under Show more options/)).toBeInTheDocument();
    expect(screen.queryByText(/no installer registers it yet/)).toBeNull();
  });

  it('FR-001: on Linux the shell note depends on the package', async () => {
    rememberHostInfo({ os: 'linux', executable: '/usr/bin/pluryprint' });
    mockSettings();
    renderSettings();
    await waitReady();
    expect(
      screen.getByText(/The \.deb package adds Pluryprint to Open With/),
    ).toBeInTheDocument();
    expect(screen.getByText(/Dolphin \(KDE\) and Nemo/)).toBeInTheDocument();

    // A later `app_info` answer re-words the note in place.
    act(() => {
      rememberHostInfo({
        os: 'linux',
        executable: '/tmp/.mount_Plury1/usr/bin/pluryprint',
      });
    });
    expect(
      screen.getByText(/An AppImage registers no right-click entry by itself/),
    ).toBeInTheDocument();
  });

  it('FR-001: on macOS the shell note points at the Finder Services entry', async () => {
    const platform = vi
      .spyOn(window.navigator, 'platform', 'get')
      .mockReturnValue('MacIntel');
    try {
      mockSettings();
      renderSettings();
      await waitReady();
      expect(
        screen.getByText(/Finder › Services › Print with Pluryprint/),
      ).toBeInTheDocument();
      // It is not a future milestone any more.
      expect(screen.queryByText(/\(M2\)/)).not.toBeInTheDocument();
    } finally {
      platform.mockRestore();
    }
  });

  it('drops the old shell-integration switches', async () => {
    mockSettings();
    renderSettings();
    await waitReady();

    expect(screen.queryByText('Right-click menu')).not.toBeInTheDocument();
    expect(
      screen.queryByText('Filter unsupported files'),
    ).not.toBeInTheDocument();
  });

  it('disables backend rows but keeps appearance working when unavailable', async () => {
    // No Tauri host: drop the internals a prior mockIPC may have left behind.
    const w = window as unknown as {
      __TAURI_INTERNALS__?: unknown;
      __TAURI__?: unknown;
    };
    delete w.__TAURI_INTERNALS__;
    delete w.__TAURI__;

    const { setTheme } = renderSettings({ theme: 'light' });

    await waitFor(() =>
      expect(useSettingsStore.getState().status).toBe('unavailable'),
    );
    expect(
      screen.getByText(/only appearance is adjustable/),
    ).toBeInTheDocument();

    // A backend row is visually + interactively disabled.
    expect(controlOf('Crash reports').style.pointerEvents).toBe('none');
    expect(controlOf('Language').style.pointerEvents).toBe('none');
    // NFR-041: …and out of reach of the keyboard and assistive tech too.
    expect(controlOf('Crash reports')).toHaveAttribute('inert');
    expect(controlOf('Crash reports')).toHaveAttribute('aria-disabled', 'true');
    expect(controlOf('Max file size')).toHaveAttribute('inert');
    // The preview's built-in list must not make the default-profile row live:
    // picking one would only bounce off the missing host.
    expect(controlOf('Default profile')).toHaveAttribute('inert');

    // Appearance stays enabled offline (local-only, nothing to revert).
    expect(controlOf('Theme').style.pointerEvents).not.toBe('none');
    expect(controlOf('Theme')).not.toHaveAttribute('inert');
    // NFR-041: a backend row is really disabled — a disabled fieldset takes
    // its controls out of the tab order and the accessibility tree announces
    // them as unavailable, rather than only blocking the pointer.
    expect(controlOf('Crash reports')).toBeDisabled();
    expect(controlOf('Language')).toBeDisabled();
    expect(
      within(controlOf('Crash reports')).getByRole('switch'),
    ).toBeDisabled();
    expect(within(controlOf('Language')).getByRole('combobox')).toBeDisabled();

    expect(controlOf('Theme')).not.toBeDisabled();
    fireEvent.click(screen.getByText('Dark'));
    expect(setTheme).toHaveBeenCalledWith('dark');
  });

  it('unit: density is published on the document root so the whole app can follow it', () => {
    // The setting used to reach exactly one list (the Print Job file table).
    // `applyDensity` is the app-wide hook: an attribute to select on and the
    // spacing variables the Settings rows already consume.
    applyDensity('compact');
    expect(document.documentElement.dataset.density).toBe('compact');
    expect(document.documentElement.style.getPropertyValue('--mp-row-py')).toBe(
      '8px',
    );

    applyDensity('comfortable');
    expect(document.documentElement.dataset.density).toBe('comfortable');
    expect(document.documentElement.style.getPropertyValue('--mp-row-py')).toBe(
      '14px',
    );
  });

  it('E: disables the appearance controls while the settings are loading', () => {
    // Preset 'loading' so the mount init() short-circuits and the status holds.
    useSettingsStore.setState({ status: 'loading', settings: null });
    mockSettings();
    renderSettings({ mode: 'light', setMode: vi.fn() });

    expect(useSettingsStore.getState().status).toBe('loading');
    // Theme + Density are dimmed and non-interactive so a change can't be
    // silently reverted by the arriving backend snapshot.
    expect(controlOf('Theme').style.pointerEvents).toBe('none');
    expect(controlOf('Density').style.pointerEvents).toBe('none');
    // NFR-041: the keyboard can't reach them either (no Tab + Arrow change).
    expect(controlOf('Theme')).toHaveAttribute('inert');
    expect(controlOf('Density')).toHaveAttribute('inert');
    expect(controlOf('Theme')).toBeDisabled();
    expect(controlOf('Density')).toBeDisabled();
    // …including from the keyboard: the radios are natively disabled.
    for (const radio of within(controlOf('Theme')).getAllByRole('radio')) {
      expect(radio).toBeDisabled();
    }
  });

  it('NFR-041: enabled rows carry no inert/aria-disabled once settings load', async () => {
    mockSettings();
    renderSettings({ mode: 'light', setMode: vi.fn() });
    await waitReady();

    for (const label of ['Theme', 'Crash reports', 'Max file size']) {
      expect(controlOf(label)).not.toHaveAttribute('inert');
      expect(controlOf(label)).not.toHaveAttribute('aria-disabled');
    }
  });

  it('disables the default-profile row while the profile list is not ready', async () => {
    profilesMock.state.status = 'loading';
    mockSettings();
    renderSettings();
    await waitReady();

    expect(controlOf('Default profile')).toHaveAttribute('inert');
  });

  it('offers a retry when the profile list failed to load', async () => {
    profilesMock.state.status = 'error';
    mockSettings();
    renderSettings();
    await waitReady();

    expect(controlOf('Default profile')).toHaveAttribute('inert');
    expect(
      screen.getByText(/Couldn't load the profile list/),
    ).toBeInTheDocument();
    profilesMock.init.mockClear();
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(profilesMock.init).toHaveBeenCalledTimes(1);
  });

  it('FR-094: a stored regional tag shows the effective language in the select', async () => {
    mockSettings({
      initial: settingsWith({
        app: { ...fixtureAppSettings.app, locale: 'es-MX' },
      }),
    });
    renderSettings();
    await waitReady();

    // I18nProvider renders Spanish for es-MX; the select must agree, or
    // choosing "English" (apparently already selected) could never fire.
    const select = (await screen.findByRole('combobox', {
      name: 'Idioma',
    })) as HTMLSelectElement;
    expect(select.value).toBe('es-ES');
  });

  it('NFR-041: engine fields are named and their bounds error describes them', async () => {
    mockSettings();
    renderSettings();
    const field = await screen.findByRole('textbox', {
      name: 'Concurrent conversions',
    });
    fireEvent.change(field, { target: { value: '0' } });
    fireEvent.blur(field);
    expect(field).toHaveAttribute('aria-invalid', 'true');
    expect(field).toHaveAccessibleDescription(/.+/);
  });
});
