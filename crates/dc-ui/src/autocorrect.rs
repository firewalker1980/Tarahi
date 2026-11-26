//! AutoCorrect (Review ▸ AutoCorrect Options): as you type in the text
//! editor, a word ended by a space or punctuation is replaced from the
//! list ((c) → ©, -> → →, teh → the, …), and the first letter of a
//! sentence is capitalised.

#[derive(Debug, Clone, PartialEq)]
pub struct AutoCorrect {
    pub enabled: bool,
    pub capitalize: bool,
    pub list: Vec<(String, String)>,
}

impl Default for AutoCorrect {
    fn default() -> Self {
        let list = [
            ("(c)", "©"),
            ("(r)", "®"),
            ("(tm)", "™"),
            ("...", "…"),
            ("--", "–"),
            ("->", "→"),
            ("<-", "←"),
            ("=>", "⇒"),
            ("<=", "≤"),
            (">=", "≥"),
            ("!=", "≠"),
            ("teh", "the"),
            ("adn", "and"),
            ("recieve", "receive"),
            ("seperate", "separate"),
            ("occured", "occurred"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        AutoCorrect { enabled: true, capitalize: true, list }
    }
}

fn is_delim(c: char) -> bool {
    c.is_whitespace() || matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | ')')
}

impl AutoCorrect {
    /// After a single character `typed` at plain offset `caret - 1`
    /// (the caret is after it): the corrected text and caret, or None.
    pub fn apply(&self, text: &str, caret: usize) -> Option<(String, usize)> {
        if !self.enabled || caret == 0 {
            return None;
        }
        let chars: Vec<char> = text.chars().collect();
        if caret > chars.len() || !is_delim(chars[caret - 1]) {
            return None;
        }
        // the word just finished: back to whitespace (symbols included,
        // so "(c)" and "->" are words)
        let end = caret - 1;
        let mut start = end;
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        if start == end {
            return None;
        }
        let word: String = chars[start..end].iter().collect();
        let mut out = chars.clone();
        let mut caret = caret;
        let mut changed = false;
        // whole-word list match (case-insensitive for letters, keeping a
        // leading capital)
        if let Some((_, to)) = self.list.iter().find(|(from, _)| from.eq_ignore_ascii_case(&word)) {
            let mut to = to.clone();
            if word.chars().next().is_some_and(|c| c.is_uppercase()) {
                let mut cs = to.chars();
                if let Some(f) = cs.next() {
                    to = f.to_uppercase().chain(cs).collect();
                }
            }
            let to_chars: Vec<char> = to.chars().collect();
            out.splice(start..end, to_chars.iter().copied());
            caret = caret - (end - start) + to_chars.len();
            changed = true;
        }
        // capitalise a sentence's first word
        if self.capitalize {
            let before: String = out[..start].iter().collect();
            let t = before.trim_end();
            let sentence_start = t.is_empty() || t.ends_with(['.', '!', '?']) || before.ends_with('\n');
            if sentence_start && out.get(start).is_some_and(|c| c.is_lowercase()) {
                // only real words: letters, and not a list replacement symbol
                let w: String = out[start..].iter().take_while(|c| c.is_alphabetic()).collect();
                if w.chars().count() > 1 || w == "i" {
                    let up: Vec<char> = out[start].to_uppercase().collect();
                    out.splice(start..start + 1, up.iter().copied());
                    caret = caret + up.len() - 1;
                    changed = true;
                }
            }
        }
        changed.then(|| (out.into_iter().collect(), caret))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_words_and_capitalises_sentences() {
        let ac = AutoCorrect::default();
        assert_eq!(ac.apply("(c) ", 4), Some(("© ".into(), 2)));
        assert_eq!(ac.apply("Core -> ", 8), Some(("Core → ".into(), 7)));
        assert_eq!(ac.apply("Core teh ", 9), Some(("Core the ".into(), 9)));
        assert_eq!(ac.apply("Teh,", 4), Some(("The,".into(), 4)), "keeps a capital");
        assert_eq!(ac.apply("core ", 5), Some(("Core ".into(), 5)), "sentence start");
        assert_eq!(ac.apply("Done. next ", 11), Some(("Done. Next ".into(), 11)));
        assert_eq!(ac.apply("Core router ", 12), None, "mid-sentence word stays");
        assert_eq!(ac.apply("Core", 4), None, "no delimiter yet");
        let off = AutoCorrect { enabled: false, ..AutoCorrect::default() };
        assert_eq!(off.apply("(c) ", 4), None);
    }
}
