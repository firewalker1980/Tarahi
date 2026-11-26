//! Container Tools sizing (Visio's Fit to Contents, Margins, Automatic
//! Resize) over the membership convention Tarahi uses: members carry
//! `User.msvSDContainerShape` = the container's id; the container's own
//! settings are the `User.msvSDContainerMargin` (inches),
//! `User.msvSDContainerResize` (0 no, 1 expand as needed, 2 always fit)
//! and `User.msvSDContainerLocked` rows, on the shape or its master.

use dc_core::commands::SetCell;
use dc_core::scope::shape_chain;
use dc_core::{Command, Scope};
use vsdx::model::{Cell, Document, Shape};

fn user(doc: &Document, page: i64, s: &Shape, name: &str) -> Option<f64> {
    let chain = shape_chain(doc, page, s.id).ok()?;
    doc.effective_user_row(s, &chain, name)
        .and_then(|r| r.cell("Value"))
        .and_then(|c| c.numeric())
}

/// Top-level members of `container` on `page`.
pub fn members(doc: &Document, page: i64, container: i64) -> Vec<i64> {
    let Some(p) = doc.pages.iter().find(|p| p.id == page) else { return vec![] };
    p.shapes
        .iter()
        .filter(|s| {
            s.sheet.sections_named("User").iter().any(|sec| {
                sec.named_row("msvSDContainerShape").and_then(|r| r.cell("Value")).and_then(|c| c.numeric())
                    == Some(container as f64)
            })
        })
        .map(|s| s.id)
        .collect()
}

fn rect(doc: &Document, page: i64, s: &Shape) -> Option<(f64, f64, f64, f64)> {
    let chain = shape_chain(doc, page, s.id).ok()?;
    let get = |n: &str| doc.resolve_cell(s, &chain, n).and_then(|c| c.numeric());
    let (px, py) = (get("PinX")?, get("PinY")?);
    let (w, h) = (get("Width").unwrap_or(0.0), get("Height").unwrap_or(0.0));
    let (lx, ly) = (get("LocPinX").unwrap_or(w / 2.0), get("LocPinY").unwrap_or(h / 2.0));
    Some((px - lx, py - ly, w, h))
}

pub fn margin(doc: &Document, page: i64, container: &Shape) -> f64 {
    user(doc, page, container, "msvSDContainerMargin").unwrap_or(0.25)
}

pub fn resize_mode(doc: &Document, page: i64, container: &Shape) -> i64 {
    user(doc, page, container, "msvSDContainerResize").map_or(0, |v| v as i64)
}

pub fn locked(doc: &Document, page: i64, container: &Shape) -> bool {
    user(doc, page, container, "msvSDContainerLocked").is_some_and(|v| v != 0.0)
}

/// Commands that size `container` to its members plus its margin;
/// `grow_only` keeps any side that already reaches far enough (Visio's
/// "expand as needed"). Empty when nothing changes or it has no members.
pub fn fit_cmds(doc: &Document, page: i64, container: i64, grow_only: bool) -> Vec<Box<dyn Command>> {
    let Some(p) = doc.pages.iter().find(|p| p.id == page) else { return vec![] };
    let Some(c) = p.shapes.iter().find(|s| s.id == container) else { return vec![] };
    let ids = members(doc, page, container);
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for s in p.shapes.iter().filter(|s| ids.contains(&s.id)) {
        if let Some((x, y, w, h)) = rect(doc, page, s) {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + w), y1.max(y + h));
        }
    }
    let Some((cx, cy, cw, ch)) = rect(doc, page, c) else { return vec![] };
    if x0 > x1 {
        return vec![];
    }
    let m = margin(doc, page, c);
    let (mut nx0, mut ny0, mut nx1, mut ny1) = (x0 - m, y0 - m, x1 + m, y1 + m);
    if grow_only {
        (nx0, ny0, nx1, ny1) = (nx0.min(cx), ny0.min(cy), nx1.max(cx + cw), ny1.max(cy + ch));
    }
    let (nw, nh) = (nx1 - nx0, ny1 - ny0);
    if (nx0 - cx).abs() < 1e-6 && (ny0 - cy).abs() < 1e-6 && (nw - cw).abs() < 1e-6 && (nh - ch).abs() < 1e-6 {
        return vec![];
    }
    // keep the pin at the same relative spot of the box
    let chain = match shape_chain(doc, page, container) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let get = |n: &str| doc.resolve_cell(c, &chain, n).and_then(|c| c.numeric());
    let (rx, ry) = (
        get("LocPinX").unwrap_or(cw / 2.0) / cw.max(1e-9),
        get("LocPinY").unwrap_or(ch / 2.0) / ch.max(1e-9),
    );
    let scope = Scope::Shape { page, shape: container };
    let v = |x: f64| Cell::value(format!("{x:.8}"));
    let mut cmds: Vec<Box<dyn Command>> = vec![
        Box::new(SetCell::new(scope.clone(), "Width", v(nw))),
        Box::new(SetCell::new(scope.clone(), "Height", v(nh))),
        Box::new(SetCell::new(scope.clone(), "LocPinX", v(rx * nw))),
        Box::new(SetCell::new(scope.clone(), "LocPinY", v(ry * nh))),
        Box::new(SetCell::new(scope.clone(), "PinX", v(nx0 + rx * nw))),
        Box::new(SetCell::new(scope, "PinY", v(ny0 + ry * nh))),
    ];
    cmds.extend(crate::cmd::group_cache_refresh(doc, page, container, nw / cw.max(1e-9), nh / ch.max(1e-9)));
    cmds
}

/// Containers on `page` that auto-resize (mode 1 or 2) and hold one of
/// `moved`.
pub fn auto_resizing(doc: &Document, page: i64, moved: &[i64]) -> Vec<i64> {
    let Some(p) = doc.pages.iter().find(|p| p.id == page) else { return vec![] };
    let mut out: Vec<i64> = Vec::new();
    for s in p.shapes.iter().filter(|s| moved.contains(&s.id)) {
        let cont = s
            .sheet
            .sections_named("User")
            .iter()
            .find_map(|sec| sec.named_row("msvSDContainerShape"))
            .and_then(|r| r.cell("Value"))
            .and_then(|c| c.numeric())
            .map(|v| v as i64);
        if let Some(cid) = cont {
            if out.contains(&cid) || moved.contains(&cid) {
                continue;
            }
            if let Some(c) = p.shapes.iter().find(|x| x.id == cid) {
                if resize_mode(doc, page, c) > 0 {
                    out.push(cid);
                }
            }
        }
    }
    out
}

/// Automatic Resize after a move, in the move's undo step: the fit is
/// computed when the command first runs (after the members moved) and
/// replayed as-is on redo.
pub struct FitContainers {
    page: i64,
    containers: Vec<i64>,
    inner: Option<dc_core::Macro>,
}

impl FitContainers {
    pub fn new(page: i64, containers: Vec<i64>) -> Self {
        Self { page, containers, inner: None }
    }
}

impl Command for FitContainers {
    fn label(&self) -> String {
        format!("fit containers {:?}", self.containers)
    }
    fn touched(&self, _doc: &Document) -> dc_core::TouchList {
        dc_core::TouchList {
            pages: vec![],
            shapes: self
                .containers
                .iter()
                .map(|&shape| dc_core::Touched { page: self.page, shape, touch: dc_core::Touch::Ink, sub: None })
                .collect(),
        }
    }
    fn apply(&mut self, doc: &mut Document) -> anyhow::Result<()> {
        if self.inner.is_none() {
            let mut cmds = Vec::new();
            for &c in &self.containers {
                let grow = doc
                    .pages
                    .iter()
                    .find(|p| p.id == self.page)
                    .and_then(|p| p.shapes.iter().find(|s| s.id == c))
                    .is_some_and(|s| resize_mode(doc, self.page, s) == 1);
                cmds.extend(fit_cmds(doc, self.page, c, grow));
            }
            self.inner = Some(dc_core::Macro::new("fit containers", cmds));
        }
        self.inner.as_mut().unwrap().apply(doc)
    }
    fn undo(&mut self, doc: &mut Document) -> anyhow::Result<()> {
        match &mut self.inner {
            Some(m) => m.undo(doc),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(id: i64, x: f64, y: f64, w: f64, h: f64, cont: Option<i64>) -> Shape {
        let mut s = Shape { id, type_: Some("Shape".into()), ..Shape::default() };
        for (n, v) in [("PinX", x + w / 2.0), ("PinY", y + h / 2.0), ("Width", w), ("Height", h), ("LocPinX", w / 2.0), ("LocPinY", h / 2.0)] {
            s.sheet.set_cell(n, Cell::value(v.to_string()));
        }
        if let Some(c) = cont {
            s.sheet.children.push(vsdx::model::SheetChild::Section(vsdx::model::Section {
                name: "User".into(),
                rows: vec![vsdx::model::Row {
                    n: Some("msvSDContainerShape".into()),
                    cells: vec![("Value".into(), Cell::value(c.to_string()))],
                    ..Default::default()
                }],
                ..Default::default()
            }));
        }
        s
    }

    #[test]
    fn fit_wraps_members_with_the_margin_and_undoes() {
        let mut ed = dc_core::Editor::new_blank();
        let page = ed.doc.pages[0].id;
        ed.doc.pages[0].shapes = vec![
            shape(1, 0.0, 0.0, 10.0, 10.0, None),
            shape(2, 2.0, 3.0, 1.0, 1.0, Some(1)),
            shape(3, 4.0, 5.0, 2.0, 1.0, Some(1)),
        ];
        let cmds = fit_cmds(&ed.doc, page, 1, false);
        ed.apply(dc_core::Macro::new("fit", cmds)).unwrap();
        let c = &ed.doc.pages[0].shapes[0];
        let r = rect(&ed.doc, page, c).unwrap();
        let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
        assert!(close(r.0, 1.75) && close(r.1, 2.75) && close(r.2, 4.5) && close(r.3, 3.5), "{r:?}");
        // grow-only keeps a box that already covers the members
        assert!(fit_cmds(&ed.doc, page, 1, true).is_empty());
        ed.undo().unwrap();
        assert!(close(rect(&ed.doc, page, &ed.doc.pages[0].shapes[0]).unwrap().2, 10.0));
    }
}

/// Design ▸ Auto Size (page `DrawingResizeType` = 1): the page grows by
/// whole page-size tiles to the right and upward until every top-level
/// shape fits. ponytail: grows right/up only and never shrinks; Visio
/// also extends left/down (shifting the drawing) and trims empty tiles.
pub fn autosize_cmds(doc: &Document, page: i64) -> Vec<Box<dyn Command>> {
    let Some(p) = doc.pages.iter().find(|p| p.id == page) else { return vec![] };
    let n = |c: &str| p.sheet().and_then(|s| s.cell(c)).and_then(|c| c.numeric());
    if n("DrawingResizeType") != Some(1.0) {
        return vec![];
    }
    let (Some(w), Some(h)) = (n("PageWidth"), n("PageHeight")) else { return vec![] };
    let (mut x1, mut y1) = (0.0f64, 0.0f64);
    for s in &p.shapes {
        if let Some((x, y, sw, sh)) = rect(doc, page, s) {
            x1 = x1.max(x + sw);
            y1 = y1.max(y + sh);
        }
    }
    // tile size: the page's own size (Visio grows by printer pages)
    let tile_w = n("PrintPageWidth").filter(|v| *v > 0.0).unwrap_or(w.clamp(1.0, 8.5));
    let tile_h = n("PrintPageHeight").filter(|v| *v > 0.0).unwrap_or(h.clamp(1.0, 11.0));
    let nw = if x1 > w + 1e-6 { w + ((x1 - w) / tile_w).ceil() * tile_w } else { w };
    let nh = if y1 > h + 1e-6 { h + ((y1 - h) / tile_h).ceil() * tile_h } else { h };
    let scope = Scope::PageSheet { page };
    let mut out: Vec<Box<dyn Command>> = Vec::new();
    if nw != w {
        out.push(Box::new(SetCell::new(scope.clone(), "PageWidth", Cell::value(format!("{nw}")))));
    }
    if nh != h {
        out.push(Box::new(SetCell::new(scope, "PageHeight", Cell::value(format!("{nh}")))));
    }
    out
}

#[cfg(test)]
mod autosize_tests {
    use super::*;

    #[test]
    fn page_grows_by_tiles_to_hold_the_drawing() {
        let mut ed = dc_core::Editor::new_blank();
        let page = ed.doc.pages[0].id;
        let set = |ed: &mut dc_core::Editor, c: &str, v: &str| {
            ed.apply(SetCell::new(Scope::PageSheet { page }, c, Cell::value(v))).unwrap();
        };
        set(&mut ed, "PageWidth", "8.5");
        set(&mut ed, "PageHeight", "11");
        ed.doc.pages[0].shapes = vec![tests_shape(1, 9.0, 1.0, 1.0, 1.0)];
        assert!(autosize_cmds(&ed.doc, page).is_empty(), "off unless DrawingResizeType = 1");
        set(&mut ed, "DrawingResizeType", "1");
        let cmds = autosize_cmds(&ed.doc, page);
        ed.apply(dc_core::Macro::new("auto", cmds)).unwrap();
        let w = ed.doc.pages[0].sheet().unwrap().cell("PageWidth").unwrap().numeric().unwrap();
        assert_eq!(w, 17.0, "one more page-width tile");
    }

    fn tests_shape(id: i64, x: f64, y: f64, w: f64, h: f64) -> Shape {
        let mut s = Shape { id, type_: Some("Shape".into()), ..Shape::default() };
        for (n, v) in [("PinX", x + w / 2.0), ("PinY", y + h / 2.0), ("Width", w), ("Height", h), ("LocPinX", w / 2.0), ("LocPinY", h / 2.0)] {
            s.sheet.set_cell(n, Cell::value(v.to_string()));
        }
        s
    }
}
