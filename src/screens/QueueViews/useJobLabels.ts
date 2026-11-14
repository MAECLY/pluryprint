import { useCallback, useEffect, useMemo } from 'react';

import { useLocale, useT } from '@/i18n/I18nProvider';
import type { UiQueueJobView } from '@/lib/ipc';
import { localizeJobError } from '@/lib/jobError';
import {
  requestPrinterNames,
  usePrinterNamesStore,
} from '@/stores/printerNamesStore';

/**
 * How a queued job's printer and error read on screen.
 *
 * - `printer(j)` — the printer's display name (as Print Job and Printers show
 *   it), not the OS queue id the job stores; the id when the name is unknown;
 *   "Default printer" when the job has none yet.
 * - `error(j)` — the job's error in the current UI language (FR-092),
 *   re-translated on a language switch and naming printers by display name.
 * - `origin(j)` — "From Pluryfact" and Pluryfact's label for the batch, for
 *   a job Pluryfact sent (FR-124); `undefined` for any other.
 */
export function useJobLabels(jobs: readonly UiQueueJobView[]) {
  const t = useT('queue');
  const tCommon = useT('common');
  const locale = useLocale();
  const names = usePrinterNamesStore((s) => s.names);

  const ids = useMemo(
    () => [...new Set(jobs.flatMap((j) => (j.printer ? [j.printer] : [])))],
    [jobs],
  );
  useEffect(() => {
    if (ids.length > 0) requestPrinterNames(ids);
  }, [ids]);

  const printerName = useCallback((id: string) => names[id], [names]);

  const printer = useCallback(
    (j: UiQueueJobView) =>
      j.printer ? (names[j.printer] ?? j.printer) : t('job.defaultPrinter'),
    [names, t],
  );

  const error = useCallback(
    (j: UiQueueJobView): string | undefined =>
      j.errorInfo
        ? localizeJobError(j.errorInfo, { locale, printerName })
        : j.error,
    [locale, printerName],
  );

  const origin = useCallback(
    (j: UiQueueJobView): string | undefined =>
      j.origin
        ? [tCommon('selection.fromPluryfact'), j.origin.label]
            .filter(Boolean)
            .join(' · ')
        : undefined,
    [tCommon],
  );

  return { printer, error, origin };
}
