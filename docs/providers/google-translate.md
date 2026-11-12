# Google Cloud Translation

[日本語](../ja/providers/google-translate.md)

Select the `google_translate` provider to translate through Google Cloud Translation with an API key, and save the key from the `Google Cloud API key…` code action. The provider uses the Basic edition of the API (v2), which takes API keys. The Advanced edition (v3), its Translation LLM model, glossaries, and custom models need other credentials and are not supported.

Google Cloud Translation is a translation service, not a language model: it takes no instructions. The server therefore prepares the text itself, as described under [What is sent](#what-is-sent), and a selected word is translated instead of being looked up in a dictionary entry. To translate with Gemini instead, use the [OpenAI-compatible](openai-compatible.md) provider.

## Minimal settings

```json
{
  "lsp": {
    "translation-lsp": {
      "initialization_options": {
        "target_language": "ja",
        "provider": {
          "name": "google_translate"
        }
      }
    }
  }
}
```

The other settings in `initialization_options` work as described under [Configuration](../configuration.md). The API key is not part of the settings; save it as described under [Authentication](#authentication).

## Settings

| Setting | Default | Rules |
| --- | --- | --- |
| `provider.name` | None | `"google_translate"` selects this provider. |

The provider has no other fields. Unknown fields, such as `model` or `document`, are settings errors.

With this provider, `target_language` must be a language code, such as `ja`, `pt`, or `zh-CN`, and `source_language` must be `auto` or a language code. A language name such as `Japanese` is a settings error. The code is sent as written, with `_` written as `-`; with `auto`, the API identifies the language. Which codes the API takes is decided by Google: a code it does not take is reported as an error with the API's message. Google lists them on its [language support](https://docs.cloud.google.com/translate/docs/languages) page.

Restart the language server after changing these settings.

## Authentication

In a supported document, run `editor: toggle code actions` from Zed's Command Palette, choose `Google Cloud API key…`, then run `Save Google Cloud API key`. A terminal window of the OS opens and asks for the key. Paste the key there and press Enter; the terminal does not show what you paste. When the terminal reports that the key was saved, press Enter and return to Zed: the next translation uses the key, without a language server restart. Pressing Enter without a key saves nothing.

Create the key on the [Credentials page](https://console.cloud.google.com/apis/credentials) of the Google Cloud console, in a project where the Cloud Translation API is enabled. A key for the Gemini API is saved for another address and is not used by this provider.

While no usable key is saved, `Google Cloud API key…` is the only code action of the server. While one is saved, the menu has these items instead:

- `Replace Google Cloud API key`: open the same terminal to save another key in place of the saved one.
- `Remove Google Cloud API key`: after you confirm, delete the key file from your computer, cancel the translations in progress, and discard the translations already shown. The key stays valid until you delete it in the Google Cloud console.

While a key is saved, the menu appears in Markdown and Plain Text documents, and in other documents only in a range whose translation failed, such as with a rejected key. To replace or remove the key at another time, open a Markdown or Plain Text file.

The terminal opens and the key is saved as for the Anthropic API key, described under [Authentication](anthropic.md#authentication) of that page. The notice that follows `Save Google Cloud API key` or `Replace Google Cloud API key` shows the command to run by hand when no window appears: the path of the language server, then `api-key https://translation.googleapis.com`.

The key is saved in the file `api-key-https-translation.googleapis.com-443` in the server's credential directory, with the same checks of the directory and the file as the Anthropic key file.

| OS | Key file |
| --- | --- |
| Windows | `%LOCALAPPDATA%\translation-lsp\api-key-https-translation.googleapis.com-443` |
| macOS | `~/Library/Application Support/translation-lsp/api-key-https-translation.googleapis.com-443` |
| Linux | `$XDG_CONFIG_HOME/translation-lsp/api-key-https-translation.googleapis.com-443`, or `~/.config/translation-lsp/api-key-https-translation.googleapis.com-443` when unset |

The key reaches the file from the terminal. It is not written in the Zed settings and does not pass through an environment variable or through the messages between Zed and the server.

Without a usable key the server still starts, and a translation request returns a settings error without contacting the API.

## Cost and usage

Google charges the characters of the text it translates to the billing account of the key's project. The prices and the free usage are on Google's pricing page for Cloud Translation. Translations opened in a tab are sent and charged like any other.

Automatic comment Hover and Markdown selection translation send requests without a confirmation, as described under [Usage](../usage.md).

The `Google Cloud API key…` menu saves, replaces, and removes the key. It has no usage command; usage and quotas are shown in the Google Cloud console.

## What is sent

The text submitted for translation is described under [What is sent](../usage.md#what-is-sent). Because the API takes no instructions, the server sends that text in segments, as for the DeepL API: the lines of a paragraph are joined, headings, list items, quotes, and table cells are segments of their own, code blocks are not sent, and code spans and URLs are marked to stay untranslated. The rules are described under [What is sent](deepl.md#what-is-sent) of that page.

Here the segments are sent as HTML, with each code span or URL in `<span translate="no" class="notranslate">`, and the server puts the original text back in place of each mark. A translated paragraph is one line, however many lines the original had.

Each request goes to `https://translation.googleapis.com/language/translate/v2` with `format` set to `html`, and carries the key in the `X-goog-api-key` header, not in the address. One request holds at most 128 segments and 48 KiB of them, which the API translates independently of each other; a longer text is sent in several requests, one after another, and is shown only when every request has succeeded. A translation in which the API lost, repeated, or added a mark is reported as an error instead of being shown.

With `source_language` set to `auto`, the API reports the language it detected in each segment as `detectedSourceLanguage`. For comment Hover and automatic Markdown selection, the server shows nothing when every segment is in the target language: the language codes are compared without their region, so `en` is `en-US`. Chinese codes are compared in full, because the API takes `zh-CN` or `zh` for Chinese (Simplified) and `zh-TW` for Chinese (Traditional): `zh` and `zh-CN` are the same language, and `zh-TW` is not. When `source_language` names a language, the API reports none.

## Errors and pauses

A rejected key, a refusal for the project of the key (HTTP 403, such as when the Cloud Translation API or billing is not enabled, or the key may not call the API), an exhausted daily quota, and a language code that the API does not take (HTTP 400) stop automatic translation and report the HTTP status, the reason the API gives, and its message. After saving another key or correcting the project or the settings, run a selection or document translation explicitly; automatic translation resumes when that succeeds. A stop for a rejected key ends when you save another key from `Google Cloud API key…`: the stop belongs to the key it happened with.

Saving, replacing, or removing the key needs no language server restart. Translations and failures kept for one key are not shown for another, and a translation that is in progress when the key changes is discarded when it completes. Changing a setting requires a language server restart.

Rate limits and server errors are retried once as described under [Automatic retries](../behavior.md#automatic-retries). A translation that lost a mark is not retried and does not pause automatic translation.

The rules shared by every provider are described under [Pauses](../behavior.md#pauses) and [Automatic retries](../behavior.md#automatic-retries).

## Stored data

The key is stored as plain text in the file `api-key-https-translation.googleapis.com-443` described under [Authentication](#authentication), in the directory where the ChatGPT provider stores its credentials. The `api-key` command, which the menu runs in a terminal, writes the file, and creates the directory when it is missing. `Remove Google Cloud API key` deletes the file and nothing else. Otherwise the language server only reads the file.

To open Terminal on macOS, the server writes a script to the temporary directory. The script holds the path of the language server and the address `https://translation.googleapis.com`, and no key, and deletes itself when Terminal runs it.

The server does not write the key to logs, error messages, notices, or anywhere else on disk.
