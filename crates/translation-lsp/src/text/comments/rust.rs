use super::{Comment, Kind, LineKind};
use rust_lexer::{FrontmatterAllowed, TokenKind};
use std::borrow::Cow;

pub(super) fn parse(text: &str) -> Vec<Comment> {
    // rustc's lexer terminates line comments at LF. Preserve all byte
    // offsets when accepting the editor's standalone CR and CRLF lines.
    let source = if text.contains('\r') {
        Cow::Owned(text.replace('\r', "\n"))
    } else {
        Cow::Borrowed(text)
    };
    let mut start = rust_lexer::strip_shebang(&source).unwrap_or(0);
    let mut comments = Vec::new();
    for token in rust_lexer::tokenize(&source[start..], FrontmatterAllowed::Yes) {
        let end = start + token.len as usize;
        let kind = match token.kind {
            TokenKind::LineComment { doc_style } => Some(Kind::Line(LineKind::Rust(doc_style))),
            TokenKind::BlockComment {
                doc_style,
                terminated,
            } => Some(Kind::Block {
                opening: if doc_style.is_some() { 3 } else { 2 },
                closed: terminated,
            }),
            _ => None,
        };
        if let Some(kind) = kind {
            comments.push(Comment {
                range: start..end,
                kind,
            });
        }
        start = end;
    }
    comments
}
