//! Home ▸ Tools drawing tools (Visio's Rectangle, Ellipse, Line and
//! Text): the shape a drag on the canvas creates. Geometry rows carry
//! Visio's own size formulas (`Width*1`, `Height*0.5`, …) so the shape
//! resizes like one drawn in Visio.

use vsdx::model::{Cell, Row, Section, Shape, SheetChild};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Rect,
    Ellipse,
    Line,
    Text,
}

/// Smallest drawn size; a click (no drag) gives the default size.
const MIN: f64 = 0.05;
/// Visio's click-to-place text box.
pub const TEXT_DEFAULT: (f64, f64) = (1.0, 0.25);

fn f(v: f64) -> String {
    format!("{v:.10}")
}

fn cell(v: f64, formula: Option<&str>) -> Cell {
    Cell { v: Some(f(v)), f: formula.map(String::from), ..Cell::default() }
}

fn row(t: &str, ix: i64, cells: &[(&str, f64, &str)]) -> Row {
    Row {
        t: Some(t.into()),
        ix,
        cells: cells.iter().map(|(n, v, fx)| (n.to_string(), cell(*v, Some(fx)))).collect(),
        ..Row::default()
    }
}

/// The shape for a drag from `a` to `b` (page inches, y-up).
pub fn build(kind: Kind, id: i64, a: (f64, f64), b: (f64, f64)) -> Shape {
    let name = match kind {
        Kind::Rect => "Rectangle",
        Kind::Ellipse => "Ellipse",
        Kind::Line => "Line",
        Kind::Text => "Sheet",
    };
    let mut s = Shape {
        id,
        type_: Some("Shape".into()),
        name_u: Some(format!("{name}.{id}")),
        ..Shape::default()
    };
    if kind == Kind::Line {
        return line(s, a, b);
    }
    let (x0, y0) = (a.0.min(b.0), a.1.min(b.1));
    let (w, h) = ((a.0 - b.0).abs().max(MIN), (a.1 - b.1).abs().max(MIN));
    let set = |s: &mut Shape, n: &str, v: f64, fx: Option<&str>| {
        s.sheet.children.push(SheetChild::Cell(n.into(), cell(v, fx)));
    };
    set(&mut s, "PinX", x0 + w / 2.0, None);
    set(&mut s, "PinY", y0 + h / 2.0, None);
    set(&mut s, "Width", w, None);
    set(&mut s, "Height", h, None);
    set(&mut s, "LocPinX", w / 2.0, Some("Width*0.5"));
    set(&mut s, "LocPinY", h / 2.0, Some("Height*0.5"));
    set(&mut s, "Angle", 0.0, None);
    if kind == Kind::Text {
        // a text box: no fill, no line (Visio's Text tool)
        set(&mut s, "FillPattern", 0.0, None);
        set(&mut s, "LinePattern", 0.0, None);
    }
    let mut geo = Section { name: "Geometry".into(), ix: 0, ..Section::default() };
    for n in ["NoFill", "NoLine", "NoShow", "NoSnap"] {
        geo.cells.push((n.into(), Cell::value("0")));
    }
    geo.rows = match kind {
        Kind::Ellipse => vec![row(
            "Ellipse",
            1,
            &[
                ("X", w / 2.0, "Width*0.5"),
                ("Y", h / 2.0, "Height*0.5"),
                ("A", w, "Width*1"),
                ("B", h / 2.0, "Height*0.5"),
                ("C", w / 2.0, "Width*0.5"),
                ("D", h, "Height*1"),
            ],
        )],
        _ => vec![
            row("MoveTo", 1, &[("X", 0.0, "Width*0"), ("Y", 0.0, "Height*0")]),
            row("LineTo", 2, &[("X", w, "Width*1"), ("Y", 0.0, "Height*0")]),
            row("LineTo", 3, &[("X", w, "Width*1"), ("Y", h, "Height*1")]),
            row("LineTo", 4, &[("X", 0.0, "Width*0"), ("Y", h, "Height*1")]),
            row("LineTo", 5, &[("X", 0.0, "Geometry1.X1"), ("Y", 0.0, "Geometry1.Y1")]),
        ],
    };
    s.sheet.children.push(SheetChild::Section(geo));
    s
}

/// A 1-D line from `a` to `b`: Begin/End cells, the pin at the middle,
/// Width = length, Angle along the line (Visio's Line tool).
fn line(mut s: Shape, a: (f64, f64), b: (f64, f64)) -> Shape {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx.hypot(dy).max(MIN);
    let ang = dy.atan2(dx);
    for (n, v) in [
        ("BeginX", a.0),
        ("BeginY", a.1),
        ("EndX", b.0),
        ("EndY", b.1),
        ("PinX", (a.0 + b.0) / 2.0),
        ("PinY", (a.1 + b.1) / 2.0),
        ("Width", len),
        ("Height", 0.0),
        ("LocPinX", len / 2.0),
        ("LocPinY", 0.0),
        ("Angle", ang),
        ("ObjType", 2.0),
    ] {
        s.sheet.children.push(SheetChild::Cell(n.into(), cell(v, None)));
    }
    let mut geo = Section { name: "Geometry".into(), ix: 0, ..Section::default() };
    for (n, v) in [("NoFill", "1"), ("NoLine", "0"), ("NoShow", "0"), ("NoSnap", "0")] {
        geo.cells.push((n.into(), Cell::value(v)));
    }
    geo.rows = vec![
        row("MoveTo", 1, &[("X", 0.0, "Width*0"), ("Y", 0.0, "Height*0")]),
        row("LineTo", 2, &[("X", len, "Width*1"), ("Y", 0.0, "Height*0")]),
    ];
    s.sheet.children.push(SheetChild::Section(geo));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(s: &Shape, n: &str) -> f64 {
        s.sheet.cell(n).and_then(|c| c.numeric()).unwrap()
    }

    #[test]
    fn shapes_cover_the_dragged_box() {
        let r = build(Kind::Rect, 5, (3.0, 1.0), (1.0, 2.0));
        assert_eq!((num(&r, "PinX"), num(&r, "PinY"), num(&r, "Width"), num(&r, "Height")), (2.0, 1.5, 2.0, 1.0));
        let e = build(Kind::Ellipse, 6, (0.0, 0.0), (2.0, 1.0));
        assert_eq!(e.sheet.sections_named("Geometry")[0].rows[0].t.as_deref(), Some("Ellipse"));
        let t = build(Kind::Text, 7, (0.0, 0.0), (0.0, 0.0));
        assert_eq!(num(&t, "Width"), MIN, "clicks are sized by the caller");
        assert_eq!(num(&t, "LinePattern"), 0.0);
        let l = build(Kind::Line, 8, (1.0, 1.0), (1.0, 3.0));
        assert_eq!((num(&l, "Width"), num(&l, "PinY")), (2.0, 2.0));
        assert!((num(&l, "Angle") - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
    }
}

#[cfg(test)]
mod render_check {
    /// Writes a drawing with the four tool shapes when DRAWTOOLS_OUT is
    /// set (manual Visio/render check); otherwise a no-op.
    #[test]
    fn write_sample() {
        let Some(out) = std::env::var_os("DRAWTOOLS_OUT") else { return };
        let mut ed = dc_core::Editor::new_blank();
        let page = ed.doc.pages[0].id;
        for (k, a, b) in [
            (super::Kind::Rect, (1.0, 7.0), (3.0, 8.5)),
            (super::Kind::Ellipse, (4.0, 7.0), (6.5, 8.5)),
            (super::Kind::Line, (1.0, 5.0), (6.0, 6.0)),
            (super::Kind::Text, (1.0, 3.0), (3.0, 3.5)),
        ] {
            let id = dc_core::commands::next_shape_id(&ed.doc, page);
            ed.apply(dc_core::AddShape { page, parent: None, shape: super::build(k, id, a, b) }).unwrap();
        }
        let id = dc_core::commands::next_shape_id(&ed.doc, page) - 1;
        let scope = dc_core::Scope::Shape { page, shape: id };
        let m = dc::textfunc::text_edit_macro(&ed.doc, &scope, "Text box");
        ed.apply(m).unwrap();
        ed.save_as(std::path::Path::new(&out)).unwrap();
    }
}

#[cfg(test)]
mod scale_check {
    /// SCALE_OUT=file.vsdx: a 1:10 page (Letter paper = 85 x 110 in of
    /// drawing) with a 20 x 10 rectangle, 2 pt line, 12 pt text.
    #[test]
    fn write_scaled_sample() {
        let Some(out) = std::env::var_os("SCALE_OUT") else { return };
        let mut ed = dc_core::Editor::new_blank();
        let page = ed.doc.pages[0].id;
        for (c, v) in [("PageWidth", "85"), ("PageHeight", "110"), ("PageScale", "1"), ("DrawingScale", "10")] {
            ed.apply(dc_core::SetCell::new(dc_core::Scope::PageSheet { page }, c, vsdx::Cell::value(v))).unwrap();
        }
        let mut s = super::build(super::Kind::Rect, 1, (30.0, 50.0), (50.0, 60.0));
        s.sheet.set_cell("LineWeight", vsdx::Cell::value(format!("{}", 2.0 / 72.0)));
        ed.apply(dc_core::AddShape { page, parent: None, shape: s }).unwrap();
        let scope = dc_core::Scope::Shape { page, shape: 1 };
        let m = dc::textfunc::text_edit_macro(&ed.doc, &scope, "Scaled 1:10");
        ed.apply(m).unwrap();
        ed.save_as(std::path::Path::new(&out)).unwrap();
    }
}
