mod javascript;
mod rust;
#[cfg(test)]
use javascript::source_type;
use std::ops::Range;

use lsp_types::Uri;
use rust_lexer::DocStyle;

mod c_family;
mod csharp;
mod data;
mod python;
mod syntax;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Rust(Option<DocStyle>),
    JavaScript(u8),
    CSharp(u8),
    CFamily(&'static str),
    Python,
    Data(u8),
}

enum Kind {
    Line(LineKind),
    Block { opening: u8, closed: bool },
    Docstring(Vec<Range<usize>>),
}

struct Comment {
    range: Range<usize>,
    kind: Kind,
}

// Store only source byte spans and syntax kinds. Bodies and LSP coordinates are
// produced for the requested group, rather than for every comment in a document.
pub(crate) struct Comments(Vec<Comment>);

impl Comments {
    pub fn parse(language_id: &str, uri: &Uri, text: &str) -> Self {
        if language_id == "python" {
            return Self(python::parse(text));
        }
        if matches!(language_id, "c" | "cpp") {
            return Self(c_family::parse(language_id, text));
        }
        if language_id == "csharp" {
            return Self(csharp::parse(text));
        }
        if language_id == "rust" {
            return Self(rust::parse(text));
        }
        if matches!(language_id, "json" | "jsonc" | "yaml" | "toml") {
            return Self(data::parse(language_id, text));
        }

        Self(javascript::parse(language_id, uri, text))
    }

    // A group is a range of comment indices, not a copy of the source or body.
    pub fn at(&self, text: &str, offset: usize) -> Option<Range<usize>> {
        let index = self
            .0
            .partition_point(|comment| comment.range.start <= offset)
            .checked_sub(1)?;
        let comment = &self.0[index];
        let line_end = matches!(comment.kind, Kind::Line(kind)
            if offset == comment.range.end
                && text[offset..].chars().next().is_none_or(|character| line_break(character, kind)));
        if offset > comment.range.end
            || (offset == comment.range.end && !line_end)
            || matches!(comment.kind, Kind::Block { closed: false, .. })
        {
            return None;
        }
        let mut group = index..index + 1;
        while group.start > 0 && self.adjacent(text, group.start - 1, group.start) {
            group.start -= 1;
        }
        while group.end < self.0.len() && self.adjacent(text, group.end - 1, group.end) {
            group.end += 1;
        }
        Some(group)
    }

    pub fn range(&self, group: Range<usize>) -> Range<usize> {
        self.0[group.start].range.start..self.0[group.end - 1].range.end
    }

    pub fn body(&self, text: &str, group: Range<usize>) -> String {
        let mut body = String::new();
        for index in group.clone() {
            let comment = &self.0[index];
            if index > group.start {
                let gap = &text[self.0[index - 1].range.end..comment.range.start];
                // Adjacent groups contain exactly one physical newline, followed
                // by the next comment's indent. Keep that original newline.
                body.push_str(&gap[..newline_len(gap).unwrap()]);
            }
            let opening = match comment.kind {
                Kind::Line(LineKind::Rust(style)) => {
                    if style.is_some() {
                        3
                    } else {
                        2
                    }
                }
                Kind::Docstring(ref contents) => {
                    for range in contents {
                        body.push_str(&text[range.clone()]);
                    }
                    continue;
                }
                Kind::Line(LineKind::Python) => 1,
                Kind::Line(LineKind::CFamily(prefix)) => prefix.len() as u8,
                Kind::Line(
                    LineKind::JavaScript(opening)
                    | LineKind::CSharp(opening)
                    | LineKind::Data(opening),
                )
                | Kind::Block { opening, .. } => opening,
            } as usize;
            let closing = if matches!(comment.kind, Kind::Block { .. }) {
                2
            } else {
                0
            };
            let end = comment.range.end - closing;
            let content = &text[(comment.range.start + opening).min(end)..end];
            body.push_str(content.strip_prefix(' ').unwrap_or(content));
        }
        body
    }

    fn adjacent(&self, text: &str, left: usize, right: usize) -> bool {
        let (Kind::Line(left_kind), Kind::Line(right_kind)) =
            (&self.0[left].kind, &self.0[right].kind)
        else {
            return false;
        };
        if left_kind != right_kind {
            return false;
        }
        let indent = |comment: &Comment| {
            let prefix = &text[..comment.range.start];
            let line_start = prefix
                .rfind(|character| line_break(character, *left_kind))
                .map_or(0, |offset| {
                    offset + text[offset..].chars().next().unwrap().len_utf8()
                });
            &text[line_start..comment.range.start]
        };
        let left_indent = indent(&self.0[left]);
        let right_indent = indent(&self.0[right]);
        if left_indent != right_indent || !left_indent.chars().all(char::is_whitespace) {
            return false;
        }
        let gap = &text[self.0[left].range.end..self.0[right].range.start];
        newline_len(gap).is_some_and(|length| &gap[length..] == right_indent)
    }
}

fn line_break(character: char, kind: LineKind) -> bool {
    matches!(character, '\r' | '\n')
        || matches!(kind, LineKind::JavaScript(_) | LineKind::CSharp(_))
            && matches!(character, '\u{2028}' | '\u{2029}')
        || matches!(kind, LineKind::CSharp(_)) && character == '\u{85}'
}

fn newline_len(text: &str) -> Option<usize> {
    if text.starts_with("\r\n") {
        Some(2)
    } else {
        text.chars()
            .next()
            .filter(|&character| {
                matches!(character, '\r' | '\n' | '\u{85}' | '\u{2028}' | '\u{2029}')
            })
            .map(char::len_utf8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_at(language: &str, text: &str, marker: &str) -> (String, String) {
        let uri: Uri = "untitled:comment".parse().unwrap();
        let comments = Comments::parse(language, &uri, text);
        let group = comments.at(text, text.find(marker).unwrap()).unwrap();
        (
            text[comments.range(group.clone())].into(),
            comments.body(text, group),
        )
    }

    #[test]
    fn comments_after_macros_and_incomplete_expressions() {
        let uri = "untitled:comment".parse().unwrap();
        for (language, source) in [
            (
                "cpp",
                "#define Q_OBJECT int meta;\nclass Foo { Q_OBJECT\npublic:\n// Real.\n};",
            ),
            (
                "c",
                "#define INLINE_API inline\nstatic INLINE_API int f(void) {\n// Real.\nreturn 0;\n}",
            ),
            ("c", "foo(\n// Real."),
            ("cpp", "foo(\n// Real."),
            ("python", "foo(\n# Real."),
        ] {
            let comments = Comments::parse(language, &uri, source);
            assert_eq!(comments.0.len(), 1, "{language}: {source}");
            assert_eq!(comments.body(source, 0..1), "Real.");
        }
    }

    #[test]
    fn python_comments_and_docstrings_are_distinct_from_other_strings() {
        let source = concat!(
            "# Module comment.\n",
            "\"\"\"Module doc.\"\"\"\n",
            "class Example:\n",
            "    # Class comment.\n",
            "    (\"Class \" \"doc.\")\n",
            "    @decorator\n",
            "    async def read(self):\n",
            "        r\"\"\"Function doc.\"\"\"\n",
            "        value = \"# Not a comment.\"\n",
            "        \"Not a docstring.\"\n",
            "    def other(self):\n",
            "        f\"Not a docstring either.\"\n",
        );
        let uri = "untitled:Example".parse().unwrap();
        let comments = Comments::parse("python", &uri, source);
        let bodies: Vec<_> = (0..comments.0.len())
            .map(|index| comments.body(source, index..index + 1))
            .collect();
        assert_eq!(
            bodies,
            [
                "Module comment.",
                "Module doc.",
                "Class comment.",
                "Class doc.",
                "Function doc."
            ]
        );
        for marker in ["Not a comment", "Not a docstring", "Not a docstring either"] {
            assert!(comments.at(source, source.find(marker).unwrap()).is_none());
        }
    }

    #[test]
    fn python_docstring_delimiters_newlines_and_non_docstring_contexts() {
        let uri = "file:///sample.py".parse().unwrap();
        for ending in ["\n", "\r\n", "\r"] {
            let source = format!(
                "# First.{ending}# Second.{ending}def f():{ending}    \"\"\"Read.{ending}    Carefully.{ending}    \"\"\"{ending}    pass{ending}"
            );
            assert_eq!(
                body_at("python", &source, "First.").1,
                format!("First.{ending}Second.")
            );
            assert_eq!(
                body_at("python", &source, "Read.").1,
                format!("Read.{ending}    Carefully.{ending}    ")
            );
        }
        for literal in [
            "'Doc.'",
            "u'Doc.'",
            "U\"Doc.\"",
            "r'''Doc.'''",
            "R\"\"\"Doc.\"\"\"",
            "((\"Doc.\"))",
            "('Doc' # explanation\n'.')",
        ] {
            let comments = Comments::parse("python", &uri, literal);
            assert_eq!(comments.0.len(), 1, "{literal}");
            assert_eq!(comments.body(literal, 0..1), "Doc.");
            assert!(comments.at(literal, literal.len()).is_none());
        }
        for source in [
            "b'Not a docstring.'",
            "f'Not a docstring.'",
            "('Not ' + 'a docstring.')",
            "('Not a docstring.',)",
            "value = '''Not a docstring.'''",
            "if True:\n    '''Not a docstring.'''",
            "def f():\n    pass\n    '''Not a docstring.'''",
        ] {
            assert!(
                Comments::parse("python", &uri, source).0.is_empty(),
                "{source}"
            );
        }
        let source = "#!/usr/bin/env python\n# Real.\n'''Unclosed\n# Hidden.";
        let comments = Comments::parse("python", &uri, source);
        assert_eq!(comments.0.len(), 1);
        assert_eq!(comments.body(source, 0..1), "Real.");
    }

    #[test]
    fn c_family_groups_and_doxygen_preserve_source_text() {
        for language in ["c", "cpp"] {
            for ending in ["\n", "\r\n", "\r"] {
                let text = format!("/// @brief Retry.{ending}/// Only if needed.{ending}int x;");
                assert_eq!(
                    body_at(language, &text, "Retry.").1,
                    format!("@brief Retry.{ending}Only if needed.")
                );
            }
            for (source, expected) in [
                ("// Read.", "Read."),
                ("//! Read.", "Read."),
                ("int x; ///< Read.", "Read."),
                ("int x; //!< Read.", "Read."),
                ("/** @param x Read. */", "@param x Read. "),
                ("/*! Read. */", "Read. "),
                ("int x; /**< Read. */", "Read. "),
                ("int x; /*!< Read. */", "Read. "),
            ] {
                assert_eq!(body_at(language, source, "Read.").1, expected);
            }
        }
    }

    #[test]
    fn c_family_excludes_literals_and_stops_at_ambiguous_literal_recovery() {
        let uri = "untitled:comment".parse().unwrap();
        for language in ["c", "cpp"] {
            let source = "const char *s = \"// not /* comment */\";\nchar c = '/';\n#if ENABLED\n// Real.\n#endif\n";
            let comments = Comments::parse(language, &uri, source);
            assert_eq!(comments.0.len(), 1);
            assert_eq!(comments.body(source, 0..1), "Real.");
            for broken in [
                "/* unclosed\n// hidden",
                "const char *s = \"unclosed\n// hidden",
            ] {
                let source = format!("// Real.\n{broken}");
                let comments = Comments::parse(language, &uri, &source);
                assert_eq!(comments.0.len(), 1, "{language}: {broken}");
            }
            for ending in ["\n", "\r\n", "\r"] {
                let source = format!("// Continued \\{ending}on this line.{ending}int x;");
                assert_eq!(
                    body_at(language, &source, "Continued").1,
                    format!("Continued \\{ending}on this line.")
                );
            }
            let source = "/// First.\n//! Separate.\n// Ordinary.";
            assert_eq!(body_at(language, source, "First").1, "First.");
            assert_eq!(body_at(language, source, "Separate").1, "Separate.");
        }
        for literal in [
            r###"auto s = R"tag(// not /* comment */)tag";"###,
            r###"auto s = u8R"(// not /* comment */)";"###,
            "auto s = R\"tag(unclosed\n// hidden",
        ] {
            let source = format!("// Real.\n{literal}");
            let comments = Comments::parse("cpp", &uri, &source);
            assert_eq!(comments.0.len(), 1, "{literal}");
        }
    }

    #[test]
    fn csharp_comment_groups_preserve_xml_and_source_newlines() {
        for ending in ["\n", "\r\n", "\r", "\u{85}", "\u{2028}", "\u{2029}"] {
            let text = format!(
                "class A {{}}{ending}    /// <summary>{ending}    /// Retry safely.{ending}    /// </summary>{ending}// Ordinary."
            );
            let (range, body) = body_at("csharp", &text, "Retry safely.");
            assert_eq!(
                range,
                format!("/// <summary>{ending}    /// Retry safely.{ending}    /// </summary>")
            );
            assert_eq!(
                body,
                format!("<summary>{ending}Retry safely.{ending}</summary>")
            );
            assert_eq!(body_at("csharp", &text, "Ordinary.").1, "Ordinary.");
        }
        assert_eq!(
            body_at("csharp", "/** <summary>Retry.</summary> */", "Retry.").1,
            "<summary>Retry.</summary> "
        );
        assert_eq!(
            body_at("csharp", "/* First. */ /* Second. */", "Second.").1,
            "Second. "
        );
    }

    #[test]
    fn csharp_ignores_literal_comments_and_keeps_interpolation_expression_comments() {
        let uri = "file:///Example.cs".parse().unwrap();
        for literal in [
            r####"var s = "https://example.test/* false */";"####,
            r####"var s = @"quote "" // false /* false */";"####,
            "var s = \"\"\"\n// false\n/* false */\n\"\"\";",
            r####"var s = $"https://example.test/{1}";"####,
            r####"var s = $@"https://example.test/{1}";"####,
            "var s = $$\"\"\"\n// false {{1}}\n\"\"\";",
            "var s = @\"first\u{85}// false\u{2028}/* false */\";",
            "var c = '/';",
        ] {
            let source = format!("{literal}\n// Real comment.");
            let comments = Comments::parse("csharp", &uri, &source);
            assert_eq!(comments.0.len(), 1, "{literal}");
            assert_eq!(&source[comments.0[0].range.clone()], "// Real comment.");
        }
        for source in [
            r####"var s = $"prefix {1 /* Real comment. */} // literal";"####,
            r####"var s = $@"prefix {1 /* Real comment. */} // literal";"####,
            "var s = $$\"\"\"\n// literal {{1 /* Real comment. */}}\n\"\"\";",
        ] {
            let comments = Comments::parse("csharp", &uri, source);
            assert_eq!(comments.0.len(), 1);
            assert_eq!(
                body_at("csharp", source, "Real comment.").1,
                "Real comment. "
            );
        }
    }

    #[test]
    fn csharp_skips_ambiguous_errors_and_unclosed_comments_or_strings() {
        let uri = "untitled:Example".parse().unwrap();
        for broken in [
            "class A { void F( {\n// Ambiguous comment.\n}",
            "class A {}\n/* Unclosed comment.",
            "var s = @\"\n// Inside unclosed string.",
            "var s = \"\"\"\n// Inside unclosed string.",
            "var s = \"unclosed // Inside string.",
        ] {
            let source = format!("// Valid before error.\n{broken}");
            let comments = Comments::parse("csharp", &uri, &source);
            assert_eq!(comments.0.len(), 1, "{broken}");
            assert_eq!(comments.body(&source, 0..1), "Valid before error.");
        }
    }

    fn bodies(language: &str, text: &str) -> Vec<String> {
        let uri = "untitled:comment".parse().unwrap();
        let comments = Comments::parse(language, &uri, text);
        let mut bodies = Vec::new();
        let mut index = 0;
        while index < comments.0.len() {
            let group = comments.at(text, comments.0[index].range.start).unwrap();
            bodies.push(comments.body(text, group.clone()));
            index = group.end;
        }
        bodies
    }

    const JSON: &str = concat!(
        "{\n",
        "  // line one\n",
        "  // line two\n",
        "  \"a\": \"// not\", /* block */ \"b\": \"/* not */\"\n",
        "}\n",
    );
    const YAML: &str = concat!(
        "# top\n",
        "# second\n",
        "key: value # trailing\n",
        "url: http://a/#frag\n",
        "text: \"quoted # not\"\n",
        "plain: a#b\n",
        "block: |\n",
        "  # not a comment\n",
        "list:\n",
        "  - item # c\n",
        "  # indented\n",
        "---\n",
        "# second doc\n",
    );
    const TOML: &str = concat!(
        "# top\n",
        "# second\n",
        "a = \"x # not\" # trailing\n",
        "b = 'y # not'\n",
        "c = '''\n",
        "# not\n",
        "'''\n",
        "d = \"\"\"\n",
        "# not\n",
        "\"\"\"\n",
        "[table] # after header\n",
        "arr = [\n",
        "  1, # one\n",
        "]\n",
    );

    #[test]
    fn json_yaml_and_toml_comments_exclude_strings_and_keep_source_newlines() {
        for (language, source, expected) in [
            ("json", JSON, &["line one\nline two", "block "][..]),
            ("jsonc", JSON, &["line one\nline two", "block "]),
            (
                "yaml",
                YAML,
                &["top\nsecond", "trailing", "c", "indented", "second doc"],
            ),
            (
                "toml",
                TOML,
                &["top\nsecond", "trailing", "after header", "one"],
            ),
        ] {
            for ending in ["\n", "\r\n", "\r"] {
                let source = source.replace('\n', ending);
                let expected: Vec<_> = expected
                    .iter()
                    .map(|body| body.replace('\n', ending))
                    .collect();
                assert_eq!(bodies(language, &source), expected, "{language} {ending:?}");
                let uri = "untitled:comment".parse().unwrap();
                let comments = Comments::parse(language, &uri, &source);
                for comment in &comments.0 {
                    let token = &source[comment.range.clone()];
                    assert!(!token.contains("not") && !token.ends_with(['\r', '\n']));
                }
            }
        }
    }

    #[test]
    fn hash_line_groups_break_on_blank_code_and_indent() {
        for (language, code) in [("yaml", "a: 1 # inline"), ("toml", "a = 1 # inline")] {
            for ending in ["\n", "\r\n", "\r"] {
                let text = [
                    "# first  ",
                    "#",
                    "# \tthird🚀",
                    "",
                    "# separate",
                    code,
                    "# alone",
                    "#!keep",
                ]
                .join(ending);
                assert_eq!(
                    bodies(language, &text),
                    [
                        format!("first  {ending}{ending}\tthird🚀"),
                        "separate".to_owned(),
                        "inline".to_owned(),
                        format!("alone{ending}!keep"),
                    ],
                    "{language} {ending:?}"
                );
                let (range, _) = body_at(language, &text, "# first");
                assert_eq!(range, ["# first  ", "#", "# \tthird🚀"].join(ending));
            }
        }
        let text = "a:\n  # nested\n  # group\n    # deeper\n  b: 1\n";
        assert_eq!(bodies("yaml", text), ["nested\ngroup", "deeper"]);
    }

    #[test]
    fn json_and_toml_keep_comments_around_errors_but_never_read_string_contents() {
        for (language, broken, expected) in [
            // An unclosed string ends at its line.
            (
                "json",
                "{\"a\": \"unclosed // hidden\n// after\n\"b\": 1}",
                &["after"][..],
            ),
            // The next line's first quote must not close it and expose that string.
            (
                "json",
                "[\"unclosed\n\"two /* hidden */\", \"three // hidden\"] // after",
                &["after"],
            ),
            (
                "json",
                "{\"a\": \"C:\\dir // hidden\"} // after",
                &["after"],
            ),
            ("json", "{\"a\": \"x\\\" // hidden\"} // after", &["after"]),
            ("json", "{\"a\": \"x\\\\\" // after\n}", &["after"]),
            // A backslash does not carry the string over the line break.
            ("json", "{\"a\": \"unclosed\\\n// after\n}", &["after"]),
            ("json", "{'a': 'single // hidden'} // after", &["after"]),
            (
                "json",
                "{\"a\": 1 \"b\": \"no comma\"} // after",
                &["after"],
            ),
            (
                "json",
                "{\n// before\n\"a\": 1, // same\n\"b\" // key\n}\n// after",
                &["before", "same", "key", "after"],
            ),
            (
                "json",
                "{\"a\": [1, // open\n\"b\": tru // typing",
                &["open", "typing"],
            ),
            ("json", "/* closed */ /* unclosed\n// hidden", &["closed "]),
            ("toml", "a = \"unclosed # hidden\n# after", &["after"]),
            ("toml", "a = 'unclosed # hidden\n# after", &["after"]),
            ("toml", "a = \"x\\\" # hidden\n# after", &["after"]),
            ("toml", "\"k\\q # hidden\" = 1 # after", &["after"]),
            ("toml", "a = \"\"\"unclosed\n# hidden\nb = 1 # hidden", &[]),
            ("toml", "a = '''unclosed\n# hidden\nb = 1 # hidden", &[]),
            ("toml", "a = \"\"\"x\\ # hidden\n\"\"\" # after", &["after"]),
            // The sixth quote opens a string of its own.
            (
                "toml",
                "a = \"\"\"x\"\"\"\"\"\" # hidden\n# after",
                &["after"],
            ),
            // The lexer keeps these quotes inside a bare token.
            ("toml", "a = r\"x # hidden\"\n# after", &["after"]),
            (
                "toml",
                "a = x'''\n# hidden\n''' # after\nb = don't # hidden",
                &["after"],
            ),
            (
                "toml",
                "a = # typing\nb = [1, 2 # open\n[table # header\nc = tru # after",
                &["typing", "open", "header", "after"],
            ),
        ] {
            let prefix = if language == "json" { "//" } else { "#" };
            for ending in ["\n", "\r\n", "\r"] {
                let source = format!("{prefix} Real.\n\n{broken}").replace('\n', ending);
                let mut found = bodies(language, &source);
                assert_eq!(found.remove(0), "Real.", "{broken}");
                assert_eq!(found, expected, "{broken} {ending:?}");
            }
        }
        assert!(bodies("json", "/* unclosed").is_empty());
        assert_eq!(bodies("json", "/**/ /* a\n * b */"), ["", "a\n * b "]);
        assert_eq!(bodies("toml", "\u{feff}# bom\na#b = 1"), ["bom", "b = 1"]);
    }

    #[test]
    fn yaml_keeps_the_comments_before_the_first_scanner_error() {
        for (broken, expected) in [
            ("other: \"unclosed # hidden\n# hidden\n", &[][..]),
            ("other: 'unclosed # hidden\n# hidden\n", &[]),
            ("other: ['a\nk2: v # hidden\n", &[]),
            ("map: {a:[ \"x # hidden\", b: 2}\n", &[]),
            ("other: value\n  bad: |\n    # hidden\n", &[]),
            ("other: |x\n  # hidden\n", &[]),
            ("other\n", &[]),
            ("\tother: v # tab\n", &[]),
            ("other: {a: 1 # open\n", &["open"]),
        ] {
            for ending in ["\n", "\r\n", "\r"] {
                let source = format!("# Real.\nkey: v # same\n{broken}").replace('\n', ending);
                let mut found = bodies("yaml", &source);
                assert_eq!(found.drain(..2).collect::<Vec<_>>(), ["Real.", "same"]);
                assert_eq!(found, expected, "{broken} {ending:?}");
            }
        }
        let uri = "untitled:comment".parse().unwrap();
        let mut source = "key:\n".to_owned();
        for index in 0..120 {
            source.push_str(&format!("  # between {index}\n"));
        }
        source.push_str("  value # last\n");
        assert_eq!(Comments::parse("yaml", &uri, &source).0.len(), 121);
    }

    #[test]
    fn json_on_a_single_line_is_read_in_one_pass() {
        let mut source = String::from("{");
        for index in 0.. {
            if source.len() >= 1 << 20 {
                break;
            }
            source.push_str(&format!("\"key{index}\":\"value {index}\","));
        }
        source.push_str("} // note");
        let started = std::time::Instant::now();
        assert_eq!(bodies("json", &source), ["note"]);
        // One pass takes milliseconds. Looking for the line end again at each
        // string took minutes in a debug build.
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    // Valid YAML: each quoted value continues one column deeper than its key.
    fn yaml_values_continued_one_column_deeper() -> Vec<String> {
        ['"', '\'']
            .into_iter()
            .flat_map(|quote| {
                [
                    format!("# before\nb: {quote}multi\n line{quote} # value\nc: 1 # after\n"),
                    format!(
                        "# before\na:\n  b: {quote}multi\n   line{quote} # value\n  c: 1 # after\n"
                    ),
                ]
            })
            .collect()
    }

    #[test]
    fn yaml_keeps_the_comments_before_a_value_continued_one_column_deeper() {
        for source in yaml_values_continued_one_column_deeper() {
            assert_eq!(bodies("yaml", &source)[0], "before", "{source}");
        }
    }

    #[test]
    #[ignore = "granit-parser 1.3 rejects this valid YAML; resolved by granit-parser 2.0"]
    fn yaml_reads_past_a_value_continued_one_column_deeper() {
        for source in yaml_values_continued_one_column_deeper() {
            assert_eq!(
                bodies("yaml", &source),
                ["before", "value", "after"],
                "{source}"
            );
        }
    }

    #[test]
    fn rust_tokens_ignore_literals_and_shebang_and_wait_for_closed_blocks() {
        let text = concat!(
            "#!/usr/bin/env rust-script // hidden\n",
            "let s = \"// string\";\n",
            "let r = r###\"/* raw */\"###;\n",
            "let b = br#\"// bytes\"#;\n",
            "// real\n/* outer /* nested */ end */\n",
            "/// outer\n//! inner\n/* unfinished\n// still block",
        );
        let uri = "file:///sample.rs".parse().unwrap();
        let comments = Comments::parse("rust", &uri, text);
        assert_eq!(
            comments
                .0
                .iter()
                .map(|comment| &text[comment.range.clone()])
                .collect::<Vec<_>>(),
            [
                "// real",
                "/* outer /* nested */ end */",
                "/// outer",
                "//! inner",
                "/* unfinished\n// still block"
            ]
        );
        for marker in [
            "// hidden",
            "// string",
            "/* raw",
            "// bytes",
            "/* unfinished",
            "// still block",
        ] {
            assert!(
                comments.at(text, text.find(marker).unwrap()).is_none(),
                "{marker}"
            );
        }
        for unfinished in ["let s = \"// hidden", "let r = r##\"/* hidden"] {
            assert!(Comments::parse("rust", &uri, unfinished).0.is_empty());
        }
        assert_eq!(
            body_at("rust", "/* outer /* nested */ end */", "/* outer").1,
            "outer /* nested */ end "
        );
        assert_eq!(
            body_at("rust", "/// outer\n//! inner", "//! inner").1,
            "inner"
        );
    }

    #[test]
    fn cold_line_end_carets_match_only_their_token_at_line_break_or_eof() {
        for (language, ending) in [
            ("rust", "\n"),
            ("rust", "\r\n"),
            ("rust", "\r"),
            ("javascript", "\u{2028}"),
            ("typescript", "\u{2029}"),
            ("csharp", "\r\n"),
            ("csharp", "\u{85}"),
            ("csharp", "\u{2028}"),
            ("csharp", "\u{2029}"),
            ("json", "\n"),
            ("json", "\r\n"),
            ("jsonc", "\r"),
        ] {
            let uri = "untitled:comment".parse().unwrap();
            let text = format!("  // first{ending}  // second");
            let comments = Comments::parse(language, &uri, &text);
            let group = comments.at(&text, text.find(ending).unwrap()).unwrap();
            assert_eq!(comments.body(&text, group), format!("first{ending}second"));
            assert!(comments.at(&text, text.len()).is_some());
            let indent = text.find("  // second").unwrap();
            assert!(comments.at(&text, indent).is_none());
        }
        let uri = "file:///blocks.rs".parse().unwrap();
        let text = "/* left *//* right */";
        let comments = Comments::parse("rust", &uri, text);
        let right = comments.at(text, "/* left */".len()).unwrap();
        assert_eq!(comments.body(text, right), "right ");
        assert!(comments.at(text, text.len()).is_none());
    }

    #[test]
    fn line_groups_preserve_endings_and_break_on_blank_code_kind_and_indent() {
        for (language, ending) in [
            ("rust", "\n"),
            ("rust", "\r\n"),
            ("rust", "\r"),
            ("javascript", "\r\n"),
            ("javascript", "\u{2028}"),
            ("typescript", "\u{2029}"),
            ("csharp", "\r\n"),
            ("csharp", "\u{85}"),
            ("csharp", "\u{2028}"),
            ("csharp", "\u{2029}"),
            ("json", "\n"),
            ("jsonc", "\r\n"),
            ("json", "\r"),
        ] {
            let text = [
                "  // first  ",
                "  //",
                "  // \tthird🚀",
                "",
                "  // separate",
                " // indent",
                " x(); // inline",
                " // alone",
            ]
            .join(ending);
            let (range_text, body) = body_at(language, &text, "// first");
            assert_eq!(
                range_text,
                ["// first  ", "  //", "  // \tthird🚀"].join(ending)
            );
            assert_eq!(body, format!("first  {ending}{ending}\tthird🚀"));
            for marker in ["// separate", "// indent", "// inline", "// alone"] {
                assert_eq!(body_at(language, &text, marker).0, marker);
            }
            let uri = "untitled:comment".parse().unwrap();
            let comments = Comments::parse(language, &uri, &text);
            assert!(comments.at(&text, 0).is_none());
            assert!(
                comments
                    .at(&text, text.find("  // \tthird").unwrap())
                    .is_none()
            );
            assert!(comments.at(&text, text.find(ending).unwrap()).is_some());
        }
        for marker in ["/// outer", "//! inner", "// plain", "//// ordinary"] {
            let text = "/// outer\n//! inner\n// plain\n//// ordinary";
            // Four slashes are ordinary Rust comment content, not a doc delimiter.
            assert_eq!(
                body_at("rust", text, marker).0,
                if marker == "// plain" || marker == "//// ordinary" {
                    "// plain\n//// ordinary"
                } else {
                    marker
                }
            );
        }
    }

    #[test]
    fn bodies_strip_only_one_opening_ascii_space_and_keep_tabs_unicode_and_stars() {
        for language in ["rust", "javascript", "typescript", "typescriptreact"] {
            let text = "/** \t`id` 🚀\r\n * keep  \r\n */";
            assert_eq!(
                body_at(language, text, "/**").1,
                "\t`id` 🚀\r\n * keep  \r\n "
            );
            assert_eq!(body_at(language, "/*\n * next */", "/*").1, "\n * next ");
            assert_eq!(body_at(language, "/**/", "/**/").1, "");
            for (text, expected) in [
                ("//\tkeep", "\tkeep"),
                ("//  keep", " keep"),
                ("//\u{a0}keep", "\u{a0}keep"),
                ("/*\tkeep */", "\tkeep "),
            ] {
                assert_eq!(body_at(language, text, text).1, expected);
            }
        }
        assert_eq!(body_at("rust", "/*! inner */", "/*!").1, "inner ");
    }

    #[test]
    fn js_dialects_ignore_strings_regex_templates_and_jsx_text() {
        for (language, path, typescript, jsx) in [
            ("javascript", "file:///a.js", false, true),
            ("javascript", "file:///a.jsx", false, true),
            ("javascriptreact", "untitled:a", false, true),
            ("typescript", "file:///a.ts", true, false),
            ("typescriptreact", "file:///a.tsx", true, true),
        ] {
            let mut text = "const s = '// string'; const r = /\\/\\/ regex/;\nconst t = `// template ${1 /* expr */}`;\n".to_owned();
            if typescript {
                text.push_str("const n: number = 1;\n");
            }
            if jsx {
                text.push_str(
                    "const view = <div title='// attribute'>// jsx text{/* jsx */}</div>;\n",
                );
            }
            text.push_str("// actual 🚀\n/** doc */\n");
            let uri = path.parse().unwrap();
            let comments = Comments::parse(language, &uri, &text);
            let mut expected = vec!["/* expr */"];
            if jsx {
                expected.push("/* jsx */");
            }
            expected.extend(["// actual 🚀", "/** doc */"]);
            assert_eq!(
                comments
                    .0
                    .iter()
                    .map(|comment| &text[comment.range.clone()])
                    .collect::<Vec<_>>(),
                expected,
                "{language} {path}"
            );
        }
    }

    #[test]
    fn oxc_fatal_errors_keep_prior_comments_without_guessing_later_tokens() {
        let uri = "file:///a.js".parse().unwrap();
        for tail in [
            "const t = `// unclosed\n// later",
            "const s = 'bad\n// later",
            "/* unclosed\n// later",
        ] {
            let text = format!("// before\n{tail}");
            let comments = Comments::parse("javascript", &uri, &text);
            assert_eq!(
                comments
                    .0
                    .iter()
                    .map(|comment| &text[comment.range.clone()])
                    .collect::<Vec<_>>(),
                ["// before"]
            );
            assert!(comments.at(&text, text.find("// later").unwrap()).is_none());
        }
    }

    #[test]
    fn dialect_selection_uses_oxc_path_rules_and_zed_ids() {
        for (id, path, ts, jsx, definition) in [
            ("javascript", "file:///a.jsx", false, true, false),
            ("javascript", "file:///a.cjs", false, true, false),
            ("typescript", "file:///a.d.ts", true, false, true),
            ("typescript", "file:///a.mts", true, false, false),
            (
                "typescript",
                "file:///C:/Users/test%20user/src/example.d.ts",
                true,
                false,
                true,
            ),
            (
                "javascript",
                "file://server/share/example.jsx",
                false,
                true,
                false,
            ),
            ("typescriptreact", "untitled:a", true, true, false),
        ] {
            let source = source_type(id, &path.parse().unwrap()).unwrap();
            assert_eq!(source.is_typescript(), ts);
            assert_eq!(source.is_jsx(), jsx);
            assert_eq!(source.is_typescript_definition(), definition);
        }
        assert!(source_type("markdown", &"file:///a.jsx".parse().unwrap()).is_none());
    }
}
