//! Legacy binary Visio reader (#36) over a local corpus. Binary
//! stencils and drawings are vendor content and stay out of the repo,
//! so this runs only when `TARAHI_VSD_CORPUS` names a directory:
//! every Visio 2003–2010 `.vsd/.vss/.vst` under it must convert and
//! parse, and stencils must yield masters.
//!
//!     TARAHI_VSD_CORPUS=~/stencils cargo test -p vsdx --test legacy -- --nocapture

use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ["vsd", "vss", "vst"].iter().any(|x| e.eq_ignore_ascii_case(x)))
        {
            out.push(p);
        }
    }
}

#[test]
fn legacy_corpus_converts() {
    let Some(root) = std::env::var_os("TARAHI_VSD_CORPUS") else {
        eprintln!("TARAHI_VSD_CORPUS not set; skipping");
        return;
    };
    let mut files = vec![];
    walk(Path::new(&root), &mut files);
    files.sort();
    let mut failed = vec![];
    let mut checked = 0;
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        // only Visio 2003–2010 (v11) binaries are in scope
        let Ok(vd) = vsdx::vsd::visio_stream(&bytes) else { continue };
        if vd[0x1A] != 11 {
            continue;
        }
        checked += 1;
        match vsdx::parse::parse(f) {
            Ok(doc) => {
                let stencil = f.extension().is_some_and(|e| e.eq_ignore_ascii_case("vss"));
                if stencil && doc.masters.is_empty() {
                    failed.push(format!("{}: no masters", f.display()));
                }
            }
            Err(e) => failed.push(format!("{}: {e:#}", f.display())),
        }
    }
    eprintln!("checked {checked} legacy files");
    assert!(failed.is_empty(), "{} failed:\n{}", failed.len(), failed.join("\n"));
}
