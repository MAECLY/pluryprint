import {
  render,
  screen,
  fireEvent,
  waitFor,
  within,
} from '@testing-library/react';
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks';

import type { DependencyReport, Job, QueueSnapshot } from '@/ipc/bindings';
import {
  fixtureAppInfo,
  fixturePrinters,
  fixtureQueueSnapshot,
  mkJob,
} from '@/test/fixtures';
import { resetInstallStore } from '@/stores/installStore';
import { resetPrinterNames } from '@/stores/printerNamesStore';
import { Toaster } from '@/components/AppShell/Toaster';

// Swap the tauri-specta `events` object for a no-op (the queue store subscribes
// on init); real `commands` stay so mockIPC serves the queue commands.
const eventMock = vi.hoisted(() => {
  const noop = { listen: () => Promise.resolve(() => {}) };
  return {
    events: {
      queueUpdated: noop,
      jobProgressEvent: noop,
      dependencyInstallProgress: noop,
    },
  };
});

vi.mock('@/ipc/bindings', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/ipc/bindings')>();
  return { ...actual, events: eventMock.events };
});

import { QueueScreen, type QueueView } from './QueueScreen';
import { useQueueStore } from '@/stores/queueStore';

interface Call {
  cmd: string;
  args: Record<string, unknown>;
}

function mockQueue(snapshot: QueueSnapshot = fixtureQueueSnapshot): Call[] {
  const calls: Call[] = [];
  mockIPC((cmd, args) => {
    calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> });
    if (cmd === 'queue_status') return snapshot;
    if (cmd === 'clear_done') return 4;
    if (cmd === 'cancel_all') return 6;
    return null;
  });
  return calls;
}

/** A snapshot of `jobs`, running queue. */
function snapshotOf(jobs: Job[]): QueueSnapshot {
  return { schema_version: 1, queue_paused: false, jobs };
}

function resetStore() {
  // Cancels any undo window a previous test left open (and its timer).
  resetInstallStore();
  useQueueStore.getState().undoClearDone();
  useQueueStore.setState({
    status: 'idle',
    snapshot: null,
    subscribed: false,
    pendingClear: [],
    pendingIds: [],
    notifiedBatches: [],
    passwordPrompt: null,
    uiJobs: [],
    counts: { active: 0, pending: 0, done: 0 },
    queuePaused: false,
    recentDone: [],
  });
}

interface RenderOpts {
  snapshot?: QueueSnapshot;
  view?: QueueView;
  setView?: (v: QueueView) => void;
}

/** Init the store (as App would) then render the screen in its ready state. */
async function renderReady(opts: RenderOpts = {}): Promise<Call[]> {
  const calls = mockQueue(opts.snapshot);
  await useQueueStore.getState().init();
  render(
    <>
      <QueueScreen
        view={opts.view ?? 'list'}
        setView={opts.setView ?? (() => {})}
      />
      <Toaster theme="light" />
    </>,
  );
  return calls;
}

describe('QueueScreen', () => {
  beforeEach(() => resetStore());
  afterEach(() => clearMocks());

  it('FR-031: renders the job groups and count subtitle from the snapshot', async () => {
    await renderReady();

    expect(
      screen.getByText('3 active · 3 pending · 3 done'),
    ).toBeInTheDocument();
    expect(screen.getByText('Active · 3')).toBeInTheDocument();
    expect(screen.getByText('Pending · 3')).toBeInTheDocument();
    expect(screen.getByText('Recent · 3')).toBeInTheDocument();
    expect(screen.getByText('Q3-Financial-Report.pdf')).toBeInTheDocument();
    // The obscure `converted` state is spelled out for the user.
    expect(screen.getByText(/Ready to print/)).toBeInTheDocument();
    expect(
      screen.getByText(/Interrupted by shutdown · resume to continue/),
    ).toBeInTheDocument();
  });

  it('FR-031: names a job printer as Printers does, in every view', async () => {
    resetPrinterNames();
    mockIPC((cmd) => {
      if (cmd === 'queue_status')
        return snapshotOf([
          mkJob({
            id: 'p',
            state: 'failed',
            file_name: 'site-photo.jpg',
            profile_name: 'Quick',
            printer_resolved: 'EPSON_L15160',
            error: {
              code: 'print_aborted',
              message: 'the printer aborted the job — check the printer',
            },
          }),
        ]);
      if (cmd === 'list_printers') return fixturePrinters;
      return null;
    });
    await useQueueStore.getState().init();
    for (const view of ['list', 'timeline'] as const) {
      const { unmount } = render(
        <QueueScreen view={view} setView={() => {}} />,
      );
      // Same "profile · printer" order everywhere, with the display name.
      expect(
        await screen.findByText(/Quick · EPSON L15160 Series/),
      ).toBeInTheDocument();
      expect(screen.queryByText(/EPSON_L15160/)).toBeNull();
      // FR-092: the error is a sentence from the catalog, not the raw text.
      expect(
        screen.getByText(
          /The printer aborted the job\. Check the printer, then retry\./,
        ),
      ).toBeInTheDocument();
      unmount();
    }
    resetPrinterNames();
  });

  it('FR-124: a job Pluryfact sent says so on its row, in every view', async () => {
    resetPrinterNames();
    mockIPC((cmd) => {
      if (cmd === 'queue_status')
        return snapshotOf([
          mkJob({
            id: 'pf',
            state: 'queued',
            file_name: '0000-factura.xml',
            profile_name: 'Quick',
            origin: { source: 'pluryfact', label: 'Recibidos · octubre 2026' },
          }),
          mkJob({
            id: 'own',
            state: 'queued',
            file_name: 'mine.pdf',
            profile_name: 'Quick',
          }),
        ]);
      if (cmd === 'list_printers') return fixturePrinters;
      return null;
    });
    await useQueueStore.getState().init();
    for (const view of ['list', 'board', 'timeline'] as const) {
      const { unmount } = render(
        <QueueScreen view={view} setView={() => {}} />,
      );
      expect(
        await screen.findAllByText(/From Pluryfact · Recibidos · octubre 2026/),
      ).toHaveLength(1);
      unmount();
    }
    resetPrinterNames();
  });

  it('FR-033: toggles the whole queue: pause invokes pause_queue', async () => {
    const calls = await renderReady();

    fireEvent.click(screen.getByRole('button', { name: 'Pause queue' }));
    await waitFor(() =>
      expect(calls.some((c) => c.cmd === 'pause_queue')).toBe(true),
    );
  });

  it('FR-033: shows Resume queue and invokes resume_queue when paused', async () => {
    const calls = await renderReady({
      snapshot: { ...fixtureQueueSnapshot, queue_paused: true },
    });

    fireEvent.click(screen.getByRole('button', { name: 'Resume queue' }));
    await waitFor(() =>
      expect(calls.some((c) => c.cmd === 'resume_queue')).toBe(true),
    );
  });

  it('FR-036: clears finished jobs through an in-app confirm dialog', async () => {
    await renderReady();

    // Top-bar action opens the confirm modal (window.confirm is a WKWebview no-op).
    fireEvent.click(screen.getByRole('button', { name: 'Clear done' }));
    const dialog = await screen.findByRole('dialog');
    expect(
      within(dialog).getByText(/Clear 3 finished jobs\?/),
    ).toBeInTheDocument();
    expect(within(dialog).getByText(/5 seconds to undo/)).toBeInTheDocument();

    fireEvent.click(within(dialog).getByRole('button', { name: 'Clear done' }));

    // The jobs leave the screen immediately; the backend call is deferred.
    await waitFor(() =>
      expect(screen.queryByText('Recent · 3')).not.toBeInTheDocument(),
    );
    expect(
      await screen.findByText('Cleared 3 finished jobs'),
    ).toBeInTheDocument();
  });

  it('NFR-032: the clear toast offers a 5 s Undo that puts the jobs back', async () => {
    const calls = await renderReady();

    fireEvent.click(screen.getByRole('button', { name: 'Clear done' }));
    const dialog = await screen.findByRole('dialog');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Clear done' }));

    fireEvent.click(await screen.findByRole('button', { name: 'Undo' }));

    expect(await screen.findByText('Recent · 3')).toBeInTheDocument();
    expect(await screen.findByText('3 finished jobs kept')).toBeInTheDocument();
    expect(calls.some((c) => c.cmd === 'clear_done')).toBe(false);
  });

  it('FR-032: Cancel all confirms, then cancels every unfinished job', async () => {
    const calls = await renderReady();

    fireEvent.click(screen.getByRole('button', { name: 'Cancel all' }));
    const dialog = await screen.findByRole('dialog');
    // 3 active + 3 pending in the fixture; finished ones are not counted.
    expect(
      within(dialog).getByText(/6 jobs that have not finished/),
    ).toBeInTheDocument();
    expect(calls.some((c) => c.cmd === 'cancel_all')).toBe(false);

    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel all' }));
    await waitFor(() =>
      expect(calls.some((c) => c.cmd === 'cancel_all')).toBe(true),
    );
    expect(await screen.findByText('Cancelled 6 jobs')).toBeInTheDocument();
  });

  it('FR-032: Cancel all is disabled when nothing is left to cancel', async () => {
    await renderReady({
      snapshot: snapshotOf([
        mkJob({ id: 'a', state: 'done', file_name: 'a.pdf' }),
      ]),
    });
    expect(screen.getByRole('button', { name: 'Cancel all' })).toBeDisabled();
  });

  it('FR-036: Clear done is disabled when there is no finished job', async () => {
    await renderReady({
      snapshot: snapshotOf([
        mkJob({ id: 'a', state: 'printing', file_name: 'a.pdf' }),
      ]),
    });
    expect(screen.getByRole('button', { name: 'Clear done' })).toBeDisabled();
  });

  it('FR-030: shows an unavailable state outside the Tauri host', () => {
    useQueueStore.setState({ status: 'unavailable' });
    render(<QueueScreen view="list" setView={() => {}} />);
    expect(screen.getByText('Queue unavailable')).toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: 'Retry' }),
    ).not.toBeInTheDocument();
  });

  it('NFR-033: the error state explains itself and retries the read', async () => {
    const calls = mockQueue();
    useQueueStore.setState({ status: 'error', error: 'queue.json is locked' });
    render(<QueueScreen view="list" setView={() => {}} />);

    expect(screen.getByText(/queue.json is locked/)).toBeInTheDocument();
    expect(screen.getByText(/Nothing was lost/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    await waitFor(() =>
      expect(calls.some((c) => c.cmd === 'queue_status')).toBe(true),
    );
    await waitFor(() => expect(useQueueStore.getState().status).toBe('ready'));
  });

  it('FR-030: shows the loading state while the first snapshot is in flight', () => {
    useQueueStore.setState({ status: 'loading' });
    render(<QueueScreen view="list" setView={() => {}} />);
    expect(screen.getByText('Loading the queue…')).toBeInTheDocument();
  });

  it('FR-031: shows an empty state when the queue has no job', async () => {
    await renderReady({ snapshot: snapshotOf([]) });
    expect(screen.getByText('No jobs in the queue')).toBeInTheDocument();
  });
});

describe('QueueScreen — row actions', () => {
  beforeEach(() => resetStore());
  afterEach(() => clearMocks());

  it('FR-032: Pause, Resume and Retry invoke their per-job command', async () => {
    const calls = await renderReady();

    fireEvent.click(
      screen.getAllByRole('button', { name: /^Pause (?!queue)/ })[0]!,
    );
    fireEvent.click(
      screen.getAllByRole('button', { name: /^Resume (?!queue)/ })[0]!,
    );
    fireEvent.click(screen.getByRole('button', { name: /^Retry / }));

    await waitFor(() =>
      expect(calls.filter((c) => c.cmd.endsWith('_job')).length).toBe(3),
    );
    // The first Pause button belongs to the first pausable row (`converting`).
    expect(calls.find((c) => c.cmd === 'pause_job')?.args).toEqual({
      id: 'j-converting',
    });
    expect(calls.find((c) => c.cmd === 'resume_job')?.args).toEqual({
      id: 'j-paused',
    });
    expect(calls.find((c) => c.cmd === 'retry_job')?.args).toEqual({
      id: 'j-failed',
    });
  });

  it('FR-040: a job paused because the printer cannot duplex can go one-sided, then resumes', async () => {
    const calls = mockQueue(
      snapshotOf([
        mkJob({
          id: 'j-duplex',
          state: 'paused',
          file_name: 'report.pdf',
          error: {
            code: 'unsupported_setting',
            reason: 'duplex',
            params: { printer: 'Front' },
            message: 'x',
          },
        }),
      ]),
    );
    await useQueueStore.getState().init();
    render(<QueueScreen view="list" setView={() => {}} />);

    fireEvent.click(
      await screen.findByRole('button', { name: 'Print report.pdf one-sided' }),
    );
    await waitFor(() =>
      expect(calls.some((c) => c.cmd === 'resume_job')).toBe(true),
    );
    const adjust = calls.findIndex((c) => c.cmd === 'adjust_job');
    const resume = calls.findIndex((c) => c.cmd === 'resume_job');
    expect(calls[adjust]?.args).toEqual({
      id: 'j-duplex',
      change: { printer: null, duplex: 'none' },
    });
    expect(adjust).toBeLessThan(resume);
  });

  it('FR-044: a job paused on a removed printer can be sent to another one', async () => {
    const calls: Call[] = [];
    mockIPC((cmd, args) => {
      calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> });
      if (cmd === 'queue_status')
        return snapshotOf([
          mkJob({
            id: 'j-removed',
            state: 'paused',
            file_name: 'invoice.pdf',
            error: {
              code: 'printer_disabled',
              params: { printer: 'HL_L2350' },
              message: 'x',
            },
          }),
        ]);
      if (cmd === 'list_printers') return fixturePrinters;
      if (cmd === 'get_printer_prefs') return { disabled: ['HL_L2350'] };
      return null;
    });
    await useQueueStore.getState().init();
    render(<QueueScreen view="list" setView={() => {}} />);

    const select = await screen.findByRole('combobox', {
      name: 'Send invoice.pdf to another printer',
    });
    // Only printers that can be used are offered.
    await within(select).findByText('EPSON L15160 Series');
    expect(within(select).queryByText('Brother HL-L2350DW')).toBeNull();
    // Not a one-sided problem: no such button.
    expect(screen.queryByRole('button', { name: /one-sided/ })).toBeNull();

    fireEvent.change(select, { target: { value: 'EPSON_L15160' } });
    await waitFor(() =>
      expect(calls.some((c) => c.cmd === 'resume_job')).toBe(true),
    );
    expect(calls.find((c) => c.cmd === 'adjust_job')?.args).toEqual({
      id: 'j-removed',
      change: { printer: 'EPSON_L15160', duplex: null },
    });
  });

  it('FR-032: only the actions legal for a state are offered', async () => {
    await renderReady();

    // `printing` cannot be paused (the OS owns it) and terminal rows offer
    // nothing but Retry on a failure.
    expect(
      screen.getAllByRole('button', { name: /^Pause (?!queue)/ }),
    ).toHaveLength(3);
    expect(
      screen.getAllByRole('button', { name: /^Resume (?!queue)/ }),
    ).toHaveLength(2);
    expect(screen.getAllByRole('button', { name: /^Retry / })).toHaveLength(1);
    // Row actions are named by file; the toolbar's "Cancel all" is not one.
    expect(
      screen.getAllByRole('button', { name: /^Cancel (?!all$)/ }),
    ).toHaveLength(6);
  });

  it('NFR-044: each row action names the job it acts on', async () => {
    await renderReady({
      snapshot: snapshotOf([
        mkJob({ id: 'a', state: 'queued', file_name: 'a.pdf' }),
        mkJob({ id: 'b', state: 'queued', file_name: 'b.pdf' }),
      ]),
    });
    expect(
      screen.getByRole('button', { name: 'Cancel a.pdf' }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: 'Cancel b.pdf' }),
    ).toBeInTheDocument();
  });

  it('FR-032: cancelling a non-printing job goes straight to cancel_job', async () => {
    const calls = await renderReady({
      snapshot: snapshotOf([
        mkJob({ id: 'q', state: 'queued', file_name: 'q.pdf' }),
      ]),
    });

    fireEvent.click(screen.getByRole('button', { name: 'Cancel q.pdf' }));

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    await waitFor(() =>
      expect(calls.find((c) => c.cmd === 'cancel_job')?.args).toEqual({
        id: 'q',
      }),
    );
  });

  it('FR-032: cancelling a printing job confirms first', async () => {
    const calls = await renderReady({
      snapshot: snapshotOf([
        mkJob({ id: 'p', state: 'printing', file_name: 'invoice.pdf' }),
      ]),
    });

    fireEvent.click(screen.getByRole('button', { name: 'Cancel invoice.pdf' }));
    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByText(/invoice.pdf/)).toBeInTheDocument();
    expect(calls.some((c) => c.cmd === 'cancel_job')).toBe(false);

    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel job' }));
    await waitFor(() =>
      expect(calls.find((c) => c.cmd === 'cancel_job')?.args).toEqual({
        id: 'p',
      }),
    );
  });

  it('FR-032: dismissing that confirmation leaves the job alone', async () => {
    const calls = await renderReady({
      snapshot: snapshotOf([
        mkJob({ id: 'p', state: 'printing', file_name: 'invoice.pdf' }),
      ]),
    });

    fireEvent.click(screen.getByRole('button', { name: 'Cancel invoice.pdf' }));
    const dialog = await screen.findByRole('dialog');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));

    await waitFor(() =>
      expect(screen.queryByRole('dialog')).not.toBeInTheDocument(),
    );
    expect(calls.some((c) => c.cmd === 'cancel_job')).toBe(false);
  });
});

describe('QueueScreen — progress honesty', () => {
  beforeEach(() => resetStore());
  afterEach(() => clearMocks());

  it('FR-031: a printing job with no reported fraction renders indeterminate', async () => {
    await renderReady({
      snapshot: snapshotOf([
        mkJob({
          id: 'p',
          state: 'printing',
          file_name: 'spooling.pdf',
          progress: { phase: 'printing', percent: null },
        }),
      ]),
    });

    const bar = screen.getByRole('progressbar', {
      name: 'Progress of spooling.pdf',
    });
    // ARIA spells "in progress, amount unknown" as a missing aria-valuenow.
    expect(bar).not.toHaveAttribute('aria-valuenow');
    expect(bar).toHaveAttribute(
      'aria-valuetext',
      'In progress — the print spooler reports no percentage',
    );
  });

  it('FR-031: a measured conversion still renders its percentage', async () => {
    await renderReady({
      snapshot: snapshotOf([
        mkJob({
          id: 'c',
          state: 'converting',
          file_name: 'deck.pptx',
          progress: { phase: 'converting', percent: 0.34 },
        }),
      ]),
    });

    const bar = screen.getByRole('progressbar', {
      name: 'Progress of deck.pptx',
    });
    expect(bar).toHaveAttribute('aria-valuenow', '34');
  });
});

describe('QueueScreen — alternate views', () => {
  beforeEach(() => resetStore());
  afterEach(() => clearMocks());

  it('FR-031: the view switcher reports the chosen view', async () => {
    const setView = vi.fn();
    await renderReady({ setView });

    fireEvent.click(screen.getByRole('radio', { name: 'Board' }));
    expect(setView).toHaveBeenCalledWith('board');
    fireEvent.click(screen.getByRole('radio', { name: 'Timeline' }));
    expect(setView).toHaveBeenCalledWith('timeline');
  });

  it('FR-031: the Board view lays the jobs out by phase', async () => {
    await renderReady({ view: 'board' });

    expect(
      screen.getByText('Board view — kanban across job phases'),
    ).toBeInTheDocument();
    for (const col of ['Queued', 'Converting', 'Printing', 'Done']) {
      expect(screen.getAllByText(col).length).toBeGreaterThan(0);
    }
    expect(screen.getByText('Q3-Financial-Report.pdf')).toBeInTheDocument();
    // FR-031 / WCAG 1.4.1: mixed columns say each card's state in words, so a
    // failed job is told apart from a printed one without relying on colour.
    expect(screen.getAllByText('Failed').length).toBeGreaterThan(0);
    expect(screen.getAllByText('Paused').length).toBeGreaterThan(0);
    // Board and Timeline are read-only: no per-row controls, and the whole-queue
    // controls belong to the List view.
    expect(
      screen.queryByRole('button', { name: 'Pause queue' }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: /^Cancel / }),
    ).not.toBeInTheDocument();
  });

  it('FR-031: the Timeline view shows each job in order with its duration', async () => {
    await renderReady({
      view: 'timeline',
      snapshot: snapshotOf([
        mkJob({
          id: 'fast',
          state: 'done',
          file_name: 'memo.pdf',
          started_at: '2026-07-11T10:00:00Z',
          finished_at: '2026-07-11T10:00:07Z',
        }),
        mkJob({
          id: 'slow',
          state: 'done',
          file_name: 'atlas.pdf',
          started_at: '2026-07-11T10:00:00Z',
          finished_at: '2026-07-11T10:03:12Z',
        }),
        mkJob({
          id: 'running',
          state: 'printing',
          file_name: 'now.pdf',
          started_at: '2026-07-11T10:04:00Z',
        }),
      ]),
    });

    expect(
      screen.getByText('Timeline — chronological, with durations'),
    ).toBeInTheDocument();
    expect(screen.getByText(/took 7s/)).toBeInTheDocument();
    expect(screen.getByText(/took 3m 12s/)).toBeInTheDocument();
    // An unfinished job has no duration to show — and none is invented.
    expect(screen.queryByText(/took 0s/)).not.toBeInTheDocument();
    expect(screen.getByText('now.pdf')).toBeInTheDocument();
  });

  const removedPrinterJob = () =>
    snapshotOf([
      mkJob({
        id: 'j-removed',
        state: 'paused',
        file_name: 'invoice.pdf',
        error: {
          code: 'printer_unavailable',
          params: { printer: 'HL_L2350' },
          message: 'x',
        },
      }),
    ]);

  it('FR-044: a printer list that fails to load says so and can be retried', async () => {
    let fail = true;
    const calls: Call[] = [];
    mockIPC((cmd, args) => {
      calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> });
      if (cmd === 'queue_status') return removedPrinterJob();
      if (cmd === 'list_printers') {
        if (fail) throw new Error('spooler down');
        return fixturePrinters;
      }
      if (cmd === 'get_printer_prefs') return { disabled: [] };
      return null;
    });
    await useQueueStore.getState().init();
    render(<QueueScreen view="list" setView={() => {}} />);

    expect(
      await screen.findByText(
        'The printers to send it to could not be loaded.',
      ),
    ).toBeInTheDocument();
    expect(
      screen.queryByRole('combobox', {
        name: 'Send invoice.pdf to another printer',
      }),
    ).toBeNull();

    fail = false;
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    const select = await screen.findByRole('combobox', {
      name: 'Send invoice.pdf to another printer',
    });
    await within(select).findByText('EPSON L15160 Series');
    expect(
      screen.queryByText('The printers to send it to could not be loaded.'),
    ).toBeNull();
  });

  it('FR-044: the printer list refreshes when the window regains focus', async () => {
    let printers = fixturePrinters.filter((p) => p.id !== 'EPSON_L15160');
    mockIPC((cmd) => {
      if (cmd === 'queue_status') return removedPrinterJob();
      if (cmd === 'list_printers') return printers;
      if (cmd === 'get_printer_prefs') return { disabled: [] };
      return null;
    });
    const now = vi.spyOn(Date, 'now').mockReturnValue(1_000_000);
    try {
      await useQueueStore.getState().init();
      render(<QueueScreen view="list" setView={() => {}} />);
      const select = await screen.findByRole('combobox', {
        name: 'Send invoice.pdf to another printer',
      });
      expect(within(select).queryByText('EPSON L15160 Series')).toBeNull();

      // A printer is added while the user is elsewhere, then they come back.
      printers = fixturePrinters;
      now.mockReturnValue(1_000_000 + 60_000);
      fireEvent.focus(window);
      await within(select).findByText('EPSON L15160 Series');
    } finally {
      now.mockRestore();
    }
  });

  const office = (found: boolean): DependencyReport => ({
    id: 'libre-office',
    name: 'LibreOffice',
    applicable: true,
    found,
    path: null,
    found_in: null,
    blocks: found ? [] : ['docx'],
    remediation: found ? null : 'LibreOffice is missing — …',
    install: [
      {
        method: 'winget',
        command: 'winget install --id TheDocumentFoundation.LibreOffice',
        needs_admin: true,
        paid: false,
        approx_mb: 350,
        available: true,
      },
    ],
  });

  const missingOffice = () =>
    snapshotOf([
      mkJob({
        id: 'j-docx',
        state: 'paused',
        file_name: 'letter.docx',
        error: {
          code: 'dependency_missing',
          reason: 'libre-office',
          message: 'LibreOffice is missing',
        },
      }),
    ]);

  it.each(['list', 'board', 'timeline'] as const)(
    'FR-058: a job paused for a missing converter offers to install it (%s view), then resumes',
    async (view) => {
      const calls: Call[] = [];
      mockIPC((cmd, args) => {
        calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> });
        if (cmd === 'queue_status') return missingOffice();
        if (cmd === 'app_info')
          return { ...fixtureAppInfo, dependencies: [office(false)] };
        if (cmd === 'install_dependency')
          return {
            status: 'installed',
            exit_code: 0,
            elevated: false,
            report: office(true),
          };
        return null;
      });
      await useQueueStore.getState().init();
      render(<QueueScreen view={view} setView={() => {}} />);

      fireEvent.click(
        screen.getByRole('button', {
          name: 'Install what is missing for letter.docx',
        }),
      );
      const dialog = await screen.findByRole('dialog', {
        name: 'Install LibreOffice',
      });
      // Nothing installs until the user agrees in the dialog.
      expect(calls.some((c) => c.cmd === 'install_dependency')).toBe(false);
      fireEvent.click(within(dialog).getByRole('button', { name: 'Install' }));
      await waitFor(() =>
        expect(calls.find((c) => c.cmd === 'resume_job')?.args).toEqual({
          id: 'j-docx',
        }),
      );
      expect(calls.find((c) => c.cmd === 'install_dependency')?.args).toEqual({
        id: 'libre-office',
        method: 'winget',
        consent: true,
      });
    },
  );

  it('FR-058: if the converter is there by now, the job just resumes', async () => {
    const calls: Call[] = [];
    mockIPC((cmd, args) => {
      calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> });
      if (cmd === 'queue_status') return missingOffice();
      if (cmd === 'app_info')
        return { ...fixtureAppInfo, dependencies: [office(true)] };
      return null;
    });
    await useQueueStore.getState().init();
    render(<QueueScreen view="list" setView={() => {}} />);
    fireEvent.click(
      screen.getByRole('button', {
        name: 'Install what is missing for letter.docx',
      }),
    );
    await waitFor(() =>
      expect(calls.some((c) => c.cmd === 'resume_job')).toBe(true),
    );
    expect(screen.queryByRole('dialog')).toBeNull();
  });
  it('FR-058: after an install page opened, coming back to the window checks again and resumes', async () => {
    const calls: Call[] = [];
    let installed = false;
    mockIPC((cmd, args) => {
      calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> });
      if (cmd === 'queue_status') return missingOffice();
      if (cmd === 'app_info')
        return { ...fixtureAppInfo, dependencies: [office(installed)] };
      if (cmd === 'install_dependency')
        return {
          status: 'opened',
          exit_code: null,
          elevated: false,
          report: office(false),
        };
      return null;
    });
    await useQueueStore.getState().init();
    render(<QueueScreen view="list" setView={() => {}} />);
    fireEvent.click(
      screen.getByRole('button', {
        name: 'Install what is missing for letter.docx',
      }),
    );
    const dialog = await screen.findByRole('dialog', {
      name: 'Install LibreOffice',
    });
    fireEvent.click(within(dialog).getByRole('button', { name: 'Install' }));
    // The page opened: the promise names what happens on return.
    expect(
      await within(dialog).findByText(
        /checks again when you come back to it and resumes the jobs waiting for LibreOffice/,
      ),
    ).toBeInTheDocument();
    expect(calls.some((c) => c.cmd === 'resume_job')).toBe(false);

    // Coming back while it is still missing changes nothing.
    const probes = () => calls.filter((c) => c.cmd === 'app_info').length;
    const before = probes();
    fireEvent.focus(window);
    await waitFor(() => expect(probes()).toBeGreaterThan(before));
    expect(calls.some((c) => c.cmd === 'resume_job')).toBe(false);

    // The user finished in the Store and comes back: the job resumes.
    installed = true;
    Object.defineProperty(document, 'visibilityState', {
      configurable: true,
      get: () => 'visible',
    });
    fireEvent(document, new Event('visibilitychange'));
    await waitFor(() =>
      expect(calls.find((c) => c.cmd === 'resume_job')?.args).toEqual({
        id: 'j-docx',
      }),
    );
  });

  it('FR-058: with nothing waiting on a dependency, focus does not probe dependencies', async () => {
    const calls = await renderReady();
    fireEvent.focus(window);
    await new Promise((r) => setTimeout(r, 0));
    expect(calls.some((c) => c.cmd === 'app_info')).toBe(false);
  });
});
