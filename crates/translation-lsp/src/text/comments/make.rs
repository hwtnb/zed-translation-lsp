use super::{Comment, shell};

/// Reads a makefile as GNU Make 4.3 and later do. Outside variable references
/// and function calls such as `$(...)`, a `#` that no backslash escapes starts
/// a comment, which a backslash at the end of a line continues on the next
/// line. The body of `define` is a value. A recipe line, which starts with a
/// tab after a rule until a line that is no rule, goes to the shell: its
/// comments are those of a shell script, after the `@`, `-`, and `+` that Make
/// reads at its start.
pub(super) fn parse(text: &str) -> Vec<Comment> {
    let bytes = text.as_bytes();
    let mut comments = Vec::new();
    let mut in_rule = false;
    let mut defines = 0usize;
    let mut index = if text.starts_with('\u{feff}') { 3 } else { 0 };
    while index < bytes.len() {
        let line = logical_line(bytes, index);
        let content = &bytes[index..line];
        let next = next_line(bytes, line);
        if defines > 0 {
            match directive(content) {
                Some(b"endef") => defines -= 1,
                Some(b"define") => defines += 1,
                _ => {}
            }
        } else if content.first() == Some(&b'\t') && in_rule {
            let prefix = content[1..]
                .iter()
                .take_while(|byte| matches!(byte, b'@' | b'-' | b'+' | b' ' | b'\t'))
                .count();
            let start = index + 1 + prefix;
            for mut comment in shell::parse(&text[start..line]) {
                comment.range = comment.range.start + start..comment.range.end + start;
                comments.push(comment);
            }
        } else {
            let (comment, kind) = read(content);
            if let Some(offset) = comment {
                comments.push(Comment::line(index + offset..line, 1));
            }
            match kind {
                Line::Blank | Line::Conditional => {}
                Line::Rule => in_rule = true,
                Line::Define => {
                    defines = 1;
                    in_rule = false;
                }
                Line::Other => in_rule = false,
            }
        }
        index = next;
    }
    comments
}

/// What a line that is no recipe line does to the lines after it.
enum Line {
    /// Empty, or a comment alone.
    Blank,
    /// `ifeq`, `else`, `endif`, and the like, which keep a rule open.
    Conditional,
    Rule,
    Define,
    Other,
}

/// The end of the logical line from `from`: a backslash that no backslash
/// escapes at the end of a line joins the next one.
fn logical_line(bytes: &[u8], from: usize) -> usize {
    let mut end = from;
    loop {
        end = first_line_end(bytes, end);
        let line = &bytes[from..end];
        let backslashes = line.iter().rev().take_while(|byte| **byte == b'\\').count();
        if backslashes % 2 == 0 || end >= bytes.len() {
            return end;
        }
        end = next_line(bytes, end);
    }
}

/// The end of the physical line from `from`, before its line break.
fn first_line_end(bytes: &[u8], from: usize) -> usize {
    let end = bytes[from..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(bytes.len(), |length| from + length);
    if end > from && bytes[end - 1] == b'\r' {
        end - 1
    } else {
        end
    }
}

fn next_line(bytes: &[u8], end: usize) -> usize {
    match bytes.get(end) {
        Some(b'\r') if bytes.get(end + 1) == Some(&b'\n') => end + 2,
        Some(_) => end + 1,
        None => end,
    }
}

/// The first word of a line, when it is `define`, `endef`, or a conditional,
/// after `override`, `export`, or `private`.
fn directive(line: &[u8]) -> Option<&[u8]> {
    let mut rest = line.trim_ascii_start();
    loop {
        let length = rest
            .iter()
            .position(|byte| byte.is_ascii_whitespace())
            .unwrap_or(rest.len());
        let word = &rest[..length];
        match word {
            b"override" | b"export" | b"private" => rest = rest[length..].trim_ascii_start(),
            b"define" | b"endef" | b"ifeq" | b"ifneq" | b"ifdef" | b"ifndef" | b"else"
            | b"endif" => return Some(word),
            _ => return None,
        }
    }
}

/// Where the comment of a line that is no recipe line starts, and what the line is.
fn read(line: &[u8]) -> (Option<usize>, Line) {
    let mut index = 0;
    let mut colon = false;
    let mut equals = false;
    let mut comment = None;
    while index < line.len() {
        match line[index] {
            b'$' => {
                index += 1;
                // A reference runs to the paren that closes it; `$x` and `$$`
                // are two bytes.
                if let Some(&open @ (b'(' | b'{')) = line.get(index) {
                    let close = if open == b'(' { b')' } else { b'}' };
                    let mut depth = 0usize;
                    while index < line.len() {
                        if line[index] == open {
                            depth += 1;
                        } else if line[index] == close {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        index += 1;
                    }
                }
                index += 1;
            }
            b'#' => {
                let backslashes = line[..index]
                    .iter()
                    .rev()
                    .take_while(|byte| **byte == b'\\')
                    .count();
                if backslashes % 2 == 0 {
                    comment = Some(index);
                    break;
                }
                index += 1;
            }
            b'=' if !colon => {
                equals = true;
                index += 1;
            }
            b':' if !colon && !equals => {
                // `:=`, `::=`, and `:::=` assign.
                let colons = line[index..]
                    .iter()
                    .take_while(|byte| **byte == b':')
                    .count();
                if line.get(index + colons) == Some(&b'=') {
                    equals = true;
                } else {
                    colon = true;
                }
                index += colons;
            }
            // After the `;` of a rule comes its recipe, for the shell.
            b';' if colon => break,
            _ => index += 1,
        }
    }
    let code = &line[..comment.unwrap_or(line.len())];
    let kind = if code.trim_ascii().is_empty() {
        Line::Blank
    } else {
        match directive(code) {
            Some(b"define") => Line::Define,
            Some(b"endef") => Line::Other,
            Some(_) => Line::Conditional,
            None if equals => Line::Other,
            None if colon => Line::Rule,
            None => Line::Other,
        }
    };
    (comment, kind)
}
