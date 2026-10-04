# Zed Code Translation

[日本語](README.ja.md)

Translate Rust, JavaScript/JSX, and TypeScript/TSX comments in Zed's Hover, or select Markdown prose to prepare a translation in the background. Hover over the original text to read the translation. The source stays in the editor unchanged.

The ChatGPT provider uses your ChatGPT account with permission to use your plan. Without provider settings, the extension runs in mock mode: it translates only the bundled samples into Japanese and sends no text to an external service.

## Installation

You need Rust 1.96 or later and Zed. Build the native language server and the WASM extension from the checkout root:

```sh
cargo build -p zed-code-translation-lsp --release --locked
rustup target add wasm32-wasip2
cargo build --target wasm32-wasip2 --release --locked
```

In Zed's Extensions view, choose `Install Dev Extension` and select **`crates/zed-extension`** in the checkout. See [Zed's development extension instructions](https://zed.dev/docs/extensions/developing-extensions).

## Configuration

Add the following entry to `lsp` in your existing Zed settings. Replace `binary.path` with the absolute path to the executable you built. Choose a model available to the account you will connect; the model and reasoning effort below are examples.

```json
{
  "lsp": {
    "zed-code-translation": {
      "binary": {
        "path": "/absolute/path/to/checkout/target/release/zed-code-translation-lsp",
        "arguments": []
      },
      "initialization_options": {
        "target_language": "ja",
        "source_language": "auto",
        "hover_timeout_ms": 10000,
        "selection_timeout_ms": 30000,
        "markdown_auto_translate": true,
        "selection_debounce_ms": 450,
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

UI labels are in English regardless of `target_language`. `target_language` defaults to `ja`; `source_language` defaults to `auto`. With `auto`, the same translation request identifies the source language. `provider.model` is required for ChatGPT. `reasoning_effort` is optional and accepts `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, or `max`. Unsupported models or effort values return an error.

Translation timing options are optional:

| Setting | Default | Allowed values | Effect |
| --- | --- | --- | --- |
| `hover_timeout_ms` | `10000` | Integer `1000`–`120000`, inclusive | Deadline for each comment or pending selection Hover request. |
| `selection_timeout_ms` | `30000` | Integer `1000`–`120000`, inclusive | Deadline for each manual selection request and Markdown background translation. |
| `markdown_auto_translate` | `true` | Boolean | Start Markdown prose translation after a stable selection. Set `false` to keep manual selection translation and comment Hover. |
| `selection_debounce_ms` | `450` | Integer `100`–`2000`, inclusive | Native server's wait after observing a new selection. Zed's own observation delay is additional. |

Unknown fields, wrong value types, and out-of-range values prevent the language server from starting and return an English settings error. Omitting a field uses its default.

Restart the language server after changing these settings. Settings changes do not apply to a running server. See [Zed's language server settings](https://zed.dev/docs/configuring-languages#configuring-language-servers).

If `binary.path` is omitted, the extension looks for `zed-code-translation-lsp` on the worktree's PATH. If you explicitly configure a language's `language_servers`, add `"zed-code-translation"` to the existing array. Zed uses the `JavaScript` settings for `.jsx` and `TSX` for `.tsx`.

## Changing the translation model

1. Open Zed's Command Palette and run `zed: open settings file`.
2. Edit `lsp.zed-code-translation.initialization_options.provider.model` with the model slug you want to use.
3. Set `lsp.zed-code-translation.initialization_options.provider.reasoning_effort` if needed. Omit it to use the API default, or specify one of the values listed above. An explicit `none` differs from omitting the setting.
4. Save the settings. In a document using this extension, run `editor: restart language server` from the Command Palette.

The extension sends the configured model slug to the translation API without substituting another model. The API determines whether that model and reasoning effort are available to your account.

## Connecting to ChatGPT

Open Code Actions in a supported document (`⌘.` on macOS), choose `ChatGPT account…`, then run `Continue with ChatGPT / Choose account`. Add a registration or choose a saved one, and complete sign-in and plan permission in the browser. Loading settings, opening the account menu, and Hover do not start an OAuth sign-in flow.

When the account is ready, the connection item becomes `Change ChatGPT account` and the plan permission item is hidden. Sign-out appears only while signed in. While a management operation is running, duplicate management candidates are hidden. If the connection state cannot be read, use `Check ChatGPT connection…` and follow its message.

The account menu includes:

- `Enable ChatGPT plan use`: grant plan permission when signed in without it.
- `ChatGPT Usage`: open [ChatGPT's usage settings](https://chatgpt.com/settings/usage), where your limits and credits apply.
- `Sign out of ChatGPT`: attempt to end the current registration's session and clear its local credentials. Other registrations remain. If the remote session cannot be confirmed as ended, follow the message to disconnect the app in ChatGPT settings.

Credentials are stored locally for this extension, at `~/Library/Application Support/zed-code-translation/` on macOS. Registrations use separate UUIDs even when they share an email address; reconnect through the original registration. There is no API-key setting or automatic switch to another billing route. Authentication follows [Sign in with ChatGPT](https://developers.openai.com/siwc/token-sharing-open-source/sign-in).

## Reading translations

Hover over a supported code comment, or place the cursor in it and run Zed's `editor: hover`. Adjacent line comments of the same kind and indentation are translated together; blank lines or code split the group. Each block comment is handled separately. Comment markers and one following ASCII space are removed from the text submitted for translation.

While translation is queued or running, Zed shows a spinner and text directly in the left status bar. `Queued: 1` means one queued translation. `Translating: 1, queued: 0` means one translation is running with none queued. Requests for the same range count as one translation. Other language-server activity can take priority in this shared area.

The spinner disappears when work ends, including cancellation and timeout. Hover over the original text to check the result. On clients that support server status, translation failures and timeouts leave a warning that clears on retry, editing, or closing the document. Clients without these display capabilities retain Hover and explicit-operation notifications.

With ChatGPT connected, plan use enabled, and `markdown_auto_translate` set to `true`, select Markdown prose and keep the selection stable. The default native wait is 450 ms after Zed reports the selection; Zed also waits before reporting it. Translation starts in the background; Hover over that range to read it. There is no background completion notification. Selections containing only code, link destinations, or whitespace do not start translation. A fragment that changes the interpretation of Markdown by cutting through a code delimiter or another protected span is also excluded. Include the complete delimiters when selecting, or use the manual action. Selecting prose just to copy it can also send it for translation.

To translate immediately or recover from a background failure, use the `Translate selection to …` Code Action. Manual execution shows `Translation ready. Hover over the selected text to view it.` when complete. Selection translation is also available in code languages and other supported documents. If disconnected or plan use is disabled, connect or grant permission through `ChatGPT account…` first. The translation action is hidden when there is no selection.

Check error notifications for failed manual translation or management operations. If token refresh returns an unusable grant such as `invalid_grant`, choose the saved registration under `ChatGPT account…` → `Continue with ChatGPT / Choose account` to reconnect.

Only the extracted comment body or selected text is submitted, including its protected segments. The extension does not submit the entire document or conversation history. Code, link destinations, Markdown syntax, and line endings recognized in the submitted input are preserved in the translation.

The server keeps the last translated range for each document in memory. Editing or closing the document, changing accounts, or reconnecting discards it. Source text and translations are not stored on disk. Failures, interrupted streams, cancellation, and expired requests do not show partial translations.

## Mock mode

To try the bundled samples without connecting, omit `provider` or use `{"name":"mock"}`. Hover over the comment in [samples/comment.rs](samples/comment.rs). In [samples/paragraph.md](samples/paragraph.md), select the paragraph, run `Translate selection to ja` from Code Actions, then Hover over the selection. The Hover header identifies it as `Mock translation (fixed sample)`. Mock mode requires manual execution for Markdown.

Mock mode supports only these samples and their newline variations. It returns an error for other text or target languages other than Japanese. The Hover identifies mock translations.

## Limitations

- Automatic comment Hover supports Rust, JavaScript/JSX, and TypeScript/TSX. Markdown requires a prose selection before background translation; paragraph Hover alone does not start it. Python comments are not supported.
- Selection changes and deselection are observed when Zed sends a new range request. Until then, a background request cannot be canceled immediately. A language server tracks only its most recently observed selection, so separate editors' selections are not tracked independently. Background failures are not automatically retried.
- Unclosed block comments wait until closed. If a fatal JS/TS syntax error stops parsing, later comments are unavailable for automatic extraction. Fix the syntax or use manual selection translation.
- Source text is limited to 16 KiB and translated text to 64 KiB. By default, manual selection and background translation have a 30-second deadline from translation admission; each Hover request has its own 10-second deadline. Worker queue time, API calls, credential refresh, and streaming count toward these deadlines. Selection debounce and waiting for capacity or management operations before admission do not count. Coalesced requests retain their individual deadlines. OAuth, connection, and management operations also have separate internal limits. If busy, retry after the current work finishes.
- Rate limits and account restrictions pause new translations and return an error. Check usage in ChatGPT settings. Temporary network failures retain credentials; wait before retrying.
- Zed controls Hover placement and width. The extension cannot guarantee placement above the text or that the popup leaves all source text visible.

## Development

Both crates use Rust Edition 2024 and a minimum supported Rust version of 1.96. The virtual workspace uses resolver 3. Keep `Cargo.lock` in version control and use `--locked` for checks and release builds:

```sh
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Dependencies use normal Cargo compatibility ranges written as `major.minor` (including `1.0` for `serde` and `serde_json`). Run `cargo update` deliberately, review the lockfile changes, and repeat the checks and both release builds above. Keep the Oxc parser, allocator, and span crates on the same release line. When updating `zed_extension_api`, check the target Zed host's API support as well as the WASM build. HTTP/OIDC tests use local fixtures and synthetic credentials. Each fixture generates an Ed25519 signing key from OS randomness, keeps it in memory, and uses the same key for JWT signatures and JWKS.

The Rust lexer requires matching Unicode tables. The lockfile keeps `unicode-ident` at 1.0.24 (Unicode 17) to match `unicode-properties` 0.1.4; versions 1.0.25 and 1.0.26 use Unicode 18 and fail the lexer's compile-time check. After an update that changes this package, restore the compatible lock entry with `cargo update -p unicode-ident --precise 1.0.24`, or verify that the lexer and both table crates support the same Unicode version.
