/**
 * Backend error localization (FR-092).
 *
 * Every IPC failure carries a stable `code` (its family) and, for the failures
 * the backend knows how to explain, a stable `reason` plus `params`. This module
 * maps them to the active locale:
 *
 * 1. `code` + `reason` with a catalog entry → the translated sentence, filled
 *    with `params`. The sentence lives in `common:ipcErrors.reasons.<code>.<reason>`,
 *    or — for an engine failure that reached a command or the CLI instead of a
 *    queued job — where the queue keeps it: `common:deps.<id>.remediation` for
 *    a missing dependency, `queue:jobErrors.reasons.<jobCode>.<reason>` for the
 *    rest. One sentence per failure, wherever it surfaces.
 * 2. A known `code` without a translated reason → the translated family
 *    sentence, with the backend's English detail as `{{detail}}`
 *    (`ipcErrors.codes.<code>`). `Internal`'s sentence is the exception and
 *    never quotes the detail; it stays on {@link IpcFailure.detail}.
 * 3. An unknown `code` → the raw message, or "Unknown error" when there is none.
 *
 * The English `message` is never lost: it stays on {@link IpcFailure.detail}
 * for logs and diagnostics, and a tool's own words (`params.detail`) are offered
 * as {@link IpcFailure.technical}, the secondary line under the sentence.
 */

import {
  CATALOGS,
  getActiveLocale,
  hasTranslation,
  translate,
  type Locale,
  type LocaleCatalog,
  type Namespace,
  type TranslateParams,
  type TranslationKey,
} from '@/i18n';
import type { IpcError } from '@/ipc/bindings';
import { hostOs, platformKeyVariant, type OsId } from '@/lib/platform';

/** The wire shape, loosened: an old or foreign payload may lack any field. */
export type IpcErrorPayload = Partial<
  Pick<IpcError, 'code' | 'message' | 'reason' | 'params'>
>;

/** A thrown IPC failure whose `message` is already localized. */
export class IpcFailure extends Error {
  readonly code: string | undefined;
  readonly reason: string | undefined;
  readonly params: Readonly<Record<string, string>>;
  /** The backend's English detail (logs, diagnostics). */
  readonly detail: string;
  /**
   * The technical detail worth showing under the translated sentence (a
   * tool's or the OS's own words, e.g. `lp`'s stderr), when the backend sent
   * one and the sentence does not already quote it.
   */
  readonly technical: string | undefined;

  constructor(payload: IpcErrorPayload, locale?: Locale) {
    const resolved = resolveIpcError(payload, locale);
    super(resolved.text);
    this.name = 'IpcFailure';
    this.code = payload.code ?? undefined;
    this.reason = payload.reason ?? undefined;
    this.params = stringParams(payload.params);
    this.detail = payload.message ?? '';
    this.technical = resolved.technical;
  }
}

/** The string-valued params of a payload (anything else is dropped). */
function stringParams(
  params: IpcErrorPayload['params'] | null | undefined,
): Record<string, string> {
  const out: Record<string, string> = {};
  if (!params) return out;
  for (const [k, v] of Object.entries(params)) {
    if (typeof v === 'string') out[k] = v;
  }
  return out;
}

/**
 * Interpolation values for a backend sentence. The wire carries every param as
 * a string, but a plural sentence (`_one` / `_other`) is only selected by a
 * numeric `count`, so a numeric-looking `count` becomes a number.
 */
export function sentenceParams(
  params: IpcErrorPayload['params'] | null | undefined,
): Record<string, string | number> {
  const out: Record<string, string | number> = stringParams(params);
  const count = out.count;
  if (typeof count === 'string' && /^\d+$/.test(count.trim())) {
    out.count = Number(count);
  }
  return out;
}

/** The job error code(s) whose sentences an engine IPC family reuses. */
const ENGINE_JOB_CODES: Readonly<Record<string, readonly string[]>> = {
  ConversionFailed: ['conversion_failed', 'conversion_timeout'],
  PrintFailed: ['print_failed'],
  UnsupportedFormat: ['unsupported_format'],
  PdfProtected: ['pdf_protected'],
};

/** Settings keys the backend names in `params.field`, by their UI label key. */
const SETTING_LABELS: Readonly<Record<string, string>> = {
  max_concurrent_conversions: 'engine.concurrency.label',
  conversion_timeout_seconds: 'engine.timeout.label',
  print_timeout_seconds: 'engine.printTimeout.label',
  max_file_size_mb: 'engine.maxFileSize.label',
};

/** A `{{name}}` placeholder a sentence still contains (an unfilled param). */
const PLACEHOLDER = /\{\{\s*\w+\s*\}\}/;

/**
 * Where the sentence of `code` + `reason` lives, most specific first. A
 * `common` sentence may have a per-platform variant (`…Mac`, `…Linux`, see
 * `platformKeyVariant`), tried before it: `ippfind` missing means "install
 * cups-ipp-utils" on Linux and cannot happen on macOS, for instance.
 */
function reasonKeys(
  code: string,
  reason: string,
  os: OsId = hostOs(),
): [Namespace, string][] {
  const keys: [Namespace, string][] = [];
  const withVariant = (key: string) => {
    const variant = platformKeyVariant(key, os);
    if (variant !== key) keys.push(['common', variant]);
    keys.push(['common', key]);
  };
  withVariant(`ipcErrors.reasons.${code}.${reason}`);
  if (code === 'DependencyMissing') {
    withVariant(`deps.${reason}.remediation`);
  }
  for (const jobCode of ENGINE_JOB_CODES[code] ?? []) {
    keys.push(['queue', `jobErrors.reasons.${jobCode}.${reason}`]);
  }
  return keys;
}

/** Whether `key` (or its plural pair) has a sentence in `ns`. */
function hasSentence(catalog: LocaleCatalog, ns: Namespace, key: string) {
  return (
    hasTranslation(catalog, ns, key) ||
    hasTranslation(catalog, ns, `${key}_other`)
  );
}

function resolveIpcError(
  payload: IpcErrorPayload,
  locale: Locale = getActiveLocale(),
): { text: string; technical: string | undefined } {
  const catalog = CATALOGS[locale];
  const code = payload.code;
  const detail = payload.message ?? '';
  const unknown = () => translate(catalog, 'common', 'errors.unknown');
  if (!code) return { text: detail || unknown(), technical: undefined };

  const params = sentenceParams(payload.params);
  const field = typeof params.field === 'string' ? params.field : undefined;
  if (field && SETTING_LABELS[field]) {
    params.field = translate(
      catalog,
      'settings',
      SETTING_LABELS[field] as TranslationKey<'settings'>,
    );
  }

  const reason = payload.reason;
  if (reason) {
    for (const [ns, key] of reasonKeys(code, reason)) {
      if (!hasSentence(catalog, ns, key)) continue;
      const text = translate(
        catalog,
        ns,
        key as TranslationKey<typeof ns>,
        params as TranslateParams,
      );
      // A sentence with a hole in it is worse than the next candidate.
      if (PLACEHOLDER.test(text)) continue;
      const tech =
        typeof params.detail === 'string' ? params.detail.trim() : '';
      return {
        text,
        technical: tech && !text.includes(tech) ? tech : undefined,
      };
    }
  }

  // `Internal`'s family sentence does not quote the detail: a panic or a dead
  // queue is not something to read in English mid-sentence.
  const familyKey = `ipcErrors.codes.${code}`;
  if (hasTranslation(catalog, 'common', familyKey)) {
    return {
      text: translate(
        catalog,
        'common',
        familyKey as TranslationKey<'common'>,
        { detail },
      ),
      technical: undefined,
    };
  }
  return { text: detail || unknown(), technical: undefined };
}

/** Translate one IPC error payload into the active (or given) locale. */
export function localizeIpcError(
  payload: IpcErrorPayload,
  locale: Locale = getActiveLocale(),
): string {
  return resolveIpcError(payload, locale).text;
}

/** A user-facing message for anything a promise rejected with. */
export function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/**
 * The secondary line to show under {@link errorMessage}: the technical detail
 * of an {@link IpcFailure} (a tool's own words), if it carries one.
 */
export function errorTechnical(e: unknown): string | undefined {
  return e instanceof IpcFailure ? e.technical : undefined;
}
