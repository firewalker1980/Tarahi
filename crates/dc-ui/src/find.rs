//! Find & Replace (Visio's Home ▸ Editing ▸ Find / Replace): matches in
//! shape text across one page or all pages, and the one-undo-step
//! replace macro. Offsets are plain-char offsets into the shape's text
//! (the `ReplaceTextRange` / spell-check space); field contents are
//! never matched or replaced.

use dc::spellcheck::effective_raw;
use vsdx::model::{Document, Shape};
use vsdx::text::{text_pieces, Piece};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub page: i64,
    pub shape: i64,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Opts {
    pub match_case: bool,
    pub whole_word: bool,
}

/// Every match on `pages` (page ids, in this order), shapes in document
/// order (sub-shapes after their group), matches left to right.
pub fn find_all(doc: &Document, pages: &[i64], needle: &str, o: Opts) -> Vec<Hit> {
    fn walk(doc: &Document, page: i64, shapes: &[Shape], needle: &[char], o: Opts, out: &mut Vec<Hit>) {
        for s in shapes {
            if let Some(raw) = effective_raw(doc, page, s.id) {
                let mut base = 0;
                for p in text_pieces(&raw) {
                    if let Piece::Text(t) = &p {
                        let hay: Vec<char> = t.chars().collect();
                        for (a, b) in matches(&hay, needle, o) {
                            out.push(Hit { page, shape: s.id, start: base + a, end: base + b });
                        }
                    }
                    base += p.plain().chars().count();
                }
            }
            walk(doc, page, &s.sub_shapes, needle, o, out);
        }
    }
    let needle: Vec<char> = needle.chars().collect();
    let mut out = Vec::new();
    if needle.is_empty() {
        return out;
    }
    for &pid in pages {
        if let Some(p) = doc.pages.iter().find(|p| p.id == pid) {
            walk(doc, pid, &p.shapes, &needle, o, &mut out);
        }
    }
    out
}

fn matches(hay: &[char], needle: &[char], o: Opts) -> Vec<(usize, usize)> {
    let eq = |a: char, b: char| {
        a == b || (!o.match_case && a.to_lowercase().eq(b.to_lowercase()))
    };
    let word = |c: Option<&char>| c.is_some_and(|c| c.is_alphanumeric() || *c == '_');
    let mut out = Vec::new();
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        let hit = needle.iter().enumerate().all(|(k, &c)| eq(hay[i + k], c))
            && (!o.whole_word
                || (!word(i.checked_sub(1).and_then(|j| hay.get(j))) && !word(hay.get(i + needle.len()))));
        if hit {
            out.push((i, i + needle.len()));
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

/// One undo step replacing `hits` with `to`: per shape the splices run
/// right to left so earlier offsets stay valid, then the text caches
/// refresh against what will render (the spell-check Change All rule).
pub fn replace_macro(doc: &Document, hits: &[Hit], to: &str) -> Option<dc_core::Macro> {
    let mut probe = doc.clone();
    let mut cmds: Vec<Box<dyn dc_core::Command>> = Vec::new();
    let mut touched: Vec<(i64, i64)> = Vec::new();
    let mut sorted = hits.to_vec();
    sorted.sort_by_key(|h| (h.page, h.shape, std::cmp::Reverse(h.start)));
    for h in sorted {
        let scope = dc_core::Scope::Shape { page: h.page, shape: h.shape };
        let mut cmd = dc_core::ReplaceTextRange::new(scope, h.start, h.end - h.start, to);
        if dc_core::Command::apply(&mut cmd, &mut probe).is_ok() {
            cmds.push(Box::new(cmd));
            if !touched.contains(&(h.page, h.shape)) {
                touched.push((h.page, h.shape));
            }
        }
    }
    if cmds.is_empty() {
        return None;
    }
    for (page, shape) in touched {
        let display = crate::spellcheck::probe_display(&probe, page, shape);
        let scope = dc_core::Scope::Shape { page, shape };
        cmds.extend(dc::textfunc::text_cache_refreshes(doc, &scope, &display));
    }
    Some(dc_core::Macro::new(format!("replace {} matches", hits.len()), cmds))
}

/// The hit after `cur` in `hits` order (wrapping), or the first.
pub fn next_after(hits: &[Hit], cur: Option<&Hit>) -> Option<Hit> {
    let i = cur.and_then(|c| hits.iter().position(|h| h == c)).map_or(0, |i| i + 1);
    hits.get(i % hits.len().max(1)).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(texts: &[&str]) -> Document {
        let shapes = texts
            .iter()
            .enumerate()
            .map(|(i, t)| Shape {
                id: i as i64 + 1,
                type_: Some("Shape".into()),
                sheet: vsdx::model::Sheet {
                    text_raw: Some(t.to_string()),
                    text: Some(t.to_string()),
                    ..Default::default()
                },
                ..Default::default()
            })
            .collect();
        Document {
            pages: vec![vsdx::model::Page { id: 0, name: "P".into(), shapes, ..Default::default() }],
            ..Default::default()
        }
    }

    #[test]
    fn finds_with_case_and_whole_word_options() {
        let d = doc(&["Switch switches", "core SWITCH"]);
        let all = find_all(&d, &[0], "switch", Opts::default());
        assert_eq!(all.len(), 3);
        let whole = find_all(&d, &[0], "switch", Opts { whole_word: true, ..Default::default() });
        assert_eq!(whole.iter().map(|h| (h.shape, h.start)).collect::<Vec<_>>(), vec![(1, 0), (2, 5)]);
        let case = find_all(&d, &[0], "switch", Opts { match_case: true, ..Default::default() });
        assert_eq!(case.iter().map(|h| (h.shape, h.start)).collect::<Vec<_>>(), vec![(1, 7)]);
        assert_eq!(next_after(&all, all.last()), Some(all[0].clone()), "wraps");
    }

    #[test]
    fn replace_all_is_one_undoable_step() {
        let mut ed = dc_core::Editor::new(doc(&["vlan 10 vlan", "no match"]));
        let hits = find_all(&ed.doc, &[0], "vlan", Opts::default());
        ed.apply(replace_macro(&ed.doc, &hits, "VLAN").unwrap()).unwrap();
        assert_eq!(ed.doc.pages[0].shapes[0].sheet.text.as_deref(), Some("VLAN 10 VLAN"));
        ed.undo().unwrap();
        assert_eq!(ed.doc.pages[0].shapes[0].sheet.text.as_deref(), Some("vlan 10 vlan"));
    }
}
