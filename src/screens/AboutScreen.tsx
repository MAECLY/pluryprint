import { useEffect, useState, version as REACT_VERSION } from 'react';
// The running @tauri-apps/api version, read from its own package.json (the
// package explicitly exports "./package.json"). This reflects the resolved
// dependency, not the caret range in our package.json — so it stays honest
// as the lockfile bumps. React's version comes from its runtime `version`
// export; the app's own version is authoritative from Rust (`app_info`).
import tauriPkg from '@tauri-apps/api/package.json';
import { toast } from 'sonner';
import { MPAppIcon } from '@/components/Brand';
import { ConfirmDialog, type ConfirmState } from '@/components/ConfirmDialog';
import { Icon } from '@/components/Icon';
import { events } from '@/ipc/bindings';
import { MPButton, MPCard, MPSectionLabel, MPTopBar } from '@/components/ui';
import { useT } from '@/i18n/I18nProvider';
import type { TranslationKey } from '@/i18n';
import {
  checkForUpdates,
  fetchAppInfo,
  getUpdateState,
  installUpdate,
  ipcErrorCode,
  isTauri,
  openExternal,
  openLogsFolder,
  reportIssue,
  restartApp,
  setSkippedUpdateVersion,
  type AppInfo,
  type UpdateChannel,
  type UpdateCheck,
} from '@/lib/ipc';
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
import { useSettingsStore } from '@/stores/settingsStore';
import { useHostPlatform, type HostPlatform } from '@/lib/platform';

const TAURI_VERSION = tauriPkg.version;

/**
 * The update-check flow as an explicit state machine: no status card in `idle`,
 * a spinner while `checking`, and a real result (`done`) or the thrown,
 * user-facing message (`error`) afterwards. `deferred` is FR-084's "remind me
 * later": the result is kept but its card is replaced by a one-line note, so
 * the screen stops nagging without pretending the check never happened.
 */
type UpdateState =
  | { kind: 'idle' }
  | { kind: 'checking' }
  | { kind: 'done'; check: UpdateCheck }
  | { kind: 'deferred' }
  | { kind: 'skipped'; version: string }
  | { kind: 'error'; message: string }
  // The channel has no manifest (HTTP 404): nothing published there yet, or
  // the releases are not public. Not "up to date", and not a failure either.
  | { kind: 'unavailable' }
  // FR-084 "install now": downloading + verifying, with progress when the
  // server sent a size …
  | { kind: 'installing'; version: string; percent: number | null }
  // … installed and waiting for a restart (macOS; Windows exits into the
  // installer and never gets here) …
  | { kind: 'installed'; version: string }
  // … or refused. `message` is already user-facing.
  | { kind: 'installError'; message: string };

/**
 * FR-087: whether `latest` is a major jump from `current` (`1.x` → `2.x`).
 * Mirrors `update::install::is_major_bump`, which enforces it again: the UI
 * asks, the backend refuses to proceed without the answer. Unparseable
 * versions count as major — when in doubt, ask.
 */
function isMajorUpdate(current: string, latest: string): boolean {
  const major = (v: string) => /^\s*[vV]?(\d+)\./.exec(v)?.[1];
  const a = major(current);
  const b = major(latest);
  if (a === undefined || b === undefined) return true;
  return Number(b) > Number(a);
}

/**
 * FR-084's "skip this version", which only makes sense once the check named a
 * version. Its own component so the null check narrows without an assertion.
 */
function SkipVersionButton({
  version,
  label,
  onSkip,
}: {
  version: string | null;
  label: string;
  onSkip: (version: string) => Promise<void>;
}) {
  if (version === null) return null;
  return (
    <MPButton
      variant="secondary"
      size="sm"
      onClick={() => void onSkip(version)}
    >
      {label}
    </MPButton>
  );
}

/**
 * The update channel's translated name. A lookup table rather than a computed
 * key so a channel added to the wire type fails to compile until it is
 * translated.
 */
const CHANNEL_KEY = {
  stable: 'channels.stable',
  beta: 'channels.beta',
  nightly: 'channels.nightly',
} as const satisfies Record<UpdateChannel, TranslationKey<'about'>>;

/** RFC 3339 UTC → a local `HH:MM` clock; unparseable → `—`. */
function formatClock(rfc3339: string): string {
  const d = new Date(rfc3339);
  if (Number.isNaN(d.getTime())) return '—';
  return d.toLocaleTimeString([], {
    hour12: false,
    hour: '2-digit',
    minute: '2-digit',
  });
}

/**
 * Whether a failed check means "this channel has no update feed" (the
 * backend's `channel_unavailable` reason, a 404 on the manifest) rather than
 * a real failure.
 */
function isChannelUnavailable(e: unknown): boolean {
  return (
    ipcErrorCode(e) === 'UpdateCheckFailed' &&
    e instanceof Error &&
    'reason' in e &&
    (e as Error & { reason?: unknown }).reason === 'channel_unavailable'
  );
}

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * The command that starts the real Tauri host, quoted verbatim in the
 * "IPC unavailable" card. Not a translated string: it is a literal the reader
 * types.
 */
const DEV_BRIDGE_COMMAND = 'pnpm tauri dev';

/**
 * FR-082/FR-083: what installing an update asks of the user. Windows and
 * macOS install per user, with no administrator rights. On Linux it depends
 * on the package: an AppImage replaces its own file in place, while the .deb
 * goes through the package manager, so the system asks for an administrator
 * password (`pkexec`). An unknown package gets the sentence naming both.
 */
function verifiedNoteKey(
  host: HostPlatform,
):
  | 'updates.available.verifiedNote'
  | 'updates.available.verifiedNoteLinux'
  | 'updates.available.verifiedNoteLinuxAppImage'
  | 'updates.available.verifiedNoteLinuxDeb' {
  if (host.os !== 'linux') return 'updates.available.verifiedNote';
  if (host.linuxPackage === 'appimage') {
    return 'updates.available.verifiedNoteLinuxAppImage';
  }
  if (host.linuxPackage === 'deb')
    return 'updates.available.verifiedNoteLinuxDeb';
  return 'updates.available.verifiedNoteLinux';
}

export function AboutScreen() {
  const t = useT('about');
  const host = useHostPlatform();
  const tauri = isTauri();
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [infoError, setInfoError] = useState<string | null>(null);
  const [update, setUpdate] = useState<UpdateState>({ kind: 'idle' });
  const [confirm, setConfirm] = useState<ConfirmState | null>(null);

  // Download progress for an install in flight. The listener lives as long as
  // the screen; it only ever updates an `installing` state.
  useEffect(() => {
    if (!isTauri()) return;
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    events.updateDownloadProgress
      .listen(({ payload }) => {
        const percent =
          payload.total && payload.total > 0
            ? Math.min(
                100,
                Math.round((payload.downloaded / payload.total) * 100),
              )
            : null;
        setUpdate((u) => (u.kind === 'installing' ? { ...u, percent } : u));
      })
      .then((un) => {
        if (cancelled) un();
        else unlisten = un;
      })
      .catch(() => {
        // Progress is cosmetic; the install itself still reports its result.
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const settings = useSettingsStore((s) => s.settings);
  const initSettings = useSettingsStore((s) => s.init);

  useEffect(() => {
    // Idempotent — App may also init the store; a second call is a no-op.
    initSettings();
  }, [initSettings]);

  useEffect(() => {
    let alive = true;
    fetchAppInfo()
      .then((i) => {
        if (alive) setInfo(i);
      })
      .catch((e: unknown) => {
        if (alive) setInfoError(errText(e));
      });
    return () => {
      alive = false;
    };
  }, []);

  // A version the user skipped stays skipped across restarts (FR-084), so the
  // screen opens showing that rather than an idle card. A failure here is not
  // worth surfacing: the worst case is being offered the update again.
  useEffect(() => {
    if (!isTauri()) return;
    let alive = true;
    getUpdateState()
      .then((s) => {
        if (alive && s.skipped_version) {
          setUpdate({ kind: 'skipped', version: s.skipped_version });
        }
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  async function runCheck() {
    setUpdate({ kind: 'checking' });
    try {
      const check = await checkForUpdates();
      setUpdate({ kind: 'done', check });
    } catch (e) {
      setUpdate(
        isChannelUnavailable(e)
          ? { kind: 'unavailable' }
          : { kind: 'error', message: errText(e) },
      );
    }
  }

  /**
   * FR-084 "skip this version": persisted, unlike "remind me later", so the
   * release stays quiet across restarts. A newer release prompts again — the
   * backend compares versions rather than strings.
   */
  async function skipVersion(version: string) {
    try {
      await setSkippedUpdateVersion(version);
      setUpdate({ kind: 'skipped', version });
    } catch (e) {
      toast.error(errText(e));
    }
  }

  /** Undo the skip and show the release again, without re-checking. */
  async function unskipVersion() {
    try {
      await setSkippedUpdateVersion(null);
      await runCheck();
    } catch (e) {
      toast.error(errText(e));
    }
  }

  /**
   * FR-084 "install now". FR-087: a major jump first asks a second, explicit
   * question; only a "yes" there passes `allowMajor`, which the backend
   * requires before it downloads anything.
   */
  function requestInstall(check: UpdateCheck) {
    const version = check.latest ?? UNKNOWN_VERSION;
    if (isMajorUpdate(check.current, version)) {
      setConfirm({
        title: t('updates.install.majorTitle'),
        message: t('updates.install.majorMessage', {
          version,
          current: check.current,
        }),
        confirmLabel: t('updates.install.majorConfirm', { version }),
        run: () => runInstall(version, true),
      });
      return;
    }
    void runInstall(version, false);
  }

  async function runInstall(version: string, allowMajor: boolean) {
    setUpdate({ kind: 'installing', version, percent: null });
    try {
      // The version on screen is the one confirmed; a different major in the
      // manifest is refused rather than installed on this confirmation.
      const result = await installUpdate(allowMajor, version);
      if (result.installed === null) {
        setUpdate({
          kind: 'installError',
          message: t('updates.install.nothing'),
        });
      } else {
        setUpdate({ kind: 'installed', version: result.installed });
      }
    } catch (e) {
      // A rejected signature is the one failure the user must not misread as
      // "try again and it will work": say exactly what happened. An
      // UpdateInstallFailed is already localized as "could not be installed: …"
      // by the IPC layer, so it is shown as is — wrapping it again would print
      // the prefix twice.
      const code = ipcErrorCode(e);
      setUpdate({
        kind: 'installError',
        message:
          code === 'UpdateSignatureInvalid'
            ? t('updates.install.signatureInvalid')
            : code === 'UpdateInstallFailed'
              ? errText(e)
              : t('updates.install.failed', { error: errText(e) }),
      });
    }
  }

  async function restart() {
    try {
      await restartApp();
    } catch (e) {
      toast.error(errText(e));
    }
  }

  async function open(url: string) {
    try {
      await openExternal(url);
    } catch (e) {
      toast.error(errText(e));
    }
  }

  async function revealLogs() {
    try {
      await openLogsFolder();
    } catch (e) {
      toast.error(errText(e));
    }
  }

  async function fileIssue() {
    try {
      // Empty user text is fine: the backend prefills version/os into the body.
      await reportIssue('');
    } catch (e) {
      toast.error(errText(e));
    }
  }

  const channel = settings?.updater.channel;
  // Without a backend there is no version to show — say so rather than repeat a
  // frontend literal that goes stale on the first release bump.
  let versionLine: string;
  if (!info) {
    versionLine = t('version.unavailable', { version: UNKNOWN_VERSION });
  } else if (channel) {
    versionLine = t('version.withChannel', {
      version: info.version,
      buildDate: info.build_date,
      channel: t(CHANNEL_KEY[channel]),
    });
  } else {
    versionLine = t('version.line', {
      version: info.version,
      buildDate: info.build_date,
    });
  }

  const components: { name: string; value: string; license: string }[] = [
    {
      name: 'Pluryprint',
      value: info?.version ?? UNKNOWN_VERSION,
      license: APP_LICENSE,
    },
    { name: 'React', value: REACT_VERSION, license: 'MIT' },
    { name: 'Tauri', value: TAURI_VERSION, license: 'MIT/Apache-2.0' },
  ];

  return (
    <>
      <MPTopBar title={t('title')} />
      <div
        className="mp-scroll"
        style={{ flex: 1, overflow: 'auto', padding: '24px 28px' }}
      >
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 16,
            marginBottom: 18,
          }}
        >
          <MPAppIcon size={56} />
          <div>
            <div
              style={{
                fontSize: 18,
                fontWeight: 600,
                letterSpacing: '-0.01em',
              }}
            >
              Pluryprint{' '}
              <span
                style={{
                  fontSize: 12,
                  fontWeight: 400,
                  letterSpacing: 0,
                  color: 'var(--mp-textMuted)',
                }}
              >
                {t('byline')}
              </span>
            </div>
            <div
              className="mp-mono"
              style={{
                fontSize: 11.5,
                color: 'var(--mp-textMuted)',
                marginTop: 2,
              }}
            >
              {versionLine}
            </div>
            <div
              style={{
                fontSize: 12,
                color: 'var(--mp-textMuted)',
                marginTop: 6,
                maxWidth: 420,
              }}
            >
              {t('tagline')}
            </div>
          </div>
        </div>

        <div style={{ display: 'flex', gap: 8, marginBottom: 24 }}>
          {update.kind === 'checking' ? (
            <MPButton variant="primary" size="sm" disabled>
              <span
                style={{
                  width: 12,
                  height: 12,
                  border: '2px solid currentColor',
                  borderTopColor: 'transparent',
                  opacity: 0.8,
                  borderRadius: '50%',
                  animation: 'mp-spin 0.9s linear infinite',
                  display: 'inline-block',
                }}
              />
              {t('updates.checking')}
            </MPButton>
          ) : (
            <MPButton
              variant="primary"
              icon="rotate"
              size="sm"
              disabled={!tauri}
              onClick={() => void runCheck()}
            >
              {t('updates.check')}
            </MPButton>
          )}
        </div>

        {/* NFR-044: the result of a check (up to date, available, failed,
            deferred, skipped) or of an install replaces the card in place,
            so announce it. */}
        <div aria-live="polite">
          {update.kind === 'deferred' && (
            <div
              style={{
                fontSize: 11.5,
                color: 'var(--mp-textMuted)',
                marginBottom: 24,
              }}
            >
              {t('updates.deferred')}
            </div>
          )}

          {update.kind === 'skipped' && (
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 8,
                fontSize: 11.5,
                color: 'var(--mp-textMuted)',
                marginBottom: 24,
              }}
            >
              <span>{t('updates.skipped', { version: update.version })}</span>
              <MPButton
                variant="ghost"
                size="sm"
                onClick={() => void unskipVersion()}
              >
                {t('updates.undoSkip')}
              </MPButton>
            </div>
          )}

          {update.kind === 'done' && !update.check.available && (
            <>
              <MPSectionLabel>{t('sections.updates')}</MPSectionLabel>
              <MPCard padding={14} style={{ marginBottom: 24 }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                  <div
                    style={{
                      width: 32,
                      height: 32,
                      borderRadius: '50%',
                      background: 'var(--mp-successSoft)',
                      display: 'flex',
                      alignItems: 'center',
                      justifyContent: 'center',
                    }}
                  >
                    <Icon name="check" size={16} color="var(--mp-success)" />
                  </div>
                  <div style={{ flex: 1 }}>
                    <div style={{ fontSize: 13, fontWeight: 500 }}>
                      {t('updates.upToDate.title')}
                    </div>
                    <div
                      className="mp-mono"
                      style={{
                        fontSize: 11,
                        color: 'var(--mp-textDim)',
                        marginTop: 2,
                      }}
                    >
                      {t('updates.upToDate.detail', {
                        version: update.check.current,
                        time: formatClock(update.check.checked_at),
                      })}
                    </div>
                  </div>
                </div>
              </MPCard>
            </>
          )}

          {update.kind === 'unavailable' && (
            <>
              <MPSectionLabel>{t('sections.updates')}</MPSectionLabel>
              <MPCard padding={14} style={{ marginBottom: 24 }}>
                <div
                  role="status"
                  style={{ display: 'flex', alignItems: 'center', gap: 12 }}
                >
                  <Icon name="info" size={16} color="var(--mp-textMuted)" />
                  <div style={{ flex: 1 }}>
                    <div style={{ fontSize: 13, fontWeight: 500 }}>
                      {t('updates.unavailable.title', {
                        channel: t(CHANNEL_KEY[channel ?? 'stable']),
                      })}
                    </div>
                    <div
                      style={{
                        fontSize: 11.5,
                        color: 'var(--mp-textMuted)',
                        marginTop: 2,
                      }}
                    >
                      {t('updates.unavailable.detail')}
                    </div>
                  </div>
                  <MPButton
                    variant="secondary"
                    size="sm"
                    icon="page"
                    onClick={() => void open(RELEASES_URL)}
                  >
                    {t('links.releaseNotes')}
                  </MPButton>
                  <MPButton
                    variant="secondary"
                    size="sm"
                    icon="rotate"
                    onClick={() => void runCheck()}
                  >
                    {t('updates.retry')}
                  </MPButton>
                </div>
              </MPCard>
            </>
          )}

          {update.kind === 'done' && update.check.available && (
            <>
              <MPSectionLabel>{t('sections.updates')}</MPSectionLabel>
              <MPCard
                padding={14}
                style={{
                  marginBottom: 24,
                  background: 'var(--mp-accentSoft)',
                  borderColor: 'var(--mp-accent)',
                }}
              >
                <div
                  role="group"
                  aria-label={t('updates.available.group')}
                  style={{ display: 'flex', alignItems: 'center', gap: 12 }}
                >
                  <div
                    style={{
                      width: 32,
                      height: 32,
                      borderRadius: '50%',
                      background: 'var(--mp-surface)',
                      display: 'flex',
                      alignItems: 'center',
                      justifyContent: 'center',
                    }}
                  >
                    <Icon name="download" size={16} color="var(--mp-accent)" />
                  </div>
                  <div style={{ flex: 1 }}>
                    <div style={{ fontSize: 13, fontWeight: 600 }}>
                      {t('updates.available.title', {
                        version: update.check.latest ?? UNKNOWN_VERSION,
                      })}
                    </div>
                    <div
                      style={{
                        fontSize: 11.5,
                        color: 'var(--mp-textMuted)',
                        marginTop: 2,
                      }}
                    >
                      {t('updates.available.detail', {
                        current: update.check.current,
                      })}{' '}
                      {/* FR-082/FR-083: signature-checked, per-user install
                          (on Linux, per package: see verifiedNoteKey). */}
                      {t(verifiedNoteKey(host))}
                    </div>
                  </div>
                  <div style={{ display: 'flex', gap: 8, flexShrink: 0 }}>
                    <MPButton
                      variant="primary"
                      size="sm"
                      icon="download"
                      onClick={() => requestInstall(update.check)}
                    >
                      {t('updates.available.install')}
                    </MPButton>
                    <MPButton
                      variant="secondary"
                      size="sm"
                      icon="page"
                      onClick={() =>
                        void open(update.check.url ?? RELEASES_URL)
                      }
                    >
                      {t('updates.available.releaseNotes')}
                    </MPButton>
                    {/* FR-084 "defer": session-scoped on purpose — the card comes
                      back next launch, which is what "remind me later" means. */}
                    <MPButton
                      variant="secondary"
                      size="sm"
                      onClick={() => setUpdate({ kind: 'deferred' })}
                    >
                      {t('updates.available.remindLater')}
                    </MPButton>
                    {/* FR-084 "skip this version": persisted, so it survives a
                      restart. Only this release is silenced. Skipping needs a
                      version to name, so it is offered only when the check
                      reported one. */}
                    <SkipVersionButton
                      version={update.check.latest}
                      label={t('updates.available.skipVersion')}
                      onSkip={skipVersion}
                    />
                  </div>
                </div>
              </MPCard>
            </>
          )}

          {update.kind === 'installing' && (
            <>
              <MPSectionLabel>{t('sections.updates')}</MPSectionLabel>
              <MPCard padding={14} style={{ marginBottom: 24 }}>
                <div
                  role="status"
                  style={{ display: 'flex', alignItems: 'center', gap: 10 }}
                >
                  <Icon name="download" size={16} color="var(--mp-accent)" />
                  <div style={{ flex: 1, fontSize: 12.5 }}>
                    {t('updates.install.installing', {
                      version: update.version,
                    })}
                  </div>
                  {update.percent !== null && (
                    <span
                      className="mp-mono"
                      style={{ fontSize: 11.5, color: 'var(--mp-textMuted)' }}
                    >
                      {t('updates.install.progress', {
                        percent: update.percent,
                      })}
                    </span>
                  )}
                </div>
              </MPCard>
            </>
          )}

          {update.kind === 'installed' && (
            <>
              <MPSectionLabel>{t('sections.updates')}</MPSectionLabel>
              <MPCard padding={14} style={{ marginBottom: 24 }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                  <Icon name="check" size={16} color="var(--mp-success)" />
                  <div style={{ flex: 1 }}>
                    <div style={{ fontSize: 13, fontWeight: 500 }}>
                      {t('updates.install.restartTitle', {
                        version: update.version,
                      })}
                    </div>
                    <div
                      style={{
                        fontSize: 11.5,
                        color: 'var(--mp-textMuted)',
                        marginTop: 2,
                      }}
                    >
                      {t('updates.install.restartDetail')}
                    </div>
                  </div>
                  <MPButton
                    variant="primary"
                    size="sm"
                    icon="rotate"
                    onClick={() => void restart()}
                  >
                    {t('updates.install.restart')}
                  </MPButton>
                </div>
              </MPCard>
            </>
          )}

          {update.kind === 'installError' && (
            <>
              <MPSectionLabel>{t('sections.updates')}</MPSectionLabel>
              <MPCard
                padding={14}
                style={{
                  marginBottom: 24,
                  background: 'var(--mp-errorSoft)',
                  borderColor: 'var(--mp-error)',
                }}
              >
                <div
                  role="alert"
                  style={{ display: 'flex', alignItems: 'center', gap: 12 }}
                >
                  <Icon name="alert" size={16} color="var(--mp-error)" />
                  <div
                    style={{
                      flex: 1,
                      fontSize: 12.5,
                      color: 'var(--mp-error)',
                    }}
                  >
                    {update.message}
                  </div>
                  {/* NFR-033: the install is logged step by step. */}
                  <MPButton
                    variant="secondary"
                    size="sm"
                    icon="folder"
                    onClick={() => void revealLogs()}
                  >
                    {t('paths.openLogs')}
                  </MPButton>
                  <MPButton
                    variant="secondary"
                    size="sm"
                    icon="page"
                    onClick={() => void open(RELEASES_URL)}
                  >
                    {t('links.releaseNotes')}
                  </MPButton>
                </div>
              </MPCard>
            </>
          )}

          {update.kind === 'error' && (
            <>
              <MPSectionLabel>{t('sections.updates')}</MPSectionLabel>
              <MPCard
                padding={14}
                style={{
                  marginBottom: 24,
                  background: 'var(--mp-errorSoft)',
                  borderColor: 'var(--mp-error)',
                }}
              >
                <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
                  <Icon name="alert" size={16} color="var(--mp-error)" />
                  <div
                    role="alert"
                    style={{
                      flex: 1,
                      fontSize: 12.5,
                      color: 'var(--mp-error)',
                    }}
                  >
                    {update.message}
                  </div>
                  <MPButton
                    variant="secondary"
                    size="sm"
                    icon="rotate"
                    onClick={() => void runCheck()}
                  >
                    {t('updates.retry')}
                  </MPButton>
                </div>
              </MPCard>
            </>
          )}
        </div>

        <MPSectionLabel>{t('sections.components')}</MPSectionLabel>
        <MPCard padding={0} style={{ marginBottom: 12 }}>
          {components.map((c, i) => (
            <div
              key={c.name}
              style={{
                display: 'grid',
                gridTemplateColumns: '1fr 120px 140px',
                padding: '10px 14px',
                gap: 12,
                borderBottom:
                  i < components.length - 1
                    ? '1px solid var(--mp-border)'
                    : 'none',
                fontSize: 12.5,
              }}
            >
              <span style={{ fontWeight: 500 }}>{c.name}</span>
              <span
                className="mp-mono"
                style={{ color: 'var(--mp-textMuted)' }}
              >
                {c.value}
              </span>
              <span className="mp-mono" style={{ color: 'var(--mp-textDim)' }}>
                {c.license}
              </span>
            </div>
          ))}
          <div
            style={{
              padding: '10px 14px',
              borderTop: '1px solid var(--mp-border)',
              fontSize: 11.5,
              color: 'var(--mp-textMuted)',
            }}
          >
            {t('components.platform')}{' '}
            <span className="mp-mono">
              {info
                ? [
                    info.os,
                    info.arch,
                    host.linuxPackage &&
                      t(`components.packages.${host.linuxPackage}`),
                  ]
                    .filter(Boolean)
                    .join(' · ')
                : '—'}
            </span>
          </div>
        </MPCard>
        <div style={{ marginBottom: 24 }} />

        {info && (
          <>
            <MPSectionLabel>{t('sections.paths')}</MPSectionLabel>
            <MPCard padding={14} style={{ marginBottom: 10 }}>
              <div
                className="mp-mono"
                style={{
                  fontSize: 11,
                  color: 'var(--mp-textMuted)',
                  lineHeight: 1.7,
                }}
              >
                <div>
                  <strong style={{ color: 'var(--mp-text)' }}>
                    {t('paths.config')}
                  </strong>{' '}
                  {info.config_dir}
                </div>
                <div>
                  <strong style={{ color: 'var(--mp-text)' }}>
                    {t('paths.data')}
                  </strong>{' '}
                  {info.data_dir}
                </div>
                <div>
                  <strong style={{ color: 'var(--mp-text)' }}>
                    {t('paths.logs')}
                  </strong>{' '}
                  {info.log_dir}
                </div>
              </div>
            </MPCard>
          </>
        )}
        <div style={{ marginBottom: 24 }}>
          <MPButton
            variant="ghost"
            icon="folder"
            size="sm"
            disabled={!tauri}
            onClick={() => void revealLogs()}
          >
            {t('paths.openLogs')}
          </MPButton>
        </div>

        {infoError && (
          <MPCard
            padding={14}
            style={{
              marginBottom: 24,
              background: 'var(--mp-errorSoft)',
              borderColor: 'var(--mp-error)',
            }}
          >
            <div style={{ fontSize: 12, color: 'var(--mp-error)' }}>
              {t('ipcUnavailable', {
                error: infoError,
                command: DEV_BRIDGE_COMMAND,
              })}
            </div>
          </MPCard>
        )}

        <MPSectionLabel>{t('sections.links')}</MPSectionLabel>
        <div
          style={{
            display: 'flex',
            flexWrap: 'wrap',
            gap: 8,
            marginBottom: 24,
          }}
        >
          <MPButton
            variant="ghost"
            icon="page"
            size="sm"
            disabled={!tauri}
            onClick={() => void open(RELEASES_URL)}
          >
            {t('links.releaseNotes')}
          </MPButton>
          <MPButton
            variant="ghost"
            icon="shield"
            size="sm"
            disabled={!tauri}
            onClick={() => void open(LICENSE_URL)}
          >
            {t('links.license')}
          </MPButton>
          <MPButton
            variant="ghost"
            icon="list"
            size="sm"
            disabled={!tauri}
            onClick={() => void open(NOTICES_URL)}
          >
            {t('links.notices')}
          </MPButton>
          <MPButton
            variant="ghost"
            icon="edit"
            size="sm"
            disabled={!tauri}
            onClick={() => void open(CLA_URL)}
          >
            {t('links.cla')}
          </MPButton>
          <MPButton
            variant="ghost"
            icon="help"
            size="sm"
            disabled={!tauri}
            onClick={() => void open(DOCS_URL)}
          >
            {t('links.documentation')}
          </MPButton>
          <MPButton
            variant="ghost"
            icon="alert"
            size="sm"
            disabled={!tauri}
            onClick={() => void fileIssue()}
          >
            {t('links.reportIssue')}
          </MPButton>
        </div>

        <div
          style={{
            fontSize: 11.5,
            color: 'var(--mp-textDim)',
            lineHeight: 1.7,
          }}
        >
          {t('footer.license', { license: APP_LICENSE })} {t('footer.source')}{' '}
          <button
            type="button"
            className="mp-mono"
            disabled={!tauri}
            onClick={() => void open(REPO_URL)}
            style={{
              background: 'none',
              border: 'none',
              padding: 0,
              font: 'inherit',
              color: 'var(--mp-accent)',
              cursor: tauri ? 'pointer' : 'default',
              textDecoration: 'underline',
            }}
          >
            {REPO_URL.replace('https://', '')}
          </button>
          .
        </div>
      </div>
      {confirm && (
        <ConfirmDialog state={confirm} onClose={() => setConfirm(null)} />
      )}
    </>
  );
}
