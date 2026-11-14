/**
 * Extensions the print engine accepts, in the engine's own order.
 *
 * This mirrors `SUPPORTED_EXTS` in `src-tauri/src/engine/mod.rs`, which is the
 * only authority — `HomeScreen.test.tsx` reads that Rust file and fails if the
 * two ever drift, so neither the drop zone copy nor an OS file-picker filter
 * can offer a format the engine rejects (or hide one it prints).
 *
 * Grouped by converter (see `docs/03-requirements/supported-formats.md`):
 * PDF passthrough; raster images, HEIC/HEIF and SVG/SVGZ; Office, OpenDocument
 * and RTF through LibreOffice; TXT, HTML and Markdown through the system web
 * view; XML (an SRI electronic receipt as a RIDE-like page, any other XML as a
 * readable tree) through the same web view. GIF is refused on purpose (decision D9). A format whose converter is
 * missing is still accepted — its job pauses with an install hint (FR-058).
 */
export const PRINTABLE_EXTENSIONS = [
  'pdf',
  'jpg',
  'jpeg',
  'png',
  'tif',
  'tiff',
  'bmp',
  'webp',
  'heic',
  'heif',
  'svg',
  'svgz',
  'docx',
  'doc',
  'xlsx',
  'xls',
  'pptx',
  'ppt',
  'odt',
  'ods',
  'odp',
  'rtf',
  'txt',
  'html',
  'htm',
  'md',
  'markdown',
  'xml',
] as const;

export type PrintableExtension = (typeof PRINTABLE_EXTENSIONS)[number];
