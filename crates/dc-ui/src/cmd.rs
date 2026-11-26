//! GUI-authored editing commands (implementing [`dc_core::Command`]) —
//! the pieces a canvas needs beyond the dc-core basics. All values are
//! in Visio's internal inches; the mm/in distinction is display-only
//! (see `units.rs`).

use dc_core::commands::{remove_cell, SetCell};
use dc_core::Command;
use dc_core::Scope;
use vsdx::model::{Cell, Document, Shape};

/// Move a shape's pin to a new page position (inches).
pub struct MoveShape {
    shape: i64,
    to: (f64, f64),
    x: SetCell,
    y: SetCell,
}

impl MoveShape {
    pub fn new(page: i64, shape: i64, to_pin: (f64, f64)) -> MoveShape {
        MoveShape {
            shape,
            to: to_pin,
            x: SetCell::new(
                Scope::Shape { page, shape },
                "PinX",
                Cell::value(format!("{:.6}", to_pin.0)),
            ),
            y: SetCell::new(
                Scope::Shape { page, shape },
                "PinY",
                Cell::value(format!("{:.6}", to_pin.1)),
            ),
        }
    }
}

impl Command for MoveShape {
    fn touched(&self, doc: &Document) -> dc_core::TouchList {
        // Union of the two PinX/PinY SetCells → a pure Move touch
        // (#93): the layer texture stays valid under translation.
        let mut all = self.x.touched(doc);
        all.merge(self.y.touched(doc));
        all
    }

    fn label(&self) -> String {
        format!(
            "move shape {} to ({:.3}, {:.3})",
            self.shape, self.to.0, self.to.1
        )
    }
    fn apply(&mut self, doc: &mut Document) -> anyhow::Result<()> {
        self.x.apply(doc)?;
        self.y.apply(doc)?;
        Ok(())
    }
    fn undo(&mut self, doc: &mut Document) -> anyhow::Result<()> {
        self.y.undo(doc)?;
        self.x.undo(doc)?;
        Ok(())
    }
}

/// Refresh a flattened group's inherited cell caches after a resize
/// (GitHub #59). Visio instances carry `F='Inh'` snapshots of every
/// formula-driven cell — sub-shape XForms, the Txt* block, Connection
/// and Geometry row coordinates. Resizing only the top-level sheet
/// leaves those caches at the old size, and the renderer (which treats
/// caches as authoritative, like Visio at load) keeps composing the
/// drawing at it: the selection box grows around a stale shape.
/// Visio refreshes the caches when an edit changes what they evaluate
/// to; this emits the same refresh as SetCell/SetRowCell commands,
/// riding in the resize's Macro so one undo reverts everything.
///
/// The refresh re-derives each cache by **evaluating the master's
/// formula at the instance's new dims** (`vsdx::model::instance_env`,
/// the same environment the renderer's master mirroring uses) — which
/// sees through User-cell indirections like `TxtPinX =
/// GUARD(User.TxtPinX)` where `User.TxtPinX = Width/2` (the substring
/// heuristic that scaled caches before could not, and the icon/text
/// drifted as the shape grew). Refreshed cells keep `F='Inh'`, so a
/// later resize refreshes them again from the master's formula — no
/// compounding staleness. Ratio scaling (the shape's own size ratio
/// per axis) remains as the fallback when a formula cannot be
/// evaluated; fixed formulas (`GUARD(4MM)` icons) evaluate to their
/// constant and stay put. Cells with a real local formula (GUARD,
/// functions) are never touched — they re-evaluate, they don't cache.
pub fn group_cache_refresh(
    doc: &Document,
    page: i64,
    shape: i64,
    kx: f64,
    ky: f64,
) -> Vec<Box<dyn dc_core::Command>> {
    let Some(p) = doc.pages.iter().find(|p| p.id == page) else {
        return Vec::new();
    };
    let Some(root) = vsdx::model::find_shape_by_id(&p.shapes, shape) else {
        return Vec::new();
    };
    // Instance environments at the current (pre-resize) and new group
    // dims: the master root's layout formulas — including User.*
    // indirections — re-evaluate against them (instance User overrides
    // such as ShapeLayout win; cached F='Inh' User rows do not).
    let master_root = doc.instance_source(root, &[]);
    let dim = |n: &str| {
        doc.resolve_cell(root, &[], n)
            .and_then(|c| c.numeric())
            .filter(|v| v.is_finite() && *v > 1e-9)
    };
    let (old_w, old_h) = (dim("Width").unwrap_or(1.0), dim("Height").unwrap_or(1.0));
    let (env_old, env_new) = match &master_root {
        Some(m) => (
            Some(vsdx::model::instance_env(m, root, old_w, old_h)),
            Some(vsdx::model::instance_env(m, root, old_w * kx, old_h * ky)),
        ),
        None => (None, None),
    };
    let mut cmds = Vec::new();
    walk(
        root,
        root,
        page,
        true,
        doc,
        env_old.as_ref(),
        env_new.as_ref(),
        (kx, ky),
        &mut cmds,
    );
    cmds
}

/// One (cell name, axis) pair: the axis variable whose mention in the
/// master's formula marks the value as proportional (ratio fallback).
const X_CELLS: &[&str] = &[
    "PinX",
    "Width",
    "LocPinX",
    "TxtPinX",
    "TxtWidth",
    "TxtLocPinX",
];
const Y_CELLS: &[&str] = &[
    "PinY",
    "Height",
    "LocPinY",
    "TxtPinY",
    "TxtHeight",
    "TxtLocPinY",
];

/// Does the master's formula for `name` make the value proportional to
/// the axis? `None` = no master cell at all (nothing inherited).
fn scales_with(mcell: Option<&vsdx::model::Cell>, axis: &str) -> Option<bool> {
    let c = mcell?;
    Some(match c.f.as_deref() {
        Some(f) if f != "Inh" => f.contains(axis),
        // No formula on the master cell: XForm/Txt cells are literal
        // local values (size-independent); geometry row cells are
        // proportional by the renderer's convention (handled by the
        // caller passing `literal_proportional`).
        _ => false,
    })
}

/// The master cell's formula when it is a real (evaluable) one.
fn real_formula(cell: Option<&vsdx::model::Cell>) -> Option<&str> {
    cell.and_then(|c| c.f.as_deref())
        .filter(|f| *f != "Inh" && *f != "No Formula")
}

/// Evaluate a master-source cell's formula in `env`.
fn eval_in(cell: Option<&vsdx::model::Cell>, env: &vsdx::formula::Env) -> Option<f64> {
    let f = real_formula(cell)?;
    vsdx::formula::eval(f, env)
        .ok()
        .and_then(|v| v.as_f64())
        .filter(|v| v.is_finite())
}

/// A refreshed inherited cache: the new value under a preserved
/// `F='Inh'`, so the next resize refreshes it from the master's
/// formula again instead of meeting a frozen literal.
fn cached(v: f64) -> Cell {
    Cell {
        v: Some(format!("{v:.6}")),
        f: Some("Inh".into()),
        ..Cell::default()
    }
}

#[allow(clippy::too_many_arguments)]
fn walk(
    root: &Shape,
    sh: &Shape,
    page: i64,
    is_root: bool,
    doc: &Document,
    env_old: Option<&vsdx::formula::Env>,
    env_new: Option<&vsdx::formula::Env>,
    (kx, ky): (f64, f64),
    cmds: &mut Vec<Box<dyn dc_core::Command>>,
) {
    // The inheritance source for this shape's cells: the master's
    // corresponding shape (the root's master root, a sub's master sub).
    let chain: Vec<&Shape> = vec![root];
    let source = doc.instance_source(sh, &chain);

    // The environment this shape's source cells evaluate in, and the
    // per-axis size ratio for the fallback path. Root cells evaluate in
    // the root env (bare Width/Height are the group's own); sub cells
    // in a child env — qualified root entries only, seeded with the
    // sub's own effective size (a bare name in a child formula means
    // the child's own cell, `vsdx::model::qualified_env`).
    let mut cell_env: Option<vsdx::formula::Env> = None;
    let mut ratio = (kx, ky);
    if is_root {
        cell_env = env_new.cloned();
    } else if let (Some(src), Some(eo), Some(en)) = (source, env_old, env_new) {
        let qo = vsdx::model::qualified_env(eo);
        let mut qn = vsdx::model::qualified_env(en);
        // The sub's effective size at the old/new dims: its master
        // formulas re-evaluated (`GUARD(4MM)` stays fixed, a
        // `Sheet.N!Width*1` container stretches with the group).
        let eff = |name: &str, env: &vsdx::formula::Env, own: Option<f64>| {
            eval_in(src.sheet.cell(name), env)
                .filter(|v| *v > 1e-9)
                .or(own)
        };
        let own = |n: &str| sh.sheet.cell(n).and_then(|c| c.numeric());
        let w_old = eff("Width", &qo, own("Width"));
        let h_old = eff("Height", &qo, own("Height"));
        let w_new = eff("Width", &qn, w_old.map(|v| v * kx));
        let h_new = eff("Height", &qn, h_old.map(|v| v * ky));
        ratio = (
            match (w_old, w_new) {
                (Some(a), Some(b)) if a > 1e-9 => b / a,
                _ => kx,
            },
            match (h_old, h_new) {
                (Some(a), Some(b)) if a > 1e-9 => b / a,
                _ => ky,
            },
        );
        if let (Some(w), Some(h)) = (w_new, h_new) {
            qn.insert("Width".into(), w);
            qn.insert("Height".into(), h);
        }
        cell_env = Some(qn);
    }

    for (names, axis) in [(X_CELLS, "Width"), (Y_CELLS, "Height")] {
        for name in names {
            // The root's own XForm cells are written by the resize
            // itself; only its Txt* block is cache-refreshed.
            if is_root && !name.starts_with("Txt") {
                continue;
            }
            let Some(cell) = sh.sheet.cell(name) else {
                continue;
            };
            if cell.f.as_deref() != Some("Inh") {
                continue;
            }
            let Some(v) = cell.numeric() else { continue };
            let scope = Scope::Shape { page, shape: sh.id };
            // Authoritative path: re-evaluate the master's formula at
            // the instance's new dims.
            if let Some(env) = cell_env.as_ref() {
                if let Some(nv) = source.and_then(|src| eval_in(src.sheet.cell(name), env)) {
                    cmds.push(Box::new(SetCell::new(scope, *name, cached(nv))));
                    // Seed the derived text sizes so later TxtLocPin
                    // formulas (`TxtWidth*0.5`) evaluate too.
                    if is_root && matches!(*name, "TxtWidth" | "TxtHeight") {
                        if let Some(env) = cell_env.as_mut() {
                            env.insert((*name).to_string(), nv);
                        }
                    }
                    continue;
                }
            }
            // Fallback: ratio scaling, gated by the axis mention in the
            // master's formula (a fixed formula keeps its value).
            // LocPin follows the shape's own size: evaluation already
            // handled indirect size formulas here, so this gate only
            // keeps ratio scaling off a fixed-size shape's LocPin.
            if name.starts_with("LocPin") || name.starts_with("TxtLocPin") {
                let tsize = if name.starts_with("Txt") {
                    format!("Txt{axis}")
                } else {
                    axis.to_string()
                };
                let size_scales = source
                    .and_then(|src| scales_with(src.sheet.cell(&tsize), axis))
                    .unwrap_or(true);
                if !size_scales {
                    continue;
                }
            }
            let proportional = source
                .and_then(|src| scales_with(src.sheet.cell(name), axis))
                .unwrap_or(true);
            if !proportional {
                continue;
            }
            let k = if axis == "Width" { ratio.0 } else { ratio.1 };
            cmds.push(Box::new(SetCell::new(scope, *name, cached(v * k))));
        }
    }
    // Row coordinates (Geometry, Connection): X/A/C follow Width,
    // Y/B/D follow Height — by formula evaluation when the master row
    // has one, else by the shape's own size ratio (a master cell
    // without a formula is proportional by the renderer's convention).
    if let Some(source) = source {
        const ROW_X: &[&str] = &["X", "A", "C"];
        const ROW_Y: &[&str] = &["Y", "B", "D"];
        for (own_secs, src_secs) in [
            (
                sh.sheet.sections_named("Geometry"),
                source.sheet.sections_named("Geometry"),
            ),
            (
                sh.sheet.sections_named("Connection"),
                source.sheet.sections_named("Connection"),
            ),
        ] {
            for (si, sec) in own_secs.iter().enumerate() {
                let src_sec = src_secs.get(si);
                for row in &sec.rows {
                    use dc_core::commands::RowSel;
                    let sel = match &row.n {
                        Some(n) => RowSel::Named(n.clone()),
                        None => RowSel::Ix(row.ix),
                    };
                    let src_row = src_sec.and_then(|ss| {
                        ss.rows.iter().find(|r| match &sel {
                            RowSel::Ix(ix) => r.ix == *ix,
                            RowSel::Named(n) => r.n.as_deref() == Some(n.as_str()),
                        })
                    });
                    for (name, axis) in ROW_X
                        .iter()
                        .map(|n| (*n, "Width"))
                        .chain(ROW_Y.iter().map(|n| (*n, "Height")))
                    {
                        let Some(cell) = row.cell(name) else { continue };
                        if cell.f.as_deref() != Some("Inh") {
                            continue;
                        }
                        let Some(v) = cell.numeric() else { continue };
                        let scope = Scope::Shape { page, shape: sh.id };
                        if let Some(env) = cell_env.as_ref() {
                            if let Some(nv) = src_row.and_then(|r| eval_in(r.cell(name), env)) {
                                cmds.push(Box::new(dc_core::commands::SetRowCell::new(
                                    scope,
                                    &sec.name,
                                    sec.ix,
                                    sel.clone(),
                                    name,
                                    cached(nv),
                                )));
                                continue;
                            }
                        }
                        let proportional = src_row
                            .and_then(|r| real_formula(r.cell(name)))
                            .map(|f| f.contains(axis))
                            .unwrap_or(true);
                        if !proportional {
                            continue;
                        }
                        let k = if axis == "Width" { ratio.0 } else { ratio.1 };
                        cmds.push(Box::new(dc_core::commands::SetRowCell::new(
                            scope,
                            &sec.name,
                            sec.ix,
                            sel.clone(),
                            name,
                            cached(v * k),
                        )));
                    }
                }
            }
        }
    }
    for sub in &sh.sub_shapes {
        walk(
            root,
            sub,
            page,
            false,
            doc,
            env_old,
            env_new,
            (kx, ky),
            cmds,
        );
    }
}

/// Delete every cell of a shape's own sheet and the shape itself is NOT
/// handled here — see dc_core::commands::RemoveShape for removal. This
/// helper exists for tests: exact restoration of a moved shape.
#[allow(dead_code)]
pub(crate) fn strip_pin_cells(doc: &mut Document, page: i64, shape: i64) {
    let scope = Scope::Shape { page, shape };
    if let Ok(sheet) = dc_core::scope::sheet_mut(doc, &scope) {
        let _ = remove_cell(sheet, "PinX");
        let _ = remove_cell(sheet, "PinY");
    }
}

#[allow(dead_code)]
pub(crate) fn pin_of(doc: &Document, page: i64, shape: i64) -> Option<(f64, f64)> {
    let scope = Scope::Shape { page, shape };
    let sheet = dc_core::scope::sheet(doc, &scope).ok()?;
    let x = sheet.cell("PinX")?.numeric()?;
    let y = sheet.cell("PinY")?.numeric()?;
    Some((x, y))
}

// ---------------------------------------------------------------------------
// Connector rerouting (GitHub #108)
// ---------------------------------------------------------------------------

/// Deferred connector reroute: re-attach and re-route every connector
/// glued to the moved/resized shapes, evaluated against the document
/// state AT APPLY TIME (it rides at the tail of a move/resize Macro,
/// so the moved shapes' cells are already updated — building the
/// reroute eagerly would read stale, pre-move geometry).
///
/// Static glue follows the connection row; dynamic glue re-attaches on
/// the facing side; the geometry rows carry the new route
/// (`FixedRoute=1` connectors stay put). Cached values are rewritten,
/// the corpus formulas kept. Undo restores every touched cell.
/// #218 review: the PRE-drag endpoint pairs for `also` connectors —
/// (pre_beg, pre_end) keyed by connector id.
pub type PrePose = std::collections::HashMap<i64, ((f64, f64), (f64, f64))>;

pub struct RerouteConnectors {
    page: i64,
    moved: Vec<i64>,
    /// #179: connector ids to plan REGARDLESS of which shape moved —
    /// an endpoint drop re-glues or RELEASES the connector, and a
    /// master-based connector's formula-driven frame caches must
    /// refresh either way or the render keeps the pre-drag pose.
    also: Vec<i64>,
    inner: Vec<Box<dyn dc_core::Command>>,
    /// The apply-time touch list, merged once pass 2 finishes. `undo`
    /// clears `inner` (its commands are spent), but the history asks
    /// `touched()` AFTER `undo()` runs — without the cache the
    /// connector's Ink touch would vanish and its layer texture would
    /// stay at the dragged raster under the restored bbox (#178).
    touched_cache: dc_core::TouchList,
    /// #218 review: the PRE-drag endpoint pairs for `also`
    /// connectors (the endpoint-drop path writes the new endpoint
    /// cells BEFORE this command applies, so re-reading them from the
    /// document yields pre == post and the literal-row translation
    /// delta collapses to zero). Keyed by connector id:
    /// (pre_beg, pre_end).
    also_pre: PrePose,
}

impl RerouteConnectors {
    pub fn new(page: i64, moved: Vec<i64>) -> RerouteConnectors {
        RerouteConnectors {
            page,
            moved,
            also: Vec::new(),
            inner: Vec::new(),
            touched_cache: dc_core::TouchList::default(),
            also_pre: PrePose::new(),
        }
    }

    /// Plan these connectors by ID — every glue record they carry
    /// (or a frame-only refresh when released), no shape-move needed
    /// (#179's endpoint-drop path).
    /// `new_including` with the PRE-drag endpoint pairs (#218
    /// review) — the endpoint-drop path's constructor: the drop has
    /// already written the new endpoint cells when the command
    /// applies, so the pre pose must ride along or literal geometry
    /// rows never translate.
    pub fn new_including_with_pre(
        page: i64,
        also: Vec<i64>,
        pre: PrePose,
    ) -> RerouteConnectors {
        let mut cmd = Self::new_including(page, also);
        cmd.also_pre = pre;
        cmd
    }

    pub fn new_including(page: i64, also: Vec<i64>) -> RerouteConnectors {
        RerouteConnectors {
            page,
            moved: Vec::new(),
            also,
            inner: Vec::new(),
            touched_cache: dc_core::TouchList::default(),
            also_pre: PrePose::new(),
        }
    }

    /// The connectors this reroute would rewrite, discovered READ-ONLY
    /// against `doc` — the static half of apply's pass 1: the glue
    /// records of the moved shapes plus the #179 also-list, then the
    /// #174 chain-aware connector test, the FixedRoute bail and the
    /// resolvable-endpoint check. `touched()` names them BEFORE apply
    /// so the layer store can harvest their PRE-reroute bounds (#236);
    /// apply's attach-resolution half can still drop one, which only
    /// leaves an unused harvest entry.
    fn candidate_connectors(&self, doc: &Document) -> Vec<i64> {
        self.candidate_connectors_with_records(doc)
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    /// The shared discovery (`touched()` takes the IDs, apply's pass 1
    /// the pairs, so pre-apply harvesting and apply rewrite the SAME
    /// connectors by construction — #218 review): the glue records of
    /// the moved shapes plus the #179 also-list, then the #174
    /// chain-aware connector test, the FixedRoute bail and the
    /// resolvable-endpoint check. Records are cloned owned — the
    /// connects list is a page-local value here.
    fn candidate_connectors_with_records(
        &self,
        doc: &Document,
    ) -> Vec<(i64, Vec<dc_core::connects::Connect>)> {
        use dc_core::connects::ConnectsHost;
        use std::collections::BTreeMap;
        let Some(page) = doc.pages.iter().find(|p| p.id == self.page) else {
            return Vec::new();
        };
        let connects = page.connects();
        let mut by_connector: BTreeMap<i64, Vec<&dc_core::connects::Connect>> = BTreeMap::new();
        for c in &connects.list {
            if !self.moved.contains(&c.to_sheet) {
                continue;
            }
            by_connector.entry(c.from_sheet).or_default().push(c);
        }
        for id in &self.also {
            if !by_connector.contains_key(id) {
                by_connector.insert(
                    *id,
                    connects
                        .list
                        .iter()
                        .filter(|c| c.from_sheet == *id)
                        .collect(),
                );
            }
        }
        by_connector
            .into_iter()
            .filter(|(conn_id, _)| {
                let Some(conn) = vsdx::model::find_shape_by_id(&page.shapes, *conn_id) else {
                    return false;
                };
                let Ok(chain) = dc_core::scope::shape_chain(doc, self.page, *conn_id) else {
                    return false;
                };
                if !crate::connectors::is_connector_in(doc, &chain, conn) {
                    return false;
                }
                if conn
                    .sheet
                    .cell("FixedRoute")
                    .and_then(|v| v.numeric())
                    .is_some_and(|v| v == 1.0)
                {
                    return false;
                }
                let get = |n: &str| doc.resolve_cell(conn, &chain, n).and_then(|v| v.numeric());
                let (Some(_), Some(_), Some(_), Some(_)) =
                    (get("BeginX"), get("BeginY"), get("EndX"), get("EndY"))
                else {
                    return false;
                };
                true
            })
            .map(|(id, recs)| (id, recs.into_iter().cloned().collect()))
            .collect()
    }
}

/// #202: the cached Bézier-control refreshes a reroute owes a
/// connector's CubicBezTo rows. The curved route's controls are
/// `Width*0.5`-style formulas (`connectors::curved_row_extras`);
/// after the frame rewrite their cached values hold the pre-drag
/// pose. Each formula-driven control re-evaluates against the NEW
/// signed frame (a minimal env: the frame formulas reference
/// Width/Height); a formula the env can't evaluate (endpoints etc.)
/// or a literal (customized) control keeps its current cell.
fn curved_control_caches(
    conn: &vsdx::model::Shape,
    w_new: f64,
    h_new: f64,
) -> Vec<(i64, i64, String, Cell)> {
    let mut out = Vec::new();
    let mut env = vsdx::formula::Env::new();
    env.insert("Width".to_string(), w_new);
    env.insert("Height".to_string(), h_new);
    for g in conn.sheet.sections_named("Geometry") {
        for r in &g.rows {
            // Deleted rows stay deleted (PR #305 review): a cache
            // write planned here lands AFTER a rows_replace on the
            // commit path and SetRowCell would recreate the row; the
            // preview discards it — the two paths would diverge.
            if r.is_deleted() || r.t.as_deref() != Some("CubicBezTo") {
                continue;
            }
            for name in ["A", "B", "C", "D"] {
                let Some(c) = r.cell(name) else {
                    continue;
                };
                let Some(f) = c.f.as_deref() else {
                    continue;
                };
                if f == "Inh" || f == "No Formula" {
                    continue;
                }
                let upper = f.to_uppercase();
                if !["WIDTH", "HEIGHT"].iter().any(|k| upper.contains(k)) {
                    continue;
                }
                if let Some(v) = vsdx::formula::eval(f, &env)
                    .ok()
                    .and_then(|v| v.as_f64())
                    .filter(|v| v.is_finite())
                {
                    out.push((
                        g.ix,
                        r.ix,
                        name.to_string(),
                        Cell {
                            v: Some(format!("{v:.6}")),
                            f: Some(f.to_string()),
                            ..Cell::default()
                        },
                    ));
                }
            }
        }
    }
    out
}

/// The formula an instance geometry-row cell inherits through
/// `F="Inh"`: the MASTER row cell's formula (same section ordinal,
/// same row IX — the pairing `render_all_geometry` uses). `None` when
/// the cell carries its own formula, is a plain literal, or the
/// master row cell has no formula of its own.
fn inherited_geo_formula(
    doc: &Document,
    conn: &vsdx::model::Shape,
    chain: &[&vsdx::model::Shape],
    section_ordinal: usize,
    row_ix: i64,
    name: &str,
) -> Option<String> {
    let c = conn
        .sheet
        .sections_named("Geometry")
        .get(section_ordinal)?
        .rows
        .iter()
        .find(|r| r.ix == row_ix)?
        .cell(name)?;
    if c.f.as_deref() != Some("Inh") {
        return None;
    }
    let mr = doc
        .instance_source(conn, chain)?
        .sheet
        .sections_named("Geometry")
        .get(section_ordinal)?
        .rows
        .iter()
        .find(|r| r.ix == row_ix)?;
    let f = mr.cell(name)?.f.as_deref()?;
    (f != "Inh" && f != "No Formula").then(|| f.to_string())
}

/// The corpus Line-arc pattern (#226 evidence): instance geometry
/// rows carry `F="Inh"` caches whose formulas live on the MASTER row
/// (`X=Width*1`, `Y=Height/2`), and the renderer reads the instance
/// cache as a literal (`row_cell_value`'s inherited rule). A reroute
/// must refresh those caches against the NEW frame — the row-level
/// twin of the #175 XForm cache refresh. Only cells MATERIALIZED on
/// the instance row refresh (an unmaterialized cell evaluates live
/// from the master in the renderer's merge); the cell keeps its
/// `F="Inh"` (a reroute never rewrites a formula), and a no-op cache
/// is left byte-stable.
fn inherited_row_caches(
    doc: &Document,
    conn: &vsdx::model::Shape,
    chain: &[&vsdx::model::Shape],
    w_new: f64,
    h_new: f64,
) -> Vec<(i64, i64, String, Cell)> {
    let mut env = vsdx::formula::Env::new();
    env.insert("Width".to_string(), w_new);
    env.insert("Height".to_string(), h_new);
    let Some(src) = doc.instance_source(conn, chain) else {
        return Vec::new();
    };
    let src_secs = src.sheet.sections_named("Geometry");
    let mut out = Vec::new();
    for (si, g) in conn.sheet.sections_named("Geometry").into_iter().enumerate() {
        let Some(sg) = src_secs.get(si) else { continue };
        for r in &g.rows {
            let Some(mr) = sg.rows.iter().find(|m| m.ix == r.ix) else {
                continue;
            };
            for name in ["X", "Y", "A", "B", "C", "D", "E"] {
                let Some(c) = r.cell(name) else {
                    continue;
                };
                if c.f.as_deref() != Some("Inh") {
                    continue;
                }
                let Some(f) = mr.cell(name).and_then(|m| m.f.as_deref()) else {
                    continue;
                };
                if f == "Inh" || f == "No Formula" {
                    continue;
                }
                let upper = f.to_uppercase();
                if !["WIDTH", "HEIGHT"].iter().any(|k| upper.contains(k)) {
                    continue;
                }
                let Some(v) = vsdx::formula::eval(f, &env)
                    .ok()
                    .and_then(|v| v.as_f64())
                    .filter(|v| v.is_finite())
                else {
                    continue;
                };
                if c.numeric().is_some_and(|old| (old - v).abs() < 1e-9) {
                    continue;
                }
                out.push((
                    g.ix,
                    r.ix,
                    name.to_string(),
                    Cell {
                        v: Some(format!("{v:.6}")),
                        f: Some("Inh".into()),
                        ..Cell::default()
                    },
                ));
            }
        }
    }
    out
}

/// The per-connector frame+rows plan at ONE endpoint pair — the
/// shared core of the commit-time [`RerouteConnectors`] and the live
/// endpoint-drag preview (what you drag is exactly what lands).
/// Endpoint cells are the caller's (the glue engine owns them); the
/// plan covers the frame-cache writes, the route-row rewrites, and
/// the curve-control / inherited-row cache refreshes.
pub struct FramePlan {
    /// XForm cache writes: (cell, value, keep-formula) — `Some(f)`
    /// writes through SetGluedCell (formula kept), `None` is a plain
    /// literal.
    pub cells: Vec<(String, f64, Option<String>)>,
    /// Literal route-row rewrites: (row ix, x, y).
    pub rows: Vec<(i64, f64, f64)>,
    /// #303 GROWTH: a wholesale replacement for the connector's route
    /// rows (a fresh `MoveTo(0,0)` + the route's `LineTo`s,
    /// renumbered 1..=n) in the connector's own Geometry section
    /// (section IX carried — a section with IX≠0 must not be missed
    /// or a second one created, CodeRabbit on the #303 batch). `Some`
    /// on the grow path — the connector's live rows are all straight
    /// runs or RECOGNIZED stored jumps, so the route owns them: the
    /// new route replaces the set (growing a 3-row instance to
    /// Visio's four-row Z, or normalizing a reconciled hop-carrying
    /// row set back to the bare route — the tail
    /// `ReconcileLineJumps` re-adds hops at the new crossings).
    /// `None` keeps the per-row `rows` path.
    pub rows_replace: Option<(i64, Vec<vsdx::model::Row>)>,
    /// Curve-control and inherited-row cache refreshes:
    /// (section ix, row ix, cell name, cell).
    pub row_cells: Vec<(i64, i64, String, Cell)>,
}

/// Plan [`FramePlan`] for one connector at `beg`→`endp` (post-move
/// page-space endpoints). `pre` is the PRE-drag pose
/// ((begin, end)) — the #218 literal-row translation delta — and
/// `jump_radii` is the page's resolved hop radii
/// (`dc::geom::jump_radii_in`): the grow path's stored-jump allowance
/// classifies `ArcTo` rows at the same evidence the renderer strips
/// with.
pub fn plan_connector_frame(
    doc: &Document,
    conn: &vsdx::model::Shape,
    chain: &[&vsdx::model::Shape],
    beg: (f64, f64),
    endp: (f64, f64),
    pre: ((f64, f64), (f64, f64)),
    jump_radii: (f64, f64),
) -> FramePlan {
    let (pre_beg, pre_end) = pre;
    let get = |n: &str| doc.resolve_cell(conn, chain, n).and_then(|v| v.numeric());
    let mut cells: Vec<(String, f64, Option<String>)> = Vec::new();

    // Frame + route once, from both new endpoints.
    let style = get("ShapeRouteStyle")
        .map(|v| format!("{}", v as i64))
        .unwrap_or_else(|| crate::connectors::ROUTE_RIGHT_ANGLE.into());
    let pts = crate::connectors::route(&style, beg, endp);
    let rows = crate::connectors::route_rows(beg, &pts);
    // #175: a derived cell whose effective formula
    // REFERENCES the endpoints/sizes must have its cached V
    // refreshed — the renderer reads the cache, and skipping
    // it (the #174 rule) left the XForm of the OLD geometry
    // under moved endpoints. The write goes through
    // SetGluedCell (GUARD-waived, formula kept); a
    // formula-free cell stays a plain SetCell; a CONSTANT
    // guard (Height=GUARD(0.19685DL), Angle=GUARD(0DA))
    // does not depend on the endpoints and is skipped.
    let eff = |n: &str| dc_core::commands::effective_formula(doc, conn, chain, n);
    let formula_tracks = |n: &str| -> Option<String> {
        let f = eff(n)?;
        let u = f.to_uppercase();
        ["BEGINX", "ENDX", "BEGINY", "ENDY", "WIDTH", "HEIGHT"]
            .iter()
            .any(|k| u.contains(k))
            .then_some(f)
    };
    let (w_new, h_new) = (endp.0 - beg.0, endp.1 - beg.1);
    // #176: a master-based connector whose geometry is
    // LINE-only (MoveTo/LineTo — the corpus line master's
    // own rows are the begin-corner frame (0,0)→(W,H))
    // reroutes by REWRITING its frame to span the new
    // endpoints. Its instance rows are a literal snapshot
    // of the authoring pose (a horizontal band's midline
    // `LineTo(Width, H/2)` with `Height = GUARD(0.2DL)`);
    // translating those in-frame (#175) keeps the ink on
    // the old midline, detached from the moved shape
    // whenever the endpoints differ in y. Curves keep the
    // #175 translation — rewriting an arc's rows to the
    // route polyline would destroy the curve.
    // The master's first-geometry rows — the chain half of the
    // line-geometry and route-ownership tests below.
    let master_rows: Vec<vsdx::model::Row> = conn
        .master
        .and_then(|mid| doc.master(mid))
        .and_then(|m| m.shapes.first())
        .and_then(|root| {
            root.sheet
                .sections_named("Geometry")
                .into_iter()
                .next()
                .map(|g| g.rows.clone())
        })
        .unwrap_or_default();
    // A LINE-geometry master connector — every live row a straight
    // run or a RECOGNIZED stored jump (route_owned_rows: after the
    // reconcile stores hops on a master-based line connector, its
    // ArcTo rows must not push it onto the curve-translate path —
    // CodeRabbit on the #303 batch).
    let line_frame = conn.master.is_some()
        && conn
            .sheet
            .sections_named("Geometry")
            .first()
            .is_some_and(|g| route_owned_rows(g, &master_rows, jump_radii));
    // #226 (MisterDiagram parity evidence): the CHORD-frame
    // masters — a master-based curve connector whose
    // inherited geometry rows derive their coordinates from
    // the frame (the Line-arc corpus master's
    // `EllipticalArcTo X=Width*1, Y=Height/2`) frames its
    // rows on the BEGIN→END chord: the frame width is the
    // chord, the direction lives in Angle, Height stays the
    // master's own 0. Route-frame connectors (the dynamic
    // master's guarded zero angle, the synthetic
    // begin-corner rows) keep the signed-delta model.
    // A fresh stencil-drop instance (#248) has NO local rows, so the
    // inherited-row route below can never fire for it — the master's
    // own frame formulas carry the chord signature instead
    // (`Width=SQRT((EndX-BeginX)^2+…)`, `Angle=ATAN2(…)`): the
    // instance frame IS the begin→end chord.
    let master_chord = doc
        .instance_source(conn, chain)
        .and_then(|src| src.sheet.cell("Width").and_then(|c| c.f.clone()))
        .is_some_and(|f| f.to_uppercase().contains("SQRT("));
    let chord_frame = conn.master.is_some()
        && !line_frame
        && (master_chord
            || conn
                .sheet
                .sections_named("Geometry")
                .iter()
                .enumerate()
                .any(|(si, g)| {
                    g.rows.iter().any(|r| {
                        ["X", "Y"].iter().any(|n| {
                            inherited_geo_formula(doc, conn, chain, si, r.ix, n)
                                .is_some_and(|f| {
                                    let u = f.to_uppercase();
                                    ["WIDTH", "HEIGHT"].iter().any(|k| u.contains(k))
                                })
                        })
                    })
                }));
    // LocPin derives from the EFFECTIVE size: when Height is
    // a constant guard the band keeps its height, so LocPinY
    // stays half the kept height — except a line_frame
    // connector, whose band guard is overridden so the
    // frame spans the endpoints.
    let h_eff = if formula_tracks("Height").is_some() || line_frame {
        h_new
    } else {
        get("Height").unwrap_or(h_new)
    };
    let w_eff = if formula_tracks("Width").is_some() || line_frame {
        w_new
    } else {
        get("Width").unwrap_or(w_new)
    };
    // Frame values: a formula-tracked cell evaluates its
    // EFFECTIVE formula against the NEW endpoints (the chord
    // master's `Width=SQRT((EndX-BeginX)^2+…)` is the chord,
    // not the signed dx the defaults below assume); an
    // unevaluable formula falls back to the signed value.
    // The env grows as each cell resolves so downstream
    // formulas (`LocPinX=Width*0.5`) see the post-write
    // frame — Width/Height resolve first for that.
    let mut frame_env = vsdx::formula::Env::new();
    frame_env.insert("BeginX".to_string(), beg.0);
    frame_env.insert("BeginY".to_string(), beg.1);
    frame_env.insert("EndX".to_string(), endp.0);
    frame_env.insert("EndY".to_string(), endp.1);
    for (n, v) in [
        ("Width", w_new),
        // A chord master's Height is MASTER-OWNED (a locked band or
        // a Controls-driven value): the frame loop skips it, the
        // drop's inherited cache stays, and no write can trip a
        // LockHeight refusal (#248). Signed-frame masters keep
        // Height in the loop (their guards track the endpoints).
        ("Height", if chord_frame { f64::NAN } else { h_new }),
        ("PinX", (beg.0 + endp.0) / 2.0),
        ("PinY", (beg.1 + endp.1) / 2.0),
        ("LocPinX", w_eff / 2.0),
        ("LocPinY", h_eff / 2.0),
    ] {
        if n == "Height" && chord_frame {
            // Keep the current (inherited) value in the frame env
            // for the downstream formulas; write nothing.
            frame_env.insert(n.to_string(), get(n).unwrap_or(h_new));
            continue;
        }
        match formula_tracks(n) {
            Some(f) => {
                let v = vsdx::formula::eval(&f, &frame_env)
                    .ok()
                    .and_then(|v| v.as_f64())
                    .filter(|v| v.is_finite())
                    .unwrap_or(v);
                frame_env.insert(n.to_string(), v);
                // An inherited cache — or a cell the instance does
                // not own at all (#218 review round 3: the Line-arc
                // fixture's LocPinY) — refreshes behind the MARKER:
                // the effective formula is the master's, and writing
                // it locally would override the master and break the
                // "a reroute never rewrites a formula" rule the
                // Angle and row-cache paths already keep. Only an
                // instance's OWN formula is kept verbatim.
                let own_f = conn
                    .sheet
                    .cell(n)
                    .and_then(|c| c.f.clone())
                    .filter(|f| f != "Inh" && f != "No Formula");
                let keep = own_f.unwrap_or_else(|| "Inh".to_string());
                cells.push((n.to_string(), v, Some(keep)));
            }
            // A frame dim with no formula anywhere: rewrite
            // it only when the INSTANCE carries its own
            // cell. An inherited master literal (the
            // Line-arc Height=0, no instance cell) stays
            // inherited — materializing a literal over it
            // corrupts the master link and stretches the
            // band (#226 ownership: a reroute writes only
            // what the connector owns).
            None if eff(n).is_none() && conn.sheet.cell(n).is_some() => {
                frame_env.insert(n.to_string(), v);
                cells.push((n.to_string(), v, None));
            }
            // #176: a line_frame connector refreshes every
            // frame cell through its guard (the formula is
            // kept, the cached V drives the render) — a
            // skipped constant guard would hold the old
            // band pose under the new endpoints.
            None if line_frame => {
                frame_env.insert(n.to_string(), v);
                cells.push((n.to_string(), v, eff(n)));
            }
            // Constant guard or otherwise static: skipped —
            // the frame keeps the current effective value.
            None => {
                frame_env.insert(n.to_string(), get(n).unwrap_or(v));
            }
        }
    }
    // The chord frame rotates with the endpoints; a route
    // frame keeps its guarded or zero angle. The instance's
    // own Angle cache refreshes behind its marker (Inh
    // stays Inh — a reroute never rewrites a formula).
    if chord_frame {
        let angle = (endp.1 - beg.1).atan2(endp.0 - beg.0);
        // The frame angle lives on the INSTANCE cache (the corpus
        // master has no Angle cell; a style cascade may carry an
        // ATAN2 default — that is the style's formula, not the
        // connector's own, and materializing it onto the instance
        // would rewrite ownership (#226: a reroute never rewrites a
        // formula)). Refresh the cache behind the instance's own
        // marker; a connector with its OWN Angle formula keeps it.
        let own_f = conn
            .sheet
            .cell("Angle")
            .and_then(|c| c.f.clone())
            .filter(|f| f != "Inh" && f != "No Formula");
        match own_f {
            Some(f) => cells.push(("Angle".into(), angle, Some(f))),
            None if conn.sheet.cell("Angle").is_some() => {
                cells.push(("Angle".into(), angle, Some("Inh".into())));
            }
            None => {}
        }
    }
    // #174/#175: the route plan fills rows the connector
    // ALREADY has — the arc/curve masters below never see
    // route rows, so a 2-row arc master (MoveTo +
    // EllipticalArcTo) can't grow into a 4-row elbow
    // polyline. The one growth exception is the #303
    // LINE-geometry rule in the route-frame branch: a
    // straight connector whose frame spans the endpoints
    // plans the full Z and apply materializes the missing
    // rows through InsertSectionRow (with a proper row
    // type — SetRowCell's chain-seeded default row would
    // serialize typeless).
    let existing: Vec<i64> = {
        let mut v: Vec<i64> = conn
            .sheet
            .sections_named("Geometry")
            .first()
            .map(|g| g.rows.iter().map(|r| r.ix).collect::<Vec<_>>())
            .unwrap_or_default();
        v.sort_unstable();
        v
    };
    let planned_rows = if conn.master.is_some() && !line_frame {
        // #175: a MASTER-based CURVE connector's rows are
        // its own frame (an arc's EllipticalArcTo) —
        // translating the EXISTING literal coordinates by
        // the endpoint's page delta, per axis, keeps the
        // curve; only where that axis's size tracks the
        // endpoints. Rows whose cells carry formulas
        // evaluate against the refreshed caches in the
        // renderer and are left to it.
        // The rows live in the frame's begin-corner space, so their
        // translation is the CHANGE OF THE begin→end OFFSET, not the
        // end's own movement (#218 review): a moved BEGIN (the end
        // glued elsewhere) previously translated nothing, and the
        // endpoint-drop path — whose `pre_end` already names the new
        // end — saw a zero delta either way.
        let dx = if formula_tracks("Width").is_some() {
            (endp.0 - beg.0) - (pre_end.0 - pre_beg.0)
        } else {
            0.0
        };
        let dy = if formula_tracks("Height").is_some() {
            (endp.1 - beg.1) - (pre_end.1 - pre_beg.1)
        } else {
            0.0
        };
        // A row cell is translatable only when it is a
        // plain literal — including an `F="Inh"` cache
        // whose MASTER row cell is a literal too. An
        // inherited cache whose master formula derives it
        // from the frame (the Line-arc `Width*1`) refreshes
        // as a cache below; translating the stale literal
        // by the page delta would double-move the row and
        // strip the marker (#226).
        let literal = |r: &vsdx::model::Row, name: &str| -> Option<f64> {
            let c = r.cell(name)?;
            match c.f.as_deref() {
                None => c.numeric(),
                Some("Inh") | Some("No Formula") => {
                    let tracks =
                        inherited_geo_formula(doc, conn, chain, 0, r.ix, name)
                            .is_some_and(|f| {
                                let u = f.to_uppercase();
                                ["WIDTH", "HEIGHT"].iter().any(|k| u.contains(k))
                            });
                    (!tracks).then(|| c.numeric()).flatten()
                }
                Some(_) => None,
            }
        };
        let translated = conn
            .sheet
            .sections_named("Geometry")
            .first()
            .map(|g| {
                g.rows
                    .iter()
                    .filter(|r| (2..=4).contains(&r.ix))
                    // Deleted rows are not part of the visible curve —
                    // translating one would un-delete invisible junk
                    // (#293).
                    .filter(|r| !r.is_deleted())
                    .filter_map(|r| {
                        let x = literal(r, "X")?;
                        let y = literal(r, "Y")?;
                        Some((r.ix, x + dx, y + dy))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // A curve keeps its rows — no wholesale replace on this path.
        (translated, None)
    } else {
        // The begin-corner corpus frame: the planned route,
        // filtered to rows the connector has; a connector
        // with fewer rows than the route frame puts the
        // ENDPOINT on its LAST row. Synthetic connectors
        // AND (#176) line-geometry master instances — whose
        // band snapshot rows must be rewritten to span the
        // new endpoints, with the MoveTo reset to (0,0) so
        // the frame's origin is the begin point.
        //
        // #303: when the route OWNS the row set (every live row
        // a straight run or a RECOGNIZED stored jump) and the
        // frame SPANS the new endpoints, the plan REPLACES the
        // rows wholesale — growing a three-row instance to
        // Visio's four-row Z instead of bending the last row
        // into a diagonal, and normalizing a reconciled
        // hop-carrying set (whose row count can exceed four)
        // back to the bare route; the tail ReconcileLineJumps
        // re-adds hops at the new crossings. Authored curves
        // and formula-driven rows keep the per-row path.
        let dim_landed = |n: &str, v: f64| {
            cells
                .iter()
                .find(|(cn, _, _)| cn == n)
                .map(|(_, cv, _)| (cv - v).abs() < 1e-9)
                .unwrap_or(false)
        };
        let frame_spans = dim_landed("Width", w_new) && dim_landed("Height", h_new);
        let route4 = crate::connectors::route(&style, beg, endp).len() == 4;
        let overgrown = existing.iter().any(|ix| *ix > 4);
        // line_frame's route_owned_rows call already decided this
        // (a master-based line connector is route-owned by
        // definition); a synthetic connector is owned when its rows
        // are straight-or-stored-jump.
        let rows_owned = line_frame
            || conn
                .sheet
                .sections_named("Geometry")
                .first()
                .is_some_and(|g| route_owned_rows(g, &master_rows, jump_radii));
        let grow = (route4 || overgrown)
            && !existing.is_empty()
            && rows_owned
            && frame_spans;
        let mut planned: Vec<(i64, f64, f64)> = rows
            .iter()
            .enumerate()
            .map(|(i, (x, y))| ((i + 2) as i64, *x, *y))
            .filter(|(ix, _, _)| existing.contains(ix))
            .collect();
        let max_ix = existing
            .iter()
            .copied()
            .filter(|ix| (2..=4).contains(ix))
            .max();
        if let (Some(last), Some(max_ix)) = (planned.last_mut(), max_ix) {
            if max_ix < 4 {
                *last = (max_ix, endp.0 - beg.0, endp.1 - beg.1);
            }
        }
        if line_frame && existing.contains(&1) {
            planned.insert(0, (1, 0.0, 0.0));
        }
        let rows_replace = grow.then(|| {
            let geos = conn.sheet.sections_named("Geometry");
            let geo = geos.first();
            let sec_ix = geo.map_or(0, |g| g.ix);
            // Row 1 keeps its attrs (the corpus MoveTo marker), reset
            // to the begin corner; rows 2..=4 are fresh typed LineTos
            // with the route values. The origin clones only a LIVE
            // IX-1 row — a Del="1" shadow would drag its marker into
            // the rebuilt route and delete the origin — and its type
            // is forced to an absolute MoveTo (the reconcile's row-1
            // hardening: route_owned_rows admits a LineTo/RelLineTo
            // IX 1, which never positions the pen, and a kept
            // RelMoveTo scales by the frame on the next walk; review
            // on the #303 batch).
            let mut first = geo
                .and_then(|g| g.rows.iter().find(|r| r.ix == 1 && !r.is_deleted()))
                .cloned()
                .unwrap_or_else(|| vsdx::model::Row {
                    t: Some("MoveTo".into()),
                    ix: 1,
                    ..Default::default()
                });
            first.t = Some("MoveTo".into());
            for (name, v) in [("X", 0.0), ("Y", 0.0)] {
                let cell = Cell::value(format!("{v:.6}"));
                if let Some(pos) = first.cells.iter().position(|(n, _)| n == name) {
                    first.cells[pos].1 = cell;
                } else {
                    first.cells.push((name.to_string(), cell));
                }
            }
            let mut rep = vec![first];
            for (i, (x, y)) in rows.iter().enumerate() {
                rep.push(vsdx::model::Row {
                    t: Some("LineTo".into()),
                    ix: (i + 2) as i64,
                    cells: vec![
                        ("X".to_string(), Cell::value(format!("{x:.6}"))),
                        ("Y".to_string(), Cell::value(format!("{y:.6}"))),
                    ],
                    ..Default::default()
                });
            }
            // LIVE master rows the new route does not cover would come
            // back as MASTER-ONLY rows through the walk — the replace
            // drops the instance rows that shadowed them (live
            // overrides or Del markers), resurrecting a stale tail and
            // knocking the connector out of the reconcile's coverage
            // gate. Keep them suppressed with #293 deleted overrides,
            // the reconcile's (K) block exactly — including when the
            // instance GROWS: what matters is the MASTER's row set,
            // not the old instance's (review on the #303 batch).
            let have: Vec<i64> = rep.iter().map(|r| r.ix).collect();
            for m in &master_rows {
                if !m.is_deleted() && !have.contains(&m.ix) {
                    rep.push(vsdx::model::Row {
                        t: m.t.clone(),
                        ix: m.ix,
                        attrs: vec![("Del".to_string(), "1".to_string())],
                        ..Default::default()
                    });
                }
            }
            (sec_ix, rep)
        });
        let planned = if grow { Vec::new() } else { planned };
        (planned, rows_replace)
    };
    let (planned_rows, rows_replace) = planned_rows;
    // The post-write frame the row-cache refreshes evaluate
    // against: the planned value when the frame loop
    // rewrote the dim, the current effective value when it
    // deliberately skipped (constant guard, inherited
    // literal).
    let post_dim = |name: &str, fallback: f64| -> f64 {
        cells
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, v, _)| *v)
            .unwrap_or_else(|| get(name).unwrap_or(fallback))
    };
    let (w_post, h_post) = (post_dim("Width", w_new), post_dim("Height", h_new));
    let mut row_cells = curved_control_caches(conn, w_post, h_post);
    if chord_frame {
        // The chord-frame rows' inherited caches
        // (`X=Width*1`, `Y=Height/2`) re-evaluate against
        // the new frame — the arc's end lands on the moved
        // endpoint (the #175 rule, one level down). The
        // sagitta controls (`Controls.Row_1`) don't
        // reference the frame and stay.
        row_cells.extend(inherited_row_caches(doc, conn, chain, w_post, h_post));
    }

    FramePlan {
        cells,
        rows: planned_rows,
        rows_replace,
        row_cells,
    }
}

/// The #303 grow gate: may the ROUTE own this connector's live
/// geometry rows wholesale? True when every live row is a straight
/// run, or an `ArcTo` that classifies as a STORED JUMP at `radii` —
/// the same evidence the renderer's strip uses
/// (`dc::geom::is_stored_jump_arc`). Straight rows are replaceable
/// wherever their literals live (the per-row path rewrites them
/// equally — the corpus instances carry PARTIAL rows with per-cell
/// inheritance, e.g. a MoveTo with only X local); an `ArcTo` reads
/// its A/X/Y through the instance-or-master chain and must be
/// literal (formula-driven arcs take the conservative path). Authored
/// curves (`EllipticalArcTo`, splines) and an authored semicircle
/// answer false: a wholesale route replace would destroy them.
fn route_owned_rows(
    g: &vsdx::model::Section,
    master_rows: &[vsdx::model::Row],
    radii: (f64, f64),
) -> bool {
    let live: Vec<&vsdx::model::Row> = g.rows.iter().filter(|r| !r.is_deleted()).collect();
    if live.is_empty() {
        return false;
    }
    // A literal cell through the chain: the instance's own cell, else
    // the master row's — a value with no formula, or an inherited
    // cache (`F="Inh"`/`No Formula`) whose V carries the literal. An
    // inherited cache counts ONLY when its master source carries no
    // effective formula — a formula-driven master cell (the Line-arc
    // `Width*1`) is derived tracking, and freezing its stale cached V
    // as a literal would destroy the authored row (#226's rule, the
    // planner's inherited_geo_formula; review on the #303 batch).
    let master_row = |ix: i64| master_rows.iter().find(|r| r.ix == ix);
    let lit = |r: &vsdx::model::Row, n: &str| -> Option<f64> {
        let own = r.cell(n);
        let cell = own.or_else(|| master_row(r.ix).and_then(|m| m.cell(n)))?;
        if matches!(cell.f.as_deref(), Some("Inh") | Some("No Formula")) && own.is_some() {
            let derived = master_row(r.ix)
                .and_then(|m| m.cell(n))
                .and_then(|c| c.f.as_deref())
                .is_some_and(|f| f != "Inh" && f != "No Formula");
            if derived {
                return None;
            }
        }
        match cell.f.as_deref() {
            None | Some("Inh") | Some("No Formula") => cell.numeric(),
            _ => None,
        }
    };
    // The EFFECTIVE type: a typeless local row inherits its master
    // row's type (a None-`T` override over a master `CubicBezTo` is a
    // curve, not a straight run — CodeRabbit on the #303 batch).
    fn eff_t<'a>(
        r: &'a vsdx::model::Row,
        master_rows: &'a [vsdx::model::Row],
    ) -> Option<&'a str> {
        r.t.as_deref().or_else(|| {
            master_rows
                .iter()
                .find(|m| m.ix == r.ix)
                .and_then(|m| m.t.as_deref())
        })
    }
    let straight = |t: Option<&str>| {
        matches!(
            t,
            Some("MoveTo") | Some("RelMoveTo") | Some("LineTo") | Some("RelLineTo")
        )
    };
    let pt =
        |r: &vsdx::model::Row| -> Option<(f64, f64)> { Some((lit(r, "X")?, lit(r, "Y")?)) };
    for (i, r) in live.iter().enumerate() {
        let t = eff_t(r, master_rows);
        if straight(t) {
            // A straight run is replaceable only where its
            // coordinates are LITERALS through the chain — a formula
            // (`X=Width`) is authored tracking and must survive
            // (CodeRabbit on the #303 batch).
            if lit(r, "X").is_none() || lit(r, "Y").is_none() {
                return false;
            }
            continue;
        }
        if t != Some("ArcTo") {
            return false;
        }
        // A stored jump: literal A + endpoint, a semicircle on a run
        // that continues on both sides, at the page-radius evidence.
        let (Some(a), Some(end)) = (lit(r, "A"), pt(r)) else {
            return false;
        };
        let Some(start) = i.checked_sub(1).and_then(|p| pt(live[p])) else {
            return false;
        };
        let mut prev_dir = None;
        let mut p = i;
        while p > 0 {
            p -= 1;
            if !straight(eff_t(live[p], master_rows)) {
                break; // a curve neighbor ends the run
            }
            let Some(q) = pt(live[p]) else {
                return false;
            };
            let d = (start.0 - q.0, start.1 - q.1);
            if d.0.hypot(d.1) > 1e-9 {
                prev_dir = Some(d);
                break;
            }
        }
        let mut next_dir = None;
        let mut n = i + 1;
        while n < live.len() {
            if !straight(eff_t(live[n], master_rows)) {
                break;
            }
            let Some(q) = pt(live[n]) else {
                return false;
            };
            let d = (q.0 - end.0, q.1 - end.1);
            if d.0.hypot(d.1) > 1e-9 {
                next_dir = Some(d);
                break;
            }
            n += 1;
        }
        if !dc::geom::is_stored_jump_arc(
            a,
            (end.0 - start.0, end.1 - start.1),
            prev_dir,
            next_dir,
            radii.0,
            radii.1,
        ) {
            return false;
        }
    }
    true
}

impl Command for RerouteConnectors {
    fn touched(&self, doc: &Document) -> dc_core::TouchList {
        // #178: after undo spends `inner`, the cached apply-time list
        // still names every connector that was rewritten — undo must
        // re-raster them (Ink), or their textures keep the dragged
        // raster under the restored bounding box.
        if self.inner.is_empty() {
            if !self.touched_cache.shapes.is_empty() || !self.touched_cache.pages.is_empty() {
                return self.touched_cache.clone();
            }
            // Pre-apply (#236): the apply-time list does not exist yet
            // (apply builds it), so discover the candidate connectors
            // read-only and name each one's Ink touch — with `sub` on
            // a NESTED connector, the layer store harvests its
            // pre-reroute bounds and pairs them against the post state
            // for a damage-region patch. Top-level connectors stay
            // sub-less: a reroute resizes the connector's own frame,
            // so its whole texture re-renders either way.
            let mut all = dc_core::TouchList::default();
            for conn_id in self.candidate_connectors(doc) {
                let Some(root) = dc_core::scope::top_level_ancestor(doc, self.page, conn_id) else {
                    continue;
                };
                all.merge(dc_core::TouchList {
                    pages: vec![],
                    shapes: vec![dc_core::Touched {
                        page: self.page,
                        shape: root,
                        touch: dc_core::Touch::Ink,
                        sub: (root != conn_id).then_some(conn_id),
                    }],
                });
            }
            return all;
        }
        let mut all = dc_core::TouchList::default();
        for c in &self.inner {
            all.merge(c.touched(doc));
        }
        all
    }

    fn label(&self) -> String {
        format!("reroute connectors for {} moved shape(s)", self.moved.len())
    }

    fn apply(&mut self, doc: &mut Document) -> anyhow::Result<()> {
        // Pass 1 (immutable): plan every reroute against the current
        // (post-move) document.
        struct Plan {
            conn_id: i64,
            cells: Vec<(String, f64, Option<String>)>,
            rows: Vec<(i64, f64, f64)>,
            /// #303: the wholesale route-row replacement (the grow
            /// path) — one SetSectionRows replaces the row set.
            rows_replace: Option<(i64, Vec<vsdx::model::Row>)>,
            /// #202: CubicBezTo control-cache refreshes — (section
            /// ix, row ix, cell name, new cell). The curved route's
            /// controls are
            /// Width/Height FORMULAS; the frame rewrite leaves their
            /// cached values at the pre-drag pose, and anything
            /// reading caches (hit samples, ShapeSheet) sees the old
            /// curve. Literal (customized) controls stay untouched.
            row_cells: Vec<(i64, i64, String, Cell)>,
        }
        // Both ends may glue to moved shapes (a two-shape selection
        // drag): each connector is planned ONCE from both new
        // endpoints — per-Connect plans would race on stale opposite
        // ends (CodeRabbit).
        let mut plans: Vec<Plan> = Vec::new();
        {
            // The candidate set comes from the SAME helper touched()
            // uses (#218 review) — the glue records of the moved
            // shapes plus the #179 also-list, with the #174
            // chain-aware test, the FixedRoute bail and the
            // resolvable-endpoint check already applied.
            let candidates = self.candidate_connectors_with_records(doc);
            let trect = |t: &vsdx::model::Shape| -> Option<(f64, f64, f64, f64)> {
                let ch = dc_core::scope::shape_chain(doc, self.page, t.id).ok()?;
                let g = |n: &str| doc.resolve_cell(t, &ch, n).and_then(|v| v.numeric());
                let (px, py) = (g("PinX")?, g("PinY")?);
                let (w, h) = (g("Width").unwrap_or(1.0), g("Height").unwrap_or(1.0));
                let (lx, ly) = (
                    g("LocPinX").unwrap_or(w / 2.0),
                    g("LocPinY").unwrap_or(h / 2.0),
                );
                let (x, y) = (px - lx, py - ly);
                Some((x.min(x + w), y.min(y + h), w.abs(), h.abs()))
            };
            // A stale page id (a queued/replayed reroute after the
            // page's deletion) is a no-op, not a panic (#218 review).
            let Some(page_ref) = doc.pages.iter().find(|p| p.id == self.page) else {
                return Ok(());
            };
            let page_shapes = &page_ref.shapes;
            // #303: the grow gate's stored-jump evidence — the page's
            // resolved hop radii, the same values the renderer strips
            // with.
            let jump_radii = dc::geom::jump_radii_in(doc, page_ref);
            for (conn_id, recs) in candidates {
                let Some(conn) = vsdx::model::find_shape_by_id(page_shapes, conn_id) else {
                    continue;
                };
                let Ok(chain) = dc_core::scope::shape_chain(doc, self.page, conn.id) else {
                    continue;
                };
                let get = |n: &str| doc.resolve_cell(conn, &chain, n).and_then(|v| v.numeric());
                let (Some(bx), Some(by), Some(ex), Some(ey)) =
                    (get("BeginX"), get("BeginY"), get("EndX"), get("EndY"))
                else {
                    continue;
                };
                // Resolve every moved end's attachment against the
                // post-move shapes BEFORE any write.
                let mut beg = (bx, by);
                let mut endp = (ex, ey);
                let mut end_cells: Vec<(String, f64, Option<String>)> = Vec::new();
                for c in recs {
                    let end = if c.from_cell == "BeginX" {
                        crate::connectors::End::Begin
                    } else {
                        crate::connectors::End::End
                    };
                    let Some(target) = vsdx::model::find_shape_by_id(page_shapes, c.to_sheet)
                    else {
                        continue;
                    };
                    let other = match end {
                        crate::connectors::End::Begin => endp,
                        crate::connectors::End::End => beg,
                    };
                    let attach = if c.to_part >= 100 {
                        let row_ix = c.to_part - 100;
                        trect(target).and_then(|(rx, ry, _, _)| {
                            let find = |sheet: &vsdx::model::Sheet| -> Option<(f64, f64)> {
                                for sec in sheet.sections_named("Connection") {
                                    if let Some(row) = sec.rows.iter().find(|r| r.ix == row_ix) {
                                        if let (Some(x), Some(y)) = (
                                            row.cell("X").and_then(|v| v.numeric()),
                                            row.cell("Y").and_then(|v| v.numeric()),
                                        ) {
                                            return Some((rx + x, ry + y));
                                        }
                                    }
                                }
                                None
                            };
                            find(&target.sheet).or_else(|| {
                                target
                                    .master
                                    .and_then(|mid| doc.master(mid))
                                    .and_then(|m| m.shapes.first())
                                    .and_then(|root| find(&root.sheet))
                            })
                        })
                    } else {
                        trect(target).map(|r| crate::connectors::attach_point(r, other))
                    };
                    let Some(attach) = attach else { continue };
                    match end {
                        crate::connectors::End::Begin => beg = attach,
                        crate::connectors::End::End => endp = attach,
                    }
                    let endf = conn.sheet.cell(end.cell()).and_then(|v| v.f.clone());
                    end_cells.push((end.cell().to_string(), attach.0, endf.clone()));
                    end_cells.push((end.y_cell().to_string(), attach.1, endf));
                }
                if end_cells.is_empty() && !self.also.contains(&conn_id) {
                    continue;
                }
                // Frame + route once, from both new endpoints — the
                // shared planner (the live endpoint-drag preview plans
                // through the same function).
                // The endpoint-drop path carries its PRE-drag pose
                // (#218 review): the drop wrote the new endpoint
                // cells before apply, so (bx..ey) re-read as the NEW
                // values and the literal-row delta collapsed to zero.
                let (pre_beg, pre_end) = self
                    .also_pre
                    .get(&conn.id)
                    .copied()
                    .unwrap_or(((bx, by), (ex, ey)));
                let plan = plan_connector_frame(
                    doc, conn, &chain, beg, endp, (pre_beg, pre_end), jump_radii,
                );
                let mut cells = end_cells;
                cells.extend(plan.cells);
                plans.push(Plan {
                    conn_id: conn.id,
                    cells,
                    rows: plan.rows,
                    rows_replace: plan.rows_replace,
                    row_cells: plan.row_cells,
                });
            }
        }
        // Pass 2 (mutable): apply the planned commands, capturing each
        // for undo.
        for plan in plans {
            let scope = dc_core::Scope::Shape {
                page: self.page,
                shape: plan.conn_id,
            };
            for (n, v, f) in plan.cells {
                // Formula-carrying writes are the glue engine
                // updating its own GUARDed endpoint caches —
                // SetGluedCell (plain derived cells stay SetCell).
                let mut c = if f.is_some() {
                    Box::new(dc_core::SetGluedCell::new(
                        scope.clone(),
                        n,
                        Cell {
                            v: Some(format!("{v:.6}")),
                            f,
                            ..Cell::default()
                        },
                    )) as Box<dyn Command>
                } else {
                    Box::new(SetCell::new(
                        scope.clone(),
                        n,
                        Cell {
                            v: Some(format!("{v:.6}")),
                            f,
                            ..Cell::default()
                        },
                    )) as Box<dyn Command>
                };
                c.apply(doc)?;
                self.inner.push(c);
            }
            // #303: the grow path replaces the route rows wholesale —
            // growing a three-row instance to the four-row Z, or
            // normalizing a reconciled hop-carrying set — in ONE
            // undoable SetSectionRows.
            if let Some((sec_ix, rep)) = plan.rows_replace {
                let mut c = dc_core::SetSectionRows::new(scope.clone(), "Geometry", sec_ix, rep);
                c.apply(doc)?;
                self.inner.push(Box::new(c));
            }
            for (ix, x, y) in plan.rows {
                for (cell, v) in [("X", x), ("Y", y)] {
                    let mut c = dc_core::SetRowCell::new(
                        scope.clone(),
                        "Geometry",
                        0,
                        dc_core::commands::RowSel::Ix(ix),
                        cell,
                        Cell::value(format!("{v:.6}")),
                    );
                    c.apply(doc)?;
                    self.inner.push(Box::new(c));
                }
            }
            // #202: refresh the curved route's formula-driven control
            // caches against the new frame (the formula is kept — the
            // SetGluedCell pattern; a customized literal stays).
            for (sec_ix, row_ix, name, cell) in &plan.row_cells {
                let mut c = dc_core::SetRowCell::new(
                    scope.clone(),
                    "Geometry",
                    *sec_ix,
                    dc_core::commands::RowSel::Ix(*row_ix),
                    name.clone(),
                    cell.clone(),
                );
                c.apply(doc)?;
                self.inner.push(Box::new(c));
            }
        }
        // #303: the reroute moved routes — the page's stored jumps may
        // sit at stale crossings now (the #303 repro's crossed A→B
        // keeps its old bump while nothing crosses there any more).
        // Reconcile them in the SAME undo step, after every connector
        // landed.
        let mut rec = ReconcileLineJumps::new(self.page);
        rec.apply(doc)?;
        self.inner.push(Box::new(rec));
        // #178: capture the merged touch list while `inner` still
        // holds the spent commands — `undo` clears it, but the
        // history asks `touched()` after undo.
        let mut all = dc_core::TouchList::default();
        for c in &self.inner {
            all.merge(c.touched(doc));
        }
        self.touched_cache = all;
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> anyhow::Result<()> {
        for c in self.inner.iter_mut().rev() {
            c.undo(doc)?;
        }
        self.inner.clear();
        Ok(())
    }
}

/// #303: reconcile the page's stored jump `ArcTo` rows with the
/// CURRENT crossings — the file-level twin of the #294 render-time
/// strip. Tarahi straightens a stored arc when RENDERING; other
/// readers (Visio included) draw what the rows say, so a move that
/// changes the crossings must rewrite them: stale arcs drop, and hops
/// land at the current crossings as stored `ArcTo` rows
/// (`A = −radius`, the corpus-pinned form) — Visio opening a saved
/// file sees the same hops Tarahi's overlay draws. #301's twin rides
/// the same pass with `with_strip`: a jump-size edit classifies at
/// the PRE-edit radius (so stored arcs stay recognized) and rewrites
/// at the NEW radius — the decoration tracks the setting, like
/// Visio's own re-layout.
///
/// Scope guard (authored geometry is never touched): a connector
/// reconciles only when its first Geometry section is fully
/// route-owned — every live row a straight run or an `ArcTo` the
/// renderer itself strips (`geometry_hit_segments`, the exact same
/// walk), all row coordinates literal, no master-only rows. Curves,
/// splines, formula-driven rows and `FixedRoute` connectors keep
/// their rows verbatim.
pub struct ReconcileLineJumps {
    page: i64,
    /// Classification radius override (#301): `Some(pre_edit)` when a
    /// jump-size edit just moved the page radius.
    strip: Option<(f64, f64)>,
    inner: Vec<Box<dyn Command>>,
    touched_cache: dc_core::TouchList,
    /// Whether apply has run — a completed NO-OP (empty cache) must
    /// not fall back to candidate discovery and dirty every
    /// connector's layer (CodeRabbit on the #303 batch).
    applied: bool,
}

impl ReconcileLineJumps {
    pub fn new(page: i64) -> ReconcileLineJumps {
        ReconcileLineJumps {
            page,
            strip: None,
            inner: Vec::new(),
            touched_cache: dc_core::TouchList::default(),
            applied: false,
        }
    }

    /// Classify stored jumps at `strip` (the PRE-edit radius), rewrite
    /// at the page's current radius (#301's normalize-on-edit).
    pub fn with_strip(page: i64, strip: (f64, f64)) -> ReconcileLineJumps {
        ReconcileLineJumps {
            strip: Some(strip),
            ..ReconcileLineJumps::new(page)
        }
    }
}

/// One connector's reconciliation target: the row-local route rows.
fn reconciled_rows(
    radii: (f64, f64),
    shown: bool,
    frame: &dc::geom::HitFrame,
    pts: &[(f64, f64)],
    hops: &[dc::geom::Jump],
    first_row: Option<&vsdx::model::Row>,
) -> Vec<vsdx::model::Row> {
    let to_local = |p: (f64, f64)| frame.to_local(p);
    let point_cells = |p: (f64, f64)| -> Vec<(String, Cell)> {
        vec![
            ("X".to_string(), Cell::value(format!("{:.6}", p.0))),
            ("Y".to_string(), Cell::value(format!("{:.6}", p.1))),
        ]
    };
    // Collapse collinear skeleton points first: a stripped arc leaves
    // its chord stubs, and a straight route's degenerate elbows
    // repeat — the hop split re-introduces its own endpoints, so the
    // merged skeleton is the minimal true route. A middle point
    // survives only when it TURNS the route: look ahead (the point
    // after it through the last kept), since the first candidate has
    // no prior direction.
    let mut skel: Vec<(f64, f64)> = vec![pts[0]];
    for (i, p) in pts[1..pts.len() - 1].iter().enumerate() {
        let q = pts[i + 2];
        let keep = match skel.last().copied() {
            Some(a) => {
                let (d1, d2) = ((p.0 - a.0, p.1 - a.1), (q.0 - p.0, q.1 - p.1));
                let (l1, l2) = (d1.0.hypot(d1.1), d2.0.hypot(d2.1));
                // Collinear AND same-direction: a reversal (an
                // excursion out and back) is a real route feature and
                // survives (CodeRabbit on the #303 batch).
                let removable = l1 < 1e-9
                    || (l2 >= 1e-9
                        && (d1.0 * d2.1 - d1.1 * d2.0).abs() <= 1e-6 * l1 * l2
                        && d1.0 * d2.0 + d1.1 * d2.1 > 0.0);
                !removable
            }
            None => true,
        };
        if keep {
            skel.push(*p);
        }
    }
    let last = pts[pts.len() - 1];
    if skel
        .last()
        .is_none_or(|a| (a.0 - last.0).hypot(a.1 - last.1) >= 1e-9)
    {
        skel.push(last);
    }
    let pts = &skel[..];
    // Row 1 keeps its attrs (the corpus MoveTo marker) at the
    // skeleton's start — but its TYPE becomes an absolute `MoveTo`:
    // the written X/Y are absolute locals, and a kept `RelMoveTo`
    // would scale them by the frame on the next walk (CodeRabbit on
    // the #303 batch).
    let mut first = first_row
        .cloned()
        .unwrap_or_else(|| vsdx::model::Row {
            t: Some("MoveTo".into()),
            ix: 1,
            ..Default::default()
        });
    first.t = Some("MoveTo".into());
    let start = to_local(pts[0]);
    for (name, v) in [("X", start.0), ("Y", start.1)] {
        let cell = Cell::value(format!("{v:.6}"));
        if let Some(pos) = first.cells.iter().position(|(n, _)| n == name) {
            first.cells[pos].1 = cell;
        } else {
            first.cells.push((name.to_string(), cell));
        }
    }
    let mut rows = vec![first];
    let push_line = |rows: &mut Vec<vsdx::model::Row>, p: (f64, f64)| {
        let ix = rows.len() as i64 + 1;
        rows.push(vsdx::model::Row {
            t: Some("LineTo".into()),
            ix,
            cells: point_cells(p),
            ..Default::default()
        });
    };
    for (si, w) in pts.windows(2).enumerate() {
        let (a, b) = (w[0], w[1]);
        let seg = (b.0 - a.0, b.1 - a.1);
        let len = seg.0.hypot(seg.1);
        if len < 1e-9 {
            continue;
        }
        let dir = (seg.0 / len, seg.1 / len);
        // (F) The arc's axis picks the radius in the ROW-LOCAL frame
        // — the frame's rotation can turn a page-horizontal run into
        // a local-vertical one, and the classifier re-reads the arc
        // through the local chord; picking the axis in page space
        // could write |A| at the OTHER axis's radius and fail the 5%
        // band on the next walk (CodeRabbit on the #303 batch). The
        // frame transform is rigid, so page and local distances
        // agree and the same r places the split points.
        let ldir = {
            let e = to_local(b);
            let s0 = to_local(a);
            (e.0 - s0.0, e.1 - s0.1)
        };
        let r = if ldir.0.abs() >= ldir.1.abs() {
            radii.0
        } else {
            radii.1
        };
        // The hops on THIS segment, ordered along the run.
        let mut here: Vec<f64> = hops
            .iter()
            .filter(|j| {
                let d = (j.at.0 - a.0, j.at.1 - a.1);
                let t = d.0 * dir.0 + d.1 * dir.1;
                let perp = (d.0 * dir.1 - d.1 * dir.0).abs();
                t > 1e-9 && t < len - 1e-9 && perp < 1e-6
            })
            .map(|j| (j.at.0 - a.0) * dir.0 + (j.at.1 - a.1) * dir.1)
            .collect::<Vec<_>>();
        here.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
        // The classifier strips only an axis-ALIGNED local chord
        // (`is_stored_jump_arc`) — an arc stored on a diagonal run
        // could never be recognized again, so a diagonal crossing
        // stays plain in the file (the overlay still draws the hop;
        // PR #305 review).
        let axis_aligned = {
            let alen = ldir.0.hypot(ldir.1);
            alen > 1e-9 && ldir.0.abs().min(ldir.1.abs()) <= alen * 0.02
        };
        let arcs = shown && r > 0.0 && axis_aligned;
        // (C) Two hops closer than 2r would emit an arc starting
        // before the previous one ends — a backwards segment. Walk a
        // cursor: a hop that does not clear the last arc's end is
        // left as a plain crossing.
        let mut cursor = 0.0f64;
        for t in here {
            // (D) The geometry walks cap at 64 rows: an arc pair
            // must leave room for every remaining route point (the
            // segment-end lines) and the caller's master shadows —
            // reserve, don't just cap (PR #305 review). Remaining
            // hops render through the overlay instead.
            if rows.len() + 2 + (pts.len() - (si + 1)) > 60 {
                break;
            }
            // The 2e-6 margin keeps the gap between adjacent arcs
            // nonzero after six-decimal row formatting — a zero-length
            // LineTo between arcs breaks the classifier's neighbor
            // walk and both arcs would stop being stored jumps (PR
            // #305 review).
            if !arcs || t - r <= cursor + 2e-6 || t + r >= len - 1e-9 {
                continue; // overlapping or no room — a plain crossing
            }
            cursor = t + r;
            let p0 = (a.0 + dir.0 * (t - r), a.1 + dir.1 * (t - r));
            let p1 = (a.0 + dir.0 * (t + r), a.1 + dir.1 * (t + r));
            push_line(&mut rows, to_local(p0));
            let ix = rows.len() as i64 + 1;
            let end = to_local(p1);
            rows.push(vsdx::model::Row {
                t: Some("ArcTo".into()),
                ix,
                cells: vec![
                    ("X".to_string(), Cell::value(format!("{:.6}", end.0))),
                    ("Y".to_string(), Cell::value(format!("{:.6}", end.1))),
                    // The corpus form: A = −radius (#294's pinned row).
                    ("A".to_string(), Cell::value(format!("{:.6}", -r))),
                ],
                ..Default::default()
            });
        }
        push_line(&mut rows, to_local(b));
    }
    rows
}

impl Command for ReconcileLineJumps {
    fn touched(&self, doc: &Document) -> dc_core::TouchList {
        // #178's pattern: post-apply the cache names what changed;
        // pre-apply (a queued/replayed command, a Macro's pre-damage
        // harvest) discover the candidate connectors read-only. After
        // a completed apply, the cache is the answer even when EMPTY
        // — a no-op touched nothing (falling through to discovery
        // would Ink every connector and waste their layer rasters).
        if self.applied {
            return self.touched_cache.clone();
        }
        let Some(page) = doc.pages.iter().find(|p| p.id == self.page) else {
            return dc_core::TouchList::default();
        };
        let mut list = dc_core::TouchList::default();
        for s in &page.shapes {
            if s.type_.as_deref() == Some("Guide") {
                continue;
            }
            let mut chain = vec![s];
            if let Some(mid) = s.master {
                if let Some(root) = doc.master(mid).and_then(|m| m.shapes.first()) {
                    chain.push(root);
                }
            }
            if dc::geom::is_connector_in(doc, &chain, s) {
                list.shapes.push(dc_core::Touched {
                    page: self.page,
                    shape: s.id,
                    touch: dc_core::Touch::Ink,
                    sub: None,
                });
            }
        }
        // No connectors → no candidates → nothing changes: an EMPTY
        // touch list, never a page touch (a no-op reconcile must not
        // dirty the page and clobber bounded damage regions).
        list
    }

    fn label(&self) -> String {
        "reconcile line jumps".into()
    }

    fn apply(&mut self, doc: &mut Document) -> anyhow::Result<()> {
        let Some(page) = doc.pages.iter().find(|p| p.id == self.page) else {
            return Ok(());
        };
        let radii = dc::geom::jump_radii_in(doc, page);
        let strip = self.strip.unwrap_or(radii);
        let shown = page.line_jumps_shown();
        // The hops compute at the CURRENT radius with the override's
        // classification (#301): page_jumps_stripping is exactly the
        // render overlay's crossing walk. Hidden jumps never consume
        // hops (reconciled_rows writes no arcs), so the page-wide
        // crossing discovery is skipped and the hop list is empty —
        // which also keeps the no-op fast path honest: a connector
        // with no stored arcs is not rewritten (collapsing degenerate
        // elbows) just because INVISIBLE crossings exist. The
        // per-connector strip pass below is untouched: stored arcs
        // fail plain==stripped on their own, so Hide Line Jumps still
        // straightens them (review on the #303 batch).
        let hops = if shown {
            dc::geom::page_jumps_stripping(doc, page, &[], self.strip)
        } else {
            Vec::new()
        };
        // Snapshot the candidates' data read-only, then write.
        struct Cand {
            id: i64,
            geo_ix: i64,
            rows: Vec<vsdx::model::Row>,
        }
        let mut cands: Vec<Cand> = Vec::new();
        for s in &page.shapes {
            if s.type_.as_deref() == Some("Guide") {
                continue;
            }
            // Hidden layers: nothing persists layer hiding in the
            // model (only AddLayer seeds Visible=1), so the file's
            // own visibility set is what other readers see — no
            // hidden filter here.
            let mut chain = vec![s];
            let msize = s
                .master
                .and_then(|mid| doc.master(mid))
                .and_then(|m| m.shapes.first())
                .map(|root| {
                    chain.push(root);
                    let g = |n: &str| {
                        doc.resolve_cell(root, &[root], n)
                            .and_then(|c| c.numeric())
                            .unwrap_or(1.0)
                    };
                    (g("Width"), g("Height"))
                })
                .unwrap_or((1.0, 1.0));
            if !dc::geom::is_connector_in(doc, &chain, s) {
                continue;
            }
            let weight = doc
                .resolve_cell(s, &chain, "LineWeight")
                .and_then(|c| c.numeric())
                .unwrap_or(0.01);
            if weight == 0.0 {
                continue;
            }
            if doc
                .resolve_cell(s, &chain, "FixedRoute")
                .and_then(|c| c.numeric())
                .is_some_and(|v| v == 1.0)
            {
                continue;
            }
            // One OWN geometry section, fully route-owned: all live
            // rows straight-or-stored-jump with literal coordinates,
            // and no live master-only rows the replace would orphan.
            let geos = s.sheet.sections_named("Geometry");
            if geos.len() != 1 {
                continue;
            }
            let geo = &geos[0];
            // The section's own IX — the replace targets it, never a
            // hardcoded 0 (CodeRabbit on the #303 batch).
            let geo_ix = geo.ix;
            // The walk reads at most MAX_WALK_ROWS rows — a longer
            // section's tail is invisible to it, and a replace built
            // from the walk would ERASE those rows from the file
            // (review on the #303 batch). Leave such connectors
            // verbatim, like other authored geometry; the overlay
            // still draws hops render-time for the crossings it sees.
            if geo.rows.len() > dc::geom::MAX_WALK_ROWS {
                continue;
            }
            let live: Vec<&vsdx::model::Row> =
                geo.rows.iter().filter(|r| !r.is_deleted()).collect();
            if live.len() < 2 {
                continue;
            }
            // Literal through the chain (the corpus instances carry
            // PARTIAL rows — a MoveTo with only X local, the rest
            // inherited per-cell): instance cell first, else the
            // master root's row. Straight rows need no check at all
            // (the rewrite replaces them wherever their values live);
            // an ArcTo must read A/X/Y as literals somewhere.
            let master_geo_rows: Vec<i64> = chain
                .get(1)
                .and_then(|root| {
                    root.sheet
                        .sections_named("Geometry")
                        .into_iter()
                        .next()
                        .map(|g| g.rows.iter().filter(|r| !r.is_deleted()).map(|r| r.ix).collect::<Vec<_>>())
                })
                .unwrap_or_default();
            let master_row = |ix: i64| -> Option<&vsdx::model::Row> {
                chain.get(1).and_then(|root| {
                    root.sheet
                        .sections_named("Geometry")
                        .into_iter()
                        .next()
                        .and_then(|g| g.rows.iter().find(|r| r.ix == ix))
                })
            };
            // An inherited cache (`F="Inh"`/`No Formula`) counts as a
            // literal only when its master source carries no effective
            // formula — the planner's #226 rule (see route_owned_rows)
            // applied to the reconcile walk (review on the #303 batch).
            let lit = |r: &vsdx::model::Row, n: &str| -> Option<f64> {
                let own = r.cell(n);
                let cell = own.or_else(|| master_row(r.ix).and_then(|m| m.cell(n)))?;
                if matches!(cell.f.as_deref(), Some("Inh") | Some("No Formula"))
                    && own.is_some()
                {
                    let derived = master_row(r.ix)
                        .and_then(|m| m.cell(n))
                        .and_then(|c| c.f.as_deref())
                        .is_some_and(|f| f != "Inh" && f != "No Formula");
                    if derived {
                        return None;
                    }
                }
                match cell.f.as_deref() {
                    None | Some("Inh") | Some("No Formula") => cell.numeric(),
                    _ => None,
                }
            };
            let mut arctos = 0usize;
            let mut movetos = 0usize;
            let mut owned = true;
            for r in &live {
                // The EFFECTIVE type (a typeless local row inherits
                // its master row's type — a None-`T` override over a
                // master `CubicBezTo` is a curve, CodeRabbit on the
                // #303 batch).
                let t = r
                    .t
                    .as_deref()
                    .or_else(|| master_row(r.ix).and_then(|m| m.t.as_deref()));
                match t {
                    Some("MoveTo") | Some("RelMoveTo") => {
                        movetos += 1;
                        // Straight rows must read literal coordinates
                        // through the chain — a formula (`X=Width`) is
                        // authored tracking and must survive the
                        // rewrite.
                        if lit(r, "X").is_none() || lit(r, "Y").is_none() {
                            owned = false;
                            break;
                        }
                    }
                    Some("LineTo") | Some("RelLineTo") => {
                        if lit(r, "X").is_none() || lit(r, "Y").is_none() {
                            owned = false;
                            break;
                        }
                    }
                    Some("ArcTo") => {
                        arctos += 1;
                        if lit(r, "A").is_none()
                            || lit(r, "X").is_none()
                            || lit(r, "Y").is_none()
                        {
                            owned = false;
                            break;
                        }
                    }
                    _ => {
                        owned = false;
                        break;
                    }
                }
            }
            if !owned {
                continue;
            }
            // Multi-stroke sections stay verbatim: the walk emits one
            // disjoint segment run per stroke and the rewrite flattens
            // them into a single polyline — welding the strokes with
            // LineTos across gaps the original never drew (review on
            // the #303 batch).
            if movetos > 1 {
                continue;
            }
            // No live master-only rows: they render through
            // inheritance and would survive a row replace untouched.
            // A deleted shadow row (#293) is NOT master-only — the
            // instance owns its master row as a delete, and the walk
            // filters it — so it counts as coverage. Building `have`
            // from live rows alone would strand a once-shortened
            // connector out of eligibility forever (hide → show never
            // restores the saved arcs; review on the #303 batch).
            let have: Vec<i64> = geo.rows.iter().map(|r| r.ix).collect();
            if master_geo_rows.iter().any(|ix| !have.contains(ix)) {
                continue;
            }
            let master_root = chain.get(1).copied();
            let frame = dc::geom::hit_frame(doc, &chain, s, msize);
            // The renderer's own walk, twice: the divergence between
            // the plain and stripped polylines counts STRIPPED arcs —
            // eligible only when every ArcTo strips (an authored arc
            // keeps its geometry verbatim).
            let plain = dc::geom::geometry_hit_segments(s, master_root, &frame, None);
            let stripped =
                dc::geom::geometry_hit_segments(s, master_root, &frame, Some(strip));
            if plain.len() != stripped.len() + 3 * arctos {
                continue;
            }
            // No-op fast path: nothing strips (no stored arcs) and no
            // hops land on this connector — the rows are already the
            // truth, and rewriting them would only churn the file
            // (collapsing a straight route's degenerate elbows).
            let my_hops: Vec<dc::geom::Jump> =
                hops.iter().filter(|j| j.jumper == s.id).copied().collect();
            if plain.len() == stripped.len() && my_hops.is_empty() {
                continue;
            }
            let mut pts: Vec<(f64, f64)> = Vec::with_capacity(stripped.len() + 1);
            if let Some(first) = stripped.first() {
                pts.push(first.0);
            }
            for (_, b) in &stripped {
                pts.push(*b);
            }
            if pts.len() < 2 {
                continue;
            }
            let mut target = reconciled_rows(
                radii,
                shown,
                &frame,
                &pts,
                &my_hops,
                live.first().copied(),
            );
            // (K) A shorter target would UN-cover master rows the
            // instance's overrides used to shadow — the walk would
            // re-add them as master-only rows and resurrect a stale
            // tail (CodeRabbit on the #303 batch). Keep them shadowed
            // with the #293 deleted-marker form.
            let have_t: Vec<i64> = target.iter().map(|r| r.ix).collect();
            for ix in &master_geo_rows {
                if !have_t.contains(ix) {
                    target.push(vsdx::model::Row {
                        t: master_row(*ix).and_then(|m| m.t.clone()),
                        ix: *ix,
                        attrs: vec![("Del".to_string(), "1".to_string())],
                        ..Default::default()
                    });
                }
            }
            // Idempotence: write only on a real change — compare the
            // TARGET against the section's FULL row vector (the
            // #293 deleted shadow rows included), value-wise.
            let all_rows = &geo.rows;
            let same = target.len() == all_rows.len()
                && target.iter().zip(all_rows.iter()).all(|(t, r)| {
                    t.t == r.t
                        && t.is_deleted() == r.is_deleted()
                        && match (lit(r, "X"), lit(r, "Y")) {
                            (Some(x), Some(y)) => {
                                let (tx, ty) = (
                                    t.cell("X").and_then(|c| c.numeric()).unwrap_or(f64::NAN),
                                    t.cell("Y").and_then(|c| c.numeric()).unwrap_or(f64::NAN),
                                );
                                (tx - x).abs() < 1e-6 && (ty - y).abs() < 1e-6
                            }
                            // A deleted shadow row carries no values —
                            // type + Del marker are the comparison.
                            _ => t.is_deleted() && r.is_deleted(),
                        }
                        && match (t.t.as_deref(), r.cell("A").and_then(|c| c.numeric())) {
                            (Some("ArcTo"), Some(a)) => {
                                (t.cell("A").and_then(|c| c.numeric()).unwrap_or(f64::NAN) - a)
                                    .abs()
                                    < 1e-6
                            }
                            (Some("ArcTo"), None) => false,
                            _ => true,
                        }
                });
            if same {
                continue;
            }
            cands.push(Cand {
                id: s.id,
                geo_ix,
                rows: target,
            });
        }
        // Pass 2: one wholesale row replace per changed connector.
        for c in cands {
            let mut cmd = dc_core::SetSectionRows::new(
                Scope::Shape {
                    page: self.page,
                    shape: c.id,
                },
                "Geometry",
                c.geo_ix,
                c.rows,
            );
            cmd.apply(doc)?;
            self.inner.push(Box::new(cmd));
        }
        let mut all = dc_core::TouchList::default();
        for c in &self.inner {
            all.merge(c.touched(doc));
        }
        self.touched_cache = all;
        self.applied = true;
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> anyhow::Result<()> {
        for c in self.inner.iter_mut().rev() {
            c.undo(doc)?;
        }
        self.inner.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dc_core::Editor;

    fn doc_with_shape() -> Editor {
        let mut shape = Shape {
            id: 1,
            type_: Some("Shape".into()),
            ..Shape::default()
        };
        shape.sheet.set_cell("PinX", Cell::value("1.5"));
        shape.sheet.set_cell("PinY", Cell::value("2.5"));
        let page = vsdx::model::Page {
            id: 0,
            name: "P1".into(),
            attrs: vec![("ID".into(), "0".into()), ("NameU".into(), "P1".into())],
            shapes: vec![shape],
            ..vsdx::model::Page::default()
        };
        Editor::new(Document {
            pages: vec![page],
            ..Document::default()
        })
    }

    #[test]
    fn move_shape_undo_restores_exact_cells() {
        let mut ed = doc_with_shape();
        ed.apply(MoveShape::new(0, 1, (4.0, 5.0))).unwrap();
        assert_eq!(pin_of(&ed.doc, 0, 1), Some((4.0, 5.0)));
        ed.undo().unwrap();
        // exact original strings, not reformatted numbers
        let scope = Scope::Shape { page: 0, shape: 1 };
        let sheet = dc_core::scope::sheet(&ed.doc, &scope).unwrap();
        assert_eq!(sheet.cell("PinX").unwrap().v.as_deref(), Some("1.5"));
        assert_eq!(sheet.cell("PinY").unwrap().v.as_deref(), Some("2.5"));
    }

    #[test]
    fn add_master_instance_undo_removes() {
        // AddMasterInstance now lives in dc-core (#84); keep the
        // behavioral pin here through the re-exported command.
        let mut ed = doc_with_shape();
        ed.doc.masters.push(vsdx::Master {
            id: 7,
            name_u: Some("M".into()),
            ..vsdx::Master::default()
        });
        ed.apply(dc_core::AddMasterInstance::new(0, 7, (3.0, 3.0)))
            .unwrap();
        assert!(vsdx::model::find_shape_by_id(&ed.doc.pages[0].shapes, 2).is_some());
        let s = vsdx::model::find_shape_by_id(&ed.doc.pages[0].shapes, 2).unwrap();
        assert_eq!(s.master, Some(7));
        assert_eq!(pin_of(&ed.doc, 0, 2), Some((3.0, 3.0)));
        ed.undo().unwrap();
        assert!(vsdx::model::find_shape_by_id(&ed.doc.pages[0].shapes, 2).is_none());
        // ID is reallocated identically on redo
        ed.redo().unwrap();
        assert!(vsdx::model::find_shape_by_id(&ed.doc.pages[0].shapes, 2).is_some());
    }

    /// #97 §4 relocation: the group-cache-refresh semantics (formerly a
    /// 340-line kittest) tested at the command layer — the app's resize
    /// commit is SetCells for the new dims + group_cache_refresh in one
    /// Macro, and that Macro is applied directly here.
    #[test]
    fn group_cache_refresh_reevaluates_inh_caches_through_user_indirections() {
        use crate::group_fixture::taskg_editor;
        let (mut ed, page, [root, sub, icon]) = taskg_editor();
        let num = |ed: &dc_core::Editor, id: i64, k: &str| {
            vsdx::model::find_shape_by_id(
                &ed.doc.pages.iter().find(|p| p.id == page).unwrap().shapes,
                id,
            )
            .unwrap()
            .sheet
            .cell(k)
            .and_then(|c| c.numeric())
            .unwrap_or(f64::NAN)
        };

        // Resize 1×0.5 → 1.3×0.7 the way the app commits it.
        let (kx, ky) = (1.3f64, 1.4f64);
        let mut cmds: Vec<Box<dyn dc_core::Command>> = vec![
            Box::new(SetCell::new(
                Scope::Shape { page, shape: root },
                "Width",
                Cell::value("1.3"),
            )),
            Box::new(SetCell::new(
                Scope::Shape { page, shape: root },
                "Height",
                Cell::value("0.7"),
            )),
        ];
        cmds.extend(group_cache_refresh(&ed.doc, page, root, kx, ky));
        ed.apply(dc_core::Macro::new("resize shape", cmds)).unwrap();
        let (w1, h1) = (1.3f64, 0.7f64);

        // Sub caches scaled with the group.
        assert!(
            (num(&ed, sub, "Width") - kx).abs() < 1e-3,
            "sub Width: {}",
            num(&ed, sub, "Width")
        );
        assert!(
            (num(&ed, sub, "PinX") - 0.5 * kx).abs() < 1e-3,
            "sub PinX: {}",
            num(&ed, sub, "PinX")
        );
        // Sub geometry row caches scaled too.
        let s = vsdx::model::find_shape_by_id(
            &ed.doc.pages.iter().find(|p| p.id == page).unwrap().shapes,
            sub,
        )
        .unwrap();
        let geo = &s.sheet.sections_named("Geometry")[0];
        let far_x = geo
            .rows
            .iter()
            .find(|r| r.ix == 3)
            .unwrap()
            .cell("X")
            .and_then(|c| c.numeric())
            .unwrap();
        assert!(
            (far_x - 1.0 * kx).abs() < 1e-3,
            "sub geometry X scaled: {far_x}"
        );

        // The Txt block followed, including the cells the User
        // indirection hides: TxtPinX = GUARD(User.TxtPinX) with
        // User.TxtPinX = Width/2 must re-center.
        assert!(
            (num(&ed, root, "TxtWidth") - w1).abs() < 1e-3,
            "TxtWidth: {}",
            num(&ed, root, "TxtWidth")
        );
        assert!(
            (num(&ed, root, "TxtPinX") - w1 / 2.0).abs() < 1e-3,
            "TxtPinX: {}",
            num(&ed, root, "TxtPinX")
        );
        assert!(
            (num(&ed, root, "TxtPinY") - h1 / 2.0).abs() < 1e-3,
            "TxtPinY: {}",
            num(&ed, root, "TxtPinY")
        );
        assert!(
            (num(&ed, root, "TxtLocPinX") - w1 / 2.0).abs() < 1e-3,
            "TxtLocPinX: {}",
            num(&ed, root, "TxtLocPinX")
        );
        // Refreshed cells keep F='Inh' — a later resize must refresh
        // them again, not meet frozen literals.
        let s = vsdx::model::find_shape_by_id(
            &ed.doc.pages.iter().find(|p| p.id == page).unwrap().shapes,
            root,
        )
        .unwrap();
        assert_eq!(s.sheet.cell("TxtPinX").unwrap().f.as_deref(), Some("Inh"));

        // The fixed-size icon kept its GUARD'ed size and X offset, while
        // PinY = GUARD(Sheet.1!User.IconY) with
        // User.IconY = Height - User.InfoPanelHeight/2 re-evaluated at
        // the new height (the instance's cached User.IconY must not
        // freeze it).
        assert!(
            (num(&ed, icon, "Width") - 0.157480).abs() < 1e-6,
            "icon Width: {}",
            num(&ed, icon, "Width")
        );
        assert!(
            (num(&ed, icon, "PinX") - 0.1).abs() < 1e-6,
            "icon PinX: {}",
            num(&ed, icon, "PinX")
        );
        assert!(
            (num(&ed, icon, "PinY") - (h1 - 0.1)).abs() < 1e-3,
            "icon PinY: {}",
            num(&ed, icon, "PinY")
        );

        // A SECOND resize refreshes again (the Inh markers survived).
        let mut cmds: Vec<Box<dyn dc_core::Command>> = vec![Box::new(SetCell::new(
            Scope::Shape { page, shape: root },
            "Width",
            Cell::value("1.56"),
        ))];
        cmds.extend(group_cache_refresh(&ed.doc, page, root, 1.56 / w1, 1.0));
        ed.apply(dc_core::Macro::new("resize shape", cmds)).unwrap();
        assert!(
            (num(&ed, root, "TxtPinX") - 0.78).abs() < 1e-3,
            "second refresh: {}",
            num(&ed, root, "TxtPinX")
        );

        // Each undo restores one resize's whole refresh; two undos
        // return to the authored caches.
        ed.undo().unwrap();
        assert!(
            (num(&ed, root, "TxtPinX") - w1 / 2.0).abs() < 1e-3,
            "undo #1"
        );
        ed.undo().unwrap();
        assert!(
            (num(&ed, sub, "Width") - 1.0).abs() < 1e-6,
            "undo restored sub cache"
        );
        assert!(
            (num(&ed, root, "TxtPinX") - 0.5).abs() < 1e-6,
            "undo restored TxtPinX"
        );
        assert!(
            (num(&ed, icon, "PinY") - 0.4).abs() < 1e-6,
            "undo restored icon PinY"
        );
    }
}

#[cfg(test)]
mod reroute_master_tests {
    use super::*;

    // ---- #174: reroute must move MASTER-based connectors ----------------

    use dc_core::connects::ConnectsHost;
    use vsdx::model::{Cell as VCell, Row, Section, SheetChild};

    fn vc(n: &str, v: &str) -> (String, VCell) {
        (n.to_string(), VCell::value(v.to_string()))
    }

    /// A document like the owner's connector-test.vsdx: a rectangle,
    /// a MASTER-based Dynamic connector (ObjType=2 lives on the
    /// master root; the instance carries only placement cells + local
    /// geometry rows), and a Connect gluing the connector's EndX to
    /// the rectangle (static, ToPart 100+row on Connection row 0).
    fn doc_with_master_connector(to_part: i64, arc: bool) -> (dc_core::Editor, i64) {
        let mut ed = dc_core::Editor::new_blank();
        let page_id = ed.doc.pages[0].id;
        // The target rectangle at (2..4, 5..6) with connection row 0
        // at its top-left (2, 6).
        let mut target = Shape {
            id: 10,
            type_: Some("Shape".into()),
            ..Shape::default()
        };
        for (n, v) in [
            ("PinX", "3.0"),
            ("PinY", "5.5"),
            ("Width", "2.0"),
            ("Height", "1.0"),
            ("LocPinX", "1.0"),
            ("LocPinY", "0.5"),
        ] {
            target.sheet.set_cell(n, VCell::value(v));
        }
        // Connection row 0 at the rect's left edge mid-height (local
        // (0, 0.5) → page (2, 5.5)): the static-glue anchor.
        let mut connections = Section {
            name: "Connection".into(),
            ix: 0,
            ..Section::default()
        };
        connections.rows.push(Row {
            ix: 0,
            cells: vec![vc("X", "0"), vc("Y", "0.5")],
            ..Row::default()
        });
        target.sheet.children.push(SheetChild::Section(connections));
        let mut conn_rows = vec![Row {
            t: Some("MoveTo".into()),
            ix: 1,
            cells: vec![vc("X", "0"), vc("Y", "0")],
            ..Row::default()
        }];
        if arc {
            conn_rows.push(Row {
                t: Some("EllipticalArcTo".into()),
                ix: 2,
                cells: vec![
                    vc("X", "1.0"),
                    vc("Y", "0.1"),
                    vc("A", "0.5"),
                    vc("B", "0.35"),
                ],
                ..Row::default()
            });
        } else {
            conn_rows.push(Row {
                t: Some("LineTo".into()),
                ix: 2,
                cells: vec![vc("X", "1.0"), vc("Y", "0.0")],
                ..Row::default()
            });
            conn_rows.push(Row {
                t: Some("LineTo".into()),
                ix: 3,
                cells: vec![vc("X", "1.0"), vc("Y", "0.0")],
                ..Row::default()
            });
            conn_rows.push(Row {
                t: Some("LineTo".into()),
                ix: 4,
                cells: vec![vc("X", "1.0"), vc("Y", "0.0")],
                ..Row::default()
            });
        }
        // The connector: master carries ObjType=2 + GUARDed derived
        // cells (the corpus pattern the old checks missed).
        let mut root = Shape {
            id: 1,
            type_: Some("Shape".into()),
            ..Shape::default()
        };
        root.sheet.set_cell("ObjType", VCell::value("2"));
        // The corpus dynamic-connector master pattern (#175): every
        // XForm cell derives from the endpoints through GUARDs.
        for (n, f) in [
            ("PinX", "GUARD((BeginX+EndX)/2)"),
            ("PinY", "GUARD((BeginY+EndY)/2)"),
            ("Width", "GUARD(EndX-BeginX)"),
            ("Height", "GUARD(EndY-BeginY)"),
            ("LocPinX", "GUARD(Width*0.5)"),
            ("LocPinY", "GUARD(Height*0.5)"),
        ] {
            root.sheet.set_cell(
                n,
                VCell {
                    f: Some(f.into()),
                    ..VCell::default()
                },
            );
        }
        let mut m = vsdx::Master::default();
        m.shapes.push(root);
        ed.doc.masters.push(m);
        let mut conn = Shape {
            id: 11,
            type_: Some("Shape".into()),
            master: Some(0),
            ..Shape::default()
        };
        for (n, v) in [
            ("BeginX", "1.0"),
            ("BeginY", "6.0"),
            ("EndX", "2.0"),
            ("EndY", "6.0"),
            ("PinX", "1.5"),
            ("PinY", "6.0"),
            ("Width", "1.0"),
            ("LocPinX", "0.5"),
            ("LocPinY", "0.0"),
        ] {
            conn.sheet.set_cell(n, VCell::value(v));
        }
        // A fixed-height band master (the #175 file's convention): the
        // instance overrides Height with a CONSTANT guard.
        conn.sheet.set_cell(
            "Height",
            VCell {
                v: Some("0.2".into()),
                f: Some("GUARD(0.2DL)".into()),
                ..VCell::default()
            },
        );
        let mut geo = Section {
            name: "Geometry".into(),
            ix: 0,
            ..Section::default()
        };
        geo.rows = conn_rows;
        conn.sheet.children.push(SheetChild::Section(geo));
        ed.doc.pages[0].shapes.push(target);
        ed.doc.pages[0].shapes.push(conn);
        let mut list = ed.doc.pages[0].connects();
        list.list.push(dc_core::connects::Connect::new(
            11,
            "EndX",
            12,
            10,
            "Connections.X1",
            to_part,
        ));
        ed.doc.pages[0].set_connects(&list);
        (ed, page_id)
    }

    /// #174: a master-based connector's glued endpoint follows its
    /// target after RerouteConnectors — the local-only ObjType check
    /// used to skip it entirely.
    #[test]
    fn reroute_moves_master_based_connector_static_glue() {
        let (mut ed, page_id) = doc_with_master_connector(100, false);
        // Move the target right by 1in (PinX 3.0 → 4.0).
        for sh in ed.doc.pages[0].shapes.iter_mut() {
            if sh.id == 10 {
                sh.sheet.set_cell("PinX", VCell::value("4.0"));
            }
        }
        let mut cmd = RerouteConnectors::new(page_id, vec![10]);
        cmd.apply(&mut ed.doc).unwrap();
        let conn = ed.doc.pages[0].shapes.iter().find(|s| s.id == 11).unwrap();
        let ex = conn.sheet.cell("EndX").and_then(|c| c.numeric()).unwrap();
        assert!(
            (ex - 3.0).abs() < 1e-6,
            "static glue follows the connection point: EndX {ex} (want 3.0)"
        );
        cmd.undo(&mut ed.doc).unwrap();
        let ex = ed.doc.pages[0]
            .shapes
            .iter()
            .find(|s| s.id == 11)
            .unwrap()
            .sheet
            .cell("EndX")
            .and_then(|c| c.numeric())
            .unwrap();
        assert!((ex - 2.0).abs() < 1e-6, "undo restores EndX to 2.0: {ex}");
    }

    /// Dynamic glue (ToPart 3): the endpoint walks to the target's
    /// facing edge after the move.
    #[test]
    fn reroute_moves_master_based_connector_dynamic_glue() {
        let (mut ed, page_id) = doc_with_master_connector(3, false);
        for sh in ed.doc.pages[0].shapes.iter_mut() {
            if sh.id == 10 {
                sh.sheet.set_cell("PinX", VCell::value("4.0"));
            }
        }
        let mut cmd = RerouteConnectors::new(page_id, vec![10]);
        cmd.apply(&mut ed.doc).unwrap();
        let conn = ed.doc.pages[0].shapes.iter().find(|s| s.id == 11).unwrap();
        let ex = conn.sheet.cell("EndX").and_then(|c| c.numeric()).unwrap();
        assert!(
            (ex - 3.0).abs() < 1e-6,
            "dynamic glue walks to the facing (left) edge: EndX {ex} (want 3.0)"
        );
    }

    /// #293: a connector whose inherited-deleted row 3 (`Del="1"`,
    /// no cells — the MisterDiagram/Visio pattern) reroutes THROUGH
    /// the row: the route's endpoint lands on the last structural row
    /// and the write UN-DELETES it (the Expected rule), so the saved
    /// file renders the same in Visio instead of dropping the row and
    /// losing the connector's tail.
    #[test]
    fn reroute_undeletes_a_reused_deleted_row() {
        let (mut ed, page_id) = doc_with_master_connector(3, false);
        // Shrink to the #293 shape: 3 rows, row 3 deleted (no cells).
        for sh in ed.doc.pages[0].shapes.iter_mut() {
            if sh.id == 11 {
                for sec in sh.sheet.children.iter_mut() {
                    if let SheetChild::Section(geo) = sec {
                        geo.rows.retain(|r| r.ix <= 3);
                        if let Some(r3) = geo.rows.iter_mut().find(|r| r.ix == 3) {
                            r3.attrs = vec![("Del".to_string(), "1".to_string())];
                            r3.cells.clear();
                        }
                    }
                }
            }
        }
        for sh in ed.doc.pages[0].shapes.iter_mut() {
            if sh.id == 10 {
                sh.sheet.set_cell("PinX", VCell::value("4.0"));
            }
        }
        let mut cmd = RerouteConnectors::new(page_id, vec![10]);
        cmd.apply(&mut ed.doc).unwrap();
        let conn = ed.doc.pages[0].shapes.iter().find(|s| s.id == 11).unwrap();
        let geo = conn.sheet.sections_named("Geometry").remove(0);
        // #303: the route now GROWS the instance to the full Z (Visio
        // writes four rows for this pose class) — the reused deleted
        // row un-deletes as before, and the missing tail row
        // materializes typed.
        assert_eq!(geo.rows.len(), 4, "the route grows the tail row: {rows:?}", rows = geo.rows.len());
        let r3 = geo.rows.iter().find(|r| r.ix == 3).unwrap();
        assert!(
            !r3.is_deleted(),
            "the reroute un-deleted the reused row: attrs {:?}",
            r3.attrs
        );
        assert!(
            r3.cell("Y").and_then(|c| c.numeric()).is_some_and(|y| y.abs() > 1e-6),
            "row 3 carries route values: {:?}",
            r3.cells
        );
        let r4 = geo.rows.iter().find(|r| r.ix == 4).unwrap();
        assert!(
            r4.t.as_deref() == Some("LineTo"),
            "the grown row is a typed LineTo: {:?}",
            r4.t
        );
        // Undo restores the deleted three-row pose exactly.
        cmd.undo(&mut ed.doc).unwrap();
        let conn = ed.doc.pages[0].shapes.iter().find(|s| s.id == 11).unwrap();
        let geo = conn.sheet.sections_named("Geometry").remove(0);
        assert_eq!(geo.rows.len(), 3, "undo restores the three-row pose");
        let r3 = geo.rows.iter().find(|r| r.ix == 3).unwrap();
        assert!(
            r3.is_deleted() && r3.cells.is_empty(),
            "undo restores the Del marker and the empty cells: {:?}",
            (r3.attrs.clone(), r3.cells.clone())
        );
    }

    /// An ARC connector keeps its 2-row geometry — the reroute must
    /// not materialize the tool connector's 4-row route frame onto a
    /// master whose rows are MoveTo + EllipticalArcTo.
    #[test]
    fn reroute_never_grows_an_arc_connectors_rows() {
        let (mut ed, page_id) = doc_with_master_connector(100, true);
        for sh in ed.doc.pages[0].shapes.iter_mut() {
            if sh.id == 10 {
                sh.sheet.set_cell("PinX", VCell::value("4.0"));
            }
        }
        let mut cmd = RerouteConnectors::new(page_id, vec![10]);
        cmd.apply(&mut ed.doc).unwrap();
        let conn = ed.doc.pages[0].shapes.iter().find(|s| s.id == 11).unwrap();
        let geo = conn.sheet.sections_named("Geometry").remove(0);
        assert_eq!(
            geo.rows.len(),
            2,
            "the arc keeps its MoveTo + EllipticalArcTo rows: {:?}",
            geo.rows.iter().map(|r| r.t.clone()).collect::<Vec<_>>()
        );
        assert_eq!(geo.rows[1].t.as_deref(), Some("EllipticalArcTo"));
        // #175: its endpoint row TRANSLATED IN ITS OWN FRAME — X by
        // the endpoint's x delta, Y unchanged (the constant-height
        // band does not track dy).
        let x = geo.rows[1].cell("X").and_then(|c| c.numeric()).unwrap();
        assert!(
            (x - 2.0).abs() < 1e-3,
            "the arc's end row tracks the new endpoint: X {x}"
        );
        let y = geo.rows[1].cell("Y").and_then(|c| c.numeric()).unwrap();
        assert!((y - 0.1).abs() < 1e-6, "the band's row Y is kept: {y}");
        // The guarded caches refreshed through their guards; the
        // constant band guard untouched.
        let (pinx_v, pinx_f) = {
            let c = conn.sheet.cell("PinX").unwrap();
            (c.v.clone(), c.f.clone())
        };
        assert!(
            pinx_v
                .as_deref()
                .is_some_and(|v| (v.parse::<f64>().unwrap() - 2.0).abs() < 1e-6),
            "PinX cache = (B+E)/2 = 2.0: {pinx_v:?}"
        );
        // The guard stays the MASTER's through the instance's Inh
        // marker — the corpus form (#226); the reroute refreshes V
        // only (#218 review round 3).
        assert_eq!(
            pinx_f.as_deref(),
            Some("Inh"),
            "the formula stays the master's through the marker: {pinx_f:?}"
        );
        let h = conn.sheet.cell("Height").unwrap();
        assert!(
            h.numeric().unwrap().abs() - 0.2 < 1e-9,
            "the constant band height is untouched: {:?}",
            h.v
        );
    }

    /// #178: `undo` spends `inner`, but the history asks `touched()`
    /// AFTER undo — the apply-time cache must still name the
    /// connector (Ink), or its layer texture keeps the dragged raster
    /// under the restored bounding box.
    #[test]
    fn reroute_touched_survives_undo() {
        let (mut ed, page_id) = doc_with_master_connector(100, false);
        for sh in ed.doc.pages[0].shapes.iter_mut() {
            if sh.id == 10 {
                sh.sheet.set_cell("PinX", VCell::value("4.0"));
            }
        }
        let mut cmd = RerouteConnectors::new(page_id, vec![10]);
        cmd.apply(&mut ed.doc).unwrap();
        let touched = cmd.touched(&ed.doc);
        assert!(
            touched
                .shapes
                .iter()
                .any(|t| t.shape == 11 && t.touch == dc_core::Touch::Ink),
            "apply-time touches name the connector: {touched:?}"
        );
        cmd.undo(&mut ed.doc).unwrap();
        let touched = cmd.touched(&ed.doc);
        assert!(
            touched
                .shapes
                .iter()
                .any(|t| t.shape == 11 && t.touch == dc_core::Touch::Ink),
            "post-undo touches still name the connector (the cache): {touched:?}"
        );
    }

    /// #179: an endpoint drop plans the connector BY ID — even a
    /// RELEASED (unglued) master-based connector refreshes its
    /// formula-driven frame caches from the endpoint cells, so the
    /// render follows the dropped end instead of keeping the
    /// pre-drag pose.
    #[test]
    fn reroute_including_refreshes_a_released_connector() {
        let (mut ed, page_id) = doc_with_master_connector(100, false);
        // The drop path already wrote the new endpoint values and
        // removed the Connect (released): replay that state.
        {
            let page = &mut ed.doc.pages[0];
            use dc_core::connects::ConnectsHost;
            let mut list = page.connects();
            list.list.clear();
            page.set_connects(&list);
        }
        for sh in ed.doc.pages[0].shapes.iter_mut() {
            if sh.id == 11 {
                sh.sheet.set_cell("EndX", VCell::value("3.0"));
                sh.sheet.set_cell("EndY", VCell::value("5.0"));
            }
        }
        let mut cmd = RerouteConnectors::new_including(page_id, vec![11]);
        cmd.apply(&mut ed.doc).unwrap();
        let conn = ed.doc.pages[0].shapes.iter().find(|s| s.id == 11).unwrap();
        let cell = |n: &str| conn.sheet.cell(n).and_then(|c| c.numeric()).unwrap();
        // The guarded caches refresh through their GUARDs; the band
        // guard is overridden so the frame spans the endpoints
        // (dx = 2, dy = −1 — begin (1, 6), end (3, 5)).
        assert!(
            (cell("PinX") - 2.0).abs() < 1e-6,
            "PinX = (B+E)/2: {}",
            cell("PinX")
        );
        assert!(
            (cell("PinY") - 5.5).abs() < 1e-6,
            "PinY = (B+E)/2: {}",
            cell("PinY")
        );
        assert!(
            (cell("Height") - (-1.0)).abs() < 1e-6,
            "the band guard is overridden to span dy: {}",
            cell("Height")
        );
        let pinx = conn.sheet.cell("PinX").unwrap();
        // The instance cache keeps its marker: the GUARD lives on
        // the master and the corpus instances carry F="Inh" (#226).
        // The reroute refreshes V, never localizes the formula
        // (#218 review round 3).
        assert_eq!(
            pinx.f.as_deref(),
            Some("Inh"),
            "the formula stays the master's through the marker: {:?}",
            pinx.f
        );
        cmd.undo(&mut ed.doc).unwrap();
        let h = ed.doc.pages[0]
            .shapes
            .iter()
            .find(|s| s.id == 11)
            .unwrap()
            .sheet
            .cell("Height")
            .unwrap();
        assert!(
            (h.numeric().unwrap() - 0.2).abs() < 1e-9,
            "undo restores the band: {:?}",
            h.v
        );
    }

    /// #176: a LINE-geometry master connector (the owner's
    /// connector-test dynamic connector) whose target moves
    /// DIAGONALLY must have its whole frame rewritten to span the new
    /// endpoints — the #175 in-frame translation kept the band's
    /// horizontal midline and the ink rendered detached from the
    /// moved shape.
    #[test]
    fn reroute_line_master_spans_the_new_endpoints() {
        let (mut ed, page_id) = doc_with_master_connector(100, false);
        // Move the target DOWN-RIGHT: PinX 3.0 → 3.5, PinY 5.5 → 5.0.
        // The target's local origin is (PinX−LocPinX, PinY−LocPinY) =
        // (2.5, 4.5), so its connection point (local (0, 0.5)) lands
        // at (2.5, 5.0); the begin stays (1.0, 6.0) → dx = 1.5,
        // dy = −1.
        for sh in ed.doc.pages[0].shapes.iter_mut() {
            if sh.id == 10 {
                sh.sheet.set_cell("PinX", VCell::value("3.5"));
                sh.sheet.set_cell("PinY", VCell::value("5.0"));
            }
        }
        let mut cmd = RerouteConnectors::new(page_id, vec![10]);
        cmd.apply(&mut ed.doc).unwrap();
        let conn = ed.doc.pages[0].shapes.iter().find(|s| s.id == 11).unwrap();
        let cell = |n: &str| conn.sheet.cell(n).and_then(|c| c.numeric()).unwrap();
        // The band guard is OVERRIDDEN: Height's cached V spans the
        // endpoints (dy = −1), the GUARD formula is kept for the file.
        let h = conn.sheet.cell("Height").unwrap();
        assert!(
            (h.numeric().unwrap() - (-1.0)).abs() < 1e-6,
            "Height spans dy: {:?}",
            h.v
        );
        // The instance OWNS this Height guard (a local formula, not
        // an Inh cache) — the reroute keeps it verbatim and only
        // refreshes V (#218 review round 3: absent or inherited
        // cells take the marker instead).
        assert!(
            h.f.as_deref().is_some_and(|f| f.contains("GUARD")),
            "the band guard formula is kept: {:?}",
            h.f
        );
        assert!((cell("Width") - 1.5).abs() < 1e-6, "Width spans dx");
        assert!((cell("LocPinX") - 0.75).abs() < 1e-6, "LocPinX = dx/2");
        assert!((cell("LocPinY") - (-0.5)).abs() < 1e-6, "LocPinY = dy/2");
        // The rendered frame origin is the BEGIN point and the last
        // row lands on the END point — the ink attaches to both.
        let ox = cell("PinX") - cell("LocPinX");
        let oy = cell("PinY") - cell("LocPinY");
        assert!((ox - 1.0).abs() < 1e-6, "origin x = begin x: {ox}");
        assert!((oy - 6.0).abs() < 1e-6, "origin y = begin y: {oy}");
        let geo = conn.sheet.sections_named("Geometry").remove(0);
        let mv = geo.rows.iter().find(|r| r.ix == 1).unwrap();
        assert!(
            (mv.cell("X").and_then(|c| c.numeric()).unwrap()).abs() < 1e-6
                && (mv.cell("Y").and_then(|c| c.numeric()).unwrap()).abs() < 1e-6,
            "MoveTo resets to (0,0): the begin-corner frame"
        );
        let last = geo.rows.iter().rfind(|r| r.ix > 1).unwrap();
        let (lx, ly) = (
            last.cell("X").and_then(|c| c.numeric()).unwrap(),
            last.cell("Y").and_then(|c| c.numeric()).unwrap(),
        );
        assert!(
            (lx - 1.5).abs() < 1e-6 && (ly - (-1.0)).abs() < 1e-6,
            "last row = (dx, dy) = the end point: ({lx}, {ly})"
        );
        assert!(
            ((ox + lx) - 2.5).abs() < 1e-6 && ((oy + ly) - 5.0).abs() < 1e-6,
            "origin + last row = the END point on the moved target"
        );
        cmd.undo(&mut ed.doc).unwrap();
        let conn = ed.doc.pages[0].shapes.iter().find(|s| s.id == 11).unwrap();
        // Full restore: the band pose AND its rows come back.
        assert!((conn.sheet.cell("Height").unwrap().numeric().unwrap() - 0.2).abs() < 1e-9);
        let geo = conn.sheet.sections_named("Geometry").remove(0);
        let r2 = geo.rows.iter().find(|r| r.ix == 2).unwrap();
        assert!(
            (r2.cell("X").and_then(|c| c.numeric()).unwrap() - 1.0).abs() < 1e-9
                && r2.cell("Y").and_then(|c| c.numeric()).unwrap().abs() < 1e-9,
            "undo restores the original row snapshot (1.0, 0.0)"
        );
    }

    // ---- #303: a reroute grows the connector's Geometry to the route ----

    /// #303 item 1: a three-row line-geometry instance (the #303
    /// repro's saved C→D: MoveTo + two LineTo) rerouted to a
    /// dy-dominant pose grows to the full four-row Z — the values
    /// Visio 2021 writes for the same move (the issue's measured
    /// rows) — instead of bending the last row into a diagonal.
    #[test]
    fn reroute_grows_a_three_row_connector_to_the_full_z() {
        let (mut ed, page_id) = doc_with_master_connector(100, false);
        // Drop row 4: the three-row instance.
        dc_core::commands::RemoveSectionRow::new(
            dc_core::Scope::Shape {
                page: page_id,
                shape: 11,
            },
            "Geometry",
            0,
            dc_core::commands::RowSel::Ix(4),
        )
        .apply(&mut ed.doc)
        .unwrap();
        // Move the target DOWN-RIGHT so the route's dy dominates: the
        // connection point (PinX−1, PinY) lands at (2.5, 3.5); begin
        // stays (1, 6) → dx = 1.5, dy = −2.5, mid-y = 4.75.
        for sh in ed.doc.pages[0].shapes.iter_mut() {
            if sh.id == 10 {
                sh.sheet.set_cell("PinX", VCell::value("3.5"));
                sh.sheet.set_cell("PinY", VCell::value("3.5"));
            }
        }
        let mut cmd = RerouteConnectors::new(page_id, vec![10]);
        cmd.apply(&mut ed.doc).unwrap();
        let conn = ed.doc.pages[0].shapes.iter().find(|s| s.id == 11).unwrap();
        let geo = conn.sheet.sections_named("Geometry").remove(0);
        assert_eq!(geo.rows.len(), 4, "the route grew the missing row");
        let rows: Vec<(i64, &str, f64, f64)> = geo
            .rows
            .iter()
            .map(|r| {
                (
                    r.ix,
                    r.t.as_deref().unwrap_or(""),
                    r.cell("X").and_then(|c| c.numeric()).unwrap(),
                    r.cell("Y").and_then(|c| c.numeric()).unwrap(),
                )
            })
            .collect();
        // Visio's Z: begin-corner MoveTo, then elbow, elbow, end.
        let want = [
            (1, "MoveTo", 0.0, 0.0),
            (2, "LineTo", 0.0, -1.25),
            (3, "LineTo", 1.5, -1.25),
            (4, "LineTo", 1.5, -2.5),
        ];
        for (got, w) in rows.iter().zip(want.iter()) {
            assert_eq!(got.0, w.0, "row ix");
            assert_eq!(got.1, w.1, "row {} type", w.0);
            assert!(
                (got.2 - w.2).abs() < 1e-6 && (got.3 - w.3).abs() < 1e-6,
                "row {} = ({}, {}), got ({}, {})",
                w.0,
                w.2,
                w.3,
                got.2,
                got.3
            );
        }
        // The appended row carries a TYPE (SetRowCell's chain-seeded
        // default would serialize typeless).
        assert!(
            geo.rows[3].t.as_deref() == Some("LineTo"),
            "the grown row is a typed LineTo"
        );
        // Undo returns the three-row pose (and un-deletes nothing).
        cmd.undo(&mut ed.doc).unwrap();
        let conn = ed.doc.pages[0].shapes.iter().find(|s| s.id == 11).unwrap();
        let geo = conn.sheet.sections_named("Geometry").remove(0);
        assert_eq!(geo.rows.len(), 3, "undo restores the three-row pose");
        assert!(geo.rows.iter().all(|r| r.ix != 4), "row 4 gone");
    }

    /// Review on the #303 batch: the replace cloned whatever IX-1 row
    /// the section carried — a `Del="1"` shadow dragged its marker
    /// into the rebuilt route (deleting the origin), and a non-MoveTo
    /// IX 1 (gate-admissible: route_owned_rows only checks
    /// straight-ness) never positioned the pen. The origin clones only
    /// LIVE IX-1 rows and its type is forced to an absolute MoveTo.
    #[test]
    fn reroute_replace_origin_is_a_live_move_to() {
        for shadow in [true, false] {
            let (mut ed, page_id) = doc_with_master_connector(100, false);
            {
                let conn = ed
                    .doc
                    .pages[0]
                    .shapes
                    .iter_mut()
                    .find(|s| s.id == 11)
                    .unwrap();
                for c in conn.sheet.children.iter_mut() {
                    if let vsdx::model::SheetChild::Section(sec) = c {
                        if sec.name == "Geometry" {
                            for r in sec.rows.iter_mut() {
                                if r.ix == 1 {
                                    if shadow {
                                        r.attrs.push(("Del".into(), "1".into()));
                                    } else {
                                        r.t = Some("LineTo".into());
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // The Z-growth move: the route needs all four rows.
            for sh in ed.doc.pages[0].shapes.iter_mut() {
                if sh.id == 10 {
                    sh.sheet.set_cell("PinX", VCell::value("3.5"));
                    sh.sheet.set_cell("PinY", VCell::value("3.5"));
                }
            }
            RerouteConnectors::new(page_id, vec![10])
                .apply(&mut ed.doc)
                .unwrap();
            let conn = ed.doc.pages[0].shapes.iter().find(|s| s.id == 11).unwrap();
            let geo = conn.sheet.sections_named("Geometry").remove(0);
            assert_eq!(
                geo.rows.len(),
                4,
                "shadow={shadow}: the route grew the missing row"
            );
            let first = &geo.rows[0];
            assert_eq!(first.ix, 1);
            assert!(
                !first.is_deleted(),
                "shadow={shadow}: the origin carries no Del marker: {first:?}"
            );
            assert_eq!(
                first.t.as_deref(),
                Some("MoveTo"),
                "shadow={shadow}: the origin is an absolute MoveTo"
            );
            for (name, want) in [("X", 0.0), ("Y", 0.0)] {
                assert!(
                    (first.cell(name).and_then(|c| c.numeric()).unwrap() - want).abs() < 1e-9,
                    "shadow={shadow}: origin {name} = {want}"
                );
            }
        }
    }

    /// Review on the #303 batch: the wholesale replace dropped every
    /// instance mention of master rows the new route does not cover —
    /// live overrides AND Del markers — so the walk re-appended those
    /// master rows as master-only rows (a resurrected stale tail) and
    /// the connector fell out of the reconcile's coverage gate. The
    /// replace keeps uncovered LIVE master rows suppressed with #293
    /// deleted overrides — the reconcile's (K) block, applied to the
    /// planner. Here the instance GROWS (3 rows → the 4-row Z) and the
    /// master's row 5 still needs its shadow.
    #[test]
    fn reroute_replace_shadows_master_rows_the_route_no_longer_covers() {
        let (mut ed, page_id) = doc_with_master_connector(100, false);
        // The master carries FIVE live geometry rows; the instance
        // covers only the first three. (Find the master BY ID — the
        // blank template's built-in stencil masters precede the
        // fixture's id-0 master in `doc.masters`.)
        {
            let m0 = ed
                .doc
                .masters
                .iter_mut()
                .find(|m| m.id == 0)
                .expect("the fixture master (id 0)");
            let root = m0.shapes.first_mut().unwrap();
            let mut mg = Section {
                name: "Geometry".into(),
                ix: 0,
                ..Section::default()
            };
            for (ix, t, x, y) in [
                (1, "MoveTo", 0.0, 0.0),
                (2, "LineTo", 1.0, 0.0),
                (3, "LineTo", 1.0, 1.0),
                (4, "LineTo", 2.0, 1.0),
                (5, "LineTo", 2.0, 2.0),
            ] {
                mg.rows.push(Row {
                    t: Some(t.into()),
                    ix,
                    cells: vec![vc("X", &format!("{x}")), vc("Y", &format!("{y}"))],
                    ..Row::default()
                });
            }
            root.sheet.children.push(SheetChild::Section(mg));
        }
        {
            let conn = ed
                .doc
                .pages[0]
                .shapes
                .iter_mut()
                .find(|s| s.id == 11)
                .unwrap();
            for c in conn.sheet.children.iter_mut() {
                if let vsdx::model::SheetChild::Section(sec) = c {
                    if sec.name == "Geometry" {
                        sec.rows.truncate(3);
                    }
                }
            }
        }
        // The Z-growth move.
        for sh in ed.doc.pages[0].shapes.iter_mut() {
            if sh.id == 10 {
                sh.sheet.set_cell("PinX", VCell::value("3.5"));
                sh.sheet.set_cell("PinY", VCell::value("3.5"));
            }
        }
        RerouteConnectors::new(page_id, vec![10])
            .apply(&mut ed.doc)
            .unwrap();
        let conn = ed.doc.pages[0].shapes.iter().find(|s| s.id == 11).unwrap();
        let geo = conn.sheet.sections_named("Geometry").remove(0);
        let live: Vec<&Row> = geo.rows.iter().filter(|r| !r.is_deleted()).collect();
        assert_eq!(live.len(), 4, "the four-row Z route: {geo:?}");
        assert_eq!(live[0].t.as_deref(), Some("MoveTo"));
        assert!(
            live.iter().skip(1).all(|r| r.t.as_deref() == Some("LineTo")),
            "the route rows are typed LineTo"
        );
        // Master row 5 keeps its suppression — without the shadow the
        // walk would re-append it as a master-only stale tail.
        let shadows: Vec<&Row> = geo.rows.iter().filter(|r| r.is_deleted()).collect();
        assert_eq!(
            shadows.len(),
            1,
            "exactly the uncovered master row is shadowed: {geo:?}"
        );
        assert_eq!(shadows[0].ix, 5);
        assert_eq!(shadows[0].t.as_deref(), Some("LineTo"));
    }

    /// Review on the #303 batch: `lit` accepted an instance `F="Inh"`
    /// cache as a literal without checking its master source — a
    /// formula-driven master cell (the Line-arc `Width*1` pattern) is
    /// DERIVED tracking, and the wholesale replace would freeze its
    /// stale cached V as a literal. #226's rule (the planner's
    /// inherited_geo_formula) now rides route_owned_rows: derived
    /// caches keep the connector off the replace; caches over LITERAL
    /// master cells still admit it.
    #[test]
    fn reroute_keeps_a_formula_backed_inh_cache_out_of_the_replace() {
        for derived in [true, false] {
            let (mut ed, page_id) = doc_with_master_connector(100, false);
            // Master geometry: straight rows; row 2's X is a formula
            // (derived) or a plain literal (the control).
            {
                let m0 = ed
                    .doc
                    .masters
                    .iter_mut()
                    .find(|m| m.id == 0)
                    .expect("the fixture master (id 0)");
                let root = m0.shapes.first_mut().unwrap();
                let mut mg = Section {
                    name: "Geometry".into(),
                    ix: 0,
                    ..Section::default()
                };
                for (ix, t, x, y) in [
                    (1, "MoveTo", 0.0, 0.0),
                    (2, "LineTo", 1.0, 0.0),
                    (3, "LineTo", 1.0, 1.0),
                    (4, "LineTo", 2.0, 1.0),
                ] {
                    let mut cells = vec![vc("Y", &format!("{y}"))];
                    cells.push((
                        "X".to_string(),
                        if ix == 2 && derived {
                            VCell {
                                v: Some("1.0".into()),
                                f: Some("Width*1".into()),
                                ..VCell::default()
                            }
                        } else {
                            VCell::value(format!("{x}"))
                        },
                    ));
                    mg.rows.push(Row {
                        t: Some(t.into()),
                        ix,
                        cells,
                        ..Row::default()
                    });
                }
                root.sheet.children.push(SheetChild::Section(mg));
            }
            // The instance: three rows, row 2's X an Inh cache.
            {
                let conn = ed
                    .doc
                    .pages[0]
                    .shapes
                    .iter_mut()
                    .find(|s| s.id == 11)
                    .unwrap();
                for c in conn.sheet.children.iter_mut() {
                    if let vsdx::model::SheetChild::Section(sec) = c {
                        if sec.name == "Geometry" {
                            sec.rows.truncate(3);
                            for r in sec.rows.iter_mut() {
                                if r.ix == 2 {
                                    r.cells.retain(|(n, _)| n != "X");
                                    r.cells.push((
                                        "X".to_string(),
                                        VCell {
                                            v: Some("1.0".into()),
                                            f: Some("Inh".into()),
                                            ..VCell::default()
                                        },
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            // The Z-growth move.
            for sh in ed.doc.pages[0].shapes.iter_mut() {
                if sh.id == 10 {
                    sh.sheet.set_cell("PinX", VCell::value("3.5"));
                    sh.sheet.set_cell("PinY", VCell::value("3.5"));
                }
            }
            RerouteConnectors::new(page_id, vec![10])
                .apply(&mut ed.doc)
                .unwrap();
            let conn = ed.doc.pages[0].shapes.iter().find(|s| s.id == 11).unwrap();
            let geo = conn.sheet.sections_named("Geometry").remove(0);
            if derived {
                assert_eq!(
                    geo.rows.len(),
                    3,
                    "derived={derived}: no wholesale replace on formula-backed rows"
                );
                let x2 = geo.rows[1].cell("X").unwrap();
                assert_eq!(
                    x2.f.as_deref(),
                    Some("Inh"),
                    "derived={derived}: the cache is not frozen into a literal"
                );
            } else {
                assert_eq!(
                    geo.rows.len(),
                    4,
                    "derived={derived}: a cache over a LITERAL master cell still grows the Z"
                );
            }
        }
    }

    /// #303's conservative half: a connector whose frame does NOT
    /// span the endpoints (a non-master connector's own constant
    /// `GUARD` Height — the dim write is skipped) keeps the
    /// endpoint-on-last-row clamp; growing rows under a frame that
    /// cannot span would write rows outside the frame.
    #[test]
    fn reroute_keeps_the_endpoint_clamp_for_a_guarded_frame() {
        let (mut ed, page_id) = doc_with_curved_connector();
        // Straighten the pose first is overkill — instead reuse the
        // curved fixture's shapes but rebuild the connector as a
        // straight three-row one with an OWN guarded Height.
        let mut conn = crate::connectors::connector_shape(11, (3.0, 4.0), (7.0, 4.5));
        conn.sheet.set_cell("ShapeRouteStyle", VCell::value("1"));
        conn.sheet.set_cell(
            "Height",
            VCell {
                v: Some("0.3".into()),
                f: Some("GUARD(0.3DL)".into()),
                ..VCell::default()
            },
        );
        {
            let geo = conn
                .sheet
                .children
                .iter_mut()
                .find_map(|c| match c {
                    SheetChild::Section(sec) if sec.name == "Geometry" => Some(sec),
                    _ => None,
                })
                .unwrap();
            geo.rows.truncate(2); // MoveTo + one LineTo — three rows total is
            // impossible on the synthetic builder (4 rows); two rows
            // still exercise the clamp (max ix 2 < 4).
        }
        crate::connectors::bake_end(
            &mut conn,
            crate::connectors::End::Begin,
            (3.0, 4.0),
            &crate::connectors::static_glue_formula(10, 1),
        );
        ed.doc.pages[0].shapes.retain(|s| s.id != 11);
        ed.doc.pages[0].shapes.push(conn);
        let mut list = ed.doc.pages[0].connects();
        list.list.push(dc_core::connects::Connect::new(
            11,
            "BeginX",
            crate::connectors::PART_BEGIN_X,
            10,
            "Connections.X1",
            101,
        ));
        ed.doc.pages[0].set_connects(&list);
        // Move the target down-right so the route wants a Z (both axes
        // differ) but the guarded Height skips the frame write.
        for sh in ed.doc.pages[0].shapes.iter_mut() {
            if sh.id == 10 {
                sh.sheet.set_cell("PinX", VCell::value("3.5"));
                sh.sheet.set_cell("PinY", VCell::value("4.0"));
            }
        }
        let mut cmd = RerouteConnectors::new(page_id, vec![10]);
        cmd.apply(&mut ed.doc).unwrap();
        let conn = ed.doc.pages[0].shapes.iter().find(|s| s.id == 11).unwrap();
        let geo = conn.sheet.sections_named("Geometry").remove(0);
        assert_eq!(geo.rows.len(), 2, "no growth under a non-spanning frame");
        let last = geo.rows.last().unwrap();
        let lx = last.cell("X").and_then(|c| c.numeric()).unwrap();
        let ly = last.cell("Y").and_then(|c| c.numeric()).unwrap();
        // The clamp lands the ENDPOINT on the last row (dx = 4-3 = 1
        // after the move... begin walks to (3.5+0.5-0.5, 4.0) hmm —
        // the connection row (local (1,0.5)) on the moved target lands
        // at (4.5, 4.5)); assert the last row IS the end point, not a
        // Z elbow.
        let bx = conn.sheet.cell("BeginX").and_then(|c| c.numeric()).unwrap();
        let by = conn.sheet.cell("BeginY").and_then(|c| c.numeric()).unwrap();
        let ex = conn.sheet.cell("EndX").and_then(|c| c.numeric()).unwrap();
        let ey = conn.sheet.cell("EndY").and_then(|c| c.numeric()).unwrap();
        assert!(
            ((bx + lx) - ex).abs() < 1e-6 && ((by + ly) - ey).abs() < 1e-6,
            "last row = the end point: begin ({bx},{by}) + ({lx},{ly}) vs end ({ex},{ey})"
        );
    }

    // ---- #202: the reroute refreshes a curved route's control caches ----

    /// A synthetic connector (no master) with the #108 curved route:
    /// `ShapeRouteStyle 16`, row 2 a `CubicBezTo` whose A/B/C/D are
    /// the frame formulas with their authoring-time caches, begin
    /// statically glued to shape 10's connection point.
    fn doc_with_curved_connector() -> (dc_core::Editor, i64) {
        let mut ed = dc_core::Editor::new_blank();
        let page_id = ed.doc.pages[0].id;
        let mut target = Shape {
            id: 10,
            type_: Some("Shape".into()),
            ..Shape::default()
        };
        for (n, v) in [
            ("PinX", "2.5"),
            ("PinY", "3.5"),
            ("Width", "1.0"),
            ("Height", "1.0"),
            ("LocPinX", "0.5"),
            ("LocPinY", "0.5"),
        ] {
            target.sheet.set_cell(n, VCell::value(v));
        }
        let mut conn_sec = Section {
            name: "Connection".into(),
            ix: 0,
            ..Section::default()
        };
        conn_sec.rows.push(Row {
            t: None,
            ix: 1,
            cells: vec![
                ("X".to_string(), VCell::value("1.0")),
                ("Y".to_string(), VCell::value("0.5")),
            ],
            ..Row::default()
        });
        target.sheet.children.push(SheetChild::Section(conn_sec));

        // (3,4) → (7,4.5); curved toggle pose: row 2 CubicBezTo with
        // the curved_row_extras formulas and their cached values.
        let mut conn = crate::connectors::connector_shape(11, (3.0, 4.0), (7.0, 4.5));
        conn.sheet.set_cell("ShapeRouteStyle", VCell::value("16"));
        {
            let geo = conn
                .sheet
                .children
                .iter_mut()
                .find_map(|c| match c {
                    SheetChild::Section(sec) if sec.name == "Geometry" => Some(sec),
                    _ => None,
                })
                .unwrap();
            geo.rows[1].t = Some("CubicBezTo".into());
            let extras = [
                ("A", "Width*0.5", 2.0),
                ("B", "Height*0", 0.0),
                ("C", "Width*0.5", 2.0),
                ("D", "Height*1", 0.5),
            ];
            for (name, f, v) in extras {
                geo.rows[1].cells.push((
                    name.to_string(),
                    VCell {
                        v: Some(format!("{v}")),
                        f: Some(f.to_string()),
                        ..VCell::default()
                    },
                ));
            }
        }
        crate::connectors::bake_end(
            &mut conn,
            crate::connectors::End::Begin,
            (3.0, 4.0),
            &crate::connectors::static_glue_formula(10, 1),
        );
        ed.doc.pages[0].shapes.push(target);
        ed.doc.pages[0].shapes.push(conn);
        let mut list = ed.doc.pages[0].connects();
        list.list.push(dc_core::connects::Connect::new(
            11,
            "BeginX",
            crate::connectors::PART_BEGIN_X,
            10,
            "Connections.X1",
            101,
        ));
        ed.doc.pages[0].set_connects(&list);
        (ed, page_id)
    }

    /// #202: the reroute refreshes the Bézier control CACHES against
    /// the new frame (A/C = new dx/2, D = new dy); a customized
    /// literal control keeps its value.
    #[test]
    fn reroute_refreshes_curved_control_caches_and_keeps_literals() {
        let (mut ed, page_id) = doc_with_curved_connector();
        // Move the target right by 1 in: the glued begin walks from
        // (3,4) to (4,4); the frame becomes (4,4)→(7,4.5).
        for sh in ed.doc.pages[0].shapes.iter_mut() {
            if sh.id == 10 {
                sh.sheet.set_cell("PinX", VCell::value("3.5"));
            }
        }
        let mut cmd = RerouteConnectors::new(page_id, vec![10]);
        cmd.apply(&mut ed.doc).unwrap();
        let conn = ed.doc.pages[0]
            .shapes
            .iter()
            .find(|s| s.id == 11)
            .unwrap()
            .clone();
        let cell = |n: &str| {
            conn.sheet.sections_named("Geometry")[0]
                .rows
                .iter()
                .find(|r| r.ix == 2)
                .unwrap()
                .cell(n)
                .and_then(|c| c.numeric())
                .unwrap()
        };
        // w_new = 7−4 = 3 → A/C = 1.5; the anchor's y is 3.5 (the
        // connection point), so h_new = 4.5−3.5 = 1 → D = 1 (was 0.5
        // at the baked pose — a genuine re-track, not a no-op).
        assert!(
            (cell("A") - 1.5).abs() < 1e-6,
            "A cache re-tracks: {}",
            cell("A")
        );
        assert!(
            (cell("C") - 1.5).abs() < 1e-6,
            "C cache re-tracks: {}",
            cell("C")
        );
        assert!(
            (cell("D") - 1.0).abs() < 1e-6,
            "D cache re-tracks: {}",
            cell("D")
        );
        // The formulas ride along (SetGluedCell pattern).
        assert_eq!(
            conn.sheet.sections_named("Geometry")[0].rows[1]
                .cell("A")
                .and_then(|c| c.f.as_deref()),
            Some("Width*0.5")
        );

        // A CUSTOMIZED control (literal, formula replaced by the #202
        // handle drag) keeps its value across reroutes.
        let mut ed2 = ed;
        for sh in ed2.doc.pages[0].shapes.iter_mut() {
            if sh.id == 11 {
                let geo = sh
                    .sheet
                    .children
                    .iter_mut()
                    .find_map(|c| match c {
                        SheetChild::Section(sec) if sec.name == "Geometry" => Some(sec),
                        _ => None,
                    })
                    .unwrap();
                let r2 = geo.rows.iter_mut().find(|r| r.ix == 2).unwrap();
                r2.cells.retain(|(n, _)| n != "A");
                r2.cells.push(("A".to_string(), VCell::value("2.25")));
            }
        }
        for sh in ed2.doc.pages[0].shapes.iter_mut() {
            if sh.id == 10 {
                sh.sheet.set_cell("PinX", VCell::value("4.5"));
            }
        }
        let mut cmd2 = RerouteConnectors::new(page_id, vec![10]);
        cmd2.apply(&mut ed2.doc).unwrap();
        let conn2 = ed2.doc.pages[0].shapes.iter().find(|s| s.id == 11).unwrap();
        let a2 = conn2.sheet.sections_named("Geometry")[0]
            .rows
            .iter()
            .find(|r| r.ix == 2)
            .unwrap()
            .cell("A")
            .and_then(|c| c.numeric())
            .unwrap();
        assert!(
            (a2 - 2.25).abs() < 1e-6,
            "the customized literal stays: {a2}"
        );
    }

    // ---- #203: a SPLINE-authored connector reroutes without flattening ----

    /// A master whose root is a spline connector (ObjType 2 on the
    /// master, SplineStart+2 SplineKnot rows) — the corpus
    /// connector-carried-spline shape.
    fn doc_with_spline_connector() -> (dc_core::Editor, i64) {
        let mut ed = dc_core::Editor::new_blank();
        let page_id = ed.doc.pages[0].id;
        let mut target = Shape {
            id: 10,
            type_: Some("Shape".into()),
            ..Shape::default()
        };
        for (n, v) in [
            ("PinX", "2.5"),
            ("PinY", "3.5"),
            ("Width", "1.0"),
            ("Height", "1.0"),
            ("LocPinX", "0.5"),
            ("LocPinY", "0.5"),
        ] {
            target.sheet.set_cell(n, VCell::value(v));
        }
        // The connector master root: ObjType 2 + spline rows.
        let mut root = Shape {
            id: 1,
            type_: Some("Shape".into()),
            ..Shape::default()
        };
        root.sheet.set_cell("ObjType", VCell::value("2"));
        let spline_row = |ix: i64, t: &str, x: &str, y: &str| Row {
            t: Some(t.into()),
            ix,
            cells: vec![
                ("X".to_string(), VCell::value(x)),
                ("Y".to_string(), VCell::value(y)),
            ],
            ..Row::default()
        };
        let mut sec = Section {
            name: "Geometry".into(),
            ix: 0,
            ..Section::default()
        };
        sec.rows = vec![
            spline_row(1, "MoveTo", "0", "0"),
            spline_row(2, "SplineStart", "0.5", "0.3"),
            spline_row(3, "SplineKnot", "0.8", "0.6"),
            spline_row(4, "SplineKnot", "1.0", "1.0"),
        ];
        // A/B/C on SplineStart + A on SplineKnot (the knot values).
        sec.rows[1]
            .cells
            .push(("A".to_string(), VCell::value("0.3")));
        sec.rows[1].cells.push(("B".to_string(), VCell::value("0")));
        sec.rows[1].cells.push(("C".to_string(), VCell::value("1")));
        sec.rows[1].cells.push(("D".to_string(), VCell::value("3")));
        sec.rows[2]
            .cells
            .push(("A".to_string(), VCell::value("0.6")));
        root.sheet.children.push(SheetChild::Section(sec));
        let mut m = vsdx::Master::default();
        m.shapes.push(root);
        ed.doc.masters.push(m);

        // The instance: 1D frame over the master, master_shape 1, with
        // its own spline-row SNAPSHOT (Visio saves the routed arc/curve
        // pair on the instance — the corpus pattern from utilisation
        // master5 instances).
        let mut conn = Shape {
            id: 11,
            type_: Some("Shape".into()),
            master: Some(0),
            master_shape: Some(1),
            ..Shape::default()
        };
        for (n, v) in [
            ("BeginX", "1.0"),
            ("BeginY", "1.0"),
            ("EndX", "2.0"),
            ("EndY", "2.0"),
            ("PinX", "1.5"),
            ("PinY", "1.5"),
            ("LocPinX", "0.5"),
            ("LocPinY", "0.5"),
        ] {
            conn.sheet.set_cell(n, VCell::value(v));
        }
        // The corpus tracking guards: Width/Height derive from the
        // endpoints (the #175 translate gate fires on them).
        conn.sheet.set_cell(
            "Width",
            VCell {
                v: Some("1.0".into()),
                f: Some("GUARD(Abs(EndX-BeginX))".into()),
                ..VCell::default()
            },
        );
        conn.sheet.set_cell(
            "Height",
            VCell {
                v: Some("1.0".into()),
                f: Some("GUARD(Abs(EndY-BeginY))".into()),
                ..VCell::default()
            },
        );
        let mut isec = Section {
            name: "Geometry".into(),
            ix: 0,
            ..Section::default()
        };
        let irow = |ix: i64, t: &str, x: &str, y: &str| Row {
            t: Some(t.into()),
            ix,
            cells: vec![
                ("X".to_string(), VCell::value(x)),
                ("Y".to_string(), VCell::value(y)),
            ],
            ..Row::default()
        };
        isec.rows = vec![
            irow(1, "MoveTo", "0", "0"),
            irow(2, "SplineStart", "0.5", "0.3"),
            irow(3, "SplineKnot", "0.8", "0.6"),
            irow(4, "SplineKnot", "1.0", "1.0"),
        ];
        isec.rows[1]
            .cells
            .push(("A".to_string(), VCell::value("0.3")));
        isec.rows[1]
            .cells
            .push(("B".to_string(), VCell::value("0")));
        isec.rows[1]
            .cells
            .push(("C".to_string(), VCell::value("1")));
        isec.rows[1]
            .cells
            .push(("D".to_string(), VCell::value("3")));
        isec.rows[2]
            .cells
            .push(("A".to_string(), VCell::value("0.6")));
        conn.sheet
            .children
            .push(SheetChild::Section(std::mem::take(&mut isec)));
        ed.doc.pages[0].shapes.push(target);
        ed.doc.pages[0].shapes.push(conn);
        (ed, page_id)
    }

    /// #203: a spline connector's rows survive a reroute — the types
    /// stay SplineStart/SplineKnot (never flattened to lines) and the
    /// LITERAL cells translate by the endpoint delta (#175), so the
    /// curve re-tracks with the moved endpoint.
    #[test]
    fn reroute_keeps_spline_connector_rows() {
        let (mut ed, page_id) = doc_with_spline_connector();
        // The END is dynamically glued to the target (ToPart 3 — the
        // walkglue record); moving the TARGET drives the reroute the
        // way the canvas does (the corpus flow).
        {
            use dc_core::connects::ConnectsHost;
            let mut list = ed.doc.pages[0].connects();
            list.list.push(dc_core::connects::Connect::new(
                11,
                "EndX",
                crate::connectors::PART_END_X,
                10,
                "PinX",
                crate::connectors::PART_WALKGLUE,
            ));
            ed.doc.pages[0].set_connects(&list);
        }
        for sh in ed.doc.pages[0].shapes.iter_mut() {
            if sh.id == 10 {
                sh.sheet.set_cell("PinX", VCell::value("3.5"));
                sh.sheet.set_cell("PinY", VCell::value("4.5"));
            }
        }
        let mut cmd = RerouteConnectors::new(page_id, vec![10]);
        cmd.apply(&mut ed.doc).unwrap();
        let conn = ed.doc.pages[0].shapes.iter().find(|s| s.id == 11).unwrap();
        let geo = conn
            .sheet
            .sections_named("Geometry")
            .first()
            .map(|g| g.rows.clone())
            .unwrap_or_default();
        if geo.is_empty() {
            // Instance carries no own rows — the master's spline rows
            // render through inheritance; nothing to flatten.
            return;
        }
        let types: Vec<&str> = geo.iter().map(|r| r.t.as_deref().unwrap_or("")).collect();
        assert!(
            types.contains(&"SplineStart") && types.contains(&"SplineKnot"),
            "the spline rows survive the reroute: {types:?}"
        );
        // The LITERAL end knot translates by the endpoint delta
        // (+1, +1): master (1.0, 1.0) → instance cached (2.0, 2.0).
        let last = geo.iter().rfind(|r| r.t.as_deref() == Some("SplineKnot"));
        let Some(last) = last else {
            panic!("a SplineKnot row exists");
        };
        let (lx, ly) = (
            last.cell("X").and_then(|c| c.numeric()).unwrap_or(-9.0),
            last.cell("Y").and_then(|c| c.numeric()).unwrap_or(-9.0),
        );
        // The END knot is the frame's (W, H) corner: it MUST have
        // translated with the walked endpoint (not stay at the
        // authored (1, 1)).
        let ex_new = conn
            .sheet
            .cell("EndX")
            .and_then(|c| c.numeric())
            .unwrap_or(0.0);
        let ey_new = conn
            .sheet
            .cell("EndY")
            .and_then(|c| c.numeric())
            .unwrap_or(0.0);
        assert!(
            ((lx - (ex_new - 2.0 + 1.0)).abs() < 1e-6 && (ly - (ey_new - 2.0 + 1.0)).abs() < 1e-6),
            "the literal spline end translated with the endpoint: ({lx}, {ly})"
        );
    }

    /// #236: the pre-apply `touched()` names a NESTED candidate
    /// connector with its `sub` (apply builds its list only during
    /// apply, so the pre half used to be empty and every nested
    /// reroute fell back to a full re-render), and the store pipeline
    /// — harvest → apply → pair — lands a bounded old∪new damage rect
    /// for it.
    #[test]
    fn reroute_pre_apply_touched_harvests_a_nested_connectors_damage() {
        let (mut ed, page_id) = doc_with_master_connector(100, false);
        // Wrap the connector in a group. The group's frame math is
        // irrelevant to the damage pipeline: the pairing resolves
        // whatever ink the document actually carries.
        let page = &mut ed.doc.pages[0];
        let at = page.shapes.iter().position(|s| s.id == 11).unwrap();
        let conn = page.shapes.remove(at);
        let mut group = vsdx::model::Shape {
            id: 20,
            type_: Some("Group".into()),
            ..vsdx::model::Shape::default()
        };
        for (n, v) in [
            ("PinX", "0"),
            ("PinY", "0"),
            ("Width", "5"),
            ("Height", "3"),
            ("LocPinX", "0"),
            ("LocPinY", "0"),
        ] {
            group.sheet.set_cell(n, VCell::value(v));
        }
        group.sub_shapes.push(conn);
        page.shapes.push(group);

        // Pre-apply: the candidate discovery names the nested connector.
        let cmd = RerouteConnectors::new(page_id, vec![10]);
        let pre_touched = cmd.touched(&ed.doc);
        assert!(
            pre_touched
                .shapes
                .iter()
                .any(|t| t.shape == 20 && t.sub == Some(11) && t.touch == dc_core::Touch::Ink),
            "pre-apply touched names the nested connector: {pre_touched:?}"
        );

        // The store pipeline: harvest → apply → pair.
        let mut layers = crate::layers::ShapeLayers::default();
        layers.set_page(page_id);
        let pre = layers.harvest_pre_damage(&ed.doc, &pre_touched);
        assert!(
            pre.iter().any(|(p, top, s, b)| {
                *p == page_id
                    && *top == 20
                    && *s == 11
                    && matches!(b, crate::layers::SubBound::Bounded(_))
            }),
            "the harvest pins the connector's pre bounds: {pre:?}"
        );
        let (_, touched) = ed.apply(RerouteConnectors::new(page_id, vec![10])).unwrap();
        assert!(
            touched
                .shapes
                .iter()
                .any(|t| t.shape == 20 && t.sub == Some(11)),
            "post-apply touched names the nested connector: {touched:?}"
        );
        layers.touch_damage(&ed.doc, &touched, &pre, page_id);
        assert!(
            !layers.pending_full_for_test(20),
            "the nested reroute's damage is bounded — no forced full re-render"
        );
        let d = layers.pending_damage_for_test(20).expect("bounded damage");
        assert!(d.2 > 0.0 && d.3 > 0.0, "the damage rect is real: {d:?}");
    }

    // ---- #226: the chord-frame Line-arc corpus master ------------------
    //
    // utilisation.vsdx's "Line-arc connector" master frames its rows on
    // the BEGIN→END chord: `Width=SQRT((EndX-BeginX)^2+(EndY-BeginY)^2)`,
    // `MoveTo(Width*0, Height/2)` + `EllipticalArcTo(Width*1, Height/2,
    // A=Controls.Row_1, B=Controls.Row_1.Y)`, Height a literal 0. The
    // instances carry those row cells as `F="Inh"` caches (Visio's
    // instancing) with NO Y cell — the renderer reads the instance
    // cache as a literal (`row_cell_value`), so a reroute that only
    // translates literal rows left the arc's end at the pre-drag pose
    // while the endpoint handles moved on.

    /// The Line-arc master + one instance authored at (1,6)→(3,6)
    /// (chord 2.0, sagitta 1.0/0.2), no glue records.
    fn doc_with_line_arc_connector() -> (dc_core::Editor, i64) {
        let mut ed = dc_core::Editor::new_blank();
        let page_id = ed.doc.pages[0].id;
        let guarded = |f: &str| VCell {
            f: Some(f.into()),
            ..VCell::default()
        };
        let cached_inh = |v: &str| VCell {
            v: Some(v.into()),
            f: Some("Inh".into()),
            ..VCell::default()
        };
        // Master root: ObjType 2 + the chord-frame XForm formulas; the
        // master's own V values are its canvas pose (authoring chord
        // 2.0, sagitta A=1.0 B=0.2).
        let mut root = Shape {
            id: 1,
            type_: Some("Shape".into()),
            ..Shape::default()
        };
        root.sheet.set_cell("ObjType", VCell::value("2"));
        root.sheet.set_cell("PinX", guarded("(BeginX+EndX)/2"));
        root.sheet.set_cell("PinY", guarded("(BeginY+EndY)/2"));
        root
            .sheet
            .set_cell("Width", guarded("SQRT((EndX-BeginX)^2+(EndY-BeginY)^2)"));
        root.sheet.set_cell("Height", VCell::value("0"));
        root.sheet.set_cell("LocPinX", guarded("Width*0.5"));
        root.sheet.set_cell("LocPinY", guarded("Height*0.5"));
        let mut geo = Section {
            name: "Geometry".into(),
            ix: 0,
            ..Section::default()
        };
        geo.rows.push(Row {
            t: Some("MoveTo".into()),
            ix: 1,
            cells: vec![
                ("X".to_string(), guarded("Width*0")),
                ("Y".to_string(), guarded("Height/2")),
            ],
            ..Row::default()
        });
        geo.rows.push(Row {
            t: Some("EllipticalArcTo".into()),
            ix: 2,
            cells: vec![
                ("X".to_string(), guarded("Width*1")),
                ("Y".to_string(), guarded("Height/2")),
                ("A".to_string(), guarded("Controls.Row_1")),
                ("B".to_string(), guarded("Controls.Row_1.Y")),
            ],
            ..Row::default()
        });
        root.sheet.children.push(SheetChild::Section(geo));
        let mut m = vsdx::Master::default();
        m.shapes.push(root);
        ed.doc.masters.push(m);
        // Instance: `F="Inh"` caches from the authoring pose, a literal
        // endpoint pair, geometry rows WITHOUT their Y cells (the
        // corpus instances drop fully-inherited row cells).
        let mut conn = Shape {
            id: 11,
            type_: Some("Shape".into()),
            master: Some(0),
            ..Shape::default()
        };
        conn.sheet.set_cell("BeginX", VCell::value("1.0"));
        conn.sheet.set_cell("BeginY", VCell::value("6.0"));
        conn.sheet.set_cell("EndX", VCell::value("3.0"));
        conn.sheet.set_cell("EndY", VCell::value("6.0"));
        conn.sheet.set_cell("PinX", cached_inh("2.0"));
        conn.sheet.set_cell("PinY", cached_inh("6.0"));
        conn.sheet.set_cell("Width", cached_inh("2.0"));
        conn.sheet.set_cell("LocPinX", cached_inh("1.0"));
        conn.sheet.set_cell("Angle", cached_inh("0"));
        let mut geo = Section {
            name: "Geometry".into(),
            ix: 0,
            ..Section::default()
        };
        geo.rows.push(Row {
            t: Some("MoveTo".into()),
            ix: 1,
            cells: vec![("X".to_string(), cached_inh("0"))],
            ..Row::default()
        });
        geo.rows.push(Row {
            t: Some("EllipticalArcTo".into()),
            ix: 2,
            cells: vec![
                ("X".to_string(), cached_inh("2.0")),
                ("A".to_string(), cached_inh("1.0")),
                ("B".to_string(), cached_inh("0.2")),
            ],
            ..Row::default()
        });
        conn.sheet.children.push(SheetChild::Section(geo));
        ed.doc.pages[0].shapes.push(conn);
        (ed, page_id)
    }

    fn geo_row_of(ed: &dc_core::Editor, page_id: i64, id: i64, ix: i64) -> Row {
        vsdx::model::find_shape_by_id(
            &ed.doc.pages.iter().find(|p| p.id == page_id).unwrap().shapes,
            id,
        )
        .unwrap()
        .sheet
        .sections_named("Geometry")[0]
        .rows
        .iter()
        .find(|r| r.ix == ix)
        .unwrap()
        .clone()
    }

    /// #218 review: the endpoint-drop path carries its PRE-drag pose.
    /// A released master ARC connector with LITERAL rows — the drop
    /// writes the new endpoint cells BEFORE the reroute applies, so
    /// re-reading the pose from the document collapses the literal
    /// row translation to zero and the curve stops short of the
    /// dropped end.
    #[test]
    fn reroute_drop_translates_literal_arc_rows_from_the_pre_pose() {
        let (mut ed, page_id) = doc_with_master_connector(100, true);
        let scope = Scope::Shape { page: page_id, shape: 11 };
        // RELEASED: the drop on empty page removed the glue record —
        // without this the reroute re-resolves the end onto the old
        // target's connection point instead of the dropped pose.
        ed.apply(dc_core::RemoveConnect::new(
            dc_core::ConnectsOwner::Page(page_id),
            0,
        ))
        .unwrap();
        // A freshly dropped instance's frame cells are Inh caches of
        // the master's formulas (#248) — the pose the drop starts
        // from. The fixture's literals become those caches.
        for (n, v) in [
            ("PinX", "1.5"),
            ("PinY", "6.0"),
            ("Width", "1.0"),
            ("LocPinX", "0.5"),
            ("LocPinY", "0.0"),
        ] {
            ed.apply(SetCell::new(
                scope.clone(),
                n,
                VCell {
                    v: Some(v.into()),
                    f: Some("Inh".into()),
                    ..VCell::default()
                },
            ))
            .unwrap();
        }
        // The GUI's drop: the plain endpoint move lands FIRST (this
        // is what re-reading the pose would see afterwards).
        for (n, v) in [("EndX", "4.5"), ("EndY", "7.0")] {
            ed.apply(SetCell::new(scope.clone(), n, VCell::value(v)))
                .unwrap();
        }
        // Fixture pre pose: begin (1, 6), end (2, 6).
        let pre = [(11i64, ((1.0f64, 6.0f64), (2.0f64, 6.0f64)))]
            .into_iter()
            .collect();
        let mut cmd = RerouteConnectors::new_including_with_pre(page_id, vec![11], pre);
        cmd.apply(&mut ed.doc).unwrap();

        // The begin→end offset grew from (1, 0) to (3.5, 1): the
        // literal end row's X rides the Width-tracked axis (+3.5);
        // Y stays (the instance's Height is a constant guard).
        let end_row = geo_row_of(&ed, page_id, 11, 2);
        assert!(
            (end_row.cell("X").unwrap().numeric().unwrap() - 3.5).abs() < 1e-6,
            "literal row X follows the dropped end: {}",
            end_row.cell("X").unwrap().v.clone().unwrap()
        );
        assert!((end_row.cell("Y").unwrap().numeric().unwrap() - 0.1).abs() < 1e-9);
        // The sagitta controls are untouched.
        assert!((end_row.cell("A").unwrap().numeric().unwrap() - 0.5).abs() < 1e-9);
        // The frame caches track the new chord.
        let conn = vsdx::model::find_shape_by_id(
            &ed.doc.pages.iter().find(|p| p.id == page_id).unwrap().shapes,
            11,
        )
        .unwrap();
        let g = |n: &str| conn.sheet.cell(n).and_then(|c| c.numeric()).unwrap();
        assert!((g("Width") - 3.5).abs() < 1e-6, "Width {}", g("Width"));
        assert!((g("PinX") - 2.75).abs() < 1e-6);
    }

    /// The endpoint-drag commit: an arc connector's END moved to
    /// (3.5, 6.2) — a diagonal drag. The arc's end row rides the new
    /// chord frame (X=Width*1 re-evaluated behind its Inh marker), the
    /// sagitta stays, the inherited master Height stays inherited, and
    /// the frame caches (width/pin/angle) match the endpoints.
    #[test]
    fn reroute_tracks_a_line_arc_connectors_inherited_end_row() {
        let (mut ed, page_id) = doc_with_line_arc_connector();
        let scope = Scope::Shape { page: page_id, shape: 11 };
        for (n, v) in [("EndX", "3.5"), ("EndY", "6.2")] {
            ed.apply(SetCell::new(scope.clone(), n, VCell::value(v)))
                .unwrap();
        }
        let mut cmd = RerouteConnectors::new_including(page_id, vec![11]);
        cmd.apply(&mut ed.doc).unwrap();

        let chord = 2.5f64.hypot(0.2);
        let conn = vsdx::model::find_shape_by_id(
            &ed.doc.pages.iter().find(|p| p.id == page_id).unwrap().shapes,
            11,
        )
        .unwrap();
        // The arc's END row rides the new chord — behind its marker.
        let end_row = geo_row_of(&ed, page_id, 11, 2);
        let x = end_row.cell("X").unwrap();
        assert!(
            (x.numeric().unwrap() - chord).abs() < 1e-6,
            "end row X {}",
            x.v.clone().unwrap()
        );
        assert_eq!(x.f.as_deref(), Some("Inh"), "the Inh marker survives");
        // The sagitta controls stay frame-local (not dragged along).
        assert!((end_row.cell("A").unwrap().numeric().unwrap() - 1.0).abs() < 1e-9);
        assert!((end_row.cell("B").unwrap().numeric().unwrap() - 0.2).abs() < 1e-9);
        // The begin row is Width*0 — already 0, byte-stable.
        let begin_row = geo_row_of(&ed, page_id, 11, 1);
        assert_eq!(begin_row.cell("X").unwrap().v.as_deref(), Some("0"));
        assert_eq!(begin_row.cell("X").unwrap().f.as_deref(), Some("Inh"));
        // No Y materialized on either row (the master merge serves it).
        assert!(begin_row.cell("Y").is_none() && end_row.cell("Y").is_none());
        // The inherited master Height stays inherited.
        assert!(conn.sheet.cell("Height").is_none());
        // Frame caches match the new endpoints.
        let g = |n: &str| conn.sheet.cell(n).and_then(|c| c.numeric()).unwrap();
        assert!((g("Width") - chord).abs() < 1e-6, "Width {}", g("Width"));
        assert!((g("PinX") - 2.25).abs() < 1e-6);
        assert!((g("PinY") - 6.1).abs() < 1e-6);
        assert!((g("LocPinX") - chord / 2.0).abs() < 1e-6);
        // The inherited frame caches keep their MARKER — the value
        // refreshes, the formula stays the master's (#218 review
        // round 2: a local formula copy would override the master).
        for n in ["PinX", "Width", "LocPinX"] {
            assert_eq!(
                conn.sheet.cell(n).unwrap().f.as_deref(),
                Some("Inh"),
                "{n} keeps its Inh marker"
            );
        }
        // The chord frame rotates with the drag, behind its marker.
        let angle = conn.sheet.cell("Angle").unwrap();
        assert!(
            (angle.numeric().unwrap() - 0.2f64.atan2(2.5)).abs() < 1e-6,
            "Angle {}",
            angle.v.clone().unwrap()
        );
        assert_eq!(angle.f.as_deref(), Some("Inh"));
        // Undo restores the pre-drag caches exactly.
        cmd.undo(&mut ed.doc).unwrap();
        let conn = vsdx::model::find_shape_by_id(
            &ed.doc.pages.iter().find(|p| p.id == page_id).unwrap().shapes,
            11,
        )
        .unwrap();
        assert_eq!(
            geo_row_of(&ed, page_id, 11, 2).cell("X").unwrap().v.as_deref(),
            Some("2.0")
        );
        assert_eq!(conn.sheet.cell("Angle").unwrap().v.as_deref(), Some("0"));
    }

    /// The #226 W4 witness shape: a glued END tracks its target. The
    /// begin end is static-glued to a connection point; moving the
    /// target re-resolves the attach, re-derives the chord frame, and
    /// refreshes the arc's inherited end-row cache — while BeginX
    /// carries the glue formula untouched.
    #[test]
    fn reroute_moves_a_glued_line_arc_connector_with_its_target() {
        let (mut ed, page_id) = doc_with_line_arc_connector();
        // Target rect pin (2,6), 2x1, connection row 0 at its left edge
        // mid-height → page (1,6) = the arc's begin.
        let mut target = Shape {
            id: 10,
            type_: Some("Shape".into()),
            ..Shape::default()
        };
        for (n, v) in [
            ("PinX", "2.0"),
            ("PinY", "6.0"),
            ("Width", "2.0"),
            ("Height", "1.0"),
            ("LocPinX", "1.0"),
            ("LocPinY", "0.5"),
        ] {
            target.sheet.set_cell(n, VCell::value(v));
        }
        let mut connections = Section {
            name: "Connection".into(),
            ix: 0,
            ..Section::default()
        };
        connections.rows.push(Row {
            ix: 1,
            cells: vec![vc("X", "0"), vc("Y", "0.5")],
            ..Row::default()
        });
        target.sheet.children.push(SheetChild::Section(connections));
        ed.doc.pages[0].shapes.push(target);
        // The begin end is static-glued (the corpus pattern: the glue
        // formula lives on the instance endpoint cells).
        let glue = crate::connectors::static_glue_formula(10, 1);
        {
            let conn = ed
                .doc
                .pages[0]
                .shapes
                .iter_mut()
                .find(|s| s.id == 11)
                .unwrap();
            for n in ["BeginX", "BeginY"] {
                conn.sheet.set_cell(
                    n,
                    VCell {
                        v: Some(if n == "BeginX" { "1.0" } else { "6.0" }.into()),
                        f: Some(glue.clone()),
                        ..VCell::default()
                    },
                );
            }
        }
        let mut list = ed.doc.pages[0].connects();
        list.list.push(dc_core::connects::Connect::new(
            11,
            "BeginX",
            crate::connectors::PART_BEGIN_X,
            10,
            "Connections.X1",
            101,
        ));
        ed.doc.pages[0].set_connects(&list);
        // Move the target by (-0.3, -0.5): the begin attach walks to
        // (0.7, 5.5); the end stays at (3, 6).
        for sh in ed.doc.pages[0].shapes.iter_mut() {
            if sh.id == 10 {
                sh.sheet.set_cell("PinX", VCell::value("1.7"));
                sh.sheet.set_cell("PinY", VCell::value("5.5"));
            }
        }
        ed.apply(RerouteConnectors::new(page_id, vec![10])).unwrap();

        let chord = 2.3f64.hypot(0.5);
        let conn = vsdx::model::find_shape_by_id(
            &ed.doc.pages.iter().find(|p| p.id == page_id).unwrap().shapes,
            11,
        )
        .unwrap();
        // The glue cache followed its target, formula kept.
        let bx = conn.sheet.cell("BeginX").unwrap();
        assert_eq!(bx.v.as_deref().map(|v| v.parse::<f64>().unwrap()), Some(0.7));
        assert!(
            bx.f.as_deref().is_some_and(|f| f.contains("PAR(PNT")),
            "the static glue formula stays: {:?}",
            bx.f
        );
        // The arc's end row rides the new chord; the sagitta stays.
        let end_row = geo_row_of(&ed, page_id, 11, 2);
        assert!((end_row.cell("X").unwrap().numeric().unwrap() - chord).abs() < 1e-6);
        assert!((end_row.cell("A").unwrap().numeric().unwrap() - 1.0).abs() < 1e-9);
        // The chord frame rotated toward the moved target.
        let angle = conn.sheet.cell("Angle").unwrap();
        assert!((angle.numeric().unwrap() - 0.5f64.atan2(2.3)).abs() < 1e-6);
        // The inherited master Height stays inherited.
        assert!(conn.sheet.cell("Height").is_none());
    }
}

#[cfg(test)]
mod reconcile_jump_tests {
    use super::*;
    use vsdx::model::{Cell as VCell, Row, SheetChild};

    /// The page-default jump radius (No-Style 0.125 × 2/3 / 2).
    const R: f64 = 0.125 * (2.0 / 3.0) / 2.0;

    fn xy(x: f64, y: f64) -> Vec<(String, VCell)> {
        vec![
            ("X".to_string(), VCell::value(format!("{x:.7}"))),
            ("Y".to_string(), VCell::value(format!("{y:.7}"))),
        ]
    }

    fn lineto(ix: i64, x: f64, y: f64) -> Row {
        Row {
            t: Some("LineTo".into()),
            ix,
            cells: xy(x, y),
            ..Row::default()
        }
    }

    fn set_geo_rows(s: &mut vsdx::model::Shape, rows: Vec<Row>) {
        for c in s.sheet.children.iter_mut() {
            if let SheetChild::Section(sec) = c {
                if sec.name == "Geometry" {
                    sec.rows = rows;
                    return;
                }
            }
        }
        panic!("no geometry section");
    }

    /// The #303 miniature: H (1,4)→(5,4) horizontal carrying a stored
    /// jump at PAGE x=2.5 (STALE — authored at an old crossing) and V
    /// (3,2)→(3,6) vertical, the real crossing at page x=3 (H-local
    /// x=2). The more-horizontal H hops.
    fn crossing_doc() -> (dc_core::Editor, i64) {
        let mut ed = dc_core::Editor::new_blank();
        let page_id = ed.doc.pages[0].id;
        let mut h = crate::connectors::connector_shape(5, (1.0, 4.0), (5.0, 4.0));
        set_geo_rows(
            &mut h,
            vec![
                Row {
                    t: Some("MoveTo".into()),
                    ix: 1,
                    cells: xy(0.0, 0.0),
                    ..Row::default()
                },
                lineto(2, 1.5 - R, 0.0),
                Row {
                    t: Some("ArcTo".into()),
                    ix: 3,
                    cells: {
                        let mut c = xy(1.5 + R, 0.0);
                        c.push(("A".to_string(), VCell::value(format!("{}", -R))));
                        c
                    },
                    ..Row::default()
                },
                lineto(4, 4.0, 0.0),
            ],
        );
        let v = crate::connectors::connector_shape(6, (3.0, 2.0), (3.0, 6.0));
        ed.doc.pages[0].shapes.push(h);
        ed.doc.pages[0].shapes.push(v);
        (ed, page_id)
    }

    fn rows_of(ed: &dc_core::Editor, id: i64) -> Vec<Row> {
        let s = vsdx::model::find_shape_by_id(&ed.doc.pages[0].shapes, id).unwrap();
        s.sheet
            .sections_named("Geometry")
            .into_iter()
            .next()
            .unwrap()
            .rows
            .clone()
    }

    fn cell(r: &Row, n: &str) -> f64 {
        r.cell(n).and_then(|c| c.numeric()).unwrap_or(f64::NAN)
    }

    /// #303 item 2: the stale stored arc drops and the hop lands at
    /// the CURRENT crossing — Visio opening the saved file draws what
    /// Tarahi's overlay draws.
    #[test]
    fn reconcile_rewrites_a_stale_jump_to_the_current_crossing() {
        let (mut ed, page_id) = crossing_doc();
        let mut cmd = ReconcileLineJumps::new(page_id);
        cmd.apply(&mut ed.doc).unwrap();
        let rows = rows_of(&ed, 5);
        assert_eq!(rows.len(), 4, "one hop row set: {rows:?}");
        assert_eq!(rows[0].t.as_deref(), Some("MoveTo"));
        assert_eq!(rows[1].t.as_deref(), Some("LineTo"));
        assert!((cell(&rows[1], "X") - (2.0 - R)).abs() < 1e-5);
        assert_eq!(rows[2].t.as_deref(), Some("ArcTo"));
        assert!((cell(&rows[2], "X") - (2.0 + R)).abs() < 1e-5);
        // The corpus form: A = −radius.
        assert!((cell(&rows[2], "A") - (-R)).abs() < 1e-5);
        assert_eq!(rows[3].t.as_deref(), Some("LineTo"));
        assert!((cell(&rows[3], "X") - 4.0).abs() < 1e-6);
        // The UNDER connector is a no-op (nothing strips, no hops).
        let v = rows_of(&ed, 6);
        assert_eq!(v.len(), 4, "V keeps its authored rows");
        // Undo restores the stale arc verbatim.
        cmd.undo(&mut ed.doc).unwrap();
        let rows = rows_of(&ed, 5);
        assert_eq!(rows[2].t.as_deref(), Some("ArcTo"));
        assert!((cell(&rows[2], "X") - (1.5 + R)).abs() < 1e-6);
        assert!((cell(&rows[2], "A") - (-R)).abs() < 1e-6);
    }

    /// No crossing any more: the stale arc straightens away — nothing
    /// bumps where nothing crosses (#294's render rule, now in the
    /// file), and the collinear stubs collapse.
    #[test]
    fn reconcile_straightens_a_stale_jump_with_no_crossing() {
        let (mut ed, page_id) = crossing_doc();
        // Remove V — nothing crosses H.
        ed.doc.pages[0].shapes.retain(|s| s.id != 6);
        ReconcileLineJumps::new(page_id)
            .apply(&mut ed.doc)
            .unwrap();
        let rows = rows_of(&ed, 5);
        assert!(
            rows.iter().all(|r| r.t.as_deref() != Some("ArcTo")),
            "the stale arc is gone: {rows:?}"
        );
        assert_eq!(rows.len(), 2, "the straight skeleton collapsed: {rows:?}");
        assert!((cell(&rows[1], "X") - 4.0).abs() < 1e-6);
    }

    /// #301: a jump-size edit normalizes — stored arcs stay recognized
    /// at the PRE-edit radius and rewrite at the NEW one, so the rows
    /// and the overlay agree after the change.
    #[test]
    fn reconcile_tracks_a_jump_factor_edit() {
        let (mut ed, page_id) = crossing_doc();
        // Factor 2/3 → 2: the radius doubles to 0.125.
        SetCellNew::apply_factor(&mut ed, page_id, 2.0);
        ReconcileLineJumps::with_strip(page_id, (R, R))
            .apply(&mut ed.doc)
            .unwrap();
        let rows = rows_of(&ed, 5);
        assert_eq!(rows[2].t.as_deref(), Some("ArcTo"));
        let big = 0.125 * 2.0 / 2.0;
        assert!((cell(&rows[1], "X") - (2.0 - big)).abs() < 1e-5);
        assert!((cell(&rows[2], "X") - (2.0 + big)).abs() < 1e-5);
        assert!((cell(&rows[2], "A") - (-big)).abs() < 1e-5);
    }

    /// #301's zero case: factor 0 draws no hops — recognized at the
    /// pre-edit radius, the stored arc straightens away instead of
    /// bumping.
    #[test]
    fn reconcile_factor_zero_straightens_stored_jumps() {
        let (mut ed, page_id) = crossing_doc();
        SetCellNew::apply_factor(&mut ed, page_id, 0.0);
        ReconcileLineJumps::with_strip(page_id, (R, R))
            .apply(&mut ed.doc)
            .unwrap();
        let rows = rows_of(&ed, 5);
        assert!(
            rows.iter().all(|r| r.t.as_deref() != Some("ArcTo")),
            "no bump at factor 0: {rows:?}"
        );
        assert_eq!(rows.len(), 2);
        assert!((cell(&rows[1], "X") - 4.0).abs() < 1e-6);
    }

    /// Hidden jumps (LineJumpCode 0) straighten the stored arcs — the
    /// render already draws them straight (#294); the file agrees.
    #[test]
    fn reconcile_hides_jumps_straightens_stored_arcs() {
        let (mut ed, page_id) = crossing_doc();
        SetCell::new(
            Scope::PageSheet { page: page_id },
            "LineJumpCode",
            VCell::value("0"),
        )
        .apply(&mut ed.doc)
        .unwrap();
        ReconcileLineJumps::new(page_id).apply(&mut ed.doc).unwrap();
        let rows = rows_of(&ed, 5);
        assert!(
            rows.iter().all(|r| r.t.as_deref() != Some("ArcTo")),
            "hidden jumps carry no arcs: {rows:?}"
        );
    }

    /// An AUTHORED semicircle (not at the page radius) is preserved
    /// verbatim — the radius evidence is what separates stored jumps
    /// from authored curves (#294's constraint).
    #[test]
    fn reconcile_leaves_an_authored_semicircle_alone() {
        let (mut ed, page_id) = crossing_doc();
        // Grow the arc into an authored 1-inch semicircle mid-run.
        let h = ed
            .doc
            .pages[0]
            .shapes
            .iter_mut()
            .find(|s| s.id == 5)
            .unwrap();
        set_geo_rows(
            h,
            vec![
                Row {
                    t: Some("MoveTo".into()),
                    ix: 1,
                    cells: xy(0.0, 0.0),
                    ..Row::default()
                },
                lineto(2, 1.5, 0.0),
                Row {
                    t: Some("ArcTo".into()),
                    ix: 3,
                    cells: {
                        let mut c = xy(2.5, 0.0);
                        c.push(("A".to_string(), VCell::value("-0.5")));
                        c
                    },
                    ..Row::default()
                },
                lineto(4, 4.0, 0.0),
            ],
        );
        ReconcileLineJumps::new(page_id).apply(&mut ed.doc).unwrap();
        let rows = rows_of(&ed, 5);
        assert_eq!(rows.len(), 4, "the authored rows are untouched");
        assert_eq!(rows[2].t.as_deref(), Some("ArcTo"));
        assert!((cell(&rows[2], "A") - (-0.5)).abs() < 1e-9);
        assert!((cell(&rows[2], "X") - 2.5).abs() < 1e-9);
    }

    /// The reroute tail carries the reconcile: a reroute with no
    /// glued candidates still reconciles the page, and ONE undo
    /// restores everything.
    #[test]
    fn reroute_tail_reconciles_in_the_same_undo_step() {
        let (mut ed, page_id) = crossing_doc();
        let mut cmd = RerouteConnectors::new(page_id, vec![]);
        cmd.apply(&mut ed.doc).unwrap();
        let rows = rows_of(&ed, 5);
        assert_eq!(rows[2].t.as_deref(), Some("ArcTo"));
        assert!((cell(&rows[2], "X") - (2.0 + R)).abs() < 1e-5, "hop at the crossing");
        cmd.undo(&mut ed.doc).unwrap();
        let rows = rows_of(&ed, 5);
        assert!((cell(&rows[2], "X") - (1.5 + R)).abs() < 1e-6, "undo restores the stale arc");
    }

    /// Second application is a no-op — the rewritten rows are the
    /// fixed point (idempotence keeps later edits' history clean).
    #[test]
    fn reconcile_is_idempotent() {
        let (mut ed, page_id) = crossing_doc();
        ReconcileLineJumps::new(page_id).apply(&mut ed.doc).unwrap();
        let first = rows_of(&ed, 5);
        ReconcileLineJumps::new(page_id).apply(&mut ed.doc).unwrap();
        let second = rows_of(&ed, 5);
        assert_eq!(first.len(), second.len());
        for (a, b) in first.iter().zip(second.iter()) {
            assert_eq!(a.t, b.t);
            assert!((cell(a, "X") - cell(b, "X")).abs() < 1e-9);
            assert!((cell(a, "Y") - cell(b, "Y")).abs() < 1e-9);
        }
    }

    /// CodeRabbit on the #303 batch: two crossings closer than 2r on
    /// one run must not emit overlapping arcs (the second would start
    /// before the first ends — a backwards segment that the next
    /// reconciliation would reject as authored geometry).
    #[test]
    fn reconcile_skips_an_overlapping_second_hop() {
        let (mut ed, page_id) = crossing_doc();
        // A second vertical 0.04in from V (2r = 0.0833): both cross H.
        let v2 = crate::connectors::connector_shape(7, (3.04, 2.0), (3.04, 6.0));
        ed.doc.pages[0].shapes.push(v2);
        ReconcileLineJumps::new(page_id)
            .apply(&mut ed.doc)
            .unwrap();
        let rows = rows_of(&ed, 5);
        let arcs: Vec<&Row> = rows
            .iter()
            .filter(|r| r.t.as_deref() == Some("ArcTo"))
            .collect();
        assert_eq!(arcs.len(), 1, "one arc, the second hop is a plain crossing");
        // The row X values stay monotonic along the run.
        let xs: Vec<f64> = rows.iter().map(|r| cell(r, "X")).collect();
        assert!(
            xs.windows(2).all(|w| w[1] >= w[0] - 1e-9),
            "no backwards segment: {xs:?}"
        );
    }

    /// CodeRabbit on the #303 batch: straightening a MASTER-based
    /// connector whose route shrinks must keep the master's rows
    /// shadowed — deleted-marker overrides, or the walk re-adds them
    /// as master-only rows and resurrects a stale tail.
    #[test]
    fn reconcile_shadows_master_rows_the_target_no_longer_covers() {
        let mut ed = dc_core::Editor::new_blank();
        let page_id = ed.doc.pages[0].id;
        // The kreuzung-shaped master: ObjType=2 + three geometry rows.
        let mut root = vsdx::model::Shape {
            id: 1,
            type_: Some("Shape".into()),
            ..vsdx::model::Shape::default()
        };
        root.sheet.set_cell("ObjType", VCell::value("2"));
        let mut mg = vsdx::model::Section {
            name: "Geometry".into(),
            ix: 0,
            ..Default::default()
        };
        for (ix, t, x, y) in [(1, "MoveTo", 0.0, 0.0), (2, "LineTo", 0.0, -1.0), (3, "LineTo", 1.0, -1.0)] {
            mg.rows.push(Row {
                t: Some(t.into()),
                ix,
                cells: xy(x, y),
                ..Row::default()
            });
        }
        root.sheet.children.push(SheetChild::Section(mg));
        let mut m = vsdx::Master::default();
        m.shapes.push(root);
        ed.doc.masters.push(m);
        // The instance: a stored jump on a straight run (local
        // 1.5±R), four rows covering the master's three.
        let mut h = crate::connectors::connector_shape(5, (1.0, 4.0), (4.0, 4.0));
        h.master = Some(0);
        set_geo_rows(
            &mut h,
            vec![
                Row {
                    t: Some("MoveTo".into()),
                    ix: 1,
                    cells: xy(0.0, 0.0),
                    ..Row::default()
                },
                lineto(2, 1.5 - R, 0.0),
                Row {
                    t: Some("ArcTo".into()),
                    ix: 3,
                    cells: {
                        let mut c = xy(1.5 + R, 0.0);
                        c.push(("A".to_string(), VCell::value(format!("{}", -R))));
                        c
                    },
                    ..Row::default()
                },
                lineto(4, 3.0, 0.0),
            ],
        );
        ed.doc.pages[0].shapes.push(h);
        // No crossing on the page — the stale arc straightens away and
        // the route collapses to two rows; master row 3 must be
        // shadowed, not resurrected.
        ReconcileLineJumps::new(page_id)
            .apply(&mut ed.doc)
            .unwrap();
        let rows = rows_of(&ed, 5);
        assert_eq!(rows.len(), 3, "two route rows + one shadow: {rows:?}");
        assert!((cell(&rows[1], "X") - 3.0).abs() < 1e-6);
        let shadow = &rows[2];
        assert_eq!(shadow.ix, 3);
        assert!(shadow.is_deleted(), "the master row stays shadowed");
        assert!(
            rows.iter().take(2).all(|r| r.t.as_deref() != Some("ArcTo")),
            "the stale arc is gone"
        );
    }

    /// Review on the #303 batch: a deleted shadow is coverage, not a
    /// master-only row — a master-based connector that once shrank
    /// (hidden jumps) carries `Del="1"` shadows, and the eligibility
    /// gate must keep admitting it or the NEXT pass can never restore
    /// the saved arcs: hide → show left the file without the hop the
    /// overlay draws. The full cycle on the kreuzung shape.
    #[test]
    fn reconcile_hide_show_cycle_restores_arcs_on_a_master_connector() {
        let mut ed = dc_core::Editor::new_blank();
        let page_id = ed.doc.pages[0].id;
        // The kreuzung-shaped master: ObjType=2 + three geometry rows.
        let mut root = vsdx::model::Shape {
            id: 1,
            type_: Some("Shape".into()),
            ..vsdx::model::Shape::default()
        };
        root.sheet.set_cell("ObjType", VCell::value("2"));
        let mut mg = vsdx::model::Section {
            name: "Geometry".into(),
            ix: 0,
            ..Default::default()
        };
        for (ix, t, x, y) in [(1, "MoveTo", 0.0, 0.0), (2, "LineTo", 0.0, -1.0), (3, "LineTo", 1.0, -1.0)] {
            mg.rows.push(Row {
                t: Some(t.into()),
                ix,
                cells: xy(x, y),
                ..Row::default()
            });
        }
        root.sheet.children.push(SheetChild::Section(mg));
        let mut m = vsdx::Master::default();
        m.shapes.push(root);
        ed.doc.masters.push(m);
        // The instance: a stored jump mid-run (local 1.5±R), four rows
        // covering the master's three.
        let mut h = crate::connectors::connector_shape(5, (1.0, 4.0), (4.0, 4.0));
        h.master = Some(0);
        set_geo_rows(
            &mut h,
            vec![
                Row {
                    t: Some("MoveTo".into()),
                    ix: 1,
                    cells: xy(0.0, 0.0),
                    ..Row::default()
                },
                lineto(2, 1.5 - R, 0.0),
                Row {
                    t: Some("ArcTo".into()),
                    ix: 3,
                    cells: {
                        let mut c = xy(1.5 + R, 0.0);
                        c.push(("A".to_string(), VCell::value(format!("{}", -R))));
                        c
                    },
                    ..Row::default()
                },
                lineto(4, 3.0, 0.0),
            ],
        );
        ed.doc.pages[0].shapes.push(h);
        // The real crossing: V at page x=3 — H-local x=2, where the
        // restored hop must land.
        let v = crate::connectors::connector_shape(6, (3.0, 2.0), (3.0, 6.0));
        ed.doc.pages[0].shapes.push(v);
        // HIDE: the stored arc straightens and the shrunken route
        // shadows master row 3 — the state the old gate stranded.
        SetCell::new(
            Scope::PageSheet { page: page_id },
            "LineJumpCode",
            VCell::value("0"),
        )
        .apply(&mut ed.doc)
        .unwrap();
        ReconcileLineJumps::new(page_id)
            .apply(&mut ed.doc)
            .unwrap();
        let rows = rows_of(&ed, 5);
        assert_eq!(rows.len(), 3, "two route rows + one shadow: {rows:?}");
        assert!(rows[2].is_deleted(), "master row 3 stays shadowed");
        // SHOW: still eligible — the shadow counts as coverage — and
        // the hop returns at the CURRENT crossing, the shadow row
        // re-covered by the grown target.
        SetCell::new(
            Scope::PageSheet { page: page_id },
            "LineJumpCode",
            VCell::value("1"),
        )
        .apply(&mut ed.doc)
        .unwrap();
        ReconcileLineJumps::new(page_id)
            .apply(&mut ed.doc)
            .unwrap();
        let rows = rows_of(&ed, 5);
        assert_eq!(rows.len(), 4, "the hop rows are back: {rows:?}");
        assert!(
            rows.iter().all(|r| !r.is_deleted()),
            "the grown target re-covers the master rows: {rows:?}"
        );
        assert_eq!(rows[2].t.as_deref(), Some("ArcTo"));
        assert!((cell(&rows[1], "X") - (2.0 - R)).abs() < 1e-5);
        assert!((cell(&rows[2], "X") - (2.0 + R)).abs() < 1e-5);
        assert!((cell(&rows[2], "A") - (-R)).abs() < 1e-5);
        assert_eq!(rows[3].t.as_deref(), Some("LineTo"));
        assert!((cell(&rows[3], "X") - 3.0).abs() < 1e-6);
    }

    /// Review on the #303 batch: the renderer's walk reads at most
    /// MAX_WALK_ROWS rows, and the reconcile replaces the section from
    /// that walk — a connector longer than the budget would have every
    /// row past the cap ERASED from the file. Such connectors stay
    /// verbatim (the crossing sits well inside the cap, so the buggy
    /// path would rewrite — and truncate).
    #[test]
    fn reconcile_leaves_a_connector_beyond_the_walk_row_cap_alone() {
        let (mut ed, page_id) = crossing_doc();
        // H grows past the walk's row budget with literal straight rows
        // (collinear — the row COUNT is the point, not the shape).
        let n = dc::geom::MAX_WALK_ROWS + 6;
        let mut grown = vec![Row {
            t: Some("MoveTo".into()),
            ix: 1,
            cells: xy(0.0, 0.0),
            ..Row::default()
        }];
        for i in 2..=n {
            let x = (i - 1) as f64 * 4.0 / (n - 1) as f64;
            grown.push(lineto(i as i64, x, 0.0));
        }
        let h = ed
            .doc
            .pages[0]
            .shapes
            .iter_mut()
            .find(|s| s.id == 5)
            .unwrap();
        set_geo_rows(h, grown);
        ReconcileLineJumps::new(page_id)
            .apply(&mut ed.doc)
            .unwrap();
        let rows = rows_of(&ed, 5);
        assert_eq!(
            rows.len(),
            n,
            "the tail beyond the walk cap survives: {} rows",
            rows.len()
        );
        assert!(
            (cell(&rows[n - 1], "X") - 4.0).abs() < 1e-6,
            "the terminal row is intact: {rows:?}"
        );
        assert!(
            rows.iter().all(|r| r.t.as_deref() != Some("ArcTo")),
            "the connector was left verbatim: {rows:?}"
        );
    }

    /// Review on the #303 batch: the walk emits one disjoint segment
    /// run per stroke (a MoveTo positions the pen and draws nothing),
    /// and the reconcile flattens them into a single polyline — a
    /// multi-stroke connector would come back WELDED, with LineTos
    /// across the gaps the original never drew. Multi-stroke sections
    /// stay verbatim (the crossing sits on the second stroke, so the
    /// buggy path would rewrite — and weld).
    #[test]
    fn reconcile_leaves_a_multi_stroke_connector_alone() {
        let (mut ed, page_id) = crossing_doc();
        // Two strokes on H: a short one, then a gap, then the long one
        // carrying the crossing (page x=3 — H-local x=2, mid-stroke).
        let h = ed
            .doc
            .pages[0]
            .shapes
            .iter_mut()
            .find(|s| s.id == 5)
            .unwrap();
        set_geo_rows(
            h,
            vec![
                Row {
                    t: Some("MoveTo".into()),
                    ix: 1,
                    cells: xy(0.5, 0.0),
                    ..Row::default()
                },
                lineto(2, 0.8, 0.0),
                Row {
                    t: Some("MoveTo".into()),
                    ix: 3,
                    cells: xy(1.0, 0.0),
                    ..Row::default()
                },
                lineto(4, 4.0, 0.0),
            ],
        );
        ReconcileLineJumps::new(page_id)
            .apply(&mut ed.doc)
            .unwrap();
        let rows = rows_of(&ed, 5);
        assert_eq!(rows.len(), 4, "the section is untouched: {rows:?}");
        assert_eq!(
            rows[0].t.as_deref(),
            Some("MoveTo"),
            "stroke 1 keeps its start"
        );
        assert_eq!(
            rows[2].t.as_deref(),
            Some("MoveTo"),
            "the stroke break survives — no weld across the gap: {rows:?}"
        );
        assert!(
            rows.iter().all(|r| r.t.as_deref() != Some("ArcTo")),
            "nothing was rewritten: {rows:?}"
        );
        assert!((cell(&rows[1], "X") - 0.8).abs() < 1e-9);
        assert!((cell(&rows[3], "X") - 4.0).abs() < 1e-9);
    }

    /// Review on the #303 batch (the reconcile's half of the Inh rule):
    /// an instance `F="Inh"` cache over a FORMULA-driven master cell is
    /// derived tracking — the owned walk must not classify the
    /// connector as route-owned and rewrite it, freezing the stale
    /// cached V as a literal (#226's rule; the coverage shadow keeps
    /// the gate honest so the OWNED walk is what skips).
    #[test]
    fn reconcile_leaves_formula_backed_inh_cache_rows_alone() {
        let mut ed = dc_core::Editor::new_blank();
        let page_id = ed.doc.pages[0].id;
        // The master: three straight rows, row 2's X a formula.
        let mut root = vsdx::model::Shape {
            id: 1,
            type_: Some("Shape".into()),
            ..vsdx::model::Shape::default()
        };
        root.sheet.set_cell("ObjType", VCell::value("2"));
        let mut mg = vsdx::model::Section {
            name: "Geometry".into(),
            ix: 0,
            ..Default::default()
        };
        mg.rows.push(Row {
            t: Some("MoveTo".into()),
            ix: 1,
            cells: xy(0.0, 0.0),
            ..Row::default()
        });
        mg.rows.push(Row {
            t: Some("LineTo".into()),
            ix: 2,
            cells: vec![
                (
                    "X".to_string(),
                    VCell {
                        v: Some("3.0".into()),
                        f: Some("Width*1".into()),
                        ..VCell::default()
                    },
                ),
                ("Y".to_string(), VCell::value("0")),
            ],
            ..Row::default()
        });
        mg.rows.push(Row {
            t: Some("LineTo".into()),
            ix: 3,
            cells: xy(1.0, -1.0),
            ..Row::default()
        });
        root.sheet.children.push(SheetChild::Section(mg));
        let mut m = vsdx::Master::default();
        m.shapes.push(root);
        ed.doc.masters.push(m);
        // The instance: row 2's X is an Inh CACHE over the formula;
        // master row 3 stays mentioned (a shadow) so the coverage gate
        // passes and the owned walk is the only skip.
        let mut h = crate::connectors::connector_shape(5, (1.0, 4.0), (4.0, 4.0));
        h.master = Some(0);
        set_geo_rows(
            &mut h,
            vec![
                Row {
                    t: Some("MoveTo".into()),
                    ix: 1,
                    cells: xy(0.0, 0.0),
                    ..Row::default()
                },
                Row {
                    t: Some("LineTo".into()),
                    ix: 2,
                    cells: vec![
                        (
                            "X".to_string(),
                            VCell {
                                v: Some("3.0".into()),
                                f: Some("Inh".into()),
                                ..VCell::default()
                            },
                        ),
                        ("Y".to_string(), VCell::value("0")),
                    ],
                    ..Row::default()
                },
                Row {
                    t: Some("LineTo".into()),
                    ix: 3,
                    attrs: vec![("Del".to_string(), "1".to_string())],
                    ..Row::default()
                },
            ],
        );
        ed.doc.pages[0].shapes.push(h);
        // The crossing sits on the run (page x=3 — H-local x=2).
        let v = crate::connectors::connector_shape(6, (3.0, 2.0), (3.0, 6.0));
        ed.doc.pages[0].shapes.push(v);
        ReconcileLineJumps::new(page_id)
            .apply(&mut ed.doc)
            .unwrap();
        let rows = rows_of(&ed, 5);
        assert_eq!(rows.len(), 3, "no rewrite: {rows:?}");
        assert!(
            rows.iter().all(|r| r.t.as_deref() != Some("ArcTo")),
            "no hop landed on derived-tracking rows: {rows:?}"
        );
        assert_eq!(
            rows[1].cell("X").unwrap().f.as_deref(),
            Some("Inh"),
            "the cache is not frozen into a literal: {rows:?}"
        );
        assert!(rows[2].is_deleted(), "the coverage shadow stays");
    }

    /// Review on the #303 batch: with jumps HIDDEN, the crossing
    /// discovery fed `my_hops` for crossings nobody draws — the no-op
    /// fast path then skipped nothing, and a connector with NO stored
    /// arcs got rewritten anyway (its degenerate collinear elbows
    /// collapsed). Hidden jumps use an empty hop list: nothing to do
    /// means nothing written; the strip pass (stored arcs) is a
    /// separate gate and keeps working
    /// (reconcile_hides_jumps_straightens_stored_arcs).
    #[test]
    fn reconcile_hidden_jumps_leave_stored_arc_free_connectors_alone() {
        let (mut ed, page_id) = crossing_doc();
        // H with NO stored arc — just degenerate collinear elbows on
        // the run; V still crosses it at H-local x=2.
        let h = ed
            .doc
            .pages[0]
            .shapes
            .iter_mut()
            .find(|s| s.id == 5)
            .unwrap();
        set_geo_rows(
            h,
            vec![
                Row {
                    t: Some("MoveTo".into()),
                    ix: 1,
                    cells: xy(0.0, 0.0),
                    ..Row::default()
                },
                lineto(2, 1.0, 0.0),
                lineto(3, 2.0, 0.0),
                lineto(4, 4.0, 0.0),
            ],
        );
        SetCell::new(
            Scope::PageSheet { page: page_id },
            "LineJumpCode",
            VCell::value("0"),
        )
        .apply(&mut ed.doc)
        .unwrap();
        ReconcileLineJumps::new(page_id)
            .apply(&mut ed.doc)
            .unwrap();
        let rows = rows_of(&ed, 5);
        assert_eq!(
            rows.len(),
            4,
            "no elbow collapse for a jump-less connector under hidden jumps: {rows:?}"
        );
        assert!((cell(&rows[1], "X") - 1.0).abs() < 1e-9);
        assert!((cell(&rows[3], "X") - 4.0).abs() < 1e-9);
    }

    /// CodeRabbit on the #303 batch: the collinear merge keeps a
    /// route REVERSAL (an out-and-back excursion) — collinear points
    /// in OPPOSITE directions are real route features.
    #[test]
    fn reconcile_keeps_a_route_reversal() {
        let (mut ed, page_id) = crossing_doc();
        // H runs out to 4, back to 2, then up — with the stale arc on
        // the FIRST run.
        let h = ed
            .doc
            .pages[0]
            .shapes
            .iter_mut()
            .find(|s| s.id == 5)
            .unwrap();
        set_geo_rows(
            h,
            vec![
                Row {
                    t: Some("MoveTo".into()),
                    ix: 1,
                    cells: xy(0.0, 0.0),
                    ..Row::default()
                },
                lineto(2, 1.5 - R, 0.0),
                Row {
                    t: Some("ArcTo".into()),
                    ix: 3,
                    cells: {
                        let mut c = xy(1.5 + R, 0.0);
                        c.push(("A".to_string(), VCell::value(format!("{}", -R))));
                        c
                    },
                    ..Row::default()
                },
                lineto(4, 4.0, 0.0),
                lineto(5, 2.0, 0.0),
                lineto(6, 2.0, 2.0),
            ],
        );
        // Remove V — no crossings; the straightened route keeps the
        // excursion to (4,0).
        ed.doc.pages[0].shapes.retain(|s| s.id != 6);
        ReconcileLineJumps::new(page_id)
            .apply(&mut ed.doc)
            .unwrap();
        let rows = rows_of(&ed, 5);
        let xs: Vec<(f64, f64)> = rows
            .iter()
            .map(|r| (cell(r, "X"), cell(r, "Y")))
            .collect();
        assert!(
            xs.contains(&(4.0, 0.0)),
            "the reversal apex survives the merge: {xs:?}"
        );
        assert!(
            rows.iter().all(|r| r.t.as_deref() != Some("ArcTo")),
            "the stale arc straightened away"
        );
    }

    /// A tiny helper: the factor edit as the GUI hook issues it.
    struct SetCellNew;
    impl SetCellNew {
        fn apply_factor(ed: &mut dc_core::Editor, page: i64, factor: f64) {
            SetCell::new(
                Scope::PageSheet { page },
                "LineJumpFactorX",
                VCell::value(format!("{factor:.6}")),
            )
            .apply(&mut ed.doc)
            .unwrap();
        }
    }
}

/// A command built from the document as it stands when it first runs
/// (after the steps before it in the same Macro): `build` makes the
/// inner commands once; redo replays them. `touch` names what it may
/// change, for the layer cache.
type BuildFn = Box<dyn FnOnce(&Document) -> Vec<Box<dyn Command>>>;

pub struct Deferred {
    label: String,
    touch: dc_core::TouchList,
    build: Option<BuildFn>,
    inner: Option<dc_core::Macro>,
}

impl Deferred {
    pub fn new(
        label: impl Into<String>,
        touch: dc_core::TouchList,
        build: impl FnOnce(&Document) -> Vec<Box<dyn Command>> + 'static,
    ) -> Self {
        Deferred { label: label.into(), touch, build: Some(Box::new(build)), inner: None }
    }
}

impl Command for Deferred {
    fn label(&self) -> String {
        self.label.clone()
    }
    fn touched(&self, _doc: &Document) -> dc_core::TouchList {
        self.touch.clone()
    }
    fn apply(&mut self, doc: &mut Document) -> anyhow::Result<()> {
        if let Some(b) = self.build.take() {
            self.inner = Some(dc_core::Macro::new(self.label.clone(), b(doc)));
        }
        match &mut self.inner {
            Some(m) => m.apply(doc),
            None => Ok(()),
        }
    }
    fn undo(&mut self, doc: &mut Document) -> anyhow::Result<()> {
        match &mut self.inner {
            Some(m) => m.undo(doc),
            None => Ok(()),
        }
    }
}

/// Visio's drop scale: a master drawn at a different drawing scale
/// than the page is resized to keep its size on paper when the two
/// scales differ by more than a factor of 8 (an ISR icon drawn at 1:10
/// lands 1.27 in wide on a 1:1 page, not 12.7). Returns the factor to
/// apply to the instance, 1 when none.
pub fn drop_scale(master: &vsdx::model::Master, page: &vsdx::model::Page) -> f64 {
    let ratio = |s: Option<&vsdx::model::Sheet>| {
        let n = |c: &str| s.and_then(|s| s.cell(c)).and_then(|c| c.numeric()).filter(|v| *v > 0.0);
        n("DrawingScale").unwrap_or(1.0) / n("PageScale").unwrap_or(1.0)
    };
    let (m, p) = (ratio(master.sheet()), ratio(page.sheet()));
    let r = m / p;
    if r > 8.0 || r < 1.0 / 8.0 {
        1.0 / r
    } else {
        1.0
    }
}

/// The commands that scale a just-placed 2-D instance by `k` about its
/// pin (Width, Height, LocPins, and the group's member caches).
pub fn scale_instance_cmds(doc: &Document, page: i64, id: i64, k: f64) -> Vec<Box<dyn Command>> {
    let Some(p) = doc.pages.iter().find(|p| p.id == page) else { return vec![] };
    let Some(s) = p.shapes.iter().find(|s| s.id == id) else { return vec![] };
    let Ok(chain) = dc_core::scope::shape_chain(doc, page, id) else { return vec![] };
    let get = |n: &str| doc.resolve_cell(s, &chain, n).and_then(|c| c.numeric());
    if s.sheet.cell("BeginX").is_some() {
        return vec![]; // 1-D: the drop already spans one inch
    }
    let (Some(w), Some(h)) = (get("Width"), get("Height")) else { return vec![] };
    let scope = Scope::Shape { page, shape: id };
    // a formula-driven cell (GUARD(Width*…) aspect locks, Width*0.5 loc
    // pins) gets Visio's instance cache: F='Inh' with the new value,
    // the master's formula kept; a literal gets the scaled literal
    let cell = |n: &str, x: f64| {
        let formula = doc.resolve_cell(s, &chain, n).and_then(|c| c.f.clone()).is_some_and(|f| f != "Inh" && f != "No Formula");
        Cell { v: Some(format!("{x:.10}")), f: formula.then(|| "Inh".to_string()), ..Cell::default() }
    };
    let mut cmds: Vec<Box<dyn Command>> = vec![
        Box::new(SetCell::new(scope.clone(), "Width", cell("Width", w * k))),
        Box::new(SetCell::new(scope.clone(), "Height", cell("Height", h * k))),
        Box::new(SetCell::new(scope.clone(), "LocPinX", cell("LocPinX", get("LocPinX").unwrap_or(w / 2.0) * k))),
        Box::new(SetCell::new(scope, "LocPinY", cell("LocPinY", get("LocPinY").unwrap_or(h / 2.0) * k))),
    ];
    cmds.extend(group_cache_refresh(doc, page, id, k, k));
    cmds
}


#[cfg(test)]
mod drop_scale_tests {
    use super::*;

    fn scaled(page_scale: &str, drawing_scale: &str) -> vsdx::model::Sheet {
        let mut s = vsdx::model::Sheet::default();
        s.set_cell("PageScale", Cell::value(page_scale));
        s.set_cell("DrawingScale", Cell::value(drawing_scale));
        s
    }

    /// Visio 2016: an ISR 1100 master drawn at 1:10 (12.7 in wide) drops
    /// 1.27 in wide on a 1:1 page and 12.7 on a 1:10 page; within a
    /// factor of 8 nothing rescales.
    #[test]
    fn drop_scale_follows_visio() {
        let m = vsdx::model::Master {
            children: vec![vsdx::model::IndexChild::Sheet { tag: "PageSheet".into(), sheet: scaled("1", "10") }],
            ..Default::default()
        };
        let page = |ps: &str, ds: &str| vsdx::model::Page {
            children: vec![vsdx::model::IndexChild::Sheet { tag: "PageSheet".into(), sheet: scaled(ps, ds) }],
            ..Default::default()
        };
        assert!((drop_scale(&m, &page("1", "1")) - 0.1).abs() < 1e-12);
        assert_eq!(drop_scale(&m, &page("1", "10")), 1.0);
        assert_eq!(drop_scale(&m, &page("1", "4")), 1.0, "within 8x");
    }
}
