//! Aritmética de fechas de calendario sin zona horaria, para vencimientos (D88), periodos de
//! plantillas recurrentes (D87) y trimestres de los informes (D89).
//!
//! Opera sobre [`IsoDate`] de `nestadmin-documents` sin modificarla. Calendario gregoriano
//! proléptico; años 1 a 9999.

use nestadmin_documents::IsoDate;

pub fn is_leap(year: u16) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

pub fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// La fecha existe en el calendario.
pub fn is_valid(date: IsoDate) -> bool {
    date.year >= 1
        && (1..=12).contains(&date.month)
        && date.day >= 1
        && date.day <= days_in_month(date.year, date.month)
}

/// Días desde 1970-01-01 (algoritmo de Hinnant). Exige una fecha válida.
pub fn to_days(date: IsoDate) -> i64 {
    let y = i64::from(date.year) - i64::from(date.month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(date.month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(date.day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inversa de [`to_days`]; `None` fuera de los años 1 a 9999.
pub fn from_days(days: i64) -> Option<IsoDate> {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    if !(1..=9999).contains(&year) {
        return None;
    }
    Some(IsoDate {
        year: u16::try_from(year).ok()?,
        month: u8::try_from(month).ok()?,
        day: u8::try_from(day).ok()?,
    })
}

pub fn add_days(date: IsoDate, days: i64) -> Option<IsoDate> {
    if !is_valid(date) {
        return None;
    }
    from_days(to_days(date).checked_add(days)?)
}

/// Suma `months` meses conservando el día; si el mes de destino es más corto, el último día
/// (31 de enero + 1 mes = 28 o 29 de febrero). Para series, súmese siempre desde la fecha
/// original y no de forma iterativa, para que el día no se desplace.
pub fn add_months(date: IsoDate, months: u32) -> Option<IsoDate> {
    if !is_valid(date) {
        return None;
    }
    let index = u64::from(date.year) * 12 + u64::from(date.month - 1) + u64::from(months);
    let year = u16::try_from(index / 12).ok().filter(|y| *y <= 9999)?;
    let month = u8::try_from(index % 12).ok()? + 1;
    Some(IsoDate {
        year,
        month,
        day: date.day.min(days_in_month(year, month)),
    })
}

/// Trimestre natural (1 a 4) de una fecha.
pub fn quarter_of(date: IsoDate) -> u8 {
    (date.month.clamp(1, 12) - 1) / 3 + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: u16, m: u8, day: u8) -> IsoDate {
        IsoDate {
            year: y,
            month: m,
            day,
        }
    }

    #[test]
    fn epoch_and_round_trip() {
        assert_eq!(to_days(d(1970, 1, 1)), 0);
        assert_eq!(to_days(d(2000, 3, 1)), 11_017);
        for days in [-700_000, -1, 0, 1, 11_016, 20_000, 2_900_000] {
            let date = from_days(days).unwrap();
            assert_eq!(to_days(date), days);
        }
        assert_eq!(from_days(-10_000_000), None);
        assert_eq!(from_days(10_000_000), None);
    }

    #[test]
    fn validity_and_leap_years() {
        assert!(is_valid(d(2024, 2, 29)));
        assert!(!is_valid(d(2023, 2, 29)));
        assert!(!is_valid(d(1900, 2, 29)));
        assert!(is_valid(d(2000, 2, 29)));
        assert!(!is_valid(d(2026, 13, 1)));
        assert!(!is_valid(d(2026, 4, 31)));
        assert!(!is_valid(d(0, 1, 1)));
    }

    #[test]
    fn add_days_crosses_months_and_years() {
        assert_eq!(add_days(d(2026, 12, 31), 1), Some(d(2027, 1, 1)));
        assert_eq!(add_days(d(2026, 3, 1), -1), Some(d(2026, 2, 28)));
        assert_eq!(add_days(d(2024, 3, 1), -1), Some(d(2024, 2, 29)));
        assert_eq!(add_days(d(2026, 1, 15), 30), Some(d(2026, 2, 14)));
        assert_eq!(add_days(d(9999, 12, 31), 1), None);
        assert_eq!(add_days(d(2026, 2, 30), 1), None);
    }

    #[test]
    fn add_months_clamps_to_month_end_without_drift() {
        assert_eq!(add_months(d(2026, 1, 31), 1), Some(d(2026, 2, 28)));
        assert_eq!(add_months(d(2028, 1, 31), 1), Some(d(2028, 2, 29)));
        // Desde la original no hay deriva: 31 de enero + 2 meses = 31 de marzo.
        assert_eq!(add_months(d(2026, 1, 31), 2), Some(d(2026, 3, 31)));
        assert_eq!(add_months(d(2026, 11, 15), 3), Some(d(2027, 2, 15)));
        assert_eq!(add_months(d(2026, 5, 10), 12), Some(d(2027, 5, 10)));
        assert_eq!(add_months(d(9999, 12, 1), 1), None);
    }

    #[test]
    fn quarters() {
        assert_eq!(quarter_of(d(2026, 1, 1)), 1);
        assert_eq!(quarter_of(d(2026, 3, 31)), 1);
        assert_eq!(quarter_of(d(2026, 4, 1)), 2);
        assert_eq!(quarter_of(d(2026, 9, 30)), 3);
        assert_eq!(quarter_of(d(2026, 12, 31)), 4);
    }
}
