use std::ops::Range;

use super::{Comment, Kind};

/// Reads XML as its grammar does: `<!-- ... -->` outside tags, CDATA sections,
/// processing instructions, and the declarations of a document type is a
/// comment, also in the internal subset of the document type. An unclosed
/// comment, section, instruction, or tag hides the rest of the document.
pub(super) fn parse(text: &str) -> Vec<Comment> {
    let bytes = text.as_bytes();
    let mut comments = Vec::new();
    let mut index = 0;
    while let Some(found) = find(bytes, index, b"<") {
        index = found;
        let rest = &bytes[index..];
        let end = if rest.starts_with(b"<!--") {
            comment(bytes, index, &mut comments)
        } else if rest.starts_with(b"<![CDATA[") {
            find(bytes, index + 9, b"]]>").map(|end| end + 3)
        } else if rest.starts_with(b"<?") {
            find(bytes, index + 2, b"?>").map(|end| end + 2)
        } else if rest.starts_with(b"<!DOCTYPE") {
            doctype(bytes, index + 9, &mut comments)
        } else if rest.starts_with(b"<!") || rest.get(1).is_some_and(|byte| name_start(*byte)) {
            markup_end(bytes, index + 1)
        } else {
            Some(index + 1)
        };
        let Some(end) = end else {
            break;
        };
        index = end;
    }
    comments
}

fn find(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    bytes
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|found| from + found)
}

/// Whether a byte can start the name of a tag, or is the `/` of an end tag.
fn name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b':' | b'/') || byte >= 0x80
}

/// Adds the comment at `start`, and gives where it ends.
fn comment(bytes: &[u8], start: usize, comments: &mut Vec<Comment>) -> Option<usize> {
    let after = start + 4;
    let end = find(bytes, after, b"-->")?;
    let first = (after + usize::from(bytes.get(after) == Some(&b' '))).min(end);
    comments.push(Comment {
        range: start..end + 3,
        kind: Kind::Contents(vec![Range { start: first, end }]),
    });
    Some(end + 3)
}

/// The end of a tag or declaration: the first `>` outside quotes.
fn markup_end(bytes: &[u8], from: usize) -> Option<usize> {
    let mut index = from;
    loop {
        match bytes.get(index)? {
            b'>' => return Some(index + 1),
            quote @ (b'"' | b'\'') => index = find(bytes, index + 1, &[*quote])? + 1,
            _ => index += 1,
        }
    }
}

/// The end of a document type declaration whose name starts at `from`, with
/// the comments of its internal subset.
fn doctype(bytes: &[u8], from: usize, comments: &mut Vec<Comment>) -> Option<usize> {
    let mut index = from;
    loop {
        match bytes.get(index)? {
            b'>' => return Some(index + 1),
            quote @ (b'"' | b'\'') => index = find(bytes, index + 1, &[*quote])? + 1,
            b'[' => {
                index += 1;
                // The internal subset runs to `]`, outside its declarations.
                loop {
                    let rest = &bytes[index..];
                    if rest.is_empty() {
                        return None;
                    }
                    index = if rest.starts_with(b"]") {
                        break;
                    } else if rest.starts_with(b"<!--") {
                        comment(bytes, index, comments)?
                    } else if rest.starts_with(b"<?") {
                        find(bytes, index + 2, b"?>")? + 2
                    } else if rest.starts_with(b"<!") {
                        markup_end(bytes, index + 2)?
                    } else {
                        index + 1
                    };
                }
                index += 1;
            }
            _ => index += 1,
        }
    }
}
