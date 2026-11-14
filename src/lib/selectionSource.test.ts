import { describe, expect, it } from 'vitest';
import {
  jobOriginOf,
  originForFiles,
  overridesFromSelection,
  paperFromWire,
  PLURYFACT_SOURCE,
  trackOrigin,
} from './selectionSource';

describe('selectionSource', () => {
  it('FR-124: only a PluryFact selection carries an origin to show', () => {
    expect(
      jobOriginOf({ source: 'pluryfact', label: 'Recibidos · octubre 2026' }),
    ).toEqual({ source: PLURYFACT_SOURCE, label: 'Recibidos · octubre 2026' });
    expect(jobOriginOf({ source: 'pluryfact', label: '   ' })).toEqual({
      source: 'pluryfact',
      label: null,
    });
    expect(jobOriginOf({ source: 'pluryfact' })).toEqual({
      source: 'pluryfact',
      label: null,
    });
    for (const source of ['windows-explorer', 'macos-services', '', null]) {
      expect(jobOriginOf({ source, label: 'x' })).toBeNull();
    }
    expect(jobOriginOf({})).toBeNull();
  });

  it('FR-124: the badge shows only while every file in the job came from PluryFact', () => {
    const oct = {
      source: PLURYFACT_SOURCE,
      label: 'Recibidos · octubre 2026',
    } as const;
    const tracked = trackOrigin(null, oct, ['/i/a.pdf', '/i/b.xml'], new Set());
    expect(originForFiles(tracked, ['/i/a.pdf', '/i/b.xml'])).toEqual(oct);
    // A file the user dropped in afterwards: the job is mixed, no badge.
    expect(originForFiles(tracked, ['/i/a.pdf', '/u/mine.pdf'])).toBeNull();
    // Once the user's file is removed again, the badge is back.
    expect(originForFiles(tracked, ['/i/b.xml'])).toEqual(oct);
    // An empty job has no origin; nothing tracked shows nothing.
    expect(originForFiles(tracked, [])).toBeNull();
    expect(originForFiles(null, ['/i/a.pdf'])).toBeNull();
    // A batch that added nothing changes nothing.
    expect(trackOrigin(tracked, oct, [], new Set(['/i/a.pdf']))).toBe(tracked);
  });

  it('FR-124: a second PluryFact batch extends the origin; different labels show only the name', () => {
    const oct = {
      source: PLURYFACT_SOURCE,
      label: 'Recibidos · octubre 2026',
    } as const;
    const nov = {
      source: PLURYFACT_SOURCE,
      label: 'Emitidos · noviembre 2026',
    } as const;
    const first = trackOrigin(null, oct, ['/i/a.pdf'], new Set());
    const both = trackOrigin(first, nov, ['/i/b.pdf'], new Set(['/i/a.pdf']));
    expect(originForFiles(both, ['/i/a.pdf', '/i/b.pdf'])).toEqual({
      source: PLURYFACT_SOURCE,
      label: null,
    });
    const same = trackOrigin(first, oct, ['/i/c.pdf'], new Set(['/i/a.pdf']));
    expect(originForFiles(same, ['/i/a.pdf', '/i/c.pdf'])).toEqual(oct);
    // The first batch's files are gone (printed): the new one starts afresh.
    const fresh = trackOrigin(first, nov, ['/i/b.pdf'], new Set(['/u/x.pdf']));
    expect(originForFiles(fresh, ['/i/b.pdf'])).toEqual(nov);
  });

  it('FR-123: overrides map onto the Print Job vocabulary, absent fields stay absent', () => {
    expect(overridesFromSelection(null)).toEqual({});
    expect(overridesFromSelection({})).toEqual({});
    expect(
      overridesFromSelection({
        printer: 'HP_LaserJet',
        copies: 2,
        duplex: 'long-edge',
        color: 'monochrome',
        paper: 'a4',
        orientation: 'portrait',
        collate: false,
      }),
    ).toEqual({
      printer: 'HP_LaserJet',
      copies: 2,
      duplex: 'long-edge',
      color: 'monochrome',
      paper: 'A4',
      orientation: 'portrait',
      collate: false,
    });
    expect(
      overridesFromSelection({ printer: ' ', copies: null, paper: 'Tabloid' }),
    ).toEqual({ paper: 'Tabloid' });
  });

  it('FR-123: wire paper names read back as the UI spells them', () => {
    expect(paperFromWire('a4')).toBe('A4');
    expect(paperFromWire('LETTER')).toBe('Letter');
    expect(paperFromWire('auto')).toBe('auto');
    expect(paperFromWire('AUTO')).toBe('auto');
    expect(paperFromWire('4x6')).toBe('4x6');
  });
});
