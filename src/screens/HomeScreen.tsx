import { useEffect, useState, type CSSProperties, type ReactNode } from 'react';
import { toast } from 'sonner';

import { Icon, type IconName } from '@/components/Icon';
import {
  APP_SHORTCUTS,
  shortcutLabel,
  type ShortcutId,
} from '@/components/AppShell/MPAppShell';
import {
  MPButton,
  MPCard,
  MPKbd,
  MPSectionLabel,
  FileGlyph,
  StatusDot,
  MPTopBar,
} from '@/components/ui';
import { useLocale, useT } from '@/i18n/I18nProvider';
import {
  cliCommandForDisplay,
  cliExecutable,
  cliPrintCommand,
  cliProgram,
  formatFinished,
} from '@/lib/format';
import { fetchAppInfo, isTauri, openLogsFolder } from '@/lib/ipc';
import {
  appImageFileName,
  linuxMenuKey,
  rememberedHostInfo,
  useHostPlatform,
} from '@/lib/platform';
import { findProfileIn } from '@/lib/profiles';
import { useProfilesStore } from '@/stores/profilesStore';
import { useQueueStore } from '@/stores/queueStore';
import { useJobLabels } from '@/screens/QueueViews/useJobLabels';

/**
 * Re-exported for `App`'s OS file-picker filter; the list itself lives in
 * `@/lib/formats` so every picker and the drop zone share one mirror of the
 * engine's `SUPPORTED_EXTS`.
 */
export { PRINTABLE_EXTENSIONS } from '@/lib/formats';

/** What a Home affordance asks the app to do; every one of them is wired. */
export type HomeAction = 'newjob' | 'queue' | 'files' | 'folder';

interface HomeScreenProps {
  onAction?: (action: HomeAction) => void;
}

/**
 * A card-shaped control. A real `<button>` rather than a `<div onClick>`: the
 * quick actions have to be reachable by Tab and activatable with Enter/Space
 * (NFR-041), and the focus ring comes free with `.mp-focus` (NFR-042).
 */
function CardButton({
  onClick,
  style,
  children,
  ...rest
}: {
  onClick: () => void;
  style?: CSSProperties;
  children: ReactNode;
  title?: string | undefined;
  'aria-label'?: string;
  'aria-expanded'?: boolean;
  'aria-controls'?: string;
}) {
  const [hover, setHover] = useState(false);
  return (
    <button
      type="button"
      onClick={onClick}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
      className="mp-focus"
      {...rest}
      style={{
        display: 'block',
        width: '100%',
        textAlign: 'left',
        font: 'inherit',
        color: 'inherit',
        background: 'var(--mp-surface)',
        border: '1px solid',
        borderColor: hover ? 'var(--mp-borderStrong)' : 'var(--mp-border)',
        borderRadius: 8,
        boxShadow: hover ? '0 2px 6px rgba(0,0,0,0.04)' : 'none',
        cursor: 'pointer',
        transition: 'box-shadow .15s, border-color .12s',
        ...style,
      }}
    >
      {children}
    </button>
  );
}

/** Centered notice for the recent-activity card (loading / error / empty). */
function RecentNotice({
  title,
  body,
  action,
}: {
  title: string;
  body?: string;
  action?: ReactNode;
}) {
  return (
    <div
      style={{
        padding: '18px 14px',
        textAlign: 'center',
        fontSize: 12,
        color: 'var(--mp-textMuted)',
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        gap: 6,
      }}
    >
      <div>{title}</div>
      {body && <div style={{ color: 'var(--mp-textDim)' }}>{body}</div>}
      {action}
    </div>
  );
}

export function HomeScreen({ onAction }: HomeScreenProps) {
  const t = useT('home');
  // Job states are labelled by the queue namespace so Home and the Queue screen
  // can never disagree about what "converted" is called.
  const tQueue = useT('queue');
  const tCommon = useT('common');
  const locale = useLocale();
  const recent = useQueueStore((s) => s.recentDone);
  const labels = useJobLabels(recent);
  const queueStatus = useQueueStore((s) => s.status);
  const queueError = useQueueStore((s) => s.error);
  const initQueue = useQueueStore((s) => s.init);
  const profiles = useProfilesStore((s) => s.profiles);
  const [expanded, setExpanded] = useState<string | null>(null);

  // The CLI exists (FR-008), so the card shows the command a user can really
  // run, built around the profile their jobs default to rather than a made-up
  // name. "Really run" means two things the bare `pluryprint …` line did not
  // honour: the executable is named by its full path (nothing puts it on PATH
  // yet, FR-006), and every argument is quoted for the user's shell, so a
  // profile called `Costs $HOME` reaches the CLI intact.
  const [exe, setExe] = useState<{ path: string | null; os?: string }>({
    path: null,
  });
  useEffect(() => {
    if (!isTauri()) return;
    let alive = true;
    fetchAppInfo()
      .then((info) => {
        if (alive) setExe({ path: info.executable ?? null, os: info.os });
      })
      .catch((e: unknown) => {
        // The card still works: it falls back to the bare name and says why.
        console.error('app info for the CLI command failed', e);
      });
    return () => {
      alive = false;
    };
  }, []);
  const defaultProfile = findProfileIn(profiles, null);
  // The backend's answer when there is one; the webview's platform until then
  // (or in the web preview). On Windows the command-line program is the
  // console `pluryprint-cli`; on Linux it depends on the package (the .deb
  // puts `pluryprint` on PATH, an AppImage runs by its own file name).
  const host = useHostPlatform();
  const cliOs = exe.os ?? host.os;
  const hostInfo = rememberedHostInfo();
  const cliLinux = {
    linuxPackage: host.linuxPackage,
    appImage: appImageFileName(hostInfo?.version, hostInfo?.arch),
  };
  const cliCommand = cliPrintCommand({
    executable: exe.path,
    os: cliOs,
    profile: defaultProfile.name,
    ...cliLinux,
  });
  // The path the command really names: none for the .deb (on PATH) or for an
  // AppImage's temporary mount, which the command must not point at.
  const cliPath = cliExecutable({
    executable: exe.path,
    os: cliOs,
    linuxPackage: host.linuxPackage,
  });
  const onPath = cliOs === 'linux' && host.linuxPackage === 'deb';
  const appImage = cliOs === 'linux' && host.linuxPackage === 'appimage';
  const cliCommandShown = cliCommandForDisplay(cliCommand, cliPath);

  // Twenty-five extensions do not fit on a drop zone; the copy names the
  // families instead (pinned against `PRINTABLE_EXTENSIONS` by the tests).
  const formats = t('dropzone.formatList');

  async function copyCliCommand() {
    try {
      await navigator.clipboard.writeText(cliCommand);
      toast.success(t('cli.copied'));
    } catch {
      // No clipboard permission, or no clipboard at all (the standalone web
      // preview): the command is on screen, so say how to get it manually.
      toast.error(t('cli.copyFailed'));
    }
  }

  const quickActions: {
    id: string;
    icon: IconName;
    title: string;
    body: string;
    kbd?: ShortcutId;
    run: () => void;
  }[] = [
    {
      id: 'newjob',
      icon: 'plus',
      title: t('quickActions.newJob.title'),
      body: t('quickActions.newJob.body'),
      kbd: 'newJob',
      run: () => onAction?.('newjob'),
    },
    {
      id: 'folder',
      icon: 'folder',
      title: t('quickActions.folder.title'),
      body: t('quickActions.folder.body'),
      kbd: 'printFolder',
      run: () => onAction?.('folder'),
    },
    {
      id: 'cli',
      icon: 'cmd',
      title: t('quickActions.cli.title'),
      body: t('quickActions.cli.body'),
      run: () => void copyCliCommand(),
    },
  ];

  let recentBody: ReactNode;
  if (queueStatus === 'unavailable') {
    recentBody = <RecentNotice title={t('recent.unavailable')} />;
  } else if (queueStatus === 'error') {
    // A failed read is not an empty history: saying "no recent activity yet"
    // here would hide a backend failure behind a reassuring sentence (NFR-033).
    recentBody = (
      <RecentNotice
        title={t('recent.errorTitle')}
        body={queueError ?? t('recent.errorHint')}
        action={
          <div style={{ display: 'flex', gap: 6 }}>
            <MPButton
              variant="ghost"
              icon="rotate"
              size="sm"
              onClick={() => void initQueue()}
            >
              {tCommon('actions.retry')}
            </MPButton>
            {/* NFR-033: every error offers a way to the logs. */}
            <MPButton
              variant="ghost"
              icon="folder"
              size="sm"
              onClick={() => {
                openLogsFolder().catch((e: unknown) => {
                  toast.error(e instanceof Error ? e.message : String(e));
                });
              }}
            >
              {tCommon('actions.openLogsFolder')}
            </MPButton>
          </div>
        }
      />
    );
  } else if (queueStatus === 'idle' || queueStatus === 'loading') {
    recentBody = <RecentNotice title={t('recent.loading')} />;
  } else if (recent.length === 0) {
    recentBody = <RecentNotice title={t('recent.empty')} />;
  } else {
    recentBody = recent.map((j, i, arr) => {
      const open = expanded === j.id;
      const detailsId = `recent-details-${j.id}`;
      // NFR-044/NFR-045: the label carries the status too — an aria-label
      // replaces the row's visible text, so without it a screen reader could
      // not tell a failed job from a printed one.
      const status = tQueue(`status.${j.status}`);
      const border =
        i < arr.length - 1 || open ? '1px solid var(--mp-border)' : 'none';
      return (
        <div key={j.id}>
          <CardButton
            aria-expanded={open}
            {...(open ? { 'aria-controls': detailsId } : {})}
            aria-label={
              open
                ? t('recent.collapse', { file: j.file, status })
                : t('recent.expand', { file: j.file, status })
            }
            onClick={() => setExpanded(open ? null : j.id)}
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 10,
              padding: '10px 14px',
              background: 'transparent',
              border: 'none',
              borderBottom: border,
              borderRadius: 0,
              boxShadow: 'none',
            }}
          >
            <FileGlyph ext={j.ext} size={22} />
            <div style={{ flex: 1, minWidth: 0 }}>
              <div
                style={{
                  fontSize: 12.5,
                  fontWeight: 500,
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                  whiteSpace: 'nowrap',
                }}
              >
                {j.file}
              </div>
              <div
                className="mp-mono"
                style={{
                  fontSize: 10.5,
                  color: 'var(--mp-textDim)',
                  marginTop: 1,
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                  whiteSpace: 'nowrap',
                }}
              >
                {tQueue('job.meta', {
                  profile: j.profile,
                  printer: labels.printer(j),
                })}
              </div>
            </div>
            <StatusDot status={j.status} />
            <span
              style={{
                fontSize: 10.5,
                color: 'var(--mp-textMuted)',
                textTransform: 'uppercase',
                minWidth: 60,
                textAlign: 'right',
              }}
            >
              {/* Every `JobState` has a `status.<state>` entry. */}
              {tQueue(`status.${j.status}`)}
            </span>
          </CardButton>
          {open && (
            <div
              id={detailsId}
              style={{
                padding: '10px 14px 12px 46px',
                borderBottom: i < arr.length - 1 ? border : 'none',
                display: 'flex',
                flexDirection: 'column',
                gap: 4,
                fontSize: 11.5,
                color: 'var(--mp-textMuted)',
              }}
            >
              <div>
                {t('recent.printer')}: {labels.printer(j)}
              </div>
              <div>
                {t('recent.profile')}: {j.profile}
              </div>
              <div>
                {t('recent.pages')}: {j.pages ?? t('recent.pagesUnknown')}
              </div>
              {j.finishedAt && (
                <div>
                  {t('recent.finished')}:{' '}
                  {(j.finishedAtIso &&
                    formatFinished(j.finishedAtIso, locale)) ??
                    j.finishedAt}
                </div>
              )}
              {labels.error(j) &&
                // Unconfirmed: handed to the spooler, nobody confirmed it
                // printed. A warning, as everywhere else — calling it an
                // error invites a reprint, the duplicate it warns about.
                (j.status === 'unconfirmed' ? (
                  <div style={{ color: 'var(--mp-warn)' }}>
                    {t('recent.unconfirmed')}: {labels.error(j)}
                  </div>
                ) : (
                  <div style={{ color: 'var(--mp-error)' }}>
                    {t('recent.error')}: {labels.error(j)}
                  </div>
                ))}
              <div style={{ marginTop: 4 }}>
                <MPButton
                  variant="ghost"
                  icon="queue"
                  size="sm"
                  onClick={() => onAction?.('queue')}
                >
                  {t('recent.openQueue')}
                </MPButton>
              </div>
            </div>
          )}
        </div>
      );
    });
  }

  return (
    <>
      <MPTopBar
        title={t('title')}
        subtitle={t('subtitle', { shortcut: shortcutLabel('newJob') })}
        right={
          <MPButton
            variant="primary"
            icon="plus"
            onClick={() => onAction?.('newjob')}
          >
            {t('quickActions.newJob.title')}
          </MPButton>
        }
      />
      <div
        className="mp-scroll"
        style={{ flex: 1, overflow: 'auto', padding: 20 }}
      >
        {/* Dropping works from any screen (App owns the native listener); the
            zone is also a button so the same job can be started without a
            mouse and without a file manager. */}
        <CardButton
          onClick={() => onAction?.('files')}
          aria-label={t('dropzone.action')}
          style={{
            border: '1.5px dashed var(--mp-borderStrong)',
            borderRadius: 10,
            padding: '28px 20px',
            textAlign: 'center',
            marginBottom: 20,
          }}
        >
          <div
            style={{
              display: 'inline-flex',
              padding: 10,
              background: 'var(--mp-surface2)',
              borderRadius: 10,
              marginBottom: 10,
            }}
          >
            <Icon name="upload" size={20} color="var(--mp-textMuted)" />
          </div>
          <div style={{ fontSize: 14, fontWeight: 500, marginBottom: 4 }}>
            {t('dropzone.title')}
          </div>
          <div style={{ fontSize: 12, color: 'var(--mp-textMuted)' }}>
            {t('dropzone.formats', { formats })}
          </div>
          <div
            style={{ fontSize: 11.5, color: 'var(--mp-textDim)', marginTop: 4 }}
          >
            {t('dropzone.folders')}
          </div>
        </CardButton>

        <MPSectionLabel>{t('quickActions.label')}</MPSectionLabel>
        <div
          style={{
            display: 'grid',
            // minmax(0, 1fr): a long unbreakable line (the CLI command) must
            // not widen its column past a third.
            gridTemplateColumns: 'repeat(3, minmax(0, 1fr))',
            gap: 10,
            marginBottom: 24,
          }}
        >
          {quickActions.map((q) => (
            <CardButton
              key={q.id}
              onClick={q.run}
              title={q.id === 'cli' ? cliCommand : undefined}
              style={{ padding: 14, minWidth: 0 }}
            >
              <div
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  marginBottom: 8,
                  minHeight: 18,
                }}
              >
                <Icon name={q.icon} size={16} color="var(--mp-accent)" />
                {q.kbd && <MPKbd>{shortcutLabel(q.kbd)}</MPKbd>}
              </div>
              <div style={{ fontSize: 13, fontWeight: 500 }}>{q.title}</div>
              <div
                style={{
                  fontSize: 11.5,
                  color: 'var(--mp-textMuted)',
                  marginTop: 2,
                }}
              >
                {q.body}
              </div>
              {q.id === 'cli' && (
                <div
                  className="mp-mono"
                  style={{
                    fontSize: 10.5,
                    color: 'var(--mp-textDim)',
                    marginTop: 6,
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap',
                  }}
                >
                  {cliCommandShown}
                </div>
              )}
              {q.id === 'cli' && (
                <div
                  style={{
                    fontSize: 10.5,
                    color: 'var(--mp-textDim)',
                    marginTop: 4,
                  }}
                >
                  {onPath
                    ? t('cli.onPathNote')
                    : cliPath
                      ? t('cli.pathNote')
                      : appImage
                        ? t('cli.noPathNoteAppImage', {
                            program: cliProgram(cliOs, cliLinux),
                          })
                        : t('cli.noPathNote', {
                            program: cliProgram(cliOs, cliLinux),
                          })}
                </div>
              )}
            </CardButton>
          ))}
        </div>

        <div
          style={{
            display: 'flex',
            justifyContent: 'space-between',
            alignItems: 'center',
            marginBottom: 8,
          }}
        >
          <MPSectionLabel>{t('recent.label')}</MPSectionLabel>
          <button
            onClick={() => onAction?.('queue')}
            className="mp-focus"
            style={{
              background: 'none',
              border: 'none',
              cursor: 'pointer',
              fontSize: 11.5,
              color: 'var(--mp-textMuted)',
              display: 'inline-flex',
              alignItems: 'center',
              gap: 3,
            }}
          >
            {t('recent.viewAll')}
            <Icon name="chevr" size={11} />
          </button>
        </div>
        <MPCard padding={0}>{recentBody}</MPCard>

        {/* NFR-034 asks for the shortcuts to be documented, not just to work. */}
        <div style={{ marginTop: 24 }}>
          <MPSectionLabel>{t('shortcuts.label')}</MPSectionLabel>
        </div>
        <MPCard padding={14}>
          <div
            style={{
              display: 'grid',
              gridTemplateColumns: '1fr 1fr',
              gap: '6px 16px',
            }}
          >
            {APP_SHORTCUTS.map((s) => (
              <div
                key={s.id}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  gap: 10,
                  fontSize: 12,
                }}
              >
                <span>{t(`shortcuts.${s.id}`)}</span>
                <MPKbd>{shortcutLabel(s.id)}</MPKbd>
              </div>
            ))}
          </div>
          <div
            style={{
              fontSize: 11.5,
              color: 'var(--mp-textDim)',
              marginTop: 10,
            }}
          >
            {t('shortcuts.hint')}
          </div>
        </MPCard>

        <div
          style={{
            marginTop: 20,
            padding: '10px 12px',
            background: 'var(--mp-infoSoft)',
            borderRadius: 6,
            display: 'flex',
            gap: 8,
            alignItems: 'flex-start',
          }}
        >
          <Icon name="info" size={14} color="var(--mp-info)" />
          <div
            style={{ fontSize: 12, color: 'var(--mp-info)', lineHeight: 1.5 }}
          >
            <strong>{t('tip.label')} · </strong>
            {/* FR-001: the Finder Services entry ships on macOS; on Windows
                the installer registers the Explorer entry (classic menu, so
                "Show more options" on Windows 11); on Linux the .deb adds
                "Open with" and the Dolphin/Nemo actions, while an AppImage
                adds nothing until the user integrates it. */}
            {t(
              cliOs === 'linux'
                ? 'tip.bodyLinux'
                : cliOs === 'macos'
                  ? 'tip.bodyMac'
                  : 'tip.body',
              {
                command: `${cliProgram(cliOs, cliLinux)} print --files`,
                // Same condition as the card's own note: only claim the card
                // copies the full path when it actually knows it.
                path: onPath
                  ? t('tip.pathOnPath')
                  : cliPath
                    ? t('tip.pathCopied')
                    : appImage
                      ? t('tip.pathAppImage')
                      : t('tip.pathManual'),
                menu: t(linuxMenuKey(host.linuxPackage)),
              },
            )}
          </div>
        </div>
      </div>
    </>
  );
}
