/**
 * The browser E2E harness is only useful if the app cannot tell it from the
 * real host. These tests drive it through the *generated bindings* and the
 * real `@tauri-apps/api` modules — the exact path the app takes.
 */

import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { commands, events, type Job, type Profile } from '@/ipc/bindings';
import { isTauri } from '@/lib/ipc';
import { fixturePrintersWithVirtual, fixtureProfiles } from '@/test/fixtures';

import { installIpcStub, optionsFromUrl, type E2eHarness } from './ipc-stub';
import { E2E_STUB_SRC, pluryE2eStub } from './vite-plugin';

let h: E2eHarness;

beforeEach(() => {
  h = installIpcStub(window, { tickMs: 0 });
});

afterEach(() => {
  h.backend.dispose();
  const w = window as unknown as Record<string, unknown>;
  delete w.__TAURI_INTERNALS__;
  delete w.__TAURI_EVENT_PLUGIN_INTERNALS__;
  delete w.__PLURY_E2E__;
});

function ok<T>(
  r: { status: 'ok'; data: T } | { status: 'error'; error: unknown },
): T {
  if (r.status !== 'ok')
    throw new Error(`expected ok, got ${JSON.stringify(r.error)}`);
  return r.data;
}

describe('installIpcStub', () => {
  it('makes the app believe it runs inside Tauri', () => {
    expect(isTauri()).toBe(true);
    expect(window.__PLURY_E2E__).toBe(h);
  });

  it('answers commands through the generated bindings with fixture data', async () => {
    const printers = ok(await commands.listPrinters());
    expect(printers.map((p) => p.id)).toEqual(
      fixturePrintersWithVirtual.map((p) => p.id),
    );
    expect(ok(await commands.defaultPrinter())).toBe('EPSON_L15160');
    expect(ok(await commands.listProfiles())).toHaveLength(
      fixtureProfiles.length,
    );
  });

  it('rejects with a typed IpcError, like the host', async () => {
    const r = await commands.cancelJob('nope');
    expect(r).toEqual({
      status: 'error',
      error: { code: 'JobNotFound', message: 'no job with id nope' },
    });
  });

  it('refuses commands it does not model instead of resolving undefined', async () => {
    const { invoke } = await import('@tauri-apps/api/core');
    await expect(invoke('definitely_not_a_command')).rejects.toMatchObject({
      code: 'Internal',
    });
  });

  it('FR-031: a submitted batch walks to done, emitting queue-updated and progress', async () => {
    const snapshots: Job[][] = [];
    const progress: number[] = [];
    const offQueue = await events.queueUpdated.listen((e) =>
      snapshots.push(e.payload.jobs),
    );
    const offProgress = await events.jobProgressEvent.listen((e) =>
      progress.push(e.payload.percent),
    );

    const batch = ok(
      await commands.printFiles({
        files: ['/Users/e2e/a.pdf', '/Users/e2e/b-fail.pdf'],
        settings: {
          printer: null,
          duplex: 'none',
          color_mode: 'color',
          copies: 1,
          paper_size: 'a4',
          orientation: 'auto',
          scaling: 'fit',
          page_ranges: null,
        },
        profile_name: 'Quick',
      }),
    );
    expect(batch.job_ids).toHaveLength(2);
    expect(snapshots.at(-1)?.map((j) => j.state)).toEqual(['queued', 'queued']);

    for (let i = 0; i < 30; i += 1) h.backend.tick();

    const states = Object.fromEntries(
      ok(await commands.queueStatus()).jobs.map((j) => [j.file_name, j.state]),
    );
    expect(states).toEqual({ 'a.pdf': 'done', 'b-fail.pdf': 'failed' });
    expect(progress.length).toBeGreaterThan(0);
    expect(snapshots.flat().some((j) => j.state === 'printing')).toBe(true);

    offQueue();
    offProgress();
    const seen = snapshots.length;
    await commands.pauseQueue();
    expect(snapshots).toHaveLength(seen);
  });

  it('FR-042: print_files validates copies and files like the backend', async () => {
    const r = await commands.printFiles({
      // .docx prints since B's converters; an executable never does.
      files: ['/Users/e2e/x.exe'],
      settings: {
        printer: null,
        duplex: 'none',
        color_mode: 'color',
        copies: 1,
        paper_size: 'a4',
        orientation: 'auto',
        scaling: 'fit',
        page_ranges: null,
      },
      profile_name: null,
    });
    expect(r.status).toBe('error');
  });

  it('FR-033: a paused queue does not advance', async () => {
    await commands.printFiles({
      files: ['/Users/e2e/a.pdf'],
      settings: {
        printer: null,
        duplex: 'none',
        color_mode: 'color',
        copies: 1,
        paper_size: 'a4',
        orientation: 'auto',
        scaling: 'fit',
        page_ranges: null,
      },
      profile_name: null,
    });
    await commands.pauseQueue();
    for (let i = 0; i < 10; i += 1) h.backend.tick();
    expect(ok(await commands.queueStatus()).jobs[0]?.state).toBe('queued');
  });

  it('FR-020: profiles CRUD round-trips and guards the default', async () => {
    const draft: Profile = {
      ...(fixtureProfiles[3] as Profile),
      id: '',
      name: 'E2E profile',
    };
    const saved = ok(await commands.saveProfile(draft));
    expect(saved.id).not.toBe('');
    ok(await commands.setDefaultProfile(saved.id));
    const list = ok(await commands.listProfiles());
    expect(list.find((p) => p.default)?.id).toBe(saved.id);
    expect((await commands.deleteProfile(saved.id)).status).toBe('error');
    ok(await commands.setDefaultProfile('builtin-quick'));
    ok(await commands.deleteProfile(saved.id));
    expect(
      ok(await commands.listProfiles()).some((p) => p.id === saved.id),
    ).toBe(false);
  });

  it('FR-100: settings persist in memory and out-of-range values are refused', async () => {
    const s = ok(await commands.getAppSettings());
    expect(s.meta?.limits.max_file_size_mb.max).toBe(4096);
    const next = { ...s, app: { ...s.app, theme: 'dark' as const } };
    ok(await commands.setAppSettings(next));
    expect(ok(await commands.getAppSettings()).app.theme).toBe('dark');
    const bad = {
      ...s,
      engine: { ...s.engine, max_concurrent_conversions: 99 },
    };
    expect((await commands.setAppSettings(bad)).status).toBe('error');
  });

  it('FR-100: saving settings keeps the default profile, like the backend', async () => {
    // A settings form holding a stale copy must not revert a default chosen
    // since through `set_default_profile`: the real backend ignores the
    // incoming value, so the fake has to as well.
    const stale = ok(await commands.getAppSettings());
    const other = ok(await commands.listProfiles())[1];
    if (!other) throw new Error('the fixtures have a second profile');
    ok(await commands.setDefaultProfile(other.id));
    ok(
      await commands.setAppSettings({
        ...stale,
        engine: { ...stale.engine, default_profile: 'builtin-quick' },
      }),
    );
    expect(ok(await commands.getAppSettings()).engine.default_profile).toBe(
      other.id,
    );
  });

  it('FR-004: folders are reported as such and expand per scan depth', async () => {
    const [info] = ok(
      await commands.inspectFiles(['/Users/e2e/Documents/Invoices']),
    );
    expect(info).toMatchObject({ is_dir: true, has_subfolders: true });
    const [top] = ok(
      await commands.expandFolders(
        ['/Users/e2e/Documents/Invoices'],
        'top_level',
      ),
    );
    const [all] = ok(
      await commands.expandFolders(
        ['/Users/e2e/Documents/Invoices'],
        'recursive',
      ),
    );
    expect(top?.files).toHaveLength(2);
    expect(all?.files).toHaveLength(3);
  });

  it('serves the dialog plugin with canned paths', async () => {
    const { open, save } = await import('@tauri-apps/plugin-dialog');
    h.backend.openPaths = ['/Users/e2e/one.pdf', '/Users/e2e/two.png'];
    expect(await open({ multiple: true })).toEqual([
      '/Users/e2e/one.pdf',
      '/Users/e2e/two.png',
    ]);
    expect(await save({})).toBe(h.backend.savePath);
  });

  it('FR-001: a forwarded selection and a native drop reach webview listeners', async () => {
    const got: unknown[] = [];
    await events.selectionForwarded.listen((e) => got.push(e.payload));
    const { getCurrentWebview } = await import('@tauri-apps/api/webview');
    await getCurrentWebview().onDragDropEvent((e) => got.push(e.payload.type));

    h.forwardSelection({
      paths: ['/x.pdf'],
      action: 'print-default',
      profile_id: null,
    });
    h.dropFiles(['/y.pdf']);
    expect(got).toEqual([
      { paths: ['/x.pdf'], action: 'print-default', profile_id: null },
      'drop',
    ]);
  });

  it('FR-124: a Pluryfact selection carries its source, label and overrides to the app', async () => {
    const got: unknown[] = [];
    await events.selectionForwarded.listen((e) => got.push(e.payload));
    const sel = {
      paths: [
        '/Users/e2e/Library/Application Support/com.Maecly.Pluryprint/inbox/j/0000-RIDE.pdf',
      ],
      action: 'configure' as const,
      profile_id: null,
      source: 'pluryfact',
      label: 'Recibidos · octubre 2026',
      overrides: { copies: 2, color: 'monochrome' as const },
    };
    h.forwardSelection(sel);
    expect(got).toEqual([sel]);
  });

  it('FR-127: the bridge status and Reconnect are served', async () => {
    const before = await commands.bridgeStatus();
    expect(before.status).toBe('ok');
    if (before.status !== 'ok') return;
    expect(before.data.browsers.map((b) => b.registered)).toEqual([
      true,
      false,
      false,
      false,
    ]);
    const after = await commands.bridgeReconnect();
    if (after.status !== 'ok') throw new Error('reconnect failed');
    // Every browser found is registered; the absent one stays absent.
    expect(after.data.browsers.map((b) => b.registered)).toEqual([
      true,
      true,
      true,
      false,
    ]);
  });

  it('records every command the app invoked', async () => {
    await commands.appInfo();
    await commands.queueStatus();
    expect(h.calls().map((c) => c.cmd)).toEqual(['app_info', 'queue_status']);
  });
});

describe('optionsFromUrl', () => {
  it('reads the ?e2e-* flags', () => {
    expect(optionsFromUrl('?e2e-seed=1&e2e-tick=0')).toEqual({
      seedQueue: true,
      tickMs: 0,
    });
    expect(optionsFromUrl('')).toEqual({});
  });
});

describe('pluryE2eStub (vite plugin)', () => {
  const inject = (env: Record<string, string | undefined>) => {
    const plugin = pluryE2eStub(env);
    const hook = plugin.transformIndexHtml as () => unknown;
    return { plugin, tags: hook() };
  };

  it('never applies to production builds', () => {
    expect(inject({ PLURY_E2E_STUB: '1' }).plugin.apply).toBe('serve');
  });

  it('is switched on by the e2e mode (pnpm dev:e2e) as well', () => {
    const plugin = pluryE2eStub({});
    (plugin.configResolved as (c: { mode: string }) => void)({ mode: 'e2e' });
    expect((plugin.transformIndexHtml as () => unknown[])()).toHaveLength(1);
    const dev = pluryE2eStub({});
    (dev.configResolved as (c: { mode: string }) => void)({
      mode: 'development',
    });
    expect((dev.transformIndexHtml as () => unknown[])()).toEqual([]);
  });

  it('injects the stub ahead of the app only when PLURY_E2E_STUB=1', () => {
    expect(inject({}).tags).toEqual([]);
    expect(inject({ PLURY_E2E_STUB: '0' }).tags).toEqual([]);
    expect(inject({ PLURY_E2E_STUB: '1' }).tags).toEqual([
      {
        tag: 'script',
        attrs: { type: 'module', src: E2E_STUB_SRC },
        injectTo: 'head-prepend',
      },
    ]);
  });
});
