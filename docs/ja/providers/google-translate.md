# Google Cloud Translation

[English](../../providers/google-translate.md)

`google_translate` プロバイダーを選ぶと、APIキーを使い、Google Cloud Translationを通じて翻訳します。APIキーは、コードアクションの `Google Cloud API key…` から保存します。このプロバイダーは、APIキーを使えるBasic版のAPI（v2）を使います。Advanced版（v3）と、そのTranslation LLMのモデル、用語集、カスタムモデルは、別の認証情報が要るため対応していません。

Google Cloud Translationは翻訳サービスで、言語モデルではないため、指示文を受け付けません。そのため、[送信する内容](#送信する内容)のとおり、送る文章はサーバーが整えます。また、選択した単語は、辞書の項目として引かずに翻訳します。Geminiで翻訳する場合は、[OpenAI互換](openai-compatible.md)のプロバイダーを使ってください。

## 最小の設定

```json
{
  "lsp": {
    "translation-lsp": {
      "initialization_options": {
        "target_language": "ja",
        "provider": {
          "name": "google_translate"
        }
      }
    }
  }
}
```

`initialization_options` のほかの設定は、[設定](../configuration.md)のとおりに働きます。APIキーは設定に含めません。[認証](#認証)のとおりに保存してください。

## 設定項目

| 設定 | 既定値 | 規則 |
| --- | --- | --- |
| `provider.name` | なし | `"google_translate"` でこのプロバイダーを選びます。 |

このプロバイダーには、ほかの項目はありません。`model` や `document` など、知らない項目は設定エラーです。

このプロバイダーでは、`target_language` は `ja`、`pt`、`zh-CN` のような言語コードにする必要があります。`source_language` は `auto` か言語コードです。`Japanese` のような言語名は設定エラーです。コードは、`_` を `-` にしたうえで、書いたとおりに送ります。`auto` の場合は、APIが言語を判別します。どのコードを受け付けるかはGoogleが決めます。受け付けないコードは、APIのメッセージを添えたエラーになります。コードの一覧は、Googleの[対応言語](https://docs.cloud.google.com/translate/docs/languages)のページにあります。

設定を変更したら、言語サーバーを再起動してください。

## 認証

対応する文書でZedのコマンドパレットから `editor: toggle code actions` を実行し、`Google Cloud API key…` から `Save Google Cloud API key` を実行します。OSの端末のウィンドウが開き、キーを尋ねます。そこにキーを貼り付けてEnterを押してください。貼り付けた内容は端末に表示されません。保存した旨が表示されたら、Enterを押してZedに戻ります。次の翻訳からそのキーを使い、言語サーバーの再起動は要りません。キーを入れずにEnterを押すと、何も保存しません。

キーは、Google Cloudコンソールの[認証情報のページ](https://console.cloud.google.com/apis/credentials)で、Cloud Translation APIを有効にしたプロジェクトに作ってください。Gemini APIのキーは別のアドレス用に保存され、このプロバイダーでは使いません。

使えるキーが保存されていない間、サーバーのコードアクションは `Google Cloud API key…` だけです。保存されている間、メニューは次の項目に変わります。

- `Replace Google Cloud API key` は、同じ端末を開き、保存済みのキーを別のキーに置き換えます。
- `Remove Google Cloud API key` は、確認のあと、お使いのコンピューターからキーのファイルを削除し、進行中の翻訳を取り消し、表示済みの訳文を破棄します。キーそのものは、Google Cloudコンソールで削除するまで有効です。

キーが保存されている間、メニューはMarkdownとPlain Textの文書に出ます。ほかの文書では、キーが拒否されたときなど、翻訳に失敗した範囲にだけ出ます。それ以外のときにキーを置き換えるか削除するには、MarkdownかPlain Textのファイルを開いてください。

端末の開き方とキーの保存の仕方は、Anthropic APIのキーと同じで、そのページの[認証](anthropic.md#認証)で説明します。`Save Google Cloud API key` または `Replace Google Cloud API key` のあとに出る通知には、ウィンドウが開かないときに自分で実行するコマンドが入っています。言語サーバーのパスに `api-key https://translation.googleapis.com` を続けたものです。

キーは、サーバーの認証情報のディレクトリにあるファイル `api-key-https-translation.googleapis.com-443` に保存します。ディレクトリとファイルの検査は、Anthropicのキーのファイルと同じです。

| OS | キーのファイル |
| --- | --- |
| Windows | `%LOCALAPPDATA%\translation-lsp\api-key-https-translation.googleapis.com-443` |
| macOS | `~/Library/Application Support/translation-lsp/api-key-https-translation.googleapis.com-443` |
| Linux | `$XDG_CONFIG_HOME/translation-lsp/api-key-https-translation.googleapis.com-443`。未設定の場合は `~/.config/translation-lsp/api-key-https-translation.googleapis.com-443` |

キーは、端末からファイルへ渡ります。Zedの設定には書かず、環境変数も、Zedとサーバーの間のメッセージも通りません。

使えるキーがなくてもサーバーは起動し、翻訳を依頼すると、APIへ接続せずに設定エラーを返します。

## 費用と使用量

Googleは、翻訳した文章の文字数に応じて、キーのプロジェクトの請求先アカウントに課金します。料金と無料の利用枠は、GoogleのCloud Translationの料金のページにあります。タブに開く翻訳も、ほかの翻訳と同じように送り、課金されます。

コメントのホバーとMarkdownの選択による自動翻訳は、[使い方](../usage.md)のとおり、確認なしに要求を送ります。

`Google Cloud API key…` のメニューは、キーの保存、置き換え、削除を行います。使用量を開くコマンドはありません。使用量と割り当ては、Google Cloudコンソールで確認できます。

## 送信する内容

翻訳のために送る文章は、[送信する内容](../usage.md#送信する内容)で説明します。APIは指示文を受け付けないため、サーバーはDeepL APIと同じように文章を区切って送ります。段落の各行はつなぎ、見出し、箇条書きの項目、引用、表のセルは別々に送り、コードブロックは送らず、コードとURLには訳さないよう印を付けます。規則は、DeepL APIのページの[送信する内容](deepl.md#送信する内容)で説明します。

このプロバイダーでは、区切った文章をHTMLとして送り、コードとURLを `<span translate="no" class="notranslate">` で囲みます。サーバーは、それぞれの印の位置に元の文字列を戻します。訳した段落は、元が何行でも1行になります。

要求は `https://translation.googleapis.com/language/translate/v2` に、`format` を `html` にして送ります。キーはアドレスに含めず、`X-goog-api-key` ヘッダーで送ります。1回の要求に入れるのは、区切った文章128件まで、合わせて48 KiBまでです。APIは、それぞれを互いに独立して訳します。それより長い文章は、何回かの要求に分けて順に送り、すべての要求が成功したときにだけ表示します。APIが印を失ったり、重ねたり、加えたりした訳文は、表示せずにエラーとして報告します。

`source_language` が `auto` のとき、APIは区切りごとに検出した原文の言語を `detectedSourceLanguage` で返します。コメントのホバーとMarkdownの自動の選択では、すべての区切りが翻訳先の言語のとき、サーバーは何も表示しません。言語のコードは地域を除いて比べるので、`en` と `en-US` は同じ言語です。中国語のコードは全体で比べます。APIは簡体字の中国語を `zh-CN` か `zh`、繁体字の中国語を `zh-TW` で受け付けるためです。`zh` と `zh-CN` は同じ言語で、`zh-TW` は違う言語です。`source_language` に言語を指定したときは、APIは原文の言語を返しません。

## エラーと停止

拒否されたキー、キーのプロジェクトでの拒否（HTTP 403。Cloud Translation APIや課金が有効でない場合、キーがこのAPIを呼べない場合など）、使い切った1日の割り当て、APIが受け付けない言語コード（HTTP 400）では、自動翻訳を停止し、HTTPの状態コード、APIが示す理由、メッセージを報告します。別のキーを保存するか、プロジェクトや設定を直したあと、選択範囲または文書の翻訳を明示的に実行してください。それが成功すると自動翻訳を再開します。拒否されたキーによる停止は、`Google Cloud API key…` から別のキーを保存すると終わります。停止は、それが起きたキーに限られるからです。

キーの保存、置き換え、削除に、言語サーバーの再起動は要りません。あるキーで得た訳文と失敗の記録は、別のキーでは表示しません。キーが替わったときに進行中だった翻訳は、完了しても破棄します。設定を変更した場合は、言語サーバーの再起動が必要です。

レート制限とサーバーのエラーは、[自動の再試行](../behavior.md#自動の再試行)のとおり一度だけ再試行します。印を失った訳文は再試行せず、自動翻訳も停止しません。

どのプロバイダーにも共通する規則は、[停止](../behavior.md#停止)と[自動の再試行](../behavior.md#自動の再試行)で説明します。

## 保存するデータ

キーは、[認証](#認証)で説明したファイル `api-key-https-translation.googleapis.com-443` に、平文で入ります。ChatGPTプロバイダーが認証情報を保存するのと同じディレクトリです。ファイルを書くのは、メニューが端末で動かす `api-key` コマンドです。ディレクトリがなければ、このコマンドが作ります。`Remove Google Cloud API key` は、このファイルだけを削除します。それ以外では、言語サーバーはファイルを読むだけです。

macOSで「ターミナル」を開くとき、サーバーは一時ディレクトリにスクリプトを書きます。スクリプトに入るのは言語サーバーのパスとアドレス `https://translation.googleapis.com` だけで、キーは入りません。「ターミナル」が実行すると、スクリプトは自分を削除します。

サーバーがキーをログ、エラー文、通知、ディスクのほかの場所に書くことはありません。
