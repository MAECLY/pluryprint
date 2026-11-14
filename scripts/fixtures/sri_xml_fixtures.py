#!/usr/bin/env python3
"""Generate the FICTITIOUS SRI receipt fixtures of src-tauri/tests/fixtures/sri-xml.

Every value is made up: the issuer RUC is 0999999999001, people and companies
are invented, and the access keys are built here with a valid mod-11 check
digit but describe no real receipt. Never replace these with real receipts.

Usage (from the repository root):

    python3 scripts/fixtures/sri_xml_fixtures.py src-tauri/tests/fixtures/sri-xml

Then regenerate the HTML snapshots and review the diff:

    PLURYPRINT_BLESS=1 cargo test --manifest-path src-tauri/Cargo.toml --lib xml
"""
import os, sys
from xml.sax.saxutils import escape

OUT = sys.argv[1]
os.makedirs(OUT, exist_ok=True)
RUC = "0999999999001"

def dv(k48):
    f, s = 2, 0
    for ch in reversed(k48):
        s += int(ch) * f
        f = 2 if f == 7 else f + 1
    d = 11 - s % 11
    return {11: 0, 10: 1}.get(d, d)

def clave(fecha, cod, amb, estab, pto, sec, codigo="12345678"):
    d, m, y = fecha.split("/")
    k = f"{d}{m}{y}{cod}{RUC}{amb}{estab}{pto}{sec}{codigo}1"
    assert len(k) == 48, k
    return k + str(dv(k))

def trib(cod, fecha, sec, amb="2", estab="001", pto="002", extra=""):
    k = clave(fecha, cod, amb, estab, pto, sec)
    return k, f"""<infoTributaria><ambiente>{amb}</ambiente><tipoEmision>1</tipoEmision><razonSocial>COMERCIAL ANDINA DE PRUEBAS S.A. &amp; ASOCIADOS</razonSocial><nombreComercial>Ferretería La Prueba</nombreComercial><ruc>{RUC}</ruc><claveAcceso>{k}</claveAcceso><codDoc>{cod}</codDoc><estab>{estab}</estab><ptoEmi>{pto}</ptoEmi><secuencial>{sec}</secuencial><dirMatriz>Av. Ficticia 123 y Calle Imaginaria, Guayaquil</dirMatriz>{extra}</infoTributaria>"""

def wrap(inner, k, estado="AUTORIZADO", fecha=None, amb="PRODUCCIÓN", style="cdata"):
    if fecha is None:
        fecha = f"{k[4:8]}-{k[2:4]}-{k[0:2]}T10:24:31-05:00"
    if style == "cdata":
        comp = "<![CDATA[" + inner.replace("]]>", "]]]]><![CDATA[>") + "]]>"
    else:
        comp = escape(inner)
    return f"""<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<autorizacion>
<estado>{estado}</estado>
<numeroAutorizacion>{k}</numeroAutorizacion>
<fechaAutorizacion>{fecha}</fechaAutorizacion>
<ambiente>{amb}</ambiente>
<comprobante>{comp}</comprobante>
</autorizacion>
"""

def write(name, text):
    with open(os.path.join(OUT, name), "w", encoding="utf-8") as f:
        f.write(text)

ADIC = """<infoAdicional><campoAdicional nombre="Email">compras@cliente-ficticio.ec</campoAdicional><campoAdicional nombre="Teléfono">04-000-0000</campoAdicional><campoAdicional nombre="Observación">Datos de prueba: comprobante ficticio para Pluryprint</campoAdicional></infoAdicional>"""

def detalle(cod, aux, desc, cant, pu, desc_v, tot, iva_code, tarifa, base, valor, adic="", unidad=None, interno=False):
    c1, c2 = ("codigoInterno", "codigoAdicional") if interno else ("codigoPrincipal", "codigoAuxiliar")
    aux_xml = f"<{c2}>{aux}</{c2}>" if aux else ""
    um = f"<unidadMedida>{unidad}</unidadMedida>" if unidad else ""
    ad = f"<detallesAdicionales>{adic}</detallesAdicionales>" if adic else ""
    return f"""<detalle><{c1}>{cod}</{c1}>{aux_xml}<descripcion>{desc}</descripcion>{um}<cantidad>{cant}</cantidad><precioUnitario>{pu}</precioUnitario><descuento>{desc_v}</descuento><precioTotalSinImpuesto>{tot}</precioTotalSinImpuesto>{ad}<impuestos><impuesto><codigo>2</codigo><codigoPorcentaje>{iva_code}</codigoPorcentaje><tarifa>{tarifa}</tarifa><baseImponible>{base}</baseImponible><valor>{valor}</valor></impuesto></impuestos></detalle>"""

# 1. Factura 1.1.0 in <autorizacion> (CDATA) — the portal / Pluryfact shape.
k, t = trib("01", "15/10/2026", "000000123", extra="<contribuyenteRimpe>CONTRIBUYENTE RÉGIMEN RIMPE</contribuyenteRimpe>")
fac110 = f"""<?xml version="1.0" encoding="UTF-8"?><factura id="comprobante" version="1.1.0">{t}<infoFactura><fechaEmision>15/10/2026</fechaEmision><dirEstablecimiento>Sucursal Norte, Av. de Prueba 456, Guayaquil</dirEstablecimiento><contribuyenteEspecial>12345</contribuyenteEspecial><obligadoContabilidad>SI</obligadoContabilidad><tipoIdentificacionComprador>04</tipoIdentificacionComprador><guiaRemision>001-002-000000045</guiaRemision><razonSocialComprador>CLIENTE FICTICIO CÍA. LTDA.</razonSocialComprador><identificacionComprador>0999999999001</identificacionComprador><direccionComprador>Calle Inventada 789, Quito</direccionComprador><totalSinImpuestos>183.50</totalSinImpuestos><totalDescuento>6.50</totalDescuento><totalConImpuestos><totalImpuesto><codigo>2</codigo><codigoPorcentaje>4</codigoPorcentaje><baseImponible>162.50</baseImponible><tarifa>15</tarifa><valor>24.38</valor></totalImpuesto><totalImpuesto><codigo>2</codigo><codigoPorcentaje>0</codigoPorcentaje><baseImponible>21.00</baseImponible><tarifa>0</tarifa><valor>0.00</valor></totalImpuesto></totalConImpuestos><propina>0.00</propina><importeTotal>207.88</importeTotal><moneda>DOLAR</moneda><pagos><pago><formaPago>20</formaPago><total>150.00</total><plazo>30</plazo><unidadTiempo>días</unidadTiempo></pago><pago><formaPago>01</formaPago><total>57.88</total></pago></pagos></infoFactura><detalles>{detalle("TAL-001","7861234500011","Taladro percutor 600 W","1.000000","95.000000","5.00","90.00","4","15.00","90.00","13.50",adic='<detAdicional nombre="Garantía" valor="12 meses"/>')}{detalle("BRO-010","","Juego de brocas para concreto, 10 piezas","2.000000","36.250000","1.50","71.00","4","15.00","71.00","10.65")}{detalle("CIN-003","","Cinta métrica 5 m","1.500000","1.000000","0.00","1.50","4","15.00","1.50","0.23")}{detalle("LIB-200","","Manual de bricolaje (libro, IVA 0%)","1.000000","21.000000","0.00","21.00","0","0","21.00","0.00")}</detalles>{ADIC}</factura>"""
write("factura-1.1.0-autorizacion.xml", wrap(fac110, k))

# 2. Factura 2.1.0 bare (no authorisation), IVA 5% + ICE, unidadMedida.
k, t = trib("01", "02/11/2026", "000000777", estab="002", pto="001", extra="<agenteRetencion>1</agenteRetencion>")
fac210 = f"""<?xml version="1.0" encoding="UTF-8"?>
<factura id="comprobante" version="2.1.0">{t}<infoFactura><fechaEmision>02/11/2026</fechaEmision><dirEstablecimiento>Local 2, Centro Comercial de Prueba</dirEstablecimiento><obligadoContabilidad>NO</obligadoContabilidad><tipoIdentificacionComprador>07</tipoIdentificacionComprador><razonSocialComprador>CONSUMIDOR FINAL</razonSocialComprador><identificacionComprador>9999999999999</identificacionComprador><totalSinImpuestos>40.00</totalSinImpuestos><totalDescuento>0.00</totalDescuento><totalConImpuestos><totalImpuesto><codigo>2</codigo><codigoPorcentaje>5</codigoPorcentaje><baseImponible>30.00</baseImponible><tarifa>5</tarifa><valor>1.50</valor></totalImpuesto><totalImpuesto><codigo>3</codigo><codigoPorcentaje>3011</codigoPorcentaje><baseImponible>10.00</baseImponible><valor>1.50</valor></totalImpuesto><totalImpuesto><codigo>2</codigo><codigoPorcentaje>4</codigoPorcentaje><baseImponible>11.50</baseImponible><tarifa>15</tarifa><valor>1.73</valor></totalImpuesto></totalConImpuestos><propina>2.00</propina><importeTotal>46.73</importeTotal><moneda>DOLAR</moneda><pagos><pago><formaPago>19</formaPago><total>46.73</total></pago></pagos></infoFactura><detalles>{detalle("MAT-5","","Material de construcción (tarifa 5%)","3.000000","10.000000","0.00","30.00","5","5","30.00","1.50",unidad="saco")}<detalle><codigoPrincipal>BEB-ICE</codigoPrincipal><descripcion>Bebida gravada con ICE</descripcion><unidadMedida>unidad</unidadMedida><cantidad>4.000000</cantidad><precioUnitario>2.500000</precioUnitario><descuento>0.00</descuento><precioTotalSinImpuesto>10.00</precioTotalSinImpuesto><impuestos><impuesto><codigo>3</codigo><codigoPorcentaje>3011</codigoPorcentaje><tarifa>15</tarifa><baseImponible>10.00</baseImponible><valor>1.50</valor></impuesto><impuesto><codigo>2</codigo><codigoPorcentaje>4</codigoPorcentaje><tarifa>15</tarifa><baseImponible>11.50</baseImponible><valor>1.73</valor></impuesto></impuestos></detalle></detalles></factura>
"""
write("factura-2.1.0.xml", fac210)

# 3. Factura 1.0.0 inside the SOAP RespuestaAutorizacionComprobante, escaped
#    text, two authorisations (the AUTORIZADO one wins).
k, t = trib("01", "20/09/2026", "000000010")
fac100 = f"""<?xml version="1.0" encoding="UTF-8"?><factura id="comprobante" version="1.0.0">{t}<infoFactura><fechaEmision>20/09/2026</fechaEmision><dirEstablecimiento>Matriz</dirEstablecimiento><obligadoContabilidad>SI</obligadoContabilidad><tipoIdentificacionComprador>05</tipoIdentificacionComprador><razonSocialComprador>PERSONA DE PRUEBA</razonSocialComprador><identificacionComprador>0999999999</identificacionComprador><totalSinImpuestos>10.00</totalSinImpuestos><totalDescuento>0.00</totalDescuento><totalConImpuestos><totalImpuesto><codigo>2</codigo><codigoPorcentaje>2</codigoPorcentaje><baseImponible>10.00</baseImponible><valor>1.20</valor></totalImpuesto></totalConImpuestos><propina>0.00</propina><importeTotal>11.20</importeTotal><moneda>DOLAR</moneda></infoFactura><detalles>{detalle("SRV-1","","Servicio de prueba (IVA 12%, 2023)","1.000000","10.000000","0.00","10.00","2","12.00","10.00","1.20")}</detalles></factura>"""
soap = f"""<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"><soap:Body><ns2:autorizacionComprobanteResponse xmlns:ns2="http://ec.gob.sri.ws.autorizacion"><RespuestaAutorizacionComprobante><claveAccesoConsultada>{k}</claveAccesoConsultada><numeroComprobantes>2</numeroComprobantes><autorizaciones><autorizacion><estado>NO AUTORIZADO</estado><fechaAutorizacion>2026-09-20T09:00:00-05:00</fechaAutorizacion><ambiente>PRODUCCI&#211;N</ambiente><comprobante>{escape(fac100)}</comprobante><mensajes><mensaje><identificador>45</identificador><mensaje>ERROR SECUENCIAL REGISTRADO</mensaje></mensaje></mensajes></autorizacion><autorizacion><estado>AUTORIZADO</estado><numeroAutorizacion>{k}</numeroAutorizacion><fechaAutorizacion>2026-09-20T09:05:12.481-05:00</fechaAutorizacion><ambiente>PRODUCCI&#211;N</ambiente><comprobante>{escape(fac100)}</comprobante><mensajes/></autorizacion></autorizaciones></RespuestaAutorizacionComprobante></ns2:autorizacionComprobanteResponse></soap:Body></soap:Envelope>"""
write("factura-1.0.0-respuesta-soap.xml", soap)

# 4. Factura 2.0.0, NO AUTORIZADO.
k, t = trib("01", "03/11/2026", "000000900", amb="1")
fac200 = f"""<factura id="comprobante" version="2.0.0">{t}<infoFactura><fechaEmision>03/11/2026</fechaEmision><obligadoContabilidad>NO</obligadoContabilidad><tipoIdentificacionComprador>06</tipoIdentificacionComprador><razonSocialComprador>VISITANTE DE PRUEBA</razonSocialComprador><identificacionComprador>X0000000</identificacionComprador><totalSinImpuestos>5.00</totalSinImpuestos><totalDescuento>0.00</totalDescuento><totalConImpuestos><totalImpuesto><codigo>2</codigo><codigoPorcentaje>6</codigoPorcentaje><baseImponible>5.00</baseImponible><valor>0.00</valor></totalImpuesto></totalConImpuestos><propina>0.00</propina><importeTotal>5.00</importeTotal><moneda>DOLAR</moneda><pagos><pago><formaPago>01</formaPago><total>5.00</total></pago></pagos></infoFactura><detalles>{detalle("NOB-1","","Servicio no objeto de IVA","1.000000","5.000000","0.00","5.00","6","0","5.00","0.00")}</detalles></factura>"""
write("factura-2.0.0-no-autorizado.xml", wrap(fac200, k, estado="NO AUTORIZADO", amb="PRUEBAS"))

# 5. Nota de crédito 1.1.0 (wrapper).
k, t = trib("04", "18/10/2026", "000000031")
nc110 = f"""<?xml version="1.0" encoding="UTF-8"?><notaCredito id="comprobante" version="1.1.0">{t}<infoNotaCredito><fechaEmision>18/10/2026</fechaEmision><dirEstablecimiento>Sucursal Norte, Av. de Prueba 456, Guayaquil</dirEstablecimiento><tipoIdentificacionComprador>04</tipoIdentificacionComprador><razonSocialComprador>CLIENTE FICTICIO CÍA. LTDA.</razonSocialComprador><identificacionComprador>0999999999001</identificacionComprador><contribuyenteEspecial>12345</contribuyenteEspecial><obligadoContabilidad>SI</obligadoContabilidad><codDocModificado>01</codDocModificado><numDocModificado>001-002-000000123</numDocModificado><fechaEmisionDocSustento>15/10/2026</fechaEmisionDocSustento><totalSinImpuestos>36.25</totalSinImpuestos><valorModificacion>41.69</valorModificacion><moneda>DOLAR</moneda><totalConImpuestos><totalImpuesto><codigo>2</codigo><codigoPorcentaje>4</codigoPorcentaje><baseImponible>36.25</baseImponible><valor>5.44</valor></totalImpuesto></totalConImpuestos><motivo>Devolución de un juego de brocas</motivo></infoNotaCredito><detalles>{detalle("BRO-010","","Juego de brocas para concreto, 10 piezas","1.000000","36.250000","0.00","36.25","4","15.00","36.25","5.44",interno=True)}</detalles>{ADIC}</notaCredito>"""
write("nota-credito-1.1.0.xml", wrap(nc110, k))

# 6. Nota de crédito 1.0.0 bare.
k, t = trib("04", "05/08/2026", "000000002")
nc100 = f"""<?xml version="1.0" encoding="UTF-8"?>
<notaCredito id="comprobante" version="1.0.0">{t}<infoNotaCredito><fechaEmision>05/08/2026</fechaEmision><tipoIdentificacionComprador>05</tipoIdentificacionComprador><razonSocialComprador>PERSONA DE PRUEBA</razonSocialComprador><identificacionComprador>0999999999</identificacionComprador><obligadoContabilidad>SI</obligadoContabilidad><codDocModificado>01</codDocModificado><numDocModificado>001-002-000000099</numDocModificado><fechaEmisionDocSustento>01/08/2026</fechaEmisionDocSustento><totalSinImpuestos>10.00</totalSinImpuestos><valorModificacion>11.50</valorModificacion><moneda>DOLAR</moneda><totalConImpuestos><totalImpuesto><codigo>2</codigo><codigoPorcentaje>4</codigoPorcentaje><baseImponible>10.00</baseImponible><valor>1.50</valor></totalImpuesto></totalConImpuestos><motivo>Descuento posterior a la venta</motivo></infoNotaCredito><detalles>{detalle("DSC","","Descuento comercial","1.000000","10.000000","0.00","10.00","4","15.00","10.00","1.50",interno=True)}</detalles></notaCredito>
"""
write("nota-credito-1.0.0.xml", nc100)

# 7. Nota de débito 1.0.0 (wrapper).
k, t = trib("05", "25/10/2026", "000000004")
nd = f"""<?xml version="1.0" encoding="UTF-8"?><notaDebito id="comprobante" version="1.0.0">{t}<infoNotaDebito><fechaEmision>25/10/2026</fechaEmision><dirEstablecimiento>Matriz</dirEstablecimiento><tipoIdentificacionComprador>04</tipoIdentificacionComprador><razonSocialComprador>CLIENTE FICTICIO CÍA. LTDA.</razonSocialComprador><identificacionComprador>0999999999001</identificacionComprador><obligadoContabilidad>SI</obligadoContabilidad><codDocModificado>01</codDocModificado><numDocModificado>001-002-000000123</numDocModificado><fechaEmisionDocSustento>15/10/2026</fechaEmisionDocSustento><totalSinImpuestos>12.00</totalSinImpuestos><impuestos><impuesto><codigo>2</codigo><codigoPorcentaje>4</codigoPorcentaje><tarifa>15</tarifa><baseImponible>12.00</baseImponible><valor>1.80</valor></impuesto></impuestos><valorTotal>13.80</valorTotal><pagos><pago><formaPago>20</formaPago><total>13.80</total><plazo>15</plazo><unidadTiempo>días</unidadTiempo></pago></pagos></infoNotaDebito><motivos><motivo><razon>Interés por mora (30 días)</razon><valor>10.00</valor></motivo><motivo><razon>Gastos de cobranza</razon><valor>2.00</valor></motivo></motivos>{ADIC}</notaDebito>"""
write("nota-debito-1.0.0.xml", wrap(nd, k))

# 8. Retención 1.0.0 (wrapper).
k, t = trib("07", "16/10/2026", "000000056")
r100 = f"""<?xml version="1.0" encoding="UTF-8"?><comprobanteRetencion id="comprobante" version="1.0.0">{t}<infoCompRetencion><fechaEmision>16/10/2026</fechaEmision><dirEstablecimiento>Matriz</dirEstablecimiento><contribuyenteEspecial>12345</contribuyenteEspecial><obligadoContabilidad>SI</obligadoContabilidad><tipoIdentificacionSujetoRetenido>04</tipoIdentificacionSujetoRetenido><razonSocialSujetoRetenido>PROVEEDOR FICTICIO S.A.</razonSocialSujetoRetenido><identificacionSujetoRetenido>0999999999001</identificacionSujetoRetenido><periodoFiscal>10/2026</periodoFiscal></infoCompRetencion><impuestos><impuesto><codigo>1</codigo><codigoRetencion>312</codigoRetencion><baseImponible>1000.00</baseImponible><porcentajeRetener>1.75</porcentajeRetener><valorRetenido>17.50</valorRetenido><codDocSustento>01</codDocSustento><numDocSustento>001001000004567</numDocSustento><fechaEmisionDocSustento>14/10/2026</fechaEmisionDocSustento></impuesto><impuesto><codigo>2</codigo><codigoRetencion>2</codigoRetencion><baseImponible>150.00</baseImponible><porcentajeRetener>30</porcentajeRetener><valorRetenido>45.00</valorRetenido><codDocSustento>01</codDocSustento><numDocSustento>001001000004567</numDocSustento><fechaEmisionDocSustento>14/10/2026</fechaEmisionDocSustento></impuesto></impuestos>{ADIC}</comprobanteRetencion>"""
write("retencion-1.0.0.xml", wrap(r100, k))

# 9. Retención 2.0.0 (wrapper), two supporting documents.
k, t = trib("07", "06/11/2026", "000000071")
def docsus(num, fecha, aut, total_sin, iva, total, rets):
    rr = "".join(f"<retencion><codigo>{c}</codigo><codigoRetencion>{cr}</codigoRetencion><baseImponible>{b}</baseImponible><porcentajeRetener>{p}</porcentajeRetener><valorRetenido>{v}</valorRetenido></retencion>" for c, cr, b, p, v in rets)
    return f"<docSustento><codSustento>01</codSustento><codDocSustento>01</codDocSustento><numDocSustento>{num}</numDocSustento><fechaEmisionDocSustento>{fecha}</fechaEmisionDocSustento><fechaRegistroContable>{fecha}</fechaRegistroContable><numAutDocSustento>{aut}</numAutDocSustento><pagoLocExt>01</pagoLocExt><totalSinImpuestos>{total_sin}</totalSinImpuestos><importeTotal>{total}</importeTotal><impuestosDocSustento><impuestoDocSustento><codImpuestoDocSustento>2</codImpuestoDocSustento><codigoPorcentaje>4</codigoPorcentaje><baseImponible>{total_sin}</baseImponible><tarifa>15</tarifa><valorImpuesto>{iva}</valorImpuesto></impuestoDocSustento></impuestosDocSustento><retenciones>{rr}</retenciones><pagos><pago><formaPago>20</formaPago><total>{total}</total></pago></pagos></docSustento>"
a1 = clave("01/11/2026", "01", "2", "001", "001", "000008001")
a2 = clave("03/11/2026", "01", "2", "001", "001", "000008002")
r200 = f"""<?xml version="1.0" encoding="UTF-8"?><comprobanteRetencion id="comprobante" version="2.0.0">{t}<infoCompRetencion><fechaEmision>06/11/2026</fechaEmision><dirEstablecimiento>Matriz</dirEstablecimiento><contribuyenteEspecial>12345</contribuyenteEspecial><obligadoContabilidad>SI</obligadoContabilidad><tipoIdentificacionSujetoRetenido>04</tipoIdentificacionSujetoRetenido><parteRel>NO</parteRel><razonSocialSujetoRetenido>PROVEEDOR FICTICIO S.A.</razonSocialSujetoRetenido><identificacionSujetoRetenido>0999999999001</identificacionSujetoRetenido><periodoFiscal>11/2026</periodoFiscal></infoCompRetencion><docsSustento>{docsus("001001000008001","01/11/2026",a1,"800.00","120.00","920.00",[("1","312","800.00","1.75","14.00"),("2","9","120.00","10","12.00")])}{docsus("001001000008002","03/11/2026",a2,"250.00","37.50","287.50",[("1","3440","250.00","2.75","6.88"),("2","10","37.50","20","7.50")])}</docsSustento>{ADIC}</comprobanteRetencion>"""
write("retencion-2.0.0.xml", wrap(r200, k))

# 10. Guía de remisión 1.1.0 (wrapper).
k, t = trib("06", "21/10/2026", "000000045")
def dest(nombre, ident, dirr, motivo, ruta, num, items):
    its = "".join(f"<detalle><codigoInterno>{c}</codigoInterno><descripcion>{d}</descripcion><cantidad>{q}</cantidad></detalle>" for c, d, q in items)
    return f"<destinatario><identificacionDestinatario>{ident}</identificacionDestinatario><razonSocialDestinatario>{nombre}</razonSocialDestinatario><dirDestinatario>{dirr}</dirDestinatario><motivoTraslado>{motivo}</motivoTraslado><ruta>{ruta}</ruta><codDocSustento>01</codDocSustento><numDocSustento>{num}</numDocSustento><numAutDocSustento>{clave('15/10/2026','01','2','001','002','000000123')}</numAutDocSustento><fechaEmisionDocSustento>15/10/2026</fechaEmisionDocSustento><detalles>{its}</detalles></destinatario>"
g110 = f"""<?xml version="1.0" encoding="UTF-8"?><guiaRemision id="comprobante" version="1.1.0">{t}<infoGuiaRemision><dirEstablecimiento>Bodega Sur, Km 10 vía Ficticia</dirEstablecimiento><dirPartida>Bodega Sur, Km 10 vía Ficticia, Guayaquil</dirPartida><razonSocialTransportista>TRANSPORTES DE PRUEBA S.A.</razonSocialTransportista><tipoIdentificacionTransportista>04</tipoIdentificacionTransportista><rucTransportista>0999999999001</rucTransportista><obligadoContabilidad>SI</obligadoContabilidad><fechaIniTransporte>21/10/2026</fechaIniTransporte><fechaFinTransporte>22/10/2026</fechaFinTransporte><placa>GAA-0000</placa></infoGuiaRemision><destinatarios>{dest("CLIENTE FICTICIO CÍA. LTDA.","0999999999001","Calle Inventada 789, Quito","Venta","Guayaquil - Quito","001-002-000000123",[("TAL-001","Taladro percutor 600 W","1"),("BRO-010","Juego de brocas para concreto, 10 piezas","2")])}{dest("PERSONA DE PRUEBA","0999999999","Av. Imaginaria 12, Cuenca","Venta","Guayaquil - Cuenca","001-002-000000124",[("CIN-003","Cinta métrica 5 m","6")])}</destinatarios>{ADIC}</guiaRemision>"""
write("guia-remision-1.1.0.xml", wrap(g110, k))

# 11. Guía de remisión 1.0.0 bare.
k, t = trib("06", "01/07/2026", "000000001")
g100 = f"""<?xml version="1.0" encoding="UTF-8"?>
<guiaRemision id="comprobante" version="1.0.0">{t}<infoGuiaRemision><dirPartida>Matriz</dirPartida><razonSocialTransportista>TRANSPORTISTA DE PRUEBA</razonSocialTransportista><tipoIdentificacionTransportista>05</tipoIdentificacionTransportista><rucTransportista>0999999999</rucTransportista><fechaIniTransporte>01/07/2026</fechaIniTransporte><fechaFinTransporte>01/07/2026</fechaFinTransporte><placa>GBB-1111</placa></infoGuiaRemision><destinatarios><destinatario><identificacionDestinatario>0999999999001</identificacionDestinatario><razonSocialDestinatario>SUCURSAL PROPIA DE PRUEBA</razonSocialDestinatario><dirDestinatario>Sucursal Norte</dirDestinatario><motivoTraslado>Traslado entre establecimientos</motivoTraslado><codEstabDestino>002</codEstabDestino><detalles><detalle><codigoInterno>TAL-001</codigoInterno><descripcion>Taladro percutor 600 W</descripcion><cantidad>10</cantidad></detalle></detalles></destinatario></destinatarios></guiaRemision>
"""
write("guia-remision-1.0.0.xml", g100)

# 12. Liquidación de compra 1.1.0 (wrapper).
k, t = trib("03", "10/10/2026", "000000015")
liq110 = f"""<?xml version="1.0" encoding="UTF-8"?><liquidacionCompra id="comprobante" version="1.1.0">{t}<infoLiquidacionCompra><fechaEmision>10/10/2026</fechaEmision><dirEstablecimiento>Matriz</dirEstablecimiento><contribuyenteEspecial>12345</contribuyenteEspecial><obligadoContabilidad>SI</obligadoContabilidad><tipoIdentificacionProveedor>05</tipoIdentificacionProveedor><razonSocialProveedor>ARTESANO DE PRUEBA</razonSocialProveedor><identificacionProveedor>0999999999</identificacionProveedor><direccionProveedor>Recinto Ficticio, Daule</direccionProveedor><totalSinImpuestos>300.00</totalSinImpuestos><totalDescuento>0.00</totalDescuento><totalConImpuestos><totalImpuesto><codigo>2</codigo><codigoPorcentaje>0</codigoPorcentaje><baseImponible>300.00</baseImponible><tarifa>0</tarifa><valor>0.00</valor></totalImpuesto></totalConImpuestos><importeTotal>300.00</importeTotal><moneda>DOLAR</moneda><pagos><pago><formaPago>01</formaPago><total>300.00</total></pago></pagos></infoLiquidacionCompra><detalles>{detalle("ART-01","","Canastas de paja toquilla","20.000000","15.000000","0.00","300.00","0","0","300.00","0.00",unidad="unidad")}</detalles>{ADIC}</liquidacionCompra>"""
write("liquidacion-compra-1.1.0.xml", wrap(liq110, k))

# 13. Liquidación de compra 1.0.0 bare.
k, t = trib("03", "12/03/2026", "000000003")
liq100 = f"""<?xml version="1.0" encoding="UTF-8"?>
<liquidacionCompra id="comprobante" version="1.0.0">{t}<infoLiquidacionCompra><fechaEmision>12/03/2026</fechaEmision><obligadoContabilidad>SI</obligadoContabilidad><tipoIdentificacionProveedor>05</tipoIdentificacionProveedor><razonSocialProveedor>AGRICULTOR DE PRUEBA</razonSocialProveedor><identificacionProveedor>0999999999</identificacionProveedor><totalSinImpuestos>50.00</totalSinImpuestos><totalDescuento>0.00</totalDescuento><totalConImpuestos><totalImpuesto><codigo>2</codigo><codigoPorcentaje>0</codigoPorcentaje><baseImponible>50.00</baseImponible><valor>0.00</valor></totalImpuesto></totalConImpuestos><importeTotal>50.00</importeTotal><moneda>DOLAR</moneda></infoLiquidacionCompra><detalles>{detalle("AGR-1","","Quintal de cacao","1.000000","50.000000","0.00","50.00","0","0","50.00","0.00")}</detalles></liquidacionCompra>
"""
write("liquidacion-compra-1.0.0.xml", liq100)

# 14. A generic, non-SRI XML.
write("catalogo-generico.xml", """<?xml version="1.0" encoding="UTF-8"?>
<?xml-stylesheet type="text/xsl" href="catalogo.xsl"?>
<!-- Catálogo ficticio para probar la vista genérica de Pluryprint -->
<cat:catalogo xmlns:cat="urn:ejemplo:catalogo" xmlns:xlink="http://www.w3.org/1999/xlink" version="3" actualizado="2026-11-14">
  <cat:producto id="p-1" disponible="true">
    <cat:nombre>Taladro percutor</cat:nombre>
    <cat:precio moneda="USD">95.00</cat:precio>
    <cat:enlace xlink:href="https://example.invalid/taladro"/>
  </cat:producto>
  <cat:producto id="p-2" disponible="false">
    <cat:nombre>Juego de brocas &amp; accesorios</cat:nombre>
    <cat:descripcion>Un texto largo que no cabe en una sola línea junto a su elemento, así que se imprime debajo, envuelto y con la sangría de su nivel en el árbol.</cat:descripcion>
    <cat:notas><![CDATA[<b>no es HTML</b> & se imprime tal cual]]></cat:notas>
  </cat:producto>
</cat:catalogo>
""")
print("ok")
