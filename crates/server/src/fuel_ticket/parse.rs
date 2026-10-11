//! Turns OCR'd fuel-receipt text into a suggested fuel-log entry.
//!
//! Receipts vary wildly by station and country, so nothing here is trusted on its
//! own: keyword rows ("LITROS 42,35") and unit suffixes ("1,459 €/L") produce
//! candidates, and quantity × price ≈ total picks the combination that agrees. The
//! user confirms every value in the app before anything is stored.

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime};
use serde::{Deserialize, Serialize};

/// One recognised text box: `[x1, y1, x2, y2]` in image pixels.
#[derive(Debug, Clone, Deserialize)]
pub struct OcrLine {
    pub text: String,
    #[serde(default)]
    pub score: f64,
    #[serde(rename = "box")]
    pub bbox: [f64; 4],
}

/// Suggested values; every field may be missing.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FuelTicket {
    /// Receipt wall-clock time without zone: the server does not know where the
    /// station is, the phone does.
    pub filled_at_local: Option<NaiveDateTime>,
    /// `L` or `kWh`.
    pub unit: Option<String>,
    pub quantity: Option<f64>,
    pub price_per_unit: Option<f64>,
    pub total_cost: Option<f64>,
    pub currency: Option<String>,
    pub station: Option<String>,
}

/// Groups boxes into visual rows (top to bottom, left to right) and joins each
/// row's text. Receipts print "LABEL ..... VALUE", which OCR splits into two boxes.
pub fn rows(lines: &[OcrLine]) -> Vec<String> {
    let mut boxes: Vec<&OcrLine> = lines.iter().filter(|l| !l.text.trim().is_empty()).collect();
    boxes.sort_by(|a, b| center_y(a).total_cmp(&center_y(b)));
    let mut rows: Vec<Vec<&OcrLine>> = Vec::new();
    for b in boxes {
        // Same row when the box's vertical centre falls inside the row's first box.
        match rows.last_mut() {
            Some(row) if center_y(b) >= row[0].bbox[1] && center_y(b) <= row[0].bbox[3] => {
                row.push(b)
            }
            _ => rows.push(vec![b]),
        }
    }
    rows.into_iter()
        .map(|mut row| {
            row.sort_by(|a, b| a.bbox[0].total_cmp(&b.bbox[0]));
            row.iter()
                .map(|l| l.text.trim())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

fn center_y(l: &OcrLine) -> f64 {
    (l.bbox[1] + l.bbox[3]) / 2.0
}

/// Parses receipt rows. `default_unit` comes from the car's fuel class and is
/// used when the receipt does not say.
pub fn parse(rows: &[String], default_unit: &str) -> FuelTicket {
    let norm: Vec<String> = rows.iter().map(|r| normalize(r)).collect();
    let all = norm.join("\n");
    let unit = if all.contains("kwh") {
        "kWh"
    } else {
        default_unit
    };

    let mut q = Vec::new();
    let mut p = Vec::new();
    let mut t = Vec::new();
    let mut pool = Vec::new();
    for row in &norm {
        let toks = tokens(row);
        let nums: Vec<(usize, f64)> = toks
            .iter()
            .enumerate()
            .filter_map(|(i, tok)| match tok {
                Tok::Num(v) => Some((i, *v)),
                Tok::Word(_) => None,
            })
            .collect();
        if nums.is_empty() || is_noise_row(row) {
            continue;
        }
        pool.extend(nums.iter().map(|&(_, v)| v));
        // A unit right after a number is the strongest signal there is.
        for &(i, v) in &nums {
            match toks.get(i + 1) {
                Some(Tok::Word(w)) if is_price_unit(w) => p.push((v, 3)),
                Some(Tok::Word(w)) if is_quantity_unit(w) => q.push((v, 3)),
                Some(Tok::Word(w)) if is_currency_word(w) => t.push((v, 1)),
                _ => {}
            }
        }
        // Labels: the value is the last number on the row.
        let last = nums[nums.len() - 1].1;
        if has_any(row, PRICE_WORDS) {
            p.push((last, 2));
        } else if has_any(row, QUANTITY_WORDS) {
            q.push((last, 2));
        } else if has_any(row, TOTAL_WORDS) {
            t.push((last, 2));
        }
    }

    let (quantity, price, total) = solve(&q, &p, &t, &pool);
    FuelTicket {
        filled_at_local: find_datetime(&norm),
        unit: Some(unit.to_string()),
        quantity,
        price_per_unit: price,
        total_cost: total,
        currency: find_currency(&all),
        station: find_station(rows, &norm),
    }
}

const QUANTITY_WORDS: &[&str] = &[
    "litros",
    "litres",
    "liters",
    "litre",
    "liter",
    "volumen",
    "volume",
    "cantidad",
    "cant",
    "qty",
    "quantity",
    "suministrado",
    "energia",
    "energy",
];
const PRICE_WORDS: &[&str] = &[
    "precio", "price", "p.u", "pu", "pvp", "unitario", "unit", "tarifa", "€/l", "eur/l", "/l",
    "€/kwh", "eur/kwh", "/kwh",
];
const TOTAL_WORDS: &[&str] = &[
    "total", "importe", "pagar", "amount", "pagado", "cobrado", "tarjeta", "card", "efectivo",
    "cash", "venta",
];

/// Tax breakdowns, change and discounts carry numbers that are none of ours.
fn is_noise_row(row: &str) -> bool {
    row.contains('%')
        || [
            "base imp",
            "cuota",
            "cambio",
            "descuento",
            "dto",
            "subtotal",
            "puntos",
            "saldo",
        ]
        .iter()
        .any(|w| row.contains(w))
}

fn is_price_unit(w: &str) -> bool {
    matches!(
        w,
        "€/l" | "eur/l" | "/l" | "€/lt" | "eur/lt" | "€/kwh" | "eur/kwh" | "/kwh"
    )
}

fn is_quantity_unit(w: &str) -> bool {
    matches!(
        w,
        "l" | "lt" | "lts" | "ltr" | "ltrs" | "litros" | "litres" | "liters" | "kwh"
    )
}

fn is_currency_word(w: &str) -> bool {
    matches!(
        w,
        "€" | "eur" | "euro" | "euros" | "$" | "usd" | "£" | "gbp"
    )
}

fn has_any(row: &str, words: &[&str]) -> bool {
    // "precio/l" is labelled by its first half.
    row.split(|c: char| c.is_whitespace() || c == ':').any(|w| {
        let w = w.trim_matches(|c: char| c == '.' || c == ',');
        words.contains(&w)
            || w.split('/')
                .next()
                .is_some_and(|head| words.contains(&head))
    })
}

// Plausible ranges keep a phone number or a pump id out of the answer.
const Q_RANGE: (f64, f64) = (0.5, 300.0);
const P_RANGE: (f64, f64) = (0.05, 5.0);
const T_RANGE: (f64, f64) = (0.3, 1500.0);

fn in_range(v: f64, (lo, hi): (f64, f64)) -> bool {
    v >= lo && v <= hi
}

fn agrees(q: f64, p: f64, t: f64) -> bool {
    // Receipts round each value; a cent or 1 % is printing noise.
    (q * p - t).abs() <= (0.01 * t).max(0.02)
}

/// Picks quantity, price and total. A combination that multiplies out wins over
/// labels alone; with no labels at all, any agreeing triple from the receipt's
/// numbers is still better than nothing.
fn solve(
    q: &[(f64, u8)],
    p: &[(f64, u8)],
    t: &[(f64, u8)],
    pool: &[f64],
) -> (Option<f64>, Option<f64>, Option<f64>) {
    let q: Vec<_> = q
        .iter()
        .copied()
        .filter(|c| in_range(c.0, Q_RANGE))
        .collect();
    let p: Vec<_> = p
        .iter()
        .copied()
        .filter(|c| in_range(c.0, P_RANGE))
        .collect();
    let t: Vec<_> = t
        .iter()
        .copied()
        .filter(|c| in_range(c.0, T_RANGE))
        .collect();

    let mut best: Option<((f64, f64, f64), u32)> = None;
    for &(qv, qs) in &q {
        for &(pv, ps) in &p {
            for &(tv, ts) in &t {
                let score = qs as u32 + ps as u32 + ts as u32;
                if agrees(qv, pv, tv) && best.is_none_or(|(_, s)| score > s) {
                    best = Some(((qv, pv, tv), score));
                }
            }
        }
    }
    if let Some(((qv, pv, tv), _)) = best {
        return (Some(qv), Some(pv), Some(tv));
    }

    let top = |c: &[(f64, u8)]| c.iter().max_by_key(|x| x.1).map(|x| x.0);
    let (bq, bp, bt) = (top(&q), top(&p), top(&t));
    match (bq, bp, bt) {
        (None, Some(pv), Some(tv)) => return (Some(round(tv / pv, 2)), Some(pv), Some(tv)),
        (Some(_), _, _) => return (bq, bp, bt),
        _ => {}
    }

    // Nothing labelled: search the bare numbers for q × p = t.
    let mut found = None;
    for &tv in pool.iter().filter(|v| in_range(**v, T_RANGE)) {
        for &pv in pool.iter().filter(|v| in_range(**v, P_RANGE)) {
            for &qv in pool.iter().filter(|v| in_range(**v, Q_RANGE)) {
                let trivial = qv == 1.0 || pv == tv || qv == tv;
                if !trivial && agrees(qv, pv, tv) && found.is_none_or(|(_, _, ft)| tv > ft) {
                    found = Some((qv, pv, tv));
                }
            }
        }
    }
    match found {
        Some((qv, pv, tv)) => (Some(qv), Some(pv), Some(tv)),
        None => (bq, bp, bt),
    }
}

/// Combines the parser's suggestion with the AI's. Numbers the parser found that
/// multiply out are arithmetic-checked and kept; otherwise the AI's numbers win
/// when they are plausible. Other fields prefer the AI and fall back to the parser.
pub fn merge(parsed: FuelTicket, ai: FuelTicket) -> FuelTicket {
    let checked = |t: &FuelTicket| match (t.quantity, t.price_per_unit, t.total_cost) {
        (Some(q), Some(p), Some(tc)) => agrees(q, p, tc),
        _ => false,
    };
    let plausible = |v: Option<f64>, r| v.filter(|x| x.is_finite() && in_range(*x, r));
    let unit = ai
        .unit
        .filter(|u| u == "L" || u == "kWh")
        .or(parsed.unit.clone());
    let (quantity, price_per_unit, total_cost) = if checked(&parsed) {
        (parsed.quantity, parsed.price_per_unit, parsed.total_cost)
    } else {
        (
            plausible(ai.quantity, Q_RANGE).or(parsed.quantity),
            plausible(ai.price_per_unit, P_RANGE).or(parsed.price_per_unit),
            plausible(ai.total_cost, T_RANGE).or(parsed.total_cost),
        )
    };
    let currency = ai
        .currency
        .map(|c| c.trim().to_uppercase())
        .filter(|c| c.len() == 3 && c.chars().all(|ch| ch.is_ascii_alphabetic()))
        .or(parsed.currency);
    let station = ai
        .station
        .map(|s| s.trim().chars().take(120).collect::<String>())
        .filter(|s| !s.is_empty())
        .or(parsed.station);
    FuelTicket {
        filled_at_local: ai.filled_at_local.or(parsed.filled_at_local),
        unit,
        quantity,
        price_per_unit,
        total_cost,
        currency,
        station,
    }
}

fn round(v: f64, places: i32) -> f64 {
    let f = 10f64.powi(places);
    (v * f).round() / f
}

#[derive(Debug, PartialEq)]
enum Tok {
    Num(f64),
    Word(String),
}

/// Splits a row into numbers and words. "42,35L" and "1,459€/L" split at the
/// digit boundary so the unit becomes its own token.
fn tokens(row: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    for raw in row.split(|c: char| c.is_whitespace() || c == ':' || c == '=') {
        let raw = raw.trim_matches(|c: char| c == '(' || c == ')' || c == ';');
        if raw.is_empty() {
            continue;
        }
        let split = raw
            .char_indices()
            .find(|&(i, c)| i > 0 && !(c.is_ascii_digit() || c == '.' || c == ','))
            .map(|(i, _)| i);
        let starts_numeric = raw.starts_with(|c: char| c.is_ascii_digit());
        match (starts_numeric, split) {
            (true, Some(i)) => {
                push_num_or_word(&mut out, &raw[..i]);
                out.push(Tok::Word(raw[i..].to_string()));
            }
            (true, None) => push_num_or_word(&mut out, raw),
            (false, _) => {
                // "€61,79": currency prefix glued to the amount.
                let digits_at = raw.find(|c: char| c.is_ascii_digit());
                match digits_at {
                    Some(i)
                        if i > 0
                            && raw[i..]
                                .chars()
                                .all(|c| c.is_ascii_digit() || c == '.' || c == ',') =>
                    {
                        out.push(Tok::Word(raw[..i].to_string()));
                        push_num_or_word(&mut out, &raw[i..]);
                    }
                    _ => out.push(Tok::Word(raw.to_string())),
                }
            }
        }
    }
    out
}

fn push_num_or_word(out: &mut Vec<Tok>, s: &str) {
    match parse_number(s) {
        Some(v) => out.push(Tok::Num(v)),
        None => out.push(Tok::Word(s.to_string())),
    }
}

/// Reads "42,35", "42.35", "1.234,56" and "1,234.56". A lone separator is always
/// decimal: fuel prices print three decimals ("1,459"), so "thousands" would
/// misread every price on the receipt.
pub fn parse_number(s: &str) -> Option<f64> {
    let s = s.trim_end_matches(['.', ',']);
    if s.is_empty()
        || !s
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == ',')
    {
        return None;
    }
    let last_sep = s.rfind(['.', ',']);
    let cleaned: String = match last_sep {
        None => s.to_string(),
        Some(i) => {
            let sep = s.as_bytes()[i] as char;
            let other = if sep == '.' { ',' } else { '.' };
            let repeated = s.matches(sep).count() > 1;
            if repeated && !s.contains(other) {
                // "1.234.567": thousands only.
                s.replace(sep, "")
            } else {
                let int: String = s[..i].chars().filter(char::is_ascii_digit).collect();
                format!("{int}.{}", &s[i + 1..])
            }
        }
    };
    cleaned.parse().ok().filter(|v: &f64| v.is_finite())
}

/// Lowercase, accents folded, so "GASÓLEO" and "gasoleo" match one keyword.
fn normalize(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' | 'Á' | 'À' | 'Ä' | 'Â' => 'a',
            'é' | 'è' | 'ë' | 'ê' | 'É' | 'È' | 'Ë' | 'Ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' | 'Í' | 'Ì' | 'Ï' | 'Î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' | 'Ó' | 'Ò' | 'Ö' | 'Ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' | 'Ú' | 'Ù' | 'Ü' | 'Û' => 'u',
            other => other.to_lowercase().next().unwrap_or(other),
        })
        .collect()
}

fn find_currency(all: &str) -> Option<String> {
    let has = |w: &str| all.split(|c: char| !c.is_alphanumeric()).any(|t| t == w);
    if all.contains('€') || has("eur") || has("euros") {
        Some("EUR".into())
    } else if all.contains('£') || has("gbp") {
        Some("GBP".into())
    } else if has("usd") || all.contains('$') {
        Some("USD".into())
    } else {
        None
    }
}

/// First date on the receipt, with the first time on the same row or, failing
/// that, anywhere. Day-first unless the numbers only work month-first.
fn find_datetime(rows: &[String]) -> Option<NaiveDateTime> {
    let mut date_row = None;
    let mut date = None;
    for (i, row) in rows.iter().enumerate() {
        if let Some(d) = row.split_whitespace().find_map(parse_date) {
            date = Some(d);
            date_row = Some(i);
            break;
        }
    }
    let date = date?;
    let time = date_row
        .and_then(|i| rows[i].split_whitespace().find_map(parse_time))
        .or_else(|| {
            rows.iter()
                .find_map(|r| r.split_whitespace().find_map(parse_time))
        })
        .unwrap_or(NaiveTime::MIN);
    Some(date.and_time(time))
}

fn parse_date(tok: &str) -> Option<NaiveDate> {
    let tok = tok.trim_matches(|c: char| !c.is_ascii_digit());
    let parts: Vec<&str> = tok.split(['/', '-', '.']).collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty() || p.len() > 4) {
        return None;
    }
    let n: Vec<i32> = parts
        .iter()
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let year = |y: i32, len: usize| if len == 2 { 2000 + y } else { y };
    let ymd = if parts[0].len() == 4 {
        (n[0], n[1] as u32, n[2] as u32)
    } else if parts[2].len() == 4 || parts[2].len() == 2 {
        let y = year(n[2], parts[2].len());
        if n[1] > 12 && n[0] <= 12 {
            (y, n[0] as u32, n[1] as u32)
        } else {
            (y, n[1] as u32, n[0] as u32)
        }
    } else {
        return None;
    };
    NaiveDate::from_ymd_opt(ymd.0, ymd.1, ymd.2).filter(|d| (2000..=2100).contains(&d.year()))
}

fn parse_time(tok: &str) -> Option<NaiveTime> {
    let tok = tok.trim_matches(|c: char| !c.is_ascii_digit());
    let parts: Vec<&str> = tok.split(':').collect();
    if !(2..=3).contains(&parts.len()) || parts.iter().any(|p| p.len() != 2) {
        return None;
    }
    let n: Vec<u32> = parts
        .iter()
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    NaiveTime::from_hms_opt(n[0], n[1], n.get(2).copied().unwrap_or(0))
}

/// The station's name is normally the first mostly-letters line at the top.
fn find_station(rows: &[String], norm: &[String]) -> Option<String> {
    const SKIP: &[&str] = &[
        "factura",
        "ticket",
        "recibo",
        "bienvenido",
        "welcome",
        "cif",
        "nif",
        "tel",
        "copia",
        "cliente",
        "receipt",
        "invoice",
    ];
    rows.iter()
        .zip(norm)
        .take(6)
        .find(|(raw, n)| {
            let letters = raw.chars().filter(|c| c.is_alphabetic()).count();
            let len = raw.chars().filter(|c| !c.is_whitespace()).count();
            letters >= 3 && letters * 10 >= len * 6 && !SKIP.iter().any(|w| n.contains(w))
        })
        .map(|(raw, _)| raw.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows_of(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    fn dt(s: &str) -> Option<NaiveDateTime> {
        Some(NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap())
    }

    #[test]
    fn numbers_in_both_locales() {
        assert_eq!(parse_number("42,35"), Some(42.35));
        assert_eq!(parse_number("1,459"), Some(1.459));
        assert_eq!(parse_number("1.459"), Some(1.459));
        assert_eq!(parse_number("1.234,56"), Some(1234.56));
        assert_eq!(parse_number("1,234.56"), Some(1234.56));
        assert_eq!(parse_number("1.234.567"), Some(1234567.0));
        assert_eq!(parse_number("61"), Some(61.0));
        assert_eq!(parse_number("abc"), None);
    }

    #[test]
    fn labelled_spanish_receipt() {
        let t = parse(
            &rows_of(
                "REPSOL E.S. LA ROTONDA\n\
                 CIF B12345678\n\
                 FECHA: 09/10/2026 HORA: 18:32\n\
                 SURTIDOR: 3\n\
                 PRODUCTO: DIESEL E+\n\
                 LITROS: 42,35\n\
                 PRECIO/L: 1,459 €/L\n\
                 BASE IMPONIBLE 51,07\n\
                 IVA 21% 10,72\n\
                 TOTAL: 61,79 €",
            ),
            "L",
        );
        assert_eq!(
            t,
            FuelTicket {
                filled_at_local: dt("2026-10-09 18:32:00"),
                unit: Some("L".into()),
                quantity: Some(42.35),
                price_per_unit: Some(1.459),
                total_cost: Some(61.79),
                currency: Some("EUR".into()),
                station: Some("REPSOL E.S. LA ROTONDA".into()),
            }
        );
    }

    #[test]
    fn single_line_with_units_and_card_total() {
        let t = parse(
            &rows_of(
                "Shell Station 1234\n\
                 2026-03-02 07:05:44\n\
                 Gasolina 95 30,12L x 1,689€/L 50,87\n\
                 Tarjeta 50,87 EUR",
            ),
            "L",
        );
        assert_eq!(t.quantity, Some(30.12));
        assert_eq!(t.price_per_unit, Some(1.689));
        assert_eq!(t.total_cost, Some(50.87));
        assert_eq!(t.filled_at_local, dt("2026-03-02 07:05:44"));
    }

    #[test]
    fn product_check_beats_a_wrong_label() {
        // "IMPORTE" labels the pre-discount amount; the card line is what was paid.
        let t = parse(
            &rows_of(
                "CANTIDAD 40,00\n\
                 PRECIO 1,500\n\
                 IMPORTE 66,00\n\
                 TOTAL 60,00",
            ),
            "L",
        );
        assert_eq!(
            (t.quantity, t.price_per_unit, t.total_cost),
            (Some(40.0), Some(1.5), Some(60.0))
        );
    }

    #[test]
    fn unlabelled_numbers_found_by_product() {
        let t = parse(&rows_of("PUMP 4\n25,00\n1,600\n40,00\nTEL 912345678"), "L");
        assert_eq!(
            (t.quantity, t.price_per_unit, t.total_cost),
            (Some(25.0), Some(1.6), Some(40.0))
        );
    }

    #[test]
    fn quantity_derived_from_total_and_price() {
        let t = parse(&rows_of("PRECIO 1,500 €/L\nTOTAL 60,00 €"), "L");
        assert_eq!(t.quantity, Some(40.0));
    }

    #[test]
    fn charging_receipt_is_kwh() {
        let t = parse(
            &rows_of("Ionity\n11.10.2026 12:00\nEnergy 35,20 kWh\nPrice 0,59 €/kWh\nTotal 20,77 €"),
            "L",
        );
        assert_eq!(t.unit.as_deref(), Some("kWh"));
        assert_eq!(t.quantity, Some(35.2));
        assert_eq!(t.price_per_unit, Some(0.59));
        assert_eq!(t.total_cost, Some(20.77));
        assert_eq!(t.filled_at_local, dt("2026-10-11 12:00:00"));
    }

    #[test]
    fn empty_text_suggests_nothing_but_the_unit() {
        let t = parse(&[], "kWh");
        assert_eq!(
            t,
            FuelTicket {
                unit: Some("kWh".into()),
                ..Default::default()
            }
        );
    }

    #[test]
    fn merge_keeps_checked_numbers_and_takes_ai_extras() {
        let parsed = FuelTicket {
            unit: Some("L".into()),
            quantity: Some(40.0),
            price_per_unit: Some(1.5),
            total_cost: Some(60.0),
            ..Default::default()
        };
        let ai = FuelTicket {
            quantity: Some(4.0),
            station: Some("Cepsa".into()),
            currency: Some("eur".into()),
            filled_at_local: dt("2026-10-09 18:32:00"),
            ..Default::default()
        };
        let m = merge(parsed, ai);
        assert_eq!(m.quantity, Some(40.0));
        assert_eq!(m.station.as_deref(), Some("Cepsa"));
        assert_eq!(m.currency.as_deref(), Some("EUR"));
        assert_eq!(m.filled_at_local, dt("2026-10-09 18:32:00"));
        assert_eq!(m.unit.as_deref(), Some("L"));
    }

    #[test]
    fn merge_prefers_plausible_ai_numbers_when_parser_is_unsure() {
        let parsed = FuelTicket {
            quantity: Some(4235.0),
            total_cost: Some(61.79),
            ..Default::default()
        };
        let ai = FuelTicket {
            quantity: Some(42.35),
            price_per_unit: Some(1.459),
            total_cost: Some(9999.0),
            unit: Some("gallons".into()),
            ..Default::default()
        };
        let m = merge(parsed, ai);
        assert_eq!(m.quantity, Some(42.35));
        assert_eq!(m.price_per_unit, Some(1.459));
        assert_eq!(m.total_cost, Some(61.79));
        assert_eq!(m.unit, None);
    }

    /// Real PaddleOCR output for a photographed, tilted receipt: labels and values
    /// come back as separate boxes and the € sign is lost.
    #[test]
    fn paddle_output_of_a_tilted_receipt() {
        #[derive(Deserialize)]
        struct Out {
            lines: Vec<OcrLine>,
        }
        let out: Out =
            serde_json::from_str(include_str!("testdata/paddle_repsol_rotated.json")).unwrap();
        let t = parse(&rows(&out.lines), "L");
        assert_eq!(
            (t.quantity, t.price_per_unit, t.total_cost),
            (Some(42.35), Some(1.459), Some(61.79))
        );
        assert_eq!(t.filled_at_local, dt("2026-10-09 18:32:00"));
        assert_eq!(t.currency.as_deref(), Some("EUR"));
        assert_eq!(t.station.as_deref(), Some("REPSOL E.S. LA ROTONDA"));
    }

    #[test]
    fn rows_join_label_and_value_boxes() {
        let l = |text: &str, x1: f64, y1: f64, x2: f64, y2: f64| OcrLine {
            text: text.into(),
            score: 0.9,
            bbox: [x1, y1, x2, y2],
        };
        let lines = vec![
            l("42,35", 300.0, 102.0, 360.0, 122.0),
            l("TOTAL", 10.0, 140.0, 80.0, 160.0),
            l("LITROS", 10.0, 100.0, 90.0, 120.0),
            l("61,79", 300.0, 141.0, 360.0, 161.0),
        ];
        assert_eq!(rows(&lines), vec!["LITROS 42,35", "TOTAL 61,79"]);
    }
}
