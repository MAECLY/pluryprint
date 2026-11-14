import { useCallback, useEffect, useRef, useState } from 'react';

import { FileGlyph, MPIconButton, StatusDot } from '@/components/ui';
import { useT, type TFunction } from '@/i18n/I18nProvider';
import { useQueueStore } from '@/stores/queueStore';
import {
  getPrinterPrefs,
  isTauri,
  listPrinters,
  type PrinterPrefs,
  type UiQueueJobView,
} from '@/lib/ipc';
import type { JobChange } from '@/ipc/bindings';
import type { ConfirmState } from '@/components/ConfirmDialog';
import { needsPassword } from '@/lib/pdfPassword';

import { missingDependency } from '@/lib/deps';
import type { AttentionHandlers } from './AttentionActions';
import { useJobLabels } from './useJobLabels';

interface QueueListViewProps extends AttentionHandlers {
  jobs: UiQueueJobView[];
  /** Request an in-app confirmation (used for cancelling an in-flight job). */
  onConfirm: (state: ConfirmState) => void;
}

const TERMINAL = new Set(['done', 'unconfirmed', 'failed', 'cancelled']);

/** A printer a paused job can be sent to instead. */
interface PrinterOption {
  id: string;
  name: string;
}

/**
 * What a paused job can be changed to so it prints (FR-040/FR-044): the way
 * out its pause message promises. A job paused because its printer cannot
 * print two-sided can go one-sided; one paused for an unsupported setting or
 * a printer that cannot be used can go to another printer. `null` for any
 * other job (a user pause, a missing dependency, a password).
 */
function pauseFixes(
  j: Pick<UiQueueJobView, 'status' | 'errorInfo'>,
): { oneSided: boolean; otherPrinter: boolean } | null {
  if (j.status !== 'paused' || !j.errorInfo) return null;
  const { code, reason } = j.errorInfo;
  const oneSided = code === 'unsupported_setting' && reason === 'duplex';
  const otherPrinter =
    code === 'unsupported_setting' ||
    code === 'printer_unavailable' ||
    code === 'printer_disabled';
  return oneSided || otherPrinter ? { oneSided, otherPrinter } : null;
}

/** A reload asked for sooner than this after the last one is skipped. */
const PRINTERS_REFRESH_MS = 5000;

/** The printers a paused job can be sent to, and how loading them went. */
interface UsablePrinters {
  printers: PrinterOption[];
  /** The last load failed: the row says so and offers a retry. */
  failed: boolean;
  /**
   * Load the list again. `force` skips the throttle (an explicit retry);
   * otherwise a reload right after the previous one is skipped.
   */
  reload: (force?: boolean) => void;
}

/**
 * The printers a job can be moved to: installed and not removed from
 * Pluryprint. Loaded only while some job needs the choice, and again when the
 * window regains focus or the user opens the list — a printer added in the
 * meantime shows up without leaving the Queue. A failed load is reported,
 * never swallowed: the "send to another printer" choice must not vanish
 * without a word.
 */
function useUsablePrinters(needed: boolean): UsablePrinters {
  const [printers, setPrinters] = useState<PrinterOption[]>([]);
  const [failed, setFailed] = useState(false);
  const alive = useRef(true);
  const lastLoad = useRef(0);

  const reload = useCallback((force = false) => {
    if (!isTauri()) return;
    const now = Date.now();
    if (!force && now - lastLoad.current < PRINTERS_REFRESH_MS) return;
    lastLoad.current = now;
    Promise.all([
      listPrinters(),
      getPrinterPrefs().catch((): PrinterPrefs => ({})),
    ])
      .then(([ps, prefs]) => {
        if (!alive.current) return;
        const disabled = new Set(prefs.disabled ?? []);
        setPrinters(
          ps
            .filter((p) => !disabled.has(p.id))
            .map((p) => ({ id: p.id, name: p.name })),
        );
        setFailed(false);
      })
      .catch(() => {
        if (alive.current) setFailed(true);
      });
  }, []);

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  useEffect(() => {
    if (!needed) return;
    reload(true);
    const onFocus = () => reload();
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, [needed, reload]);

  return { printers, failed, reload };
}

/**
 * A job's progress bar, honest about what the backend actually knows.
 *
 * `indeterminate` jobs (the printing phase: no spooler reports a completion
 * fraction) get a sliding pulse and NO `aria-valuenow`, which is exactly how
 * ARIA spells "in progress, amount unknown". Painting them at 0 % — or making
 * a percentage up — would be a lie. Shared with the Board view, which renders
 * the same bar at a smaller size.
 */
export function JobProgressBar({
  job,
  height,
  t,
}: {
  job: UiQueueJobView;
  height: number;
  t: TFunction<'queue'>;
}) {
  const fill =
    job.status === 'failed'
      ? 'var(--mp-error)'
      : job.status === 'done'
        ? 'var(--mp-success)'
        : job.status === 'unconfirmed'
          ? 'var(--mp-warn)'
          : 'var(--mp-accent)';
  const label = t('progress.label', { file: job.file });
  return (
    <div
      role="progressbar"
      aria-label={label}
      {...(job.indeterminate
        ? { 'aria-valuetext': t('progress.indeterminate') }
        : {
            'aria-valuemin': 0,
            'aria-valuemax': 100,
            'aria-valuenow': Math.round(job.progress * 100),
          })}
      title={job.indeterminate ? t('progress.indeterminate') : undefined}
      style={{
        height,
        background: 'var(--mp-surface3)',
        borderRadius: 3,
        overflow: 'hidden',
        position: 'relative',
      }}
    >
      {job.indeterminate ? (
        <div
          style={{
            position: 'absolute',
            top: 0,
            bottom: 0,
            left: 0,
            width: '25%',
            background: fill,
            animation: 'mp-progress 1.4s ease-in-out infinite',
          }}
        />
      ) : (
        <div
          style={{
            position: 'absolute',
            inset: 0,
            width: `${job.progress * 100}%`,
            background: fill,
            transition: 'width .3s',
          }}
        />
      )}
    </div>
  );
}

interface RowProps {
  j: UiQueueJobView;
  t: TFunction<'queue'>;
  labels: ReturnType<typeof useJobLabels>;
  onPause: () => void;
  onResume: () => void;
  onRetry: () => void;
  onCancel: () => void;
  onEnterPassword: () => void;
  /** Offer to install what the paused job is missing (FR-058). */
  onInstallDependency: () => void;
  /** Change the paused job, then resume it (see {@link pauseFixes}). */
  onAdjust: (change: JobChange) => void;
  /** Where a paused job can be sent instead, and whether loading it failed. */
  printers: UsablePrinters;
  /** A command for this job is still in flight: no second click (NFR-033). */
  busy: boolean;
}

function Row({
  j,
  t,
  labels,
  onPause,
  onResume,
  onRetry,
  onCancel,
  onEnterPassword,
  onInstallDependency,
  onAdjust,
  printers,
  busy,
}: RowProps) {
  const tCommon = useT('common');
  const terminal = TERMINAL.has(j.status);
  const fixes = pauseFixes(j);
  const targets = printers.printers.filter((p) => p.id !== j.printer);
  const error = labels.error(j);
  const meta = [
    labels.origin(j) ?? null,
    t('job.meta', { profile: j.profile, printer: labels.printer(j) }),
    j.noteKey ? t(`note.${j.noteKey}`) : null,
    error ?? null,
  ]
    .filter(Boolean)
    .join(' · ');
  const canPause =
    j.status === 'queued' ||
    j.status === 'converting' ||
    j.status === 'converted';
  return (
    <div
      style={{
        display: 'grid',
        // Room for three row actions (install, resume, cancel).
        gridTemplateColumns: '24px 1fr 110px 90px 110px 82px',
        gap: 10,
        alignItems: 'center',
        padding: '8px 14px',
        borderBottom: '1px solid var(--mp-border)',
      }}
    >
      <FileGlyph ext={j.ext} size={20} />
      <div style={{ minWidth: 0 }}>
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
          title={meta}
          style={{
            fontSize: 10.5,
            color: 'var(--mp-textDim)',
            marginTop: 1,
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            whiteSpace: 'nowrap',
          }}
        >
          {meta}
        </div>
        {fixes && (
          <div
            style={{
              display: 'flex',
              flexWrap: 'wrap',
              gap: 6,
              marginTop: 4,
              alignItems: 'center',
            }}
          >
            {fixes.oneSided && (
              <button
                type="button"
                disabled={busy}
                aria-label={t('fix.oneSidedLabel', { file: j.file })}
                onClick={() => onAdjust({ printer: null, duplex: 'none' })}
                style={{
                  fontSize: 11,
                  padding: '2px 8px',
                  borderRadius: 5,
                  border: '1px solid var(--mp-border)',
                  background: 'var(--mp-surface)',
                  color: 'var(--mp-text)',
                  cursor: busy ? 'default' : 'pointer',
                }}
              >
                {t('fix.oneSided')}
              </button>
            )}
            {fixes.otherPrinter && printers.failed && (
              <span
                role="alert"
                style={{
                  display: 'inline-flex',
                  alignItems: 'center',
                  gap: 6,
                  fontSize: 11,
                  color: 'var(--mp-warn)',
                }}
              >
                {t('fix.printersFailed')}
                <button
                  type="button"
                  className="mp-focus"
                  onClick={() => printers.reload(true)}
                  style={{
                    fontSize: 11,
                    padding: '2px 8px',
                    borderRadius: 5,
                    border: '1px solid var(--mp-border)',
                    background: 'var(--mp-surface)',
                    color: 'var(--mp-text)',
                    cursor: 'pointer',
                  }}
                >
                  {tCommon('actions.retry')}
                </button>
              </span>
            )}
            {fixes.otherPrinter && targets.length > 0 && (
              <select
                aria-label={t('fix.sendToLabel', { file: j.file })}
                disabled={busy}
                // Opening the list refreshes it: a printer added since the
                // last load appears without leaving the Queue.
                onFocus={() => printers.reload()}
                value=""
                onChange={(e) => {
                  if (e.target.value) {
                    onAdjust({ printer: e.target.value, duplex: null });
                  }
                }}
                style={{
                  fontSize: 11,
                  padding: '2px 6px',
                  borderRadius: 5,
                  border: '1px solid var(--mp-border)',
                  background: 'var(--mp-surface)',
                  color: 'var(--mp-text)',
                  maxWidth: 220,
                }}
              >
                <option value="">{t('fix.sendTo')}</option>
                {targets.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                  </option>
                ))}
              </select>
            )}
          </div>
        )}
      </div>
      <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
        <StatusDot status={j.status} />
        <span
          className="mp-mono"
          style={{
            fontSize: 10.5,
            color: 'var(--mp-textMuted)',
            textTransform: 'uppercase',
          }}
        >
          {/* Every `JobState` has a `status.<state>` entry, so the template
              literal resolves to a known key of the namespace. */}
          {t(`status.${j.status}`)}
        </span>
      </div>
      <JobProgressBar job={j} height={5} t={t} />
      <div
        className="mp-mono"
        style={{
          fontSize: 10.5,
          color: 'var(--mp-textMuted)',
          textAlign: 'right',
        }}
      >
        {j.finishedAt
          ? t('job.times', { from: j.startedAt, to: j.finishedAt })
          : j.startedAt}
      </div>
      <div style={{ display: 'flex', gap: 2, justifyContent: 'flex-end' }}>
        {canPause && (
          <MPIconButton
            icon="pause"
            size={24}
            label={t('rowActions.pause', { file: j.file })}
            disabled={busy}
            onClick={onPause}
          />
        )}
        {needsPassword(j.status, j.errorInfo) && (
          <MPIconButton
            icon="lock"
            size={24}
            label={t('password.rowAction', { file: j.file })}
            disabled={busy}
            onClick={onEnterPassword}
          />
        )}
        {missingDependency(j) && (
          <MPIconButton
            icon="download"
            size={24}
            label={t('fix.installLabel', { file: j.file })}
            disabled={busy}
            onClick={onInstallDependency}
          />
        )}
        {j.status === 'paused' && !needsPassword(j.status, j.errorInfo) && (
          <MPIconButton
            icon="play"
            size={24}
            label={t('rowActions.resume', { file: j.file })}
            disabled={busy}
            onClick={onResume}
          />
        )}
        {j.status === 'failed' && (
          <MPIconButton
            icon="rotate"
            size={24}
            label={t('rowActions.retry', { file: j.file })}
            disabled={busy}
            onClick={onRetry}
          />
        )}
        {!terminal && (
          <MPIconButton
            icon="x"
            size={24}
            label={t('rowActions.cancel', { file: j.file })}
            disabled={busy}
            onClick={onCancel}
          />
        )}
      </div>
    </div>
  );
}

export function QueueListView({
  jobs,
  onConfirm,
  onEnterPassword,
  onInstallDependency,
}: QueueListViewProps) {
  const t = useT('queue');
  const pauseJob = useQueueStore((s) => s.pauseJob);
  const resumeJob = useQueueStore((s) => s.resumeJob);
  const retryJob = useQueueStore((s) => s.retryJob);
  const cancelJob = useQueueStore((s) => s.cancelJob);
  const adjustAndResume = useQueueStore((s) => s.adjustAndResume);
  const pendingIds = useQueueStore((s) => s.pendingIds);
  const labels = useJobLabels(jobs);
  const printers = useUsablePrinters(
    jobs.some((j) => pauseFixes(j)?.otherPrinter),
  );

  const groups = {
    active: jobs.filter((j) =>
      ['printing', 'converting', 'converted'].includes(j.status),
    ),
    pending: jobs.filter((j) => ['queued', 'paused'].includes(j.status)),
    done: jobs.filter((j) =>
      ['done', 'unconfirmed', 'failed', 'cancelled'].includes(j.status),
    ),
  };

  // Cancelling a printing job interrupts a job the spooler already started, so
  // it gets a confirm; every other non-terminal state cancels immediately.
  const requestCancel = (j: UiQueueJobView) => {
    if (j.status === 'printing') {
      onConfirm({
        title: t('confirm.cancelTitle'),
        message: t('confirm.cancelMessage', { file: j.file }),
        confirmLabel: t('confirm.cancelConfirm'),
        run: () => cancelJob(j.id),
      });
    } else {
      void cancelJob(j.id);
    }
  };

  return (
    <div className="mp-scroll" style={{ flex: 1, overflow: 'auto' }}>
      {(
        [
          { key: 'active', label: t('groups.active'), items: groups.active },
          { key: 'pending', label: t('groups.pending'), items: groups.pending },
          { key: 'done', label: t('groups.done'), items: groups.done },
        ] as const
      ).map(
        (g) =>
          g.items.length > 0 && (
            <div key={g.key}>
              <div
                style={{
                  padding: '10px 14px',
                  background: 'var(--mp-surface2)',
                  borderBottom: '1px solid var(--mp-border)',
                  borderTop:
                    g.key !== 'active' ? '1px solid var(--mp-border)' : 'none',
                  display: 'flex',
                  justifyContent: 'space-between',
                  alignItems: 'center',
                }}
              >
                <span
                  className="mp-mono"
                  style={{
                    fontSize: 10.5,
                    color: 'var(--mp-textMuted)',
                    textTransform: 'uppercase',
                    letterSpacing: '0.08em',
                    fontWeight: 500,
                  }}
                >
                  {t('groups.header', {
                    label: g.label,
                    count: g.items.length,
                  })}
                </span>
              </div>
              {g.items.map((j) => (
                <Row
                  key={j.id}
                  j={j}
                  t={t}
                  labels={labels}
                  onPause={() => void pauseJob(j.id)}
                  onResume={() => void resumeJob(j.id)}
                  onRetry={() => void retryJob(j.id)}
                  onCancel={() => requestCancel(j)}
                  onEnterPassword={() => onEnterPassword(j)}
                  onInstallDependency={() => onInstallDependency(j)}
                  onAdjust={(change) => void adjustAndResume(j.id, change)}
                  printers={printers}
                  busy={pendingIds.includes(j.id)}
                />
              ))}
            </div>
          ),
      )}
    </div>
  );
}
