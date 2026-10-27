# Development

[日本語](ja/development.md)

This page describes the required Rust versions, the checks to run, the mock provider, CI, the size limit ceilings, the dependency policy, and the dependency notices.

## Checks

The crates use Rust Edition 2024. The native language server and `xtask` require Rust 1.97 or later. The WASM extension declares no `rust-version`: Cargo selects dependency versions for the whole workspace that suit the lowest `rust-version` among its members, so declaring the older compiler of the Zed registry there would hold back the dependencies of the native server. CI builds the extension with the registry's compiler instead. Dependency updates take priority over retaining an older minimum Rust version; update the affected requirement when needed. The virtual workspace uses resolver 3. Keep `Cargo.lock` in version control and use `--locked` for checks and release builds:

```sh
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## Mock provider

With `"provider": { "name": "mock" }` in the settings, the server uses the mock provider. It sends no text to an external service and returns fixed Japanese translations of the bundled samples, so the Zed flow can be checked without an account or an API key.

Hover over the comment in [samples/comment.rs](../samples/comment.rs), or the ordinary and XML documentation comments in [samples/comment.cs](../samples/comment.cs). C/C++ samples are available in [samples/comment.c](../samples/comment.c) and [samples/comment.cpp](../samples/comment.cpp), including Doxygen comments.

For Python comments and docstrings, use [samples/comment.py](../samples/comment.py). For Go, Java, Kotlin, Swift, Ruby, PHP, and SQL comments, use [samples/comment.go](../samples/comment.go), [samples/comment.java](../samples/comment.java), [samples/comment.kt](../samples/comment.kt), [samples/comment.swift](../samples/comment.swift), [samples/comment.rb](../samples/comment.rb), [samples/comment.php](../samples/comment.php), and [samples/comment.sql](../samples/comment.sql). [samples/comment.html](../samples/comment.html) holds an HTML comment and comments in a script and a style sheet, and [samples/comment.css](../samples/comment.css) a CSS comment. For JSON, YAML, and TOML comments, use [samples/comment.jsonc](../samples/comment.jsonc), [samples/comment.yaml](../samples/comment.yaml), and [samples/comment.toml](../samples/comment.toml). For shell script, PowerShell, Dockerfile, and Docker Compose comments, use [samples/comment.sh](../samples/comment.sh), [samples/comment.ps1](../samples/comment.ps1), [samples/Dockerfile](../samples/Dockerfile), and [samples/compose.yaml](../samples/compose.yaml). In [samples/paragraph.md](../samples/paragraph.md), select the paragraph, run `Translate selection to Japanese (ja)` from Code Actions, then Hover over the selection. To see a dictionary entry, select only `retry_count` in the same sample and run the same action. The mock provider requires manual execution for Markdown.

The mock provider supports only these samples, their newline variations, and the term `retry_count`. It returns an error for other text or target languages other than Japanese. The Hover header identifies its translations as `Mock translation (fixed sample)`.

The stdio tests start the executable with `--mock-delay-ms <milliseconds>`, `--mock-fail`, or `--mock-timeout-ms <milliseconds>` to delay mock translations, make them fail, or set their deadlines. These arguments affect only the mock provider.

## CI and release checks

The [CI workflow](../.github/workflows/ci.yml) runs workspace tests, formatting, Clippy, the native release build, and the WASM release build on Windows, macOS, and Linux. Local success on one OS does not establish success on the others. Before releasing, check the CI results for all three OSes and test the Zed flow on each: install the extension, locate the native server, connect, translate a comment, and open and save a document translation. HTTP/OIDC fixtures use synthetic credentials and do not require a ChatGPT account or an Anthropic API key. Tests do not call the real APIs or read the credential directory of the user: the tests that start the language server as a process start only the mock provider or no provider, and the tests of the ChatGPT and Anthropic providers use directories of their own. The same executable run with `anthropic-key` asks for the Anthropic API key in a terminal and saves it instead of serving an editor. The tests that run it that way give it a home directory of their own and a pseudo-terminal, and run on macOS and Linux only, because Windows takes the credential directory from the OS and not from the environment. The workspace lint `unsafe_code` denies `unsafe` code in every crate; the helper code of these tests is the only exception, allowed at the functions that need it with the reason for each.

CI runs the tests one at a time (`RUST_TEST_THREADS` is `1`), so the tests that measure time do not share the runner's CPUs with other tests.

The Zed registry builds extensions with its own compiler, currently Rust 1.90. A second CI job builds the WASM extension with that compiler. When a dependency of the extension needs a newer compiler, decide again how to stay compatible with the registry.

The development-only `xtask` crate packages the native server and checks what a release is made from. The same CI job runs these two commands:

```sh
cargo run -p xtask --locked -- check-source
cargo run -p xtask --locked -- check-notices
```

`check-source` checks that the versions, licenses, and release targets agree. `check-notices` generates the dependency notices described under [Dependency notices](#dependency-notices). `cargo run -p xtask --locked -- --help` lists the packaging commands.

The extension downloads the server from the GitHub release of its own version. The [release assets workflow](../.github/workflows/release-assets.yml) runs the packaging commands for every release target and, when started with `draft-release` enabled, creates a draft release that holds the six archives and `SHA256SUMS`. Publish that release before registering the version with Zed.

## Size limit ceilings

The ceilings of `translation_limits`, listed under [Size limits](configuration.md#size-limits), bound the memory that configured worst cases can take. The input budget bounds accepted source payloads. The output budget is smaller, because receiving a translation also holds escaped SSE events and several decoded snapshots of the output. The separate input limits allow large documents while keeping Hover and selection input small. The job and cache-entry limits also bound the scans of the scheduler and the cache when texts are tiny. Receive limits account for JSON escaping, repeated output snapshots, and SSE event overhead and scale with `output_kib`; they bound transport processing apart from the decoded translation and do not allocate the maximum up front.

## Dependencies

Specify dependencies with Cargo compatibility ranges written as `major.minor`. Document a compatibility reason for any exact version pin. When updating dependencies, review the lockfile changes from `cargo update`, run the checks above, and build both the native server and WASM extension in release mode. Keep related Oxc crates on the same release line. When updating `zed_extension_api`, also check the target Zed host's API support.

When adding comment extraction for a code language, compare a Tree-sitter grammar with a reader of the server's own. Compare the comments each finds with those of the language's own lexer, on real files and on generated texts, and compare their time. Ruby and PHP use the server's own readers: on these checks they agreed with the lexers of the languages where the grammars did not, and took less than a tenth of the time. Java, Kotlin, and Swift use the server's own readers for the same reasons: they agreed with javac, Kotlin's lexer, and the Swift compiler on real files where the grammars of Kotlin and Swift did not, and took about a fiftieth of the time. Go, HTML, and CSS, whose comments follow from a few rules of their tokenizers, use the server's own readers too. SQL has no lexer that its dialects share; its reader follows the token rules of Zed's SQL grammar, and reads the text between `$$` as SQL to find the comments in the body of a function. Keep the existing lexer for Rust and Oxc for JavaScript and TypeScript.

A reader is measured against the language's own lexer. Fix a reader where it differs from that lexer on real files or in a form that code commonly uses, where it panics, and where its time grows faster than the size of the text. Where it differs only on texts made for the purpose, and the difference only leaves a comment out or takes part of a literal as a comment, describe the difference under the language in [Languages](languages.md) instead of adding to the reader.

Go, Java, Kotlin, Swift, Ruby, PHP, HTML, CSS, SQL, JSON, TOML, shell script, PowerShell, and Dockerfile comments are read by the server's own readers, and YAML with the scanner of `granit-parser`. The scripts of an HTML document are read by Oxc. The thread that handles requests waits while the comments of a document are read, so a reader must take time in proportion to the size of the text, whatever the text holds. The reading runs on a thread of its own with a stack of 256 MiB, because Oxc reads nesting by recursion and has no limit of its own on the depth ([oxc#21953](https://github.com/oxc-project/oxc/issues/21953)); only the part of the stack in use takes memory. Code nested more than about 100,000 levels deep still overflows it. The PowerShell and TOML readers replaced the lexers of `pwsh-parser` and `toml_parser`, which took time in proportion to the square of the size on some texts. The server's own readers read each part once, without starting again from an earlier place, and do not recurse. The shell, Dockerfile, Kotlin, Swift, Ruby, and PHP readers keep what is open on a stack of their own, so only memory limits the depth of nesting. The Ruby reader looks up whether a word is a local variable in one set, whatever the depth of the scopes. The tests `readers_take_time_in_proportion_to_the_text` and `readers_take_time_in_proportion_to_texts_of_several_parts` check the time of all fifteen readers on repeated pieces of text; add a new reader to both. They do not reach every text, so when changing a reader, check each place that reads back or ahead of the current character.

The current exception to compatibility ranges is `unicode-ident = "=1.0.24"`, pinned to keep the Rust lexer and Unicode tables on the same Unicode version. Remove the pin and update them together when the lexer and related Unicode tables support the same version.

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
