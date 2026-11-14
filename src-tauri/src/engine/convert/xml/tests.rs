//! FR-057a / FR-057b: the XML converter's parser, model, pages and its
//! behaviour on hostile files. Every fixture is fictitious (issuer RUC
//! 0999999999001, made-up access keys with a valid check digit) and lives in
//! `tests/fixtures/sri-xml/`.
//!
//! The HTML snapshots are in `tests/fixtures/sri-xml/snapshots/`. After an
//! intended change to the pages, regenerate them with
//! `PLURYPRINT_BLESS=1 cargo test --lib xml` and review the diff.

use std::path::PathBuf;

use super::sri::{self, Kind};
use super::*;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sri-xml")
}

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixtures().join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn receipt(name: &str) -> sri::Receipt {
    let bytes = fixture(name);
    let text = decode(&bytes).unwrap();
    let doc = parse(&text).unwrap();
    sri::read(&doc).unwrap_or_else(|| panic!("{name} is an SRI receipt"))
}

fn html(name: &str) -> String {
    to_html(&fixture(name), name, Lang::Es).unwrap()
}

/// Every fixture with the type and version it must be read as.
const RECEIPTS: &[(&str, Kind, &str, bool)] = &[
    (
        "factura-1.0.0-respuesta-soap.xml",
        Kind::Factura,
        "1.0.0",
        true,
    ),
    (
        "factura-1.1.0-autorizacion.xml",
        Kind::Factura,
        "1.1.0",
        true,
    ),
    (
        "factura-2.0.0-no-autorizado.xml",
        Kind::Factura,
        "2.0.0",
        true,
    ),
    ("factura-2.1.0.xml", Kind::Factura, "2.1.0", false),
    (
        "liquidacion-compra-1.0.0.xml",
        Kind::LiquidacionCompra,
        "1.0.0",
        false,
    ),
    (
        "liquidacion-compra-1.1.0.xml",
        Kind::LiquidacionCompra,
        "1.1.0",
        true,
    ),
    ("nota-credito-1.0.0.xml", Kind::NotaCredito, "1.0.0", false),
    ("nota-credito-1.1.0.xml", Kind::NotaCredito, "1.1.0", true),
    ("nota-debito-1.0.0.xml", Kind::NotaDebito, "1.0.0", true),
    (
        "guia-remision-1.0.0.xml",
        Kind::GuiaRemision,
        "1.0.0",
        false,
    ),
    ("guia-remision-1.1.0.xml", Kind::GuiaRemision, "1.1.0", true),
    ("retencion-1.0.0.xml", Kind::Retencion, "1.0.0", true),
    ("retencion-2.0.0.xml", Kind::Retencion, "2.0.0", true),
];

// ---------------------------------------------------------------------------
// Recognition and model
// ---------------------------------------------------------------------------

#[test]
fn fr_057b_every_type_and_version_is_recognised() {
    for (name, kind, version, wrapped) in RECEIPTS {
        let r = receipt(name);
        assert_eq!(r.kind, *kind, "{name}");
        assert_eq!(r.version, *version, "{name}");
        assert_eq!(r.authorization.is_some(), *wrapped, "{name}");
        assert_eq!(r.issuer.ruc, "0999999999001", "{name}");
        assert_eq!(r.clave_acceso.len(), 49, "{name}");
        let check = sri::access_key_check_digit(&r.clave_acceso[..48]).unwrap();
        assert_eq!(
            r.clave_acceso.as_bytes()[48] - b'0',
            check,
            "{name}: fixture keys carry a valid check digit"
        );
        assert!(!r.numero.is_empty(), "{name}");
    }
}

#[test]
fn fr_057b_factura_in_its_autorizacion_reads_every_section() {
    let r = receipt("factura-1.1.0-autorizacion.xml");
    let a = r.authorization.as_ref().unwrap();
    assert!(a.is_authorized());
    assert_eq!(a.numero, r.clave_acceso);
    assert_eq!(a.fecha, "2026-10-15T10:24:31-05:00");
    assert_eq!(r.numero, "001-002-000000123");
    assert_eq!(r.fecha_emision, "15/10/2026");
    assert_eq!(
        r.issuer.razon_social,
        "COMERCIAL ANDINA DE PRUEBAS S.A. & ASOCIADOS"
    );
    assert_eq!(r.issuer.nombre_comercial, "Ferretería La Prueba");
    assert_eq!(r.issuer.contribuyente_especial, "12345");
    assert_eq!(r.issuer.obligado_contabilidad, "SI");
    assert_eq!(r.issuer.regimen, "CONTRIBUYENTE RÉGIMEN RIMPE");
    let p = r.party.as_ref().unwrap();
    assert_eq!(p.nombre, "CLIENTE FICTICIO CÍA. LTDA.");
    assert_eq!(p.tipo_identificacion, "04");
    assert_eq!(r.items.len(), 4);
    assert_eq!(
        r.items[0].adicionales,
        [("Garantía".into(), "12 meses".into())]
    );
    assert_eq!(r.items[0].codigo_aux, "7861234500011");
    assert_eq!(r.taxes.len(), 2);
    assert_eq!(
        sri::tax_labels(&r.taxes[0]),
        ("Subtotal 15%".into(), "IVA 15%".into())
    );
    assert_eq!(r.total, "207.88");
    assert_eq!(r.payments.len(), 2);
    assert_eq!(r.payments[0].plazo, "30 días");
    assert_eq!(r.adicional.len(), 3);
    assert!(r
        .header
        .contains(&("Guía de remisión".into(), "001-002-000000045".into())));
}

#[test]
fn fr_057b_soap_response_uses_the_authorised_one_and_escaped_text() {
    let r = receipt("factura-1.0.0-respuesta-soap.xml");
    let a = r.authorization.unwrap();
    assert!(
        a.is_authorized(),
        "the AUTORIZADO entry wins over the first"
    );
    assert_eq!(a.ambiente, "PRODUCCIÓN");
    assert_eq!(r.taxes[0].codigo_porcentaje, "2");
    assert_eq!(
        sri::tax_labels(&r.taxes[0]),
        ("Subtotal 12%".into(), "IVA 12%".into())
    );
}

#[test]
fn fr_057b_retention_versions_have_their_own_shapes() {
    let v1 = receipt("retencion-1.0.0.xml");
    assert_eq!(v1.withholdings.len(), 2);
    assert!(v1.support.is_empty());
    let w = &v1.withholdings[0];
    assert_eq!(
        (w.codigo.as_str(), w.codigo_retencion.as_str()),
        ("1", "312")
    );
    assert_eq!(w.doc.as_ref().unwrap().numero, "001001000004567");
    assert_eq!(v1.total, "62.50");
    assert_eq!(v1.party.as_ref().unwrap().nombre, "PROVEEDOR FICTICIO S.A.");

    let v2 = receipt("retencion-2.0.0.xml");
    assert!(v2.withholdings.is_empty());
    assert_eq!(v2.support.len(), 2);
    assert_eq!(v2.support[1].retenciones.len(), 2);
    assert_eq!(v2.support[0].doc.importe_total, "920.00");
    assert_eq!(v2.support[0].doc.autorizacion.len(), 49);
    // 14.00 + 12.00 + 6.88 + 7.50, added exactly.
    assert_eq!(v2.total, "40.38");
    assert!(v2
        .header
        .contains(&("Período fiscal".into(), "11/2026".into())));
}

#[test]
fn fr_057b_delivery_guide_reads_transport_and_recipients() {
    let r = receipt("guia-remision-1.1.0.xml");
    assert!(r.party.is_none());
    let s = r.shipment.unwrap();
    assert_eq!(s.placa, "GAA-0000");
    assert_eq!(s.transportista, "TRANSPORTES DE PRUEBA S.A.");
    assert_eq!(s.destinatarios.len(), 2);
    assert_eq!(s.destinatarios[0].items.len(), 2);
    assert_eq!(s.destinatarios[0].ruta, "Guayaquil - Quito");
    assert_eq!(
        s.destinatarios[0].doc.as_ref().unwrap().numero,
        "001-002-000000123"
    );
    let old = receipt("guia-remision-1.0.0.xml").shipment.unwrap();
    assert_eq!(old.destinatarios[0].estab_destino, "002");
    assert!(old.destinatarios[0].doc.is_none());
}

#[test]
fn fr_057b_notes_and_purchase_settlement() {
    let nc = receipt("nota-credito-1.1.0.xml");
    assert_eq!(nc.total, "41.69");
    assert_eq!(
        nc.items[0].codigo, "BRO-010",
        "codigoInterno is the main code"
    );
    assert!(nc.header.contains(&(
        "Comprobante que se modifica".into(),
        "Factura 001-002-000000123".into()
    )));
    assert!(nc.header.contains(&(
        "Razón de modificación".into(),
        "Devolución de un juego de brocas".into()
    )));

    let nd = receipt("nota-debito-1.0.0.xml");
    assert_eq!(nd.motivos.len(), 2);
    assert_eq!(nd.total, "13.80");
    assert_eq!(nd.taxes[0].tarifa, "15");

    let liq = receipt("liquidacion-compra-1.1.0.xml");
    let p = liq.party.unwrap();
    assert_eq!(p.nombre, "ARTESANO DE PRUEBA");
    assert_eq!(p.direccion, "Recinto Ficticio, Daule");
    assert_eq!(liq.items[0].unidad, "unidad");
    assert_eq!(liq.total, "300.00");
}

#[test]
fn fr_057b_unauthorised_and_unwrapped_receipts_say_so() {
    let page = html("factura-2.0.0-no-autorizado.xml");
    assert!(
        page.contains("no está autorizado por el SRI (estado: NO AUTORIZADO)"),
        "{page}"
    );
    assert!(page.contains("El SRI no autorizó este comprobante."));
    assert!(page.contains("Pruebas"), "environment 1 is Pruebas");

    let page = html("factura-2.1.0.xml");
    assert!(page.contains("No consta en el archivo"), "{page}");
    assert!(page.contains("que no incluye los datos de autorización del SRI"));
    assert!(!page.contains("class=\"alert\""));
}

#[test]
fn fr_057b_the_page_carries_the_ride_conventions() {
    let page = html("factura-1.1.0-autorizacion.xml");
    for needle in [
        "R.U.C.",
        "<div class=\"type\">Factura</div>",
        "No. <span class=\"mono\">001-002-000000123</span>",
        "Número de autorización",
        "08/11/2026",
        "Clave de acceso",
        "<svg class=\"barcode\"",
        "Subtotal 15%",
        "Subtotal 0%",
        "Subtotal sin impuestos",
        "Total descuento",
        "IVA 15%",
        "Propina",
        "Valor total",
        "207.88",
        "Información adicional",
        "Otros con utilización del sistema financiero",
        "Contribuyente especial Nro.",
        "Obligado a llevar contabilidad",
        "Representación impresa generada por Pluryprint a partir del XML autorizado.",
        "COMERCIAL ANDINA DE PRUEBAS S.A. &amp; ASOCIADOS",
    ] {
        if needle == "08/11/2026" {
            // The authorisation date is the fixture's, 15/10/2026.
            assert!(page.contains("15/10/2026 10:24:31 -05:00"), "{page}");
            continue;
        }
        assert!(page.contains(needle), "missing {needle:?}");
    }
    // Nothing loads: no scripts, no URLs.
    assert!(!page.contains("<script"));
    assert!(!page.contains("http://") || page.contains("http://www.w3.org/2000/svg"));
    assert!(!page.contains("https://"));
    assert!(!page.contains("src="));
}

#[test]
fn fr_057b_ice_and_tip_are_printed_only_when_the_file_has_them() {
    // No ICE tax in the file: no ICE row, not even 0.00.
    for name in [
        "factura-1.1.0-autorizacion.xml",
        "liquidacion-compra-1.1.0.xml",
        "liquidacion-compra-1.0.0.xml",
    ] {
        assert!(!html(name).contains("<td>ICE</td>"), "{name}");
    }
    let page = html("factura-2.1.0.xml");
    assert!(page.contains("<td>ICE</td>"), "the file has an ICE tax");
    // A tip of 0.00 the file states is printed; a missing one is not.
    assert!(html("factura-1.1.0-autorizacion.xml").contains("<td>Propina</td>"));
    let without_tip = String::from_utf8(fixture("factura-2.1.0.xml"))
        .unwrap()
        .replace("<propina>2.00</propina>", "");
    let page = to_html(without_tip.as_bytes(), "f.xml", Lang::Es).unwrap();
    assert!(!page.contains("Propina"), "{page}");
}

#[test]
fn fr_057b_footer_follows_the_app_language_labels_stay_spanish() {
    let bytes = fixture("retencion-1.0.0.xml");
    let en = to_html(&bytes, "r.xml", Lang::En).unwrap();
    assert!(en.contains("Printed representation generated by Pluryprint from the authorised XML."));
    assert!(en.contains("Comprobante de retención"));
    assert!(en.contains("Valor retenido"));
    assert!(en.contains("schema 1.0.0"));
    assert_eq!(Lang::from_locale("es-ES"), Lang::Es);
    assert_eq!(Lang::from_locale("es_EC"), Lang::Es);
    assert_eq!(Lang::from_locale("en-US"), Lang::En);
    assert_eq!(Lang::from_locale(""), Lang::En);
}

#[test]
fn fr_057b_access_key_check_digit() {
    // Mod 11 with weights 2..7 from the right; 11 → 0, 10 → 1.
    assert_eq!(sri::access_key_check_digit("123"), None);
    let r = receipt("factura-1.1.0-autorizacion.xml");
    let key = r.clave_acceso;
    assert_eq!(
        Some(key.as_bytes()[48] - b'0'),
        sri::access_key_check_digit(&key[..48])
    );
    assert_eq!(sri::access_key_check_digit(&"0".repeat(48)), Some(0));
}

#[test]
fn fr_057b_catalogues() {
    assert_eq!(sri::doc_name("01"), "Factura");
    assert_eq!(sri::doc_name("99"), "Código 99");
    assert_eq!(sri::payment_name("19"), "Tarjeta de crédito");
    assert_eq!(sri::id_type_name("07"), "Consumidor final");
    assert_eq!(sri::withheld_tax_name("6"), "ISD");
    let t = |cp: &str, tarifa: &str| sri::Tax {
        codigo: "2".into(),
        codigo_porcentaje: cp.into(),
        tarifa: tarifa.into(),
        ..sri::Tax::default()
    };
    assert_eq!(sri::tax_labels(&t("6", "")).0, "Subtotal no objeto de IVA");
    assert_eq!(sri::tax_labels(&t("7", "")).0, "Subtotal exento de IVA");
    assert_eq!(sri::tax_labels(&t("5", "")).1, "IVA 5%");
    assert_eq!(sri::tax_labels(&t("10", "")).1, "IVA 13%");
    assert_eq!(sri::tax_labels(&t("99", "8.00")).1, "IVA 8%");
    assert_eq!(sri::tax_labels(&t("99", "")).1, "IVA código 99");
}

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

/// Compare `actual` with the stored snapshot, or write it when
/// `PLURYPRINT_BLESS` is set.
fn snapshot(name: &str, actual: &str) {
    let path = fixtures().join("snapshots").join(name);
    if std::env::var_os("PLURYPRINT_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e} (run with PLURYPRINT_BLESS=1 to create it)",
            path.display()
        )
    });
    // Git may check the snapshot out with CRLF on Windows.
    let expected = expected.replace("\r\n", "\n");
    assert!(
        expected == actual,
        "{name} changed; review and rerun with PLURYPRINT_BLESS=1.\n--- actual ---\n{actual}"
    );
}

#[test]
fn fr_057a_fr_057b_html_snapshots() {
    for (name, ..) in RECEIPTS {
        snapshot(&name.replace(".xml", ".html"), &html(name));
    }
    snapshot("catalogo-generico.html", &html("catalogo-generico.xml"));
    let en = to_html(
        &fixture("catalogo-generico.xml"),
        "catalogo-generico.xml",
        Lang::En,
    )
    .unwrap();
    snapshot("catalogo-generico.en.html", &en);
}

// ---------------------------------------------------------------------------
// Generic XML
// ---------------------------------------------------------------------------

#[test]
fn fr_057a_generic_xml_prints_as_an_escaped_tree() {
    let page = html("catalogo-generico.xml");
    assert!(
        page.contains("Documento XML, elemento raíz &lt;cat:catalogo&gt;"),
        "{page}"
    );
    assert!(page.contains("&lt;cat:producto</span>"), "{page}");
    assert!(page.contains("<span class=\"a\">id=</span><span class=\"v\">\"p-1\"</span>"));
    assert!(
        page.contains("<span class=\"a\">xlink:href=</span>"),
        "{page}"
    );
    assert!(
        page.contains("Juego de brocas &amp;amp; accesorios")
            || page.contains("Juego de brocas &amp; accesorios")
    );
    // CDATA is text: printed, escaped, never markup.
    assert!(
        page.contains("&lt;b&gt;no es HTML&lt;/b&gt; &amp; se imprime tal cual"),
        "{page}"
    );
    assert!(!page.contains("<b>no es HTML"));
    assert!(page.contains("class=\"l c\""), "comments are shown: {page}");
    assert!(
        page.contains("xml-stylesheet"),
        "processing instructions are shown"
    );
    // Long text goes under its element.
    assert!(page.contains("class=\"l x txt\""), "{page}");
    // One guide per level; nothing nests.
    assert!(page.contains("<s><i></i><i></i></s>"), "{page}");
    assert!(!page.contains("<div class=\"l\"><div"), "{page}");
    // Past the guided levels, the rest is plain indentation.
    let deep = format!("{}{}", "<a>".repeat(40), "</a>".repeat(40));
    let page = to_html(deep.as_bytes(), "d.xml", Lang::En).unwrap();
    assert!(page.contains("style=\"--d:15\"><s>"), "{page}");
    assert!(page.contains("<u></u></s>"), "{page}");
}

#[test]
fn fr_057a_a_non_sri_autorizacion_or_broken_inner_receipt_prints_as_a_tree() {
    let cases = [
        // The inner receipt is not well-formed.
        "<autorizacion><estado>AUTORIZADO</estado><comprobante><![CDATA[<factura><infoTributaria>]]></comprobante></autorizacion>",
        // CDATA that ends early: the rest is text, still not a receipt.
        "<autorizacion><estado>AUTORIZADO</estado><comprobante><![CDATA[<factura>]]>]]&gt;</comprobante></autorizacion>",
        // A receipt-looking root without infoTributaria.
        "<factura version=\"1.0.0\"><infoFactura/></factura>",
        // An inner document with a DTD is never parsed as one.
        "<autorizacion><comprobante><![CDATA[<!DOCTYPE f [<!ENTITY a \"x\">]><factura><infoTributaria/></factura>]]></comprobante></autorizacion>",
    ];
    for xml in cases {
        let page = to_html(xml.as_bytes(), "x.xml", Lang::Es).unwrap();
        assert!(page.contains("<div class=\"tree\">"), "{xml}\n{page}");
        assert!(!page.contains("<!ENTITY"), "{page}");
    }
}

// ---------------------------------------------------------------------------
// Hostile input
// ---------------------------------------------------------------------------

#[test]
fn nfr_security_xml_hostile_input_table() {
    let deep_open = "<a>".repeat(10_000);
    let deep_close = "</a>".repeat(10_000);
    let deep = format!("{deep_open}{deep_close}");
    let billion = r#"<?xml version="1.0"?>
<!DOCTYPE lolz [
 <!ENTITY lol "lol">
 <!ENTITY lol2 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;">
 <!ENTITY lol3 "&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;">
]>
<lolz>&lol3;</lolz>"#;
    let cases: Vec<(&str, Vec<u8>, XmlError)> = vec![
        ("billion laughs", billion.as_bytes().to_vec(), XmlError::Dtd),
        (
            "external DTD",
            br#"<?xml version="1.0"?><!DOCTYPE factura SYSTEM "http://127.0.0.1:9/x.dtd"><factura/>"#.to_vec(),
            XmlError::Dtd,
        ),
        (
            "external entity",
            br#"<!DOCTYPE f [<!ENTITY x SYSTEM "file:///etc/passwd">]><f>&x;</f>"#.to_vec(),
            XmlError::Dtd,
        ),
        (
            "DTD after a comment and a PI",
            b"<?xml version=\"1.0\"?><!-- c --><?pi x?>\n<!DOCTYPE f><f/>".to_vec(),
            XmlError::Dtd,
        ),
        ("10k levels", deep.into_bytes(), XmlError::TooDeep),
        (
            "undefined entity",
            b"<f>&nope;</f>".to_vec(),
            XmlError::Invalid(String::new()),
        ),
        (
            "control character",
            b"<f>a\x01b</f>".to_vec(),
            XmlError::Invalid(String::new()),
        ),
        (
            "unclosed CDATA",
            b"<f><![CDATA[never closed</f>".to_vec(),
            XmlError::Invalid(String::new()),
        ),
        (
            "not XML",
            b"%PDF-1.7 not xml".to_vec(),
            XmlError::Invalid(String::new()),
        ),
        (
            "not UTF-8 and no declared encoding",
            b"<f>\xff\xfe\xfd</f>".to_vec(),
            XmlError::Invalid(String::new()),
        ),
        ("empty", Vec::new(), XmlError::Invalid(String::new())),
    ];
    for (label, bytes, expected) in cases {
        let got = to_html(&bytes, "x.xml", Lang::Es).unwrap_err();
        match (&got, &expected) {
            (XmlError::Invalid(_), XmlError::Invalid(_)) => {}
            _ => assert_eq!(got, expected, "{label}"),
        }
    }
}

#[test]
fn nfr_security_xml_too_many_nodes_or_bytes() {
    let many = format!("<r>{}</r>", "<a/>".repeat(MAX_XML_NODES as usize + 1));
    assert_eq!(
        to_html(many.as_bytes(), "x.xml", Lang::Es).unwrap_err(),
        XmlError::TooLarge
    );
    let big = vec![b' '; MAX_XML_BYTES as usize + 1];
    assert_eq!(
        to_html(&big, "x.xml", Lang::Es).unwrap_err(),
        XmlError::TooLarge
    );
    // Exactly at the depth limit is fine.
    let ok = format!(
        "{}{}",
        "<a>".repeat(MAX_XML_DEPTH),
        "</a>".repeat(MAX_XML_DEPTH)
    );
    assert!(to_html(ok.as_bytes(), "x.xml", Lang::Es).is_ok());
}

#[test]
fn nfr_security_xml_markup_in_values_is_escaped() {
    // A receipt whose every text field tries to inject markup.
    let bytes = fixture("factura-2.1.0.xml");
    let text = String::from_utf8(bytes).unwrap();
    let evil =
        "&lt;script&gt;alert(1)&lt;/script&gt;&lt;img src=x onerror=alert(2)&gt;&quot;&apos;";
    let text = text
        .replace("Material de construcción (tarifa 5%)", evil)
        .replace("CONSUMIDOR FINAL", evil)
        .replace("Ferretería La Prueba", evil)
        .replace("<estab>002</estab>", &format!("<estab>{evil}</estab>"));
    let page = to_html(text.as_bytes(), "<img src=y>.xml", Lang::Es).unwrap();
    assert!(!page.contains("<script"), "{page}");
    assert!(!page.contains("<img"), "{page}");
    assert!(
        page.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
        "{page}"
    );
    assert!(
        page.contains("<title>&lt;img src=y&gt;.xml</title>"),
        "{page}"
    );

    // The same in a generic file, attributes included.
    let generic = br#"<r a="&quot;&gt;&lt;script&gt;x&lt;/script&gt;"><b>&lt;iframe src=//h/s&gt;</b><!-- <script>c</script> --></r>"#;
    let page = to_html(generic, "g.xml", Lang::En).unwrap();
    assert!(!page.contains("<script"), "{page}");
    assert!(!page.contains("<iframe"), "{page}");
    assert!(
        page.contains("XML document, root element &lt;r&gt;"),
        "{page}"
    );
}

#[test]
fn nfr_security_xml_nesting_scan() {
    assert!(!nesting_exceeds("<a><b/><c x='>'>t</c></a>", 2));
    assert!(nesting_exceeds("<a><b><c/></b></a>", 1));
    assert!(!nesting_exceeds(
        "<a><!-- <b><b><b> --><![CDATA[<b><b>]]><?p <b>?></a>",
        1
    ));
    assert!(!nesting_exceeds("<r><a q=\"<b><b>\"/></r>", 1));
    let deep = format!("{}{}", "<a>".repeat(100_000), "</a>".repeat(100_000));
    assert!(nesting_exceeds(&deep, MAX_XML_DEPTH));
    // Unclosed tags still count: a parser would descend into them.
    assert!(nesting_exceeds(&"<a>".repeat(300), MAX_XML_DEPTH));
}

#[test]
fn fr_057a_encodings() {
    // UTF-16 with a BOM.
    let mut le = vec![0xFF, 0xFE];
    for u in "<f>año</f>".encode_utf16() {
        le.extend_from_slice(&u.to_le_bytes());
    }
    assert!(to_html(&le, "x.xml", Lang::Es).unwrap().contains("año"));
    // UTF-8 with a BOM.
    let bom = "\u{FEFF}<f>ñ</f>".as_bytes().to_vec();
    assert!(to_html(&bom, "x.xml", Lang::Es).unwrap().contains('ñ'));
    // Declared ISO-8859-1.
    let latin = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><f>a\xf1o</f>";
    assert!(to_html(latin, "x.xml", Lang::Es).unwrap().contains("año"));
    // The declaration governs even when the Latin-1 bytes also happen to be
    // valid UTF-8: 0xC3 0xB1 is "Ã±" in ISO-8859-1, not "ñ".
    let both = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><f>a\xc3\xb1o</f>";
    assert!(std::str::from_utf8(both).is_ok());
    let page = to_html(both, "x.xml", Lang::Es).unwrap();
    assert!(page.contains("aÃ±o"), "{page}");
    assert!(!page.contains("año"));
    // Undeclared or declared UTF-8: UTF-8, strictly.
    let utf8 = "<?xml version=\"1.0\" encoding='UTF-8'?><f>año</f>".as_bytes();
    assert!(to_html(utf8, "x.xml", Lang::Es).unwrap().contains("año"));
    assert!(to_html(b"<f>a\xf1o</f>", "x.xml", Lang::Es).is_err());
    // The prolog walk.
    assert!(declares_dtd(
        "\u{FEFF}<?xml version=\"1.0\"?>\n<!DOCTYPE x><x/>"
    ));
    assert!(!declares_dtd("<x><!DOCTYPE in text is not a prolog/></x>"));
    assert!(!declares_dtd(
        "<?xml version=\"1.0\"?><!-- <!DOCTYPE x> --><x/>"
    ));
}

#[test]
fn fr_057b_page_paper_defaults_to_a4() {
    assert_eq!(
        page_for(&PageSetup::default()).css_size.as_deref(),
        Some(DEFAULT_CSS_SIZE)
    );
    let letter = PageSetup {
        css_size: Some("8.5in 11in".into()),
    };
    assert_eq!(page_for(&letter), letter);
}

#[test]
fn fr_057b_failures_carry_their_stable_reasons() {
    let reason = |e: XmlError| match e.into_engine() {
        EngineError::ConversionFailed(f) => (f.reason, f.params.len()),
        other => panic!("{other:?}"),
    };
    assert_eq!(reason(XmlError::TooLarge), (Some("xml_too_large"), 1));
    assert_eq!(reason(XmlError::TooDeep), (Some("xml_too_deep"), 1));
    assert_eq!(reason(XmlError::Dtd), (Some("xml_dtd_refused"), 0));
    assert_eq!(
        reason(XmlError::Invalid("x".into())),
        (Some("xml_invalid"), 0)
    );
    for r in [
        "xml_too_large",
        "xml_too_deep",
        "xml_dtd_refused",
        "xml_invalid",
    ] {
        assert!(
            crate::util::reasons::is_terminal_conversion_reason(r),
            "{r}"
        );
    }
}

/// No input panics: thousands of deterministic mutations of every fixture
/// (byte flips, truncations, duplicated and deleted runs) either print or
/// fail with a reason. Release builds abort on panic, so this matters.
#[test]
fn nfr_security_xml_mutations_never_panic() {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut rand = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut names: Vec<String> = RECEIPTS.iter().map(|(n, ..)| n.to_string()).collect();
    names.push("catalogo-generico.xml".into());
    for name in names {
        let original = fixture(&name);
        for _ in 0..150 {
            let mut bytes = original.clone();
            for _ in 0..(1 + rand() % 4) {
                if bytes.is_empty() {
                    break;
                }
                let at = (rand() as usize) % bytes.len();
                match rand() % 5 {
                    0 => bytes[at] = (rand() & 0xFF) as u8,
                    1 => bytes.truncate(at),
                    2 => {
                        let end = (at + (rand() as usize % 64)).min(bytes.len());
                        let run = bytes[at..end].to_vec();
                        bytes.splice(at..at, run);
                    }
                    3 => {
                        let end = (at + (rand() as usize % 64)).min(bytes.len());
                        bytes.drain(at..end);
                    }
                    _ => {
                        let pick = [&b"<"[..], b">", b"]]>", b"<![CDATA[", b"&", b"\"", b"\xC3"];
                        let p = pick[(rand() as usize) % pick.len()];
                        bytes.splice(at..at, p.iter().copied());
                    }
                }
            }
            let _ = to_html(&bytes, &name, Lang::Es);
        }
    }
}
