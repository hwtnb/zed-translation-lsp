use std::{
    thread,
    time::{Duration, Instant},
};

use super::*;

fn parsed(language: &str, uri: &Uri, text: &str) -> Comments {
    Comments::parse(
        Language::from_id(language).unwrap(),
        uri,
        text,
        Duration::from_secs(60),
    )
    .unwrap()
}

fn body_at(language: &str, text: &str, marker: &str) -> (String, String) {
    let uri: Uri = "untitled:comment".parse().unwrap();
    let comments = parsed(language, &uri, text);
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
        let comments = parsed(language, &uri, source);
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
    let comments = parsed("python", &uri, source);
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
        let comments = parsed("python", &uri, literal);
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
        assert!(parsed("python", &uri, source).0.is_empty(), "{source}");
    }
    let source = "#!/usr/bin/env python\n# Real.\n'''Unclosed\n# Hidden.";
    let comments = parsed("python", &uri, source);
    assert_eq!(comments.0.len(), 1);
    assert_eq!(comments.body(source, 0..1), "Real.");
}

#[test]
fn python_reads_statements_after_many_comments_in_proportion_to_the_text() {
    let uri = "untitled:comment".parse().unwrap();
    let text = format!("{}{}", "# c\n".repeat(20_000), "f()\n".repeat(20_000));
    let started = Instant::now();
    assert_eq!(parsed("python", &uri, &text).0.len(), 20_000);
    // One pass takes a fraction of a second in a debug build, and a few
    // seconds on a slow machine. Passing the comments again for each
    // statement took more than a minute.
    assert!(started.elapsed() < Duration::from_secs(20));
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
        let comments = parsed(language, &uri, source);
        assert_eq!(comments.0.len(), 1);
        assert_eq!(comments.body(source, 0..1), "Real.");
        for broken in [
            "/* unclosed\n// hidden",
            "const char *s = \"unclosed\n// hidden",
        ] {
            let source = format!("// Real.\n{broken}");
            let comments = parsed(language, &uri, &source);
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
        let comments = parsed("cpp", &uri, &source);
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
        let comments = parsed("csharp", &uri, &source);
        assert_eq!(comments.0.len(), 1, "{literal}");
        assert_eq!(&source[comments.0[0].range.clone()], "// Real comment.");
    }
    for source in [
        r####"var s = $"prefix {1 /* Real comment. */} // literal";"####,
        r####"var s = $@"prefix {1 /* Real comment. */} // literal";"####,
        "var s = $$\"\"\"\n// literal {{1 /* Real comment. */}}\n\"\"\";",
    ] {
        let comments = parsed("csharp", &uri, source);
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
        let comments = parsed("csharp", &uri, &source);
        assert_eq!(comments.0.len(), 1, "{broken}");
        assert_eq!(comments.body(&source, 0..1), "Valid before error.");
    }
}

fn bodies(language: &str, text: &str) -> Vec<String> {
    let uri = "untitled:comment".parse().unwrap();
    let comments = parsed(language, &uri, text);
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
            let comments = parsed(language, &uri, &source);
            for comment in &comments.0 {
                let token = &source[comment.range.clone()];
                assert!(!token.contains("not") && !token.ends_with(['\r', '\n']));
            }
        }
    }
}

#[test]
fn hash_line_groups_break_on_blank_code_and_indent() {
    for (language, code) in [
        ("yaml", "a: 1 # inline"),
        ("toml", "a = 1 # inline"),
        ("dockercompose", "a: 1 # inline"),
        ("shellscript", "a=1 # inline"),
        ("powershell", "$a = 1 # inline"),
    ] {
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
        // A backslash hides nothing in a literal string.
        ("toml", "a = 'x\\' # same\n# after", &["same", "after"]),
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
    assert_eq!(parsed("yaml", &uri, &source).0.len(), 121);
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
    let started = Instant::now();
    assert_eq!(bodies("json", &source), ["note"]);
    // One pass takes milliseconds. Looking for the line end again at each
    // string took minutes in a debug build.
    assert!(started.elapsed() < Duration::from_secs(5));
}

/// Valid YAML: each quoted value continues one column deeper than its key.
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
fn yaml_reads_past_a_value_continued_one_column_deeper() {
    for source in yaml_values_continued_one_column_deeper() {
        assert_eq!(
            bodies("yaml", &source),
            ["before", "value", "after"],
            "{source}"
        );
    }
}

/// The source of each comment, before grouping.
fn sources(language: &str, uri: &str, text: &str) -> Vec<String> {
    parsed(language, &uri.parse().unwrap(), text)
        .0
        .iter()
        .map(|comment| text[comment.range.clone()].to_owned())
        .collect()
}

fn shell(text: &str) -> Vec<String> {
    sources("shellscript", "untitled:comment", text)
}

#[test]
fn shell_comments_exclude_strings_expansions_and_here_documents() {
    for ending in ["\n", "\r\n", "\r"] {
        let text = [
            "#!/bin/sh",
            "# Leading",
            "# group.",
            "echo \"# string\" '# raw' $'# ansi \\' c' ${x#y} ${x:-# z} $# a#b # inline",
            "echo $(( 1 # 2 )) $[ 1 # 2 ] ; (( x # 3 )) ;# arithmetic",
            "cat <<EOF | grep \"# pattern\" }#x ]#x # operator",
            "# here-document $x $(echo # y",
            ")",
            "EOF",
            "cat <<-'EOF' <<\\B <<\"C D\" # three",
            "\t# quoted here-document",
            "\tEOF",
            "# second",
            "B",
            "# third",
            "C D",
            "cat <<< word # here-string",
            "echo a \\",
            "  | cat # continued",
            "x=$(echo y # nested",
            ") `echo z # backquoted",
            "` \"$(echo \"$(echo # deep",
            ")\" # deeper",
            ")\" <(echo # process",
            ")",
            "if true; then # keyword",
            "  : # indented",
            "fi",
            "#!later",
        ]
        .join(ending);
        assert_eq!(
            shell(&text),
            [
                "# Leading",
                "# group.",
                "# inline",
                "# arithmetic",
                "# operator",
                "# three",
                "# here-string",
                "# continued",
                "# nested",
                "# backquoted",
                "# deep",
                "# deeper",
                "# process",
                "# keyword",
                "# indented",
                "#!later",
            ],
            "{ending:?}"
        );
        assert_eq!(
            body_at("shellscript", &text, "# Leading").1,
            format!("Leading{ending}group.")
        );
    }
}

#[test]
fn shell_reads_unclosed_text_to_the_end_and_checks_no_structure() {
    let all = ["# before", "# inside", "# after"];
    for (text, kept) in [
        // A quotation, an expansion, or a here-document that nothing
        // closes runs to the end.
        (
            "# before\necho \"open\n# inside\necho\n# after\n",
            &all[..1],
        ),
        ("# before\necho 'open\n# inside\necho\n# after\n", &all[..1]),
        (
            "# before\necho $'open\n# inside\necho\n# after\n",
            &all[..1],
        ),
        (
            "# before\necho ${open\n# inside\necho\n# after\n",
            &all[..1],
        ),
        (
            "# before\necho $((open\n# inside\necho\n# after\n",
            &all[..1],
        ),
        ("# before\ncat <<EOF\n# inside\necho\n# after\n", &all[..1]),
        // Commands go on inside a substitution that nothing closes.
        ("# before\necho $(open\n# inside\necho\n# after\n", &all[..]),
        ("# before\necho `open\n# inside\necho\n# after\n", &all[..]),
        // Keywords and parentheses need no partner.
        ("# before\necho a\nfi\n# inside\n)\n# after\n", &all[..]),
        (
            "# before\nif true; then\n# inside\necho\n# after\n",
            &all[..],
        ),
        ("# before\nf() {\n# inside\necho\n# after\n", &all[..]),
        (
            "# before\ncase x in\n# inside\na) echo ;;\n# after\n",
            &all[..],
        ),
        (
            "# before\n# inside\nif ) cat <<EOF\n# not\nEOF\n# after\n",
            &all[..],
        ),
    ] {
        for ending in ["\n", "\r\n", "\r"] {
            let text = text.replace('\n', ending);
            assert_eq!(shell(&text), kept, "{text:?}");
        }
    }
}

#[test]
fn shell_follows_bash_on_what_is_data() {
    for (text, kept) in [
        // A here-document goes on after a line that a backslash joins to
        // the next one, and after a delimiter with a blank behind it.
        ("cat <<EOF\na \\\nEOF\n# not\nEOF\n# 1\n", 1),
        ("cat <<EOF\na \\\\\nEOF\n# 1\n", 1),
        ("cat <<'EOF'\na \\\nEOF\n# 1\n", 1),
        ("cat <<EOF\nEO\\\nF\n# 1\n", 1),
        ("cat <<EOF\nEOF \n# not\nEOF\n# 1\n", 1),
        ("cat <<-EOF\n \tEOF\n\t# not\n\tEOF\n# 1\n", 1),
        ("cat <<E\"O\"F <<''\n# not\nEOF\n# not\n\n# 1\n", 1),
        // A word after the operator is not a comment for its `#`.
        ("cat <<EOF }#x ]#x a#b # 1\n# not\nEOF\n", 1),
        // Inside `$( )`, the body also ends at a line that starts with
        // the delimiter and holds `)`.
        ("x=$(cat <<EOF\n# not\nEOF) # 1\n# 2\n", 2),
        ("( cat <<EOF\n# not\nEOF) # not\nEOF\n) # 1\n", 1),
        // `$((` runs to where its parentheses balance, arithmetic or not.
        (
            "echo $((echo a # not\n) ; echo b) $(( $(echo # 1\n) )) # 2\n",
            2,
        ),
        ("cat >(( x # not )) <(( y # not )) # 1\n", 1),
        ("echo $[ 1 # not ] $[ $(echo # 1\n) ] # 2\n", 2),
        ("for ((i = 0; i < 2; i++)); do (( x # not )); done # 1\n", 1),
        // Without a second `)` right after the first one that balances,
        // `((` opens two subshells.
        ("((echo a); (echo \"b)\")) # 1\n", 1),
        ("(( $(echo # 1\n) ) ; echo ) # 2\n", 2),
        ("if ((( a == b - 2 )) && (( c )) ); then : # 1\nfi\n", 1),
        // The `)` of a `case` pattern does not close `$( )`, and `case`
        // is no keyword everywhere.
        (
            "x=\"$(case $y in a|esac) : # 1\n;; (b|esac) ;& c) ;;& esac)\" # 2\n",
            2,
        ),
        (
            "x=\"$(echo case; for i do case $i in a) : # 1\n;; esac; done)\" # 2\n",
            2,
        ),
        ("x=\"$(time -p case $y in a) : # 1\n;; esac)\" # 2\n", 2),
        ("x=\"$(function f case $y in a) : # 1\n;; esac)\" # 2\n", 2),
        ("x=\"$(f() case $y in a) : # 1\n;; esac)\" # 2\n", 2),
        ("f ( ) ( : # 1\n) # 2\n", 2),
        ("x=$(x=(case a); [[ case == b ]]; echo a >f case) # 1\n", 1),
        (
            "x=$(if a; then case $y in a) case $z in b) ;; esac ;; esac; fi) # 1\n",
            1,
        ),
        // A pattern after `=~` holds `(` and `|`, and a pattern holds
        // groups.
        ("[[ $x =~ (a|b # not ) && $y =~ a|#not ]] # 1\n", 1),
        ("[[ $x =~|#not ]] # 1\n", 1),
        ("[[ $x == @(a| # not ) ]] # 1\n", 1),
        // The subscript of an assignment runs to its `]`.
        ("a[1 # not ]=x b[ # not ]=y # 1\necho a[ # 2\n", 2),
        (
            "x=(a) y[ # not ]=1 # 1\n>f z[ # not ]=1 # 2\nx=1 >f y[ # 3\n",
            3,
        ),
        ("<<< a x[ # not ]=1 # 1\n", 1),
        ("arr=([[ a (b # not ) ]] # 1\n c)#not # 2\n", 2),
        ("arr=([a # not ]=b # 1\n) # 2\n", 2),
        // `$$` is one parameter, so no `$'…'` starts in it.
        ("echo $$'a\\'b' # not ' # 1\n", 1),
        ("echo ${x:-$'a\\'# not'} \"$'a' # not \" # 1\n", 1),
        // A quotation inside `${ }` or a string, and its escapes.
        ("echo ${x:-'}' # not } \"a \\\" # not\" # 1\n", 1),
        // A backslash joins two lines, inside a word too.
        ("echo a\\\n# not\n# 1\n", 1),
        ("x=\"$(ca\\\nse $y in a) : # 1\n;; esac)\" # 2\n", 2),
        ("echo a \\\n\\\n# 1\n", 1),
    ] {
        let expected: Vec<_> = (1..=kept).map(|number| format!("# {number}")).collect();
        for ending in ["\n", "\r\n", "\r"] {
            let text = text.replace('\n', ending);
            assert_eq!(shell(&text), expected, "{text:?}");
        }
    }
}

#[test]
fn shell_takes_no_comment_from_a_pattern_of_zsh() {
    for (text, kept) in [
        ("[[ $x == (#b)(*)=(*) ]] && print $match # 1\n", 1),
        ("print -l *(.N) **/*(#qN) (a|b)# <-> <1-9>(#c2) # 1\n", 1),
        (
            "[[ $x = <-># ]] && print ([[:space:]]|)# (a # not\n) # 1\n",
            1,
        ),
        ("[[ $x == (a|b)# && $y != (c # not\n) ]] && print # 1\n", 1),
        ("print ${(j:,:)x} ${x:#a*} ${#x} $#x ${x%%#*} # 1\n", 1),
        ("(( #x > 1 )) && print $(( [#16] 255 )) # 1\n", 1),
        (
            "case $x in\n((#i)*.bz2|(#i)*.bzip2)\n# 1\n;;\n(#b)(*)x) : # 2\n;|\n(a) : ;;\nesac # 3\n",
            3,
        ),
        // The words of a loop and of an array can hold comments.
        ("for file (\n# 1\n/a # 2\n); do :; done # 3\n", 3),
        ("x=( ${(f)\"$(print a # 1\n)\"} (#i)b* *(/) ) # 2\n", 2),
        // Right after `(`, `)`, or `|`, bash reads a comment where zsh
        // can read a pattern. Neither is returned.
        ("(#not\necho a)#not\necho b |#not\ncat # 1\n", 1),
        ("(# don't\necho a) # 1\n", 1),
        ("x=$(# not\necho a) # 1\n", 1),
        ("echo a;# 1\necho b &# 2\n", 2),
        // For bash, `!#not` is a word.
        ("echo a &!#not\necho b &! # 1\n", 1),
    ] {
        let expected: Vec<_> = (1..=kept).map(|number| format!("# {number}")).collect();
        for ending in ["\n", "\r\n", "\r"] {
            let text = text.replace('\n', ending);
            assert_eq!(shell(&text), expected, "{text:?}");
        }
    }
}

#[test]
fn shell_returns_a_comment_in_backquotes_when_a_line_break_ends_it() {
    for (text, kept) in [
        ("echo `echo a # not` # 1\n", 1),
        ("echo `echo a # 1\n` \"`echo \"a # not\" # 2\n`\" # 3\n", 3),
        ("echo `cat <<EOF\n# not\nEOF\n# 1\n` # 2\n", 2),
        // The backquote ends the command, whatever is open there.
        ("echo `echo 'a` # 1\n", 1),
        // The shell removes some backslashes before it reads the
        // command. Nothing is returned after one of them.
        ("echo `echo a # 1\necho \\\\ # not\n: # not\n` # 2\n", 2),
    ] {
        let expected: Vec<_> = (1..=kept).map(|number| format!("# {number}")).collect();
        for ending in ["\n", "\r\n", "\r"] {
            let text = text.replace('\n', ending);
            assert_eq!(shell(&text), expected, "{text:?}");
        }
    }
}

#[test]
fn shell_stops_where_it_cannot_tell_how_a_shell_reads_on() {
    for text in [
        // bash reads `((` as two subshells here, and reads their text
        // again as commands, which hold a comment or a `case`.
        "((echo a # not\n); (echo b))",
        "((case $x in a) echo ;; esac); echo b)",
        // bash 5.3 reads commands in `${ }`.
        "echo ${ echo a; }",
        // A backslash joins the two halves of an operator.
        "cat <\\\n<EOF\n# not\nEOF",
        "echo \"$\\\n(echo \")\" # not\n)\"",
        // bash keeps the expansion in a delimiter as it is written.
        "cat <<E$x\n# not\nE$x",
        // bash reads the body after the rest of the line.
        "echo $(cat <<EOF)\n# not\nEOF",
        // zsh can begin a here-document inside the group of a pattern,
        // and takes a group into the delimiter of one.
        "echo x(cat <<EOF\n# not\nEOF)",
        "cat <<EOF(a)\n# not\nEOF",
        // zsh reads one pattern here, and bash a command.
        "x=$(case $y in (a)echo ;; esac)",
        // A `)` that bash rejects.
        "x=$(case $y in a) echo ) ;; esac)",
    ] {
        for ending in ["\n", "\r\n", "\r"] {
            let text = format!("# before\n{text}\n# after\n").replace('\n', ending);
            assert_eq!(shell(&text), ["# before"], "{text:?}");
        }
    }
}

#[test]
fn shell_reads_deep_nesting_and_long_input_in_one_pass() {
    let depth = 200_000;
    let megabyte = 1 << 20;
    let nested = |open: &str, inside: &str, close: &str| {
        format!(
            "echo {}{inside}{}\n# note",
            open.repeat(depth),
            close.repeat(depth)
        )
    };
    let line = "x=1; echo \"$x\" | grep a && : # note\n";
    let texts = [
        nested("$(", "", ")"),
        nested("(", ":", ")").replacen("echo ", "", 1),
        nested("$((", "1", "))"),
        nested("${a[", "1", "]}"),
        nested("\"$(", "", ")\""),
        format!(
            "cat {}\n{}# note",
            "<<E ".repeat(depth),
            "E\n".repeat(depth)
        ),
        line.repeat(megabyte / line.len()),
        format!("{}\n# note", "echo \"a\" 'b'; ".repeat(megabyte / 14)),
        // Other things that nest or repeat.
        nested("( ", ":", " )").replacen("echo ", "", 1),
        nested("{ ", ":", "; }").replacen("echo ", "", 1),
        nested("$(case x in a) ", ":", ";; esac)"),
        nested("<(", "", ")"),
        nested("x=(", "", ")"),
        nested("${a:-", "", "}"),
        nested("a[", "", "]").replacen("echo ", "", 1),
        format!(
            "[[ {}a{} ]]\n# note",
            "( ".repeat(depth),
            " )".repeat(depth)
        ),
        format!("cat <<E\n{}b\nE\n# note", "a \\\n".repeat(megabyte / 4)),
        format!("cat <<E\n{}\nE\n# note", "\\".repeat(megabyte)),
        format!(
            "{}\n# note",
            "echo `echo a # note\n`\n".repeat(megabyte / 21)
        ),
        format!("{}\n# note", "(#b)".repeat(megabyte / 4)),
        format!("{}\n# note", "f() { :; }; ".repeat(megabyte / 12)),
        format!("{}\n# note", "<1 ".repeat(megabyte / 3)),
    ];
    let uri = "untitled:comment".parse().unwrap();
    let started = Instant::now();
    for text in &texts {
        let comments = parsed("shellscript", &uri, text);
        let last = comments
            .0
            .last()
            .map(|comment| &text[comment.range.clone()]);
        assert_eq!(last, Some("# note"), "{:?}", &text[..40]);
        assert_eq!(comments.0.len(), text.matches("# note").count());
    }
    // Each takes milliseconds when read once, on a stack that the heap holds.
    // All of them take seconds in a debug build on a slow machine; reading a
    // megabyte again at each piece would take hours.
    assert!(started.elapsed() < Duration::from_secs(60));
}

#[test]
fn shell_ranges_are_bytes_of_the_source_after_other_scripts_and_a_byte_order_mark() {
    for ending in ["\n", "\r\n", "\r"] {
        let text = [
            "greeting=\"日本語😀\" # 挨拶😀",
            "x=$(echo \"é😀\" # 中😀",
            ") # 後",
            "cat <<終 # 区切り",
            "# 本文😀",
            "終",
            "  # 一行目",
            "  # 二行目😀",
        ]
        .join(ending);
        assert_eq!(
            shell(&text),
            [
                "# 挨拶😀",
                "# 中😀",
                "# 後",
                "# 区切り",
                "# 一行目",
                "# 二行目😀"
            ],
            "{ending:?}"
        );
        assert_eq!(
            body_at("shellscript", &text, "# 一行目"),
            (
                format!("# 一行目{ending}  # 二行目😀"),
                format!("一行目{ending}二行目😀")
            )
        );
    }
    // The mark is not part of the first comment, nor of the shebang.
    assert_eq!(shell("\u{feff}# one\necho # two\n"), ["# one", "# two"]);
    assert_eq!(shell("\u{feff}#!/bin/sh\n# one\n"), ["# one"]);
}

#[test]
fn powershell_comments_exclude_strings_requires_and_the_shebang() {
    for ending in ["\n", "\r\n", "\r"] {
        let text = [
            "#!/usr/bin/env pwsh",
            "#Requires -Version 7",
            "# Leading",
            "# group.",
            "  #requires -Modules X",
            "$a = \"# string 日本語\" # inline 😀",
            "<# block",
            "   comment #>",
            "$b = @\"",
            "# here-string 😀",
            "\"@ # after",
            "$c = '# raw' <# inner #> ; $d = $(Get-Date # nested",
            ")",
            "function f { # brace",
            "}",
            "#!later",
        ]
        .join(ending);
        assert_eq!(
            sources("powershell", "file:///a.ps1", &text),
            [
                "# Leading".to_owned(),
                "# group.".to_owned(),
                "# inline 😀".to_owned(),
                format!("<# block{ending}   comment #>"),
                "# after".to_owned(),
                "<# inner #>".to_owned(),
                "# nested".to_owned(),
                "# brace".to_owned(),
                "#!later".to_owned(),
            ],
            "{ending:?}"
        );
        assert_eq!(
            bodies("powershell", &text),
            [
                format!("Leading{ending}group."),
                "inline 😀".to_owned(),
                format!("block{ending}   comment "),
                "after".to_owned(),
                "inner ".to_owned(),
                "nested".to_owned(),
                "brace".to_owned(),
                "!later".to_owned(),
            ],
            "{ending:?}"
        );
        // An unclosed block comment, string, or here-string hides the rest.
        for open in [
            "<# open",
            "$a = \"open",
            "$a = 'open",
            "$a = @\"",
            "$a = @'",
        ] {
            let text = ["# before", open, "# inside", "# after"].join(ending);
            assert_eq!(
                sources("powershell", "file:///a.ps1", &text),
                ["# before"],
                "{open}"
            );
            let uri = "file:///a.ps1".parse().unwrap();
            let comments = parsed("powershell", &uri, &text);
            assert!(comments.at(&text, text.find(open).unwrap()).is_none());
        }
    }
    assert_eq!(
        sources(
            "powershell",
            "file:///a.ps1",
            "\u{feff}#!/usr/bin/env pwsh\n#REQUIRES -Version 7\n# one\n"
        ),
        ["# one"]
    );
    assert_eq!(
        sources("powershell", "file:///a.ps1", "\u{feff}# one\n<##>\n"),
        ["# one", "<##>"]
    );
}

fn powershell(text: &str) -> Vec<String> {
    sources("powershell", "file:///a.ps1", text)
}

#[test]
fn powershell_takes_comments_where_a_token_starts_and_none_from_quoted_text() {
    let cases: &[(&str, &[&str])] = &[
        // After a blank, and after a character that ends every token.
        (
            "a #c1\n(#c2\n)#c3\n{#c4\n}#c5\n,#c6\n;#c7\n|#c8\n&#c9",
            &[
                "#c1", "#c2", "#c3", "#c4", "#c5", "#c6", "#c7", "#c8", "#c9",
            ],
        ),
        // After a string or a block comment that began a token.
        (
            "\"s\"#c1\n's'#c2\n<# b #>#c3\n@(1)#c4\n@{}#c5",
            &["#c1", "#c2", "<# b #>", "#c3", "#c4", "#c5"],
        ),
        // Not inside a word, nor after a backtick. A backtick before a line break continues the line.
        (
            "Write-Host a#n\nhttp://a/#n\n`#n # c1\n$x = 1 `\n  # c2",
            &["# c1", "# c2"],
        ),
        // Two quotes in a row, and a quote after a backtick, do not end a string.
        (
            "'it''s # n' # c1\n\"a \"\"# n\"\" b\" # c2\n\"a `\" # n\" # c3",
            &["# c1", "# c2", "# c3"],
        ),
        // Other quotation marks work as `'` and `"`, also mixed.
        (
            "\u{2018}# n\u{2019} # c1\n\u{201c}# n\u{201d} # c2\n\u{201a}# n\u{201b} # c3\n\u{201e}# n\" # c4\n'# n\u{2019}\u{2019} # n\u{2018} # c5",
            &["# c1", "# c2", "# c3", "# c4", "# c5"],
        ),
        // A here-string starts after a line with blanks only, and ends at a line that starts with its quote and `@`.
        (
            "@\"\n# n\n \"@ # n\n\"@ # c1\n@'\n# n\n\"@ # n\n'@ # c2\n@\u{201c} \t\n# n\n\u{201d}@ # c3\n@\"\n\"@ # c4",
            &["# c1", "# c2", "# c3", "# c4"],
        ),
        // A `$( )` in a string ends where its parentheses do, whatever the quotes in it.
        (
            "\"a $(')' # n\n) b\" # c1\n\"$(\"# n\")\" # c2\n\"$(\"\"# n\"\")\" # c3",
            &["# c1", "# c2", "# c3"],
        ),
        // A `${ }` runs to the first `}` without a backtick, also inside a string.
        (
            "${a # n} # c1\n${b`} # n} # c2\n\"${a # n} # n\" # c3",
            &["# c1", "# c2", "# c3"],
        ),
        // A `$( )` or `@( )` that starts a token holds commands.
        (
            "$x = $(\n# c1\n'# n' ) # c2\n$y = @(\n\"# n\" # c3\n)",
            &["# c1", "# c2", "# c3"],
        ),
        // A `$( )` after other text may hold strings without parentheses.
        (
            "a=$(b 'c' \"d\") # c1\n!$(a) # c2\n-Skip:$($x -lt 7) # c3",
            &["# c1", "# c2", "# c3"],
        ),
        // A backtick escapes a quote, but is part of a type name.
        (
            "cmd `\"$a`\" # c1\ncmd --a=`\"x#n`\" # c2\n[List`1]$x # c3\ncmd `\"x # c4`\"",
            &["# c1", "# c2", "# c3", "# c4`\""],
        ),
        // Blanks of other scripts end a token.
        (
            "a\u{a0}#c1\na\u{3000}#c2\na\u{85}#c3\na\u{b}#c4\na\u{c}#c5",
            &["#c1", "#c2", "#c3", "#c4", "#c5"],
        ),
        // In single quotes, a backtick and `$(` are text. In an argument, `@"` is.
        // A backtick before a blank escapes nothing where a token starts.
        (
            "'a`' # c1\n'a $(' # c2\n`a@\" # n\" # c3\na ` #c4",
            &["# c1", "# c2", "# c3", "#c4"],
        ),
        // A type with `,` and nothing after it that would open a string, and an index with one.
        (
            "[Dictionary[string, int]]$x # c1\n$h[0, 'a # n'] # c2\n[int[]]('a', 'b') # c3\n(1)[0, 'a # n'] # c4\n$x[,'a # n'] # c5",
            &["# c1", "# c2", "# c3", "# c4", "# c5"],
        ),
        // A block comment needs both of its ends.
        (
            "<##> <#\n# n #> # c1\n<#>#> # c2",
            &["<##>", "<#\n# n #>", "# c1", "<#>#>", "# c2"],
        ),
    ];
    for (text, expected) in cases {
        for ending in ["\n", "\r\n", "\r"] {
            let text = text.replace('\n', ending);
            let expected: Vec<_> = expected
                .iter()
                .map(|source| source.replace('\n', ending))
                .collect();
            assert_eq!(powershell(&text), expected, "{text:?}");
        }
    }
}

#[test]
fn powershell_reads_on_as_powershell_does_where_the_text_looks_finished() {
    let cases: &[(&str, &[&str])] = &[
        // `$$` and `@$` are one variable each, so what follows them is no `$( )` or `${ }`.
        (
            "\"$$(\" # c1\n\"$${\" # c2\na$$(b) # c3\n@${ # c4\n}\n@\"\n$$(\n\"@ # c5",
            &["# c1", "# c2", "# c3", "# c4", "# c5"],
        ),
        // In a string, a quote inside `${ }` and a parenthesis after a backtick inside `$( )` are plain text.
        (
            "\"${a\"b} # n\" # c1\n\"$(`)\" # c2\n\"${a$(} # n\" # c3",
            &["# c1", "# c2", "# c3"],
        ),
        // A here-string does not end inside its `$( )` or `${ }`, where a backtick escapes nothing or the `}`.
        (
            "@\"\n$(\n\"@ # n\n`)\n\"@ # c1\n@\"\n${a\n\"@ # n\n`}}\n\"@ # c2",
            &["# c1", "# c2"],
        ),
        // A here-string with text after its opening ends at `"@` on that line, or with the line.
        (
            "@\" x\n# c1\n@' x '@ # c2\n@\" x \u{201d}@ # n \"@ # c3",
            &["# c1", "# c2", "# c3"],
        ),
        // `u{...}` after a backtick names a character, braces and all.
        (
            "cmd `u{41} # c1\n${a`u{41}} # c2\n`u{1F600 # c3",
            &["# c1", "# c2", "# c3"],
        ),
        // `u{...}` inside `${ }` keeps its `}`, and a `$( )` in an argument runs to its last parenthesis.
        ("${a`u{41}#n} # c1\ncmd `a$( # n\n) # c2", &["# c1", "# c2"]),
        // What follows `[name,` is read as usual after the line, and a parameter ends with its word.
        (
            "[T,\n'b # n'] # c1\na$([T,\n'b') # c2\ncmd -a =`\"x # n`\" # c3",
            &["# c1", "# c2", "# n`\" # c3"],
        ),
        // `--%` makes a command take the rest of its line as it is written. Only plain dashes count.
        // The next line is read again, whoever reads the first one.
        (
            "cmd --% a # n \"\n# c1\ncmd -`-`% # n\n# c2\ncmd -'-%' # n\n# c3\ncmd \u{2014}\u{2013}% # c4\n@'\n--%\n'@ # c5",
            &["# c1", "# c2", "# c3", "# c4", "# c5"],
        ),
    ];
    for (text, expected) in cases {
        for ending in ["\n", "\r\n", "\r"] {
            let text = text.replace('\n', ending);
            assert_eq!(&powershell(&text), expected, "{text:?}");
        }
    }
}

#[test]
fn powershell_leaves_out_what_only_one_reading_of_the_text_takes_for_a_comment() {
    // A `#` after other text starts a comment or not, by what the parser
    // expects there. Nothing is taken from the rest of that line.
    let leaves: &[(&str, &[&str])] = &[
        ("$x#n # n\n# c", &["# c"]),
        ("$x = 5#n \"n\" <# n #> # n\n# c", &["# c"]),
        ("cmd a` #n # n\n# c", &["# c"]),
        // A command takes the text right after `${ }` into the same argument.
        ("${a}#n # n\n# c", &["# c"]),
        // A backtick before a blank ends a token or joins the blank to an argument.
        ("$x=` #n # n\n# c", &["# c"]),
        // In an argument, `<#` is plain text.
        ("cmd `a<# n #> # n\n# c", &["#> # n", "# c"]),
        // A NUL, a byte order mark, and a control character after a backtick start an argument.
        ("a\0#n\n \u{feff}#n\n`\u{1c}#n\n# c", &["# c"]),
    ];
    // Where the readings differ for more than a line, and where something
    // is not closed, the comments before that place are the result.
    let stops = [
        // A `#`, `<#`, here-string, or `$( )` after other text is read in two ways.
        "$x#it's\n# n\n' # n",
        "$x=@\"\n# n\n\"@",
        "a<# n #>",
        "a$(b # n\n)",
        "a=$(b ')' c)",
        "a$(b `) c)",
        "a$(${b})",
        "a$(@'\n'@)",
        "a$(--% b)",
        // After a `#` that may have begun a comment, nothing reaches past the end of its line.
        "a#n ${\n}",
        "a#n b$(\n)",
        // What is not closed hides the rest.
        "<# open",
        "\"open",
        "'open",
        "@\"\nopen",
        "${open",
        "\"$(open\"",
        "@\"\n$(\n\"@",
        // After `--%`, a `|` or `&&` ends what the command takes, and a `#` before it may have begun a comment.
        "cmd --% a | b",
        "cmd --% a && b",
        "a#n --% b",
        "cmd --% a --% b",
        // After `[name,`, PowerShell can read the name of an assembly, where a quote opens nothing.
        "[T, a'b] 'x # n'",
        "$x -is[T, a'b] 'x # n'",
        "}[T,#n",
        "[A[b], c'd] 'x # n'",
        "[a`, #n ]",
        "a$([T, a'b'])",
        // In the parameter of a command, a backtick and `$` are text, and `{` opens a block.
        // After `@` inside a word, `${` can be a variable or not.
        "cmd -a=`\"x # n`\"",
        "1>&2-eq$`'s # n'",
        "-foo${a # n} ; ${b`} # n} # n",
        "a@${b`} # n} # n",
        "echo a`\"b`\"",
        "-a`u{41}",
    ];
    for ending in ["\n", "\r\n", "\r"] {
        for (text, expected) in leaves {
            let text = text.replace('\n', ending);
            assert_eq!(&powershell(&text), expected, "{text:?}");
        }
        for stop in stops {
            let text = format!("# before\n{stop}\n# after\n").replace('\n', ending);
            assert_eq!(powershell(&text), ["# before"], "{text:?}");
        }
    }
}

#[test]
fn powershell_takes_a_backtick_before_a_line_break_as_powershell_does() {
    // In a here-string in double quotes and in an argument, a backtick
    // escapes one character: LF, CR alone, or only the CR of CRLF. So the
    // line after it ends the here-string, or starts a token, only after CRLF.
    let cases: &[(&str, [&[&str]; 3])] = &[
        (
            "# before\n$x = @\"\ntext`\n\"@\n# 本文\n\"@\n# after\n",
            [
                &["# before", "# after"],
                &["# before", "# 本文"],
                &["# before", "# after"],
            ],
        ),
        (
            "cmd `a`\n#n # c1\n# c2\n",
            [&["# c1", "# c2"], &["#n # c1", "# c2"], &["# c1", "# c2"]],
        ),
        (
            "cmd a`\n#n # n\n# c\n",
            [&["# c"], &["#n # n", "# c"], &["# c"]],
        ),
        ("a#n b`\n# c\n", [&[], &["# c"], &[]]),
    ];
    for (text, expected) in cases {
        for (ending, expected) in ["\n", "\r\n", "\r"].into_iter().zip(expected) {
            let text = text.replace('\n', ending);
            assert_eq!(&powershell(&text), expected, "{text:?}");
        }
    }
}

#[test]
fn powershell_reads_deep_nesting_and_long_input_in_one_pass() {
    let depth = 200_000;
    let megabyte = 1 << 20;
    let nested = |open: &str, inside: &str, close: &str| {
        format!(
            "{}{inside}{}\n# note",
            open.repeat(depth),
            close.repeat(depth)
        )
    };
    let line = "$x = \"a\" + 'b' | f -g $(h) # note\n";
    let texts = [
        nested("\"$(", "", ")\""),
        nested("(", "1", ")"),
        nested("{", "", "}"),
        nested("@(", "", ")"),
        nested("$(", "", ")"),
        nested("@{a=", "1", "}"),
        nested("a$(", "", ")"),
        line.repeat(megabyte / line.len()),
        format!("{}\n# note", "f \"a\" 'b'; ".repeat(megabyte / 11)),
        format!("{}\n# note", "a#b ".repeat(megabyte / 4)),
        format!("@\"\n{}\n\"@\n# note", "$()\n".repeat(megabyte / 4)),
    ];
    let started = Instant::now();
    for text in &texts {
        let found = powershell(text);
        assert_eq!(found.last().map(String::as_str), Some("# note"));
        assert_eq!(found.len(), text.matches("# note").count());
    }
    // Each takes milliseconds when read once, without a stack. All of them
    // take seconds in a debug build on a slow machine; reading a megabyte
    // again at each piece would take hours.
    assert!(started.elapsed() < Duration::from_secs(60));
}

/// The example of the request, with tabs before the lines that `<<-` strips.
const DOCKERFILE: &str = concat!(
    "# syntax=docker/dockerfile:1\n",
    "# escape=\\\n",
    "# c1\n",
    "FROM a # not\n",
    "RUN echo x \\\n",
    "  # c2\n",
    "  && echo y\n",
    "RUN <<EOF\n",
    "# not\n",
    "echo z\n",
    "EOF\n",
    "ENV A=\"# not\"\n",
    "  # c3\n",
    "COPY <<-'EOT' /x\n",
    "\t# not\n",
    "\tEOT\n",
    "RUN cat <<< \"x\"\n",
    "# c4\n",
    "RUN echo \"a <<EOF b\"\n",
    "# c5\n",
);

#[test]
fn dockerfile_comments_are_whole_lines_outside_directives_and_here_documents() {
    for ending in ["\n", "\r\n", "\r"] {
        assert_eq!(
            bodies("dockerfile", &DOCKERFILE.replace('\n', ending)),
            ["c1", "c2", "c3", "c4", "c5"],
            "{ending:?}"
        );
        let text = [
            "# first  ",
            "#",
            "# \tthird🚀",
            "",
            "# separate",
            "FROM a",
            "  # indented",
            "# alone",
            "#!keep",
        ]
        .join(ending);
        assert_eq!(
            bodies("dockerfile", &text),
            [
                format!("first  {ending}{ending}\tthird🚀"),
                "separate".to_owned(),
                "indented".to_owned(),
                format!("alone{ending}!keep"),
            ],
            "{ending:?}"
        );
    }
    for (text, expected) in [
        // Directives stop at the first comment, blank line, or instruction.
        (
            "# Syntax = x\n#check=skip=all\n# c1\n# escape=`\nFROM a\n",
            &["# c1", "# escape=`"][..],
        ),
        ("# c1\n# syntax=x\nFROM a\n", &["# c1", "# syntax=x"]),
        ("\n# syntax=x\nFROM a\n", &["# syntax=x"]),
        ("FROM a\n# syntax=x\n", &["# syntax=x"]),
        ("# other=x\n# syntax=x\n", &["# other=x", "# syntax=x"]),
        ("# syntax=\n# c1\n", &["# syntax=", "# c1"]),
        ("#!/usr/bin/env docker\n# syntax=x\n# c1\n", &["# c1"]),
        ("\u{feff}# syntax=x\n# c1\n", &["# c1"]),
        // The escape directive names the continuation character.
        (
            "# escape=`\nRUN a `\n  <<EOF\n# not\nEOF\n# c1\n",
            &["# c1"],
        ),
        (
            "# escape=`\nRUN a \\\n# c1\nRUN <<EOF\n# not\nEOF\n",
            &["# c1"],
        ),
        // Comments and blank lines between continued lines do not end the instruction.
        (
            "RUN a \\\n\n  # c1\n  <<EOF\n# not\nEOF\n# c2\n",
            &["# c1", "# c2"],
        ),
        (
            "RUN a \\\\\n# c1\nRUN b \\  \n# c2\n <<X\n# not\nX\n# c3",
            &["# c1", "# c2", "# c3"],
        ),
        ("RUN <<E\\\nOF\n# not\nEOF\n# c1\n", &["# c1"]),
        // Here-documents are read in order, each to its own line.
        (
            "RUN <<A 3<<-\"B C\" x\n# not\nA\n\t# not\n\tB C\n# c1\n",
            &["# c1"],
        ),
        ("RUN <<A\n# not\nB\n A\nA \n# not\nA\n# c1\n", &["# c1"]),
        ("COPY <<-EOT /x\n  # not\n  EOT\n# not\n", &[]),
        ("RUN <<EOF\n# not\n", &[]),
        ("ONBUILD RUN <<EOF\n# not\nEOF\n# c1\n", &["# c1"]),
        ("add <<EOF /x\n# not\nEOF\n# c1\n", &["# c1"]),
        // Not here-documents: other instructions, other operators, and quoted text.
        ("ENV A=1 <<EOF\n# c1\nEOF\n# c2\n", &["# c1", "# c2"]),
        ("RUNNER <<EOF\n# c1\n", &["# c1"]),
        ("RUN cat <<<EOF && echo $((1<<2)) a<<EOF\n# c1\n", &["# c1"]),
        ("RUN echo 'a <<EOF b'\n# c1\nEOF\n# c2\n", &["# c1", "# c2"]),
        (
            "RUN echo \"a <<EOF b\"\n# c1\nEOF\n# c2\n",
            &["# c1", "# c2"],
        ),
        ("RUN echo \\<<EOF b\n# c1\nEOF\n# c2\n", &["# c1", "# c2"]),
        ("RUN cat <<< EOF\n# c1\nEOF\n# c2\n", &["# c1", "# c2"]),
        // An escaped quote does not open a string.
        ("RUN echo \\\" <<EOF\n# not\nEOF\n# c1\n", &["# c1"]),
        // Two escape characters at the end of a line do not continue it.
        ("ENV A=b\\\\\nRUN <<EOF\n# not\nEOF\n# c1\n", &["# c1"]),
        ("RUN [\"sh\", \"-c\", \"cat <<EOF\"]\n# c1\n", &["# c1"]),
        ("RUN << \n# c1\n", &["# c1"]),
        // Other scripts before and inside comments.
        (
            "# 日本語😀\nRUN echo \"é😀 # not\" \\\n  # 継続😀\n  && true\nCOPY <<終 /x\n# not 😀\n終\n  # 後😀",
            &["# 日本語😀", "# 継続😀", "# 後😀"],
        ),
    ] {
        assert_eq!(
            sources("dockerfile", "file:///Dockerfile", text),
            expected,
            "{text:?}"
        );
    }
}

#[test]
fn dockerfile_reads_the_delimiter_of_a_here_document_as_buildkit_does() {
    let cases: &[(&str, &[&str])] = &[
        // In double quotes, a backslash is removed only before `"`, `$`, and a
        // backslash. In single quotes it stays, and outside quotes it is removed.
        (
            "FROM scratch\nCOPY <<\"E\\OF\" /payload\nEOF\n# not\nE\\OF\n# c1\n",
            &["# c1"],
        ),
        (
            "COPY <<\"E\\\"OF\" /x\nE\\\"OF\n# not\nE\"OF\n# c1\n",
            &["# c1"],
        ),
        (
            "COPY <<\"E\\\\OF\" /x\nE\\\\OF\nEOF\n# not\nE\\OF\n# c1\n",
            &["# c1"],
        ),
        (
            "COPY <<\"E\\$OF\" /x\nE\\$OF\n# not\nE$OF\n# c1\n",
            &["# c1"],
        ),
        ("COPY <<'E\\OF' /x\nEOF\n# not\nE\\OF\n# c1\n", &["# c1"]),
        ("COPY <<E\\OF /x\nE\\OF\n# not\nEOF\n# c1\n", &["# c1"]),
        ("COPY <<'E\"O\\\\F' /x\n# not\nE\"O\\\\F\n# c1\n", &["# c1"]),
        ("COPY <<\"E'O\\nF\" /x\n# not\nE'O\\nF\n# c1\n", &["# c1"]),
        ("RUN <<\\E\\\"O\\'F\\\\\n# not\nE\"O'F\\\n# c1\n", &["# c1"]),
        // The escape directive does not change the reading of a delimiter.
        (
            "# escape=`\nCOPY <<\"E\\\"O`F\" /x\n# not\nE\"O`F\n# c1\n",
            &["# c1"],
        ),
        // The parts of one word are joined, and a `$` name stays as written.
        (
            "RUN <<E\"O\"'F' <<$A <<${B} <<\"$C\"\n# not\nEOF\n# not\n$A\n# not\n${B}\n# not\n$C\n# c1\n",
            &["# c1"],
        ),
        (
            "RUN << \tEOF 3<< -EOF\n# not\nEOF\n# not\n-EOF\n# c1\n",
            &["# c1"],
        ),
        (
            "RUN a=${b:-c} ${d/e/f} <<EOF\n# not\nEOF\n# c1\n",
            &["# c1"],
        ),
        // An escaped blank belongs to the delimiter.
        ("RUN <<E\\ OF\n# not\nE OF\n# c1\n", &["# c1"]),
        // The lexer skips a byte order mark at the start of the delimiter.
        ("RUN <<\u{feff}EOF\n# not\nEOF\n# c1\n", &["# c1"]),
        // An error of the lexer leaves the instruction without here-documents, also after the error.
        ("RUN ${} <<EOF\n# c1\n", &["# c1"]),
        ("RUN ${a:#b} <<EOF\n# c1\n", &["# c1"]),
        ("RUN ${a!b} <<EOF\n# c1\n", &["# c1"]),
        ("RUN \"open <<EOF\n# c1\n", &["# c1"]),
        // The replacement of `${name/pattern/replacement}` runs to its `}`, and is split into words.
        ("RUN ${d/e/'} <<EOF '}\n# not\nEOF\n# c1\n", &["# c1"]),
        // The text of `${name ... }` is split into words once more, whatever its quotes.
        ("RUN echo ${a:- <<EOF}\n# not\nEOF}\n# c1\n", &["# c1"]),
        ("RUN echo ${a:-\" <<EOF \"}\n# not\nEOF\n# c1\n", &["# c1"]),
        ("RUN echo \"${a:- <<EOF}\"\n# c1\n", &["# c1"]),
        // A string around `${name ... }` stays in the word, and what came before it does not.
        (
            "RUN echo ${a:- <<\"E${b:-c}\"}\n# not\nE${b:-c}}\n# c1\n",
            &["# c1"],
        ),
    ];
    for (text, expected) in cases {
        for ending in ["\n", "\r\n", "\r"] {
            let text = text.replace('\n', ending);
            assert_eq!(
                &sources("dockerfile", "file:///Dockerfile", &text),
                expected,
                "{text:?}"
            );
        }
    }
}

#[test]
fn dockerfile_hides_what_was_meant_as_a_here_document_where_buildkit_has_none() {
    let cases: &[(&str, &[&str])] = &[
        // A blank after `<<-`, and an instruction that the lexer rejects after the word of `<<`.
        ("RUN cat <<- EOF\n\t# not\n\tEOF\n# c1\n", &["# c1"]),
        ("RUN <<EOF echo \"it's\n# not\nEOF\n# c1\n", &["# c1"]),
        ("RUN <<EOF echo ${x\n# not\nEOF\n# c1\n", &["# c1"]),
        // BuildKit reads this one too.
        ("RUN cat << EOF\n# not\nEOF\n# c1\n", &["# c1"]),
        // The delimiter is the next word after any blank, with its quotes removed.
        (
            "RUN <<\u{3000}EOF 3<<-\t\"A B\"\n# not\nEOF\n\t# not\n\tA B\n# c1\n",
            &["# c1"],
        ),
        // The next word is no delimiter where it is an operator itself. At the end there is none.
        ("RUN <<- <<A <<-\n# not\nA\n# c1\n", &["# c1"]),
        ("RUN <<- <<<A\n# not\n", &[]),
        // An empty delimiter ends at an empty line.
        ("RUN <<\"\"\n# not\n\n# c1\n", &["# c1"]),
        ("RUN <<-'' <<EOF\n# not\n\t\n# not\nEOF\n# c1\n", &["# c1"]),
        // A delimiter may hold `<` after its first character.
        (
            "RUN <<\"a<b\" <<a\\<c\n# not\na<b\n# not\na<c\n# c1\n",
            &["# c1"],
        ),
        ("RUN <<EOF<in\n# not\nEOF\n# not\n", &[]),
        // After an error of the lexer, the words before it count, and the ones after it do not.
        ("RUN <<EOF 'open\n# not\nEOF\n# c1\n", &["# c1"]),
        ("RUN <<EOF ${}\n# not\nEOF\n# c1\n", &["# c1"]),
        ("RUN <<EOF ${a:-b\n# not\nEOF\n# c1\n", &["# c1"]),
        ("RUN <<EOF ${a!b} ${a:#b}\n# not\nEOF\n# c1\n", &["# c1"]),
        (
            "RUN echo \"open <<EOF\n# c1\nEOF\n# c2\n",
            &["# c1", "# c2"],
        ),
        // A line that one reading hides starts a here-document in the other. A comment is
        // a line that both readings take for one.
        ("RUN <<- X\nRUN <<A\nX\n# not\nA\n# c1\n", &["# c1"]),
        ("RUN <<\"a<b\"\nRUN <<A\na<b\n# not\nA\n# c1\n", &["# c1"]),
        ("RUN <<X 'open\nRUN <<A\nX\n# not\nA\n# c1\n", &["# c1"]),
        ("RUN <<X \"open\nRUN <<A\nX\n# not\nA\n# c1\n", &["# c1"]),
        // BuildKit reads a blank after `<<` itself, so both readings are the same here.
        ("RUN << X\nRUN <<A\nX\n# c1\nA\n# c2\n", &["# c1", "# c2"]),
        // A word of `<<` or a delimiter that lost its start to `${name ... }` is not read.
        ("RUN <<E${x:-y}\n# not\nE${x:-y}\n# not\n", &[]),
        ("RUN <<\"E${x:+y}\"\n# not\nE${x:+y}\n# not\n", &[]),
        ("RUN <<- E${x:-y}\n# not\n${x:-y}\n# not\n", &[]),
    ];
    // No delimiter can be read from these, so no line after them is taken.
    let stops = [
        "RUN <<\"EOF",
        "RUN <<'EOF",
        "RUN <<EOF${x",
        "RUN <<EOF${}",
        "RUN <<E${x:-y}OF",
        "RUN <<\"E${x:+y}\"",
        "RUN 3<<-${x/y/z}",
        "RUN <<- \"EOF",
        "RUN <<- E${x:-y}",
        "RUN <<A <<\"B",
        "RUN echo ${a:- <<EOF",
        "RUN ${d/e/\\} <<EOF",
        "RUN echo ${a:- <<E\\\"}",
    ];
    for ending in ["\n", "\r\n", "\r"] {
        for (text, expected) in cases {
            let text = text.replace('\n', ending);
            assert_eq!(
                &sources("dockerfile", "file:///Dockerfile", &text),
                expected,
                "{text:?}"
            );
        }
        for stop in stops {
            let text =
                format!("# before\n{stop}\n# after\nEOF\nB\n# after\n").replace('\n', ending);
            assert_eq!(
                sources("dockerfile", "file:///Dockerfile", &text),
                ["# before"],
                "{text:?}"
            );
        }
    }
}

#[test]
fn dockerfile_long_lines_and_many_here_documents_are_read_in_one_pass() {
    let megabyte = 1 << 20;
    let line = format!("RUN echo {}\n# note", "\"a\" b <<<c ".repeat(megabyte / 11));
    let documents = format!(
        "{}# note",
        "RUN <<A 3<<-B\n# not\nA\n\t# not\n\tB\n".repeat(megabyte / 32)
    );
    let operators = format!(
        "RUN {}\n{}# note",
        "<<A ".repeat(megabyte / 6),
        "A\n".repeat(megabyte / 6)
    );
    let continued = format!("{}b\n# note", "RUN a \\\n".repeat(megabyte / 8));
    let depth = megabyte / 8;
    let nested = format!(
        "RUN <<A {}{}\n# not\nA\n# note",
        "\"${a:-".repeat(depth),
        "}\"".repeat(depth)
    );
    let started = Instant::now();
    for text in [line, documents, operators, continued, nested] {
        assert!(text.len() >= megabyte - 64);
        assert_eq!(bodies("dockerfile", &text), ["note"]);
    }
    // Each takes milliseconds when read once. All of them take seconds in a
    // debug build on a slow machine; reading a megabyte again at each piece
    // would take hours.
    assert!(started.elapsed() < Duration::from_secs(60));
}

#[test]
fn readers_take_time_in_proportion_to_the_text() {
    // Each sign on its own, and the openings that a reader could read on from.
    let signs = ")]}([{\"'#\\`=,:-|><&*!%@$; \na";
    let signs: Vec<_> = signs.chars().map(String::from).collect();
    let openings = [
        "a ",
        "a = ",
        "[[",
        "\"\"\"",
        "'''",
        "<<E\n",
        "$(",
        "${",
        "$((",
        "@\"\n",
        "\"@",
        "<#",
        "#>",
        "--% ",
        "$x=1 +\n",
        "'a",
        "\"a\"@",
        "\\\n",
        "${a:-",
        "\"${a/",
        "/*",
        "//",
        "<!--",
        "--",
        "<![CDATA[",
        "<?",
        "</",
        "<a b='",
        "<a b=\"",
        "<a ",
        "<script>",
        "<style>",
        "<!--<script>",
        "url(",
        "\\75 rl(",
        "1e",
        "-->",
        "<?php ",
        "<?php #",
        "<?php \"{$",
        "<?php <<<A\n",
        "{$",
        "?>",
        "x /",
        "%w[",
        "?#",
        "<<~A\n",
        "=begin\n",
        "\"#{",
        "a::<<",
        "a, ",
        "->(a) {",
        "in [a, ",
        "rescue => a\n",
        "/(?<a>)/ =~ ",
        "/(?x)#(?<a>)",
        "(?#",
        "a[",
        "(a, b = ",
        "def f(a = proc { |b| ",
        "=begin\r\na\r\n=end\r\n",
        "$a-> /**/ __halt_compiler ",
        "1%url(",
        "&#x2f;",
    ];
    let pieces: Vec<_> = signs.iter().map(String::as_str).chain(openings).collect();
    let size = 64 << 10;
    let uri = "untitled:comment".parse().unwrap();
    for language in [
        "powershell",
        "toml",
        "dockerfile",
        "json",
        "shellscript",
        "yaml",
        "go",
        "css",
        "html",
        "ruby",
        "php",
    ] {
        // A Dockerfile reads words only in an instruction that can have a here-document.
        let heads: &[&str] = if language == "dockerfile" {
            &["", "RUN <<E "]
        } else {
            &[""]
        };
        for (head, piece, tail) in heads.iter().flat_map(|head| {
            pieces
                .iter()
                .flat_map(move |piece| ["", "\n", "\nx"].map(|tail| (head, piece, tail)))
        }) {
            let text = format!("{head}{}{tail}", piece.repeat(size / piece.len()));
            let started = Instant::now();
            parsed(language, &uri, &text);
            // One pass takes milliseconds in a debug build. Reading the
            // rest of the text again at each piece took over 20 seconds.
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "{language} {head:?} {piece:?} {tail:?}"
            );
        }
    }
}

#[test]
fn dockerfile_decides_once_what_a_word_is() {
    // After a long word of digits, each of many `${name ... }` read that
    // word again to tell whether it waits for a delimiter.
    let size = 256 << 10;
    let digits = "1".repeat(size / 2);
    let expansions = "${a:-x}".repeat(size / 14);
    let started = Instant::now();
    for text in [
        format!("# note\nRUN {digits} {expansions} <<E\n"),
        format!("# note\nRUN <<- {digits} {expansions}\n"),
    ] {
        assert_eq!(bodies("dockerfile", &text), ["note"]);
    }
    // Each takes milliseconds when read once, and about a second in a debug
    // build on a slow machine. Reading the word again at each `${name ... }`
    // took three minutes.
    assert!(started.elapsed() < Duration::from_secs(30));
}

#[test]
fn readers_take_time_in_proportion_to_texts_of_several_parts() {
    // One piece repeated does not show a reader that reads one part again
    // for every piece of another part. Here each text has three to five
    // parts, and each part is a piece repeated once, a few times, or often.
    let pieces = [
        "1",
        "12345",
        "a",
        "ab",
        " ",
        "\t",
        "\n",
        "\r\n",
        "\"",
        "'",
        "<<",
        "<<-",
        "<<E",
        "<<E\n",
        "${a:-x}",
        "${a}",
        "${",
        "$(",
        ")",
        "\\",
        "`",
        "#",
        "@\"",
        "\"@",
        "@\"\n",
        "\n\"@",
        "<#",
        "#>",
        "--%",
        "$",
        "(",
        "{",
        "}",
        "[",
        "]",
        "=",
        ",",
        ";",
        "|",
        "-",
        "\"\"\"",
        "'''",
        ": ",
        "- ",
        "RUN ",
        "a = ",
        "$x",
        "%",
        "[a,",
        "\"${a",
        "\\\n",
        "`\n",
        "E\n",
        "日",
        "\u{2018}",
        "$$",
        "<",
        ">",
        "&",
        "@",
        "!",
        "?",
        "*",
        "/",
        "x=(",
        "$((",
        "case x in ",
        "esac ",
        "[[ ",
        "' # ",
        "\" # ",
        "/*",
        "*/",
        "//",
        "<!--",
        "-->",
        "--",
        "<a ",
        "</a",
        "<![CDATA[",
        "<style>",
        "</style>",
        "<textarea>",
        "url(",
        "\\75 ",
        "1e",
        "<?php ",
        "{$",
        "${",
        "<<<A\n",
        "<<~A\n",
        "A\n",
        "#{",
        "%w[",
        "?#",
        "x = 1\n",
        "x /",
        "=begin\n",
        "=end\n",
        "def f(a) ",
        "def f(a = b, c:) ",
        "{ |a| ",
        "-> a { ",
        "->(x=(",
        ";",
        "a, b = ",
        "in {a:, ",
        "/(?<a>)/ =~ ",
        "\r\n",
    ];
    // A fixed sequence, so that every run reads the same texts.
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut below = |bound: usize| {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        (state.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 33) as usize % bound
    };
    let uri = "untitled:comment".parse().unwrap();
    // A script in HTML is read by Oxc, whose depth of nesting is limited by
    // the stack; the pieces here hold no `<script>`.
    for language in [
        "powershell",
        "toml",
        "dockerfile",
        "json",
        "shellscript",
        "yaml",
        "go",
        "css",
        "html",
        "ruby",
        "php",
    ] {
        for _ in 0..160 {
            let size = (16 + below(49)) << 10;
            // The long parts share the size by their weights.
            let parts: Vec<_> = (0..3 + below(3))
                .map(|_| (pieces[below(pieces.len())], below(2) * (1 + below(8))))
                .collect();
            let weights = parts.iter().map(|part| part.1).sum::<usize>().max(1);
            let mut text = String::new();
            // A Dockerfile reads words only in an instruction that can have a here-document.
            if language == "dockerfile" && below(2) == 0 {
                text.push_str("RUN ");
            }
            for (piece, weight) in &parts {
                let repeats = match weight {
                    0 => 1 + below(3),
                    _ => size * weight / weights / piece.len() + 1,
                };
                text.push_str(&piece.repeat(repeats));
            }
            let started = Instant::now();
            parsed(language, &uri, &text);
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "{language} {parts:?}"
            );
        }
    }
}

#[test]
fn docker_compose_is_read_as_yaml() {
    assert_eq!(bodies("dockercompose", YAML), bodies("yaml", YAML));
    assert!(!bodies("dockercompose", YAML).is_empty());
}

fn go(text: &str) -> Vec<String> {
    sources("go", "untitled:comment", text)
}

#[test]
fn go_comments_exclude_literals_and_directives() {
    let text = concat!(
        "// Package a reads.\n",
        "//go:build linux\n",
        "//nolint:errcheck\n",
        "//export F\n",
        "//line a.go:1\n",
        "//TODO: kept\n",
        "//http://kept\n",
        "s := \"// not\" + `/* not */` + '/' // trailing\n",
        "r := '\\'' // after rune\n",
        "t := \"\\\" // not\" /* block */\n",
        "u := `\n// raw\n`\n",
        "v := \"unclosed // not\n",
        "w := '/ // not\n",
        "// after unclosed\n",
        "/* unclosed",
    );
    assert_eq!(
        go(text),
        [
            "// Package a reads.",
            "//TODO: kept",
            "//http://kept",
            "// trailing",
            "// after rune",
            "/* block */",
            "// after unclosed",
        ]
    );
    assert_eq!(go("x := `unclosed\n// not"), Vec::<String>::new());
    // A directive divides the comments around it.
    assert_eq!(
        bodies(
            "go",
            "// One\n// two\n//go:generate x\n// Three\nfunc f() {}\n"
        ),
        ["One\ntwo", "Three"]
    );
}

fn css(text: &str) -> Vec<String> {
    sources("css", "untitled:comment", text)
}

#[test]
fn css_comments_exclude_strings_escapes_and_urls() {
    let text = concat!(
        "/* a */ a { content: \"/* not */\"; b: '/* not' }\n",
        "c { background: url(http://x/*y) } /* after url */\n",
        "d { background: URL( \"/* not */\" ) /* after quoted url */ }\n",
        "e { x: myurl(/* f */) }\n",
        "\\/* not */\n",
        "g { content: \"a\\\n/* not */\" }\n",
        "h { content: \"unclosed\n/* after bad string */\n",
        "i { background: url(a\\)/*b) /* after escaped parenthesis */\n",
        "j { --x: 1%url(a/*b*/c) } /* after percentage */\n",
        "/* unclosed",
    );
    assert_eq!(
        css(text),
        [
            "/* a */",
            "/* after url */",
            "/* after quoted url */",
            "/* f */",
            "/* after bad string */",
            "/* after escaped parenthesis */",
            "/* after percentage */",
        ]
    );
    assert_eq!(bodies("css", "/* Retry\n   now */"), ["Retry\n   now "]);
}

fn html(text: &str) -> Vec<String> {
    sources("html", "untitled:comment", text)
}

#[test]
fn html_comments_exclude_tags_and_raw_text_and_read_scripts_and_styles() {
    let text = concat!(
        "<!DOCTYPE html>\n",
        "<!-- top -->\n",
        "<p title=\"<!-- not -->\" data-x='<!-- not' data-y=a<!--not>text <!--inline--></p>\n",
        "<textarea><!-- not --></textarea><TITLE><!-- not --></title >\n",
        "<script>\n// js line\nlet s = \"<!-- not -->\"; /* js block */\n</script>\n",
        "<script type=\"application/json\">{\"a\": \"// not\"} // not</script>\n",
        "<script type=module>// module line\n</script>\n",
        "<script type=' text/JavaScript '>// typed line\n</script>\n",
        "<script language=vbscript>' not // not</script>\n",
        "<script type=\"text&#x2F;javascript\">// referenced type\n</script>\n",
        "<script type=\"text&sol;javascript&#0;\">' not // not</script>\n",
        "<script type language=vbscript>// empty type\n</script>\n",
        "<script type=\"&Tab;text/javascript&NewLine;\">// named blanks\n</script>\n",
        "<style>/* css */ a { content: \"<!-- not -->\" }</style>\n",
        "<!-- a -- b --->\n",
        "<!---->\n",
        "<!-->\n",
        "<!--->\n",
        "<!-- bang --!>\n",
        "<![CDATA[ <!-- not --> ]]>\n",
        "<?xml <!-- not ?>\n",
        "</ <!-- not >\n",
        "<plaintext><!-- not -->",
    );
    assert_eq!(
        html(text),
        [
            "<!-- top -->",
            "<!--inline-->",
            "// js line",
            "/* js block */",
            "// module line",
            "// typed line",
            "// referenced type",
            "// empty type",
            "// named blanks",
            "/* css */",
            "<!-- a -- b --->",
            "<!---->",
            "<!-->",
            "<!--->",
            "<!-- bang --!>",
        ]
    );
    // One blank after `<!--` is left out, as after other comment marks.
    assert_eq!(
        bodies("html", "<!-- top -->\n<!--  a -- b --->\n<!-->"),
        ["top ", " a -- b -", ""]
    );
    // An unclosed comment, tag, or quote hides the rest.
    for text in [
        "<!-- a -->\n<!-- b",
        "<!-- a -->\n<p title=\"<!-- b -->",
        "<!-- a -->\n<p <!-- b -->",
        "<!-- a -->\n<!DOCTYPE <!-- b -->",
        "<!-- a -->\n<style>",
    ] {
        assert_eq!(html(text), ["<!-- a -->"], "{text}");
    }
}

#[test]
fn html_scripts_end_where_the_tokenizer_ends_them() {
    // After `<!--`, `<script>` makes the next `</script>` part of the script.
    let text = concat!(
        "<script><!--\n",
        "document.write('<script></script>'); // inside\n",
        "--></script><!-- after -->",
    );
    assert_eq!(html(text), ["<!--", "// inside", "-->", "<!-- after -->"]);
    // Without `<!--`, the first `</script>` ends it.
    assert_eq!(
        html("<script>// a '</script>'; // b\n</script><!-- after -->"),
        ["// a '", "<!-- after -->"]
    );
    assert_eq!(
        html("<script><!-- x </SCRIPT ><!-- after -->"),
        ["<!-- x ", "<!-- after -->"]
    );
    // A script that the document ends is read to the end.
    assert_eq!(html("<script>// a\nlet b; // c"), ["// a", "// c"]);
    // Adjacent line comments of a script are one group, as in JavaScript.
    assert_eq!(
        bodies("html", "<script>\n  // one\n  // two\n</script>"),
        ["one\ntwo"]
    );
}

fn ruby(text: &str) -> Vec<String> {
    sources("ruby", "untitled:comment", text)
}

#[test]
fn ruby_comments_exclude_literals_and_follow_what_precedes_an_operator() {
    let text = concat!(
        "#!/usr/bin/env ruby\n",
        "# top\n",
        "x = 1 # trailing\n",
        "s = \"# not #{x # inside interpolation\n",
        "} # not\" # after string\n",
        "r = /# not/ # after regexp\n",
        "y = x /2 # after division\n",
        "p /# not/ # after method\n",
        "w = %w[# not [# not]] # after words\n",
        "c = ?# # after character\n",
        "h = <<~EOS # after heredoc\n",
        "  # not\n",
        "  EOS\n",
        "g = $' # after global\n",
        "t = :\"# not\" + :$' # after symbols\n",
        "def /(o) o /2 end # after operator\n",
        "z.<<EOS # after method name\n",
        "[1].each { |b| b /2 } # after block\n",
        "b /# not/ # b is a method again\n",
        "def m(d) = d /2 # after endless def\n",
        "d /# not/ # d is a method again\n",
        "for e in [] do end\n",
        "e /2 # after for\n",
        "=begin\n",
        "block\n",
        "=end\n",
        "__END__\n",
        "# not\n",
    );
    assert_eq!(
        ruby(text),
        [
            "# top",
            "# trailing",
            "# inside interpolation",
            "# after string",
            "# after regexp",
            "# after division",
            "# after method",
            "# after words",
            "# after character",
            "# after heredoc",
            "# after global",
            "# after symbols",
            "# after operator",
            "# after method name",
            "# after block",
            "# b is a method again",
            "# after endless def",
            "# d is a method again",
            "# after for",
            "=begin\nblock\n=end",
        ]
    );
    assert_eq!(bodies("ruby", "=begin\nOne\ntwo\n=end"), ["One\ntwo\n"]);
    // An unclosed literal or `=begin` hides the rest.
    for text in [
        "# a\n\"# b",
        "# a\n%w[# b",
        "# a\n<<~EOS\n# b",
        "# a\n=begin\n# b",
    ] {
        assert_eq!(ruby(text), ["# a"], "{text}");
    }
}

#[test]
fn ruby_variables_are_declared_where_ruby_declares_them() {
    // `y /2 # c` is a division when `y` is a variable, and `x /# r/` a
    // regular expression when `x` is a method.
    let lines = [
        ("def f(y = x)\n  x /# r/ # c\nend\n", vec!["# c"]),
        ("def f(y = x)\n  y /2 # c\nend\n", vec!["# c"]),
        (
            "def f(y = g(1, x), z: x)\n  z /2 # c\n  x /# r/\nend\n",
            vec!["# c"],
        ),
        ("def f y, z = x\n  z /2 # c\n  x /# r/\nend\n", vec!["# c"]),
        ("def f((y, z), w)\n  z /2 # c\nend\n", vec!["# c"]),
        ("def +(y)\n  y /2 # c\nend\n", vec!["# c"]),
        ("def f = x\nx /# r/\n", vec![]),
        (
            "[1].each { |y = x; z| z /2 # c\n  x /# r/\n}\n",
            vec!["# c"],
        ),
        ("->(y, z = x) { z /2 # c\n}\nz /# r/\n", vec!["# c"]),
        ("-> y { y /2 # c\n}\n", vec!["# c"]),
        ("y, (z, *w) = 1, [2]\nw /2 # c\n", vec!["# c"]),
        ("y.z, w = 1, 2\nw /2 # c\ny /# r/\n", vec!["# c"]),
        ("g y, z\nz /# r/\n", vec![]),
        ("y,\n  z = 1, 2\nz /2 # c\n", vec!["# c"]),
        ("begin\nrescue Foo => y\n  y /2 # c\nend\n", vec!["# c"]),
        ("変数 = 1\n変数 /2 # c\n", vec!["# c"]),
        ("case 1\nin [y, {z:}]\n  z /2 # c\nend\n", vec!["# c"]),
        ("case 1\nin {y: z}\n  y /# r/\nend\n", vec![]),
        ("case 1\nin ^y\n  y /# r/\nend\n", vec![]),
        ("if /(?<y>a)(?'z'b)/ =~ s\n  z /2 # c\nend\n", vec!["# c"]),
        ("/(?<y>a)#{s}/ =~ s\ny /# r/\n", vec![]),
        // A comment in a regular expression names no group.
        ("/# (?<y>x)\n/x =~ s\ny /# r/\n", vec![]),
        (
            "/(?x: #(?<y>)\n)(?-x)#(?<z>x)/ =~ s\nz /2 # c\ny /# r/\n",
            vec!["# c"],
        ),
        ("/(?#(?<y>x)(?<z>x)/ =~ s\nz /2 # c\ny /# r/\n", vec!["# c"]),
        // A block or lambda in a default value leaves the list around it.
        (
            "def f(y = proc { |w| w }, z)\n  z /2 # c\nend\n",
            vec!["# c"],
        ),
        (
            "def f(y = ->(w) { w }, z:)\n  z /2 # c\n  w /# r/\nend\n",
            vec!["# c"],
        ),
        ("->(y, z = y /2 # c\n) { z }\ny /# r/\n", vec!["# c"]),
        ("foo((y, z = 1, 2))\ny /2 # c\n", vec!["# c"]),
        ("r = (y, z = [4, 2])\ny /2 # c\n", vec!["# c"]),
        ("y, a[0] = 4, 2\ny /2 # c\na /# r/\n", vec!["# c"]),
        ("foo(y, z = 1)\ny /# r/\n", vec![]),
    ];
    for (text, comments) in lines {
        assert_eq!(ruby(text), comments, "{text}");
        // Ruby reads CRLF as LF.
        let crlf = text.replace('\n', "\r\n");
        assert_eq!(ruby(&crlf), comments, "{crlf:?}");
    }
    // A backslash before CRLF goes on with the statement and the
    // here-document.
    for ending in ["\n", "\r\n"] {
        let text = [
            "x = 4", "x \\", "/2 # c", "s = <<E", "a\\", "E", "# not", "E", "",
        ]
        .join(ending);
        assert_eq!(ruby(&text), ["# c"], "{text:?}");
    }
    assert_eq!(
        bodies("ruby", "=begin\r\nOne\r\n=end\r\n# Two"),
        ["One\r\n", "Two"]
    );
}

fn php(text: &str) -> Vec<String> {
    sources("php", "untitled:comment", text)
}

#[test]
fn php_comments_are_read_in_code_and_exclude_strings_and_attributes() {
    let text = concat!(
        "<!-- not --> # not\n",
        "<?php\n",
        "# hash\n",
        "// slashes\n",
        "/* block */ /** doc */\n",
        "#[Attribute] // after attribute\n",
        "$s = 'it\\'s # not'; // after single\n",
        "$d = \"{$a['# not']} // not\"; // after double\n",
        "$h = <<<EOT\n",
        "# not\n",
        "{$x /* in interpolation */}\n",
        "EOT; // after heredoc\n",
        "$n = <<<'EOT'\n",
        "# not {$x /* not */}\n",
        "  EOT;\n",
        "echo 1; // before close ?> # not\n",
        "<?= $x # short echo ?>\n",
        "<?php $x-> /* property */ __halt_compiler(); // after property\n",
        "__halt_compiler(); # not\n",
    );
    assert_eq!(
        php(text),
        [
            "# hash",
            "// slashes",
            "/* block */",
            "/** doc */",
            "// after attribute",
            "// after single",
            "// after double",
            "/* in interpolation */",
            "// after heredoc",
            "// before close ",
            "# short echo ",
            "/* property */",
            "// after property",
        ]
    );
    assert_eq!(
        bodies("php", "<?php\n// One\n// two\n# Three\n/** Four */"),
        ["One\ntwo", "Three", "Four "]
    );
    // An unclosed comment, string, or here-document hides the rest.
    for text in [
        "<?php # a\n/* # b",
        "<?php # a\n\"# b",
        "<?php # a\n'# b",
        "<?php # a\n<<<EOT\n# b",
    ] {
        assert_eq!(php(text), ["# a"], "{text}");
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
    let comments = parsed("rust", &uri, text);
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
        assert!(parsed("rust", &uri, unfinished).0.is_empty());
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
        let comments = parsed(language, &uri, &text);
        let group = comments.at(&text, text.find(ending).unwrap()).unwrap();
        assert_eq!(comments.body(&text, group), format!("first{ending}second"));
        assert!(comments.at(&text, text.len()).is_some());
        let indent = text.find("  // second").unwrap();
        assert!(comments.at(&text, indent).is_none());
    }
    let uri = "file:///blocks.rs".parse().unwrap();
    let text = "/* left *//* right */";
    let comments = parsed("rust", &uri, text);
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
        let comments = parsed(language, &uri, &text);
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
        ("typescript", "file:///a.ts", true, false),
        ("typescriptreact", "file:///a.tsx", true, true),
    ] {
        let mut text = "const s = '// string'; const r = /\\/\\/ regex/;\nconst t = `// template ${1 /* expr */}`;\n".to_owned();
        if typescript {
            text.push_str("const n: number = 1;\n");
        }
        if jsx {
            text.push_str("const view = <div title='// attribute'>// jsx text{/* jsx */}</div>;\n");
        }
        text.push_str("// actual 🚀\n/** doc */\n");
        let uri = path.parse().unwrap();
        let comments = parsed(language, &uri, &text);
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
        let comments = parsed("javascript", &uri, &text);
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
        let source = source_type(Language::from_id(id).unwrap(), &path.parse().unwrap()).unwrap();
        assert_eq!(source.is_typescript(), ts);
        assert_eq!(source.is_jsx(), jsx);
        assert_eq!(source.is_typescript_definition(), definition);
    }
    assert!(source_type(Language::Markdown, &"file:///a.jsx".parse().unwrap()).is_none());
}

#[test]
fn the_limit_stops_the_tree_sitter_languages_and_no_other() {
    let uri: Uri = "file:///sample".parse().unwrap();
    let text = "// c\n# c\n";
    for language in ["c", "cpp", "csharp", "python"] {
        assert_eq!(
            Comments::parse(
                Language::from_id(language).unwrap(),
                &uri,
                text,
                Duration::ZERO
            )
            .err(),
            Some(Failure::TimedOut),
            "{language}"
        );
    }
    for language in [
        "rust",
        "javascript",
        "typescript",
        "json",
        "jsonc",
        "yaml",
        "toml",
        "dockercompose",
        "shellscript",
        "powershell",
        "dockerfile",
        "markdown",
    ] {
        assert!(
            Comments::parse(
                Language::from_id(language).unwrap(),
                &uri,
                text,
                Duration::ZERO
            )
            .is_ok(),
            "{language}"
        );
    }
}

#[test]
fn reading_stops_at_the_limit_while_parsing_and_while_walking_the_tree() {
    let uri: Uri = "file:///sample.py".parse().unwrap();
    let limit = Duration::from_millis(100);

    // Tree-sitter looks at the limit between its steps. On ordinary text a
    // step is short, so parsing stops close to the limit. Parsed to its end,
    // this text takes over ten seconds in a debug build.
    let ordinary = "value = 1\n".repeat(1_600_000);
    let start = Instant::now();
    let read = syntax::parse(
        ordinary.as_bytes(),
        tree_sitter_python::LANGUAGE.into(),
        syntax::Recovery::StopAtLiteral,
        start + limit,
        |_| None,
    );
    let elapsed = start.elapsed();
    assert!(read.is_none());
    assert!(elapsed >= limit, "{elapsed:?}");
    assert!(elapsed < limit + Duration::from_secs(5), "{elapsed:?}");

    // On this many continued lines a step can take seconds on a slow
    // machine, so only the result is checked here.
    let continued = "\\\n".repeat(40 * 1024);
    let read = Comments::parse(Language::Python, &uri, &continued, limit);
    assert_eq!(read.err(), Some(Failure::TimedOut));

    // This text is parsed at once. Each of its hundreds of nodes is made
    // slow to visit, so the limit passes during the walk. Visiting them all
    // would take half a minute and give the comments.
    let mut visited = 0;
    let read = syntax::parse(
        "f()\n".repeat(100).as_bytes(),
        tree_sitter_python::LANGUAGE.into(),
        syntax::Recovery::StopAtLiteral,
        Instant::now() + Duration::from_millis(500),
        |_| {
            visited += 1;
            thread::sleep(Duration::from_millis(50));
            None
        },
    );
    assert!(read.is_none());
    assert!(visited > 0);
}

#[test]
fn a_panic_while_reading_is_a_failure_that_names_its_place_only() {
    let uri: Uri = "file:///sample.rs".parse().unwrap();
    // ra-ap-rustc_lexer 0.179 panics on an opening of more than 65,535 dashes.
    let text = format!("{}\n// Secret.\n", "-".repeat(65_536));
    let Err(Failure::Panicked(Some(place))) =
        Comments::parse(Language::Rust, &uri, &text, Duration::from_secs(60))
    else {
        panic!("Expected a panic");
    };
    let (file, line) = place.rsplit_once(':').unwrap();
    assert!(file.ends_with(".rs"), "{place}");
    assert!(line.parse::<u32>().is_ok(), "{place}");
    assert!(!READING.get());
    assert_eq!(parsed("rust", &uri, "// Read.").0.len(), 1);
}
