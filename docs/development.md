# Development

[日本語](ja/development.md)

This page describes the required Rust versions, the checks to run, CI, and the dependency policy.

## Checks

Both crates use Rust Edition 2024. The native language server requires Rust 1.97 or later; the WASM extension requires Rust 1.96 or later. Dependency updates take priority over retaining an older minimum Rust version; update the affected requirement when needed. The virtual workspace uses resolver 3. `.gitattributes` checks every text file out with LF line endings on all three OSes; the stdio tests embed the sample files and depend on it. Keep `Cargo.lock` in version control and use `--locked` for checks and release builds:

```sh
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## CI and release checks

The [CI workflow](../.github/workflows/ci.yml) runs workspace tests, formatting, Clippy, the native release build, and the WASM release build on Windows, macOS, and Linux. Local success on one OS does not establish success on the others. Before releasing, check the CI results for all three OSes and test the Zed flow on each: install the extension, locate the native server, connect, translate a comment, and open and save a document translation. HTTP/OIDC fixtures use synthetic credentials and do not require a ChatGPT account or an Anthropic API key. Tests do not call the real APIs, and the stdio tests remove `TRANSLATION_LSP_ANTHROPIC_API_KEY` from the server process.

## Dependencies

Specify dependencies with Cargo compatibility ranges written as `major.minor`. Document a compatibility reason for any exact version pin. When updating dependencies, review the lockfile changes from `cargo update`, run the checks above, and build both the native server and WASM extension in release mode. Keep related Oxc crates on the same release line. When updating `zed_extension_api`, also check the target Zed host's API support.

Evaluate Tree-sitter grammars first when adding comment extraction for a code language. JSON, TOML, and YAML comments are decided by tokens alone, so read them with a lexer instead of a grammar. Keep the existing lexer for Rust and Oxc for JavaScript and TypeScript.

JSON, TOML, shell script, PowerShell, and Dockerfile comments are read by the server's own readers, and YAML with the scanner of `granit-parser`. A reader runs on the thread that handles requests, so it must take time in proportion to the size of the text, whatever the text holds. The server's own readers read each part once, without starting again from an earlier place, and do not recurse. The shell and Dockerfile readers keep what is open on a stack of their own, so only memory limits the depth of nesting. The tests `readers_take_time_in_proportion_to_the_text` and `readers_take_time_in_proportion_to_texts_of_several_parts` check the time of all six readers on repeated pieces of text; add a new reader to both. They do not reach every text, so when changing a reader, check each place that reads back or ahead of the current character.

The current exception to compatibility ranges is `unicode-ident = "=1.0.24"`, pinned to keep the Rust lexer and Unicode tables on the same Unicode version. Remove the pin and update them together when the lexer and related Unicode tables support the same version.

`granit-parser` 1.3 reports an error for a valid multi-line quoted YAML value whose continuation lines are indented only one column deeper than its key, so comments from that value on are unavailable. Upstream has fixed this, and the fix is expected in 2.0. When updating to 2.0, enable its `comments` feature, remove the `ignore` attribute from the `yaml_reads_past_a_value_continued_one_column_deeper` test, and delete the sentence about this limit from `docs/languages.md` and `docs/ja/languages.md`.
