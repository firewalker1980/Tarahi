//! Text-function measurement + block-cache refresh (GitHub #147).
//!
//! Text-aware blocks carry formulas like the Utilisation connector's
//! `TxtWidth = MAX(TEXTWIDTH(TheText), 5*Char.Size)` and
//! `TxtHeight = TEXTHEIGHT(TheText, TxtWidth)`; their cached `V`s were
//! measured by Visio with real font metrics at AUTHORING time, so a
//! text edit left the block sized for the old text (#21 measured the
//! wrap but only for the render decision). This module is the
//! measurement side of the fix:
//!
//! - [`SheetTextMeasure`] implements `vsdx::formula::TextMeasure` —
//!   the bottom-crate bridge the evaluator calls `TEXTWIDTH` /
//!   `TEXTHEIGHT` through — measuring with the SAME face, size, line
//!   height, insets and greedy wrap `emit_text` renders with, so the
//!   refreshed caches and the rendered lines cannot disagree. The
//!   trailing line break does NOT add a line (#148's gold verdict:
//!   Visio's own corpus caches show one-visual-line texts measuring
//!   exactly one line height).
//! - [`text_edit_macro`] composes the edit + cache refresh into one
//!   undo step: the `SetText`, then a [`dc_core::SetCellCache`] per
//!   text-derived cell whose effective formula re-evaluates cleanly.
//!   Formula cells keep their `F` byte-identical — only the cache
//!   refreshes (the #59 `group_cache_refresh` precedent for undoable
//!   cache maintenance).
//!
//! Layering: the evaluator lives in `vsdx` and the metrics engine
//! here; the trait parameter is the bridge, per #147's approach — no
//! crate dependency inversion.

use dc_core::{Command, Macro, Scope, SetCellCache, SetText};
use vsdx::formula::TextMeasure;
use vsdx::model::{Document, Shape};

/// #234 composed slack: every NONEMPTY composed text adds exactly
/// 0.00005 in (witnessed on a corpus master's TxtHeight 0.15005 in);
/// empty blocks compose without it. An empty displayed `fld` is still
/// composed text (fields ride #267 — the rule lands here for the
/// plain-text composition).
const COMPOSED_SLACK: f64 = 0.00005;

/// The trim set measure and render share (#148's trailing-break trim,
/// #235's keep-the-NBSP): explicit breaks, spaces and tabs — a
/// trailing NO-BREAK SPACE is composed content (it hangs past the
/// margin), so the blanket `str::trim_end` would eat it.
const TRAILING_TRIM: [char; 4] = ['\r', '\n', ' ', '\t'];

/// The measurement context for one shape's text block: everything
/// `TEXTWIDTH`/`TEXTHEIGHT` need, resolved exactly as the renderer
/// resolves it.
pub struct SheetTextMeasure {
    /// The NEW text — what the formulas must evaluate against (the
    /// sheet still carries the old one while commands are built).
    text: String,
    /// Resolved character formatting + block metrics (one
    /// `resolve_text_block` call; font/size/style, line height,
    /// insets).
    tb: crate::svg::TextBlock,
}

impl SheetTextMeasure {
    /// Resolve the measurement context for `shape` with `text` as the
    /// text the formulas will see.
    pub fn new(doc: &Document, shape: &Shape, chain: &[&Shape], text: impl Into<String>) -> Self {
        let (w, h) = crate::svg::shape_size(doc, shape, chain);
        SheetTextMeasure {
            text: text.into(),
            tb: crate::svg::resolve_text_block(doc, shape, chain, w, h),
        }
    }

    /// The per-direction PAINT metrics — the faces and sizes
    /// `emit_text` draws each paragraph direction with (#257/#230):
    /// LTR paragraphs the Latin family at `font_size` × ITS script
    /// reduction, RTL paragraphs the complex-script chain (CS font,
    /// else the Latin family per the spec's zero semantics) at
    /// `cs_size` × THAT chain's reduction. TEXTWIDTH and the
    /// TEXTHEIGHT wrap both measure on this pair (Greptile PR #284
    /// round 6): measuring RTL on the Latin family's reduction saved
    /// ~⅓-narrow TxtWidth boxes whenever the two chains' witness
    /// states differed — e.g. a witnessed Calibri (0.667) against an
    /// unwitnessed Segoe UI painting full size.
    fn paint_metrics(
        &self,
    ) -> (
        crate::textmetrics::ResolvedFace,
        crate::textmetrics::ResolvedFace,
        f64,
        f64,
    ) {
        let face =
            crate::textmetrics::resolve(self.tb.font.as_deref(), self.tb.bold, self.tb.italic);
        let cs_chain = self.tb.cs_font.as_deref().or(self.tb.font.as_deref());
        let cs_face = crate::textmetrics::resolve(cs_chain, self.tb.bold, self.tb.italic);
        let red_l = crate::svg::script_metrics(self.tb.font.as_deref(), self.tb.pos)
            .map_or(1.0, |(r, _)| r);
        let red_r = crate::svg::script_metrics(cs_chain, self.tb.pos).map_or(1.0, |(r, _)| r);
        let cs_size = self.tb.cs_size.unwrap_or(self.tb.font_size);
        (face, cs_face, self.tb.font_size * red_l, cs_size * red_r)
    }

    /// The composed line slot (#234): the font size quantized onto
    /// the 18-fractional-bit grid FIRST, then the spacing multiplier
    /// (default 120 %) — no further rounding (the measured 6/8/10/12
    /// pt corrections are the raw size-grid residues × spacing).
    /// The slot uses the NOMINAL size (PR #284 review: a
    /// `script_reduce` factor here measured TEXTHEIGHT at ~2/3 of the
    /// laid-out height for Pos blocks, saving short TxtHeight boxes —
    /// #230's decision is slots stay nominal, only glyph measurement
    /// reduces).
    fn q_line_height(&self) -> f64 {
        let q = crate::textmetrics::quantize18(self.tb.font_size);
        let spacing = if self.tb.font_size > 0.0 {
            self.tb.line_height / self.tb.font_size
        } else {
            1.2
        };
        (q * spacing).max(q)
    }

    /// The per-line slot twin of `svg::line_slot_height` on the
    /// composed sizes (an RTL line under a larger complex-script size
    /// takes the proportionally taller slot — PR #269 review).
    fn q_slot(&self, rtl: bool) -> f64 {
        if !rtl {
            return self.q_line_height();
        }
        let cs = self.tb.cs_size.unwrap_or(self.tb.font_size);
        let ratio = if self.tb.font_size > 0.0 {
            cs / self.tb.font_size
        } else {
            1.0
        };
        self.q_line_height() * ratio.max(1.0)
    }

    /// #234 whitespace collapse: a whitespace-only node carrying BOTH
    /// a newline and a space (and no other character) is excluded
    /// from composition entirely — width and height collapse.
    fn collapsed(raw: &str) -> bool {
        raw.chars().all(char::is_whitespace) && raw.contains('\n') && raw.contains(' ')
    }

    /// The wrapped lines at `width` block inches (insets subtracted,
    /// same as the renderer), keeping each line's direction so the
    /// height sums the same per-line slots the renderer paints. The
    /// wrap runs at the DRAWN size (#230) and honors the paragraph
    /// indents AND the bullet tab offset (#231, PR #284 review): the
    /// renderer narrows a bulleted paragraph's first line to the
    /// computed tab stop, so the measure must wrap at the same width
    /// or a bulleted multi-line block's TxtHeight disagrees with the
    /// laid-out lines.
    fn lines_at(&self, text: &str, width: f64) -> Vec<crate::svg::WrapLine> {
        // Per-direction paint metrics (CodeRabbit local review +
        // Greptile PR #284 round 6): LTR paragraphs wrap at the Latin
        // family's drawn size, RTL paragraphs at the complex-script
        // chain's — the same faces and sizes `emit_text` paints with
        // (shared with `text_width` via `paint_metrics`).
        let (face, cs_face, size_l, size_r) = self.paint_metrics();
        // The per-cluster fallback CHAIN `emit_text` measures the
        // runs a paragraph's own face cannot cover (#282 zh-Hans grew
        // the #257 single Vazirmatn fallback to a chain): Vazirmatn
        // for the Arabic script, then the vendored CJK faces in
        // `dc::CJK_FACE_ORDER` — SC for Han, JP for the kana/JIS
        // Kanji the SC subset lacks, KR for Hangul — in the same
        // order the raster fontdb registers them, which is the order
        // usvg's per-cluster fallback picks a face (the ja/ko wave
        // #282 extended this chain and emit_text's TOGETHER: a
        // TEXTHEIGHT built on the old pair would size Korean text
        // with missing-glyph estimates the render never draws).
        // Unregistered (the dc unit-test binaries), every entry
        // resolves like the Latin default and the runs never split.
        let mut fallbacks = Vec::new();
        fallbacks.push(crate::textmetrics::resolve(
            Some("Vazirmatn"),
            self.tb.bold,
            self.tb.italic,
        ));
        for family in crate::CJK_FACE_ORDER {
            fallbacks.push(crate::textmetrics::resolve(
                Some(family),
                self.tb.bold,
                self.tb.italic,
            ));
        }
        let fb_chain: Vec<&crate::textmetrics::ResolvedFace> = fallbacks.iter().collect();
        let usable = (width - self.tb.insets.0 - self.tb.insets.1).max(0.02);
        // The first-line offset the RENDERER lays out (inches): the
        // bullet's tab stop for a bulleted block, max(IndFirst, 0)
        // otherwise — the shared rule in `svg::bullet_stop_offset_in`
        // keeps the measure and the paint on one layout.
        let first_off = if self.tb.bullet.is_some() {
            crate::svg::bullet_stop_offset_in(&self.tb)
                .unwrap_or(0.0)
                .max(0.0)
        } else {
            crate::svg::unbulleted_first_offset_in(&self.tb)
        };
        crate::svg::wrap_block_text(
            &face,
            text,
            size_l,
            usable,
            &cs_face,
            size_r,
            &fb_chain,
            first_off,
            self.tb.ind_left,
        )
    }
}

impl TextMeasure for SheetTextMeasure {
    fn the_text(&self) -> Option<String> {
        Some(self.text.clone())
    }

    fn text_width(&self, text: &str) -> Option<f64> {
        // TEXTWIDTH = the COMPOSED width (#234): the size quantized
        // onto the 18-fractional-bit grid, per-glyph advances rounded
        // onto that grid, PLUS one terminal-run space advance (from
        // the final run, even though it paints no glyph) and the
        // 0.00005 composed allowance. The trailing break never
        // widens a line — measure trimmed like `emit_text` (#148);
        // the trailing NBSP survives the trim (#235) and rides the
        // composition. (The old bare-advance form — e.g. DBUML_M's
        // 10 pt 'PK\r\n' eyeballed as Segoe UI's bare P+K — predates
        // the measured rules; Visio's composition always carries the
        // terminal space.)
        //
        // Each paragraph measures on the face and size it PAINTS
        // with (Greptile PR #284 round 6): LTR on the Latin family,
        // RTL on the complex-script chain — the same per-direction
        // pair `lines_at` wraps with below. BOTH directions
        // RUN-SPLIT like the paint (RTL since local CodeRabbit round
        // 3; LTR too since #282's CJK wave): the renderer measures
        // and rasterizes runs the paragraph's own chain can't cover
        // through the per-cluster fallback (the vendored Vazirmatn /
        // CJK faces), so TEXTWIDTH composes the same runs and takes
        // its terminal-run space from the final run's face instead
        // of missing-glyph estimates.
        let text = text.trim_end_matches(TRAILING_TRIM);
        if text.is_empty() {
            return Some(0.0);
        }
        let (face, cs_face, size_l, size_r) = self.paint_metrics();
        // The same fallback chain `lines_at`/`emit_text` measure with
        // (see `lines_at` for the CJK_FACE_ORDER walk) — BOTH
        // directions run-split since #282: an LTR paragraph carrying
        // Han glyphs composes those runs on the vendored CJK face the
        // raster paints them with.
        let mut fallbacks = Vec::new();
        fallbacks.push(crate::textmetrics::resolve(
            Some("Vazirmatn"),
            self.tb.bold,
            self.tb.italic,
        ));
        for family in crate::CJK_FACE_ORDER {
            fallbacks.push(crate::textmetrics::resolve(
                Some(family),
                self.tb.bold,
                self.tb.italic,
            ));
        }
        let fb_chain: Vec<&crate::textmetrics::ResolvedFace> = fallbacks.iter().collect();
        let mut best = 0.0;
        for para in text.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
            let w = if crate::svg::paragraph_base_rtl(para) {
                let q = crate::textmetrics::quantize18(size_r);
                cs_face.composed_width_in_runs(&fb_chain, para, size_r)
                    + cs_face.final_run_face(&fb_chain, para).space_advance_in(q)
            } else {
                let q = crate::textmetrics::quantize18(size_l);
                face.composed_width_in_runs(&fb_chain, para, size_l)
                    + face.final_run_face(&fb_chain, para).space_advance_in(q)
            };
            if w > best {
                best = w;
            }
        }
        Some(best + COMPOSED_SLACK)
    }

    fn text_height(&self, text: &str, width: f64) -> Option<f64> {
        // TEXTHEIGHT = the SUM of the wrapped lines' composed slots +
        // the block's top and bottom insets + the 0.00005 composed
        // slack for NONEMPTY text (#234; corpus gold: DBUML_M's
        // all-zero-margin 'PK\r\n' caches 1.2 em-scale — no insets
        // added; TIMELN's default-T/B-margins shape caches 1 line +
        // 2×0.0555). The wrap runs against the width minus the
        // LEFT/RIGHT insets — the same usable width `emit_text` wraps
        // at (an RTL line under a larger ComplexScriptSize takes a
        // taller slot — PR #269 review). Quantization moves a slot by
        // ≲ 2e-6 in against the painted slots (#234 measures on the
        // grid, paint keeps raw) — documented, invisible.
        let trimmed = text.trim_end_matches(TRAILING_TRIM);
        let ins_tb = self.tb.insets.2 + self.tb.insets.3;
        if trimmed.is_empty() {
            // An empty block still composes one line — INDEPENDENT of
            // the wrap width (#234), and WITHOUT the slack (the
            // witness: 0.15, not 0.15005-style sums on empties).
            // The #234 collapse: a whitespace-only node carrying a
            // newline AND a space composes nothing at all.
            if Self::collapsed(text) {
                return Some(0.0);
            }
            return Some(self.q_line_height() + ins_tb);
        }
        let lines_h = self
            .lines_at(trimmed, width)
            .iter()
            .map(|l| self.q_slot(l.rtl))
            .sum::<f64>();
        Some(lines_h + ins_tb + COMPOSED_SLACK)
    }
}

/// The text-block cells whose caches a text edit refreshes, in
/// dependency order (`TxtHeight`'s formula references `TxtWidth`).
const TEXT_DERIVED_CELLS: [&str; 2] = ["TxtWidth", "TxtHeight"];

/// One text edit, one undo step: the [`SetText`] plus refreshed
/// cached `V`s for every text-derived block cell whose effective
/// formula re-evaluates cleanly under the new text. Formulas that
/// fail evaluation (metrics gaps, unsupported functions) keep their
/// cached `V` — exactly the pre-#147 behavior for that cell.
///
/// Non-shape scopes and shapes without the cells are untouched: this
/// returns a bare `SetText` macro, and untouched documents never
/// re-evaluate (rendering stays byte-identical without an edit).
pub fn text_edit_macro(doc: &Document, scope: &Scope, text: &str) -> Macro {
    // Only the changed span is replaced in the raw text, so every
    // formatting marker around it (<cp>/<pp>/<tp>: colours, bold, sizes,
    // paragraph alignment) survives; SetText stored the plain draft and
    // flattened rich text to its first format.
    let base = match scope {
        Scope::Shape { page, shape } => crate::spellcheck::effective_raw(doc, *page, *shape)
            .map(|raw| vsdx::text::text_pieces(&raw).iter().map(|p| p.plain()).collect::<String>()),
        _ => None,
    };
    let edit: Box<dyn Command> = match base {
        Some(base) if !base.is_empty() => {
            let (start, len, repl) = changed_span(&base, text);
            Box::new(dc_core::ReplaceTextRange::new(scope.clone(), start, len, repl))
        }
        _ => Box::new(SetText::new(scope.clone(), text)),
    };
    let mut cmds: Vec<Box<dyn Command>> = vec![edit];
    cmds.extend(text_cache_refreshes(doc, scope, text));
    Macro::new(format!("set text of {scope}"), cmds)
}

/// The span of `old` that `new` replaced: (start char, old char count,
/// the new chars), from the common prefix and suffix.
pub fn changed_span(old: &str, new: &str) -> (usize, usize, String) {
    let a: Vec<char> = old.chars().collect();
    let b: Vec<char> = new.chars().collect();
    let mut p = 0;
    while p < a.len() && p < b.len() && a[p] == b[p] {
        p += 1;
    }
    let mut s = 0;
    while s < a.len() - p && s < b.len() - p && a[a.len() - 1 - s] == b[b.len() - 1 - s] {
        s += 1;
    }
    (p, a.len() - p - s, b[p..b.len() - s].iter().collect())
}

/// The `SetCellCache` refreshes for text-derived block cells
/// (`TEXTWIDTH`/`TEXTHEIGHT` formulas), measured against `text` —
/// what the shape will render. Shared by the text-edit macro and the
/// field-insert path (#267: an inserted field changes the SUBSTITUTED
/// text, and the measurement must see the same display the renderer
/// paints — CodeRabbit round 2).
pub fn text_cache_refreshes(doc: &Document, scope: &Scope, text: &str) -> Vec<Box<dyn Command>> {
    let mut cmds: Vec<Box<dyn Command>> = Vec::new();
    if let Scope::Shape { page, shape } = scope {
        if let (Some(p), Ok(chain)) = (
            doc.pages.iter().find(|p| p.id == *page),
            dc_core::shape_chain(doc, *page, *shape),
        ) {
            if let Some(s) = vsdx::model::find_shape_by_id(&p.shapes, *shape) {
                let measure = SheetTextMeasure::new(doc, s, &chain, text);
                let mut env = crate::svg::shape_env(doc, s, &chain);
                // `shape_env` carries Char.Size only when the chain
                // resolves a numeric cell; rendering always has a size
                // (resolve_text_block defaults one), so fill any gap
                // with the SAME size measurement uses — a formula
                // referencing Char.Size now evaluates against what
                // will actually be drawn (CodeRabbit).
                env.entry("Char.Size".to_string())
                    .or_insert(measure.tb.font_size);
                let mut refreshed: Vec<(String, f64)> = Vec::new();
                for name in TEXT_DERIVED_CELLS {
                    let Some(f) = dc_core::effective_formula(doc, s, &chain, name) else {
                        continue;
                    };
                    let up = f.to_ascii_uppercase();
                    if !up.contains("TEXTWIDTH") && !up.contains("TEXTHEIGHT") {
                        continue; // the cell's size does not depend on the text
                    }
                    // Later cells see earlier refreshes (CodeRabbit):
                    // TxtHeight's formula references the REFRESHED
                    // TxtWidth, not the stale cache.
                    for (n, v) in &refreshed {
                        env.insert(n.clone(), *v);
                    }
                    match vsdx::formula::eval_measured(&f, &env, &measure) {
                        Ok(v) => {
                            if let Some(n) = v.as_f64() {
                                // A finite ZERO is a valid measurement
                                // (PR #284 review round 4): clearing a
                                // label must refresh the cached width
                                // down to 0, not leave the stale value.
                                // Negatives and non-finites keep the
                                // cached V.
                                if n.is_finite() && n >= 0.0 {
                                    refreshed.push((name.to_string(), n));
                                }
                            }
                        }
                        Err(_) => continue, // keep the cached V for this cell
                    }
                }
                for (name, v) in refreshed {
                    cmds.push(Box::new(SetCellCache::new(scope.clone(), name, v)));
                }
            }
        }
    }
    cmds
}

#[cfg(test)]
mod tests {
    use super::*;
    use vsdx::model::{Cell, Shape, SheetChild};

    fn text_aware_shape() -> Shape {
        let mut s = Shape::default();
        s.sheet.set_cell("Width", Cell::value("2.0"));
        s.sheet.set_cell("Height", Cell::value("1.0"));
        s.sheet.set_cell(
            "TxtWidth",
            Cell {
                v: Some("1.0".into()),
                f: Some("MAX(Width,TEXTWIDTH(TheText))".into()),
                ..Cell::default()
            },
        );
        s.sheet.set_cell(
            "TxtHeight",
            Cell {
                v: Some("0.1666666666666667".into()),
                f: Some("TEXTHEIGHT(TheText,TxtWidth)".into()),
                ..Cell::default()
            },
        );
        s
    }

    /// The evaluator-level semantics the corpus gold pinned (#148)
    /// plus the #234 composed rules: a trailing CRLF adds no line; an
    /// empty text composes one line WITHOUT the slack; nonempty text
    /// adds the 0.00005 slack and measures on the 18-fractional-bit
    /// grid; a whitespace-only newline+space node collapses.
    #[test]
    fn measure_matches_the_corpus_line_semantics() {
        let doc = Document::default();
        let s = text_aware_shape();
        let chain = [&s];
        let m = SheetTextMeasure::new(&doc, &s, &chain, "");
        let one = "label";
        // Corpus equation (CodeRabbit pass on #147): height = lines ×
        // line height + top/bottom insets (default 2×0.0555 here).
        let ins_tb = m.tb.insets.2 + m.tb.insets.3;
        assert!(
            (ins_tb - 0.111).abs() < 1e-9,
            "fixture uses default T/B insets"
        );
        // The composed slot (#234): quantized size × 120 % spacing.
        let qh = m.q_line_height();
        assert!(
            (qh - 1.2 * crate::textmetrics::quantize18(m.tb.font_size)).abs() < 1e-12,
            "the composed slot is the grid-quantized size times the spacing"
        );
        let h1 = m.text_height(&format!("{one}\r\n"), 2.0).unwrap();
        let h1_plain = m.text_height(one, 2.0).unwrap();
        assert_eq!(h1, h1_plain, "trailing CRLF adds no line (#148)");
        assert!(
            (h1 - (qh + ins_tb + COMPOSED_SLACK)).abs() < 1e-12,
            "one line + insets + the composed slack (#234): {h1}"
        );
        // Empty text still composes one line — WITHOUT the slack
        // (the #234 witness: empties compose bare).
        assert_eq!(m.text_height("", 2.0), Some(qh + ins_tb));
        assert_eq!(
            m.text_height("\r\n", 2.0),
            Some(qh + ins_tb),
            "a bare break is one empty paragraph — one line (#148)"
        );
        // The #234 collapse: a whitespace-only node carrying a
        // newline AND a space composes nothing.
        assert_eq!(m.text_height("\n   ", 2.0), Some(0.0));
        assert_eq!(m.text_width("\n   "), Some(0.0));
        // …while a node with a newline but NO space keeps the
        // one-line composition (#148's bare-break gold).
        assert_eq!(m.text_height("\n\n", 2.0), Some(qh + ins_tb));
        // Two visual lines add exactly ONE more composed slot.
        let h2 = m.text_height(&format!("{one}\r\n{one}\r\n"), 2.0).unwrap();
        assert!((h2 - h1 - qh).abs() < 1e-12);
        // A narrow block wraps the same words into more lines.
        let wide = m.text_height("one two three four", 4.0).unwrap();
        let narrow = m.text_height("one two three four", 0.6).unwrap();
        assert!(narrow > wide, "narrow={narrow} wide={wide}");
        // Zero-margin blocks (DBUML's 'PK\r\n' masters, all four
        // margins explicit 0) add NOTHING beyond the composition:
        // exactly one slot + the slack.
        let mut z = text_aware_shape();
        for n in ["LeftMargin", "RightMargin", "TopMargin", "BottomMargin"] {
            z.sheet.set_cell(n, Cell::value("0"));
        }
        let chain_z = [&z];
        let mz = SheetTextMeasure::new(&doc, &z, &chain_z, "");
        assert!((mz.tb.insets.2 + mz.tb.insets.3).abs() < 1e-12);
        let zq = mz.q_line_height();
        assert!(
            (mz.text_height("PK\r\n", 1.0).unwrap() - (zq + COMPOSED_SLACK)).abs() < 1e-12,
            "zero margins: exactly one composed slot + slack"
        );
        // Width measures the widest composed paragraph — nonempty
        // text carries the terminal-run space + slack, so it is
        // strictly wider than the bare shaped advance (#234), and the
        // trailing break never widens a line (#148).
        let bare = crate::textmetrics::resolve(m.tb.font.as_deref(), false, false)
            .width_in(one, m.tb.font_size);
        let w = m.text_width(one).unwrap();
        assert!(w > bare, "composed {w} > bare {bare}");
        assert_eq!(
            m.text_width(&format!("{one}\r\n")).unwrap(),
            w,
            "the trailing break never widens a line"
        );
        // An empty width composes nothing.
        assert_eq!(m.text_width(""), Some(0.0));
    }

    /// #231 + PR #284 review: a bulleted block's TEXTHEIGHT wraps at
    /// the TAB-STOP-narrowed first line, the same layout the renderer
    /// paints — text that fits one line at the full width but wraps
    /// when the bullet pushes the first line to the tab stop measures
    /// two lines.
    #[test]
    fn bulleted_height_wraps_at_the_tab_stop() {
        let doc = Document::default();
        let mut s = text_aware_shape();
        // Tab stop 0.5 in; the bullet pushes the first line's text
        // start to 0.5 — the second word then wraps.
        s.sheet.children.push(SheetChild::Section(vsdx::Section {
            name: "Paragraph".into(),
            rows: vec![vsdx::Row {
                cells: vec![
                    ("Bullet".to_string(), Cell::value("1")),
                    ("HorzAlign".to_string(), Cell::value("0")),
                ],
                ..vsdx::Row::default()
            }],
            ..vsdx::Section::default()
        }));
        let m = SheetTextMeasure::new(&doc, &s, &[&s], "");
        let text = "word ".repeat(10) + "word";
        let text = text.as_str();
        // Fixture preconditions (deterministic): the text fits one
        // line at the full usable width but NOT at the tab-stop
        // narrowed width — the wrap is purely the bullet offset's
        // doing.
        let usable_plain = 4.0 - m.tb.insets.0 - m.tb.insets.1;
        let stop = crate::svg::bullet_stop_offset_in(&m.tb).unwrap();
        let usable_bulleted = usable_plain - stop;
        let face = crate::textmetrics::resolve(m.tb.font.as_deref(), false, false);
        let w = face.width_in(text, m.tb.font_size);
        assert!(
            w <= usable_plain && w > usable_bulleted,
            "fixture: {w:.3} must fit {usable_plain:.3} but overflow {usable_bulleted:.3}"
        );
        let h = m.text_height(text, 4.0).unwrap();
        // One line of the same block WITHOUT the bullet, for the
        // single-line height reference.
        let mut plain = text_aware_shape();
        plain
            .sheet
            .children
            .push(SheetChild::Section(vsdx::Section {
                name: "Paragraph".into(),
                rows: vec![vsdx::Row {
                    cells: vec![("HorzAlign".to_string(), Cell::value("0"))],
                    ..vsdx::Row::default()
                }],
                ..vsdx::Section::default()
            }));
        let mp = SheetTextMeasure::new(&doc, &plain, &[&plain], "");
        let h_plain = mp.text_height(text, 4.0).unwrap();
        assert!(
            h > h_plain,
            "the bulleted first line wraps: {h} vs one-line {h_plain}"
        );
    }

    /// #230 + PR #284 review: TEXTHEIGHT's line slots stay NOMINAL
    /// under Character.Pos — only glyph measurement reduces — or a
    /// superscript block's refreshed TxtHeight box would be ~2/3 of
    /// the laid-out height.
    #[test]
    fn pos_slots_stay_nominal() {
        let doc = Document::default();
        let mut s = text_aware_shape();
        s.sheet.children.push(SheetChild::Section(vsdx::Section {
            name: "Character".into(),
            rows: vec![vsdx::Row {
                cells: vec![("Pos".to_string(), Cell::value("1"))],
                ..vsdx::Row::default()
            }],
            ..vsdx::Section::default()
        }));
        let m = SheetTextMeasure::new(&doc, &s, &[&s], "");
        let qh = m.q_line_height();
        assert!(
            (qh - 1.2 * crate::textmetrics::quantize18(m.tb.font_size)).abs() < 1e-12,
            "the Pos slot quantizes the NOMINAL size: {qh}"
        );
    }

    /// Greptile PR #284 round 6: TEXTWIDTH measures each paragraph on
    /// the face and size it PAINTS with — an RTL paragraph rides the
    /// complex-script chain at ITS reduction. A block whose Latin
    /// family is script-witnessed (Calibri → 0.667) but whose CS
    /// family is not (Segoe UI → nominal) must measure RTL at FULL
    /// size, not a third small: the saved TxtWidth has to cover the
    /// drawn glyphs or an edit wraps needlessly.
    #[test]
    fn rtl_text_width_measures_the_complex_script_chain() {
        let doc = Document::default();
        let mut s = text_aware_shape();
        s.sheet.children.push(SheetChild::Section(vsdx::Section {
            name: "Character".into(),
            rows: vec![vsdx::Row {
                cells: vec![
                    ("Font".to_string(), Cell::value("Calibri")),
                    ("ComplexScriptFont".to_string(), Cell::value("Segoe UI")),
                    ("Pos".to_string(), Cell::value("1")),
                ],
                ..vsdx::Row::default()
            }],
            ..vsdx::Section::default()
        }));
        let m = SheetTextMeasure::new(&doc, &s, &[&s], "");
        let fa = "\u{0633}\u{0644}\u{0627}\u{0645}"; // base-RTL Arabic
        assert!(
            crate::svg::paragraph_base_rtl(fa),
            "fixture: the paragraph is base-RTL"
        );
        // The CS pair this block paints RTL with: chain = Segoe UI,
        // reduction NONE (unwitnessed family) → the nominal size.
        let (face_l, cs_face, size_l, size_r) = m.paint_metrics();
        assert!(
            size_r > size_l * 1.4,
            "fixture: the CS chain draws full size while Calibri scripts at 0.667 \
             ({size_r} vs {size_l})"
        );
        let q = crate::textmetrics::quantize18(size_r);
        let want = cs_face.composed_width_in(fa, q) + cs_face.space_advance_in(q) + COMPOSED_SLACK;
        let got = m.text_width(fa).unwrap();
        assert!(
            (got - want).abs() < 1e-9,
            "RTL width composes on the CS chain at its drawn size: {got} vs {want}"
        );
        // The pre-fix form — the Latin family at its 0.667 reduction —
        // measured a third smaller; the gap is the regression guard.
        let q_l = crate::textmetrics::quantize18(size_l);
        let old = face_l.composed_width_in(fa, q_l) + face_l.space_advance_in(q_l) + COMPOSED_SLACK;
        assert!(
            got > old,
            "the CS-chain measure must outrun the Latin-reduced one: {got} vs {old}"
        );
        // …while an LTR paragraph in the SAME block still measures on
        // the Latin chain at its reduction.
        let ltr = "label";
        let q_ltr = crate::textmetrics::quantize18(size_l);
        let want_ltr =
            face_l.composed_width_in(ltr, q_ltr) + face_l.space_advance_in(q_ltr) + COMPOSED_SLACK;
        assert!(
            (m.text_width(ltr).unwrap() - want_ltr).abs() < 1e-9,
            "LTR width still composes on the Latin chain"
        );
    }

    /// #234: the 18-fractional-bit size grid reproduces the measured
    /// 120 %-spacing height corrections exactly — their grid-unit
    /// residues at 6/8/10/12 pt are −0.4, −0.1333, +0.1333, +0.4
    /// steps of 2^-18 in.
    #[test]
    fn size_grid_matches_the_measured_spacing_corrections() {
        for (pt, corr) in [
            (6.0, -0.000001525879),
            (8.0, -0.000000508626),
            (10.0, 0.000000508626),
            (12.0, 0.000001525879),
        ] {
            let size = pt / 72.0;
            let got = crate::textmetrics::quantize18(size) * 1.2 - size * 1.2;
            assert!(
                (got - corr).abs() < 1e-10,
                "{pt} pt at 120 %: {got} vs measured {corr}"
            );
        }
    }

    /// The macro: caches refresh in the same undo step, formulas stay
    /// byte-identical, undo restores the prior caches.
    #[test]
    fn text_edit_macro_refreshes_caches_in_one_undo_step() {
        let mut doc = Document::default();
        doc.pages.push(vsdx::model::Page {
            id: 0,
            shapes: vec![text_aware_shape()],
            ..vsdx::model::Page::default()
        });
        let scope = Scope::Shape { page: 0, shape: 0 };
        let s = doc.pages[0].shapes[0].sheet.clone();

        // A long text: TEXTWIDTH > Width, so TxtWidth must grow, and
        // with it TxtHeight (2 wrapped lines at the default 10 pt).
        let long = "a considerably longer label that cannot fit one line";
        let mut m = text_edit_macro(&doc, &scope, long);
        m.apply(&mut doc).unwrap();

        let after = &doc.pages[0].shapes[0].sheet;
        let tw: f64 = after
            .cell("TxtWidth")
            .unwrap()
            .v
            .clone()
            .unwrap()
            .parse()
            .unwrap();
        let th: f64 = after
            .cell("TxtHeight")
            .unwrap()
            .v
            .clone()
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            tw > 2.0,
            "TxtWidth = MAX(Width, TEXTWIDTH) must follow the long text: {tw}"
        );
        assert!(th > 0.2, "wrapped height for the long text: {th}");
        // Formulas byte-identical, V-only refresh.
        assert_eq!(
            after.cell("TxtWidth").unwrap().f,
            s.cell("TxtWidth").unwrap().f
        );
        assert_eq!(
            after.cell("TxtHeight").unwrap().f,
            s.cell("TxtHeight").unwrap().f
        );
        // The text itself landed.
        assert_eq!(after.text.as_deref(), Some(long));

        // One undo step restores text AND prior caches.
        m.undo(&mut doc).unwrap();
        let restored = &doc.pages[0].shapes[0].sheet;
        assert_eq!(
            restored.cell("TxtWidth").unwrap().v,
            s.cell("TxtWidth").unwrap().v
        );
        assert_eq!(
            restored.cell("TxtHeight").unwrap().v,
            s.cell("TxtHeight").unwrap().v
        );
        assert_eq!(restored.text, None);
    }

    /// PR #284 review round 4: a finite zero measurement refreshes
    /// the cache — clearing a label drops the cached TxtWidth to 0
    /// instead of leaving the stale value.
    #[test]
    fn clearing_the_label_refreshes_the_width_to_zero() {
        let mut doc = Document::default();
        // A BARE TEXTWIDTH formula: the fixture's default
        // MAX(Width, TEXTWIDTH) would mask the zero behind Width.
        let mut shape = text_aware_shape();
        shape.sheet.set_cell(
            "TxtWidth",
            Cell {
                v: Some("1.0".into()),
                f: Some("TEXTWIDTH(TheText)".into()),
                ..Cell::default()
            },
        );
        doc.pages.push(vsdx::model::Page {
            id: 0,
            shapes: vec![shape],
            ..vsdx::model::Page::default()
        });
        let scope = Scope::Shape { page: 0, shape: 0 };
        // First an edit with content (caches a positive width)…
        let mut m = text_edit_macro(&doc, &scope, "some text");
        m.apply(&mut doc).unwrap();
        let w: f64 = doc.pages[0].shapes[0]
            .sheet
            .cell("TxtWidth")
            .unwrap()
            .v
            .clone()
            .unwrap()
            .parse()
            .unwrap();
        assert!(w > 0.0, "the label cached a positive width: {w}");
        // …then clearing it: the refresh must land the ZERO.
        let mut m = text_edit_macro(&doc, &scope, "");
        m.apply(&mut doc).unwrap();
        let w: f64 = doc.pages[0].shapes[0]
            .sheet
            .cell("TxtWidth")
            .unwrap()
            .v
            .clone()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(w, 0.0, "the cleared label refreshed the cache to zero");
    }

    /// A formula-free block (plain `V` cells, no `F`) never gets
    /// cache writes — the macro is a bare SetText.
    #[test]
    fn formula_free_blocks_stay_bare() {
        let mut doc = Document::default();
        let mut s = Shape::default();
        s.sheet.set_cell("TxtWidth", Cell::value("1.5"));
        s.sheet.set_cell("TxtHeight", Cell::value("0.2"));
        doc.pages.push(vsdx::model::Page {
            id: 0,
            shapes: vec![s],
            ..vsdx::model::Page::default()
        });
        let scope = Scope::Shape { page: 0, shape: 0 };
        let m = text_edit_macro(&doc, &scope, "hello");
        assert_eq!(m.cmds.len(), 1, "no cache refresh without formulas");
    }

    /// TxtHeight evaluates against the REFRESHED TxtWidth (CodeRabbit
    /// on #147): a text that would wrap at the stale width but fits at
    /// the refreshed one must cache a one-line height.
    #[test]
    fn txt_height_uses_the_refreshed_txt_width() {
        let mut doc = Document::default();
        let mut s = text_aware_shape();
        // Stale cache: TxtWidth V=1.0 (usable 0.8 with default insets)
        // — the long text wraps there. The formula is
        // MAX(Width=2.0, TEXTWIDTH) so the refresh widens the block to
        // 2.0 (usable 1.8) where the same text fits on one line.
        s.sheet.set_cell("Width", Cell::value("2.0"));
        s.sheet.set_cell(
            "TxtWidth",
            Cell {
                v: Some("1.0".into()),
                f: Some("MAX(Width,TEXTWIDTH(TheText))".into()),
                ..Cell::default()
            },
        );
        doc.pages.push(vsdx::model::Page {
            id: 0,
            shapes: vec![s],
            ..vsdx::model::Page::default()
        });
        let scope = Scope::Shape { page: 0, shape: 0 };
        let text = "a moderately wide label";
        let mut m = text_edit_macro(&doc, &scope, text);
        m.apply(&mut doc).unwrap();
        let after = &doc.pages[0].shapes[0].sheet;
        let th: f64 = after
            .cell("TxtHeight")
            .unwrap()
            .v
            .clone()
            .unwrap()
            .parse()
            .unwrap();
        let tw: f64 = after
            .cell("TxtWidth")
            .unwrap()
            .v
            .clone()
            .unwrap()
            .parse()
            .unwrap();
        assert!((tw - 2.0).abs() < 1e-9, "MAX(Width, TEXTWIDTH) = {tw}");
        let chain: [&Shape; 0] = [];
        let shape = &doc.pages[0].shapes[0];
        let measure = SheetTextMeasure::new(&doc, shape, &chain, text);
        let one_line = measure.text_height(text, 2.0).unwrap();
        // Preconditions on the fixture text: it wraps at the STALE
        // width and fits at the REFRESHED one.
        assert!(
            measure.text_height(text, 1.0).unwrap() > one_line,
            "fixture must wrap at the stale width"
        );
        assert!(
            (th - one_line).abs() < 1e-9,
            "one line (+ insets) at the refreshed width: {th} vs {one_line}"
        );
    }
}

#[cfg(test)]
mod keep_format_tests {
    use super::*;

    #[test]
    fn editing_rich_text_keeps_its_formatting_markers() {
        assert_eq!(changed_span("abc", "abXc"), (2, 0, "X".into()));
        assert_eq!(changed_span("Rood = DMZ", "Rood = LAN"), (7, 3, "LAN".into()));
        let mut ed = dc_core::Editor::new_blank();
        let page = ed.doc.pages[0].id;
        let mut s = vsdx::model::Shape { id: 1, type_: Some("Shape".into()), ..Default::default() };
        s.sheet.text_raw = Some("<cp IX='0'/>Legend\n<cp IX='1'/>Rood = <cp IX='2'/>DMZ\n".into());
        s.sheet.text = Some("Legend\nRood = DMZ\n".into());
        ed.doc.pages[0].shapes.push(s);
        let scope = Scope::Shape { page, shape: 1 };
        ed.apply(text_edit_macro(&ed.doc, &scope, "Legend\nRood = LAN\n")).unwrap();
        let raw = ed.doc.pages[0].shapes.last().unwrap().sheet.text_raw.clone().unwrap();
        assert_eq!(raw, "<cp IX='0'/>Legend\n<cp IX='1'/>Rood = <cp IX='2'/>LAN\n");
    }
}
