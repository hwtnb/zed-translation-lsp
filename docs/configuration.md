# Configuration

[日本語](ja/configuration.md)

This page describes the settings under `lsp.translation-lsp`. The fields of each provider are listed on the [ChatGPT](providers/chatgpt.md) and [Anthropic API](providers/anthropic.md) pages.

## Full example

The extension ID, language server settings key, and native executable name are `translation-lsp`.

Add the following entry to `lsp` in your existing Zed settings. Replace `binary.path` with the absolute path to the executable you built. Choose a model available to the account you will connect; the model and reasoning effort below are examples.

```json
{
  "lsp": {
    "translation-lsp": {
      "binary": {
        "path": "REPLACE_WITH_ABSOLUTE_SERVER_PATH",
        "arguments": []
      },
      "initialization_options": {
        "target_language": "ja",
        "source_language": "auto",
        "hover_timeout_ms": 20000,
        "selection_timeout_ms": 30000,
        "document_timeout_ms": 180000,
        "markdown_auto_translate": true,
        "selection_debounce_ms": 450,
        "translation_limits": {
          "input_kib": {
            "hover": 16,
            "document": 1024
          },
          "output_kib": 4096
        },
        "provider": {
          "name": "chatgpt",
          "model": "gpt-6.1-sol",
          "reasoning_effort": "low"
        }
      }
    }
  }
}
```

## Server path

Use a path for the OS where the language server runs, including `.exe` on Windows. For example:

| OS | Example `binary.path` value |
| --- | --- |
| Windows | `C:/Users/you/src/translation-lsp/target/release/translation-lsp.exe` |
| macOS | `/Users/you/src/translation-lsp/target/release/translation-lsp` |
| Linux | `/home/you/src/translation-lsp/target/release/translation-lsp` |

Windows paths can use forward slashes in JSON. If you use backslashes, escape each one as `\\`. Do not include extra shell quotes inside the path value, even when the path contains spaces.

If `binary.path` is omitted, the extension looks for `translation-lsp` on the worktree's PATH.

### Language server lists

If you explicitly configure a language's `language_servers`, add `"translation-lsp"` to the existing array. The settings of each language are listed under [Language server settings](languages.md#language-server-settings).

## Translation languages

UI labels are in English regardless of `target_language`. `target_language` defaults to `ja`; `source_language` defaults to `auto`. With `auto`, the same translation request identifies the source language.

## Timing

Translation timing options are optional:

| Setting | Default | Allowed values | Effect |
| --- | --- | --- | --- |
| `hover_timeout_ms` | `20000` | Integer `1000`–`120000`, inclusive | Response deadline from receipt, including queue time, for each Hover request. |
| `selection_timeout_ms` | `30000` | Integer `1000`–`120000`, inclusive | Execution budget from dispatch for retained manual selections and Markdown background translations. |
| `document_timeout_ms` | `180000` | Integer `1000`–`600000`, inclusive | Execution budget from dispatch for full-document or selection translations opened in a tab; queue time is additional. |
| `markdown_auto_translate` | `true` | Boolean | Start Markdown prose translation after a stable selection. Set `false` to keep manual selection translation and comment Hover. |
| `selection_debounce_ms` | `450` | Integer `100`–`2000`, inclusive | Native server's wait after observing a new selection. Zed's own observation delay is additional. |

## Size limits

Translation size limits belong to `initialization_options.translation_limits`. Values are positive integers in KiB (1 KiB = 1024 UTF-8 bytes), not character or token counts. Omitted objects and fields use their own defaults.

| Setting inside `translation_limits` | Default | Applies to |
| --- | --- | --- |
| `input_kib.hover` | `16` | New comment Hover requests, Markdown background translation, and manual selection Hover previews. |
| `input_kib.document` | `1024` | Whole documents and selections translated into a separate tab. |
| `output_kib` | `4096` | Completed translations, regardless of display destination. |

For example, `"translation_limits": {"input_kib": {"document": 2048}}` changes only the document input limit to 2 MiB. Zero, negative, fractional, null, unknown, and nonnumeric values are rejected, as are sizes that cannot be represented on the server platform. Restart the language server after editing these settings.

Explicit requests over the input limit report the size, limit, and setting name; oversized automatic selections are skipped. Existing cached translations can still be read without another translation request.

## Work and cache limits

Work and cache limits are optional objects inside `translation_limits`:

```json
"translation_limits": {
  "work": { "admitted": 8, "concurrent": 2, "documents": 1, "automatic": 3, "hover_waiters": 16 },
  "cache": { "entries": 64, "capacity_kib": 16384 }
}
```

`admitted` counts running and queued translation operations, including canceled workers until they terminate. `concurrent` limits all executing translations, `documents` limits operations dispatched as tab translations, `automatic` limits jobs retained by comment Hover or automatic selection without a manual or tab request, and `hover_waiters` limits individual pending Hover responses. All counts must be positive integers. Require `documents <= concurrent <= admitted` and `automatic < admitted`. This reserves admission capacity for explicit requests, but does not reserve an idle worker. Increasing concurrency can increase simultaneous provider usage.

`cache.entries` bounds the combined successful, failed, and tab cache entries across all documents. `cache.capacity_kib` bounds their stored text, not total process memory. The effective capacity is the greater of this value and `output_kib`, so one maximum-size result can fit. Metadata, source documents, in-flight input, and temporary tab files are outside this text budget. Defaults are a starting point for sequential reading, not measured provider or hardware limits. Omitted nested fields retain their defaults; restart the language server after changing them.

## Maximum values

The following are **maximum configurable values**, separate from the defaults above. Larger values fail initialization rather than being silently clamped.

| Setting inside `translation_limits` | Maximum |
| --- | --- |
| `input_kib.hover` | `1024` (1 MiB) |
| `input_kib.document`, `output_kib` | `16384` (16 MiB each) |
| `work.admitted` | `64` |
| `work.concurrent`, `work.documents` | `8` each |
| `work.automatic` | `63`, also less than `admitted` |
| `work.hover_waiters` | `256` |
| `cache.entries` | `4096` |
| `cache.capacity_kib` | `262144` (256 MiB) |

Two combined limits also apply: `max(input_kib.hover, input_kib.document) × work.admitted` must not exceed `262144` KiB (256 MiB), and `output_kib × work.concurrent` must not exceed `65536` KiB (64 MiB). For example, 16 MiB document input with 64 accepted jobs is rejected; use at most 16 accepted jobs at that input limit. A 16 MiB output limit permits at most four simultaneous translations. These checks apply to configured worst cases, even when the queue is currently empty.

These ceilings are a local resource policy, not measured hardware limits or provider guarantees. The input budget bounds accepted source payloads; the smaller output budget leaves room for escaped SSE events and multiple decoded output snapshots. The individual input caps allow larger documents while keeping automatic/selection Hover input more restricted. Job and cache-entry caps also bound scheduler and cache scans when texts are tiny. Cache text has a separate 256 MiB ceiling. Source documents, parser state, allocator overhead, transport buffers, and preview files are additional costs, so these figures are **not a process-memory guarantee**. Raising a limit beyond these ceilings requires reviewing this policy in the server, rather than overriding it in Zed.

Receive limits account for JSON escaping, repeated output snapshots, and SSE event overhead and scale with `output_kib`. They bound transport processing separately from the decoded translation; they do not allocate the maximum up front.

Increasing a local limit does not increase the model's context or output capacity or extend the request deadline. Long documents are sent in one request, without automatic splitting. Failed, incomplete, and oversized results are not displayed or saved as completed translations.

## Settings errors and restart

Unknown fields, wrong value types, and out-of-range values prevent the language server from starting and return an English settings error. Omitting a field uses its default.

Restart the language server after changing these settings. Settings changes do not apply to a running server. See [Zed's language server settings](https://zed.dev/docs/configuring-languages#configuring-language-servers).

## Changing the translation model

1. Open Zed's Command Palette and run `zed: open settings file`.
2. Edit `lsp.translation-lsp.initialization_options.provider.model` with the model slug you want to use.
3. Set the provider's other fields if needed. They are listed on the [ChatGPT](providers/chatgpt.md#settings) and [Anthropic API](providers/anthropic.md#settings) pages.
4. Save the settings. In a document using this extension, run `editor: restart language server` from the Command Palette.

The extension sends the configured model slug to the translation API without substituting another model. The API determines whether that model and reasoning effort are available to your account.

Translation requests use ordinary text output. Choose a model available to your ChatGPT account or Anthropic API key; the extension does not switch to another model when a request fails.

### Tab translations with another model

To use a different model only for translations opened in a tab, add `provider.document`. `Translate selection to … and open in tab` and `Translate document to … and open in tab` then send the model and options written there. Comment Hover, automatic selection, manual selection translation, and retries keep using the surrounding `provider` values.

`provider.document.model` is required. The other values inside `provider.document` are optional and are not inherited from the surrounding settings: an omitted value takes its own default, whatever is written outside.

`provider.document` cannot select another provider. Setting it also keeps tab translations separate from range translations, as described under [Admission and execution slots](behavior.md#admission-and-execution-slots).

The values that `provider.document` accepts, and an example, are on each provider's page: [ChatGPT](providers/chatgpt.md#tab-translations-with-another-model) and [Anthropic API](providers/anthropic.md#tab-translations-with-another-model).
