//! Binary formula decoding (#36): the compiled RPN token stream of a
//! v11 cell formula back to Visio's universal formula text.
//!
//! Formula entries trail a record: `u32 len, 0x02, u8 cell, tokens, 0xFE`.
//! Tokens (all little-endian):
//! - `0x20..=0x5F` + f64: number; the byte is its unit code (0x20 none),
//!   the value is in internal units (inches, radians, days)
//! - `0x62` u16 integer; `0x60` u8 n + n UTF-16 units + NUL: string
//! - `0x75` u16 cell, u8, u8: object cell of this sheet; `0x74` the same
//!   plus u32 sheet id (0 = the page)
//! - `0x72` u32 row, u8 section, u8 cell, u8 flags: section cell; `0x70`
//!   the same preceded by u32 sheet id. Named sections (User, Prop,
//!   Controls, Actions, Hyperlink) store the row name's string-pool slot.
//! - operators as in spreadsheets: 0x03 + 0x04 - 0x05 * 0x06 / 0x07 ^
//!   0x08 & 0x09 < 0x0A <= 0x0B = 0x0C >= 0x0D > 0x0E <>, 0x13 negate,
//!   0x12 unary plus, 0x14 percent; 0xE4 parentheses; 0xF0 padding
//! - `0x80`/`0x7A` u16 fn, u16: fixed-arity call; `0x81` u32 argc, u16 fn, u16
//! - IF: `0xA2`/`0xA0`/`0xA3` + u32 jump, `0x93` else marker, `0x90` end
//! - compact geometry: `0x8B` POLYLINE (u8 xType, u8 yType, u32 n, n × x,y)
//!   and `0x8A` NURBS (f64 last knot, u16 degree, u8 xType, u8 yType,
//!   u32 n, n × x,y,knot,weight)

use super::ftables::{FUNCS, GEOM_HEADER, OBJ_CELLS, OBJ_ROWS, SECTION_CELLS, UNITS};
use super::{f64_at, u16_at, u32_at};

/// Formula entries of one record: (cell index, token bytes without 0xFE).
pub fn entries(d: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = vec![];
    let mut p = 0;
    while p + 7 <= d.len() {
        let len = u32_at(d, p) as usize;
        if len >= 7 && p + len <= d.len() && d[p + 4] == 2 && d[p + len - 1] == 0xFE {
            out.push((d[p + 5], &d[p + 6..p + len - 1]));
            p += len;
        } else {
            p += 1;
        }
    }
    out
}

/// Visio's number text: 14 significant digits, no trailing zeros,
/// exponent form (`1.5E-5`) outside 1e-4..1e15. Visio rounds twice:
/// to 16 significant digits, then half-up to 14 (0.95622803125167499
/// shows as ...68, 0.79562897708449465 as ...49).
pub fn num(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return "0".into();
    }
    let s16 = format!("{:.15e}", v);
    let (m, e) = s16.split_once('e').unwrap();
    let e: i32 = e.parse().unwrap_or(0);
    let neg = m.starts_with('-');
    let digits: Vec<u8> = m
        .bytes()
        .filter(|b| b.is_ascii_digit())
        .map(|b| b - b'0')
        .collect();
    let mut d14: Vec<u8> = digits[..14].to_vec();
    let mut exp = e;
    if digits[14] >= 5 {
        let mut i = 13;
        loop {
            if d14[i] < 9 {
                d14[i] += 1;
                break;
            }
            d14[i] = 0;
            if i == 0 {
                d14.insert(0, 1);
                d14.pop();
                exp += 1;
                break;
            }
            i -= 1;
        }
    }
    while d14.len() > 1 && *d14.last().unwrap() == 0 {
        d14.pop();
    }
    let ds: String = d14.iter().map(|d| (b'0' + d) as char).collect();
    let sign = if neg { "-" } else { "" };
    if !(-4..15).contains(&exp) {
        let (a, b) = ds.split_at(1);
        let m = if b.is_empty() {
            a.to_string()
        } else {
            format!("{a}.{b}")
        };
        return format!("{sign}{m}E{}{}", if exp < 0 { '-' } else { '+' }, exp.abs());
    }
    if exp < 0 {
        format!("{sign}0.{}{ds}", "0".repeat((-exp - 1) as usize))
    } else if (exp as usize) + 1 >= ds.len() {
        format!("{sign}{ds}{}", "0".repeat(exp as usize + 1 - ds.len()))
    } else {
        let (a, b) = ds.split_at(exp as usize + 1);
        format!("{sign}{a}.{b}")
    }
}

fn obj_cell(id: u16) -> Option<&'static str> {
    OBJ_CELLS.iter().find(|c| c.0 == id).map(|c| c.1)
}

/// The cell names of a COM object-section row.
pub fn obj_row(row: u16) -> Option<&'static [&'static str]> {
    OBJ_ROWS.iter().find(|r| r.0 == row).map(|r| r.1)
}

const GEOM_ROW: &[&str] = &["X", "Y", "A", "B", "C", "D", "E"];

/// Cell names of a (non-object) section's rows by COM cell index.
pub fn section_cells(sec: u16) -> Option<&'static [&'static str]> {
    if (10..10 + 200).contains(&sec) && !(240..=249).contains(&sec) {
        return Some(GEOM_ROW);
    }
    SECTION_CELLS.iter().find(|s| s.0 == sec).map(|s| s.1)
}

fn section_prefix(sec: u16) -> Option<String> {
    Some(match sec {
        3 => "Char".into(),
        4 => "Para".into(),
        5 => "Tabs".into(),
        6 => "Scratch".into(),
        7 => "Connections".into(),
        9 => "Controls".into(),
        240 => "Actions".into(),
        241 => "Layers".into(),
        242 => "User".into(),
        243 => "Prop".into(),
        244 => "Hyperlink".into(),
        245 => "Reviewer".into(),
        248 => "LineGradientStops".into(),
        249 => "FillGradientStops".into(),
        s if (10..240).contains(&s) => format!("Geometry{}", s - 9),
        _ => return None,
    })
}

/// The universal name of section `sec`, row `row`, cell `cell`.
/// `pool` resolves named rows.
pub fn section_cell_name(sec: u16, row: u32, cell: u8, pool: &[String]) -> Option<String> {
    if sec == 1 {
        return obj_row(row as u16)?
            .get(cell as usize)
            .filter(|n| !n.is_empty())
            .map(|n| n.to_string());
    }
    let prefix = section_prefix(sec)?;
    let c = cell as usize;
    match sec {
        9 | 240 | 242 | 243 | 244 => {
            let rname = pool.get(row as usize).filter(|s| !s.is_empty())?;
            let cn = *section_cells(sec)?.get(c)?;
            Some(if c == 0 {
                format!("{prefix}.{rname}")
            } else {
                format!("{prefix}.{rname}.{cn}")
            })
        }
        s if (10..240).contains(&s) => {
            if row == 0 {
                Some(format!("{prefix}.{}", GEOM_HEADER.get(c)?))
            } else {
                Some(format!("{prefix}.{}{row}", GEOM_ROW.get(c)?))
            }
        }
        6 => Some(format!("{prefix}.{}{}", section_cells(6)?.get(c)?, row + 1)),
        7 if c < 2 => Some(format!("{prefix}.{}{}", section_cells(7)?.get(c)?, row + 1)),
        _ => {
            let cn = *section_cells(sec)?.get(c)?;
            if cn.is_empty() || cn.starts_with('_') {
                return None;
            }
            Some(if row == 0 {
                format!("{prefix}.{cn}")
            } else {
                format!("{prefix}.{cn}[{}]", row + 1)
            })
        }
    }
}

fn sheet_prefix(id: u32, own: u32) -> String {
    if id == own {
        String::new()
    } else if id == 0 {
        "ThePage!".into()
    } else {
        format!("Sheet.{id}!")
    }
}

/// Decode a token stream to formula text; None when it holds a token,
/// function or reference this decoder does not know.
/// `own` is the id of the sheet holding the formula: references to it
/// are written without a `Sheet.N!` prefix, as Visio does.
pub fn decode(t: &[u8], pool: &[String], own: u32) -> Option<String> {
    let mut st: Vec<String> = vec![];
    let mut p = 0;
    let mut ifs: Vec<bool> = vec![]; // per open IF: saw the else marker
    while p < t.len() {
        let b = t[p];
        match b {
            0xFE => break,
            0x20..=0x5F => {
                let v = f64_at(t, p + 1);
                st.push(if b == 0x20 {
                    num(v)
                } else {
                    let (_, name, f) = UNITS.iter().find(|u| u.0 == b)?;
                    format!("{}{name}", num(v / f))
                });
                p += 9;
            }
            0x62 => {
                st.push(u16_at(t, p + 1).to_string());
                p += 3;
            }
            0x60 => {
                let n = *t.get(p + 1)? as usize;
                let s = t.get(p + 2..p + 2 + 2 * n)?;
                let units: Vec<u16> = s
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes(*c))
                    .collect();
                st.push(format!("\"{}\"", String::from_utf16_lossy(&units)));
                p += 2 + 2 * n + 2;
            }
            0x75 => {
                st.push(obj_cell(u16_at(t, p + 1))?.to_string());
                p += 5;
            }
            0x74 => {
                st.push(format!(
                    "{}{}",
                    sheet_prefix(u32_at(t, p + 5), own),
                    obj_cell(u16_at(t, p + 1))?
                ));
                p += 9;
            }
            0x72 => {
                st.push(section_cell_name(
                    *t.get(p + 5)? as u16,
                    u32_at(t, p + 1),
                    *t.get(p + 6)?,
                    pool,
                )?);
                p += 8;
            }
            0x70 => {
                let n = section_cell_name(
                    *t.get(p + 9)? as u16,
                    u32_at(t, p + 5),
                    *t.get(p + 10)?,
                    pool,
                )?;
                st.push(format!("{}{n}", sheet_prefix(u32_at(t, p + 1), own)));
                p += 12;
            }
            0x03..=0x0E => {
                let r = st.pop()?;
                let l = st.pop()?;
                let op = [
                    "+", "-", "*", "/", "^", "&", "<", "<=", "=", ">=", ">", "<>",
                ][(b - 3) as usize];
                st.push(format!("{l}{op}{r}"));
                p += 1;
            }
            0x12 => p += 1,
            0x13 => {
                let a = st.pop()?;
                st.push(format!("-{a}"));
                p += 1;
            }
            0x14 => {
                let a = st.pop()?;
                st.push(format!("{a}%"));
                p += 1;
            }
            0xE4 => {
                let a = st.pop()?;
                st.push(format!("({a})"));
                p += 1;
            }
            0xF0 => p += 1,
            0x8B => {
                let n = u32_at(t, p + 3) as usize;
                let body = t.get(p + 7..p + 7 + 16 * n)?;
                let pts: Vec<String> = body
                    .as_chunks::<16>()
                    .0
                    .iter()
                    .map(|c| format!("{},{}", num(f64_at(c, 0)), num(f64_at(c, 8))))
                    .collect();
                let mut s = format!("POLYLINE({}, {}", t[p + 1], t[p + 2]);
                for q in pts {
                    s.push_str(", ");
                    s.push_str(&q);
                }
                st.push(s + ")");
                p += 7 + 16 * n;
            }
            0x8A => {
                let n = u32_at(t, p + 13) as usize;
                let body = t.get(p + 17..p + 17 + 32 * n)?;
                let mut s = format!(
                    "NURBS({}, {}, {}, {}",
                    num(f64_at(t, p + 1)),
                    u16_at(t, p + 9),
                    t[p + 11],
                    t[p + 12]
                );
                for c in body.as_chunks::<32>().0.iter() {
                    let _ = std::fmt::Write::write_fmt(
                        &mut s,
                        format_args!(
                            ", {},{},{},{}",
                            num(f64_at(c, 0)),
                            num(f64_at(c, 8)),
                            num(f64_at(c, 16)),
                            num(f64_at(c, 24))
                        ),
                    );
                }
                st.push(s + ")");
                p += 17 + 32 * n;
            }
            0x80 | 0x7A | 0x81 => {
                let (argc, fid, size) = if b == 0x81 {
                    (u32_at(t, p + 1) as usize, u16_at(t, p + 5), 9)
                } else {
                    let fid = u16_at(t, p + 1);
                    let (_, a) = FUNCS.get(fid as usize)?;
                    if *a < 0 {
                        return None;
                    }
                    (*a as usize, fid, 5)
                };
                let (name, _) = FUNCS.get(fid as usize)?;
                if name.is_empty() || st.len() < argc {
                    return None;
                }
                let args = st.split_off(st.len() - argc);
                st.push(format!("{name}({})", args.join(",")));
                p += size;
            }
            0xA2 => {
                ifs.push(false);
                p += 5;
            }
            0xA0 | 0xA3 => p += 5,
            0x93 => {
                *ifs.last_mut()? = true;
                p += 1;
            }
            0x90 => {
                let n = if ifs.pop()? { 3 } else { 2 };
                if st.len() < n {
                    return None;
                }
                let args = st.split_off(st.len() - n);
                st.push(format!("IF({})", args.join(",")));
                p += 1;
            }
            _ => return None,
        }
    }
    if st.len() == 1 {
        st.pop()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect()
    }

    /// Token streams observed in real stencils, with Visio's own text.
    #[test]
    fn decodes_observed_formulas() {
        let pool: Vec<String> = (0..8).map(|i| format!("Row_{i}")).collect();
        for (tok, want) in [
            ("75 02 00 00 60 20 00 00 00 00 00 00 e0 3f 05", "Width*0.5"),
            ("75 06 00 00 60 13", "-Angle"),
            ("74 03 00 00 60 05 00 00 00 62 a4 06 05", "Sheet.5!Height*1700"),
            ("72 01 00 00 00 0a 01 c0", "Geometry1.Y1"),
            ("75 0a 00 00 60 75 0c 00 00 60 03 e4 62 02 00 06", "(BeginX+EndX)/2"),
            ("75 4c 00 00 60 81 01 00 00 00 2d 00 00 40 75 02 00 00 60 81 02 00 00 00 09 00 00 40", "MAX(TEXTWIDTH(TheText),Width)"),
            ("41 00 00 00 00 00 00 1c 40 80 41 00 00 40", "GUARD(7IN)"),
            (
                "75 07 00 00 60 75 08 00 00 60 80 22 00 00 40 a2 1d 00 00 00 a0 0d 00 00 00 62 01 00 a3 10 00 00 00 93 20 00 00 00 00 00 00 f0 bf 90 75 06 00 00 60 05",
                "IF(BITXOR(FlipX,FlipY),1,-1)*Angle",
            ),
            ("72 06 00 00 00 09 01 c0", "Controls.Row_6.Y"),
            (
                "8b 00 00 02 00 00 00 00 00 00 00 00 00 f0 3f 00 00 00 00 00 00 e0 3f 00 00 00 00 00 00 00 00 00 00 00 00 00 00 f0 3f",
                "POLYLINE(0, 0, 1,0.5, 0,1)",
            ),
            ("60 03 61 00 62 00 63 00 00 00", "\"abc\""),
            ("74 52 00 00 60 00 00 00 00", "ThePage!PageScale"),
            (
                "62 00 00 62 00 00 62 01 00 62 00 00 62 01 00 62 01 00 62 00 00 62 01 00 62 00 00 62 00 00 81 0a 00 00 00 9f 00 00 40",
                "POLYLINE(0,0,1,0,1,1,0,1,0,0)",
            ),
        ] {
            assert_eq!(decode(&hex(tok), &pool, u32::MAX).as_deref(), Some(want), "{tok}");
        }
    }

    #[test]
    fn number_text() {
        assert_eq!(num(0.92857142857142860315), "0.92857142857143");
        assert_eq!(num(26.385557218336), "26.385557218336");
        assert_eq!(num(1.0), "1");
        assert_eq!(num(-0.25), "-0.25");
        assert_eq!(num(3.1682737811678e-5), "3.1682737811678E-5");
        assert_eq!(num(1700.0), "1700");
        assert_eq!(num(0.95622803125167499), "0.95622803125168");
        assert_eq!(num(0.79562897708449465), "0.79562897708449");
    }
}
