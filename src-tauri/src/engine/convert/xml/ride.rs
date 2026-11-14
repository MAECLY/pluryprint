//! The RIDE-like page of an SRI receipt (FR-057b).
//!
//! Laid out the way Ecuadorian RIDEs (*Representación Impresa del Documento
//! Electrónico*) are, so an accountant reads it without looking for
//! anything: the issuer on the left and the document box on the right (RUC,
//! type, number, authorisation, environment, emission, access key with its
//! barcode), the buyer underneath, the detail table, then additional
//! information and payments on the left and the totals on the right. The
//! retention, the delivery guide and the debit note have their own middle
//! sections.
//!
//! The page is sober on purpose: black on white for any printer, one sans
//! family whose metrics match on every OS (Helvetica / Arial), figures in
//! tabular numerals, hairline tables, rounded boxes as RIDEs draw them. The
//! one detail that is Pluryprint's own is the access key printed in its
//! nine segments (date, type, RUC, environment, series, sequence, code,
//! emission, check digit), which is how people actually read it.
//!
//! Labels are Spanish: they are the document's, as the SRI names them. Only
//! the footer, which is Pluryprint speaking, follows the app language.
//!
//! Every value is escaped ([`super::escape`]); nothing is loaded.

use super::code128;
use super::sri::{self, Kind, Receipt, SupportDoc, Withholding};
use super::{escape as e, Lang};

/// Print stylesheet of the RIDE page. No `@page` size: the job sets the
/// paper (A4 by default, [`super::DEFAULT_CSS_SIZE`]).
pub const RIDE_CSS: &str = r#"
* { box-sizing: border-box; }
html { font-family: "Helvetica Neue", Helvetica, Arial, "Liberation Sans", sans-serif;
  font-size: 8.3pt; line-height: 1.32; color: #000; background: #fff;
  -webkit-print-color-adjust: exact; print-color-adjust: exact; }
body { margin: 0; }
.mono { font-family: "SF Mono", Menlo, Consolas, "DejaVu Sans Mono", "Liberation Mono", monospace; }
.n { text-align: right; white-space: nowrap; font-variant-numeric: tabular-nums; }
.muted { color: #4a4a4a; }
.nowrap, table.lines td.nowrap { white-space: nowrap; overflow-wrap: normal; }
.box { border: 0.75pt solid #000; border-radius: 2.2mm; padding: 2.6mm 3mm; break-inside: avoid; }
.head { display: flex; gap: 3mm; align-items: stretch; }
.issuer { flex: 1 1 50%; display: flex; flex-direction: column; gap: 2.4mm; }
.issuer .name { font-size: 13pt; font-weight: 700; line-height: 1.15; letter-spacing: -0.01em; overflow-wrap: anywhere; }
.issuer .trade { font-size: 9.5pt; margin-top: 0.8mm; overflow-wrap: anywhere; }
.issuer .facts { margin-top: auto; }
.doc { flex: 1 1 50%; }
.doc .ruc { font-size: 10.5pt; }
.doc .ruc b { font-weight: 700; margin-right: 1.5mm; }
.doc .type { font-size: 14.5pt; font-weight: 700; line-height: 1.1; margin: 1.4mm 0 0.6mm; }
.doc .no { font-size: 10.5pt; margin-bottom: 2mm; }
dl.facts { display: grid; grid-template-columns: max-content 1fr; column-gap: 2.6mm; row-gap: 0.5mm; margin: 0; }
dl.facts dt { color: #4a4a4a; }
dl.facts dd { margin: 0; overflow-wrap: anywhere; }
dl.facts.two { grid-template-columns: max-content 1fr max-content 1fr; }
.auth { margin-bottom: 1.2mm; }
.auth .label, .key .label { color: #4a4a4a; margin-bottom: 0.4mm; }
.auth .authno { font-size: 7.4pt; letter-spacing: 0.01em; white-space: nowrap; }
.key { margin-top: 2.2mm; }
.key svg.barcode { display: block; width: 100%; margin-top: 0.6mm; }
.key .segs { display: flex; justify-content: space-between; margin-top: 0.9mm; padding-bottom: 2.6mm; }
.key .seg { position: relative; }
.key .seg b { font-weight: 400; font-size: 6.9pt; letter-spacing: 0.01em; }
.key .seg i { position: absolute; left: 50%; top: 100%; transform: translateX(-50%); white-space: nowrap;
  font-style: normal; font-size: 5pt; color: #4a4a4a; margin-top: 0.1mm; }
.key .seg:last-child i, .key .seg:nth-last-child(2) i { left: auto; right: 0; transform: none; }
.key .whole { font-size: 7.4pt; margin-top: 1mm; overflow-wrap: anywhere; }
.party { margin-top: 3mm; }
.alert { margin-top: 3mm; border: 1.5pt solid #000; border-radius: 2.2mm; padding: 2mm 3mm; font-weight: 700; }
table.lines { width: 100%; border-collapse: collapse; margin-top: 3mm; }
table.lines thead { display: table-header-group; }
table.lines th { font-weight: 700; font-size: 7.4pt; text-align: left; vertical-align: bottom;
  background: #ececec; border-top: 0.75pt solid #000; border-bottom: 0.75pt solid #000; padding: 1.1mm 1.4mm; }
table.lines th.n { text-align: right; }
table.lines td { padding: 1mm 1.4mm; border-bottom: 0.4pt solid #b4b4b4; vertical-align: top; overflow-wrap: anywhere; }
table.lines tr { break-inside: avoid; }
table.lines td .extra { color: #4a4a4a; font-size: 7.2pt; margin-top: 0.3mm; }
table.lines tr.group td { background: #f4f4f4; border-bottom: 0.75pt solid #000; padding-top: 1.4mm; }
.section { margin-top: 3.5mm; }
.section h2 { font-size: 8.6pt; font-weight: 700; margin: 0 0 1mm; }
.foot { display: flex; gap: 3mm; margin-top: 3mm; align-items: flex-start; break-inside: avoid; }
.foot .left { flex: 1 1 56%; display: flex; flex-direction: column; gap: 3mm; }
.foot .right { flex: 0 0 42%; margin-left: auto; }
table.totals { width: 100%; border-collapse: collapse; border-top: 0.75pt solid #000; }
table.totals td { padding: 0.75mm 1.4mm; border-bottom: 0.4pt solid #b4b4b4; }
table.totals tr.grand td { font-weight: 700; font-size: 9.6pt; border-top: 1pt solid #000; border-bottom: 1pt solid #000; padding: 1.3mm 1.4mm; }
table.plain { width: 100%; border-collapse: collapse; }
table.plain th { text-align: left; font-weight: 700; font-size: 7.4pt; border-bottom: 0.75pt solid #000; padding: 0.6mm 1.2mm 0.8mm; }
table.plain th.n { text-align: right; }
table.plain td { padding: 0.7mm 1.2mm; border-bottom: 0.4pt solid #b4b4b4; vertical-align: top; overflow-wrap: anywhere; }
.end { break-inside: avoid; }
.note { margin-top: 5mm; padding-top: 1.4mm; border-top: 0.4pt solid #8a8a8a; font-size: 6.8pt; color: #333; }
"#;

/// `15.00` → `15`, `12.50` → `12.5`; anything that is not a plain decimal
/// is returned trimmed. Pure.
pub fn trim_decimal(s: &str) -> String {
    let s = s.trim();
    if !is_decimal(s) || !s.contains('.') {
        return s.to_string();
    }
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn is_decimal(s: &str) -> bool {
    let body = s.strip_prefix('-').unwrap_or(s);
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    !body.is_empty()
        && !(int.is_empty() && frac.is_empty())
        && int.bytes().all(|b| b.is_ascii_digit())
        && frac.bytes().all(|b| b.is_ascii_digit())
}

/// A decimal with at least `min_frac` decimals and no trailing zeros past
/// them (`100.000000` → `100.00`, `0.123400` → `0.1234`); never rounded,
/// because the file's figure is the legal one. Anything that is not a plain
/// decimal is returned trimmed. Pure.
pub fn amount(s: &str, min_frac: usize) -> String {
    let s = s.trim();
    if !is_decimal(s) {
        return s.to_string();
    }
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, s),
    };
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    let int = int.trim_start_matches('0');
    let int = if int.is_empty() { "0" } else { int };
    let mut frac = frac.trim_end_matches('0').to_string();
    while frac.len() < min_frac {
        frac.push('0');
    }
    let sign = if neg { "-" } else { "" };
    if frac.is_empty() {
        format!("{sign}{int}")
    } else {
        format!("{sign}{int}.{frac}")
    }
}

/// `2026-11-08T15:10:22-05:00` → `08/11/2026 15:10:22 -05:00` (fractions of
/// a second dropped); anything else is returned as written. Pure.
pub fn auth_date(s: &str) -> String {
    let s = s.trim();
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| {
        b.get(r.clone())
            .is_some_and(|x| x.iter().all(u8::is_ascii_digit))
    };
    if b.len() >= 19
        && digits(0..4)
        && b[4] == b'-'
        && digits(5..7)
        && b[7] == b'-'
        && digits(8..10)
        && (b[10] == b'T' || b[10] == b' ')
        && digits(11..13)
        && b[13] == b':'
        && digits(14..16)
        && b[16] == b':'
        && digits(17..19)
    {
        let mut rest = &s[19..];
        if let Some(frac) = rest.strip_prefix('.') {
            rest = frac.trim_start_matches(|c: char| c.is_ascii_digit());
        }
        let zone = match rest {
            "" => String::new(),
            "Z" => " UTC".to_string(),
            z => format!(" {z}"),
        };
        return format!(
            "{}/{}/{} {}{zone}",
            &s[8..10],
            &s[5..7],
            &s[0..4],
            &s[11..19]
        );
    }
    s.to_string()
}

/// The nine segments of a 49-digit access key with their captions, or
/// `None` for anything else. Pure.
pub fn key_segments(key: &str) -> Option<Vec<(&str, &'static str)>> {
    if key.len() != 49 || !key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let parts: [(usize, &'static str); 9] = [
        (8, "fecha"),
        (2, "tipo"),
        (13, "RUC"),
        (1, "amb."),
        (6, "serie"),
        (9, "secuencial"),
        (8, "código"),
        (1, "em."),
        (1, "DV"),
    ];
    let mut at = 0;
    Some(
        parts
            .iter()
            .map(|(len, caption)| {
                let seg = &key[at..at + len];
                at += len;
                (seg, *caption)
            })
            .collect(),
    )
}

/// One `<dt>/<dd>` pair, skipped when `value` is empty.
fn fact(out: &mut String, label: &str, value: &str) {
    if !value.trim().is_empty() {
        out.push_str(&format!("<dt>{}</dt><dd>{}</dd>", e(label), e(value)));
    }
}

/// The footer, in the app language.
fn footer(r: &Receipt, lang: Lang) -> String {
    let doc = format!(
        "{}{}",
        r.kind.title(),
        if r.version.is_empty() {
            String::new()
        } else {
            match lang {
                Lang::Es => format!(", esquema {}", r.version),
                Lang::En => format!(", schema {}", r.version),
            }
        }
    );
    let sentence = match (&r.authorization, lang) {
        (Some(a), Lang::Es) if a.is_authorized() => {
            "Representación impresa generada por Pluryprint a partir del XML autorizado."
        }
        (Some(_), Lang::Es) => {
            "Representación impresa generada por Pluryprint a partir del XML. El SRI no autorizó este comprobante."
        }
        (None, Lang::Es) => {
            "Representación impresa generada por Pluryprint a partir del XML del comprobante, que no incluye los datos de autorización del SRI."
        }
        (Some(a), Lang::En) if a.is_authorized() => {
            "Printed representation generated by Pluryprint from the authorised XML."
        }
        (Some(_), Lang::En) => {
            "Printed representation generated by Pluryprint from the XML. The SRI did not authorise this receipt."
        }
        (None, Lang::En) => {
            "Printed representation generated by Pluryprint from the receipt's XML, which does not include the SRI's authorisation."
        }
    };
    format!(
        "<div class=\"note\">{} <span class=\"muted\">{}</span></div>\n",
        e(sentence),
        e(&doc)
    )
}

fn head(r: &Receipt) -> String {
    let i = &r.issuer;
    let mut out = String::from("<div class=\"head\">\n<div class=\"box issuer\">\n<div>");
    out.push_str(&format!(
        "<div class=\"name\">{}</div>",
        e(if i.razon_social.is_empty() {
            "—"
        } else {
            &i.razon_social
        })
    ));
    if !i.nombre_comercial.is_empty() && i.nombre_comercial != i.razon_social {
        out.push_str(&format!(
            "<div class=\"trade\">{}</div>",
            e(&i.nombre_comercial)
        ));
    }
    out.push_str("</div>\n<dl class=\"facts\">");
    fact(&mut out, "Dirección matriz", &i.dir_matriz);
    if i.dir_establecimiento != i.dir_matriz {
        fact(&mut out, "Dirección sucursal", &i.dir_establecimiento);
    }
    fact(
        &mut out,
        "Contribuyente especial Nro.",
        &i.contribuyente_especial,
    );
    fact(
        &mut out,
        "Obligado a llevar contabilidad",
        &i.obligado_contabilidad,
    );
    fact(
        &mut out,
        "Agente de retención, resolución Nro.",
        &i.agente_retencion,
    );
    if !i.regimen.is_empty() {
        out.push_str(&format!("<dt>Régimen</dt><dd>{}</dd>", e(&i.regimen)));
    }
    out.push_str("</dl>\n</div>\n");

    // The document box.
    out.push_str("<div class=\"box doc\">\n");
    out.push_str(&format!(
        "<div class=\"ruc\"><b>R.U.C.</b><span class=\"mono\">{}</span></div>\n",
        e(&i.ruc)
    ));
    out.push_str(&format!(
        "<div class=\"type\">{}</div>\n",
        e(r.kind.title())
    ));
    if !r.numero.is_empty() {
        out.push_str(&format!(
            "<div class=\"no\">No. <span class=\"mono\">{}</span></div>\n",
            e(&r.numero)
        ));
    }
    let missing = "No consta en el archivo";
    // The authorisation number is a 49-digit key (10 digits for the old
    // offline scheme): it gets a line of its own, unbroken.
    let numero = r
        .authorization
        .as_ref()
        .map(|a| a.numero.as_str())
        .filter(|n| !n.is_empty());
    out.push_str("<div class=\"auth\"><div class=\"label\">Número de autorización</div>");
    match numero {
        Some(n) => out.push_str(&format!("<div class=\"authno mono\">{}</div>", e(n))),
        None => out.push_str(&format!("<div>{}</div>", e(missing))),
    }
    out.push_str("</div>\n<dl class=\"facts\">");
    match &r.authorization {
        Some(a) => {
            let fecha = auth_date(&a.fecha);
            fact(
                &mut out,
                "Fecha y hora de autorización",
                if fecha.is_empty() { missing } else { &fecha },
            );
            if !a.is_authorized() {
                fact(&mut out, "Estado en el SRI", &a.estado);
            }
        }
        None => fact(&mut out, "Fecha y hora de autorización", missing),
    }
    let ambiente = if r.ambiente.is_empty() {
        r.authorization
            .as_ref()
            .map(|a| a.ambiente.clone())
            .unwrap_or_default()
    } else {
        sri::ambiente_name(&r.ambiente)
    };
    fact(&mut out, "Ambiente", &ambiente);
    fact(&mut out, "Emisión", &sri::emision_name(&r.tipo_emision));
    out.push_str("</dl>\n");
    if !r.clave_acceso.is_empty() {
        out.push_str("<div class=\"key\"><div class=\"label\">Clave de acceso</div>");
        if let Some(svg) = code128::svg(&r.clave_acceso, 11.0) {
            out.push_str(&svg);
        }
        match key_segments(&r.clave_acceso) {
            Some(segs) => {
                out.push_str("<div class=\"segs mono\">");
                for (digits, caption) in segs {
                    out.push_str(&format!(
                        "<span class=\"seg\"><b>{}</b><i>{}</i></span>",
                        e(digits),
                        e(caption)
                    ));
                }
                out.push_str("</div>");
            }
            None => out.push_str(&format!(
                "<div class=\"whole mono\">{}</div>",
                e(&r.clave_acceso)
            )),
        }
        out.push_str("</div>\n");
    }
    out.push_str("</div>\n</div>\n");
    out
}

fn alert(r: &Receipt) -> String {
    match &r.authorization {
        Some(a) if !a.is_authorized() => format!(
            "<div class=\"alert\">Este comprobante no está autorizado por el SRI (estado: {}).</div>\n",
            e(if a.estado.is_empty() { "sin estado" } else { &a.estado })
        ),
        _ => String::new(),
    }
}

fn party(r: &Receipt) -> String {
    let mut facts = String::new();
    if let Some(p) = &r.party {
        let name_label = match r.kind {
            Kind::Retencion => "Sujeto retenido",
            Kind::LiquidacionCompra => "Proveedor",
            _ => "Razón social / Nombres y apellidos",
        };
        fact(&mut facts, name_label, &p.nombre);
        fact(
            &mut facts,
            sri::id_type_name(&p.tipo_identificacion),
            &p.identificacion,
        );
        fact(&mut facts, "Dirección", &p.direccion);
    }
    fact(&mut facts, "Fecha de emisión", &r.fecha_emision);
    for (label, value) in &r.header {
        fact(&mut facts, label, value);
    }
    if facts.is_empty() {
        return String::new();
    }
    format!("<div class=\"box party\"><dl class=\"facts\">{facts}</dl></div>\n")
}

fn item_rows(r: &Receipt) -> String {
    if r.items.is_empty() {
        return String::new();
    }
    let with_aux = r.items.iter().any(|i| !i.codigo_aux.is_empty());
    let mut out = String::from("<table class=\"lines\"><thead><tr><th>Cód. principal</th>");
    if with_aux {
        out.push_str("<th>Cód. auxiliar</th>");
    }
    out.push_str(
        "<th class=\"n\">Cantidad</th><th>Descripción</th><th class=\"n\">Precio unitario</th>\
         <th class=\"n\">Descuento</th><th class=\"n\">Precio total</th></tr></thead><tbody>\n",
    );
    for i in &r.items {
        out.push_str(&format!("<tr><td class=\"mono\">{}</td>", e(&i.codigo)));
        if with_aux {
            out.push_str(&format!("<td class=\"mono\">{}</td>", e(&i.codigo_aux)));
        }
        let mut desc = e(&i.descripcion);
        if !i.unidad.is_empty() {
            desc.push_str(&format!(" <span class=\"muted\">({})</span>", e(&i.unidad)));
        }
        for (k, v) in &i.adicionales {
            desc.push_str(&format!(
                "<div class=\"extra\">{}{}{}</div>",
                e(k),
                if k.is_empty() || v.is_empty() {
                    ""
                } else {
                    ": "
                },
                e(v)
            ));
        }
        out.push_str(&format!(
            "<td class=\"n\">{}</td><td>{desc}</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td></tr>\n",
            e(&amount(&i.cantidad, 2)),
            e(&amount(&i.precio_unitario, 2)),
            e(&amount(&i.descuento, 2)),
            e(&amount(&i.total, 2)),
        ));
    }
    out.push_str("</tbody></table>\n");
    out
}

fn motivo_rows(r: &Receipt) -> String {
    if r.motivos.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "<table class=\"lines\"><thead><tr><th>Razón de la modificación</th><th class=\"n\">Valor de la modificación</th></tr></thead><tbody>\n",
    );
    for (razon, valor) in &r.motivos {
        out.push_str(&format!(
            "<tr><td>{}</td><td class=\"n\">{}</td></tr>\n",
            e(razon),
            e(&amount(valor, 2))
        ));
    }
    out.push_str("</tbody></table>\n");
    out
}

fn doc_number(n: &str) -> String {
    // 001001000000123 → 001-001-000000123, as people write it.
    if n.len() == 15 && n.bytes().all(|b| b.is_ascii_digit()) {
        format!("{}-{}-{}", &n[0..3], &n[3..6], &n[6..])
    } else {
        n.to_string()
    }
}

fn withholding_cells(w: &Withholding) -> String {
    format!(
        "<td>{}</td><td class=\"mono\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td>",
        e(&sri::withheld_tax_name(&w.codigo)),
        e(&w.codigo_retencion),
        e(&amount(&w.base, 2)),
        e(&trim_decimal(&w.porcentaje)),
        e(&amount(&w.valor, 2)),
    )
}

fn retention_rows(r: &Receipt) -> String {
    let mut out = String::new();
    let period = r
        .header
        .iter()
        .find(|(k, _)| k == "Período fiscal")
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    if !r.withholdings.is_empty() {
        out.push_str(
            "<table class=\"lines\"><thead><tr><th>Comprobante</th><th>Número</th><th>Fecha de emisión</th>\
             <th>Ejercicio fiscal</th><th>Impuesto</th><th>Código</th><th class=\"n\">Base imponible</th>\
             <th class=\"n\">% de retención</th><th class=\"n\">Valor retenido</th></tr></thead><tbody>\n",
        );
        for w in &r.withholdings {
            let doc = w.doc.clone().unwrap_or_default();
            out.push_str(&format!(
                "<tr><td>{}</td><td class=\"mono nowrap\">{}</td><td class=\"nowrap\">{}</td><td>{}</td>{}</tr>\n",
                e(&sri::doc_name(&doc.cod_doc)),
                e(&doc_number(&doc.numero)),
                e(&doc.fecha),
                e(period),
                withholding_cells(w)
            ));
        }
        out.push_str("</tbody></table>\n");
    }
    if !r.support.is_empty() {
        out.push_str(
            "<table class=\"lines\"><thead><tr><th>Impuesto</th><th>Código</th><th class=\"n\">Base imponible</th>\
             <th class=\"n\">% de retención</th><th class=\"n\">Valor retenido</th></tr></thead><tbody>\n",
        );
        for g in &r.support {
            out.push_str(&format!(
                "<tr class=\"group\"><td colspan=\"5\">{}</td></tr>\n",
                support_doc_line(&g.doc)
            ));
            for w in &g.retenciones {
                out.push_str(&format!("<tr>{}</tr>\n", withholding_cells(w)));
            }
        }
        out.push_str("</tbody></table>\n");
    }
    out
}

fn support_doc_line(d: &SupportDoc) -> String {
    let mut parts = vec![format!(
        "<b>{}</b> <span class=\"mono\">{}</span>",
        e(&sri::doc_name(&d.cod_doc)),
        e(&doc_number(&d.numero))
    )];
    if !d.fecha.is_empty() {
        parts.push(format!("emitido el {}", e(&d.fecha)));
    }
    if !d.importe_total.is_empty() {
        parts.push(format!("total {}", e(&amount(&d.importe_total, 2))));
    }
    if !d.autorizacion.is_empty() {
        parts.push(format!(
            "autorización <span class=\"mono\">{}</span>",
            e(&d.autorizacion)
        ));
    }
    parts.join(", ")
}

fn shipment(r: &Receipt) -> String {
    let Some(s) = &r.shipment else {
        return String::new();
    };
    let mut out = String::from("<div class=\"box party\"><dl class=\"facts\">");
    fact(&mut out, "Dirección de partida", &s.dir_partida);
    fact(&mut out, "Transportista", &s.transportista);
    fact(
        &mut out,
        "Identificación del transportista",
        &s.ruc_transportista,
    );
    fact(&mut out, "Placa", &s.placa);
    fact(&mut out, "Inicio del transporte", &s.fecha_inicio);
    fact(&mut out, "Fin del transporte", &s.fecha_fin);
    for (label, value) in &r.header {
        fact(&mut out, label, value);
    }
    out.push_str("</dl></div>\n");
    for d in &s.destinatarios {
        out.push_str("<div class=\"section\"><div class=\"box\"><dl class=\"facts\">");
        fact(&mut out, "Destinatario", &d.nombre);
        fact(&mut out, "Identificación", &d.identificacion);
        fact(&mut out, "Dirección", &d.direccion);
        fact(&mut out, "Motivo del traslado", &d.motivo);
        fact(&mut out, "Ruta", &d.ruta);
        fact(&mut out, "Documento aduanero", &d.doc_aduanero);
        fact(&mut out, "Establecimiento de destino", &d.estab_destino);
        if let Some(doc) = &d.doc {
            let line = format!(
                "{} {}",
                sri::doc_name(&doc.cod_doc),
                doc_number(&doc.numero)
            );
            fact(&mut out, "Comprobante de venta", line.trim());
            fact(&mut out, "Fecha de emisión", &doc.fecha);
            fact(&mut out, "Autorización", &doc.autorizacion);
        }
        out.push_str("</dl></div>\n");
        if !d.items.is_empty() {
            let with_aux = d.items.iter().any(|i| !i.codigo_aux.is_empty());
            out.push_str(
                "<table class=\"lines\"><thead><tr><th class=\"n\">Cantidad</th><th>Descripción</th>\
                 <th>Cód. principal</th>",
            );
            if with_aux {
                out.push_str("<th>Cód. auxiliar</th>");
            }
            out.push_str("</tr></thead><tbody>\n");
            for i in &d.items {
                let mut desc = e(&i.descripcion);
                for (k, v) in &i.adicionales {
                    desc.push_str(&format!(
                        "<div class=\"extra\">{}{}{}</div>",
                        e(k),
                        if k.is_empty() || v.is_empty() {
                            ""
                        } else {
                            ": "
                        },
                        e(v)
                    ));
                }
                out.push_str(&format!(
                    "<tr><td class=\"n\">{}</td><td>{desc}</td><td class=\"mono\">{}</td>",
                    e(&amount(&i.cantidad, 2)),
                    e(&i.codigo),
                ));
                if with_aux {
                    out.push_str(&format!("<td class=\"mono\">{}</td>", e(&i.codigo_aux)));
                }
                out.push_str("</tr>\n");
            }
            out.push_str("</tbody></table>\n");
        }
        out.push_str("</div>\n");
    }
    out
}

fn totals(r: &Receipt) -> String {
    let mut rows: Vec<(String, String)> = Vec::new();
    let push = |rows: &mut Vec<(String, String)>, label: &str, value: &str| {
        rows.push((label.to_string(), amount(value, 2)));
    };
    match r.kind {
        Kind::GuiaRemision => return String::new(),
        Kind::Retencion => {
            if r.total.is_empty() {
                return String::new();
            }
            return format!(
                "<table class=\"totals\"><tr class=\"grand\"><td>Total retenido (calculado)</td><td class=\"n\">{}</td></tr></table>",
                e(&amount(&r.total, 2))
            );
        }
        _ => {}
    }
    for t in r.taxes.iter().filter(|t| t.codigo == "2") {
        push(&mut rows, &sri::tax_labels(t).0, &t.base);
    }
    if !r.subtotal.is_empty() {
        push(&mut rows, "Subtotal sin impuestos", &r.subtotal);
    }
    if !r.descuento.is_empty() {
        push(&mut rows, "Total descuento", &r.descuento);
    }
    // ICE and the tip are printed only when the file carries them: a row
    // of 0.00 the XML does not have would be invented data.
    for t in r.taxes.iter().filter(|t| t.codigo == "3") {
        push(&mut rows, "ICE", &t.valor);
    }
    for t in r
        .taxes
        .iter()
        .filter(|t| t.codigo == "2" && !matches!(t.codigo_porcentaje.as_str(), "0" | "6" | "7"))
    {
        push(&mut rows, &sri::tax_labels(t).1, &t.valor);
    }
    for t in r
        .taxes
        .iter()
        .filter(|t| t.codigo != "2" && t.codigo != "3")
    {
        push(&mut rows, &sri::tax_labels(t).1, &t.valor);
    }
    if r.kind == Kind::Factura && !r.propina.is_empty() {
        push(&mut rows, "Propina", &r.propina);
    }
    let mut out = String::from("<table class=\"totals\">");
    for (label, value) in rows {
        out.push_str(&format!(
            "<tr><td>{}</td><td class=\"n\">{}</td></tr>",
            e(&label),
            e(&value)
        ));
    }
    let grand = match r.kind {
        Kind::NotaCredito => "Valor de la modificación",
        _ => "Valor total",
    };
    out.push_str(&format!(
        "<tr class=\"grand\"><td>{}</td><td class=\"n\">{}</td></tr></table>",
        e(grand),
        e(&amount(&r.total, 2))
    ));
    out
}

fn left_column(r: &Receipt) -> String {
    let mut out = String::new();
    if !r.adicional.is_empty() {
        out.push_str("<div class=\"box\"><div class=\"section\" style=\"margin:0\"><h2>Información adicional</h2><dl class=\"facts\">");
        for (k, v) in &r.adicional {
            out.push_str(&format!("<dt>{}</dt><dd>{}</dd>", e(k), e(v)));
        }
        out.push_str("</dl></div></div>");
    }
    if !r.payments.is_empty() {
        let with_term = r.payments.iter().any(|p| !p.plazo.is_empty());
        out.push_str(
            "<table class=\"plain\"><thead><tr><th>Forma de pago</th><th class=\"n\">Valor</th>",
        );
        if with_term {
            out.push_str("<th class=\"n\">Plazo</th>");
        }
        out.push_str("</tr></thead><tbody>");
        for p in &r.payments {
            out.push_str(&format!(
                "<tr><td>{}</td><td class=\"n\">{}</td>",
                e(&sri::payment_name(&p.forma_pago)),
                e(&amount(&p.total, 2))
            ));
            if with_term {
                out.push_str(&format!("<td class=\"n\">{}</td>", e(&p.plazo)));
            }
            out.push_str("</tr>");
        }
        out.push_str("</tbody></table>");
    }
    out
}

/// The whole RIDE page for `r`, titled `title`. Pure.
pub fn to_html(r: &Receipt, title: &str, lang: Lang) -> String {
    let mut body = String::new();
    body.push_str(&head(r));
    body.push_str(&alert(r));
    if r.kind == Kind::GuiaRemision {
        body.push_str(&shipment(r));
    } else {
        body.push_str(&party(r));
    }
    body.push_str(&item_rows(r));
    body.push_str(&motivo_rows(r));
    body.push_str(&retention_rows(r));
    // The closing blocks and the footer travel together, so the footer is
    // never alone on a last page.
    let left = left_column(r);
    let right = totals(r);
    body.push_str("<div class=\"end\">");
    if !left.is_empty() || !right.is_empty() {
        body.push_str(&format!(
            "<div class=\"foot\"><div class=\"left\">{left}</div><div class=\"right\">{right}</div></div>\n"
        ));
    }
    body.push_str(&footer(r, lang));
    body.push_str("</div>\n");
    format!(
        "<!DOCTYPE html>\n<html lang=\"es\"><head><meta charset=\"utf-8\"><title>{}</title>\n\
         <style>{RIDE_CSS}</style>\n</head><body>\n{body}</body></html>\n",
        e(title)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr_057b_amounts_are_never_rounded() {
        assert_eq!(amount("100.000000", 2), "100.00");
        assert_eq!(amount("1.500000", 2), "1.50");
        assert_eq!(amount("0.123400", 2), "0.1234");
        assert_eq!(amount("007.1", 2), "7.10");
        assert_eq!(amount("15", 2), "15.00");
        assert_eq!(amount("-3.5", 2), "-3.50");
        assert_eq!(amount(" 2.00 ", 0), "2");
        assert_eq!(amount("N/A", 2), "N/A");
        assert_eq!(amount("", 2), "");
        assert_eq!(amount("1e5", 2), "1e5");
        assert_eq!(trim_decimal("15.00"), "15");
        assert_eq!(trim_decimal("12.50"), "12.5");
        assert_eq!(trim_decimal("2"), "2");
        assert_eq!(trim_decimal("x.10"), "x.10");
    }

    #[test]
    fn fr_057b_authorisation_dates_read_as_in_ecuador() {
        assert_eq!(
            auth_date("2026-11-08T15:10:22-05:00"),
            "08/11/2026 15:10:22 -05:00"
        );
        assert_eq!(
            auth_date("2026-11-08T15:10:22.317-05:00"),
            "08/11/2026 15:10:22 -05:00"
        );
        assert_eq!(auth_date("2026-11-08T15:10:22Z"), "08/11/2026 15:10:22 UTC");
        assert_eq!(auth_date("08/11/2026 15:10:22"), "08/11/2026 15:10:22");
        assert_eq!(auth_date(""), "");
    }

    #[test]
    fn fr_057b_access_key_segments() {
        let key = "1411202601099999999900110010010000000011234567811";
        let segs = key_segments(key).unwrap();
        let digits: Vec<&str> = segs.iter().map(|(d, _)| *d).collect();
        assert_eq!(
            digits,
            [
                "14112026",
                "01",
                "0999999999001",
                "1",
                "001001",
                "000000001",
                "12345678",
                "1",
                "1"
            ]
        );
        assert_eq!(digits.concat(), key);
        assert!(key_segments("123").is_none());
        assert!(key_segments(&"x".repeat(49)).is_none());
    }

    #[test]
    fn fr_057b_doc_numbers_get_their_dashes() {
        assert_eq!(doc_number("001001000000123"), "001-001-000000123");
        assert_eq!(doc_number("001-001-000000123"), "001-001-000000123");
        assert_eq!(doc_number("12"), "12");
    }
}
