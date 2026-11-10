import { describe, expect, it } from 'vitest';

import { errorTechnical, IpcFailure, localizeIpcError } from './ipcError';
import { rememberHostInfo } from '@/lib/platform';

describe('localizeIpcError', () => {
  it('FR-092: translates a known code + reason with its params', () => {
    const payload = {
      code: 'UpdateCheckFailed',
      reason: 'bad_status',
      params: { status: '502' },
      message: 'github.com returned an unexpected status (502)',
    } as const;
    expect(localizeIpcError(payload, 'es-ES')).toBe(
      'github.com devolvió un estado inesperado (502).',
    );
    expect(localizeIpcError(payload, 'en-US')).toBe(
      'github.com returned an unexpected status (502).',
    );
  });

  it('FR-092: a profile name clash is fully translated', () => {
    expect(
      localizeIpcError(
        {
          code: 'InvalidRequest',
          reason: 'name_taken',
          params: { name: 'Oficina' },
          message: 'a profile named "Oficina" already exists',
        },
        'es-ES',
      ),
    ).toBe('Ya existe un perfil llamado «Oficina».');
  });

  it('FR-092: a known code without a known reason keeps the detail in a translated frame', () => {
    expect(
      localizeIpcError(
        { code: 'PrintFailed', reason: 'brand_new', message: 'lp: boom' },
        'es-ES',
      ),
    ).toBe('Error al imprimir: lp: boom');
    expect(
      localizeIpcError({ code: 'JobNotFound', message: '0192-…' }, 'es-ES'),
    ).toBe('Ese trabajo ya no está en la cola.');
  });

  it('FR-092: falls back to the raw message only for an unknown code', () => {
    expect(
      localizeIpcError(
        { code: 'SomethingNew' as never, message: 'raw detail' },
        'es-ES',
      ),
    ).toBe('raw detail');
    // …and never to an English literal when there is no message at all.
    expect(localizeIpcError({ code: 'SomethingNew' as never }, 'es-ES')).toBe(
      'Error desconocido',
    );
  });

  it('FR-092: an Internal failure has no English detail in its sentence', () => {
    const text = localizeIpcError(
      { code: 'Internal', message: 'print queue is not running' },
      'es-ES',
    );
    expect(text).toBe(
      'Algo ha fallado dentro de Pluryprint. La carpeta de registros tiene los detalles.',
    );
    expect(text).not.toMatch(/queue/);
  });

  it('FR-092: an engine failure that reached a command reuses the queue sentence', () => {
    expect(
      localizeIpcError(
        {
          code: 'ConversionFailed',
          reason: 'pdf_truncated',
          message: 'not a valid PDF: truncated',
        },
        'es-ES',
      ),
    ).toBe('El PDF está incompleto o dañado.');
    // A converter timeout is in the ConversionFailed family too.
    expect(
      localizeIpcError(
        {
          code: 'ConversionFailed',
          reason: 'tool_timed_out',
          params: { tool: 'LibreOffice', secs: '120' },
          message: 'LibreOffice did not finish within 120s and was stopped',
        },
        'es-ES',
      ),
    ).toBe('LibreOffice no terminó en 120 s y se detuvo.');
  });

  it('FR-058/FR-092: a missing dependency says the shared remediation', () => {
    expect(
      localizeIpcError(
        {
          code: 'DependencyMissing',
          reason: 'libre-office',
          message: 'LibreOffice is not installed',
        },
        'es-ES',
      ),
    ).toMatch(/^Falta LibreOffice/);
  });

  it('FR-092: a numeric count param picks the plural sentence', () => {
    const outside = (count: string) =>
      localizeIpcError(
        {
          code: 'InvalidRequest',
          reason: 'page_range_outside',
          params: { name: 'a.pdf', span: '2-3', count },
          message: 'a.pdf: range 2-3 is outside the document',
        },
        'es-ES',
      );
    expect(outside('1')).toBe(
      '«a.pdf»: 2-3 queda fuera del documento, que solo tiene 1 página.',
    );
    expect(outside('8')).toBe(
      '«a.pdf»: 2-3 queda fuera del documento, que solo tiene 8 páginas.',
    );
  });

  it('FR-092: a rejected setting is named by its own label', () => {
    expect(
      localizeIpcError(
        {
          code: 'InvalidRequest',
          reason: 'setting_out_of_range',
          params: { field: 'max_file_size_mb', min: '1', max: '4096' },
          message: 'max_file_size_mb must be between 1 and 4096, got 0',
        },
        'en-US',
      ),
    ).toBe('Max file size must be between 1 and 4096.');
  });

  it("FR-092: a tool's own words are the technical line, not part of the sentence", () => {
    const err = new IpcFailure(
      {
        code: 'PrintFailed',
        reason: 'spooler_rejected',
        params: { detail: 'lp: The printer or class does not exist.' },
        message: 'lp: The printer or class does not exist.',
      },
      'es-ES',
    );
    expect(err.message).toBe(
      'El servicio de impresión del sistema rechazó el trabajo.',
    );
    expect(err.technical).toBe('lp: The printer or class does not exist.');
    expect(errorTechnical(err)).toBe(err.technical);
    expect(errorTechnical(new Error('x'))).toBeUndefined();
  });

  it('IpcFailure keeps the English detail and the stable fields', () => {
    const err = new IpcFailure(
      {
        code: 'DependencyMissing',
        reason: 'ippfind_missing',
        message: '`ippfind` is not installed',
      },
      'es-ES',
    );
    expect(err).toBeInstanceOf(Error);
    expect(err.message).toMatch(/herramientas cliente de CUPS/);
    expect(err.detail).toBe('`ippfind` is not installed');
    expect(err.code).toBe('DependencyMissing');
    expect(err.reason).toBe('ippfind_missing');
  });

  it('on Linux, a reason with a Linux sentence says how to fix it there', () => {
    const payload = {
      code: 'DependencyMissing',
      reason: 'ippfind_missing',
      message: '`ippfind` is not installed',
    } as const;
    // Off Linux (jsdom's webview reads as Windows), the generic sentence.
    expect(localizeIpcError(payload, 'en-US')).not.toMatch(/cups-ipp-utils/);
    rememberHostInfo({ os: 'linux' });
    expect(localizeIpcError(payload, 'en-US')).toMatch(
      /apt install cups-ipp-utils/,
    );
    expect(localizeIpcError(payload, 'es-ES')).toMatch(
      /apt install cups-ipp-utils/,
    );
    // A reason without a Linux variant keeps its one sentence.
    expect(
      localizeIpcError(
        { code: 'PrintFailed', reason: 'set_default_failed', message: 'x' },
        'en-US',
      ),
    ).toBe('The system could not set the default printer.');
  });

  it('on Linux, a missing dependency reaching a command names its Linux remedy', () => {
    rememberHostInfo({ os: 'linux' });
    expect(
      localizeIpcError(
        { code: 'DependencyMissing', reason: 'libre-office', message: 'x' },
        'en-US',
      ),
    ).toMatch(/apt install libreoffice/);
  });
});
