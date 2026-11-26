//! Developer ▸ Operations (Visio's shape operations): Union, Combine,
//! Fragment, Intersect, Subtract on the selected shapes' filled
//! outlines. Outlines come from the renderer (each shape's own layer
//! SVG, flattened to polygons in page inches); the booleans run in
//! `i_overlay`; the results are plain shapes with one Geometry section
//! per contour and the first selected shape's formatting (as Visio).

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::single::SingleFloatOverlay;
use resvg::usvg;
use vsdx::model::{Cell, Document, Row, Section, Shape, SheetChild};

/// Contours of one region: the outer boundary then its holes.
pub type Region = Vec<Vec<[f64; 2]>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Union,
    Combine,
    Fragment,
    Intersect,
    Subtract,
}

/// The shape's filled outline in page inches (y up), as contours.
pub fn outline(doc: &Document, page: &vsdx::Page, id: i64) -> Vec<Vec<[f64; 2]>> {
    let Some(bounds) = dc::svg::shape_ink_bounds(doc, page, id) else { return vec![] };
    let Ok(svg) = dc::svg::render_shape_layer(doc, page, id, &[], bounds) else { return vec![] };
    let Ok(tree) = dc::raster::parse_svg(&svg, &dc::raster::font_db()) else { return vec![] };
    let page_h = page.sheet().and_then(|s| s.cell("PageHeight")).and_then(|c| c.numeric()).unwrap_or(11.0);
    // usvg folds the viewBox (the bounds, in inches) into the
    // transforms: map its pixel space back to page inches, y up
    let (sx, sy) = (bounds.2 / tree.size().width() as f64, bounds.3 / tree.size().height() as f64);
    let to_page = move |x: f32, y: f32| [bounds.0 + x as f64 * sx, page_h - (bounds.1 + y as f64 * sy)];
    let mut out = Vec::new();
    collect(tree.root(), &to_page, &mut out);
    out
}

fn collect(g: &usvg::Group, to_page: &dyn Fn(f32, f32) -> [f64; 2], out: &mut Vec<Vec<[f64; 2]>>) {
    for node in g.children() {
        match node {
            usvg::Node::Group(g) => collect(g, to_page, out),
            usvg::Node::Path(p) if p.fill().is_some() => {
                let ts = p.abs_transform();
                let map = |x: f32, y: f32| {
                    let mut pt = usvg::tiny_skia_path::Point::from_xy(x, y);
                    ts.map_point(&mut pt);
                    to_page(pt.x, pt.y)
                };
                let mut cur: Vec<[f64; 2]> = Vec::new();
                let mut last = (0.0f32, 0.0f32);
                for seg in p.data().segments() {
                    use usvg::tiny_skia_path::PathSegment as S;
                    match seg {
                        S::MoveTo(a) => {
                            if cur.len() > 2 {
                                out.push(std::mem::take(&mut cur));
                            }
                            cur.clear();
                            cur.push(map(a.x, a.y));
                            last = (a.x, a.y);
                        }
                        S::LineTo(a) => {
                            cur.push(map(a.x, a.y));
                            last = (a.x, a.y);
                        }
                        S::QuadTo(c, a) => {
                            for i in 1..=12 {
                                let t = i as f32 / 12.0;
                                let u = 1.0 - t;
                                let x = u * u * last.0 + 2.0 * u * t * c.x + t * t * a.x;
                                let y = u * u * last.1 + 2.0 * u * t * c.y + t * t * a.y;
                                cur.push(map(x, y));
                            }
                            last = (a.x, a.y);
                        }
                        S::CubicTo(c1, c2, a) => {
                            for i in 1..=16 {
                                let t = i as f32 / 16.0;
                                let u = 1.0 - t;
                                let x = u * u * u * last.0 + 3.0 * u * u * t * c1.x + 3.0 * u * t * t * c2.x + t * t * t * a.x;
                                let y = u * u * u * last.1 + 3.0 * u * u * t * c1.y + 3.0 * u * t * t * c2.y + t * t * t * a.y;
                                cur.push(map(x, y));
                            }
                            last = (a.x, a.y);
                        }
                        S::Close => {
                            if cur.len() > 2 {
                                out.push(std::mem::take(&mut cur));
                            }
                        }
                    }
                }
                if cur.len() > 2 {
                    out.push(cur);
                }
            }
            _ => {}
        }
    }
}

fn shapes_of(c: &[Vec<[f64; 2]>]) -> Vec<Region> {
    // even-odd within one shape's outline: let the library normalise it
    let empty: Vec<Vec<[f64; 2]>> = Vec::new();
    c.to_vec().overlay(&empty, OverlayRule::Union, FillRule::EvenOdd)
}

fn flat(r: &[Region]) -> Vec<Vec<[f64; 2]>> {
    r.iter().flatten().cloned().collect()
}

/// Run `op` over the outlines (first = the first selected shape).
/// Returns the result regions; Fragment returns every piece.
pub fn run(op: Op, outlines: &[Vec<Vec<[f64; 2]>>]) -> Vec<Region> {
    let Some((first, rest)) = outlines.split_first() else { return vec![] };
    let mut acc: Vec<Region> = shapes_of(first);
    match op {
        Op::Union | Op::Combine | Op::Intersect => {
            let rule = match op {
                Op::Union => OverlayRule::Union,
                Op::Combine => OverlayRule::Xor,
                _ => OverlayRule::Intersect,
            };
            for o in rest {
                acc = flat(&acc).overlay(&flat(&shapes_of(o)), rule, FillRule::EvenOdd);
            }
            // Union/Combine/Intersect make ONE shape (its regions as sections)
            if acc.is_empty() { vec![] } else { vec![flat(&acc)] }
        }
        Op::Subtract => {
            let mut others: Vec<Region> = Vec::new();
            for o in rest {
                others = flat(&others).overlay(&flat(&shapes_of(o)), OverlayRule::Union, FillRule::EvenOdd);
            }
            acc = flat(&acc).overlay(&flat(&others), OverlayRule::Difference, FillRule::EvenOdd);
            if acc.is_empty() { vec![] } else { vec![flat(&acc)] }
        }
        Op::Fragment => {
            let mut pieces: Vec<Region> = acc;
            for o in rest {
                let s = shapes_of(o);
                let mut next = Vec::new();
                let mut covered: Vec<Region> = Vec::new();
                for p in &pieces {
                    next.extend(p.clone().overlay(&flat(&s), OverlayRule::Intersect, FillRule::EvenOdd));
                    next.extend(p.clone().overlay(&flat(&s), OverlayRule::Difference, FillRule::EvenOdd));
                    covered = flat(&covered).overlay(p, OverlayRule::Union, FillRule::EvenOdd);
                }
                next.extend(flat(&s).overlay(&flat(&covered), OverlayRule::Difference, FillRule::EvenOdd));
                pieces = next;
            }
            pieces.retain(|r| area(r).abs() > 1e-6);
            pieces
        }
    }
}

fn area(r: &Region) -> f64 {
    r.iter()
        .map(|c| {
            let n = c.len();
            (0..n).map(|i| c[i][0] * c[(i + 1) % n][1] - c[(i + 1) % n][0] * c[i][1]).sum::<f64>() / 2.0
        })
        .sum()
}

/// A plain shape for one region: Geometry section per contour (local
/// coordinates of the bounding box), `style` cells and style ids from
/// the first selected shape.
pub fn region_shape(id: i64, region: &Region, style: &Shape) -> Option<Shape> {
    let pts = region.iter().flatten();
    let (x0, y0, x1, y1) = pts.fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |a, p| (a.0.min(p[0]), a.1.min(p[1]), a.2.max(p[0]), a.3.max(p[1])));
    if x0 > x1 {
        return None;
    }
    let (w, h) = ((x1 - x0).max(1e-4), (y1 - y0).max(1e-4));
    let mut s = Shape {
        id,
        type_: Some("Shape".into()),
        name_u: Some(format!("Sheet.{id}")),
        line_style: style.line_style,
        fill_style: style.fill_style,
        text_style: style.text_style,
        ..Shape::default()
    };
    let f = |v: f64| Cell::value(format!("{v:.10}"));
    for (n, v) in [("PinX", x0 + w / 2.0), ("PinY", y0 + h / 2.0), ("Width", w), ("Height", h), ("LocPinX", w / 2.0), ("LocPinY", h / 2.0)] {
        s.sheet.children.push(SheetChild::Cell(n.into(), f(v)));
    }
    const FORMAT: [&str; 12] = [
        "LineWeight", "LineColor", "LinePattern", "LineCap", "Rounding", "FillForegnd", "FillBkgnd", "FillPattern",
        "FillForegndTrans", "FillBkgndTrans", "LineColorTrans", "ShdwPattern",
    ];
    for n in FORMAT {
        if let Some(c) = style.sheet.cell(n) {
            s.sheet.children.push(SheetChild::Cell(n.into(), Cell { v: c.v.clone(), u: c.u.clone(), ..Cell::default() }));
        }
    }
    for (ix, contour) in region.iter().enumerate() {
        let mut sec = Section { name: "Geometry".into(), ix: ix as i64, ..Section::default() };
        for (n, v) in [("NoFill", "0"), ("NoLine", "0"), ("NoShow", "0"), ("NoSnap", "0")] {
            sec.cells.push((n.into(), Cell::value(v)));
        }
        let pt = |p: &[f64; 2]| vec![("X".to_string(), f(p[0] - x0)), ("Y".to_string(), f(p[1] - y0))];
        sec.rows.push(Row { t: Some("MoveTo".into()), ix: 1, cells: pt(&contour[0]), ..Row::default() });
        for (i, p) in contour.iter().skip(1).chain(std::iter::once(&contour[0])).enumerate() {
            sec.rows.push(Row { t: Some("LineTo".into()), ix: i as i64 + 2, cells: pt(p), ..Row::default() });
        }
        s.sheet.children.push(SheetChild::Section(sec));
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Vec<Vec<[f64; 2]>> {
        vec![vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]]]
    }

    #[test]
    fn booleans_on_two_overlapping_squares() {
        let (a, b) = (rect(0.0, 0.0, 2.0, 2.0), rect(1.0, 1.0, 2.0, 2.0));
        let ar = |r: &[Region]| r.iter().map(|x| area(x).abs()).sum::<f64>();
        let near = |x: f64, y: f64| (x - y).abs() < 1e-6;
        assert!(near(ar(&run(Op::Union, &[a.clone(), b.clone()])), 7.0));
        assert!(near(ar(&run(Op::Intersect, &[a.clone(), b.clone()])), 1.0));
        assert!(near(ar(&run(Op::Subtract, &[a.clone(), b.clone()])), 3.0));
        assert!(near(ar(&run(Op::Combine, &[a.clone(), b.clone()])), 6.0));
        let frag = run(Op::Fragment, &[a, b]);
        assert_eq!(frag.len(), 3, "A-B, A∩B, B-A");
        assert!(near(ar(&frag), 7.0));
    }
}

#[cfg(test)]
mod outline_tests {
    #[test]
    fn outline_is_in_page_inches() {
        let mut ed = dc_core::Editor::new_blank();
        let page = ed.doc.pages[0].id;
        let s = crate::drawtools::build(crate::drawtools::Kind::Rect, 1, (1.0, 2.0), (3.0, 3.0));
        ed.apply(dc_core::AddShape { page, parent: None, shape: s }).unwrap();
        let o = super::outline(&ed.doc, &ed.doc.pages[0], 1);
        let xs: Vec<f64> = o.iter().flatten().map(|p| p[0]).collect();
        let ys: Vec<f64> = o.iter().flatten().map(|p| p[1]).collect();
        let mm = |v: &[f64]| (v.iter().cloned().fold(f64::MAX, f64::min), v.iter().cloned().fold(f64::MIN, f64::max));
        let (x, y) = (mm(&xs), mm(&ys));
        let near = |a: f64, b: f64| (a - b).abs() < 0.01;
        assert!(near(x.0, 1.0) && near(x.1, 3.0) && near(y.0, 2.0) && near(y.1, 3.0), "{x:?} {y:?}");
    }
}

#[cfg(test)]
mod visual_check {
    /// OPS_OUT=file.vsdx: each operation on a rectangle + ellipse, side
    /// by side (manual Tarahi / Visio comparison); otherwise a no-op.
    #[test]
    fn write_sample() {
        let Some(out) = std::env::var_os("OPS_OUT") else { return };
        use super::Op;
        let mut ed = dc_core::Editor::new_blank();
        let page = ed.doc.pages[0].id;
        let mut id = 1;
        for (i, op) in [Op::Union, Op::Combine, Op::Fragment, Op::Intersect, Op::Subtract].into_iter().enumerate() {
            let x = 0.5 + i as f64 * 1.6;
            let r = crate::drawtools::build(crate::drawtools::Kind::Rect, 100 + id, (x, 8.0), (x + 1.0, 9.0));
            let e = crate::drawtools::build(crate::drawtools::Kind::Ellipse, 101 + id, (x + 0.4, 8.5), (x + 1.4, 9.5));
            ed.apply(dc_core::AddShape { page, parent: None, shape: r.clone() }).unwrap();
            ed.apply(dc_core::AddShape { page, parent: None, shape: e }).unwrap();
            let p = ed.doc.pages[0].clone();
            let o = vec![super::outline(&ed.doc, &p, 100 + id), super::outline(&ed.doc, &p, 101 + id)];
            ed.apply(dc_core::RemoveShape::new(page, 100 + id)).unwrap();
            ed.apply(dc_core::RemoveShape::new(page, 101 + id)).unwrap();
            for reg in super::run(op, &o) {
                let mut sh = super::region_shape(id, &reg, &r).unwrap();
                // pieces in different greys so Fragment's parts show
                sh.sheet.set_cell("FillForegnd", vsdx::Cell::value(format!("#{0:02x}{0:02x}{0:02x}", 0x60 + (id as u8 % 5) * 0x20)));
                ed.apply(dc_core::AddShape { page, parent: None, shape: sh }).unwrap();
                id += 1;
            }
            id += 2;
        }
        ed.save_as(std::path::Path::new(&out)).unwrap();
    }
}
