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

A selection of up to three words and 64 characters on one line that does not end with `.`, `!`, `?`, `。`, `！`, or `？`, such as `retain`, `take over`, or the identifier `retain_completed`, is looked up instead of translated. Words are counted by the Unicode word boundaries, which also divide languages written without spaces: in Japanese, `保持する` is two words and `設定を保存する` four, because particles and endings count as words. Hover over it to read a dictionary entry: the term, its meanings with their parts of speech, with the meaning it has in its line first, the words of an identifier or compound and what they mean together, and one example sentence with its translation. This applies to automatic Markdown selections, `Translate selection to …`, and `… and open in tab`. A selection of the whole document is translated, as `Translate document to … and open in tab` translates it. A comment read by Comment Hover is always translated, even when it is one word.

The labels of an entry, such as `Example:`, are in English. The meanings and parts of speech are in the target language.

## Opening a translation in a tab

In Markdown or Plain Text, use `Translate selection to … and open in tab` to read the translation in a separate document. With no selection, the action becomes `Translate document to … and open in tab` and translates the whole document. Both operations use `translation_limits.input_kib.document`, which defaults to 1 MiB. The client must support `window/showDocument`.

Translations opened in a tab acknowledge the command immediately and open the result when translation completes. Full documents and selections both default to a three-minute execution budget, in addition to time spent in the queue. Text length does not automatically extend it. Use the range or all-translations cancellation Code Actions to stop pending work. Editing or closing the source document also prevents the result from opening.

Markdown translations open as temporary `.md` files. Run `markdown: open preview` from the Command Palette to see the rendered document and return to its source tab when needed. Plain Text translations open as `.txt` files. The extension leaves your normal Markdown opening preference unchanged. Existing translation tabs are snapshots. Edit the original and run the action again to obtain a new translation.

Use **Save As** to keep a translation before restarting the language server or closing Zed. The server removes its temporary files on normal shutdown; an abnormal exit can leave them in the system temporary directory. Saved copies are yours to keep.

Relative links and images resolve from the temporary file location. To use the original relative paths, save the translation beside the source.

A translation is model output. Text in the source that addresses the model can make the translation contain links and images that the source does not have. `markdown: open preview` loads images from their `http` and `https` URLs as it renders them, so the server named in such a URL can receive any text of the translation placed in the URL. When someone else wrote the source, read the translation tab as text before opening the preview.

Reopening the same cached tab translation reuses its temporary file and focuses that document. If the file was deleted or its saved contents changed, the server creates a fresh file without overwriting it. A newly translated result gets a separate file. Temporary files are still removed on server exit; copies saved with Save As are preserved.

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

Completed translations are displayed as text. The model receives the selected passage as a whole and is instructed to return only its translation, preserving meaning, code, identifiers, and URLs. The passage is marked as the text to translate, and the model is instructed to translate any instructions or questions in it instead of following or answering them. The server does not split the translation into JSON fragments or reject it for changes in Markdown formatting or line breaks. These instructions do not guarantee translation accuracy.

To look up a term, the model receives the term and, marked separately, the line it is on, at most 200 characters on each side of it. It is instructed to reply with a JSON object of meanings, word parts, and an example. The server builds the entry from that object and escapes every text of the reply, so the reply cannot add links, images, or other formatting to the Hover. A reply that is not such an object is reported as an error; retry it from Code Actions.

Explicit selection and document actions send the requested text even when it is indented or contains only code. The model is instructed to preserve code and identifiers. Markdown prose eligibility limits only automatic background translation; the extension does not report the original text as a successful translation without contacting the provider.

The extension submits the extracted comment body, the selected text, or the whole document when you explicitly choose document translation. For a term, it also submits the term's line. It does not submit conversation history or write translations back into the source document.
