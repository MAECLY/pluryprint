/**
 * Built-in print profiles and the profile → wire-settings mapping.
 *
 * `BUILT_IN_PROFILES` are the three server-defined built-ins, mirrored here so
 * the UI has a static fallback when the backend is unreachable (the standalone
 * Vite preview) and so `findProfileIn` never comes up empty. The real source of
 * truth is `list_profiles` (see `profilesStore`); these ids/names/values match
 * the Rust `builtins()` exactly. The backend names the built-ins in the app's
 * language (FR-093), so a fallback shown to the user goes through
 * {@link builtInProfiles}, which names them from `profiles:builtinNames`.
 *
 * A profile is a starting point; the Print Job overrides panel is the real
 * settings source (FR-040/041). `buildPrintRequest` collapses a profile baseline
 * plus its overrides into the exact [`PrintRequest`] the backend expects (see
 * `src/ipc/bindings.ts`).
 */

import {
  CATALOGS,
  getActiveLocale,
  translate,
  type Locale,
  type TranslationKey,
} from '@/i18n';
import type {
  Duplex,
  JobSettings,
  PaperSize,
  PrintRequest,
} from '@/ipc/bindings';
import {
  resolvePrinterId as matchPrinterId,
  type PrinterChoice,
} from '@/lib/printerIdentity';
import type { UiProfile } from '@/types/domain';

/**
 * Paper value meaning "whatever this printer is set to": the printer's
 * override from the Printers screen, else its own default. No media flag is
 * sent for it.
 */
export const AUTO_PAPER = 'auto';

/** The sizes every printer path has always offered, in display order. */
export const CLASSIC_PAPERS = ['A4', 'Letter', 'Legal', 'A3'] as const;

/**
 * The wire spelling of a display paper size. The classic four go lowercase
 * (the format stored profiles have always used); `auto` and any size a printer
 * reports (`Tabloid`, `4x6`, `A6`…) travel verbatim — the backend matches them
 * against the printer case-insensitively and sends the printer's own spelling.
 */
export function paperToWire(paper: string): PaperSize {
  const classic = CLASSIC_PAPERS.find(
    (c) => c.toLowerCase() === paper.toLowerCase(),
  );
  return classic ? classic.toLowerCase() : paper;
}

/**
 * The paper choices to offer for a printer: "printer default", the classic
 * four, then every other size the printer reports (deduplicated
 * case-insensitively), then `current` if it is none of those — a stored value
 * must stay selectable rather than snap to something else.
 */
export function paperChoices(
  printerPapers: readonly string[],
  current: string,
): string[] {
  const out: string[] = [AUTO_PAPER, ...CLASSIC_PAPERS];
  const seen = new Set(out.map((p) => p.toLowerCase()));
  for (const p of [...printerPapers, current]) {
    const key = p.toLowerCase();
    if (p.trim() && !seen.has(key)) {
      seen.add(key);
      out.push(p);
    }
  }
  return out;
}

/** Sentinel used in the UI for "use the app/OS default printer". */
export const DEFAULT_PRINTER = '<default>';

export const BUILT_IN_PROFILES: UiProfile[] = [
  {
    id: 'builtin-quick',
    name: 'Quick',
    builtin: true,
    default: true,
    printer: DEFAULT_PRINTER,
    color: 'color',
    duplex: 'off',
    copies: 1,
    paper: 'A4',
    orientation: 'auto',
    scaling: 'fit',
    quality: 'normal',
    tray: 'auto',
  },
  {
    id: 'builtin-duplex-bw',
    name: 'Duplex BW',
    builtin: true,
    printer: DEFAULT_PRINTER,
    color: 'monochrome',
    duplex: 'long-edge',
    copies: 1,
    paper: 'A4',
    orientation: 'auto',
    scaling: 'fit',
    quality: 'normal',
    tray: 'auto',
  },
  {
    id: 'builtin-photo-color',
    name: 'Photo Color',
    builtin: true,
    printer: DEFAULT_PRINTER,
    color: 'color',
    duplex: 'off',
    copies: 1,
    paper: 'A4',
    orientation: 'auto',
    scaling: 'none',
    quality: 'high',
    tray: 'auto',
  },
];

const localized = new Map<Locale, UiProfile[]>();

/**
 * The built-ins named in `locale` (default: the language the UI renders in),
 * as the backend's `localize_builtins` names them. Every setting is
 * {@link BUILT_IN_PROFILES}'s; English returns that very list. Memoized per
 * locale, so callers can compare by identity.
 */
export function builtInProfiles(
  locale: Locale = getActiveLocale(),
): UiProfile[] {
  if (locale === 'en-US') return BUILT_IN_PROFILES;
  let list = localized.get(locale);
  if (!list) {
    list = BUILT_IN_PROFILES.map((p) => ({
      ...p,
      name: translate(
        CATALOGS[locale],
        'profiles',
        `builtinNames.${p.id}` as TranslationKey<'profiles'>,
      ),
    }));
    localized.set(locale, list);
  }
  return list;
}

/** Per-invocation tweaks layered on top of the selected profile's baseline. */
export interface PrintJobOverrides {
  printer?: string;
  duplex?: UiProfile['duplex'];
  color?: UiProfile['color'];
  copies?: number;
  paper?: string;
  orientation?: UiProfile['orientation'];
  scaling?: UiProfile['scaling'];
  range?: string;
  collate?: boolean;
}

/**
 * Resolve `id` against a live profile list, with a deterministic fallback
 * chain: exact match → the list's default profile → the first profile → the
 * first built-in, named in the UI language (so this never returns undefined even for an empty list).
 */
export function findProfileIn(
  profiles: UiProfile[],
  id: string | null | undefined,
): UiProfile {
  const match = id != null ? profiles.find((p) => p.id === id) : undefined;
  if (match) return match;
  const def = profiles.find((p) => p.default);
  if (def) return def;
  return profiles[0] ?? builtInProfiles()[0]!;
}

/**
 * The profile an integration *named*, or `undefined` when it names none we
 * know. `ref` is an id or a display name — the CLI documents
 * `--profile <NAME-OR-ID>` and the context-menu payload carries whatever the
 * integration chose — so an exact id wins, then a case-insensitive name.
 *
 * Unlike {@link findProfileIn} there is no fallback: a caller that was asked
 * for a specific profile has to be able to tell the user it was not found,
 * instead of printing with a different one in silence.
 */
export function findProfileByRef(
  profiles: UiProfile[],
  ref: string | null | undefined,
): UiProfile | undefined {
  if (ref == null) return undefined;
  const byId = profiles.find((p) => p.id === ref);
  if (byId) return byId;
  const wanted = ref.trim().toLowerCase();
  if (wanted === '') return undefined;
  return profiles.find((p) => p.name.trim().toLowerCase() === wanted);
}

/** Bounds of the copies field (the server is authoritative too). */
export const MIN_COPIES = 1;
export const MAX_COPIES = 999;

/** Copies must be an integer in `1..=999` (the server is authoritative too). */
export function isValidCopies(raw: string): boolean {
  const s = raw.trim();
  if (!/^\d+$/.test(s)) return false;
  const n = Number(s);
  return n >= MIN_COPIES && n <= MAX_COPIES;
}

/** Pages are 1-based u32s, so cap what the backend can actually hold. */
const MAX_PAGE = 0xffff_ffff;

/** A single 1-based page number: digits only (no interior/edge spaces), 1..=u32. */
function isPageToken(token: string): boolean {
  if (!/^\d+$/.test(token)) return false;
  const n = Number(token);
  return n >= 1 && n <= MAX_PAGE;
}

/**
 * Client-side mirror of the engine's page-range grammar: comma-separated
 * entries, each `N` or `A-B` (1-based, `B >= A`). Empty means "all pages" (the
 * field is optional and the backend never sees a blank range). The server
 * re-validates; this only gates the UI so the button reflects intent.
 *
 * Whitespace is tolerated only where the Rust parser tolerates it — around
 * commas and around the `-` of a span — NOT inside a page number. The engine
 * parses each entry token-by-token (`u32::parse`), so `"1 2"` is two would-be
 * numbers glued together and is rejected there; stripping all whitespace here
 * (as this used to) would accept it as `12` and then fail server-side with a
 * confusing error. See `src-tauri/src/engine/pagerange.rs`.
 */
export function isValidPageRange(raw: string): boolean {
  return parsePageRange(raw) !== null;
}

/** One inclusive, 1-based page span. */
export interface PageSpan {
  start: number;
  end: number;
}

/**
 * Parse a page-range expression into its spans, or `null` when the grammar is
 * violated. An empty expression means "all pages" and parses to `[]`.
 *
 * Same grammar as {@link isValidPageRange} — that predicate is now this
 * function's yes/no shadow, so the two can never disagree.
 */
export function parsePageRange(raw: string): PageSpan[] | null {
  if (raw.trim() === '') return [];
  const spans: PageSpan[] = [];
  for (const rawEntry of raw.split(',')) {
    const entry = rawEntry.trim();
    if (entry === '') return null; // stray empty entry (",," / "1,") — rejected
    const dash = entry.indexOf('-');
    if (dash === -1) {
      if (!isPageToken(entry)) return null;
      spans.push({ start: Number(entry), end: Number(entry) });
    } else {
      // Split on the FIRST dash only (mirrors Rust's `split_once`); trim each
      // side so " 1 - 3 " is fine but "1 2-3" (space inside a number) is not.
      const lhs = entry.slice(0, dash).trim();
      const rhs = entry.slice(dash + 1).trim();
      if (!isPageToken(lhs) || !isPageToken(rhs)) return null;
      if (Number(rhs) < Number(lhs)) return null;
      spans.push({ start: Number(lhs), end: Number(rhs) });
    }
  }
  return spans;
}

/** The little a document needs to expose to be bounds-checked. */
export interface RangeCheckable {
  name: string;
  ext: string;
  pages?: number | null | undefined;
}

/**
 * The first document `raw` overflows, or `null` when the range fits everywhere
 * it applies. Client-side mirror of `engine::pagerange::check_bounds` +
 * `ipc::jobs::check_ranges_fit_documents`, so the inline error (FR-042/FR-043)
 * says the same thing the backend would say on submit.
 *
 * Two narrowings, both matching the backend exactly:
 * - **PDFs only** — the engine drops the range for images (they are one page),
 *   so an image must never invalidate a range it will not receive. Documents
 *   that are converted first (DOCX, HTML, …) do take the range, but their page
 *   count only exists after conversion, so they cannot be bounded here.
 * - **Known page counts only** — `pages` is `null`/absent for PDFs too large or
 *   too broken to probe; an unmeasurable document cannot be bounded.
 *
 * A syntactically invalid range returns `null`: that error is already reported
 * by {@link isValidPageRange}, and reporting two errors for one field would
 * just be noise.
 */
export function findPageRangeOverflow(
  raw: string,
  files: readonly RangeCheckable[],
): { name: string; pages: number } | null {
  const spans = parsePageRange(raw);
  if (spans === null || spans.length === 0) return null;
  const last = Math.max(...spans.map((s) => s.end));
  for (const f of files) {
    if (f.ext.toLowerCase() !== 'pdf') continue;
    const pages = f.pages;
    if (pages == null || pages <= 0) continue;
    if (last > pages) return { name: f.name, pages };
  }
  return null;
}

/** The backend's name limit (`profiles::MAX_NAME_CHARS`), in characters. */
export const MAX_PROFILE_NAME_CHARS = 100;

/**
 * A name's length as the backend counts it: Unicode scalar values (Rust's
 * `chars().count()`), not UTF-16 code units. `'😀'.length` is 2, so counting
 * code units would reject a name of 60 emoji that the backend accepts — and
 * then lock every later edit of that profile behind a bogus error.
 */
export function profileNameLength(name: string): number {
  return Array.from(name).length;
}

/** Cut `s` to at most `max` characters (code points), never inside a pair. */
function truncateChars(s: string, max: number): string {
  const chars = Array.from(s);
  return chars.length <= max ? s : chars.slice(0, max).join('');
}

/** Why a job cannot be printed as it stands, or `null` when it can. */
export type PrintBlocker = 'noFiles' | 'copies' | 'range';

/**
 * The single "can this job print?" rule, shared by the Print button and the
 * Ctrl/⌘+P shortcut so the two can never disagree (FR-042/FR-043).
 *
 * `copiesDraft` is what the copies field *shows* when that differs from the
 * committed override — the overrides only ever receive valid values, so
 * without the draft a field reading `1200` (invalid) would still print the
 * `120` committed one keystroke earlier. The range is checked for grammar and
 * against every PDF's known page count, exactly like the inline error.
 */
export function printBlocker(
  files: readonly RangeCheckable[],
  profile: UiProfile,
  overrides: PrintJobOverrides,
  copiesDraft: string | null,
): PrintBlocker | null {
  if (files.length === 0) return 'noFiles';
  if (copiesDraft !== null && !isValidCopies(copiesDraft)) return 'copies';
  const range = overrides.range ?? profile.pageRange ?? '';
  if (!isValidPageRange(range)) return 'range';
  if (findPageRangeOverflow(range, files) !== null) return 'range';
  return null;
}

/**
 * Validate a profile name for the editor. A name must be non-empty when
 * trimmed, at most 100 characters, and unique (case-insensitive) among
 * `existing` — excluding the profile being edited (`selfId`). Returns a
 * user-facing error string, or `null` when the name is acceptable.
 */
export function validateProfileName(
  name: string,
  existing: UiProfile[],
  selfId?: string,
): string | null {
  const trimmed = name.trim();
  if (trimmed === '') return 'Enter a profile name.';
  if (profileNameLength(trimmed) > MAX_PROFILE_NAME_CHARS)
    return 'Name must be 100 characters or fewer.';
  const lower = trimmed.toLowerCase();
  const clash = existing.some(
    (p) => p.id !== selfId && p.name.trim().toLowerCase() === lower,
  );
  if (clash) return 'A profile with this name already exists.';
  return null;
}

/**
 * The first of `base`, `"base 2"`, `"base 3"`… whose name is not already taken
 * (case-insensitive) by `existing`. Used to seed new/cloned drafts with a name
 * that already passes {@link validateProfileName} — including its length rule:
 * the base is shortened (never the counter) so `"<base> N"` still fits in
 * {@link MAX_PROFILE_NAME_CHARS}. Cloning a 95-character profile would
 * otherwise seed a "Copy of …" draft that cannot be saved as is.
 */
export function nextUniqueName(base: string, existing: UiProfile[]): string {
  const taken = new Set(existing.map((p) => p.name.trim().toLowerCase()));
  const first = truncateChars(base.trim(), MAX_PROFILE_NAME_CHARS).trimEnd();
  if (!taken.has(first.toLowerCase())) return first;
  for (let n = 2; ; n++) {
    const suffix = ` ${n}`;
    const room = MAX_PROFILE_NAME_CHARS - suffix.length;
    const candidate = `${truncateChars(first, room).trimEnd()}${suffix}`;
    if (!taken.has(candidate.toLowerCase())) return candidate;
  }
}

/**
 * The printer *id* (queue name) a profile's stored printer refers to.
 *
 * Profiles must store the id — it is what the spooler needs (`lp -d`,
 * SumatraPDF `-print-to`); on CUPS the display name (`printer-info`) differs
 * from it, and a job sent to the display name targets a queue that does not
 * exist. Older builds of the editor stored the display name, so a value is
 * matched with the backend's rules (`printerIdentity.resolvePrinterId`, the
 * mirror of `ipc::printers::match_installed`): an id, an id ignoring case, or
 * a display name carried by exactly one printer maps to that printer's id.
 * Anything else (the default sentinel, an ambiguous name, an offline printer)
 * is returned unchanged.
 */
export function resolvePrinterId(
  value: string,
  printers: readonly PrinterChoice[],
): string {
  if (!value || value === DEFAULT_PRINTER) return value;
  return matchPrinterId(printers, value) ?? value;
}

/** UI duplex vocabulary → wire `Duplex`. */
const DUPLEX_TO_WIRE: Record<UiProfile['duplex'], Duplex> = {
  off: 'none',
  'long-edge': 'long',
  'short-edge': 'short',
};

/**
 * The UI sentinel for "let the printer pick the tray". Everything else is a
 * driver-supplied bin name and is passed through verbatim, because bin names
 * are model-specific ("Tray 2", "Drawer 1", "Kassette 1") and we are in no
 * position to normalize them.
 */
export const AUTO_TRAY = 'auto';

/**
 * A profile's tray as the wire wants it: the bin name, or `null` for the
 * printer's own default. `auto` and blanks both mean "no choice", and a blank
 * would otherwise reach the driver as an empty `InputSlot=` that selects
 * nothing (the backend rejects it — see `JobSettings::validate`).
 */
export function toWireTray(tray: string | null | undefined): string | null {
  const trimmed = (tray ?? '').trim();
  if (trimmed === '' || trimmed.toLowerCase() === AUTO_TRAY) return null;
  return trimmed;
}

/**
 * Collapse a profile baseline plus its overrides into a wire [`JobSettings`].
 * The mapping is intentionally faithful — no hidden flags — so the backend's
 * `preview_command` and the actual dispatch agree with what the UI shows.
 *
 * The page range comes from `overrides.range` when set, otherwise the
 * profile's saved `pageRange`, otherwise "all pages".
 *
 * `quality` and `tray` come straight from the profile: the overrides panel does
 * not expose them, but the profile editor does, so they have to travel with the
 * request or they would be settings the user can edit and save that never reach
 * a printer. What each backend can do with them (CUPS applies both; SumatraPDF
 * applies the tray and says so when it cannot apply the quality) is the engine's
 * business, and the honesty preview reports it.
 */
export function toJobSettings(
  profile: UiProfile,
  overrides: PrintJobOverrides,
): JobSettings {
  const merged = { ...profile, ...overrides };
  const printer =
    !merged.printer || merged.printer === DEFAULT_PRINTER
      ? null
      : merged.printer;
  const range = (overrides.range ?? profile.pageRange ?? '').trim();
  return {
    printer,
    duplex: DUPLEX_TO_WIRE[merged.duplex],
    color_mode: merged.color,
    copies: merged.copies,
    paper_size: paperToWire(merged.paper),
    orientation: merged.orientation,
    scaling: merged.scaling,
    page_ranges: range ? range : null,
    quality: merged.quality,
    tray: toWireTray(merged.tray),
    collate: merged.collate ?? true,
  };
}

/**
 * Build the full [`PrintRequest`] for `paths` under the given profile/overrides.
 *
 * `hold` submits the batch with every job paused (the context menu's "add to
 * the queue"); it is left off the request entirely otherwise. `origin` tags
 * every job with the integration the batch came from (FR-124), so the Queue
 * says so; left off when there is none.
 */
export function buildPrintRequest(
  paths: string[],
  profile: UiProfile,
  overrides: PrintJobOverrides,
  opts: {
    hold?: boolean;
    origin?: { source: string; label: string | null } | null;
  } = {},
): PrintRequest {
  return {
    files: paths,
    settings: toJobSettings(profile, overrides),
    profile_name: profile.name,
    ...(opts.hold ? { hold: true } : {}),
    ...(opts.origin
      ? { origin: { source: opts.origin.source, label: opts.origin.label } }
      : {}),
  };
}
