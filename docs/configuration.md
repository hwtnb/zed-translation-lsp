# Configuration

[日本語](ja/configuration.md)

This page describes the settings under `lsp.translation-lsp`. The fields of each provider are listed on the [ChatGPT](providers/chatgpt.md) and [Anthropic API](providers/anthropic.md) pages.

## Full example

The extension ID, language server settings key, and native executable name are `translation-lsp`.

Add the following entry to `lsp` in your existing Zed settings. Choose a model available to the account you will connect; the model and reasoning effort below are examples.

```json
{
  "lsp": {
    "translation-lsp": {
      "initialization_options": {
        "target_language": "ja",
        "source_language": "auto",
        "hover_timeout_ms": 20000,
        "selection_timeout_ms": 30000,
        "document_timeout_ms": 180000,
        "selection_debounce_ms": 450,
        "comment_parse_timeout_ms": 1000,
        "features": {
          "comment_hover": true,
          "automatic_selection": true,
          "selection_action": true,
          "document_action": true
        },
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
          "effort": "low"
        }
      }
    }
  }
}
```

## Server path

### Lookup order

The extension starts the first server it finds, in this order:

1. The executable that `binary.path` names.
2. `translation-lsp` on the worktree's PATH.
3. The server the extension downloaded.

`binary.arguments` and `binary.env` apply to the server from any of the three.

### Automatic download and cache

When the first two give no server, the extension downloads one from this repository's [GitHub Releases](https://github.com/hwtnb/zed-translation-lsp/releases) and keeps it in its work directory. It downloads the server of its own version, which is not always the latest release. Later starts use the kept server and need no network access. After the extension is updated, it downloads the server of its new version and removes the servers of other versions.

A download that does not complete is discarded, and the next start tries again. The extension never starts a partly downloaded server. The download goes through Zed's HTTPS downloader; the extension does not compare the archive with the release's `SHA256SUMS`.

A server on PATH comes before the downloaded one, so updating the extension does not replace it.

### Manual installation

Set `binary.path` when the extension cannot reach GitHub, when no server is published for your system, or to use a server you built. Either build the server as described under [Build the server from source](../README.md#build-the-server-from-source), or download the archive for your system from the release of the extension's version and extract it. To check a downloaded archive, compare its SHA-256 with its line in the release's `SHA256SUMS`: use `sha256sum` on Linux, `shasum -a 256` on macOS, or `Get-FileHash -Algorithm SHA256` in PowerShell. Keep the license files that the archive contains next to the executable.

`binary.path` is an absolute path for the OS where the server runs, including `.exe` on Windows. For example:

| OS | Example `binary.path` value |
| --- | --- |
| Windows | `C:/Users/you/src/translation-lsp/target/release/translation-lsp.exe` |
| macOS | `/Users/you/src/translation-lsp/target/release/translation-lsp` |
| Linux | `/home/you/src/translation-lsp/target/release/translation-lsp` |

Windows paths can use forward slashes in JSON. If you use backslashes, escape each one as `\\`. Do not include extra shell quotes inside the path value, even when the path contains spaces.

### Language server lists

If you explicitly configure a language's `language_servers`, add `"translation-lsp"` to the existing array. The settings of each language are listed under [Language server settings](languages.md#language-server-settings).

To turn this extension off for one language, put `"!translation-lsp"` in that language's `language_servers`, followed by `"..."` to keep the other servers:

```json
{
  "languages": {
    "YAML": {
      "language_servers": ["!translation-lsp", "..."]
    }
  }
}
```

In that language, neither Hover translation nor the Code Actions appear. To turn off only some features in a language, use [`features`](#features).

## Translation languages

UI labels are in English regardless of `target_language`. `target_language` defaults to `ja`; `source_language` defaults to `auto`. With `auto`, the same translation request identifies the source language.

A value that is an ISO 639 language code, alone or with subtags such as `zh-Hans` or `pt-BR`, is shown with the English name of its language, as `Japanese (ja)`, both in Code Action titles and in the instructions sent to the provider. Any other value, such as `Japanese`, is used as written.

## Features

`features` turns each way of starting a translation on or off, for every language or for one language. Every value is a boolean.

| Setting inside `features` | What it turns on or off | Language IDs |
| --- | --- | --- |
| `comment_hover` | Starting a translation by Hover over a comment. | `rust`, `javascript`, `typescript`, `typescriptreact`, `csharp`, `c`, `cpp`, `python`, `go`, `ruby`, `php`, `html`, `css`, `json`, `jsonc`, `yaml`, `toml`, `shellscript`, `powershell`, `dockerfile`, `dockercompose` |
| `automatic_selection` | Starting a translation after a stable Markdown selection. | `markdown` |
| `selection_action` | The `Translate selection to …` Code Action. | Every language ID in the table below |
| `document_action` | The `Translate selection to … and open in tab` and `Translate document to … and open in tab` Code Actions. | `markdown`, `plaintext` |

`features`, each setting inside it, `languages`, each language inside `languages`, and each setting of a language can be omitted. For a feature in a language, the value under `languages.<language ID>` applies. Without it, the value written directly inside `features` applies. Without that either, the feature is on.

The following example turns tab translations off in every language, and comment Hover off in YAML and TOML:

```json
"features": {
  "comment_hover": true,
  "automatic_selection": true,
  "selection_action": true,
  "document_action": false,
  "languages": {
    "yaml": { "comment_hover": false },
    "toml": { "comment_hover": false }
  }
}
```

Where `comment_hover` is off, Hover over a comment starts no translation. A translation started by `Translate selection to …` still appears on Hover. Where `selection_action` or `document_action` is off, its Code Actions are not listed. The cancel, retry, and account menu Code Actions are not affected.

The keys of `languages` are the following language IDs, in lowercase:

| Zed language | Language ID |
| --- | --- |
| Rust | `rust` |
| JavaScript, including `.jsx` files | `javascript` |
| TypeScript | `typescript` |
| TSX (`.tsx` files) | `typescriptreact` |
| CSharp | `csharp` |
| C | `c` |
| C++ | `cpp` |
| Python | `python` |
| Go | `go` |
| Ruby | `ruby` |
| PHP | `php` |
| HTML | `html` |
| CSS | `css` |
| JSON | `json` |
| JSONC | `jsonc` |
| YAML | `yaml` |
| TOML | `toml` |
| Shell Script | `shellscript` |
| PowerShell | `powershell` |
| Dockerfile | `dockerfile` |
| Docker Compose | `dockercompose` |
| Markdown | `markdown` |
| Plain Text | `plaintext` |

Any other key of `languages`, such as `javascriptreact`, is a settings error. So is a feature written for a language outside its language IDs, such as `languages.rust.document_action`.

## Timing

Translation timing options are optional:

| Setting | Default | Allowed values | Effect |
| --- | --- | --- | --- |
| `hover_timeout_ms` | `20000` | Integer `1000`–`120000`, inclusive | Response deadline from receipt, including queue time, for each Hover request. |
| `selection_timeout_ms` | `30000` | Integer `1000`–`120000`, inclusive | Execution budget from dispatch for retained manual selections and Markdown background translations. |
| `document_timeout_ms` | `180000` | Integer `1000`–`600000`, inclusive | Execution budget from dispatch for full-document or selection translations opened in a tab; queue time is additional. |
| `selection_debounce_ms` | `450` | Integer `100`–`2000`, inclusive | Native server's wait after observing a new selection. Zed's own observation delay is additional. |
| `comment_parse_timeout_ms` | `1000` | Integer `100`–`60000`, inclusive | Limit on reading the comments of a C, C++, C#, or Python document, from the start of the reading. See [Documents whose comments cannot be read](languages.md#documents-whose-comments-cannot-be-read). |

## Size limits

Translation size limits belong to `initialization_options.translation_limits`. Values are positive integers in KiB (1 KiB = 1024 UTF-8 bytes), not character or token counts. Omitted objects and fields use their own defaults.

| Setting inside `translation_limits` | Default | Applies to |
| --- | --- | --- |
| `input_kib.hover` | `16` | New comment Hover requests, Markdown background translation, and manual selection translations shown in Hover. |
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

These ceilings are a local resource policy, not measured hardware limits or provider guarantees. Source documents, parser state, allocator overhead, transport buffers, and temporary translation files are additional costs, so these figures are **not a process-memory guarantee**. A limit beyond these ceilings needs a change in the server; Zed settings cannot raise them. The reasons for the ceilings are described under [Size limit ceilings](development.md#size-limit-ceilings).

Increasing a local limit does not increase the model's context or output capacity or extend the request deadline. Long documents are sent in one request, without automatic splitting. Failed, incomplete, and oversized results are not displayed or saved as completed translations.

## Settings errors and restart

Unknown fields, wrong value types, and out-of-range values prevent the language server from starting and return an English settings error. Omitting a field uses its default. Without `provider`, the language server translates nothing and asks you to choose a provider, as described under [Set up a provider](../README.md#set-up-a-provider).

Restart the language server after changing these settings. Settings changes do not apply to a running server. See [Zed's language server settings](https://zed.dev/docs/configuring-languages#configuring-language-servers).

## Changing the translation model

1. Open Zed's Command Palette and run `zed: open settings file`.
2. Edit `lsp.translation-lsp.initialization_options.provider.model` with the model slug you want to use.
3. Set the provider's other fields if needed. They are listed on the [ChatGPT](providers/chatgpt.md#settings) and [Anthropic API](providers/anthropic.md#settings) pages.
4. Save the settings. In a document using this extension, run `editor: restart language server` from the Command Palette.

The extension sends the configured model slug to the translation API without substituting another model. The API determines whether that model, effort, and speed are available to your account.

Translation requests use ordinary text output. Choose a model available to your ChatGPT account or Anthropic API key; the extension does not switch to another model when a request fails.

### Tab translations with another model

To use a different model only for translations opened in a tab, add `provider.document`. `Translate selection to … and open in tab` and `Translate document to … and open in tab` then send the model and options written there. Comment Hover, automatic selection, manual selection translation, and retries keep using the surrounding `provider` values.

`provider.document.model` is required. The other values inside `provider.document` are optional and are not inherited from the surrounding settings: an omitted value takes its own default, whatever is written outside.

`provider.document` cannot select another provider. Setting it also keeps tab translations separate from range translations, as described under [Admission and execution slots](behavior.md#admission-and-execution-slots).

The values that `provider.document` accepts, and an example, are on each provider's page: [ChatGPT](providers/chatgpt.md#tab-translations-with-another-model) and [Anthropic API](providers/anthropic.md#tab-translations-with-another-model).
