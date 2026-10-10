use pwsh_parser::tokenize;

use super::{Comment, Kind, checked};

pub(super) fn parse(text: &str) -> Vec<Comment> {
    let tokens = tokenize(text).tokens;
    let ranges = tokens
        .iter()
        .filter(|token| token.kind.is_comment())
        .map(|token| token.span.start..token.span.end)
        .filter(|range| {
            let source = text.get(range.clone()).unwrap_or_default();
            let shebang = range.start == 0 && source.starts_with("#!");
            let requires = source
                .get(1..9)
                .is_some_and(|name| name.eq_ignore_ascii_case("requires"));
            !shebang && !(requires && source.starts_with('#'))
        });
    // The lexer ends an unclosed block comment at the end of the text. It is
    // not a comment yet, and nothing follows it.
    checked(text, ranges, |source| {
        Kind::hash_line(source).or_else(|| {
            (source.len() >= 4 && source.starts_with("<#") && source.ends_with("#>")).then_some(
                Kind::Block {
                    opening: 2,
                    closed: true,
                },
            )
        })
    })
}
