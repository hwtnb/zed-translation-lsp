# Translation LSP

[English](README.md)

ZedでRust、JavaScript／JSX、TypeScript／TSX、C#、C、C++、Python、JSON、JSONC、YAML、TOML、シェルスクリプト、PowerShell、Dockerfile、Docker Composeのコメント、Pythonのドキュメンテーション文字列、選択したMarkdownなどの文章を翻訳します。訳文はホバー表示（テキスト上で開く情報表示）で読めます。原文はエディタに残ります。コードのコメントにマウスポインターを重ねると、初回から翻訳が始まります。ChatGPTまたはAnthropicが利用可能な状態では、Markdownの文章を選択して安定させるとバックグラウンドで翻訳し、その範囲のホバーで訳文を読めます。

ChatGPTを使う場合は、本人のChatGPTアカウントで接続し、ChatGPTプランの利用を許可します。Anthropicを使う場合は、本人のAPIキーでAnthropic APIを呼びます。プロバイダーを選ぶまでは、文章を外部へ送りません。

## 導入

Translation LSPは、開発用拡張として導入します。Zedがチェックアウトから拡張をビルドし、拡張が言語サーバーを取得します。

1. Zedと、[rustup経由のRust](https://www.rust-lang.org/tools/install)を導入します。Zedは、開発用拡張のビルドにRustを使います。
2. このリポジトリをクローンし、[公開済みリリース](https://github.com/hwtnb/zed-translation-lsp/releases)のタグをチェックアウトします。
3. Zedの拡張画面で `Install Dev Extension` を選び、チェックアウト内の **`crates/zed-extension`** を指定します。[Zedの開発用拡張の導入手順](https://zed.dev/docs/extensions/developing-extensions)も参照してください。

言語サーバーを最初に起動するとき、拡張は自分と同じバージョンのサーバーをそのリリースから取得し、以後の起動では保存したサーバーを使います。リリースのないバージョンのチェックアウトでは、取得できるサーバーがありません。サーバーを配布している環境は次のとおりです。

| OS | バージョン | CPU |
| --- | --- | --- |
| macOS | macOS 12以降 | AppleシリコンとIntel |
| Linux | Ubuntu 22.04以降、Debian 12以降、そのほかglibc 2.35以降のディストリビューション | ARM64とx86-64 |
| Windows | x86-64はWindows 10以降、ARM64はWindows 11 | ARM64とx86-64 |

サーバーは、Authenticodeの署名もAppleの公証も受けていません。取得したサーバーをOSが起動しない場合や、お使いの環境向けのサーバーが配布されていない場合は、サーバーをソースからビルドしてください。

### サーバーをソースからビルドする

[rustup経由のRust](https://www.rust-lang.org/tools/install)（1.97以降）と、ネイティブ依存のビルドに使うC/C++の開発環境を導入します。

| OS | ビルドの前提 | ネイティブ実行ファイル |
| --- | --- | --- |
| Windows | MSVC版Rust、Visual Studio Build Toolsの「C++によるデスクトップ開発」とWindows SDK。x86-64ではNASM、ARM64ではclang-clも導入し、Visual Studioの開発者用PowerShellまたはコマンドプロンプトを使います。 | `target/release/translation-lsp.exe` |
| macOS | Xcode Command Line Tools（`xcode-select --install`）。 | `target/release/translation-lsp` |
| Linux | GCCまたはClangとC/C++開発ツール。Debian/Ubuntuでは `build-essential` と `ca-certificates` を導入します。 | `target/release/translation-lsp` |

Tree-sitterとTLSプロバイダーはネイティブコードをビルドします。別のツールチェーン構成はTLSプロバイダーの[Windows向け要件](https://aws.github.io/aws-lc-rs/requirements/windows.html)と[Linux向け要件](https://aws.github.io/aws-lc-rs/requirements/linux.html)を参照してください。

リポジトリのルートで次のコマンドを実行します。PowerShell・コマンドプロンプト・Bash・Zshで共通です。

```sh
cargo build -p translation-lsp --release --locked
```

ビルドした実行ファイルの絶対パスを、[サーバーのパス](docs/ja/configuration.md#サーバーのパス)の説明に沿って `lsp.translation-lsp.binary.path` に設定します。拡張はその実行ファイルを起動し、サーバーを取得しません。

C#、C、C++、Python、JSON、JSONC、YAML、TOML、Shell Script、PowerShell、Dockerfile、Docker Composeの言語サーバーを明示している場合は、[言語サーバーの設定](docs/ja/languages.md#言語サーバーの設定)を参照してください。

## プロバイダーの設定

既存のZed設定の `lsp` に次のどちらかの項目を追加してプロバイダーを選び、Zedのコードアクションから接続します。設定を変更したら言語サーバーを再起動してください。すべての設定は[設定](docs/ja/configuration.md)で説明します。

### ChatGPT

```json
{
  "lsp": {
    "translation-lsp": {
      "initialization_options": {
        "provider": { "name": "chatgpt", "model": "gpt-6-luna", "effort": "low" }
      }
    }
  }
}
```

対応する文書でZedのコマンドパレットから `editor: toggle code actions` を実行し、`ChatGPT account…` から `Continue with ChatGPT / Choose account` を実行して、ブラウザで認証とプラン利用の許可を完了します。ほかの設定とアカウント管理メニューは[ChatGPT](docs/ja/providers/chatgpt.md)で説明します。

### Anthropic API

```json
{
  "lsp": {
    "translation-lsp": {
      "initialization_options": {
        "provider": { "name": "anthropic", "model": "claude-haiku-5-5", "effort": "low" }
      }
    }
  }
}
```

対応する文書でZedのコマンドパレットから `editor: toggle code actions` を実行し、`Anthropic API key…` から `Save Anthropic API key` を実行します。開いた端末のウィンドウにキーを貼り付けて、Enterを押してください。キーの作り方、ほかの設定、費用は[Anthropic API](docs/ja/providers/anthropic.md)で説明します。

## 基本的な使い方

| 操作 | 方法 |
| --- | --- |
| コメントのホバー | 対応するコードのコメントにマウスポインターを重ねるか、カーソルを置いてZedの `editor: hover` を実行します。 |
| Markdownの選択 | ChatGPTが利用可能であるか、Anthropicプロバイダーを選んでいて、`features.automatic_selection` が `true` の場合、Markdownの文章を選択して安定させるとバックグラウンドで翻訳します。その範囲でホバーを開いて訳文を読みます。 |
| `Translate selection to …` | すぐに実行したい場合やバックグラウンド翻訳が失敗した場合は、コードアクションの `Translate selection to …` を使います。明示実行では完了通知 `Translation ready. Hover over the selected text to view it.` が出ます。 |
| `… and open in tab` | Markdownまたはプレーンテキスト（Zedの言語名は `Plain Text`）では、`Translate selection to … and open in tab` で訳文を別タブに開けます。選択がない場合は `Translate document to … and open in tab` となり、文書全体を翻訳します。 |
| 取り消し | 1件を止めるには、その範囲にカーソルか選択範囲を置き、コードアクションの `Cancel translation of lines …` を実行します。`Cancel all translations` はアカウントメニューを残してすべての翻訳を取り消します。 |

これらの操作の規則は[使い方](docs/ja/usage.md)で説明します。

## 送信する前に

- コメントの上を通りすぎるだけでも、Zedがホバー要求を送って受け付けられた場合は、翻訳の利用量を消費します。
- コメントに認証情報が書かれていれば、それも一緒に送信されます。
- `features.automatic_selection` が `true` の場合、Markdownの文章は、コピーのために選択しただけでも送信されることがあります。

1回の要求に含める内容は[送信する内容](docs/ja/usage.md#送信する内容)で説明します。費用は各プロバイダーのページで説明します。

## ドキュメント

- [設定](docs/ja/configuration.md): 設定の全体例、サーバーのパス、翻訳する言語、機能の切り替え、タイミング、サイズの上限、受付とキャッシュの上限、モデルの変更。
- [ChatGPT](docs/ja/providers/chatgpt.md): ChatGPTプロバイダーの設定と、アカウントへの接続。
- [Anthropic API](docs/ja/providers/anthropic.md): Anthropicプロバイダーの設定と、APIキー。
- [使い方](docs/ja/usage.md): ホバー、選択、タブに開く翻訳、状態表示、取り消しと再試行、送信する内容。
- [動作](docs/ja/behavior.md): 受付、処理が続く条件、自動選択、キャッシュ、期限、再試行、停止。
- [対応言語](docs/ja/languages.md): 対応する言語、コメントの抽出、言語サーバーの設定。
- [開発](docs/ja/development.md): 検証、モックのプロバイダーとサンプル、CI、依存関係の方針。

## ライセンス

[MIT](LICENSE)です。サーバーの各リリースアーカイブには、サーバーが依存するライブラリのライセンスと、そのソースの取得先が入っています。
