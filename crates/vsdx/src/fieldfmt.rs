//! Field format pictures (GitHub #267) — the expansion a Field row's
//! `Format` picture applies to its evaluated `Value`.
//!
//! Every numeric anchor in this module is pinned from the shipped
//! Visio 2016 stencil library (`Sample/Office16/Visio Content/1033`,
//! measured 2026-11-13/19): the `<fld IX='n'>display</fld>` marker
//! content of a real Visio-authored row is that row's Value `V`
//! rendered through its Format picture, so the (Value V, Value U,
//! picture, marker) quadruples are ground truth for the renderer.
//!
//! **Units** — the mystery the quadruples solve: the Value cell's `U`
//! attribute declares the display unit (`U='M'` renders metres,
//! `U='MM'` millimetres — the corpus's own `_M` stencils carry BOTH,
//! which is why the same internal value appears as `3m.` in one
//! master and `3000mm.` in another). `U='DL'`/`'STR'`/absent declare
//! nothing and the picture falls back to the document's default unit
//! (ISO-A paper → metric `mm.`, else `in.`); `U='PER'` is a percent
//! fraction (`0.25` → `25%`); `U='IN_F'` is feet-inch, which only the
//! `esc(10)` picture renders in `10' 0"` form (decimal pictures show
//! `72.0 in.`).
//!
//! Two corpus anchors supersede the Microsoft FIELDPICTURE page
//! (parity review #223): **`esc(0)` is the GENERAL format with NO
//! unit suffix** — the page claims "one decimal place and a lowercase
//! unit description", but the corpus's own `esc(0)` markers render
//! bare strings (`F`, `C`, `12" x 12"`) — and **`esc(8)` is three
//! decimals without units** (`V='2'` → `2.000`).
//!
//! The per-code decimal table is pinned per code from the attested
//! quadruples: `esc(1)` 0 decimals + unit (`0.7874…` in → `20 mm.`),
//! `esc(2)` 0 decimals no unit (`1.5748…` in, `U='MM'` → `40`),
//! `esc(3)` 0 decimals + unit, percent-aware (`0.25 PER` → `25%`,
//! `300 MM` → `7620 mm.`, `288 FT` → `24 ft.`), `esc(5)` 1 decimal +
//! unit (`70.866… M` → `1.8 m.`), `esc(7)` 2 decimals + unit
//! (`3000.00 mm.`), `esc(8)` 3 decimals no unit, `esc(10)` feet-inch
//! (`120 IN_F` → `10' 0"`). Codes 4, 6, 9, 11–16 are unattested in
//! the corpus (17's only markers carry empty values); they fall back
//! to the general format — documented, not guessed.
//!
//! Dates arrive both as ISO timestamps (`2014-05-15T00:00:00`) and
//! Excel serials (`42095`); the epoch is corpus-pinned
//! (`42005` = 2015-01-01, the shipped cached rows).

/// The document's default display units when a field's Value `U`
/// declares none (`DL`/`STR`/absent): ISO-A paper → metric, else US.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnitSystem {
    #[default]
    Us,
    Metric,
}

/// The display unit a field's value carries: declared by the Value
/// cell's `U` attribute, or the document default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    In,
    Ft,
    Yd,
    Mi,
    Mm,
    Cm,
    M,
    Km,
    /// Percent fraction: `0.25` renders `25%`.
    Per,
    /// Feet-inch (`IN_F`): `esc(10)` renders `10' 0"`, decimal
    /// pictures fall back to inches.
    InF,
}

impl Unit {
    /// Resolve the display unit from a Value `U` attribute; `None`
    /// (no declaration) keeps the document default.
    pub fn from_u(u: &str) -> Option<Unit> {
        Some(match u {
            "IN" => Unit::In,
            "FT" => Unit::Ft,
            "YD" => Unit::Yd,
            "MI" => Unit::Mi,
            "MM" => Unit::Mm,
            "CM" => Unit::Cm,
            "M" => Unit::M,
            "KM" => Unit::Km,
            "PER" => Unit::Per,
            "IN_F" => Unit::InF,
            _ => return None,
        })
    }

    pub fn default_for(system: UnitSystem) -> Unit {
        match system {
            UnitSystem::Us => Unit::In,
            UnitSystem::Metric => Unit::Mm,
        }
    }

    /// Short lowercase label (the format-pictures `u` escape:
    /// "in.", "mm."). Feet-inch has none — it renders as `10' 0"`.
    fn label(self) -> Option<&'static str> {
        Some(match self {
            Unit::In => "in.",
            Unit::Ft => "ft.",
            Unit::Yd => "yd.",
            Unit::Mi => "mi.",
            Unit::Mm => "mm.",
            Unit::Cm => "cm.",
            Unit::M => "m.",
            Unit::Km => "km.",
            Unit::Per | Unit::InF => return None,
        })
    }

    /// Multiplier from internal inches to this unit's magnitude
    /// (1 in = 25.4 mm exactly).
    pub(crate) fn inches_to(self, v: f64) -> f64 {
        match self {
            Unit::In | Unit::InF | Unit::Per => v,
            Unit::Ft => v / 12.0,
            Unit::Yd => v / 36.0,
            Unit::Mi => v / 63360.0,
            Unit::Mm => v * 25.4,
            Unit::Cm => v * 2.54,
            Unit::M => v * 0.0254,
            Unit::Km => v * 2.54e-5,
        }
    }
}

/// The FIELDPICTURE(n) → picture-string table, pinned from the
/// corpus's own Format F→V pairs: n in 0..=17 → `esc(n)`, 37 → `@`,
/// 39 → `@+`, 200/202 → `{<n>}` (the Office predefined short/long
/// date pictures). Unattested codes keep the observed family shape
/// rather than inventing new syntax.
pub fn field_picture(n: i64) -> String {
    match n {
        0..=17 => format!("esc({n})"),
        37 => "@".to_string(),
        39 => "@+".to_string(),
        other => format!("{{<{other}>}}"),
    }
}

/// Render `value` — a raw evaluated Value string: a number (internal
/// inches for lengths), an ISO timestamp, an Excel date serial, or
/// plain text — through a format `picture`.
///
/// `unit_hint` is the Value cell's `U` attribute (see [`Unit`]);
/// `default_units` is the document default when the hint declares
/// nothing. Unparseable input degrades to the value as-is: a field
/// must never render emptier than its own cached string.
pub fn format_value(
    value: &str,
    unit_hint: Option<&str>,
    picture: &str,
    default_units: UnitSystem,
) -> String {
    let picture = picture.trim();
    // Numeric/date parsing works on the TRIMMED view; text
    // substitution and every FALLBACK return the ORIGINAL value —
    // whitespace is content (`"  A  "` stays `"  A  "`, a
    // whitespace-only value never becomes empty) (CodeRabbit PR
    // review).
    let original = value;
    let value = value.trim();
    let unit = unit_hint
        .and_then(Unit::from_u)
        .unwrap_or_else(|| Unit::default_for(default_units));
    match picture {
        "" => original.to_string(),
        "@" => original.to_string(),
        "@+" => original.to_uppercase(),
        "@-" => original.to_lowercase(),
        // `c` is the SHORT general date (corpus-pinned); `C` is the
        // LONG form ([MS] format pictures; CodeRabbit PR review).
        // Both fall back to the ORIGINAL (untrimmed) value — the
        // parse works on the trimmed view, but a non-date's
        // whitespace is content (CodeRabbit PR review).
        "c" => match parse_date(value) {
            Some(d) => short_date_parts(&d),
            None => original.to_string(),
        },
        "C" => match parse_date(value) {
            Some(d) => format!("{} {}, {}", month_name(&d), d.d, d.y),
            None => original.to_string(),
        },
        p if p.starts_with("esc(") && p.ends_with(')') => {
            let code: i64 = match p[4..p.len() - 1].trim().parse() {
                Ok(c) => c,
                Err(_) => return original.to_string(),
            };
            esc_format(value, code, unit, original)
        }
        p if p.starts_with("{{") && p.ends_with("}}") => match parse_date(value) {
            Some(d) => date_picture(&d, &p[2..p.len() - 2]),
            None => original.to_string(),
        },
        p if p.starts_with("{<") && p.ends_with(">}") => {
            let code: i64 = match p[2..p.len() - 2].trim().parse() {
                Ok(c) => c,
                Err(_) => return original.to_string(),
            };
            match code {
                // 200: predefined SHORT date (corpus: DOCCREATION
                // 2012-07-26T10:23:29 → `7/26/2012`).
                200 => match parse_date(value) {
                    Some(d) => short_date_parts(&d),
                    None => original.to_string(),
                },
                // 202: predefined LONG date (corpus: DOCLASTSAVE
                // 2012-07-18T23:57:59 → `July 18, 2012`).
                202 => match parse_date(value) {
                    Some(d) => format!("{} {}, {}", month_name(&d), d.d, d.y),
                    None => original.to_string(),
                },
                _ => original.to_string(),
            }
        }
        // Digit pictures: `0`, `0.00`, `1`, with or without the `u`
        // unit suffix (no space before the label — the corpus's
        // `0u`/`0.00u` markers: `3m.`, `50.00mm.`).
        p if is_digit_picture(p) => digit_picture(
            value,
            p,
            unit,
            original,
            // An EXPLICIT unit declaration (a recognized `U` code,
            // not the `DL`/absent paper default) converts even
            // without the `u` label — the caller already expressed
            // the value in internal inches for that display unit
            // (FORMATEX's explicit in/out units; CodeRabbit PR
            // review: `FORMATEX(1,"0.00","ft")` must render `1.00`,
            // not the raw inch count `12.00`).
            unit_hint.is_some_and(|u| Unit::from_u(u).is_some()),
        ),
        // A bare date picture (`FORMAT(User.Date,"d")`, the Gantt
        // set): every character is a date token or separator.
        p if is_bare_date_picture(p) => match parse_date(value) {
            Some(d) => date_picture(&d, p),
            None => original.to_string(),
        },
        _ => original.to_string(),
    }
}

/// `0`/`.`/`u`-only pictures (plus a bare `1`, the corpus's
/// integer-with-no-unit oddity).
/// A picture made only of date tokens and date separators — the
/// corpus's bare `FORMAT(...,"d")` form.
fn is_bare_date_picture(p: &str) -> bool {
    !p.is_empty()
        && p.chars().any(|c| "MdDyhs".contains(c))
        && p.chars()
            .all(|c| "MdDyhmHst".contains(c) || "/-.:, ".contains(c))
}

fn is_digit_picture(p: &str) -> bool {
    // Interior spaces are Visio's `0.00 u` spacing before the unit
    // label ([MS] FORMATEX; CodeRabbit PR review) — allowed between
    // the digits and a trailing `u`/`U`.
    let squeezed: String = p.chars().filter(|&c| c != ' ').collect();
    !p.is_empty()
        && squeezed
            .chars()
            .all(|c| c == '0' || c == '.' || c == 'u' || c == 'U' || c == '1')
        && squeezed.chars().any(|c| c == '0' || c == '1')
}

/// A digit picture: decimals from the `0`s after the `.`, unit label
/// appended (no space for `0.00u`, Visio's own space preserved for
/// `0.00 u`).
///
/// `explicit_unit` marks a unit the CALLER declared (a recognized
/// Value-cell `U` / FORMATEX in-out code — the value already sits in
/// internal inches for that display unit); without it, a bare digit
/// picture (`0`, `0.00`) renders the number as-is.
fn digit_picture(value: &str, p: &str, unit: Unit, original: &str, explicit_unit: bool) -> String {
    let has_unit = p.contains('u') || p.contains('U');
    let decimals = p
        .split_once('.')
        .map_or(0, |(_, frac)| frac.chars().filter(|&c| c == '0').count());
    let Some(n) = leading_number(value) else {
        return original.to_string();
    };
    let dec_unit = match unit {
        Unit::InF => Unit::In,
        u => u,
    };
    // A BARE digit picture (`0`, `0.00`) over the paper-family
    // DEFAULT renders the number as-is — no length conversion: the
    // corpus's `0` rows carry counts and page numbers (V='0' → `0`),
    // and converting would turn Page Number 2 into `51` on metric
    // paper (Greptile round 1). The `u` suffix and an EXPLICIT unit
    // declaration are both length displays (CodeRabbit PR review).
    let shown = if has_unit || explicit_unit {
        dec_unit.inches_to(n)
    } else {
        n
    };
    // An overflowing conversion (1E308 × 25.4 = inf) must not blank
    // the field — the formatter's contract is a nonempty fallback
    // (CodeRabbit PR review).
    if !shown.is_finite() {
        return original.to_string();
    }
    let mut out = format_decimals(shown, decimals);
    if has_unit {
        if let Some(label) = dec_unit.label() {
            // Visio's `0.00 u` spelling keeps its space before the
            // label; `0.00u` stays compact.
            if p.contains(" u") || p.contains(" U") {
                out.push(' ');
            }
            out.push_str(label);
        }
    }
    out
}

/// The `esc(n)` table — the per-code rendering rules pinned from the
/// corpus quadruples (see the module docs).
fn esc_format(value: &str, code: i64, unit: Unit, original: &str) -> String {
    // String values that carry their own percent sign render as the
    // number + `%` (corpus: `10 %` STR → `10%` under esc(1)).
    if value.ends_with('%') {
        if let Some(n) = leading_number(value) {
            return match code {
                5 => format!("{}%", format_decimals(n, 1)),
                7 => format!("{}%", format_decimals(n, 2)),
                8 => format!("{}%", format_decimals(n, 3)),
                _ => format!("{}%", format_decimals(n, 0)),
            };
        }
    }
    // The percent unit renders as ×100 + `%` under every decimal
    // picture (corpus: `0.25 PER` → `25%` under esc(3)).
    if unit == Unit::Per {
        if let Some(n) = leading_number(value) {
            return match code {
                5 => format!("{}%", format_decimals(n * 100.0, 1)),
                7 => format!("{}%", format_decimals(n * 100.0, 2)),
                8 => format!("{}%", format_decimals(n * 100.0, 3)),
                _ => format!("{}%", format_decimals(n * 100.0, 0)),
            };
        }
    }
    // Under esc(3), a string that is already number+unit text
    // (`1270mm`, `4 ft.`) normalizes to `<n> <label>.` (SHPREC).
    // Other pictures leave text alone (esc(0) passes `0.6 in.`
    // through verbatim, ANNOT_U).
    if code == 3 && !value.is_empty() {
        if let Some(rest) = split_trailing_unit_text(value) {
            return rest;
        }
    }
    let Some(n) = leading_number(value) else {
        // Not a number at all — the general pass-through.
        return original.to_string();
    };
    let (decimals, with_label) = match code {
        1 => (0, true),
        2 => (0, false),
        3 => (0, true),
        5 => (1, true),
        7 => (2, true),
        8 => (3, false),
        10 => return feet_inches(n),
        // 0, 4, 6, 9, 11–17: the general format (esc(0) is the
        // #223-measured general; the rest are unattested). A value
        // that is not wholly numeric is text (`12" x 12"`) and
        // passes through verbatim.
        _ => {
            return match value.trim().parse::<f64>() {
                Ok(v) if value.trim().chars().any(|c| c.is_ascii_digit()) => general_number(v),
                _ => original.to_string(),
            };
        }
    };
    let dec_unit = match unit {
        Unit::InF => Unit::In,
        u => u,
    };
    let shown = dec_unit.inches_to(n);
    // Overflowing conversions keep the original visible (the
    // nonempty-fallback contract).
    if !shown.is_finite() {
        return original.to_string();
    }
    let mut out = format_decimals(shown, decimals);
    if with_label {
        if let Some(label) = dec_unit.label() {
            // The esc pictures put a space before the short label.
            out.push(' ');
            out.push_str(label);
        }
    }
    out
}

/// A trailing `number unit` string (`1270mm`, `4 ft.`) normalized to
/// `<n> <label>.` when the tail names a known unit; `None` leaves the
/// caller its other rules.
fn split_trailing_unit_text(value: &str) -> Option<String> {
    let split = value.char_indices().rfind(|(_, c)| c.is_ascii_digit())?;
    let (head, tail) = value.split_at(split.0 + 1);
    let n = leading_number(head)?;
    let tail_clean = tail.trim().trim_end_matches('.');
    let unit = Unit::from_u(&tail_clean.to_ascii_uppercase())?;
    match unit {
        Unit::Per | Unit::InF => None,
        u => {
            let label = u.label()?;
            Some(format!("{} {}", format_decimals(n, 0), label))
        }
    }
}

/// `120` (inches) → `10' 0"` — the feet-inch picture (esc(10)).
fn feet_inches(v: f64) -> String {
    // Feet beyond i64 range (1E21 inches) fall back to the general
    // number instead of a saturating cast whose carry overflows
    // (CodeRabbit PR review).
    if !v.is_finite() || v < 0.0 || !(0.0..8.29e18).contains(&(v / 12.0)) {
        return general_number(v);
    }
    let mut ft = (v / 12.0).floor() as i64;
    let mut inches = ((v - 12.0 * ft as f64) * 100.0).round() / 100.0;
    if inches >= 12.0 {
        // Rounding crossed a foot boundary.
        ft += 1;
        inches -= 12.0;
    }
    format!("{ft}' {inches}\"")
}

/// Compact number: integers plain, fractions with trailing zeros
/// trimmed (the general picture's number shape).
fn general_number(v: f64) -> String {
    if !v.is_finite() {
        return String::new();
    }
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// Fixed decimals with the corpus's exact digit shape.
fn format_decimals(v: f64, decimals: usize) -> String {
    if !v.is_finite() {
        return String::new();
    }
    format!("{v:.decimals$}", decimals = decimals)
}

/// The first numeric run of a value string (`"10 %"`, `"0.25"`,
/// `"118.1102362204724"`), ignoring surrounding text.
fn leading_number(value: &str) -> Option<f64> {
    let mut i = 0;
    for (idx, ch) in value.char_indices() {
        if ch.is_ascii_digit() || ch == '.' || ch == '-' || ch == '+' {
            i = idx;
            break;
        }
        if !ch.is_whitespace() {
            return None;
        }
        i = idx + 1;
    }
    let start = i;
    let bytes = value.as_bytes();
    let mut end = start;
    while end < bytes.len()
        && (bytes[end].is_ascii_digit()
            || bytes[end] == b'.'
            || bytes[end] == b'-'
            || bytes[end] == b'+')
    {
        end += 1;
    }
    // A scientific exponent belongs to the number (`1.2E-3`).
    if end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
        let mut exp_end = end + 1;
        if exp_end < bytes.len() && (bytes[exp_end] == b'-' || bytes[exp_end] == b'+') {
            exp_end += 1;
        }
        let digits_start = exp_end;
        while exp_end < bytes.len() && bytes[exp_end].is_ascii_digit() {
            exp_end += 1;
        }
        if exp_end > digits_start {
            end = exp_end;
        }
    }
    let tok = &value[start..end];
    if tok.is_empty() || !tok.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    // Shape validation stays with the float parser: a manual
    // punctuation count rejected `-1.2E-3` (sign in BOTH the mantissa
    // and the exponent exceeds two), while `parse()` accepts it and
    // still rejects malformed tokens like `1.2.3` (CodeRabbit PR
    // review).
    tok.parse().ok()
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DateParts {
    y: i32,
    d: u32,
    mo: u32,
    h: u32,
    mi: u32,
    s: u32,
}

/// Parse a field value as a date: ISO timestamp first, then an Excel
/// day serial (epoch corpus-pinned: 42005 = 2015-01-01).
fn parse_date(value: &str) -> Option<DateParts> {
    // ISO 8601: YYYY-MM-DD[T ]HH:MM[:SS]
    let bytes = value.as_bytes();
    if bytes.len() >= 10
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
    {
        let y: i32 = value[0..4].parse().ok()?;
        let mo: u32 = value[5..7].parse().ok()?;
        let d: u32 = value[8..10].parse().ok()?;
        // Calendar validity: `2024-02-30` is NOT a date — the
        // formatter's contract leaves unparseable input unchanged
        // (February 29 stays valid in leap years; CodeRabbit PR
        // review).
        if !(1..=12).contains(&mo) || d < 1 || d > days_in_month(y, mo) {
            return None;
        }
        let (h, mi, s) = match value.get(10..) {
            Some(t) if t.starts_with('T') || t.starts_with(' ') => {
                let t = &t[1..];
                let mut it = t.split(':');
                let h: u32 = it.next()?.get(0..2)?.parse().ok()?;
                let mi: u32 = it
                    .next()
                    .and_then(|m| m.get(0..2))
                    .and_then(|x| x.parse().ok())
                    .unwrap_or(0);
                let s: u32 = it
                    .next()
                    .and_then(|x| x.get(0..2))
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                if h > 23 || mi > 59 || s > 59 {
                    return None;
                }
                (h, mi, s)
            }
            _ => (0, 0, 0),
        };
        return Some(DateParts { y, d, mo, h, mi, s });
    }
    // Excel serial (fractional days; the corpus caches whole days).
    let serial: f64 = leading_number(value)?;
    if !(0.0..2_958_466.0).contains(&serial) {
        return None;
    }
    // Split the NONNEGATIVE serial into day + fraction BEFORE the
    // epoch offset: `25568.5` (1969-12-31 noon) made `(serial -
    // 25569).fract()` negative and the u32 cast zeroed it
    // (CodeRabbit PR review). Rounding can carry a full day
    // (42005.999999 → 86400 s): advance the date and use midnight.
    let day_count = serial.floor();
    let mut days = (day_count - 25569.0) as i64; // 25569 = 1970-01-01
    let mut secs_of_day = ((serial - day_count) * 86400.0).round() as u32;
    if secs_of_day >= 86_400 {
        days += 1;
        secs_of_day -= 86_400;
    }
    let (y, mo, d) = civil_from_days(days);
    Some(DateParts {
        y,
        d,
        mo,
        h: secs_of_day / 3600,
        mi: (secs_of_day / 60) % 60,
        s: secs_of_day % 60,
    })
}

/// Days in a month, leap years included.
fn days_in_month(y: i32, mo: u32) -> u32 {
    match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Days-since-epoch → civil date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    ((y + i64::from(m <= 2)) as i32, m, d)
}

/// Short date `M/d/yyyy` — the corpus's `c` and `{<200>}` rendering.
fn short_date_parts(d: &DateParts) -> String {
    format!("{}/{}/{}", d.mo, d.d, d.y)
}

fn month_name(d: &DateParts) -> &'static str {
    const NAMES: [&str; 12] = [
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
    NAMES[(d.mo.clamp(1, 12) - 1) as usize]
}

fn weekday_name(d: &DateParts) -> &'static str {
    // Days since 1970-01-01, a Thursday (weekday index 4 with
    // Sunday = 0).
    let (y, m, dd) = (d.y as i64, d.mo as i64, d.d as i64);
    let a = if m < 3 { y - 1 } else { y };
    let mm = if m < 3 { m + 9 } else { m - 3 }; // Mar = 0
    let days = 365 * a + a / 4 - a / 100 + a / 400 + (153 * mm + 2) / 5 + dd - 1 - 719_468;
    const DAYS: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    let idx = ((days % 7) + 7 + 4) % 7; // 1970-01-01 → Thursday
    DAYS[idx as usize]
}

/// An Office `{{…}}` date picture: the standard token set
/// (`aaaa`-class locale tokens outside it stay literal).
fn date_picture(d: &DateParts, pic: &str) -> String {
    // Tokenize once so an `m`-run can see its neighbors: minutes
    // beside a time token (`HH:mm`, `mm:ss`), months otherwise
    // (`m/d/yy`) — the [MS] format-picture rule.
    let mut toks: Vec<(String, usize)> = Vec::new();
    let mut rest = pic;
    while !rest.is_empty() {
        let (tok, len) = next_date_token(rest);
        // The advance MUST come from the branch taken — `len` is 0
        // for a literal char, and advancing by it would loop forever
        // (the review round's OOM: the kernel SIGKILLed the test
        // binary).
        let adv = match tok {
            Some(t) => {
                toks.push((t.to_string(), len));
                len
            }
            None => {
                let ch = rest.chars().next().unwrap();
                toks.push((ch.to_string(), ch.len_utf8()));
                ch.len_utf8()
            }
        };
        rest = &rest[adv.min(rest.len())..];
    }
    let mut out = String::new();
    for (i, (t, _)) in toks.iter().enumerate() {
        if t.chars().all(|c| c == 'm') {
            // Minutes beside a time token — across one separator
            // (`HH:mm`, `mm:ss`); months otherwise (`m/d/yy`).
            let is_sep = |nt: &str| !nt.is_empty() && nt.chars().all(|c| ":.-/ ".contains(c));
            let mut time_neighbor = false;
            for dir in [-1isize, 1] {
                let mut k = i as isize + dir;
                let mut crossed_sep = false;
                while k >= 0 && (k as usize) < toks.len() {
                    let (nt, _) = &toks[k as usize];
                    let first = nt.chars().next().unwrap_or(' ');
                    if "hHs".contains(first) {
                        time_neighbor = true;
                        break;
                    }
                    if !crossed_sep && is_sep(nt) {
                        crossed_sep = true;
                        k += dir;
                        continue;
                    }
                    break;
                }
            }
            let expanded = match (t.len(), time_neighbor) {
                (1, true) => format!("{}", d.mi),
                (2, true) => format!("{:02}", d.mi),
                (1, false) => format!("{}", d.mo),
                (2, false) => format!("{:02}", d.mo),
                _ => month_name(d)[..t.len().min(3)].to_string(),
            };
            out.push_str(&expanded);
        } else {
            out.push_str(&expand_date_token(d, t));
        }
    }
    out
}

/// The longest date token starting `rest`, with its byte length.
/// A run of one date-letter character is one token (longest run
/// wins); anything else is a literal.
fn next_date_token(rest: &str) -> (Option<&str>, usize) {
    let Some(c0) = rest.chars().next() else {
        return (None, 0);
    };
    // Lowercase `m` IS a date letter (month/minute per context) —
    // omitting it tokenized `mm` as two literal `m`s (the review
    // round's `{{HH:mm}}` → `13:55` bug).
    if !"MdDymhHst".contains(c0) {
        return (None, 0);
    }
    let len = rest
        .chars()
        .take_while(|&c| c == c0)
        .map(char::len_utf8)
        .sum::<usize>();
    (Some(&rest[..len]), len)
}

fn expand_date_token(d: &DateParts, t: &str) -> String {
    // 12-hour clock: midnight and noon render as 12.
    let h12 = match d.h % 12 {
        0 => 12,
        h => h,
    };
    match t {
        "d" => format!("{}", d.d),
        "dd" => format!("{:02}", d.d),
        // Uppercase-D runs are weekday forms: `{{DD}}` is the
        // 2-letter weekday (corpus witness: FORMATEX(1,"{{DD}}",,,
        // 1033) = "Su", serial 1 = Sunday); longer runs extend the
        // same ladder.
        "DD" => weekday_name(d)[..2].to_string(),
        "DDD" => weekday_name(d)[..3].to_string(),
        "DDDD" | "DDDDD" | "DDDDDD" => weekday_name(d).to_string(),
        "ddd" => weekday_name(d)[..3].to_string(),
        "dddd" => weekday_name(d).to_string(),
        // Short/long system date (unattested in the corpus pictures;
        // the system shapes the corpus's `c`/`{<200>}`/`{<202>}` use).
        "ddddd" => short_date_parts(d),
        "dddddd" => format!("{} {}, {}", month_name(d), d.d, d.y),
        "M" => format!("{}", d.mo),
        "MM" => format!("{:02}", d.mo),
        "MMM" => month_name(d)[..3].to_string(),
        "MMMM" => month_name(d).to_string(),
        "MMMMM" => month_name(d)[..1].to_string(),
        "yy" => format!("{:02}", d.y.rem_euclid(100)),
        "yyyy" => format!("{}", d.y),
        "y" => format!("{}", d.y.rem_euclid(100)),
        "h" => format!("{h12}"),
        "hh" => format!("{h12:02}"),
        "H" => format!("{}", d.h),
        "HH" => format!("{:02}", d.h),
        "m" => format!("{}", d.mi),
        "mm" => format!("{:02}", d.mi),
        "s" => format!("{}", d.s),
        "ss" => format!("{:02}", d.s),
        "t" => if d.h < 12 { "A" } else { "P" }.to_string(),
        "tt" => if d.h < 12 { "AM" } else { "PM" }.to_string(),
        _ => t.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: UnitSystem = UnitSystem::Metric;
    const U: UnitSystem = UnitSystem::Us;

    /// Every pin is a (Value V, Value U, picture, marker content)
    /// quadruple measured off the shipped corpus's own rows.
    #[test]
    fn corpus_esc_pins() {
        // esc(0): general, no unit — strings pass through (#223
        // anchor: the MS page's "one decimal + unit" is wrong).
        assert_eq!(format_value("F", Some("STR"), "esc(0)", M), "F");
        assert_eq!(
            format_value("12\" x 12\"", Some("STR"), "esc(0)", U),
            "12\" x 12\""
        );
        assert_eq!(format_value("0.6 in.", Some("STR"), "esc(0)", U), "0.6 in.");
        // esc(1): 0 decimals + unit. CALOUT_M/U (U='DL' → doc
        // default): 0.7874 in → 20 mm (metric), 1 → 1 in (US).
        assert_eq!(
            format_value("0.7874015748031494", Some("DL"), "esc(1)", M),
            "20 mm."
        );
        assert_eq!(format_value("1", Some("DL"), "esc(1)", U), "1 in.");
        assert_eq!(format_value("10 %", Some("STR"), "esc(1)", M), "10%");
        // esc(2): 0 decimals in the declared unit, no label
        // (TRANSP_M: 1.5748 in, U='MM' → 40).
        assert_eq!(
            format_value("1.57480314960631", Some("MM"), "esc(2)", M),
            "40"
        );
        // esc(3): 0 decimals + unit, percent-aware (CHART/SHPREC).
        assert_eq!(format_value("0.25", Some("PER"), "esc(3)", M), "25%");
        assert_eq!(format_value("0.4", Some("PER"), "esc(3)", U), "40%");
        assert_eq!(format_value("300", Some("MM"), "esc(3)", M), "7620 mm.");
        assert_eq!(format_value("288", Some("FT"), "esc(3)", U), "24 ft.");
        assert_eq!(format_value("1270mm", Some("STR"), "esc(3)", M), "1270 mm.");
        assert_eq!(format_value("4 ft.", Some("STR"), "esc(3)", U), "4 ft.");
        // esc(5): 1 decimal + unit (FURN_M: 70.866 in, U='M').
        assert_eq!(
            format_value("70.86614173228347", Some("M"), "esc(5)", M),
            "1.8 m."
        );
        assert_eq!(
            format_value("31.49606299212599", Some("M"), "esc(5)", M),
            "0.8 m."
        );
        // FURN_U: U='IN_F' renders decimal inches under esc(5).
        assert_eq!(format_value("72", Some("IN_F"), "esc(5)", U), "72.0 in.");
        assert_eq!(format_value("120", Some("IN_F"), "esc(5)", U), "120.0 in.");
        // esc(7): 2 decimals + unit (WADOWI_M: U='MM'/'M').
        assert_eq!(
            format_value("118.1102362204725", Some("MM"), "esc(7)", M),
            "3000.00 mm."
        );
        assert_eq!(
            format_value("118.1102362204724", Some("M"), "esc(7)", M),
            "3.00 m."
        );
        // esc(8): 3 decimals, no unit (#223 anchor; ANNOT_U U='IN').
        assert_eq!(format_value("2", Some("IN"), "esc(8)", U), "2.000");
        // esc(10): feet-inch (WALL_U U='IN_F').
        assert_eq!(format_value("120", Some("IN_F"), "esc(10)", U), "10' 0\"");
    }

    #[test]
    fn corpus_digit_pictures() {
        // `0u`: 0 decimals + unit, NO space (WADOWI_M).
        assert_eq!(format_value("118.1102362204724", Some("M"), "0u", M), "3m.");
        assert_eq!(
            format_value("118.1102362204725", Some("MM"), "0u", M),
            "3000mm."
        );
        assert_eq!(
            format_value("47.24409448818895", Some("MM"), "0u", M),
            "1200mm."
        );
        // `0.00u` (the geometry-stencil quadruples).
        assert_eq!(
            format_value("1.968503937007874", Some("MM"), "0.00u", M),
            "50.00mm."
        );
        assert_eq!(format_value("2", Some("IN"), "0.00u", U), "2.00in.");
        assert_eq!(format_value("0", None, "0", M), "0");
        // A BARE digit picture never unit-converts: Page Number 2 on
        // metric paper stays `2` (Greptile round 1).
        assert_eq!(format_value("2", None, "0", M), "2");
        assert_eq!(format_value("2.5", None, "0.00", M), "2.50");
    }

    #[test]
    fn corpus_date_pins() {
        assert_eq!(
            format_value("2014-05-15T00:00:00", None, "c", U),
            "5/15/2014"
        );
        assert_eq!(
            format_value("2012-07-26T10:23:29", None, "{<200>}", U),
            "7/26/2012"
        );
        assert_eq!(
            format_value("2012-07-18T23:57:59", None, "{<202>}", U),
            "July 18, 2012"
        );
    }

    #[test]
    fn excel_serial_epoch_is_corpus_pinned() {
        // 42005 = 2015-01-01 (the shipped cached rows).
        assert_eq!(format_value("42005", None, "{{MMM yyyy}}", U), "Jan 2015");
        // TIMELN_EXPANDEDBLOCK: 42095/42186/42278 → Apr/Jul/Oct 2015.
        assert_eq!(format_value("42095", None, "{{MMM yyyy}}", U), "Apr 2015");
        assert_eq!(format_value("42186", None, "{{MMM yyyy}}", U), "Jul 2015");
        assert_eq!(format_value("42278", None, "{{MMM yyyy}}", U), "Oct 2015");
        // 41275 = 2013-01-01 (the `{{d MMMM yyyy}}` corpus sample).
        assert_eq!(
            format_value("41275", None, "{{d MMMM yyyy}}", U),
            "1 January 2013"
        );
        // TIMELN's `{{MMM-yy}}` quadruples: 42005/42370/42736 are
        // the Jan-1 anchors of 2015/2016/2017.
        assert_eq!(format_value("42005", None, "{{MMM-yy}}", U), "Jan-15");
        assert_eq!(format_value("42370", None, "{{MMM-yy}}", U), "Jan-16");
        assert_eq!(format_value("42736", None, "{{MMM-yy}}", U), "Jan-17");
        // The `{{d MMMM yyyy}}` V samples (41275 = 2013-01-01) carry
        // no cached marker — epoch-derived, not marker-pinned.
        assert_eq!(
            format_value("41275", None, "{{d MMMM yyyy}}", U),
            "1 January 2013"
        );
    }

    #[test]
    fn text_pictures() {
        assert_eq!(format_value("Asset", Some("STR"), "@", U), "Asset");
        assert_eq!(
            format_value("Microsoft Corporation", Some("STR"), "@+", U),
            "MICROSOFT CORPORATION"
        );
        assert_eq!(
            format_value("Title Blocks", Some("STR"), "@+", U),
            "TITLE BLOCKS"
        );
        // The feet-inch strings under `@` stay verbatim (WALL_U).
        assert_eq!(format_value("8'-0\"", Some("STR"), "@", U), "8'-0\"");
    }

    #[test]
    fn field_picture_table() {
        assert_eq!(field_picture(0), "esc(0)");
        assert_eq!(field_picture(8), "esc(8)");
        assert_eq!(field_picture(17), "esc(17)");
        assert_eq!(field_picture(37), "@");
        assert_eq!(field_picture(39), "@+");
        assert_eq!(field_picture(200), "{<200>}");
        assert_eq!(field_picture(202), "{<202>}");
    }

    #[test]
    fn month_vs_minute_m_runs() {
        // `{{m/d/yy}}` renders the MONTH (CodeRabbit round 1).
        assert_eq!(
            format_value("2014-05-15T00:00:00", None, "{{m/d/yy}}", U),
            "5/15/14"
        );
        // `mm` beside a time token is minutes.
        assert_eq!(
            format_value("2014-05-15T13:07:00", None, "{{HH:mm}}", U),
            "13:07"
        );
    }

    #[test]
    fn malformed_date_prefix_does_not_panic() {
        assert_eq!(format_value("2024-01-1é", None, "c", U), "2024-01-1é");
    }

    #[test]
    fn scientific_notation_parses() {
        assert_eq!(format_value("1.2E-3", Some("IN"), "esc(8)", U), "0.001");
    }

    #[test]
    fn pr_review_fieldfmt_pins() {
        // `C` is the LONG general date; `c` stays short.
        assert_eq!(
            format_value("2014-05-15T00:00:00", None, "C", U),
            "May 15, 2014"
        );
        assert_eq!(
            format_value("2014-05-15T00:00:00", None, "c", U),
            "5/15/2014"
        );
        // Whitespace is content for text pictures and the empty picture.
        assert_eq!(format_value("  A  ", Some("STR"), "@", U), "  A  ");
        assert_eq!(format_value("  A  ", Some("STR"), "", U), "  A  ");
        // Interior space before the unit label (Visio's `0.00 u`).
        assert_eq!(format_value("1", Some("CM"), "0.00 u", U), "2.54 cm.");
        assert_eq!(format_value("1", Some("CM"), "0.00u", U), "2.54cm.");
        // Sign in both mantissa and exponent parses.
        assert_eq!(format_value("-1.2E-3", Some("IN"), "esc(8)", U), "-0.001");
        // Overflowing conversion keeps the value visible (1E308 ×
        // 25.4 overflows f64; the BARE `0.00` never converts).
        assert_eq!(format_value("1E308", Some("MM"), "0.00u", M), "1E308");
        // Invalid calendar dates fall back verbatim.
        assert_eq!(format_value("2024-02-30", None, "c", U), "2024-02-30");
        assert_eq!(format_value("2024-02-29", None, "c", U), "2/29/2024");
        // Pre-1970 serials keep their time fraction (25568.5 =
        // 1969-12-31 noon).
        assert_eq!(
            format_value("25568.5", None, "{{yyyy-MM-dd HH:mm:ss}}", U),
            "1969-12-31 12:00:00"
        );
        // A rounding carry advances the day (42005.999999).
        assert_eq!(
            format_value("42005.999999", None, "{{yyyy-MM-dd HH:mm:ss}}", U),
            "2015-01-02 00:00:00"
        );
        // Feet beyond i64 degrade to the general number, no panic.
        assert_eq!(
            format_value("1E21", Some("IN_F"), "esc(10)", U),
            general_number(1e21)
        );
    }

    #[test]
    fn unparseable_value_falls_back_to_itself() {
        assert_eq!(format_value("hello", Some("STR"), "esc(5)", U), "hello");
        assert_eq!(format_value("not a date", None, "c", U), "not a date");
    }

    /// The `c` picture's failure fallback keeps the ORIGINAL
    /// whitespace — `short_date` used to return its trimmed input
    /// (CodeRabbit PR review).
    #[test]
    fn short_date_failure_fallback_keeps_whitespace() {
        assert_eq!(format_value("  not a date  ", None, "c", U), "  not a date  ");
        // A valid date still formats through the trimmed view.
        assert_eq!(format_value(" 42005 ", None, "c", U), "1/1/2015");
    }

    /// An EXPLICIT unit declaration converts a bare digit picture —
    /// the caller already expressed the value in internal inches for
    /// that display unit (FORMATEX's in/out units; CodeRabbit PR
    /// review). The paper-family DEFAULT still renders as-is.
    #[test]
    fn explicit_unit_converts_bare_digit_pictures() {
        // 12 internal inches displayed in feet.
        assert_eq!(format_value("12", Some("FT"), "0.00", U), "1.00");
        assert_eq!(format_value("1", Some("CM"), "0.00", U), "2.54");
        // `DL`/absent (the paper default) keeps the bare-count rule.
        assert_eq!(format_value("2", Some("DL"), "0", M), "2");
        assert_eq!(format_value("2", None, "0", M), "2");
        // The `u`-suffixed behavior is unchanged.
        assert_eq!(format_value("12", Some("FT"), "0.00u", U), "1.00ft.");
    }
}
