use tree_sitter::{Language, Node, Parser};

use super::Comment;

pub(super) enum Recovery {
    SkipError,
    StopAtLiteral,
}

pub(super) fn parse(
    source: &[u8],
    language: Language,
    recovery: Recovery,
    mut classify: impl FnMut(Node<'_>) -> Option<Comment>,
) -> Vec<Comment> {
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .expect("Bundled grammar must match the Tree-sitter runtime");
    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };
    let mut comments = Vec::new();
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        if node.is_error() {
            let fragment = &source[node.byte_range()];
            // Recovery may place apparent comments after an unfinished literal.
            // Without a trustworthy token boundary, keep only earlier comments.
            if matches!(recovery, Recovery::StopAtLiteral)
                && (fragment.contains(&b'"')
                    || fragment.contains(&b'\'')
                    || fragment.windows(2).any(|pair| pair == b"/*"))
            {
                return comments;
            }
        } else {
            if let Some(comment) = classify(node) {
                comments.push(comment);
            } else if cursor.goto_first_child() {
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
