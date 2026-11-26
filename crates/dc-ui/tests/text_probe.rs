//! TEXT_PROBE=<drawing>: for every shape with text (groups' members
//! too), double-click its text block centre and report which shape's
//! text the editor opened, and whether the text colour stands out from
//! what is under it. Not a regular test: it prints a report.

use dc_ui::app::App;
use vsdx::model::Shape;

fn lum(c: &str) -> f64 {
    let n = u32::from_str_radix(c.trim_start_matches('#'), 16).unwrap_or(0);
    let (r, g, b) = (((n >> 16) & 255) as f64, ((n >> 8) & 255) as f64, (n & 255) as f64);
    0.299 * r + 0.587 * g + 0.114 * b
}

fn walk<'a>(shapes: &'a [Shape], depth: usize, out: &mut Vec<(&'a Shape, usize)>) {
    for s in shapes {
        out.push((s, depth));
        walk(&s.sub_shapes, depth + 1, out);
    }
}

#[test]
fn text_probe() {
    let Some(path) = std::env::var_os("TEXT_PROBE") else { return };
    let mut app = App::with_doc(std::path::Path::new(&path));
    let pages = app.debug_doc().unwrap().pages.len();
    let (mut ok, mut bad) = (0, 0);
    for pi in 0..pages {
        app.debug_goto_page_idx(pi);
        let doc = app.debug_doc().unwrap().clone();
        let page = &doc.pages[pi];
        if page.attrs.iter().any(|(k, v)| k == "Background" && v == "1") {
            continue;
        }
        let mut all = Vec::new();
        walk(&page.shapes, 0, &mut all);
        for (s, depth) in all {
            let Some(raw) = dc::spellcheck::effective_raw(&doc, page.id, s.id) else { continue };
            let plain: String = vsdx::text::text_pieces(&raw).iter().map(|p| p.plain()).collect();
            if plain.trim().is_empty() || !App::has_text(&doc, page.id, s.id) {
                continue; // no text, or hidden (HideText)
            }
            let Some((x, y, w, h)) = dc::svg::text_block_page_rect(&doc, page, s.id) else {
                println!("NO-BLOCK p{} #{} d{depth} {:?}", page.id, s.id, plain);
                bad += 1;
                continue;
            };
            // any point of the text block (5 x 3 grid) that opens it
            let mut got = None;
            'grid: for fy in [0.5, 0.3, 0.7] {
                for fx in [0.5, 0.3, 0.7, 0.15, 0.85] {
                    app.debug_dbl_click_page((x + w * fx, y + h * fy));
                    if app.debug_text_edit().is_none() {
                        app.debug_f2(); // the double-click did something else (Shape Data)
                    }
                    got = app.debug_text_edit().map(|(_, id)| id);
                    if got == Some(s.id) {
                        break 'grid;
                    }
                }
            }
            if got.is_none() && app.debug_status().contains("locked") {
                // Visio too: the vendor label unlocks from the right-click menu
                if app.debug_run_action(s.id, "Enable Text Label Editing") {
                    println!("  action status: {:?}", app.debug_status());
                    app.debug_f2();
                    got = app.debug_text_edit().map(|(_, id)| id);
                    let d = app.debug_doc().unwrap();
                    let sh = vsdx::model::find_shape_by_id(&d.pages[pi].shapes, s.id).unwrap();
                    let rows: Vec<_> = sh.sheet.sections_named("Actions").iter().flat_map(|sec| sec.rows.iter().map(|r| (r.n.clone(), r.cell("Checked").cloned()))).collect();
                    println!("  #{} unlocked via right-click: editor={got:?} status={:?} actions={rows:?}", s.id, app.debug_status());
                }
            }
            let draft = app.debug_text_edit_draft();
            let tb = dc::svg::text_block(&doc, page, s.id);
            let contrast = tb.as_ref().map(|tb| {
                let under = tb.text_bkgnd.as_ref().map(|(c, _)| c.clone());
                (tb.color.clone(), under)
            });
            let low = contrast.as_ref().is_some_and(|(c, under)| (lum(c) - lum(under.as_deref().unwrap_or("#FFFFFF"))).abs() < 60.0);
            let fine = got == Some(s.id) && draft.as_deref().map(str::trim) == Some(plain.trim());
            if fine && !low {
                ok += 1;
            } else {
                bad += 1;
                println!(
                    "p{} #{} d{depth} {:?}: editor={got:?} draft={:?} color={:?}",
                    page.id,
                    s.id,
                    plain.chars().take(30).collect::<String>(),
                    draft.map(|d| d.chars().take(30).collect::<String>()),
                    contrast
                );
            }
        }
    }
    println!("TEXT PROBE: {ok} ok, {bad} problems");
}

/// TEXT_PROBE_SHOT=<drawing>;<label shape>;<connector>: render the page
/// around a label, with the connector's hit segments drawn in red.
#[test]
fn text_probe_shot() {
    let Some(arg) = std::env::var_os("TEXT_PROBE_SHOT") else { return };
    let arg = arg.to_string_lossy().to_string();
    let v: Vec<&str> = arg.split(';').collect();
    let app = App::with_doc(std::path::Path::new(v[0]));
    let doc = app.debug_doc().unwrap();
    let (label, conn): (i64, i64) = (v[1].parse().unwrap(), v[2].parse().unwrap());
    let page = doc.pages.iter().find(|p| vsdx::model::find_shape_by_id(&p.shapes, label).is_some()).unwrap();
    let (x, y, w, h) = dc::svg::text_block_page_rect(doc, page, label).unwrap();
    println!("label block {x:.3},{y:.3} {w:.3}x{h:.3}");
    let svg = dc::svg::render_page(doc, page).unwrap();
    let ph = page.sheet().and_then(|s| s.cell("PageHeight")).and_then(|c| c.numeric()).unwrap();
    let segs = dc_ui::app::debug_connector_segments(doc, page, conn);
    let mut red = String::new();
    for ((ax, ay), (bx, by)) in &segs {
        red += &format!(r#"<line x1="{ax}" y1="{}" x2="{bx}" y2="{}" stroke="red" stroke-width="0.03"/>"#, ph - ay, ph - by);
    }
    red += &format!(r#"<rect x="{x}" y="{}" width="{w}" height="{h}" fill="none" stroke="blue" stroke-width="0.02"/>"#, ph - y - h);
    let svg = svg.replacen("</svg>", &format!("{red}</svg>"), 1);
    std::fs::write("/tmp/claude-1000/dt/shot.svg", svg).unwrap();
    println!("segments: {segs:?}");
}

/// TEXT_LOOK=<drawing>: per visible text, the pixels of its text block
/// before and while editing, compared; large differences are reported
/// and both crops saved to /tmp/claude-1000/dt/look/.
#[test]
fn text_look() {
    use egui_kittest::Harness;
    let Some(path) = std::env::var_os("TEXT_LOOK") else { return };
    dc_ui::svgimg::pin_determinism_for_tests();
    let app = App::with_doc(std::path::Path::new(&path));
    let mut h = Harness::builder()
        .with_size(egui::vec2(1400.0, 900.0))
        .with_max_steps(40)
        .build_ui_state(
            |ui, app: &mut App| {
                dc_ui::theme::install(ui.ctx());
                app.draw(ui);
            },
            app,
        );
    h.run();
    let out = std::path::Path::new("/tmp/claude-1000/dt/look");
    let _ = std::fs::remove_dir_all(out);
    std::fs::create_dir_all(out).unwrap();
    let doc = h.state().debug_doc().unwrap().clone();
    let only: Option<Vec<i64>> = std::env::var("TEXT_LOOK_ONLY").ok().map(|v| v.split(',').filter_map(|x| x.parse().ok()).collect());
    let (mut ok, mut bad) = (0, 0);
    for (pi, page) in doc.pages.iter().enumerate() {
        if page.attrs.iter().any(|(k, v)| k == "Background" && v == "1") {
            continue;
        }
        h.state_mut().debug_goto_page_idx(pi);
        h.run();
        let mut all = Vec::new();
        walk(&page.shapes, 0, &mut all);
        for (s, _) in all {
            if !App::has_text(&doc, page.id, s.id) || only.as_ref().is_some_and(|o| !o.contains(&s.id)) {
                continue;
            }
            let Some((x, y, w, hh)) = dc::svg::text_block_page_rect(&doc, page, s.id) else { continue };
            h.state_mut().debug_center_on((x + w / 2.0, y + hh / 2.0));
            h.state_mut().debug_select_for_test(s.id);
            for _ in 0..4 { h.run(); }
            h.state_mut().debug_f2();
            for _ in 0..4 { h.run(); }
            if h.state().debug_text_edit().map(|(_, id)| id) != Some(s.id) {
                continue; // locked / not editable: covered by text_probe
            }
            let r = h.state().debug_text_edit_rect().unwrap();
            let after = h.render().unwrap();
            // "before" at the editor's own zoom and pan (the editor zooms
            // in on small text)
            let (z, px, py) = h.state().debug_view_for_test();
            h.state_mut().debug_end_edit();
            h.state_mut().debug_set_view(z, (px, py));
            h.state_mut().debug_select_for_test(s.id);
            for _ in 0..6 { h.run(); }
            let before = h.render().unwrap();
            // compare inside the block (inset 3 px: chrome hairline)
            let (x0, y0) = ((r.min.x + 3.0).max(0.0) as u32, (r.min.y + 3.0).max(0.0) as u32);
            let (x1, y1) = ((r.max.x - 3.0).min(1399.0) as u32, (r.max.y - 3.0).min(899.0) as u32);
            if x1 <= x0 || y1 <= y0 {
                continue;
            }
            let (mut diff, mut n) = (0u64, 0u64);
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let a = before.get_pixel(xx, yy);
                    let b = after.get_pixel(xx, yy);
                    let d: u32 = (0..3).map(|i| (a[i] as i32 - b[i] as i32).unsigned_abs()).sum();
                    if d > 120 {
                        diff += 1;
                    }
                    n += 1;
                }
            }
            let frac = diff as f64 / n.max(1) as f64;
            if frac > 0.06 {
                bad += 1;
                println!("LOOK p{} #{} {:.0}% pixels differ, block {:.0}x{:.0}", page.id, s.id, frac * 100.0, r.width(), r.height());
                let pad = 30;
                let crop = |img: &image::RgbaImage| image::imageops::crop_imm(img, x0.saturating_sub(pad), y0.saturating_sub(pad), (x1 - x0 + 2 * pad).min(1399), (y1 - y0 + 2 * pad).min(899)).to_image();
                crop(&before).save(out.join(format!("{}-before.png", s.id))).unwrap();
                crop(&after).save(out.join(format!("{}-edit.png", s.id))).unwrap();
            } else {
                ok += 1;
            }
        }
    }
    println!("TEXT LOOK: {ok} look the same, {bad} differ");
}

#[test]
fn text_probe_tb() {
    let Some(arg) = std::env::var_os("TEXT_PROBE_TB") else { return };
    let arg = arg.to_string_lossy().to_string();
    let (path, id) = arg.split_once(';').unwrap();
    let app = App::with_doc(std::path::Path::new(path));
    let doc = app.debug_doc().unwrap();
    let id: i64 = id.parse().unwrap();
    let page = doc.pages.iter().find(|p| vsdx::model::find_shape_by_id(&p.shapes, id).is_some()).unwrap();
    let tb = dc::svg::text_block(doc, page, id).unwrap();
    if let Some((per, looks)) = dc::svg::char_looks(doc, page, id) {
        let sh = vsdx::model::find_shape_by_id(&page.shapes, id).unwrap();
        let txt = sh.sheet.text.clone().unwrap_or_default().replace("\r\n", "\n");
        let line: String = txt.chars().zip(per.iter()).map(|(c, p)| format!("{}{}", if c == '\n' { '|' } else if c == '\t' { '>' } else { c }, p)).collect();
        println!("LOOKS {line}\n {looks:?}");
        println!("TEXT {:?}", sh.sheet.text);
    }
    println!("TB halign={} valign={:?} size={} lh={} color={} block={:?}", tb.halign, tb.valign, tb.font_size, tb.line_height, tb.color, dc::svg::text_block_page_rect(doc, page, id));
}

/// TEXT_EDIT_SAVE=<drawing>;<out>: edit the cabling legend (DMZ -> LAN)
/// through the editor and save, for a Visio check of its runs.
#[test]
fn text_edit_save() {
    let Some(arg) = std::env::var_os("TEXT_EDIT_SAVE") else { return };
    let arg = arg.to_string_lossy().to_string();
    let (src, out) = arg.split_once(';').unwrap();
    let mut app = App::with_doc(std::path::Path::new(src));
    app.debug_goto_page_idx(0);
    app.debug_select_for_test(2167);
    app.debug_f2();
    let d = app.debug_text_edit_draft().unwrap();
    app.debug_set_draft_for_test(&d.replace("DMZ", "LAN"));
    app.debug_end_edit_commit();
    app.debug_save_as_for_test(std::path::Path::new(out));
}
