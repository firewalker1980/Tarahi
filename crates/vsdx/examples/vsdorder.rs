//! Print the child order arrays of the page (0x27) and master (0x1d) lists (#36 RE aid).
use vsdx::vsd;
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).unwrap();
    let doc = vsd::visio_stream(&std::fs::read(path)?)?;
    let tr = vsd::trailer(&doc);
    for p in vsd::children(&tr, &vsd::stream(&doc, &tr)) {
        if p.typ == 0x27 || p.typ == 0x1d {
            let (kids, order) = vsd::children_ordered(&p, &vsd::stream(&doc, &p));
            let slots: Vec<String> = kids.iter().enumerate().filter(|(_, k)| k.typ == 0x15 || k.typ == 0x1e).map(|(i, k)| format!("{i}:{:x}", k.typ)).collect();
            println!("list 0x{:x}: slots {slots:?}\n  order {order:?}", p.typ);
            if p.typ == 0x27 {
                for (i, k) in kids.iter().enumerate().filter(|(_, k)| k.typ == 0x15) {
                    let s = vsd::stream(&doc, k);
                    if let Some(c) = vsd::chunks(&s).into_iter().find(|c| c.level == 0) {
                        let h: String = c.data.iter().take(64).map(|b| format!("{b:02x} ")).collect();
                        println!("  page slot {i}: {h}");
                    }
                }
            }
        }
    }
    Ok(())
}
