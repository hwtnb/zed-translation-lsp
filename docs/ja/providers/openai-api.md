# OpenAI API

[English](../../providers/openai-api.md)

`openai_api` プロバイダーを選ぶと、APIキーを使い、OpenAI APIを通じて翻訳します。APIキーは、コードアクションの `OpenAI API key…` から保存します。ChatGPTのプランを使う場合は、[ChatGPT](chatgpt.md)プロバイダーを選んでください。

## 最小の設定

```json
{
  "lsp": {
    "translation-lsp": {
      "initialization_options": {
        "provider": {
          "name": "openai_api",
          "model": "gpt-6-luna",
          "effort": "low"
        }
      }
    }
  }
}
```

`initialization_options` 内のほかの項目は、[設定](../configuration.md)で説明するとおりに使えます。APIキーは設定に書きません。[認証](#認証)のとおりに保存してください。

## 設定項目

| 設定 | 既定値 | 規則 |
| --- | --- | --- |
| `provider.name` | なし | `"openai_api"` でこのプロバイダーを選びます。 |
| `provider.model` | `gpt-6-luna` | 省略できます。空の値、空白や制御文字を含む値、256バイトを超える値は設定エラーです。指定した値をそのまま送り、別のモデルへ置き換えません。 |
| `provider.effort` | `low` | 省略できます。`none`、`minimal`、`low`、`medium`、`high`、`xhigh`、`max` のいずれかです。どの要求にも推論の作業量として含めます。 |
| `provider.speed` | なし | 省略できます。`auto`、`default`、`flex`、`scale`、`priority`、`fast`、`ultrafast` のいずれかで、要求の `service_tier` として送ります。省略した場合は `service_tier` を送りません。`null` と、Anthropic用の `standard` は設定エラーです。 |
| `provider.document` | なし | 省略できます。タブに開く翻訳で使うモデルと設定を入れるオブジェクトです。中の値は `provider.model`、`provider.effort`、`provider.speed` を引き継ぎません。中で省略した値は、下の行の既定値になります。中の未知の項目は設定エラーです。 |
| `provider.document.model` | なし | `provider.document` を書く場合は必須です。規則は `provider.model` と同じです。 |
| `provider.document.effort` | `low` | 省略できます。選べる値は `provider.effort` と同じです。 |
| `provider.document.speed` | なし | 省略できます。選べる値は `provider.speed` と同じです。 |

選べる値は、ChatGPTプロバイダーと同じです。モデルごとに受け付ける作業量は一部だけで、OpenAIの資料によると、`none` や `minimal` を拒否するモデルもあります。組織で使えないモデル、作業量、速度を指定すると、エラーになります。

未知の項目は拒否します。APIキーや接続先を指定する設定項目はありません。別の接続先へ送るには、[OpenAI互換API](openai-compatible.md)のプロバイダーを使ってください。設定を変更したら言語サーバーを再起動してください。

### タブ翻訳だけ別のモデルを使う

```json
"provider": {
  "name": "openai_api",
  "model": "gpt-6-luna",
  "effort": "low",
  "document": { "model": "gpt-6.1-sol", "effort": "medium" }
}
```

`provider.document` の中に書ける値と、省略したときの既定値は、上の表にあります。上のモデルと作業量は例です。

`provider.document` を設定したときにどのプロバイダーでも変わる点は、[タブ翻訳だけ別のモデルを使う](../configuration.md#タブ翻訳だけ別のモデルを使う)で説明します。

## 認証

対応する文書でZedのコマンドパレットから `editor: toggle code actions` を実行し、`OpenAI API key…` から `Save OpenAI API key` を実行します。OSの端末のウィンドウが開き、キーを尋ねます。そこにキーを貼り付けてEnterを押してください。貼り付けた内容は端末に表示されません。保存した旨が表示されたら、Enterを押してZedに戻ります。次の翻訳からそのキーを使い、言語サーバーの再起動は要りません。キーを入れずにEnterを押すと、何も保存しません。

キーは、OpenAI Platformの[APIキーのページ](https://platform.openai.com/api-keys)で作ってください。

使えるキーが保存されていない間、サーバーのコードアクションは `OpenAI API key…` だけです。保存されている間、メニューは次の項目に変わります。

- `Replace OpenAI API key` は、同じ端末を開き、保存済みのキーを別のキーに置き換えます。
- `Remove OpenAI API key` は、確認のあと、お使いのコンピューターからキーのファイルを削除し、進行中の翻訳を取り消し、表示済みの訳文を破棄します。キーそのものは、OpenAI Platformで削除するまで有効です。

キーが保存されている間、メニューはMarkdownとPlain Textの文書に出ます。ほかの文書では、キーが拒否されたときなど、翻訳に失敗した範囲にだけ出ます。それ以外のときにキーを置き換えるか削除するには、MarkdownかPlain Textのファイルを開いてください。

端末の開き方とキーの保存の仕方は、Anthropic APIのキーと同じで、そのページの[認証](anthropic.md#認証)で説明します。`Save OpenAI API key` または `Replace OpenAI API key` のあとに出る通知には、ウィンドウが開かないときに自分で実行するコマンドが入っています。言語サーバーのパスに `api-key https://api.openai.com` を続けたものです。

キーは、サーバーの認証情報のディレクトリにあるファイル `api-key-https-api.openai.com-443` に保存します。ディレクトリとファイルの検査は、Anthropicのキーのファイルと同じです。[OpenAI互換API](openai-compatible.md)のプロバイダーも、`base_url` が `https://api.openai.com` にある場合は、同じファイルを使います。

| OS | キーのファイル |
| --- | --- |
| Windows | `%LOCALAPPDATA%\translation-lsp\api-key-https-api.openai.com-443` |
| macOS | `~/Library/Application Support/translation-lsp/api-key-https-api.openai.com-443` |
| Linux | `$XDG_CONFIG_HOME/translation-lsp/api-key-https-api.openai.com-443`。未設定の場合は `~/.config/translation-lsp/api-key-https-api.openai.com-443` |

キーは、端末からファイルへ渡ります。Zedの設定には書かず、環境変数も、Zedとサーバーの間のメッセージも通りません。サーバーは `OPENAI_API_KEY` を読みません。

使えるキーがなくてもサーバーは起動し、翻訳を依頼すると、APIへ接続せずに設定エラーを返します。

## 費用と使用量

APIの利用は、キーを発行したOpenAI Platformの組織に課金されます。ChatGPTのプランに含まれる利用枠とは別です。`provider.document` を設定すると、タブ翻訳にはそこで指定したモデルの料金がかかります。

コメントのホバーとMarkdownの選択による自動翻訳は、[使い方](../usage.md)のとおり、確認なしに要求を送ります。

`OpenAI API key…` のメニューは、キーの保存、置き換え、削除を行います。使用量を開くコマンドはありません。使用量はOpenAI Platformで確認できます。

## 送信する内容

翻訳のために送る文章は、[送信する内容](../usage.md#送信する内容)で説明します。

要求は `https://api.openai.com/v1/responses` に、`store` を `false` にして送ります。`instructions` として翻訳の指示文を、ユーザーの入力1件として翻訳する文章を `<source_text>` タグで囲んで含めます。単語を辞書で引く場合は、指示文を辞書の指示文にし、入力には語を `<term>` タグで、語のある行を `<line>` タグで囲んで含めます。会話履歴は送りません。ホバーで読む翻訳では、出力を16,384トークンに原文（辞書で引く場合は語のある行）1バイトあたり4トークンを足した値までに抑えます。`translation_limits.output_kib` をバイト数に換算した値（1 KiBあたり1,024）がそれより小さい場合は、その値に合わせます。コメントの中の文章によって、モデルに長い出力を書かせないためです。推論のトークンもこの上限に含まれます。タブに開く翻訳はトークンの上限を送らず、`translation_limits.output_kib` で抑えます。上限に達した訳文は、途中までの内容を表示せずにエラーとして報告します。

## エラーと停止

拒否されたキー、使い切った利用枠、存在しないモデル、モデルが受け付けない `effort` や `speed` の値、権限のエラーでは、自動翻訳を停止し、HTTPの状態コード、エラーコード、要求IDを報告します。OpenAI Platformで請求の問題を解消するか設定を直したあと、選択範囲または文書の翻訳を明示的に実行してください。それが成功すると自動翻訳を再開します。拒否されたキーによる停止は、`OpenAI API key…` から別のキーを保存すると終わります。停止は、それが起きたキーに限られるからです。

キーの保存、置き換え、削除に、言語サーバーの再起動は要りません。あるキーで得た訳文と失敗の記録は、別のキーでは表示しません。キーが替わったときに進行中だった翻訳は、完了しても破棄します。設定を変更した場合は、言語サーバーの再起動が必要です。

レート制限とサーバーのエラーは、[自動の再試行](../behavior.md#自動の再試行)のとおり一度だけ再試行します。応答がコンテンツフィルターで止められた場合は、その分類を添えて失敗とし、再試行せず、自動翻訳も停止しません。

どのプロバイダーにも共通する規則は、[停止](../behavior.md#停止)と[自動の再試行](../behavior.md#自動の再試行)で説明します。

## 保存するデータ

キーは、[認証](#認証)で説明したファイル `api-key-https-api.openai.com-443` に、平文で入ります。ChatGPTプロバイダーが認証情報を保存するのと同じディレクトリです。ファイルを書くのは、メニューが端末で動かす `api-key` コマンドです。ディレクトリがなければ、このコマンドが作ります。`Remove OpenAI API key` は、このファイルだけを削除します。それ以外では、言語サーバーはファイルを読むだけです。

macOSで「ターミナル」を開くとき、サーバーは一時ディレクトリにスクリプトを書きます。スクリプトに入るのは言語サーバーのパスとアドレス `https://api.openai.com` だけで、キーは入りません。「ターミナル」が実行すると、スクリプトは自分を削除します。

サーバーがキーをログ、エラー文、通知、ディスクのほかの場所に書くことはありません。
