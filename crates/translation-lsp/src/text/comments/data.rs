use std::borrow::Cow;

use granit_parser::{Scanner, StrInput, TokenType};
use toml_parser::{Source, lexer::TokenKind};

use super::{Comment, Kind};

// Comments in these formats are decided by tokens alone, so read tokens and
// leave the structure unchecked. Errors in the structure then hide nothing.
pub(super) fn parse(language_id: &str, text: &str) -> Vec<Comment> {
    match language_id {
        "yaml" | "dockercompose" => yaml(text),
        "toml" => toml(text),
        _ => json(text),
    }
}

fn json(text: &str) -> Vec<Comment> {
    let bytes = text.as_bytes();
    let line_end = |from: usize| {
        bytes[from..]
            .iter()
            .position(|byte| matches!(byte, b'\r' | b'\n'))
            .map_or(bytes.len(), |length| from + length)
    };
    let mut comments = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        match bytes[index] {
            // An unclosed string ends at its line, as JSON strings cannot span
            // lines. Single quotes are not JSON, but nothing inside is a comment.
            quote @ (b'"' | b'\'') => {
                let in_line = |at: usize| !matches!(bytes.get(at), None | Some(b'\r' | b'\n'));
                index += 1;
                while in_line(index) {
                    let byte = bytes[index];
                    index += 1;
                    if byte == quote {
                        break;
                    }
                    // A backslash hides the next character, but not the line break.
                    if byte == b'\\' && in_line(index) {
                        index += 1;
                    }
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'/') => {
                index = line_end(index);
                comments.push(Comment::line(start..index, 2));
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                // An unclosed block comment hides the rest of the document.
                let Some(length) = text[index + 2..].find("*/") else {
                    break;
                };
                index += length + 4;
                comments.push(Comment {
                    range: start..index,
                    kind: Kind::Block {
                        opening: 2,
                        closed: true,
                    },
                });
            }
            _ => index += 1,
        }
    }
    comments
}

fn toml(text: &str) -> Vec<Comment> {
    // The lexer ends an unclosed one-line string at LF only. Preserve all byte
    // offsets when accepting the editor's standalone CR and CRLF lines.
    let source = if text.contains('\r') {
        Cow::Owned(text.replace('\r', "\n"))
    } else {
        Cow::Borrowed(text)
    };
    let mut comments = Vec::new();
    let mut offset = 0;
    'lex: loop {
        for token in Source::new(&source[offset..]).lex() {
            let span = offset + token.span().start()..offset + token.span().end();
            match token.kind() {
                TokenKind::Comment => comments.push(Comment::line(span, 1)),
                // The lexer keeps a quote inside a bare token, as in `r"# x"`,
                // as part of that token. Read on from the quote instead, so
                // that the text after it stays a string.
                TokenKind::Atom => {
                    if let Some(quote) = source[span.clone()].find(['"', '\'']) {
                        offset = span.start + quote;
                        continue 'lex;
                    }
                }
                _ => {}
            }
        }
        return comments;
    }
}

fn yaml(text: &str) -> Vec<Comment> {
    // The scanner reports its first error after the comments before it.
    Scanner::new(StrInput::new(text))
        .map_while(Result::ok)
        .filter(|token| matches!(token.token_type(), TokenType::Comment(_)))
        .filter_map(|token| token.span().byte_range())
        .map(|range| Comment::line(range, 1))
        .collect()
}
