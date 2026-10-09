# 開発

[English](../development.md)

このページでは、必要なRustの版、実行する検証、CI、依存関係の方針を説明します。

## 検証

両クレートはRust Edition 2024です。ネイティブ言語サーバーにはRust 1.97以降、WASM拡張にはRust 1.96以降が必要です。古い最低対応Rustの維持より依存更新を優先し、必要に応じて対象クレートの要件を引き上げます。仮想ワークスペースには依存関係リゾルバーのバージョン3を使います。`Cargo.lock` をバージョン管理し、検証とリリースビルドは `--locked` で実行してください。

```sh
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## CIとリリース前の確認

[CIワークフロー](../../.github/workflows/ci.yml)はWindows・macOS・Linuxそれぞれでワークスペース全体のテスト、fmt、Clippy、ネイティブとWASMのリリースビルドを実行します。1 OSでの成功だけを他OSの成功とは扱いません。リリース前には3 OSのCI結果に加え、各OSのZedで拡張導入・ネイティブ起動・認証・コメント翻訳・文書翻訳の表示と保存を確認してください。HTTP/OIDCのテスト用環境は合成の認証情報を使うため、ChatGPTアカウントもAnthropicのAPIキーも不要です。テストは実際のAPIを呼びません。stdioテストは、サーバーのプロセスから `TRANSLATION_LSP_ANTHROPIC_API_KEY` を取り除きます。

## 依存関係

依存関係のバージョンは `major.minor` のCargo互換範囲で指定します。特定のバージョンに固定する場合は、互換性上の理由を記録してください。更新時は `cargo update` によるロックファイルの差分を確認し、上記の検証とネイティブ・WASM両方のリリースビルドを実行してください。Oxcの関連クレートは同じリリース系列に揃えます。`zed_extension_api` の更新時は、対象ZedのAPI対応も確認してください。

新しいコード言語のコメント抽出には、Tree-sitterの文法を第一候補として検証します。既存のRust用字句解析器とJavaScript／TypeScript用Oxcは継続して使用します。

現在のバージョン固定の例外は `unicode-ident = "=1.0.24"` です。Rustの字句解析器とUnicodeテーブルの版を揃えるために固定しています。字句解析器と関連するUnicodeテーブルが同じ版に対応した時点で、固定を外してまとめて更新してください。
