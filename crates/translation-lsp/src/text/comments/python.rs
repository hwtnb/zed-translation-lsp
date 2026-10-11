use std::{borrow::Cow, collections::HashSet, ops::Range, time::Instant};

use tree_sitter::Node;

use super::{Comment, Kind, LineKind, syntax};

pub(super) fn parse(text: &str, deadline: Instant) -> Option<Vec<Comment>> {
    let mut source = Cow::Borrowed(text.as_bytes());
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\r' && text.as_bytes().get(index + 1) != Some(&b'\n') {
            source.to_mut()[index] = b'\n';
        }
    }
    // The statement that would be the docstring of each body met so far. A body
    // is met before its statements, so a statement is looked up here instead of
    // being searched for among the comments that precede it.
    let mut first_statements = HashSet::new();
    syntax::parse(
        &source,
        tree_sitter_python::LANGUAGE.into(),
        syntax::Recovery::StopAtLiteral,
        deadline,
        |node| {
            if node.kind() == "comment" {
                if node.start_byte() == 0 && text.starts_with("#!") {
                    return None;
                }
                let mut range = node.byte_range();
                if text[range.clone()].ends_with('\r') {
                    range.end -= 1;
                }
                return Some(Comment {
                    range,
                    kind: Kind::Line(LineKind::Python),
                });
            }
            if let Some(statement) = first_statement(node) {
                first_statements.insert(statement.id());
            }
            if node.kind() != "expression_statement"
                || node.has_error()
                || !first_statements.contains(&node.id())
            {
                return None;
            }
            let expression = sole_expression(node)?;
            let contents = docstring_contents(expression, text)?;
            Some(Comment {
                range: expression.byte_range(),
                kind: Kind::Docstring(contents),
            })
        },
    )
}

// The first statement of a module, or of the body of a class or a function.
fn first_statement(owner: Node<'_>) -> Option<Node<'_>> {
    let body = match owner.kind() {
        "module" => owner,
        "class_definition" | "function_definition" => owner
            .child_by_field_name("body")
            .filter(|body| body.kind() == "block")?,
        _ => return None,
    };
    body.named_children(&mut body.walk())
        .find(|child| child.kind() != "comment")
}

fn sole_expression(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    let mut children = node
        .named_children(&mut cursor)
        .filter(|child| child.kind() != "comment");
    let first = children.next()?;
    children.next().is_none().then_some(first)
}

fn docstring_contents(mut node: Node<'_>, text: &str) -> Option<Vec<Range<usize>>> {
    while node.kind() == "parenthesized_expression" {
        node = sole_expression(node)?;
    }
    match node.kind() {
        "string" => Some(vec![string_content(node, text)?]),
        "concatenated_string" => node
            .named_children(&mut node.walk())
            .filter(|child| child.kind() != "comment")
            .map(|child| string_content(child, text))
            .collect(),
        _ => None,
    }
}

fn string_content(node: Node<'_>, text: &str) -> Option<Range<usize>> {
    if node.kind() != "string" {
        return None;
    }
    let start = node
        .child(0)
        .filter(|child| child.kind() == "string_start")?;
    let end = node
        .child(node.child_count() - 1)
        .filter(|child| child.kind() == "string_end" && !child.is_missing())?;
    let opening = &text[start.byte_range()];
    let prefix = opening.trim_end_matches(['\'', '"']);
    matches!(prefix, "" | "r" | "R" | "u" | "U").then_some(start.end_byte()..end.start_byte())
}
