# Translation LSP

[日本語](README.ja.md)

Translate comments in Rust, JavaScript/JSX, TypeScript/TSX, C#, C, C++, Python, Go, Ruby, PHP, HTML, CSS, JSON, JSONC, YAML, TOML, shell scripts, PowerShell, Dockerfile, and Docker Compose, including Python docstrings, in Zed's Hover. Select Markdown prose to prepare a translation in the background. Hover over the original text to read the translation. The source stays in the editor unchanged.

The ChatGPT provider uses your ChatGPT account with permission to use your plan. The Anthropic API and OpenAI API providers call those APIs with your API key. The OpenAI-compatible provider calls a service you name, such as OpenRouter, the Gemini API, Ollama, or LM Studio, with an API key only when you save one. Until you select a provider, the extension sends no text to an external service.

## Installation

Install Translation LSP as a development extension: Zed builds the extension from a checkout, and the extension downloads the language server.

1. Install Zed and [Rust through rustup](https://www.rust-lang.org/tools/install). Zed builds development extensions with Rust.
2. Clone this repository and check out the tag of a [published release](https://github.com/hwtnb/zed-translation-lsp/releases).
3. In Zed's Extensions view, choose `Install Dev Extension` and select **`crates/zed-extension`** in the checkout. See [Zed's development extension instructions](https://zed.dev/docs/extensions/developing-extensions).

When the language server first starts, the extension downloads the server of its own version from that release and keeps it for later starts. A checkout whose version has no release has no server to download. Servers are published for these systems:

| OS | Versions | CPU |
| --- | --- | --- |
| macOS | macOS 12 or later | Apple silicon and Intel |
| Linux | Ubuntu 22.04 or later, Debian 12 or later, and other distributions with glibc 2.35 or later | ARM64 and x86-64 |
| Windows | Windows 10 or later on x86-64, Windows 11 on ARM64 | ARM64 and x86-64 |

The servers are not signed with Authenticode and not notarized by Apple. If your system does not start the downloaded server, or no server is published for it, build the server from source.

### Build the server from source

Install [Rust through rustup](https://www.rust-lang.org/tools/install) (Rust 1.97 or later) and a C/C++ toolchain, which the native dependencies need:

| OS | Build prerequisites | Native executable |
| --- | --- | --- |
| Windows | MSVC Rust toolchain, Visual Studio Build Tools with Desktop development with C++ and a Windows SDK; NASM on x86-64, clang-cl on ARM64. Use a Visual Studio developer PowerShell or command prompt. | `target/release/translation-lsp.exe` |
| macOS | Xcode Command Line Tools (`xcode-select --install`). | `target/release/translation-lsp` |
| Linux | GCC or Clang with C/C++ development tools; on Debian/Ubuntu, install `build-essential` and `ca-certificates`. | `target/release/translation-lsp` |

Tree-sitter and the TLS provider compile native code. See the TLS provider's [Windows](https://aws.github.io/aws-lc-rs/requirements/windows.html) and [Linux](https://aws.github.io/aws-lc-rs/requirements/linux.html) build requirements for other toolchain configurations.

Run the following command from the checkout root. It works in PowerShell, Command Prompt, Bash, and Zsh:

```sh
cargo build -p translation-lsp --release --locked
```

Set `lsp.translation-lsp.binary.path` to the absolute path of the executable you built, as described under [Server path](docs/configuration.md#server-path). The extension then starts that executable and downloads nothing.

If you list language servers explicitly for C#, C, C++, Python, Go, Ruby, PHP, HTML, CSS, JSON, JSONC, YAML, TOML, Shell Script, PowerShell, Dockerfile, or Docker Compose, see [Language server settings](docs/languages.md#language-server-settings).

## Set up a provider

Without a provider, the language server translates nothing. When it starts, it asks you to choose one, with buttons that open Zed's settings file and this section. Later, `Set up Translation LSP…` in the code actions of a supported document offers the same buttons.

Select one provider by adding one of the following entries to `lsp` in your existing Zed settings, then connect it from Zed's code actions. Restart the language server after changing the settings. `target_language` is the language translations are written in; see [Other common settings](#other-common-settings). Every setting is described under [Configuration](docs/configuration.md).

### ChatGPT

```json
{
  "lsp": {
    "translation-lsp": {
      "initialization_options": {
        "target_language": "ja",
        "provider": {
          "name": "chatgpt",
          "model": "gpt-6-luna",
          "effort": "low"
        }
      }
    }
  }
}
```

In a supported document, run `editor: toggle code actions` from Zed's Command Palette, choose `ChatGPT account…`, then run `Continue with ChatGPT / Choose account`, and complete sign-in and plan permission in the browser. See [ChatGPT](docs/providers/chatgpt.md) for the other settings and the account menu.

### Anthropic API

```json
{
  "lsp": {
    "translation-lsp": {
      "initialization_options": {
        "target_language": "ja",
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

In a supported document, run `editor: toggle code actions` from Zed's Command Palette, choose `Anthropic API key…`, then run `Save Anthropic API key`. Paste the key in the terminal window that opens and press Enter. See [Anthropic API](docs/providers/anthropic.md) for creating the key, the other settings, and cost.

### OpenAI API

```json
{
  "lsp": {
    "translation-lsp": {
      "initialization_options": {
        "target_language": "ja",
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

In a supported document, run `editor: toggle code actions` from Zed's Command Palette, choose `OpenAI API key…`, then run `Save OpenAI API key`. Paste the key in the terminal window that opens and press Enter. See [OpenAI API](docs/providers/openai-api.md) for the other settings and cost.

### OpenAI-compatible API

```json
{
  "lsp": {
    "translation-lsp": {
      "initialization_options": {
        "target_language": "ja",
        "provider": {
          "name": "openai_compatible",
          "base_url": "http://localhost:11434/v1",
          "model": "gemma3"
        }
      }
    }
  }
}
```

Set `base_url` to the address of the service and `model` to one of its model names; the example above uses a model pulled into Ollama on this computer. A service on this computer that needs no key, such as Ollama, is ready without one. For a service that needs a key, choose `OpenAI-compatible API key…` from the code actions, then run `Save OpenAI-compatible API key`. See [OpenAI-compatible API](docs/providers/openai-compatible.md) for the addresses of some services and the other settings.

### Other common settings

These settings sit next to `provider` in `initialization_options`. Each can be omitted.

| Setting | Default | What it sets |
| --- | --- | --- |
| `target_language` | `ja` | The language translations are written in: a language code such as `en`, `ja`, or `zh-Hans`, or a language name. |
| `source_language` | `auto` | The language of the original text. With `auto`, the model identifies it. |
| `features.comment_hover` | `true` | Whether Hover over a comment starts a translation. |
| `features.automatic_selection` | `true` | Whether a stable Markdown selection starts a translation in the background. |

`features` can also turn each way of translating on or off for one language. See [Translation languages](docs/configuration.md#translation-languages) and [Features](docs/configuration.md#features).

## Basic usage

| Operation | How |
| --- | --- |
| Comment Hover | Hover over a supported code comment, or place the cursor in it and run Zed's `editor: hover`. |
| Markdown selection | With a provider ready, as described under [Set up a provider](#set-up-a-provider), and `features.automatic_selection` set to `true`, select Markdown prose and keep the selection stable. Translation starts in the background; Hover over that range to read it. |
| `Translate selection to …` | To translate immediately or recover from a background failure, use the `Translate selection to …` Code Action. Manual execution shows `Translation ready. Hover over the selected text to view it.` when complete. |
| Word lookup | Select up to three words or one identifier on one line, such as `retain` or `retain_completed`. Hover shows a dictionary entry instead of a translation. |
| `… and open in tab` | In Markdown or Plain Text, use `Translate selection to … and open in tab` to read the translation in a separate document. With no selection, the action becomes `Translate document to … and open in tab` and translates the whole document. |
| Cancel | To stop one accepted translation, place the cursor or a selection in its range and choose `Cancel translation of lines …` from Code Actions. `Cancel all translations` stops all translation work without canceling an account menu. |

The rules of these operations are described under [Usage](docs/usage.md).

## Before you send text

- Merely passing over a comment can consume provider usage once Zed sends a Hover request.
- If a comment contains a credential, the credential is sent with it.
- With `features.automatic_selection` set to `true`, selecting Markdown prose just to copy it can also send it for translation.
- Looking up a word also sends the line it is on.

What each request contains is described under [What is sent](docs/usage.md#what-is-sent). Cost is described on each provider's page.

## Documentation

- [Configuration](docs/configuration.md): the full settings example, the server path, languages, features, timing, size limits, work and cache limits, and changing the model.
- [ChatGPT](docs/providers/chatgpt.md): the settings of the ChatGPT provider and connecting your account.
- [Anthropic API](docs/providers/anthropic.md): the settings of the Anthropic provider and its API key.
- [OpenAI API](docs/providers/openai-api.md): the settings of the OpenAI API provider and its API key.
- [OpenAI-compatible API](docs/providers/openai-compatible.md): the settings of the OpenAI-compatible provider, the addresses of some services, and their API keys.
- [Usage](docs/usage.md): Hover, selections, looking up words, translations opened in a tab, status, canceling and retrying, and what is sent.
- [Behavior](docs/behavior.md): admission, what keeps work running, automatic selection, the cache, deadlines, retries, and pauses.
- [Languages](docs/languages.md): supported languages, comment extraction, and language server settings.
- [Development](docs/development.md): checks, the mock provider and its samples, CI, and the dependency policy.

## License

[MIT](LICENSE). Each release archive of the server contains the licenses of the server's dependencies and where to download their sources.
