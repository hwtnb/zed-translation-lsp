use std::ops::Range;

use icu_segmenter::{WordSegmenter, options::WordBreakInvariantOptions};

/// The most words a selection may have to be looked up as a term.
const MAX_WORDS: usize = 3;
/// The most characters of a term. A longer token, such as minified code, is
/// translated as text.
const MAX_CHARS: usize = 64;
/// The characters of the term's line kept on each side of it.
const LINE_CONTEXT_CHARS: usize = 200;
const SENTENCE_ENDS: [char; 6] = ['.', '!', '?', '。', '！', '？'];

/// Whether a selection is a word, short phrase or identifier to look up rather
/// than text to translate: up to three words on one line that do not end a
/// sentence.
pub(crate) fn is_term(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty()
        && !text.contains(['\n', '\r'])
        && text.chars().nth(MAX_CHARS).is_none()
        && !text.ends_with(SENTENCE_ENDS)
        && (1..=MAX_WORDS).contains(&words(text))
}

/// The words of `text` by the Unicode word boundaries, which also divide
/// languages written without spaces, such as Japanese, Chinese and Thai.
fn words(text: &str) -> usize {
    WordSegmenter::new_auto(WordBreakInvariantOptions::default())
        .segment_str(text)
        .iter_with_word_type()
        .filter(|(_, word_type)| word_type.is_word_like())
        .count()
}

/// The line of `text` that `term` lies on, which tells the sense the term has
/// there. A long line is cut to the characters next to the term, and only
/// those are read.
pub(crate) fn line(text: &str, term: Range<usize>) -> &str {
    let start = term.start - context(text[..term.start].chars().rev());
    let end = term.end + context(text[term.end..].chars());
    text[start..end].trim()
}

/// The bytes of the characters of the line that `chars` goes along, up to
/// [`LINE_CONTEXT_CHARS`] of them.
fn context(chars: impl Iterator<Item = char>) -> usize {
    chars
        .take_while(|character| !matches!(character, '\n' | '\r'))
        .take(LINE_CONTEXT_CHARS)
        .map(char::len_utf8)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn up_to_three_words_on_one_line_that_end_no_sentence_are_terms() {
        for term in [
            "retain",
            "  retain\n",
            "retain_completed",
            "isDirty",
            "TranslationProvider::translate",
            "take over",
            "in the end",
            "It works",
            "however,",
            "保持する",
            "翻訳プロバイダー",
            "コンフィギュレーション",
            "設定を保存",
            "翻译结果",
            "การแปล",
            &"x".repeat(64),
        ] {
            assert!(is_term(term), "{term}");
        }
        for text in [
            "",
            " \n ",
            "take it over now",
            "retain\ncompleted",
            "It works.",
            "Really?",
            "動く。",
            "設定を保存する",
            "翻訳結果をホバーに表示する",
            "-> ::",
            &"x".repeat(65),
        ] {
            assert!(!is_term(text), "{text}");
        }
    }

    #[test]
    fn the_line_of_the_term_is_cut_to_the_characters_next_to_it() {
        let text = "first\r\n  // Hover waiters retain independent cancellation.  \nlast";
        let start = text.find("retain").unwrap();
        assert_eq!(
            line(text, start..start + 6),
            "// Hover waiters retain independent cancellation."
        );
        assert_eq!(line("retain", 0..6), "retain");

        let before = "あ".repeat(250);
        let after = "い".repeat(250);
        let text = format!("{before}term{after}\nnext");
        let start = before.len();
        let cut = line(&text, start..start + 4);
        assert_eq!(cut, format!("{}term{}", "あ".repeat(200), "い".repeat(200)));
    }
}
