import { FileGlyph, MPBadge, StatusDot, type BadgeTone } from '@/components/ui';
import { useT, type TFunction } from '@/i18n/I18nProvider';
import type { UiQueueJobView } from '@/lib/ipc';

import { AttentionActions, type AttentionHandlers } from './AttentionActions';
import { useJobLabels } from './useJobLabels';

interface QueueTimelineViewProps extends AttentionHandlers {
  jobs: UiQueueJobView[];
}

function tone(status: UiQueueJobView['status']): BadgeTone {
  if (status === 'done') return 'success';
  if (status === 'unconfirmed') return 'warn';
  if (status === 'failed') return 'error';
  if (status === 'printing') return 'accent';
  return 'neutral';
}

/**
 * Elapsed time as a compact `12s` / `3m 07s` / `1h 04m` string. The view
 * promises durations, and `startedAt`/`finishedAt` are already formatted to a
 * local clock, so the adapter carries the raw elapsed milliseconds
 * (`durationMs`) for exactly this.
 */
function formatDuration(ms: number, t: TFunction<'queue'>): string {
  const seconds = Math.max(0, Math.round(ms / 1000));
  if (seconds < 60) return t('duration.seconds', { seconds });
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    return t('duration.minutes', { minutes, seconds: seconds % 60 });
  }
  return t('duration.hours', {
    hours: Math.floor(minutes / 60),
    minutes: minutes % 60,
  });
}

export function QueueTimelineView({
  jobs,
  onEnterPassword,
  onInstallDependency,
}: QueueTimelineViewProps) {
  const t = useT('queue');
  const labels = useJobLabels(jobs);
  return (
    // The snapshot keeps jobs in submission order, which is the chronological
    // order this view shows.
    <div
      className="mp-scroll"
      style={{ flex: 1, overflow: 'auto', padding: '14px 20px' }}
    >
      {jobs.map((j, i) => (
        <div
          key={j.id}
          style={{
            display: 'flex',
            gap: 12,
            paddingBottom: 14,
            position: 'relative',
          }}
        >
          <div
            style={{
              display: 'flex',
              flexDirection: 'column',
              alignItems: 'center',
              flexShrink: 0,
              paddingTop: 4,
            }}
          >
            <StatusDot status={j.status} />
            {i < jobs.length - 1 && (
              <div
                style={{
                  flex: 1,
                  width: 1,
                  background: 'var(--mp-border)',
                  marginTop: 4,
                }}
              />
            )}
          </div>
          <div style={{ flex: 1, minWidth: 0, paddingBottom: 6 }}>
            <div
              className="mp-mono"
              style={{
                fontSize: 10.5,
                color: 'var(--mp-textDim)',
                marginBottom: 2,
              }}
            >
              {j.finishedAt
                ? t('job.times', { from: j.startedAt, to: j.finishedAt })
                : j.startedAt}
              {j.durationMs !== undefined
                ? ` · ${t('job.took', {
                    duration: formatDuration(j.durationMs, t),
                  })}`
                : ''}
            </div>
            <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
              <FileGlyph ext={j.ext} size={18} />
              <span style={{ fontSize: 12.5, fontWeight: 500 }}>{j.file}</span>
              {/* Every `JobState` has a `status.<state>` entry. */}
              <MPBadge tone={tone(j.status)}>{t(`status.${j.status}`)}</MPBadge>
            </div>
            <div
              className="mp-mono"
              style={{
                fontSize: 10.5,
                color: 'var(--mp-textMuted)',
                marginTop: 4,
              }}
            >
              {labels.origin(j) && `${labels.origin(j)} · `}
              {t('job.meta', {
                profile: j.profile,
                printer: labels.printer(j),
              })}
              {j.pages != null && ` · ${t('job.pages', { count: j.pages })}`}
              {j.noteKey ? ` · ${t(`note.${j.noteKey}`)}` : ''}
              {labels.error(j) ? ` · ${labels.error(j)}` : ''}
            </div>
            <AttentionActions
              job={j}
              onEnterPassword={onEnterPassword}
              onInstallDependency={onInstallDependency}
            />
          </div>
        </div>
      ))}
    </div>
  );
}
