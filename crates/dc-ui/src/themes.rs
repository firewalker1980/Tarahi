//! Design ▸ Themes / Variants / Colors / Effects / Connectors and Home ▸
//! Quick Styles, with Tarahi's own palettes (no Microsoft themes). A
//! theme is written as plain cells (fill, line, text colour, shadow,
//! rounding), which Visio shows as they are. The page remembers its
//! theme (`User.TarahiTheme`) so shapes dropped later take it too.

use dc_core::{Command, Scope};
use vsdx::model::{Cell, Document, Shape};

pub struct Palette {
    pub name: &'static str,
    /// Text and outline colour.
    pub dark: &'static str,
    pub accents: [&'static str; 4],
}

pub const PALETTES: &[Palette] = &[
    Palette { name: "Harbor", dark: "#1F2D3D", accents: ["#2E86AB", "#5FAD9C", "#F6AE2D", "#E4572E"] },
    Palette { name: "Meadow", dark: "#283618", accents: ["#606C38", "#8AA05A", "#DDA15E", "#BC6C25"] },
    Palette { name: "Ember", dark: "#3D0C02", accents: ["#C8553D", "#F28F3B", "#E9B44C", "#588B8B"] },
    Palette { name: "Slate", dark: "#222831", accents: ["#4F6D7A", "#7FA7B5", "#393E46", "#C0D6DF"] },
    Palette { name: "Orchid", dark: "#2D1E2F", accents: ["#7B2D8E", "#B565A7", "#E0AFA0", "#5C80BC"] },
    Palette { name: "Sand", dark: "#3E3A32", accents: ["#A68A64", "#C2A878", "#7F9C96", "#4F6367"] },
    Palette { name: "Lagoon", dark: "#0B3954", accents: ["#087E8B", "#3CBBB1", "#BFD7EA", "#FF5A5F"] },
    Palette { name: "Mono", dark: "#111111", accents: ["#444444", "#777777", "#AAAAAA", "#222222"] },
];

/// Design ▸ Effects.
pub const EFFECTS: usize = 4; // none, shadow, rounded, rounded + shadow
/// Design ▸ Connectors: (line weight pt, end arrow).
pub const CONNECTORS: [(f64, u8); 4] = [(0.75, 0), (0.75, 4), (1.5, 4), (2.25, 13)];

/// Quick Style intensity: light tint, accent, dark.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Intensity {
    Subtle,
    Moderate,
    Intense,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub palette: usize,
    pub variant: usize,
    pub effect: usize,
    pub connector: usize,
}

impl Theme {
    pub fn encode(&self) -> String {
        format!("{};{};{};{}", self.palette, self.variant, self.effect, self.connector)
    }
    fn decode(s: &str) -> Option<Theme> {
        let v: Vec<usize> = s.split(';').map(|x| x.trim().parse().ok()).collect::<Option<_>>()?;
        (v.len() == 4 && v[0] < PALETTES.len()).then(|| Theme { palette: v[0], variant: v[1] % 4, effect: v[2] % EFFECTS, connector: v[3] % CONNECTORS.len() })
    }
}

/// The page's theme, if one was applied.
pub fn page_theme(doc: &Document, page: i64) -> Option<Theme> {
    let v = doc
        .pages
        .iter()
        .find(|p| p.id == page)?
        .sheet()?
        .sections_named("User")
        .into_iter()
        .find_map(|s| s.named_row("TarahiTheme"))?
        .cell("Value")?
        .v
        .clone()?;
    Theme::decode(&v)
}

fn hex(c: &str) -> (f64, f64, f64) {
    let n = u32::from_str_radix(c.trim_start_matches('#'), 16).unwrap_or(0);
    (((n >> 16) & 255) as f64, ((n >> 8) & 255) as f64, (n & 255) as f64)
}

/// `c` mixed toward white (t > 0) or black (t < 0).
fn mix(c: &str, t: f64) -> String {
    let (r, g, b) = hex(c);
    let f = |x: f64| if t >= 0.0 { x + (255.0 - x) * t } else { x * (1.0 + t) };
    format!("#{:02X}{:02X}{:02X}", f(r).round() as u8, f(g).round() as u8, f(b).round() as u8)
}

fn is_dark(c: &str) -> bool {
    let (r, g, b) = hex(c);
    0.299 * r + 0.587 * g + 0.114 * b < 140.0
}

/// (fill, line, text) of a quick style.
pub fn quick_colors(p: &Palette, accent: usize, i: Intensity) -> (String, String, String) {
    let a = p.accents[accent % 4];
    match i {
        Intensity::Subtle => (mix(a, 0.8), a.to_string(), p.dark.to_string()),
        Intensity::Moderate => {
            let text = if is_dark(a) { "#FFFFFF" } else { p.dark };
            (a.to_string(), mix(a, -0.35), text.to_string())
        }
        Intensity::Intense => (mix(a, -0.45), mix(a, -0.65), "#FFFFFF".to_string()),
    }
}

fn is_1d(s: &Shape) -> bool {
    s.sheet.cell("BeginX").is_some() || s.sheet.cell("EndX").is_some()
}

fn set(cmds: &mut Vec<Box<dyn Command>>, scope: &Scope, name: &str, v: &str) {
    cmds.push(Box::new(dc_core::SetCell::new(scope.clone(), name, Cell::value(v))));
}

fn text_color(cmds: &mut Vec<Box<dyn Command>>, scope: &Scope, c: &str) {
    cmds.push(Box::new(dc_core::SetRowCell::new(scope.clone(), "Character", 0, dc_core::commands::RowSel::Ix(0), "Color", Cell::value(c))));
}

/// Commands styling one shape: a 2-D shape gets the quick style's fill,
/// line, text and the effect; a 1-D one (connector) the connector style.
pub fn style_shape(page: i64, s: &Shape, p: &Palette, accent: usize, i: Intensity, effect: usize, connector: usize) -> Vec<Box<dyn Command>> {
    let scope = Scope::Shape { page, shape: s.id };
    let mut cmds: Vec<Box<dyn Command>> = Vec::new();
    if is_1d(s) {
        let (w, arrow) = CONNECTORS[connector % CONNECTORS.len()];
        set(&mut cmds, &scope, "LineColor", p.dark);
        set(&mut cmds, &scope, "LineWeight", &format!("{}", w / 72.0));
        set(&mut cmds, &scope, "EndArrow", &arrow.to_string());
        return cmds;
    }
    let (fill, line, text) = quick_colors(p, accent, i);
    set(&mut cmds, &scope, "FillForegnd", &fill);
    set(&mut cmds, &scope, "FillPattern", "1");
    set(&mut cmds, &scope, "LineColor", &line);
    text_color(&mut cmds, &scope, &text);
    let (shadow, round) = match effect % EFFECTS {
        0 => (false, false),
        1 => (true, false),
        2 => (false, true),
        _ => (true, true),
    };
    set(&mut cmds, &scope, "ShdwPattern", if shadow { "1" } else { "0" });
    if shadow {
        set(&mut cmds, &scope, "ShdwForegnd", p.dark);
        set(&mut cmds, &scope, "ShdwForegndTrans", "0.6");
        set(&mut cmds, &scope, "ShapeShdwType", "1");
        set(&mut cmds, &scope, "ShapeShdwOffsetX", "0.0625");
        set(&mut cmds, &scope, "ShapeShdwOffsetY", "-0.0625");
    }
    set(&mut cmds, &scope, "Rounding", if round { "0.0833" } else { "0" });
    cmds
}

/// Whether a shape takes theme colours: drawn shapes, not text-only
/// boxes (no fill and no line) or pictures.
fn themable(doc: &Document, page: i64, s: &Shape) -> bool {
    if s.sheet.cell("ForeignType").is_some() || s.type_.as_deref() == Some("Foreign") {
        return false;
    }
    let Ok(chain) = dc_core::scope::shape_chain(doc, page, s.id) else { return false };
    let num = |n: &str| doc.resolve_cell(s, &chain, n).and_then(|c| c.numeric());
    !(num("FillPattern") == Some(0.0) && num("LinePattern") == Some(0.0))
}

/// One shape in the page's theme (a drop on a themed page).
pub fn theme_shape(doc: &Document, page: i64, id: i64) -> Vec<Box<dyn Command>> {
    let Some(t) = page_theme(doc, page) else { return vec![] };
    let Some(s) = doc.pages.iter().find(|p| p.id == page).and_then(|p| p.shapes.iter().find(|s| s.id == id)) else { return vec![] };
    if !themable(doc, page, s) {
        return vec![];
    }
    style_shape(page, s, &PALETTES[t.palette], t.variant, Intensity::Moderate, t.effect, t.connector)
}

/// The whole page in theme `t`, remembered on the page.
pub fn apply_theme(doc: &Document, page: i64, t: Theme) -> Vec<Box<dyn Command>> {
    let mut cmds: Vec<Box<dyn Command>> = Vec::new();
    if let Some(p) = doc.pages.iter().find(|p| p.id == page) {
        for s in p.shapes.iter().filter(|s| themable(doc, page, s)) {
            cmds.extend(style_shape(page, s, &PALETTES[t.palette], t.variant, Intensity::Moderate, t.effect, t.connector));
        }
    }
    cmds.push(Box::new(dc_core::SetRowCell::new(
        Scope::PageSheet { page },
        "User",
        0,
        dc_core::commands::RowSel::Named("TarahiTheme".into()),
        "Value",
        Cell::value(t.encode()),
    )));
    cmds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_styles_shapes_and_is_remembered() {
        let mut ed = dc_core::Editor::new_blank();
        let page = ed.doc.pages[0].id;
        let mut box_ = Shape { id: 1, type_: Some("Shape".into()), ..Shape::default() };
        box_.sheet.set_cell("Width", Cell::value("1"));
        let mut conn = Shape { id: 2, type_: Some("Shape".into()), ..Shape::default() };
        conn.sheet.set_cell("BeginX", Cell::value("0"));
        ed.doc.pages[0].shapes = vec![box_, conn];
        let t = Theme { palette: 0, variant: 1, effect: 1, connector: 2 };
        ed.apply(dc_core::Macro::new("theme", apply_theme(&ed.doc, page, t))).unwrap();
        let cell = |i: usize, n: &str| ed.doc.pages[0].shapes[i].sheet.cell(n).and_then(|c| c.v.clone());
        assert_eq!(cell(0, "FillForegnd").as_deref(), Some(PALETTES[0].accents[1]));
        assert_eq!(cell(0, "ShdwPattern").as_deref(), Some("1"));
        assert_eq!(cell(1, "EndArrow").as_deref(), Some("4"));
        assert!(cell(1, "FillForegnd").is_none(), "connectors keep no fill");
        assert_eq!(page_theme(&ed.doc, page), Some(t));
        // a later drop takes the theme
        ed.doc.pages[0].shapes.push(Shape { id: 3, type_: Some("Shape".into()), ..Shape::default() });
        assert!(!theme_shape(&ed.doc, page, 3).is_empty());
        assert_eq!(mix("#000000", 0.5), "#808080");
    }
}
