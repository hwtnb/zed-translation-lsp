use std::{borrow::Cow, ops::Range};

use lsp_types::Uri;
use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_span::SourceType;
use rust_lexer::{DocStyle, FrontmatterAllowed, TokenKind};

mod c_family;
mod csharp;
mod python;
mod syntax;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Rust(Option<DocStyle>),
    JavaScript(u8),
    CSharp(u8),
    CFamily(&'static str),
    Python,
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
            // rustc's lexer terminates line comments at LF. Preserve all byte
            // offsets when accepting the editor's standalone CR and CRLF lines.
            let source = if text.contains('\r') {
                Cow::Owned(text.replace('\r', "\n"))
            } else {
                Cow::Borrowed(text)
            };
            let mut start = rust_lexer::strip_shebang(&source).unwrap_or(0);
            let mut comments = Vec::new();
            for token in rust_lexer::tokenize(&source[start..], FrontmatterAllowed::Yes) {
                let end = start + token.len as usize;
                let kind = match token.kind {
                    TokenKind::LineComment { doc_style } => {
                        Some(Kind::Line(LineKind::Rust(doc_style)))
                    }
                    TokenKind::BlockComment {
                        doc_style,
                        terminated,
                    } => Some(Kind::Block {
                        opening: if doc_style.is_some() { 3 } else { 2 },
                        closed: terminated,
                    }),
                    _ => None,
                };
                if let Some(kind) = kind {
                    comments.push(Comment {
                        range: start..end,
                        kind,
                    });
                }
                start = end;
            }
            return Self(comments);
        }

        let Some(source_type) = source_type(language_id, uri) else {
            return Self(Vec::new());
        };
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, text, source_type).parse();
        Self(
            parsed
                .program
                .comments
                .iter()
                .map(|comment| Comment {
                    range: comment.span.start as usize..comment.span.end as usize,
                    kind: if comment.is_line() {
                        Kind::Line(LineKind::JavaScript(
                            (comment.content_span().start - comment.span.start) as u8,
                        ))
                    } else {
                        Kind::Block {
                            opening: if comment.is_jsdoc() { 3 } else { 2 },
                            closed: true,
                        }
                    },
                })
                .collect(),
        )
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
                Kind::Line(LineKind::JavaScript(opening) | LineKind::CSharp(opening))
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

fn source_type(language_id: &str, uri: &Uri) -> Option<SourceType> {
    let (typescript, jsx) = match language_id {
        "javascript" | "javascriptreact" => (false, true),
        "typescript" => (true, false),
        "typescriptreact" => (true, true),
        _ => return None,
    };
    // Zed's JavaScript grammar accepts JSX, including in .js files. Oxc supplies
    // the URI's module/declaration rules, including .mjs/.cjs/.mts/.cts/.d.ts.
    let source = SourceType::from_path(uri.path().as_str()).unwrap_or_default();
    Some(
        source
            .with_typescript(typescript && !source.is_typescript())
            .with_javascript(!typescript)
            .with_jsx(jsx),
    )
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
    fn c_family_excludes_literals_and_stops_before_ambiguous_error_recovery() {
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
