# ChatGPT

[日本語](../ja/providers/chatgpt.md)

The ChatGPT provider uses your ChatGPT account with permission to use your plan.

## Minimal settings

```json
{
  "lsp": {
    "translation-lsp": {
      "binary": {
        "path": "REPLACE_WITH_ABSOLUTE_SERVER_PATH"
      },
      "initialization_options": {
        "provider": { "name": "chatgpt", "model": "gpt-6.1-sol", "reasoning_effort": "low" }
      }
    }
  }
}
```

The other `initialization_options` described under [Configuration](../configuration.md) apply unchanged.

## Settings

| Setting | Default | Rules |
| --- | --- | --- |
| `provider.name` | None | `"chatgpt"` selects this provider. |
| `provider.model` | None | Required. Empty values, values that contain whitespace or control characters, and values longer than 256 bytes are settings errors. The value is sent as written, without substituting another model. |
| `provider.reasoning_effort` | None | Optional. One of `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, or `max`. When omitted, the API default is used. An explicit `none` differs from omitting the setting. |
| `provider.document` | None | Optional. An object holding the model and options for translations opened in a tab. Its values are not inherited from `provider.model` or `provider.reasoning_effort`: a value omitted inside it takes the default in the rows below. Unknown fields inside it are settings errors. |
| `provider.document.model` | None | Required when `provider.document` is present. The rules of `provider.model` apply. |
| `provider.document.reasoning_effort` | None | Optional. The same values as `provider.reasoning_effort`. |

Unsupported models or effort values return an error. Unknown fields are rejected. Restart the language server after changing these settings.

### Tab translations with another model

```json
"provider": {
  "name": "chatgpt",
  "model": "gpt-6.1-sol",
  "reasoning_effort": "low",
  "document": { "model": "gpt-6.1-sol", "reasoning_effort": "medium" }
}
```

A `reasoning_effort` written outside `provider.document` does not apply to tab translations. When `provider.document` omits it, tab requests carry no reasoning effort and the model uses its own default. To fix the reasoning effort of tab translations, write it inside `provider.document` as well. The model and reasoning effort above are examples.

What `provider.document` changes for every provider is described under [Tab translations with another model](../configuration.md#tab-translations-with-another-model).

## Authentication

In a supported document, run `editor: toggle code actions` from Zed's Command Palette, choose `ChatGPT account…`, then run `Continue with ChatGPT / Choose account`. Add a registration or choose a saved one, and complete sign-in and plan permission in the browser. Loading settings, opening the account menu, and Hover do not start an OAuth sign-in flow.

When the account is ready, the connection item becomes `Change ChatGPT account` and the plan permission item is hidden. Sign-out appears only while signed in. While a management operation is running, duplicate management candidates are hidden. If the connection state cannot be read, use `Check ChatGPT connection…` and follow its message.

The account menu includes:

- `Enable ChatGPT plan use`: grant plan permission when signed in without it.
- `ChatGPT Usage`: open [ChatGPT's usage settings](https://chatgpt.com/settings/usage), where your limits and credits apply.
- `Sign out of ChatGPT`: attempt to end the current registration's session and clear its local credentials. Other registrations remain. If the remote session cannot be confirmed as ended, follow the message to disconnect the app in ChatGPT settings.

If token refresh returns an unusable grant such as `invalid_grant`, choose the saved registration under `ChatGPT account…` → `Continue with ChatGPT / Choose account` to reconnect.

Authentication follows [Sign in with ChatGPT](https://developers.openai.com/siwc/token-sharing-open-source/sign-in).

## Cost and usage

The ChatGPT provider does not accept an API key, and the extension never switches to another billing route automatically.

Check usage in ChatGPT settings. The `ChatGPT Usage` item under [Authentication](#authentication) opens them.

## What is sent

The text submitted for translation is described under [What is sent](../usage.md#what-is-sent).

## Errors and pauses

Failures retain credentials unless token refresh confirms an unusable grant.

The rules shared by every provider are described under [Pauses](../behavior.md#pauses) and [Automatic retries](../behavior.md#automatic-retries).

## Stored data

Credentials are stored locally in this extension's OS-specific configuration directory:

| OS | Credential directory |
| --- | --- |
| Windows | `%LOCALAPPDATA%\translation-lsp\` |
| macOS | `~/Library/Application Support/translation-lsp/` |
| Linux | `$XDG_CONFIG_HOME/translation-lsp/`, or `~/.config/translation-lsp/` when unset |

The server resolves this location through the OS using `dirs::config_local_dir()`. Registrations use separate UUIDs even when they share an email address; reconnect through the original registration.

Authentication data uses `auth.json` version 2. On first access, the server migrates version 1 under the credential-store lock without creating a credential backup. Migration preserves tokens, registrations, and the selected account. Each registration receives an authorization ID: token refresh preserves it, while reconnecting replaces it. Refresh and temporary store locks preserve cached translations; the server checks authorization again before displaying them.

Older binaries cannot read version 2. To downgrade, convert the current credentials to version 1 or sign in again.
