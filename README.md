# Translation LSP

[日本語](README.ja.md)

Translate comments in Rust, JavaScript/JSX, TypeScript/TSX, C#, C, C++, Python, JSON, JSONC, YAML, TOML, shell scripts, PowerShell, Dockerfile, and Docker Compose, including Python docstrings, in Zed's Hover. Select Markdown prose to prepare a translation in the background. Hover over the original text to read the translation. The source stays in the editor unchanged.

The ChatGPT provider uses your ChatGPT account with permission to use your plan. The Anthropic provider calls the Anthropic API with your API key. Without provider settings, the extension runs in mock mode: it translates only the bundled samples into Japanese and sends no text to an external service.

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

If you list language servers explicitly for C#, C, C++, Python, JSON, JSONC, YAML, TOML, Shell Script, PowerShell, Dockerfile, or Docker Compose, see [Language server settings](docs/languages.md#language-server-settings).

## Minimal settings

Add the following entry to `lsp` in your existing Zed settings. If you built the server yourself, also set `binary.path` as described under [Server path](docs/configuration.md#server-path).

```json
{
  "lsp": {
    "translation-lsp": {
      "initialization_options": {
        "provider": { "name": "mock" }
      }
    }
  }
}
```

This example selects mock mode. To translate your own text, replace `provider` with the settings on the [ChatGPT](docs/providers/chatgpt.md) or [Anthropic API](docs/providers/anthropic.md) page. Restart the language server after changing these settings. Every setting is described under [Configuration](docs/configuration.md).

## Mock mode

To try the bundled samples without connecting, omit `provider` or use `{"name":"mock"}`. Hover over the comment in [samples/comment.rs](samples/comment.rs), or the ordinary and XML documentation comments in [samples/comment.cs](samples/comment.cs). C/C++ samples are available in [samples/comment.c](samples/comment.c) and [samples/comment.cpp](samples/comment.cpp), including Doxygen comments.

For Python comments and docstrings, use [samples/comment.py](samples/comment.py). For JSON, YAML, and TOML comments, use [samples/comment.jsonc](samples/comment.jsonc), [samples/comment.yaml](samples/comment.yaml), and [samples/comment.toml](samples/comment.toml). For shell script, PowerShell, Dockerfile, and Docker Compose comments, use [samples/comment.sh](samples/comment.sh), [samples/comment.ps1](samples/comment.ps1), [samples/Dockerfile](samples/Dockerfile), and [samples/compose.yaml](samples/compose.yaml). In [samples/paragraph.md](samples/paragraph.md), select the paragraph, run `Translate selection to ja` from Code Actions, then Hover over the selection. The Hover header identifies it as `Mock translation (fixed sample)`. Mock mode requires manual execution for Markdown.

Mock mode supports only these samples and their newline variations. It returns an error for other text or target languages other than Japanese. The Hover identifies mock translations.

## Basic usage

| Operation | How |
| --- | --- |
| Comment Hover | Hover over a supported code comment, or place the cursor in it and run Zed's `editor: hover`. |
| Markdown selection | With ChatGPT connected and plan use enabled, or with the Anthropic provider selected, and `features.automatic_selection` set to `true`, select Markdown prose and keep the selection stable. Translation starts in the background; Hover over that range to read it. |
| `Translate selection to …` | To translate immediately or recover from a background failure, use the `Translate selection to …` Code Action. Manual execution shows `Translation ready. Hover over the selected text to view it.` when complete. |
| `… and open in tab` | In Markdown or Plain Text, use `Translate selection to … and open in tab` to read the translation in a separate document. With no selection, the action becomes `Translate document to … and open in tab` and translates the whole document. |
| Cancel | To stop one accepted translation, place the cursor or a selection in its range and choose `Cancel translation of lines …` from Code Actions. `Cancel all translations` stops all translation work without canceling an account menu. |

The rules of these operations are described under [Usage](docs/usage.md).

## Before you send text

- Merely passing over a comment can consume provider usage once Zed sends a Hover request.
- If a comment contains a credential, the credential is sent with it.
- With `features.automatic_selection` set to `true`, selecting Markdown prose just to copy it can also send it for translation.
- Mock mode sends no text to an external service.

What each request contains is described under [What is sent](docs/usage.md#what-is-sent). Cost is described on each provider's page.

## Documentation

- [Configuration](docs/configuration.md): the full settings example, the server path, languages, features, timing, size limits, work and cache limits, and changing the model.
- [ChatGPT](docs/providers/chatgpt.md): the settings of the ChatGPT provider and connecting your account.
- [Anthropic API](docs/providers/anthropic.md): the settings of the Anthropic provider and its API key.
- [Usage](docs/usage.md): Hover, selections, translations opened in a tab, status, canceling and retrying, and what is sent.
- [Behavior](docs/behavior.md): admission, what keeps work running, automatic selection, the cache, deadlines, retries, and pauses.
- [Languages](docs/languages.md): supported languages, comment extraction, and language server settings.
- [Development](docs/development.md): checks, CI, and the dependency policy.

## License

[MIT](LICENSE). Each release archive of the server contains the licenses of the server's dependencies and where to download their sources.
