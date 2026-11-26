//! Data ▸ link shapes to an external table (Visio's Link Data to Shapes,
//! the CSV case): read a table, write a row's columns into a shape's
//! Shape Data, link automatically by matching a key column to the
//! shapes' text or a data field, and refresh linked shapes from the
//! re-read file. The source path and key column ride in the document
//! sheet's User rows so Refresh works after reopening.

use dc_core::{Command, Scope, SetRowCell};
use vsdx::model::{Cell, Document, Shape};

/// A read table: header names and rows (each as long as the header).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// RFC 4180 CSV with the separator detected from the header line
/// (`,`, `;` as Excel writes in comma-decimal locales, or tab). Quoted
/// fields may hold separators, quotes (`""`) and line breaks.
pub fn parse_csv(text: &str) -> Table {
    let text = text.trim_start_matches('\u{feff}');
    let head = text.lines().next().unwrap_or("");
    let sep = [';', '\t', ',']
        .into_iter()
        .max_by_key(|c| head.matches(*c).count())
        .filter(|c| head.contains(*c))
        .unwrap_or(',');
    let mut records: Vec<Vec<String>> = Vec::new();
    let (mut rec, mut field, mut quoted) = (Vec::new(), String::new(), false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            (true, '"') => quoted = false,
            (true, c) => field.push(c),
            (false, '"') if field.is_empty() => quoted = true,
            (false, c) if c == sep => rec.push(std::mem::take(&mut field)),
            (false, '\r') => {}
            (false, '\n') => {
                rec.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut rec));
            }
            (false, c) => field.push(c),
        }
    }
    if !field.is_empty() || !rec.is_empty() {
        rec.push(field);
        records.push(rec);
    }
    records.retain(|r| r.iter().any(|f| !f.trim().is_empty()));
    let mut it = records.into_iter();
    let columns: Vec<String> = it.next().unwrap_or_default().into_iter().map(|c| c.trim().to_string()).collect();
    let rows = it
        .map(|mut r| {
            r.resize(columns.len(), String::new());
            r
        })
        .collect();
    Table { columns, rows }
}

/// The first worksheet of an Excel / ODS workbook as a table (first
/// row = headers; numbers without a trailing ".0", dates as shown by
/// calamine's display form).
pub fn read_workbook(path: &std::path::Path) -> anyhow::Result<Table> {
    use calamine::{Data, Reader};
    let mut wb = calamine::open_workbook_auto(path)?;
    let range = wb.worksheet_range_at(0).ok_or_else(|| anyhow::anyhow!("no worksheet"))??;
    let cell = |d: &Data| match d {
        Data::Empty => String::new(),
        Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", *f as i64),
        other => other.to_string(),
    };
    let mut rows = range.rows().map(|r| r.iter().map(cell).collect::<Vec<String>>());
    let columns: Vec<String> = rows.next().unwrap_or_default().into_iter().map(|c| c.trim().to_string()).collect();
    let rows = rows
        .filter(|r| r.iter().any(|v| !v.trim().is_empty()))
        .map(|mut r| {
            r.resize(columns.len(), String::new());
            r
        })
        .collect();
    Ok(Table { columns, rows })
}

/// A table from a file: CSV/TXT as text, anything else as a workbook.
pub fn read_table(path: &std::path::Path) -> anyhow::Result<Table> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if ext == "csv" || ext == "txt" {
        let bytes = std::fs::read(path)?;
        let text = String::from_utf8(bytes.clone()).unwrap_or_else(|_| bytes.iter().map(|&b| b as char).collect());
        Ok(parse_csv(&text))
    } else {
        read_workbook(path)
    }
}

/// What a key value is compared with.
#[derive(Debug, Clone, PartialEq)]
pub enum MatchOn {
    Text,
    /// A Shape Data field, by label.
    Field(String),
}

/// A Prop row name for a column label: letters, digits and `_` (Visio's
/// row-name rule), never empty.
pub fn row_name(label: &str) -> String {
    let s: String = label.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if s.is_empty() { "Column".into() } else { s }
}

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

/// The commands writing `row` of `t` into shape `id`'s Shape Data.
pub fn write_row(t: &Table, row: &[String], page: i64, id: i64) -> Vec<Box<dyn Command>> {
    let scope = Scope::Shape { page, shape: id };
    let mut out: Vec<Box<dyn Command>> = Vec::new();
    for (col, v) in t.columns.iter().zip(row) {
        let name = dc_core::commands::RowSel::Named(row_name(col));
        // Prop row cell names (.vsdx vocabulary)
        const LABEL: &str = "Label";
        for (cell, val) in [(LABEL, col.as_str()), ("Type", "0"), ("Value", v.as_str())] {
            out.push(Box::new(SetRowCell::new(scope.clone(), "Prop", 0, name.clone(), cell, Cell::value(val.to_string()))));
        }
    }
    out
}

/// A shape's value for `on`: its text, or the field with that label.
fn key_of(doc: &Document, page: i64, s: &Shape, on: &MatchOn) -> Option<String> {
    match on {
        MatchOn::Text => s.sheet.text.clone(),
        MatchOn::Field(label) => {
            let chain = dc_core::scope::shape_chain(doc, page, s.id).ok()?;
            let sheet = doc.effective_sheet(s, &chain);
            let hit = sheet.sections_named("Prop").into_iter().flat_map(|sec| sec.rows.iter()).find(|r| {
                r.cell("Label").and_then(|c| c.v.as_deref()).map(norm) == Some(norm(label))
                    || r.n.as_deref().map(norm) == Some(norm(label))
            })?;
            hit.cell("Value").and_then(|c| c.v.clone()).or_else(|| {
                s.sheet
                    .sections_named("Prop")
                    .into_iter()
                    .flat_map(|sec| sec.rows.iter())
                    .find(|r| r.n == hit.n)
                    .and_then(|r| r.cell("Value"))
                    .and_then(|c| c.v.clone())
            })
        }
    }
}

/// Automatically Link: every top-level shape on `pages` whose `on`
/// value equals a row's `key` column gets that row. Returns the
/// commands and the number of shapes linked.
pub fn auto_link(doc: &Document, pages: &[i64], t: &Table, key: usize, on: &MatchOn) -> (Vec<Box<dyn Command>>, usize) {
    let mut cmds = Vec::new();
    let mut n = 0;
    for p in doc.pages.iter().filter(|p| pages.contains(&p.id)) {
        for s in &p.shapes {
            let Some(v) = key_of(doc, p.id, s, on).map(|v| norm(&v)).filter(|v| !v.is_empty()) else { continue };
            if let Some(row) = t.rows.iter().find(|r| r.get(key).map(|k| norm(k)) == Some(v.clone())) {
                cmds.extend(write_row(t, row, p.id, s.id));
                n += 1;
            }
        }
    }
    (cmds, n)
}

/// Refresh: shapes carrying the key column as Shape Data get their
/// row again from the (re-read) table. Returns commands and count.
pub fn refresh(doc: &Document, t: &Table, key: usize) -> (Vec<Box<dyn Command>>, usize) {
    let Some(col) = t.columns.get(key) else { return (vec![], 0) };
    let on = MatchOn::Field(col.clone());
    let pages: Vec<i64> = doc.pages.iter().map(|p| p.id).collect();
    auto_link(doc, &pages, t, key, &on)
}

/// The document sheet's link settings: (source path, key column).
pub fn stored_link(doc: &Document) -> Option<(String, String)> {
    let user = |n: &str| {
        doc.sheet
            .sections_named("User")
            .into_iter()
            .find_map(|s| s.named_row(n))
            .and_then(|r| r.cell("Value"))
            .and_then(|c| c.v.clone())
    };
    Some((user("TarahiDataSource")?, user("TarahiDataKey")?))
}

/// Commands storing the link settings in the document sheet.
pub fn store_link(path: &str, key: &str) -> Vec<Box<dyn Command>> {
    [("TarahiDataSource", path), ("TarahiDataKey", key)]
        .into_iter()
        .map(|(n, v)| {
            Box::new(SetRowCell::new(
                Scope::Document,
                "User",
                0,
                dc_core::commands::RowSel::Named(n.into()),
                "Value",
                Cell::value(v.to_string()),
            )) as Box<dyn Command>
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_quotes_semicolons_and_newlines() {
        let t = parse_csv("\u{feff}Name;IP;Note\r\ncore;10.0.0.1;\"a; b\"\r\nedge;10.0.0.2;\"two\nlines \"\"x\"\"\"\r\n\r\n");
        assert_eq!(t.columns, vec!["Name", "IP", "Note"]);
        assert_eq!(t.rows[0], vec!["core", "10.0.0.1", "a; b"]);
        assert_eq!(t.rows[1][2], "two\nlines \"x\"");
        let c = parse_csv("a,b\n1,2");
        assert_eq!((c.columns.len(), c.rows), (2, vec![vec!["1".to_string(), "2".to_string()]]));
    }

    #[test]
    fn auto_link_by_text_then_refresh_by_key_field() {
        let mut ed = dc_core::Editor::new_blank();
        let page = ed.doc.pages[0].id;
        let mk = |id: i64, text: &str| {
            let mut s = Shape { id, type_: Some("Shape".into()), ..Shape::default() };
            s.sheet.text = Some(text.into());
            s
        };
        ed.doc.pages[0].shapes = vec![mk(1, "core"), mk(2, "Edge "), mk(3, "printer")];
        let t = parse_csv("Name,IP address\ncore,10.0.0.1\nedge,10.0.0.2\n");
        let (cmds, n) = auto_link(&ed.doc, &[page], &t, 0, &MatchOn::Text);
        assert_eq!(n, 2);
        ed.apply(dc_core::Macro::new("link", cmds)).unwrap();
        let ip = |ed: &dc_core::Editor, id: usize| {
            ed.doc.pages[0].shapes[id].sheet.sections_named("Prop")[0].named_row("IP_address").and_then(|r| r.cell("Value")).and_then(|c| c.v.clone())
        };
        assert_eq!(ip(&ed, 1).as_deref(), Some("10.0.0.2"));
        // the file changes: refresh updates through the Name field
        let t2 = parse_csv("Name,IP address\ncore,10.9.9.9\nedge,10.0.0.2\n");
        let (cmds, n) = refresh(&ed.doc, &t2, 0);
        assert_eq!(n, 2);
        ed.apply(dc_core::Macro::new("refresh", cmds)).unwrap();
        assert_eq!(ip(&ed, 0).as_deref(), Some("10.9.9.9"));
    }

    /// A minimal hand-made workbook (one sheet: inline strings and a
    /// number) reads as a table.
    #[test]
    fn reads_an_xlsx_workbook() {
        use std::io::Write;
        let path = std::env::temp_dir().join(format!("tarahi-wb-{}.xlsx", std::process::id()));
        let mut z = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let o = zip::write::SimpleFileOptions::default();
        let parts = [
            ("[Content_Types].xml", r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#),
            ("_rels/.rels", r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#),
            ("xl/workbook.xml", r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#),
            ("xl/_rels/workbook.xml.rels", r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#),
            ("xl/worksheets/sheet1.xml", r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Name</t></is></c><c r="B1" t="inlineStr"><is><t>VLAN</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>core</t></is></c><c r="B2"><v>10</v></c></row></sheetData></worksheet>"#),
        ];
        for (n, x) in parts {
            z.start_file(n, o).unwrap();
            z.write_all(x.as_bytes()).unwrap();
        }
        z.finish().unwrap();
        let t = read_table(&path).unwrap();
        assert_eq!(t.columns, vec!["Name", "VLAN"]);
        assert_eq!(t.rows, vec![vec!["core".to_string(), "10".to_string()]]);
    }

    /// XLSX_CHECK=file: print what a real workbook reads as.
    #[test]
    fn print_real_workbook() {
        let Some(p) = std::env::var_os("XLSX_CHECK") else { return };
        let t = read_table(std::path::Path::new(&p)).unwrap();
        eprintln!("{:?}\n{:?}", t.columns, t.rows);
    }
}
