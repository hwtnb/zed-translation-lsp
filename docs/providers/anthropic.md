# Anthropic API

[日本語](../ja/providers/anthropic.md)

Select the `anthropic` provider to translate with Claude through the Anthropic API, and pass an API key to the language server through its environment.

## Minimal settings

```json
{
  "lsp": {
    "translation-lsp": {
      "binary": {
        "path": "REPLACE_WITH_ABSOLUTE_SERVER_PATH",
        "env": { "TRANSLATION_LSP_ANTHROPIC_API_KEY": "sk-ant-…" }
      },
      "initialization_options": {
        "provider": { "name": "anthropic", "model": "claude-haiku-5-5", "effort": "low" }
      }
    }
  }
}
```

The other `initialization_options` described under [Configuration](../configuration.md) apply unchanged.

## Settings

| Setting | Default | Rules |
| --- | --- | --- |
| `provider.name` | None | `"anthropic"` selects this provider. |
| `provider.model` | `claude-haiku-5-5` | Optional. Empty and whitespace-only values are settings errors. The value is sent as written, without substituting another model. |
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

The server reads the key only from the `TRANSLATION_LSP_ANTHROPIC_API_KEY` environment variable of its own process, once at startup. Set it in `lsp.translation-lsp.binary.env` as shown above, then restart the language server.

The server deliberately ignores `ANTHROPIC_API_KEY`: Claude Code prefers that variable over a subscription login, so exporting it for your whole shell would move your ordinary Claude Code usage to API billing.

Without a key the server still starts, and a translation request returns a settings error that names the variable without contacting the API.

## Cost and usage

API usage is billed to the Claude Console organization that owns the key. It is separate from the usage included in a Claude subscription. With `provider.document`, tab translations are billed at the rates of the model named there.

Automatic comment Hover and Markdown selection translation send requests without a confirmation, as described under [Usage](../usage.md).

There is no account menu or usage command for this provider.

## What is sent

The text submitted for translation is described under [What is sent](../usage.md#what-is-sent).

Each request contains the translation instructions and the text to translate as a single user message. Conversation history is not sent. The request asks for at most 64,000 output tokens, or fewer when `translation_limits.output_kib` is smaller; a translation that reaches that limit is reported as an error instead of being shown incomplete.

## Errors and pauses

A billing error, a rejected key, an unknown model, and a permission error stop automatic translation and report the HTTP status, the API's error type, and the request ID. After fixing billing in the Claude Console or correcting `provider.model` or `provider.document.model`, run a selection or document translation explicitly; automatic translation resumes when that succeeds. Changing the key or any setting requires a language server restart.

Rate limits and overload are retried once as described under [Automatic retries](../behavior.md#automatic-retries). If the model declines a text, the request fails with the reported category, is not retried, and does not pause automatic translation.

The rules shared by every provider are described under [Pauses](../behavior.md#pauses) and [Automatic retries](../behavior.md#automatic-retries).

## Stored data

The key is written in `lsp.translation-lsp.binary.env`, as described under [Authentication](#authentication). This stores the key as plain text in your Zed settings file, so keep that file out of shared repositories and synchronized dotfiles. The server does not write the key to logs, error messages, or disk.
