# 開発

[English](../development.md)

このページでは、必要なRustの版、実行する検証、CI、依存関係の方針を説明します。

## 検証

両クレートはRust Edition 2024です。ネイティブ言語サーバーにはRust 1.97以降、WASM拡張にはRust 1.96以降が必要です。古い最低対応Rustの維持より依存更新を優先し、必要に応じて対象クレートの要件を引き上げます。仮想ワークスペースには依存関係リゾルバーのバージョン3を使います。`.gitattributes` は、3つのOSのどれでもテキストファイルを改行LFでチェックアウトします。stdioテストはサンプルファイルを埋め込んでおり、この改行に依存します。`Cargo.lock` をバージョン管理し、検証とリリースビルドは `--locked` で実行してください。

```sh
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## CIとリリース前の確認

[CIワークフロー](../../.github/workflows/ci.yml)はWindows・macOS・Linuxそれぞれでワークスペース全体のテスト、fmt、Clippy、ネイティブとWASMのリリースビルドを実行します。1 OSでの成功だけを他OSの成功とは扱いません。リリース前には3 OSのCI結果に加え、各OSのZedで拡張導入・ネイティブ起動・認証・コメント翻訳・文書翻訳の表示と保存を確認してください。HTTP/OIDCのテスト用環境は合成の認証情報を使うため、ChatGPTアカウントもAnthropicのAPIキーも不要です。テストは実際のAPIを呼びません。stdioテストは、サーバーのプロセスから `TRANSLATION_LSP_ANTHROPIC_API_KEY` を取り除きます。

## 依存関係

依存関係のバージョンは `major.minor` のCargo互換範囲で指定します。特定のバージョンに固定する場合は、互換性上の理由を記録してください。更新時は `cargo update` によるロックファイルの差分を確認し、上記の検証とネイティブ・WASM両方のリリースビルドを実行してください。Oxcの関連クレートは同じリリース系列に揃えます。`zed_extension_api` の更新時は、対象ZedのAPI対応も確認してください。

新しいコード言語のコメント抽出には、Tree-sitterの文法を第一候補として検証します。JSON、TOML、YAMLではコメントが字句だけで決まるため、文法ではなく字句の読み取りを使います。既存のRust用字句解析器とJavaScript／TypeScript用Oxcは継続して使用します。

JSON、TOML、シェルスクリプト、PowerShell、Dockerfileのコメントは、サーバー自身の読み取りで読みます。YAMLは `granit-parser` のスキャナーで読みます。読み取りは要求を処理するスレッドで動くため、内容にかかわらず、かかる時間がテキストの大きさに比例する必要があります。サーバー自身の読み取りは、各部分を1回だけ読み、前の位置から読み直さず、再帰も使いません。シェルとDockerfileの読み取りは、開いているものを自前のスタックに積むため、入れ子の深さを制限するのはメモリーだけです。テスト `readers_take_time_in_proportion_to_the_text` と `readers_take_time_in_proportion_to_texts_of_several_parts` が、短い文字列を繰り返したテキストで、YAMLを含む6つの読み取りの時間を確かめます。読み取りを追加するときは、両方のテストに加えてください。これらのテストがすべてのテキストに届くわけではないので、読み取りを変えるときは、現在の文字より前や先を読む箇所を一つずつ確かめてください。

現在のバージョン固定の例外は `unicode-ident = "=1.0.24"` です。Rustの字句解析器とUnicodeテーブルの版を揃えるために固定しています。字句解析器と関連するUnicodeテーブルが同じ版に対応した時点で、固定を外してまとめて更新してください。

`granit-parser` 1.3 は、引用符付きの複数行のYAMLの値で、継続行がキーより1桁だけ深い正しい書き方をエラーにします。そのため、その値から後ろのコメントを抽出できません。上流では修正済みで、2.0で公開される見込みです。2.0へ更新するときは、`comments` の機能フラグを有効にし、テスト `yaml_reads_past_a_value_continued_one_column_deeper` の `ignore` を外し、`docs/languages.md` と `docs/ja/languages.md` からこの制限の文を消してください。
