# Translation LSP

[日本語](README.ja.md)

Translate comments in Rust, JavaScript/JSX, TypeScript/TSX, C#, C, C++, and Python, including Python docstrings, in Zed's Hover. Select Markdown prose to prepare a translation in the background. Hover over the original text to read the translation. The source stays in the editor unchanged.

The ChatGPT provider uses your ChatGPT account with permission to use your plan. Without provider settings, the extension runs in mock mode: it translates only the bundled samples into Japanese and sends no text to an external service.

## Installation

Development and use target **Windows, macOS, and Linux**. Install Zed and [Rust through rustup](https://www.rust-lang.org/tools/install) (Rust 1.97 or later). The native dependencies also need a C/C++ toolchain:

| OS | Build prerequisites | Native executable |
| --- | --- | --- |
| Windows | MSVC Rust toolchain, Visual Studio Build Tools with Desktop development with C++ and a Windows SDK; NASM on x86-64. Use a Visual Studio developer PowerShell or command prompt. | `target/release/translation-lsp.exe` |
| macOS | Xcode Command Line Tools (`xcode-select --install`). | `target/release/translation-lsp` |
| Linux | GCC or Clang with C/C++ development tools; on Debian/Ubuntu, install `build-essential` and `ca-certificates`. | `target/release/translation-lsp` |

Tree-sitter and the TLS provider compile native code. See the TLS provider's [Windows](https://aws.github.io/aws-lc-rs/requirements/windows.html) and [Linux](https://aws.github.io/aws-lc-rs/requirements/linux.html) build requirements for other toolchain configurations.

Run the following commands from the checkout root. They work in PowerShell, Command Prompt, Bash, and Zsh:

```sh
cargo build -p translation-lsp --release --locked
rustup target add wasm32-wasip2
cargo build -p translation-lsp-extension --target wasm32-wasip2 --release --locked
```

In Zed's Extensions view, choose `Install Dev Extension` and select **`crates/zed-extension`** in the checkout. See [Zed's development extension instructions](https://zed.dev/docs/extensions/developing-extensions).

For C# files, install Zed's [C# language extension](https://github.com/zed-extensions/csharp). Its settings key is `CSharp`. If you configure `languages.CSharp.language_servers` explicitly, add `"translation-lsp"` alongside your existing C# server. This translation extension does not replace Roslyn or OmniSharp.

C and C++ are built into Zed. If you list their language servers explicitly, add `"translation-lsp"` under `languages.C.language_servers` and `languages."C++".language_servers`, alongside clangd.

Python is built into Zed. If you configure `languages.Python.language_servers`, add `"translation-lsp"` to that list alongside your existing Python servers.

## Configuration

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

Use a path for the OS where the language server runs, including `.exe` on Windows. For example:

| OS | Example `binary.path` value |
| --- | --- |
| Windows | `C:/Users/you/src/translation-lsp/target/release/translation-lsp.exe` |
| macOS | `/Users/you/src/translation-lsp/target/release/translation-lsp` |
| Linux | `/home/you/src/translation-lsp/target/release/translation-lsp` |

Windows paths can use forward slashes in JSON. If you use backslashes, escape each one as `\\`. Do not include extra shell quotes inside the path value, even when the path contains spaces.

UI labels are in English regardless of `target_language`. `target_language` defaults to `ja`; `source_language` defaults to `auto`. With `auto`, the same translation request identifies the source language. `provider.model` is required for ChatGPT. `reasoning_effort` is optional and accepts `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, or `max`. Unsupported models or effort values return an error.

Translation timing options are optional:

| Setting | Default | Allowed values | Effect |
| --- | --- | --- | --- |
| `hover_timeout_ms` | `20000` | Integer `1000`–`120000`, inclusive | Deadline for each comment or pending selection Hover request. |
| `selection_timeout_ms` | `30000` | Integer `1000`–`120000`, inclusive | Deadline for each manual selection Hover-preview request and Markdown background translation. |
| `document_timeout_ms` | `180000` | Integer `1000`–`600000`, inclusive | Deadline for full-document or selection translations opened in a tab, including queue waiting time. |
| `markdown_auto_translate` | `true` | Boolean | Start Markdown prose translation after a stable selection. Set `false` to keep manual selection translation and comment Hover. |
| `selection_debounce_ms` | `450` | Integer `100`–`2000`, inclusive | Native server's wait after observing a new selection. Zed's own observation delay is additional. |

Translation size limits belong to `initialization_options.translation_limits`. Values are positive integers in KiB (1 KiB = 1024 UTF-8 bytes), not character or token counts. Omitted objects and fields use their own defaults.

| Setting inside `translation_limits` | Default | Applies to |
| --- | --- | --- |
| `input_kib.hover` | `16` | New comment Hover requests, Markdown background translation, and manual selection Hover previews. |
| `input_kib.document` | `1024` | Whole documents and selections translated into a separate tab. |
| `output_kib` | `4096` | Completed translations, regardless of display destination. |

For example, `"translation_limits": {"input_kib": {"document": 2048}}` changes only the document input limit to 2 MiB. Zero, negative, fractional, null, unknown, and nonnumeric values are rejected, as are sizes that cannot be represented on the server platform. Restart the language server after editing these settings. Explicit requests over the input limit report the size, limit, and setting name; oversized automatic selections are skipped. Existing cached translations can still be read without another translation request.

Receive limits account for JSON escaping, repeated output snapshots, and SSE event overhead and scale with `output_kib`. They bound transport processing separately from the decoded translation; they do not allocate the maximum up front. Increasing a local limit does not increase the model's context or output capacity or extend the request deadline. Long documents are sent in one request, without automatic splitting. Failed, incomplete, and oversized results are not displayed or saved as completed translations.

Translations opened in a tab acknowledge the command immediately and open the result when translation completes. Full documents and selections both default to a three-minute deadline, independent of Hover and background translation deadlines. Text length does not automatically extend it. Use the progress indicator to cancel pending work. Editing or closing the source document also prevents the result from opening.

Unknown fields, wrong value types, and out-of-range values prevent the language server from starting and return an English settings error. Omitting a field uses its default.

Restart the language server after changing these settings. Settings changes do not apply to a running server. See [Zed's language server settings](https://zed.dev/docs/configuring-languages#configuring-language-servers).

If `binary.path` is omitted, the extension looks for `translation-lsp` on the worktree's PATH. If you explicitly configure a language's `language_servers`, add `"translation-lsp"` to the existing array. Zed uses the `JavaScript` settings for `.jsx` and `TSX` for `.tsx`.

## Changing the translation model

1. Open Zed's Command Palette and run `zed: open settings file`.
2. Edit `lsp.translation-lsp.initialization_options.provider.model` with the model slug you want to use.
3. Set `lsp.translation-lsp.initialization_options.provider.reasoning_effort` if needed. Omit it to use the API default, or specify one of the values listed above. An explicit `none` differs from omitting the setting.
4. Save the settings. In a document using this extension, run `editor: restart language server` from the Command Palette.

The extension sends the configured model slug to the translation API without substituting another model. The API determines whether that model and reasoning effort are available to your account.

Translation requests use ordinary text output. Choose a model available to your ChatGPT account; the extension does not switch to another model when a request fails.

## Connecting to ChatGPT

In a supported document, run `editor: toggle code actions` from Zed's Command Palette, choose `ChatGPT account…`, then run `Continue with ChatGPT / Choose account`. Add a registration or choose a saved one, and complete sign-in and plan permission in the browser. Loading settings, opening the account menu, and Hover do not start an OAuth sign-in flow.

When the account is ready, the connection item becomes `Change ChatGPT account` and the plan permission item is hidden. Sign-out appears only while signed in. While a management operation is running, duplicate management candidates are hidden. If the connection state cannot be read, use `Check ChatGPT connection…` and follow its message.

The account menu includes:

- `Enable ChatGPT plan use`: grant plan permission when signed in without it.
- `ChatGPT Usage`: open [ChatGPT's usage settings](https://chatgpt.com/settings/usage), where your limits and credits apply.
- `Sign out of ChatGPT`: attempt to end the current registration's session and clear its local credentials. Other registrations remain. If the remote session cannot be confirmed as ended, follow the message to disconnect the app in ChatGPT settings.

Credentials are stored locally in this extension's OS-specific configuration directory:

| OS | Credential directory |
| --- | --- |
| Windows | `%LOCALAPPDATA%\translation-lsp\` |
| macOS | `~/Library/Application Support/translation-lsp/` |
| Linux | `$XDG_CONFIG_HOME/translation-lsp/`, or `~/.config/translation-lsp/` when unset |

The server resolves this location through the OS using `dirs::config_local_dir()`. Registrations use separate UUIDs even when they share an email address; reconnect through the original registration. There is no API-key setting or automatic switch to another billing route. Authentication follows [Sign in with ChatGPT](https://developers.openai.com/siwc/token-sharing-open-source/sign-in).

Authentication data uses `auth.json` version 2. On first access, the server migrates version 1 under the credential-store lock without creating a credential backup. Migration preserves tokens, registrations, and the selected account. Each registration receives an authorization ID: token refresh preserves it, while reconnecting replaces it. Refresh and temporary store locks preserve cached translations; the server checks authorization again before displaying them.

Older binaries cannot read version 2. To downgrade, convert the current credentials to version 1 or sign in again.

## Reading translations

Hover over a supported code comment, or place the cursor in it and run Zed's `editor: hover`. Adjacent line comments of the same kind and indentation are translated together; blank lines or code split the group. Each block comment is handled separately. Comment markers and one following ASCII space are removed from the text submitted for translation.

Up to two translations in different documents run at once, with up to four admitted in total. Only one range per document can be in flight. Requests for the same range share one translation.

Accepted requests retain their queue order. Canceled and expired queued requests are removed before admitting new work, even when both workers are occupied. A different range in a document that already has a translation in flight is rejected as busy rather than added to that document's queue. The admission limit and deadlines still apply.

Changing or clearing the observed Markdown selection retires its automatic translation. An explicit translation of that range or another pending Hover can keep the shared work alive. Manual translation actions survive selection changes and ordinary menu operations; opening and dismissing `ChatGPT account…` also leaves translations running. Editing anywhere in the source document, closing it, changing authorization, canceling translation progress, or shutting down the server retires affected work. An editor cancellation of an individual Hover or pending selection command removes only that request; work continues while another request still needs it. Document-tab translation acknowledges admission immediately and continues independently of that command response. Queue time counts toward every translation deadline.

While translation is queued or running, Zed shows a spinner and text directly in the left status bar. `Queued: 1` means one queued translation. `Translating: 1, queued: 0` means one translation is running with none queued. Requests for the same range count as one translation. Other language-server activity can take priority in this shared area.

The spinner disappears when work ends, including cancellation and timeout. Hover over the original text to check the result. On clients that support server status, translation failures and timeouts leave a warning that clears on retry, editing, or closing the document. Clients without these display capabilities retain Hover and explicit-operation notifications.

With ChatGPT connected, plan use enabled, and `markdown_auto_translate` set to `true`, select Markdown prose and keep the selection stable. The default native wait is 450 ms after Zed reports the selection; Zed also waits before reporting it. Translation starts in the background; Hover over that range to read it. There is no background completion notification. Selections containing only code, link destinations, or whitespace do not start translation. A fragment that changes the interpretation of Markdown by cutting through a code delimiter or another protected span is also excluded. Include the complete delimiters when selecting, or use the manual action. Selecting prose just to copy it can also send it for translation.

To translate immediately or recover from a background failure, use the `Translate selection to …` Code Action. Manual execution shows `Translation ready. Hover over the selected text to view it.` when complete. Selection translation is also available in code languages and other supported documents. If disconnected or plan use is disabled, connect or grant permission through `ChatGPT account…` first. The Hover translation action is hidden when there is no selection.

In Markdown or Plain Text, use `Translate selection to … and open in tab` to read the translation in a separate document. With no selection, the action becomes `Translate document to … and open in tab` and translates the whole document. Both operations use `translation_limits.input_kib.document`, which defaults to 1 MiB. The client must support `window/showDocument`.

Markdown translations open as temporary `.md` files. Run `markdown: open preview` from the Command Palette to see the rendered document and return to its source tab when needed. Plain Text translations open as `.txt` files. The extension leaves your normal Markdown opening preference unchanged. Existing translation tabs are snapshots. Edit the original and run the action again to obtain a new translation.

Use **Save As** to keep a translation before restarting the language server or closing Zed. The server removes its temporary files on normal shutdown; an abnormal exit can leave them in the system temporary directory. Saved copies are yours to keep.

Relative links and images resolve from the temporary file location. To use the original relative paths, save the translation beside the source.

Check error notifications for failed manual translation or management operations. If token refresh returns an unusable grant such as `invalid_grant`, choose the saved registration under `ChatGPT account…` → `Continue with ChatGPT / Choose account` to reconnect.

Completed translations are displayed as text. The model receives the selected passage as a whole and is instructed to return only its translation, preserving meaning, code, identifiers, and URLs. The server does not split the translation into JSON fragments or reject it for changes in Markdown formatting or line breaks. These instructions do not guarantee translation accuracy.

Explicit selection and document actions send the requested text even when it is indented or contains only code. The model is instructed to preserve code and identifiers. Markdown prose eligibility limits only automatic background translation; the extension does not report the original text as a successful translation without contacting the provider.

The extension submits the extracted comment body, the selected text, or the whole document when you explicitly choose document translation. It does not submit conversation history or write translations back into the source document.

The server keeps the last translated range for each document in memory. Editing or closing the document, changing accounts, or reconnecting discards it. Hover does not store text on disk. Opening a translation tab writes the completed translation to a temporary file as described above. Failures, interrupted streams, cancellation, and expired requests do not show partial translations.

## Mock mode

To try the bundled samples without connecting, omit `provider` or use `{"name":"mock"}`. Hover over the comment in [samples/comment.rs](samples/comment.rs), or the ordinary and XML documentation comments in [samples/comment.cs](samples/comment.cs). C/C++ samples are available in [samples/comment.c](samples/comment.c) and [samples/comment.cpp](samples/comment.cpp), including Doxygen comments.

For Python comments and docstrings, use [samples/comment.py](samples/comment.py). In [samples/paragraph.md](samples/paragraph.md), select the paragraph, run `Translate selection to ja` from Code Actions, then Hover over the selection. The Hover header identifies it as `Mock translation (fixed sample)`. Mock mode requires manual execution for Markdown.

Mock mode supports only these samples and their newline variations. It returns an error for other text or target languages other than Japanese. The Hover identifies mock translations.

## Limitations

- Automatic comment Hover supports Rust, JavaScript/JSX, TypeScript/TSX, C#, C, C++, and Python, including Python docstrings. Markdown requires a prose selection before background translation; paragraph Hover alone does not start it.
- Selection changes and deselection are observed when Zed sends a new range request. Until then, a background request cannot be canceled immediately. A language server tracks only its most recently observed selection, so separate editors' selections are not tracked independently. Background failures do not start a new translation job. An explicit temporary service error can trigger one retry within the current job.
- C# supports `//`, `/* ... */`, `///`, and `/** ... */`. Adjacent line comments of the same kind and indentation are translated together. XML documentation is displayed as tagged text in Hover; the model is instructed to retain tags and attributes. There is no XML renderer or structural validation. Comments inside ambiguous syntax-error regions are skipped by automatic Hover; manual selection translation remains available.
- C/C++ supports ordinary and Doxygen comments (`///`, `//!`, `/**`, `/*!`, including trailing `<` forms). Documentation commands such as `@param` remain in the displayed text; the model is instructed to retain commands and parameter names. Syntax-error regions are skipped, so recognized comments after macros or incomplete expressions remain available. If an error region contains a quote or block-comment delimiter, extraction stops there to avoid treating unfinished literal contents as comments. Use manual selection for those ambiguous regions.
- Python supports `#` comments and docstrings at the beginning of a module, class, or function body. Ordinary, raw, Unicode, parenthesized, and implicitly concatenated string docstrings are supported; assigned strings, f-strings, and bytes are not automatic targets. Quotes and string prefixes are removed from docstrings; source escapes and indentation are passed through without evaluating Python. Shebangs are skipped. Syntax-error regions are skipped. If an error region contains a quote, extraction stops there to avoid translating apparent comments inside unfinished strings; manual selection remains available.
- Unclosed block comments wait until closed. If a fatal JS/TS syntax error stops parsing, later comments are unavailable for automatic extraction. Fix the syntax or use manual selection translation.
- Default input limits are 16 KiB for Hover previews and 1 MiB for documents; the default output limit is 4 MiB. Configure them with `translation_limits`. From translation admission, translations opened in a tab default to 180 seconds, manual selection Hover previews and background translation to 30 seconds, and each Hover request to 20 seconds. Worker queue time, API calls, credential refresh, and streaming count toward these deadlines. Selection debounce and waiting for capacity or management operations before admission do not count. Coalesced requests retain their individual deadlines. OAuth, connection, and management operations also have separate internal limits. If busy, retry after the current work finishes.
- An explicit temporary service error, including an overload reported after HTTP 200, is retried at most once within the original deadline. The server waits two seconds by default, or uses a numeric `Retry-After` of up to 120 seconds. Other `Retry-After` values leave the error for manual retry. Cancellation, document changes, and account changes also stop a pending retry. Connection failures and interrupted streams are not automatically replayed.
- Usage limits, account restrictions, and configuration errors pause automatic translation and return an error. After resolving the cause, explicitly run a selection or document translation to retry; restarting or reconnecting is unnecessary. Automatic translation stays paused until an explicit retry succeeds. Check usage in ChatGPT settings. Failures retain credentials unless token refresh confirms an unusable grant.
- Zed controls Hover placement and width. The extension cannot guarantee placement above the text or that the popup leaves all source text visible.

## Development

Both crates use Rust Edition 2024. The native language server requires Rust 1.97 or later; the WASM extension requires Rust 1.96 or later. Dependency updates take priority over retaining an older minimum Rust version; update the affected requirement when needed. The virtual workspace uses resolver 3. Keep `Cargo.lock` in version control and use `--locked` for checks and release builds:

```sh
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

The [CI workflow](.github/workflows/ci.yml) runs workspace tests, formatting, Clippy, the native release build, and the WASM release build on Windows, macOS, and Linux. Local success on one OS does not establish success on the others. Before releasing, check the CI results for all three OSes and test the Zed flow on each: install the extension, locate the native server, connect, translate a comment, and open and save a document translation. HTTP/OIDC fixtures use synthetic credentials and do not require a ChatGPT account.

Dependencies use normal Cargo compatibility ranges written as `major.minor` (including `1.0` for `serde` and `serde_json`). Run `cargo update` deliberately, review the lockfile changes, and repeat the checks and both release builds above. Keep the Oxc parser, allocator, and span crates on the same release line. When updating `zed_extension_api`, check the target Zed host's API support as well as the WASM build. HTTP/OIDC tests use local fixtures and synthetic credentials. Each fixture generates an Ed25519 signing key from OS randomness, keeps it in memory, and uses the same key for JWT signatures and JWKS.

The Rust lexer (`ra-ap-rustc_lexer` 0.176) requires matching Unicode tables. The native server manifest pins `unicode-ident` to `=1.0.24` (Unicode 17) to match `unicode-properties` 0.1.4. Versions 1.0.25 and 1.0.26 use Unicode 18 and fail the lexer's compile-time check. Keep this compatibility constraint until the lexer and both table crates support the same Unicode version; then remove the pin and update them together.

The OIDC SDK still depends on `ed25519-dalek` 2.2 and `rand_core` 0.6; the fixture generator uses `ed25519-dalek` 3.0 with `getrandom` 0.4. These versions coexist without changing the SDK. Cargo currently resolves the SDK's older cryptography stack to `crypto-common` 0.1.6 and `generic-array` 0.14.9. The [0.1.7 release](https://github.com/RustCrypto/traits/pull/2088) only pins `generic-array` to 0.14.7 to suppress deprecation warnings. This project does not force that downgrade.

C# comment extraction uses Tree-sitter 0.27 and `tree-sitter-c-sharp` 0.23. Prefer evaluating Tree-sitter grammars for additional code languages. Rust continues to use its lexer, and JavaScript/TypeScript use Oxc. Parser input normalizes C# Unicode line terminators while preserving byte offsets; comment text is always read from the unchanged source.

C/C++ extraction uses `tree-sitter-c` 0.24 and `tree-sitter-cpp` 0.23 with the same Tree-sitter runtime. Raw strings and backslash-continued line comments are parsed by their language grammars.

Python extraction uses `tree-sitter-python` 0.25. Docstrings are identified by their position in the syntax tree, not by triple quotes alone.
