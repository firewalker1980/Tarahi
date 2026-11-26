//! Review ▸ Shape Reports (Visio's Report Definition, the inventory
//! case): one row per shape that carries Shape Data, a column per data
//! label, saved as CSV. Also Word Count over the shapes' text.

use vsdx::model::{Document, Shape};

pub struct Report {
    pub columns: Vec<String>,
    /// page name, shape name, then one value per column
    pub rows: Vec<(String, String, Vec<String>)>,
}

/// The shapes with Shape Data on `pages` (all pages when empty), their
/// effective (master-inherited) Prop rows by label.
pub fn build(doc: &Document, pages: &[i64]) -> Report {
    let mut columns: Vec<String> = Vec::new();
    let mut rows = Vec::new();
    for p in doc.pages.iter().filter(|p| pages.is_empty() || pages.contains(&p.id)) {
        let mut stack: Vec<&Shape> = p.shapes.iter().rev().collect();
        while let Some(s) = stack.pop() {
            stack.extend(s.sub_shapes.iter().rev());
            let Ok(chain) = dc_core::scope::shape_chain(doc, p.id, s.id) else { continue };
            let sheet = doc.effective_sheet(s, &chain);
            let mut vals: Vec<(String, String)> = Vec::new();
            for sec in sheet.sections_named("Prop") {
                for r in sec.rows.iter().filter(|r| !crate::props::invisible(r)) {
                    let name = r.n.clone().unwrap_or_default();
                    let label = r.cell("Label").and_then(|c| c.v.clone()).filter(|l| !l.is_empty()).unwrap_or(name);
                    let v = r.cell("Value").and_then(|c| c.v.clone()).unwrap_or_default();
                    vals.push((label, v));
                }
            }
            if vals.is_empty() {
                continue;
            }
            for (l, _) in &vals {
                if !columns.contains(l) {
                    columns.push(l.clone());
                }
            }
            // the display Name attribute, else NameU
            let name = s
                .attrs
                .iter()
                .find(|(k, _)| k == "Name")
                .map(|(_, v)| v.clone())
                .or(s.name_u.clone())
                .unwrap_or_else(|| format!("Sheet.{}", s.id));
            rows.push((p.name.clone(), name, vals));
        }
    }
    let rows = rows
        .into_iter()
        .map(|(pg, n, vals)| {
            let cells = columns
                .iter()
                .map(|c| vals.iter().find(|(l, _)| l == c).map(|(_, v)| v.clone()).unwrap_or_default())
                .collect();
            (pg, n, cells)
        })
        .collect();
    Report { columns, rows }
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// RFC 4180 CSV: Page, Shape, then the data columns.
pub fn to_csv(r: &Report, page_h: &str, shape_h: &str) -> String {
    let mut out = String::new();
    let head: Vec<String> = [page_h.to_string(), shape_h.to_string()].into_iter().chain(r.columns.iter().cloned()).collect();
    out += &head.iter().map(|h| csv_field(h)).collect::<Vec<_>>().join(",");
    out += "\r\n";
    for (pg, n, cells) in &r.rows {
        let line: Vec<String> = [pg, n].into_iter().chain(cells.iter()).map(|c| csv_field(c)).collect();
        out += &line.join(",");
        out += "\r\n";
    }
    out
}

/// Words and characters in the shapes' text on `pages` (all when
/// empty).
pub fn word_count(doc: &Document, pages: &[i64]) -> (usize, usize) {
    let (mut words, mut chars) = (0, 0);
    for p in doc.pages.iter().filter(|p| pages.is_empty() || pages.contains(&p.id)) {
        let mut stack: Vec<&Shape> = p.shapes.iter().collect();
        while let Some(s) = stack.pop() {
            stack.extend(s.sub_shapes.iter());
            if let Some(t) = &s.sheet.text {
                words += t.split_whitespace().count();
                chars += t.chars().filter(|c| !c.is_whitespace()).count();
            }
        }
    }
    (words, chars)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vsdx::model::{Cell, Row, Section, SheetChild};

    fn shape(id: i64, name: &str, props: &[(&str, &str)], text: &str) -> Shape {
        let mut s = Shape { id, name_u: Some(name.into()), ..Shape::default() };
        s.sheet.text = Some(text.into());
        if !props.is_empty() {
            s.sheet.children.push(SheetChild::Section(Section {
                name: "Prop".into(),
                rows: props
                    .iter()
                    .map(|(l, v)| Row {
                        n: Some(l.replace(' ', "")),
                        cells: vec![("Label".into(), Cell::value(*l)), ("Value".into(), Cell::value(*v))],
                        ..Row::default()
                    })
                    .collect(),
                ..Section::default()
            }));
        }
        s
    }

    #[test]
    fn report_columns_csv_and_word_count() {
        let mut ed = dc_core::Editor::new_blank();
        ed.doc.pages[0].shapes = vec![
            shape(1, "core", &[("IP address", "10.0.0.1"), ("VLAN", "10")], "core switch"),
            shape(2, "label", &[], "just text here"),
            shape(3, "fw, edge", &[("VLAN", "20")], ""),
        ];
        let r = build(&ed.doc, &[]);
        assert_eq!(r.columns, vec!["IP address", "VLAN"]);
        assert_eq!(r.rows.len(), 2);
        let csv = to_csv(&r, "Page", "Shape");
        let page = &ed.doc.pages[0].name;
        assert_eq!(
            csv,
            format!("Page,Shape,IP address,VLAN\r\n{page},core,10.0.0.1,10\r\n{page},\"fw, edge\",,20\r\n")
        );
        assert_eq!(word_count(&ed.doc, &[]), (5, 22));
    }
}
