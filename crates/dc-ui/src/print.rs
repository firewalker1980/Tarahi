//! Printing (File ▸ Print): the drawing goes out as a PDF (the export
//! pipeline) to the system spooler — CUPS `lp` where it exists, else the
//! system PDF viewer, which prints from there.

use std::path::{Path, PathBuf};
use vsdx::model::{Cell, Document, SheetChild};

/// A copy of `doc` for printing: layers whose `Print` cell is 0 are
/// switched off (Visio leaves them off the paper but shows them).
pub fn printable(doc: &Document) -> Document {
    let mut d = doc.clone();
    for p in &mut d.pages {
        let Some(sheet) = p.sheet_mut() else { continue };
        for c in &mut sheet.children {
            if let SheetChild::Section(s) = c {
                if s.name != "Layer" {
                    continue;
                }
                for r in &mut s.rows {
                    if r.cell("Print").and_then(|c| c.numeric()) == Some(0.0) {
                        match r.cells.iter_mut().find(|(n, _)| n == "Visible") {
                            Some((_, c)) => *c = Cell::value("0"),
                            None => r.cells.push(("Visible".into(), Cell::value("0"))),
                        }
                    }
                }
            }
        }
    }
    d
}

/// CUPS printers and the default one (empty when there is no `lpstat`).
pub fn printers() -> (Vec<String>, Option<String>) {
    let run = |arg: &str| {
        std::process::Command::new("lpstat")
            .arg(arg)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    };
    let list: Vec<String> = run("-e").lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
    let default = run("-d").rsplit(':').next().map(str::trim).filter(|d| !d.is_empty()).map(String::from);
    (list, default)
}

/// Write the pages (0-based; empty = all) to a temporary PDF.
pub fn write_pdf(doc: &Document, pages: &[usize]) -> anyhow::Result<PathBuf> {
    let out = std::env::temp_dir().join(format!("tarahi-print-{}.pdf", std::process::id()));
    let opts = dc::export::Options::default();
    dc::export::write_document(&printable(doc), &out, pages, dc::export::Format::Pdf, &opts)?;
    Ok(out)
}

/// Send a PDF to `printer` (`lp -d … -n …`), or — without CUPS — open
/// it in the system viewer. Returns the spooler's message.
pub fn send(pdf: &Path, printer: Option<&str>, copies: u32) -> anyhow::Result<String> {
    if let Some(p) = printer {
        let out = std::process::Command::new("lp")
            .args(["-d", p, "-n", &copies.max(1).to_string(), "-o", "fit-to-page"])
            .arg(pdf)
            .output()?;
        anyhow::ensure!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr).trim());
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(pdf).spawn()?;
    Ok(String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_printing_layers_are_switched_off_for_print() {
        let mut ed = dc_core::Editor::new_blank();
        let page = ed.doc.pages[0].id;
        ed.apply(dc_core::AddLayer::new(page, "notes")).unwrap();
        ed.apply(dc_core::SetRowCell::new(
            dc_core::Scope::PageSheet { page },
            "Layer",
            0,
            dc_core::commands::RowSel::Ix(0),
            "Print",
            Cell::value("0"),
        ))
        .unwrap();
        assert!(dc_core::layers::invisible(&ed.doc, page).is_empty());
        assert_eq!(dc_core::layers::invisible(&printable(&ed.doc), page), vec![0]);
    }
}
