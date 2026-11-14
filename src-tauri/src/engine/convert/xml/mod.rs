//! XML → HTML → the HTML route (FR-057a, FR-057b; ADR-0017).
//!
//! Two pages come out of an XML file:
//!
//! - **An SRI electronic receipt** (Ecuador's *comprobantes electrónicos*:
//!   factura, nota de crédito, nota de débito, comprobante de retención,
//!   guía de remisión, liquidación de compra) prints as a **RIDE-like
//!   representation** ([`ride`]): issuer, number, authorisation, access key
//!   with its Code 128 barcode ([`code128`]), buyer, lines, taxes, payments
//!   and additional information, laid out the way Ecuadorian RIDEs are.
//!   [`sri`] recognises the file — the `<autorizacion>` the SRI web service
//!   and portal hand out (the receipt inside it as CDATA or escaped text),
//!   the whole `RespuestaAutorizacionComprobante` (SOAP envelope or not), or a
//!   bare receipt — and reads it into a model.
//! - **Any other XML** prints as a readable, indented tree ([`generic`]).
//!
//! The page then goes through the page renderer of [`super::html`] like a
//! text or Markdown file: a policy that forbids scripts and connections, the
//! system web view with scripts off and no network.
//!
//! ## An XML file is untrusted input
//!
//! - **No DTD.** A `<!DOCTYPE …>` anywhere in the prolog is refused
//!   (`xml_dtd_refused`) before the parser sees the file, and the parser runs
//!   with `allow_dtd` off as well: no internal entities (billion laughs), no
//!   external DTD or entity, nothing is ever fetched or read from disk.
//! - **Bounded.** At most [`MAX_XML_BYTES`] on disk (`xml_too_large`), at
//!   most [`MAX_XML_NODES`] nodes (same reason) and [`MAX_XML_DEPTH`] levels
//!   of nesting (`xml_too_deep`). The receipt inside an `<autorizacion>` is
//!   a second document parsed with the same rules; when it does not parse,
//!   the outer file prints as a tree instead.
//! - **Everything is escaped** into the page ([`escape`]); a `<script>` in a
//!   product description prints as text. The page references nothing: the
//!   barcode is inline SVG.
//!
//! A file that is not well-formed is `xml_invalid`. All four reasons are
//! terminal: the same file fails the same way again.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use super::{PageSetup, WorkDir};
use crate::engine::{EngineError, Failure};

pub mod code128;
pub mod generic;
pub mod ride;
pub mod sri;

/// Extensions this converter handles.
pub const XML_EXTS: &[&str] = &["xml"];

/// Largest XML file printed (20 MiB). An SRI receipt is a few dozen KB.
pub const MAX_XML_BYTES: u64 = 20 * 1024 * 1024;

/// Most nodes (elements, text, comments…) one document may hold.
pub const MAX_XML_NODES: u32 = 200_000;

/// Deepest element nesting accepted. An SRI receipt inside a SOAP response
/// is about ten levels deep.
pub const MAX_XML_DEPTH: usize = 256;

/// The paper an XML page is laid out on when the job leaves it to the
/// renderer (paper `auto`): A4, the RIDE's own size.
pub const DEFAULT_CSS_SIZE: &str = "210mm 297mm";

/// Language of the words Pluryprint adds to the page (the generic view's
/// header, the RIDE's footer). The RIDE's own labels are Spanish whatever
/// the app language: they are the document's, as the SRI defines them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    /// Spanish.
    Es,
    /// English.
    En,
}

impl Lang {
    /// The language of an app locale (`es-ES` → Spanish, anything else →
    /// English, the app's default). Pure.
    pub fn from_locale(locale: &str) -> Self {
        let lang = locale.split(['-', '_']).next().unwrap_or("");
        if lang.eq_ignore_ascii_case("es") {
            Lang::Es
        } else {
            Lang::En
        }
    }
}

/// Why an XML file cannot be printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XmlError {
    /// Bigger than [`MAX_XML_BYTES`] or with more than [`MAX_XML_NODES`].
    TooLarge,
    /// The file declares a DTD.
    Dtd,
    /// Nesting deeper than [`MAX_XML_DEPTH`].
    TooDeep,
    /// Not well-formed (or not text); the parser's message.
    Invalid(String),
}

impl XmlError {
    /// The engine failure for this error, with its stable reason.
    pub fn into_engine(self) -> EngineError {
        let failure = match self {
            XmlError::TooLarge => Failure::new(
                "xml_too_large",
                format!(
                    "the XML file is larger than {} MB or holds more than {MAX_XML_NODES} nodes",
                    MAX_XML_BYTES / (1024 * 1024)
                ),
            )
            .param("limit", (MAX_XML_BYTES / (1024 * 1024)).to_string()),
            XmlError::Dtd => Failure::new(
                "xml_dtd_refused",
                "the XML file declares a DTD (<!DOCTYPE>), which is not processed",
            ),
            XmlError::TooDeep => Failure::new(
                "xml_too_deep",
                format!("the XML file nests elements more than {MAX_XML_DEPTH} levels deep"),
            )
            .param("limit", MAX_XML_DEPTH.to_string()),
            XmlError::Invalid(why) => {
                Failure::new("xml_invalid", format!("not a well-formed XML file: {why}"))
            }
        };
        EngineError::ConversionFailed(failure)
    }
}

/// Escape text for HTML content and attribute values. Pure.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + s.len() / 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            // C0/C1 controls other than tab and line breaks cannot reach
            // here from a well-formed XML 1.0 file, but a decoded C1 can.
            c if c.is_control() && !matches!(c, '\t' | '\n' | '\r') => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
    out
}

/// The text of an XML file: a byte-order mark (UTF-8, UTF-16 LE/BE) first;
/// else, when the declaration says ISO-8859-1, Latin-1 or Windows-1252,
/// Windows-1252 — even if the bytes would also read as UTF-8, because the
/// declaration governs (XML 1.0, appendix F); else UTF-8. Anything else is
/// not text this converter reads. Pure.
pub fn decode(bytes: &[u8]) -> Result<String, XmlError> {
    if let Some((encoding, bom)) = encoding_rs::Encoding::for_bom(bytes) {
        let (text, had_errors) = encoding.decode_without_bom_handling(&bytes[bom..]);
        if had_errors {
            return Err(XmlError::Invalid(
                "the text is not valid in its encoding".into(),
            ));
        }
        return Ok(text.into_owned());
    }
    if matches!(
        declared_encoding(bytes).as_str(),
        "iso-8859-1" | "latin1" | "latin-1" | "iso8859-1" | "windows-1252" | "cp1252"
    ) {
        return Ok(encoding_rs::WINDOWS_1252
            .decode_without_bom_handling(bytes)
            .0
            .into_owned());
    }
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| XmlError::Invalid("the file is not UTF-8 text".into()))
}

/// The encoding the XML declaration at the very start of `bytes` names,
/// lower-cased; empty when there is no declaration or it names none. The
/// declaration is ASCII in every encoding this converter reads. Pure.
fn declared_encoding(bytes: &[u8]) -> String {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(200)]).to_ascii_lowercase();
    head.strip_prefix("<?xml")
        .and_then(|rest| rest.split("?>").next())
        .and_then(|decl| decl.split("encoding").nth(1))
        .map(|v| {
            v.trim_start_matches([' ', '='])
                .trim_start_matches(['"', '\''])
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect::<String>()
        })
        .unwrap_or_default()
}

/// Whether the prolog of `text` declares a DTD. A `<!DOCTYPE` may only stand
/// before the root element, after the declaration, processing instructions,
/// comments and whitespace, so the prolog is walked exactly. Pure.
pub fn declares_dtd(text: &str) -> bool {
    let mut rest = text.trim_start_matches('\u{FEFF}');
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("<?") {
            match after.find("?>") {
                Some(end) => rest = &after[end + 2..],
                None => return false,
            }
        } else if let Some(after) = rest.strip_prefix("<!--") {
            match after.find("-->") {
                Some(end) => rest = &after[end + 3..],
                None => return false,
            }
        } else {
            return rest.starts_with("<!DOCTYPE") || rest.starts_with("<!doctype");
        }
    }
}

/// Whether the elements of `text` nest deeper than `limit`, measured by a
/// flat scan of its tags (comments, CDATA sections, processing instructions
/// and quoted attribute values skipped), stopping as soon as it is. Exact
/// for a well-formed file; on a malformed one it never counts lower than a
/// parser could descend before failing. Pure.
pub fn nesting_exceeds(text: &str, limit: usize) -> bool {
    let b = text.as_bytes();
    let mut depth = 0usize;
    let mut i = 0usize;
    let skip_past = |from: usize, end: &[u8]| -> usize {
        b[from..]
            .windows(end.len())
            .position(|w| w == end)
            .map_or(b.len(), |p| from + p + end.len())
    };
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &b[i..];
        if rest.starts_with(b"<!--") {
            i = skip_past(i + 4, b"-->");
        } else if rest.starts_with(b"<![CDATA[") {
            i = skip_past(i + 9, b"]]>");
        } else if rest.starts_with(b"<?") {
            i = skip_past(i + 2, b"?>");
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = skip_past(i + 2, b">");
        } else if rest.starts_with(b"<!") {
            i = skip_past(i + 2, b">");
        } else {
            // A start tag: find its `>` outside quoted attribute values.
            let mut j = i + 1;
            let mut quote: Option<u8> = None;
            while j < b.len() {
                match (quote, b[j]) {
                    (Some(q), c) if c == q => quote = None,
                    (Some(_), _) => {}
                    (None, b'"' | b'\'') => quote = Some(b[j]),
                    (None, b'>') => break,
                    _ => {}
                }
                j += 1;
            }
            let self_closing = j > 0 && j < b.len() && b[j - 1] == b'/';
            if !self_closing {
                depth += 1;
                if depth > limit {
                    return true;
                }
            }
            i = j + 1;
        }
    }
    false
}

/// Parse `text` under this module's rules (no DTD, node and depth limits).
pub fn parse(text: &str) -> Result<roxmltree::Document<'_>, XmlError> {
    if declares_dtd(text) {
        return Err(XmlError::Dtd);
    }
    // roxmltree parses nested elements recursively: a file nested thousands
    // of levels deep would exhaust the stack before any check after parsing
    // could run, so the nesting is measured first, without recursion.
    if nesting_exceeds(text, MAX_XML_DEPTH) {
        return Err(XmlError::TooDeep);
    }
    let options = roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: MAX_XML_NODES,
    };
    let doc = roxmltree::Document::parse_with_options(text, options).map_err(|e| match e {
        roxmltree::Error::DtdDetected => XmlError::Dtd,
        roxmltree::Error::NodesLimitReached
        | roxmltree::Error::AttributesLimitReached
        | roxmltree::Error::NamespacesLimitReached => XmlError::TooLarge,
        other => XmlError::Invalid(other.to_string()),
    })?;
    if depth_of(&doc) > MAX_XML_DEPTH {
        return Err(XmlError::TooDeep);
    }
    Ok(doc)
}

/// One step of a depth-first walk ([`traverse`]).
#[derive(Debug, Clone, Copy)]
pub enum Edge<'a, 'i> {
    /// Entering a node.
    Open(roxmltree::Node<'a, 'i>),
    /// Leaving it, after its children.
    Close(roxmltree::Node<'a, 'i>),
}

/// Walk the tree under `root` (included) depth-first, without recursion,
/// so a deep document cannot exhaust the stack.
pub fn traverse<'a, 'i>(root: roxmltree::Node<'a, 'i>) -> impl Iterator<Item = Edge<'a, 'i>> {
    let mut next = Some(Edge::Open(root));
    std::iter::from_fn(move || {
        let current = next?;
        next = match current {
            Edge::Open(n) => Some(match n.first_child() {
                Some(c) => Edge::Open(c),
                None => Edge::Close(n),
            }),
            Edge::Close(n) if n == root => None,
            Edge::Close(n) => match n.next_sibling() {
                Some(s) => Some(Edge::Open(s)),
                None => n.parent().map(Edge::Close),
            },
        };
        Some(current)
    })
}

/// Deepest element nesting in `doc` (the root element is 1). Iterative.
pub fn depth_of(doc: &roxmltree::Document<'_>) -> usize {
    let mut depth = 0usize;
    let mut deepest = 0usize;
    for edge in traverse(doc.root()) {
        match edge {
            Edge::Open(n) if n.is_element() => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            Edge::Close(n) if n.is_element() => depth -= 1,
            _ => {}
        }
    }
    deepest
}

/// The HTML page for the XML file `bytes`, titled `title`: a RIDE-like page
/// for an SRI receipt, else the readable tree. Pure.
pub fn to_html(bytes: &[u8], title: &str, lang: Lang) -> Result<String, XmlError> {
    if bytes.len() as u64 > MAX_XML_BYTES {
        return Err(XmlError::TooLarge);
    }
    let text = decode(bytes)?;
    let doc = parse(&text)?;
    if let Some(receipt) = sri::read(&doc) {
        return Ok(ride::to_html(&receipt, title, lang));
    }
    Ok(generic::to_html(&doc, title, lang))
}

/// The paper an XML page uses: the job's, else A4 ([`DEFAULT_CSS_SIZE`]).
pub fn page_for(page: &PageSetup) -> PageSetup {
    PageSetup {
        css_size: Some(
            page.css_size
                .clone()
                .unwrap_or_else(|| DEFAULT_CSS_SIZE.to_string()),
        ),
    }
}

/// Convert the XML file at `input` into a PDF at `output`. The route's entry
/// point.
pub fn convert(
    input: &Path,
    output: &Path,
    work: &WorkDir,
    cancel: &AtomicBool,
    timeout: Option<Duration>,
    page: &PageSetup,
) -> Result<(), EngineError> {
    let cannot_read = |e: std::io::Error| {
        EngineError::ConversionFailed(
            Failure::new(
                "cannot_read_input",
                format!("cannot read {}: {e}", input.display()),
            )
            .param("file", input.display().to_string()),
        )
    };
    let size = std::fs::metadata(input).map_err(cannot_read)?.len();
    if size > MAX_XML_BYTES {
        return Err(XmlError::TooLarge.into_engine());
    }
    let bytes = std::fs::read(input).map_err(cannot_read)?;
    let title = input
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document.xml".to_string());
    let lang = Lang::from_locale(&crate::settings::Config::load_or_default().app.locale);
    let html = to_html(&bytes, &title, lang).map_err(XmlError::into_engine)?;
    let base_dir = input.parent().unwrap_or_else(|| Path::new("."));
    super::html::render_document(
        html.as_bytes(),
        base_dir,
        output,
        work,
        cancel,
        timeout,
        &page_for(page),
    )
}

#[cfg(test)]
mod tests;
