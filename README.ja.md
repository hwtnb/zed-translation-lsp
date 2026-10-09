# Translation LSP

[English](README.md)

ZedでRust、JavaScript／JSX、TypeScript／TSX、C#、C、C++、Pythonのコメント、Pythonのドキュメンテーション文字列、選択したMarkdownなどの文章を翻訳します。訳文はホバー表示（テキスト上で開く情報表示）で読めます。原文はエディタに残ります。コードのコメントにマウスポインターを重ねると、初回から翻訳が始まります。ChatGPTまたはAnthropicが利用可能な状態では、Markdownの文章を選択して安定させるとバックグラウンドで翻訳し、その範囲のホバーで訳文を読めます。

ChatGPTを使う場合は、本人のChatGPTアカウントで接続し、ChatGPTプランの利用を許可します。Anthropicを使う場合は、本人のAPIキーでAnthropic APIを呼びます。設定なしでは、同梱サンプルだけを日本語に訳す模擬モードで動作し、文章を外部へ送りません。

## 導入

開発・利用の対象は **Windows・macOS・Linux** です。Zedと、[rustup経由のRust](https://www.rust-lang.org/tools/install)（1.97以降）を導入してください。ネイティブ依存のビルドにはC/C++の開発環境も必要です。

| OS | ビルドの前提 | ネイティブ実行ファイル |
| --- | --- | --- |
| Windows | MSVC版Rust、Visual Studio Build Toolsの「C++によるデスクトップ開発」とWindows SDK。x86-64ではNASMも導入し、Visual Studioの開発者用PowerShellまたはコマンドプロンプトを使います。 | `target/release/translation-lsp.exe` |
| macOS | Xcode Command Line Tools（`xcode-select --install`）。 | `target/release/translation-lsp` |
| Linux | GCCまたはClangとC/C++開発ツール。Debian/Ubuntuでは `build-essential` と `ca-certificates` を導入します。 | `target/release/translation-lsp` |

Tree-sitterとTLSプロバイダーはネイティブコードをビルドします。別のツールチェーン構成はTLSプロバイダーの[Windows向け要件](https://aws.github.io/aws-lc-rs/requirements/windows.html)と[Linux向け要件](https://aws.github.io/aws-lc-rs/requirements/linux.html)を参照してください。

リポジトリのルートで次のコマンドを実行します。PowerShell・コマンドプロンプト・Bash・Zshで共通です。

```sh
cargo build -p translation-lsp --release --locked
rustup target add wasm32-wasip2
cargo build -p translation-lsp-extension --target wasm32-wasip2 --release --locked
```

Zedの拡張画面で `Install Dev Extension` を選び、リポジトリ内の **`crates/zed-extension`** を指定します。[Zedの開発用拡張の導入手順](https://zed.dev/docs/extensions/developing-extensions)も参照してください。

C#、C、C++、Pythonの言語サーバーを明示している場合は、[言語サーバーの設定](docs/ja/languages.md#言語サーバーの設定)を参照してください。

## 最小の設定

既存のZed設定の `lsp` に次の項目を追加し、`binary.path` をビルドした実行ファイルの絶対パスへ置き換えます。

```json
{
  "lsp": {
    "translation-lsp": {
      "binary": {
        "path": "REPLACE_WITH_ABSOLUTE_SERVER_PATH"
      },
      "initialization_options": {
        "provider": { "name": "mock" }
      }
    }
  }
}
```

この例は模擬モードを選びます。自分の文章を翻訳するには、`provider` を [ChatGPT](docs/ja/providers/chatgpt.md) または [Anthropic API](docs/ja/providers/anthropic.md) のページにある設定へ置き換えてください。設定を変更したら言語サーバーを再起動してください。すべての設定は[設定](docs/ja/configuration.md)で説明します。

## 模擬モード

`provider` を省略するか `{"name":"mock"}` にすると、接続せずに同梱サンプルを試せます。[samples/comment.rs](samples/comment.rs) のコメント、または [samples/comment.cs](samples/comment.cs) の通常コメント・XMLドキュメントコメントにマウスポインターを重ねてください。C/C++は [samples/comment.c](samples/comment.c) と [samples/comment.cpp](samples/comment.cpp) でDoxygenコメントも試せます。

Pythonのコメントとドキュメンテーション文字列は [samples/comment.py](samples/comment.py) で試せます。[samples/paragraph.md](samples/paragraph.md) では段落を選択し、コードアクションの `Translate selection to ja` を実行してから、選択範囲にマウスポインターを重ねます。模擬モードのMarkdownは手動実行が必要です。

模擬モードはこれらのサンプルと改行の違いだけに対応します。ほかの本文や日本語以外の訳先ではエラーを返します。ホバーには `Mock translation (fixed sample)` と表示します。

## 基本的な使い方

| 操作 | 方法 |
| --- | --- |
| コメントのホバー | 対応するコードのコメントにマウスポインターを重ねるか、カーソルを置いてZedの `editor: hover` を実行します。 |
| Markdownの選択 | ChatGPTが利用可能であるか、Anthropicプロバイダーを選んでいて、`markdown_auto_translate` が `true` の場合、Markdownの文章を選択して安定させるとバックグラウンドで翻訳します。その範囲でホバーを開いて訳文を読みます。 |
| `Translate selection to …` | すぐに実行したい場合やバックグラウンド翻訳が失敗した場合は、コードアクションの `Translate selection to …` を使います。明示実行では完了通知 `Translation ready. Hover over the selected text to view it.` が出ます。 |
| `… and open in tab` | Markdownまたはプレーンテキスト（Zedの言語名は `Plain Text`）では、`Translate selection to … and open in tab` で訳文を別タブに開けます。選択がない場合は `Translate document to … and open in tab` となり、文書全体を翻訳します。 |
| 取り消し | 1件を止めるには、その範囲にカーソルか選択範囲を置き、コードアクションの `Cancel translation of lines …` を実行します。`Cancel all translations` はアカウントメニューを残してすべての翻訳を取り消します。 |

これらの操作の規則は[使い方](docs/ja/usage.md)で説明します。

## 送信する前に

- コメントの上を通りすぎるだけでも、Zedがホバー要求を送って受け付けられた場合は、翻訳の利用量を消費します。
- `markdown_auto_translate` が `true` の場合、Markdownの文章は、コピーのために選択しただけでも送信されることがあります。
- 模擬モードでは、文章を外部へ送りません。

1回の要求に含める内容は[送信する内容](docs/ja/usage.md#送信する内容)で説明します。費用は各プロバイダーのページで説明します。

## ドキュメント

- [設定](docs/ja/configuration.md): 設定の全体例、サーバーのパス、翻訳する言語、タイミング、サイズの上限、受付とキャッシュの上限、モデルの変更。
- [ChatGPT](docs/ja/providers/chatgpt.md): ChatGPTプロバイダーの設定と、アカウントへの接続。
- [Anthropic API](docs/ja/providers/anthropic.md): Anthropicプロバイダーの設定と、APIキー。
- [使い方](docs/ja/usage.md): ホバー、選択、タブに開く翻訳、状態表示、取り消しと再試行、送信する内容。
- [動作](docs/ja/behavior.md): 受付、処理が続く条件、自動選択、キャッシュ、期限、再試行、停止。
- [対応言語](docs/ja/languages.md): 対応する言語、コメントの抽出、言語サーバーの設定。
- [開発](docs/ja/development.md): 検証、CI、依存関係の方針。
