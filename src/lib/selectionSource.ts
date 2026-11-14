/**
 * Where a context-menu selection came from, as far as the UI shows it, and
 * the print settings it asked for (FR-123/FR-124).
 *
 * Every integration tags its handoff with a `source` (`windows-explorer`,
 * `macos-services`, …). Only the browser bridge's is surfaced: a batch
 * PluryFact sent carries a "From PluryFact" badge (and PluryFact's own label
 * for it, e.g. "Recibidos · octubre 2026") on the Print Job screen and in the
 * toast that confirms it was queued, so documents that did not come from a
 * file manager are recognisable as such. Any other source is informational
 * and shows nothing.
 */

import type { Selection, SelectionOverrides } from '@/lib/ipc';
import {
  AUTO_PAPER,
  CLASSIC_PAPERS,
  type PrintJobOverrides,
} from '@/lib/profiles';

/** The `source` the browser bridge writes into its handoff. */
export const PLURYFACT_SOURCE = 'pluryfact';

/** A batch the UI labels with its origin. */
export interface JobOrigin {
  /** Which integration: only `pluryfact` is labelled today. */
  source: typeof PLURYFACT_SOURCE;
  /** The integration's own label for the batch, when it sent one. */
  label: string | null;
}

/** The origin to show for `selection`, or `null` when there is none to show. */
export function jobOriginOf(
  selection: Pick<Selection, 'source' | 'label'>,
): JobOrigin | null {
  if (selection.source !== PLURYFACT_SOURCE) return null;
  const label = selection.label?.trim() ?? '';
  return { source: PLURYFACT_SOURCE, label: label === '' ? null : label };
}

/**
 * The display spelling of a wire paper size: the classic four as the UI
 * spells them (`a4` → `A4`), `auto` as the "printer default" choice, and any
 * size a printer reports as it is.
 */
export function paperFromWire(paper: string): string {
  if (paper.toLowerCase() === AUTO_PAPER) return AUTO_PAPER;
  return (
    CLASSIC_PAPERS.find((c) => c.toLowerCase() === paper.toLowerCase()) ?? paper
  );
}

/**
 * The Print Job overrides a selection's settings stand for. A field the
 * integration left out stays out, so the profile's value applies.
 */
export function overridesFromSelection(
  o: SelectionOverrides | null | undefined,
): PrintJobOverrides {
  const out: PrintJobOverrides = {};
  if (!o) return out;
  if (o.printer != null && o.printer.trim() !== '') out.printer = o.printer;
  if (o.copies != null) out.copies = o.copies;
  if (o.duplex != null) out.duplex = o.duplex;
  if (o.color != null) out.color = o.color;
  if (o.paper != null) out.paper = paperFromWire(o.paper);
  if (o.orientation != null) out.orientation = o.orientation;
  if (o.collate != null) out.collate = o.collate;
  return out;
}
