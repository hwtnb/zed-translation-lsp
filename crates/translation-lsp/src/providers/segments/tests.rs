use super::{Markup, Segmented, batches};

/// What the service is sent for `text`, and the text put back from
/// translations that start each segment with `T:`.
fn round_trip(text: &str) -> (Vec<String>, String) {
    let segmented = Segmented::new(text);
    let marked = segmented.marked(Markup::Xml);
    let translations: Vec<String> = marked.iter().map(|text| format!("T:{text}")).collect();
    let assembled = segmented.assemble(Markup::Xml, &translations).unwrap();
    (marked, assembled)
}

#[test]
fn lines_of_a_sentence_are_joined_into_one_segment() {
    for text in [
        "Retry only if the remote service\nreports a temporary failure.",
        // A docstring keeps the indentation of its later lines.
        "Retry only if the remote service\n    reports a temporary failure.",
        "Retry only if the remote service\r\nreports a temporary failure.",
    ] {
        let (marked, assembled) = round_trip(text);
        assert_eq!(
            marked,
            ["Retry only if the remote service reports a temporary failure."],
            "{text:?}"
        );
        assert_eq!(
            assembled,
            "T:Retry only if the remote service reports a temporary failure."
        );
    }
    // Lines of a script without spaces between words join without one.
    let (marked, _) = round_trip("リモートサービスが\n一時的な障害を報告した場合");
    assert_eq!(marked, ["リモートサービスが一時的な障害を報告した場合"]);
}

#[test]
fn headings_items_and_quotes_keep_their_marks_and_code_blocks_are_not_sent() {
    let text = "\
## Setup
Install the tool and run
`translation-lsp --stdio`.

- Fast
- Private and
  local
1. [ ] Check the box

> Quoted text
> on two lines

```sh
echo \"Not translated\"
```
---
";
    let (marked, assembled) = round_trip(text);
    assert_eq!(
        marked,
        [
            "Setup",
            "Install the tool and run <x i=\"0\">`translation-lsp --stdio`</x>.",
            "Fast",
            "Private and local",
            "Check the box",
            "Quoted text on two lines",
        ]
    );
    assert_eq!(
        assembled,
        "\
## T:Setup
T:Install the tool and run `translation-lsp --stdio`.

- T:Fast
- T:Private and local
1. [ ] T:Check the box

> T:Quoted text on two lines

```sh
echo \"Not translated\"
```
---
"
    );
}

#[test]
fn a_line_after_a_colon_or_before_a_tag_starts_a_segment() {
    let (marked, assembled) = round_trip(
        "Args:\n    retries: How often to retry.\nSee the notes.\n@param delay The wait",
    );
    assert_eq!(
        marked,
        [
            "Args:",
            "retries: How often to retry. See the notes.",
            "@param delay The wait"
        ]
    );
    assert_eq!(
        assembled,
        "T:Args:\n    T:retries: How often to retry. See the notes.\nT:@param delay The wait"
    );
}

#[test]
fn stars_of_a_block_comment_decorate_its_lines_but_items_at_the_margin_are_items() {
    let javadoc = "*\n * Retry only if the remote service\n * reports a temporary failure.\n *\n * - First\n * @param delay The wait\n ";
    let (marked, assembled) = round_trip(javadoc);
    assert_eq!(
        marked,
        [
            "Retry only if the remote service reports a temporary failure.",
            "First",
            "@param delay The wait",
        ]
    );
    assert_eq!(
        assembled,
        "*\n * T:Retry only if the remote service reports a temporary failure.\n *\n * - T:First\n * T:@param delay The wait\n "
    );

    let (marked, _) = round_trip("Features:\n* fast\n* private");
    assert_eq!(marked, ["Features:", "fast", "private"]);
}

#[test]
fn table_cells_are_segments_and_the_delimiter_row_is_kept() {
    let (marked, assembled) =
        round_trip("| Setting | Default |\n| --- | :---: |\n| `model` | The newest model |");
    assert_eq!(marked, ["Setting", "Default", "The newest model"]);
    assert_eq!(
        assembled,
        "| T:Setting | T:Default |\n| --- | :---: |\n| `model` | T:The newest model |"
    );
}

#[test]
fn code_spans_and_urls_are_marked_and_put_back_as_they_were() {
    let text = "See https://example.com/a_(b), call ``a`b`` & read <https://example.com>.";
    let (marked, assembled) = round_trip(text);
    assert_eq!(
        marked,
        [
            "See <x i=\"0\">https://example.com/a_(b)</x>, call <x i=\"1\">``a`b``</x> &amp; read &lt;<x i=\"2\">https://example.com</x>&gt;."
        ]
    );
    assert_eq!(assembled, format!("T:{text}"));
    // An unmatched backtick and a scheme inside a word are prose.
    let (marked, _) = round_trip("A ` tick and xhttps://example.com");
    assert_eq!(marked, ["A ` tick and xhttps://example.com"]);
}

#[test]
fn segments_without_letters_outside_code_are_kept_and_not_sent() {
    let text = "`retain()`\n\n42\n\nReal text";
    let segmented = Segmented::new(text);
    assert_eq!(segmented.marked(Markup::Xml), ["Real text"]);
    assert_eq!(
        segmented
            .assemble(Markup::Xml, &["Translated".into()])
            .unwrap(),
        "`retain()`\n\n42\n\nTranslated"
    );
}

#[test]
fn translations_that_lose_or_repeat_a_mark_or_do_not_match_are_refused() {
    let segmented = Segmented::new("Call `a` then `b`.");
    let assemble = |translations: &[&str]| {
        let translations: Vec<String> = translations.iter().map(|&t| t.into()).collect();
        segmented.assemble(Markup::Xml, &translations)
    };
    assert_eq!(
        assemble(&["<x i=\"1\">b</x> の後で <x i=\"0\">a</x> を呼ぶ &amp; &#39;x&#x27;"]).unwrap(),
        "`b` の後で `a` を呼ぶ & 'x'"
    );
    // A self-closing mark stands for its part too.
    assert_eq!(
        assemble(&["<x i=\"0\"/> と <x i=\"1\"/>"]).unwrap(),
        "`a` と `b`"
    );
    for broken in [
        "<x i=\"0\">a</x> only",
        "<x i=\"0\">a</x> <x i=\"0\">a</x> <x i=\"1\">b</x>",
        "<x i=\"0\">a</x> <x i=\"1\">b</x> <x i=\"2\">c</x>",
        "<x i=\"0\">a</x> <b>added</b> <x i=\"1\">b</x>",
        "<x i=\"0\">a <x i=\"1\">b</x>",
    ] {
        assert_eq!(assemble(&[broken]), None, "{broken}");
    }
    assert_eq!(assemble(&[]), None);
    assert_eq!(assemble(&["<x i=\"0\"/><x i=\"1\"/>", "extra"]), None);
}

#[test]
fn long_texts_are_grouped_into_requests_that_fit() {
    let texts: Vec<String> = [
        "a".repeat(60),
        "b".repeat(40),
        "c".into(),
        "d".repeat(101),
        "e".into(),
    ]
    .into_iter()
    .collect();
    let lengths = |bytes, count| -> Vec<usize> {
        batches(&texts, bytes, count)
            .iter()
            .map(|batch| batch.len())
            .collect()
    };
    assert_eq!(lengths(100, usize::MAX), [2, 1, 1, 1]);
    assert_eq!(lengths(96, usize::MAX), [1, 2, 1, 1]);
    assert_eq!(lengths(1000, 2), [2, 2, 1]);
    assert!(batches(&[], 100, 2).is_empty());
}

#[test]
fn html_marks_carry_their_index_through_attributes_the_service_adds() {
    let segmented = Segmented::new("Call `a` then `b`.");
    assert_eq!(
        segmented.marked(Markup::Html),
        [
            "Call <span translate=\"no\" class=\"notranslate\" id=\"i0\">`a`</span> then <span translate=\"no\" class=\"notranslate\" id=\"i1\">`b`</span>."
        ]
    );
    let translation = "<span translate=\"no\" class=\"notranslate\" id=\"i1\" style=\"direction:rtl\">b</span> の後で <span id=\"i0\">a</span> を呼ぶ";
    assert_eq!(
        segmented
            .assemble(Markup::Html, &[translation.into()])
            .unwrap(),
        "`b` の後で `a` を呼ぶ"
    );
    // A mark of the other markup is a tag the service added.
    assert_eq!(
        segmented.assemble(Markup::Html, &["<x i=\"0\">a</x> <x i=\"1\">b</x>".into()]),
        None
    );
}
