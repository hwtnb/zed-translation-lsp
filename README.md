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
| `hover_timeout_ms` | `20000` | Integer `1000`–`120000`, inclusive | Response deadline from receipt, including queue time, for each Hover request. |
| `selection_timeout_ms` | `30000` | Integer `1000`–`120000`, inclusive | Execution budget from dispatch for retained manual selections and Markdown background translations. |
| `document_timeout_ms` | `180000` | Integer `1000`–`600000`, inclusive | Execution budget from dispatch for full-document or selection translations opened in a tab; queue time is additional. |
| `markdown_auto_translate` | `true` | Boolean | Start Markdown prose translation after a stable selection. Set `false` to keep manual selection translation and comment Hover. |
| `selection_debounce_ms` | `450` | Integer `100`–`2000`, inclusive | Native server's wait after observing a new selection. Zed's own observation delay is additional. |

Translation size limits belong to `initialization_options.translation_limits`. Values are positive integers in KiB (1 KiB = 1024 UTF-8 bytes), not character or token counts. Omitted objects and fields use their own defaults.

| Setting inside `translation_limits` | Default | Applies to |
| --- | --- | --- |
| `input_kib.hover` | `16` | New comment Hover requests, Markdown background translation, and manual selection Hover previews. |
| `input_kib.document` | `1024` | Whole documents and selections translated into a separate tab. |
| `output_kib` | `4096` | Completed translations, regardless of display destination. |

For example, `"translation_limits": {"input_kib": {"document": 2048}}` changes only the document input limit to 2 MiB. Zero, negative, fractional, null, unknown, and nonnumeric values are rejected, as are sizes that cannot be represented on the server platform. Restart the language server after editing these settings. Explicit requests over the input limit report the size, limit, and setting name; oversized automatic selections are skipped. Existing cached translations can still be read without another translation request.

Work and cache limits are optional objects inside `translation_limits`:

```json
"translation_limits": {
  "work": { "admitted": 8, "concurrent": 2, "documents": 1, "automatic": 3, "hover_waiters": 16 },
  "cache": { "entries": 64, "capacity_kib": 16384 }
}
```

`admitted` counts running and queued translation operations, including canceled workers until they terminate. `concurrent` limits all executing translations, `documents` limits operations dispatched as tab translations, `automatic` limits jobs retained by comment Hover or automatic selection without a manual or tab request, and `hover_waiters` limits individual pending Hover responses. All counts must be positive integers. Require `documents <= concurrent <= admitted` and `automatic < admitted`. This reserves admission capacity for explicit requests, but does not reserve an idle worker. Increasing concurrency can increase simultaneous provider usage.

`cache.entries` bounds the combined successful, failed, and tab cache entries across all documents. `cache.capacity_kib` bounds their stored text, not total process memory. The effective capacity is the greater of this value and `output_kib`, so one maximum-size result can fit. Metadata, source documents, in-flight input, and temporary tab files are outside this text budget. Defaults are a starting point for sequential reading, not measured provider or hardware limits. Omitted nested fields retain their defaults; restart the language server after changing them.

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

These ceilings are a local resource policy, not measured hardware limits or provider guarantees. The input budget bounds accepted source payloads; the smaller output budget leaves room for escaped SSE events and multiple decoded output snapshots. The individual input caps allow larger documents while keeping automatic/selection Hover input more restricted. Job and cache-entry caps also bound scheduler and cache scans when texts are tiny. Cache text has a separate 256 MiB ceiling. Source documents, parser state, allocator overhead, transport buffers, and preview files are additional costs, so these figures are **not a process-memory guarantee**. The default settings are unchanged. Raising a limit beyond these ceilings requires reviewing this policy in the server, rather than overriding it in Zed.

Receive limits account for JSON escaping, repeated output snapshots, and SSE event overhead and scale with `output_kib`. They bound transport processing separately from the decoded translation; they do not allocate the maximum up front. Increasing a local limit does not increase the model's context or output capacity or extend the request deadline. Long documents are sent in one request, without automatic splitting. Failed, incomplete, and oversized results are not displayed or saved as completed translations.

Reopening the same cached tab translation reuses its temporary file and focuses that document. If the file was deleted or its saved contents changed, the server creates a fresh preview without overwriting it. A newly translated result gets a separate file. Temporary previews are still removed on server exit; copies saved with Save As are preserved.

Translations opened in a tab acknowledge the command immediately and open the result when translation completes. Full documents and selections both default to a three-minute execution budget, in addition to time spent in the queue. Text length does not automatically extend it. Use the range or all-translations cancellation Code Actions to stop pending work. Editing or closing the source document also prevents the result from opening.

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

By default, up to two translations run at once and eight are accepted across all open documents. Multiple ranges in the same document can be accepted. Exact duplicate targets share one operation. At most one tab translation starts at a time, leaving capacity for range translations. Accepted work keeps its queue order, except that range work may bypass a tab waiting for the tab limit. A tab request for an already-running range shares that operation and does not acquire another slot. Configure these limits below `translation_limits.work`.

Selection and tab commands acknowledge acceptance immediately. Changing or clearing a selection, switching tabs, or canceling that command's LSP response does not cancel accepted work. A Hover response can be canceled or time out independently, without canceling an accepted comment translation. Moving from comment A to B keeps A running or queued; return to either comment to read its completed result. Merely passing over a comment can therefore consume provider usage once Zed sends a Hover request. Editing anywhere in the source document, closing it, changing authorization, or shutting down the server invalidates affected work and cached results. Token refresh does not invalidate them. Opening and dismissing `ChatGPT account…` leaves translations running.

To stop one accepted translation, place the cursor or a selection in its range and choose `Cancel translation of lines …` from Code Actions. `Cancel all translations` stops all translation work without canceling an account menu. Old cancellation actions cannot cancel a later request for the same range. The server also accepts progress cancellation for all translations, if the client exposes that control. The actual Zed progress control and action refresh need UI verification; Code Actions are the primary cancellation route.

A stable automatic selection that overlaps the immediately preceding selection replaces only that previous operation's automatic ownership. Empty or ineligible selections do not break this adjustment chain. Observing another document or a non-overlapping eligible selection does. Explicit requests and other Hover waiters keep shared work alive. Comment Hover and automatic selection share the default limit of three automatic operations. A comment Hover rejected at capacity is not retained; return to it when capacity is available or run a manual selection translation. For automatic selection, when capacity is full, only the latest unaccepted candidate remains: selecting D then E may leave D untranslated. The candidate is labeled `selection not yet accepted` and is not included in the accepted queue count. An explicit request rejected for capacity reports that it was not accepted.

Hover first returns the newest completed translation covering the position. A newer pending or failed request never hides a readable completed result. Without a completed result, Hover joins a pending range, or displays its recorded failure with a manual retry instruction. It can find previously requested ranges after deselection, including manual selections in code files. Manual completion and failure notifications identify the file name and lines. Automatic completion does not produce a popup. To retry a failed range without selecting its text again, place the cursor inside it and choose `Retry translation of lines …` from Code Actions. The action retries the recorded range as an explicit request, including after a usage-limit pause. Hover alone continues to show the failure. Code Action requests restricted to unrelated kinds do not change automatic selection tracking.

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

The server caches multiple ranges in memory, bounded across the server by `translation_limits.cache`. The most recently requested successful range covering the Hover position wins, even if requests finish out of order. A new successful result removes older results that it fully contains. Other entries are evicted in least-recently-used order when count or capacity is exceeded. Tab results are separate from Hover results, with at most one successful tab result per source document, and share the global cache budget. Reopening the same cached tab result reuses its temporary file unless it was deleted or its saved contents changed.

After eviction, Markdown Hover does not automatically translate the range again; select it or run the manual action. Comment Hover can start a new translation. Editing or closing the source document, changing accounts, or reconnecting removes affected entries. Hover does not store text on disk; opening a translation tab writes the completed result to a temporary file. Failed, interrupted, canceled, and expired work never publishes a partial translation. A retained range failure is shown as an error, not as translated text.

## Mock mode

To try the bundled samples without connecting, omit `provider` or use `{"name":"mock"}`. Hover over the comment in [samples/comment.rs](samples/comment.rs), or the ordinary and XML documentation comments in [samples/comment.cs](samples/comment.cs). C/C++ samples are available in [samples/comment.c](samples/comment.c) and [samples/comment.cpp](samples/comment.cpp), including Doxygen comments.

For Python comments and docstrings, use [samples/comment.py](samples/comment.py). In [samples/paragraph.md](samples/paragraph.md), select the paragraph, run `Translate selection to ja` from Code Actions, then Hover over the selection. The Hover header identifies it as `Mock translation (fixed sample)`. Mock mode requires manual execution for Markdown.

Mock mode supports only these samples and their newline variations. It returns an error for other text or target languages other than Japanese. The Hover identifies mock translations.

## Limitations

- Automatic comment Hover supports Rust, JavaScript/JSX, TypeScript/TSX, C#, C, C++, and Python, including Python docstrings. Markdown requires a prose selection before background translation; paragraph Hover alone does not start it.
- Selection changes are observed through Zed range requests. One server tracks the latest candidate and adjustment chain, not independent selections in separate editors. Previously accepted distinct ranges remain. Copying selected prose can still trigger automatic translation. A failed background operation is not automatically submitted again.
- C# supports `//`, `/* ... */`, `///`, and `/** ... */`. Adjacent line comments of the same kind and indentation are translated together. XML documentation is displayed as tagged text in Hover; the model is instructed to retain tags and attributes. There is no XML renderer or structural validation. Comments inside ambiguous syntax-error regions are skipped by automatic Hover; manual selection translation remains available.
- C/C++ supports ordinary and Doxygen comments (`///`, `//!`, `/**`, `/*!`, including trailing `<` forms). Documentation commands such as `@param` remain in the displayed text; the model is instructed to retain commands and parameter names. Syntax-error regions are skipped, so recognized comments after macros or incomplete expressions remain available. If an error region contains a quote or block-comment delimiter, extraction stops there to avoid treating unfinished literal contents as comments. Use manual selection for those ambiguous regions.
- Python supports `#` comments and docstrings at the beginning of a module, class, or function body. Ordinary, raw, Unicode, parenthesized, and implicitly concatenated string docstrings are supported; assigned strings, f-strings, and bytes are not automatic targets. Quotes and string prefixes are removed from docstrings; source escapes and indentation are passed through without evaluating Python. Shebangs are skipped. Syntax-error regions are skipped. If an error region contains a quote, extraction stops there to avoid translating apparent comments inside unfinished strings; manual selection remains available.
- Unclosed block comments wait until closed. If a fatal JS/TS syntax error stops parsing, later comments are unavailable for automatic extraction. Fix the syntax or use manual selection translation.
- Default input limits are 16 KiB for Hover previews and 1 MiB for documents; the output limit is 4 MiB. Each Hover response waits up to 20 seconds from receipt, including queue time. Manual and automatic selection work has a 30-second execution budget and tab work has 180 seconds, starting when dispatched to a worker. Retained comment translations use `hover_timeout_ms`, with a default 20-second execution cap starting at dispatch, independently of the Hover response deadline. Repeated joins never restart execution time; adding a longer-lived owner can increase the budget from the original start. API calls, credential refresh, retries, and streaming use that budget. Queue time is additional for retained work, and admission and concurrency limits bound the number of operations ahead of it. OAuth and management operations have separate internal limits.
- An explicit temporary service error, including an overload reported after HTTP 200, is retried at most once within the original deadline. The server waits two seconds by default, or uses a numeric `Retry-After` of up to 120 seconds. Other `Retry-After` values leave the error for manual retry. Cancellation, document changes, and account changes also stop a pending retry. Connection failures and interrupted streams are not automatically replayed.
- Usage limits, account restrictions, and configuration errors pause automatic translation and return an error. After resolving the cause, explicitly run a selection or document translation to retry; restarting or reconnecting is unnecessary. Automatic translation stays paused until an explicit retry succeeds. Jobs accepted before the stop do not send new HTTP requests; repeated stop notifications are suppressed and affected retained ranges keep their failure state. Check usage in ChatGPT settings. Failures retain credentials unless token refresh confirms an unusable grant.
- Zed controls Hover placement and width. The extension cannot guarantee placement above the text or that the popup leaves all source text visible.

## Development

Both crates use Rust Edition 2024. The native language server requires Rust 1.97 or later; the WASM extension requires Rust 1.96 or later. Dependency updates take priority over retaining an older minimum Rust version; update the affected requirement when needed. The virtual workspace uses resolver 3. Keep `Cargo.lock` in version control and use `--locked` for checks and release builds:

```sh
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

The [CI workflow](.github/workflows/ci.yml) runs workspace tests, formatting, Clippy, the native release build, and the WASM release build on Windows, macOS, and Linux. Local success on one OS does not establish success on the others. Before releasing, check the CI results for all three OSes and test the Zed flow on each: install the extension, locate the native server, connect, translate a comment, and open and save a document translation. HTTP/OIDC fixtures use synthetic credentials and do not require a ChatGPT account.

Specify dependencies with Cargo compatibility ranges written as `major.minor`. Document a compatibility reason for any exact version pin. When updating dependencies, review the lockfile changes from `cargo update`, run the checks above, and build both the native server and WASM extension in release mode. Keep related Oxc crates on the same release line. When updating `zed_extension_api`, also check the target Zed host's API support.

Evaluate Tree-sitter grammars first when adding comment extraction for a code language. Keep the existing lexer for Rust and Oxc for JavaScript and TypeScript.

The current exception to compatibility ranges is `unicode-ident = "=1.0.24"`, pinned to keep the Rust lexer and Unicode tables on the same Unicode version. Remove the pin and update them together when the lexer and related Unicode tables support the same version.
