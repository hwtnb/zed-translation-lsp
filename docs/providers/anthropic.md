# Anthropic API

[日本語](../ja/providers/anthropic.md)

Select the `anthropic` provider to translate with Claude through the Anthropic API, and save an API key from the `Anthropic API key…` code action.

## Minimal settings

```json
{
  "lsp": {
    "translation-lsp": {
      "initialization_options": {
        "provider": {
          "name": "anthropic",
          "model": "claude-haiku-5-5",
          "effort": "low"
        }
      }
    }
  }
}
```

The other `initialization_options` described under [Configuration](../configuration.md) apply unchanged. The API key is not part of the settings; save it as described under [Authentication](#authentication).

## Settings

| Setting | Default | Rules |
| --- | --- | --- |
| `provider.name` | None | `"anthropic"` selects this provider. |
| `provider.model` | `claude-haiku-5-5` | Optional. Empty values, values that contain whitespace or control characters, and values longer than 256 bytes are settings errors. The value is sent as written, without substituting another model. |
| `provider.effort` | `low` | Optional. One of `low`, `medium`, `high`, `xhigh`, or `max`. The ChatGPT-only values `none` and `minimal` are settings errors. |
| `provider.thinking` | Depends on `model` and `effort` | Optional. When omitted, it is `false` if `model` is `claude-haiku-5-5` and `effort` is `low`, `medium`, or `high`, and `true` otherwise. An explicit `false` together with `effort` `xhigh` or `max` is a settings error, because the API rejects that combination. |
| `provider.speed` | None | Optional. `standard` or `fast`, sent as the request's `speed` together with the header `anthropic-beta: fast-mode-2026-02-01`, also for `standard`. When omitted, neither is sent. `null` and the ChatGPT-only values such as `priority` are settings errors. |
| `provider.document` | None | Optional. An object holding the model and options for translations opened in a tab. Its values are not inherited from `provider.model`, `provider.effort`, `provider.thinking`, or `provider.speed`: a value omitted inside it takes the default in the rows below. Unknown fields inside it are settings errors. |
| `provider.document.model` | None | Required when `provider.document` is present. The rules of `provider.model` apply. |
| `provider.document.effort` | `low` | Optional. The same values as `provider.effort`. |
| `provider.document.thinking` | Depends on `provider.document.model` and `provider.document.effort` | Optional. The rule of `provider.thinking` applies to the values inside `provider.document`. |
| `provider.document.speed` | None | Optional. The same values as `provider.speed`. |

`effort` sets how much work the model spends on a response; `low` is the lightest and cheapest. Every request sends `effort`, so `model` must support it; according to the API documentation, older models such as Claude Haiku 4.5 do not.

With `thinking` set to `false`, the model answers without a reasoning step. With `true`, the model decides when to reason before answering. Reasoning is never displayed, and its tokens count toward the cost and the output token limit. Some models do not accept requests without thinking: if you use another model and set `thinking` to `false` explicitly, the API can return HTTP 400. In that case, omit `thinking` or set it to `true`. The omitted value is chosen by comparing `model` with `claude-haiku-5-5` exactly.

`speed` set to `fast` runs the model with faster output at a higher price. According to [Fast mode](https://platform.claude.com/docs/en/build-with-claude/fast-mode), it is a research preview on the Claude API only, for Claude Opus 5.5, Claude Opus 5, and Claude Opus 4.8, and access is requested from your account manager or through the waitlist. With another model, the API returns an error for `fast` or processes the request at the standard speed. Exceeding the Fast mode rate limits returns HTTP 429 or 529, which is handled like any rate limit or overload.

When a translation with `speed` completes, the server writes the requested value and the `usage.speed` the response reports to the language server log, for example `translation-lsp: Anthropic speed requested fast, served fast`. In Zed, run `dev: open language server logs` from the Command Palette to read it. When the response reports no value, the line says `(not reported)`. The source text, the translation, and the key are not written.

Unknown fields are rejected: there is no setting for the API key or the API endpoint. Restart the language server after changing these settings.

### Tab translations with another model

```json
"provider": {
  "name": "anthropic",
  "model": "claude-haiku-5-5",
  "effort": "low",
  "document": { "model": "claude-sonnet-5-5" }
}
```

The values that `provider.document` accepts, and the defaults they take when omitted, are in the table above.

What `provider.document` changes for every provider is described under [Tab translations with another model](../configuration.md#tab-translations-with-another-model).

## Authentication

In a supported document, run `editor: toggle code actions` from Zed's Command Palette, choose `Anthropic API key…`, then run `Save Anthropic API key`. A terminal window of the OS opens and asks for the key. Paste the key there and press Enter; the terminal does not show what you paste. When the terminal reports that the key was saved, press Enter and return to Zed: the next translation uses the key, without a language server restart. Pressing Enter without a key saves nothing.

Create the key in the [Claude Console](https://platform.claude.com/settings/keys) for a single workspace. The server sends no workspace ID, so the API refuses every request made with a key that is not scoped to one workspace. You can give the key an expiration; a request with an expired key fails as a rejected key.

While no usable key is saved, `Anthropic API key…` is the only code action of the server. While one is saved, the menu has these items instead:

- `Replace Anthropic API key`: open the same terminal to save another key in place of the saved one.
- `Remove Anthropic API key`: after you confirm, delete the key file from this computer, cancel the translations in progress, and discard the translations already shown. The key stays valid until you delete it in the Claude Console.

The server opens Terminal on macOS and a new console window on Windows. On Linux it tries these in order and uses the first that starts: `xdg-terminal-exec`, `x-terminal-emulator`, `gnome-terminal`, `ptyxis`, `konsole`, `xfce4-terminal`, `kitty`, `alacritty`, `foot`, `wezterm`, `xterm`.

The notice that follows `Save Anthropic API key` or `Replace Anthropic API key` shows a command: the path of the language server, then `anthropic-key`. When no window appears, run that command in a terminal yourself. That is the case on Linux when none of the terminals above starts, and in remote development, where the server runs on the remote machine and opens no window on yours; run the command in a terminal on the remote machine. In PowerShell, put `&` before a path in quotation marks. The command asks for the key in the terminal it runs in and fails where there is none; it does not read the key from a pipe.

The key is saved in the file `anthropic-api-key` in the server's credential directory, and the server checks the directory and the file each time it needs the key. It reads the file again when the modification time, the size, or, on macOS and Linux, the inode of the file has changed. The file holds the key alone; whitespace and line breaks around the key are ignored. A file that you create or edit yourself is read the same way.

| OS | Key file |
| --- | --- |
| Windows | `%LOCALAPPDATA%\translation-lsp\anthropic-api-key` |
| macOS | `~/Library/Application Support/translation-lsp/anthropic-api-key` |
| Linux | `$XDG_CONFIG_HOME/translation-lsp/anthropic-api-key`, or `~/.config/translation-lsp/anthropic-api-key` when unset |

On macOS and Linux, the directory must have mode `0700` and the file mode `0600`, exactly, and neither may be a symbolic link. On Windows, neither may be a symbolic link; the server does not check who can access them. The command creates the directory and the file that way. It does not change a directory that exists with other permissions: it saves nothing and reports what the directory must be.

The key reaches the file from the terminal. It is not written in the Zed settings and does not pass through an environment variable or through the messages between Zed and the server. The server also ignores `ANTHROPIC_API_KEY`: Claude Code prefers that variable over a subscription login, so exporting it for your whole shell would move your ordinary Claude Code usage to API billing.

Without a usable key the server still starts, and a translation request returns a settings error without contacting the API. The error says whether no key is saved, the file or its directory is not private, or the file is empty or holds something besides the key.

## Cost and usage

API usage is billed to the Claude Console organization that owns the key. It is separate from the usage included in a Claude subscription. With `provider.document`, tab translations are billed at the rates of the model named there.

Automatic comment Hover and Markdown selection translation send requests without a confirmation, as described under [Usage](../usage.md).

The `Anthropic API key…` menu saves, replaces, and removes the key. It has no usage command; usage is shown in the Claude Console.

## What is sent

The text submitted for translation is described under [What is sent](../usage.md#what-is-sent).

Each request contains the translation instructions as the system prompt and the text to translate as a single user message. Conversation history is not sent. The request asks for at most 64,000 output tokens, or fewer when `translation_limits.output_kib` is smaller; a translation that reaches that limit is reported as an error instead of being shown incomplete.

## Errors and pauses

A billing error, a rejected key, a key that is not scoped to one workspace, an unknown model, and a permission error stop automatic translation and report the HTTP status, the API's error type, and the request ID. After fixing billing in the Claude Console or correcting `provider.model` or `provider.document.model`, run a selection or document translation explicitly; automatic translation resumes when that succeeds. A stop for a rejected key, or for a key that is not scoped to one workspace, ends when you save another key from `Anthropic API key…`: the stop belongs to the key it happened with.

Saving, replacing, or removing the key needs no language server restart. Translations and failures kept for one key are not shown for another, and a translation that is in progress when the key changes is discarded when it completes. Changing a setting requires a language server restart.

Rate limits and overload are retried once as described under [Automatic retries](../behavior.md#automatic-retries). If the model declines a text, the request fails with the reported category, is not retried, and does not pause automatic translation.

The rules shared by every provider are described under [Pauses](../behavior.md#pauses) and [Automatic retries](../behavior.md#automatic-retries).

## Stored data

The key is stored as plain text in the file `anthropic-api-key` described under [Authentication](#authentication), in the directory where the ChatGPT provider stores its credentials. The `anthropic-key` command, which the menu runs in a terminal, writes the file, and creates the directory when it is missing. `Remove Anthropic API key` deletes the file and nothing else. Otherwise the language server only reads the file.

To open Terminal on macOS, the server writes a script to the temporary directory. The script holds the path of the language server and no key, and deletes itself when Terminal runs it.

The server does not write the key to logs, error messages, notices, or anywhere else on disk.
