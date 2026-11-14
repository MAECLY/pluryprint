import {
  useCallback,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
} from 'react';
import { useLatest } from '@/lib/useLatest';
import { toast } from 'sonner';

import { MPSidebar, type ScreenId } from '@/components/AppShell/MPSidebar';
import { MPAppShell, useAppShortcuts } from '@/components/AppShell/MPAppShell';
import { Toaster } from '@/components/AppShell/Toaster';
import { ConfirmDialog, type ConfirmState } from '@/components/ConfirmDialog';
import { useDensity, useTheme, type ResolvedTheme } from '@/lib/theme';
import {
  BUILT_IN_PROFILES,
  buildPrintRequest,
  findProfileByRef,
  findProfileIn,
  printBlocker,
} from '@/lib/profiles';
import {
  isTauri,
  openExternal,
  openLogsFolder,
  printFiles,
  takePendingSelections,
  takeRetiredRendererNotice,
  type Selection,
} from '@/lib/ipc';
import { logError } from '@/lib/errorLog';
import {
  jobOriginOf,
  originForFiles,
  overridesFromSelection,
  trackOrigin,
  type JobOrigin,
  type TrackedOrigin,
} from '@/lib/selectionSource';
import { errorMessage } from '@/lib/ipcError';
import {
  addPathsToJob,
  clearPendingFolders,
  getExpandingCount,
  isJobDropListenerReady,
  resolveSelectionFiles,
  subscribeExpanding,
} from '@/lib/jobFiles';
import { events } from '@/ipc/bindings';
import { announceInstallEnd, realInstallBackend } from '@/lib/install';
import { useInstallStore } from '@/stores/installStore';
import { useProfilesStore } from '@/stores/profilesStore';
import { setQueueNavigator, useQueueStore } from '@/stores/queueStore';
import { useSettingsStore } from '@/stores/settingsStore';
import { useT } from '@/i18n/I18nProvider';
import type { UiFile, UiProfile } from '@/types/domain';

import { HomeScreen, PRINTABLE_EXTENSIONS } from '@/screens/HomeScreen';
import {
  PrintJobScreen,
  type PrintJobOverrides,
} from '@/screens/PrintJobScreen';
import { QueueScreen, type QueueView } from '@/screens/QueueScreen';
import { PrintersScreen } from '@/screens/PrintersScreen';
import { ProfilesScreen } from '@/screens/ProfilesScreen';
import { SettingsScreen } from '@/screens/SettingsScreen';
import { AboutScreen } from '@/screens/AboutScreen';
import { OnboardingFlow } from '@/screens/auxiliary/OnboardingFlow';

/**
 * Cache of `settings.app.onboarding_complete`. The config is what counts: a
 * packaged upgrade starts with an empty webview storage (ADR-0009).
 */
const ONBOARD_KEY = 'pluryprint:onboarding-complete';

/**
 * How long the "an update is available" toast stays up. Long enough to be read
 * by someone who was not looking at the moment it appeared (the check fires 30
 * seconds after launch, unprompted), short enough that it still goes away on
 * its own. Missing it costs little: the About screen's "Check for updates"
 * finds the same release on demand (the launch-time result itself is not kept).
 */
const UPDATE_TOAST_MS = 15_000;

/**
 * The profile list once it is authoritative: kicks off the (idempotent) load
 * if nobody has, and waits out one already in flight. Whatever it settles on —
 * the backend's list, or the store's built-in fallback after an error — is
 * returned.
 */
async function settledProfiles(): Promise<UiProfile[]> {
  const store = useProfilesStore;
  if (store.getState().status === 'idle') void store.getState().init();
  if (store.getState().status === 'loading') {
    await new Promise<void>((resolve) => {
      const unsubscribe = store.subscribe((s) => {
        if (s.status !== 'loading') {
          unsubscribe();
          resolve();
        }
      });
    });
  }
  return store.getState().profiles;
}

/** The profiles a context-menu selection is resolved against. */
interface FreshProfiles {
  profiles: UiProfile[];
  /**
   * Why the list could not be re-read from disk, when it could not (a
   * corrupt `profiles.toml`, the backend refusing). `profiles` is then only
   * the cached list — or, if that never loaded, the built-in fallback — so a
   * reference missing from it proves nothing.
   */
  unreadable: string | null;
  /** Whether `profiles` is a list really read from disk at some point. */
  trusted: boolean;
}

/**
 * The profile list as it is on disk *now*, for acting on a profile reference
 * this window did not pick itself (a context-menu selection, FR-001).
 *
 * `profiles.toml` is also written by the CLI (`profiles create`, which also
 * rewrites the shell menu so the new profile is offered at once), by a second
 * window and by hand, and the store has no watcher. Resolving against the
 * cached list used to miss such a profile and print with the default instead.
 * When the reload itself fails, the caller is told why: a profile missing
 * from a list that could not be read is not "a profile that does not exist",
 * and a built-in fallback is not the user's default.
 */
async function freshProfiles(): Promise<FreshProfiles> {
  const settled = await settledProfiles();
  try {
    const profiles = await useProfilesStore.getState().reload();
    return { profiles, unreadable: null, trusted: true };
  } catch (e) {
    logError('app.reload-profiles', e);
    return {
      profiles: settled,
      unreadable: errorMessage(e),
      trusted: useProfilesStore.getState().status === 'ready',
    };
  }
}

function readOnboardingDone() {
  try {
    return window.localStorage.getItem(ONBOARD_KEY) === '1';
  } catch {
    return false;
  }
}

function App() {
  const t = useT('common');
  // Shortcut feedback and the cancel confirmation are documented on the Home
  // screen, so they share its namespace.
  const tHome = useT('home');
  // The launch-time update toast is the same news the About screen's update
  // card shows, so it reuses that screen's strings verbatim.
  const tAbout = useT('about');
  // The one-time notice about the retired HTML renderer is about a setting.
  const tSettings = useT('settings');
  // An install picked up after a reload is announced like the dialog would.
  const tPrintJob = useT('printJob');
  const tPrintJobRef = useLatest(tPrintJob);
  const { mode, resolved, setMode } = useTheme();
  const { density, setDensity } = useDensity();
  const [active, setActive] = useState<ScreenId>('home');
  const [onboardingSeenHere, setOnboardingSeenHere] =
    useState(readOnboardingDone);

  // Print job state. `profileChoice` is the user's explicit selection; while
  // null the job follows the app's default profile, so changing the default in
  // Settings/Profiles retargets future jobs until the user picks one manually.
  const [files, setFiles] = useState<UiFile[]>([]);
  const [profileChoice, setProfileChoice] = useState<string | null>(null);
  const [overrides, setOverrides] = useState<PrintJobOverrides>({});
  // FR-124: files on the Print Job screen that came from PluryFact (through
  // the browser bridge). The screen says so only while every file in the job
  // is one of them (`shownOrigin` below). Cleared once the job is printed or
  // emptied.
  const [jobOrigin, setJobOrigin] = useState<TrackedOrigin | null>(null);
  // What the copies field shows while it differs from the committed value
  // (see PrintJobScreen). Lives here so Ctrl/⌘+P refuses the same invalid
  // entry the Print button does.
  const [copiesDraft, setCopiesDraft] = useState<string | null>(null);
  const profiles = useProfilesStore((s) => s.profiles);
  const profilesStatus = useProfilesStore((s) => s.status);
  const initProfiles = useProfilesStore((s) => s.init);
  const effectiveProfile = findProfileIn(profiles, profileChoice);
  const effectiveProfileId = effectiveProfile.id;

  // The job as of the last render, for async code that must not act on the
  // stale list its closure captured (a submit that resolves after more files
  // arrived, the "added N files" counts).
  const filesRef = useLatest(files);
  const currentPaths = useCallback(
    () => new Set(filesRef.current.map((f) => f.path)),
    [filesRef],
  );

  // Folder walks still running: the job is still growing.
  const expanding = useSyncExternalStore(subscribeExpanding, getExpandingCount);

  /** Forget this job's per-invocation tweaks (FR-040: they never stick). */
  const resetJobSettings = () => {
    setOverrides({});
    setCopiesDraft(null);
  };

  /**
   * Point the job at a profile — from the dropdown, Ctrl/⌘+K or a selection.
   * A new profile is a new starting point (FR-041), so the overrides made
   * against the previous one go with it.
   */
  const chooseProfile = (id: string | null) => {
    setProfileChoice(id);
    resetJobSettings();
  };

  // Queue state (jobs live in the queue store; App owns only the view toggle)
  const [queueView, setQueueView] = useState<QueueView>('list');
  const initQueue = useQueueStore((s) => s.init);
  const queueCount = useQueueStore((s) => s.counts.active + s.counts.pending);
  const uiJobs = useQueueStore((s) => s.uiJobs);
  const cancelJob = useQueueStore((s) => s.cancelJob);

  // Destructive shortcuts confirm first (never `window.confirm`, which is a
  // no-op in WKWebView).
  const [confirm, setConfirm] = useState<ConfirmState | null>(null);

  // App settings (backend config.toml): loaded once; appearance reconciles below.
  const initSettings = useSettingsStore((s) => s.init);
  const settings = useSettingsStore((s) => s.settings);
  const settingsStatus = useSettingsStore((s) => s.status);
  const patchSettings = useSettingsStore((s) => s.patch);
  // The tour waits for the config: an upgrade records it there as seen, and
  // offering it while the config loads would flash it at every upgrader.
  const onboardingDone =
    onboardingSeenHere || settings?.app.onboarding_complete === true;
  const showOnboarding =
    !onboardingDone &&
    settingsStatus !== 'idle' &&
    settingsStatus !== 'loading';

  const [selectedProfileId, setSelectedProfileId] = useState(
    BUILT_IN_PROFILES[0]!.id,
  );

  // Fallback so the document is never class-less before useTheme's first apply.
  useEffect(() => {
    if (
      typeof document !== 'undefined' &&
      !document.documentElement.classList.contains('mp-light') &&
      !document.documentElement.classList.contains('mp-dark')
    ) {
      document.documentElement.classList.add('mp-light');
    }
  }, []);

  // Load the queue snapshot and subscribe to live updates (idempotent).
  useEffect(() => {
    void initQueue();
  }, [initQueue]);

  // The queue store's "a job needs you" toast (FR-071) offers "Open queue";
  // only the shell knows how to get there.
  useEffect(() => {
    setQueueNavigator(() => setActive('queue'));
    return () => setQueueNavigator(null);
  }, []);

  // FR-058: a dependency install outlives the window. After a reload, find
  // the one the backend is still running (or whose stopped installer may
  // still be finishing), so its dialog shows it, with Stop, instead of an
  // Install the backend would refuse.
  useEffect(() => {
    if (!isTauri()) return;
    useInstallStore
      .getState()
      .hydrate(realInstallBackend, {
        // No screen's files wait for an install found after a reload: the
        // store resumes the queue's waiting jobs and the toast says so.
        announce: (end) =>
          announceInstallEnd(tPrintJobRef.current, end, false, true),
        unknownEnd: (name) =>
          tPrintJobRef.current('install.result.unknownEnd', { name }),
      })
      .catch((e: unknown) => logError('app.install-hydrate', e));
  }, [tPrintJobRef]);

  // Load profiles and app settings once (both idempotent, retryable).
  useEffect(() => {
    void initProfiles();
    void initSettings();
  }, [initProfiles, initSettings]);

  // Appearance reconcile: localStorage applied instantly at boot (no flash);
  // once the backend config arrives it is the source of truth and overwrites
  // a diverging local value (the setters keep localStorage in sync).
  useEffect(() => {
    if (!settings) return;
    if (settings.app.theme !== mode) setMode(settings.app.theme);
    if (settings.app.density && settings.app.density !== density) {
      setDensity(settings.app.density);
    }
    // Reconcile only when the backend value changes, not on local edits.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settings?.app.theme, settings?.app.density]);

  // A deleted custom profile silently falls back to the app default. Only act
  // when the store is authoritative ('ready'); while it is loading/error/
  // unavailable the list can be transiently empty or stale, and clearing an
  // explicit choice then would wipe the user's selection for no reason.
  // Adjusted while rendering rather than in an effect, so no frame shows the
  // job pointing at a profile that no longer exists.
  if (
    profilesStatus === 'ready' &&
    profileChoice &&
    !profiles.some((p) => p.id === profileChoice)
  ) {
    setProfileChoice(null);
  }

  // An emptied job is no longer the batch an integration sent (FR-124).
  // Adjusted while rendering, like the profile choice above.
  if (files.length === 0 && jobOrigin !== null) {
    setJobOrigin(null);
  }
  // The badge the Print Job screen shows: none for a mixed job, so neither
  // the user's own files nor a later drop are labelled as PluryFact's.
  const shownOrigin = originForFiles(
    jobOrigin,
    files.map((f) => f.path),
  );

  // App-level drag & drop (FR-007): the Home screen advertises "drag them
  // here", so drops must work from any screen, not only while the Print Job
  // screen (which owns its own listener + overlay) is mounted. Once the job
  // screen's listener is registered it handles the drop, so this one stands
  // down to avoid double-adding — but only then: until it is registered (just
  // after navigating there) a drop reaches this listener alone and must not
  // be lost.
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    import('@tauri-apps/api/webview')
      .then(({ getCurrentWebview }) =>
        getCurrentWebview().onDragDropEvent((event) => {
          if (event.payload.type !== 'drop') return;
          if (isJobDropListenerReady()) return;
          void addPathsToJob(event.payload.paths, setFiles, currentPaths);
          setActive('job');
        }),
      )
      .then((un) => {
        if (cancelled) un();
        else unlisten = un;
      })
      .catch((e) => {
        logError('app.drag-drop-listener', e);
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [currentPaths]);

  // FR-080: the backend runs the launch-time update check (delayed, throttled
  // to once per 24h) and pushes this event only when there is something the
  // user should hear about — a strictly newer release they have not skipped
  // (FR-084). All of that policy lives in Rust, so there is nothing to filter
  // here: receiving the event *is* the decision to say something.
  //
  // The toast deliberately stops at the release notes. Installing — with
  // FR-087's extra confirmation for a major jump, progress, and a restart —
  // and skipping a version live on the About screen, where the full context
  // is visible; a toast is too small and too fleeting for either.
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    events.updateAvailable
      .listen(({ payload }) => {
        const version = payload.latest;
        if (!version) return;
        const url = payload.url;
        toast.info(tAbout('updates.available.title', { version }), {
          description: tAbout('updates.available.detail', {
            current: payload.current,
          }),
          duration: UPDATE_TOAST_MS,
          action: url
            ? {
                label: tAbout('updates.available.releaseNotes'),
                // A rejected URL or a machine with no default browser must
                // say so; an unhandled rejection here was invisible.
                onClick: () => {
                  openExternal(url).catch((e: unknown) => {
                    toast.error(e instanceof Error ? e.message : String(e));
                  });
                },
              }
            : undefined,
        });
      })
      .then((un) => {
        if (cancelled) un();
        else unlisten = un;
      })
      .catch((e: unknown) => {
        logError('app.update-available-listener', e);
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [tAbout]);

  // Round 6: the headless-browser HTML renderer (`[engine] html_renderer =
  // "browser"`, a hand edit of config.toml) is gone. Someone who had chosen it
  // hears once, at launch, that their pages now print with the system web
  // view. The backend removes the retired key as it answers, so this is a
  // one-shot whatever happens to the toast; it is not guarded by the effect's
  // cleanup for the same reason the selection drain is not.
  useEffect(() => {
    if (!isTauri()) return;
    takeRetiredRendererNotice()
      .then((retired) => {
        if (!retired) return;
        toast.info(tSettings('notices.htmlRendererRetired.title'), {
          description: tSettings('notices.htmlRendererRetired.detail'),
          duration: Infinity,
        });
      })
      .catch((e: unknown) => {
        logError('app.retired-renderer-notice', e);
      });
  }, [tSettings]);

  const completeOnboarding = () => {
    try {
      window.localStorage.setItem(ONBOARD_KEY, '1');
    } catch {
      /* noop */
    }
    setOnboardingSeenHere(true);
    // A no-op while no config is loaded (the browser preview).
    void patchSettings({ app: { onboarding_complete: true } });
  };

  // Guards against a double-click on Print submitting the batch twice while
  // the first print_files round-trip is still in flight.
  const submittingRef = useRef(false);

  /**
   * Spool the interactive job (the Print button and Ctrl/⌘+P) as one batch.
   *
   * On success only the files that were *sent* leave the job: a file added
   * while print_files was in flight (a late folder walk, a drop) stays for the
   * next batch instead of vanishing unprinted. The per-job overrides go too —
   * they were tweaks for this job, and carrying them silently into the next
   * one (or into a context-menu print) is how 10 copies of the wrong pages
   * reach the wrong printer. When nothing is left, unanswered folder
   * questions from this job are dropped with it.
   */
  const printJob = async () => {
    if (!isTauri()) {
      toast.error(t('errors.desktopOnly'));
      return;
    }
    const list = filesRef.current;
    if (list.length === 0 || submittingRef.current) return;
    submittingRef.current = true;
    const profileObj = findProfileIn(
      useProfilesStore.getState().profiles,
      profileChoice,
    );
    const req = buildPrintRequest(
      list.map((f) => f.path),
      profileObj,
      overrides,
    );
    const id = toast.loading(t('print.sending', { count: list.length }));
    try {
      const batch = await printFiles(req);
      toast.success(t('print.queued', { count: batch.job_ids.length }), {
        id,
        description: profileObj.name,
      });
      const sent = new Set(req.files);
      const remaining = filesRef.current.filter((f) => !sent.has(f.path));
      setFiles((cur) => cur.filter((f) => !sent.has(f.path)));
      resetJobSettings();
      if (remaining.length === 0) {
        clearPendingFolders();
        setJobOrigin(null);
      }
      setActive('queue');
    } catch (e) {
      // Keep the files so the user can fix and resubmit. NFR-033: the error
      // also offers the logs, whichever screen started the print (Home's
      // shortcut and the context menu land here too).
      toast.error(e instanceof Error ? e.message : String(e), {
        id,
        action: {
          label: t('actions.openLogsFolder'),
          onClick: () => {
            openLogsFolder().catch((err: unknown) => {
              toast.error(err instanceof Error ? err.message : String(err));
            });
          },
        },
      });
    } finally {
      submittingRef.current = false;
    }
  };

  /**
   * Spool a context-menu selection that prints (or queues) right away.
   *
   * Deliberately independent of the interactive job: it never reads or
   * clears the files the user is composing on the Print Job screen, and it
   * prints with the profile exactly as saved — no leftover overrides from an
   * earlier job. `hold` (the "add to the queue" entry) submits the batch with
   * every job paused, so only *this* batch waits for the user; the rest of the
   * queue keeps running.
   */
  const submitSelection = async (
    list: UiFile[],
    profileObj: UiProfile,
    hold: boolean,
    selectionOverrides: PrintJobOverrides = {},
    origin: JobOrigin | null = null,
  ) => {
    if (!isTauri()) {
      toast.error(t('errors.desktopOnly'));
      return;
    }
    const req = buildPrintRequest(
      list.map((f) => f.path),
      profileObj,
      selectionOverrides,
      { hold },
    );
    const id = toast.loading(t('print.sending', { count: list.length }));
    try {
      const batch = await printFiles(req);
      const count = batch.job_ids.length;
      // FR-124: a batch PluryFact sent says so, with PluryFact's own label.
      const description = origin
        ? [t('selection.fromPluryfact'), origin.label, profileObj.name]
            .filter(Boolean)
            .join(' · ')
        : profileObj.name;
      toast.success(
        hold ? t('print.held', { count }) : t('print.queued', { count }),
        { id, description },
      );
      setActive('queue');
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e), { id });
    }
  };

  /**
   * Act on one selection the OS context menu handed us (FR-001/FR-003).
   *
   * - `configure` — open the Print Job screen with the files added through
   *   the same pipeline a drop uses (same inspection, refusal toasts and
   *   FR-004 folder question), and let the user decide;
   * - `print-default` — print straight away with the requested (or default)
   *   profile, which is the whole point of the "Print with Pluryprint" entry;
   * - `queue` — enqueue without starting, for review in the Queue screen.
   *
   * The last two never pass through the interactive job: folders are walked
   * top-level without a question (nobody is looking at a dialog), and the
   * batch is exactly the selection.
   *
   * A profile the integration named — by id or by name, as
   * `--profile <NAME-OR-ID>` allows — is resolved against the list re-read
   * from disk, since it may have been created after this window loaded its
   * list. One that still matches nothing is never swapped for the default:
   * nothing is printed or queued, the user is told so, and the files are
   * opened on the Print Job screen to pick a profile there.
   */
  const applySelection = async (selection: Selection) => {
    if (selection.paths.length === 0) return;
    // FR-005: a selection cut down to the handoff cap is never dropped silently.
    if (
      selection.truncated_from != null &&
      selection.truncated_from > selection.paths.length
    ) {
      toast.warning(
        t('selection.truncated', {
          sent: selection.paths.length,
          total: selection.truncated_from,
        }),
      );
    }
    // A selection can arrive before the profile list has loaded (it is
    // drained right at launch), or name a profile created after it loaded
    // (the CLI); either way the cached list is not the authority.
    const { profiles, unreadable, trusted } = await freshProfiles();
    const named = findProfileByRef(profiles, selection.profile_id);
    // The reference the integration asked for and nothing matched.
    const unknownRef =
      selection.profile_id && !named ? selection.profile_id : null;
    // The list could not be read: a missing reference is not "no such
    // profile", and with no real list at all the default is not known either.
    const cannotResolve =
      unreadable !== null && (unknownRef !== null || (!named && !trusted));

    if (
      selection.action === 'configure' ||
      unknownRef !== null ||
      cannotResolve
    ) {
      if (cannotResolve) {
        toast.error(
          t(
            selection.action === 'configure'
              ? 'print.profilesUnreadable'
              : 'print.profilesUnreadableNotPrinted',
            { reason: unreadable },
          ),
        );
      } else if (unknownRef !== null) {
        toast.error(
          t(
            selection.action === 'configure'
              ? 'print.unknownProfile'
              : 'print.unknownProfileNotPrinted',
            { name: unknownRef },
          ),
        );
      }
      setActive('job');
      const origin = jobOriginOf(selection);
      const wanted = overridesFromSelection(selection.overrides);
      // A job the user had already started keeps its profile and tweaks:
      // the settings a PluryFact batch asks for are for that batch, not for
      // files from somewhere else. The user is told, and decides.
      const hadFiles = filesRef.current.length > 0;
      const asksSettings =
        origin !== null &&
        (named !== undefined || Object.keys(wanted).length > 0);
      if (hadFiles && asksSettings) {
        toast.info(t('selection.settingsKept'));
      } else {
        if (named) chooseProfile(named.id);
        // FR-123: settings the integration asked for pre-fill the screen;
        // the user still confirms. (Choosing the profile above already
        // cleared the previous job's tweaks.)
        if (Object.keys(wanted).length > 0) {
          if (!named) resetJobSettings();
          setOverrides(wanted);
        }
      }
      const added = await addPathsToJob(
        selection.paths,
        setFiles,
        currentPaths,
      );
      // FR-124: remember which files PluryFact brought; the badge shows only
      // while they are all the job holds.
      if (origin) {
        const addedPaths = added.map((f) => f.path);
        setJobOrigin((prev) =>
          trackOrigin(prev, origin, addedPaths, currentPaths()),
        );
      }
      return;
    }

    const files = await resolveSelectionFiles(selection.paths);
    if (files.length === 0) {
      toast.info(t('print.nothingPrintable'));
      return;
    }
    await submitSelection(
      files,
      named ?? findProfileIn(profiles, null),
      selection.action === 'queue',
      overridesFromSelection(selection.overrides),
      jobOriginOf(selection),
    );
  };

  // Kept in a ref so the subscription below can stay mounted for the life of
  // the window while still calling the current closure.
  const applySelectionRef = useLatest(applySelection);

  // FR-001/NFR-014: pick up the context-menu selections the backend parked.
  //
  // Two routes, one delivery point. A selection can be on the command line of
  // the process that opened this window (the shell extension launched us), or
  // it can arrive later — a second launch the single-instance plugin forwarded,
  // or a macOS Finder service message — while the window is already up. The
  // backend parks *both* in the same store and additionally emits
  // `selection-forwarded` for the second kind.
  //
  // So the event is treated as a doorbell, not as a payload: every route ends
  // in `take_pending_selections`, whose drain is atomic and destructive. That
  // is what makes a hot selection impossible to act on twice (once from the
  // event, once from the next drain) and impossible to miss.
  //
  // Every drain is chained onto the previous one, so selections are handled
  // strictly one after another even when the doorbell rings while an earlier
  // selection is still being inspected or submitted — two overlapping drains
  // used to race, and the loser was dropped without a word.
  //
  // Subscribing *before* the first drain closes the other side of that race: a
  // selection parked between the two is picked up by the drain, and one parked
  // after it rings the doorbell we are already listening to.
  const drainChain = useRef<Promise<void>>(Promise.resolve());
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;

    const drainOnce = async () => {
      let selections: Selection[];
      try {
        selections = await takePendingSelections();
      } catch (e) {
        logError('app.take-pending-selections', e);
        return;
      }
      // Deliberately not guarded by `cancelled`: the backend has already
      // handed these over, so dropping them here would lose the user's files
      // for good.
      for (const selection of selections) {
        try {
          await applySelectionRef.current(selection);
        } catch (e) {
          console.error('a context-menu selection failed', e);
        }
      }
    };
    const drain = () => {
      drainChain.current = drainChain.current.then(drainOnce);
    };

    events.selectionForwarded
      .listen(() => drain())
      .then((un) => {
        if (cancelled) {
          un();
          return;
        }
        unlisten = un;
        drain();
      })
      .catch((e: unknown) => {
        logError('app.selection-forwarded-listener', e);
        // A launch-time selection is still waiting; a missing subscription
        // only costs us the hot ones.
        drain();
      });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [applySelectionRef]);

  /**
   * Open the OS picker and feed the selection into the job pipeline.
   *
   * Files and directories are two mutually exclusive OS dialogs, so the caller
   * says which one it wants. Either way the paths land in `addPathsToJob`, so a
   * folder gets exactly the same FR-004 recurse-or-not question a drop would,
   * and the app moves to the job screen where that question is asked.
   */
  const pickPaths = async (directory: boolean) => {
    if (!isTauri()) {
      toast.error(t('errors.desktopOnly'));
      return;
    }
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const picked = directory
        ? await open({ multiple: true, directory: true })
        : await open({
            multiple: true,
            filters: [
              {
                name: t('filePicker.filterName'),
                extensions: [...PRINTABLE_EXTENSIONS],
              },
            ],
          });
      if (!picked) return;
      setActive('job');
      await addPathsToJob(
        Array.isArray(picked) ? picked : [picked],
        setFiles,
        currentPaths,
      );
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e));
    }
  };

  /** Move the job to the next profile in the list (NFR-034 "switch profile"). */
  const switchProfile = () => {
    if (profiles.length === 0) return;
    const index = profiles.findIndex((p) => p.id === effectiveProfileId);
    // `-1` (the current profile vanished) restarts at the first entry.
    const next = profiles[(index + 1) % profiles.length] ?? profiles[0]!;
    chooseProfile(next.id);
    toast.info(tHome('shortcuts.profileSwitched', { name: next.name }));
  };

  /**
   * Cancel the job the user would mean by "cancel": the one on the printer, or
   * the next one in line when nothing is running yet.
   */
  const cancelCurrentJob = () => {
    const target =
      uiJobs.find(
        (j) =>
          j.status === 'printing' ||
          j.status === 'converting' ||
          j.status === 'converted',
      ) ?? uiJobs.find((j) => j.status === 'queued' || j.status === 'paused');
    if (!target) {
      toast.info(tHome('shortcuts.nothingToCancel'));
      return;
    }
    setConfirm({
      title: tHome('cancelConfirm.title'),
      message: tHome('cancelConfirm.message', { file: target.file }),
      confirmLabel: tHome('cancelConfirm.action'),
      run: () => cancelJob(target.id),
    });
  };

  // NFR-034: every primary action has a shortcut, and every shortcut the UI
  // advertises does something. Suspended while a modal owns the keyboard.
  useAppShortcuts(
    {
      newJob: () => setActive('job'),
      printFolder: () => void pickPaths(true),
      print: () => {
        // The same rule the Print button applies (FR-042/FR-043): a field
        // showing an invalid value must never print an older valid one.
        const blocker = printBlocker(
          files.map((f) => ({
            name: `${f.name}.${f.ext}`,
            ext: f.ext,
            pages: f.pages,
          })),
          effectiveProfile,
          overrides,
          copiesDraft,
        );
        if (blocker !== null || expanding > 0) {
          setActive('job');
          toast.info(
            blocker === 'noFiles'
              ? tHome('shortcuts.printNeedsFiles')
              : blocker !== null
                ? tHome('shortcuts.printBlocked')
                : tHome('shortcuts.printWaitsForFolder'),
          );
          return;
        }
        void printJob();
      },
      switchProfile,
      openQueue: () => setActive('queue'),
      cancel: cancelCurrentJob,
    },
    !showOnboarding && confirm === null,
  );

  return (
    <>
      <MPAppShell>
        <div style={{ flex: 1, display: 'flex', minHeight: 0 }}>
          <MPSidebar
            active={active}
            onChange={setActive}
            queueCount={queueCount}
          />
          <main
            style={{
              flex: 1,
              display: 'flex',
              flexDirection: 'column',
              minWidth: 0,
              minHeight: 0,
            }}
          >
            {active === 'home' && (
              <HomeScreen
                onAction={(a) => {
                  if (a === 'newjob') setActive('job');
                  else if (a === 'queue') setActive('queue');
                  else void pickPaths(a === 'folder');
                }}
              />
            )}
            {active === 'job' && (
              <PrintJobScreen
                files={files}
                setFiles={setFiles}
                profile={effectiveProfileId}
                setProfile={chooseProfile}
                overrides={overrides}
                setOverrides={setOverrides}
                copiesDraft={copiesDraft}
                setCopiesDraft={setCopiesDraft}
                density={density}
                onPrint={() => void printJob()}
                origin={shownOrigin}
              />
            )}
            {active === 'queue' && (
              <QueueScreen view={queueView} setView={setQueueView} />
            )}
            {active === 'printers' && <PrintersScreen />}
            {active === 'profiles' && (
              <ProfilesScreen
                selectedId={selectedProfileId}
                onSelect={setSelectedProfileId}
              />
            )}
            {active === 'settings' && (
              <SettingsScreen
                theme={resolved}
                setTheme={(t: ResolvedTheme) => setMode(t)}
                mode={mode}
                setMode={setMode}
                density={density}
                setDensity={setDensity}
              />
            )}
            {active === 'about' && <AboutScreen />}
          </main>
        </div>
        {confirm && (
          <ConfirmDialog state={confirm} onClose={() => setConfirm(null)} />
        )}
      </MPAppShell>
      <Toaster theme={resolved} />
      {showOnboarding && <OnboardingFlow onComplete={completeOnboarding} />}
    </>
  );
}

export default App;
