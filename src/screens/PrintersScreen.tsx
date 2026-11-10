import {
  Fragment,
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from 'react';
import { useLatest } from '@/lib/useLatest';

import { toOsId, type OsId as HostOs } from '@/lib/platform';
import { CenteredNotice } from '@/components/CenteredNotice';
import { ConfirmDialog, type ConfirmState } from '@/components/ConfirmDialog';
import { ModalOverlay } from '@/components/Modal';
import { useModalFocus } from '@/components/useModal';
import { Icon, type IconName } from '@/components/Icon';
import {
  MPBadge,
  MPButton,
  MPCard,
  MPCheckbox,
  MPIconButton,
  MPInput,
  MPSectionLabel,
  MPSelect,
  MPTopBar,
  StatusDot,
  type BadgeTone,
  type SelectOption,
} from '@/components/ui';
import { useT, type TFunction } from '@/i18n/I18nProvider';
import { toast } from 'sonner';
import {
  addPrinter,
  discoverPrinters,
  fetchAppInfo,
  fetchDefaultPrinter,
  getPrinterPolicy,
  getPrinterPrefs,
  isTauri,
  listPrinters,
  openPrinterSettings,
  printTestPage,
  setDefaultPrinter,
  setPrinterPrefs,
  type PrinterOverride,
  type PrinterPolicy,
  type PrinterPrefs,
} from '@/lib/ipc';
import { errorTechnical } from '@/lib/ipcError';
import {
  buildPrinterUri,
  DEFAULT_PORTS,
  isValidDisplayName,
  isValidHost,
  isValidPort,
  MAX_PORT,
  MIN_PORT,
  type PrinterProtocol,
} from '@/lib/printerAddress';
import { resolvePrinterId } from '@/lib/printerIdentity';
import { primePrinterNames } from '@/stores/printerNamesStore';
import type {
  UiDiscoveredPrinter,
  UiPrinter,
  UiPrinterState,
  UiPrinterType,
} from '@/types/domain';

type PrinterMode = 'default' | 'add-network' | 'add-ip' | 'add-system';

/** StatusDot statuses reused for printer states. */
type DotStatus = 'done' | 'cancelled' | 'failed' | 'printing' | 'queued';

type T = TFunction<'printers'>;

/** Icon per printer family; the label comes from the `type.*` catalog keys. */
const TYPE_ICON: Record<UiPrinterType, IconName> = {
  laser: 'printer',
  inkjet: 'printer',
  mfp: 'printer',
  wide: 'image',
  label: 'page',
  virtual: 'file',
  printer: 'printer',
};

const STATUS_META: Record<
  UiPrinter['status'],
  { tone: BadgeTone; dot: DotStatus }
> = {
  ready: { tone: 'success', dot: 'done' },
  offline: { tone: 'neutral', dot: 'cancelled' },
  error: { tone: 'error', dot: 'failed' },
  busy: { tone: 'accent', dot: 'printing' },
};

const TONER_COLORS: Record<string, string> = {
  cyan: '#0E9CC0',
  magenta: '#C81E5B',
  yellow: '#D9A300',
  black: '#2A2620',
};

const STATE_TONE: Record<UiPrinterState, BadgeTone> = {
  default: 'accent',
  active: 'neutral',
  disabled: 'neutral',
};

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Normalize a user-typed extension to leading-dot lowercase (e.g. ".pdf"). */
function normalizeExt(raw: string): string | null {
  const t = raw.trim().toLowerCase().replace(/^\*/, '');
  if (!t) return null;
  const ext = t.startsWith('.') ? t : `.${t}`;
  return /^\.[a-z0-9]+$/.test(ext) ? ext : null;
}

/**
 * The host OS, as reported by the backend (`app_info.os` = Rust's
 * `std::env::consts::OS`, read with `lib/platform`'s `toOsId`), or `null`
 * while unknown.
 *
 * This is deliberately NOT `navigator.platform`: that API is deprecated and
 * frozen, describes the webview rather than the host, and silently defaulted
 * every unrecognized value to macOS — which decided a real business rule (which
 * protocols the add-by-IP form offers). It was also evaluated at module scope,
 * so tests could never reach the Windows branch. Copy that only words a hint
 * may use `useHostPlatform`'s guess; this screen waits for the backend.
 */
type OsId = HostOs | null;

const OS_LABEL: Record<HostOs, string> = {
  windows: 'Windows',
  macos: 'macOS',
  linux: 'Linux',
};

/**
 * Render a translated template carrying `{{slot}}` placeholders, substituting
 * React nodes for them.
 *
 * Keeping the whole sentence in ONE catalog entry lets a translator control
 * word order (Spanish rarely keeps the English one), while emphasis and
 * monospace styling stay in the component instead of leaking markup into JSON.
 */
function RichText({
  template,
  slots,
}: {
  template: string;
  slots: Record<string, ReactNode>;
}) {
  return (
    <>
      {template.split(/(\{\{\w+\}\})/g).map((part, i) => {
        const name = /^\{\{(\w+)\}\}$/.exec(part)?.[1];
        const slot = name === undefined ? undefined : slots[name];
        return <Fragment key={i}>{slot ?? part}</Fragment>;
      })}
    </>
  );
}

/** Inline monospace token (a file name, an error code) inside prose. */
function Mono({ children }: { children: ReactNode }) {
  return <span className="mp-mono">{children}</span>;
}

function LevelBar({
  label,
  value,
  color,
}: {
  label: string;
  value: number;
  color: string;
}) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
      <span
        className="mp-mono"
        style={{
          fontSize: 10,
          color: 'var(--mp-textDim)',
          width: 54,
          textTransform: 'capitalize',
        }}
      >
        {label}
      </span>
      <div
        style={{
          flex: 1,
          height: 6,
          background: 'var(--mp-surface3)',
          borderRadius: 3,
          overflow: 'hidden',
        }}
      >
        <div
          style={{
            height: '100%',
            width: `${value * 100}%`,
            background: color,
            transition: 'width .3s',
          }}
        />
      </div>
      <span
        className="mp-mono"
        style={{
          fontSize: 10,
          color: value < 0.2 ? 'var(--mp-error)' : 'var(--mp-textMuted)',
          width: 30,
          textAlign: 'right',
        }}
      >
        {Math.round(value * 100)}%
      </span>
    </div>
  );
}

/**
 * One capability: met (check), not offered (cross), or `unknown` — the driver
 * or the probe did not say, which is neither a promise nor a refusal.
 */
function CapPill({
  ok,
  unknown = false,
  children,
}: {
  ok: boolean;
  unknown?: boolean;
  children: ReactNode;
}) {
  const met = ok && !unknown;
  return (
    <div
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 5,
        padding: '4px 9px',
        borderRadius: 5,
        fontSize: 11.5,
        background: met ? 'var(--mp-successSoft)' : 'var(--mp-surface2)',
        color: met ? 'var(--mp-success)' : 'var(--mp-textDim)',
        border: `1px solid ${met ? 'transparent' : 'var(--mp-border)'}`,
      }}
    >
      <Icon name={unknown ? 'info' : met ? 'check' : 'x'} size={11} />
      {children}
    </div>
  );
}

function Block({
  label,
  action,
  children,
}: {
  label: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div style={{ marginBottom: 18 }}>
      <div
        style={{
          display: 'flex',
          justifyContent: 'space-between',
          alignItems: 'center',
          marginBottom: 8,
        }}
      >
        <MPSectionLabel>{label}</MPSectionLabel>
        {action}
      </div>
      {children}
    </div>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div
      style={{
        display: 'grid',
        gridTemplateColumns: 'minmax(90px, 120px) minmax(0, 1fr)',
        gap: 14,
        padding: '9px 0',
        borderBottom: '1px solid var(--mp-border)',
        alignItems: 'center',
      }}
    >
      <span style={{ fontSize: 12, color: 'var(--mp-textMuted)' }}>
        {label}
      </span>
      <span style={{ fontSize: 12.5, minWidth: 0, overflowWrap: 'anywhere' }}>
        {children}
      </span>
    </div>
  );
}

/**
 * Everything the Printers screen shows, asked of the OS at once. Only the
 * printer list itself can fail the load; the rest degrade to "unknown".
 */
async function fetchPrintersView() {
  const [list, def, prefs, policy, info] = await Promise.all([
    listPrinters(),
    fetchDefaultPrinter().catch(() => null),
    getPrinterPrefs().catch(() => ({}) as PrinterPrefs),
    getPrinterPolicy().catch(() => null),
    // Platform-dependent copy and the add-by-IP protocol list read this;
    // a failure just leaves the OS unknown, which every consumer handles.
    fetchAppInfo().catch(() => null),
  ]);
  return { list, def, prefs, policy, info };
}
type PrintersView = Awaited<ReturnType<typeof fetchPrintersView>>;

export function PrintersScreen() {
  const t = useT('printers');
  // Outside Tauri there is nothing to list: no spinner, the no-host notice.
  const [printers, setPrinters] = useState<UiPrinter[] | null>(() =>
    isTauri() ? null : [],
  );
  const [prefs, setPrefs] = useState<PrinterPrefs>({});
  // Latest prefs for slow async handlers: merging into a render-time capture
  // after an awaited OS call would silently revert concurrent prefs changes.
  const prefsRef = useLatest(prefs);
  const [policy, setPolicy] = useState<PrinterPolicy | null>(null);
  const [os, setOs] = useState<OsId>(null);
  const [error, setError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [defaultId, setDefaultId] = useState<string | null>(null);
  const [mode, setMode] = useState<PrinterMode>('default');
  const [busy, setBusy] = useState(false);
  const [ruleDraft, setRuleDraft] = useState<string | null>(null);
  const [confirmState, setConfirmState] = useState<ConfirmState | null>(null);
  // Missing printers the user chose to "keep paused" — hidden until reload.
  const [dismissedMissing, setDismissedMissing] = useState<Set<string>>(
    () => new Set(),
  );
  // Reassign target for the missing-printer banner (falls back to first ready).
  const [reassignTarget, setReassignTarget] = useState<string | null>(null);

  /** Show what the OS answered. */
  const show = useCallback((view: PrintersView) => {
    const { list, def, prefs: p, policy: pol, info } = view;
    setPrinters(list);
    primePrinterNames(list);
    setPrefs(p ?? {});
    setPolicy(pol);
    setOs(toOsId(info?.os));
    setDefaultId(def ?? list.find((q) => q.isDefault)?.id ?? null);
    setSelectedId((cur) =>
      cur && list.some((q) => q.id === cur) ? cur : (list[0]?.id ?? null),
    );
  }, []);
  const showFailure = useCallback((e: unknown) => {
    setError(errText(e));
    setPrinters([]);
  }, []);

  /** Rescan: back to the loading state, then ask again. */
  const load = useCallback(async () => {
    if (!isTauri()) return;
    setPrinters(null);
    setError(null);
    // "Keep paused" lasts until the next Rescan/reload (spec § Edge states).
    setDismissedMissing(new Set());
    await fetchPrintersView().then(show, showFailure);
  }, [show, showFailure]);

  // The first load: the initial state already is the loading state.
  useEffect(() => {
    if (!isTauri()) return;
    let alive = true;
    fetchPrintersView().then(
      (view) => {
        if (alive) show(view);
      },
      (e: unknown) => {
        if (alive) showFailure(e);
      },
    );
    return () => {
      alive = false;
    };
  }, [show, showFailure]);

  // Reset the inline "add rule" input whenever the selected printer changes
  // (adjusted while rendering, so no frame shows it under another printer).
  const [ruleDraftFor, setRuleDraftFor] = useState(selectedId);
  if (ruleDraftFor !== selectedId) {
    setRuleDraftFor(selectedId);
    setRuleDraft(null);
  }

  // Persist a prefs change optimistically; reload + toast on failure. Resolves
  // to whether the save landed, so a caller chaining it after an OS action
  // never reports success for a half-done change.
  const savePrefs = useCallback(
    async (next: PrinterPrefs, success?: string): Promise<boolean> => {
      setPrefs(next);
      try {
        const saved = await setPrinterPrefs(next);
        setPrefs(saved);
        if (success) toast.success(success);
        return true;
      } catch (e) {
        toast.error(errText(e));
        void load();
        return false;
      }
    },
    [load],
  );

  const ready = printers?.filter((p) => p.status === 'ready').length ?? 0;
  const topBar = (
    <MPTopBar
      title={t('title')}
      subtitle={
        printers === null
          ? t('subtitle.detecting')
          : t('subtitle.counts', { total: printers.length, ready })
      }
      right={
        <>
          <MPButton
            variant="ghost"
            icon="rotate"
            size="sm"
            onClick={() => void load()}
          >
            {t('actions.rescan')}
          </MPButton>
          <MPButton
            variant="primary"
            icon="plus"
            size="sm"
            disabled={printers === null}
            onClick={() => setMode('add-network')}
          >
            {t('actions.addPrinter')}
          </MPButton>
        </>
      }
    />
  );

  if (printers === null || printers.length === 0) {
    // The add-printer flow must stay reachable with zero installed printers —
    // that is exactly the first-run scenario it exists for. The network/by-IP
    // tabs need no installed printers at all.
    return (
      <>
        {topBar}
        <CenteredNotice
          loading={printers === null}
          error={isTauri() ? error : t('notice.noHost')}
          loadingTitle={t('notice.loadingTitle')}
          loadingBody={t('notice.loadingBody')}
          emptyTitle={t('notice.emptyTitle')}
          errorTitle={t('notice.errorTitle')}
          hint={t('notice.hint')}
        />
        {printers !== null &&
          (mode === 'add-network' ||
            mode === 'add-ip' ||
            mode === 'add-system') && (
            <AddPrinterModal
              mode={mode}
              onMode={setMode}
              os={os}
              onOpenSettings={() =>
                void openPrinterSettings().catch((e) => toast.error(errText(e)))
              }
              onAdded={() => {
                setMode('default');
                void load();
              }}
              printers={printers}
              prefs={prefs}
              // No printer is listed here, so there is nothing to untick.
              removal={{ policyId: null, osDefaultId: defaultId }}
              onSavePrefs={savePrefs}
              onConfirm={setConfirmState}
            />
          )}
      </>
    );
  }

  // Non-null: the null/empty-printers case returned early above, so
  // `printers[0]` always exists here.
  const sel = printers.find((p) => p.id === selectedId) ?? printers[0]!;
  const sm = STATUS_META[sel.status];

  const disabledSet = new Set(prefs.disabled ?? []);
  // Effective default: org policy wins, then the Pluryprint app default, then
  // whatever the OS reports.
  // An admin may write the display name in policy.json; resolve it to the
  // queue id the same way the print path does, so the star lands on the
  // printer jobs will really use.
  const policyRef = policy?.default_printer?.trim() || null;
  const policyId = resolvePrinterId(printers, policyRef) ?? policyRef;
  const appDefaultId = policyId ?? prefs.default_printer ?? defaultId;
  // When policy pins a default, the app treats it as read-only.
  const policyLocked = policyRef != null;
  const lockedName = printers.find((p) => p.id === policyId)?.name ?? null;
  const stateOf = (id: string): UiPrinterState =>
    disabledSet.has(id)
      ? 'disabled'
      : id === appDefaultId
        ? 'default'
        : 'active';
  const selState = stateOf(sel.id);
  // Where jobs go today, for the "what removing it changes" question.
  const removalContext: RemovalContext = {
    prefs,
    policyId,
    osDefaultId: defaultId,
  };
  const isDefault = selState === 'default';
  const isDisabled = selState === 'disabled';
  const selOverride = prefs.overrides?.[sel.id];
  const selRoutes = Object.entries(prefs.routes ?? {})
    .filter(([, pid]) => pid === sel.id)
    .map(([ext]) => ext)
    .sort();

  // Missing-printer reconciliation: ids the prefs reference (routing targets,
  // per-printer overrides, app default) that the spooler no longer enumerates.
  // Ids the user chose to keep paused this session are hidden.
  const enumeratedIds = new Set(printers.map((p) => p.id));
  const referencedIds = new Set<string>();
  for (const pid of Object.values(prefs.routes ?? {})) {
    if (pid) referencedIds.add(pid);
  }
  for (const pid of Object.keys(prefs.overrides ?? {})) referencedIds.add(pid);
  if (prefs.default_printer) referencedIds.add(prefs.default_printer);
  const missingIds = [...referencedIds]
    .filter((id) => !enumeratedIds.has(id) && !dismissedMissing.has(id))
    .sort();
  const missingId = missingIds[0] ?? null;
  // Reassign targets: ready, not soft-disabled in Pluryprint, and not a
  // virtual queue — routing a physical printer's rules to "Save as PDF" is
  // almost never what the user meant.
  const reassignCandidates = printers.filter(
    (p) =>
      p.status === 'ready' && !disabledSet.has(p.id) && p.type !== 'virtual',
  );
  const reassignEffective =
    reassignTarget && reassignCandidates.some((p) => p.id === reassignTarget)
      ? reassignTarget
      : (reassignCandidates[0]?.id ?? '');

  const supplyLabel = (channel: string): string =>
    channel === 'cyan'
      ? t('supplies.cyan')
      : channel === 'magenta'
        ? t('supplies.magenta')
        : channel === 'yellow'
          ? t('supplies.yellow')
          : channel === 'black'
            ? t('supplies.black')
            : channel;

  // Action handlers — plain functions (declared after the early return, so
  // they are not hooks). Each gives toast feedback + error handling.
  const handleSetDefault = async () => {
    setBusy(true);
    const id = toast.loading(t('toast.settingDefault', { name: sel.name }));
    try {
      await setDefaultPrinter(sel.id);
      setDefaultId(sel.id);
      // Merge into the LATEST prefs, not the render-time capture: the OS call
      // above can be slow, and whole-object-saving stale prefs would silently
      // revert any concurrent prefs change (routes, disabled, overrides).
      const saved = await savePrefs({
        ...prefsRef.current,
        default_printer: sel.id,
      });
      if (saved) {
        toast.success(t('toast.defaultSet', { name: sel.name }), { id });
      } else {
        // The OS default changed but Pluryprint's did not: say exactly that
        // (savePrefs already toasted the reason) instead of claiming success.
        toast.warning(t('toast.defaultSetPartial', { name: sel.name }), {
          id,
        });
      }
    } catch (e) {
      toast.error(errText(e), { id, description: errorTechnical(e) });
    } finally {
      setBusy(false);
    }
  };

  const runTestPage = async () => {
    setBusy(true);
    const id = toast.loading(t('toast.sendingTestPage', { name: sel.name }));
    try {
      await printTestPage(sel.id);
      // Deliberately not "delivered": on the Windows fallback path the OS
      // helper cannot report whether the page was actually accepted, so the
      // copy points the user at the printer's own queue instead of claiming
      // more than the backend knows.
      toast.success(t('toast.testPageSent', { name: sel.name }), { id });
    } catch (e) {
      toast.error(errText(e), { id, description: errorTechnical(e) });
    } finally {
      setBusy(false);
    }
  };

  const handleTestPage = () => {
    setConfirmState({
      title: t('confirm.testPageTitle'),
      message: t('confirm.testPageMessage', { name: sel.name }),
      confirmLabel: t('confirm.testPageConfirm'),
      run: runTestPage,
    });
  };

  const setDisabled = async (disable: boolean) => {
    const removed = os
      ? t('toast.removed', { name: sel.name, os: OS_LABEL[os] })
      : t('toast.removedUnknownOs', { name: sel.name });
    let saved: PrinterPrefs;
    if (disable) {
      saved = withoutPrinter(prefsRef.current, sel.id);
    } else {
      const next = new Set(prefsRef.current.disabled ?? []);
      next.delete(sel.id);
      saved = { ...prefsRef.current, disabled: [...next] };
    }
    await savePrefs(
      saved,
      disable ? removed : t('toast.restored', { name: sel.name }),
    );
  };

  /**
   * Remove (soft-disable) or restore the selected printer. Removing a printer
   * jobs currently land on — Pluryprint's default, the system default, or a
   * file-type route target — first says where those jobs will go instead.
   */
  const handleToggleDisabled = async (disable: boolean) => {
    if (!disable) return setDisabled(false);
    const consequences = removalConsequences(sel.id, removalContext, t);
    if (consequences.length === 0) return setDisabled(true);
    setConfirmState({
      title: t('confirm.removeTitle', { name: sel.name }),
      message: consequences.join(' '),
      confirmLabel: t('confirm.removeConfirm'),
      run: () => setDisabled(true),
    });
  };

  /**
   * Write (or clear) a per-printer tray/paper override.
   *
   * An empty `value` is the "Printer default" option: it must DELETE the key,
   * not persist an empty string — the CUPS test page turns `Some("")` into a
   * bare `-o media=`, which is not what the user asked for. When the last field
   * goes, the whole per-printer entry goes too, so prefs never accumulate empty
   * objects that later look like references to a missing printer.
   */
  const handleOverride = async (field: 'tray' | 'paper', value: string) => {
    const overrides = { ...(prefs.overrides ?? {}) };
    const entry: PrinterOverride = { ...(overrides[sel.id] ?? {}) };
    if (field === 'tray') {
      if (value) entry.tray = value;
      else delete entry.tray;
    } else if (value) entry.paper = value;
    else delete entry.paper;

    if (entry.tray == null && entry.paper == null) delete overrides[sel.id];
    else overrides[sel.id] = entry;

    const message = value
      ? field === 'tray'
        ? t('toast.savedTray')
        : t('toast.savedPaper')
      : field === 'tray'
        ? t('toast.clearedTray')
        : t('toast.clearedPaper');
    await savePrefs({ ...prefs, overrides }, message);
  };

  const handleAddRule = async (raw: string) => {
    setRuleDraft(null);
    // Blurring or submitting the still-empty input just cancels the draft;
    // only actual invalid text earns an error toast.
    if (!raw.trim()) return;
    const ext = normalizeExt(raw);
    if (!ext) {
      toast.error(t('toast.invalidExtension'));
      return;
    }
    await savePrefs(
      { ...prefs, routes: { ...(prefs.routes ?? {}), [ext]: sel.id } },
      t('toast.routeAdded', { ext, name: sel.name }),
    );
  };

  const handleRemoveRule = async (ext: string) => {
    const routes = { ...(prefs.routes ?? {}) };
    delete routes[ext];
    await savePrefs({ ...prefs, routes });
  };

  const handleOpenSettings = async () => {
    try {
      await openPrinterSettings();
    } catch (e) {
      toast.error(errText(e));
    }
  };

  // Repoint every reference to a missing printer at `target`.
  const handleReassignMissing = async (mid: string, target: string) => {
    if (!target) return;
    const targetName = printers.find((p) => p.id === target)?.name ?? target;
    const routes = { ...(prefs.routes ?? {}) };
    for (const [ext, pid] of Object.entries(routes)) {
      if (pid === mid) routes[ext] = target;
    }
    // Tray/paper overrides are bin/media names specific to one model — they
    // don't transfer to a different device, so drop them rather than carry
    // stale names forward.
    const overrides = { ...(prefs.overrides ?? {}) };
    delete overrides[mid];
    const next: PrinterPrefs = { ...prefs, routes, overrides };
    if (prefs.default_printer === mid) next.default_printer = target;
    setReassignTarget(null);
    await savePrefs(next, t('toast.referencesUpdated', { name: targetName }));
  };

  // Drop every reference to a missing printer without reassigning.
  const handleForgetMissing = async (mid: string) => {
    const routes = { ...(prefs.routes ?? {}) };
    for (const [ext, pid] of Object.entries(routes)) {
      if (pid === mid) delete routes[ext];
    }
    const overrides = { ...(prefs.overrides ?? {}) };
    delete overrides[mid];
    const next: PrinterPrefs = { ...prefs, routes, overrides };
    if (prefs.default_printer === mid) next.default_printer = null;
    await savePrefs(next, t('toast.referencesRemoved'));
  };

  // Keep the missing printer's jobs paused; hide the banner for this session
  // without touching prefs.
  const handleDismissMissing = (mid: string) => {
    setReassignTarget(null);
    setDismissedMissing((cur) => new Set(cur).add(mid));
  };

  return (
    <>
      {topBar}
      <div
        style={{ flex: 1, display: 'flex', minHeight: 0, position: 'relative' }}
      >
        {/* Master list */}
        <div
          className="mp-scroll"
          style={{
            // 264 px on a wide window, down to 220 px at the 880 px minimum.
            width: 'clamp(220px, 24vw, 264px)',
            flexShrink: 0,
            overflow: 'auto',
            borderRight: '1px solid var(--mp-border)',
            padding: '10px 8px',
            background: 'var(--mp-surface2)',
          }}
        >
          {printers.map((p) => {
            const active = p.id === sel.id;
            const pState = stateOf(p.id);
            return (
              <button
                key={p.id}
                onClick={() => setSelectedId(p.id)}
                // NFR-045: the row on show in the detail pane is not signalled
                // by its background alone.
                {...(active ? { 'aria-current': true } : {})}
                className="mp-focus"
                style={{
                  width: '100%',
                  display: 'flex',
                  alignItems: 'center',
                  gap: 10,
                  padding: '9px 10px',
                  border: 'none',
                  background: active ? 'var(--mp-surface)' : 'transparent',
                  borderRadius: 6,
                  cursor: 'pointer',
                  textAlign: 'left',
                  marginBottom: 2,
                  opacity: pState === 'disabled' ? 0.5 : 1,
                  boxShadow: active
                    ? '0 1px 2px rgba(0,0,0,0.04), 0 0 0 1px var(--mp-border)'
                    : 'none',
                }}
              >
                <div
                  style={{
                    width: 30,
                    height: 30,
                    borderRadius: 6,
                    background: 'var(--mp-surface2)',
                    border: '1px solid var(--mp-border)',
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    flexShrink: 0,
                  }}
                >
                  <Icon
                    name={TYPE_ICON[p.type]}
                    size={15}
                    color="var(--mp-textMuted)"
                  />
                </div>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div
                    style={{ display: 'flex', alignItems: 'center', gap: 6 }}
                  >
                    <span
                      title={p.name}
                      style={{
                        fontSize: 12.5,
                        fontWeight: 500,
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                        whiteSpace: 'nowrap',
                        minWidth: 0,
                      }}
                    >
                      {p.name}
                    </span>
                    {pState === 'default' && (
                      <Icon name="star" size={11} color="var(--mp-accent)" />
                    )}
                  </div>
                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 5,
                      marginTop: 2,
                    }}
                  >
                    <StatusDot status={STATUS_META[p.status].dot} />
                    <span
                      className="mp-mono"
                      style={{
                        fontSize: 10,
                        color: 'var(--mp-textDim)',
                        flex: 1,
                        minWidth: 0,
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                        whiteSpace: 'nowrap',
                      }}
                    >
                      {t('header.listEntry', {
                        status: t(`status.${p.status}`),
                        type: t(`type.${p.type}`),
                      })}
                    </span>
                    <MPBadge tone={STATE_TONE[pState]}>
                      {t(`state.${pState}`)}
                    </MPBadge>
                  </div>
                </div>
              </button>
            );
          })}
          <button
            onClick={() => setMode('add-network')}
            className="mp-focus"
            style={{
              width: '100%',
              display: 'flex',
              alignItems: 'center',
              gap: 8,
              padding: '9px 10px',
              border: '1px dashed var(--mp-borderStrong)',
              background: 'transparent',
              borderRadius: 6,
              cursor: 'pointer',
              marginTop: 6,
              color: 'var(--mp-textMuted)',
              fontSize: 12.5,
            }}
          >
            <Icon name="plus" size={14} /> {t('actions.addAPrinter')}
          </button>
        </div>

        {/* Detail */}
        <div
          className="mp-scroll"
          style={{ flex: 1, overflow: 'auto', padding: '18px 22px' }}
        >
          {/* Header */}
          <div
            style={{
              display: 'flex',
              alignItems: 'flex-start',
              justifyContent: 'space-between',
              flexWrap: 'wrap',
              marginBottom: 16,
              gap: 12,
            }}
          >
            <div
              style={{
                display: 'flex',
                gap: 12,
                minWidth: 0,
                flex: '1 1 260px',
              }}
            >
              <div
                style={{
                  width: 44,
                  height: 44,
                  borderRadius: 9,
                  background: 'var(--mp-surface2)',
                  border: '1px solid var(--mp-border)',
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'center',
                  flexShrink: 0,
                }}
              >
                <Icon
                  name={TYPE_ICON[sel.type]}
                  size={22}
                  color="var(--mp-textMuted)"
                />
              </div>
              <div style={{ minWidth: 0 }}>
                <div
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    gap: 8,
                    flexWrap: 'wrap',
                  }}
                >
                  <h2
                    style={{
                      margin: 0,
                      fontSize: 17,
                      fontWeight: 600,
                      letterSpacing: '-0.01em',
                      minWidth: 0,
                      overflowWrap: 'anywhere',
                    }}
                  >
                    {sel.name}
                  </h2>
                  <MPBadge
                    tone={sm.tone}
                    icon={
                      sm.dot === 'done'
                        ? 'check'
                        : sm.dot === 'failed'
                          ? 'alert'
                          : undefined
                    }
                  >
                    {t(`status.${sel.status}`)}
                  </MPBadge>
                  {isDefault && (
                    <MPBadge tone="accent" icon="star">
                      {t('header.defaultBadge')}
                    </MPBadge>
                  )}
                </div>
                <div
                  className="mp-mono"
                  style={{
                    fontSize: 11,
                    color: 'var(--mp-textDim)',
                    marginTop: 3,
                  }}
                >
                  {printerMeta(t, sel)}
                </div>
              </div>
            </div>
            <div
              style={{
                display: 'flex',
                flexWrap: 'wrap',
                gap: 6,
                flexShrink: 0,
                maxWidth: '100%',
              }}
            >
              <MPButton
                variant="ghost"
                icon="printer"
                size="sm"
                disabled={busy || isDisabled}
                onClick={handleTestPage}
              >
                {t('actions.testPage')}
              </MPButton>
              <span
                style={{ display: 'inline-flex' }}
                title={
                  policyLocked
                    ? t('tooltip.policyLocked')
                    : isDefault
                      ? t('tooltip.alreadyDefault')
                      : isDisabled
                        ? t('tooltip.restoreFirst')
                        : undefined
                }
              >
                <MPButton
                  variant="ghost"
                  icon="star"
                  size="sm"
                  disabled={busy || isDefault || isDisabled || policyLocked}
                  onClick={() => void handleSetDefault()}
                >
                  {t('actions.setDefault')}
                </MPButton>
              </span>
              {isDisabled ? (
                <MPButton
                  variant="secondary"
                  icon="rotate"
                  size="sm"
                  onClick={() => void handleToggleDisabled(false)}
                >
                  {t('actions.restore')}
                </MPButton>
              ) : sel.type !== 'virtual' ? (
                <MPButton
                  variant="danger"
                  icon="trash"
                  size="sm"
                  onClick={() => void handleToggleDisabled(true)}
                >
                  {t('actions.remove')}
                </MPButton>
              ) : null}
            </div>
          </div>

          {/* Missing-printer reconciliation banner */}
          {missingId && (
            <div
              style={{
                display: 'flex',
                gap: 10,
                padding: '12px 14px',
                background: 'var(--mp-warnSoft)',
                border: '1px solid var(--mp-warn)',
                borderRadius: 8,
                marginBottom: 18,
              }}
            >
              <Icon name="alert" size={16} color="var(--mp-warn)" />
              <div style={{ flex: 1 }}>
                <div
                  style={{
                    fontSize: 12.5,
                    fontWeight: 500,
                    color: 'var(--mp-warn)',
                  }}
                >
                  {t('missing.title')}
                </div>
                <div
                  style={{
                    fontSize: 12,
                    color: 'var(--mp-text)',
                    marginTop: 3,
                    lineHeight: 1.5,
                  }}
                >
                  {/* The dispatcher pauses a job whose printer the spooler no
                      longer enumerates (PrintError::PrinterMissing is not
                      terminal), so the copy says paused — not failed. */}
                  <RichText
                    template={t('missing.body')}
                    slots={{
                      id: <Mono>{missingId}</Mono>,
                      outcome: <strong>{t('missing.bodyOutcome')}</strong>,
                    }}
                  />
                  {missingIds.length > 1 && (
                    <> {t('missing.more', { count: missingIds.length - 1 })}</>
                  )}
                </div>
                <div
                  style={{
                    display: 'flex',
                    gap: 6,
                    marginTop: 10,
                    alignItems: 'center',
                    flexWrap: 'wrap',
                  }}
                >
                  <span
                    style={{ fontSize: 11.5, color: 'var(--mp-textMuted)' }}
                  >
                    {t('missing.reassignTo')}
                  </span>
                  <MPSelect
                    size="sm"
                    aria-label={t('missing.reassignLabel')}
                    value={reassignEffective}
                    onChange={(e) => setReassignTarget(e.target.value)}
                    options={reassignCandidates.map((p) => ({
                      value: p.id,
                      label: p.name,
                    }))}
                  />
                  <MPButton
                    variant="primary"
                    size="sm"
                    disabled={!reassignEffective}
                    onClick={() =>
                      void handleReassignMissing(missingId, reassignEffective)
                    }
                  >
                    {t('missing.update')}
                  </MPButton>
                  <MPButton
                    variant="ghost"
                    size="sm"
                    onClick={() => void handleForgetMissing(missingId)}
                  >
                    {t('missing.forget')}
                  </MPButton>
                  <MPButton
                    variant="ghost"
                    size="sm"
                    onClick={() => handleDismissMissing(missingId)}
                  >
                    {t('missing.keepPaused')}
                  </MPButton>
                </div>
              </div>
            </div>
          )}

          {/* Org-managed default note (policy.json) */}
          {policyLocked && (
            <div
              style={{
                display: 'flex',
                gap: 8,
                padding: '9px 12px',
                background: 'var(--mp-surface2)',
                border: '1px solid var(--mp-border)',
                borderRadius: 6,
                marginBottom: 18,
                alignItems: 'center',
              }}
            >
              <Icon name="lock" size={13} color="var(--mp-textMuted)" />
              <span style={{ fontSize: 11.5, color: 'var(--mp-textMuted)' }}>
                {lockedName ? (
                  <RichText
                    template={t('policy.managed')}
                    slots={{
                      name: <strong>{lockedName}</strong>,
                      file: <Mono>policy.json</Mono>,
                    }}
                  />
                ) : (
                  <RichText
                    template={t('policy.managedMissing')}
                    slots={{
                      id: <Mono>{policy?.default_printer}</Mono>,
                      file: <Mono>policy.json</Mono>,
                    }}
                  />
                )}
              </span>
            </div>
          )}

          <div
            style={{
              display: 'grid',
              gridTemplateColumns: 'repeat(auto-fit, minmax(280px, 1fr))',
              gap: 24,
            }}
          >
            {/* Left column */}
            <div>
              <Block label={t('connection.label')}>
                <Row label={t('connection.driver')}>
                  <span className="mp-mono" style={{ fontSize: 11.5 }}>
                    {sel.driver}
                  </span>
                </Row>
                <Row label={t('connection.port')}>
                  <span className="mp-mono" style={{ fontSize: 11.5 }}>
                    {sel.conn}
                  </span>
                </Row>
                {/* Most spoolers leave location empty; an empty cell reads as a
                    rendering bug, so say the field was not reported. */}
                <Row label={t('connection.location')}>
                  {sel.location.trim() || (
                    <span style={{ color: 'var(--mp-textDim)' }}>
                      {t('connection.notReported')}
                    </span>
                  )}
                </Row>
                <Row label={t('connection.shared')}>
                  {sel.shared
                    ? t('connection.sharedYes')
                    : t('connection.sharedNo')}
                </Row>
                {/* Many drivers omit the resolution list; the enumeration
                    contract says to HIDE the row rather than print " dpi". */}
                {sel.maxDpi != null && (
                  <Row label={t('connection.maxResolution')}>
                    <span className="mp-mono" style={{ fontSize: 11.5 }}>
                      {t('connection.dpi', { dpi: sel.maxDpi })}
                    </span>
                  </Row>
                )}
              </Block>

              <Block label={t('defaults.label')}>
                <div
                  style={{
                    display: 'grid',
                    gridTemplateColumns: '1fr 1fr',
                    gap: 10,
                  }}
                >
                  <OverrideSelect
                    label={t('defaults.tray')}
                    empty={t('defaults.noTrays')}
                    placeholder={t('defaults.printerDefault')}
                    driverDefault={sel.defTray}
                    value={selOverride?.tray ?? ''}
                    options={sel.trays.map((tray) => tray.name)}
                    onChange={(v) => void handleOverride('tray', v)}
                  />
                  <OverrideSelect
                    label={t('defaults.paper')}
                    empty={t('defaults.noPapers')}
                    placeholder={t('defaults.printerDefault')}
                    driverDefault={sel.defPaper}
                    value={selOverride?.paper ?? ''}
                    options={sel.papers}
                    onChange={(v) => void handleOverride('paper', v)}
                  />
                </div>
              </Block>

              <Block label={t('routes.label')}>
                <div
                  style={{
                    display: 'flex',
                    flexWrap: 'wrap',
                    gap: 6,
                    alignItems: 'center',
                  }}
                >
                  {selRoutes.map((x) => (
                    <span
                      key={x}
                      className="mp-mono"
                      style={{
                        fontSize: 11,
                        padding: '3px 8px',
                        background: 'var(--mp-accentSoft)',
                        color: 'var(--mp-accent)',
                        borderRadius: 5,
                        display: 'inline-flex',
                        alignItems: 'center',
                        gap: 4,
                      }}
                    >
                      {x}
                      <button
                        type="button"
                        aria-label={t('routes.removeRule', { ext: x })}
                        className="mp-focus"
                        onClick={() => void handleRemoveRule(x)}
                        style={{
                          display: 'inline-flex',
                          background: 'none',
                          border: 'none',
                          padding: 0,
                          cursor: 'pointer',
                          color: 'var(--mp-accent)',
                        }}
                      >
                        <Icon name="x" size={9} />
                      </button>
                    </span>
                  ))}
                  {ruleDraft !== null ? (
                    <input
                      autoFocus
                      className="mp-mono mp-focus"
                      aria-label={t('routes.label')}
                      placeholder={t('routes.placeholder')}
                      value={ruleDraft}
                      onChange={(e) => setRuleDraft(e.target.value)}
                      onBlur={() => void handleAddRule(ruleDraft)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') void handleAddRule(ruleDraft);
                        if (e.key === 'Escape') setRuleDraft(null);
                      }}
                      style={{
                        fontSize: 11,
                        width: 64,
                        padding: '3px 8px',
                        borderRadius: 5,
                        border: '1px solid var(--mp-accent)',
                        background: 'var(--mp-surface)',
                        color: 'var(--mp-text)',
                        outline: 'none',
                      }}
                    />
                  ) : (
                    <button
                      type="button"
                      className="mp-focus"
                      onClick={() => setRuleDraft('')}
                      style={{
                        fontSize: 11,
                        padding: '3px 8px',
                        background: 'transparent',
                        border: '1px dashed var(--mp-borderStrong)',
                        borderRadius: 5,
                        color: 'var(--mp-textMuted)',
                        cursor: 'pointer',
                        display: 'inline-flex',
                        alignItems: 'center',
                        gap: 4,
                      }}
                    >
                      <Icon name="plus" size={10} /> {t('routes.addRule')}
                    </button>
                  )}
                </div>
                <div
                  style={{
                    fontSize: 11,
                    color: 'var(--mp-textDim)',
                    marginTop: 8,
                    lineHeight: 1.5,
                  }}
                >
                  {t('routes.hint')}
                </div>
                {isDisabled && selRoutes.length > 0 && (
                  <div
                    style={{
                      fontSize: 11,
                      color: 'var(--mp-warn)',
                      marginTop: 6,
                      lineHeight: 1.5,
                    }}
                  >
                    {t('routes.skippedWhileRemoved')}
                  </div>
                )}
              </Block>
            </div>

            {/* Right column */}
            <div>
              <Block
                label={t('capabilities.label')}
                action={
                  <span
                    className="mp-mono"
                    style={{ fontSize: 9.5, color: 'var(--mp-textDim)' }}
                  >
                    {t('capabilities.source')}
                  </span>
                }
              >
                <div
                  style={{
                    display: 'flex',
                    flexWrap: 'wrap',
                    gap: 6,
                    marginBottom: 12,
                  }}
                >
                  <CapPill ok={sel.duplex} unknown={sel.duplexUnknown === true}>
                    {sel.duplexUnknown
                      ? t('capabilities.duplexUnknown')
                      : sel.duplex
                        ? t('capabilities.duplex')
                        : t('capabilities.duplexUnavailable')}
                  </CapPill>
                  <CapPill ok={sel.color} unknown={sel.colorUnknown === true}>
                    {sel.colorUnknown
                      ? t('capabilities.colorUnknown')
                      : sel.color
                        ? t('capabilities.color')
                        : t('capabilities.monoOnly')}
                  </CapPill>
                  {/* A driver that reports nothing is not a capability met:
                      "0 paper sizes" used to carry an affirmative check. */}
                  <CapPill ok={sel.papers.length > 0}>
                    {t('capabilities.papers', { count: sel.papers.length })}
                  </CapPill>
                  <CapPill ok={sel.trays.length > 0}>
                    {t('capabilities.trays', { count: sel.trays.length })}
                  </CapPill>
                </div>
                {!sel.duplex && !sel.duplexUnknown && (
                  <div
                    style={{
                      display: 'flex',
                      gap: 7,
                      padding: '8px 10px',
                      background: 'var(--mp-warnSoft)',
                      borderRadius: 6,
                      marginBottom: 8,
                    }}
                  >
                    <Icon name="info" size={12} color="var(--mp-warn)" />
                    <span
                      style={{
                        fontSize: 11,
                        color: 'var(--mp-warn)',
                        lineHeight: 1.45,
                      }}
                    >
                      {t('capabilities.duplexWarning')}
                    </span>
                  </div>
                )}
                <div style={{ display: 'flex', flexWrap: 'wrap', gap: 5 }}>
                  {sel.papers.map((p) => (
                    <span
                      key={p}
                      className="mp-mono"
                      style={{
                        fontSize: 10.5,
                        padding: '3px 7px',
                        background: 'var(--mp-surface2)',
                        border: '1px solid var(--mp-border)',
                        borderRadius: 4,
                        color: 'var(--mp-textMuted)',
                      }}
                    >
                      {p}
                    </span>
                  ))}
                </div>
              </Block>

              {Object.keys(sel.levels).length > 0 && (
                <Block label={t('supplies.label')}>
                  <div
                    style={{
                      display: 'flex',
                      flexDirection: 'column',
                      gap: 8,
                      padding: '4px 0',
                    }}
                  >
                    {Object.entries(sel.levels).map(([k, v]) => (
                      <LevelBar
                        key={k}
                        label={supplyLabel(k)}
                        value={v ?? 0}
                        color={TONER_COLORS[k] ?? 'var(--mp-text)'}
                      />
                    ))}
                  </div>
                </Block>
              )}

              {/* Hidden, like Supply levels, when the driver reports no trays. */}
              {sel.trays.length > 0 && (
                <Block label={t('trays.label')}>
                  <MPCard padding={0}>
                    {sel.trays.map((tray, i) => (
                      <div
                        key={tray.name}
                        style={{
                          display: 'flex',
                          alignItems: 'center',
                          gap: 10,
                          padding: '9px 12px',
                          borderBottom:
                            i < sel.trays.length - 1
                              ? '1px solid var(--mp-border)'
                              : 'none',
                        }}
                      >
                        <Icon name="file" size={13} color="var(--mp-textDim)" />
                        <span
                          style={{ fontSize: 12, fontWeight: 500, width: 64 }}
                        >
                          {tray.name}
                        </span>
                        <span
                          className="mp-mono"
                          style={{
                            fontSize: 11,
                            color: 'var(--mp-textMuted)',
                            flex: 1,
                          }}
                        >
                          {tray.paper ?? t('trays.unknownPaper')}
                        </span>
                        {tray.level != null && (
                          <div
                            style={{
                              width: 46,
                              height: 5,
                              background: 'var(--mp-surface3)',
                              borderRadius: 3,
                              overflow: 'hidden',
                            }}
                          >
                            <div
                              style={{
                                height: '100%',
                                width: `${tray.level * 100}%`,
                                background:
                                  tray.level < 0.2
                                    ? 'var(--mp-error)'
                                    : 'var(--mp-success)',
                              }}
                            />
                          </div>
                        )}
                      </div>
                    ))}
                  </MPCard>
                </Block>
              )}
            </div>
          </div>
        </div>

        {/* Add-printer modal */}
        {(mode === 'add-network' ||
          mode === 'add-ip' ||
          mode === 'add-system') && (
          <AddPrinterModal
            mode={mode}
            onMode={setMode}
            os={os}
            onOpenSettings={() => void handleOpenSettings()}
            onAdded={() => {
              setMode('default');
              void load();
            }}
            printers={printers}
            prefs={prefs}
            removal={{ policyId, osDefaultId: defaultId }}
            onSavePrefs={savePrefs}
            onConfirm={setConfirmState}
          />
        )}

        {confirmState && (
          <ConfirmDialog
            state={confirmState}
            onClose={() => setConfirmState(null)}
          />
        )}
      </div>
    </>
  );
}

/**
 * One "Defaults for this printer" select.
 *
 * The first option is always **Printer default** with an empty value, so an
 * override can be cleared again — without it the control was write-once, and a
 * printer whose driver reports no default rendered a blank, value-less select.
 * When the driver reports nothing selectable at all the select is replaced by a
 * note, because an empty dropdown is a dead control.
 */
function OverrideSelect({
  label,
  empty,
  placeholder,
  driverDefault,
  value,
  options,
  onChange,
}: {
  label: string;
  empty: string;
  placeholder: string;
  driverDefault: string | undefined;
  value: string;
  options: string[];
  onChange: (value: string) => void;
}) {
  return (
    <div>
      <div
        style={{
          fontSize: 11,
          color: 'var(--mp-textMuted)',
          marginBottom: 5,
        }}
      >
        {label}
      </div>
      {options.length === 0 ? (
        <div
          style={{ fontSize: 11, color: 'var(--mp-textDim)', lineHeight: 1.45 }}
        >
          {empty}
        </div>
      ) : (
        <MPSelect
          fullWidth
          aria-label={label}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          options={[
            {
              value: '',
              label: driverDefault
                ? `${placeholder} · ${driverDefault}`
                : placeholder,
            },
            ...options.map((o) => ({ value: o, label: o })),
          ]}
        />
      )}
    </div>
  );
}

type AddTab = 'network' | 'ip' | 'system';

interface AddPrinterModalProps {
  mode: PrinterMode;
  onMode: (m: PrinterMode) => void;
  /** Host OS from `app_info`, or null while unknown. */
  os: OsId;
  onOpenSettings: () => void;
  /** Called after a printer is installed — closes the modal + reloads. */
  onAdded: () => void;
  printers: UiPrinter[];
  prefs: PrinterPrefs;
  /** Where jobs go today (see `removalConsequences`). */
  removal: Omit<RemovalContext, 'prefs'>;
  onSavePrefs: (
    next: PrinterPrefs,
    success?: string,
  ) => void | Promise<unknown>;
  onConfirm: (state: ConfirmState) => void;
}

const ADD_TABS: { value: AddTab; mode: PrinterMode }[] = [
  { value: 'network', mode: 'add-network' },
  { value: 'ip', mode: 'add-ip' },
  { value: 'system', mode: 'add-system' },
];

const tabId = (tab: AddTab) => `mp-add-printer-tab-${tab}`;
const panelId = (tab: AddTab) => `mp-add-printer-panel-${tab}`;

/**
 * The add-printer tab strip.
 *
 * It looks like `MPSegmented` but is a real **tablist**, not a radiogroup:
 * these controls swap the visible panel rather than pick a value inside a form,
 * which is precisely the distinction between the two ARIA patterns. Each tab
 * owns its panel via `aria-controls`/`aria-labelledby`, so a screen reader
 * announces "tab 2 of 3" and can jump straight to the panel — neither of which
 * a radiogroup offers. Arrow keys move between tabs (automatic activation),
 * with a single tab stop for the whole strip.
 */
function AddPrinterTabs({
  value,
  onChange,
  options,
  label,
}: {
  value: AddTab;
  onChange: (tab: AddTab) => void;
  options: { value: AddTab; label: string }[];
  label: string;
}) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);

  const move = (from: number, delta: number) => {
    const count = options.length;
    if (count === 0) return;
    const next = (from + delta + count) % count;
    const option = options[next];
    if (!option) return;
    onChange(option.value);
    refs.current[next]?.focus();
  };

  const onKeyDown = (e: KeyboardEvent<HTMLButtonElement>, index: number) => {
    switch (e.key) {
      case 'ArrowRight':
      case 'ArrowDown':
        e.preventDefault();
        move(index, 1);
        break;
      case 'ArrowLeft':
      case 'ArrowUp':
        e.preventDefault();
        move(index, -1);
        break;
      case 'Home':
        e.preventDefault();
        move(-1, 1);
        break;
      case 'End':
        e.preventDefault();
        move(0, -1);
        break;
      default:
        break;
    }
  };

  return (
    <div
      role="tablist"
      aria-label={label}
      style={{
        display: 'inline-flex',
        padding: 2,
        height: 28,
        background: 'var(--mp-surface2)',
        border: '1px solid var(--mp-border)',
        borderRadius: 7,
      }}
    >
      {options.map((o, i) => {
        const active = o.value === value;
        return (
          <button
            key={o.value}
            type="button"
            role="tab"
            id={tabId(o.value)}
            aria-selected={active}
            aria-controls={panelId(o.value)}
            tabIndex={active ? 0 : -1}
            ref={(el) => {
              refs.current[i] = el;
            }}
            onClick={() => onChange(o.value)}
            onKeyDown={(e) => onKeyDown(e, i)}
            className="mp-focus"
            style={{
              height: 24,
              padding: '0 8px',
              fontSize: 11.5,
              fontWeight: active ? 500 : 400,
              background: active ? 'var(--mp-surface)' : 'transparent',
              color: active ? 'var(--mp-text)' : 'var(--mp-textMuted)',
              border: 'none',
              borderRadius: 5,
              cursor: 'pointer',
              boxShadow: active
                ? '0 1px 2px rgba(0,0,0,0.05), 0 0 0 1px var(--mp-border)'
                : 'none',
              whiteSpace: 'nowrap',
            }}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

function AddPrinterModal({
  mode,
  onMode,
  os,
  onOpenSettings,
  onAdded,
  printers,
  prefs,
  removal,
  onSavePrefs,
  onConfirm,
}: AddPrinterModalProps) {
  const t = useT('printers');
  const tab: AddTab =
    mode === 'add-ip' ? 'ip' : mode === 'add-system' ? 'system' : 'network';
  const dialogRef = useRef<HTMLDivElement>(null);
  const titleId = useId();

  // NFR-041/NFR-044: focus moves onto the selected tab, Tab stays inside,
  // Escape closes, and focus returns to "Add a printer" afterwards.
  useModalFocus(dialogRef, {
    onEscape: () => onMode('default'),
    initialFocusSelector: '[role="tab"][aria-selected="true"]',
  });

  return (
    <ModalOverlay>
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="mp-scroll"
        style={{
          width: 480,
          maxHeight: '88%',
          overflow: 'auto',
          background: 'var(--mp-surface)',
          border: '1px solid var(--mp-border)',
          borderRadius: 12,
          boxShadow: '0 24px 60px rgba(0,0,0,0.28)',
        }}
      >
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
            padding: '14px 16px',
            borderBottom: '1px solid var(--mp-border)',
          }}
        >
          <div id={titleId} style={{ fontSize: 14, fontWeight: 600 }}>
            {t('modal.title')}
          </div>
          <MPIconButton
            icon="x"
            size={26}
            label={t('modal.close')}
            onClick={() => onMode('default')}
          />
        </div>

        <div style={{ padding: '14px 16px' }}>
          <div style={{ marginBottom: 14 }}>
            <AddPrinterTabs
              label={t('modal.tabsLabel')}
              value={tab}
              onChange={(next) =>
                onMode(
                  ADD_TABS.find((x) => x.value === next)?.mode ?? 'add-network',
                )
              }
              options={ADD_TABS.map((x) => ({
                value: x.value,
                label: t(`modal.tabs.${x.value}`),
              }))}
            />
          </div>

          <div
            role="tabpanel"
            id={panelId(tab)}
            aria-labelledby={tabId(tab)}
            tabIndex={0}
          >
            {tab === 'network' && <NetworkTab onAdded={onAdded} os={os} />}
            {tab === 'ip' && <IpTab onAdded={onAdded} os={os} />}
            {tab === 'system' && (
              <SystemTab
                printers={printers}
                prefs={prefs}
                os={os}
                removal={removal}
                onSavePrefs={onSavePrefs}
                onConfirm={onConfirm}
              />
            )}
          </div>
        </div>

        <div
          style={{
            display: 'flex',
            justifyContent: 'space-between',
            alignItems: 'center',
            gap: 8,
            padding: '12px 16px',
            borderTop: '1px solid var(--mp-border)',
          }}
        >
          <MPButton variant="ghost" icon="openwith" onClick={onOpenSettings}>
            {t('modal.openSettings')}
          </MPButton>
          <MPButton variant="secondary" onClick={() => onMode('default')}>
            {tab === 'system' ? t('modal.done') : t('modal.close')}
          </MPButton>
        </div>
      </div>
    </ModalOverlay>
  );
}

/** Label + control stacked vertically, reused by the modal forms. */
function ModalField({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  return (
    <div>
      <div
        style={{
          fontSize: 11,
          color: 'var(--mp-textMuted)',
          marginBottom: 5,
        }}
      >
        {label}
      </div>
      {children}
    </div>
  );
}

/** Inline error text shown under a control or row after a failed add. */
function InlineError({ children, id }: { children: ReactNode; id?: string }) {
  return (
    <div
      {...(id !== undefined ? { id } : {})}
      style={{
        display: 'flex',
        gap: 6,
        alignItems: 'flex-start',
        fontSize: 11,
        color: 'var(--mp-error)',
        lineHeight: 1.45,
      }}
    >
      <Icon name="alert" size={12} color="var(--mp-error)" />
      <span style={{ flex: 1 }}>{children}</span>
    </div>
  );
}

/**
 * "Type · connection · N jobs in its queue" for the detail header. The
 * connection is left out when it only repeats the type (a virtual printer
 * reports "Virtual" for both).
 */
function printerMeta(t: T, p: UiPrinter): string {
  const type = t(`type.${p.type}`);
  const queue = t('header.queued', { count: p.queue });
  const conn = p.conn.trim();
  return conn === '' || conn.toLowerCase() === type.toLowerCase()
    ? t('header.metaNoConn', { type, queue })
    : t('header.meta', { type, conn, queue });
}

/** Emphasised reference to another control, used inside translated prose. */
function Ref({ children }: { children: ReactNode }) {
  return <strong>{children}</strong>;
}

/** "Scan network" tab — mDNS/Bonjour discovery with a per-row Add action. */
function NetworkTab({ onAdded, os }: { onAdded: () => void; os: OsId }) {
  const t = useT('printers');
  // Windows has no WSD/mDNS backend yet (documented follow-up): the command
  // returns an empty list there, which the UI used to paint as "no printers
  // found on the network" after a fake spinner. Say what is actually true and
  // don't call the command at all.
  const unsupported = os === 'windows';
  // null = still scanning; discovery re-runs each time this tab mounts.
  const [discovered, setDiscovered] = useState<UiDiscoveredPrinter[] | null>(
    unsupported ? [] : null,
  );
  const [addingId, setAddingId] = useState<string | null>(null);
  const [rowError, setRowError] = useState<Record<string, string>>({});
  const [scanError, setScanError] = useState<string | null>(null);
  // The OS can be learnt after the tab mounted: start over, adjusted while
  // rendering, whenever that changes whether there is anything to scan.
  const [scanFor, setScanFor] = useState(unsupported);
  if (scanFor !== unsupported) {
    setScanFor(unsupported);
    setDiscovered(unsupported ? [] : null);
    setScanError(null);
  }

  useEffect(() => {
    if (unsupported) return;
    let alive = true;
    discoverPrinters()
      .then((d) => {
        if (alive) setDiscovered(d);
      })
      .catch((e) => {
        // A failed scan is not an empty network — say so, don't render the
        // "no new printers" copy over a real error.
        if (alive) {
          setDiscovered([]);
          setScanError(errText(e));
        }
      });
    return () => {
      alive = false;
    };
  }, [unsupported]);

  const handleAdd = async (d: UiDiscoveredPrinter) => {
    setAddingId(d.id);
    setRowError((e) => {
      const next = { ...e };
      delete next[d.id];
      return next;
    });
    const tid = toast.loading(t('toast.adding', { name: d.name }));
    try {
      await addPrinter(d.name, d.id);
      toast.success(t('toast.added', { name: d.name }), { id: tid });
      onAdded();
    } catch (err) {
      const msg = errText(err);
      setRowError((e) => ({ ...e, [d.id]: msg }));
      toast.error(msg, { id: tid, description: errorTechnical(err) });
    } finally {
      setAddingId(null);
    }
  };

  const slots = {
    byIp: <Ref>{t('modal.tabs.ip')}</Ref>,
    openSettings: <Ref>{t('modal.openSettings')}</Ref>,
  };

  if (unsupported) {
    return (
      <div
        style={{
          fontSize: 12,
          color: 'var(--mp-textMuted)',
          lineHeight: 1.5,
          padding: '8px 2px',
        }}
      >
        <RichText template={t('network.unsupported')} slots={slots} />
      </div>
    );
  }

  return (
    <>
      <div
        style={{
          fontSize: 12,
          color: 'var(--mp-textMuted)',
          marginBottom: 12,
          lineHeight: 1.5,
        }}
      >
        <RichText template={t('network.intro')} slots={slots} />
      </div>

      {discovered === null && (
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 8,
            marginBottom: 10,
          }}
        >
          <div
            style={{
              width: 14,
              height: 14,
              border: '2px solid var(--mp-surface3)',
              borderTopColor: 'var(--mp-accent)',
              borderRadius: '50%',
              animation: 'mp-spin 0.9s linear infinite',
            }}
          />
          <span style={{ fontSize: 12, color: 'var(--mp-textMuted)' }}>
            {t('network.scanning')}
          </span>
        </div>
      )}
      {discovered !== null && discovered.length === 0 && (
        <div
          style={{
            fontSize: 12,
            color: scanError ? 'var(--mp-error)' : 'var(--mp-textMuted)',
            padding: '8px 2px',
          }}
        >
          {scanError
            ? t('network.failed', { error: scanError })
            : t('network.empty')}
        </div>
      )}
      {discovered !== null && discovered.length > 0 && (
        <MPCard padding={0}>
          {discovered.map((d, i) => (
            <div
              key={d.id}
              style={{
                borderBottom:
                  i < discovered.length - 1
                    ? '1px solid var(--mp-border)'
                    : 'none',
              }}
            >
              <div
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 10,
                  padding: '11px 12px',
                }}
              >
                <div
                  style={{
                    width: 30,
                    height: 30,
                    borderRadius: 6,
                    background: 'var(--mp-surface2)',
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                  }}
                >
                  <Icon
                    name={TYPE_ICON[d.type]}
                    size={15}
                    color="var(--mp-textMuted)"
                  />
                </div>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ fontSize: 12.5, fontWeight: 500 }}>
                    {d.name}
                  </div>
                  <div
                    className="mp-mono"
                    style={{ fontSize: 10.5, color: 'var(--mp-textDim)' }}
                  >
                    {d.conn} · {d.via}
                  </div>
                </div>
                <MPButton
                  variant="secondary"
                  size="sm"
                  icon="plus"
                  // Any in-flight add disables every row: clicking a second
                  // row mid-add would race two lpadmin/Add-Printer calls.
                  disabled={addingId !== null}
                  onClick={() => void handleAdd(d)}
                >
                  {addingId === d.id ? t('network.adding') : t('network.add')}
                </MPButton>
              </div>
              {rowError[d.id] && (
                <div style={{ padding: '0 12px 10px 52px' }}>
                  <InlineError>{rowError[d.id]}</InlineError>
                </div>
              )}
            </div>
          ))}
        </MPCard>
      )}
    </>
  );
}

/**
 * Protocol options for the add-by-IP form.
 *
 * Windows can only install driverless IPP/IPPS queues via the Microsoft IPP
 * Class Driver; raw socket / LPD / USB devices must go through the OS pane, and
 * the backend rejects them outright. While the OS is still unknown the full
 * list is offered — the backend is the authority and refuses with a readable
 * message either way.
 */
function protoOptions(t: T, isWindows: boolean): SelectOption[] {
  const base: SelectOption[] = [
    { value: 'ipp', label: t('ip.proto.ipp') },
    { value: 'ipps', label: t('ip.proto.ipps') },
  ];
  return isWindows
    ? base
    : [
        ...base,
        { value: 'socket', label: t('ip.proto.socket') },
        { value: 'lpd', label: t('ip.proto.lpd') },
      ];
}

/** "By IP address" tab — compose a device URI and install it. */
function IpTab({ onAdded, os }: { onAdded: () => void; os: OsId }) {
  const t = useT('printers');
  const isWindows = os === 'windows';
  const [host, setHost] = useState('');
  const [proto, setProto] = useState<PrinterProtocol>('ipp');
  const [port, setPort] = useState(String(DEFAULT_PORTS.ipp));
  const [queuePath, setQueuePath] = useState('');
  const [name, setName] = useState('');
  const [nameEdited, setNameEdited] = useState(false);
  const [busy, setBusy] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // The display name tracks the host until the user types their own.
  const effectiveName = nameEdited ? name : host;
  const hostOk = isValidHost(host);
  // Blank means "the protocol's default port"; anything typed must be 1–65535.
  const portOk = port.trim() === '' || isValidPort(port);
  const nameOk = isValidDisplayName(effectiveName);
  const canConnect = !busy && hostOk && portOk && nameOk;
  const hasQueuePath = proto === 'ipp' || proto === 'ipps';
  const hostInvalid = host !== '' && !hostOk;
  const hostErrorId = useId();
  const portErrorId = useId();

  const onProtoChange = (value: string) => {
    const p = value as PrinterProtocol;
    setProto(p);
    setPort(String(DEFAULT_PORTS[p]));
  };

  const handleConnect = async () => {
    const finalName = effectiveName.trim();
    if (!hostOk || !portOk || !nameOk) return;
    const portArg = port.trim() === '' ? undefined : Number(port.trim());
    const uri = buildPrinterUri(proto, host, portArg, queuePath);
    setBusy(true);
    setErrorMsg(null);
    const tid = toast.loading(t('toast.adding', { name: finalName }));
    try {
      await addPrinter(finalName, uri);
      toast.success(t('toast.added', { name: finalName }), { id: tid });
      onAdded();
    } catch (err) {
      const msg = errText(err);
      setErrorMsg(msg);
      toast.error(msg, { id: tid, description: errorTechnical(err) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
      <ModalField label={t('ip.host')}>
        <MPInput
          fullWidth
          aria-label={t('ip.host')}
          value={host}
          onChange={(e) => setHost(e.target.value)}
          placeholder={t('ip.hostPlaceholder')}
          error={hostInvalid}
          aria-describedby={hostInvalid ? hostErrorId : undefined}
        />
        {hostInvalid && (
          <div style={{ marginTop: 6 }}>
            <InlineError id={hostErrorId}>{t('ip.hostInvalid')}</InlineError>
          </div>
        )}
      </ModalField>

      <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 10 }}>
        <ModalField label={t('ip.protocol')}>
          <MPSelect
            fullWidth
            aria-label={t('ip.protocol')}
            value={proto}
            onChange={(e) => onProtoChange(e.target.value)}
            options={protoOptions(t, isWindows)}
          />
        </ModalField>
        <ModalField label={t('ip.port')}>
          <MPInput
            fullWidth
            type="number"
            min={MIN_PORT}
            max={MAX_PORT}
            aria-label={t('ip.port')}
            value={port}
            onChange={(e) => setPort(e.target.value)}
            error={!portOk}
            aria-describedby={portOk ? undefined : portErrorId}
          />
        </ModalField>
      </div>

      {isWindows && (
        <div
          style={{
            display: 'flex',
            gap: 7,
            padding: '8px 10px',
            background: 'var(--mp-surface2)',
            border: '1px solid var(--mp-border)',
            borderRadius: 6,
          }}
        >
          <Icon name="info" size={12} color="var(--mp-textMuted)" />
          <span
            style={{
              fontSize: 11,
              color: 'var(--mp-textMuted)',
              lineHeight: 1.45,
            }}
          >
            <RichText
              template={t('ip.windowsNote')}
              slots={{ openSettings: <Ref>{t('modal.openSettings')}</Ref> }}
            />
          </span>
        </div>
      )}

      <ModalField label={t('ip.queuePath')}>
        <MPInput
          fullWidth
          mono
          aria-label={t('ip.queuePath')}
          value={queuePath}
          onChange={(e) => setQueuePath(e.target.value)}
          placeholder={
            hasQueuePath ? t('ip.queuePathPlaceholder') : t('ip.queuePathNone')
          }
        />
      </ModalField>

      <ModalField label={t('ip.displayName')}>
        <MPInput
          fullWidth
          aria-label={t('ip.displayName')}
          value={effectiveName}
          onChange={(e) => {
            setNameEdited(true);
            setName(e.target.value);
          }}
          placeholder={t('ip.displayNamePlaceholder')}
          error={effectiveName !== '' && !nameOk}
        />
      </ModalField>

      {!portOk && (
        <InlineError id={portErrorId}>{t('ip.portInvalid')}</InlineError>
      )}
      {errorMsg && <InlineError>{errorMsg}</InlineError>}

      <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
        <MPButton
          variant="primary"
          icon="plus"
          disabled={!canConnect}
          onClick={() => void handleConnect()}
        >
          {busy ? t('ip.connecting') : t('ip.connect')}
        </MPButton>
      </div>
    </div>
  );
}

/** Who the jobs on "System default" go to, and which printers rules target. */
interface RemovalContext {
  prefs: PrinterPrefs;
  /** The organization policy's default (resolved to a queue id), if any. */
  policyId: string | null;
  /** The OS default printer's id, if any. */
  osDefaultId: string | null;
}

/**
 * What removing printer `id` from Pluryprint does to jobs, one sentence per
 * consequence; empty when nothing depends on it. The single source for both
 * places a printer can be removed (the detail header and Add printer → From
 * system), so neither can skip the question.
 *
 * Jobs on "System default" go to the organization policy's default when there
 * is one (it replaces the app default, as on the print path), else to
 * Pluryprint's own default, else to the OS default.
 */
function removalConsequences(
  id: string,
  { prefs, policyId, osDefaultId }: RemovalContext,
  t: TFunction<'printers'>,
): string[] {
  const routes = Object.entries(prefs.routes ?? {})
    .filter(([, pid]) => pid === id)
    .map(([ext]) => ext)
    .sort();
  const isPolicyDefault = policyId === id;
  const isAppDefault = !policyId && prefs.default_printer === id;
  const isOsFallback =
    !policyId &&
    osDefaultId === id &&
    (!prefs.default_printer || prefs.default_printer === id);
  return [
    isPolicyDefault ? t('confirm.removePolicyDefault') : null,
    isAppDefault ? t('confirm.removeAppDefault') : null,
    routes.length > 0
      ? t('confirm.removeRoutes', {
          count: routes.length,
          exts: routes.join(', '),
        })
      : null,
    isOsFallback ? t('confirm.removeOsDefault') : null,
  ].filter((x): x is string => x !== null);
}

/**
 * `prefs` with printer `id` removed from Pluryprint. A removed printer cannot
 * be Pluryprint's default: the backend would skip it anyway, so the default
 * is cleared rather than left pointing at it — and coming back the day the
 * printer is restored, without a word.
 */
function withoutPrinter(prefs: PrinterPrefs, id: string): PrinterPrefs {
  const disabled = new Set(prefs.disabled ?? []);
  disabled.add(id);
  return {
    ...prefs,
    disabled: [...disabled],
    ...(prefs.default_printer === id ? { default_printer: null } : {}),
  };
}

/** "From system" tab — tick queues to manage in Pluryprint (soft-disable). */
function SystemTab({
  printers,
  prefs,
  os,
  removal,
  onSavePrefs,
  onConfirm,
}: {
  printers: UiPrinter[];
  prefs: PrinterPrefs;
  os: OsId;
  /** Where jobs go today, to say what unticking a printer changes. */
  removal: Omit<RemovalContext, 'prefs'>;
  onSavePrefs: (
    next: PrinterPrefs,
    success?: string,
  ) => void | Promise<unknown>;
  /** Ask before an untick that moves jobs (same question as Remove). */
  onConfirm: (state: ConfirmState) => void;
}) {
  const t = useT('printers');
  const disabled = new Set(prefs.disabled ?? []);
  // A virtual printer (Save as PDF, Microsoft Print to PDF) cannot be removed
  // from the detail view either; it is listed here only when it was removed
  // before, so it can be brought back.
  const listed = printers.filter(
    (p) => p.type !== 'virtual' || disabled.has(p.id),
  );

  const toggle = async (p: UiPrinter) => {
    if (disabled.has(p.id)) {
      const next = new Set(disabled);
      next.delete(p.id);
      await onSavePrefs(
        { ...prefs, disabled: [...next] },
        t('toast.managed', { name: p.name }),
      );
      return;
    }
    // Unticking is removing: the same consequences, the same question, and
    // the app default cleared, exactly as the detail header's Remove does.
    const hide = () =>
      onSavePrefs(
        withoutPrinter(prefs, p.id),
        t('toast.hidden', { name: p.name }),
      ) as Promise<void> | void;
    const consequences = removalConsequences(p.id, { ...removal, prefs }, t);
    if (consequences.length === 0) {
      await hide();
      return;
    }
    onConfirm({
      title: t('confirm.removeTitle', { name: p.name }),
      message: consequences.join(' '),
      confirmLabel: t('confirm.removeConfirm'),
      run: async () => {
        await hide();
      },
    });
  };

  return (
    <>
      <div
        style={{
          fontSize: 12,
          color: 'var(--mp-textMuted)',
          marginBottom: 12,
          lineHeight: 1.5,
        }}
      >
        {os
          ? t('system.intro', { os: OS_LABEL[os] })
          : t('system.introUnknownOs')}
      </div>
      <MPCard padding={0}>
        {listed.map((p, i) => (
          <div
            key={p.id}
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 10,
              padding: '10px 12px',
              borderBottom:
                i < listed.length - 1 ? '1px solid var(--mp-border)' : 'none',
            }}
          >
            <MPCheckbox
              checked={!disabled.has(p.id)}
              onChange={() => void toggle(p)}
              label={
                <span style={{ fontSize: 12.5 }}>
                  <span>{p.name}</span>
                  <span
                    className="mp-mono"
                    style={{ color: 'var(--mp-textDim)', marginLeft: 6 }}
                  >
                    · {t(`type.${p.type}`)}
                  </span>
                </span>
              }
            />
          </div>
        ))}
      </MPCard>
    </>
  );
}
