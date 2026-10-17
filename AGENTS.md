# Repository guidance

These instructions apply to the entire repository. Paths and commands below are
relative to the repository root.

## Project layout

- `crates/zed-extension`: Zed's WASM extension bridge and `extension.toml` manifest.
  Select this directory when using **Install Dev Extension**.
- `crates/translation-lsp`: native language server. `src/config.rs` owns settings;
  `src/lsp/` owns requests, scheduling, previews, and activity;
  `src/providers/` owns translation providers and authentication;
  `src/text/` owns comment extraction and Markdown input eligibility;
  `src/language.rs` names the languages that `extension.toml` registers. Run with
  `anthropic-key`, the same executable asks for the Anthropic API key in a
  terminal and saves it instead of serving an editor.
- `crates/translation-lsp/tests/stdio.rs`: integration tests for the LSP protocol.
- `crates/translation-lsp/tests/key_command.rs`: integration tests for the
  `anthropic-key` command on a pseudo-terminal, for macOS and Linux.
- `xtask`: development-only native packaging, license collection, and artifact checks.
- `README.md` and `docs/`: canonical English usage and development documentation.
  `README.md` is the entry point; `docs/` holds the details. `README.ja.md` and
  `docs/ja/` provide the Japanese version under the same file names. Consult the
  relevant page for settings, installation, supported behavior, or dependency
  compatibility.

## Engineering conventions

- Use Rust Edition 2024 and workspace resolver 3. Prioritize dependency updates
  over preserving an older minimum Rust version. Raise the affected manifest
  requirements and update the affected pages in both languages when dependencies
  require newer Rust; verify the resulting build compatibility.
- The WASM bridge also needs to build with the Zed registry's compiler (currently
  Rust 1.90). Keep that check in CI; reconsider registry compatibility explicitly
  when a bridge dependency requires a newer compiler. Do not declare
  `rust-version` for the bridge: Cargo would then select dependency versions
  that suit that compiler for the whole workspace, the native server included.
- Use named module files with matching subdirectories, such as `providers.rs`
  and `providers/chatgpt.rs`. Do not introduce `mod.rs` files.
- Do not write `unsafe` code. The workspace lint `unsafe_code` denies it in
  every crate. Where a task cannot be done without it, report that before
  writing it. An accepted exception is allowed at the item that needs it, with
  `#[allow(unsafe_code, reason = "…")]`, and each `unsafe` block states what it
  relies on. The only exception is the pseudo-terminal helper code in
  `crates/translation-lsp/tests/key_command.rs`.
- Keep providers replaceable through `TranslationProvider` and generic static
  dispatch. Put provider-specific behavior under `src/providers/`.
- Target Windows, macOS, and Linux. Document OS-specific prerequisites and paths;
  use OS-neutral editor action names in shared instructions. Keep all three
  platforms in CI, and distinguish build checks from actual Zed verification.
- Write a comment that says what an item is or does, such as a function, type,
  field, variant, or constant, as a doc comment (`///`, or `//!` for a module).
  Keep `//` for reasons about particular lines and for a comment that covers
  several items, such as the `cfg` variants of one item.
- Follow `.editorconfig` and rustfmt. Keep UI labels in English independently of
  the translation target language. Update the affected pages in both languages
  for user-facing changes.
- Use `major.minor` Cargo compatibility ranges and track `Cargo.lock`. Any exact
  pin needs a documented compatibility reason. Keep Oxc crates on the same release
  line; consult the Unicode compatibility note in `docs/development.md` before
  lexer updates.
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
  Hover waiters retain independent cancellation and deadlines, including queue
  waiting time. Accepted comment Hover work survives its response cancellation or
  expiry, shares the automatic admission limit with selection automation, and
  retains completed results and failure state for revisits. Accepted distinct
  ranges and explicit selection/tab translations
  survive selection changes and request cancellation. Only the immediately preceding
  overlapping automatic selection may be superseded by a stable adjustment; other
  owners keep shared work alive. Execution deadlines start at dispatch and never restart
  on joins; promotion uses the original start. Bound admission and preserve FIFO,
  except that range work may bypass a tab waiting for the tab concurrency limit.
  Release execution slots only on worker termination. Ship retained work together
  with range cancellation and per-range failure state.
- Keep blocking network work off the LSP request loop. Preserve bounded
  concurrency and credential-store locking and revision checks.
- Keep stdout exclusively for LSP messages. Do not log credentials or document
  contents. Hover translations stay in memory. Explicit document previews may
  write completed translations to temporary files for standard editor viewing;
  remove those files on server exit and preserve user-created Save As copies.
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
cargo build -p translation-lsp --release --locked
cargo build -p translation-lsp-extension --target wasm32-wasip2 --release --locked
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
