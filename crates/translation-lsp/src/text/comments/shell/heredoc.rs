//! Here-documents: the word after `<<`, and the bodies that follow the line.

use super::{Position, Reader};

pub(super) struct HereDocument {
    delimiter: Vec<u8>,
    /// `<<-`, which ignores the tabs that lead a line.
    tabs: bool,
    /// A quoted delimiter, which keeps a backslash at the end of a line.
    quoted: bool,
}

impl Reader<'_> {
    /// Reads the word after `<<`, which ends the body once its quotes are
    /// removed.
    pub(super) fn here_document(&mut self) -> Option<()> {
        if self.peek(0) == Some(b'\\') && self.line_break(self.at + 1) > 0 {
            return None;
        }
        let tabs = self.peek(0) == Some(b'-');
        self.at += usize::from(tabs);
        while matches!(self.peek(0), Some(b' ' | b'\t')) {
            self.at += 1;
        }
        let start = self.at;
        let mut delimiter = Vec::new();
        let mut quoted = false;
        let mut double = false;
        while let Some(byte) = self.peek(0) {
            self.at += 1;
            match byte {
                b'"' => {
                    quoted = true;
                    double = !double;
                }
                // bash keeps an expansion as it is written, and other shells
                // do not.
                b'$' | b'`' => return None,
                b'\\' => {
                    let length = self.line_break(self.at);
                    let escaped = self.peek(0);
                    if length > 0 {
                        self.at += length;
                    } else if double && !matches!(escaped, Some(b'$' | b'`' | b'"' | b'\\')) {
                        delimiter.push(byte);
                    } else {
                        quoted = true;
                        delimiter.extend(escaped);
                        self.at += 1;
                    }
                }
                _ if double => delimiter.push(byte),
                b'\'' => {
                    quoted = true;
                    let end = self.bytes[self.at..self.limit]
                        .iter()
                        .position(|&byte| byte == b'\'')
                        .map_or(self.limit, |length| self.at + length);
                    delimiter.extend_from_slice(&self.bytes[self.at..end]);
                    self.at = end + 1;
                }
                // zsh takes a group into the word, and bash rejects it.
                b'(' => return None,
                b' ' | b'\t' | b'\n' | b'\r' | b';' | b'&' | b'|' | b')' | b'<' | b'>' => {
                    self.at -= 1;
                    break;
                }
                _ => delimiter.push(byte),
            }
        }
        if self.at > start {
            self.here_documents.push(HereDocument {
                delimiter,
                tabs,
                quoted,
            });
            if self.stream.position == Position::Target {
                self.stream.position = Position::Redirected;
            }
        }
        Some(())
    }

    /// Skips the bodies of the here-documents that the line before began.
    pub(super) fn bodies(&mut self) -> Option<()> {
        let first = self.stream.here_documents;
        for index in first..self.here_documents.len() {
            let HereDocument {
                delimiter,
                tabs,
                quoted,
            } = &self.here_documents[index];
            // The lines that a backslash at the end has joined so far.
            let mut joined = Vec::new();
            while self.at < self.limit {
                let start = self.at;
                let end = self.line_end(start);
                self.at = end + self.line_break(end);
                let line = &self.bytes[start..end];
                let backslashes = line.iter().rev().take_while(|&&byte| byte == b'\\');
                if !quoted && end < self.limit && backslashes.count() % 2 == 1 {
                    joined.extend_from_slice(&line[..line.len() - 1]);
                    continue;
                }
                let whole = if joined.is_empty() {
                    line
                } else {
                    joined.extend_from_slice(line);
                    joined.as_slice()
                };
                let leading = whole.iter().take_while(|&&byte| byte == b'\t').count();
                let body = &whole[if *tabs { leading } else { 0 }..];
                if body == delimiter.as_slice() {
                    break;
                }
                // Inside `$( )`, bash also ends the body at a line that starts
                // with the delimiter and holds `)`, and reads the rest of that
                // line as commands.
                if self.stream.substitution
                    && body
                        .strip_prefix(delimiter.as_slice())
                        .is_some_and(|rest| rest.contains(&b')'))
                {
                    if !joined.is_empty() {
                        return None;
                    }
                    self.at = end - (body.len() - delimiter.len());
                    break;
                }
                joined.clear();
            }
        }
        self.here_documents.truncate(first);
        Some(())
    }
}
