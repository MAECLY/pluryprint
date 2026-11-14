//! Recognise an SRI electronic receipt and read it into a [`Receipt`]
//! (FR-057b).
//!
//! The six receipt types of Ecuador's *Servicio de Rentas Internas* and the
//! versions of their XML schemas the SRI publishes (*Ficha técnica de
//! comprobantes electrónicos*):
//!
//! | Root element | `codDoc` | Versions |
//! |---|---|---|
//! | `factura` | 01 | 1.0.0, 1.1.0, 2.0.0, 2.1.0 |
//! | `liquidacionCompra` | 03 | 1.0.0, 1.1.0 |
//! | `notaCredito` | 04 | 1.0.0, 1.1.0 |
//! | `notaDebito` | 05 | 1.0.0 |
//! | `guiaRemision` | 06 | 1.0.0, 1.1.0 |
//! | `comprobanteRetencion` | 07 | 1.0.0, 2.0.0 |
//!
//! Three shapes of file carry one ([`read`]):
//!
//! - the **`<autorizacion>`** the portal downloads and Pluryfact saves: the
//!   authorisation state, number, date and environment, and the receipt as
//!   a string inside `<comprobante>` (CDATA or escaped text);
//! - the web service's whole **`RespuestaAutorizacionComprobante`**, bare or
//!   in its SOAP envelope, which holds one or more `<autorizacion>` (the
//!   `AUTORIZADO` one is used, else the first);
//! - a **bare receipt** (the signed XML an invoicing system keeps), which has
//!   no authorisation data: the page says so instead of inventing it.
//!
//! The reader is tolerant: fields are looked up by local name (namespace
//! prefixes and the XML-DSig `<ds:Signature>` are ignored), a missing
//! optional field is empty, and an unknown version is read with the fields of
//! the newest one. A file without `<infoTributaria>` is not a receipt and
//! prints as a tree.

use roxmltree::{Document, Node};

/// The receipt types (`codDoc`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `01` Factura.
    Factura,
    /// `03` Liquidación de compra de bienes y prestación de servicios.
    LiquidacionCompra,
    /// `04` Nota de crédito.
    NotaCredito,
    /// `05` Nota de débito.
    NotaDebito,
    /// `06` Guía de remisión.
    GuiaRemision,
    /// `07` Comprobante de retención.
    Retencion,
}

impl Kind {
    /// The receipt type of a root element's local name.
    pub fn from_root(name: &str) -> Option<Self> {
        Some(match name {
            "factura" => Kind::Factura,
            "liquidacionCompra" => Kind::LiquidacionCompra,
            "notaCredito" => Kind::NotaCredito,
            "notaDebito" => Kind::NotaDebito,
            "guiaRemision" => Kind::GuiaRemision,
            "comprobanteRetencion" => Kind::Retencion,
            _ => return None,
        })
    }

    /// The `infoXxx` element of this type.
    fn info_element(self) -> &'static str {
        match self {
            Kind::Factura => "infoFactura",
            Kind::LiquidacionCompra => "infoLiquidacionCompra",
            Kind::NotaCredito => "infoNotaCredito",
            Kind::NotaDebito => "infoNotaDebito",
            Kind::GuiaRemision => "infoGuiaRemision",
            Kind::Retencion => "infoCompRetencion",
        }
    }

    /// The title printed on the RIDE.
    pub fn title(self) -> &'static str {
        match self {
            Kind::Factura => "Factura",
            Kind::LiquidacionCompra => "Liquidación de compra de bienes y prestación de servicios",
            Kind::NotaCredito => "Nota de crédito",
            Kind::NotaDebito => "Nota de débito",
            Kind::GuiaRemision => "Guía de remisión",
            Kind::Retencion => "Comprobante de retención",
        }
    }
}

/// The SRI's verdict, from the `<autorizacion>` around the receipt.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Authorization {
    /// `AUTORIZADO`, `NO AUTORIZADO`, `EN PROCESO`…
    pub estado: String,
    /// The authorisation number (the access key, for offline receipts).
    pub numero: String,
    /// When the SRI authorised it, as the SRI wrote it.
    pub fecha: String,
    /// The environment as the SRI wrote it (`PRODUCCIÓN`).
    pub ambiente: String,
}

impl Authorization {
    /// Whether the SRI authorised the receipt.
    pub fn is_authorized(&self) -> bool {
        self.estado.trim().eq_ignore_ascii_case("AUTORIZADO")
    }
}

/// The issuer (`infoTributaria` plus the establishment fields of the info
/// block).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Issuer {
    /// Razón social.
    pub razon_social: String,
    /// Nombre comercial.
    pub nombre_comercial: String,
    /// RUC.
    pub ruc: String,
    /// Dirección matriz.
    pub dir_matriz: String,
    /// Dirección del establecimiento.
    pub dir_establecimiento: String,
    /// Contribuyente especial (resolution number).
    pub contribuyente_especial: String,
    /// Obligado a llevar contabilidad (`SI` / `NO`).
    pub obligado_contabilidad: String,
    /// `contribuyenteRimpe` (`CONTRIBUYENTE RÉGIMEN RIMPE`…).
    pub regimen: String,
    /// `agenteRetencion` (resolution number).
    pub agente_retencion: String,
}

/// The other party: buyer, supplier or withheld subject.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Party {
    /// Name.
    pub nombre: String,
    /// RUC, cédula, passport…
    pub identificacion: String,
    /// The SRI identification type code (`04` RUC, `05` cédula…).
    pub tipo_identificacion: String,
    /// Address.
    pub direccion: String,
}

/// One line of the detail table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Item {
    /// Main code (`codigoPrincipal` / `codigoInterno`).
    pub codigo: String,
    /// Auxiliary code (`codigoAuxiliar` / `codigoAdicional`).
    pub codigo_aux: String,
    /// Description.
    pub descripcion: String,
    /// Unit of measure (factura 2.1.0).
    pub unidad: String,
    /// Quantity.
    pub cantidad: String,
    /// Unit price.
    pub precio_unitario: String,
    /// Discount.
    pub descuento: String,
    /// Line total without taxes.
    pub total: String,
    /// `detAdicional` name/value pairs.
    pub adicionales: Vec<(String, String)>,
}

/// One tax total (`totalImpuesto` / `impuesto`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tax {
    /// `2` IVA, `3` ICE, `5` IRBPNR.
    pub codigo: String,
    /// The rate code (`codigoPorcentaje`).
    pub codigo_porcentaje: String,
    /// The rate, when the file states it (`tarifa`).
    pub tarifa: String,
    /// Taxable base.
    pub base: String,
    /// Tax amount.
    pub valor: String,
}

/// One payment (`pago`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Payment {
    /// The SRI payment method code (`01`, `19`…).
    pub forma_pago: String,
    /// Amount.
    pub total: String,
    /// Term, with its unit (`30 días`).
    pub plazo: String,
}

/// One withholding (`impuesto` in 1.0.0, `retencion` in 2.0.0).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Withholding {
    /// `1` renta, `2` IVA, `6` ISD.
    pub codigo: String,
    /// The withholding code (`312`, `725`…).
    pub codigo_retencion: String,
    /// Taxable base.
    pub base: String,
    /// Percentage withheld.
    pub porcentaje: String,
    /// Amount withheld.
    pub valor: String,
    /// The supporting document, on the line itself (1.0.0).
    pub doc: Option<SupportDoc>,
}

/// A supporting document (*documento sustento*) of a withholding.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SupportDoc {
    /// Its type code (`01` factura…).
    pub cod_doc: String,
    /// Its number (`001001000000123` or `001-001-000000123`).
    pub numero: String,
    /// Its issue date.
    pub fecha: String,
    /// Its authorisation number (2.0.0).
    pub autorizacion: String,
    /// Its subtotal without taxes (2.0.0).
    pub total_sin_impuestos: String,
    /// Its total (2.0.0).
    pub importe_total: String,
}

/// A supporting document with its withholdings (2.0.0 `docSustento`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SupportGroup {
    /// The document.
    pub doc: SupportDoc,
    /// What was withheld on it.
    pub retenciones: Vec<Withholding>,
}

/// One recipient of a delivery guide (`destinatario`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recipient {
    /// Name.
    pub nombre: String,
    /// Identification.
    pub identificacion: String,
    /// Address.
    pub direccion: String,
    /// Reason for the transfer.
    pub motivo: String,
    /// Route.
    pub ruta: String,
    /// Customs document.
    pub doc_aduanero: String,
    /// Destination establishment code.
    pub estab_destino: String,
    /// The document it supports: type, number, authorisation, date.
    pub doc: Option<SupportDoc>,
    /// The goods moved.
    pub items: Vec<Item>,
}

/// The transport data of a delivery guide.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shipment {
    /// Starting point.
    pub dir_partida: String,
    /// Carrier.
    pub transportista: String,
    /// Carrier identification.
    pub ruc_transportista: String,
    /// Licence plate.
    pub placa: String,
    /// Transport start date.
    pub fecha_inicio: String,
    /// Transport end date.
    pub fecha_fin: String,
    /// Recipients.
    pub destinatarios: Vec<Recipient>,
}

/// An SRI receipt, as printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    /// Its type.
    pub kind: Kind,
    /// The schema version (`version` attribute).
    pub version: String,
    /// The SRI's authorisation, when the file carries it.
    pub authorization: Option<Authorization>,
    /// `ambiente`: `1` pruebas, `2` producción.
    pub ambiente: String,
    /// `tipoEmision`: `1` normal.
    pub tipo_emision: String,
    /// The 49-digit access key.
    pub clave_acceso: String,
    /// `estab-ptoEmi-secuencial`.
    pub numero: String,
    /// Issue date.
    pub fecha_emision: String,
    /// Issuer.
    pub issuer: Issuer,
    /// Buyer, supplier or withheld subject; `None` for a delivery guide.
    pub party: Option<Party>,
    /// Extra header fields, label and value, in print order (fiscal period,
    /// modified document, reason, delivery guide…).
    pub header: Vec<(String, String)>,
    /// Detail lines.
    pub items: Vec<Item>,
    /// Debit note reasons (`razon`, `valor`).
    pub motivos: Vec<(String, String)>,
    /// Tax totals.
    pub taxes: Vec<Tax>,
    /// Subtotal without taxes.
    pub subtotal: String,
    /// Total discount.
    pub descuento: String,
    /// Tip.
    pub propina: String,
    /// Grand total.
    pub total: String,
    /// Payments.
    pub payments: Vec<Payment>,
    /// Withholdings of a 1.0.0 retention (each with its document).
    pub withholdings: Vec<Withholding>,
    /// Supporting documents of a 2.0.0 retention.
    pub support: Vec<SupportGroup>,
    /// Transport data of a delivery guide.
    pub shipment: Option<Shipment>,
    /// `infoAdicional` (`campoAdicional nombre="…"`).
    pub adicional: Vec<(String, String)>,
}

// ---------------------------------------------------------------------------
// Small node helpers (by local name, namespace-agnostic)
// ---------------------------------------------------------------------------

fn child<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn children<'a, 'i: 'a>(node: Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> {
    node.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == name)
}

/// All text inside `node` (CDATA sections included), concatenated.
fn all_text(node: Node<'_, '_>) -> String {
    node.descendants()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect()
}

/// The trimmed text of `node`'s child `name`, or an empty string.
fn text(node: Option<Node<'_, '_>>, name: &str) -> String {
    node.and_then(|n| child(n, name))
        .map(|c| all_text(c).trim().to_string())
        .unwrap_or_default()
}

/// The first of `names` that `node` has as a non-empty child.
fn text_any(node: Option<Node<'_, '_>>, names: &[&str]) -> String {
    names
        .iter()
        .map(|n| text(node, n))
        .find(|t| !t.is_empty())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Recognition
// ---------------------------------------------------------------------------

/// The receipt `doc` carries, or `None` when it is not one (module docs).
pub fn read(doc: &Document<'_>) -> Option<Receipt> {
    let root = doc.root_element();
    if let Some(kind) = Kind::from_root(root.tag_name().name()) {
        return read_receipt(root, kind, None);
    }
    // `<autorizacion>` (alone, or inside a RespuestaAutorizacionComprobante,
    // or inside its SOAP envelope): the AUTORIZADO one, else the first.
    let auths: Vec<Node<'_, '_>> = doc
        .descendants()
        .filter(|n| {
            n.is_element()
                && n.tag_name().name() == "autorizacion"
                && child(*n, "comprobante").is_some()
        })
        .collect();
    let auth = auths
        .iter()
        .find(|a| text(Some(**a), "estado").eq_ignore_ascii_case("AUTORIZADO"))
        .or_else(|| auths.first())
        .copied()?;
    let authorization = Authorization {
        estado: text(Some(auth), "estado"),
        numero: text(Some(auth), "numeroAutorizacion"),
        fecha: text(Some(auth), "fechaAutorizacion"),
        ambiente: text(Some(auth), "ambiente"),
    };
    let inner = all_text(child(auth, "comprobante")?);
    let inner = inner.trim();
    // The receipt is a document of its own, parsed under the same rules; a
    // broken one leaves the outer file to print as a tree.
    let inner_doc = super::parse(inner).ok()?;
    let inner_root = inner_doc.root_element();
    let kind = Kind::from_root(inner_root.tag_name().name())?;
    read_receipt(inner_root, kind, Some(authorization))
}

fn read_receipt(
    root: Node<'_, '_>,
    kind: Kind,
    authorization: Option<Authorization>,
) -> Option<Receipt> {
    let trib = Some(child(root, "infoTributaria")?);
    let info = child(root, kind.info_element());
    let numero = [
        text(trib, "estab"),
        text(trib, "ptoEmi"),
        text(trib, "secuencial"),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join("-");
    let issuer = Issuer {
        razon_social: text(trib, "razonSocial"),
        nombre_comercial: text(trib, "nombreComercial"),
        ruc: text(trib, "ruc"),
        dir_matriz: text(trib, "dirMatriz"),
        dir_establecimiento: text(info, "dirEstablecimiento"),
        contribuyente_especial: text(info, "contribuyenteEspecial"),
        obligado_contabilidad: text(info, "obligadoContabilidad"),
        regimen: text_any(trib, &["contribuyenteRimpe", "regimenMicroempresas"]),
        agente_retencion: text(trib, "agenteRetencion"),
    };
    let mut receipt = Receipt {
        kind,
        version: root.attribute("version").unwrap_or("").trim().to_string(),
        authorization,
        ambiente: text(trib, "ambiente"),
        tipo_emision: text(trib, "tipoEmision"),
        clave_acceso: text(trib, "claveAcceso"),
        numero,
        fecha_emision: text(info, "fechaEmision"),
        issuer,
        party: None,
        header: Vec::new(),
        items: Vec::new(),
        motivos: Vec::new(),
        taxes: Vec::new(),
        subtotal: text(info, "totalSinImpuestos"),
        descuento: text(info, "totalDescuento"),
        propina: text(info, "propina"),
        total: String::new(),
        payments: payments(info),
        withholdings: Vec::new(),
        support: Vec::new(),
        shipment: None,
        adicional: adicional(root),
    };
    let header = |label: &str, value: String, out: &mut Vec<(String, String)>| {
        if !value.is_empty() {
            out.push((label.to_string(), value));
        }
    };
    match kind {
        Kind::Factura => {
            receipt.party = Some(party(info, "Comprador"));
            header(
                "Guía de remisión",
                text(info, "guiaRemision"),
                &mut receipt.header,
            );
            receipt.items = items(root);
            receipt.taxes = taxes(
                info.and_then(|i| child(i, "totalConImpuestos")),
                "totalImpuesto",
            );
            receipt.total = text(info, "importeTotal");
        }
        Kind::LiquidacionCompra => {
            receipt.party = Some(party(info, "Proveedor"));
            receipt.items = items(root);
            receipt.taxes = taxes(
                info.and_then(|i| child(i, "totalConImpuestos")),
                "totalImpuesto",
            );
            receipt.total = text(info, "importeTotal");
        }
        Kind::NotaCredito | Kind::NotaDebito => {
            receipt.party = Some(party(info, "Comprador"));
            header("RISE", text(info, "rise"), &mut receipt.header);
            let modificado = [
                doc_name(&text(info, "codDocModificado")),
                text(info, "numDocModificado"),
            ]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
            header(
                "Comprobante que se modifica",
                modificado,
                &mut receipt.header,
            );
            header(
                "Fecha de emisión del comprobante que se modifica",
                text(info, "fechaEmisionDocSustento"),
                &mut receipt.header,
            );
            if kind == Kind::NotaCredito {
                header(
                    "Razón de modificación",
                    text(info, "motivo"),
                    &mut receipt.header,
                );
                receipt.items = items(root);
                receipt.taxes = taxes(
                    info.and_then(|i| child(i, "totalConImpuestos")),
                    "totalImpuesto",
                );
                receipt.total = text(info, "valorModificacion");
            } else {
                receipt.motivos = root
                    .children()
                    .filter(|c| c.is_element() && c.tag_name().name() == "motivos")
                    .flat_map(|m| children(m, "motivo").collect::<Vec<_>>())
                    .map(|m| (text(Some(m), "razon"), text(Some(m), "valor")))
                    .collect();
                receipt.taxes = taxes(info.and_then(|i| child(i, "impuestos")), "impuesto");
                receipt.total = text(info, "valorTotal");
            }
        }
        Kind::Retencion => {
            receipt.party = Some(Party {
                nombre: text(info, "razonSocialSujetoRetenido"),
                identificacion: text(info, "identificacionSujetoRetenido"),
                tipo_identificacion: text(info, "tipoIdentificacionSujetoRetenido"),
                direccion: String::new(),
            });
            header(
                "Período fiscal",
                text(info, "periodoFiscal"),
                &mut receipt.header,
            );
            let parte_rel = text(info, "parteRel");
            header("Parte relacionada", parte_rel, &mut receipt.header);
            if let Some(list) = child(root, "impuestos") {
                // 1.0.0: one line per tax, each naming its document.
                receipt.withholdings = children(list, "impuesto")
                    .map(|i| {
                        let mut w = withholding(i);
                        w.doc = Some(SupportDoc {
                            cod_doc: text(Some(i), "codDocSustento"),
                            numero: text(Some(i), "numDocSustento"),
                            fecha: text(Some(i), "fechaEmisionDocSustento"),
                            ..SupportDoc::default()
                        });
                        w
                    })
                    .collect();
            }
            if let Some(docs) = child(root, "docsSustento") {
                // 2.0.0: documents, each with its withholdings.
                receipt.support = children(docs, "docSustento")
                    .map(|d| SupportGroup {
                        doc: SupportDoc {
                            cod_doc: text(Some(d), "codDocSustento"),
                            numero: text(Some(d), "numDocSustento"),
                            fecha: text(Some(d), "fechaEmisionDocSustento"),
                            autorizacion: text(Some(d), "numAutDocSustento"),
                            total_sin_impuestos: text(Some(d), "totalSinImpuestos"),
                            importe_total: text(Some(d), "importeTotal"),
                        },
                        retenciones: child(d, "retenciones")
                            .map(|r| children(r, "retencion").map(withholding).collect())
                            .unwrap_or_default(),
                    })
                    .collect();
            }
            receipt.total = total_withheld(&receipt);
        }
        Kind::GuiaRemision => {
            receipt.fecha_emision = String::new();
            receipt.shipment = Some(Shipment {
                dir_partida: text(info, "dirPartida"),
                transportista: text(info, "razonSocialTransportista"),
                ruc_transportista: text(info, "rucTransportista"),
                placa: text(info, "placa"),
                fecha_inicio: text(info, "fechaIniTransporte"),
                fecha_fin: text(info, "fechaFinTransporte"),
                destinatarios: child(root, "destinatarios")
                    .map(|d| children(d, "destinatario").map(recipient).collect())
                    .unwrap_or_default(),
            });
            header("RISE", text(info, "rise"), &mut receipt.header);
        }
    }
    Some(receipt)
}

fn party(info: Option<Node<'_, '_>>, role: &str) -> Party {
    Party {
        nombre: text(info, &format!("razonSocial{role}")),
        identificacion: text(info, &format!("identificacion{role}")),
        tipo_identificacion: text(info, &format!("tipoIdentificacion{role}")),
        direccion: text(info, &format!("direccion{role}")),
    }
}

fn items(root: Node<'_, '_>) -> Vec<Item> {
    child(root, "detalles")
        .map(|d| children(d, "detalle").map(item).collect())
        .unwrap_or_default()
}

fn item(d: Node<'_, '_>) -> Item {
    let d = Some(d);
    Item {
        codigo: text_any(d, &["codigoPrincipal", "codigoInterno"]),
        codigo_aux: text_any(d, &["codigoAuxiliar", "codigoAdicional"]),
        descripcion: text(d, "descripcion"),
        unidad: text(d, "unidadMedida"),
        cantidad: text(d, "cantidad"),
        precio_unitario: text(d, "precioUnitario"),
        descuento: text(d, "descuento"),
        total: text(d, "precioTotalSinImpuesto"),
        adicionales: d
            .and_then(|d| child(d, "detallesAdicionales"))
            .map(|a| {
                children(a, "detAdicional")
                    .map(|x| {
                        (
                            x.attribute("nombre").unwrap_or("").trim().to_string(),
                            x.attribute("valor").unwrap_or("").trim().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn taxes(list: Option<Node<'_, '_>>, element: &str) -> Vec<Tax> {
    list.map(|l| {
        children(l, element)
            .map(|t| {
                let t = Some(t);
                Tax {
                    codigo: text(t, "codigo"),
                    codigo_porcentaje: text(t, "codigoPorcentaje"),
                    tarifa: text(t, "tarifa"),
                    base: text(t, "baseImponible"),
                    valor: text(t, "valor"),
                }
            })
            .collect()
    })
    .unwrap_or_default()
}

fn payments(info: Option<Node<'_, '_>>) -> Vec<Payment> {
    info.and_then(|i| child(i, "pagos"))
        .map(|p| {
            children(p, "pago")
                .map(|p| {
                    let p = Some(p);
                    let plazo = [text(p, "plazo"), text(p, "unidadTiempo")]
                        .into_iter()
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ");
                    Payment {
                        forma_pago: text(p, "formaPago"),
                        total: text(p, "total"),
                        plazo,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn withholding(w: Node<'_, '_>) -> Withholding {
    let w = Some(w);
    Withholding {
        codigo: text(w, "codigo"),
        codigo_retencion: text(w, "codigoRetencion"),
        base: text(w, "baseImponible"),
        porcentaje: text(w, "porcentajeRetener"),
        valor: text(w, "valorRetenido"),
        doc: None,
    }
}

fn recipient(d: Node<'_, '_>) -> Recipient {
    let n = Some(d);
    let cod_doc = text(n, "codDocSustento");
    let numero = text(n, "numDocSustento");
    let doc = (!cod_doc.is_empty() || !numero.is_empty()).then(|| SupportDoc {
        cod_doc,
        numero,
        fecha: text(n, "fechaEmisionDocSustento"),
        autorizacion: text(n, "numAutDocSustento"),
        ..SupportDoc::default()
    });
    Recipient {
        nombre: text(n, "razonSocialDestinatario"),
        identificacion: text(n, "identificacionDestinatario"),
        direccion: text(n, "dirDestinatario"),
        motivo: text(n, "motivoTraslado"),
        ruta: text(n, "ruta"),
        doc_aduanero: text(n, "docAduaneroUnico"),
        estab_destino: text(n, "codEstabDestino"),
        doc,
        items: child(d, "detalles")
            .map(|x| children(x, "detalle").map(item).collect())
            .unwrap_or_default(),
    }
}

fn adicional(root: Node<'_, '_>) -> Vec<(String, String)> {
    child(root, "infoAdicional")
        .map(|a| {
            children(a, "campoAdicional")
                .map(|c| {
                    (
                        c.attribute("nombre").unwrap_or("").trim().to_string(),
                        all_text(c).trim().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The exact sum of every amount withheld, with as many decimals as the
/// most precise of them (never rounded or truncated); empty when one of
/// them is not a number or the sum does not fit. The XML does not carry
/// this total, so the page labels it as calculated. Pure.
fn total_withheld(r: &Receipt) -> String {
    let values: Vec<&str> = r
        .withholdings
        .iter()
        .chain(r.support.iter().flat_map(|g| g.retenciones.iter()))
        .map(|w| w.valor.trim())
        .collect();
    if values.is_empty() {
        return String::new();
    }
    let mut parsed = Vec::with_capacity(values.len());
    for v in &values {
        match parse_decimal(v) {
            Some(d) => parsed.push(d),
            None => return String::new(),
        }
    }
    let scale = parsed.iter().map(|&(_, s)| s).max().unwrap_or(0);
    let mut sum: i128 = 0;
    for (units, s) in parsed {
        let Some(v) = 10i128
            .checked_pow((scale - s) as u32)
            .and_then(|f| units.checked_mul(f))
            .and_then(|v| sum.checked_add(v))
        else {
            return String::new();
        };
        sum = v;
    }
    let sign = if sum < 0 { "-" } else { "" };
    let digits = format!("{:0>width$}", sum.unsigned_abs(), width = scale + 1);
    let (int, frac) = digits.split_at(digits.len() - scale);
    if frac.is_empty() {
        format!("{sign}{int}")
    } else {
        format!("{sign}{int}.{frac}")
    }
}

/// A decimal string as (value in units of its last decimal, number of
/// decimals); `None` for anything else or more than 18 decimals.
fn parse_decimal(s: &str) -> Option<(i128, usize)> {
    let (neg, s) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    if int.is_empty() && frac.is_empty()
        || !int.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
        || int.len() > 20
        || frac.len() > 18
    {
        return None;
    }
    let digits = format!("{int}{frac}");
    let v: i128 = if digits.is_empty() {
        0
    } else {
        digits.parse().ok()?
    };
    Some((if neg { -v } else { v }, frac.len()))
}

// ---------------------------------------------------------------------------
// Catalogues of the SRI's ficha técnica
// ---------------------------------------------------------------------------

/// The name of a document type code (`codDoc`), else `Código NN`.
pub fn doc_name(code: &str) -> String {
    let name = match code {
        "" => return String::new(),
        "01" => "Factura",
        "02" => "Nota de venta",
        "03" => "Liquidación de compra",
        "04" => "Nota de crédito",
        "05" => "Nota de débito",
        "06" => "Guía de remisión",
        "07" => "Comprobante de retención",
        "08" => "Boleto de espectáculos públicos",
        "09" => "Tiquete de máquina registradora",
        "11" => "Pasaje de transporte aéreo",
        "12" => "Documento de institución financiera",
        "18" => "Documento autorizado en ventas excepto N/C N/D",
        "19" => "Comprobante de pago de cuotas o aportes",
        "20" => "Documento del Estado y sector público",
        "21" => "Carta de porte aéreo",
        "41" => "Comprobante de venta emitido por reembolso",
        "294" => "Liquidación de empresas de seguros",
        _ => return format!("Código {code}"),
    };
    name.to_string()
}

/// The name of an identification type code.
pub fn id_type_name(code: &str) -> &'static str {
    match code {
        "04" => "RUC",
        "05" => "Cédula",
        "06" => "Pasaporte",
        "07" => "Consumidor final",
        "08" => "Identificación del exterior",
        _ => "Identificación",
    }
}

/// The name of a payment method code (*Tabla 24*).
pub fn payment_name(code: &str) -> String {
    let name = match code {
        "01" => "Sin utilización del sistema financiero",
        "15" => "Compensación de deudas",
        "16" => "Tarjeta de débito",
        "17" => "Dinero electrónico",
        "18" => "Tarjeta prepago",
        "19" => "Tarjeta de crédito",
        "20" => "Otros con utilización del sistema financiero",
        "21" => "Endoso de títulos",
        "" => return String::new(),
        _ => return format!("Código {code}"),
    };
    name.to_string()
}

/// The environment of an `ambiente` code.
pub fn ambiente_name(code: &str) -> String {
    match code {
        "1" => "Pruebas".to_string(),
        "2" => "Producción".to_string(),
        other => other.to_string(),
    }
}

/// The emission type of a `tipoEmision` code.
pub fn emision_name(code: &str) -> String {
    match code {
        "1" => "Normal".to_string(),
        "2" => "Indisponibilidad del sistema".to_string(),
        other => other.to_string(),
    }
}

/// The name of a withheld tax code.
pub fn withheld_tax_name(code: &str) -> String {
    match code {
        "1" => "Renta".to_string(),
        "2" => "IVA".to_string(),
        "6" => "ISD".to_string(),
        "" => String::new(),
        other => format!("Código {other}"),
    }
}

/// What a tax total is, for the totals table: `(subtotal label, tax label)`.
/// IVA is labelled by its rate (`codigoPorcentaje`, *Tabla 17*); the
/// `tarifa` the file states wins for a rate code this table does not know.
pub fn tax_labels(t: &Tax) -> (String, String) {
    match t.codigo.as_str() {
        "2" => {
            let rate = match t.codigo_porcentaje.as_str() {
                "0" => Some("0%".to_string()),
                "2" => Some("12%".to_string()),
                "3" => Some("14%".to_string()),
                "4" => Some("15%".to_string()),
                "5" => Some("5%".to_string()),
                "8" => Some("diferenciado".to_string()),
                "10" => Some("13%".to_string()),
                _ => None,
            };
            match t.codigo_porcentaje.as_str() {
                "6" => ("Subtotal no objeto de IVA".into(), "IVA no objeto".into()),
                "7" => ("Subtotal exento de IVA".into(), "IVA exento".into()),
                _ => {
                    let rate = rate.unwrap_or_else(|| {
                        let tarifa = super::ride::trim_decimal(&t.tarifa);
                        if tarifa.is_empty() {
                            format!("código {}", t.codigo_porcentaje)
                        } else {
                            format!("{tarifa}%")
                        }
                    });
                    (format!("Subtotal {rate}"), format!("IVA {rate}"))
                }
            }
        }
        "3" => ("Base ICE".into(), "ICE".into()),
        "5" => ("Base IRBPNR".into(), "IRBPNR".into()),
        other => (
            format!("Base impuesto {other}"),
            format!("Impuesto {other}"),
        ),
    }
}

/// The mod-11 check digit the SRI puts at the end of an access key, for
/// its first 48 digits; `None` when they are not 48 digits.
pub fn access_key_check_digit(first48: &str) -> Option<u8> {
    if first48.len() != 48 || !first48.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut factor = 2u32;
    let mut sum = 0u32;
    for b in first48.bytes().rev() {
        sum += u32::from(b - b'0') * factor;
        factor = if factor == 7 { 2 } else { factor + 1 };
    }
    let d = 11 - (sum % 11);
    Some(match d {
        11 => 0,
        10 => 1,
        d => d as u8,
    })
}
