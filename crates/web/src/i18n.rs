//! Localisation: UI strings, numbers and dates for English and Spanish.
//!
//! Dependency-free on purpose. Each locale is a plain `(key, text)` table in
//! Rust ([`en::TABLE`], [`es::TABLE`]); the unit tests at the bottom fail when
//! a key is missing from either table, when placeholders disagree, or when the
//! source calls `t("…")` with a key that has no entry — so a gap is a red test,
//! never a blank label.
//!
//! * [`t`] looks a key up in the current locale and returns `&'static str`.
//!   It reads the locale signal, so any reactive closure that calls it re-runs
//!   when the language changes. In `view!`, use [`tr!`](crate::tr) for a
//!   ready-made reactive closure: `<h1>{tr!("nav.trips")}</h1>`.
//! * [`tf`] fills `{name}` placeholders; [`tp`] picks the `.one` / `.other`
//!   plural variant of a key and fills `{n}`.
//! * [`num`] / [`int`] format numbers with the locale's decimal and thousands
//!   separators; [`date`] / [`datetime`] localise month and weekday names.
//!
//! Only displayed strings go through here. Values sent to the API, stored in
//! inputs (`<input type=number|date>`) or embedded in chart JSON as numbers stay
//! machine-formatted.

use std::cell::Cell;
use std::collections::HashMap;

use chrono::{Datelike, NaiveDate, NaiveDateTime, Weekday};
use leptos::prelude::*;

mod en;
mod es;

/// Reactive translated text for `view!`: `{tr!("key")}` or `title=tr!("key")`.
macro_rules! tr {
    ($key:literal) => {
        move || $crate::i18n::t($key)
    };
}

/// localStorage key caching the resolved locale, so the next page load (and the
/// inline boot script in index.html) starts in the right language before
/// `/api/me` answers.
const STORAGE_KEY: &str = "locale";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Locale {
    #[default]
    En,
    Es,
}

impl Locale {
    pub const ALL: [Locale; 2] = [Locale::En, Locale::Es];

    /// BCP 47 tag, also the value stored in `users.locale`.
    pub fn as_str(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::Es => "es",
        }
    }

    /// `"es"`, `"es-MX"`, `"ES_es"` → Spanish; `"en…"` → English; else `None`.
    pub fn parse(raw: &str) -> Option<Self> {
        let lang = raw
            .trim()
            .split(['-', '_'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        match lang.as_str() {
            "es" => Some(Locale::Es),
            "en" => Some(Locale::En),
            _ => None,
        }
    }

    fn table(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Locale::En => en::TABLE,
            Locale::Es => es::TABLE,
        }
    }

    fn decimal_sep(self) -> char {
        match self {
            Locale::En => '.',
            Locale::Es => ',',
        }
    }

    /// Thousands separator. English keeps the app's original ungrouped output
    /// (`15000 km`); Spanish groups with a period (`15.000 km`, `1.234,5 km`).
    fn group_sep(self) -> Option<char> {
        match self {
            Locale::En => None,
            Locale::Es => Some('.'),
        }
    }
}

/// Reactive locale, provided at the app root by [`provide_locale`].
pub type LocaleSignal = RwSignal<Locale>;

thread_local! {
    /// The root signal, so `t()` works in event handlers and spawned futures
    /// where the reactive owner (and so `use_context`) is gone.
    static SIGNAL: Cell<Option<LocaleSignal>> = const { Cell::new(None) };
    /// Locale used when no signal exists (unit tests).
    static FALLBACK: Cell<Locale> = const { Cell::new(Locale::En) };
    static MAPS: [HashMap<&'static str, &'static str>; 2] = Locale::ALL.map(|l| {
        l.table().iter().copied().collect()
    });
}

/// Browser preference: Spanish when `navigator.language` starts with `es`.
pub fn browser_locale() -> Locale {
    web_sys::window()
        .and_then(|w| w.navigator().language())
        .filter(|lang| lang.to_ascii_lowercase().starts_with("es"))
        .map(|_| Locale::Es)
        .unwrap_or(Locale::En)
}

/// The account's stored preference (`Some("en" | "es")`), else the browser's.
pub fn resolve(preference: Option<&str>) -> Locale {
    preference
        .and_then(Locale::parse)
        .unwrap_or_else(browser_locale)
}

fn cached_locale() -> Option<Locale> {
    let storage = web_sys::window()?.local_storage().ok()??;
    Locale::parse(&storage.get_item(STORAGE_KEY).ok()??)
}

/// Create the locale signal. Call once at the app root; it keeps `<html lang>`
/// and the localStorage cache in step with the signal.
pub fn provide_locale() {
    let sig: LocaleSignal = RwSignal::new(cached_locale().unwrap_or_else(browser_locale));
    SIGNAL.with(|s| s.set(Some(sig)));
    provide_context(sig);
    Effect::new(move |_| {
        let loc = sig.get();
        let Some(win) = web_sys::window() else { return };
        if let Some(el) = win.document().and_then(|d| d.document_element()) {
            let _ = el.set_attribute("lang", loc.as_str());
        }
        if let Ok(Some(storage)) = win.local_storage() {
            let _ = storage.set_item(STORAGE_KEY, loc.as_str());
        }
        publish_js_table(&win, loc);
    });
}

/// Expose the `js.*` entries as `window.__ctpI18n` for the inline map, chart
/// and QR scripts, which look text up with `tt(key, englishFallback)`.
fn publish_js_table(win: &web_sys::Window, loc: Locale) {
    let obj = js_sys::Object::new();
    for (key, text) in loc.table().iter().filter(|(k, _)| k.starts_with("js.")) {
        let _ = js_sys::Reflect::set(&obj, &(*key).into(), &(*text).into());
    }
    let _ = js_sys::Reflect::set(win, &"__ctpI18n".into(), &obj);
}

/// Switch the UI language; every `t()` caller re-renders.
pub fn set_locale(loc: Locale) {
    match SIGNAL.with(Cell::get) {
        Some(sig) => {
            if sig.get_untracked() != loc {
                sig.set(loc);
            }
        }
        None => FALLBACK.with(|f| f.set(loc)),
    }
}

/// Current locale. Tracked: calling it inside a reactive closure subscribes.
pub fn locale() -> Locale {
    match SIGNAL.with(Cell::get) {
        Some(sig) => sig.try_get().unwrap_or_default(),
        None => FALLBACK.with(Cell::get),
    }
}

/// Run `f` with `loc` as the current locale (tests; no signal involved).
#[cfg(test)]
pub fn with_locale<R>(loc: Locale, f: impl FnOnce() -> R) -> R {
    let prev = FALLBACK.with(|p| p.replace(loc));
    let out = f();
    FALLBACK.with(|p| p.set(prev));
    out
}

fn lookup(loc: Locale, key: &str) -> Option<&'static str> {
    MAPS.with(|m| m[loc as usize].get(key).copied())
}

/// Translate `key` into the current locale. A key missing from the Spanish table
/// falls back to English, and one missing everywhere shows the key itself (the
/// unit tests keep either from shipping).
pub fn t(key: &'static str) -> &'static str {
    let loc = locale();
    lookup(loc, key)
        .or_else(|| lookup(Locale::En, key))
        .unwrap_or(key)
}

/// Replace `{name}` placeholders in `template`. Unknown names are left as-is.
pub fn fill(template: &str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let name = &after[..close];
                match args.iter().find(|(n, _)| *n == name) {
                    Some((_, v)) => out.push_str(&v.to_string()),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Translate `key` and fill its placeholders: `tf("trips.from", &[("n", &3)])`.
pub fn tf(key: &'static str, args: &[(&str, &dyn std::fmt::Display)]) -> String {
    fill(t(key), args)
}

/// Plural: `key.one` when `n == 1`, else `key.other`, with `{n}` filled in
/// (formatted for the locale). English and Spanish share this one/other rule.
pub fn tp(key: &'static str, n: i64) -> String {
    let variant = format!("{key}{}", if n == 1 { ".one" } else { ".other" });
    let template = lookup(locale(), &variant)
        .or_else(|| lookup(Locale::En, &variant))
        .map(str::to_string)
        .unwrap_or(variant);
    fill(&template, &[("n", &int(n))])
}

// ---------------------------------------------------------------------------
// Numbers
// ---------------------------------------------------------------------------

/// Group the digits of a non-negative integer string with `sep` every three
/// digits, from 1000 up (`1.234`, `12.345`).
fn group(digits: &str, sep: Option<char>) -> String {
    let Some(sep) = sep.filter(|_| digits.len() >= 4) else {
        return digits.to_string();
    };
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(sep);
        }
        out.push(ch);
    }
    out
}

/// Format `v` with `decimals` fraction digits for `loc`.
/// English `1234.5`, Spanish `1.234,5`. Non-finite values render as `—`.
pub fn num_in(loc: Locale, v: f64, decimals: usize) -> String {
    if !v.is_finite() {
        return "—".into();
    }
    let raw = format!("{:.*}", decimals, v.abs());
    let (int_part, frac) = match raw.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (raw.as_str(), None),
    };
    // "-0.0" after rounding reads as zero.
    let negative = v < 0.0 && raw.chars().any(|c| c.is_ascii_digit() && c != '0');
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    out.push_str(&group(int_part, loc.group_sep()));
    if let Some(f) = frac {
        out.push(loc.decimal_sep());
        out.push_str(f);
    }
    out
}

/// [`num_in`] for the current locale.
pub fn num(v: f64, decimals: usize) -> String {
    num_in(locale(), v, decimals)
}

/// Integer with the current locale's thousands separator.
pub fn int(v: i64) -> String {
    let loc = locale();
    let digits = v.unsigned_abs().to_string();
    let grouped = group(&digits, loc.group_sep());
    if v < 0 {
        format!("-{grouped}")
    } else {
        grouped
    }
}

// ---------------------------------------------------------------------------
// Dates
// ---------------------------------------------------------------------------

const MONTHS_SHORT_EN: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const MONTHS_LONG_EN: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const MONTHS_SHORT_ES: [&str; 12] = [
    "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sept", "oct", "nov", "dic",
];
const MONTHS_LONG_ES: [&str; 12] = [
    "enero",
    "febrero",
    "marzo",
    "abril",
    "mayo",
    "junio",
    "julio",
    "agosto",
    "septiembre",
    "octubre",
    "noviembre",
    "diciembre",
];
const WEEKDAYS_SHORT_EN: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const WEEKDAYS_LONG_EN: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const WEEKDAYS_SHORT_ES: [&str; 7] = ["lun", "mar", "mié", "jue", "vie", "sáb", "dom"];
const WEEKDAYS_LONG_ES: [&str; 7] = [
    "lunes",
    "martes",
    "miércoles",
    "jueves",
    "viernes",
    "sábado",
    "domingo",
];

/// Abbreviated month name, `month` 1-12.
pub fn month_short(month: u32) -> &'static str {
    let i = (month.clamp(1, 12) - 1) as usize;
    match locale() {
        Locale::En => MONTHS_SHORT_EN[i],
        Locale::Es => MONTHS_SHORT_ES[i],
    }
}

/// Abbreviated weekday name.
pub fn weekday_short(day: Weekday) -> &'static str {
    let i = day.num_days_from_monday() as usize;
    match locale() {
        Locale::En => WEEKDAYS_SHORT_EN[i],
        Locale::Es => WEEKDAYS_SHORT_ES[i],
    }
}

/// Rewrite the name specifiers of a chrono pattern (`%a %A %b %B`) as literal,
/// localised names for `d`, so chrono only formats the numeric fields.
fn localize_pattern(d: &impl Datelike, pattern: &str) -> String {
    let loc = locale();
    let m = d.month0() as usize;
    let w = d.weekday().num_days_from_monday() as usize;
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let Some(spec) = chars.next() else {
            out.push('%');
            break;
        };
        let name = match (spec, loc) {
            ('b', Locale::En) => MONTHS_SHORT_EN[m],
            ('b', Locale::Es) => MONTHS_SHORT_ES[m],
            ('B', Locale::En) => MONTHS_LONG_EN[m],
            ('B', Locale::Es) => MONTHS_LONG_ES[m],
            ('a', Locale::En) => WEEKDAYS_SHORT_EN[w],
            ('a', Locale::Es) => WEEKDAYS_SHORT_ES[w],
            ('A', Locale::En) => WEEKDAYS_LONG_EN[w],
            ('A', Locale::Es) => WEEKDAYS_LONG_ES[w],
            _ => {
                out.push('%');
                out.push(spec);
                continue;
            }
        };
        out.push_str(name);
    }
    out
}

/// chrono `format` with localised month/weekday names (`%a %A %b %B`).
pub fn date(d: &NaiveDate, pattern: &str) -> String {
    d.format(&localize_pattern(d, pattern)).to_string()
}

/// chrono `format` with localised names, for a (local) date-time.
pub fn datetime(dt: &NaiveDateTime, pattern: &str) -> String {
    dt.format(&localize_pattern(dt, pattern)).to_string()
}

/// Numeric date pattern for display: ISO `2026-09-24` in English (the app's
/// existing style), `24/09/2026` in Spanish.
pub fn date_pattern() -> &'static str {
    match locale() {
        Locale::En => "%Y-%m-%d",
        Locale::Es => "%d/%m/%Y",
    }
}

/// [`date_pattern`] followed by `HH:MM`.
pub fn datetime_pattern() -> &'static str {
    match locale() {
        Locale::En => "%Y-%m-%d %H:%M",
        Locale::Es => "%d/%m/%Y %H:%M",
    }
}

/// [`date_pattern`] followed by `HH:MM:SS`.
pub fn datetime_seconds_pattern() -> &'static str {
    match locale() {
        Locale::En => "%Y-%m-%d %H:%M:%S",
        Locale::Es => "%d/%m/%Y %H:%M:%S",
    }
}

/// An RFC 3339 timestamp in the browser's local zone, as [`datetime_pattern`]
/// (or `pattern` when given). Unparseable input is returned unchanged.
pub fn local_datetime(raw: &str, pattern: Option<&str>) -> String {
    match chrono::DateTime::parse_from_rfc3339(raw.trim()) {
        Ok(dt) => datetime(
            &dt.with_timezone(&chrono::Local).naive_local(),
            pattern.unwrap_or(datetime_pattern()),
        ),
        Err(_) => raw.to_string(),
    }
}

/// Re-render a stored `YYYY-MM-DD` string in the locale's date order; anything
/// else is returned unchanged.
pub fn iso_date(raw: &str) -> String {
    match NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d") {
        Ok(d) => date(&d, date_pattern()),
        Err(_) => raw.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Domain labels for API enum values (the values themselves never change)
// ---------------------------------------------------------------------------

/// `GASOLINE` / `DIESEL` / `HYBRID` / `FULL_ELECTRIC` → display name.
/// Unknown or empty values are shown as sent.
pub fn fuel_class_label(raw: &str) -> String {
    let key = match raw.trim().to_ascii_uppercase().as_str() {
        "GASOLINE" => "fuel.gasoline",
        "DIESEL" => "fuel.diesel",
        "HYBRID" => "fuel.hybrid",
        "FULL_ELECTRIC" => "fuel.electric",
        _ => return raw.to_string(),
    };
    t(key).to_string()
}

/// Share role (`owner` / `editor` / `viewer`) → display name.
pub fn role_label(raw: &str) -> String {
    let key = match raw {
        "owner" => "role.owner",
        "editor" => "role.editor",
        "viewer" => "role.viewer",
        _ => return raw.to_string(),
    };
    t(key).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashSet};

    fn placeholders(s: &str) -> BTreeSet<&str> {
        let mut out = BTreeSet::new();
        let mut rest = s;
        while let Some(open) = rest.find('{') {
            let after = &rest[open + 1..];
            let Some(close) = after.find('}') else { break };
            out.insert(&after[..close]);
            rest = &after[close + 1..];
        }
        out
    }

    fn keys(table: &[(&'static str, &'static str)]) -> Vec<&'static str> {
        table.iter().map(|(k, _)| *k).collect()
    }

    #[test]
    fn tables_have_no_duplicate_keys() {
        for loc in Locale::ALL {
            let mut seen = HashSet::new();
            for k in keys(loc.table()) {
                assert!(seen.insert(k), "{loc:?}: duplicate key {k:?}");
            }
        }
    }

    #[test]
    fn every_english_key_has_a_spanish_entry_and_vice_versa() {
        let en: BTreeSet<_> = keys(en::TABLE).into_iter().collect();
        let es: BTreeSet<_> = keys(es::TABLE).into_iter().collect();
        let missing: Vec<_> = en.difference(&es).collect();
        assert!(missing.is_empty(), "missing from es: {missing:?}");
        let extra: Vec<_> = es.difference(&en).collect();
        assert!(extra.is_empty(), "in es but not en: {extra:?}");
    }

    #[test]
    fn placeholder_names_match_across_locales() {
        for (key, en_text) in en::TABLE {
            let es_text = lookup(Locale::Es, key).unwrap_or_default();
            assert_eq!(
                placeholders(en_text),
                placeholders(es_text),
                "placeholders differ for {key:?}"
            );
        }
    }

    #[test]
    fn no_entry_is_empty() {
        for loc in Locale::ALL {
            for (k, v) in loc.table() {
                assert!(!v.trim().is_empty(), "{loc:?}: empty text for {k:?}");
            }
        }
    }

    /// Text is often glued to a neighbour (`" · full"`), so both locales must
    /// agree on leading and trailing spaces.
    #[test]
    fn edge_whitespace_matches_across_locales() {
        for (key, en_text) in en::TABLE {
            let es_text = lookup(Locale::Es, key).unwrap_or_default();
            assert_eq!(
                (en_text.starts_with(' '), en_text.ends_with(' ')),
                (es_text.starts_with(' '), es_text.ends_with(' ')),
                "edge whitespace differs for {key:?}"
            );
        }
    }

    /// Every `t("…")`, `tr!("…")`, `tf("…"` and `tp("…"` literal in the
    /// source must exist in the English table (plurals as `.one` + `.other`).
    #[test]
    fn every_key_used_in_the_source_exists() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let mut files = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        let en: HashSet<_> = keys(en::TABLE).into_iter().collect();
        let mut missing = Vec::new();
        let mut used = 0usize;
        for file in files {
            if file.ends_with("i18n.rs") {
                continue;
            }
            let src = std::fs::read_to_string(&file).unwrap();
            for (call, plural) in [
                ("t(\"", false),
                ("tr!(\"", false),
                ("tf(\"", false),
                ("tp(\"", true),
                // Inline scripts: `tt('js.…', 'fallback')`.
                ("tt('", false),
            ] {
                let quote = if call.ends_with('\'') { '\'' } else { '"' };
                let mut from = 0;
                while let Some(pos) = src[from..].find(call) {
                    let start = from + pos;
                    from = start + call.len();
                    let prev = src[..start].chars().next_back();
                    if prev.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    let Some(end) = src[from..].find(quote) else {
                        continue;
                    };
                    let key = &src[from..from + end];
                    used += 1;
                    // `tt('js.traffic.' + level, …)`: a prefix needs at least one entry.
                    if key.ends_with('.') {
                        if !en.iter().any(|k| k.starts_with(key)) {
                            missing.push(format!("{}: {key}*", file.display()));
                        }
                        continue;
                    }
                    let wanted: Vec<String> = if plural {
                        vec![format!("{key}.one"), format!("{key}.other")]
                    } else {
                        vec![key.to_string()]
                    };
                    for k in wanted {
                        if !en.contains(k.as_str()) {
                            missing.push(format!("{}: {k}", file.display()));
                        }
                    }
                }
            }
        }
        assert!(used > 0, "scanner found no keys");
        assert!(
            missing.is_empty(),
            "keys without a table entry:\n{}",
            missing.join("\n")
        );
    }

    #[test]
    fn lookup_follows_the_locale() {
        assert_eq!(with_locale(Locale::En, || t("nav.trips")), "Trips");
        assert_eq!(with_locale(Locale::Es, || t("nav.trips")), "Viajes");
        assert_eq!(t("no.such.key"), "no.such.key");
    }

    #[test]
    fn fill_replaces_named_placeholders() {
        assert_eq!(fill("{n} trips", &[("n", &3)]), "3 trips");
        assert_eq!(
            fill("{a} → {b} ({a})", &[("a", &"x"), ("b", &"y")]),
            "x → y (x)"
        );
        assert_eq!(fill("{missing} stays", &[]), "{missing} stays");
        assert_eq!(fill("brace { alone", &[]), "brace { alone");
    }

    #[test]
    fn plurals_pick_one_or_other() {
        with_locale(Locale::En, || {
            assert_eq!(tp("common.trips_count", 1), "1 trip");
            assert_eq!(tp("common.trips_count", 1234), "1234 trips");
        });
        with_locale(Locale::Es, || {
            assert_eq!(tp("common.trips_count", 1), "1 viaje");
            assert_eq!(tp("common.trips_count", 0), "0 viajes");
            assert_eq!(tp("common.trips_count", 1234), "1.234 viajes");
        });
    }

    #[test]
    fn numbers_use_the_locale_separators() {
        assert_eq!(num_in(Locale::En, 1234.5, 1), "1234.5");
        assert_eq!(num_in(Locale::Es, 1234.5, 1), "1.234,5");
        assert_eq!(num_in(Locale::Es, 11.8, 1), "11,8");
        assert_eq!(num_in(Locale::Es, 999.0, 0), "999");
        assert_eq!(num_in(Locale::Es, 1_234_567.891, 2), "1.234.567,89");
        assert_eq!(num_in(Locale::En, -1234.0, 0), "-1234");
        assert_eq!(num_in(Locale::Es, -0.04, 1), "0,0");
        assert_eq!(num_in(Locale::En, f64::NAN, 1), "—");
        with_locale(Locale::Es, || assert_eq!(int(-12345), "-12.345"));
    }

    #[test]
    fn locale_parse_and_resolve() {
        assert_eq!(Locale::parse("es-MX"), Some(Locale::Es));
        assert_eq!(Locale::parse("ES"), Some(Locale::Es));
        assert_eq!(Locale::parse("en_GB"), Some(Locale::En));
        assert_eq!(Locale::parse("fr"), None);
        assert_eq!(Locale::parse(""), None);
        assert_eq!(resolve(Some("es")), Locale::Es);
    }

    #[test]
    fn dates_localise_names() {
        let d = NaiveDate::from_ymd_opt(2026, 1, 5).unwrap();
        with_locale(Locale::En, || {
            assert_eq!(date(&d, "%a %d %b %Y"), "Mon 05 Jan 2026");
            assert_eq!(date(&d, "%A %B"), "Monday January");
            assert_eq!(iso_date("2026-01-05"), "2026-01-05");
        });
        with_locale(Locale::Es, || {
            assert_eq!(date(&d, "%a %d %b %Y"), "lun 05 ene 2026");
            assert_eq!(date(&d, "%A, %d de %B"), "lunes, 05 de enero");
            assert_eq!(iso_date("2026-01-05"), "05/01/2026");
            let dt = d.and_hms_opt(14, 3, 0).unwrap();
            assert_eq!(datetime(&dt, datetime_pattern()), "05/01/2026 14:03");
            // Escaped percent and numeric specifiers pass through untouched.
            assert_eq!(date(&d, "%%b %m"), "%b 01");
        });
    }
}
