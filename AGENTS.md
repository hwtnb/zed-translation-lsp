# Repository guidance

These instructions apply to the entire repository. Paths and commands below are
relative to the repository root.

## Project layout

- `crates/zed-extension`: Zed's WASM extension bridge and `extension.toml` manifest.
  Select this directory when using **Install Dev Extension**.
- `crates/translation-lsp`: native language server. `src/config.rs` owns settings;
  `src/lsp/` owns requests, scheduling, previews, and activity;
  `src/providers/` owns translation providers and authentication;
  `src/text/` owns comment extraction and Markdown protection.
- `crates/translation-lsp/tests/stdio.rs`: integration tests for the LSP protocol.
- `README.md`: canonical English usage and development documentation.
  `README.ja.md` provides the Japanese version. Consult the relevant section for
  settings, installation, supported behavior, or dependency compatibility.

## Engineering conventions

- Use Rust Edition 2024 and workspace resolver 3. Preserve the minimum supported
  Rust version declared in the manifests; verify compatibility when changing
  dependencies or using newer language features.
- Use named module files with matching subdirectories, such as `providers.rs`
  and `providers/chatgpt.rs`. Do not introduce `mod.rs` files.
- Keep providers replaceable through `TranslationProvider` and generic static
  dispatch. Put provider-specific behavior under `src/providers/`.
- Follow `.editorconfig` and rustfmt. Keep UI labels in English independently of
  the translation target language. Update both READMEs for user-facing changes.
- Use `major.minor` Cargo compatibility ranges and track `Cargo.lock`. Any exact
  pin needs a documented compatibility reason. Keep Oxc crates on the same release
  line; consult the README's Unicode compatibility note before lexer updates.
- When updating `zed_extension_api`, verify the intended Zed host supports that
  API version as well as checking the WASM build.

## Behavior to preserve

- Translation previews leave the source document unchanged. Send the selected
  text or comment body as prose and display the completed translation directly.
  Prompt the model to preserve code, identifiers, URLs, and meaning. Do not require
  JSON translation segments or enforce Markdown structure or byte equality on the
  generated translation. Markdown parsing only determines input eligibility.
- Never publish partial, canceled, timed-out, or stale translations. Changes to
  documents or authentication context must invalidate affected cached results.
  Coalesced requests retain independent cancellation and deadlines, including
  queue waiting time.
- Keep blocking network work off the LSP request loop. Preserve bounded
  concurrency and credential-store locking and revision checks.
- Keep stdout exclusively for LSP messages. Do not log credentials or document
  contents, and do not persist source text or translations.
- HTTP/OIDC tests use local fixtures and synthetic credentials. Generate test
  signing keys in memory rather than committing real or fixed private keys.

## Verification

For Rust changes, run from the repository root:

```sh
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Use `--workspace`: the default workspace member is only the WASM extension, so
plain `cargo test` does not cover the language server.

Build the affected deliverable; build both after dependency updates:

```sh
cargo build -p zed-code-translation-lsp --release --locked
cargo build -p zed-code-translation --target wasm32-wasip2 --release --locked
```

The WASM build requires the `wasm32-wasip2` Rust target. For documentation-only
changes, check referenced paths and commands and run `git diff --check`.

Verify changed behavior with focused tests. For UI changes, inspect the actual
Zed interaction when available. Report unit tests, stdio tests, WASM builds,
Zed UI checks, and live-provider checks separately; do not claim one proves the
others. State checks that could not be run.

## Repository hygiene

- Preserve unrelated working-tree and staged changes.
- Do not add temporary plans, handoffs, investigation logs, credentials, or
  machine-specific configuration to the repository. Keep shared instructions
  usable from a checkout without private paths or untracked documents.
- Before committing, review the complete staged diff for secrets, local paths,
  generated artifacts, and unintended changes. Report and resolve questionable
  content with the user rather than assuming it is safe.
- Use Conventional Commits with a concrete description, following existing Git
  history. Group a coherent change and its fixes into one commit rather than
  splitting incidental corrections into separate commits.
