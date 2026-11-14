interface FileGlyphProps {
  ext: string;
  size?: number;
}

const COLORS: Record<string, [string, string]> = {
  pdf: ['#FBE2E8', '#9F1239'],
  docx: ['#D9EEF1', '#0E7490'],
  doc: ['#D9EEF1', '#0E7490'],
  xlsx: ['#E5EFD2', '#3F6212'],
  xls: ['#E5EFD2', '#3F6212'],
  pptx: ['#FAEFD0', '#A16207'],
  ppt: ['#FAEFD0', '#A16207'],
  jpg: ['#FDEBD3', '#C2410C'],
  jpeg: ['#FDEBD3', '#C2410C'],
  png: ['#FDEBD3', '#C2410C'],
  heic: ['#FDEBD3', '#C2410C'],
  tiff: ['#FDEBD3', '#C2410C'],
  txt: ['#F0EDE5', '#6B665D'],
  md: ['#E8E2F4', '#5B21B6'],
  markdown: ['#E8E2F4', '#5B21B6'],
  html: ['#FAEFD0', '#A16207'],
  xml: ['#E5EFD2', '#3F6212'],
  odt: ['#D9EEF1', '#0E7490'],
};

export function FileGlyph({ ext, size = 30 }: FileGlyphProps) {
  const [bg, fg] = COLORS[ext.toLowerCase()] ?? ['#F0EDE5', '#6B665D'];
  return (
    <div
      style={{
        width: size,
        height: size * 1.25,
        position: 'relative',
        flexShrink: 0,
        background: bg,
        borderRadius: 3,
        boxShadow: 'inset 0 0 0 1px rgba(0,0,0,0.06)',
        display: 'flex',
        alignItems: 'flex-end',
        justifyContent: 'center',
        paddingBottom: 3,
      }}
    >
      <div
        style={{
          position: 'absolute',
          top: 0,
          right: 0,
          width: size * 0.32,
          height: size * 0.32,
          background: 'var(--mp-bg)',
          clipPath: 'polygon(0 0, 100% 100%, 100% 0)',
        }}
      />
      <span
        className="mp-mono"
        style={{
          fontSize: Math.max(7.5, size * 0.26),
          fontWeight: 600,
          color: fg,
          textTransform: 'uppercase',
          letterSpacing: '0.04em',
        }}
      >
        {ext}
      </span>
    </div>
  );
}
