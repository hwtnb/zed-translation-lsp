use std::{borrow::Cow, ops::Range};

use tree_sitter::Node;

use super::{Comment, Kind, LineKind, syntax};

pub(super) fn parse(text: &str) -> Vec<Comment> {
    let mut source = Cow::Borrowed(text.as_bytes());
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\r' && text.as_bytes().get(index + 1) != Some(&b'\n') {
            source.to_mut()[index] = b'\n';
        }
    }
    syntax::parse(
        &source,
        tree_sitter_python::LANGUAGE.into(),
        syntax::Recovery::StopAtLiteral,
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
            if node.kind() != "expression_statement" || node.has_error() || !is_docstring(node) {
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

fn is_docstring(statement: Node<'_>) -> bool {
    let Some(parent) = statement.parent() else {
        return false;
    };
    let is_body = parent.kind() == "module"
        || parent.kind() == "block"
            && parent.parent().is_some_and(|owner| {
                matches!(owner.kind(), "class_definition" | "function_definition")
                    && owner.child_by_field_name("body") == Some(parent)
            });
    is_body
        && parent
            .named_children(&mut parent.walk())
            .find(|child| child.kind() != "comment")
            == Some(statement)
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
