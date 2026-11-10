# Feature Matrix

> **TL;DR** — A feature-by-feature comparison of Pluryprint (MVP and v1.0) against the major commercial and open-source batch-printing tools. The matrix shows where the market leaders dominate (format breadth, polished GUIs), where they all fall short (modern Windows shell integration), and where Pluryprint chooses to focus.

## Legend

| Symbol | Meaning |
| --- | --- |
| ✓ | Fully supported. |
| ✗ | Not supported. |
| Partial | Supported with significant caveats (subset of formats, limited config, etc.). |
| Planned | Not in MVP, scheduled on the roadmap. |
| n/a | Not applicable to the product's category. |

For the source materials behind each cell, see [competitor-analysis.md](./competitor-analysis.md).

## Comparison Table

| Feature | Print Conductor | 2Printer | Batch & Print Pro | FolderMill | clawPDF | Pluryprint MVP | Pluryprint v1.0 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| **Open source** | ✗ | ✗ | ✗ | ✗ | ✓ (AGPL-3.0) | ✓ (AGPL-3.0) | ✓ (AGPL-3.0) |
| **Free of charge** | ✗ ($149) | ✗ ($149) | ✗ ($79.95) | ✗ (B2B) | ✓ | ✓ | ✓ |
| **Multi-format input** | ✓ (90+) | ✓ (90+) | ✓ (30+) | ✓ (90+) | n/a (virtual printer) | Partial (PDF, Office, images, text/MD, HTML) | ✓ (broad, plugin-extensible) |
| **PDF rendering without Adobe** | ✓ | ✓ | ✓ | ✓ | n/a | ✓ (pdfium/mupdf) | ✓ |
| **Microsoft Office (DOCX/XLSX/PPTX)** | ✓ | ✓ | ✓ | ✓ | n/a | Partial (headless LibreOffice if installed; Interop fallback) | ✓ |
| **Image formats (JPG/PNG/TIFF/BMP/GIF/WebP)** | ✓ | ✓ | ✓ | ✓ | n/a | ✓ | ✓ |
| **CAD formats (DWG/DXF/STEP)** | ✓ | ✓ | ✗ | ✓ | n/a | ✗ | Planned (plugin) |
| **Email formats (EML/MSG/PST)** | ✓ | ✓ | ✗ | ✓ | n/a | ✗ | Planned (plugin) |
| **Modern Windows 11 context menu (top-level)** | ✗ | ✗ | ✗ | ✗ | ✗ | ✓ (Sparse MSIX + IExplorerCommand) | ✓ |
| **Legacy Windows context menu** | ✗ | ✗ | ✗ | ✗ | ✗ | ✓ | ✓ |
| **No 15-file selection limit** | n/a (no shell integration) | n/a | n/a | n/a | n/a | ✓ (full IShellItemArray) | ✓ |
| **GUI** | ✓ | ✗ | ✓ | ✓ | ✓ (config) | ✓ (Tauri 2 + React) | ✓ |
| **CLI** | Partial (file list import) | ✓ (full) | ✓ | ✗ | ✓ (limited) | Partial (basic flags) | ✓ (2Printer-style flag taxonomy) |
| **Hot folder / watch mode** | ✗ | ✗ | ✓ | ✓ (core feature) | ✗ | ✗ | Planned |
| **Email monitor (POP3/IMAP)** | ✗ | ✗ | ✓ | ✗ | ✗ | ✗ | ✗ (out of scope) |
| **Cloud monitor (Dropbox/Drive)** | ✗ | ✗ | ✓ | ✗ | ✗ | ✗ | ✗ (out of scope) |
| **Windows Service mode** | ✗ | ✗ | ✓ | ✓ | ✗ | ✗ | Planned (post-v1.0) |
| **Drag-and-drop file list** | ✓ | n/a (CLI) | ✓ | n/a (hot folder) | n/a | ✓ | ✓ |
| **Import file list from TXT/CSV** | ✓ | ✗ | ✗ | ✗ | ✗ | Planned | ✓ |
| **Saved profiles** | ✓ | ✗ (per-invocation) | ✓ | ✓ | ✓ | Planned | ✓ |
| **Per-printer defaults** | ✓ | ✗ | ✓ | ✓ | ✓ | ✓ | ✓ |
| **File-type → printer routing** | ✗ | ✗ | Partial | ✓ | ✗ | Planned | ✓ |
| **Duplex (long/short edge)** | ✓ | ✓ | ✓ | ✓ | n/a | ✓ | ✓ |
| **Color / grayscale toggle** | ✓ | ✓ | ✓ | ✓ | n/a | ✓ | ✓ |
| **Paper size selection** | ✓ | ✓ | ✓ | ✓ | n/a | ✓ | ✓ |
| **Orientation** | ✓ | ✓ | ✓ | ✓ | n/a | ✓ | ✓ |
| **Copies** | ✓ | ✓ | ✓ | ✓ | n/a | ✓ | ✓ |
| **Page range** | ✓ | ✓ | ✓ | ✓ | n/a | ✓ | ✓ |
| **Collate (multiple modes)** | ✓ (3 modes) | ✓ | ✓ | ✓ | n/a | Partial (single mode) | ✓ |
| **N-up (multiple pages per sheet)** | ✓ | ✓ | ✗ | ✓ | n/a | ✗ | Planned |
| **Input tray selection** | ✓ | ✓ | ✗ | ✓ | n/a | Planned | ✓ |
| **Watermark (text)** | ✓ | ✓ | ✗ | ✓ | n/a | ✗ | Planned |
| **Watermark (image)** | ✓ | ✓ | ✗ | ✓ | n/a | ✗ | Planned |
| **Watermark (barcode)** | ✓ | ✓ | ✗ | ✗ | n/a | ✗ | ✗ (out of scope) |
| **Separator pages** | ✓ | ✓ | ✓ | ✗ | n/a | Planned | ✓ |
| **Blank page removal** | ✗ | ✗ | ✓ | ✗ | n/a | ✗ | Planned |
| **Overlay PDF (letterhead)** | ✗ | ✗ | ✓ | ✗ | n/a | ✗ | Planned |
| **Page text matching** | ✗ | ✗ | ✓ | ✗ | n/a | ✗ | ✗ (plugin candidate) |
| **Multi-printer load balancing** | ✓ (PC11+) | ✗ | ✓ | ✗ | n/a | ✗ | Planned |
| **Single-job print mode (ordering guarantee)** | ✓ | ✓ | ✓ | ✓ | n/a | ✓ | ✓ |
| **Job log / report** | ✓ | ✓ (XML) | ✓ | ✓ | ✗ | ✓ (text log) | ✓ (text + JSON) |
| **Scheduler** | ✗ | ✗ | ✓ | ✓ | ✗ | ✗ | Planned |
| **Auto-update** | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ (Tauri updater, signed) | ✓ |
| **Telemetry by default** | Unclear | Unclear | Unclear | Unclear | ✗ | ✗ | ✗ |
| **Privacy-first (no cloud, no account)** | n/a | n/a | n/a | n/a | ✓ | ✓ | ✓ |
| **Windows x64 support** | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| **Windows ARM64 support** | ✗ | ✗ | ✗ | ✗ | ✓ | ✓ | ✓ |
| **macOS support** | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | Planned (macOS 13+) |
| **Linux support** | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | Preview from 0.0.5: x86_64 and ARM64, AppImage and `.deb` ([ADR-0015](../adr/0015-linux-support.md)) |
| **Plugin / extension API** | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | Planned (Rust trait + WIT) |

## Reading the Matrix

A few things to notice when scanning the table:

### Where commercial leaders dominate

- **Format breadth.** Print Conductor and FolderMill (sharing fCoder's renderer) cover 90+ formats. We will not match this at MVP; we should not pretend we will.
- **Polish in advanced print options.** N-up, multi-mode collate, watermarks, and separator pages are all features the commercial tools have iterated on for over a decade.
- **Automation surface.** Batch & Print Pro's email + cloud + service combination is uniquely powerful.

### Where every competitor falls short

- **Modern Windows 11 context menu.** Not a single competitor — commercial or OSS — ships an `IExplorerCommand`-based top-level entry. They all live as separate windows users have to launch and then drag files into.
- **No 15-file workaround.** None of them ships a fix for the cap, because none of them is integrated into the shell to begin with.
- **Open source + multi-format + active maintenance.** This intersection is empty in the OSS column.

### Where Pluryprint MVP commits

- Free, AGPL-3.0, OSS.
- Modern + legacy context menu integration.
- The file formats users batch-print 95% of the time (PDF, Office, images, text/MD, HTML).
- Core print options (duplex, color, paper, orientation, copies, page range, basic collate).
- Single-job print mode (ordering correctness).
- Basic CLI.
- Auto-update via signed Tauri updater.
- ARM64 from day one.

### Where Pluryprint v1.0 closes the gap

- Profiles, file-type routing, separator pages, watermarks, n-up, file-list import, full CLI.
- Multi-printer load balancing.
- Hot folders.
- Plugin API for the long tail of formats (CAD, email, etc.).

### Where Pluryprint deliberately says no

- Email monitor (POP3/IMAP/OAuth complexity for a niche use case).
- Cloud folder monitor (Dropbox/Drive — out of scope, privacy-first).
- Barcode watermarks (use a dedicated label tool).
- Page text matching as a *core* feature (plugin only).

## Format Coverage Detail

Because the comparison table compresses "multi-format input" into a single cell, here is the explicit breakdown for MVP and v1.0:

| Format family | MVP | v1.0 | Notes |
| --- | --- | --- | --- |
| PDF (PDF 1.4–2.0, PDF/A) | ✓ | ✓ | `pdfium` (preferred) or `mupdf` (license fallback). Never Adobe Reader. |
| Microsoft Office (DOCX, XLSX, PPTX) | Partial | ✓ | Prefer headless LibreOffice if installed. Optional Office Interop fallback when Office is detected. |
| Legacy Office (DOC, XLS, PPT) | Partial | ✓ | Same path as modern Office. |
| Plain text (TXT) | ✓ | ✓ | Native renderer. |
| Markdown (MD) | ✓ | ✓ | Rendered to HTML, then printed via WebView2. |
| HTML / XHTML | ✓ | ✓ | Via WebView2 (Windows), WKWebView (macOS) or WebKitGTK (Linux). |
| RTF | ✗ | ✓ | Via headless LibreOffice. |
| CSV | Partial | ✓ | As tabular text or via spreadsheet engine. |
| JPEG, PNG, TIFF, BMP, GIF, WebP | ✓ | ✓ | `image-rs`. |
| SVG | ✗ | ✓ | `resvg`. |
| HEIC/HEIF | ✗ | ✓ | The OS decoder (`sips` on macOS, WIC on Windows); on Linux libheif's tools or ImageMagick, installed by the user. |
| EPUB | ✗ | Planned | v1+ plugin. |
| DjVu | ✗ | Planned | v1+ plugin. |
| AutoCAD DWG/DXF | ✗ | Planned | Plugin via `oda-libraries` or `dxf-rs`. |
| 3D (STEP/STL) | ✗ | ✗ | Out of scope — niche use case. |
| Email (EML/MSG/PST) | ✗ | Planned | Plugin. |
| Archives (ZIP/7z/RAR) | ✗ | Planned | Auto-extract and recurse. |
| Zebra ZPL | ✗ | Planned | Plugin (label printers). |

## Pricing Comparison (At a Glance)

| Tool | 1 user | 5 users | 20 users | 50 users |
| --- | --- | --- | --- | --- |
| Print Conductor | $149 | $495 ($99/u) | $1,580 ($79/u) | $2,950 ($59/u) |
| 2Printer | $149 | per quote | per quote | per quote |
| Batch & Print Pro Standard | $79.95 | volume | volume | volume |
| Batch & Print Pro Premium | $109.95 | volume | volume | volume |
| FolderMill | per quote | per quote | per quote | per quote |
| **Pluryprint** | **$0** | **$0** | **$0** | **$0** |

## Related Documents

- [Competitor Analysis](./competitor-analysis.md)
- [Market Gap](./market-gap.md)
- [Windows Context Menu Limitations](./windows-context-menu-limitations.md)
- [Product Vision](../01-vision/product-vision.md)
