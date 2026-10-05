use std::borrow::Cow;

use super::{Comment, Kind, LineKind, syntax};

pub(super) fn parse(text: &str) -> Vec<Comment> {
    syntax::parse(
        &parser_source(text),
        tree_sitter_c_sharp::LANGUAGE.into(),
        syntax::Recovery::SkipError,
        |node| {
            if node.kind() != "comment" {
                return None;
            }
            let range = node.byte_range();
            let token = &text[range.clone()];
            let kind = if token.starts_with("//") {
                Kind::Line(LineKind::CSharp(if token.starts_with("///") {
                    3
                } else {
                    2
                }))
            } else {
                Kind::Block {
                    opening: if token.starts_with("/**") { 3 } else { 2 },
                    closed: token.ends_with("*/"),
                }
            };
            Some(Comment { range, kind })
        },
    )
}

fn parser_source(text: &str) -> Cow<'_, [u8]> {
    let mut source = Cow::Borrowed(text.as_bytes());
    for (offset, character) in text.char_indices() {
        if matches!(character, '\u{85}' | '\u{2028}' | '\u{2029}') {
            // The grammar ends line comments only at CR/LF. Keep original byte
            // offsets while making the remaining C# line terminators visible.
            let bytes = source.to_mut();
            bytes[offset] = b'\n';
            bytes[offset + 1..offset + character.len_utf8()].fill(b' ');
        }
    }
    source
}
