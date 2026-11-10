import { describe, it, expect, vi, afterEach } from 'vitest';

import {
  CATALOGS,
  NAMESPACES,
  SUPPORTED_LOCALES,
  isLocale,
  matchLocale,
  translate,
  type LocaleCatalog,
  type Namespace,
  type TranslationKey,
} from './index';

/** Flatten a namespace bundle into `a.b.c` → value pairs. */
function flatten(node: unknown, prefix = ''): Record<string, string> {
  if (typeof node !== 'object' || node === null) return {};
  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(node)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === 'string') out[path] = value;
    else Object.assign(out, flatten(value, path));
  }
  return out;
}

describe('translate', () => {
  afterEach(() => vi.restoreAllMocks());

  it('FR-092: resolves nested dot-notation keys', () => {
    expect(translate(CATALOGS['en-US'], 'common', 'actions.cancel')).toBe(
      'Cancel',
    );
    expect(translate(CATALOGS['es-ES'], 'common', 'actions.cancel')).toBe(
      'Cancelar',
    );
  });

  it('FR-092: interpolates {{param}} placeholders and leaves unknown ones', () => {
    const catalog = withCommon({ greet: 'Hi {{name}}, {{missing}}' });
    expect(translate(catalog, 'common', key('greet'), { name: 'Ana' })).toBe(
      'Hi Ana, {{missing}}',
    );
  });

  it('FR-092: selects the _one / _other form from `count`', () => {
    expect(
      translate(CATALOGS['en-US'], 'common', 'print.queued', { count: 1 }),
    ).toBe('1 job queued');
    expect(
      translate(CATALOGS['en-US'], 'common', 'print.queued', { count: 3 }),
    ).toBe('3 jobs queued');
    expect(
      translate(CATALOGS['es-ES'], 'common', 'units.file', { count: 2 }),
    ).toBe('archivos');
  });

  it('FR-091: falls back to English when the key is missing in the locale', () => {
    // A Spanish catalog that lost one key (what a half-translated namespace
    // would look like at runtime).
    const partial = withCommon({}, CATALOGS['es-ES']);
    expect(translate(partial, 'common', 'actions.cancel')).toBe('Cancel');
  });

  it('FR-092: an unknown key returns the key itself and warns once', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const missing = key('nope.not.here');
    expect(() => translate(CATALOGS['en-US'], 'common', missing)).not.toThrow();
    expect(translate(CATALOGS['en-US'], 'common', missing)).toBe(
      'nope.not.here',
    );
    // Second call must not warn again — a re-rendering screen would spam.
    expect(warn).toHaveBeenCalledTimes(1);
  });
});

describe('locale matching', () => {
  it('FR-093: maps an OS tag onto a supported locale, else undefined', () => {
    expect(matchLocale('es-ES')).toBe('es-ES');
    expect(matchLocale('es-MX')).toBe('es-ES');
    expect(matchLocale('en')).toBe('en-US');
    expect(matchLocale('de-DE')).toBeUndefined();
    expect(matchLocale(undefined)).toBeUndefined();
  });

  it('narrows arbitrary config strings', () => {
    expect(isLocale('en-US')).toBe(true);
    expect(isLocale('fr-FR')).toBe(false);
  });
});

describe('catalog parity', () => {
  // The regression guard: every page agent adds keys to `en/<ns>.json` and
  // `es/<ns>.json`, and nothing else checks that they stay in step.
  it.each(SUPPORTED_LOCALES.filter((l) => l !== 'en-US'))(
    'FR-091: %s has exactly the English keys, all non-empty',
    (locale) => {
      for (const ns of NAMESPACES) {
        const en = flatten(CATALOGS['en-US'][ns]);
        const other = flatten(CATALOGS[locale][ns]);
        const missing = Object.keys(en).filter((k) => !(k in other));
        const extra = Object.keys(other).filter((k) => !(k in en));
        expect(
          { missing, extra },
          `locales/${locale.split('-')[0]}/${ns}.json is out of sync with locales/en/${ns}.json`,
        ).toEqual({ missing: [], extra: [] });

        for (const [k, value] of Object.entries(other)) {
          expect(
            value.trim(),
            `locales/${locale.split('-')[0]}/${ns}.json has an empty value for "${k}"`,
          ).not.toBe('');
        }
        for (const [k, value] of Object.entries(en)) {
          expect(
            value.trim(),
            `locales/en/${ns}.json has an empty value for "${k}"`,
          ).not.toBe('');
        }
      }
    },
  );

  it('FR-092: every namespace file exists in every locale', () => {
    const expected: Namespace[] = [
      'common',
      'home',
      'printJob',
      'queue',
      'printers',
      'profiles',
      'settings',
      'about',
    ];
    expect([...NAMESPACES].sort()).toEqual([...expected].sort());
    for (const locale of SUPPORTED_LOCALES) {
      expect(Object.keys(CATALOGS[locale]).sort()).toEqual(
        [...expected].sort(),
      );
    }
  });
});

// --- helpers ---------------------------------------------------------------

/**
 * Keys are type-checked against the English catalog, so the ad-hoc bundles and
 * deliberately-missing keys these tests need have to be cast in.
 */
function key(path: string): TranslationKey<'common'> {
  return path as TranslationKey<'common'>;
}

/** A catalog whose `common` namespace is replaced by `bundle`. */
function withCommon(
  bundle: object,
  base: LocaleCatalog = CATALOGS['en-US'],
): LocaleCatalog {
  return { ...base, common: bundle as LocaleCatalog['common'] };
}

describe('per-platform variants', () => {
  // `lib/platform`'s `platformKeyVariant` looks for `<key>Mac` / `<key>Linux`
  // and falls back to `<key>`: a variant without its plain sentence would be
  // the only copy for that platform and silently missing on Windows.
  it('every …Mac / …Linux sentence has the plain sentence it varies', () => {
    for (const ns of NAMESPACES) {
      const keys = flatten(CATALOGS['en-US'][ns]);
      for (const key of Object.keys(keys)) {
        const base = /^(.+?)(Mac|Linux)$/.exec(key)?.[1];
        if (!base) continue;
        expect(
          base in keys,
          `${ns}:${key} has no plain "${base}" to fall back to`,
        ).toBe(true);
      }
    }
  });

  it('the Linux copy never sends a Linux user to Explorer or Finder', () => {
    for (const locale of SUPPORTED_LOCALES) {
      for (const ns of NAMESPACES) {
        for (const [key, value] of Object.entries(
          flatten(CATALOGS[locale][ns]),
        )) {
          if (!/Linux|linuxMenu/.test(key)) continue;
          expect(value, `${locale} ${ns}:${key}`).not.toMatch(
            /Explor|Finder|winget|Microsoft Store|pluryprint-cli/,
          );
        }
      }
    }
  });
});
