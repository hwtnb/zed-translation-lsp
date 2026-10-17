use std::{borrow::Cow, time::Instant};

use super::{Comment, Kind, LineKind, syntax};
use crate::language::Language;

pub(super) fn parse(language: Language, text: &str, deadline: Instant) -> Option<Vec<Comment>> {
    let language = if language == Language::C {
        tree_sitter_c::LANGUAGE
    } else {
        tree_sitter_cpp::LANGUAGE
    };
    let mut source = Cow::Borrowed(text.as_bytes());
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\r' && text.as_bytes().get(index + 1) != Some(&b'\n') {
            source.to_mut()[index] = b'\n';
        }
    }
    syntax::parse(
        &source,
        language.into(),
        syntax::Recovery::StopAtLiteral,
        deadline,
        |node| {
            if node.kind() != "comment" {
                return None;
            }
            let mut range = node.byte_range();
            let token = &text[range.clone()];
            let kind = if token.starts_with("//") {
                if token.ends_with('\r') {
                    range.end -= 1;
                }
                let prefix = ["///<", "//!<", "///", "//!", "//"]
                    .into_iter()
                    .find(|prefix| token.starts_with(prefix))
                    .unwrap();
                Kind::Line(LineKind::CFamily(prefix))
            } else {
                let opening = ["/**<", "/*!<", "/**", "/*!", "/*"]
                    .into_iter()
                    .find(|prefix| token.starts_with(prefix))
                    .unwrap()
                    .len() as u8;
                Kind::Block {
                    opening,
                    closed: token.ends_with("*/"),
                }
            };
            Some(Comment { range, kind })
        },
    )
}
