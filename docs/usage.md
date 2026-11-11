# Usage

[日本語](ja/usage.md)

This page describes how to request, read, cancel, and retry translations, and what text is sent. The rules for accepting, keeping, and caching work are described under [Behavior](behavior.md).

## Comment Hover

Hover over a supported code comment, or place the cursor in it and run Zed's `editor: hover`. Adjacent line comments of the same kind and indentation are translated together; blank lines or code split the group. Each block comment is handled separately. Comment markers and one following ASCII space are removed from the text submitted for translation.

Zed controls Hover placement and width. The extension cannot guarantee placement above the text or that the popup leaves all source text visible.

## Markdown selection

With a provider ready, as described under [Set up a provider](../README.md#set-up-a-provider), and `features.automatic_selection` set to `true`, select Markdown prose and keep the selection stable. The default native wait is 450 ms after Zed reports the selection; Zed also waits before reporting it. Translation starts in the background; Hover over that range to read it. There is no background completion notification.

Selections containing only code, link destinations, or whitespace do not start translation. A fragment that changes the interpretation of Markdown by cutting through a code delimiter or another protected span is also excluded. Include the complete delimiters when selecting, or use the manual action.

Selecting prose just to copy it can also send it for translation.

## Manual selection translation

To translate immediately or recover from a background failure, use the `Translate selection to …` Code Action. Manual execution shows `Translation ready. Hover over the selected text to view it.` when complete. Selection translation is also available in code languages and other supported documents. With the ChatGPT provider, if disconnected or plan use is disabled, connect or grant permission through `ChatGPT account…` first. With the Anthropic API or OpenAI API provider, if no key is saved, save one through `Anthropic API key…` or `OpenAI API key…` first. The Hover translation action is hidden when there is no selection.

## Looking up a word

A selection of up to three words and 64 characters on one line that does not end with `.`, `!`, `?`, `。`, `！`, or `？`, such as `retain`, `take over`, or the identifier `retain_completed`, is looked up instead of translated. Words are counted by the Unicode word boundaries, which also divide languages written without spaces: in Japanese, `保持する` is two words and `設定を保存する` four, because particles and endings count as words. Hover over it to read a dictionary entry: the term, its meanings with their parts of speech, with the meaning it has in its line first, the words of an identifier or compound and what they mean together, and one example sentence with its translation. This applies to automatic Markdown selections, `Translate selection to …`, and `… and open in tab`. A selection of the whole document is translated, as `Translate document to … and open in tab` translates it. A comment read by Comment Hover is always translated, even when it is one word. The DeepL API and Google Cloud Translation write no dictionary entries: with them, such a selection is translated like any other.

The labels of an entry, such as `Example:`, are in English. The meanings and parts of speech are in the target language.

## How Hover shows a translation

Hover shows the title `Translation LSP` and, below a rule, the translation. With `hover.header` set to `false`, it shows only the translation. See [Hover](configuration.md#hover).

The translation of a comment, of a selection in code, or of Plain Text shows as Markdown that keeps this formatting of the translation:

- code spans, and code blocks fenced or indented after a blank line
- headings that start with `#` or are underlined with `===` or `---`
- lists that start with `-`, `*`, `+`, or a number, block quotes, tables, and rules
- emphasis with `*` and `**`
- links written as `[text](URL)` to a URL that the original text contains

Every other character shows as written: `__init__`, `--verbose`, `\d+\.\d+`, `Vec<T>`, `<!-- note -->`, `~a~`, and `[^1]` stay as they are, and quotes and `...` are not replaced with typographic ones. Each line break of the translation shows as a line break, and one blank line or more separates paragraphs. Spaces at the start of a line inside a paragraph do not show. A translation whose formatting cannot be kept this way shows its lines as written, without formatting.

Links open only URLs that the original text contains: text in the source that addresses the model can make the translation contain links that the source does not have. A URL of the original ends before the punctuation of the sentence around it, such as `.` or an unmatched `)`. In the translation:

- A URL written alone is a link when it starts with a URL of the original and the text ends after it, or continues with whitespace or a character outside ASCII, possibly after punctuation. In `詳細はhttps://example.com/docsを参照`, `https://example.com/docs` is a link.
- A link written as `[text](URL)` or `<URL>` stays when its URL is exactly one of the original.
- A stretch of text without spaces that contains two URLs, such as a URL with another URL in its query, makes no link.

Every other URL shows as text with an invisible word joiner (U+2060) after its `:`, and text copied from Hover includes it. URLs inside code show as written and are not links.

With `hover.format` set to `"markdown"`, the translation of a Markdown document shows as its own Markdown. Its links and images stay when each destination is exactly one of the original, such as the same relative path, or starts with `#`. If any other destination appears, or a URL that escapes or character references divide, such as `https&#58;//`, the translation shows in a code block as written instead. Its URLs written alone follow the rules above, and URLs in the text of an image are never links. A dictionary entry shows as described under [Looking up a word](#looking-up-a-word). With `hover.format` set to `"text"`, every translation except a dictionary entry shows in a code block as written.

## Opening a translation in a tab

In Markdown or Plain Text, use `Translate selection to … and open in tab` to read the translation in a separate document. With no selection, the action becomes `Translate document to … and open in tab` and translates the whole document. Both operations use `translation_limits.input_kib.document`, which defaults to 1 MiB. The client must support `window/showDocument`.

Translations opened in a tab acknowledge the command immediately and open the result when translation completes. Full documents and selections both default to a three-minute execution budget, in addition to time spent in the queue. Text length does not automatically extend it. Use the range or all-translations cancellation Code Actions to stop pending work. Editing or closing the source document also prevents the result from opening.

Markdown translations open as temporary `.md` files. Run `markdown: open preview` from the Command Palette to see the rendered document and return to its source tab when needed. Plain Text translations open as `.txt` files. The extension leaves your normal Markdown opening preference unchanged. Existing translation tabs are snapshots. Edit the original and run the action again to obtain a new translation.

Use **Save As** to keep a translation before restarting the language server or closing Zed. The server removes its temporary files on normal shutdown; an abnormal exit can leave them in the system temporary directory. Saved copies are yours to keep.

Relative links and images resolve from the temporary file location. To use the original relative paths, save the translation beside the source.

A translation is model output. Text in the source that addresses the model can make the translation contain links and images that the source does not have. `markdown: open preview` loads images from their `http` and `https` URLs as it renders them, so the server named in such a URL can receive any text of the translation placed in the URL. When someone else wrote the source, read the translation tab as text before opening the preview.

Reopening the same cached tab translation reuses its temporary file and focuses that document. If the file was deleted or its saved contents changed, the server creates a fresh file without overwriting it. A newly translated result gets a separate file. Temporary files are still removed on server exit; copies saved with Save As are preserved.

## CodeLens

With `"code_lens": "on"` in Zed's settings, the server shows short commands on a line above text. Zed's default is `"off"`. Zed shows the commands of one line together, separated by `|`.

| Where | Line |
| --- | --- |
| A code comment with no translation | `Translate \| Settings` |
| A translated range | The first line of the translation, up to 72 characters, then `Settings` in code |
| A failed range | `Translation failed \| Retry`, then `Settings` in code |
| A range being translated | `Translating… \| Cancel`, then `Settings` in code |
| Above the first line of Markdown and Plain Text | `Translation LSP: Translate in tab \| Auto-translate: On \| <account menu> \| Zed settings` |

`Translate` translates the comment as `Translate selection to …` does, and appears with a provider ready and `selection_action` on. Translated, failed, and running ranges come from comment Hover, selections, and `Translate`; tab translations get no line. Editing the document removes the lines of the ranges whose translations it invalidates.

Click the first line of a translation to read all of it in a card, or `Translation failed` to read the reason. The card stays until you close it. Its `Retry translating this text` button translates the range again. Zed's copy button on the card copies the text as Markdown, with backslashes before symbols and `---` lines before and after it.

`Translate in tab` translates the whole document as `Translate document to … and open in tab` does. It appears with a provider ready and `document_action` on, in a client that supports `window/showDocument`. The account menu item is the provider's account menu, such as `ChatGPT account…`. `Zed settings` opens Zed's settings file, also only in a client that supports `window/showDocument`.

`Settings` opens a menu with the same items: `Turn off automatic translation` or `Turn on automatic translation`, the account menu, and `Open Zed settings file`. Turning automatic translation off stops comment Hover and automatic Markdown selection from starting translations until the language server restarts, and cancels the translations they started that nothing else asked for. Translations already finished still appear on Hover, and `Translate`, Code Actions, and tab translations keep working. `Auto-translate: On` above Markdown and Plain Text turns it off the same way, and then reads `Auto-translate: Off`.

In a file with many comments, each comment gets a line. With [`features.comment_lens`](configuration.md#features) set to `false`, comments without a translation, a failure, or one in progress get no line, and the menu moves above the first line, as in Markdown.

## Status and notifications

While translation is queued or running, Zed shows a spinner and text directly in the left status bar. `Queued: 1` means one queued translation. `Translating: 1, queued: 0` means one translation is running with none queued. Requests for the same range count as one translation. Its tab translation counts separately with `provider.document`, and with the Anthropic API, OpenAI API, and OpenAI-compatible providers when the range request was already sent. Other language-server activity can take priority in this shared area.

The spinner disappears when work ends, including cancellation and timeout. Hover over the original text to check the result. On clients that support server status, translation failures and timeouts leave a warning that clears on retry, editing, or closing the document. Clients without these display capabilities retain Hover and explicit-operation notifications.

Manual completion and failure notifications identify the file name and lines. Automatic completion does not produce a popup. Check error notifications for failed manual translation or management operations.

When the comments of a document cannot be read, the server shows one of the following warnings, once per document until the document is reopened. `<file>` is the file name. See [Documents whose comments cannot be read](languages.md#documents-whose-comments-cannot-be-read).

| Warning | Cause |
| --- | --- |
| `Reading comments in <file> timed out. Comments in this file are not recognized until you reopen it. To allow more time, adjust comment_parse_timeout_ms in Zed settings.` | The reading took longer than `comment_parse_timeout_ms`. |
| `Reading comments in <file> failed. Comments in this file are not recognized until you reopen it.` | The reading failed inside the server. |

## Canceling and retrying

To stop one accepted translation, place the cursor or a selection in its range and choose `Cancel translation of lines …` from Code Actions. With `provider.document`, a tab translation is listed as `Cancel tab translation of lines …`, so one range can offer both actions; canceling one leaves the other running.

`Cancel all translations` stops all translation work without canceling an account menu. Old cancellation actions cannot cancel a later request for the same range. The server also accepts progress cancellation for all translations, if the client exposes that control. Zed's progress control and its refresh of the listed actions have not been verified in the editor; use Code Actions as the primary cancellation route.

To retry a failed range without selecting its text again, place the cursor inside it and choose `Retry translation of lines …` from Code Actions. The action retries the recorded range as an explicit request, including after a usage-limit pause. Hover alone continues to show the failure.

## What is sent

Hover shows completed translations as described under [How Hover shows a translation](#how-hover-shows-a-translation). The model receives the selected passage as a whole and is instructed to return only its translation, preserving meaning, code, identifiers, and URLs. The passage is marked as the text to translate, and the model is instructed to translate any instructions or questions in it instead of following or answering them. The server does not split the translation into JSON fragments or reject it for changes in Markdown formatting or line breaks. These instructions do not guarantee translation accuracy. The DeepL API and Google Cloud Translation take no instructions: the server sends the text to them in segments, with code spans and URLs marked, and puts the translation together again, as described under [What is sent](providers/deepl.md#what-is-sent) of the DeepL API page.

To look up a term, the model receives the term and, marked separately, the line it is on, at most 200 characters on each side of it. It is instructed to reply with a JSON object of meanings, word parts, and an example. The server builds the entry from that object and escapes every text of the reply, so the reply cannot add links, images, or other formatting to the Hover. A reply that is not such an object is reported as an error; retry it from Code Actions.

Explicit selection and document actions send the requested text even when it is indented or contains only code. The model is instructed to preserve code and identifiers. Markdown prose eligibility limits only automatic background translation; the extension does not report the original text as a successful translation without contacting the provider.

The extension submits the extracted comment body, the selected text, or the whole document when you explicitly choose document translation. For a term, it also submits the term's line. It does not submit conversation history or write translations back into the source document.
