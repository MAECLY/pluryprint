import { FileGlyph, MPCard, StatusDot } from '@/components/ui';
import { useT } from '@/i18n/I18nProvider';
import type { UiQueueJobView } from '@/lib/ipc';
import { AttentionActions, type AttentionHandlers } from './AttentionActions';
import { JobProgressBar } from './QueueListView';
import { useJobLabels } from './useJobLabels';

interface QueueBoardViewProps extends AttentionHandlers {
  jobs: UiQueueJobView[];
}

export function QueueBoardView({
  jobs,
  onEnterPassword,
  onInstallDependency,
}: QueueBoardViewProps) {
  const t = useT('queue');
  const labels = useJobLabels(jobs);
  const cols = [
    {
      key: 'queued',
      label: t('board.queued'),
      // `converted` (PDF ready, waiting for a printer) has no column of its own;
      // it belongs with the not-yet-printing work.
      items: jobs.filter((j) =>
        ['queued', 'paused', 'converted'].includes(j.status),
      ),
    },
    {
      key: 'converting',
      label: t('board.converting'),
      items: jobs.filter((j) => j.status === 'converting'),
    },
    {
      key: 'printing',
      label: t('board.printing'),
      items: jobs.filter((j) => j.status === 'printing'),
    },
    {
      key: 'done',
      label: t('board.done'),
      items: jobs.filter((j) =>
        ['done', 'unconfirmed', 'failed', 'cancelled'].includes(j.status),
      ),
    },
  ];
  return (
    <div
      className="mp-scroll"
      style={{
        flex: 1,
        overflow: 'auto',
        padding: 14,
        display: 'grid',
        // minmax(0, …): a long file name ellipsizes instead of widening its
        // column past the window at 880 px.
        gridTemplateColumns: 'repeat(4, minmax(0, 1fr))',
        alignItems: 'start',
        gap: 12,
      }}
    >
      {cols.map((c) => (
        <div
          key={c.key}
          style={{
            display: 'flex',
            flexDirection: 'column',
            gap: 8,
            minWidth: 0,
          }}
        >
          <div
            style={{
              display: 'flex',
              justifyContent: 'space-between',
              padding: '2px 4px',
            }}
          >
            <span
              className="mp-mono"
              style={{
                fontSize: 10.5,
                color: 'var(--mp-textMuted)',
                textTransform: 'uppercase',
                letterSpacing: '0.08em',
              }}
            >
              {c.label}
            </span>
            <span
              className="mp-mono"
              style={{ fontSize: 10.5, color: 'var(--mp-textDim)' }}
            >
              {c.items.length}
            </span>
          </div>
          {c.items.map((j) => (
            <MPCard key={j.id} padding={10} hoverable>
              <div
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 8,
                  marginBottom: 6,
                }}
              >
                <FileGlyph ext={j.ext} size={18} />
                <span
                  title={j.file}
                  style={{
                    fontSize: 11.5,
                    fontWeight: 500,
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap',
                    flex: 1,
                    minWidth: 0,
                  }}
                >
                  {j.file}
                </span>
              </div>
              <div
                className="mp-mono"
                style={{
                  fontSize: 10,
                  color: 'var(--mp-textDim)',
                  marginBottom: 8,
                }}
              >
                {/* An unknown count is left out, never shown as "0p". */}
                {labels.origin(j) && `${labels.origin(j)} · `}
                {j.pages != null && `${t('job.pages', { count: j.pages })} · `}
                {j.profile}
                {j.noteKey ? ` · ${t(`note.${j.noteKey}`)}` : ''}
              </div>
              {/* FR-031 / WCAG 1.4.1: the Queued and Done columns mix several
                  states, so each card says its state in words — the dot's
                  colour alone cannot tell failed from printed. */}
              <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                <StatusDot status={j.status} />
                <span
                  style={{
                    fontSize: 10,
                    color:
                      j.status === 'failed'
                        ? 'var(--mp-error)'
                        : 'var(--mp-textMuted)',
                    textTransform: 'uppercase',
                    whiteSpace: 'nowrap',
                  }}
                >
                  {t(`status.${j.status}`)}
                </span>
                <div style={{ flex: 1 }}>
                  <JobProgressBar job={j} height={3} t={t} />
                </div>
              </div>
              {labels.error(j) && (
                <div
                  style={{
                    fontSize: 10.5,
                    // Unconfirmed is a warning, not a failure (FR-071).
                    color:
                      j.status === 'unconfirmed'
                        ? 'var(--mp-warn)'
                        : 'var(--mp-error)',
                    marginTop: 6,
                    overflowWrap: 'anywhere',
                  }}
                >
                  {labels.error(j)}
                </div>
              )}
              <AttentionActions
                job={j}
                onEnterPassword={onEnterPassword}
                onInstallDependency={onInstallDependency}
              />
            </MPCard>
          ))}
        </div>
      ))}
    </div>
  );
}
