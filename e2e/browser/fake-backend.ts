/**
 * In-memory stand-in for the Rust backend, for the macOS browser E2E harness.
 *
 * `tauri-driver` cannot drive WKWebView, so on macOS the app is exercised in a
 * plain browser instead: `ipc-stub.ts` installs `window.__TAURI_INTERNALS__`
 * and routes every `invoke` here. The state starts from the same wire-shaped
 * fixtures the Vitest suite uses (`src/test/fixtures.ts`), and the queue
 * really moves: submitted jobs walk queued → converting → converted →
 * printing → done on a timer, emitting `job-progress-event` and
 * `queue-updated` exactly like the dispatcher.
 *
 * It mirrors the *contract* (command names, argument names, wire shapes,
 * `IpcError` rejections), not the implementation. Anything the UI depends on
 * that is not modelled here rejects with `Internal: not stubbed`, loudly,
 * rather than resolving `undefined`.
 */

import type {
  AppInfo,
  AppSettings,
  BridgeOverview,
  DiscoveredPrinter,
  FileInfo,
  FolderExpansion,
  FolderScan,
  IpcError,
  Job,
  JobBatch,
  JobState,
  Printer,
  PrinterPolicy,
  PrinterPrefs,
  PrintRequest,
  Profile,
  QueueSnapshot,
  Selection,
  SettingsMeta,
  UpdateCheck,
  UpdateState,
} from '@/ipc/bindings';
import {
  fixtureAppInfo,
  fixtureAppSettings,
  fixtureDiscovered,
  fixturePrintersWithVirtual,
  fixtureProfiles,
  fixtureQueueSnapshot,
  fixtureUpdateCheckUpToDate,
} from '@/test/fixtures';
import { PRINTABLE_EXTENSIONS } from '@/lib/formats';

/** Emits a backend event to every frontend listener of `event`. */
export type Emit = (event: string, payload: unknown) => void;

export interface FakeBackendOptions {
  /** Milliseconds between queue ticks; `0` disables the timer (tests tick by hand). */
  tickMs?: number;
  /** Start from the fixture queue (every lifecycle state) instead of an empty one. */
  seedQueue?: boolean;
  /** Paths the open dialog returns (`plugin:dialog|open`). */
  openPaths?: string[];
  /** Path the save dialog returns (`plugin:dialog|save`), `null` = cancelled. */
  savePath?: string | null;
}

/** Folders the fake filesystem knows; anything else with no extension is a file. */
const FAKE_FOLDERS: Record<string, { top: string[]; nested: string[] }> = {
  '/Users/e2e/Documents/Invoices': {
    top: [
      '/Users/e2e/Documents/Invoices/inv-001.pdf',
      '/Users/e2e/Documents/Invoices/inv-002.pdf',
    ],
    nested: ['/Users/e2e/Documents/Invoices/2025/inv-900.pdf'],
  },
  '/Users/e2e/Documents/Flat': {
    top: ['/Users/e2e/Documents/Flat/a.pdf', '/Users/e2e/Documents/Flat/b.png'],
    nested: [],
  },
};

const DEFAULT_OPEN_PATHS = [
  '/Users/e2e/Documents/Q3-Report.pdf',
  '/Users/e2e/Pictures/site-photo.jpg',
];

// The engine's own list, so the fake accepts exactly what the app offers.
const SUPPORTED = new Set<string>(PRINTABLE_EXTENSIONS);
// One-page inputs (pictures and drawings, as the engine's `is_single_page`).
const IMAGE = new Set([
  'png',
  'jpg',
  'jpeg',
  'tif',
  'tiff',
  'bmp',
  'webp',
  'svg',
  'svgz',
]);
const MEDIA = { a4: 'A4', letter: 'Letter', legal: 'Legal', a3: 'A3' } as const;
const TERMINAL: JobState[] = ['done', 'unconfirmed', 'failed', 'cancelled'];

const clone = <T>(v: T): T => structuredClone(v);

function err(code: IpcError['code'], message: string): IpcError {
  return { code, message } as IpcError;
}

function basename(p: string): string {
  return p.split(/[\\/]/).pop() ?? p;
}

function extOf(p: string): string {
  const name = basename(p);
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
}

/** Fake page count: images are one page; PDFs take it from a `-<n>p` suffix, else 3. */
function pagesOf(p: string): number | null {
  const ext = extOf(p);
  if (IMAGE.has(ext)) return 1;
  if (ext !== 'pdf') return null;
  const m = /-(\d+)p\.pdf$/i.exec(p);
  return m?.[1] ? Number(m[1]) : 3;
}

function settingsMeta(): SettingsMeta {
  return {
    config_unreadable: false,
    config_path:
      '/Users/e2e/Library/Application Support/com.Maecly.Pluryprint/config.toml',
    limits: {
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
    },
  };
}

export class FakeBackend {
  readonly calls: { cmd: string; args: unknown }[] = [];
  private emit: Emit = () => {};
  private timer: ReturnType<typeof setInterval> | undefined;
  private seq = 0;

  printers: Printer[] = clone(fixturePrintersWithVirtual);
  discovered: DiscoveredPrinter[] = clone(fixtureDiscovered);
  prefs: PrinterPrefs = {
    default_printer: null,
    disabled: [],
    routes: {},
    overrides: {},
  };
  policy: PrinterPolicy = { default_printer: null };
  profiles: Profile[] = clone(fixtureProfiles);
  settings: AppSettings = {
    ...clone(fixtureAppSettings),
    meta: settingsMeta(),
  };
  appInfo: AppInfo = clone(fixtureAppInfo);
  updateCheck: UpdateCheck = clone(fixtureUpdateCheckUpToDate);
  updateState: UpdateState = { last_check_at: null, skipped_version: null };
  queue: QueueSnapshot;
  pendingSelections: Selection[] = [];
  /**
   * FR-127: the browser bridge as Settings > Browser integration sees it.
   * Chrome is registered; Brave and Edge are installed but not; Chromium is
   * absent. `bridge_reconnect` registers every browser found.
   */
  bridge: BridgeOverview = {
    platform: 'macos',
    installed: true,
    translocated: false,
    browsers: (['chrome', 'edge', 'brave', 'chromium'] as const).map(
      (browser) => ({
        browser,
        detected: browser !== 'chromium',
        manifest_path: `/Users/e2e/Library/Application Support/${browser}/NativeMessagingHosts/com.maecly.pluryprint.json`,
        registered: browser === 'chrome',
        points_here: browser === 'chrome',
        binary_exists: browser === 'chrome',
        allowed_origins:
          browser === 'chrome'
            ? ['chrome-extension://ibgodfpcihlddbbfaoooehfgbmahipbj/']
            : [],
      }),
    ),
  };
  openPaths: string[];
  savePath: string | null;

  constructor(opts: FakeBackendOptions = {}) {
    this.queue = opts.seedQueue
      ? clone(fixtureQueueSnapshot)
      : { schema_version: 1, queue_paused: false, jobs: [] };
    this.openPaths = opts.openPaths ?? DEFAULT_OPEN_PATHS;
    this.savePath =
      opts.savePath ?? '/Users/e2e/Desktop/profile.pluryprintprofile';
    const tickMs = opts.tickMs ?? 700;
    if (tickMs > 0) this.timer = setInterval(() => this.tick(), tickMs);
  }

  /** Where events go; `ipc-stub.ts` wires this to the registered listeners. */
  connect(emit: Emit): void {
    this.emit = emit;
  }

  dispose(): void {
    if (this.timer) clearInterval(this.timer);
  }

  /** Hand a context-menu selection to the running app (as a second launch would). */
  forwardSelection(sel: Selection): void {
    this.emit('selection-forwarded', sel);
  }

  /** Answer one `invoke`. Rejects with an `IpcError` exactly like the host. */
  async invoke(
    cmd: string,
    args: Record<string, unknown> = {},
  ): Promise<unknown> {
    this.calls.push({ cmd, args });
    return this.dispatch(cmd, args);
  }

  // -------------------------------------------------------------------------
  // Commands
  // -------------------------------------------------------------------------

  private dispatch(cmd: string, a: Record<string, unknown>): unknown {
    switch (cmd) {
      // --- diag / about ---
      case 'app_info':
        return clone(this.appInfo);
      case 'open_logs_folder':
      case 'report_issue':
      case 'open_external':
        return null;
      case 'check_for_updates':
        this.updateState.last_check_at = new Date().toISOString();
        return {
          ...clone(this.updateCheck),
          checked_at: new Date().toISOString(),
        };
      case 'get_update_state':
        return clone(this.updateState);
      case 'set_skipped_update_version':
        this.updateState.skipped_version = (a.version as string | null) ?? null;
        return null;

      // --- printers ---
      case 'list_printers':
        return this.printers.map((p) => ({
          ...clone(p),
          is_default: p.id === this.effectiveDefaultPrinter(),
        }));
      case 'default_printer':
        return this.effectiveDefaultPrinter();
      case 'discover_printers':
        return clone(this.discovered);
      case 'get_printer_prefs':
        return clone(this.prefs);
      case 'set_printer_prefs':
        this.prefs = clone(a.prefs as PrinterPrefs);
        return clone(this.prefs);
      case 'set_default_printer':
        return this.setDefaultPrinter(a.name as string);
      case 'print_test_page':
        this.requirePrinter(a.name as string);
        return null;
      case 'open_printer_settings':
        return null;
      case 'add_printer':
        return this.addPrinter(a.name as string, a.uri as string);
      case 'get_printer_policy':
        return clone(this.policy);

      // --- jobs / queue ---
      case 'inspect_files':
        return (a.paths as string[]).map((p) => this.inspect(p));
      case 'expand_folders':
        return (a.paths as string[]).map((p) =>
          this.expand(p, a.scan as FolderScan),
        );
      case 'preview_command':
        return this.preview(a.req as PrintRequest);
      case 'print_files':
        return this.printFiles(a.req as PrintRequest);
      case 'queue_status':
        return clone(this.queue);
      case 'cancel_job':
        return this.mutateJob(a.id as string, (j) => {
          if (TERMINAL.includes(j.state))
            throw err('InvalidState', `job ${j.id} has already finished`);
          this.finish(j, 'cancelled');
        });
      case 'cancel_all': {
        const live = this.queue.jobs.filter((j) => !TERMINAL.includes(j.state));
        live.forEach((j) => this.finish(j, 'cancelled'));
        this.publish();
        return live.length;
      }
      case 'pause_job':
        return this.mutateJob(a.id as string, (j) => {
          if (!['queued', 'converted'].includes(j.state)) {
            throw err(
              'InvalidState',
              `job ${j.id} cannot be paused while ${j.state}`,
            );
          }
          j.state = 'paused';
          j.progress = null;
        });
      case 'resume_job':
        return this.mutateJob(a.id as string, (j) => {
          if (j.state !== 'paused')
            throw err('InvalidState', `job ${j.id} is not paused`);
          j.state = 'queued';
          j.error = null;
        });
      case 'adjust_job':
        return this.mutateJob(a.id as string, (j) => {
          if (j.state !== 'paused')
            throw err('InvalidState', 'only a paused job can be changed');
          const change = a.change as {
            printer?: string | null;
            duplex?: Job['settings']['duplex'] | null;
          };
          if (change.printer != null) {
            const printer = change.printer.trim();
            j.settings.printer = printer === '' ? null : printer;
            j.printer_resolved = null;
          }
          if (change.duplex != null) j.settings.duplex = change.duplex;
        });
      case 'retry_job':
        return this.mutateJob(a.id as string, (j) => {
          if (j.state !== 'failed')
            throw err('InvalidState', `job ${j.id} has not failed`);
          j.state = 'queued';
          j.error = null;
          j.finished_at = null;
        });
      case 'pause_queue':
        this.queue.queue_paused = true;
        this.publish();
        return null;
      case 'resume_queue':
        this.queue.queue_paused = false;
        this.publish();
        return null;
      case 'clear_done': {
        const before = this.queue.jobs.length;
        this.queue.jobs = this.queue.jobs.filter(
          (j) => !TERMINAL.includes(j.state),
        );
        this.publish();
        return before - this.queue.jobs.length;
      }
      case 'take_pending_selections': {
        const out = this.pendingSelections;
        this.pendingSelections = [];
        return out;
      }

      // --- browser bridge (FR-127) ---
      case 'bridge_status':
        return clone(this.bridge);
      case 'bridge_reconnect':
        this.bridge = {
          ...this.bridge,
          browsers: this.bridge.browsers.map((b) =>
            b.detected
              ? {
                  ...b,
                  registered: true,
                  points_here: true,
                  binary_exists: true,
                  allowed_origins: [
                    'chrome-extension://ibgodfpcihlddbbfaoooehfgbmahipbj/',
                  ],
                }
              : b,
          ),
        };
        return clone(this.bridge);

      // --- profiles ---
      case 'list_profiles':
        return this.listProfiles();
      case 'get_profile':
        return this.stamp(this.requireProfile(a.id as string));
      case 'save_profile':
        return this.saveProfile(a.profile as Profile);
      case 'delete_profile':
        return this.deleteProfile(a.id as string);
      case 'set_default_profile':
        this.requireProfile(a.id as string);
        this.settings.engine.default_profile = a.id as string;
        return null;
      case 'export_profile':
        this.requireProfile(a.id as string);
        return null;
      case 'import_profiles':
        return this.importProfiles(a.path as string);

      // --- settings ---
      case 'get_app_settings':
        return clone(this.settings);
      case 'set_app_settings': {
        const next = clone(a.settings as AppSettings);
        const engine = next.engine;
        const limits = settingsMeta().limits;
        for (const key of Object.keys(limits) as (keyof typeof limits)[]) {
          const b = limits[key];
          // `print_timeout_seconds` is optional on the wire (an older
          // config.toml omits it); the backend reads it as its default.
          const v = engine[key] ?? b.default;
          const ok =
            (v === 0 && b.zero === 'disables') || (v >= b.min && v <= b.max);
          if (!ok)
            throw err(
              'InvalidRequest',
              `${key} must be between ${b.min} and ${b.max}`,
            );
        }
        // As in `ipc::settings::set_app_settings_at`: the default profile is
        // owned by `set_default_profile`, so whatever the client sends for it
        // (often a stale settings cache) is ignored and the stored one kept.
        next.engine = {
          ...next.engine,
          default_profile: this.settings.engine.default_profile ?? null,
        };
        this.settings = { ...next, meta: settingsMeta() };
        return clone(this.settings);
      }

      // --- app services the specs never assert on ---
      case 'notify_native':
        // The browser window is the foreground: no OS notification.
        return false;
      case 'log_frontend_error':
      case 'restart_app':
        return null;
      case 'install_update':
        throw err(
          'UpdateInstallFailed',
          'installing is not available in the E2E fake backend',
        );
      // FR-058: the consent flag is the contract; a page "opens" and the
      // dependency stays missing (nothing is installed in a browser).
      case 'install_dependency': {
        if (a.consent !== true) {
          throw err('InvalidRequest', 'installing needs the user consent');
        }
        const report = (this.appInfo.dependencies ?? []).find(
          (d) => d.id === a.id,
        );
        if (!report) throw err('InvalidRequest', `unknown dependency ${a.id}`);
        return {
          status: 'opened',
          exit_code: null,
          elevated: false,
          report: clone(report),
        };
      }
      // Nothing runs in the browser, so there is never an install to stop,
      // nor one holding the install slot.
      case 'cancel_dependency_install':
        return false;
      case 'dependency_install_status':
        return { running: null, last: null };
      // FR-059: the password is taken and the job goes back to the queue,
      // where the timer converts it (any password "opens" the fake PDF).
      case 'job_supply_password':
        return this.mutateJob(a.id as string, (j) => {
          if (j.state !== 'paused' || j.error?.code !== 'pdf_protected') {
            throw err('InvalidState', `job ${j.id} is not waiting for one`);
          }
          j.state = 'queued';
          j.error = null;
        });

      default:
        throw err('Internal', `not stubbed in the E2E fake backend: ${cmd}`);
    }
  }

  // -------------------------------------------------------------------------
  // Printers
  // -------------------------------------------------------------------------

  private effectiveDefaultPrinter(): string | null {
    return (
      this.prefs.default_printer ??
      this.printers.find((p) => p.is_default)?.id ??
      null
    );
  }

  private requirePrinter(name: string): Printer {
    const p = this.printers.find((x) => x.id === name || x.name === name);
    if (!p) throw err('InvalidRequest', `printer '${name}' is not installed`);
    return p;
  }

  private setDefaultPrinter(name: string): null {
    const target = this.requirePrinter(name);
    this.printers.forEach((p) => (p.is_default = p.id === target.id));
    this.prefs.default_printer = null;
    return null;
  }

  private addPrinter(name: string, uri: string): null {
    if (!name.trim()) throw err('InvalidRequest', 'a printer name is required');
    if (!/^(ipp|ipps|lpd|socket|http|https):\/\//.test(uri)) {
      throw err('InvalidRequest', `unsupported printer URI: ${uri}`);
    }
    const id = name.trim().replace(/\s+/g, '_');
    if (this.printers.some((p) => p.id === id)) {
      throw err('InvalidRequest', `a printer named '${name}' already exists`);
    }
    this.printers.push({
      ...clone(fixturePrintersWithVirtual[1] as Printer),
      id,
      name: name.trim(),
      is_default: false,
      connection: uri,
      driver: 'IPP Everywhere',
    });
    return null;
  }

  // -------------------------------------------------------------------------
  // Files
  // -------------------------------------------------------------------------

  private inspect(path: string): FileInfo {
    const name = basename(path);
    const folder = FAKE_FOLDERS[path];
    const base = {
      path,
      name,
      ext: extOf(path),
      bytes: 0,
      pages: null,
      reason: null,
      reason_code: null,
    };
    if (folder) {
      return {
        ...base,
        ext: '',
        supported: false,
        reason_code: 'folder',
        is_dir: true,
        has_subfolders: folder.nested.length > 0,
      };
    }
    if (/missing/i.test(name)) {
      return {
        ...base,
        supported: false,
        reason: 'File not found',
        reason_code: 'not_found',
        is_dir: false,
        has_subfolders: false,
      };
    }
    const ext = extOf(path);
    if (!SUPPORTED.has(ext)) {
      return {
        ...base,
        supported: false,
        reason: 'Unsupported file type',
        reason_code: 'unsupported_type',
        is_dir: false,
        has_subfolders: false,
      };
    }
    return {
      ...base,
      bytes: 240 * 1024,
      pages: pagesOf(path),
      supported: true,
      is_dir: false,
      has_subfolders: false,
    };
  }

  private expand(root: string, scan: FolderScan): FolderExpansion {
    const folder = FAKE_FOLDERS[root];
    if (!folder) throw err('FileNotFound', `folder not found: ${root}`);
    const files =
      scan === 'recursive'
        ? [...folder.top, ...folder.nested]
        : [...folder.top];
    return {
      root,
      scan,
      files: files.sort(),
      skipped: [],
      files_truncated: false,
      depth_truncated: scan === 'top_level' && folder.nested.length > 0,
      limits: { max_files: 5000, max_depth: 16 },
    };
  }

  // -------------------------------------------------------------------------
  // Jobs
  // -------------------------------------------------------------------------

  private validate(req: PrintRequest): void {
    if (req.files.length === 0)
      throw err('InvalidRequest', 'no files to print');
    const { copies, page_ranges } = req.settings;
    if (!Number.isInteger(copies) || copies < 1 || copies > 999) {
      throw err('InvalidRequest', 'copies must be between 1 and 999');
    }
    if (
      page_ranges != null &&
      !/^\s*\d+(\s*-\s*\d+)?(\s*,\s*\d+(\s*-\s*\d+)?)*\s*$/.test(page_ranges)
    ) {
      throw err('InvalidRequest', `invalid page range: ${page_ranges}`);
    }
    for (const f of req.files) {
      const info = this.inspect(f);
      if (!info.supported) {
        throw err(
          info.reason === 'File not found'
            ? 'FileNotFound'
            : 'UnsupportedFormat',
          `${info.name}: ${info.reason}`,
        );
      }
    }
  }

  private preview(req: PrintRequest): string {
    const s = req.settings;
    const printer = s.printer ?? this.effectiveDefaultPrinter() ?? '<default>';
    const first = req.files[0] ? basename(req.files[0]) : 'document.pdf';
    const opts = [
      `-o sides=${s.duplex === 'none' ? 'one-sided' : s.duplex === 'long' ? 'two-sided-long-edge' : 'two-sided-short-edge'}`,
      s.color_mode === 'monochrome' ? '-o ColorModel=Gray' : '',
      s.copies > 1 ? `-n ${s.copies}` : '',
      s.paper_size === 'auto'
        ? ''
        : `-o media=${MEDIA[s.paper_size as keyof typeof MEDIA] ?? s.paper_size}`,
      s.page_ranges
        ? `-o page-ranges=${s.page_ranges.replace(/\s+/g, '')}`
        : '',
    ].filter(Boolean);
    return `lp -d "${printer}" ${opts.join(' ')} "${first}"`;
  }

  private printFiles(req: PrintRequest): JobBatch {
    this.validate(req);
    const batch_id = this.nextId('batch');
    const now = new Date().toISOString();
    const jobs: Job[] = req.files.map((path) => ({
      id: this.nextId('job'),
      batch_id,
      state: 'queued',
      input_path: path,
      file_name: basename(path),
      input_ext: extOf(path),
      input_bytes: 240 * 1024,
      pages: pagesOf(path),
      cache_pdf: null,
      settings: clone(req.settings),
      profile_name: req.profile_name,
      printer_resolved: null,
      attempts: 0,
      error: null,
      progress: null,
      submitted_at: now,
      started_at: null,
      finished_at: null,
    }));
    this.queue.jobs.push(...jobs);
    this.publish();
    return { batch_id, job_ids: jobs.map((j) => j.id) };
  }

  private mutateJob(id: string, f: (j: Job) => void): null {
    const job = this.queue.jobs.find((j) => j.id === id);
    if (!job) throw err('JobNotFound', `no job with id ${id}`);
    f(job);
    this.publish();
    return null;
  }

  private finish(j: Job, state: JobState): void {
    j.state = state;
    j.progress = null;
    j.finished_at = new Date().toISOString();
  }

  /**
   * Advance the queue one step. Converting and printing each take a few ticks
   * of progress; at most one job prints at a time (the dispatcher's single
   * print slot). A file whose name contains `fail` fails at the print stage.
   */
  tick(): void {
    if (this.queue.queue_paused) return;
    let changed = false;
    const now = new Date().toISOString();

    for (const j of this.queue.jobs) {
      if (j.state === 'converting' || j.state === 'printing') {
        const pct = Math.min(1, (j.progress?.percent ?? 0) + 0.34);
        if (pct < 1) {
          j.progress = { phase: j.state, percent: pct };
          this.emit('job-progress-event', {
            id: j.id,
            phase: j.state,
            percent: pct,
          });
          continue;
        }
        if (j.state === 'converting') {
          j.state = 'converted';
          j.progress = null;
        } else if (/fail/i.test(j.file_name)) {
          j.error = {
            code: 'print_failed',
            message: 'The printer rejected the job',
          };
          this.finish(j, 'failed');
        } else {
          this.finish(j, 'done');
        }
        changed = true;
      }
    }

    const next = this.queue.jobs.find((j) => j.state === 'queued');
    if (next) {
      next.state = 'converting';
      next.attempts += 1;
      next.started_at ??= now;
      next.progress = { phase: 'converting', percent: 0 };
      changed = true;
    }

    const printing = this.queue.jobs.some((j) => j.state === 'printing');
    const ready = this.queue.jobs.find((j) => j.state === 'converted');
    if (!printing && ready) {
      ready.state = 'printing';
      ready.printer_resolved =
        ready.settings.printer ?? this.effectiveDefaultPrinter();
      ready.progress = { phase: 'printing', percent: 0 };
      changed = true;
    }

    if (changed) this.publish();
  }

  private publish(): void {
    this.emit('queue-updated', clone(this.queue));
  }

  private nextId(prefix: string): string {
    this.seq += 1;
    return `e2e-${prefix}-${String(this.seq).padStart(4, '0')}`;
  }

  // -------------------------------------------------------------------------
  // Profiles
  // -------------------------------------------------------------------------

  private defaultProfileId(): string {
    const id = this.settings.engine.default_profile;
    return id && this.profiles.some((p) => p.id === id) ? id : 'builtin-quick';
  }

  private stamp(p: Profile): Profile {
    return { ...clone(p), default: p.id === this.defaultProfileId() };
  }

  private listProfiles(): Profile[] {
    return this.profiles.map((p) => this.stamp(p));
  }

  private requireProfile(id: string): Profile {
    const p = this.profiles.find((x) => x.id === id);
    if (!p) throw err('ProfileNotFound', `no profile with id ${id}`);
    return p;
  }

  private saveProfile(input: Profile): Profile {
    const id = input.id.trim();
    if (id.startsWith('builtin-'))
      throw err('InvalidRequest', 'built-in profiles cannot be edited');
    const name = input.name.trim();
    if (!name) throw err('InvalidRequest', 'a profile needs a name');
    if (
      this.profiles.some(
        (p) => p.id !== id && p.name.toLowerCase() === name.toLowerCase(),
      )
    ) {
      throw err('InvalidRequest', `a profile named "${name}" already exists`);
    }
    if (input.copies < 1 || input.copies > 999) {
      throw err('InvalidRequest', 'copies must be between 1 and 999');
    }
    const saved: Profile = {
      ...clone(input),
      id: id || this.nextId('profile'),
      name,
      builtin: false,
      default: false,
    };
    const at = this.profiles.findIndex((p) => p.id === saved.id);
    if (at >= 0) this.profiles[at] = saved;
    else this.profiles.push(saved);
    return this.stamp(saved);
  }

  private deleteProfile(id: string): null {
    const p = this.requireProfile(id);
    if (p.builtin)
      throw err('InvalidRequest', 'built-in profiles cannot be deleted');
    if (id === this.defaultProfileId())
      throw err('InvalidRequest', 'set another profile as default first');
    this.profiles = this.profiles.filter((x) => x.id !== id);
    return null;
  }

  private importProfiles(path: string): Profile[] {
    if (!/\.(pluryprintprofile|toml)$/i.test(path)) {
      throw err('InvalidRequest', 'this is not a Pluryprint profile file');
    }
    const base = basename(path).replace(/\.[^.]+$/, '');
    let name = `Imported ${base}`;
    for (let n = 2; this.profiles.some((p) => p.name === name); n += 1)
      name = `Imported ${base} (${n})`;
    const saved = this.saveProfile({
      ...clone(fixtureProfiles[3] as Profile),
      id: '',
      name,
    });
    return [saved];
  }
}
