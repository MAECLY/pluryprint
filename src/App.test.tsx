import {
  render,
  screen,
  fireEvent,
  within,
  waitFor,
  act,
} from '@testing-library/react';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks';
import { toast } from 'sonner';

/**
 * Capture the native drag & drop handler App registers, so a drop can be
 * simulated without a real webview. Hoisted because `vi.mock` is.
 */
const webview = vi.hoisted(() => ({
  handler: null as
    null | ((e: { payload: { type: string; paths?: string[] } }) => void),
  /** Every handler registered, in order (the shell's comes first). */
  handlers: [] as ((e: {
    payload: { type: string; paths?: string[] };
  }) => void)[],
  /** Registrations never complete while set (a slow `listen`). */
  hold: false,
}));
vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: (cb: (e: { payload: { type: string } }) => void) => {
      webview.handler = cb as typeof webview.handler;
      webview.handlers.push(cb as (typeof webview.handlers)[number]);
      return webview.hold
        ? new Promise<() => void>(() => {})
        : Promise.resolve(() => {});
    },
  }),
}));

/**
 * The tauri-specta `events` object drives the event plugin over IPC, which
 * mockIPC would also intercept. Swapping just `events` for a controllable fake
 * (commands stay real, so mockIPC still serves invokes) is how the rest of the
 * suite emits backend events on demand — see `queueStore.test.ts`.
 */
type EventListener = (e: { payload: unknown }) => void;
const eventMock = vi.hoisted(() => {
  const registry = {
    updateAvailable: new Set<(e: { payload: unknown }) => void>(),
    queueUpdated: new Set<(e: { payload: unknown }) => void>(),
    jobProgressEvent: new Set<(e: { payload: unknown }) => void>(),
    selectionForwarded: new Set<(e: { payload: unknown }) => void>(),
  };
  const listener = (set: Set<(e: { payload: unknown }) => void>) => ({
    listen: (cb: (e: { payload: unknown }) => void) => {
      set.add(cb);
      return Promise.resolve(() => set.delete(cb));
    },
  });
  return {
    registry,
    events: {
      updateAvailable: listener(registry.updateAvailable),
      queueUpdated: listener(registry.queueUpdated),
      jobProgressEvent: listener(registry.jobProgressEvent),
      selectionForwarded: listener(registry.selectionForwarded),
    },
  };
});
vi.mock('@/ipc/bindings', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/ipc/bindings')>();
  return { ...actual, events: eventMock.events };
});

import App from './App';
import { shortcutLabel } from '@/components/AppShell/MPAppShell';
import { toUiQueueJob } from '@/lib/ipc';
import {
  fixtureAppSettings,
  fixturePrinters,
  fixtureProfiles,
  mkJob,
} from '@/test/fixtures';
import { useProfilesStore } from '@/stores/profilesStore';
import { useQueueStore } from '@/stores/queueStore';
import { useSettingsStore } from '@/stores/settingsStore';
import { BUILT_IN_PROFILES } from '@/lib/profiles';

/** Answer the commands App's screens/stores fire during integration flows. */
function mockShell(overrides: Record<string, unknown> = {}) {
  mockIPC((cmd) => {
    if (cmd in overrides) return overrides[cmd];
    switch (cmd) {
      case 'list_profiles':
        return fixtureProfiles;
      case 'get_app_settings':
        return fixtureAppSettings;
      case 'queue_status':
        return { schema_version: 1, queue_paused: false, jobs: [] };
      case 'list_printers':
        return [];
      case 'get_printer_prefs':
        return {};
      // Every launch drains the context-menu store; the ordinary answer is
      // "nothing was handed to this window".
      case 'take_pending_selections':
        return [];
      default:
        return null;
    }
  });
}

describe('App', () => {
  beforeEach(() => {
    // Skip onboarding modal so the main shell is visible.
    window.localStorage.setItem('pluryprint:onboarding-complete', '1');
    window.localStorage.removeItem('pluryprint:theme');
    window.localStorage.removeItem('pluryprint:density');
    document.documentElement.classList.remove('mp-light', 'mp-dark');
    // Module-level zustand stores leak state across tests — reset them.
    useProfilesStore.setState({
      status: 'idle',
      profiles: BUILT_IN_PROFILES,
      error: undefined,
    });
    useSettingsStore.setState({
      status: 'idle',
      settings: null,
      error: undefined,
    });
    useQueueStore.setState({
      status: 'idle',
      snapshot: null,
      subscribed: false,
      uiJobs: [],
      counts: { active: 0, pending: 0, done: 0 },
      queuePaused: false,
      recentDone: [],
    });
    webview.handler = null;
    webview.handlers = [];
    webview.hold = false;
    for (const set of Object.values(eventMock.registry)) set.clear();
  });
  afterEach(() => clearMocks());

  it('renders the sidebar nav items', () => {
    render(<App />);
    // Sidebar items live inside <nav>; pick that scope to avoid false
    // positives from screen content that mentions the same words.
    const nav = screen.getByRole('navigation');
    expect(nav).toBeInTheDocument();
    // "Print Job" is unique to the sidebar; a sanity check for the rest.
    expect(screen.getByText('Print Job')).toBeInTheDocument();
    expect(within(nav).getByText('Printers')).toBeInTheDocument();
    expect(screen.getByText('Profiles')).toBeInTheDocument();
    expect(screen.getByText('Settings')).toBeInTheDocument();
  });

  it('ADR-0009: the tour stays hidden when config.toml records it as seen', async () => {
    // The packaged upgrade starts with an empty webview storage.
    window.localStorage.removeItem('pluryprint:onboarding-complete');
    mockShell({
      get_app_settings: {
        ...fixtureAppSettings,
        app: { ...fixtureAppSettings.app, onboarding_complete: true },
      },
    });
    render(<App />);
    await waitFor(() =>
      expect(useSettingsStore.getState().status).toBe('ready'),
    );
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('ADR-0009: a first run shows the tour and saves its completion to config.toml', async () => {
    window.localStorage.removeItem('pluryprint:onboarding-complete');
    const writes: Array<{ app: { onboarding_complete?: boolean } }> = [];
    mockIPC((cmd, args) => {
      switch (cmd) {
        case 'list_profiles':
          return fixtureProfiles;
        case 'get_app_settings':
          return fixtureAppSettings;
        case 'set_app_settings': {
          const { settings } = args as {
            settings: { app: { onboarding_complete?: boolean } };
          };
          writes.push(settings);
          return settings;
        }
        case 'queue_status':
          return { schema_version: 1, queue_paused: false, jobs: [] };
        case 'take_pending_selections':
          return [];
        default:
          return null;
      }
    });
    render(<App />);
    const dialog = await screen.findByRole('dialog');
    fireEvent.click(within(dialog).getByText('Skip'));
    await waitFor(() =>
      expect(writes.some((w) => w.app.onboarding_complete === true)).toBe(true),
    );
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('starts on the Home screen with the drop zone visible', () => {
    render(<App />);
    expect(screen.getByText(/Drop files to print/i)).toBeInTheDocument();
  });

  it('navigates to the Printers screen and lists real printers from IPC', async () => {
    mockIPC((cmd) => {
      if (cmd === 'list_printers') return fixturePrinters;
      if (cmd === 'default_printer') return 'EPSON_L15160';
      if (cmd === 'discover_printers') return [];
      if (cmd === 'take_pending_selections') return [];
      return null;
    });
    render(<App />);
    const nav = screen.getByRole('navigation');
    fireEvent.click(within(nav).getByText('Printers'));
    // The enumerated printer name appears (master list + detail) once the
    // screen's IPC load resolves.
    await waitFor(() =>
      expect(screen.getAllByText(/EPSON L15160 Series/).length).toBeGreaterThan(
        0,
      ),
    );
  });

  it('integration: store profiles (custom included) feed the Print Job dropdown', async () => {
    mockShell();
    render(<App />);
    fireEvent.click(
      within(screen.getByRole('navigation')).getByText('Print Job'),
    );
    // The custom profile from list_profiles is offered alongside builtins.
    await waitFor(() =>
      expect(
        screen.getByRole('option', { name: 'Contracts duplex' }),
      ).toBeInTheDocument(),
    );
    expect(
      screen.getByRole('option', { name: 'Quick · built-in' }),
    ).toBeInTheDocument();
  });

  it('integration: the app default profile drives the initial Print Job selection (FR-022)', async () => {
    // Server marks the custom profile as the default.
    mockShell({
      list_profiles: fixtureProfiles.map((p) => ({
        ...p,
        default: p.name === 'Contracts duplex',
      })),
    });
    render(<App />);
    fireEvent.click(
      within(screen.getByRole('navigation')).getByText('Print Job'),
    );
    await waitFor(() =>
      expect(screen.getByDisplayValue('Contracts duplex')).toBeInTheDocument(),
    );
  });

  it('integration: an explicit profile choice survives navigating away and back', async () => {
    mockShell();
    render(<App />);
    const nav = screen.getByRole('navigation');
    fireEvent.click(within(nav).getByText('Print Job'));
    const select = await screen.findByDisplayValue('Quick · built-in');
    fireEvent.change(select, { target: { value: 'builtin-duplex-bw' } });
    expect(
      screen.getByDisplayValue('Duplex BW · built-in'),
    ).toBeInTheDocument();

    fireEvent.click(within(nav).getByText('Home'));
    fireEvent.click(within(nav).getByText('Print Job'));
    expect(
      screen.getByDisplayValue('Duplex BW · built-in'),
    ).toBeInTheDocument();
  });

  it('an explicit profile choice survives a transient profiles-store error', async () => {
    const CUSTOM_ID = '01890a5e-7b2c-7c3d-8e4f-1a2b3c4d5e6f';
    mockShell();
    render(<App />);
    const nav = screen.getByRole('navigation');
    fireEvent.click(within(nav).getByText('Print Job'));

    // Explicitly choose the custom profile.
    const select = await screen.findByDisplayValue('Quick · built-in');
    fireEvent.change(select, { target: { value: CUSTOM_ID } });
    expect(screen.getByDisplayValue('Contracts duplex')).toBeInTheDocument();

    // Snapshot the adapted list, then drive the store through a transient
    // error (profiles momentarily empty) and back to ready. The fallback
    // effect must not clear the explicit choice while the store is not ready.
    const adapted = useProfilesStore.getState().profiles;
    act(() => {
      useProfilesStore.setState({
        status: 'error',
        profiles: [],
        error: 'boom',
      });
    });
    act(() => {
      useProfilesStore.setState({
        status: 'ready',
        profiles: adapted,
        error: undefined,
      });
    });

    // The choice was preserved across the error (not reset to the default).
    expect(screen.getByDisplayValue('Contracts duplex')).toBeInTheDocument();
  });

  it('integration: the backend theme wins over the localStorage cache on boot', async () => {
    window.localStorage.setItem('pluryprint:theme', 'light');
    mockShell({
      get_app_settings: {
        ...fixtureAppSettings,
        app: { ...fixtureAppSettings.app, theme: 'dark' },
      },
    });
    render(<App />);
    await waitFor(() =>
      expect(document.documentElement.classList.contains('mp-dark')).toBe(true),
    );
    // The reconciled value is mirrored back into the localStorage cache.
    expect(window.localStorage.getItem('pluryprint:theme')).toBe('dark');
  });

  // -------------------------------------------------------------------------
  // OS context-menu selections (FR-001 / FR-003 / NFR-014)
  //
  // The seam these cover is the one the feature used to fall through: the
  // backend parked the selection and emitted the event, and nothing in the
  // webview ever asked for it, so "Print with Pluryprint" opened an empty
  // Home screen. Each test drives one of the two arrival routes end to end —
  // drain on mount, and the doorbell event — through the real ingestion
  // pipeline.
  // -------------------------------------------------------------------------

  describe('context-menu selections', () => {
    /** What `inspect_files` resolves the selection's one path to. */
    const REPORT = {
      path: '/tmp/Report.pdf',
      name: 'Report.pdf',
      ext: 'pdf',
      bytes: 2048,
      pages: 3,
      supported: true,
      reason: null,
      is_dir: false,
      has_subfolders: false,
    };

    const CUSTOM_PROFILE_ID = '01890a5e-7b2c-7c3d-8e4f-1a2b3c4d5e6f';

    interface Call {
      cmd: string;
      args: Record<string, unknown>;
    }

    function selection(
      action: 'configure' | 'print-default' | 'queue',
      profileId: string | null = null,
    ) {
      return { paths: ['/tmp/Report.pdf'], action, profile_id: profileId };
    }

    /**
     * Mock the shell with a *destructive* pending store, like the backend's:
     * whatever has been parked is handed out on the next drain and then gone,
     * so a selection can never be acted on twice however many drains run.
     */
    function mockSelections() {
      const calls: Call[] = [];
      let parked: unknown[] = [];
      // What `list_profiles` reads from disk; `writeProfiles` stands in for
      // another process (the CLI) rewriting profiles.toml.
      let onDisk: unknown[] = fixtureProfiles;
      // When set, reading profiles.toml fails (a corrupt file).
      let corrupt = false;
      mockIPC((cmd, args) => {
        calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> });
        switch (cmd) {
          case 'take_pending_selections': {
            const drained = parked;
            parked = [];
            return drained;
          }
          case 'inspect_files':
            // Every path is a printable PDF named after itself.
            return ((args as { paths: string[] }).paths ?? []).map((p) => ({
              ...REPORT,
              path: p,
              name: p.split('/').pop() ?? p,
            }));
          case 'print_files':
            return { batch_id: 'b1', job_ids: ['j1'] };
          case 'list_profiles':
            if (corrupt) {
              throw {
                code: 'Internal',
                message: 'profiles.toml is corrupt',
              };
            }
            return onDisk;
          case 'get_app_settings':
            return fixtureAppSettings;
          case 'queue_status':
            return { schema_version: 1, queue_paused: false, jobs: [] };
          default:
            return null;
        }
      });
      return {
        calls,
        park: (selections: unknown[]) => {
          // Appends, like the backend store: a second launch never erases a
          // selection that is still waiting to be drained.
          parked = [...parked, ...selections];
        },
        writeProfiles: (list: unknown[]) => {
          onDisk = list;
        },
        corruptProfiles: () => {
          corrupt = true;
        },
      };
    }

    /** Fire the backend's "a selection is waiting" doorbell. */
    function ringDoorbell() {
      act(() => {
        for (const cb of eventMock.registry.selectionForwarded) {
          cb({ payload: selection('configure') });
        }
      });
    }

    /** The selection's file, rendered as a row of the print job. */
    function reportRows() {
      return screen.findAllByRole('button', { name: /Report\.pdf/ });
    }

    it('FR-001: a selection parked before the window existed opens the job with its files', async () => {
      const { calls, park } = mockSelections();
      park([selection('configure')]);

      render(<App />);

      expect((await reportRows()).length).toBeGreaterThan(0);
      expect(calls.find((c) => c.cmd === 'inspect_files')?.args).toEqual({
        paths: ['/tmp/Report.pdf'],
      });
      // `configure` opens the dialog and stops there — the user still decides.
      expect(calls.some((c) => c.cmd === 'print_files')).toBe(false);
    });

    it('FR-005: a selection the shell extension had to cut short says so', async () => {
      const { park } = mockSelections();
      park([{ ...selection('configure'), truncated_from: 12000 }]);

      render(<App />);

      expect(
        await screen.findByText(
          /Only the first 1 of the 12000 selected items reached Pluryprint/,
        ),
      ).toBeInTheDocument();
      // The part that did arrive is still loaded.
      expect((await reportRows()).length).toBeGreaterThan(0);
    });

    it('NFR-014: a selection forwarded into an already-open window arrives too', async () => {
      const { calls, park } = mockSelections();
      render(<App />);
      // Nothing was waiting at launch: the app is on Home, untouched.
      await waitFor(() =>
        expect(eventMock.registry.selectionForwarded.size).toBe(1),
      );
      expect(screen.getByText(/Drop files to print/i)).toBeInTheDocument();

      // A second launch parks a selection and the backend rings the doorbell.
      park([selection('configure')]);
      ringDoorbell();

      expect((await reportRows()).length).toBeGreaterThan(0);
      expect(calls.filter((c) => c.cmd === 'inspect_files')).toHaveLength(1);
    });

    it('FR-001: “Print with Pluryprint” prints the selection with the profile it named', async () => {
      const { calls, park } = mockSelections();
      park([selection('print-default', CUSTOM_PROFILE_ID)]);

      render(<App />);

      await waitFor(() =>
        expect(calls.some((c) => c.cmd === 'print_files')).toBe(true),
      );
      const req = calls.find((c) => c.cmd === 'print_files')!.args.req as {
        files: string[];
      };
      expect(req.files).toEqual(['/tmp/Report.pdf']);
      // The profile the integration asked for, not the app default.
      expect(await screen.findByText('Contracts duplex')).toBeInTheDocument();
    });

    /** What the browser bridge hands over for a PluryFact batch. */
    const PLURYFACT = {
      source: 'pluryfact',
      label: 'Recibidos · octubre 2026',
      overrides: {
        copies: 2,
        duplex: 'long-edge',
        color: 'monochrome',
        paper: 'a4',
        collate: false,
      },
    } as const;

    it('FR-124: a PluryFact selection is labelled on the Print Job screen and pre-fills its settings', async () => {
      const { calls, park } = mockSelections();
      park([{ ...selection('configure'), ...PLURYFACT }]);

      render(<App />);

      expect((await reportRows()).length).toBeGreaterThan(0);
      const origin = await screen.findByTestId('job-origin');
      expect(origin).toHaveTextContent('From PluryFact');
      expect(origin).toHaveTextContent('Recibidos · octubre 2026');
      // FR-123: the settings PluryFact asked for are in the panel, unprinted.
      expect(screen.getByLabelText(/^Copies$/i)).toHaveValue(2);
      expect(calls.some((c) => c.cmd === 'print_files')).toBe(false);
    });

    it('FR-124: a PluryFact batch joining a job the user started is not labelled and keeps the job settings', async () => {
      const { calls, park } = mockSelections();
      // The user's own file, from Explorer, already in the job.
      park([{ ...selection('configure'), source: 'windows-explorer' }]);
      render(<App />);
      expect((await reportRows()).length).toBeGreaterThan(0);

      // Then PluryFact sends a batch (with its own print settings).
      park([
        {
          ...selection('configure'),
          ...PLURYFACT,
          paths: ['/tmp/inbox/0000-factura.pdf'],
        },
      ]);
      ringDoorbell();

      expect(
        (await screen.findAllByRole('button', { name: /0000-factura\.pdf/ }))
          .length,
      ).toBeGreaterThan(0);
      // Mixed job: no "From PluryFact" on the user's own file.
      expect(screen.queryByTestId('job-origin')).toBeNull();
      // The job keeps its settings, and the user is told why.
      expect(
        await screen.findByText(/PluryFact's print settings were not applied/),
      ).toBeInTheDocument();
      expect(screen.getByLabelText(/^Copies$/i)).toHaveValue(1);
      expect(calls.some((c) => c.cmd === 'print_files')).toBe(false);
    });

    it('FR-124: a selection from a file manager carries no PluryFact label', async () => {
      const { park } = mockSelections();
      park([{ ...selection('configure'), source: 'windows-explorer' }]);

      render(<App />);

      expect((await reportRows()).length).toBeGreaterThan(0);
      expect(screen.queryByTestId('job-origin')).toBeNull();
      expect(screen.queryByText('From PluryFact')).toBeNull();
    });

    it('FR-123: a PluryFact queue selection is submitted with its overrides on top of the profile', async () => {
      const { calls, park } = mockSelections();
      park([{ ...selection('queue'), ...PLURYFACT }]);

      render(<App />);

      await waitFor(() =>
        expect(calls.some((c) => c.cmd === 'print_files')).toBe(true),
      );
      const req = calls.find((c) => c.cmd === 'print_files')!.args.req as {
        hold?: boolean;
        settings: {
          copies: number;
          duplex: string;
          color_mode: string;
          paper_size: string;
          collate: boolean;
        };
      };
      expect(req.hold).toBe(true);
      expect(req.settings.copies).toBe(2);
      expect(req.settings.duplex).toBe('long');
      expect(req.settings.color_mode).toBe('monochrome');
      expect(req.settings.paper_size).toBe('a4');
      expect(req.settings.collate).toBe(false);
      // The confirmation names where the batch came from.
      expect(
        await screen.findByText(/From PluryFact · Recibidos · octubre 2026/),
      ).toBeInTheDocument();
    });

    it('FR-001: “Add to print queue” enqueues without starting the printer', async () => {
      const { calls, park } = mockSelections();
      park([selection('queue')]);

      render(<App />);

      await waitFor(() =>
        expect(calls.some((c) => c.cmd === 'print_files')).toBe(true),
      );
      // Only this batch is held (its jobs land paused); the rest of the queue
      // keeps running, so the global pause is never touched.
      const req = calls.find((c) => c.cmd === 'print_files')!.args.req as {
        hold?: boolean;
      };
      expect(req.hold).toBe(true);
      expect(calls.some((c) => c.cmd === 'pause_queue')).toBe(false);
    });

    it('FR-001: a print-now selection leaves the job the user was composing alone', async () => {
      const { calls, park } = mockSelections();
      render(<App />);
      await waitFor(() => expect(webview.handler).not.toBeNull());
      // The user stages a file on the Print Job screen and sets 5 copies…
      await act(async () => {
        webview.handler!({
          payload: { type: 'drop', paths: ['/tmp/Staged.pdf'] },
        });
      });
      const copies = await screen.findByDisplayValue('1');
      fireEvent.change(copies, { target: { value: '5' } });

      // …then right-clicks another file and picks "Print with Pluryprint".
      park([selection('print-default')]);
      ringDoorbell();
      await waitFor(() =>
        expect(calls.some((c) => c.cmd === 'print_files')).toBe(true),
      );
      const req = calls.find((c) => c.cmd === 'print_files')!.args.req as {
        files: string[];
        settings: { copies: number };
      };
      // Exactly the selection, with the profile as saved — not the job's
      // leftover overrides.
      expect(req.files).toEqual(['/tmp/Report.pdf']);
      expect(req.settings.copies).toBe(1);

      // The staged file is still there for the user.
      fireEvent.click(screen.getByRole('button', { name: /print job/i }));
      expect(
        (await screen.findAllByRole('button', { name: /Staged\.pdf/ })).length,
      ).toBeGreaterThan(0);
    });

    it('FR-008: a profile named by name (not id) is honoured', async () => {
      const { calls, park } = mockSelections();
      park([selection('print-default', 'contracts DUPLEX')]);
      render(<App />);
      await waitFor(() =>
        expect(calls.some((c) => c.cmd === 'print_files')).toBe(true),
      );
      const req = calls.find((c) => c.cmd === 'print_files')!.args.req as {
        profile_name: string;
      };
      expect(req.profile_name).toBe('Contracts duplex');
    });

    it('FR-001: a print-now selection naming an unknown profile prints nothing', async () => {
      const { calls, park } = mockSelections();
      park([selection('print-default', 'Nope')]);
      render(<App />);
      expect(
        await screen.findByText(
          /No profile is called “Nope”, so nothing was printed/,
        ),
      ).toBeInTheDocument();
      // The files land on the Print Job screen for the user to finish…
      expect((await reportRows()).length).toBeGreaterThan(0);
      // …and the default profile is never used behind their back.
      expect(calls.some((c) => c.cmd === 'print_files')).toBe(false);
    });

    it('FR-001: profiles that cannot be read are reported as such, never as a missing profile', async () => {
      const { calls, park, corruptProfiles } = mockSelections();
      corruptProfiles();
      park([selection('print-default', 'Invoices')]);
      render(<App />);
      expect(
        await screen.findByText(
          /Your profiles could not be read .*so nothing was printed/,
        ),
      ).toBeInTheDocument();
      expect(screen.queryByText(/No profile is called/)).toBeNull();
      expect((await reportRows()).length).toBeGreaterThan(0);
      expect(calls.some((c) => c.cmd === 'print_files')).toBe(false);
    });

    it('FR-001: with no readable profile list, “Print with Pluryprint” does not fall back to a built-in profile', async () => {
      const { calls, park, corruptProfiles } = mockSelections();
      corruptProfiles();
      park([selection('print-default')]);
      render(<App />);
      expect(
        await screen.findByText(/Your profiles could not be read/),
      ).toBeInTheDocument();
      expect((await reportRows()).length).toBeGreaterThan(0);
      expect(calls.some((c) => c.cmd === 'print_files')).toBe(false);
    });

    it('FR-001: a queue selection naming an unknown profile queues nothing', async () => {
      const { calls, park } = mockSelections();
      park([selection('queue', 'Nope')]);
      render(<App />);
      expect(
        await screen.findByText(/No profile is called “Nope”/),
      ).toBeInTheDocument();
      expect((await reportRows()).length).toBeGreaterThan(0);
      expect(calls.some((c) => c.cmd === 'print_files')).toBe(false);
    });

    it('FR-001: a profile created by the CLI after launch is honoured, not swapped for the default', async () => {
      const { calls, park, writeProfiles } = mockSelections();
      render(<App />);
      // The window has loaded its profile list…
      await waitFor(() =>
        expect(useProfilesStore.getState().status).toBe('ready'),
      );
      await waitFor(() =>
        expect(eventMock.registry.selectionForwarded.size).toBe(1),
      );
      // …then `pluryprint-cli profiles create` adds one, and the refreshed
      // shell menu offers it straight away.
      writeProfiles([
        ...fixtureProfiles,
        {
          ...fixtureProfiles[3]!,
          id: 'cli-created',
          name: 'E2E Hold',
          copies: 2,
          duplex: 'long-edge',
          default: false,
        },
      ]);
      park([selection('print-default', 'E2E Hold')]);
      ringDoorbell();

      await waitFor(() =>
        expect(calls.some((c) => c.cmd === 'print_files')).toBe(true),
      );
      const req = calls.find((c) => c.cmd === 'print_files')!.args.req as {
        profile_name: string;
        settings: { copies: number };
      };
      expect(req.profile_name).toBe('E2E Hold');
      expect(req.settings.copies).toBe(2);
    });

    it('NFR-014: selections that arrive back to back are all printed, in order', async () => {
      const { calls, park } = mockSelections();
      render(<App />);
      await waitFor(() =>
        expect(eventMock.registry.selectionForwarded.size).toBe(1),
      );
      park([selection('print-default')]);
      ringDoorbell();
      park([selection('queue')]);
      ringDoorbell();
      await waitFor(() =>
        expect(calls.filter((c) => c.cmd === 'print_files')).toHaveLength(2),
      );
      const reqs = calls
        .filter((c) => c.cmd === 'print_files')
        .map((c) => c.args.req as { hold?: boolean });
      expect(reqs.map((r) => r.hold === true)).toEqual([false, true]);
    });

    it('does nothing at all when no selection was handed to this window', async () => {
      const { calls } = mockSelections();

      render(<App />);

      await waitFor(() =>
        expect(calls.some((c) => c.cmd === 'take_pending_selections')).toBe(
          true,
        ),
      );
      expect(screen.getByText(/Drop files to print/i)).toBeInTheDocument();
      expect(calls.some((c) => c.cmd === 'inspect_files')).toBe(false);
      expect(calls.some((c) => c.cmd === 'print_files')).toBe(false);
    });
  });

  // -------------------------------------------------------------------------
  // Launch-time update check (FR-080 / FR-084)
  // -------------------------------------------------------------------------

  describe('update-available notification', () => {
    /**
     * What the backend pushes after a launch-time check found something. The
     * decision to push at all (preference, 24h throttle, skipped version) is
     * made in Rust and covered by `src-tauri/tests/update_startup.rs`; the
     * frontend's job starts here.
     */
    const CHECK = {
      current: '1.0.0',
      latest: 'v1.2.0',
      available: true,
      url: 'https://github.com/MAECLY/pluryprint/releases/tag/v1.2.0',
      notes: 'Faster printing',
      checked_at: '2026-07-20T12:00:00Z',
    };

    function emitUpdateAvailable(payload: unknown = CHECK) {
      const set = eventMock.registry.updateAvailable as Set<EventListener>;
      act(() => {
        for (const cb of set) cb({ payload });
      });
    }

    /** Render App and wait until its event listener is registered. */
    async function renderSubscribed() {
      render(<App />);
      await waitFor(() =>
        expect(eventMock.registry.updateAvailable.size).toBe(1),
      );
    }

    it('FR-080: tells the user from whatever screen they are on', async () => {
      mockShell();
      await renderSubscribed();
      // Still on Home — the point is that the About screen is not required.
      expect(screen.getByText(/Drop files to print/i)).toBeInTheDocument();

      emitUpdateAvailable();
      expect(
        await screen.findByText('Version v1.2.0 is available'),
      ).toBeInTheDocument();
      expect(screen.getByText("You're on 1.0.0.")).toBeInTheDocument();
    });

    it('links to the release notes and leaves installing to the About screen', async () => {
      const calls: string[] = [];
      mockIPC((cmd, args) => {
        calls.push(cmd);
        if (cmd === 'open_external')
          return (args as { url: string } | undefined)?.url ?? null;
        if (cmd === 'get_app_settings') return fixtureAppSettings;
        if (cmd === 'list_profiles') return fixtureProfiles;
        if (cmd === 'queue_status')
          return { schema_version: 1, queue_paused: false, jobs: [] };
        if (cmd === 'take_pending_selections') return [];
        return null;
      });
      await renderSubscribed();
      emitUpdateAvailable();

      // Installing needs the About screen's context (FR-087's major-version
      // confirmation, progress, restart), so the toast never offers it.
      const action = await screen.findByRole('button', {
        name: 'Release notes',
      });
      expect(
        screen.queryByRole('button', { name: /install|update now/i }),
      ).toBeNull();

      fireEvent.click(action);
      await waitFor(() => expect(calls).toContain('open_external'));
    });

    it('FR-080: a release link that cannot open says so instead of failing silently', async () => {
      mockIPC((cmd) => {
        if (cmd === 'open_external') throw new Error('no default browser');
        if (cmd === 'get_app_settings') return fixtureAppSettings;
        if (cmd === 'list_profiles') return fixtureProfiles;
        if (cmd === 'queue_status')
          return { schema_version: 1, queue_paused: false, jobs: [] };
        if (cmd === 'take_pending_selections') return [];
        return null;
      });
      await renderSubscribed();
      emitUpdateAvailable();

      fireEvent.click(
        await screen.findByRole('button', { name: 'Release notes' }),
      );
      expect(await screen.findByText('no default browser')).toBeInTheDocument();
    });

    it('says nothing when the payload names no version', async () => {
      mockShell();
      await renderSubscribed();
      emitUpdateAvailable({ ...CHECK, latest: null, url: null });
      await waitFor(() =>
        expect(screen.queryByText(/is available/)).toBeNull(),
      );
    });
  });

  // -------------------------------------------------------------------------
  // Round 6: the retired headless-browser renderer, told once at launch
  // -------------------------------------------------------------------------

  describe('retired HTML renderer notice', () => {
    afterEach(() => {
      // The notice stays until dismissed; never leak it into other tests.
      act(() => {
        toast.dismiss();
      });
    });

    it('FR-055: tells a user who had chosen the browser renderer, once', async () => {
      const calls: string[] = [];
      mockIPC((cmd) => {
        calls.push(cmd);
        switch (cmd) {
          case 'take_retired_renderer_notice':
            return true;
          case 'get_app_settings':
            return fixtureAppSettings;
          case 'list_profiles':
            return fixtureProfiles;
          case 'queue_status':
            return { schema_version: 1, queue_paused: false, jobs: [] };
          case 'take_pending_selections':
            return [];
          default:
            return null;
        }
      });
      render(<App />);
      expect(
        await screen.findByText('Web pages now print with the system renderer'),
      ).toBeInTheDocument();
      expect(
        screen.getByText(/headless Edge\/Chrome renderer .* has been removed/),
      ).toBeInTheDocument();
      expect(
        calls.filter((c) => c === 'take_retired_renderer_notice').length,
      ).toBeGreaterThanOrEqual(1);
    });

    it('says nothing when there was no retired renderer', async () => {
      const calls: string[] = [];
      mockIPC((cmd) => {
        calls.push(cmd);
        if (cmd === 'take_retired_renderer_notice') return false;
        if (cmd === 'take_pending_selections') return [];
        if (cmd === 'get_app_settings') return fixtureAppSettings;
        return null;
      });
      render(<App />);
      await waitFor(() =>
        expect(calls).toContain('take_retired_renderer_notice'),
      );
      expect(
        screen.queryByText('Web pages now print with the system renderer'),
      ).toBeNull();
    });
  });

  // -------------------------------------------------------------------------
  // Keyboard shortcuts (NFR-034)
  // -------------------------------------------------------------------------

  describe('keyboard shortcuts', () => {
    interface Call {
      cmd: string;
      args: Record<string, unknown>;
    }

    /** `mockShell` plus a recording of every command that went out. */
    function recordShell(overrides: Record<string, unknown> = {}): Call[] {
      const calls: Call[] = [];
      mockIPC((cmd, args) => {
        calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> });
        if (cmd in overrides) {
          const answer = overrides[cmd];
          if (answer instanceof Error) throw answer;
          return answer;
        }
        switch (cmd) {
          case 'list_profiles':
            return fixtureProfiles;
          case 'get_app_settings':
            return fixtureAppSettings;
          case 'queue_status':
            return { schema_version: 1, queue_paused: false, jobs: [] };
          case 'list_printers':
            return [];
          case 'get_printer_prefs':
            return {};
          case 'inspect_files':
            return [
              {
                path: '/tmp/Report.pdf',
                name: 'Report.pdf',
                ext: 'pdf',
                bytes: 2048,
                pages: 3,
                supported: true,
                reason: null,
                is_dir: false,
                has_subfolders: false,
              },
            ];
          case 'print_files':
            return { batch_id: 'b1', job_ids: ['j1'] };
          case 'take_pending_selections':
            return [];
          default:
            return null;
        }
      });
      return calls;
    }

    /** Press a shortcut the way the platform modifier requires. */
    function press(key: string, target: Element | Window = window) {
      fireEvent.keyDown(target, {
        key,
        ctrlKey: !shortcutLabel('newJob').startsWith('⌘'),
        metaKey: shortcutLabel('newJob').startsWith('⌘'),
      });
    }

    it('NFR-034: opens a new print job and the queue from anywhere', async () => {
      recordShell();
      render(<App />);
      press('n');
      expect(await screen.findByText('Add files')).toBeInTheDocument();
      press('j');
      // The Queue screen's own body, not the sidebar entry.
      expect(await screen.findByText(/No jobs in the queue/i)).toBeVisible();
    });

    it('NFR-034: switches the job to the next profile', async () => {
      recordShell();
      render(<App />);
      press('n');
      const select = await screen.findByDisplayValue('Quick · built-in');
      expect(select).toBeInTheDocument();
      press('k');
      await waitFor(() =>
        expect(screen.queryByDisplayValue('Quick · built-in')).toBeNull(),
      );
    });

    it('NFR-034: prints the current job, after a drop put files in it', async () => {
      const calls = recordShell();
      render(<App />);
      await waitFor(() => expect(webview.handler).not.toBeNull());
      await act(async () => {
        webview.handler!({
          payload: { type: 'drop', paths: ['/tmp/Report.pdf'] },
        });
      });
      // The dropped file is now a row of the job (its per-row controls name it).
      expect(
        (await screen.findAllByRole('button', { name: /Report\.pdf/ })).length,
      ).toBeGreaterThan(0);

      press('p');
      await waitFor(() =>
        expect(calls.some((c) => c.cmd === 'print_files')).toBe(true),
      );
    });

    /**
     * Round-6 e2e flake: a drop made right after clicking Print Job did
     * nothing. The screen's own listener was still registering, and the
     * shell's stood down because the screen was active, so neither took it.
     */
    it('FR-007: a drop made while Print Job is still registering its listener is not lost', async () => {
      const calls = recordShell();
      render(<App />);
      await waitFor(() => expect(webview.handler).not.toBeNull());
      const shell = webview.handler!;
      // The Print Job screen's listener never finishes registering.
      webview.hold = true;
      press('n');
      expect(await screen.findByText('Add files')).toBeInTheDocument();
      await act(async () => {
        shell({ payload: { type: 'drop', paths: ['/tmp/Report.pdf'] } });
      });
      expect(
        (await screen.findAllByRole('button', { name: /Report\.pdf/ })).length,
      ).toBeGreaterThan(0);
      expect(calls.filter((c) => c.cmd === 'inspect_files')).toHaveLength(1);
    });

    it('FR-007: once Print Job listens, one drop is taken once', async () => {
      const calls = recordShell();
      render(<App />);
      await waitFor(() => expect(webview.handlers).toHaveLength(1));
      press('n');
      await waitFor(() => expect(webview.handlers).toHaveLength(2));
      // Let the screen's registration complete.
      await act(async () => {});
      const drop = { payload: { type: 'drop', paths: ['/tmp/Report.pdf'] } };
      await act(async () => {
        for (const h of webview.handlers) h(drop);
      });
      expect(
        (await screen.findAllByRole('button', { name: /Report\.pdf/ })).length,
      ).toBeGreaterThan(0);
      expect(calls.filter((c) => c.cmd === 'inspect_files')).toHaveLength(1);
    });

    it('NFR-033: a failed print from a shortcut offers the logs folder', async () => {
      const calls = recordShell({ print_files: new Error('spooler offline') });
      render(<App />);
      await waitFor(() => expect(webview.handler).not.toBeNull());
      await act(async () => {
        webview.handler!({
          payload: { type: 'drop', paths: ['/tmp/Report.pdf'] },
        });
      });
      expect(
        (await screen.findAllByRole('button', { name: /Report\.pdf/ })).length,
      ).toBeGreaterThan(0);

      press('p');
      expect(await screen.findByText('spooler offline')).toBeInTheDocument();
      fireEvent.click(
        await screen.findByRole('button', { name: 'Open the logs folder' }),
      );
      await waitFor(() =>
        expect(calls.some((c) => c.cmd === 'open_logs_folder')).toBe(true),
      );
    });

    it('FR-042: Ctrl/⌘+P refuses a job whose copies field is invalid', async () => {
      const calls = recordShell();
      render(<App />);
      await waitFor(() => expect(webview.handler).not.toBeNull());
      await act(async () => {
        webview.handler!({
          payload: { type: 'drop', paths: ['/tmp/Report.pdf'] },
        });
      });
      const copies = await screen.findByDisplayValue('1');
      // Typed one keystroke at a time: 120 is valid and gets committed, 1200
      // is not — the shortcut must not print the stale 120.
      fireEvent.change(copies, { target: { value: '120' } });
      fireEvent.change(copies, { target: { value: '1200' } });
      press('p');
      expect(
        await screen.findByText(/cannot be printed as it is/i),
      ).toBeInTheDocument();
      expect(calls.some((c) => c.cmd === 'print_files')).toBe(false);
    });

    it('FR-041: Ctrl/⌘+K shows the new profile’s copies and drops old overrides', async () => {
      recordShell();
      render(<App />);
      press('n');
      const copies = await screen.findByDisplayValue('1');
      fireEvent.change(copies, { target: { value: '7' } });
      expect(screen.getByDisplayValue('7')).toBeInTheDocument();
      press('k');
      // The next profile's own value, not the override made against Quick.
      await waitFor(() =>
        expect(screen.queryByDisplayValue('7')).not.toBeInTheDocument(),
      );
    });

    it('NFR-034: printing with an empty job opens it instead of spooling nothing', async () => {
      const calls = recordShell();
      render(<App />);
      press('p');
      expect(await screen.findByText('Add files')).toBeInTheDocument();
      expect(calls.some((c) => c.cmd === 'print_files')).toBe(false);
    });

    it('NFR-034: cancel asks first, then cancels the job on the printer', async () => {
      const calls = recordShell();
      render(<App />);
      act(() => {
        useQueueStore.setState({
          status: 'ready',
          uiJobs: [
            toUiQueueJob(
              mkJob({
                id: 'j-printing',
                state: 'printing',
                file_name: 'Q3-Report.pdf',
              }),
            ),
          ],
        });
      });
      press('.');
      // Never `window.confirm`: a real in-app dialog naming the job.
      const dialog = await screen.findByRole('dialog');
      expect(within(dialog).getByText(/Q3-Report\.pdf/)).toBeInTheDocument();
      expect(calls.some((c) => c.cmd === 'cancel_job')).toBe(false);

      fireEvent.click(
        within(dialog).getByRole('button', { name: 'Cancel the job' }),
      );
      await waitFor(() =>
        expect(calls.find((c) => c.cmd === 'cancel_job')?.args).toEqual({
          id: 'j-printing',
        }),
      );
    });

    it('NFR-034: cancel with an idle queue says so instead of opening a dialog', async () => {
      recordShell();
      render(<App />);
      press('.');
      // The "says so" half: without it this test passes even if the shortcut
      // does nothing at all, since an absent dialog is also the initial state.
      expect(
        await screen.findByText('No job is printing or waiting right now.'),
      ).toBeInTheDocument();
      await waitFor(() =>
        expect(screen.queryByRole('dialog')).not.toBeInTheDocument(),
      );
    });

    it('NFR-041: shortcuts stand down while the user is typing in a field', async () => {
      recordShell();
      render(<App />);
      press('n');
      const input = await screen.findByPlaceholderText(/1-5/i);
      fireEvent.change(input, { target: { value: '1-3' } });
      // Ctrl+J inside the field must reach the field, not the shell.
      press('j', input);
      expect(screen.queryByText(/No jobs in the queue/i)).toBeNull();
      expect(input).toBeInTheDocument();
    });

    it('NFR-034: Ctrl/⌘+P never reaches the webview, even while typing', async () => {
      recordShell();
      render(<App />);
      press('n');
      const input = await screen.findByPlaceholderText(/1-5/i);
      const event = new KeyboardEvent('keydown', {
        key: 'p',
        bubbles: true,
        cancelable: true,
        ctrlKey: !shortcutLabel('newJob').startsWith('⌘'),
        metaKey: shortcutLabel('newJob').startsWith('⌘'),
      });
      input.dispatchEvent(event);
      // Our print stands down in a field, but the browser's print dialog of
      // the app's own UI must not open either.
      expect(event.defaultPrevented).toBe(true);
    });

    it('regression: a bare keypress (no modifier) never triggers a shortcut', async () => {
      recordShell();
      render(<App />);
      fireEvent.keyDown(window, { key: 'j' });
      fireEvent.keyDown(window, { key: 'n' });
      await waitFor(() =>
        expect(screen.getByText(/Drop files to print/i)).toBeInTheDocument(),
      );
    });
  });
});
