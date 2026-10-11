use std::{ops::ControlFlow, time::Instant};

use tree_sitter::{Language, Node, ParseOptions, ParseState, Parser};

use super::Comment;

pub(super) enum Recovery {
    SkipError,
    StopAtLiteral,
}

pub(super) fn parse(
    source: &[u8],
    language: Language,
    recovery: Recovery,
    deadline: Instant,
    mut classify: impl FnMut(Node<'_>) -> Option<Comment>,
) -> Option<Vec<Comment>> {
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .expect("Bundled grammar must match the Tree-sitter runtime");
    let mut late = false;
    let mut progress = |_: &ParseState| {
        late = Instant::now() >= deadline;
        if late {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let tree = parser.parse_with_options(
        &mut |offset, _| source.get(offset..).unwrap_or_default(),
        None,
        Some(ParseOptions::new().progress_callback(&mut progress)),
    );
    if late {
        return None;
    }
    let Some(tree) = tree else {
        return Some(Vec::new());
    };
    let mut comments = Vec::new();
    let mut cursor = tree.walk();
    loop {
        // The limit is for the whole reading, so it holds during the walk too.
        if Instant::now() >= deadline {
            return None;
        }
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
                return Some(comments);
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
                return Some(comments);
            }
        }
    }
}
