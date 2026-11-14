//! Everything the bridge checks about what the extension sends, as pure
//! functions: document names, checksums, base64, and what each kind of
//! document must look like.
//!
//! The extension is trusted as far as the browser vouches for its origin and
//! no further: every name is reduced to a safe base name, every document is
//! hashed and compared, and every document's content is checked against its
//! declared kind before the app ever sees it.

use base64::Engine as _;
use sha2::Digest as _;

use super::protocol::{ErrorCode, FileKind, ProtocolError};

/// Longest stored name (stem plus extension), in characters, before the
/// `NNNN-` order prefix.
pub const MAX_NAME_CHARS: usize = 120;

/// Bytes of the start of a document kept for the content check.
pub const HEAD_BYTES: usize = 1024;

/// Largest XML document checked for well-formedness (20 MiB). Bigger XML is
/// refused: no electronic voucher comes near it, and the parser builds a tree.
pub const MAX_XML_BYTES: usize = 20 * 1024 * 1024;

/// Most XML nodes accepted, a bound on the tree the check builds.
pub const MAX_XML_NODES: u32 = 1_000_000;

/// Deepest XML element nesting accepted. The parser descends recursively, so
/// the nesting is measured by [`nesting_exceeds`] *before* parsing: a
/// document of thousands of nested elements would otherwise overflow the
/// stack. An SRI voucher nests about six levels.
pub const MAX_XML_DEPTH: usize = 128;

/// Names Windows reserves for devices, whatever the extension.
const WINDOWS_RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Reduce a name the extension sent to a safe file name for `kind`.
///
/// - only the last component survives (`/` and `\` both separate), so no
///   name can climb out of its job folder;
/// - control characters and the characters Windows forbids (`<>:"/\|?*`)
///   become `_`; leading and trailing dots and spaces go, so `..` and `.`
///   cannot survive as a name;
/// - a trailing `.pdf`/`.xml`/`.txt` is dropped and the extension of `kind`
///   is put back, so the stored extension always matches what was checked;
/// - Windows device names (`CON`, `NUL`, `COM1`…) get a leading `_`;
/// - the result is at most [`MAX_NAME_CHARS`] characters; an empty one is
///   `document`.
pub fn sanitize_name(raw: &str, kind: FileKind) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut stem = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    // Drop one known extension; another kind's included, so `a.xml` sent as
    // a pdf becomes `a.pdf`, not `a.xml.pdf`.
    if let Some((s, ext)) = stem.rsplit_once('.') {
        if ["pdf", "xml", "txt"]
            .iter()
            .any(|known| ext.eq_ignore_ascii_case(known))
        {
            stem = s;
        }
    }
    let stem = stem.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let ext = kind.extension();
    let budget = MAX_NAME_CHARS - ext.len() - 1;
    let mut stem: String = stem.chars().take(budget).collect();
    // Truncation can leave a trailing dot or space, which Windows strips.
    while stem.ends_with(['.', ' ']) {
        stem.pop();
    }
    if stem.is_empty() {
        stem = "document".to_string();
    }
    let device = stem.split('.').next().unwrap_or("").trim_end();
    if WINDOWS_RESERVED
        .iter()
        .any(|r| r.eq_ignore_ascii_case(device))
    {
        stem.insert(0, '_');
        if stem.chars().count() > budget {
            stem = stem.chars().take(budget).collect();
        }
    }
    format!("{stem}.{ext}")
}

/// The file name a document is stored under: its position in the print order
/// (zero-padded, so a directory listing sorts like the job) and its sanitized
/// name.
pub fn stored_name(index: u32, sanitized: &str) -> String {
    format!("{index:04}-{sanitized}")
}

/// Whether `s` is a SHA-256 in lowercase hex.
pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Lowercase hex of a digest.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    out
}

/// SHA-256 of `bytes`, as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&sha2::Sha256::digest(bytes))
}

/// Decode a chunk: standard base64 with padding, nothing else.
pub fn decode_chunk(data: &str) -> Result<Vec<u8>, ProtocolError> {
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| ProtocolError::bad_request("data is not valid base64"))
}

/// The most bytes `encoded` base64 characters can decode to: lets the session
/// refuse an over-size chunk before decoding it.
pub fn decoded_len_bound(encoded: usize) -> u64 {
    (encoded as u64).div_ceil(4) * 3
}

/// Check a complete document's content against its declared kind.
///
/// `head` is at least the first [`HEAD_BYTES`] bytes (all of them for a
/// shorter document); `whole` is the full document, read back only for the
/// kinds that need it (XML and text). The PDF check is the header only:
/// PluryPrint prints PDFs as they are and has its own checks downstream.
pub fn check_content(
    kind: FileKind,
    head: &[u8],
    whole: Option<&[u8]>,
) -> Result<(), ProtocolError> {
    match kind {
        FileKind::Pdf => check_pdf(head),
        FileKind::Xml => check_xml(whole.unwrap_or(head)),
        FileKind::Txt => check_text(whole.unwrap_or(head)),
    }
}

/// A PDF must start with `%PDF-`.
pub fn check_pdf(head: &[u8]) -> Result<(), ProtocolError> {
    if head.starts_with(b"%PDF-") {
        Ok(())
    } else {
        Err(ProtocolError::new(
            ErrorCode::BadPdf,
            "the document does not start with %PDF-",
        ))
    }
}

/// An XML document must be UTF-8 (a BOM is allowed), at most
/// [`MAX_XML_BYTES`], well formed, without a DTD (so no entity of any kind
/// is declared, let alone expanded), at most [`MAX_XML_NODES`] nodes and
/// [`MAX_XML_DEPTH`] levels deep.
pub fn check_xml(bytes: &[u8]) -> Result<(), ProtocolError> {
    if bytes.len() > MAX_XML_BYTES {
        return Err(ProtocolError::new(
            ErrorCode::TooLarge,
            format!("XML documents are limited to {MAX_XML_BYTES} bytes"),
        ));
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ProtocolError::bad_request("the XML document is not UTF-8"))?;
    if nesting_exceeds(text.as_bytes(), MAX_XML_DEPTH) {
        return Err(ProtocolError::bad_request(format!(
            "the XML document nests deeper than {MAX_XML_DEPTH} levels"
        )));
    }
    let options = roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: MAX_XML_NODES,
    };
    roxmltree::Document::parse_with_options(text, options).map_err(|e| {
        ProtocolError::bad_request(format!(
            "the XML document is not well formed: {}",
            xml_reason(&e)
        ))
    })?;
    Ok(())
}

/// Whether the element nesting of `xml` goes deeper than `max`, measured
/// without parsing (and so without recursion): start tags that are not
/// self-closing open a level, end tags close one; comments, CDATA sections,
/// processing instructions and declarations are skipped whole, and a `>`
/// inside a quoted attribute value does not end a tag. Malformed input is
/// left for the parser to refuse; this only has to be sure the parser never
/// descends further than `max`.
pub fn nesting_exceeds(xml: &[u8], max: usize) -> bool {
    fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
        hay.get(from..)?
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|p| from + p)
    }
    let mut depth = 0usize;
    let mut i = 0usize;
    while let Some(lt) = xml[i..].iter().position(|&b| b == b'<').map(|p| i + p) {
        let rest = &xml[lt..];
        let end = if rest.starts_with(b"<!--") {
            find(xml, lt + 4, b"-->").map(|e| e + 3)
        } else if rest.starts_with(b"<![CDATA[") {
            find(xml, lt + 9, b"]]>").map(|e| e + 3)
        } else if rest.starts_with(b"<?") {
            find(xml, lt + 2, b"?>").map(|e| e + 2)
        } else if rest.starts_with(b"<!") {
            find(xml, lt + 2, b">").map(|e| e + 1)
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            find(xml, lt + 2, b">").map(|e| e + 1)
        } else {
            // A start tag: find its `>` outside quotes.
            let mut quote = None;
            let mut j = lt + 1;
            let mut close = None;
            while j < xml.len() {
                let b = xml[j];
                match quote {
                    Some(q) if b == q => quote = None,
                    Some(_) => {}
                    None if b == b'"' || b == b'\'' => quote = Some(b),
                    None if b == b'>' => {
                        close = Some(j);
                        break;
                    }
                    None => {}
                }
                j += 1;
            }
            if let Some(c) = close {
                if xml[c - 1] != b'/' {
                    depth += 1;
                    if depth > max {
                        return true;
                    }
                }
            }
            close.map(|c| c + 1)
        };
        match end {
            Some(e) => i = e,
            None => return false,
        }
    }
    false
}

/// The parser's reason without the document text it may quote.
fn xml_reason(e: &roxmltree::Error) -> String {
    match e {
        roxmltree::Error::DtdDetected => "a DTD is not allowed".to_string(),
        roxmltree::Error::NodesLimitReached => "too many nodes".to_string(),
        other => other.to_string().chars().take(160).collect(),
    }
}

/// A text report must be UTF-8 or ISO-8859-1 text: no NUL byte and no
/// control character other than tab, line feed, form feed and carriage
/// return (and, for ISO-8859-1, none of the C1 range 0x80–0x9F, which that
/// encoding leaves undefined).
pub fn check_text(bytes: &[u8]) -> Result<(), ProtocolError> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let allowed_control = |c: char| matches!(c, '\t' | '\n' | '\r' | '\x0c');
    let ok = match std::str::from_utf8(bytes) {
        Ok(text) => !text.chars().any(|c| c.is_control() && !allowed_control(c)),
        Err(_) => !bytes.iter().any(|&b| {
            let c = b as char;
            (b < 0x20 && !allowed_control(c)) || b == 0x7f || (0x80..=0x9f).contains(&b)
        }),
    };
    if ok {
        Ok(())
    } else {
        Err(ProtocolError::bad_request(
            "the text document is not UTF-8 or ISO-8859-1 text",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr_121_sanitize_keeps_ordinary_names() {
        assert_eq!(
            sanitize_name("factura_001-001-000000123.pdf", FileKind::Pdf),
            "factura_001-001-000000123.pdf"
        );
        assert_eq!(
            sanitize_name("Reporte de octubre.txt", FileKind::Txt),
            "Reporte de octubre.txt"
        );
        assert_eq!(
            sanitize_name("ñandú café.xml", FileKind::Xml),
            "ñandú café.xml"
        );
        // The extension follows the kind, whatever the name said.
        assert_eq!(sanitize_name("a.xml", FileKind::Pdf), "a.pdf");
        assert_eq!(sanitize_name("a.PDF", FileKind::Pdf), "a.pdf");
        assert_eq!(sanitize_name("a.exe", FileKind::Pdf), "a.exe.pdf");
        assert_eq!(sanitize_name("a", FileKind::Xml), "a.xml");
    }

    /// Table of hostile names: each one ends as a single, harmless base name.
    #[test]
    fn fr_121_sanitize_table_of_hostile_names() {
        let cases: &[(&str, &str)] = &[
            ("../../etc/passwd", "passwd.pdf"),
            ("..\\..\\Windows\\System32\\cmd.exe", "cmd.exe.pdf"),
            ("/abs/path/x.pdf", "x.pdf"),
            ("C:\\Users\\a\\x.pdf", "x.pdf"),
            ("..", "document.pdf"),
            (".", "document.pdf"),
            ("...pdf", "pdf.pdf"),
            ("", "document.pdf"),
            ("   ", "document.pdf"),
            ("a/..", "document.pdf"),
            (".hidden.pdf", "hidden.pdf"),
            ("trailing. . .pdf", "trailing.pdf"),
            ("a\u{0}b.pdf", "a_b.pdf"),
            ("a\nb\rc\td.pdf", "a_b_c_d.pdf"),
            ("a<b>c:d\"e|f?g*h.pdf", "a_b_c_d_e_f_g_h.pdf"),
            ("CON.pdf", "_CON.pdf"),
            ("nul", "_nul.pdf"),
            ("com1.report.pdf", "_com1.report.pdf"),
            ("LPT9 .pdf", "_LPT9.pdf"),
            ("console.pdf", "console.pdf"),
        ];
        for (raw, want) in cases {
            assert_eq!(sanitize_name(raw, FileKind::Pdf), *want, "{raw:?}");
        }
    }

    #[test]
    fn fr_121_sanitize_bounds_the_length_in_characters() {
        let long = format!("{}.pdf", "ñ".repeat(500));
        let out = sanitize_name(&long, FileKind::Pdf);
        assert_eq!(out.chars().count(), MAX_NAME_CHARS);
        assert!(out.ends_with(".pdf"));
        // A truncation that would end on a dot or a space does not.
        let dotty = format!("{}.{}", "a".repeat(MAX_NAME_CHARS - 6), "b".repeat(50));
        let out = sanitize_name(&dotty, FileKind::Txt);
        assert!(!out.trim_end_matches(".txt").ends_with('.'), "{out}");
    }

    /// Property-style check over a generated table of hostile pieces: every
    /// combination stays one component, joined under a job folder it never
    /// leaves, and is never empty, `.` or `..`.
    #[test]
    fn fr_121_no_sanitized_name_escapes_its_folder() {
        let pieces = [
            "", ".", "..", "/", "\\", "a", "ñ", " ", "\u{0}", "\u{1f}", "\u{7f}", "\u{202e}", ":",
            "*", "CON", "x.pdf", ".xml", "C:", "~", "%2e%2e", "\u{2215}", "\u{ff0f}",
        ];
        let job = std::path::Path::new("/inbox/job");
        let mut checked = 0;
        for a in pieces {
            for b in pieces {
                for c in pieces {
                    let raw = format!("{a}{b}{c}");
                    for kind in [FileKind::Pdf, FileKind::Xml, FileKind::Txt] {
                        let name = sanitize_name(&raw, kind);
                        assert!(!name.is_empty() && name != "." && name != "..", "{raw:?}");
                        assert!(!name.contains(['/', '\\']), "{raw:?} -> {name:?}");
                        assert!(!name.chars().any(char::is_control), "{raw:?} -> {name:?}");
                        assert!(name.chars().count() <= MAX_NAME_CHARS);
                        assert!(name.ends_with(kind.extension()));
                        let stored = stored_name(4_999, &name);
                        let path = job.join(&stored);
                        assert_eq!(path.parent(), Some(job), "{raw:?} -> {stored:?}");
                        assert_eq!(
                            path.components().count(),
                            job.components().count() + 1,
                            "{raw:?}"
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert_eq!(checked, pieces.len().pow(3) * 3);
    }

    #[test]
    fn fr_121_stored_names_sort_in_print_order() {
        assert_eq!(stored_name(0, "a.pdf"), "0000-a.pdf");
        assert_eq!(stored_name(42, "a.pdf"), "0042-a.pdf");
        let mut names: Vec<String> = [10, 2, 1000, 0]
            .iter()
            .map(|i| stored_name(*i, "x.pdf"))
            .collect();
        names.sort();
        assert_eq!(
            names,
            ["0000-x.pdf", "0002-x.pdf", "0010-x.pdf", "1000-x.pdf"]
        );
    }

    #[test]
    fn fr_121_sha256_matches_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(is_sha256_hex(&sha256_hex(b"abc")));
        assert!(!is_sha256_hex(&sha256_hex(b"abc").to_uppercase()));
        assert!(!is_sha256_hex("abc"));
        assert!(!is_sha256_hex(&"g".repeat(64)));
    }

    #[test]
    fn fr_121_base64_is_strict() {
        assert_eq!(decode_chunk("JVBERi0=").unwrap(), b"%PDF-");
        assert_eq!(decode_chunk("").unwrap(), b"");
        for bad in ["JVBERi0", "JVB ERi0=", "JVBERi0=\n", "@@@@", "JVBERi0=="] {
            assert_eq!(
                decode_chunk(bad).unwrap_err().code,
                ErrorCode::BadRequest,
                "{bad:?}"
            );
        }
        assert!(decoded_len_bound(8) >= 5);
        assert_eq!(decoded_len_bound(0), 0);
    }

    #[test]
    fn fr_121_pdf_needs_its_header() {
        assert!(check_pdf(b"%PDF-1.7\n...").is_ok());
        for bad in [&b""[..], b"%PD", b" %PDF-1.7", b"PK\x03\x04", b"<html>"] {
            assert_eq!(check_pdf(bad).unwrap_err().code, ErrorCode::BadPdf);
        }
    }

    #[test]
    fn fr_122_xml_must_be_well_formed_without_a_dtd() {
        assert!(check_xml(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?><factura id=\"comprobante\"><a>1</a></factura>").is_ok());
        assert!(check_xml(b"\xEF\xBB\xBF<autorizacion><comprobante><![CDATA[<factura/>]]></comprobante></autorizacion>").is_ok());
        let hostile: &[&[u8]] = &[
            // Billion laughs: refused at the DTD, nothing is expanded.
            b"<?xml version=\"1.0\"?><!DOCTYPE lolz [<!ENTITY lol \"lol\"><!ENTITY lol2 \"&lol;&lol;\">]><lolz>&lol2;</lolz>",
            // External entity (XXE).
            b"<?xml version=\"1.0\"?><!DOCTYPE r [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><r>&x;</r>",
            // External DTD.
            b"<!DOCTYPE r SYSTEM \"http://example.invalid/r.dtd\"><r/>",
            // An undeclared entity.
            b"<r>&nope;</r>",
            // Broken CDATA and unclosed tags.
            b"<r><![CDATA[ unterminated </r>",
            b"<r><a></r>",
            b"",
            b"not xml",
            // Not UTF-8.
            b"<r>\xff\xfe</r>",
        ];
        for doc in hostile {
            let e = check_xml(doc).unwrap_err();
            assert_eq!(
                e.code,
                ErrorCode::BadRequest,
                "{}",
                String::from_utf8_lossy(doc)
            );
            assert!(!e.message.contains("passwd"), "{}", e.message);
        }
    }

    #[test]
    fn fr_122_xml_depth_and_size_are_bounded() {
        let deep = format!("{}{}", "<a>".repeat(10_000), "</a>".repeat(10_000));
        assert_eq!(
            check_xml(deep.as_bytes()).unwrap_err().code,
            ErrorCode::BadRequest
        );
        // Unclosed, too: the parser never gets to recurse into it.
        let unclosed = "<a x='>'>".repeat(100_000);
        assert_eq!(
            check_xml(unclosed.as_bytes()).unwrap_err().code,
            ErrorCode::BadRequest
        );
        let ok = format!(
            "{}{}",
            "<a>".repeat(MAX_XML_DEPTH),
            "</a>".repeat(MAX_XML_DEPTH)
        );
        assert!(check_xml(ok.as_bytes()).is_ok());
        let big = vec![b' '; MAX_XML_BYTES + 1];
        assert_eq!(check_xml(&big).unwrap_err().code, ErrorCode::TooLarge);
    }

    #[test]
    fn fr_122_nesting_is_measured_without_parsing() {
        let n = |s: &str, max| nesting_exceeds(s.as_bytes(), max);
        assert!(!n("<a><b/><c></c></a>", 2));
        assert!(n("<a><b><c/></b></a>", 1));
        // Self-closing tags, comments, CDATA, PIs and quoted `>` do not count.
        assert!(!n("<a><b/><b/><b/></a>", 1));
        assert!(!n("<a><!-- <b><c><d> --></a>", 1));
        assert!(!n("<a><![CDATA[<b><c><d>]]></a>", 1));
        assert!(!n("<?xml version='1.0'?><a><?pi <b><c>?></a>", 1));
        assert!(!n(r#"<a t="x>y" u='<z>'/>"#, 0));
        assert!(n(r#"<a t="x/>"><b>"#, 1), "a quoted /> does not self-close");
        // The SRI shape: the whole voucher inside CDATA is one level.
        assert!(!n("<autorizacion><comprobante><![CDATA[<factura><a><b><c/></b></a></factura>]]></comprobante></autorizacion>", 2));
    }

    #[test]
    fn fr_122_text_is_utf8_or_latin1() {
        assert!(check_text("Reporte\tde octubre\r\nñ €\n\x0c".as_bytes()).is_ok());
        assert!(
            check_text(b"Raz\xf3n social: \xd1and\xfa").is_ok(),
            "ISO-8859-1"
        );
        assert!(check_text(b"\xEF\xBB\xBFwith BOM").is_ok());
        for bad in [
            &b"a\x00b"[..],
            b"\x1b[31mred",
            b"bin\x81ary\xff",
            "a\u{7}b".as_bytes(),
        ] {
            assert_eq!(check_text(bad).unwrap_err().code, ErrorCode::BadRequest);
        }
    }
}
