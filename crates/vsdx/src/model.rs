//! The VSDX document model: sheets of cells/sections, with master and
//! stylesheet inheritance resolution.
//!
//! v0.2: every container is ordered and every element keeps its original
//! attribute list, so a parsed document can be written back without data
//! loss (unknown cells, attributes, and whole elements survive verbatim).

use std::borrow::Cow;

use crate::formula;
use crate::opc::Package;

/// Ordered attribute list, in document order (round-trip fidelity).
pub type Attrs = Vec<(String, String)>;

/// An element the model does not interpret (`Icon`, `FaceNames`,
/// `DocumentSettings`, …), kept verbatim: its opening-tag attributes plus
/// the raw inner XML, re-emitted unchanged on write.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RawEl {
    pub name: String,
    pub attrs: Attrs,
    /// Inner XML exactly as read (children, text, entities, CDATA).
    pub inner: String,
}

/// One ShapeSheet cell: `N` name, `V` value, `U` unit, `F` formula.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Cell {
    pub v: Option<String>,
    pub u: Option<String>,
    pub f: Option<String>,
    /// Full original attribute list (`N`, `V`, `U`, `F`, `E`, …) in
    /// document order; kept so writing is lossless.
    pub attrs: Attrs,
    /// Raw inner XML when the cell has children (`<RefBy/>`), verbatim.
    pub raw_inner: Option<String>,
}

impl Cell {
    /// A plain value cell (no unit, no formula) — for tests and editing.
    pub fn value(v: impl Into<String>) -> Cell {
        Cell {
            v: Some(v.into()),
            ..Cell::default()
        }
    }
    pub fn numeric(&self) -> Option<f64> {
        self.v.as_deref().and_then(|v| v.trim().parse::<f64>().ok())
    }
    pub fn is_inherited(&self) -> bool {
        self.f.as_deref() == Some("Inh")
    }
    /// `V='Themed'`: the value comes from the document theme through the
    /// style chain (`THEMEVAL()` on the Theme style, `F='Inh'` below
    /// it). Without an evaluated theme it carries no value of its own —
    /// resolution continues to the next style up, ending at the No Style
    /// defaults (Visio 2021 witness: Basic/Network stencil masters
    /// dropped into a theme-less drawing paint white fills and black
    /// lines, never solid black).
    pub fn is_themed(&self) -> bool {
        self.v.as_deref().map(str::trim) == Some("Themed")
    }
    /// A Themed cell that EVALUATES the theme (`THEMEVAL()`, not `Inh`):
    /// without an evaluated theme it takes the No Style value outright
    /// — Visio 2021 paints a Box master (`FillForegnd=THEMEVAL()` on the
    /// master shape, style "Block Normal" with a cached tint) plain
    /// white in a theme-less drawing, never the style's tint.
    pub fn is_theme_eval(&self) -> bool {
        self.is_themed() && !self.is_inherited()
    }
}

/// A row inside a section (optionally typed, e.g. `T='MoveTo'`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    /// Row type (`MoveTo`, `LineTo`, …) for geometry rows; `None` otherwise.
    pub t: Option<String>,
    /// Row index within the section (the `IX` attribute), 0 when absent.
    pub ix: i64,
    /// Named row (`N='Label'`) for named sections (User, Prop, …).
    pub n: Option<String>,
    /// Full original attribute list, in document order.
    pub attrs: Attrs,
    pub cells: Vec<(String, Cell)>,
}

impl Row {
    pub fn cell(&self, name: &str) -> Option<&Cell> {
        self.cells.iter().find(|(n, _)| n == name).map(|(_, c)| c)
    }

    /// `Del="1"` — the row is deleted locally (GitHub #293). The
    /// attribute is schema-defined on `Row_Type` (xsd:boolean) and the
    /// shipped corpus carries it pervasively: the built-in catalog's
    /// 371 stencils — repackaged from the shipped Visio 2016 masters —
    /// hold ~44,000 deleted rows across 79 stencils, and Visio honors
    /// them on open (the #293 oracle: the deleted connector row renders
    /// gone in Visio 2021). `xsd:boolean` accepts both spellings.
    pub fn is_deleted(&self) -> bool {
        self.attrs
            .iter()
            .find(|(n, _)| n == "Del")
            .and_then(|(_, v)| match v.trim().to_ascii_lowercase().as_str() {
                "1" | "true" => Some(true),
                "0" | "false" => Some(false),
                _ => None,
            })
            .unwrap_or(false)
    }

    /// Drop the local deletion marker (an edit that needs the row
    /// again un-deletes it — GitHub #293's Expected).
    pub fn undelete(&mut self) {
        self.attrs.retain(|(n, _)| n != "Del");
    }
}

/// A sheet section, e.g. `Geometry`, `User`, `Character`, `Connection`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Section {
    /// Section name (`N` attribute).
    pub name: String,
    /// Section index (`IX`), 0 for the first Geometry, etc.
    pub ix: i64,
    /// Full original attribute list, in document order.
    pub attrs: Attrs,
    /// Section-level cells (e.g. Geometry `NoFill`/`NoLine`), in order.
    pub cells: Vec<(String, Cell)>,
    pub rows: Vec<Row>,
}

impl Section {
    pub fn cell(&self, name: &str) -> Option<&Cell> {
        self.cells.iter().find(|(n, _)| n == name).map(|(_, c)| c)
    }
    pub fn row_by_ix(&self, ix: i64) -> Option<&Row> {
        self.rows.iter().find(|r| r.ix == ix)
    }
    pub fn named_row(&self, name: &str) -> Option<&Row> {
        self.rows.iter().find(|r| r.n.as_deref() == Some(name))
    }
}

/// One child of a sheet body, in document order. Real sheets interleave
/// `Cell`s with `Trigger`s (and occasionally other elements), so order is
/// part of the round-trip contract.
#[derive(Debug, Clone, PartialEq)]
pub enum SheetChild {
    Cell(String, Cell),
    Section(Section),
    Raw(RawEl),
}

/// A ShapeSheet: an ordered child list plus the derived text fields.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sheet {
    /// Opening-tag attributes (`PageSheet LineStyle='0' …`).
    pub attrs: Attrs,
    /// Cells, sections, and raw elements (`Trigger`, …) in document order.
    pub children: Vec<SheetChild>,
    /// Plain text content of `<Text>` (markup stripped) — rendering input.
    pub text: Option<String>,
    /// Raw inner XML of `<Text>` (markup, entities, CDATA verbatim) —
    /// writing output. `Some("")` for an empty `<Text/>`.
    pub text_raw: Option<String>,
}

impl Sheet {
    pub fn cell(&self, name: &str) -> Option<&Cell> {
        self.children.iter().find_map(|c| match c {
            SheetChild::Cell(n, cell) if n == name => Some(cell),
            _ => None,
        })
    }

    /// Insert or replace a cell by name, preserving position on replace.
    pub fn set_cell(&mut self, name: &str, cell: Cell) {
        let cell = Cell {
            attrs: {
                let mut a: Attrs = vec![("N".into(), name.to_string())];
                if let Some(v) = &cell.v {
                    a.push(("V".into(), v.clone()));
                }
                if let Some(u) = &cell.u {
                    a.push(("U".into(), u.clone()));
                }
                if let Some(f) = &cell.f {
                    a.push(("F".into(), f.clone()));
                }
                a
            },
            ..cell
        };
        for child in self.children.iter_mut() {
            if let SheetChild::Cell(n, slot) = child {
                if n == name {
                    *slot = cell;
                    return;
                }
            }
        }
        self.children.push(SheetChild::Cell(name.to_string(), cell));
    }

    /// Whether the sheet carries any section.
    pub fn has_sections(&self) -> bool {
        self.children
            .iter()
            .any(|c| matches!(c, SheetChild::Section(_)))
    }

    /// All sections with the given name, ordered by IX.
    pub fn sections_named(&self, name: &str) -> Vec<&Section> {
        let mut v: Vec<&Section> = self
            .children
            .iter()
            .filter_map(|c| match c {
                SheetChild::Section(s) if s.name == name => Some(s),
                _ => None,
            })
            .collect();
        v.sort_by_key(|s| s.ix);
        v
    }

    /// The section with exactly this (name, ix), if present.
    pub fn section(&self, name: &str, ix: i64) -> Option<&Section> {
        self.children.iter().find_map(|c| match c {
            SheetChild::Section(s) if s.name == name && s.ix == ix => Some(s),
            _ => None,
        })
    }
}

/// A shape (page-level or nested group member).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Shape {
    pub id: i64,
    pub name_u: Option<String>,
    /// `Type` attribute: `Shape`, `Group`, `Foreign`, `Page`, …
    pub type_: Option<String>,
    /// `Master` attribute — master ID this shape instantiates.
    pub master: Option<i64>,
    /// `MasterShape` attribute — master sub-shape ID this shape maps to.
    pub master_shape: Option<i64>,
    /// `LineStyle` / `FillStyle` / `TextStyle` attribute defaults (IDs).
    pub line_style: Option<i64>,
    pub fill_style: Option<i64>,
    pub text_style: Option<i64>,
    /// Full original attribute list (`ID`, `NameU`, `UniqueID`, …).
    pub attrs: Attrs,
    pub sheet: Sheet,
    /// Order of the post-sheet children (`<Text>`, raw children like
    /// `<ForeignData>`, `<Shapes>`) as they appeared in the source.
    pub flow: Vec<ShapeFlow>,
    /// Unknown shape children (`ForeignData`, …), kept verbatim in order.
    pub raw_children: Vec<RawEl>,
    pub sub_shapes: Vec<Shape>,
}

/// Where a shape's `<Text>`, raw children, and `<Shapes>` appeared.
#[derive(Debug, Clone, PartialEq)]
pub enum ShapeFlow {
    Text,
    Raw(usize),
    Shapes,
}

/// A child of a `<Page>`/`<Master>` index entry, in document order.
#[derive(Debug, Clone, PartialEq)]
pub enum IndexChild {
    /// `<PageSheet>`/`<MasterSheet>` — `tag` preserves which, `sheet.attrs`
    /// hold the opening-tag attributes.
    Sheet { tag: String, sheet: Sheet },
    /// `<Rel r:id='…'/>`.
    Rel(Attrs),
    /// Anything else (notably `<Icon>`), kept verbatim.
    Raw(RawEl),
}

/// A master (stencil shape definition): its index entry and shape tree.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Master {
    pub id: i64,
    pub name_u: Option<String>,
    /// Masters-index `<Master>` attributes, in order.
    pub attrs: Attrs,
    /// Index children (`MasterSheet`, `Rel`, `Icon`, …) in order.
    pub children: Vec<IndexChild>,
    /// The master's shape tree from its part (usually one root shape).
    pub shapes: Vec<Shape>,
    /// Sheet from the master part itself, if any (rare).
    pub contents_sheet: Sheet,
    /// Opening-tag attrs of the `<MasterContents>` root.
    pub contents_attrs: Attrs,
    /// Unknown children of the contents root, verbatim.
    pub contents_raw: Vec<RawEl>,
    /// Part name the master contents were parsed from.
    pub part: String,
}

impl Master {
    /// The master's sheet: the index `PageSheet`/`MasterSheet` when present,
    /// else the part's own sheet.
    pub fn sheet(&self) -> Option<&Sheet> {
        self.children.iter().find_map(|c| match c {
            IndexChild::Sheet { sheet, .. } => Some(sheet),
            _ => None,
        })
    }
}

/// A page: its pages-index entry plus the shape tree from its part.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Page {
    pub id: i64,
    pub name: String,
    /// Pages-index `<Page>` attributes (`ID`, `NameU`, `ViewScale`, …).
    pub attrs: Attrs,
    /// Index children (`PageSheet`, `Rel`, …) in order.
    pub children: Vec<IndexChild>,
    pub shapes: Vec<Shape>,
    /// Opening-tag attrs of the `<PageContents>` root.
    pub contents_attrs: Attrs,
    /// Unknown children of the contents root (`<Connects>`, …), verbatim.
    pub contents_raw: Vec<RawEl>,
    /// Part name the page contents were parsed from ("" for stencil pages
    /// that have no page part).
    pub part: String,
}

impl Page {
    /// The page's line-jump display switch (#251): the PageSheet's
    /// `LineJumpCode` (the PageLayout-section cell — the oracle
    /// enumeration of a real Visio-authored page carried
    /// `LineJumpCode = 1`, Visio's "horizontal lines jump" default).
    /// Absent shows (the shipped corpus carries no jump cells at
    /// all). **0 = visPLOJumpNone** — the documented off switch
    /// ([MS-Learn] LineJumpCode cell, Page Layout section; values
    /// 0–5). 6 stays treated as off too (#218 review: files written
    /// by tools that use it as "none" keep their intent).
    pub fn line_jumps_shown(&self) -> bool {
        self.sheet()
            .and_then(|s| s.cell("LineJumpCode"))
            .and_then(|c| c.numeric())
            .map(|v| v != 0.0 && v != 6.0)
            .unwrap_or(true)
    }

    /// The page's sheet: the inline `<PageSheet>` from the pages index.
    pub fn sheet(&self) -> Option<&Sheet> {
        self.children.iter().find_map(|c| match c {
            IndexChild::Sheet { sheet, .. } => Some(sheet),
            _ => None,
        })
    }

    /// [`Page::sheet`] for mutation (page-default cells a writer or
    /// test sets in place).
    pub fn sheet_mut(&mut self) -> Option<&mut Sheet> {
        self.children.iter_mut().find_map(|c| match c {
            IndexChild::Sheet { sheet, .. } => Some(sheet),
            _ => None,
        })
    }
}

/// A stylesheet from `document.xml`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StyleSheet {
    pub id: i64,
    pub name_u: Option<String>,
    pub attrs: Attrs,
    pub sheet: Sheet,
}

/// A child of `<VisioDocument>`, in document order (round-trip fidelity:
/// unknown children are kept as [`RawEl`] and re-emitted in place).
#[derive(Debug, Clone, PartialEq)]
pub enum DocItem {
    StyleSheets,
    Colors,
    DocumentSheet,
    Raw(RawEl),
}

/// A media part added at runtime (GitHub #113, picture insertion):
/// the package part (`visio/media/imageN.png`), its bytes, the OWNING
/// page id, and the relationship id that page's `ForeignData`
/// references — the writer materializes the part, the page-part rels
/// entry, and the content-type default.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaRef {
    pub part: String,
    pub bytes: Vec<u8>,
    pub page: i64,
    pub rid: String,
    /// `png` / `jpeg` — extension and content-type family.
    pub ext: String,
}

/// A fully parsed VSDX-family package.
#[derive(Debug, Clone, Default)]
pub struct Document {
    pub pages: Vec<Page>,
    /// Runtime-added media parts (#113): empty for parsed-only docs;
    /// the write path merges them (retained-package documents only).
    pub extra_media: Vec<MediaRef>,
    /// Masters in masters-index order.
    pub masters: Vec<Master>,
    /// Stylesheets in document order.
    pub stylesheets: Vec<StyleSheet>,
    /// Color table entries (`Colors`) in document order.
    pub colors: Vec<(i64, String)>,
    /// `<VisioDocument>` children in document order.
    pub items: Vec<DocItem>,
    /// Document-level sheet (`DocumentSheet`).
    pub sheet: Sheet,
    /// `<VisioDocument>` opening-tag attributes (namespaces included).
    pub root_attrs: Attrs,
    /// `<Pages>`/`<Masters>` index root attributes.
    pub pages_attrs: Attrs,
    pub masters_attrs: Attrs,
    /// Main document part name (usually `visio/document.xml`).
    pub doc_part: String,
    /// Pages-index part name (usually `visio/pages/pages.xml`).
    pub pages_part: String,
    /// Masters-index part name, when the document has masters.
    pub masters_part: Option<String>,
    /// The raw source package, retained so unmodeled parts round-trip
    /// byte-for-byte. `None` for documents built in code.
    pub package: Option<Package>,
    /// The theme's variant colors, read once from `package` on first
    /// use: style resolution asks for them on every THEMEVAL cell, and
    /// re-reading the theme XML each time made big pages crawl.
    pub theme_variants: std::sync::OnceLock<Vec<String>>,
}

/// Text Block Format cells without a `Txt`/`Text` prefix. They belong
/// to the text style in Visio, so the style cascade must start at the
/// shape's `TextStyle`, not its line or fill style.
fn is_text_block_cell(name: &str) -> bool {
    matches!(
        name,
        "VerticalAlign"
            | "LeftMargin"
            | "RightMargin"
            | "TopMargin"
            | "BottomMargin"
            | "DefaultTabStop"
    )
}

impl Document {
    pub fn master(&self, id: i64) -> Option<&Master> {
        self.masters.iter().find(|m| m.id == id)
    }
    pub fn stylesheet(&self, id: i64) -> Option<&StyleSheet> {
        self.stylesheets.iter().find(|s| s.id == id)
    }
    pub fn color(&self, ix: i64) -> Option<&String> {
        self.colors.iter().find(|(i, _)| *i == ix).map(|(_, c)| c)
    }

    /// Resolve a page-layout cell (the PageSheet's PageLayout section —
    /// `LineJumpFactorX`, `LineToLineX`, …) through the page's style
    /// inheritance: the PageSheet's own cell first, then the chain its
    /// `LineStyle` attribute opens. Visio writes pages with
    /// `LineStyle="0"` and **no inline layout cells** — the "No Style"
    /// stylesheet in document.xml carries the values, which is how
    /// Visio itself resolves them (GitHub #294: the shipped repro's
    /// page sets nothing and Visio still hops with `LineJumpFactorX =
    /// 0.6667` from stylesheet 0; reading the PageSheet alone fell
    /// back to 1.0). `None` when nothing in the chain carries the cell.
    pub fn resolve_page_layout_cell(&self, page: &Page, name: &str) -> Option<Cell> {
        let sheet = page.sheet()?;
        if let Some(c) = sheet.cell(name) {
            if !c.is_inherited() {
                return Some(c.clone());
            }
        }
        let style_id = sheet
            .attrs
            .iter()
            .find(|(n, _)| n == "LineStyle")
            .and_then(|(_, v)| v.parse::<i64>().ok())
            .unwrap_or(0);
        self.stylesheet_cascade_cell(style_id, name)
    }

    /// The page's resolved `LineToLineX` (inches — the horizontal
    /// clearance between parallel connectors), falling back to Visio's
    /// 0.125 in default when neither the PageSheet nor its style chain
    /// carries it (#294).
    pub fn page_line_to_line_x(&self, page: &Page) -> f64 {
        self.resolve_page_layout_cell(page, "LineToLineX")
            .and_then(|c| c.numeric())
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(0.125)
    }

    /// The page's resolved `LineToLineY` (inches — the vertical
    /// clearance), falling back to Visio's 0.125 in default like the X
    /// twin (#294; PR #299 review — vertical stored jumps are authored
    /// at the Y spacing, so the Y cells need their own resolver).
    pub fn page_line_to_line_y(&self, page: &Page) -> f64 {
        self.resolve_page_layout_cell(page, "LineToLineY")
            .and_then(|c| c.numeric())
            .filter(|v| v.is_finite() && *v > 0.0)
            .unwrap_or(0.125)
    }

    /// The page's resolved `LineJumpFactorX` (Visio's jump-size
    /// multiplier), falling back to Visio's 2/3 default (#294 — the
    /// shipped corpus's "No Style" and the #294 repro both carry
    /// 0.66666666666667; the old PageSheet-only read defaulted to 1.0).
    /// An EXPLICIT 0 is valid ([MS-Learn] LineJumpFactorX cell) and
    /// means zero-size hops — only missing or non-finite/negative
    /// values fall back (#294 review).
    pub fn page_line_jump_factor(&self, page: &Page) -> f64 {
        self.resolve_page_layout_cell(page, "LineJumpFactorX")
            .and_then(|c| c.numeric())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(2.0 / 3.0)
    }

    /// The page's resolved `LineJumpFactorY` — the vertical hop-size
    /// multiplier, same fallback contract as the X twin (PR #299
    /// review).
    pub fn page_line_jump_factor_y(&self, page: &Page) -> f64 {
        self.resolve_page_layout_cell(page, "LineJumpFactorY")
            .and_then(|c| c.numeric())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(2.0 / 3.0)
    }

    /// Deep model equality ignoring the retained raw package (part bytes
    /// legitimately differ after a write).
    pub fn same_model(&self, other: &Document) -> bool {
        self.pages == other.pages
            && self.masters == other.masters
            && self.stylesheets == other.stylesheets
            && self.colors == other.colors
            && self.items == other.items
            && self.sheet == other.sheet
            && self.root_attrs == other.root_attrs
            && self.doc_part == other.doc_part
            && self.pages_part == other.pages_part
            && self.masters_part == other.masters_part
    }

    /// The master-side shape whose sheet an instance inherits from.
    ///
    /// `Master='8'` resolves inside master 8 (optionally to its `MasterShape`
    /// sub-shape). A shape carrying only `MasterShape` inherits from that
    /// sub-shape of the nearest enclosing group's master — the standard
    /// layout for group instances. A bare sub-shape of a master-backed group
    /// falls back to ID matching inside that master.
    pub fn instance_source<'a>(
        &'a self,
        shape: &'a Shape,
        ancestors: &[&'a Shape],
    ) -> Option<&'a Shape> {
        if let Some(mid) = shape.master {
            let m = self.master(mid)?;
            return match shape.master_shape {
                Some(msid) => find_shape_by_id(&m.shapes, msid),
                None => m.shapes.first(),
            };
        }
        // Nearest ancestor that instantiates a master.
        let enclosing = ancestors.iter().rev().find(|a| a.master.is_some())?;
        let m = self.master(enclosing.master?)?;
        if let Some(msid) = shape.master_shape {
            return find_shape_by_id(&m.shapes, msid);
        }
        // Bare sub-shape: match by ID inside the master tree. Visio
        // renumbers inline copies (`Name.NNN` suffix), so fall back to the
        // NameU base when the ID does not exist in the master.
        find_shape_by_id(&m.shapes, shape.id).or_else(|| {
            let name = shape.name_u.as_deref()?;
            let (base, sfx) = name.rsplit_once('.')?;
            if sfx.is_empty() || !sfx.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            find_shape_by_name_u(&m.shapes, base)
        })
    }

    /// Resolve a cell for `shape` walking the inheritance chain:
    /// shape → master root shape (or master sub-shape via `MasterShape`)
    /// → stylesheet (shape's LineStyle/FillStyle/TextStyle default 0) →
    /// built-in defaults.
    /// Like [`resolve_cell`](Self::resolve_cell) but returns `None` when
    /// no explicit value exists on the shape, its master, or the style
    /// cascade — i.e. stops before the document-sheet and builtin
    /// fallbacks. Used to spot values that are only defaults (e.g. 1-D
    /// connectors must not inherit the builtin FillPattern=1).
    pub fn resolve_cell_explicit(
        &self,
        shape: &Shape,
        ancestors: &[&Shape],
        name: &str,
    ) -> Option<Cell> {
        if let Some(c) = shape.sheet.cell(name) {
            if c.v.is_some() && is_xform_cell(name) {
                return Some(c.clone());
            }
            if (!c.is_inherited() && !c.is_themed()) || self.theme_evaluates(c) {
                return Some(c.clone());
            }
            if c.is_theme_eval() {
                return self.stylesheet_cascade_cell(0, name);
            }
        }
        if let Some(t) = self.instance_source(shape, ancestors) {
            if let Some(c) = t.sheet.cell(name) {
                if (!c.is_inherited() && !c.is_themed()) || self.theme_evaluates(c) {
                    return Some(c.clone());
                }
                if c.is_theme_eval() {
                    return self.stylesheet_cascade_cell(0, name);
                }
            }
        }
        let style_id = self.shape_style_for(shape, ancestors, name);
        self.stylesheet_cascade_cell(style_id, name)
    }

    /// Variant colors from `visio/theme/theme1.xml` (DrawingML scheme),
    /// indexed by `THEMEVAL("VariantColorN")` formulas (N = 1…8).
    pub fn theme_variant_colors(&self) -> Vec<String> {
        self.theme_variants_cached().to_vec()
    }

    /// [`Document::theme_variant_colors`] without the copy, parsed once.
    fn theme_variants_cached(&self) -> &[String] {
        self.theme_variants
            .get_or_init(|| self.parse_theme_variant_colors())
    }

    fn parse_theme_variant_colors(&self) -> Vec<String> {
        let Some(pkg) = self.package.as_ref() else {
            return Vec::new();
        };
        let Some(raw) = pkg.part("visio/theme/theme1.xml") else {
            return Vec::new();
        };
        let xml = String::from_utf8_lossy(raw).into_owned();
        let mut out = Vec::new();
        // <vt:varColorN><a:srgbClr val="RRGGBB"/>...</vt:varColorN>
        for n in 1..=8 {
            let tag = format!("varColor{n}");
            let Some(pos) = xml.find(&tag) else { continue };
            let window = &xml[pos..(pos + 300).min(xml.len())];
            if let Some(vpos) = window.find("srgbClr val=\"") {
                let rest = &window[vpos + 13..];
                if let Some(end) = rest.find('"') {
                    if end == 6 {
                        out.push(format!("#{}", &rest[..end]));
                        continue;
                    }
                }
            }
            break;
        }
        out
    }

    /// A Themed `THEMEVAL("VariantColorN")` cell that this document's
    /// theme actually evaluates: it is a real value (the renderer reads
    /// the variant color from the formula), not a theme-less No Style
    /// fallback (Greptile on PR #335).
    fn theme_evaluates(&self, c: &Cell) -> bool {
        let Some(f) = c.f.as_deref() else {
            return false;
        };
        if !c.is_theme_eval() || !f.contains("THEMEVAL") {
            return false;
        }
        let variants = self.theme_variants_cached().len();
        (1..=variants).any(|n| f.contains(&format!("VariantColor{n}")))
    }

    /// The stylesheet a shape's `name` cell cascades into: the shape's
    /// own LineStyle/FillStyle/TextStyle attribute, else its master
    /// source's — a master instance (or sub-shape) written without
    /// style attributes uses the master shape's styles, not No Style
    /// (Visio 2021 witness: network-stencil sub-shapes take their
    /// gradient stops from "Tower Front" & co.).
    pub fn shape_style_for(&self, shape: &Shape, ancestors: &[&Shape], name: &str) -> i64 {
        let pick = |s: &Shape| match name {
            n if n.starts_with("Line") => s.line_style,
            n if n.starts_with("Fill") => s.fill_style,
            n if n.starts_with("Txt") | n.starts_with("Text") | is_text_block_cell(n) => {
                s.text_style
            }
            _ => s.line_style.or(s.fill_style),
        };
        pick(shape)
            .or_else(|| self.instance_source(shape, ancestors).and_then(pick))
            .unwrap_or(0)
    }

    /// The shared stylesheet walk (GitHub #294): `start_id`'s sheet first,
    /// then up the style chain, 8 links deep — the linking `LineStyle`
    /// cell when a style carries one, else the style's own parent
    /// attribute for this kind of cell (`FillStyle` for Fill cells,
    /// `TextStyle` for text-block cells, `LineStyle` otherwise; the
    /// schema carries the link in both places). A `V='Themed'` value is
    /// no value without an evaluated theme ([`Cell::is_themed`]), so the
    /// walk continues past it — the shipped stencils chain
    /// Normal → Theme → No Style through attributes, and the Theme
    /// style's `THEMEVAL()` cells used to stop the walk (or, before the
    /// attribute link was followed, the cell-only walk ended early) and
    /// paint theme-less drawings solid black.
    fn stylesheet_cascade_cell(&self, start_id: i64, name: &str) -> Option<Cell> {
        let mut cur = self.stylesheet(start_id)?;
        // The style's own parent for THIS kind of cell (a style's
        // FillStyle/LineStyle/TextStyle attributes name three parents).
        let parent_attr = match name {
            n if n.starts_with("Fill") => "FillStyle",
            n if n.starts_with("Txt") | n.starts_with("Text") | is_text_block_cell(n) => {
                "TextStyle"
            }
            _ => "LineStyle",
        };
        for _ in 0..8 {
            if let Some(c) = cur.sheet.cell(name) {
                if (!c.is_inherited() && !c.is_themed()) || self.theme_evaluates(c) {
                    return Some(c.clone());
                }
                // THEMEVAL() with no evaluated theme: the No Style value.
                if c.is_theme_eval() && cur.id != 0 {
                    if let Some(no_style) = self.stylesheet(0) {
                        if let Some(c) = no_style.sheet.cell(name) {
                            if !c.is_themed() {
                                return Some(c.clone());
                            }
                        }
                    }
                    return None;
                }
            }
            // A style without its own value — absent, `Inh`, or Themed
            // with no evaluated theme — inherits from its parent style
            // for this kind of cell: the linking cell when present,
            // else the style's FillStyle/LineStyle/TextStyle attribute
            // (the only link the stencils' Normal → Theme → No Style
            // chain carries).
            let next_id = cur
                .sheet
                .cell("LineStyle")
                .and_then(|c| c.numeric())
                .map(|v| v as i64)
                .or_else(|| {
                    cur.attrs
                        .iter()
                        .find(|(n, _)| n == parent_attr)
                        .and_then(|(_, v)| v.parse::<i64>().ok())
                });
            match next_id {
                Some(nid) if nid != cur.id => match self.stylesheet(nid) {
                    Some(next) => cur = next,
                    None => break,
                },
                _ => break,
            }
        }
        None
    }

    pub fn resolve_cell(&self, shape: &Shape, ancestors: &[&Shape], name: &str) -> Option<Cell> {
        if let Some(c) = shape.sheet.cell(name) {
            // Xform/placement cells keep an instance-local snapshot under
            // F='Inh': Visio caches the resolved position in `V` when the
            // master is instanced (the master's own value is its canvas
            // placement, meaningless on the page). Style cells
            // (LineWeight, FillForegnd, …) live-inherit: master wins.
            if c.v.is_some() && is_xform_cell(name) {
                return Some(c.clone());
            }
            if (!c.is_inherited() && !c.is_themed()) || self.theme_evaluates(c) {
                return Some(c.clone());
            }
            // THEMEVAL() with no evaluated theme: the No Style value, else
            // the terminal defaults — never the master's or the style's
            // value (CodeRabbit on PR #335).
            if c.is_theme_eval() {
                return self
                    .stylesheet_cascade_cell(0, name)
                    .or_else(|| self.terminal_default(name));
            }
        }
        // Master inheritance.
        if let Some(t) = self.instance_source(shape, ancestors) {
            if let Some(c) = t.sheet.cell(name) {
                if (!c.is_inherited() && !c.is_themed()) || self.theme_evaluates(c) {
                    return Some(c.clone());
                }
                if c.is_theme_eval() {
                    return self
                        .stylesheet_cascade_cell(0, name)
                        .or_else(|| self.terminal_default(name));
                }
            }
        }
        // Stylesheet cascade.
        let style_id = self.shape_style_for(shape, ancestors, name);
        if let Some(c) = self.stylesheet_cascade_cell(style_id, name) {
            return Some(c);
        }
        self.terminal_default(name)
    }

    /// The last resort below the style cascade: the document sheet's
    /// own value, else the built-in default.
    fn terminal_default(&self, name: &str) -> Option<Cell> {
        if let Some(c) = self.sheet.cell(name) {
            if !c.is_inherited() {
                return Some(c.clone());
            }
        }
        builtin_defaults()
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| Cell {
                v: Some(v.to_string()),
                u: None,
                f: None,
                attrs: Default::default(),
                raw_inner: None,
            })
    }

    /// The shape (or master shape) whose sheet geometry should be rendered
    /// for `shape` — i.e. the effective sheet after master inheritance.
    pub fn effective_sheet<'a>(
        &'a self,
        shape: &'a Shape,
        ancestors: &[&'a Shape],
    ) -> Cow<'a, Sheet> {
        if shape.sheet.has_sections() {
            return Cow::Borrowed(&shape.sheet);
        }
        if let Some(src) = self.instance_source(shape, ancestors) {
            return Cow::Borrowed(&src.sheet);
        }
        Cow::Borrowed(&shape.sheet)
    }

    /// Resolve a named User row for `shape` across the master chain: the
    /// instance sheet first, then the master root shape's sheet — the
    /// same layering [`resolve_cell`](Self::resolve_cell) uses for
    /// cells. `effective_sheet` cannot serve this: it is either/or (a
    /// sheet with ANY local sections returns only that sheet), so a real
    /// Visio-authored instance — which virtually always carries local
    /// Geometry/Character snapshots — would hide the master root's
    /// `User.msvStructureType` and friends, breaking container
    /// recognition for exactly the containers that carry the row
    /// (#191).
    pub fn effective_user_row<'a>(
        &'a self,
        shape: &'a Shape,
        ancestors: &[&'a Shape],
        name: &str,
    ) -> Option<&'a Row> {
        if let Some(r) = shape
            .sheet
            .sections_named("User")
            .iter()
            .find_map(|sec| sec.named_row(name))
        {
            return Some(r);
        }
        let src = self.instance_source(shape, ancestors)?;
        src.sheet
            .sections_named("User")
            .iter()
            .find_map(|sec| sec.named_row(name))
    }

    /// Resolve a section (by name and IX) for `shape` across the master
    /// chain: the instance sheet first, then the master root's — the
    /// same layering [`effective_user_row`](Self::effective_user_row)
    /// uses for rows. Like that accessor, `effective_sheet` cannot
    /// serve this: it is either/or (a sheet with ANY local sections
    /// returns only that sheet), so a real Visio-authored instance
    /// would hide the master root's other sections — the seed
    /// `SetRowCell` materializes a missing section from (#196).
    pub fn effective_section<'a>(
        &'a self,
        shape: &'a Shape,
        ancestors: &[&'a Shape],
        name: &str,
        ix: i64,
    ) -> Option<&'a Section> {
        if let Some(s) = shape.sheet.section(name, ix) {
            return Some(s);
        }
        let src = self.instance_source(shape, ancestors)?;
        src.sheet.section(name, ix)
    }
}

/// Built-in defaults observed in shipped stylesheets ("No Style", ID 0).
pub fn builtin_defaults() -> Vec<(&'static str, f64)> {
    vec![
        // (name, value in internal units: inches / radians / ints)
        ("LineWeight", 0.01041666666666667),
        ("LinePattern", 1.0),
        ("LineColor", 0.0),
        ("FillPattern", 1.0),
        ("FillForegnd", 1.0),
        ("FillBkgnd", 0.0),
        ("BeginArrow", 0.0),
        ("EndArrow", 0.0),
        ("Rounding", 0.0),
    ]
}

/// Cells whose `F='Inh'` marks an instance-time snapshot rather than live
/// master inheritance (see `resolve_cell`).
fn is_xform_cell(name: &str) -> bool {
    matches!(
        name,
        "PinX"
            | "PinY"
            | "LocPinX"
            | "LocPinY"
            | "Width"
            | "Height"
            | "Angle"
            | "FlipX"
            | "FlipY"
            | "TxtPinX"
            | "TxtPinY"
            | "TxtLocPinX"
            | "TxtLocPinY"
            | "TxtWidth"
            | "TxtHeight"
            | "TxtAngle"
            | "BeginX"
            | "BeginY"
            | "EndX"
            | "EndY"
    )
}

/// Evaluation environment for re-evaluating a master root's cells at
/// instance size — shared by the SVG renderer's master mirroring
/// (issue #15) and the editor's flattened-group cache refresh
/// (issue #59), so both compute placements identically. Seeds the
/// group's effective `Width`/`Height` under the bare and `Sheet.N!`
/// spellings master formulas use, plus the root's User cells — their
/// formulas re-evaluate too, because layout cells like
/// `User.IconY = Height - User.InfoPanelHeight/2` reference the
/// (instance) `Height`. Instance-local User rows override the master's
/// (an instance can change editable rows such as `ShapeLayout`), and
/// two passes let a User formula reference another User formula.
pub fn instance_env(
    master_root: &Shape,
    instance: &Shape,
    width: f64,
    height: f64,
) -> formula::Env {
    let mut env = formula::Env::new();
    env.insert("Width".into(), width);
    env.insert("Height".into(), height);
    env.insert(format!("Sheet.{}!Width", master_root.id), width);
    env.insert(format!("Sheet.{}!Height", master_root.id), height);
    let mut users: Vec<(String, Cell)> = Vec::new();
    for user in master_root.sheet.sections_named("User") {
        for row in user.rows.iter().filter(|r| r.n.is_some()) {
            if let Some(c) = row.cell("Value") {
                users.push((row.n.clone().unwrap(), c.clone()));
            }
        }
    }
    for user in instance.sheet.sections_named("User") {
        for row in user.rows.iter().filter(|r| r.n.is_some()) {
            if let Some(c) = row.cell("Value") {
                // An instance row cached under F='Inh' is a frozen
                // snapshot of the master's formula — the master row (and
                // its formula, re-evaluated below) still owns the value.
                // Only a genuine local override (V with no F, or a real
                // formula — e.g. an instance's ShapeLayout=4) replaces
                // the master's row. Treating caches as overrides would
                // freeze layout cells like User.IconY at their save-time
                // value when re-evaluating after an edit (issue #59).
                if c.is_inherited() {
                    continue;
                }
                if let Some(slot) = users
                    .iter_mut()
                    .find(|(n, _)| n == row.n.as_deref().unwrap())
                {
                    slot.1 = c.clone();
                } else {
                    users.push((row.n.clone().unwrap(), c.clone()));
                }
            }
        }
    }
    let set = |env: &mut formula::Env, name: &str, v: f64| {
        env.insert(format!("User.{name}"), v);
        env.insert(format!("Sheet.{}!User.{name}", master_root.id), v);
    };
    for (name, c) in &users {
        if let Some(v) = c.numeric().filter(|v| v.is_finite()) {
            set(&mut env, name, v);
        }
    }
    for _ in 0..2 {
        let mut changed = false;
        for (name, c) in &users {
            let Some(f) = c.f.as_deref() else { continue };
            if f == "Inh" || f == "No Formula" {
                continue;
            }
            if let Some(v) = formula::eval(f, &env)
                .ok()
                .and_then(|v| v.as_f64())
                .filter(|v| v.is_finite())
            {
                let cur = env.get(&format!("User.{name}")).copied();
                if cur != Some(v) {
                    set(&mut env, name, v);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    env
}

/// A sub-shape's evaluation environment base: only QUALIFIED root
/// entries (`Sheet.N!…`) carry over — a bare name in a child formula
/// means the child's own cell, so the root's unqualified Width/Height
/// and User.* must not leak in. The caller then seeds the child's own
/// effective `Width`/`Height` (child sheets are not in the root env;
/// bare child-cell references fail to resolve and fall back).
pub fn qualified_env(root_env: &formula::Env) -> formula::Env {
    root_env
        .iter()
        .filter(|(k, _)| k.contains('!'))
        .map(|(k, v)| (k.clone(), *v))
        .collect()
}

pub fn find_shape_by_id(shapes: &[Shape], id: i64) -> Option<&Shape> {
    for s in shapes {
        if s.id == id {
            return Some(s);
        }
        if let Some(found) = find_shape_by_id(&s.sub_shapes, id) {
            return Some(found);
        }
    }
    None
}

/// Mutable counterpart of [`find_shape_by_id`].
pub fn find_shape_by_id_mut(shapes: &mut [Shape], id: i64) -> Option<&mut Shape> {
    for s in shapes {
        if s.id == id {
            return Some(s);
        }
        if let Some(found) = find_shape_by_id_mut(&mut s.sub_shapes, id) {
            return Some(found);
        }
    }
    None
}

/// Depth-first `NameU` lookup within a shape tree (used for matching inline
/// copies to their master originals).
pub fn find_shape_by_name_u<'a>(shapes: &'a [Shape], name: &str) -> Option<&'a Shape> {
    for s in shapes {
        if s.name_u.as_deref() == Some(name) {
            return Some(s);
        }
        if let Some(found) = find_shape_by_name_u(&s.sub_shapes, name) {
            return Some(found);
        }
    }
    None
}

/// Snap & glue configuration stored in `<DocumentSettings>` (GitHub
/// #109). Bit values follow the Visio automation enums so documents
/// interoperate with the shipped app. Snap: GRID 1, RULER_SUBDIVISIONS
/// 2, GUIDES 4, SELECTION_HANDLES 8, VERTICES 16, CONNECTION_POINTS 32,
/// GEOMETRY 64, ALIGNMENT_BOXES 128, SHAPE_EXTENSIONS 256,
/// SHAPE_INTERSECTIONS 512. Glue: GUIDES 1, SHAPE_HANDLES 2,
/// SHAPE_VERTICES 4, CONNECTION_POINTS 8. The shipped Visio 2016 corpus
/// defaults are SnapSettings 295 (grid, ruler subdivisions, guides,
/// connection points, shape extensions), GlueSettings 9 (guides,
/// connection points), DynamicGridEnabled 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DocSnapGlue {
    pub snap: u32,
    pub glue: u32,
    pub dynamic_grid: bool,
}

impl DocSnapGlue {
    /// The shipped Visio 2016 corpus defaults (see the struct docs).
    pub const CORPUS_DEFAULT: DocSnapGlue = DocSnapGlue {
        snap: 295,
        glue: 9,
        dynamic_grid: true,
    };
}

/// `SnapSettings` bit values (Visio automation `VisSnapSettings` /
/// `VisSnapSettings2` enums).
pub mod snap_bits {
    pub const GRID: u32 = 1;
    pub const RULER_SUBDIVISIONS: u32 = 2;
    pub const GUIDES: u32 = 4;
    pub const SELECTION_HANDLES: u32 = 8;
    pub const VERTICES: u32 = 16;
    pub const CONNECTION_POINTS: u32 = 32;
    pub const GEOMETRY: u32 = 64;
    pub const ALIGNMENT_BOXES: u32 = 128;
    pub const SHAPE_EXTENSIONS: u32 = 256;
    pub const SHAPE_INTERSECTIONS: u32 = 512;
}

/// `GlueSettings` bit values (Visio automation `VisGlueSettings`).
pub mod glue_bits {
    pub const GUIDES: u32 = 1;
    pub const SHAPE_HANDLES: u32 = 2;
    pub const SHAPE_VERTICES: u32 = 4;
    pub const CONNECTION_POINTS: u32 = 8;
}

/// Canonical `<DocumentSettings>` child order (the corpus shape):
/// a missing snap/glue child is inserted before the first child that
/// follows it in this order, so a patched element keeps Visio's layout.
const SETTINGS_CHILD_ORDER: &[&str] = &[
    "GlueSettings",
    "SnapSettings",
    "SnapExtensions",
    "SnapAngles",
    "DynamicGridEnabled",
    "ProtectStyles",
    "ProtectShapes",
    "ProtectMasters",
    "ProtectBkgnds",
];

/// Text of `<Name>value</Name>` inside `inner`, when present.
fn read_settings_child(inner: &str, name: &str) -> Option<u32> {
    let open = format!("<{name}>");
    let p = inner.find(&open)? + open.len();
    let rest = &inner[p..];
    let end = rest.find(&format!("</{name}>"))?;
    rest[..end].trim().parse().ok()
}

/// Replace (or insert, in canonical position) one settings child.
fn set_settings_child(inner: &str, name: &str, value: &str) -> String {
    let full = format!("<{name}>{value}</{name}>");
    // Paired form: swap the inner text, keep everything else verbatim.
    if let Some(p) = inner.find(&format!("<{name}>")) {
        if let Some(end) = inner[p..].find(&format!("</{name}>")) {
            let close = p + end + name.len() + 3;
            let mut out = String::with_capacity(inner.len() + value.len());
            out.push_str(&inner[..p]);
            out.push_str(&full);
            out.push_str(&inner[close..]);
            return out;
        }
    }
    // Self-closing form: expand in place.
    let sc = format!("<{name}/>");
    if let Some(p) = inner.find(&sc) {
        let mut out = String::with_capacity(inner.len() + value.len());
        out.push_str(&inner[..p]);
        out.push_str(&full);
        out.push_str(&inner[p + sc.len()..]);
        return out;
    }
    // Absent: insert before the first canonically-later child, else
    // append at the end.
    let later = &SETTINGS_CHILD_ORDER[SETTINGS_CHILD_ORDER
        .iter()
        .position(|n| *n == name)
        .map_or(0, |i| i + 1)..];
    for cand in later {
        for form in [format!("<{cand}>"), format!("<{cand}/>")] {
            if let Some(p) = inner.find(&form) {
                let mut out = String::with_capacity(inner.len() + full.len());
                out.push_str(&inner[..p]);
                out.push_str(&full);
                out.push_str(&inner[p..]);
                return out;
            }
        }
    }
    format!("{inner}{full}")
}

impl Document {
    /// The document's snap & glue settings, straight from a
    /// `<DocumentSettings>` element. `None` when the document carries
    /// no such element (a fresh blank package); a present element with
    /// an absent child reads that child as its shipped default.
    pub fn doc_snap_glue(&self) -> Option<DocSnapGlue> {
        let raw = self.items.iter().find_map(|it| match it {
            DocItem::Raw(r) if r.name == "DocumentSettings" => Some(r),
            _ => None,
        })?;
        let d = DocSnapGlue::CORPUS_DEFAULT;
        Some(DocSnapGlue {
            snap: read_settings_child(&raw.inner, "SnapSettings").unwrap_or(d.snap),
            glue: read_settings_child(&raw.inner, "GlueSettings").unwrap_or(d.glue),
            dynamic_grid: read_settings_child(&raw.inner, "DynamicGridEnabled")
                .map(|v| v != 0)
                .unwrap_or(d.dynamic_grid),
        })
    }

    /// Write the snap & glue cells into `<DocumentSettings>`, creating
    /// the element (first child of `VisioDocument`, corpus order) when
    /// the document has none. Only the three modeled children are
    /// touched; attributes and every other child — `SnapExtensions`,
    /// `SnapAngles`, `Protect*` — stay verbatim.
    pub fn set_doc_snap_glue(&mut self, s: DocSnapGlue) {
        let dg = if s.dynamic_grid { "1" } else { "0" };
        if let Some(ix) = self
            .items
            .iter()
            .position(|it| matches!(it, DocItem::Raw(r) if r.name == "DocumentSettings"))
        {
            if let DocItem::Raw(r) = &mut self.items[ix] {
                r.inner = set_settings_child(&r.inner, "GlueSettings", &s.glue.to_string());
                r.inner = set_settings_child(&r.inner, "SnapSettings", &s.snap.to_string());
                r.inner = set_settings_child(&r.inner, "DynamicGridEnabled", dg);
            }
            return;
        }
        // No element yet (the blank package): synthesize one with the
        // corpus child set — SnapExtensions 34 and an empty SnapAngles
        // are the shipped defaults, written once and then verbatim.
        let mut inner = String::new();
        inner = set_settings_child(&inner, "GlueSettings", &s.glue.to_string());
        inner = set_settings_child(&inner, "SnapSettings", &s.snap.to_string());
        inner = set_settings_child(&inner, "SnapExtensions", "34");
        inner.push_str("<SnapAngles/>");
        inner = set_settings_child(&inner, "DynamicGridEnabled", dg);
        self.items.insert(
            0,
            DocItem::Raw(RawEl {
                name: "DocumentSettings".to_string(),
                attrs: Vec::new(),
                inner,
            }),
        );
    }
}

#[cfg(test)]
mod snap_glue_tests {
    use super::*;

    /// #294: a Visio page carries NO inline layout cells — `LineStyle="0"`
    /// hands resolution to the "No Style" stylesheet in document.xml
    /// (the shipped repro: LineToLineX 0.125, LineJumpFactorX 0.66667
    /// on stylesheet 0, nothing on the PageSheet). Reading the
    /// PageSheet alone fell back to 1.0 and drew 3× jumps.
    #[test]
    fn page_layout_cells_resolve_through_the_style_chain() {
        let mut doc = Document::default();
        let mut page = Page::default();
        let mut sheet = Sheet::default();
        sheet.attrs.push(("LineStyle".to_string(), "0".to_string()));
        page.children.push(IndexChild::Sheet {
            tag: "PageSheet".to_string(),
            sheet,
        });
        // NON-default values: the fixture must fail if resolution
        // skipped style 0 and fell through to the Visio defaults (the
        // bare-page case below pins those separately, #294 review).
        let mut style = StyleSheet {
            id: 0,
            ..Default::default()
        };
        style.sheet.set_cell("LineToLineX", Cell::value("0.2"));
        style.sheet.set_cell("LineJumpFactorX", Cell::value("1.5"));
        doc.stylesheets.push(style);
        doc.pages.push(page);

        assert!(
            (doc.page_line_to_line_x(&doc.pages[0]) - 0.2).abs() < 1e-9,
            "LineToLineX comes off the style chain"
        );
        assert!(
            (doc.page_line_jump_factor(&doc.pages[0]) - 1.5).abs() < 1e-9,
            "LineJumpFactorX comes off the style chain"
        );
        let radius = doc.page_line_to_line_x(&doc.pages[0])
            * doc.page_line_jump_factor(&doc.pages[0])
            / 2.0;
        assert!((radius - 0.15).abs() < 1e-9);

        // An inline PageSheet cell wins over the chain (the GUI's jump
        // size dialog writes here).
        doc.pages[0]
            .sheet_mut()
            .unwrap()
            .set_cell("LineJumpFactorX", Cell::value("2"));
        assert!((doc.page_line_jump_factor(&doc.pages[0]) - 2.0).abs() < 1e-9);

        // Attribute-only parent links (#294 review): the schema carries
        // the parent in the StyleSheet's LineStyle ATTRIBUTE as well as
        // the cell — a style without the linking cell used to stop the
        // walk at the fallback.
        let mut chain = Document::default();
        let mut page = Page::default();
        let mut sheet = Sheet::default();
        sheet.attrs.push(("LineStyle".to_string(), "7".to_string()));
        page.children.push(IndexChild::Sheet {
            tag: "PageSheet".to_string(),
            sheet,
        });
        let mut mid = StyleSheet {
            id: 7,
            ..Default::default()
        };
        mid.attrs
            .push(("LineStyle".to_string(), "8".to_string()));
        let mut top = StyleSheet {
            id: 8,
            ..Default::default()
        };
        top.sheet.set_cell("LineJumpFactorX", Cell::value("2"));
        chain.stylesheets.extend([mid, top]);
        chain.pages.push(page);
        assert!(
            (chain.page_line_jump_factor(&chain.pages[0]) - 2.0).abs() < 1e-9,
            "the attribute parent link continues the walk"
        );

        // An EXPLICIT 0 is a zero-size hop, not the default (#294
        // review) — the overlay skips radius-0 pages.
        doc.pages[0]
            .sheet_mut()
            .unwrap()
            .set_cell("LineJumpFactorX", Cell::value("0"));
        assert_eq!(doc.page_line_jump_factor(&doc.pages[0]), 0.0);

        // Nothing anywhere — Visio's defaults, never 1.0.
        let mut bare = Document::default();
        let mut bare_page = Page::default();
        bare_page.children.push(IndexChild::Sheet {
            tag: "PageSheet".to_string(),
            sheet: Sheet::default(),
        });
        bare.pages.push(bare_page);
        assert!((bare.page_line_to_line_x(&bare.pages[0]) - 0.125).abs() < 1e-9);
        assert!((bare.page_line_jump_factor(&bare.pages[0]) - 2.0 / 3.0).abs() < 1e-9);
    }

    /// Themed cells without an evaluated theme (Visio 2021 witness,
    /// stencil masters dropped into a theme-less drawing):
    /// - `F='Inh'` Themed inherits up the style chain, which follows each
    ///   style's FillStyle/LineStyle attribute (Normal → Theme → No Style);
    /// - `THEMEVAL()` Themed takes the No Style value outright;
    /// - an instance without style attributes uses its master shape's
    ///   styles, not No Style.
    #[test]
    fn themed_cells_and_master_styles_resolve_like_visio() {
        let style = |id: i64, parent: Option<&str>, fill: Option<(&str, Option<&str>)>| {
            let mut st = StyleSheet {
                id,
                ..Default::default()
            };
            if let Some(p) = parent {
                for a in ["LineStyle", "FillStyle", "TextStyle"] {
                    st.attrs.push((a.to_string(), p.to_string()));
                }
            }
            if let Some((v, f)) = fill {
                st.sheet.set_cell(
                    "FillForegnd",
                    Cell {
                        v: Some(v.into()),
                        f: f.map(Into::into),
                        ..Cell::default()
                    },
                );
            }
            st
        };
        let mut doc = Document::default();
        doc.stylesheets.extend([
            style(0, None, Some(("1", None))),
            style(6, Some("0"), Some(("Themed", Some("THEMEVAL()")))),
            style(3, Some("6"), None),
            style(22, Some("6"), Some(("#dde2cd", Some("THEMEGUARD(…)")))),
            style(14, Some("6"), Some(("#cabb97", None))),
        ]);
        let shape = |fill_style: Option<i64>| Shape {
            id: 1,
            fill_style,
            ..Shape::default()
        };
        // Normal → Theme (THEMEVAL) → No Style: white, never "Themed".
        let normal = shape(Some(3));
        let v = doc.resolve_cell(&normal, &[], "FillForegnd").unwrap().v;
        assert_eq!(v.as_deref(), Some("1"));
        // A master shape whose own cell is THEMEVAL(): No Style, not the
        // style's cached tint.
        let mut master_root = shape(Some(22));
        master_root.sheet.set_cell(
            "FillForegnd",
            Cell {
                v: Some("Themed".into()),
                f: Some("THEMEVAL()".into()),
                ..Cell::default()
            },
        );
        // A master shape relying on its style: the style's value.
        let tower = shape(Some(14));
        for (id, root) in [(12, master_root), (13, tower)] {
            doc.masters.push(Master {
                id,
                shapes: vec![root],
                ..Master::default()
            });
        }
        let mut box_inst = shape(None);
        box_inst.master = Some(12);
        let v = doc.resolve_cell(&box_inst, &[], "FillForegnd").unwrap().v;
        assert_eq!(v.as_deref(), Some("1"), "THEMEVAL() → No Style");
        let mut tower_inst = shape(None);
        tower_inst.master = Some(13);
        let v = doc.resolve_cell(&tower_inst, &[], "FillForegnd").unwrap().v;
        assert_eq!(
            v.as_deref(),
            Some("#cabb97"),
            "a style-less instance cascades into its master's FillStyle"
        );
    }

    /// Each kind of cell walks its own parent attribute: a style whose
    /// LineStyle, FillStyle and TextStyle attributes name three
    /// different parents hands each kind the value of its own parent
    /// (Greptile on PR #335: one shared parent could not tell them
    /// apart).
    #[test]
    fn style_parents_are_picked_per_kind_of_cell() {
        let mut doc = Document::default();
        for (id, cell, v) in [
            (31, "LineColor", "#110000"),
            (32, "FillForegnd", "#002200"),
            (33, "TextBkgnd", "#000033"),
        ] {
            let mut st = StyleSheet {
                id,
                ..Default::default()
            };
            // Every parent carries all three cells, so a wrong pick
            // returns a wrong color instead of nothing.
            for other in ["LineColor", "FillForegnd", "TextBkgnd"] {
                let val = if other == cell { v } else { "#ffffff" };
                st.sheet.set_cell(other, Cell::value(val));
            }
            doc.stylesheets.push(st);
        }
        let mut child = StyleSheet {
            id: 30,
            ..Default::default()
        };
        for (attr, parent, cell) in [
            ("LineStyle", "31", "LineColor"),
            ("FillStyle", "32", "FillForegnd"),
            ("TextStyle", "33", "TextBkgnd"),
        ] {
            child.attrs.push((attr.to_string(), parent.to_string()));
            // Themed `Inh`: no value here, the walk goes to the parent.
            child.sheet.set_cell(
                cell,
                Cell {
                    v: Some("Themed".into()),
                    f: Some("Inh".into()),
                    ..Cell::default()
                },
            );
        }
        doc.stylesheets.push(child);
        let s = Shape {
            id: 1,
            line_style: Some(30),
            fill_style: Some(30),
            text_style: Some(30),
            ..Shape::default()
        };
        let v = |n: &str| doc.resolve_cell(&s, &[], n).unwrap().v;
        assert_eq!(v("LineColor").as_deref(), Some("#110000"));
        assert_eq!(v("FillForegnd").as_deref(), Some("#002200"));
        assert_eq!(v("TextBkgnd").as_deref(), Some("#000033"));
    }

    /// A document whose theme part defines `varColor1`.
    fn doc_with_theme() -> Document {
        use std::io::Write;
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default();
            z.start_file("[Content_Types].xml", opts).unwrap();
            z.write_all(br#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"/>"#)
                .unwrap();
            z.start_file("visio/theme/theme1.xml", opts).unwrap();
            z.write_all(
                br#"<a:theme><vt:varColor1><a:srgbClr val="4472C4"/></vt:varColor1></a:theme>"#,
            )
            .unwrap();
            z.finish().unwrap();
        }
        buf.set_position(0);
        Document {
            package: Some(Package::from_reader(buf).unwrap()),
            ..Document::default()
        }
    }

    /// A Themed `THEMEVAL("VariantColorN")` cell that the document's
    /// theme evaluates keeps its formula for the renderer — on the
    /// shape, on its master and in a style — instead of falling to the
    /// No Style value (Greptile on PR #335). Without that variant it
    /// still takes the No Style value.
    #[test]
    fn evaluable_theme_variants_keep_their_formula() {
        let themed = |f: &str| Cell {
            v: Some("Themed".into()),
            f: Some(f.into()),
            ..Cell::default()
        };
        let mut doc = doc_with_theme();
        // The theme XML is read once and cached on the document.
        assert!(doc.theme_variants.get().is_none());
        assert_eq!(doc.theme_variant_colors(), vec!["#4472C4".to_string()]);
        assert_eq!(doc.theme_variants.get().map(Vec::len), Some(1));
        let mut no_style = StyleSheet::default();
        no_style.sheet.set_cell("FillForegnd", Cell::value("1"));
        let mut theme = StyleSheet {
            id: 6,
            ..Default::default()
        };
        theme
            .sheet
            .set_cell("FillForegnd", themed(r#"THEMEVAL("VariantColor1")"#));
        doc.stylesheets.extend([no_style, theme]);
        let f = |c: Option<Cell>| c.and_then(|c| c.f);
        // On the shape itself.
        let mut s = Shape {
            id: 1,
            ..Shape::default()
        };
        s.sheet
            .set_cell("FillForegnd", themed(r#"THEMEVAL("VariantColor1")"#));
        assert_eq!(
            f(doc.resolve_cell(&s, &[], "FillForegnd")).as_deref(),
            Some(r#"THEMEVAL("VariantColor1")"#)
        );
        assert!(doc
            .resolve_cell_explicit(&s, &[], "FillForegnd")
            .is_some_and(|c| c.is_themed()));
        // In the shape's style.
        let styled = Shape {
            id: 2,
            fill_style: Some(6),
            ..Shape::default()
        };
        assert_eq!(
            f(doc.resolve_cell(&styled, &[], "FillForegnd")).as_deref(),
            Some(r#"THEMEVAL("VariantColor1")"#)
        );
        // A variant the theme does not define: the No Style value.
        s.sheet
            .set_cell("FillForegnd", themed(r#"THEMEVAL("VariantColor5")"#));
        let c = doc.resolve_cell(&s, &[], "FillForegnd").unwrap();
        assert_eq!(c.v.as_deref(), Some("1"));
    }

    /// A `THEMEVAL()` cell that finds nothing in No Style goes to the
    /// terminal defaults; it does not resume the master's or the
    /// style's value below it (CodeRabbit on PR #335).
    #[test]
    fn a_missed_themeval_fallback_does_not_resume_inheritance() {
        let mut doc = Document::default();
        // No Style without a FillForegnd cell; style 14 with a tint.
        doc.stylesheets.push(StyleSheet::default());
        let mut tint = StyleSheet {
            id: 14,
            ..Default::default()
        };
        tint.sheet.set_cell("FillForegnd", Cell::value("#cabb97"));
        doc.stylesheets.push(tint);
        let mut s = Shape {
            id: 1,
            fill_style: Some(14),
            ..Shape::default()
        };
        s.sheet.set_cell(
            "FillForegnd",
            Cell {
                v: Some("Themed".into()),
                f: Some("THEMEVAL()".into()),
                ..Cell::default()
            },
        );
        let builtin = builtin_defaults()
            .iter()
            .find(|(n, _)| *n == "FillForegnd")
            .map(|(_, v)| v.to_string());
        let v = doc.resolve_cell(&s, &[], "FillForegnd").and_then(|c| c.v);
        assert_eq!(v, builtin, "not the style's #cabb97");
        assert!(doc.resolve_cell_explicit(&s, &[], "FillForegnd").is_none());
    }

    /// A minimal document carrying one verbatim `DocumentSettings`
    /// element (exactly what the parser produces for the corpus shape
    /// — the RawEl round-trip itself is covered by the corpus tests).
    fn doc_with_settings(attrs: Attrs, inner: &str) -> Document {
        let mut doc = Document::default();
        doc.items.push(DocItem::Raw(RawEl {
            name: "DocumentSettings".to_string(),
            attrs,
            inner: inner.to_string(),
        }));
        doc.items.push(DocItem::DocumentSheet);
        doc
    }

    /// The shipped corpus element (attrs + children in Visio order).
    const CORPUS_INNER: &str = "<GlueSettings>9</GlueSettings><SnapSettings>295</SnapSettings>\
<SnapExtensions>34</SnapExtensions><SnapAngles/>\
<DynamicGridEnabled>1</DynamicGridEnabled><ProtectStyles>0</ProtectStyles>";

    #[test]
    fn reads_corpus_defaults() {
        let doc = doc_with_settings(
            vec![
                ("TopPage".to_string(), "0".to_string()),
                ("DefaultTextStyle".to_string(), "3".to_string()),
            ],
            CORPUS_INNER,
        );
        let s = doc.doc_snap_glue().expect("DocumentSettings present");
        assert_eq!(s.snap, 295);
        assert_eq!(s.glue, 9);
        assert!(s.dynamic_grid);
    }

    #[test]
    fn missing_element_reads_none() {
        let doc = Document::default();
        assert!(doc.doc_snap_glue().is_none());
    }

    #[test]
    fn setting_patches_only_the_three_children() {
        let mut doc =
            doc_with_settings(vec![("TopPage".to_string(), "0".to_string())], CORPUS_INNER);
        let before = doc
            .items
            .iter()
            .find_map(|it| match it {
                DocItem::Raw(r) if r.name == "DocumentSettings" => Some(r.clone()),
                _ => None,
            })
            .unwrap();
        doc.set_doc_snap_glue(DocSnapGlue {
            snap: 65847,
            glue: 0,
            dynamic_grid: false,
        });
        let after = doc
            .items
            .iter()
            .find_map(|it| match it {
                DocItem::Raw(r) if r.name == "DocumentSettings" => Some(r.clone()),
                _ => None,
            })
            .unwrap();
        // Attributes and every unmodeled child stay verbatim.
        assert_eq!(before.attrs, after.attrs);
        assert!(after.inner.contains("<SnapExtensions>34</SnapExtensions>"));
        assert!(after.inner.contains("<SnapAngles/>"));
        assert!(after.inner.contains("<ProtectStyles>0</ProtectStyles>"));
        // The three modeled children carry the new values, in corpus
        // child order.
        let g = after.inner.find("<GlueSettings>").unwrap();
        let s = after.inner.find("<SnapSettings>").unwrap();
        let d = after.inner.find("<DynamicGridEnabled>").unwrap();
        assert!(g < s && s < d, "corpus child order: {}", after.inner);
        assert!(after.inner.contains("<GlueSettings>0</GlueSettings>"));
        assert!(after.inner.contains("<SnapSettings>65847</SnapSettings>"));
        assert!(after
            .inner
            .contains("<DynamicGridEnabled>0</DynamicGridEnabled>"));
        // And reading back yields exactly what was written.
        let read = doc.doc_snap_glue().unwrap();
        assert_eq!(read.snap, 65847);
        assert_eq!(read.glue, 0);
        assert!(!read.dynamic_grid);
    }

    #[test]
    fn setting_creates_a_missing_element_in_corpus_shape() {
        let mut doc = Document::default();
        doc.items.push(DocItem::DocumentSheet);
        doc.set_doc_snap_glue(DocSnapGlue::CORPUS_DEFAULT);
        // First child of VisioDocument, like the corpus.
        assert!(matches!(doc.items.first(), Some(DocItem::Raw(r)) if r.name == "DocumentSettings"));
        let inner = match doc.items.first() {
            Some(DocItem::Raw(r)) => r.inner.clone(),
            _ => panic!("DocumentSettings element"),
        };
        assert_eq!(
            inner,
            "<GlueSettings>9</GlueSettings><SnapSettings>295</SnapSettings>\
             <SnapExtensions>34</SnapExtensions><SnapAngles/>\
             <DynamicGridEnabled>1</DynamicGridEnabled>"
        );
        assert_eq!(doc.doc_snap_glue(), Some(DocSnapGlue::CORPUS_DEFAULT));
    }

    #[test]
    fn patching_inserts_missing_children_in_canonical_position() {
        // An element with only a later-order child: the missing
        // GlueSettings/SnapSettings must land before SnapExtensions,
        // and DynamicGridEnabled after it.
        let mut inner =
            String::from("<SnapExtensions>34</SnapExtensions><ProtectStyles>0</ProtectStyles>");
        inner = set_settings_child(&inner, "GlueSettings", "1");
        inner = set_settings_child(&inner, "SnapSettings", "2");
        inner = set_settings_child(&inner, "DynamicGridEnabled", "0");
        assert_eq!(
            inner,
            "<GlueSettings>1</GlueSettings><SnapSettings>2</SnapSettings>\
             <SnapExtensions>34</SnapExtensions><DynamicGridEnabled>0</DynamicGridEnabled>\
             <ProtectStyles>0</ProtectStyles>"
        );
    }

    #[test]
    fn patching_expands_a_self_closing_child() {
        let inner = set_settings_child("<SnapSettings/>x", "SnapSettings", "295");
        assert_eq!(inner, "<SnapSettings>295</SnapSettings>x");
    }
}
