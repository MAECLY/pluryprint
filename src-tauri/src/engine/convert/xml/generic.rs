//! Any XML file as a readable, indented tree (FR-057a).
//!
//! One line per element, attribute list included, its text on the same
//! line when it is short and alone; comments and processing instructions in
//! grey; long text wrapped under its element. The page is a flat list of
//! lines (each with its depth as an inline custom property), not nested
//! boxes, so a deep file costs the web view nothing extra, and a guide line
//! per level shows the structure on paper.
//!
//! The words Pluryprint adds — the header with the file name and the root
//! element — follow the app language. Everything from the file is escaped.

use roxmltree::{Document, Node, NodeType};

use super::{escape as e, Lang};

/// Text longer than this goes on its own lines under the element.
const INLINE_TEXT: usize = 72;

/// Print stylesheet of the tree page. No `@page` size: the job sets it.
pub const TREE_CSS: &str = r#"
* { box-sizing: border-box; }
html { font-family: "Helvetica Neue", Helvetica, Arial, "Liberation Sans", sans-serif;
  font-size: 8.5pt; line-height: 1.4; color: #000; background: #fff;
  -webkit-print-color-adjust: exact; print-color-adjust: exact; }
body { margin: 0; }
header { border-bottom: 0.75pt solid #000; padding-bottom: 2mm; margin-bottom: 2.5mm; }
header h1 { font-size: 12pt; font-weight: 700; margin: 0; overflow-wrap: anywhere; }
header p { margin: 0.6mm 0 0; color: #4a4a4a; }
.tree { font-family: "SF Mono", Menlo, Consolas, "DejaVu Sans Mono", "Liberation Mono", monospace; font-size: 7.8pt; }
.l { display: flex; break-inside: avoid; }
.l > s { flex: none; display: flex; text-decoration: none; }
.l > s > i { width: 4.2mm; border-left: 0.5pt solid #bdbdbd; }
.l > s > u { width: calc(var(--d) * 4.2mm); }
.l > span.b { flex: 1 1 auto; min-width: 0; white-space: pre-wrap; overflow-wrap: anywhere; }
.t { color: #000; font-weight: 700; }
.a { color: #4a4a4a; }
.v { color: #000; }
.x { color: #000; }
.c { color: #6a6a6a; font-style: italic; }
.p { color: #6a6a6a; }
.txt > span.b { padding-left: 2.4mm; }
"#;

fn qualified(node: Node<'_, '_>) -> String {
    let name = node.tag_name();
    match name.namespace().and_then(|ns| node.lookup_prefix(ns)) {
        Some(prefix) if !prefix.is_empty() => format!("{prefix}:{}", name.name()),
        _ => name.name().to_string(),
    }
}

fn open_tag(node: Node<'_, '_>, close: bool) -> String {
    let mut out = format!("<span class=\"t\">&lt;{}</span>", e(&qualified(node)));
    for a in node.attributes() {
        let name = match a.namespace().and_then(|ns| node.lookup_prefix(ns)) {
            Some(prefix) if !prefix.is_empty() => format!("{prefix}:{}", a.name()),
            _ => a.name().to_string(),
        };
        out.push_str(&format!(
            " <span class=\"a\">{}=</span><span class=\"v\">\"{}\"</span>",
            e(&name),
            e(a.value())
        ));
    }
    out.push_str(&format!(
        "<span class=\"t\">{}</span>",
        if close { "/&gt;" } else { "&gt;" }
    ));
    out
}

fn close_tag(node: Node<'_, '_>) -> String {
    format!("<span class=\"t\">&lt;/{}&gt;</span>", e(&qualified(node)))
}

/// Indentation guides drawn per level, up to this depth; deeper levels are
/// indented without them (a guide per level would cost the page more than
/// it shows).
const MAX_GUIDES: usize = 24;

fn line(out: &mut String, depth: usize, class: &str, html: &str) {
    let guides = depth.min(MAX_GUIDES);
    let mut indent = "<i></i>".repeat(guides);
    if depth > guides {
        indent.push_str("<u></u>");
    }
    out.push_str(&format!(
        "<div class=\"l{}\" style=\"--d:{}\"><s>{indent}</s><span class=\"b\">{html}</span></div>\n",
        if class.is_empty() {
            String::new()
        } else {
            format!(" {class}")
        },
        depth - guides
    ));
}

/// Whether `node`'s only meaningful child is one short text.
fn short_text(node: Node<'_, '_>) -> Option<String> {
    let mut kids = node
        .children()
        .filter(|c| !(c.is_text() && c.text().is_some_and(|t| t.trim().is_empty())));
    let only = kids.next()?;
    if kids.next().is_some() || !only.is_text() {
        return None;
    }
    let t = only.text()?.trim();
    (t.chars().count() <= INLINE_TEXT && !t.contains('\n')).then(|| t.to_string())
}

/// The page body for `doc`: one line per node, iteratively.
fn tree(doc: &Document<'_>) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    let mut skip_inside: Option<roxmltree::NodeId> = None;
    for edge in super::traverse(doc.root()) {
        match edge {
            super::Edge::Open(n) => {
                if skip_inside.is_some() && n.parent().map(|p| p.id()) == skip_inside {
                    continue;
                }
                match n.node_type() {
                    NodeType::Root => {}
                    NodeType::Element => {
                        let has_children = n.has_children();
                        if !has_children {
                            line(&mut out, depth, "", &open_tag(n, true));
                        } else if let Some(t) = short_text(n) {
                            line(
                                &mut out,
                                depth,
                                "",
                                &format!(
                                    "{}<span class=\"x\">{}</span>{}",
                                    open_tag(n, false),
                                    e(&t),
                                    close_tag(n)
                                ),
                            );
                            skip_inside = Some(n.id());
                        } else {
                            line(&mut out, depth, "", &open_tag(n, false));
                        }
                        depth += 1;
                    }
                    NodeType::Text => {
                        let t = n.text().unwrap_or("").trim();
                        if !t.is_empty() {
                            line(&mut out, depth, "x txt", &e(t));
                        }
                    }
                    NodeType::Comment => {
                        let t = n.text().unwrap_or("").trim();
                        line(&mut out, depth, "c", &format!("&lt;!-- {} --&gt;", e(t)));
                    }
                    NodeType::PI => {
                        if let Some(pi) = n.pi() {
                            let value = pi.value.unwrap_or("");
                            line(
                                &mut out,
                                depth,
                                "p",
                                &format!("&lt;?{} {}?&gt;", e(pi.target), e(value)),
                            );
                        }
                    }
                }
            }
            super::Edge::Close(n) => {
                if n.is_element() {
                    depth = depth.saturating_sub(1);
                    if skip_inside == Some(n.id()) {
                        skip_inside = None;
                    } else if n.has_children() {
                        line(&mut out, depth, "", &close_tag(n));
                    }
                }
            }
        }
    }
    out
}

/// The whole tree page for `doc`, titled `title`. Pure.
pub fn to_html(doc: &Document<'_>, title: &str, lang: Lang) -> String {
    let root = doc.root_element();
    let elements = doc.descendants().filter(Node::is_element).count();
    let subtitle = match lang {
        Lang::Es => format!(
            "Documento XML, elemento raíz <{}>, {} {}.",
            qualified(root),
            elements,
            if elements == 1 {
                "elemento"
            } else {
                "elementos"
            }
        ),
        Lang::En => format!(
            "XML document, root element <{}>, {} {}.",
            qualified(root),
            elements,
            if elements == 1 { "element" } else { "elements" }
        ),
    };
    let html_lang = match lang {
        Lang::Es => "es",
        Lang::En => "en",
    };
    format!(
        "<!DOCTYPE html>\n<html lang=\"{html_lang}\"><head><meta charset=\"utf-8\"><title>{t}</title>\n\
         <style>{TREE_CSS}</style>\n</head><body>\n<header><h1>{t}</h1><p>{s}</p></header>\n\
         <div class=\"tree\">\n{body}</div>\n</body></html>\n",
        t = e(title),
        s = e(&subtitle),
        body = tree(doc)
    )
}
