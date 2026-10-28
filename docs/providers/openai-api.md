# OpenAI API

[日本語](../ja/providers/openai-api.md)

Select the `openai_api` provider to translate through the OpenAI API with an API key, and save the key from the `OpenAI API key…` code action. To use a ChatGPT plan instead, select the [ChatGPT](chatgpt.md) provider.

## Minimal settings

```json
{
  "lsp": {
    "translation-lsp": {
      "initialization_options": {
        "provider": {
          "name": "openai_api",
          "model": "gpt-6-luna",
          "effort": "low"
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
| `provider.name` | None | `"openai_api"` selects this provider. |
| `provider.model` | `gpt-6-luna` | Optional. Empty values, values that contain whitespace or control characters, and values longer than 256 bytes are settings errors. The value is sent as written, without substituting another model. |
| `provider.effort` | `low` | Optional. One of `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, or `max`. Every request sends it as the reasoning effort. |
| `provider.speed` | None | Optional. One of `auto`, `default`, `flex`, `scale`, `priority`, `fast`, or `ultrafast`, sent as the request's `service_tier`. When omitted, no `service_tier` is sent. `null` and the Anthropic-only `standard` are settings errors. |
| `provider.document` | None | Optional. An object holding the model and options for translations opened in a tab. Its values are not inherited from `provider.model`, `provider.effort`, or `provider.speed`: a value omitted inside it takes the default in the rows below. Unknown fields inside it are settings errors. |
| `provider.document.model` | None | Required when `provider.document` is present. The rules of `provider.model` apply. |
| `provider.document.effort` | `low` | Optional. The same values as `provider.effort`. |
| `provider.document.speed` | None | Optional. The same values as `provider.speed`. |

The settings take the same values as those of the ChatGPT provider. Each model accepts only some of the effort values; according to the OpenAI documentation, some models reject `none` or `minimal`. Models, effort values, or speeds that your organization cannot use return an error.

Unknown fields are rejected: there is no setting for the API key or the API endpoint. To send requests elsewhere, use the [OpenAI-compatible](openai-compatible.md) provider. Restart the language server after changing these settings.

### Tab translations with another model

```json
"provider": {
  "name": "openai_api",
  "model": "gpt-6-luna",
  "effort": "low",
  "document": { "model": "gpt-6.1-sol", "effort": "medium" }
}
```

The values that `provider.document` accepts, and the defaults they take when omitted, are in the table above. The models and efforts above are examples.

What `provider.document` changes for every provider is described under [Tab translations with another model](../configuration.md#tab-translations-with-another-model).

## Authentication

In a supported document, run `editor: toggle code actions` from Zed's Command Palette, choose `OpenAI API key…`, then run `Save OpenAI API key`. A terminal window of the OS opens and asks for the key. Paste the key there and press Enter; the terminal does not show what you paste. When the terminal reports that the key was saved, press Enter and return to Zed: the next translation uses the key, without a language server restart. Pressing Enter without a key saves nothing.

Create the key on the [API keys page](https://platform.openai.com/api-keys) of the OpenAI Platform.

While no usable key is saved, `OpenAI API key…` is the only code action of the server. While one is saved, the menu has these items instead:

- `Replace OpenAI API key`: open the same terminal to save another key in place of the saved one.
- `Remove OpenAI API key`: after you confirm, delete the key file from your computer, cancel the translations in progress, and discard the translations already shown. The key stays valid until you delete it in the OpenAI Platform.

While a key is saved, the menu appears in Markdown and Plain Text documents, and in other documents only in a range whose translation failed, such as with a rejected key. To replace or remove the key at another time, open a Markdown or Plain Text file.

The terminal opens and the key is saved as for the Anthropic API key, described under [Authentication](anthropic.md#authentication) of that page. The notice that follows `Save OpenAI API key` or `Replace OpenAI API key` shows the command to run by hand when no window appears: the path of the language server, then `api-key https://api.openai.com`.

The key is saved in the file `api-key-https-api.openai.com-443` in the server's credential directory, with the same checks of the directory and the file as the Anthropic key file. The [OpenAI-compatible](openai-compatible.md) provider uses the same file when its `base_url` is on `https://api.openai.com`.

| OS | Key file |
| --- | --- |
| Windows | `%LOCALAPPDATA%\translation-lsp\api-key-https-api.openai.com-443` |
| macOS | `~/Library/Application Support/translation-lsp/api-key-https-api.openai.com-443` |
| Linux | `$XDG_CONFIG_HOME/translation-lsp/api-key-https-api.openai.com-443`, or `~/.config/translation-lsp/api-key-https-api.openai.com-443` when unset |

The key reaches the file from the terminal. It is not written in the Zed settings and does not pass through an environment variable or through the messages between Zed and the server. The server does not read `OPENAI_API_KEY`.

Without a usable key the server still starts, and a translation request returns a settings error without contacting the API.

## Cost and usage

API usage is billed to the OpenAI Platform organization that owns the key. It is separate from the usage included in a ChatGPT plan. With `provider.document`, tab translations are billed at the rates of the model named there.

Automatic comment Hover and Markdown selection translation send requests without a confirmation, as described under [Usage](../usage.md).

The `OpenAI API key…` menu saves, replaces, and removes the key. It has no usage command; usage is shown in the OpenAI Platform.

## What is sent

The text submitted for translation is described under [What is sent](../usage.md#what-is-sent).

Each request goes to `https://api.openai.com/v1/responses` with `store` set to `false`. It contains the translation instructions as `instructions` and the text to translate, between `<source_text>` tags, as a single user input. To look up a word, the instructions are the dictionary instructions, and the input holds the term between `<term>` tags and its line between `<line>` tags. Conversation history is not sent. A translation read in Hover asks for at most 16,384 output tokens plus 4 tokens per byte of the source, or of the line of a term, or fewer when the number of bytes in `translation_limits.output_kib` (1,024 per KiB) is smaller, so that text in a comment cannot make the model write at length; reasoning counts toward this limit. A translation opened in a tab sends no token limit and is bounded by `translation_limits.output_kib`. A translation that reaches its limit is reported as an error instead of being shown incomplete.

## Errors and pauses

A rejected key, an exhausted quota, an unknown model, a value the model does not accept for `effort` or `speed`, and a permission error stop automatic translation and report the HTTP status, the error code, and the request ID. After correcting billing in the OpenAI Platform or the settings, run a selection or document translation explicitly; automatic translation resumes when that succeeds. A stop for a rejected key ends when you save another key from `OpenAI API key…`: the stop belongs to the key it happened with.

Saving, replacing, or removing the key needs no language server restart. Translations and failures kept for one key are not shown for another, and a translation that is in progress when the key changes is discarded when it completes. Changing a setting requires a language server restart.

Rate limits and server errors are retried once as described under [Automatic retries](../behavior.md#automatic-retries). If the response is stopped by the content filter, the request fails with that category, is not retried, and does not pause automatic translation.

The rules shared by every provider are described under [Pauses](../behavior.md#pauses) and [Automatic retries](../behavior.md#automatic-retries).

## Stored data

The key is stored as plain text in the file `api-key-https-api.openai.com-443` described under [Authentication](#authentication), in the directory where the ChatGPT provider stores its credentials. The `api-key` command, which the menu runs in a terminal, writes the file, and creates the directory when it is missing. `Remove OpenAI API key` deletes the file and nothing else. Otherwise the language server only reads the file.

To open Terminal on macOS, the server writes a script to the temporary directory. The script holds the path of the language server and the address `https://api.openai.com`, and no key, and deletes itself when Terminal runs it.

The server does not write the key to logs, error messages, notices, or anywhere else on disk.
