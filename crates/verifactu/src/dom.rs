//! Árbol XML mínimo para leer respuestas y peticiones. Se navega por nombre local, de modo
//! que los prefijos que elija la AEAT (`env:`, `soapenv:`, `tikR:`…) no importan. Rechaza
//! `DOCTYPE` para no procesar entidades declaradas por el emisor.

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::Error;

#[derive(Debug, Default, Clone)]
pub(crate) struct Nodo {
    pub nombre: String,
    pub texto: String,
    pub hijos: Vec<Nodo>,
}

impl Nodo {
    pub fn hijo(&self, nombre: &str) -> Option<&Nodo> {
        self.hijos.iter().find(|h| h.nombre == nombre)
    }

    pub fn hijos<'a>(&'a self, nombre: &'a str) -> impl Iterator<Item = &'a Nodo> + 'a {
        self.hijos.iter().filter(move |h| h.nombre == nombre)
    }

    /// Texto recortado de un hijo directo.
    pub fn texto_de(&self, nombre: &str) -> Option<String> {
        self.hijo(nombre).map(|h| h.texto.trim().to_owned())
    }

    /// Primer descendiente (en profundidad, incluido el propio nodo) con ese nombre.
    pub fn buscar(&self, nombre: &str) -> Option<&Nodo> {
        if self.nombre == nombre {
            return Some(self);
        }
        self.hijos.iter().find_map(|h| h.buscar(nombre))
    }

    /// Ruta de hijos directos.
    pub fn ruta(&self, ruta: &[&str]) -> Option<&Nodo> {
        ruta.iter().try_fold(self, |n, p| n.hijo(p))
    }
}

fn nombre_local(q: &str) -> String {
    q.rsplit(':').next().unwrap_or(q).to_owned()
}

pub(crate) fn parsear(xml: &str) -> Result<Nodo, Error> {
    let mut reader = Reader::from_str(xml);
    let mut pila: Vec<Nodo> = vec![Nodo::default()];
    let err = |e: &dyn std::fmt::Display| Error::Xml(e.to_string());
    loop {
        match reader.read_event().map_err(|e| err(&e))? {
            Event::Start(e) => pila.push(Nodo {
                nombre: nombre_local(e.name().as_ref()),
                ..Nodo::default()
            }),
            Event::Empty(e) => {
                let n = Nodo {
                    nombre: nombre_local(e.name().as_ref()),
                    ..Nodo::default()
                };
                pila.last_mut().expect("raíz").hijos.push(n);
            }
            Event::End(_) => {
                let n = pila.pop().expect("pila");
                match pila.last_mut() {
                    Some(p) => p.hijos.push(n),
                    None => return Err(Error::Xml("etiqueta de cierre sobrante".to_owned())),
                }
            }
            Event::Text(t) => {
                let s = t.xml10_content();
                pila.last_mut().expect("raíz").texto.push_str(&s);
            }
            Event::CData(t) => {
                let s: &str = &t;
                pila.last_mut().expect("raíz").texto.push_str(s);
            }
            Event::GeneralRef(r) => {
                let c = if let Some(c) = r.resolve_char_ref().map_err(|e| err(&e))? {
                    c.to_string()
                } else {
                    let nombre: &str = &r;
                    match nombre {
                        "amp" => "&",
                        "lt" => "<",
                        "gt" => ">",
                        "quot" => "\"",
                        "apos" => "'",
                        _ => return Err(Error::Xml(format!("entidad desconocida &{nombre};"))),
                    }
                    .to_owned()
                };
                pila.last_mut().expect("raíz").texto.push_str(&c);
            }
            Event::DocType(_) => {
                return Err(Error::Xml("DOCTYPE no admitido".to_owned()));
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if pila.len() != 1 {
        return Err(Error::Xml("documento incompleto".to_owned()));
    }
    let raiz = pila.pop().expect("raíz");
    raiz.hijos
        .into_iter()
        .next()
        .ok_or_else(|| Error::Xml("documento vacío".to_owned()))
}
