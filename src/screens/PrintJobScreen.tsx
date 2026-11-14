import {
  MPBadge,
  MPButton,
  MPCard,
  MPIconButton,
  MPInput,
  MPKbd,
  MPSectionLabel,
  MPSegmented,
  MPSelect,
  MPSwitch,
  MPTopBar,
  FileGlyph,
} from '@/components/ui';
import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  useSyncExternalStore,
  type Dispatch,
  type KeyboardEvent,
  type ReactNode,
  type SetStateAction,
} from 'react';
import { useLatest } from '@/lib/useLatest';
import { toast } from 'sonner';
import {
  builtInProfiles,
  buildPrintRequest,
  AUTO_PAPER,
  DEFAULT_PRINTER,
  paperChoices,
  findProfileIn,
  findPageRangeOverflow,
  isValidCopies,
  isValidPageRange,
  MAX_COPIES,
  MIN_COPIES,
  printBlocker,
  type PrintJobOverrides,
} from '@/lib/profiles';
import { distinguishingFolders, fmtKb } from '@/lib/format';
import { PRINTABLE_EXTENSIONS } from '@/lib/formats';
import {
  fetchAppInfo,
  getPrinterPolicy,
  getPrinterPrefs,
  isTauri,
  listPrinters,
  previewCommand,
  type PrinterPolicy,
  type PrinterPrefs,
} from '@/lib/ipc';
import { resolvePrinterId } from '@/lib/printerIdentity';
// Straight from the generated bindings: `lib/ipc` re-exports the wire types the
// other screens use, and this one is not on that list yet.
import type { DependencyReport } from '@/ipc/bindings';
import { isMacPlatform } from '@/components/AppShell/MPAppShell';
import { DependencyInstallDialog } from '@/components/DependencyInstallDialog';
import { remediationKey } from '@/lib/deps';
import { useHostPlatform } from '@/lib/platform';
import { Icon } from '@/components/Icon';
import { useLocale, useT } from '@/i18n/I18nProvider';
import { useProfilesStore } from '@/stores/profilesStore';
import { primePrinterNames } from '@/stores/printerNamesStore';
import {
  addPathsToJob,
  expandFolderIntoJob,
  getExpandingCount,
  getPendingFolders,
  isJobDropListenerReady,
  resolvePendingFolder,
  setJobDropListenerReady,
  subscribeExpanding,
  subscribePendingFolders,
} from '@/lib/jobFiles';
import type { Density } from '@/lib/theme';
import type { JobOrigin } from '@/lib/selectionSource';
import type { UiFile, UiProfile } from '@/types/domain';

// Re-exported for callers that still import the override shape from the screen
// (e.g. App). The canonical definition lives with the profile mapping.
export type { PrintJobOverrides };

/** Extensions the print engine accepts — mirrors the backend allowlist. */
const SUPPORTED = PRINTABLE_EXTENSIONS;

/** Turn an unknown thrown value into a user-facing message. */
function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** A warning under a setting the chosen printer reports it cannot honour. */
/**
 * The name of the printer "System default" would print to, when that printer
 * is removed from Pluryprint — else `null`. Mirrors the print path
 * (`resolve_app_printer` after `with_policy`): an organization policy's
 * default replaces the app default; with neither usable, the OS default
 * applies. A policy-pinned printer that is removed is named too (the job
 * pauses); one that is fine means nothing else matters.
 */
function removedSystemDefaultName(
  printers: readonly { id: string; name: string; isDefault: boolean }[],
  prefs: PrinterPrefs,
  policy: PrinterPolicy | null,
  disabled: ReadonlySet<string>,
): string | null {
  const policyId = resolvePrinterId(printers, policy?.default_printer);
  if (policyId) {
    return disabled.has(policyId)
      ? (printers.find((p) => p.id === policyId)?.name ?? policyId)
      : null;
  }
  const appDefault = prefs.default_printer ?? null;
  const appDefaultUsable =
    appDefault !== null &&
    !disabled.has(appDefault) &&
    printers.some((p) => p.id === appDefault);
  if (appDefaultUsable) return null;
  const osDefault = printers.find((p) => p.isDefault);
  return osDefault && disabled.has(osDefault.id) ? osDefault.name : null;
}

function CapabilityNote({ children }: { children: ReactNode }) {
  return (
    <div
      role="note"
      style={{
        display: 'flex',
        gap: 6,
        alignItems: 'flex-start',
        marginTop: 6,
        fontSize: 11,
        lineHeight: 1.45,
        color: 'var(--mp-warn)',
      }}
    >
      <Icon name="alert" size={12} color="var(--mp-warn)" />
      <span style={{ flex: 1, minWidth: 0 }}>{children}</span>
    </div>
  );
}

interface PrintJobScreenProps {
  files: UiFile[];
  setFiles: Dispatch<SetStateAction<UiFile[]>>;
  profile: string;
  setProfile: (id: string) => void;
  overrides: PrintJobOverrides;
  setOverrides: (o: PrintJobOverrides) => void;
  /**
   * What the copies field shows while it differs from the committed value
   * (an invalid entry, or one being typed). Owned by the caller when given,
   * so the Ctrl/⌘+P shortcut — which lives outside this screen — refuses the
   * same invalid entry the Print button does; `null` shows the effective
   * value, so a profile change from anywhere is reflected at once.
   */
  copiesDraft?: string | null;
  setCopiesDraft?: (draft: string | null) => void;
  density?: Density;
  onPrint?: () => void;
  /**
   * FR-124: the batch came from an integration the screen names ("From
   * Pluryfact"), with that integration's label for it.
   */
  origin?: JobOrigin | null;
}

export function PrintJobScreen({
  files,
  setFiles,
  profile,
  setProfile,
  overrides,
  setOverrides,
  copiesDraft: draftProp,
  setCopiesDraft: setDraftProp,
  density = 'comfortable',
  onPrint,
  origin = null,
}: PrintJobScreenProps) {
  const t = useT('printJob');
  // macOS labels the modifier Option (⌥), everywhere else it is Alt.
  const mac = isMacPlatform();
  // The empty list names the file manager files come from: Explorer or
  // Finder, or on Linux whichever one the desktop uses.
  const host = useHostPlatform();
  const tCommon = useT('common');

  // The copies draft lives with the caller when it passes one (App does);
  // otherwise this screen keeps its own, so it still works standalone.
  const [localDraft, setLocalDraft] = useState<string | null>(null);
  const copiesDraft = setDraftProp ? (draftProp ?? null) : localDraft;
  const setCopiesDraft = setDraftProp ?? setLocalDraft;

  // What is in the job right now, for the "added N files" counts: a folder
  // dropped twice must not claim to have added its files the second time.
  const filesRef = useLatest(files);
  const currentPaths = () => new Set(filesRef.current.map((f) => f.path));

  // Profiles come from the shared store (built-ins + customs). Init is
  // idempotent; the static built-ins are the fallback until it resolves (and
  // the store's own fallback outside Tauri), so the dropdown is never empty.
  const locale = useLocale();
  const storeProfiles = useProfilesStore((s) => s.profiles);
  const initProfiles = useProfilesStore((s) => s.init);
  useEffect(() => {
    void initProfiles();
  }, [initProfiles]);
  const profileList =
    storeProfiles.length > 0 ? storeProfiles : builtInProfiles(locale);

  const profileObj = findProfileIn(profileList, profile);
  const merged = { ...profileObj, ...overrides };
  const tauri = isTauri();

  // Real installed printers for the override dropdown (empty outside Tauri),
  // excluding printers disabled in Pluryprint. The *id* is the queue name the
  // spooler needs (`lp -d`, SumatraPDF `-print-to`); the display name only
  // labels it, and on CUPS the two genuinely differ (`printer-info` vs the
  // queue name). Sending the label would produce a job for a printer that does
  // not exist.
  const [printers, setPrinters] = useState<
    {
      id: string;
      name: string;
      papers: string[];
      color: boolean;
      duplex: boolean;
      colorUnknown: boolean;
      duplexUnknown: boolean;
    }[]
  >([]);
  // "System default" resolves at print time to the organization policy's
  // default, else Pluryprint's own default, else the OS default — the order
  // the print path uses. If the printer it lands on was removed from
  // Pluryprint the job is refused at print time, so name it here.
  const [removedSystemDefault, setRemovedSystemDefault] = useState<
    string | null
  >(null);
  // Printers removed from Pluryprint, by id → display name. They are not
  // offered, but a profile may still pin one: it is then labelled and
  // explained instead of showing a bare queue id.
  const [removedPrinters, setRemovedPrinters] = useState<Map<string, string>>(
    () => new Map(),
  );
  useEffect(() => {
    if (!isTauri()) return;
    let alive = true;
    Promise.all([
      listPrinters(),
      getPrinterPrefs().catch((): PrinterPrefs => ({})),
      getPrinterPolicy().catch((): PrinterPolicy | null => null),
    ])
      .then(([ps, prefs, policy]) => {
        primePrinterNames(ps);
        if (!alive) return;
        const disabled = new Set(prefs.disabled ?? []);
        setRemovedSystemDefault(
          removedSystemDefaultName(ps, prefs, policy, disabled),
        );
        setRemovedPrinters(
          new Map(
            ps.filter((p) => disabled.has(p.id)).map((p) => [p.id, p.name]),
          ),
        );
        setPrinters(
          ps
            .filter((p) => !disabled.has(p.id))
            .map((p) => ({
              id: p.id,
              name: p.name,
              papers: p.papers,
              color: p.color,
              duplex: p.duplex,
              colorUnknown: p.colorUnknown === true,
              duplexUnknown: p.duplexUnknown === true,
            })),
        );
      })
      .catch((e) => {
        // Non-fatal (the dropdown just stays on "System default"), but not
        // silent: a real enumeration failure is worth a trace in dev tools.
        console.error('printer list load failed', e);
      });
    return () => {
      alive = false;
    };
  }, []);
  const currentPrinter = merged.printer;
  // A profile saved before this screen used ids — or one naming a printer that
  // is currently offline — would otherwise silently snap to "System default".
  // Keep the stored value selectable, labelled with itself.
  const unknownPrinter =
    currentPrinter &&
    currentPrinter !== DEFAULT_PRINTER &&
    !printers.some((p) => p.id === currentPrinter)
      ? currentPrinter
      : null;
  // The stored printer is one the user removed from Pluryprint (by id, or by
  // a display name a profile saved): the job would pause with
  // `printer_disabled`, so say so, by name.
  const removedChoice = unknownPrinter
    ? (removedPrinters.get(unknownPrinter) ??
      [...removedPrinters.values()].find((n) => n === unknownPrinter) ??
      null)
    : null;
  // What the chosen printer reports it cannot do. Only an explicit choice is
  // checked: "System default" resolves at print time (routes, app default),
  // so naming a printer here could be wrong. A capability the OS could not
  // report is unknown, not missing: no warning (the engine prints duplex on
  // an unknown, and a colour printer whose driver names its colour option
  // differently is not "black and white only").
  const chosenPrinter = printers.find((p) => p.id === currentPrinter);
  const noDuplex =
    chosenPrinter !== undefined &&
    !chosenPrinter.duplex &&
    !chosenPrinter.duplexUnknown &&
    merged.duplex !== 'off';
  const noColor =
    chosenPrinter !== undefined &&
    !chosenPrinter.color &&
    !chosenPrinter.colorUnknown &&
    merged.color === 'color';
  const printerOptions = [
    { value: DEFAULT_PRINTER, label: t('settings.systemDefault') },
    ...printers.map((p) => ({ value: p.id, label: p.name })),
    ...(unknownPrinter
      ? [
          {
            value: unknownPrinter,
            label: removedChoice
              ? t('settings.removedPrinterOption', { name: removedChoice })
              : unknownPrinter,
          },
        ]
      : []),
  ];

  // FR-058: the external programs the print pipeline shells out to, probed by
  // the backend (`engine::deps`) and shipped on `app_info`. Probed on mount
  // and again whenever a missing one could have been installed meanwhile: the
  // user adds files again, comes back to the window (a Store or web install
  // finishes outside the app), or presses "Check again". A failed fetch
  // leaves the last answer in place — with nothing known yet that is an empty
  // list, which surfaces no warning: we then know nothing rather than know it
  // is missing.
  const [dependencies, setDependencies] = useState<DependencyReport[]>([]);
  // Every probe asked for bumps `probeRequest`; the effect below runs the
  // newest one and drops the answer of any it superseded. The mount probe is
  // asked for by the initial state.
  const [probeRequest, setProbeRequest] = useState(() => (isTauri() ? 1 : 0));
  const [probing, setProbing] = useState(isTauri);
  // FR-058 / D13: the dependency the user asked to install (consent dialog).
  const [installing, setInstalling] = useState<DependencyReport | null>(null);
  const replaceDependency = (fresh: DependencyReport) =>
    setDependencies((all) => all.map((d) => (d.id === fresh.id ? fresh : d)));
  const probeDependencies = useCallback(() => {
    if (!isTauri()) return;
    setProbing(true);
    setProbeRequest((n) => n + 1);
  }, []);
  useEffect(() => {
    if (probeRequest === 0) return;
    let current = true;
    fetchAppInfo()
      .then((info) => {
        if (current) setDependencies(info.dependencies ?? []);
      })
      .catch((e) => {
        console.error('dependency probe failed', e);
      })
      .finally(() => {
        if (current) setProbing(false);
      });
    return () => {
      current = false;
    };
  }, [probeRequest]);

  // Only dependencies that actually gate a file the user added are shown: the
  // backend lists what each missing program blocks *today* (never what it would
  // enable one day), and we intersect that with the extensions in this job. So
  // a missing LibreOffice is silent — it blocks nothing the engine can print —
  // while a missing SumatraPDF on Windows, which is the final printer for every
  // format, is not.
  const jobExts = new Set(files.map((f) => f.ext.toLowerCase()));
  const blockingDependencies = dependencies.filter(
    (d) =>
      d.applicable &&
      !d.found &&
      d.remediation !== null &&
      d.blocks.some((ext) => jobExts.has(ext)),
  );

  // While something is missing, look again when it may have been fixed: files
  // added, or the window regaining focus after the Store / download page (the
  // check the "opened" install result promises).
  const anyBlocking = blockingDependencies.length > 0;
  const fileCount = files.length;
  // Noticed while rendering; `dependencies` is read, not watched, so a probe
  // never triggers a probe.
  const [seenFiles, setSeenFiles] = useState(fileCount);
  if (fileCount !== seenFiles) {
    setSeenFiles(fileCount);
    if (
      fileCount > seenFiles &&
      dependencies.some((d) => d.applicable && !d.found)
    ) {
      probeDependencies();
    }
  }
  useEffect(() => {
    if (!anyBlocking) return;
    const onFocus = () => probeDependencies();
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, [anyBlocking, probeDependencies]);

  // An image is one page; a PDF too large or broken to probe has no known
  // count, and must neither be shown as an image nor counted as one page.
  const isImage = (ext: string) =>
    ext.toLowerCase() !== 'pdf' &&
    (SUPPORTED as readonly string[]).includes(ext.toLowerCase());
  const unknownPages = files.filter((f) => !f.pages && !isImage(f.ext)).length;
  const totalPages = files.reduce(
    (s, f) => s + (f.pages ?? (isImage(f.ext) ? 1 : 0)),
    0,
  );
  const totalKb = files.reduce((s, f) => s + f.kb, 0);

  /**
   * Inspect the given paths and append the printable ones. Shared with the
   * app-level drop handler; dedup happens inside the functional state update,
   * so concurrent adds can't lose files or duplicate them.
   */
  async function addPaths(paths: string[]): Promise<void> {
    await addPathsToJob(paths, setFiles, currentPaths);
  }

  /**
   * Open the OS file picker (Tauri only) and add the chosen files.
   *
   * Two pickers, because the OS dialogs are mutually exclusive: files with the
   * printable-extension filter, or a directory (FR-007). The folder path lands
   * in the same `addPaths` pipeline, so it gets the same FR-004 question a drop
   * would.
   */
  async function onPick(directory: boolean): Promise<void> {
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const picked = directory
        ? await open({ multiple: true, directory: true })
        : await open({
            multiple: true,
            filters: [
              {
                name: tCommon('filePicker.filterName'),
                extensions: [...SUPPORTED],
              },
            ],
          });
      if (!picked) return;
      await addPaths(Array.isArray(picked) ? picked : [picked]);
    } catch (e) {
      toast.error(message(e));
    }
  }

  // Native OS drag & drop of files onto the window (FR-007). Registered once;
  // addPaths dedupes inside a functional update, so a stable listener is safe.
  // Until the registration completes the app shell's listener takes the drops
  // (see `setJobDropListenerReady`), so none is lost while this one starts.
  const [dragging, setDragging] = useState(false);
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    let ready = false;
    import('@tauri-apps/api/webview')
      .then(({ getCurrentWebview }) =>
        getCurrentWebview().onDragDropEvent((event) => {
          if (event.payload.type === 'over') {
            setDragging(true);
          } else if (event.payload.type === 'drop') {
            setDragging(false);
            if (ready && isJobDropListenerReady()) {
              void addPaths(event.payload.paths);
            }
          } else {
            setDragging(false);
          }
        }),
      )
      .then((un) => {
        if (cancelled) {
          un();
        } else {
          unlisten = un;
          ready = true;
          setJobDropListenerReady(true);
        }
      })
      .catch((e) => {
        console.error('drag-drop listener failed', e);
      });
    return () => {
      cancelled = true;
      if (ready) setJobDropListenerReady(false);
      unlisten?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Folders waiting on the FR-004 recurse-or-not question. The store lives in
  // `jobFiles` so a folder dropped from any screen (the app-level listener then
  // navigates here) surfaces the same prompt.
  const pendingFolders = useSyncExternalStore(
    subscribePendingFolders,
    getPendingFolders,
  );

  // Folder walks still running: the job is still growing, so Print waits.
  const expanding = useSyncExternalStore(subscribeExpanding, getExpandingCount);

  // Copies shows the draft while one exists (so an out-of-range keystroke can
  // show an error instead of being silently clamped) and the effective value
  // otherwise — derived, never a stale copy taken at mount.
  const copiesText = copiesDraft ?? String(merged.copies);
  const copiesError = !isValidCopies(copiesText);
  const copiesErrorId = useId();
  const rangeHintId = useId();

  // The page range is DERIVED, never buffered: what the field shows has to be
  // exactly what `toJobSettings` sends (FR-041 wants the profile's value
  // pre-filled). `undefined` inherits the profile's saved range; `''` is an
  // explicit override back to "all pages" — the only way to defeat a profile
  // range, so clearing the field must keep meaning that.
  const rangeText = overrides.range ?? profileObj.pageRange ?? '';
  const rangeSyntaxError = !isValidPageRange(rangeText);
  // FR-042/FR-043: bounds are checked inline against the real page counts, so
  // "500-600" on an 8-page report fails here instead of spooling an empty job.
  const rangeOverflow = rangeSyntaxError
    ? null
    : findPageRangeOverflow(
        rangeText,
        files.map((f) => ({
          name: `${f.name}.${f.ext}`,
          ext: f.ext,
          pages: f.pages,
        })),
      );
  const rangeError = rangeSyntaxError || rangeOverflow !== null;

  /**
   * A new profile is a new starting point (FR-041): its own values are
   * pre-filled, so every override made against the previous one goes.
   * Keeping them would leave the dropdown naming a profile whose settings are
   * not the ones shown or sent.
   */
  function onProfileChange(id: string): void {
    setProfile(id);
    setOverrides({});
    setCopiesDraft(null);
  }

  // Honesty preview: the exact command the backend would run, debounced so it
  // does not fire on every keystroke. Hidden outside Tauri, with no files, or
  // while a field is invalid; a failed preview shows nothing (it is advisory).
  const [preview, setPreview] = useState('');
  const canPreview = tauri && files.length > 0 && !copiesError && !rangeError;
  // A preview that can no longer be shown is dropped while rendering, so it
  // never flashes back when previewing becomes possible again.
  if (!canPreview && preview !== '') setPreview('');
  useEffect(() => {
    if (!canPreview) return;
    const req = buildPrintRequest(
      files.map((f) => f.path),
      profileObj,
      overrides,
    );
    // `stale` guards the in-flight promise: without it a slow response for
    // superseded settings could render over the current ones.
    let stale = false;
    const t = setTimeout(() => {
      previewCommand(req)
        .then((s) => {
          if (!stale) setPreview(s);
        })
        .catch(() => {
          if (!stale) setPreview('');
        });
    }, 300);
    return () => {
      stale = true;
      clearTimeout(t);
    };
  }, [canPreview, files, profileObj, overrides]);

  const canPrint =
    expanding === 0 &&
    printBlocker(
      files.map((f) => ({
        name: `${f.name}.${f.ext}`,
        ext: f.ext,
        pages: f.pages,
      })),
      profileObj,
      overrides,
      copiesDraft,
    ) === null;

  // Same-named files from different folders get a folder hint each.
  const folderHints = distinguishingFolders(files.map((f) => f.path));

  // NFR-041: keyboard focus must survive list edits. A moved row's DOM node
  // can be re-inserted (which blurs it), and a removed row takes its focused
  // button with it, so after each edit we put focus back explicitly.
  const listRef = useRef<HTMLDivElement>(null);
  const rowRefs = useRef(new Map<string, HTMLDivElement>());
  const focusAfterEdit = useRef<string | null>(null);
  useEffect(() => {
    const target = focusAfterEdit.current;
    if (target === null) return;
    focusAfterEdit.current = null;
    (rowRefs.current.get(target) ?? listRef.current)?.focus();
  }, [files]);

  const remove = (id: string) => {
    const i = files.findIndex((f) => f.id === id);
    // Focus the row that takes its place, else the one before, else the list.
    focusAfterEdit.current = files[i + 1]?.id ?? files[i - 1]?.id ?? '';
    setFiles(files.filter((f) => f.id !== id));
  };
  const move = (id: string, dir: -1 | 1) => {
    const i = files.findIndex((f) => f.id === id);
    const j = i + dir;
    if (i < 0 || j < 0 || j >= files.length) return;
    const next = files.slice();
    [next[i], next[j]] = [next[j]!, next[i]!];
    focusAfterEdit.current = id;
    setFiles(next);
  };

  /**
   * Keyboard reordering for a focused row. Alt is the modifier because the bare
   * arrows belong to the scroll container; focus is put back on the moved row
   * after the reorder (see `focusAfterEdit`).
   */
  const onRowKeyDown = (e: KeyboardEvent<HTMLDivElement>, id: string) => {
    if (!e.altKey || (e.key !== 'ArrowUp' && e.key !== 'ArrowDown')) return;
    e.preventDefault();
    move(id, e.key === 'ArrowUp' ? -1 : 1);
  };

  const summary = t('subtitle', {
    files: t('units.file', { count: files.length }),
    pages:
      unknownPages > 0
        ? t('units.pageAtLeast', { count: totalPages })
        : t('units.page', { count: totalPages }),
    size: fmtKb(totalKb),
  });

  return (
    <>
      {installing && (
        <DependencyInstallDialog
          report={installing}
          onClose={() => setInstalling(null)}
          onResolved={replaceDependency}
        />
      )}
      <MPTopBar
        title={t('title')}
        subtitle={
          origin ? (
            <span data-testid="job-origin">
              <MPBadge tone="accent">
                {tCommon('selection.fromPluryfact')}
              </MPBadge>{' '}
              {origin.label ? `${origin.label} · ` : ''}
              {summary}
            </span>
          ) : (
            summary
          )
        }
        right={
          <>
            <MPButton
              variant="ghost"
              icon="plus"
              size="sm"
              onClick={() => void onPick(false)}
              disabled={!tauri}
            >
              {t('actions.addFiles')}
            </MPButton>
            <MPButton
              variant="ghost"
              icon="folder"
              size="sm"
              onClick={() => void onPick(true)}
              disabled={!tauri}
            >
              {t('actions.addFolder')}
            </MPButton>
            <MPButton
              variant="primary"
              icon="printer"
              onClick={onPrint}
              disabled={!canPrint}
            >
              {t('actions.print', { count: files.length })}
            </MPButton>
          </>
        }
      />
      <div style={{ flex: 1, display: 'flex', minHeight: 0 }}>
        {/* File list */}
        <div
          className="mp-scroll"
          style={{
            flex: 1,
            overflow: 'auto',
            padding: '14px 16px',
            minWidth: 0,
            outline: dragging ? '2px dashed var(--mp-accent)' : 'none',
            outlineOffset: -4,
            background: dragging ? 'var(--mp-accentSoft)' : 'transparent',
            transition: 'background .12s',
          }}
        >
          {blockingDependencies.length > 0 && (
            <section
              // `alert`: the files are already in the list and the Print button
              // is still live, so a user who does not look up will spool a job
              // that cannot run. This has to reach a screen reader on its own.
              role="alert"
              aria-label={t('dependencies.title')}
              style={{
                border: '1px solid var(--mp-warn)',
                background: 'var(--mp-warnSoft)',
                borderRadius: 8,
                padding: '12px 14px',
                marginBottom: 12,
              }}
            >
              <MPSectionLabel>{t('dependencies.title')}</MPSectionLabel>
              {blockingDependencies.map((dep) => {
                const affected = files.filter((f) =>
                  dep.blocks.includes(f.ext.toLowerCase()),
                ).length;
                return (
                  <div key={dep.id} style={{ marginTop: 8 }}>
                    {/* The remediation sentence in the app's language
                        (`common:deps.<id>.remediation`, FR-092): it names the
                        program and what it blocks. */}
                    <div style={{ fontSize: 12.5 }}>
                      {tCommon(remediationKey(dep.id))}
                    </div>
                    <div
                      style={{
                        display: 'flex',
                        alignItems: 'center',
                        gap: 10,
                        marginTop: 4,
                      }}
                    >
                      <span
                        style={{ fontSize: 11, color: 'var(--mp-textDim)' }}
                      >
                        {t('dependencies.affected', { count: affected })}
                      </span>
                      {dep.install.length > 0 && (
                        <MPButton
                          variant="secondary"
                          size="sm"
                          onClick={() => setInstalling(dep)}
                        >
                          {t('dependencies.installButton')}
                        </MPButton>
                      )}
                      <MPButton
                        variant="ghost"
                        size="sm"
                        icon="rotate"
                        disabled={probing}
                        onClick={probeDependencies}
                      >
                        {probing
                          ? t('dependencies.checking')
                          : t('dependencies.checkAgain')}
                      </MPButton>
                    </div>
                  </div>
                );
              })}
            </section>
          )}
          {pendingFolders.length > 0 && (
            <section
              aria-label={t('folders.title', {
                count: pendingFolders.length,
              })}
              style={{
                border: '1px solid var(--mp-accent)',
                background: 'var(--mp-accentSoft)',
                borderRadius: 8,
                padding: '12px 14px',
                marginBottom: 12,
              }}
            >
              <MPSectionLabel>
                {t('folders.title', { count: pendingFolders.length })}
              </MPSectionLabel>
              {pendingFolders.map((folder) => (
                <div key={folder.path} style={{ marginTop: 8 }}>
                  <div style={{ fontSize: 12.5, marginBottom: 8 }}>
                    {t('folders.question', { name: folder.name })}
                  </div>
                  <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
                    <MPButton
                      variant="primary"
                      size="sm"
                      onClick={() =>
                        void expandFolderIntoJob(
                          folder,
                          'recursive',
                          setFiles,
                          currentPaths,
                        )
                      }
                    >
                      {t('folders.recursive')}
                    </MPButton>
                    <MPButton
                      size="sm"
                      onClick={() =>
                        void expandFolderIntoJob(
                          folder,
                          'top_level',
                          setFiles,
                          currentPaths,
                        )
                      }
                    >
                      {t('folders.topLevel')}
                    </MPButton>
                    <MPButton
                      variant="ghost"
                      size="sm"
                      onClick={() => resolvePendingFolder(folder.path)}
                    >
                      {t('folders.skip')}
                    </MPButton>
                  </div>
                </div>
              ))}
            </section>
          )}
          <div
            style={{
              display: 'flex',
              justifyContent: 'space-between',
              alignItems: 'center',
              marginBottom: 8,
            }}
          >
            <MPSectionLabel>{t('list.heading')}</MPSectionLabel>
            <div
              style={{
                display: 'flex',
                gap: 6,
                alignItems: 'center',
              }}
            >
              <MPKbd
                aria-label={
                  mac
                    ? t('list.reorderKeysSpokenMac')
                    : t('list.reorderKeysSpoken')
                }
              >
                {mac ? t('list.reorderKeysMac') : t('list.reorderKeys')}
              </MPKbd>
              <span style={{ fontSize: 10.5, color: 'var(--mp-textDim)' }}>
                {t('list.reorderHint')}
              </span>
            </div>
          </div>
          <MPCard padding={0}>
            <div
              ref={listRef}
              role="list"
              aria-label={t('list.label')}
              tabIndex={-1}
              style={{ outline: 'none' }}
            >
              {files.map((f, i) => {
                const folder = folderHints.get(f.path);
                return (
                  // A focusable list item carrying its own arrow-key handler is
                  // the standard reorderable-list pattern: the row is the thing
                  // being moved, so it is the thing that must hold focus. The
                  // rule only knows that `listitem` is not natively interactive.
                  /* eslint-disable-next-line jsx-a11y/no-noninteractive-element-interactions */
                  <div
                    key={f.id}
                    ref={(el) => {
                      if (el) rowRefs.current.set(f.id, el);
                      else rowRefs.current.delete(f.id);
                    }}
                    role="listitem"
                    // eslint-disable-next-line jsx-a11y/no-noninteractive-tabindex
                    tabIndex={0}
                    aria-label={t('list.row', {
                      position: i + 1,
                      total: files.length,
                      name: `${f.name}.${f.ext}`,
                    })}
                    onKeyDown={(e) => onRowKeyDown(e, f.id)}
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 10,
                      padding: density === 'compact' ? '6px 12px' : '10px 12px',
                      borderBottom:
                        i < files.length - 1
                          ? '1px solid var(--mp-border)'
                          : 'none',
                    }}
                  >
                    <span
                      className="mp-mono"
                      style={{
                        fontSize: 10.5,
                        color: 'var(--mp-textDim)',
                        width: 18,
                        textAlign: 'right',
                      }}
                    >
                      {(i + 1).toString().padStart(2, '0')}
                    </span>
                    <FileGlyph
                      ext={f.ext}
                      size={density === 'compact' ? 18 : 24}
                    />
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div
                        title={f.path}
                        style={{
                          display: 'flex',
                          alignItems: 'baseline',
                          gap: 6,
                          minWidth: 0,
                          fontSize: 12.5,
                          whiteSpace: 'nowrap',
                        }}
                      >
                        <span
                          style={{
                            fontWeight: 500,
                            overflow: 'hidden',
                            textOverflow: 'ellipsis',
                            minWidth: 0,
                          }}
                        >
                          {f.name}.{f.ext}
                        </span>
                        {/* Two files with the same name: say which folder each
                          one is in, so they can be told apart. */}
                        {folder && (
                          <span
                            className="mp-mono"
                            style={{
                              fontSize: 10.5,
                              color: 'var(--mp-textDim)',
                              overflow: 'hidden',
                              textOverflow: 'ellipsis',
                              flexShrink: 2,
                              minWidth: 40,
                            }}
                          >
                            {t('list.inFolder', { folder })}
                          </span>
                        )}
                      </div>
                      {density !== 'compact' && (
                        <div
                          className="mp-mono"
                          style={{
                            fontSize: 10.5,
                            color: 'var(--mp-textDim)',
                            marginTop: 1,
                          }}
                        >
                          {f.pages
                            ? t('units.page', { count: f.pages })
                            : isImage(f.ext)
                              ? t('list.image')
                              : t('list.pagesUnknown')}{' '}
                          · {fmtKb(f.kb)}
                        </div>
                      )}
                    </div>
                    <MPIconButton
                      icon="chevu"
                      size={22}
                      label={t('list.moveUp', { name: `${f.name}.${f.ext}` })}
                      disabled={i === 0}
                      onClick={() => move(f.id, -1)}
                    />
                    <MPIconButton
                      icon="chevd"
                      size={22}
                      label={t('list.moveDown', { name: `${f.name}.${f.ext}` })}
                      disabled={i === files.length - 1}
                      onClick={() => move(f.id, 1)}
                    />
                    <MPIconButton
                      icon="x"
                      size={22}
                      label={t('list.remove', { name: `${f.name}.${f.ext}` })}
                      onClick={() => remove(f.id)}
                    />
                  </div>
                );
              })}
            </div>
            {files.length === 0 && (
              <div
                style={{
                  padding: '30px 16px',
                  textAlign: 'center',
                  color: 'var(--mp-textMuted)',
                  fontSize: 12.5,
                }}
              >
                {tauri
                  ? t(
                      host.os === 'linux'
                        ? 'empty.desktopLinux'
                        : 'empty.desktop',
                    )
                  : t('empty.web')}
              </div>
            )}
          </MPCard>
        </div>

        {/* Settings panel */}
        <div
          className="mp-scroll"
          style={{
            width: 280,
            flexShrink: 0,
            overflow: 'auto',
            background: 'var(--mp-surface2)',
            borderLeft: '1px solid var(--mp-border)',
            padding: '14px 16px',
          }}
        >
          <MPSectionLabel>{t('settings.profile')}</MPSectionLabel>
          <MPSelect
            fullWidth
            aria-label={t('settings.profile')}
            value={profile}
            onChange={(e) => onProfileChange(e.target.value)}
            options={profileList.map((p) => ({
              value: p.id,
              label: p.name + (p.builtin ? t('settings.builtinSuffix') : ''),
            }))}
          />
          <div style={{ height: 14 }} />

          <MPSectionLabel>{t('settings.printer')}</MPSectionLabel>
          <MPSelect
            fullWidth
            aria-label={t('settings.printer')}
            value={currentPrinter}
            onChange={(e) =>
              setOverrides({ ...overrides, printer: e.target.value })
            }
            options={printerOptions}
          />
          {currentPrinter === DEFAULT_PRINTER && removedSystemDefault && (
            <CapabilityNote>
              {t('capability.systemDefaultRemoved', {
                name: removedSystemDefault,
              })}
            </CapabilityNote>
          )}
          {removedChoice && (
            <CapabilityNote>
              {t('capability.printerRemoved', { name: removedChoice })}
            </CapabilityNote>
          )}
          <div style={{ height: 14 }} />

          <MPSectionLabel>{t('settings.sides')}</MPSectionLabel>
          <MPSegmented
            fullWidth
            aria-label={t('settings.sides')}
            value={merged.duplex}
            onChange={(v) => setOverrides({ ...overrides, duplex: v })}
            options={[
              { value: 'off', label: t('sides.single') },
              { value: 'long-edge', label: t('sides.long') },
              { value: 'short-edge', label: t('sides.short') },
            ]}
          />
          {noDuplex && chosenPrinter && (
            <CapabilityNote>
              {t('capability.noDuplex', { name: chosenPrinter.name })}
            </CapabilityNote>
          )}
          <div style={{ height: 14 }} />

          <MPSectionLabel>{t('settings.color')}</MPSectionLabel>
          <MPSegmented
            fullWidth
            aria-label={t('settings.color')}
            value={merged.color}
            onChange={(v) => setOverrides({ ...overrides, color: v })}
            options={[
              { value: 'color', label: t('color.color') },
              { value: 'monochrome', label: t('color.monochrome') },
            ]}
          />
          {noColor && chosenPrinter && (
            <CapabilityNote>
              {t('capability.noColor', { name: chosenPrinter.name })}
            </CapabilityNote>
          )}
          <div style={{ height: 14 }} />

          <MPSectionLabel>{t('settings.copies')}</MPSectionLabel>
          <MPInput
            fullWidth
            mono
            aria-label={t('settings.copies')}
            {...(copiesError ? { 'aria-describedby': copiesErrorId } : {})}
            value={copiesText}
            onChange={(e) => {
              const raw = e.target.value;
              if (isValidCopies(raw)) {
                const copies = Number(raw.trim());
                setOverrides({ ...overrides, copies });
                // A canonical entry needs no draft: the field then follows
                // the effective value again. "05" keeps its draft so the
                // caret is not yanked mid-edit.
                setCopiesDraft(raw.trim() === String(copies) ? null : raw);
              } else {
                setCopiesDraft(raw);
              }
            }}
            type="number"
            min={MIN_COPIES}
            max={MAX_COPIES}
            suffix={t('settings.copiesSuffix')}
            error={copiesError}
          />
          {copiesError && (
            <div
              id={copiesErrorId}
              role="alert"
              style={{
                fontSize: 11,
                color: 'var(--mp-error)',
                marginTop: 4,
              }}
            >
              {t('copies.error')}
            </div>
          )}
          <div style={{ height: 8 }} />
          <MPSwitch
            checked={merged.collate ?? true}
            label={t('settings.collate')}
            aria-label={t('settings.collate')}
            onChange={() =>
              setOverrides({ ...overrides, collate: !(merged.collate ?? true) })
            }
          />
          <div style={{ height: 14 }} />

          <div
            style={{
              display: 'grid',
              gridTemplateColumns: '1fr 1fr',
              gap: 10,
            }}
          >
            <div>
              <MPSectionLabel>{t('settings.paper')}</MPSectionLabel>
              <MPSelect
                fullWidth
                aria-label={t('settings.paper')}
                value={merged.paper}
                onChange={(e) =>
                  setOverrides({ ...overrides, paper: e.target.value })
                }
                options={paperChoices(
                  printers.find((p) => p.id === currentPrinter)?.papers ?? [],
                  merged.paper,
                ).map((v) => ({
                  value: v,
                  label:
                    v === AUTO_PAPER ? t('settings.paperPrinterDefault') : v,
                }))}
              />
            </div>
            <div>
              <MPSectionLabel>{t('settings.orientation')}</MPSectionLabel>
              <MPSelect
                fullWidth
                aria-label={t('settings.orientationLabel')}
                value={merged.orientation}
                onChange={(e) =>
                  setOverrides({
                    ...overrides,
                    orientation: e.target.value as UiProfile['orientation'],
                  })
                }
                options={[
                  { value: 'auto', label: t('orientation.auto') },
                  { value: 'portrait', label: t('orientation.portrait') },
                  { value: 'landscape', label: t('orientation.landscape') },
                ]}
              />
            </div>
          </div>
          <div style={{ height: 14 }} />

          <MPSectionLabel>{t('settings.scaling')}</MPSectionLabel>
          <MPSegmented
            fullWidth
            aria-label={t('settings.scaling')}
            value={merged.scaling}
            onChange={(v) => setOverrides({ ...overrides, scaling: v })}
            options={[
              { value: 'fit', label: t('scaling.fit') },
              { value: 'shrink', label: t('scaling.shrink') },
              { value: 'none', label: t('scaling.none') },
            ]}
          />
          <div style={{ height: 14 }} />

          <MPSectionLabel>{t('settings.pageRange')}</MPSectionLabel>
          <MPInput
            fullWidth
            mono
            placeholder={t('pageRange.placeholder')}
            aria-label={t('settings.pageRange')}
            aria-describedby={rangeHintId}
            value={rangeText}
            onChange={(e) =>
              setOverrides({ ...overrides, range: e.target.value })
            }
            error={rangeError}
          />
          <div
            id={rangeHintId}
            {...(rangeError ? { role: 'alert' as const } : {})}
            style={{
              fontSize: 11,
              color: rangeError ? 'var(--mp-error)' : 'var(--mp-textDim)',
              marginTop: 4,
            }}
          >
            {rangeSyntaxError
              ? t('pageRange.syntaxError')
              : rangeOverflow
                ? t('pageRange.outOfBounds', {
                    name: rangeOverflow.name,
                    count: rangeOverflow.pages,
                  })
                : t('pageRange.hint')}
          </div>

          {tauri && files.length > 0 && preview && (
            <>
              <div style={{ height: 18 }} />
              <div
                style={{
                  padding: '10px 12px',
                  background: 'var(--mp-surface)',
                  border: '1px solid var(--mp-border)',
                  borderRadius: 6,
                }}
              >
                <div
                  className="mp-mono"
                  style={{
                    fontSize: 10.5,
                    color: 'var(--mp-textDim)',
                    marginBottom: 6,
                    letterSpacing: '0.06em',
                    textTransform: 'uppercase',
                  }}
                >
                  {t('preview.title')}
                </div>
                <div
                  className="mp-mono"
                  style={{
                    fontSize: 10.5,
                    color: 'var(--mp-textMuted)',
                    lineHeight: 1.5,
                    wordBreak: 'break-all',
                    // One command per group of files, with a comment line
                    // naming them, so line breaks are meaningful.
                    whiteSpace: 'pre-wrap',
                  }}
                >
                  {preview}
                </div>
              </div>
            </>
          )}
        </div>
      </div>
    </>
  );
}
