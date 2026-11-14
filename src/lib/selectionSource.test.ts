import { describe, expect, it } from 'vitest';
import {
  jobOriginOf,
  overridesFromSelection,
  paperFromWire,
  PLURYFACT_SOURCE,
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
