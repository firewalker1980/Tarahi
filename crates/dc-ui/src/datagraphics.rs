//! Data ▸ Data Graphics, in forms Visio reads natively: a **text**
//! graphic appends chosen Shape Data fields to each shape's text as
//! live fields (they move and update with the shape), and **colour by
//! value** fills shapes per distinct value of a field, remembered on the
//! page (`User.TarahiColorBy`) for Insert ▸ Legend.

use dc_core::{Command, Scope};
use vsdx::model::{Cell, Document, Shape};

/// Ten clearly distinct fills (value order).
pub const PALETTE: [&str; 10] = [
    "#4E79A7", "#F28E2B", "#59A14F", "#E15759", "#76B7B2", "#EDC948", "#B07AA1", "#FF9DA7", "#9C755F", "#BAB0AC",
];

/// A shape's effective Shape Data as (row name, label, value).
pub fn fields(doc: &Document, page: i64, s: &Shape) -> Vec<(String, String, String)> {
    let Ok(chain) = dc_core::scope::shape_chain(doc, page, s.id) else { return vec![] };
    let own = |n: &str| {
        s.sheet
            .sections_named("Prop")
            .into_iter()
            .flat_map(|sec| sec.rows.iter())
            .find(|r| r.n.as_deref() == Some(n))
            .and_then(|r| r.cell("Value"))
            .and_then(|c| c.v.clone())
    };
    let sheet = doc.effective_sheet(s, &chain);
    let mut out: Vec<(String, String, String)> = Vec::new();
    for sec in sheet.sections_named("Prop") {
        for r in sec.rows.iter().filter(|r| !crate::props::invisible(r)) {
            let n = r.n.clone().unwrap_or_default();
            let label = crate::props::label(r, &n);
            let v = own(&n).or_else(|| r.cell("Value").and_then(|c| c.v.clone())).unwrap_or_default();
            out.push((n, label, v));
        }
    }
    // rows only on the instance (added in Tarahi)
    for sec in s.sheet.sections_named("Prop") {
        for r in &sec.rows {
            let n = r.n.clone().unwrap_or_default();
            if !out.iter().any(|(m, ..)| *m == n) && !crate::props::invisible(r) {
                let label = crate::props::label(r, &n);
                out.push((n, label, r.cell("Value").and_then(|c| c.v.clone()).unwrap_or_default()));
            }
        }
    }
    out
}

/// Text graphic: commands appending, per chosen label, a line holding
/// that field to shape `id`'s text.
pub fn text_cmds(doc: &Document, page: i64, s: &Shape, labels: &[String]) -> Vec<Box<dyn Command>> {
    let scope = Scope::Shape { page, shape: s.id };
    let have = fields(doc, page, s);
    let mut cmds: Vec<Box<dyn Command>> = Vec::new();
    let mut nonempty = s.sheet.text.as_deref().is_some_and(|t| !t.is_empty());
    for l in labels {
        let Some((name, _, v)) = have.iter().find(|(_, lab, _)| lab == l) else { continue };
        if nonempty {
            let len = dc::spellcheck::effective_raw(doc, page, s.id)
                .map_or(0, |raw| vsdx::text::text_pieces(&raw).iter().map(|p| p.plain().chars().count()).sum());
            // the newline goes at the end of what is there so far
            cmds.push(Box::new(EndNewline { scope: scope.clone(), at: len, inner: None }));
        }
        let mut f = dc_core::InsertField::new(scope.clone(), format!("Prop.{name}"), dc_core::FieldInsert::End);
        f.display = Some(v.clone());
        cmds.push(Box::new(f));
        nonempty = true;
    }
    cmds
}

/// A newline appended at the then-current end of the text (computed
/// when it runs, after the earlier fields in the same step).
struct EndNewline {
    scope: Scope,
    at: usize,
    inner: Option<dc_core::ReplaceTextRange>,
}

impl Command for EndNewline {
    fn label(&self) -> String {
        "data graphic line".into()
    }
    fn apply(&mut self, doc: &mut Document) -> anyhow::Result<()> {
        if self.inner.is_none() {
            let Scope::Shape { page, shape } = self.scope else { anyhow::bail!("shape scope") };
            let len = dc::spellcheck::effective_raw(doc, page, shape)
                .map_or(self.at, |raw| vsdx::text::text_pieces(&raw).iter().map(|p| p.plain().chars().count()).sum());
            self.inner = Some(dc_core::ReplaceTextRange::new(self.scope.clone(), len, 0, "\n"));
        }
        self.inner.as_mut().unwrap().apply(doc)
    }
    fn undo(&mut self, doc: &mut Document) -> anyhow::Result<()> {
        match &mut self.inner {
            Some(c) => c.undo(doc),
            None => Ok(()),
        }
    }
}

/// Colour by value: per shape on `page` carrying `label`, its fill set
/// to the value's palette colour; the distinct values in order.
pub fn color_cmds(doc: &Document, page: i64, label: &str) -> (Vec<Box<dyn Command>>, Vec<(String, &'static str)>) {
    let Some(p) = doc.pages.iter().find(|p| p.id == page) else { return (vec![], vec![]) };
    let mut vals: Vec<String> = Vec::new();
    let mut hits: Vec<(i64, String)> = Vec::new();
    for s in &p.shapes {
        if let Some((_, _, v)) = fields(doc, page, s).into_iter().find(|(_, l, _)| l == label) {
            if v.trim().is_empty() {
                continue;
            }
            if !vals.contains(&v) {
                vals.push(v.clone());
            }
            hits.push((s.id, v));
        }
    }
    vals.sort_by(|a, b| match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(x), Ok(y)) => x.total_cmp(&y),
        _ => a.cmp(b),
    });
    let map: Vec<(String, &'static str)> = vals.iter().enumerate().map(|(i, v)| (v.clone(), PALETTE[i % PALETTE.len()])).collect();
    let mut cmds: Vec<Box<dyn Command>> = Vec::new();
    for (id, v) in hits {
        let c = map.iter().find(|(x, _)| *x == v).map(|(_, c)| *c).unwrap_or(PALETTE[0]);
        let scope = Scope::Shape { page, shape: id };
        cmds.push(Box::new(dc_core::SetCell::new(scope.clone(), "FillForegnd", Cell::value(c))));
        cmds.push(Box::new(dc_core::SetCell::new(scope, "FillPattern", Cell::value("1"))));
    }
    cmds.push(Box::new(dc_core::SetRowCell::new(
        Scope::PageSheet { page },
        "User",
        0,
        dc_core::commands::RowSel::Named("TarahiColorBy".into()),
        "Value",
        Cell::value(label.to_string()),
    )));
    (cmds, map)
}

/// The page's colour-by-value field, if one was applied.
pub fn color_by(doc: &Document, page: i64) -> Option<String> {
    doc.pages
        .iter()
        .find(|p| p.id == page)?
        .sheet()?
        .sections_named("User")
        .into_iter()
        .find_map(|s| s.named_row("TarahiColorBy"))
        .and_then(|r| r.cell("Value"))
        .and_then(|c| c.v.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(id: i64, vlan: &str) -> Shape {
        let mut s = Shape { id, type_: Some("Shape".into()), ..Shape::default() };
        s.sheet.text = Some(format!("sw{id}"));
        s.sheet.text_raw = Some(format!("sw{id}"));
        s.sheet.children.push(vsdx::model::SheetChild::Section(vsdx::model::Section {
            name: "Prop".into(),
            rows: vec![vsdx::model::Row {
                n: Some("VLAN".into()),
                cells: vec![("Label".into(), Cell::value("VLAN")), ("Value".into(), Cell::value(vlan))],
                ..Default::default()
            }],
            ..Default::default()
        }));
        s
    }

    #[test]
    fn color_by_value_and_text_fields() {
        let mut ed = dc_core::Editor::new_blank();
        let page = ed.doc.pages[0].id;
        ed.doc.pages[0].shapes = vec![shape(1, "20"), shape(2, "10"), shape(3, "20")];
        let (cmds, map) = color_cmds(&ed.doc, page, "VLAN");
        assert_eq!(map.iter().map(|(v, _)| v.as_str()).collect::<Vec<_>>(), vec!["10", "20"], "numeric order");
        ed.apply(dc_core::Macro::new("color", cmds)).unwrap();
        let fill = |ed: &dc_core::Editor, i: usize| ed.doc.pages[0].shapes[i].sheet.cell("FillForegnd").and_then(|c| c.v.clone());
        assert_eq!(fill(&ed, 0), fill(&ed, 2));
        assert_ne!(fill(&ed, 0), fill(&ed, 1));
        assert_eq!(color_by(&ed.doc, page).as_deref(), Some("VLAN"));

        let s = ed.doc.pages[0].shapes[0].clone();
        let cmds = text_cmds(&ed.doc, page, &s, &["VLAN".to_string()]);
        ed.apply(dc_core::Macro::new("text", cmds)).unwrap();
        let raw = ed.doc.pages[0].shapes[0].sheet.text_raw.clone().unwrap_or_default();
        assert!(raw.starts_with("sw1\n") && raw.contains("<fld"), "{raw}");
    }
}
