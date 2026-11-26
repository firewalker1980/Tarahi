//! Legacy binary Visio reader (.vsd/.vss/.vst, Visio 2003–2010, #36).
//!
//! Clean-room: the layout below was worked out from sample files and
//! checked against Visio's own .vss → .vssx conversions; no Microsoft
//! code or documentation is used.
//!
//! Container: an OLE compound file whose `VisioDocument` stream starts
//! with the text "Visio (TM) Drawing", the file version at 0x1A and the
//! trailer pointer at 0x24. Every stream is reached through 18-byte
//! pointers (type, address, offset, length, format). Format bit 0x02 =
//! LZSS-compressed, 0x10 = holds a child pointer list, 0x80 = holds
//! chunks (ShapeSheet records).

use anyhow::{bail, Context, Result};
use std::io::Read;

pub mod cells;
pub mod emit;
pub mod formula;
mod ftables;

pub use emit::to_package;

/// OLE compound file magic.
pub const OLE_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

pub fn is_ole(bytes: &[u8]) -> bool {
    bytes.starts_with(&OLE_MAGIC)
}

/// The `VisioDocument` stream of an OLE file.
pub fn visio_stream(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut cf = cfb::CompoundFile::open(std::io::Cursor::new(bytes)).context("OLE container")?;
    let mut s = cf
        .open_stream("/VisioDocument")
        .context("no VisioDocument stream")?;
    let mut out = Vec::new();
    s.read_to_end(&mut out)?;
    if !out.starts_with(b"Visio (TM) Drawing") || out.len() < 0x36 {
        bail!("not a Visio binary document");
    }
    Ok(out)
}

/// 4 KiB ring-buffer LZSS: a flag byte (LSB first, 1 = literal), back
/// references are 2 bytes: 12-bit window offset (biased by 18) and a
/// 4-bit length + 3.
pub fn lzss(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len() * 3);
    let mut ring = [0u8; 4096];
    let mut pos = 0usize;
    let mut i = 0usize;
    while i < src.len() {
        let flags = src[i];
        i += 1;
        for bit in 0..8 {
            if i >= src.len() {
                break;
            }
            if flags & (1 << bit) != 0 {
                let b = src[i];
                i += 1;
                out.push(b);
                ring[pos] = b;
                pos = (pos + 1) & 0xFFF;
            } else {
                if i + 1 >= src.len() {
                    return out;
                }
                let (lo, hi) = (src[i] as usize, src[i + 1] as usize);
                i += 2;
                let off = ((lo | ((hi & 0xF0) << 4)) + 18) & 0xFFF;
                for k in 0..(hi & 0x0F) + 3 {
                    let b = ring[(off + k) & 0xFFF];
                    out.push(b);
                    ring[pos] = b;
                    pos = (pos + 1) & 0xFFF;
                }
            }
        }
    }
    out
}

pub fn u16_at(d: &[u8], p: usize) -> u16 {
    d.get(p..p + 2)
        .map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
}
pub fn u32_at(d: &[u8], p: usize) -> u32 {
    d.get(p..p + 4)
        .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
pub fn f64_at(d: &[u8], p: usize) -> f64 {
    d.get(p..p + 8)
        .map_or(0.0, |b| f64::from_le_bytes(b.try_into().unwrap()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ptr {
    pub typ: u32,
    pub off: u32,
    pub len: u32,
    pub fmt: u16,
}

impl Ptr {
    fn at(d: &[u8], p: usize) -> Self {
        Ptr {
            typ: u32_at(d, p),
            off: u32_at(d, p + 8),
            len: u32_at(d, p + 12),
            fmt: u16_at(d, p + 16),
        }
    }
    pub fn compressed(&self) -> bool {
        self.fmt & 2 != 0
    }
    pub fn has_children(&self) -> bool {
        self.fmt & 0x10 != 0
    }
    pub fn has_chunks(&self) -> bool {
        self.fmt & 0x80 != 0
    }
}

/// The decoded content of one stream.
pub fn stream(doc: &[u8], p: &Ptr) -> Vec<u8> {
    let (a, b) = (p.off as usize, p.off as usize + p.len as usize);
    let raw = doc.get(a..b.min(doc.len())).unwrap_or(&[]);
    if p.compressed() {
        lzss(raw)
    } else {
        raw.to_vec()
    }
}

pub fn trailer(doc: &[u8]) -> Ptr {
    Ptr::at(doc, 0x24)
}

/// The child pointer list of a stream. Compressed streams start with
/// a 4-byte prefix (`base` = 4), others with none; the u32 at `base` is
/// the list offset relative to `base`. There: `u32 count`, `u32 ?`,
/// then `count` pointers (usually followed by an order array).
pub fn children(p: &Ptr, s: &[u8]) -> Vec<Ptr> {
    children_ordered(p, s).0
}

/// [`children`] plus the order array that follows the list (`u32 n`,
/// `n` slot indices): the display order of pages and masters.
pub fn children_ordered(p: &Ptr, s: &[u8]) -> (Vec<Ptr>, Vec<u32>) {
    let base = if p.compressed() { 4 } else { 0 };
    let lo = u32_at(s, base) as usize + base;
    let n = u32_at(s, lo) as usize;
    if n > 100_000 || lo + 8 + 18 * n > s.len() {
        return (vec![], vec![]);
    }
    // The order array fills the rest of the stream; its count is either
    // its first word or (page lists) the word just before the list.
    let after = lo + 8 + 18 * n;
    let words: Vec<u32> = (after..s.len().saturating_sub(3))
        .step_by(4)
        .map(|o| u32_at(s, o))
        .collect();
    let order = match words.first() {
        Some(&m) if m as usize == words.len() - 1 => words[1..].to_vec(),
        _ => words,
    };
    ((0..n).map(|k| Ptr::at(s, lo + 8 + 18 * k)).collect(), order)
}

/// One ShapeSheet record in a chunk stream.
#[derive(Debug, Clone)]
pub struct Chunk<'a> {
    pub pos: usize,
    pub typ: u32,
    pub id: u32,
    pub list: u32,
    pub level: u16,
    pub unk: u8,
    pub data: &'a [u8],
}

thread_local! {
    /// The file being read is Visio 2000 (v6): 8-bit strings and no
    /// row trailers. ponytail: a per-thread flag set by the converter
    /// instead of threading a version through every reader; pass a
    /// version value around once more v6 differences turn up.
    static V6: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Mark the current thread as reading a v6 (Visio 2000) file.
pub fn set_v6(on: bool) {
    V6.with(|v| v.set(on));
}

/// Is the current file Visio 2000 (8-bit strings)?
pub fn is_v6() -> bool {
    V6.with(|v| v.get())
}

/// A NUL-terminated string: UTF-16LE (v11) or 8-bit Windows-1252 (v6,
/// read as Latin-1).
pub fn text(d: &[u8]) -> String {
    if is_v6() {
        let end = d.iter().position(|&b| b == 0).unwrap_or(d.len());
        return d[..end].iter().map(|&b| b as char).collect();
    }
    let units: Vec<u16> = d
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
        .collect();
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// Chunk header: type u32, id u32, list u32, data length u32, level
/// u16, u8 flag; then the data; then a trailer of 8 bytes when the
/// chunk carries a list and (v11 only) 4 bytes for level-2 rows
/// flagged 0x55.
pub fn chunks(s: &[u8]) -> Vec<Chunk<'_>> {
    let v6 = is_v6();
    let mut out = vec![];
    let mut p = 0;
    while p + 19 <= s.len() {
        let typ = u32_at(s, p);
        if typ == 0 {
            p += 4;
            continue;
        }
        let len = u32_at(s, p + 12) as usize;
        let (list, level, unk) = (u32_at(s, p + 8), u16_at(s, p + 16), s[p + 18]);
        let end = (p + 19 + len).min(s.len());
        // a list chunk's 8-byte "trailer" is the tail of its row-order
        // array, so it belongs to the record data (see [`list_order`])
        let data_end = if list != 0 {
            (end + 8).min(s.len())
        } else {
            end
        };
        out.push(Chunk {
            pos: p,
            typ,
            id: u32_at(s, p + 4),
            list,
            level,
            unk,
            data: &s[p + 19..data_end],
        });
        p = end;
        if list != 0 || trail8(typ) {
            p += 8;
        }
        if level == 2 && unk == 0x55 && !v6 {
            p += 4;
        }
    }
    out
}

/// Chunk types that always carry an 8-byte trailer (refined empirically).
fn trail8(_typ: u32) -> bool {
    false
}

/// One ShapeSheet record of a shape, whichever way it was stored.
#[derive(Debug, Clone)]
pub struct Rec {
    pub typ: u32,
    /// Row index inside its list (inline: chunk id; streamed: slot).
    pub id: u32,
    /// The list (section) it belongs to: (list type, list id), or None
    /// for records directly on the shape.
    pub list: Option<(u32, u32)>,
    pub data: Vec<u8>,
}

/// The row order of a section list record: `u32` offset (+8) of the
/// order array, `u32` its size in bytes, … `u32` count at 20, then the
/// row ids in display order from byte 32 (z-order for shape lists).
/// None when the record has none.
pub fn list_order(d: &[u8]) -> Option<Vec<u32>> {
    let off = u32_at(d, 0) as usize + 8;
    let n = u32_at(d, 20) as usize;
    (n > 0 && off + 4 * n <= d.len()).then(|| (0..n).map(|k| u32_at(d, off + 4 * k)).collect())
}

/// Section list streams (0x64 Scratch … 0x76): always hold a child list,
/// whatever their format flags say (later v11 writers leave the flag off).
pub fn is_list_type(t: u32) -> bool {
    (0x64..=0x76).contains(&t)
}

/// Shape-like stream types (page sheet, group, shape, foreign, …).
pub fn is_shape_type(t: u32) -> bool {
    matches!(t, 0x46..=0x4e)
}

/// Records of an inline chunk run whose top level is `base`: a list
/// chunk at `base` opens a section (0x68 lists hold the shape-level
/// single-row sections), deeper chunks are its rows; list-less chunks
/// at `base` are shape-level records (text, NameIDX, …).
pub fn tree_records(ch: &[Chunk], base: u16) -> Vec<Rec> {
    let mut out = vec![];
    let mut cur: Option<(u32, u32)> = None;
    for c in ch {
        if c.level < base {
            continue;
        }
        if c.level == base {
            if c.list != 0 {
                cur = if c.typ == 0x68 {
                    None
                } else {
                    // keep the list record: it carries the rows' order
                    out.push(Rec {
                        typ: c.typ,
                        id: c.id,
                        list: None,
                        data: c.data.to_vec(),
                    });
                    Some((c.typ, c.id))
                };
            } else {
                cur = None;
                out.push(Rec {
                    typ: c.typ,
                    id: c.id,
                    list: None,
                    data: c.data.to_vec(),
                });
            }
        } else {
            out.push(Rec {
                typ: c.typ,
                id: c.id,
                list: cur,
                data: c.data.to_vec(),
            });
        }
    }
    out
}

/// All records of the shape stream `p`, whichever way they are stored:
/// inline chunks, or child streams (list streams → rows by slot). The
/// shape header comes first with `id == u32::MAX`.
pub fn shape_records(doc: &[u8], p: &Ptr) -> Vec<Rec> {
    let mut out = vec![];
    let s = stream(doc, p);
    if p.has_chunks() {
        let ch = chunks(&s);
        if let Some(h) = ch.iter().find(|c| c.level == 0) {
            out.push(Rec {
                typ: h.typ,
                id: u32::MAX,
                list: None,
                data: h.data.to_vec(),
            });
        }
        out.extend(tree_records(&ch, 1));
    } else {
        let base = if p.compressed() { 4 } else { 0 };
        out.push(Rec {
            typ: p.typ,
            id: u32::MAX,
            list: None,
            data: s.get(base..).unwrap_or(&[]).to_vec(),
        });
    }
    if p.has_children() {
        for (i, c) in children(p, &s).iter().enumerate() {
            collect(doc, c, i as u32, None, &mut out);
        }
    }
    out
}

fn collect(doc: &[u8], p: &Ptr, slot: u32, list: Option<(u32, u32)>, out: &mut Vec<Rec>) {
    if p.typ == 0 || p.len == 0 || is_shape_type(p.typ) {
        return;
    }
    let s = stream(doc, p);
    let sect = if p.typ == 0x68 {
        None
    } else {
        Some((p.typ, slot))
    };
    if p.has_chunks() {
        for c in chunks(&s) {
            out.push(Rec {
                typ: c.typ,
                id: c.id,
                list: sect,
                data: c.data.to_vec(),
            });
        }
    } else if p.has_children() || is_list_type(p.typ) {
        let (kids, order) = children_ordered(p, &s);
        if p.typ != 0x68 {
            // the same layout as an inline list record (see [`list_order`])
            let mut d = vec![0u8; 32];
            d[0..4].copy_from_slice(&24u32.to_le_bytes());
            d[4..8].copy_from_slice(&((4 * order.len()) as u32).to_le_bytes());
            d[20..24].copy_from_slice(&(order.len() as u32).to_le_bytes());
            for o in &order {
                d.extend_from_slice(&o.to_le_bytes());
            }
            out.push(Rec {
                typ: p.typ,
                id: slot,
                list: None,
                data: d,
            });
        }
        for (i, c) in kids.iter().enumerate() {
            collect(doc, c, i as u32, sect, out);
        }
    } else {
        let data = if p.compressed() {
            s.get(4..).unwrap_or(&[]).to_vec()
        } else {
            s
        };
        out.push(Rec {
            typ: p.typ,
            id: slot,
            list,
            data,
        });
    }
}

/// The sheets (page sheet, shapes) of a page or master container `p`,
/// by shape id: (stream/chunk type, records with the header first as
/// `id == u32::MAX`). Shapes are stored either as child streams (slot
/// = id) or inline in the container's own chunk stream, where each
/// level-1 shape chunk opens a shape whose records follow one level
/// deeper.
pub fn sheets(doc: &[u8], p: &Ptr) -> Vec<(u32, u32, Vec<Rec>)> {
    let s = stream(doc, p);
    let mut out = vec![];
    if p.has_chunks() {
        let ch = chunks(&s);
        let mut i = 0;
        while i < ch.len() {
            let c = &ch[i];
            if c.level == 1 && is_shape_type(c.typ) {
                let mut j = i + 1;
                while j < ch.len() && ch[j].level > 1 {
                    j += 1;
                }
                let mut recs = vec![Rec {
                    typ: c.typ,
                    id: u32::MAX,
                    list: None,
                    data: c.data.to_vec(),
                }];
                recs.extend(tree_records(&ch[i + 1..j], 2));
                out.push((c.typ, c.id, recs));
                i = j;
            } else {
                i += 1;
            }
        }
    }
    if p.has_children() {
        for (i, c) in children(p, &s).iter().enumerate() {
            if is_shape_type(c.typ) {
                out.push((c.typ, i as u32, shape_records(doc, c)));
            }
        }
    }
    out
}

/// Walk every stream reachable from the trailer (depth-first, each
/// stream once), calling `f(depth, ptr, data)`.
pub fn walk(doc: &[u8], mut f: impl FnMut(usize, &Ptr, &[u8])) {
    fn rec(
        doc: &[u8],
        p: Ptr,
        depth: usize,
        seen: &mut std::collections::HashSet<(u32, u32)>,
        f: &mut dyn FnMut(usize, &Ptr, &[u8]),
    ) {
        if p.typ == 0 || p.len == 0 || !seen.insert((p.off, p.len)) {
            return;
        }
        let s = stream(doc, &p);
        f(depth, &p, &s);
        if p.has_children() {
            for c in children(&p, &s) {
                rec(doc, c, depth + 1, seen, f);
            }
        }
    }
    rec(doc, trailer(doc), 0, &mut Default::default(), &mut f);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Literal run, then a back reference copying it again.
    #[test]
    fn lzss_literals_and_backref() {
        // flags 0b0000_0111: 3 literals "abc", then ref to window offset 18-18=0, len 3
        let src = [0x07, b'a', b'b', b'c', 0x00, 0x00];
        // offset field (0x000 + 18) & 0xFFF = 18 → ring[18..21], which is zeros;
        // so encode offset so that it lands at 0: stored = 0 - 18 mod 4096 = 4078 = 0xFEE
        let src2 = [0x07, b'a', b'b', b'c', 0xEE, 0xF0];
        assert_eq!(lzss(&src)[..3], *b"abc");
        assert_eq!(lzss(&src2), b"abcabc");
    }
}
