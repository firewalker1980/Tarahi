//! Dump the stream/chunk tree of a legacy binary Visio file (#36 RE aid).
//! usage: vsddump file.vss [chunks|json]
//!   json: one line per shape chunk: {"m":master slot,"s":shape slot,"t":type,"id":..,"lvl":..,"d":"hex"}
use vsdx::vsd;

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let doc = vsd::visio_stream(&std::fs::read(&args[1])?)?;
    let mode = args.get(2).map(String::as_str).unwrap_or("");
    if mode == "names" {
        // string pool (0x32) slots, and the 0x3f/0x40 name-ref records
        let tr = vsd::stream(&doc, &vsd::trailer(&doc));
        for p in vsd::children(&vsd::trailer(&doc), &tr) {
            let s = vsd::stream(&doc, &p);
            if p.typ == 0x32 {
                for (i, c) in vsd::children(&p, &s).iter().enumerate() {
                    let d = vsd::stream(&doc, c);
                    let d = if c.compressed() { &d[4..] } else { &d[..] };
                    let txt: String = char::decode_utf16(
                        d.get(8..)
                            .unwrap_or(&[])
                            .chunks(2)
                            .map(|b| u16::from_le_bytes([b[0], b.get(1).copied().unwrap_or(0)])),
                    )
                    .map(|r| r.unwrap_or('?'))
                    .collect();
                    println!("pool[{i}] {} | {txt:?}", hex(&d[..8.min(d.len())]));
                }
            }
            if p.typ == 0x1d {
                for (mi, m) in vsd::children(&p, &s).iter().enumerate() {
                    if m.typ == 0x3f || m.typ == 0x44 {
                        let ms = vsd::stream(&doc, m);
                        for (k, e) in vsd::children(m, &ms).iter().enumerate() {
                            let d = vsd::stream(&doc, e);
                            println!("0x{:x}[{mi}][{k}] {}", m.typ, hex(&d[..d.len().min(60)]));
                        }
                    }
                }
            }
        }
        return Ok(());
    }
    if mode == "find" {
        // vsddump file find <utf16 text>: which streams hold it
        let needle: Vec<u8> = args[3]
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        vsd::walk(&doc, |depth, p, s| {
            if let Some(i) = s.windows(needle.len()).position(|w| w == needle) {
                println!(
                    "{depth} ptr 0x{:x} off=0x{:x} fmt=0x{:x} at {i}: {}",
                    p.typ,
                    p.off,
                    p.fmt,
                    hex(&s[i.saturating_sub(24)..(i + 40).min(s.len())])
                );
            }
        });
        return Ok(());
    }
    if mode == "json" {
        let tr = vsd::stream(&doc, &vsd::trailer(&doc));
        // masters (0x1d → 0x1e) and pages (0x27 → 0x15, reported as m = 1000 + slot)
        for p in vsd::children(&vsd::trailer(&doc), &tr)
            .iter()
            .filter(|p| p.typ == 0x1d || p.typ == 0x27)
        {
            let page_base = if p.typ == 0x27 { 1000 } else { 0 };
            let ms = vsd::stream(&doc, p);
            for (mi, m) in vsd::children(p, &ms).iter().enumerate() {
                if m.typ == 0 {
                    continue;
                }
                let mi = mi + page_base;
                for (_t, si, recs) in vsd::sheets(&doc, m) {
                    for r in recs {
                        let (lt, li) = r.list.unwrap_or((0, 0));
                        println!(
                            "{{\"m\":{mi},\"s\":{si},\"st\":{_t},\"t\":{},\"id\":{},\"lt\":{lt},\"li\":{li},\"d\":\"{}\"}}",
                            r.typ, r.id, hex(&r.data)
                        );
                    }
                }
            }
        }
        return Ok(());
    }
    if mode == "find" {
        // vsddump file find <utf16 text>: which streams hold it
        let needle: Vec<u8> = args[3]
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        vsd::walk(&doc, |depth, p, s| {
            if let Some(i) = s.windows(needle.len()).position(|w| w == needle) {
                println!(
                    "{depth} ptr 0x{:x} off=0x{:x} fmt=0x{:x} at {i}: {}",
                    p.typ,
                    p.off,
                    p.fmt,
                    hex(&s[i.saturating_sub(24)..(i + 40).min(s.len())])
                );
            }
        });
        return Ok(());
    }
    if mode == "json" {
        let tr = vsd::stream(&doc, &vsd::trailer(&doc));
        // masters (0x1d → 0x1e) and pages (0x27 → 0x15, reported as m = 1000 + slot)
        for p in vsd::children(&vsd::trailer(&doc), &tr)
            .iter()
            .filter(|p| p.typ == 0x1d || p.typ == 0x27)
        {
            let page_base = if p.typ == 0x27 { 1000 } else { 0 };
            let ms = vsd::stream(&doc, p);
            for (mi, m) in vsd::children(p, &ms).iter().enumerate() {
                if m.typ == 0 {
                    continue;
                }
                let mi = mi + page_base;
                let mstream = vsd::stream(&doc, m);
                for (si, s) in vsd::children(m, &mstream).iter().enumerate() {
                    if !vsd::is_shape_type(s.typ) {
                        continue;
                    }
                    for r in vsd::shape_records(&doc, s) {
                        let (lt, li) = r.list.unwrap_or((0, 0));
                        println!(
                            "{{\"m\":{mi},\"s\":{si},\"st\":{},\"t\":{},\"id\":{},\"lt\":{lt},\"li\":{li},\"d\":\"{}\"}}",
                            s.typ, r.typ, r.id, hex(&r.data)
                        );
                    }
                }
            }
        }
        return Ok(());
    }
    println!("version {}", doc[0x1A]);
    vsd::walk(&doc, |depth, p, s| {
        let ind = "  ".repeat(depth);
        println!(
            "{ind}ptr 0x{:x} off=0x{:x} len={} fmt=0x{:x} -> {}B",
            p.typ,
            p.off,
            p.len,
            p.fmt,
            s.len()
        );
        if mode == "chunks" && p.has_chunks() {
            for c in vsd::chunks(s) {
                let h: String = c
                    .data
                    .iter()
                    .take(40)
                    .map(|b| format!("{b:02x} "))
                    .collect();
                println!(
                    "{ind}  [{:x}] 0x{:x} id={} list={} lvl={} u=0x{:x} len={} {h}",
                    c.pos,
                    c.typ,
                    c.id,
                    c.list,
                    c.level,
                    c.unk,
                    c.data.len()
                );
            }
        }
    });
    Ok(())
}
