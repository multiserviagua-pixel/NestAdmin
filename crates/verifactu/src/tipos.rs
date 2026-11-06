//! Tipos básicos con el formato exacto que exigen los esquemas de la AEAT: fechas
//! `DD-MM-AAAA`, fecha-hora con huso `AAAA-MM-DDThh:mm:ss±hh:mm`, importes con punto
//! decimal y dos decimales, NIF de nueve caracteres.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::Error;

/// Fecha de expedición u operación (`sf:fecha`, `DD-MM-AAAA`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Fecha {
    anio: u16,
    mes: u8,
    dia: u8,
}

impl Fecha {
    pub fn new(anio: u16, mes: u8, dia: u8) -> Result<Self, Error> {
        if !(1..=9999).contains(&anio) || !(1..=12).contains(&mes) {
            return Err(Error::Formato(format!("fecha {dia:02}-{mes:02}-{anio:04}")));
        }
        if dia == 0 || dia > dias_mes(anio, mes) {
            return Err(Error::Formato(format!("fecha {dia:02}-{mes:02}-{anio:04}")));
        }
        Ok(Self { anio, mes, dia })
    }

    pub fn anio(&self) -> u16 {
        self.anio
    }
    pub fn mes(&self) -> u8 {
        self.mes
    }
    pub fn dia(&self) -> u8 {
        self.dia
    }
}

impl fmt::Display for Fecha {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}-{:02}-{:04}", self.dia, self.mes, self.anio)
    }
}

impl FromStr for Fecha {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        let err = || Error::Formato(format!("fecha «{s}», se espera DD-MM-AAAA"));
        let b = s.as_bytes();
        if b.len() != 10 || b[2] != b'-' || b[5] != b'-' {
            return Err(err());
        }
        let dia = num(&s[0..2]).ok_or_else(err)?;
        let mes = num(&s[3..5]).ok_or_else(err)?;
        let anio = num(&s[6..10]).ok_or_else(err)?;
        Fecha::new(anio as u16, mes as u8, dia as u8)
    }
}

/// Fecha y hora con huso horario (`FechaHoraHusoGenRegistro`), con precisión de segundos.
/// El desplazamiento se guarda en minutos respecto a UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FechaHoraHuso {
    fecha: Fecha,
    hora: u8,
    minuto: u8,
    segundo: u8,
    huso_minutos: i16,
}

impl FechaHoraHuso {
    pub fn new(
        fecha: Fecha,
        hora: u8,
        minuto: u8,
        segundo: u8,
        huso_minutos: i16,
    ) -> Result<Self, Error> {
        if hora > 23 || minuto > 59 || segundo > 59 {
            return Err(Error::Formato(format!(
                "hora {hora:02}:{minuto:02}:{segundo:02}"
            )));
        }
        if huso_minutos.abs() > 14 * 60 {
            return Err(Error::Formato(format!("huso horario {huso_minutos} min")));
        }
        Ok(Self {
            fecha,
            hora,
            minuto,
            segundo,
            huso_minutos,
        })
    }

    /// Construye la fecha-hora a partir de segundos Unix y un huso. Útil para fechar con el
    /// reloj del sistema sin depender de una biblioteca de fechas.
    pub fn desde_unix(segundos: i64, huso_minutos: i16) -> Result<Self, Error> {
        let local = segundos + i64::from(huso_minutos) * 60;
        let dias = local.div_euclid(86_400);
        let resto = local.rem_euclid(86_400);
        let (anio, mes, dia) = civil_desde_dias(dias);
        if !(1..=9999).contains(&anio) {
            return Err(Error::Formato(format!(
                "instante fuera de rango: {segundos}"
            )));
        }
        let fecha = Fecha::new(anio as u16, mes, dia)?;
        FechaHoraHuso::new(
            fecha,
            (resto / 3600) as u8,
            (resto % 3600 / 60) as u8,
            (resto % 60) as u8,
            huso_minutos,
        )
    }

    pub fn fecha(&self) -> Fecha {
        self.fecha
    }

    pub fn huso_minutos(&self) -> i16 {
        self.huso_minutos
    }

    /// Segundos Unix (UTC) del instante, para comparar fechas con husos distintos.
    pub fn unix(&self) -> i64 {
        let dias = dias_desde_civil(
            i64::from(self.fecha.anio),
            u32::from(self.fecha.mes),
            u32::from(self.fecha.dia),
        );
        dias * 86_400
            + i64::from(self.hora) * 3600
            + i64::from(self.minuto) * 60
            + i64::from(self.segundo)
            - i64::from(self.huso_minutos) * 60
    }
}

impl fmt::Display for FechaHoraHuso {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let signo = if self.huso_minutos < 0 { '-' } else { '+' };
        let h = self.huso_minutos.unsigned_abs();
        write!(
            f,
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}{:02}:{:02}",
            self.fecha.anio,
            self.fecha.mes,
            self.fecha.dia,
            self.hora,
            self.minuto,
            self.segundo,
            signo,
            h / 60,
            h % 60
        )
    }
}

impl FromStr for FechaHoraHuso {
    type Err = Error;
    /// Acepta solo la forma que exige la AEAT: `AAAA-MM-DDThh:mm:ss±hh:mm`. `Z`, las
    /// fracciones de segundo y la ausencia de huso se rechazan.
    fn from_str(s: &str) -> Result<Self, Error> {
        let err = || {
            Error::Formato(format!(
                "fecha-hora «{s}», se espera AAAA-MM-DDThh:mm:ss±hh:mm"
            ))
        };
        let b = s.as_bytes();
        if b.len() != 25
            || b[4] != b'-'
            || b[7] != b'-'
            || b[10] != b'T'
            || b[13] != b':'
            || b[16] != b':'
            || !(b[19] == b'+' || b[19] == b'-')
            || b[22] != b':'
        {
            return Err(err());
        }
        let anio = num(&s[0..4]).ok_or_else(err)?;
        let mes = num(&s[5..7]).ok_or_else(err)?;
        let dia = num(&s[8..10]).ok_or_else(err)?;
        let hora = num(&s[11..13]).ok_or_else(err)?;
        let minuto = num(&s[14..16]).ok_or_else(err)?;
        let segundo = num(&s[17..19]).ok_or_else(err)?;
        let hh = num(&s[20..22]).ok_or_else(err)?;
        let mm = num(&s[23..25]).ok_or_else(err)?;
        if mm > 59 {
            return Err(err());
        }
        let mut huso = (hh * 60 + mm) as i16;
        if b[19] == b'-' {
            huso = -huso;
        }
        let fecha = Fecha::new(anio as u16, mes as u8, dia as u8)?;
        FechaHoraHuso::new(fecha, hora as u8, minuto as u8, segundo as u8, huso)
    }
}

/// Importe en céntimos (`ImporteSgn12.2Type`: signo opcional, hasta 12 dígitos enteros y dos
/// decimales). Se escribe siempre con dos decimales, igual en el XML y en la huella.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Importe(i64);

impl Importe {
    /// Límite de `ImporteSgn12.2Type`: 999 999 999 999,99.
    pub const MAX_CENTIMOS: i64 = 99_999_999_999_999;

    pub fn centimos(c: i64) -> Result<Self, Error> {
        if c.abs() > Self::MAX_CENTIMOS {
            return Err(Error::Formato(format!(
                "importe fuera de rango: {c} céntimos"
            )));
        }
        Ok(Self(c))
    }

    pub fn en_centimos(&self) -> i64 {
        self.0
    }
}

impl fmt::Display for Importe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let signo = if self.0 < 0 { "-" } else { "" };
        let a = self.0.unsigned_abs();
        write!(f, "{signo}{}.{:02}", a / 100, a % 100)
    }
}

impl FromStr for Importe {
    type Err = Error;
    /// Lee el patrón `(\+|-)?\d{1,12}(\.\d{0,2})?` del XSD.
    fn from_str(s: &str) -> Result<Self, Error> {
        let err = || Error::Formato(format!("importe «{s}»"));
        let (neg, cuerpo) = match s.as_bytes().first() {
            Some(b'-') => (true, &s[1..]),
            Some(b'+') => (false, &s[1..]),
            _ => (false, s),
        };
        let (ent, dec) = cuerpo.split_once('.').unwrap_or((cuerpo, ""));
        if ent.is_empty() || ent.len() > 12 || dec.len() > 2 {
            return Err(err());
        }
        if !ent.bytes().chain(dec.bytes()).all(|c| c.is_ascii_digit()) {
            return Err(err());
        }
        let e: i64 = ent.parse().map_err(|_| err())?;
        let d: i64 = match dec.len() {
            0 => 0,
            1 => dec.parse::<i64>().map_err(|_| err())? * 10,
            _ => dec.parse().map_err(|_| err())?,
        };
        let c = e * 100 + d;
        Importe::centimos(if neg { -c } else { c })
    }
}

/// Porcentaje en centésimas (`Tipo2.2Type`: hasta tres enteros y dos decimales, sin signo).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Porcentaje(u32);

impl Porcentaje {
    pub fn centesimas(c: u32) -> Result<Self, Error> {
        if c > 99_999 {
            return Err(Error::Formato(format!("porcentaje fuera de rango: {c}")));
        }
        Ok(Self(c))
    }
    pub fn en_centesimas(&self) -> u32 {
        self.0
    }
}

impl fmt::Display for Porcentaje {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{:02}", self.0 / 100, self.0 % 100)
    }
}

/// Comprueba la forma de un NIF (`NIFType`: nueve caracteres alfanuméricos ASCII). No
/// comprueba la letra de control ni el censo: eso lo hace la AEAT (errores 4107, 4109).
pub fn validar_nif(nif: &str, campo: &str) -> Result<(), Error> {
    if nif.len() == 9 && nif.bytes().all(|c| c.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        Err(Error::Campo {
            campo: campo.to_owned(),
            motivo: "debe tener 9 caracteres alfanuméricos".to_owned(),
        })
    }
}

/// Comprueba una longitud máxima en caracteres y, si `min_uno`, que no esté vacío.
pub(crate) fn validar_texto(
    valor: &str,
    campo: &str,
    max: usize,
    min_uno: bool,
) -> Result<(), Error> {
    let n = valor.trim().chars().count();
    if min_uno && n == 0 {
        return Err(Error::Campo {
            campo: campo.to_owned(),
            motivo: "es obligatorio".to_owned(),
        });
    }
    if valor.chars().count() > max {
        return Err(Error::Campo {
            campo: campo.to_owned(),
            motivo: format!("supera {max} caracteres"),
        });
    }
    if valor.chars().any(|c| c.is_control()) {
        return Err(Error::Campo {
            campo: campo.to_owned(),
            motivo: "contiene caracteres de control".to_owned(),
        });
    }
    Ok(())
}

fn num(s: &str) -> Option<u32> {
    if s.bytes().all(|c| c.is_ascii_digit()) {
        s.parse().ok()
    } else {
        None
    }
}

fn bisiesto(anio: u16) -> bool {
    (anio.is_multiple_of(4) && !anio.is_multiple_of(100)) || anio.is_multiple_of(400)
}

fn dias_mes(anio: u16, mes: u8) -> u8 {
    match mes {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if bisiesto(anio) => 29,
        _ => 28,
    }
}

// Algoritmos de H. Hinnant (dominio público) para pasar de fecha civil a días desde 1970.
fn dias_desde_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_desde_dias(z: i64) -> (i64, u8, u8) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatos() {
        let f: Fecha = "29-02-2024".parse().unwrap();
        assert_eq!(f.to_string(), "29-02-2024");
        assert!("29-02-2023".parse::<Fecha>().is_err());
        assert!("2024-01-01".parse::<Fecha>().is_err());

        let t: FechaHoraHuso = "2024-01-01T19:20:30+01:00".parse().unwrap();
        assert_eq!(t.to_string(), "2024-01-01T19:20:30+01:00");
        assert_eq!(t.unix(), 1_704_133_230);
        assert_eq!(FechaHoraHuso::desde_unix(1_704_133_230, 60).unwrap(), t);
        let n: FechaHoraHuso = "2024-12-31T23:59:59-03:30".parse().unwrap();
        assert_eq!(n.to_string(), "2024-12-31T23:59:59-03:30");
        assert!("2024-01-01T19:20:30Z".parse::<FechaHoraHuso>().is_err());
        assert!("2024-01-01T19:20:30.5+01:00"
            .parse::<FechaHoraHuso>()
            .is_err());

        assert_eq!(Importe::centimos(12_345).unwrap().to_string(), "123.45");
        assert_eq!(Importe::centimos(-5).unwrap().to_string(), "-0.05");
        assert_eq!(Importe::centimos(0).unwrap().to_string(), "0.00");
        assert_eq!("123.1".parse::<Importe>().unwrap().en_centimos(), 12_310);
        assert_eq!("-7".parse::<Importe>().unwrap().en_centimos(), -700);
        assert!("1.234".parse::<Importe>().is_err());
        assert!("1,23".parse::<Importe>().is_err());
        assert!(Importe::centimos(Importe::MAX_CENTIMOS + 1).is_err());

        assert_eq!(Porcentaje::centesimas(2100).unwrap().to_string(), "21.00");
        assert!(validar_nif("89890001K", "NIF").is_ok());
        assert!(validar_nif("8989001K", "NIF").is_err());
    }
}
