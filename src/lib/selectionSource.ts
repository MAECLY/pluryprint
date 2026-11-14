/**
 * Where a context-menu selection came from, as far as the UI shows it, and
 * the print settings it asked for (FR-123/FR-124).
 *
 * Every integration tags its handoff with a `source` (`windows-explorer`,
 * `macos-services`, …). Only the browser bridge's is surfaced: a batch
 * Pluryfact sent carries a "From Pluryfact" badge (and Pluryfact's own label
 * for it, e.g. "Recibidos · octubre 2026") on the Print Job screen and in the
 * toast that confirms it was queued, and on each of its rows in the Queue,
 * so documents that did not come from a file manager are recognisable as
 * such. Any other source is informational and shows nothing.
 *
 * The Print Job screen can mix files: a Pluryfact batch may land in a job
 * the user had already started, and the user may drop more files into a
 * Pluryfact job. So the origin is tracked with the paths it brought
 * ([`TrackedOrigin`]), and shown only while every file in the job is one of
 * them ([`originForFiles`]).
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

/** An origin and the job files it brought. */
export interface TrackedOrigin extends JobOrigin {
  /** Paths of the files this origin added to the job. */
  paths: ReadonlySet<string>;
}

/**
 * Record that `origin` added `added` to a job whose files are now `current`.
 *
 * An earlier origin whose files are all gone from the job is forgotten. One
 * still in the job, from the same integration, is extended; when the two
 * batches carry different labels neither label is shown, only the
 * integration's name. Nothing added leaves `prev` as it was.
 */
export function trackOrigin(
  prev: TrackedOrigin | null,
  origin: JobOrigin,
  added: readonly string[],
  current: ReadonlySet<string>,
): TrackedOrigin | null {
  if (added.length === 0) return prev;
  const live = prev ? [...prev.paths].filter((p) => current.has(p)) : [];
  if (!prev || live.length === 0 || prev.source !== origin.source) {
    return { ...origin, paths: new Set(added) };
  }
  return {
    source: origin.source,
    label: prev.label === origin.label ? origin.label : null,
    paths: new Set([...live, ...added]),
  };
}

/**
 * The origin to show for a job holding `paths`: the tracked one only when
 * every file came from it, so neither the user's own files nor a later drop
 * are labelled as Pluryfact's. `null` for an empty job.
 */
export function originForFiles(
  tracked: TrackedOrigin | null,
  paths: readonly string[],
): JobOrigin | null {
  if (!tracked || paths.length === 0) return null;
  if (!paths.every((p) => tracked.paths.has(p))) return null;
  return { source: tracked.source, label: tracked.label };
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
