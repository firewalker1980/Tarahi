//! Legacy binary Visio files in the app (#36): a `.vss` lands in the
//! Shapes pane and a `.vsd` opens as a document with its pages, and
//! the shell frames with both. Binary files are vendor/customer
//! content and stay out of the repo, so this runs only when
//! `TARAHI_VSD_CORPUS` names a directory holding at least one of each:
//!
//!     TARAHI_VSD_CORPUS=~/stencils cargo test -p dc-ui --test legacy_open

use dc_ui::app::App;
use egui_kittest::Harness;
use std::path::{Path, PathBuf};

fn find(dir: &Path, ext: &str) -> Option<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&d)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .collect();
        entries.sort();
        for p in entries {
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)) {
                return Some(p);
            }
        }
    }
    None
}

fn harness_with(app: App) -> Harness<'static, App> {
    dc_ui::svgimg::pin_determinism_for_tests();
    Harness::builder()
        .with_size(egui::vec2(1280.0, 800.0))
        .build_ui_state(
            |ui, app: &mut App| {
                dc_ui::theme::install(ui.ctx());
                app.draw(ui);
            },
            app,
        )
}

#[test]
fn legacy_stencil_and_drawing_open_in_the_app() {
    let Some(root) = std::env::var_os("TARAHI_VSD_CORPUS") else {
        eprintln!("TARAHI_VSD_CORPUS not set; skipping");
        return;
    };
    let root = PathBuf::from(root);
    let (Some(vss), Some(vsd)) = (find(&root, "vss"), find(&root, "vsd")) else {
        panic!("TARAHI_VSD_CORPUS needs at least one .vss and one .vsd");
    };

    // A .vsd opens as a document with pages.
    let app = App::with_doc(&vsd);
    assert!(
        app.debug_doc_open(),
        "{}: {}",
        vsd.display(),
        app.debug_status()
    );
    let mut h = harness_with(app);
    h.run();

    // A .vss opened on top lands in the Shapes pane, the editor keeps
    // the drawing, and the shell keeps framing.
    h.state_mut().debug_load_for_test(vss.clone());
    h.run();
    assert!(h.state().debug_doc_open());
    let entries = h.state().debug_stencil_entries();
    let stem = vss.file_stem().unwrap().to_string_lossy().into_owned();
    assert!(
        entries.iter().any(|(name, _)| *name == stem),
        "{stem} not in the Shapes pane: {entries:?} ({})",
        h.state().debug_status()
    );
}

/// Visio's drop scale on a real vendor stencil: the Cisco ISR 1100
/// masters are drawn at 1:10; Visio 2016 drops the first one 1.27 in
/// wide on a 1:1 page (12.7 in on the master). Corpus-gated like the
/// test above; skips when the stencil isn't in it.
#[test]
fn scaled_master_drops_at_page_size() {
    let Some(root) = std::env::var_os("TARAHI_VSD_CORPUS") else { return };
    let Some(vssx) = walk(&PathBuf::from(root)).into_iter().find(|p| p.to_string_lossy().contains("ISR 1100") && p.extension().is_some_and(|e| e == "vssx")) else {
        eprintln!("no ISR 1100 stencil in the corpus; skipping");
        return;
    };
    let mut h = harness_with(App::with_blank());
    h.run();
    h.state_mut().debug_load_for_test(vssx);
    for _ in 0..20 {
        h.run();
    }
    let key = h
        .state()
        .debug_master_cells()
        .into_iter()
        .map(|(k, _)| k)
        .find(|k| matches!(k.0, dc_ui::stencil::StencilId::Open(_)))
        .expect("stencil master in the pane");
    h.state_mut().arm_master_for_test(key);
    let to = h.state().debug_canvas_rect().center();
    h.hover_at(to);
    h.run();
    h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.run();
    h.event(egui::Event::PointerButton { pos: to, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run();
    h.run();
    let (_, sid) = h.state().debug_sel().expect("placed");
    let w: f64 = h.state().debug_cell_for_test(sid, "Width").expect("Width").parse().unwrap();
    assert!((w - 1.27).abs() < 0.01, "width {w}");
    // DROPSCALE_OUT: save the drawing for a visual check (Tarahi / Visio)
    if let Some(out) = std::env::var_os("DROPSCALE_OUT") {
        h.state_mut().preset_save_as_for_test(Some(PathBuf::from(out.clone())));
        h.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::S);
        for _ in 0..30 {
            h.run();
            if Path::new(&out).exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out
}
