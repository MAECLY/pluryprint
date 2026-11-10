/**
 * Which `common:deps.<id>.*` sentence describes a missing dependency on this
 * platform (FR-058, FR-092).
 *
 * A dependency can block different files, and be installed differently, per
 * OS — LibreOffice is RTF's route on Windows and Linux, while macOS prints
 * RTF through `textutil`; on Linux it comes from the distribution's packages
 * rather than an installer — so a dependency may carry a `remediationMac` or
 * `remediationLinux` variant next to its `remediation`. Every surface that
 * names a missing dependency (Print Job's card, the install dialog, a paused
 * job's error) goes through here so they all say the same thing.
 */

import type { UiQueueJobView } from '@/lib/ipc';
import { hostOs, platformKeyVariant, type OsId } from '@/lib/platform';
import {
  CATALOGS,
  getActiveLocale,
  hasTranslation,
  type Locale,
  type TranslationKey,
} from '@/i18n';

/**
 * The `common` key of `id`'s remediation sentence on this platform: the
 * host's ({@link hostOs}) unless `os` says otherwise. `mac` is the older
 * two-way switch, kept for callers that only distinguish macOS.
 */
export function remediationKey(
  id: string,
  opts: { os?: OsId; mac?: boolean; locale?: Locale } = {},
): TranslationKey<'common'> {
  const os =
    opts.os ??
    (opts.mac === undefined ? hostOs() : opts.mac ? 'macos' : 'windows');
  const catalog = CATALOGS[opts.locale ?? getActiveLocale()];
  const base = `deps.${id}.remediation`;
  const variant = platformKeyVariant(base, os);
  if (variant !== base && hasTranslation(catalog, 'common', variant)) {
    return variant as TranslationKey<'common'>;
  }
  return base as TranslationKey<'common'>;
}

/**
 * The dependency a paused job is waiting for (FR-058), or `null`. A job that
 * stopped on `dependency_missing` carries the dependency id as its reason;
 * the Queue offers to install it (the same consent dialog Print Job uses)
 * and then resumes the job.
 */
export function missingDependency(
  j: Pick<UiQueueJobView, 'status' | 'errorInfo'>,
): string | null {
  if (j.status !== 'paused') return null;
  if (j.errorInfo?.code !== 'dependency_missing') return null;
  return j.errorInfo.reason ?? null;
}
