# OpenAI-compatible API

[日本語](../ja/providers/openai-compatible.md)

Select the `openai_compatible` provider to translate with a service that accepts the Chat Completions requests of the OpenAI API, such as OpenRouter, the Gemini API, Ollama, or LM Studio. `base_url` names the service: requests go there, and an API key is sent with them only when you have saved one for that service from the `OpenAI-compatible API key…` code action.

## Settings for each service

Choose the `provider` of the service you use from the sections below, and write it in `initialization_options` under `lsp` → `translation-lsp` in the Zed settings. The `base_url` values are those in each service's documentation, and the models are examples. Model names are those of the service you send to. The other settings in `initialization_options` work as described under [Configuration](../configuration.md).

### Gemini API

```json
"provider": {
  "name": "openai_compatible",
  "base_url": "https://generativelanguage.googleapis.com/v1beta/openai/",
  "model": "gemini-3.8-flash"
}
```

Save a Gemini API key as described under [Authentication](#authentication).

### OpenRouter

```json
"provider": {
  "name": "openai_compatible",
  "base_url": "https://openrouter.ai/api/v1",
  "model": "google/gemini-3.8-flash"
}
```

Save an OpenRouter API key. OpenRouter model names start with the name of the company that offers the model, as in `google/gemini-3.8-flash`.

### Ollama on this computer

```json
"provider": {
  "name": "openai_compatible",
  "base_url": "http://localhost:11434/v1",
  "model": "MODEL"
}
```

Replace `MODEL` with the name of a model you pulled into Ollama, as `ollama list` shows it. No key is needed.

### LM Studio on this computer

```json
"provider": {
  "name": "openai_compatible",
  "base_url": "http://localhost:1234/v1",
  "model": "MODEL"
}
```

Replace `MODEL` with the identifier of a model loaded in LM Studio. No key is needed.

### Keys belong to one service

A key saved for one service is not sent to another: a Gemini API key is not sent to OpenRouter. After you change `base_url` to another service, save that service's key if it needs one. When a service refuses a request that went without a key, the error names the address that has no key saved. How keys are stored is described under [Authentication](#authentication).

## Settings

| Setting | Default | Rules |
| --- | --- | --- |
| `provider.name` | None | `"openai_compatible"` selects this provider. |
| `provider.base_url` | None | Required. The URL that requests go to with `/chat/completions` after it. It must start with `https://`, or with `http://` for a service on this computer: `localhost`, an address from `127.0.0.1` to `127.255.255.255`, or `::1`. A user name, a password, a query, or a fragment is a settings error, as is a value longer than 2,048 bytes. A `/` at the end is ignored. |
| `provider.model` | None | Required. Empty values, values that contain whitespace or control characters, and values longer than 256 bytes are settings errors. The value is sent as written, without substituting another model. |
| `provider.effort` | None | Optional. Sent as the request's `reasoning_effort` as written. When omitted, no `reasoning_effort` is sent. Empty values, values that contain whitespace or control characters, and values longer than 64 bytes are settings errors. |
| `provider.document` | None | Optional. An object holding the model and options for translations opened in a tab. Its values are not inherited from `provider.model` or `provider.effort`: a value omitted inside it is not sent. Tab translations go to the same `base_url`. Unknown fields inside it are settings errors. |
| `provider.document.model` | None | Required when `provider.document` is present. The rules of `provider.model` apply. |
| `provider.document.effort` | None | Optional. The rules of `provider.effort` apply. |

Each service takes `reasoning_effort` values of its own, so the server checks only their form. According to the Gemini API documentation, it takes values such as `minimal`, `low`, `medium`, and `high`, and turns each into the reasoning level of the model. According to the Ollama documentation, the values a model takes depend on the model.

Unknown fields are rejected: there is no setting for the API key. Restart the language server after changing these settings.

### Tab translations with another model

```json
"provider": {
  "name": "openai_compatible",
  "base_url": "https://openrouter.ai/api/v1",
  "model": "google/gemini-3.8-flash",
  "document": { "model": "anthropic/claude-sonnet-5.5" }
}
```

The values that `provider.document` accepts are in the table above. The models above are examples.

What `provider.document` changes for every provider is described under [Tab translations with another model](../configuration.md#tab-translations-with-another-model).

## Authentication

A service that needs no key, such as Ollama or LM Studio on this computer, is used without one. Requests carry no `Authorization` header until you save a key.

To save a key, run `editor: toggle code actions` from Zed's Command Palette in a supported document, choose `OpenAI-compatible API key…`, then run `Save OpenAI-compatible API key`. A terminal window of the OS opens, names the address the key will be sent to, and asks for the key. Paste the key there and press Enter; the terminal does not show what you paste. The next translation sends the key as `Authorization: Bearer`, without a language server restart.

While a key is saved, the menu has these items instead:

- `Replace OpenAI-compatible API key`: open the same terminal to save another key in place of the saved one.
- `Remove OpenAI-compatible API key`: after you confirm, delete the key file of this service from this computer, cancel the translations in progress, and discard the translations already shown. Later requests carry no key. The key stays valid until you delete it at the service.

The menu appears in Markdown and Plain Text documents, and in other documents only in a range whose translation failed, such as when the service refused the request without a key. To save, replace, or remove the key at another time, open a Markdown or Plain Text file.

The terminal opens and the key is saved as for the Anthropic API key, described under [Authentication](anthropic.md#authentication) of that page. The notice that follows `Save OpenAI-compatible API key` or `Replace OpenAI-compatible API key` shows the command to run by hand when no window appears: the path of the language server, then `api-key` and the address of the service, such as `api-key https://openrouter.ai`.

A key belongs to the scheme, host, and port of `base_url`, and is sent there only. Each has a file of its own in the server's credential directory, named `api-key-`, the scheme, the host, and the port, joined by `-`, such as `api-key-https-openrouter.ai-443` or `api-key-http-localhost-11434`. An IPv6 address is written in brackets with its colons as `_`, as in `api-key-http-[__1]-8080`. Changing `base_url` to another service uses that service's key, or none; the keys of other services stay in their files. The directory and the files are checked as the Anthropic key file is.

| OS | Credential directory |
| --- | --- |
| Windows | `%LOCALAPPDATA%\translation-lsp\` |
| macOS | `~/Library/Application Support/translation-lsp/` |
| Linux | `$XDG_CONFIG_HOME/translation-lsp/`, or `~/.config/translation-lsp/` when unset |

The [OpenAI API](openai-api.md) provider uses the file `api-key-https-api.openai.com-443`, so the two providers share the key for `https://api.openai.com`.

The key reaches the file from the terminal. It is not written in the Zed settings and does not pass through an environment variable or through the messages between Zed and the server.

A key file that cannot be used, because it is empty, holds something besides the key, or is not private, stops translations with a settings error until you correct or remove it.

## Cost and usage

The server does not count usage or cost; check them at the service. With `provider.document`, tab translations use the model named there.

Automatic comment Hover and Markdown selection translation send requests without a confirmation, as described under [Usage](../usage.md).

A model on this computer may take longer than the default deadlines. Raise `hover_timeout_ms`, `selection_timeout_ms`, or `document_timeout_ms` when translations time out; see [Timing](../configuration.md#timing).

## What is sent

The text submitted for translation is described under [What is sent](../usage.md#what-is-sent).

Each request is a streamed Chat Completions request with two messages: the translation instructions as the system message, and the text to translate, between `<source_text>` tags, as the user message. To look up a word, the system message holds the dictionary instructions instead, and the user message holds the term between `<term>` tags and its line between `<line>` tags. Conversation history is not sent. A translation read in Hover sends `max_tokens` of 16,384 plus 4 per byte of the source, or of the line of a term, or the number of bytes in `translation_limits.output_kib` (1,024 per KiB) when that is smaller, so that text in a comment cannot make the model write at length. A translation opened in a tab sends no `max_tokens`, since a limit above a model's own may be refused, and is bounded by `translation_limits.output_kib`.

The translation is the text of the reply's first choice. Reasoning that a service returns in a field of its own is not shown. Reasoning that a model writes into the text of the reply is shown as part of the translation.

## Errors and pauses

A completed reply ends with the `finish_reason` `stop`. A reply that ends at the token limit (`length`), is stopped by a content filter (`content_filter`), ends for another reason, or ends without a reason is reported as an error instead of being shown incomplete. An error that the service sends after the reply has begun is reported as a stream failure; when its code is a number, as OpenRouter gives it, the code is read as the HTTP status.

A refused key or a missing one (HTTP 401), missing credit (402), a permission error (403), and an unknown model or address (404) stop automatic translation and report the HTTP status, the error code, and the request ID when the service gives them. For HTTP 401, the error also names the address the request went to, and says whether a key was saved for it. After correcting the cause, run a selection or document translation explicitly; automatic translation resumes when that succeeds. A stop for a refused key ends when you save another key from `OpenAI-compatible API key…`.

Saving, replacing, or removing the key needs no language server restart. Translations and failures kept with one key, or without a key, are not shown with another, and a translation that is in progress when the key changes is discarded when it completes. Changing a setting requires a language server restart.

A timeout (408), a rate limit (429), and server errors, also those reported in the stream or with the code `rate_limit_exceeded`, `server_error`, or `server_is_overloaded`, are retried once as described under [Automatic retries](../behavior.md#automatic-retries). A content filter stop is not retried and does not pause automatic translation.

The rules shared by every provider are described under [Pauses](../behavior.md#pauses) and [Automatic retries](../behavior.md#automatic-retries).

## Stored data

Keys are stored as plain text in the files described under [Authentication](#authentication), in the directory where the ChatGPT provider stores its credentials. The `api-key` command, which the menu runs in a terminal, writes the file, and creates the directory when it is missing. `Remove OpenAI-compatible API key` deletes the file of the current service and nothing else. Otherwise the language server only reads the files.

To open Terminal on macOS, the server writes a script to the temporary directory. The script holds the path of the language server and the address of the service, and no key, and deletes itself when Terminal runs it.

The server does not write the key to logs, error messages, notices, or anywhere else on disk.
