# Development

[日本語](ja/development.md)

This page describes the required Rust versions, the checks to run, CI, the dependency policy, and the dependency notices.

## Checks

The crates use Rust Edition 2024. The native language server and `xtask` require Rust 1.97 or later. The WASM extension declares no `rust-version`: Cargo selects dependency versions for the whole workspace that suit the lowest `rust-version` among its members, so declaring the older compiler of the Zed registry there would hold back the dependencies of the native server. CI builds the extension with the registry's compiler instead. Dependency updates take priority over retaining an older minimum Rust version; update the affected requirement when needed. The virtual workspace uses resolver 3. Keep `Cargo.lock` in version control and use `--locked` for checks and release builds:

```sh
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## CI and release checks

The [CI workflow](../.github/workflows/ci.yml) runs workspace tests, formatting, Clippy, the native release build, and the WASM release build on Windows, macOS, and Linux. Local success on one OS does not establish success on the others. Before releasing, check the CI results for all three OSes and test the Zed flow on each: install the extension, locate the native server, connect, translate a comment, and open and save a document translation. HTTP/OIDC fixtures use synthetic credentials and do not require a ChatGPT account or an Anthropic API key. Tests do not call the real APIs or read the credential directory of the user: the tests that start the server process start only the mock provider, and the tests of the ChatGPT and Anthropic providers use directories of their own.

CI runs the tests one at a time (`RUST_TEST_THREADS` is `1`), so the tests that measure time do not share the runner's CPUs with other tests.

The Zed registry builds extensions with its own compiler, currently Rust 1.90. A second CI job builds the WASM extension with that compiler. When a dependency of the extension needs a newer compiler, decide again how to stay compatible with the registry.

The development-only `xtask` crate packages the native server and checks what a release is made from. The same CI job runs these two commands:

```sh
cargo run -p xtask --locked -- check-source
cargo run -p xtask --locked -- check-notices
```

`check-source` checks that the versions, licenses, and release targets agree. `check-notices` generates the dependency notices described under [Dependency notices](#dependency-notices). `cargo run -p xtask --locked -- --help` lists the packaging commands.

The extension downloads the server from the GitHub release of its own version. The [release assets workflow](../.github/workflows/release-assets.yml) runs the packaging commands for every release target and, when started with `draft-release` enabled, creates a draft release that holds the six archives and `SHA256SUMS`. Publish that release before registering the version with Zed.

## Dependencies

Specify dependencies with Cargo compatibility ranges written as `major.minor`. Document a compatibility reason for any exact version pin. When updating dependencies, review the lockfile changes from `cargo update`, run the checks above, and build both the native server and WASM extension in release mode. Keep related Oxc crates on the same release line. When updating `zed_extension_api`, also check the target Zed host's API support.

Evaluate Tree-sitter grammars first when adding comment extraction for a code language. JSON, TOML, and YAML comments are decided by tokens alone, so read them with a lexer instead of a grammar. Keep the existing lexer for Rust and Oxc for JavaScript and TypeScript.

JSON, TOML, shell script, PowerShell, and Dockerfile comments are read by the server's own readers, and YAML with the scanner of `granit-parser`. A reader runs on the thread that handles requests, so it must take time in proportion to the size of the text, whatever the text holds. The server's own readers read each part once, without starting again from an earlier place, and do not recurse. The shell and Dockerfile readers keep what is open on a stack of their own, so only memory limits the depth of nesting. The tests `readers_take_time_in_proportion_to_the_text` and `readers_take_time_in_proportion_to_texts_of_several_parts` check the time of all six readers on repeated pieces of text; add a new reader to both. They do not reach every text, so when changing a reader, check each place that reads back or ahead of the current character.

The current exception to compatibility ranges is `unicode-ident = "=1.0.24"`, pinned to keep the Rust lexer and Unicode tables on the same Unicode version. Remove the pin and update them together when the lexer and related Unicode tables support the same version.

`granit-parser` 1.3 reports an error for a valid multi-line quoted YAML value whose continuation lines are indented only one column deeper than its key, so comments from that value on are unavailable. Upstream has fixed this, and the fix is expected in 2.0. When updating to 2.0, enable its `comments` feature, remove the `ignore` attribute from the `yaml_reads_past_a_value_continued_one_column_deeper` test, and delete the sentence about this limit from `docs/languages.md` and `docs/ja/languages.md`.

## Dependency notices

Each release archive of the server contains `THIRD_PARTY_LICENSES.txt`: the licenses of the server's dependencies on that target, and where to download the source of each. `xtask` generates the file while packaging and does not keep it in the repository. It runs [cargo-about](https://github.com/EmbarkStudios/cargo-about) 0.9.2 with `about.toml` and the template `xtask/notices.hbs`. Install that version with:

```sh
cargo install --locked --features cli cargo-about@0.9.2
```

`about.toml` lists the accepted licenses. A dependency that offers none of them makes `check-notices` and packaging fail; review its license before adding it to the list.

`THIRD_PARTY_LICENSES.txt` gives each license in the text of the crate's authors, with their copyright notice. Where cargo-about finds no license file in a crate by itself, `about.toml` names the file and its SHA-256 checksum. For a crate that ships no license file, it names a file of the crate's repository on GitHub, and cargo-about downloads that file, at the commit the crate was published from, every time it generates the notices. Generating them therefore needs network access.

When a checksum differs or a download fails, cargo-about uses the standard text of the license, which names no copyright holder, and still succeeds. `xtask` then fails and names the crates; if a download failed, it first runs cargo-about again, up to three times in all. `STANDARD_TEXT` in `xtask/src/notices.rs` lists the crates that may stand under a standard text, each with the reason; `THIRD_PARTY_LICENSES.txt` says so above that text.

cargo-about uses the license that `about.toml` gives for a crate in place of the license the crate declares, and its checksums cover only the files named there. `xtask` therefore also fails when a crate declares another license than its entry gives. `DECLARED` in `xtask/src/notices.rs` lists the crates whose entry differs from the declaration on purpose, each with the declaration that the entry was written for.

When `check-notices` fails after a dependency update, read the license file of each crate it names before changing `about.toml`. If the file changed, review the change, then set the checksum to the SHA-256 checksum of the new file. If `about.toml` does not name the crate, add an entry for its license file. If a crate declares another license, also look for license files that the crate added, then update its entry, and `DECLARED` if the entry differs from the declaration on purpose. A file of a crate is in Cargo's registry source directory; a file of a repository is at `https://raw.githubusercontent.com/<owner>/<repository>/<commit>/<path>`, with the commit that the crate's `.cargo_vcs_info.json` records. `ra-ap-rustc_lexer` records no commit there, so also copy the commit named in its description to `override-git-commit`.

To change the cargo-about version, change it in `xtask/src/notices.rs` and in both workflows.
