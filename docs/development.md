# Development

[日本語](ja/development.md)

This page describes the required Rust versions, the checks to run, CI, and the dependency policy.

## Checks

Both crates use Rust Edition 2024. The native language server requires Rust 1.97 or later; the WASM extension requires Rust 1.96 or later. Dependency updates take priority over retaining an older minimum Rust version; update the affected requirement when needed. The virtual workspace uses resolver 3. Keep `Cargo.lock` in version control and use `--locked` for checks and release builds:

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

The current exception to compatibility ranges is `unicode-ident = "=1.0.24"`, pinned to keep the Rust lexer and Unicode tables on the same Unicode version. Remove the pin and update them together when the lexer and related Unicode tables support the same version.

`granit-parser` 1.3 reports an error for a valid multi-line quoted YAML value whose continuation lines are indented only one column deeper than its key, so comments from that value on are unavailable. Upstream has fixed this, and the fix is expected in 2.0. When updating to 2.0, enable its `comments` feature, remove the `ignore` attribute from the `yaml_reads_past_a_value_continued_one_column_deeper` test, and delete the sentence about this limit from `docs/languages.md` and `docs/ja/languages.md`.
