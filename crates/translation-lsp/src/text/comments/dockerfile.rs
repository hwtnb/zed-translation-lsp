use super::Comment;

// A comment is a line whose first character after its indent is `#`. Read
// instructions only as far as that takes: the lines they continue on, and the
// here-documents that follow them, whose lines are not comments.
pub(super) fn parse(text: &str) -> Vec<Comment> {
    let mut comments = Vec::new();
    let mut lines = Lines { text, offset: 0 };
    let mut comment = |start: usize, line: &str| {
        let end = start + line.len();
        comments.push(Comment::line(end - line.trim_start().len()..end, 1));
    };
    // Parser directives come first, after a shebang if there is one.
    let mut directives = true;
    let mut escape = '\\';
    while let Some((start, line)) = lines.next() {
        let content = line.trim_start();
        if content.starts_with('#') {
            if let Some(value) = directive(content).filter(|_| directives) {
                if let Some(value) = value.filter(|value| matches!(*value, "\\" | "`")) {
                    escape = value.chars().next().unwrap_or(escape);
                }
            } else if !(start == 0 && content.starts_with("#!")) {
                directives = false;
                comment(start, line);
            }
            continue;
        }
        directives = false;
        let mut instruction = String::new();
        let mut continues = add_line(&mut instruction, content, escape);
        while continues && let Some((start, line)) = lines.next() {
            let content = line.trim_start();
            if content.starts_with('#') {
                comment(start, line);
            } else if !content.is_empty() {
                continues = add_line(&mut instruction, line, escape);
            }
        }
        for (strip_tabs, delimiter) in here_documents(&instruction) {
            for (_, line) in lines.by_ref() {
                let line = if strip_tabs {
                    line.trim_start_matches('\t')
                } else {
                    line
                };
                if line == delimiter {
                    break;
                }
            }
        }
    }
    comments
}

struct Lines<'a> {
    text: &'a str,
    offset: usize,
}

impl<'a> Iterator for Lines<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let start = self.offset;
        let rest = self.text.get(start..).filter(|rest| !rest.is_empty())?;
        let length = rest.find(['\r', '\n']).unwrap_or(rest.len());
        let (line, ending) = rest.split_at(length);
        self.offset += length
            + if ending.starts_with("\r\n") {
                2
            } else {
                ending.len().min(1)
            };
        Some((start, line))
    }
}

// The value of the escape directive, or `None` for another directive.
fn directive(comment: &str) -> Option<Option<&str>> {
    let (name, value) = comment[1..].split_once('=')?;
    if value.is_empty() {
        return None;
    }
    let (name, value) = (name.trim(), value.trim());
    if name.eq_ignore_ascii_case("escape") {
        return Some(Some(value));
    }
    ["syntax", "check"]
        .iter()
        .any(|known| name.eq_ignore_ascii_case(known))
        .then_some(None)
}

// Adds a line without its continuation character, and tells whether the
// instruction continues on a later line.
fn add_line(instruction: &mut String, line: &str, escape: char) -> bool {
    let continued = line
        .trim_end_matches([' ', '\t'])
        .strip_suffix(escape)
        .filter(|line| !line.ends_with(escape));
    instruction.push_str(continued.unwrap_or(line));
    continued.is_some()
}

// The here-documents that follow an instruction, in order: whether leading tabs
// are stripped from the line that ends each one, and that line.
fn here_documents(instruction: &str) -> Vec<(bool, String)> {
    let mut words = instruction.split_whitespace();
    let mut keyword = words.next().unwrap_or_default();
    if keyword.eq_ignore_ascii_case("onbuild") {
        keyword = words.next().unwrap_or_default();
    }
    let mut found = Vec::new();
    if !["run", "copy", "add"]
        .iter()
        .any(|known| keyword.eq_ignore_ascii_case(known))
    {
        return found;
    }
    let mut characters = instruction.chars().peekable();
    // A here-document is a word that starts with `<<`, after digits if any.
    let mut starts_word = true;
    while let Some(character) = characters.next() {
        match character {
            '\\' => {
                characters.next();
            }
            '\'' => {
                characters.by_ref().find(|&next| next == '\'');
            }
            '"' => {
                while let Some(next) = characters.next()
                    && next != '"'
                {
                    if next == '\\' {
                        characters.next();
                    }
                }
            }
            '<' if starts_word && characters.next_if_eq(&'<').is_some() => {
                let strip_tabs = characters.next_if_eq(&'-').is_some();
                while characters
                    .next_if(|&next| next == ' ' || next == '\t')
                    .is_some()
                {}
                let mut delimiter = String::new();
                while let Some(next) = characters.next_if(|next| !next.is_whitespace()) {
                    match next {
                        '\\' => delimiter.extend(characters.next()),
                        '\'' => {
                            delimiter.extend(characters.by_ref().take_while(|&next| next != '\''))
                        }
                        '"' => {
                            while let Some(next) = characters.next()
                                && next != '"'
                            {
                                delimiter.push(if next == '\\' {
                                    characters.next().unwrap_or(next)
                                } else {
                                    next
                                });
                            }
                        }
                        _ => delimiter.push(next),
                    }
                }
                // A third `<` makes the operator a here-string.
                if !delimiter.is_empty() && !delimiter.contains('<') {
                    found.push((strip_tabs, delimiter));
                }
                starts_word = true;
                continue;
            }
            _ => {}
        }
        starts_word = character.is_whitespace() || starts_word && character.is_ascii_digit();
    }
    found
}
