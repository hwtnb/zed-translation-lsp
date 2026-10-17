use super::{Comment, Kind, LineKind};
use lsp_types::Uri;
use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_span::SourceType;

use crate::language::Language;

pub(super) fn parse(language: Language, uri: &Uri, text: &str) -> Vec<Comment> {
    let Some(source_type) = source_type(language, uri) else {
        return Vec::new();
    };
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, text, source_type).parse();
    parsed
        .program
        .comments
        .iter()
        .map(|comment| Comment {
            range: comment.span.start as usize..comment.span.end as usize,
            kind: if comment.is_line() {
                Kind::Line(LineKind::JavaScript(
                    (comment.content_span().start - comment.span.start) as u8,
                ))
            } else {
                Kind::Block {
                    opening: if comment.is_jsdoc() { 3 } else { 2 },
                    closed: true,
                }
            },
        })
        .collect()
}

pub(super) fn source_type(language: Language, uri: &Uri) -> Option<SourceType> {
    let (typescript, jsx) = match language {
        Language::JavaScript => (false, true),
        Language::TypeScript => (true, false),
        Language::TypeScriptReact => (true, true),
        _ => return None,
    };
    // Zed's JavaScript grammar accepts JSX, including in .js files. Oxc supplies
    // the URI's module/declaration rules, including .mjs/.cjs/.mts/.cts/.d.ts.
    let source = SourceType::from_path(uri.path().as_str()).unwrap_or_default();
    Some(
        source
            .with_typescript(typescript && !source.is_typescript())
            .with_javascript(!typescript)
            .with_jsx(jsx),
    )
}
