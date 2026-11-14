/**
 * Home screen tests.
 *
 * Hermetic: the queue and profile stores are driven directly (no IPC host, no
 * user config) and `sonner` is mocked, so nothing here touches the network, the
 * filesystem of the user, or a real backend. The one file read is the engine's
 * own source inside this repository — the regression that keeps the advertised
 * format list from drifting away from what the engine accepts.
 */

import {
  cleanup,
  render,
  screen,
  fireEvent,
  within,
} from '@testing-library/react';
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

const toasts = vi.hoisted(() => ({
  error: vi.fn(),
  warning: vi.fn(),
  success: vi.fn(),
  info: vi.fn(),
}));
vi.mock('sonner', () => ({ toast: toasts }));

import { HomeScreen, PRINTABLE_EXTENSIONS } from './HomeScreen';
import { shortcutLabel, APP_SHORTCUTS } from '@/components/AppShell/MPAppShell';
import { BUILT_IN_PROFILES } from '@/lib/profiles';
import { toUiQueueJob } from '@/lib/ipc';
import { fixtureAppInfo, mkJob } from '@/test/fixtures';
import { useProfilesStore } from '@/stores/profilesStore';
import { useQueueStore } from '@/stores/queueStore';
import type { UiQueueJobView } from '@/lib/ipc';

const REPO_ROOT = process.cwd();

const doneJob = toUiQueueJob(
  mkJob({
    id: 'j-done',
    state: 'done',
    file_name: 'Site-Photo-North.jpg',
    profile_name: 'Photo Color',
    printer_resolved: 'EPSON L15160',
    finished_at: '2026-07-11T10:39:31Z',
  }),
);
const failedJob = toUiQueueJob(
  mkJob({
    id: 'j-failed',
    state: 'failed',
    file_name: 'Report-Broken.pdf',
    error: { code: 'print_failed', message: 'Paper jam in Tray 2' },
    finished_at: '2026-07-11T10:38:55Z',
  }),
);

/** Put the queue store in a known state; `ready` unless told otherwise. */
function setQueue(
  recentDone: UiQueueJobView[],
  over: Partial<{
    status: 'idle' | 'loading' | 'ready' | 'unavailable' | 'error';
    error: string;
  }> = {},
) {
  useQueueStore.setState({
    status: over.status ?? 'ready',
    error: over.error,
    recentDone,
  });
}

function renderHome(onAction = vi.fn()) {
  render(<HomeScreen onAction={onAction} />);
  return onAction;
}

describe('HomeScreen', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useProfilesStore.setState({
      status: 'ready',
      profiles: BUILT_IN_PROFILES,
      error: undefined,
    });
    setQueue([]);
  });

  // -------------------------------------------------------------------------
  // Formats advertised by the drop zone
  // -------------------------------------------------------------------------

  it('FR-005/FR-052..057: the drop zone names every format family the engine prints', () => {
    renderHome();
    const zone = screen.getByRole('button', { name: /click to choose them/i });
    // Family label in the copy → the extensions it stands for. Every printable
    // extension must be covered by exactly one advertised family.
    const families: Record<string, string[]> = {
      PDF: ['pdf'],
      JPG: ['jpg', 'jpeg'],
      PNG: ['png'],
      TIFF: ['tif', 'tiff'],
      BMP: ['bmp'],
      WebP: ['webp'],
      HEIC: ['heic', 'heif'],
      SVG: ['svg', 'svgz'],
      Word: ['docx', 'doc'],
      Excel: ['xlsx', 'xls'],
      PowerPoint: ['pptx', 'ppt'],
      OpenDocument: ['odt', 'ods', 'odp'],
      TXT: ['txt'],
      RTF: ['rtf'],
      HTML: ['html', 'htm'],
      Markdown: ['md', 'markdown'],
      XML: ['xml'],
    };
    for (const label of Object.keys(families)) {
      expect(zone).toHaveTextContent(label);
    }
    const covered = Object.values(families).flat().sort();
    expect(covered).toEqual([...PRINTABLE_EXTENSIONS].sort());
    // Nothing the engine rejects is promised.
    for (const absent of [/\bGIF\b/, /\bEPUB\b/]) {
      expect(document.body.textContent ?? '').not.toMatch(absent);
    }
  });

  it('regression: the advertised list matches SUPPORTED_EXTS in the Rust engine', () => {
    const source = readFileSync(
      join(REPO_ROOT, 'src-tauri/src/engine/mod.rs'),
      'utf8',
    );
    const decl = /pub const SUPPORTED_EXTS: &\[&str\] = &\[([^\]]*)\]/.exec(
      source,
    );
    expect(decl, 'SUPPORTED_EXTS not found in the engine').not.toBeNull();
    const fromRust = Array.from(decl![1]!.matchAll(/"([a-z0-9]+)"/g)).map(
      (m) => m[1]!,
    );
    expect(fromRust.length).toBeGreaterThan(0);
    expect([...PRINTABLE_EXTENSIONS]).toEqual(fromRust);
  });

  // -------------------------------------------------------------------------
  // Quick actions (each one its own destination)
  // -------------------------------------------------------------------------

  it('FR-004: every quick action triggers its own action, not the same one', () => {
    const onAction = renderHome();
    fireEvent.click(screen.getByRole('button', { name: /^new print job$/i }));
    expect(onAction).toHaveBeenLastCalledWith('newjob');

    fireEvent.click(screen.getByRole('button', { name: /print a folder/i }));
    expect(onAction).toHaveBeenLastCalledWith('folder');

    fireEvent.click(
      screen.getByRole('button', { name: /click to choose them/i }),
    );
    expect(onAction).toHaveBeenLastCalledWith('files');

    fireEvent.click(screen.getByRole('button', { name: /view all queue/i }));
    expect(onAction).toHaveBeenLastCalledWith('queue');
    expect(onAction.mock.calls.map((c) => c[0])).toEqual([
      'newjob',
      'folder',
      'files',
      'queue',
    ]);
  });

  it('NFR-041: the quick actions are real buttons, operable with Enter alone', () => {
    const onAction = renderHome();
    const folder = screen.getByRole('button', { name: /print a folder/i });
    folder.focus();
    expect(folder).toHaveFocus();
    // A <button> fires click on Enter/Space natively; the regression this
    // guards is the old `<div onClick>`, which never reaches this assertion.
    fireEvent.click(document.activeElement!);
    expect(onAction).toHaveBeenCalledWith('folder');
  });

  // -------------------------------------------------------------------------
  // CLI (FR-008) — the command exists, so the card hands it over
  // -------------------------------------------------------------------------

  it('FR-008/FR-009: off macOS the CLI card copies a pluryprint-cli command PowerShell can run', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: { writeText },
    });
    renderHome();
    const card = screen.getByRole('button', { name: /run from cli/i });
    // pluryprint.exe is a GUI program PowerShell does not wait for; the
    // console twin is what a script runs.
    expect(card).toHaveTextContent('pluryprint-cli print --files');
    // Built around the profile the app really defaults to.
    expect(card).toHaveTextContent('--profile Quick');
    // Outside the desktop host the path is unknown, so the card says so.
    expect(card).toHaveTextContent(/not on your PATH yet/i);
    expect(card).toHaveTextContent(
      /replace pluryprint-cli with its full path/i,
    );

    fireEvent.click(card);
    await vi.waitFor(() => expect(writeText).toHaveBeenCalledTimes(1));
    expect(writeText.mock.calls[0]![0]).toBe(
      "pluryprint-cli print --files '*.pdf' --profile Quick",
    );
    expect(toasts.success).toHaveBeenCalled();
  });

  it('FR-009: in the Windows app, the card names pluryprint-cli.exe next to the app, for PowerShell', async () => {
    mockIPC((cmd) => {
      if (cmd === 'app_info') {
        return {
          ...fixtureAppInfo,
          os: 'windows',
          executable:
            'C:\\Users\\Ana\\AppData\\Local\\Pluryprint\\pluryprint.exe',
        };
      }
      return null;
    });
    try {
      renderHome();
      const card = screen.getByRole('button', { name: /run from cli/i });
      // The card shows the command with the folder folded into "…", so a
      // long install path cannot stretch it; the tooltip (and the copy)
      // carry the full path.
      await vi.waitFor(() =>
        expect(card).toHaveTextContent(
          "…\\pluryprint-cli.exe print --files '*.pdf' --profile Quick",
        ),
      );
      expect(card).not.toHaveTextContent('AppData');
      expect(card).toHaveAttribute(
        'title',
        "C:\\Users\\Ana\\AppData\\Local\\Pluryprint\\pluryprint-cli.exe print --files '*.pdf' --profile Quick",
      );
    } finally {
      clearMocks();
      delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
      delete (window as unknown as Record<string, unknown>).__TAURI__;
    }
  });

  it('FR-006/FR-008: in the app, the command names the real executable, quoted', async () => {
    mockIPC((cmd) => {
      if (cmd === 'app_info') {
        return {
          ...fixtureAppInfo,
          os: 'macos',
          executable:
            '/Applications/Pluryprint App.app/Contents/MacOS/pluryprint',
        };
      }
      return null;
    });
    useProfilesStore.setState({
      profiles: BUILT_IN_PROFILES.map((p) =>
        p.default ? { ...p, name: 'Costs $HOME' } : p,
      ),
    });
    try {
      renderHome();
      const card = screen.getByRole('button', { name: /run from cli/i });
      await vi.waitFor(() =>
        expect(card).toHaveAttribute(
          'title',
          "'/Applications/Pluryprint App.app/Contents/MacOS/pluryprint' print --files '*.pdf' --profile 'Costs $HOME'",
        ),
      );
      expect(card).toHaveTextContent(
        "'…/pluryprint' print --files '*.pdf' --profile 'Costs $HOME'",
      );
      expect(card).toHaveTextContent(/full path/i);
      // …and so does the tip, which only now may say the card includes it.
      expect(document.body.textContent).toMatch(
        /the Run from CLI card copies a command that already does/,
      );
    } finally {
      clearMocks();
      delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
      delete (window as unknown as Record<string, unknown>).__TAURI__;
    }
  });

  it('FR-008: from the .deb, the command is the bare pluryprint already on PATH', async () => {
    mockIPC((cmd) => {
      if (cmd === 'app_info') {
        return {
          ...fixtureAppInfo,
          os: 'linux',
          arch: 'x86_64',
          executable: '/usr/bin/pluryprint',
        };
      }
      return null;
    });
    try {
      renderHome();
      const card = screen.getByRole('button', { name: /run from cli/i });
      await vi.waitFor(() =>
        expect(card).toHaveAttribute(
          'title',
          "pluryprint print --files '*.pdf' --profile Quick",
        ),
      );
      expect(card).toHaveTextContent(/already on your PATH/i);
      const text = document.body.textContent ?? '';
      expect(text).toMatch(/\.deb package put it on your PATH/);
      // FR-001: the .deb registers "Open with" and the Dolphin/Nemo actions.
      expect(text).toMatch(/choose Pluryprint under Open With/);
      expect(text).toMatch(/Dolphin and Nemo/);
      expect(text).not.toMatch(/Explorer|Finder|pluryprint-cli/);
    } finally {
      clearMocks();
      delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
      delete (window as unknown as Record<string, unknown>).__TAURI__;
    }
  });

  it('FR-008: an AppImage that updated itself in place is named by its real file', async () => {
    mockIPC((cmd) => {
      if (cmd === 'app_info') {
        return {
          ...fixtureAppInfo,
          os: 'linux',
          arch: 'x86_64',
          version: '0.0.6',
          bundle_type: 'appimage',
          executable: '/home/ada/Apps/Pluryprint_0.0.5_linux_x64.AppImage',
        };
      }
      return null;
    });
    try {
      renderHome();
      const card = screen.getByRole('button', { name: /run from cli/i });
      await vi.waitFor(() =>
        expect(card).toHaveAttribute(
          'title',
          "/home/ada/Apps/Pluryprint_0.0.5_linux_x64.AppImage print --files '*.pdf' --profile Quick",
        ),
      );
      expect(card).not.toHaveTextContent('0.0.6_linux');
    } finally {
      clearMocks();
      delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
      delete (window as unknown as Record<string, unknown>).__TAURI__;
    }
  });

  it('FR-008: from an AppImage, the command runs the AppImage file, never its temporary mount', async () => {
    mockIPC((cmd) => {
      if (cmd === 'app_info') {
        return {
          ...fixtureAppInfo,
          os: 'linux',
          arch: 'aarch64',
          version: '0.0.5',
          executable: '/tmp/.mount_PluryZ9/usr/bin/pluryprint',
        };
      }
      return null;
    });
    try {
      renderHome();
      const card = screen.getByRole('button', { name: /run from cli/i });
      await vi.waitFor(() =>
        expect(card).toHaveAttribute(
          'title',
          "./Pluryprint_0.0.5_linux_arm64.AppImage print --files '*.pdf' --profile Quick",
        ),
      );
      expect(card).not.toHaveTextContent('.mount_');
      expect(card).toHaveTextContent(/from the folder you saved it in/);
      const text = document.body.textContent ?? '';
      // An AppImage registers no right-click entry by itself: say so.
      expect(text).toMatch(/An AppImage adds no right-click entry by itself/);
      expect(text).not.toMatch(/Dolphin and Nemo also offer/);
    } finally {
      clearMocks();
      delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
      delete (window as unknown as Record<string, unknown>).__TAURI__;
    }
  });

  it('FR-008: a clipboard refusal is reported instead of silently doing nothing', async () => {
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: {
        writeText: vi.fn().mockRejectedValue(new Error('denied')),
      },
    });
    renderHome();
    fireEvent.click(screen.getByRole('button', { name: /run from cli/i }));
    await vi.waitFor(() => expect(toasts.error).toHaveBeenCalledTimes(1));
  });

  // -------------------------------------------------------------------------
  // Recent activity
  // -------------------------------------------------------------------------

  it('renders the recent jobs from the queue store, newest first', () => {
    setQueue([doneJob, failedJob]);
    renderHome();
    expect(screen.getByText('Site-Photo-North.jpg')).toBeInTheDocument();
    expect(screen.getByText('Report-Broken.pdf')).toBeInTheDocument();
    // Status is rendered with the shared queue labels, not the raw state.
    expect(screen.getByText('Done')).toBeInTheDocument();
    expect(screen.getByText('Failed')).toBeInTheDocument();
  });

  it('shows the empty state when the queue is ready and has no history', () => {
    renderHome();
    expect(screen.getByText(/no recent activity yet/i)).toBeInTheDocument();
  });

  it('uses the id of the clicked job to reveal its details (and can hide them)', () => {
    setQueue([doneJob, failedJob]);
    renderHome();
    const row = screen.getByRole('button', {
      name: /show the details of Report-Broken\.pdf/i,
    });
    expect(row).toHaveAttribute('aria-expanded', 'false');
    // NFR-044: the status is part of the row's accessible name.
    expect(row).toHaveAccessibleName(/Report-Broken\.pdf \(Failed\)/);
    fireEvent.click(row);
    expect(row).toHaveAttribute('aria-expanded', 'true');
    expect(
      document.getElementById(row.getAttribute('aria-controls') ?? ''),
    ).toHaveTextContent(/Paper jam in Tray 2/);
    // The detail belongs to the clicked job, not to the first row.
    expect(screen.getByText(/Paper jam in Tray 2/)).toBeInTheDocument();
    // The other row stays collapsed.
    expect(
      screen.getByRole('button', {
        name: /show the details of Site-Photo-North\.jpg/i,
      }),
    ).toHaveAttribute('aria-expanded', 'false');

    fireEvent.click(
      screen.getByRole('button', {
        name: /hide the details of Report-Broken\.pdf/i,
      }),
    );
    expect(screen.queryByText(/Paper jam in Tray 2/)).not.toBeInTheDocument();
  });

  it('says "unknown" for a page count it does not have, and dates an older finish', () => {
    const old = toUiQueueJob({
      ...mkJob({
        id: 'j-old',
        state: 'failed',
        file_name: 'Huge.pdf',
        finished_at: '2020-01-02T10:00:00Z',
      }),
      // A PDF too large to probe: the backend never learned its page count.
      pages: null,
    });
    setQueue([old]);
    renderHome();
    fireEvent.click(
      screen.getByRole('button', { name: /show the details of Huge\.pdf/i }),
    );
    expect(screen.getByText(/Pages: unknown/)).toBeInTheDocument();
    expect(screen.queryByText(/Pages: 0/)).not.toBeInTheDocument();
    // Not today, so the date is part of it (not a bare clock).
    expect(screen.getByText(/Finished:.*2020/)).toBeInTheDocument();
  });

  it('FR-031: an unconfirmed job is a warning in recent activity, never a red "Error"', () => {
    const unconfirmed = toUiQueueJob(
      mkJob({
        id: 'j-unconfirmed',
        state: 'unconfirmed',
        file_name: 'Maybe-Printed.pdf',
        error: {
          code: 'print_unconfirmed',
          reason: 'left_queue',
          message: 'left the queue',
        },
        finished_at: '2026-07-11T10:39:00Z',
      }),
    );
    setQueue([unconfirmed]);
    renderHome();
    fireEvent.click(
      screen.getByRole('button', {
        name: /show the details of Maybe-Printed\.pdf/i,
      }),
    );
    const line = screen.getByText(/^Not confirmed:/);
    expect(line).toHaveStyle({ color: 'var(--mp-warn)' });
    expect(screen.queryByText(/^Error:/)).not.toBeInTheDocument();
  });

  it('offers a way into the queue from the expanded job', () => {
    setQueue([doneJob]);
    const onAction = renderHome();
    fireEvent.click(
      screen.getByRole('button', { name: /show the details of/i }),
    );
    fireEvent.click(screen.getByRole('button', { name: /open the queue/i }));
    expect(onAction).toHaveBeenCalledWith('queue');
  });

  // -------------------------------------------------------------------------
  // Queue failures are not disguised as an empty history (NFR-033)
  // -------------------------------------------------------------------------

  it('NFR-033: a queue read failure is reported with its message and a retry', () => {
    const init = vi.fn().mockResolvedValue(undefined);
    setQueue([], { status: 'error', error: 'queue.json is corrupt' });
    useQueueStore.setState({ init });
    renderHome();
    expect(
      screen.queryByText(/no recent activity yet/i),
    ).not.toBeInTheDocument();
    expect(screen.getByText(/could not be read/i)).toBeInTheDocument();
    expect(screen.getByText('queue.json is corrupt')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /retry/i }));
    expect(init).toHaveBeenCalledTimes(1);
  });

  it('NFR-033: a queue read failure links to the logs folder', async () => {
    const calls: string[] = [];
    mockIPC((cmd) => {
      calls.push(cmd);
      return null;
    });
    try {
      setQueue([], { status: 'error', error: 'queue.json is corrupt' });
      renderHome();
      fireEvent.click(
        screen.getByRole('button', { name: 'Open the logs folder' }),
      );
      await vi.waitFor(() => expect(calls).toContain('open_logs_folder'));
    } finally {
      clearMocks();
      delete (window as unknown as { __TAURI_INTERNALS__?: unknown })
        .__TAURI_INTERNALS__;
    }
  });

  it('NFR-033: an unavailable backend says so instead of showing an empty history', () => {
    setQueue([], { status: 'unavailable' });
    renderHome();
    expect(
      screen.queryByText(/no recent activity yet/i),
    ).not.toBeInTheDocument();
    expect(screen.getByText(/run Pluryprint to see them/i)).toBeInTheDocument();
  });

  it('distinguishes "still loading" from "nothing here"', () => {
    setQueue([], { status: 'loading' });
    renderHome();
    expect(screen.getByText(/loading recent activity/i)).toBeInTheDocument();
    expect(
      screen.queryByText(/no recent activity yet/i),
    ).not.toBeInTheDocument();
  });

  // -------------------------------------------------------------------------
  // Shortcuts surface (NFR-034)
  // -------------------------------------------------------------------------

  it('NFR-034: documents every shortcut the app implements, with its key cap', () => {
    renderHome();
    const section = screen.getByText(/keyboard shortcuts/i).parentElement!;
    const caps = within(section.parentElement!)
      .getAllByText(/^(Ctrl\+|⌘)/)
      .map((el) => el.textContent);
    for (const shortcut of APP_SHORTCUTS) {
      expect(caps).toContain(shortcutLabel(shortcut.id));
    }
  });

  it('NFR-034: every key cap on screen belongs to a real shortcut', () => {
    renderHome();
    const real = new Set(APP_SHORTCUTS.map((s) => shortcutLabel(s.id)));
    for (const cap of document.querySelectorAll('kbd')) {
      expect(real).toContain(cap.textContent);
    }
  });

  // -------------------------------------------------------------------------
  // Copy honesty
  // -------------------------------------------------------------------------

  it('does not claim the shell right-click integration already exists', () => {
    renderHome();
    const text = document.body.textContent ?? '';
    expect(text).not.toMatch(/adds itself to the right-click menu/i);
    // The CLI, which does exist, is what the tip points at (off macOS, the
    // console pluryprint-cli).
    expect(text).toMatch(/pluryprint-cli print --files/);
  });

  it('FR-006: without the executable path, the tip does not promise the card has it', () => {
    renderHome();
    const text = document.body.textContent ?? '';
    // The card falls back to the bare name and says so…
    expect(text).toMatch(/replace pluryprint-cli with its full path/i);
    // …so the tip must not claim the card copies the full path.
    expect(text).not.toMatch(/card copies a command that already does/);
    expect(text).toMatch(/full path, in the app's folder/);
  });

  it('FR-001: the tip names the right-click entry each platform really has', () => {
    renderHome();
    const text = document.body.textContent ?? '';
    // The installer registers the Explorer entry (classic menu).
    expect(text).not.toMatch(/not yet registered/i);
    expect(text).toMatch(
      /In Explorer, right-click files and choose Pluryprint/,
    );
    expect(text).toMatch(/Windows 11 the entry is under Show more options/);
    cleanup();

    const platform = vi
      .spyOn(window.navigator, 'platform', 'get')
      .mockReturnValue('MacIntel');
    try {
      renderHome();
      expect(document.body.textContent ?? '').toMatch(
        /Services › Print with Pluryprint/,
      );
    } finally {
      platform.mockRestore();
    }
  });
});
