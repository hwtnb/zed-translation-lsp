# Anthropic API

[English](../../providers/anthropic.md)

`anthropic` プロバイダーを選ぶと、Anthropic APIを通じてClaudeで翻訳します。APIキーは言語サーバーの環境変数で渡します。

## 最小の設定

```json
{
  "lsp": {
    "translation-lsp": {
      "binary": {
        "path": "REPLACE_WITH_ABSOLUTE_SERVER_PATH",
        "env": { "TRANSLATION_LSP_ANTHROPIC_API_KEY": "sk-ant-…" }
      },
      "initialization_options": {
        "provider": { "name": "anthropic", "model": "claude-haiku-5-5", "effort": "low" }
      }
    }
  }
}
```

[設定](../configuration.md)で説明するほかの `initialization_options` は、同じように使えます。

## 設定項目

| 設定 | 既定値 | 規則 |
| --- | --- | --- |
| `provider.name` | なし | `"anthropic"` でこのプロバイダーを選びます。 |
| `provider.model` | `claude-haiku-5-5` | 省略できます。空の値、空白や制御文字を含む値、256バイトを超える値は設定エラーです。指定した値をそのまま送り、別のモデルへ置き換えません。 |
| `provider.effort` | `low` | 省略できます。`low`、`medium`、`high`、`xhigh`、`max` のいずれかです。ChatGPT用の `none` と `minimal` は設定エラーです。 |
| `provider.thinking` | `model` と `effort` で決まる | 省略できます。省略した場合、`model` が `claude-haiku-5-5` で `effort` が `low`、`medium`、`high` のいずれかなら `false`、それ以外は `true` です。明示した `false` と、`effort` の `xhigh` または `max` の組み合わせは設定エラーです。APIがこの組み合わせを拒否するためです。 |
| `provider.document` | なし | 省略できます。タブに開く翻訳で使うモデルと設定を入れるオブジェクトです。中の値は `provider.model`、`provider.effort`、`provider.thinking` を引き継ぎません。中で省略した値は、下の行の既定値になります。中の未知の項目は設定エラーです。 |
| `provider.document.model` | なし | `provider.document` を書く場合は必須です。規則は `provider.model` と同じです。 |
| `provider.document.effort` | `low` | 省略できます。選べる値は `provider.effort` と同じです。 |
| `provider.document.thinking` | `provider.document.model` と `provider.document.effort` で決まる | 省略できます。`provider.thinking` と同じ規則を、`provider.document` の中の値に当てはめます。 |

`effort` は、モデルが応答にかける作業量を決めます。`low` が最も軽く、費用も最も小さくなります。要求には必ず `effort` を含めるため、`model` は `effort` に対応している必要があります。APIの資料によると、Claude Haiku 4.5などの古いモデルは対応していません。

`thinking` が `false` のとき、モデルは推論の段階を挟まずに答えます。`true` にすると、答える前に推論するかどうかをモデルが判断します。推論の内容は表示しません。推論に使ったトークンは、費用と出力トークンの上限に含まれます。モデルによっては、推論なしの要求を受け付けません。別のモデルを使い、`thinking` に明示的に `false` を指定すると、APIがHTTP 400を返すことがあります。その場合は `thinking` を省略するか、`true` にしてください。省略時の値は、`model` が `claude-haiku-5-5` と完全に一致するかどうかで決めます。

未知の項目は拒否します。APIキーや接続先を指定する設定項目はありません。設定を変更したら言語サーバーを再起動してください。

### タブ翻訳だけ別のモデルを使う

```json
"provider": {
  "name": "anthropic",
  "model": "claude-haiku-5-5",
  "effort": "low",
  "document": { "model": "claude-sonnet-5-5" }
}
```

`provider.document` の中に書ける値と、省略したときの既定値は、上の表にあります。

`provider.document` を設定したときにどのプロバイダーでも変わる点は、[タブ翻訳だけ別のモデルを使う](../configuration.md#タブ翻訳だけ別のモデルを使う)で説明します。

## 認証

サーバーは、自分のプロセスの環境変数 `TRANSLATION_LSP_ANTHROPIC_API_KEY` だけから、起動時に一度だけキーを読みます。上の例のように `lsp.translation-lsp.binary.env` に書き、言語サーバーを再起動してください。

サーバーは `ANTHROPIC_API_KEY` をあえて読みません。Claude Codeはこの名前の環境変数をサブスクリプションのログインより優先するため、シェル全体に設定すると、ふだんのClaude Codeの利用がAPIの課金に切り替わるからです。

キーがなくてもサーバーは起動し、翻訳を依頼すると、APIへ接続せずに環境変数名を示す設定エラーを返します。

## 費用と使用量

APIの利用は、キーを発行したClaude Consoleの組織に課金されます。Claudeのサブスクリプションに含まれる利用枠とは別です。`provider.document` を設定すると、タブ翻訳にはそこで指定したモデルの料金がかかります。

コメントのホバーとMarkdownの選択による自動翻訳は、[使い方](../usage.md)のとおり、確認なしに要求を送ります。

このプロバイダーには、アカウント用のメニューや使用量を開くコマンドはありません。

## 送信する内容

翻訳のために送る文章は、[送信する内容](../usage.md#送信する内容)で説明します。

1回の要求には、翻訳の指示文と、翻訳する文章を入れたユーザーメッセージ1件だけを含めます。会話履歴は送りません。出力は最大64,000トークンを要求し、`translation_limits.output_kib` がそれより小さい場合はその値に合わせます。この上限に達した訳文は、途中までの内容を表示せずにエラーとして報告します。

## エラーと停止

請求のエラー、拒否されたキー、存在しないモデル、権限のエラーでは、自動翻訳を停止し、HTTPの状態コード、APIのエラー種別、要求IDを報告します。Claude Consoleで請求の問題を解消するか、`provider.model` または `provider.document.model` を直したあと、選択範囲または文書の翻訳を明示的に実行してください。それが成功すると自動翻訳を再開します。キーや設定を変更した場合は、言語サーバーの再起動が必要です。

レート制限と過負荷は、[自動の再試行](../behavior.md#自動の再試行)のとおり一度だけ再試行します。モデルが文章の翻訳を断った場合は、報告された分類を添えて失敗とし、再試行せず、自動翻訳も停止しません。

どのプロバイダーにも共通する規則は、[停止](../behavior.md#停止)と[自動の再試行](../behavior.md#自動の再試行)で説明します。

## 保存するデータ

キーは、[認証](#認証)のとおり `lsp.translation-lsp.binary.env` に書きます。この方法では、キーがZedの設定ファイルに平文で入ります。設定ファイルを共有リポジトリや、同期しているドットファイルに含めないでください。サーバーがキーをログ、エラー文、ディスクに書くことはありません。
