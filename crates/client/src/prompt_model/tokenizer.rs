//! The model's tokenizer, matching Hugging Face `tokenizers` for this setup
//! (the tokenizer of `minishlab/blink-multilingual-64M`):
//!
//! 1. Added tokens (whole CJK words and two special tokens) are cut out of
//!    the raw text first, leftmost and longest.
//! 2. The rest is normalized like `BertNormalizer` (clean text, spaces around
//!    CJK characters, lowercase; no accent stripping).
//! 3. Split on whitespace and around punctuation (`BertPreTokenizer`), then
//!    into pieces of at most 100 characters (`FixedLength`).
//! 4. WordPiece, greedy longest match with `##` for continuations; a word
//!    that can't be covered becomes `[UNK]`.
//!
//! The Unicode tables (punctuation, "other" characters) come from the model
//! file, so they match what the model was trained with.

use std::collections::HashMap;

pub struct Tokenizer {
    pub(super) vocab: HashMap<String, u32>,
    pub(super) added: HashMap<String, u32>,
    /// Longest added token, in characters.
    pub(super) added_max_chars: usize,
    pub(super) punctuation: Vec<(u32, u32)>,
    pub(super) other: Vec<(u32, u32)>,
    pub(super) unk_id: u32,
    pub(super) max_input_chars_per_word: usize,
    pub(super) fixed_length: usize,
}

impl Tokenizer {
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let mut ids = Vec::new();
        let chars: Vec<(usize, char)> = text.char_indices().collect();
        let mut plain_start = 0; // byte offset of the text not yet handled
        let mut i = 0;
        while i < chars.len() {
            match self.added_at(text, &chars, i) {
                Some((id, end)) => {
                    let start = chars[i].0;
                    self.encode_plain(&text[plain_start..start], &mut ids);
                    ids.push(id);
                    plain_start = chars.get(end).map_or(text.len(), |c| c.0);
                    i = end;
                }
                None => i += 1,
            }
        }
        self.encode_plain(&text[plain_start..], &mut ids);
        ids
    }

    /// The longest added token starting at char `i`: its id and the char
    /// index after it.
    fn added_at(&self, text: &str, chars: &[(usize, char)], i: usize) -> Option<(u32, usize)> {
        let longest = self.added_max_chars.min(chars.len() - i);
        for len in (1..=longest).rev() {
            let start = chars[i].0;
            let end = chars.get(i + len).map_or(text.len(), |c| c.0);
            if let Some(&id) = self.added.get(&text[start..end]) {
                return Some((id, i + len));
            }
        }
        None
    }

    fn encode_plain(&self, text: &str, ids: &mut Vec<u32>) {
        if text.is_empty() {
            return;
        }
        let normalized = self.normalize(text);
        for word in self.pre_tokenize(&normalized) {
            let chars: Vec<char> = word.chars().collect();
            for piece in chars.chunks(self.fixed_length) {
                self.wordpiece(piece, ids);
            }
        }
    }

    fn normalize(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for c in text.chars() {
            // Clean text: drop NUL, U+FFFD and control characters, turn any
            // whitespace into a space.
            if c == '\0' || c == '\u{fffd}' || self.is_control(c) {
                continue;
            }
            let c = if is_whitespace(c) { ' ' } else { c };
            if is_chinese_char(c) {
                out.push(' ');
                out.extend(c.to_lowercase());
                out.push(' ');
            } else {
                out.extend(c.to_lowercase());
            }
        }
        out
    }

    /// Split on whitespace (dropped) and around punctuation (kept alone).
    fn pre_tokenize<'a>(&self, text: &'a str) -> Vec<&'a str> {
        let mut words = Vec::new();
        let mut start = None;
        for (i, c) in text.char_indices() {
            if c.is_whitespace() {
                if let Some(s) = start.take() {
                    words.push(&text[s..i]);
                }
            } else if self.is_punctuation(c) {
                if let Some(s) = start.take() {
                    words.push(&text[s..i]);
                }
                words.push(&text[i..i + c.len_utf8()]);
            } else if start.is_none() {
                start = Some(i);
            }
        }
        if let Some(s) = start {
            words.push(&text[s..]);
        }
        words
    }

    fn wordpiece(&self, word: &[char], ids: &mut Vec<u32>) {
        if word.len() > self.max_input_chars_per_word {
            ids.push(self.unk_id);
            return;
        }
        let mut pieces = Vec::new();
        let mut start = 0;
        let mut piece = String::new();
        while start < word.len() {
            let mut end = word.len();
            let mut found = None;
            while start < end {
                piece.clear();
                if start > 0 {
                    piece.push_str("##");
                }
                piece.extend(&word[start..end]);
                if let Some(&id) = self.vocab.get(&piece) {
                    found = Some(id);
                    break;
                }
                end -= 1;
            }
            match found {
                Some(id) => pieces.push(id),
                None => {
                    ids.push(self.unk_id);
                    return;
                }
            }
            start = end;
        }
        ids.extend(pieces);
    }

    fn is_control(&self, c: char) -> bool {
        !matches!(c, '\t' | '\n' | '\r') && in_ranges(&self.other, c)
    }

    fn is_punctuation(&self, c: char) -> bool {
        c.is_ascii_punctuation() || in_ranges(&self.punctuation, c)
    }
}

fn is_whitespace(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r') || c.is_whitespace()
}

fn is_chinese_char(c: char) -> bool {
    matches!(
        c as u32,
        0x4E00..=0x9FFF
            | 0x3400..=0x4DBF
            | 0x20000..=0x2A6DF
            | 0x2A700..=0x2B73F
            | 0x2B740..=0x2B81F
            | 0x2B920..=0x2CEAF
            | 0xF900..=0xFAFF
            | 0x2F800..=0x2FA1F
    )
}

/// `ranges` is sorted and non-overlapping.
fn in_ranges(ranges: &[(u32, u32)], c: char) -> bool {
    let c = c as u32;
    match ranges.binary_search_by(|&(first, _)| first.cmp(&c)) {
        Ok(_) => true,
        Err(0) => false,
        Err(i) => c <= ranges[i - 1].1,
    }
}
