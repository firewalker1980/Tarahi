//! Binary Visio (v11) → VSDX package, so the existing XML parser,
//! renderer and writer handle legacy stencils and drawings unchanged.
//!
//! Record layouts (cell offsets, unit codes) were derived by scoring
//! binary slots against Visio's own .vss → .vssx conversions of real
//! stencils (`correlate2.py` in the RE notes); see [`super::cells`].

use super::cells::{self, Enc, CELLS, ROWS, SECTIONS};
use super::{children, f64_at, stream, trailer, u16_at, u32_at, visio_stream, Ptr, Rec};
use anyhow::{bail, Result};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;

const NS: &str = "xmlns='http://schemas.microsoft.com/office/visio/2012/main' \
xmlns:r='http://schemas.openxmlformats.org/officeDocument/2006/relationships' xml:space='preserve'";

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '\'' => o.push_str("&apos;"),
            '"' => o.push_str("&quot;"),
            // XML 1.0 forbids most C0 controls; Visio text uses \r/\n/\t
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            '\u{2028}' | '\u{2029}' => o.push('\n'),
            c => o.push(c),
        }
    }
    o
}

fn utf16(d: &[u8]) -> String {
    super::text(d)
}

/// Format a cell value the way Visio writes V (shortest round-trip).
fn num(v: f64) -> String {
    if v == v.trunc() && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else if v.abs() < 1e-9 || v.abs() >= 1e15 || !v.is_finite() {
        // denormal-ish noise and huge values: exponent form, not
        // hundreds of digits
        if v.is_finite() { format!("{v:E}") } else { "0".into() }
    } else {
        format!("{v}")
    }
}

/// The record data of a stream that holds one record (strip the
/// 4-byte length prefix of compressed streams).
fn rec_data(doc: &[u8], p: &Ptr) -> Vec<u8> {
    let s = stream(doc, p);
    if p.compressed() {
        s.get(4..).unwrap_or(&[]).to_vec()
    } else {
        s
    }
}

/// NameIDX (0xC9): `u32 count`, then 13-byte entries
/// (u32 name slot, u32 universal-name slot, u32 element id, u8 flags).
fn name_idx(d: &[u8]) -> HashMap<u32, (u32, u32)> {
    let n = u32_at(d, 0) as usize;
    (0..n)
        .filter(|k| 4 + 13 * k + 12 <= d.len())
        .map(|k| {
            let p = 4 + 13 * k;
            (u32_at(d, p + 8), (u32_at(d, p), u32_at(d, p + 4)))
        })
        .collect()
}

struct Ctx<'a> {
    doc: &'a [u8],
    palette: Vec<[u8; 3]>,
    /// id of the sheet being written (self references drop `Sheet.N!`)
    cur: u32,
    pool: Vec<String>,
    media: Vec<(String, Vec<u8>)>,
}

impl Ctx<'_> {
    fn name(&self, slot: u32) -> Option<&str> {
        self.pool
            .get(slot as usize)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }
}

/// Convert a legacy binary Visio file (OLE) to the bytes of an
/// equivalent VSDX-family package (.vssx for stencils, .vsdx otherwise).
pub fn to_package(bytes: &[u8]) -> Result<Vec<u8>> {
    let doc = visio_stream(bytes)?;
    let ver = doc[0x1A];
    super::set_v6(ver == 6);
    if ver != 11 && ver != 6 {
        bail!("Visio binary version {ver} is not supported yet (only Visio 2003–2010 files)");
    }
    let tr = trailer(&doc);
    let top = children(&tr, &stream(&doc, &tr));
    let find = |t: u32| top.iter().find(|p| p.typ == t).copied();

    let mut cx = Ctx {
        doc: &doc,
        palette: vec![],
        cur: u32::MAX,
        pool: vec![],
        media: vec![],
    };
    if let Some(p) = find(0x32) {
        let s = stream(&doc, &p);
        cx.pool = children(&p, &s)
            .iter()
            .map(|c| {
                if c.typ == 0 {
                    String::new()
                } else {
                    utf16(rec_data(&doc, c).get(4..).unwrap_or(&[]))
                }
            })
            .collect();
    }

    let mut docxml =
        format!("<?xml version='1.0' encoding='utf-8' ?>\n<VisioDocument {NS}><DocumentSettings/>");
    // Colors (0x16): u8 ?, u8 ?, u8 count, u8 ?, then RGBA quads.
    docxml.push_str("<Colors>");
    if let Some(p) = find(0x16) {
        let d = rec_data(&doc, &p);
        let n = d.get(2).copied().unwrap_or(0) as usize;
        for i in 0..n {
            if let Some(c) = d.get(4 + 4 * i..8 + 4 * i) {
                cx.palette.push([c[0], c[1], c[2]]);
                let _ = write!(
                    docxml,
                    "<ColorEntry IX='{i}' RGB='#{:02X}{:02X}{:02X}'/>",
                    c[0], c[1], c[2]
                );
            }
        }
    }
    docxml.push_str("</Colors><FaceNames>");
    if let Some(p) = find(0xd8) {
        let s = stream(&doc, &p);
        for (i, c) in children(&p, &s).iter().enumerate() {
            if c.typ == 0 {
                continue;
            }
            let name = utf16(rec_data(&doc, c).get(4..).unwrap_or(&[]));
            let _ = write!(
                docxml,
                "<FaceName ID='{i}' NameU='{0}' Name='{0}'/>",
                esc(&name)
            );
        }
    }
    docxml.push_str("</FaceNames><StyleSheets>");
    if let Some(p) = find(0x1a) {
        docxml.push_str(&stylesheets(&mut cx, &p));
    }
    docxml.push_str("</StyleSheets></VisioDocument>");

    let mut masters_xml = format!("<?xml version='1.0' encoding='utf-8' ?>\n<Masters {NS}>");
    let mut master_parts: Vec<(u32, String)> = vec![];
    if let Some(p) = find(0x1d) {
        let s = stream(&doc, &p);
        let kids = children(&p, &s);
        let names = kids
            .iter()
            .find(|k| k.typ == 0xc9)
            .map(|k| name_idx(&rec_data(&doc, k)))
            .unwrap_or_default();
        for (id, m) in kids.iter().enumerate() {
            if m.typ != 0x1e {
                continue;
            }
            let id = id as u32;
            let (nm, nmu) = names.get(&id).copied().map_or((None, None), |(a, b)| {
                (cx.name(a).map(str::to_owned), cx.name(b).map(str::to_owned))
            });
            let nmu = nmu
                .or_else(|| nm.clone())
                .unwrap_or_else(|| format!("Master.{id}"));
            let nm = nm.unwrap_or_else(|| nmu.clone());
            let ms = stream(&doc, m);
            let mk = children(m, &ms);
            let prompt = mk
                .iter()
                .find(|k| k.typ == 0x0a)
                .map(|k| utf16(&rec_data(&doc, k)))
                .unwrap_or_default();
            let (sheet, shapes) =
                contents(&mut cx, m, &format!("master{}", master_parts.len() + 1));
            let n = master_parts.len() + 1;
            let _ = write!(
                masters_xml,
                "<Master ID='{id}' NameU='{}' IsCustomNameU='1' Name='{}' IsCustomName='1' Prompt='{}' IconSize='1' AlignName='2' MatchByName='0' MasterType='2'>{}<Rel r:id='rId{n}'/></Master>",
                esc(&nmu),
                esc(&nm),
                esc(&prompt),
                sheet,
            );
            master_parts.push((id, format!("<?xml version='1.0' encoding='utf-8' ?>\n<MasterContents {NS}><Shapes>{shapes}</Shapes></MasterContents>")));
        }
    }
    masters_xml.push_str("</Masters>");

    // Pages (0x27): page sheet + shapes; stencils carry one empty page.
    let mut pages_xml = format!("<?xml version='1.0' encoding='utf-8' ?>\n<Pages {NS}>");
    let mut page_parts: Vec<String> = vec![];
    if let Some(p) = find(0x27) {
        let s = stream(&doc, &p);
        for (i, pg) in children(&p, &s).iter().enumerate() {
            if pg.typ != 0x15 {
                continue;
            }
            let (sheet, shapes) = contents(&mut cx, pg, &format!("page{}", page_parts.len() + 1));
            let n = page_parts.len() + 1;
            let _ = write!(
                pages_xml,
                "<Page ID='{i}' NameU='Page-{n}' Name='Page-{n}'>{}<Rel r:id='rId{n}'/></Page>",
                sheet
            );
            page_parts.push(format!("<?xml version='1.0' encoding='utf-8' ?>\n<PageContents {NS}><Shapes>{shapes}</Shapes></PageContents>"));
        }
    }
    pages_xml.push_str("</Pages>");

    package(
        &docxml,
        &masters_xml,
        &master_parts,
        &pages_xml,
        &page_parts,
        &cx.media,
    )
}

/// Stylesheets (0x1a): one inline chunk stream; each level-1 0x4a
/// chunk opens a style whose records follow one level deeper.
fn stylesheets(cx: &mut Ctx, p: &Ptr) -> String {
    let s = stream(cx.doc, p);
    let ch = super::chunks(&s);
    let names = ch
        .iter()
        .find(|c| c.typ == 0xc9)
        .map(|c| name_idx(c.data))
        .unwrap_or_default();
    let mut out = String::new();
    let mut i = 0;
    while i < ch.len() {
        if ch[i].typ != 0x4a {
            i += 1;
            continue;
        }
        let id = ch[i].id;
        let head = ch[i].data.to_vec();
        let mut j = i + 1;
        while j < ch.len() && ch[j].level > ch[i].level && ch[j].typ != 0x4a {
            j += 1;
        }
        let recs = super::tree_records(&ch[i + 1..j], ch[i].level + 1);
        let nm = names
            .get(&id)
            .and_then(|&(a, _)| cx.name(a))
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Style.{id}"));
        let (l, f, t) = style_refs(&head);
        let _ = write!(
            out,
            "<StyleSheet ID='{id}' NameU='{0}' IsCustomNameU='1' Name='{0}' IsCustomName='1'{1}>{2}</StyleSheet>",
            esc(&nm),
            style_attrs(id, l, f, t),
            sheet_body(cx, &recs, None)
        );
        i = j;
    }
    out
}

/// Line/fill/text style ids in a shape or style header (u32 each;
/// 0xFFFFFFFF = none).
fn style_refs(head: &[u8]) -> (u32, u32, u32) {
    (
        u32_at(head, cells::HDR_LINE_STYLE),
        u32_at(head, cells::HDR_FILL_STYLE),
        u32_at(head, cells::HDR_TEXT_STYLE),
    )
}

fn style_attrs(self_id: u32, l: u32, f: u32, t: u32) -> String {
    let mut a = String::new();
    for (n, v) in [("LineStyle", l), ("FillStyle", f), ("TextStyle", t)] {
        if v != u32::MAX && v != self_id && v < 100_000 {
            let _ = write!(a, " {n}='{v}'");
        }
    }
    a
}

/// A page or master: (`<PageSheet>` XML, shapes XML).
fn contents(cx: &mut Ctx, container: &Ptr, part: &str) -> (String, String) {
    let all = super::sheets(cx.doc, container);
    let Some((_, _, srecs)) = all.iter().find(|(t, _, _)| *t == 0x46) else {
        return (String::new(), String::new());
    };
    let shapes: HashMap<u32, (u32, Vec<Rec>)> = all
        .iter()
        .filter(|(t, _, _)| *t != 0x46)
        .map(|(t, id, r)| (*id, (*t, r.clone())))
        .collect();
    let names = srecs
        .iter()
        .find(|r| r.typ == 0xc9 && r.list.is_none())
        .map(|r| name_idx(&r.data))
        .unwrap_or_default();
    let top = child_ids(srecs);
    cx.cur = 0;
    let sheet_xml = format!(
        "<PageSheet>{}</PageSheet>",
        sheet_body(cx, srecs, Some(part))
    );
    let mut out = String::new();
    let mut seen = std::collections::HashSet::new();
    for id in top {
        shape_xml(cx, &shapes, &names, id, part, &mut out, &mut seen);
    }
    (sheet_xml, out)
}

/// Child shape ids of a page sheet or group: the 0x83 rows (u32 id,
/// u32 type) of its shape list, in z-order.
fn child_ids(recs: &[Rec]) -> Vec<u32> {
    recs.iter()
        .filter(|r| r.typ == 0x83)
        .map(|r| u32_at(&r.data, 0))
        .collect()
}

fn shape_type(t: u32) -> &'static str {
    match t {
        0x47 => "Group",
        0x4e => "Foreign",
        0x4d => "Guide",
        _ => "Shape",
    }
}

fn shape_xml(
    cx: &mut Ctx,
    shapes: &HashMap<u32, (u32, Vec<Rec>)>,
    names: &HashMap<u32, (u32, u32)>,
    id: u32,
    part: &str,
    out: &mut String,
    seen: &mut std::collections::HashSet<u32>,
) {
    let Some((typ, recs)) = shapes.get(&id) else {
        return;
    };
    if !seen.insert(id) {
        return; // malformed cycle
    }
    let head = recs
        .iter()
        .find(|r| r.id == u32::MAX)
        .map(|r| r.data.clone())
        .unwrap_or_default();
    let (l, f, t) = style_refs(&head);
    let mut attrs = format!("ID='{id}' Type='{}'", shape_type(*typ));
    if let Some(&(n, u)) = names.get(&id) {
        if let Some(nu) = cx.name(u).or_else(|| cx.name(n)) {
            let _ = write!(attrs, " NameU='{}'", esc(nu));
        }
        if let Some(nm) = cx.name(n) {
            let _ = write!(attrs, " Name='{}'", esc(nm));
        }
    }
    attrs.push_str(&style_attrs(u32::MAX, l, f, t));
    cx.cur = id;
    let _ = write!(out, "<Shape {attrs}>{}", sheet_body(cx, recs, Some(part)));
    let kids = child_ids(recs);
    if !kids.is_empty() {
        out.push_str("<Shapes>");
        for k in kids {
            shape_xml(cx, shapes, names, k, part, out, seen);
        }
        out.push_str("</Shapes>");
    }
    out.push_str("</Shape>");
}

/// A cell value read from `d` at `off` with encoding `enc`: (V, U).
fn value(
    pal: &[[u8; 3]],
    d: &[u8],
    off: usize,
    enc: Enc,
) -> Option<(String, Option<&'static str>)> {
    let v = match enc {
        Enc::F64 => {
            if off + 8 > d.len() {
                return None;
            }
            let u = if off > 0 {
                cells::unit(d[off - 1])
            } else {
                None
            };
            return Some((num(f64_at(d, off)), u));
        }
        Enc::Color => {
            let c = d.get(off..off + 4)?;
            let rgb = [c[1], c[2], c[3]];
            return Some((
                if pal.get(c[0] as usize) == Some(&rgb) {
                    c[0].to_string()
                } else {
                    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
                },
                None,
            ));
        }
        Enc::Trans => *d.get(off)? as f64 / 256.0,
        Enc::Num => {
            if off == 0 || off + 8 > d.len() || d[off - 1] == 0xE8 {
                return None;
            }
            return Some((num(f64_at(d, off)), cells::unit(d[off - 1])));
        }
        Enc::U8 => *d.get(off)? as f64,
        Enc::U16 => {
            if off + 2 > d.len() {
                return None;
            }
            u16_at(d, off) as f64
        }
        Enc::Bit(b) => ((*d.get(off)? >> b) & 1) as f64,
    };
    Some((num(v), None))
}

/// Cell names of a record's formula slots (its COM row's cell order).
fn formula_names(typ: u32) -> Option<&'static [&'static str]> {
    use super::formula::{obj_row, section_cells};
    let obj = |r| obj_row(r);
    match typ {
        0x9b => obj(1),
        0x85 => obj(2),
        0x86 => obj(3),
        0x9d => obj(4),
        0x84 => obj(5),
        0x98 => obj(9),
        0x92 => obj(10),
        0x87 => obj(11),
        0x9c => obj(12),
        0xa0 => obj(15),
        0xa3 => obj(16),
        0xa4 => obj(17),
        0xbe => obj(22),
        0xbf => obj(23),
        0x89 => Some(super::ftables::GEOM_HEADER),
        0x8a | 0x8b | 0x8c | 0x8d | 0x8f | 0x90 | 0xa6 | 0xa7 | 0xc1 | 0xc3 => section_cells(10),
        0x94 => section_cells(3),
        0x95 => section_cells(4),
        0x9e => section_cells(6),
        0x99 => section_cells(7),
        0xaa => section_cells(9),
        0xa9 => section_cells(240),
        0xb4 => section_cells(242),
        0xb6 => section_cells(243),
        0xc4 => section_cells(244),
        0xa1 => Some(&["Value", "", "Format"]),
        _ => None,
    }
}

/// Default (unnamed) first cell of a named-row section.
fn first_cell(sec: &str) -> &'static str {
    match sec {
        "User" | "Property" => "Value",
        "Actions" => "Menu",
        "Hyperlink" => "Description",
        "Control" => "X",
        _ => "",
    }
}

/// `"text"` (quotes doubled inside) → `text`; None for anything else.
fn plain_string(f: &str) -> Option<String> {
    let inner = f.strip_prefix('"')?.strip_suffix('"')?;
    let s = inner.replace("\"\"", "\"");
    // a lone quote left over means it was not one literal
    (!inner.replace("\"\"", "").contains('"')).then_some(s)
}

/// One emitted cell: name, value (V, U) and formula.
type CellOut = (
    String,
    Option<(String, Option<&'static str>)>,
    Option<String>,
);

/// Cells of one record: table values merged with decoded formulas.
fn record_cells(
    cx: &Ctx,
    r: &Rec,
    defs: &[(&'static str, usize, Enc)],
    sec: Option<&str>,
) -> Vec<CellOut> {
    let mut out: Vec<CellOut> = defs
        .iter()
        .filter_map(|&(n, off, enc)| {
            value(&cx.palette, &r.data, off, enc).map(|v| (n.to_string(), Some(v), None))
        })
        .collect();
    if let Some(names) = formula_names(r.typ) {
        for (ci, tok) in super::formula::entries(&r.data) {
            let Some(n) = names.get(ci as usize).copied() else {
                continue;
            };
            let n = if n.is_empty() {
                sec.map(first_cell).unwrap_or("")
            } else {
                n
            };
            if n.is_empty() || n.starts_with('_') {
                continue;
            }
            let Some(f) = super::formula::decode(tok, &cx.pool, cx.cur) else {
                continue;
            };
            // a plain string literal in a cell without a stored number is
            // the cell's text value, as Visio writes it (a numeric cell
            // keeps its value and the string stays the formula)
            let has_value = out.iter().any(|c| c.0 == n && c.1.is_some());
            if let (Some(s), false) = (plain_string(&f), has_value) {
                let u = (n == "Value").then_some("STR");
                match out.iter_mut().find(|c| c.0 == n) {
                    Some(c) => c.1 = Some((s, u)),
                    None => out.push((n.to_string(), Some((s, u)), None)),
                }
                continue;
            }
            match out.iter_mut().find(|c| c.0 == n) {
                Some(c) => c.2 = Some(f),
                None => out.push((n.to_string(), None, Some(f))),
            }
        }
    }
    out
}

fn write_cells(out: &mut String, cells: &[CellOut]) {
    for (n, v, f) in cells {
        let _ = write!(out, "<Cell N='{n}'");
        if let Some((v, u)) = v {
            let _ = write!(out, " V='{}'", esc(v));
            if let Some(u) = u {
                let _ = write!(out, " U='{u}'");
            }
        } else if let Some(f) = f
            .as_deref()
            .filter(|f| f.starts_with("POLYLINE(") || f.starts_with("NURBS("))
        {
            // geometry functions are values of their own
            let _ = write!(out, " V='{}'", esc(f));
        }
        if let Some(f) = f {
            let _ = write!(out, " F='{}'", esc(f));
        }
        out.push_str("/>");
    }
}

/// Cells, sections, text and foreign data of one sheet.
fn sheet_body(cx: &mut Ctx, recs: &[Rec], part: Option<&str>) -> String {
    let mut out = String::new();
    // shape-level cells
    for r in recs.iter().filter(|r| r.list.is_none()) {
        let defs: Vec<(&'static str, usize, Enc)> = CELLS
            .iter()
            .filter(|c| c.t == r.typ)
            .map(|c| (c.name, c.off, c.enc))
            .collect();
        let cells = record_cells(cx, r, &defs, None);
        write_cells(&mut out, &cells);
    }
    // sections: group rows by their list
    let mut order: Vec<(u32, u32)> = vec![];
    let mut rows: HashMap<(u32, u32), Vec<&Rec>> = HashMap::new();
    for r in recs.iter() {
        if let Some(l) = r.list {
            if SECTIONS.iter().any(|s| s.0 == l.0) {
                rows.entry(l).or_insert_with(|| {
                    order.push(l);
                    vec![]
                });
                rows.get_mut(&l).unwrap().push(r);
            }
        }
    }
    let mut geom_ix = 0;
    for l in order {
        let (_, sec) = SECTIONS.iter().find(|s| s.0 == l.0).unwrap();
        let named = matches!(*sec, "User" | "Property" | "Actions" | "Hyperlink");
        // row names (NameIDX inside the list): row id → pool slot
        let row_names = rows[&l]
            .iter()
            .find(|r| r.typ == 0xc9)
            .map(|r| name_idx(&r.data))
            .unwrap_or_default();
        let ix = if *sec == "Geometry" {
            geom_ix += 1;
            format!(" IX='{}'", geom_ix - 1)
        } else {
            String::new()
        };
        let _ = write!(out, "<Section N='{sec}'{ix}>");
        for r in &rows[&l] {
            let Some(rd) = ROWS.iter().find(|d| d.t == r.typ) else {
                continue;
            };
            let cells = record_cells(cx, r, rd.cells, Some(sec));
            if rd.header {
                write_cells(&mut out, &cells);
                continue;
            }
            let t = rd.row_t.map_or(String::new(), |t| format!(" T='{t}'"));
            let rname = row_names
                .get(&r.id)
                .and_then(|&(_, u)| cx.name(u))
                .map(str::to_owned);
            match rname {
                Some(n) if named || *sec == "Control" => {
                    let _ = write!(out, "<Row N='{}'{t}>", esc(&n));
                }
                _ => {
                    let _ = write!(out, "<Row{t} IX='{}'>", r.id);
                }
            }
            write_cells(&mut out, &cells);
            out.push_str("</Row>");
        }
        out.push_str("</Section>");
    }
    // text (0x0E): 8 header bytes, then UTF-16
    if let Some(r) = recs.iter().find(|r| r.typ == 0x0e) {
        let txt = utf16(r.data.get(8..).unwrap_or(&[]));
        if !txt.is_empty() {
            let _ = write!(out, "<Text>{}</Text>", text_runs(&txt, recs));
        }
    }
    // foreign data (0x0C) described by the Foreign record (0x98)
    if let (Some(fd), Some(part)) = (recs.iter().find(|r| r.typ == 0x0c), part) {
        if let Some(x) = foreign(
            cx,
            &fd.data,
            recs.iter()
                .find(|r| r.typ == 0x98)
                .map(|r| r.data.as_slice()),
            part,
        ) {
            out.push_str(&x);
        }
    }
    out
}

/// Text with `<cp>`/`<pp>` run markers. Character (0x94) and paragraph
/// (0x95) rows start with a u32 run length in UTF-16 units; 0 or a
/// missing tail means "to the end".
fn text_runs(txt: &str, recs: &[Rec]) -> String {
    let lens = |t: u32| {
        let mut rows: Vec<&Rec> = recs.iter().filter(|r| r.typ == t).collect();
        rows.sort_by_key(|r| r.id);
        rows.iter().map(|r| u32_at(&r.data, 0) as usize).collect::<Vec<_>>()
    };
    // run starts (in UTF-16 units) → marker index
    let starts = |l: Vec<usize>| {
        let mut s = vec![0usize];
        let mut acc = 0;
        for n in l.iter().take(l.len().saturating_sub(1)) {
            if *n == 0 {
                break;
            }
            acc += n;
            s.push(acc);
        }
        s
    };
    let (cps, pps) = (starts(lens(0x94)), starts(lens(0x95)));
    let mut out = String::new();
    let mut pos = 0usize;
    let mut fields = 0;
    let mut buf = [0u16; 2];
    for ch in txt.chars() {
        if let Some(i) = cps.iter().position(|&s| s == pos) {
            let _ = write!(out, "<cp IX='{i}'/>");
        }
        if let Some(i) = pps.iter().position(|&s| s == pos) {
            let _ = write!(out, "<pp IX='{i}'/>");
        }
        if ch == '\u{FFFC}' {
            // field placeholder: the renderer evaluates the Field row
            let _ = write!(out, "<fld IX='{fields}'></fld>");
            fields += 1;
        } else {
            let c = if ch == '\r' { '\n' } else { ch };
            out.push_str(&esc(c.encode_utf8(&mut [0; 4])));
        }
        pos += ch.encode_utf16(&mut buf).len();
    }
    out
}

/// `<ForeignData>` for an embedded picture, its bytes queued as media.
fn foreign(cx: &mut Ctx, data: &[u8], info: Option<&[u8]>, part: &str) -> Option<String> {
    let (ty, ext, bytes): (&str, &str, Vec<u8>) =
        if data.starts_with(&[1, 0, 0, 0]) && data.get(40..44) == Some(b" EMF") {
            ("EnhMetaFile", "emf", data.to_vec())
        } else if data.starts_with(&[1, 0, 9, 0]) || data.starts_with(&[2, 0, 9, 0]) {
            // Visio's own conversions label legacy WMF pictures
            // EnhMetaFile; the renderer plays either under that type
            ("EnhMetaFile", "wmf", data.to_vec())
        } else if data.starts_with(b"\x89PNG") {
            ("Bitmap", "png", data.to_vec())
        } else if data.starts_with(&[0xff, 0xd8]) {
            ("Bitmap", "jpg", data.to_vec())
        } else if u32_at(data, 0) == 40 || u32_at(data, 0) == 124 || u32_at(data, 0) == 108 {
            ("Bitmap", "bmp", dib_to_bmp(data))
        } else {
            return None;
        };
    let (mm, ex, ey) = info.map_or((8, 0, 0), |i| (u16_at(i, 38), u16_at(i, 40), u16_at(i, 42)));
    let n = cx.media.len() + 1;
    let name = format!("visio/media/image{n}.{ext}");
    cx.media.push((format!("{part}|{name}"), bytes));
    let comp = match ext {
        "png" => " CompressionType='PNG'",
        "jpg" => " CompressionType='JPEG'",
        _ => "",
    };
    Some(format!(
        "<ForeignData ForeignType='{ty}'{comp} MappingMode='{mm}' ExtentX='{ex}' ExtentY='{ey}'><Rel r:id='rIdM{n}'/></ForeignData>"
    ))
}

/// A packed DIB with a BITMAPFILEHEADER in front.
fn dib_to_bmp(dib: &[u8]) -> Vec<u8> {
    let hsize = u32_at(dib, 0) as usize;
    let bits = u16_at(dib, 14) as usize;
    let used = u32_at(dib, 32) as usize;
    let colors = if used != 0 {
        used
    } else if bits <= 8 {
        1 << bits
    } else {
        0
    };
    let masks = if u32_at(dib, 16) == 3 && hsize == 40 {
        12
    } else {
        0
    };
    let off = 14 + hsize + masks + 4 * colors;
    let mut b = Vec::with_capacity(14 + dib.len());
    b.extend_from_slice(b"BM");
    b.extend_from_slice(&((14 + dib.len()) as u32).to_le_bytes());
    b.extend_from_slice(&[0; 4]);
    b.extend_from_slice(&(off as u32).to_le_bytes());
    b.extend_from_slice(dib);
    b
}

fn package(
    docxml: &str,
    masters_xml: &str,
    masters: &[(u32, String)],
    pages_xml: &str,
    pages: &[String],
    media: &[(String, Vec<u8>)],
) -> Result<Vec<u8>> {
    let stencil = !masters.is_empty() && pages.iter().all(|p| !p.contains("<Shape "));
    let main = if stencil {
        "application/vnd.ms-visio.stencil.main+xml"
    } else {
        "application/vnd.ms-visio.drawing.main+xml"
    };
    let mut ct = String::from(
        "<?xml version='1.0' encoding='UTF-8' standalone='yes'?>\n<Types xmlns='http://schemas.openxmlformats.org/package/2006/content-types'>\
<Default Extension='rels' ContentType='application/vnd.openxmlformats-package.relationships+xml'/>\
<Default Extension='xml' ContentType='application/xml'/>\
<Default Extension='emf' ContentType='image/x-emf'/><Default Extension='wmf' ContentType='image/x-wmf'/>\
<Default Extension='png' ContentType='image/png'/><Default Extension='jpg' ContentType='image/jpeg'/><Default Extension='bmp' ContentType='image/bmp'/>",
    );
    let _ = write!(
        ct,
        "<Override PartName='/visio/document.xml' ContentType='{main}'/>"
    );
    ct.push_str("<Override PartName='/visio/masters/masters.xml' ContentType='application/vnd.ms-visio.masters+xml'/>");
    for i in 1..=masters.len() {
        let _ = write!(ct, "<Override PartName='/visio/masters/master{i}.xml' ContentType='application/vnd.ms-visio.master+xml'/>");
    }
    ct.push_str("<Override PartName='/visio/pages/pages.xml' ContentType='application/vnd.ms-visio.pages+xml'/>");
    for i in 1..=pages.len() {
        let _ = write!(ct, "<Override PartName='/visio/pages/page{i}.xml' ContentType='application/vnd.ms-visio.page+xml'/>");
    }
    ct.push_str("</Types>");

    const R: &str = "http://schemas.microsoft.com/visio/2010/relationships";
    let rels = |items: &[(String, String, String)]| {
        let mut s = String::from("<?xml version='1.0' encoding='UTF-8' standalone='yes'?>\n<Relationships xmlns='http://schemas.openxmlformats.org/package/2006/relationships'>");
        for (id, ty, tg) in items {
            let _ = write!(s, "<Relationship Id='{id}' Type='{ty}' Target='{tg}'/>");
        }
        s.push_str("</Relationships>");
        s
    };
    let mut files: Vec<(String, Vec<u8>)> = vec![
        ("[Content_Types].xml".into(), ct.into_bytes()),
        (
            "_rels/.rels".into(),
            rels(&[(
                "rId1".into(),
                format!("{R}/document"),
                "visio/document.xml".into(),
            )])
            .into_bytes(),
        ),
        ("visio/document.xml".into(), docxml.as_bytes().to_vec()),
        (
            "visio/_rels/document.xml.rels".into(),
            rels(&[
                (
                    "rId1".into(),
                    format!("{R}/masters"),
                    "masters/masters.xml".into(),
                ),
                (
                    "rId2".into(),
                    format!("{R}/pages"),
                    "pages/pages.xml".into(),
                ),
            ])
            .into_bytes(),
        ),
        (
            "visio/masters/masters.xml".into(),
            masters_xml.as_bytes().to_vec(),
        ),
        (
            "visio/masters/_rels/masters.xml.rels".into(),
            rels(
                &(1..=masters.len())
                    .map(|i| {
                        (
                            format!("rId{i}"),
                            format!("{R}/master"),
                            format!("master{i}.xml"),
                        )
                    })
                    .collect::<Vec<_>>(),
            )
            .into_bytes(),
        ),
        (
            "visio/pages/pages.xml".into(),
            pages_xml.as_bytes().to_vec(),
        ),
        (
            "visio/pages/_rels/pages.xml.rels".into(),
            rels(
                &(1..=pages.len())
                    .map(|i| {
                        (
                            format!("rId{i}"),
                            format!("{R}/page"),
                            format!("page{i}.xml"),
                        )
                    })
                    .collect::<Vec<_>>(),
            )
            .into_bytes(),
        ),
    ];
    // per-part media relationships
    let mut part_media: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
    for (i, (key, bytes)) in media.iter().enumerate() {
        let (part, name) = key.split_once('|').unwrap();
        part_media.entry(part.to_string()).or_default().push((
            format!("rIdM{}", i + 1),
            "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image".into(),
            format!("../{}", name.trim_start_matches("visio/")),
        ));
        files.push((name.to_string(), bytes.clone()));
    }
    for (i, (_, x)) in masters.iter().enumerate() {
        files.push((
            format!("visio/masters/master{}.xml", i + 1),
            x.as_bytes().to_vec(),
        ));
        if let Some(r) = part_media.get(&format!("master{}", i + 1)) {
            files.push((
                format!("visio/masters/_rels/master{}.xml.rels", i + 1),
                rels(r).into_bytes(),
            ));
        }
    }
    for (i, x) in pages.iter().enumerate() {
        files.push((
            format!("visio/pages/page{}.xml", i + 1),
            x.as_bytes().to_vec(),
        ));
        if let Some(r) = part_media.get(&format!("page{}", i + 1)) {
            files.push((
                format!("visio/pages/_rels/page{}.xml.rels", i + 1),
                rels(r).into_bytes(),
            ));
        }
    }
    let mut buf = std::io::Cursor::new(Vec::new());
    let mut z = zip::ZipWriter::new(&mut buf);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (n, b) in files {
        z.start_file(n, opts)?;
        z.write_all(&b)?;
    }
    z.finish()?;
    Ok(buf.into_inner())
}
