use std::borrow::Cow;

use tree_sitter::Parser;

use super::{Comment, Kind, LineKind};

pub(super) fn parse(text: &str) -> Vec<Comment> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_c_sharp::LANGUAGE.into())
        .expect("Bundled C# grammar must match the Tree-sitter runtime");
    let source = parser_source(text);
    let Some(tree) = parser.parse(source.as_ref(), None) else {
        return Vec::new();
    };
    let mut comments = Vec::new();
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        if !node.is_error() {
            if node.kind() == "comment" {
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
                comments.push(Comment { range, kind });
            }
            if cursor.goto_first_child() {
                continue;
            }
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return comments;
            }
        }
    }
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
