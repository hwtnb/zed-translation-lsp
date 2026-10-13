# Anthropic API

[日本語](../ja/providers/anthropic.md)

Select the `anthropic` provider to translate with Claude through the Anthropic API, and save an API key in a file that the language server reads.

## Minimal settings

```json
{
  "lsp": {
    "translation-lsp": {
      "initialization_options": {
        "provider": { "name": "anthropic", "model": "claude-haiku-5-5", "effort": "low" }
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
| `provider.document` | None | Optional. An object holding the model and options for translations opened in a tab. Its values are not inherited from `provider.model`, `provider.effort`, or `provider.thinking`: a value omitted inside it takes the default in the rows below. Unknown fields inside it are settings errors. |
| `provider.document.model` | None | Required when `provider.document` is present. The rules of `provider.model` apply. |
| `provider.document.effort` | `low` | Optional. The same values as `provider.effort`. |
| `provider.document.thinking` | Depends on `provider.document.model` and `provider.document.effort` | Optional. The rule of `provider.thinking` applies to the values inside `provider.document`. |

`effort` sets how much work the model spends on a response; `low` is the lightest and cheapest. Every request sends `effort`, so `model` must support it; according to the API documentation, older models such as Claude Haiku 4.5 do not.

With `thinking` set to `false`, the model answers without a reasoning step. With `true`, the model decides when to reason before answering. Reasoning is never displayed, and its tokens count toward the cost and the output token limit. Some models do not accept requests without thinking: if you use another model and set `thinking` to `false` explicitly, the API can return HTTP 400. In that case, omit `thinking` or set it to `true`. The omitted value is chosen by comparing `model` with `claude-haiku-5-5` exactly.

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

The server reads the key only from the file `anthropic-api-key` in its credential directory, once at startup. The file holds the key alone; whitespace and line breaks around the key are ignored.

| OS | Key file |
| --- | --- |
| Windows | `%LOCALAPPDATA%\translation-lsp\anthropic-api-key` |
| macOS | `~/Library/Application Support/translation-lsp/anthropic-api-key` |
| Linux | `$XDG_CONFIG_HOME/translation-lsp/anthropic-api-key`, or `~/.config/translation-lsp/anthropic-api-key` when unset |

Create the key in the [Claude Console](https://platform.claude.com/settings/keys) for a single workspace. The server sends no workspace ID, so the API refuses every request made with a key that is not scoped to one workspace. You can give the key an expiration; a request with an expired key fails as a rejected key.

On macOS, copy the key to the clipboard, then run these commands in a terminal:

```sh
dir=~/Library/Application\ Support/translation-lsp
mkdir -p "$dir" && chmod 700 "$dir"
(umask 077; pbpaste > "$dir/anthropic-api-key")
chmod 600 "$dir/anthropic-api-key"
```

On Linux, run these commands, paste the key into the terminal, press Enter, then press Ctrl+D:

```sh
dir="${XDG_CONFIG_HOME:-$HOME/.config}/translation-lsp"
mkdir -p "$dir" && chmod 700 "$dir"
(umask 077; cat > "$dir/anthropic-api-key")
chmod 600 "$dir/anthropic-api-key"
```

On Windows, copy the key to the clipboard, then run these commands in PowerShell:

```powershell
$dir = "$env:LOCALAPPDATA\translation-lsp"
New-Item -ItemType Directory -Force $dir | Out-Null
Get-Clipboard | Set-Content -Encoding ascii "$dir\anthropic-api-key"
```

With these commands the key is not part of a command, so it does not enter the shell history.

On macOS and Linux, the directory must have mode `0700` and the file mode `0600`, exactly, and neither may be a symbolic link. On Windows, neither may be a symbolic link; the server does not check who can access them.

To replace the key, write the new key to the same file, then restart the language server.

The server reads no environment variable for the key, so the key is not written in the Zed settings. It also ignores `ANTHROPIC_API_KEY`: Claude Code prefers that variable over a subscription login, so exporting it for your whole shell would move your ordinary Claude Code usage to API billing.

Without a usable key the server still starts, and a translation request returns a settings error without contacting the API. The error says whether the file is missing, the file or its directory is not private, or the file is empty or holds something besides the key.

## Cost and usage

API usage is billed to the Claude Console organization that owns the key. It is separate from the usage included in a Claude subscription. With `provider.document`, tab translations are billed at the rates of the model named there.

Automatic comment Hover and Markdown selection translation send requests without a confirmation, as described under [Usage](../usage.md).

There is no account menu or usage command for this provider.

## What is sent

The text submitted for translation is described under [What is sent](../usage.md#what-is-sent).

Each request contains the translation instructions and the text to translate as a single user message. Conversation history is not sent. The request asks for at most 64,000 output tokens, or fewer when `translation_limits.output_kib` is smaller; a translation that reaches that limit is reported as an error instead of being shown incomplete.

## Errors and pauses

A billing error, a rejected key, a key that is not scoped to one workspace, an unknown model, and a permission error stop automatic translation and report the HTTP status, the API's error type, and the request ID. After fixing billing in the Claude Console or correcting `provider.model` or `provider.document.model`, run a selection or document translation explicitly; automatic translation resumes when that succeeds. Changing the key or any setting requires a language server restart.

Rate limits and overload are retried once as described under [Automatic retries](../behavior.md#automatic-retries). If the model declines a text, the request fails with the reported category, is not retried, and does not pause automatic translation.

The rules shared by every provider are described under [Pauses](../behavior.md#pauses) and [Automatic retries](../behavior.md#automatic-retries).

## Stored data

The key is stored as plain text in the file `anthropic-api-key` that you create as described under [Authentication](#authentication), in the directory where the ChatGPT provider stores its credentials. The server only reads that file: it does not create, change, or delete the file or its directory. The server does not write the key to logs, error messages, or anywhere else on disk.
