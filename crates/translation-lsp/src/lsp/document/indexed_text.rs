use std::{iter, ops::Range as ByteRange};

use lsp_types::{Position, Range};

pub(super) struct IndexedText {
    text: String,
    line_starts: Vec<usize>,
}

impl IndexedText {
    pub fn new(text: String) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(line_breaks(text.as_bytes(), 0));
        Self { text, line_starts }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn offset(&self, position: Position) -> Result<usize, &'static str> {
        let line = position.line as usize;
        let start = *self
            .line_starts
            .get(line)
            .ok_or("Position line is outside the document")?;
        let end = match self.line_starts.get(line + 1) {
            Some(&next) => {
                let bytes = self.text.as_bytes();
                if bytes[next - 1] == b'\n' && next >= 2 && bytes[next - 2] == b'\r' {
                    next - 2
                } else {
                    next - 1
                }
            }
            None => self.text.len(),
        };
        let mut units = 0;
        for (offset, character) in self.text[start..end].char_indices() {
            if units == position.character {
                return Ok(start + offset);
            }
            units += character.len_utf16() as u32;
            if units > position.character {
                return Err("Position splits a UTF-16 surrogate pair");
            }
        }
        if units == position.character {
            Ok(end)
        } else {
            Err("Position character is outside the line")
        }
    }

    pub fn range(&self, range: Range) -> Result<ByteRange<usize>, &'static str> {
        if range.start > range.end {
            return Err("Range start must not follow its end");
        }
        Ok(self.offset(range.start)?..self.offset(range.end)?)
    }

    pub fn position(&self, offset: usize) -> Result<Position, &'static str> {
        if !self.text.is_char_boundary(offset)
            || (offset > 0
                && self.text.as_bytes().get(offset - 1) == Some(&b'\r')
                && self.text.as_bytes().get(offset) == Some(&b'\n'))
        {
            return Err("Offset is not an LSP position boundary");
        }
        let line = self.line_starts.partition_point(|&start| start <= offset) - 1;
        let character = self.text[self.line_starts[line]..offset]
            .encode_utf16()
            .count();
        Ok(Position::new(
            u32::try_from(line).map_err(|_| "Position line is outside the document")?,
            u32::try_from(character).map_err(|_| "Position character is outside the line")?,
        ))
    }

    pub fn replace(&mut self, range: Range, replacement: &str) -> Result<(), &'static str> {
        let bytes = self.range(range)?;
        let removed = bytes.len();
        // Include both splice boundaries. Do not cut a CRLF at the far edge.
        // Byte scanning is safe even if this extra byte belongs to UTF-8 prose.
        let start = bytes.start.saturating_sub(1);
        let mut end = (bytes.end + 1).min(self.text.len());
        if end > 0
            && self.text.as_bytes()[end - 1] == b'\r'
            && self.text.as_bytes().get(end) == Some(&b'\n')
        {
            end += 1;
        }
        let first = self.line_starts.partition_point(|&offset| offset <= start);
        let after = self.line_starts.partition_point(|&offset| offset <= end);
        self.text.replace_range(bytes, replacement);
        let end = end - removed + replacement.len();
        if removed != replacement.len() {
            for offset in &mut self.line_starts[after..] {
                *offset = *offset - removed + replacement.len();
            }
        }
        let starts = line_breaks(&self.text.as_bytes()[start..end], start);
        self.line_starts.splice(first..after, starts);
        Ok(())
    }
}

fn line_breaks(bytes: &[u8], base: usize) -> impl Iterator<Item = usize> + '_ {
    let mut offset = 0;
    iter::from_fn(move || {
        while offset < bytes.len() {
            let byte = bytes[offset];
            offset += 1;
            if byte == b'\r' {
                if bytes.get(offset) == Some(&b'\n') {
                    offset += 1;
                }
                return Some(base + offset);
            }
            if byte == b'\n' {
                return Some(base + offset);
            }
        }
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference_offset(text: &str, position: Position) -> Result<usize, &'static str> {
        let bytes = text.as_bytes();
        let mut line = 0;
        let mut start = 0;
        let mut end = 0;
        while end < bytes.len() {
            if matches!(bytes[end], b'\n' | b'\r') {
                if line == position.line {
                    break;
                }
                let crlf = bytes[end] == b'\r' && bytes.get(end + 1) == Some(&b'\n');
                end += if crlf { 2 } else { 1 };
                start = end;
                line += 1;
            } else {
                end += 1;
            }
        }
        if line != position.line {
            return Err("Position line is outside the document");
        }

        let mut units = 0;
        for (offset, character) in text[start..end].char_indices() {
            if units == position.character {
                return Ok(start + offset);
            }
            units += character.len_utf16() as u32;
            if units > position.character {
                return Err("Position splits a UTF-16 surrogate pair");
            }
        }
        if units == position.character {
            Ok(end)
        } else {
            Err("Position character is outside the line")
        }
    }

    fn reference_position(text: &str, offset: usize) -> Result<Position, &'static str> {
        if !text.is_char_boundary(offset)
            || (offset > 0
                && text.as_bytes().get(offset - 1) == Some(&b'\r')
                && text.as_bytes().get(offset) == Some(&b'\n'))
        {
            return Err("Offset is not an LSP position boundary");
        }
        let mut position = Position::new(0, 0);
        let mut characters = text[..offset].chars().peekable();
        while let Some(character) = characters.next() {
            match character {
                '\r' | '\n' => {
                    if character == '\r' && characters.peek() == Some(&'\n') {
                        characters.next();
                    }
                    position.line += 1;
                    position.character = 0;
                }
                _ => position.character += character.len_utf16() as u32,
            }
        }
        Ok(position)
    }

    fn assert_matches(indexed: &IndexedText, expected: &str) {
        assert_eq!(indexed.as_str(), expected);
        for offset in 0..=expected.len() + 1 {
            assert_eq!(
                indexed.position(offset),
                reference_position(expected, offset),
                "byte {offset} in {expected:?}"
            );
        }
        let lines = expected
            .bytes()
            .filter(|&b| b == b'\r' || b == b'\n')
            .count()
            + 2;
        for line in 0..lines {
            for character in 0..=expected.encode_utf16().count() + 1 {
                let position = Position::new(line as u32, character as u32);
                assert_eq!(
                    indexed.offset(position),
                    reference_offset(expected, position),
                    "{position:?} in {expected:?}"
                );
            }
        }
        assert_eq!(
            indexed.offset(Position::new(u32::MAX, 0)),
            reference_offset(expected, Position::new(u32::MAX, 0))
        );
    }

    fn points(text: &str) -> Vec<(usize, Position)> {
        (0..=text.len())
            .filter_map(|offset| {
                reference_position(text, offset)
                    .ok()
                    .map(|position| (offset, position))
            })
            .collect()
    }

    #[test]
    fn indexed_positions_match_full_scan_reference() {
        for text in [
            "",
            "a",
            "\n",
            "\r",
            "\r\n",
            "\r\n\r\n",
            "\n\r",
            "\r\r\n",
            "前🚀x\r\n\tsecond `name`\rlast\n",
            "a\u{2028}🚀\u{2029}b",
        ] {
            assert_matches(&IndexedText::new(text.into()), text);
        }
    }

    #[test]
    fn replacements_at_every_boundary_preserve_line_index() {
        let mut corpus = vec![String::new()];
        let mut level = vec![String::new()];
        for _ in 0..3 {
            level = level
                .iter()
                .flat_map(|prefix| {
                    ["a", "\r", "\n", "🚀", "前"].map(|suffix| format!("{prefix}{suffix}"))
                })
                .collect();
            corpus.extend(level.clone());
        }
        for text in corpus {
            let positions = points(&text);
            for &(start, start_position) in &positions {
                for &(end, end_position) in &positions {
                    if start > end {
                        continue;
                    }
                    for replacement in ["", "x", "\r", "\n", "\r\n", "\n\r", "🚀", "\r🚀\n"] {
                        let mut indexed = IndexedText::new(text.clone());
                        let mut expected = text.clone();
                        indexed
                            .replace(Range::new(start_position, end_position), replacement)
                            .unwrap();
                        expected.replace_range(start..end, replacement);
                        assert_matches(&indexed, &expected);
                    }
                }
            }
        }
    }

    #[test]
    fn edit_sequences_join_split_and_shift_existing_lines() {
        for original in ["a\rX\nb", "a\r\nb\nc\rd", "前🚀\r\nnext\r", "", "last\n"] {
            let mut indexed = IndexedText::new(original.into());
            let mut expected = original.to_owned();
            let mut state = 53_u64;
            for step in 0..180 {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                let positions = points(&expected);
                let left = (state as usize) % positions.len();
                let right = ((state >> 32) as usize) % positions.len();
                let (start, start_position) = positions[left.min(right)];
                let (end, end_position) = positions[left.max(right)];
                let replacement =
                    ["", "x", "\r", "\n", "\r\n", "前🚀", "x\r\ny\r", "\n\r"][step % 8];
                indexed
                    .replace(Range::new(start_position, end_position), replacement)
                    .unwrap();
                expected.replace_range(start..end, replacement);
                assert_matches(&indexed, &expected);
            }
        }
    }

    #[test]
    fn rejected_ranges_leave_text_and_index_unchanged() {
        let original = "前🚀\r\nnext";
        let mut indexed = IndexedText::new(original.into());
        for range in [
            Range::new(Position::new(0, 2), Position::new(0, 3)),
            Range::new(Position::new(3, 0), Position::new(3, 0)),
            Range::new(Position::new(1, 2), Position::new(0, 0)),
        ] {
            assert!(indexed.replace(range, "changed").is_err());
            assert_matches(&indexed, original);
        }
    }
}
