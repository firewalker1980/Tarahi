//! Check Spelling (GitHub #121) — the dialog-driven pass over the
//! page's shape text (Visio's F7 model: no live squigglies; spelling is
//! a command that walks shape by shape with a Check Spelling dialog).
//!
//! This module owns the domain (the app.rs decomposition direction):
//! the session walk, the Change/Change-All command builders, the
//! app-level custom dictionary store, and the dialog window. `app.rs`
//! keeps only the state field, the F7 dispatch, and the outcome
//! forwarding. The checker itself lives in `dc::spellcheck`
//! (spellbook-backed, en/de/fa embedded dictionaries plus local
//! discovery); field runs never check, and a Change splices the raw
//! text through `dc_core::ReplaceTextRange` so every other marker
//! survives.

use std::path::PathBuf;

use egui::Context;
use vsdx::model::{Document, Page, Shape};

use dc::spellcheck::{self, Checker, LangEntry, WordToken};
use dc_core::Command as _;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// The app-level spell state: the open session plus the two word lists
/// that outlive it. `app.rs` holds one field of this.
pub(crate) struct SpellState {
    /// The open Check Spelling pass (`None` = dialog closed).
    pub session: Option<SpellSession>,
    /// Whether Add persists to the user's custom-dictionary store —
    /// production only; headless instances stay in-memory so a kittest
    /// Add never rewrites the real config (CodeRabbit round).
    pub persist: bool,
    /// The check language — a dictionary stem from
    /// [`spellcheck::available`]. Empty until the first pass, which
    /// picks the UI language's dictionary when one exists.
    pub lang: String,
    /// The app-level custom dictionary (Add adds here and persists).
    pub custom: Vec<String>,
    /// Ignore All — session-scoped, in-memory only (Visio's rule).
    pub ignore_all: Vec<String>,
    /// The dictionary list the dialog's dropdown offers (refreshed on
    /// pass start; local discovery may add entries after boot).
    langs: Vec<LangEntry>,
}

impl SpellState {
    /// Production: loads the persisted custom dictionary.
    pub(crate) fn prod() -> Self {
        let mut s = Self::headless();
        s.persist = true;
        s.custom = load_custom();
        s
    }

    /// Headless ctors: no user-machine reads (the stencil pinning rule),
    /// so the custom dictionary starts empty and nothing persists.
    pub(crate) fn headless() -> Self {
        SpellState {
            session: None,
            persist: false,
            lang: String::new(),
            custom: Vec::new(),
            ignore_all: Vec::new(),
            langs: Vec::new(),
        }
    }

    /// Start a pass over `page`: gather the checkable shapes, resolve
    /// the language, and find the first misspelling. `Ok(false)` means
    /// the page is clean (no session opens).
    pub(crate) fn start(&mut self, doc: &Document, page: i64) -> bool {
        self.lang = self.resolve_lang();
        self.ignore_all.clear();
        self.langs = spellcheck::available();
        let Some(p) = doc.pages.iter().find(|p| p.id == page) else {
            return false;
        };
        let shapes = shape_ids_with_effective_text(doc, p);
        let total = shapes.len();
        let mut session = SpellSession {
            page,
            shapes,
            idx: 0,
            cursor: 0,
            word: None,
            suggestions: Vec::new(),
            sel: 0,
            total,
        };
        let Some(checker) = self.build_checker() else {
            return false;
        };
        let found = scan(doc, &mut session, &checker);
        if found {
            self.session = Some(session);
        } else {
            // A completed pass closes any live session (PR #299 review):
            // a re-click while the dialog is up must not keep showing a
            // word the user just fixed or undid.
            self.session = None;
        }
        found
    }

    /// The dictionary stem for a pass: the chosen one, else the UI
    /// language's dictionary, else English.
    fn resolve_lang(&self) -> String {
        if !self.lang.is_empty() {
            return self.lang.clone();
        }
        let wanted = match crate::i18n::current() {
            crate::l10n::L10n::En => "en",
            crate::l10n::L10n::De => "de",
            crate::l10n::L10n::Fa => "fa",
            crate::l10n::L10n::Es => "es",
            // No CJK hunspell path (#298): the CJK UIs check English.
            crate::l10n::L10n::ZhHans => "en",
            crate::l10n::L10n::Ja => "en",
            crate::l10n::L10n::Ko => "en",
        };
        spellcheck::available()
            .into_iter()
            .find(|e| e.stem.starts_with(wanted))
            .map(|e| e.stem)
            .unwrap_or_else(|| "en_US".to_string())
    }

    /// The pass's decision function: the dictionary plus the custom
    /// dictionary and Ignore-All lists. Owned (not borrowed from the
    /// session) so the scan can borrow the session while it runs.
    fn build_checker(&self) -> Option<Checker> {
        let dict = spellcheck::get(&self.lang)?;
        let mut checker = Checker::new(dict);
        for w in &self.custom {
            checker.add_custom(w.clone());
        }
        for w in &self.ignore_all {
            checker.ignore_all(w.clone());
        }
        Some(checker)
    }

    /// Scan forward from the session cursor for the next misspelling,
    /// walking into subsequent shapes when the current one is clean.
    /// `false` means the pass is done (the caller closes with the
    /// completion status).
    pub(crate) fn advance(&mut self, doc: &Document) -> bool {
        let Some(checker) = self.build_checker() else {
            return false;
        };
        let Some(session) = self.session.as_mut() else {
            return false;
        };
        scan(doc, session, &checker)
    }
}

/// The scan loop (free so the session borrow never aliases the state).
fn scan(doc: &Document, session: &mut SpellSession, checker: &Checker) -> bool {
    while session.idx < session.shapes.len() {
        let shape = session.shapes[session.idx];
        let Some(raw) = spellcheck::effective_raw(doc, session.page, shape) else {
            session.idx += 1;
            session.cursor = 0;
            continue;
        };
        if let Some(tok) = checker.first_misspelling(&raw, session.cursor) {
            let suggestions = checker.suggest(&tok.word);
            session.word = Some(WordEntry {
                shape,
                token: tok,
                plain: spellcheck::plain_of(&raw),
            });
            session.suggestions = suggestions;
            session.sel = 0;
            return true;
        }
        session.idx += 1;
        session.cursor = 0;
    }
    session.word = None;
    session.suggestions.clear();
    false
}

/// One flagged misspelling: the shape it lives on, its plain-space span
/// (the `ReplaceTextRange` coordinates), and the shape's full plain
/// text for the word-in-context line.
#[derive(Debug, Clone)]
pub(crate) struct WordEntry {
    pub shape: i64,
    pub token: WordToken,
    pub plain: String,
}

/// One open spelling pass over a page's shapes.
pub(crate) struct SpellSession {
    pub page: i64,
    /// The page's text-bearing shapes in reading order (group members
    /// included — Visio checks the whole tree).
    pub shapes: Vec<i64>,
    /// Index into `shapes` of the shape under inspection.
    pub idx: usize,
    /// Plain-char cursor within the current shape (past the last
    /// accepted/replaced word).
    pub cursor: usize,
    /// The flagged word, when one is under inspection.
    pub word: Option<WordEntry>,
    /// The dictionary's ranked suggestions for the flagged word.
    pub suggestions: Vec<String>,
    /// The selected suggestion index.
    pub sel: usize,
    /// Total text-bearing shapes on the page (the "shape N of M" line).
    pub total: usize,
}

/// The page's shapes whose EFFECTIVE text resolves (local text, master
/// text, or text inherited through an enclosing group's master — the
/// `master_shape`-only child of a master-backed group has no markers of
/// its own but still renders the master's words), in document order,
/// group members included (CodeRabbit round: the old static predicate
/// dropped such children before `effective_raw` could resolve them).
fn shape_ids_with_effective_text(doc: &Document, page: &Page) -> Vec<i64> {
    fn walk(out: &mut Vec<i64>, doc: &Document, page_id: i64, shapes: &[Shape]) {
        for s in shapes {
            if spellcheck::effective_raw(doc, page_id, s.id).is_some() {
                out.push(s.id);
            }
            walk(out, doc, page_id, &s.sub_shapes);
        }
    }
    let mut out = Vec::new();
    walk(&mut out, doc, page.id, &page.shapes);
    out
}

// ---------------------------------------------------------------------------
// Command builders (the Change / Change All macros)
// ---------------------------------------------------------------------------

/// The undo step for one accepted Change: the raw splice plus the
/// text-derived cache refreshes (#145/#147/#148), measured against the
/// display the renderer will paint — the substituted text when the
/// shape carries field markers, the plain text otherwise. Returns the
/// macro and the plain-char cursor to resume from (the replacement's
/// start — a fix that introduces a new typo re-flags immediately).
pub(crate) fn replace_macro(
    doc: &Document,
    page: i64,
    shape: i64,
    token: &WordToken,
    to: &str,
) -> Option<(dc_core::Macro, usize)> {
    let scope = dc_core::Scope::Shape { page, shape };
    // The measurement probe: the same replacements applied to a clone,
    // so the caches measure exactly what will render (the end_text_edit
    // convention).
    let mut probe = doc.clone();
    let mut cmd = dc_core::ReplaceTextRange::new(
        scope.clone(),
        token.start,
        token.end - token.start,
        to,
    );
    cmd.apply(&mut probe).ok()?;
    let display = probe_display(&probe, page, shape);
    let mut cmds: Vec<Box<dyn dc_core::Command>> = vec![Box::new(cmd)];
    cmds.extend(dc::textfunc::text_cache_refreshes(doc, &scope, &display));
    let cursor = token.start + to.chars().count();
    Some((
        dc_core::Macro::new(format!("spell change of shape {shape}"), cmds),
        cursor,
    ))
}

/// The undo step for Change All: every remaining occurrence of `word`
/// on the page's checkable shapes — one `ReplaceTextRange` each — in a
/// single macro, so one undo reverts the whole pass (the #136/#59 Macro
/// precedent). Occurrences apply per shape in DESCENDING start order,
/// so each splice's plain offsets stay valid against the raw the
/// command actually sees.
pub(crate) fn change_all_macro(
    doc: &Document,
    page: i64,
    shapes: &[i64],
    lang: &str,
    word: &str,
    to: &str,
) -> Option<dc_core::Macro> {
    // Contract (the Visio "change all instances of this word" semantics):
    // every occurrence of the FLAGGED word across the page's shapes,
    // including occurrences the walk already passed. The flagged word is
    // by definition not in the Ignore-All list, and single-Ignores
    // record words, not occurrences — a Change All after an Ignore
    // deliberately overrides the earlier skip (the oracle caveat stands:
    // re-verify against a Visio bottle when one returns).
    let _dict = spellcheck::get(lang); // dictionary presence gate
    let mut cmds: Vec<Box<dyn dc_core::Command>> = Vec::new();
    let mut probe = doc.clone();
    let mut touched: Vec<i64> = Vec::new();
    for shape in shapes {
        let Some(raw) = spellcheck::effective_raw(doc, page, *shape) else {
            continue;
        };
        let mut edits: Vec<WordToken> = spellcheck::words(&raw)
            .into_iter()
            .filter(|t| t.word.to_lowercase() == word.to_lowercase())
            .collect();
        if edits.is_empty() {
            continue;
        }
        edits.sort_by_key(|t| std::cmp::Reverse(t.start));
        for tok in edits {
            let scope = dc_core::Scope::Shape {
                page,
                shape: *shape,
            };
            let mut cmd = dc_core::ReplaceTextRange::new(
                scope.clone(),
                tok.start,
                tok.end - tok.start,
                to,
            );
            if cmd.apply(&mut probe).is_ok() {
                cmds.push(Box::new(cmd));
                if !touched.contains(shape) {
                    touched.push(*shape);
                }
            }
        }
    }
    if cmds.is_empty() {
        return None;
    }
    for shape in &touched {
        let display = probe_display(&probe, page, *shape);
        let scope = dc_core::Scope::Shape {
            page,
            shape: *shape,
        };
        cmds.extend(dc::textfunc::text_cache_refreshes(doc, &scope, &display));
    }
    Some(dc_core::Macro::new(
        format!("spell change all of {word:?}"),
        cmds,
    ))
}

/// The display text a probe-rendered shape paints: the substituted text
/// for a field-bearing shape (resolved against the probe document's
/// page context, the layer path's rule), else the plain text.
pub(crate) fn probe_display(probe: &Document, page: i64, shape: i64) -> String {
    let Some(p) = probe.pages.iter().find(|p| p.id == page) else {
        return String::new();
    };
    let Some(sh) = vsdx::model::find_shape_by_id(&p.shapes, shape) else {
        return String::new();
    };
    if let Ok(chain) = dc_core::scope::shape_chain(probe, page, shape) {
        if let Some(sub) =
            dc::fields::substituted_text(probe, sh, &chain, Some(&dc::fields::FieldsCtx::for_page(probe, p)))
        {
            return sub;
        }
    }
    sh.sheet.text.clone().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Custom dictionary store (app-level config — never .vsdx content)
// ---------------------------------------------------------------------------

/// The persisted custom dictionary (schema v1: `{"words": [...]}`),
/// beside `language.json`/`recent.json` in the app-config dir. The
/// `language.json` precedent: an app-level store the document never
/// carries.
pub(crate) fn custom_path() -> Option<PathBuf> {
    crate::recent::config_base().map(|b| b.join("diagramcraft").join("custom-dictionary.json"))
}

/// Load the custom dictionary (best-effort: unreadable/absent/older
/// schemas start empty).
pub(crate) fn load_custom() -> Vec<String> {
    let Some(p) = custom_path() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&p) else {
        return Vec::new();
    };
    let Some(v) = serde_json::from_str::<serde_json::Value>(&text).ok() else {
        return Vec::new();
    };
    v.get("words")
        .and_then(|w| w.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|w| w.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Persist the custom dictionary (best-effort).
pub(crate) fn save_custom(words: &[String]) {
    let Some(p) = custom_path() else {
        return;
    };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let json = serde_json::json!({ "version": 1, "words": words });
    let _ = std::fs::write(&p, json.to_string());
}

/// Test hook: write the custom dictionary to an explicit path (the
/// language.json test pattern — kittest runs parallel in one process,
/// so the real config path is never touched).
#[doc(hidden)]
pub fn save_custom_to_for_test(path: &std::path::Path, words: &[String]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let json = serde_json::json!({ "version": 1, "words": words });
    let _ = std::fs::write(path, json.to_string());
}

/// Test hook: read a custom dictionary back from an explicit path.
#[doc(hidden)]
pub fn load_custom_from_for_test(path: &std::path::Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Some(v) = serde_json::from_str::<serde_json::Value>(&text).ok() else {
        return Vec::new();
    };
    v.get("words")
        .and_then(|w| w.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|w| w.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Dialog
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use vsdx::model::Shape;

    fn doc_with_shape_text(text: &str) -> Document {
        let sheet = vsdx::model::Sheet {
            text_raw: Some(text.to_string()),
            text: Some(text.to_string()),
            ..vsdx::model::Sheet::default()
        };
        let shape = Shape {
            id: 1,
            type_: Some("Shape".into()),
            sheet,
            ..Shape::default()
        };
        let page = vsdx::model::Page {
            id: 0,
            name: "P1".into(),
            shapes: vec![shape],
            ..vsdx::model::Page::default()
        };
        Document {
            pages: vec![page],
            ..Document::default()
        }
    }

    /// CodeRabbit round: Change All must not leave the scan offset
    /// stale — replacing BOTH occurrences in "tehhh tehhh wrng" moves
    /// "wrng" from 12 to 8, and the resumed scan must still flag it.
    #[test]
    fn change_all_rescans_the_updated_text() {
        let doc = doc_with_shape_text("tehhh tehhh wrng");
        let page = 0;
        let shapes = vec![1];
        let mut mac = change_all_macro(&doc, page, &shapes, "en_US", "tehhh", "the")
            .expect("replacements exist");
        let mut probe = doc.clone();
        mac.apply(&mut probe).unwrap();
        // The word list of the UPDATED text: no tehhh left, and wrng
        // sits at its new position 8.
        let raw = spellcheck::effective_raw(&probe, page, 1).unwrap();
        let toks = spellcheck::words(&raw);
        assert_eq!(
            toks.iter().map(|t| t.word.as_str()).collect::<Vec<_>>(),
            vec!["the", "the", "wrng"]
        );
        let wrng = toks.last().unwrap();
        assert_eq!(wrng.start, 8, "the scan sees the UPDATED offsets");
        assert_eq!(&raw[wrng.start..wrng.end], "wrng");
        // A fresh scan from cursor 0 (what the Change All handler does)
        // flags wrng — nothing is skipped.
        let dict = spellcheck::get("en_US").unwrap();
        let checker = Checker::new(dict);
        let flagged = checker.first_misspelling(&raw, 0).expect("wrng flagged");
        assert_eq!(flagged.word, "wrng");
    }
}

/// What the dialog decided this frame (the fields_dlg outcome pattern —
/// app.rs performs the commands).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SpellOutcome {
    Open,
    /// Skip this occurrence.
    Ignore,
    /// Skip every remaining occurrence of the word this pass.
    IgnoreAll,
    /// Replace this occurrence with the selected suggestion.
    Change { to: String },
    /// Replace every remaining occurrence of the word on the page.
    ChangeAll { to: String },
    /// Add the word to the app-level custom dictionary.
    Add,
    /// The language dropdown changed — restart the pass.
    Restart,
    /// Close the dialog.
    Close,
}

/// The Check Spelling dialog. `state.lang` mutates when the dropdown
/// picks a different dictionary (caller restarts the pass).
pub(crate) fn window(ctx: &Context, open: &mut bool, state: &mut SpellState) -> SpellOutcome {
    let mut outcome = SpellOutcome::Open;
    let mut lang_changed = false;
    let Some(session) = state.session.as_mut() else {
        return SpellOutcome::Close;
    };
    egui::Window::new(crate::i18n::strs().msg_spell_dialog_title())
        .open(open)
        .resizable(false)
        .show(ctx, |ui| {
            let s = crate::i18n::strs();
            let Some(word) = &session.word else {
                return;
            };
            ui.horizontal(|ui| {
                ui.label(s.msg_spell_not_in_dictionary());
                ui.strong(word.token.word.clone());
            });
            // The word in context: the shape's plain text with the
            // misspelling highlighted.
            let plain = &word.plain;
            let start = word.token.start.min(plain.chars().count());
            let end = word.token.end.min(plain.chars().count());
            let before: String = plain.chars().take(start).collect();
            let mid: String = plain.chars().skip(start).take(end - start).collect();
            let after: String = plain.chars().skip(end).collect();
            ui.horizontal_wrapped(|ui| {
                ui.label(before);
                ui.label(
                    egui::RichText::new(mid)
                        .strong()
                        .background_color(ui.visuals().faint_bg_color),
                );
                ui.label(after);
            });
            ui.separator();
            ui.label(s.msg_spell_suggestions());
            if session.suggestions.is_empty() {
                ui.weak(s.msg_spell_no_suggestions());
            } else {
                session.sel = session.sel.min(session.suggestions.len() - 1);
                egui::ScrollArea::vertical()
                    .max_height(140.0)
                    .show(ui, |ui| {
                        for (i, sug) in session.suggestions.iter().enumerate() {
                            ui.selectable_value(&mut session.sel, i, sug.clone());
                        }
                    });
            }
            ui.separator();
            // The language dropdown (the house manual-dropdown pattern).
            ui.horizontal(|ui| {
                ui.label(s.msg_language());
                state.langs = spellcheck::available();
                let current = state
                    .langs
                    .iter()
                    .find(|e| e.stem == state.lang)
                    .map(|e| e.display_name())
                    .unwrap_or_else(|| state.lang.clone());
                let r = ui.button(format!("{current}  ›"));
                r.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::ComboBox,
                        true,
                        s.msg_language().as_str(),
                    )
                });
                egui::Popup::menu(&r)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
                    .show(|ui| {
                        ui.set_min_width(150.0);
                        let langs = state.langs.clone();
                        for e in &langs {
                            let clicked = ui
                                .selectable_label(e.stem == state.lang, e.display_name())
                                .clicked();
                            if clicked && e.stem != state.lang {
                                state.lang = e.stem.clone();
                                lang_changed = true;
                            }
                        }
                    });
            });
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Visio's order: Ignore · Ignore All · Change · Change
                // All · Add (right-to-left layout paints the row's last
                // button leftmost).
                if ui.button(s.msg_spell_add()).clicked() {
                    outcome = SpellOutcome::Add;
                }
                if ui.button(s.msg_spell_change_all()).clicked() {
                    // Without a suggestion there is nothing to change
                    // to — the buttons advance instead (the Visio
                    // dialog's no-suggestion flow).
                    outcome = match session.suggestions.get(session.sel) {
                        Some(to) => SpellOutcome::ChangeAll { to: to.clone() },
                        None => SpellOutcome::Ignore,
                    };
                }
                if ui.button(s.msg_spell_change()).clicked() {
                    outcome = match session.suggestions.get(session.sel) {
                        Some(to) => SpellOutcome::Change { to: to.clone() },
                        None => SpellOutcome::Ignore,
                    };
                }
                if ui.button(s.msg_spell_ignore_all()).clicked() {
                    outcome = SpellOutcome::IgnoreAll;
                }
                if ui.button(s.msg_spell_ignore()).clicked() {
                    outcome = SpellOutcome::Ignore;
                }
            });
            // The shape counter ("shape 3 of 12").
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.weak(s.msg_spell_shape_counter(
                    (session.idx + 1) as i64,
                    session.total as i64,
                ));
            });
        });
    // Enter = the default button (Change; with no suggestions it
    // advances, the Visio dialog's no-suggestion flow).
    if outcome == SpellOutcome::Open
        && ctx.input(|i| i.key_pressed(egui::Key::Enter))
        && session.word.is_some()
    {
        outcome = match session.suggestions.get(session.sel) {
            Some(to) => SpellOutcome::Change { to: to.clone() },
            None => SpellOutcome::Ignore,
        };
    }
    if lang_changed {
        outcome = SpellOutcome::Restart;
    }
    if !*open {
        outcome = SpellOutcome::Close;
    }
    outcome
}
