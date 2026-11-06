//! Simulador local del servicio `VerifactuSOAP` de la AEAT para pruebas.
//!
//! Escucha solo en `127.0.0.1`, con datos en memoria. Reproduce lo que documenta la AEAT y
//! nada más: `Fault` para errores de esquema o cabecera (4102, 4103, 4104, 4114, 4116),
//! rechazo de altas duplicadas (3000) con `RegistroDuplicado`, anulación de un registro
//! inexistente (3002) o ya anulado (3001), huella que no coincide con la recalculada
//! (`AceptadoConErrores`, 2000) y `TiempoEsperaEnvio` configurable. No comprueba firmas,
//! certificados ni censo real. Los envíos anticipados (antes del tiempo de espera y con menos
//! de 1000 registros) se cuentan pero no se rechazan, porque la AEAT no publica qué hace con
//! ellos.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use quick_xml::escape::escape;

use crate::dom::{self, Nodo};
use crate::huella::{self, CamposAlta, CamposAnulacion};
use crate::xml::{NS_RESPUESTA_SUMINISTRO, NS_SOAP, NS_SUMINISTRO_INFORMACION};
use crate::{FechaHoraHuso, MAX_REGISTROS_POR_ENVIO};

/// Error forzado para un número de serie concreto.
#[derive(Debug, Clone)]
pub struct ErrorForzado {
    pub codigo: u32,
    pub descripcion: String,
    /// `true`: `AceptadoConErrores`; `false`: `Incorrecto`.
    pub aceptado: bool,
}

#[derive(Debug, Clone)]
pub struct ConfigSimulador {
    pub tiempo_espera_envio: u32,
    /// NIF del censo simulado. `None` admite cualquier NIF bien formado.
    pub censo: Option<Vec<String>>,
    /// Errores forzados por `NumSerieFactura`.
    pub errores: HashMap<String, ErrorForzado>,
}

impl Default for ConfigSimulador {
    fn default() -> Self {
        Self {
            tiempo_espera_envio: crate::TIEMPO_ESPERA_INICIAL_S,
            censo: None,
            errores: HashMap::new(),
        }
    }
}

/// Estado de un registro guardado en el simulador.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Almacenado {
    pub id_peticion: String,
    /// `Correcta`, `AceptadaConErrores` o `Anulada`.
    pub estado: String,
    pub huella: String,
}

#[derive(Debug, Default)]
pub struct Estado {
    pub config: ConfigSimulador,
    /// Clave: (NIF emisor, serie y número, fecha de expedición).
    pub registros: HashMap<(String, String, String), Almacenado>,
    /// Peticiones recibidas (cuerpos completos), incluidas las rechazadas.
    pub peticiones: Vec<String>,
    pub envios_procesados: u32,
    pub envios_anticipados: u32,
    /// La siguiente petición se procesa pero se cierra la conexión sin responder.
    pub perder_siguiente_respuesta: bool,
    /// La siguiente petición recibe este estado HTTP sin procesarse.
    pub fallo_http_siguiente: Option<u16>,
    ultimo_envio: Option<Instant>,
    secuencia: u32,
}

pub struct SimuladorAeat {
    direccion: SocketAddr,
    estado: Arc<Mutex<Estado>>,
    parar: Arc<AtomicBool>,
    hilo: Option<JoinHandle<()>>,
}

impl SimuladorAeat {
    pub fn iniciar(config: ConfigSimulador) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let direccion = listener.local_addr()?;
        let estado = Arc::new(Mutex::new(Estado {
            config,
            ..Estado::default()
        }));
        let parar = Arc::new(AtomicBool::new(false));
        let (e, p) = (estado.clone(), parar.clone());
        let hilo = thread::spawn(move || {
            for conexion in listener.incoming() {
                if p.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(s) = conexion {
                    let _ = atender(s, &e);
                }
            }
        });
        Ok(Self {
            direccion,
            estado,
            parar,
            hilo: Some(hilo),
        })
    }

    /// URL del servicio simulado (cualquier ruta vale).
    pub fn url(&self) -> String {
        format!(
            "http://{}/wlpl/TIKE-CONT/ws/SistemaFacturacion/VerifactuSOAP",
            self.direccion
        )
    }

    pub fn direccion(&self) -> SocketAddr {
        self.direccion
    }

    /// Acceso al estado para preparar escenarios y comprobar efectos.
    pub fn estado(&self) -> std::sync::MutexGuard<'_, Estado> {
        self.estado.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for SimuladorAeat {
    fn drop(&mut self) {
        self.parar.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect_timeout(&self.direccion, Duration::from_secs(1));
        if let Some(h) = self.hilo.take() {
            let _ = h.join();
        }
    }
}

fn atender(mut s: TcpStream, estado: &Mutex<Estado>) -> std::io::Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut lector = BufReader::new(s.try_clone()?);
    let mut linea = String::new();
    lector.read_line(&mut linea)?;
    let mut longitud = 0usize;
    loop {
        let mut h = String::new();
        if lector.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        if let Some((n, v)) = h.split_once(':') {
            if n.trim().eq_ignore_ascii_case("content-length") {
                longitud = v.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut cuerpo = vec![0u8; longitud];
    lector.read_exact(&mut cuerpo)?;
    let cuerpo = String::from_utf8_lossy(&cuerpo).into_owned();

    let mut e = estado.lock().unwrap_or_else(|e| e.into_inner());
    e.peticiones.push(cuerpo.clone());
    if let Some(codigo) = e.fallo_http_siguiente.take() {
        drop(e);
        return responder(
            &mut s,
            codigo,
            "text/plain",
            "servicio no disponible (simulado)",
        );
    }
    if !linea.starts_with("POST ") {
        drop(e);
        return responder(
            &mut s,
            500,
            "text/xml; charset=utf-8",
            &fallo(4135, "Esta URL no puede ser utilizada mediante GET."),
        );
    }
    let (codigo, xml) = procesar(&mut e, &cuerpo);
    let perder = std::mem::take(&mut e.perder_siguiente_respuesta);
    drop(e);
    if perder {
        return Ok(());
    }
    responder(&mut s, codigo, "text/xml; charset=utf-8", &xml)
}

fn responder(s: &mut TcpStream, codigo: u16, tipo: &str, cuerpo: &str) -> std::io::Result<()> {
    let texto = match codigo {
        200 => "OK",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Error",
    };
    write!(
        s,
        "HTTP/1.1 {codigo} {texto}\r\nContent-Type: {tipo}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{cuerpo}",
        cuerpo.len()
    )?;
    s.flush()?;
    // Cierre ordenado: el FIN llega aunque otro proceso haya heredado el socket (en Windows,
    // un hijo lanzado en paralelo puede conservarlo y provocar un reinicio al salir).
    s.shutdown(std::net::Shutdown::Write)
}

/// `Fault` SOAP con el formato `Codigo[NNNN].Descripción` que usa la AEAT.
pub fn fallo(codigo: u32, descripcion: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><env:Envelope xmlns:env=\"{NS_SOAP}\"><env:Body><env:Fault><faultcode>env:Client</faultcode><faultstring>Codigo[{codigo}].{}</faultstring></env:Fault></env:Body></env:Envelope>",
        escape(descripcion)
    )
}

struct Linea {
    nif: String,
    serie: String,
    fecha: String,
    operacion: &'static str,
    estado: &'static str,
    codigo: Option<u32>,
    descripcion: Option<String>,
    duplicado: Option<Almacenado>,
}

fn procesar(e: &mut Estado, cuerpo: &str) -> (u16, String) {
    let raiz = match dom::parsear(cuerpo) {
        Ok(r) => r,
        Err(_) => {
            return (
                500,
                fallo(
                    4103,
                    "Se ha producido un error inesperado al parsear el XML.",
                ),
            )
        }
    };
    let Some(envio) = raiz.buscar("RegFactuSistemaFacturacion") else {
        return (
            500,
            fallo(
                4102,
                "El XML no cumple el esquema. Falta informar campo obligatorio.",
            ),
        );
    };
    let Some(obligado) = envio.ruta(&["Cabecera", "ObligadoEmision"]) else {
        return (
            500,
            fallo(
                4102,
                "El XML no cumple el esquema. Falta informar campo obligatorio.",
            ),
        );
    };
    let nif = obligado.texto_de("NIF").unwrap_or_default();
    let nombre = obligado.texto_de("NombreRazon").unwrap_or_default();
    if nif.len() != 9 || !nif.bytes().all(|c| c.is_ascii_alphanumeric()) {
        return (500, fallo(4116, "Error en la cabecera: el campo NIF del bloque ObligadoEmision tiene un formato incorrecto."));
    }
    if let Some(censo) = &e.config.censo {
        if !censo.contains(&nif) {
            return (500, fallo(4104, "Error en la cabecera: el valor del campo NIF del bloque ObligadoEmision no está identificado."));
        }
    }
    let registros: Vec<&Nodo> = envio.hijos("RegistroFactura").collect();
    if registros.is_empty() {
        return (
            500,
            fallo(
                4102,
                "El XML no cumple el esquema. Falta informar campo obligatorio.",
            ),
        );
    }
    if registros.len() > MAX_REGISTROS_POR_ENVIO {
        return (500, fallo(4114, "El XML no cumple con el esquema: se ha superado el límite máximo permitido de facturas a registrar."));
    }

    let ahora = Instant::now();
    if let Some(u) = e.ultimo_envio {
        let espera = Duration::from_secs(u64::from(e.config.tiempo_espera_envio));
        if ahora.duration_since(u) < espera && registros.len() < MAX_REGISTROS_POR_ENVIO {
            e.envios_anticipados += 1;
        }
    }
    e.ultimo_envio = Some(ahora);
    e.envios_procesados += 1;
    e.secuencia += 1;
    let id_peticion = format!("SIM{:017}", e.secuencia);

    let mut lineas = Vec::new();
    for rf in registros {
        let linea = if let Some(a) = rf.hijo("RegistroAlta") {
            alta(e, a, &id_peticion)
        } else if let Some(a) = rf.hijo("RegistroAnulacion") {
            anulacion(e, a)
        } else {
            return (500, fallo(4136, "No se ha enviado el nodo RegistroAlta o el anterior al nodo RegistroAlta no es correcto."));
        };
        lineas.push(linea);
    }
    let correctos = lineas.iter().filter(|l| l.estado == "Correcto").count();
    let incorrectos = lineas.iter().filter(|l| l.estado == "Incorrecto").count();
    let estado_envio = if correctos == lineas.len() {
        "Correcto"
    } else if incorrectos == lineas.len() {
        "Incorrecto"
    } else {
        "ParcialmenteCorrecto"
    };
    let csv = (incorrectos < lineas.len()).then(|| format!("CSVSIM{:010}", e.secuencia));
    (
        200,
        respuesta(
            &nif,
            &nombre,
            csv.as_deref(),
            e.config.tiempo_espera_envio,
            estado_envio,
            &lineas,
        ),
    )
}

fn texto(n: &Nodo, ruta: &[&str]) -> String {
    let (ultimo, padres) = ruta.split_last().expect("ruta");
    n.ruta(padres)
        .and_then(|p| p.texto_de(ultimo))
        .unwrap_or_default()
}

fn aplicar_forzado(e: &Estado, serie: &str, l: &mut Linea) -> bool {
    if let Some(f) = e.config.errores.get(serie) {
        l.estado = if f.aceptado {
            "AceptadoConErrores"
        } else {
            "Incorrecto"
        };
        l.codigo = Some(f.codigo);
        l.descripcion = Some(f.descripcion.clone());
        return !f.aceptado;
    }
    false
}

fn alta(e: &mut Estado, a: &Nodo, id_peticion: &str) -> Linea {
    let nif = texto(a, &["IDFactura", "IDEmisorFactura"]);
    let serie = texto(a, &["IDFactura", "NumSerieFactura"]);
    let fecha = texto(a, &["IDFactura", "FechaExpedicionFactura"]);
    let mut l = Linea {
        nif: nif.clone(),
        serie: serie.clone(),
        fecha: fecha.clone(),
        operacion: "Alta",
        estado: "Correcto",
        codigo: None,
        descripcion: None,
        duplicado: None,
    };
    let clave = (nif.clone(), serie.clone(), fecha.clone());
    let subsanacion = texto(a, &["Subsanacion"]) == "S";
    if let Some(prev) = e.registros.get(&clave) {
        if !subsanacion {
            l.estado = "Incorrecto";
            l.codigo = Some(3000);
            l.descripcion = Some("Registro de facturación duplicado.".to_owned());
            l.duplicado = Some(prev.clone());
            return l;
        }
    } else if subsanacion {
        l.estado = "Incorrecto";
        l.codigo = Some(3002);
        l.descripcion = Some("No existe el registro de facturación.".to_owned());
        return l;
    }
    if aplicar_forzado(e, &serie, &mut l) {
        return l;
    }
    let huella_xml = texto(a, &["Huella"]);
    let calculada = huella::sha256_hex(&huella::cadena_alta(&CamposAlta {
        id_emisor_factura: &nif,
        num_serie_factura: &serie,
        fecha_expedicion_factura: &fecha,
        tipo_factura: &texto(a, &["TipoFactura"]),
        cuota_total: &texto(a, &["CuotaTotal"]),
        importe_total: &texto(a, &["ImporteTotal"]),
        huella_anterior: &texto(a, &["Encadenamiento", "RegistroAnterior", "Huella"]),
        fecha_hora_huso_gen_registro: &texto(a, &["FechaHoraHusoGenRegistro"]),
    }));
    if calculada != huella_xml && l.codigo.is_none() {
        l.estado = "AceptadoConErrores";
        l.codigo = Some(2000);
        l.descripcion = Some("El cálculo de la huella suministrada es incorrecta.".to_owned());
    }
    let estado = if l.estado == "Correcto" {
        "Correcta"
    } else {
        "AceptadaConErrores"
    };
    e.registros.insert(
        clave,
        Almacenado {
            id_peticion: id_peticion.to_owned(),
            estado: estado.to_owned(),
            huella: huella_xml,
        },
    );
    l
}

fn anulacion(e: &mut Estado, a: &Nodo) -> Linea {
    let nif = texto(a, &["IDFactura", "IDEmisorFacturaAnulada"]);
    let serie = texto(a, &["IDFactura", "NumSerieFacturaAnulada"]);
    let fecha = texto(a, &["IDFactura", "FechaExpedicionFacturaAnulada"]);
    let mut l = Linea {
        nif: nif.clone(),
        serie: serie.clone(),
        fecha: fecha.clone(),
        operacion: "Anulacion",
        estado: "Correcto",
        codigo: None,
        descripcion: None,
        duplicado: None,
    };
    let clave = (nif.clone(), serie.clone(), fecha.clone());
    let sin_previo = texto(a, &["SinRegistroPrevio"]) == "S";
    match e.registros.get(&clave) {
        None if !sin_previo => {
            l.estado = "Incorrecto";
            l.codigo = Some(3002);
            l.descripcion = Some("No existe el registro de facturación.".to_owned());
            return l;
        }
        Some(p) if p.estado == "Anulada" => {
            l.estado = "Incorrecto";
            l.codigo = Some(3001);
            l.descripcion = Some("El registro de facturación ya ha sido dado de baja.".to_owned());
            return l;
        }
        _ => {}
    }
    if aplicar_forzado(e, &serie, &mut l) {
        return l;
    }
    let huella_xml = texto(a, &["Huella"]);
    let calculada = huella::sha256_hex(&huella::cadena_anulacion(&CamposAnulacion {
        id_emisor_factura_anulada: &nif,
        num_serie_factura_anulada: &serie,
        fecha_expedicion_factura_anulada: &fecha,
        huella_anterior: &texto(a, &["Encadenamiento", "RegistroAnterior", "Huella"]),
        fecha_hora_huso_gen_registro: &texto(a, &["FechaHoraHusoGenRegistro"]),
    }));
    if calculada != huella_xml && l.codigo.is_none() {
        l.estado = "AceptadoConErrores";
        l.codigo = Some(2000);
        l.descripcion = Some("El cálculo de la huella suministrada es incorrecta.".to_owned());
    }
    let id_peticion = e
        .registros
        .get(&clave)
        .map(|p| p.id_peticion.clone())
        .unwrap_or_default();
    e.registros.insert(
        clave,
        Almacenado {
            id_peticion,
            estado: "Anulada".to_owned(),
            huella: huella_xml,
        },
    );
    l
}

fn respuesta(
    nif: &str,
    nombre: &str,
    csv: Option<&str>,
    tiempo_espera: u32,
    estado_envio: &str,
    lineas: &[Linea],
) -> String {
    let ahora = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let ts = FechaHoraHuso::desde_unix(ahora, 60)
        .map(|t| t.to_string())
        .unwrap_or_default();
    let mut s = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><env:Envelope xmlns:env=\"{NS_SOAP}\"><env:Header/><env:Body><tikR:RespuestaRegFactuSistemaFacturacion xmlns:tikR=\"{NS_RESPUESTA_SUMINISTRO}\" xmlns:tik=\"{NS_SUMINISTRO_INFORMACION}\">"
    );
    if let Some(c) = csv {
        s += &format!(
            "<tikR:CSV>{c}</tikR:CSV><tikR:DatosPresentacion><tik:NIFPresentador>{}</tik:NIFPresentador><tik:TimestampPresentacion>{ts}</tik:TimestampPresentacion></tikR:DatosPresentacion>",
            escape(nif)
        );
    }
    s += &format!(
        "<tikR:Cabecera><tik:ObligadoEmision><tik:NombreRazon>{}</tik:NombreRazon><tik:NIF>{}</tik:NIF></tik:ObligadoEmision></tikR:Cabecera><tikR:TiempoEsperaEnvio>{tiempo_espera}</tikR:TiempoEsperaEnvio><tikR:EstadoEnvio>{estado_envio}</tikR:EstadoEnvio>",
        escape(nombre),
        escape(nif)
    );
    for l in lineas {
        s += &format!(
            "<tikR:RespuestaLinea><tikR:IDFactura><tik:IDEmisorFactura>{}</tik:IDEmisorFactura><tik:NumSerieFactura>{}</tik:NumSerieFactura><tik:FechaExpedicionFactura>{}</tik:FechaExpedicionFactura></tikR:IDFactura><tikR:Operacion><tik:TipoOperacion>{}</tik:TipoOperacion></tikR:Operacion><tikR:EstadoRegistro>{}</tikR:EstadoRegistro>",
            escape(&l.nif),
            escape(&l.serie),
            escape(&l.fecha),
            l.operacion,
            l.estado
        );
        if let Some(c) = l.codigo {
            s += &format!("<tikR:CodigoErrorRegistro>{c}</tikR:CodigoErrorRegistro>");
        }
        if let Some(d) = &l.descripcion {
            s += &format!(
                "<tikR:DescripcionErrorRegistro>{}</tikR:DescripcionErrorRegistro>",
                escape(d)
            );
        }
        if let Some(d) = &l.duplicado {
            s += &format!(
                "<tikR:RegistroDuplicado><tik:IdPeticionRegistroDuplicado>{}</tik:IdPeticionRegistroDuplicado><tik:EstadoRegistroDuplicado>{}</tik:EstadoRegistroDuplicado></tikR:RegistroDuplicado>",
                escape(&d.id_peticion),
                escape(&d.estado)
            );
        }
        s += "</tikR:RespuestaLinea>";
    }
    s + "</tikR:RespuestaRegFactuSistemaFacturacion></env:Body></env:Envelope>"
}
