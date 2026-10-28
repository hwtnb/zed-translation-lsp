# Anthropic API

[English](../../providers/anthropic.md)

`anthropic_api` プロバイダーを選ぶと、Anthropic APIを通じてClaudeで翻訳します。APIキーは、コードアクションの `Anthropic API key…` から保存します。

## 最小の設定

```json
{
  "lsp": {
    "translation-lsp": {
      "initialization_options": {
        "provider": {
          "name": "anthropic_api",
          "model": "claude-haiku-5-5",
          "effort": "low"
        }
      }
    }
  }
}
```

[設定](../configuration.md)で説明するほかの `initialization_options` は、同じように使えます。APIキーは設定に書きません。[認証](#認証)のとおりに保存してください。

## 設定項目

| 設定 | 既定値 | 規則 |
| --- | --- | --- |
| `provider.name` | なし | `"anthropic_api"` でこのプロバイダーを選びます。v0.2.0までの名前 `"anthropic"` でも選べます。 |
| `provider.model` | `claude-haiku-5-5` | 省略できます。空の値、空白や制御文字を含む値、256バイトを超える値は設定エラーです。指定した値をそのまま送り、別のモデルへ置き換えません。 |
| `provider.effort` | `low` | 省略できます。`low`、`medium`、`high`、`xhigh`、`max` のいずれかです。ChatGPT用の `none` と `minimal` は設定エラーです。 |
| `provider.thinking` | `model` と `effort` で決まる | 省略できます。省略した場合、`model` が `claude-haiku-5-5` で `effort` が `low`、`medium`、`high` のいずれかなら `false`、それ以外は `true` です。明示した `false` と、`effort` の `xhigh` または `max` の組み合わせは設定エラーです。APIがこの組み合わせを拒否するためです。 |
| `provider.speed` | なし | 省略できます。`standard` か `fast` で、要求の `speed` として、ヘッダー `anthropic-beta: fast-mode-2026-02-01` と一緒に送ります。`standard` でもヘッダーを付けます。省略した場合は、どちらも送りません。`null` と、`priority` などのChatGPT用の値は設定エラーです。 |
| `provider.document` | なし | 省略できます。タブに開く翻訳で使うモデルと設定を入れるオブジェクトです。中の値は `provider.model`、`provider.effort`、`provider.thinking`、`provider.speed` を引き継ぎません。中で省略した値は、下の行の既定値になります。中の未知の項目は設定エラーです。 |
| `provider.document.model` | なし | `provider.document` を書く場合は必須です。規則は `provider.model` と同じです。 |
| `provider.document.effort` | `low` | 省略できます。選べる値は `provider.effort` と同じです。 |
| `provider.document.thinking` | `provider.document.model` と `provider.document.effort` で決まる | 省略できます。`provider.thinking` と同じ規則を、`provider.document` の中の値に当てはめます。 |
| `provider.document.speed` | なし | 省略できます。選べる値は `provider.speed` と同じです。 |

`effort` は、モデルが応答にかける作業量を決めます。`low` が最も軽く、費用も最も小さくなります。要求には必ず `effort` を含めるため、`model` は `effort` に対応している必要があります。APIの資料によると、Claude Haiku 4.5などの古いモデルは対応していません。

`thinking` が `false` のとき、モデルは推論の段階を挟まずに答えます。`true` にすると、答える前に推論するかどうかをモデルが判断します。推論の内容は表示しません。推論に使ったトークンは、費用と出力トークンの上限に含まれます。モデルによっては、推論なしの要求を受け付けません。別のモデルを使い、`thinking` に明示的に `false` を指定すると、APIがHTTP 400を返すことがあります。その場合は `thinking` を省略するか、`true` にしてください。省略時の値は、`model` が `claude-haiku-5-5` と完全に一致するかどうかで決めます。

`speed` を `fast` にすると、モデルは出力を速く返し、料金は高くなります。[Fast mode](https://platform.claude.com/docs/en/build-with-claude/fast-mode)によると、research previewで、使えるのはClaude APIだけです。対象はClaude Opus 5.5、Claude Opus 5、Claude Opus 4.8で、営業担当かwaitlistから利用を申し込む必要があります。ほかのモデルでは、APIが `fast` にエラーを返すか、標準の速度で処理します。Fast modeの上限を超えるとHTTP 429か529になり、ほかのレート制限や過負荷と同じように扱います。

`speed` を書いた翻訳が完了すると、頼んだ値と、応答が報告した `usage.speed` を言語サーバーのログに書きます。例: `translation-lsp: Anthropic speed requested fast, served fast`。Zedでは、コマンドパレットから `dev: open language server logs` を実行すると読めます。応答が値を報告しなかったときは `(not reported)` と書きます。原文、訳文、キーは書きません。

未知の項目は拒否します。APIキーや接続先を指定する設定項目はありません。設定を変更したら言語サーバーを再起動してください。

### タブ翻訳だけ別のモデルを使う

```json
"provider": {
  "name": "anthropic_api",
  "model": "claude-haiku-5-5",
  "effort": "low",
  "document": { "model": "claude-sonnet-5-5" }
}
```

`provider.document` の中に書ける値と、省略したときの既定値は、上の表にあります。

`provider.document` を設定したときにどのプロバイダーでも変わる点は、[タブ翻訳だけ別のモデルを使う](../configuration.md#タブ翻訳だけ別のモデルを使う)で説明します。

## 認証

対応する文書でZedのコマンドパレットから `editor: toggle code actions` を実行し、`Anthropic API key…` から `Save Anthropic API key` を実行します。OSの端末のウィンドウが開き、キーを尋ねます。そこにキーを貼り付けてEnterを押してください。貼り付けた内容は端末に表示されません。保存した旨が表示されたら、Enterを押してZedに戻ります。次の翻訳からそのキーを使い、言語サーバーの再起動は要りません。キーを入れずにEnterを押すと、何も保存しません。

キーは、[Claude Console](https://platform.claude.com/settings/keys)で、1つのワークスペース用に作ってください。サーバーはワークスペースのIDを送らないため、ワークスペースを1つに絞っていないキーでの要求は、APIがすべて拒否します。キーには有効期限を付けられます。期限が切れたキーでの要求は、拒否されたキーとして失敗します。

使えるキーが保存されていない間、サーバーのコードアクションは `Anthropic API key…` だけです。保存されている間、メニューは次の項目に変わります。

- `Replace Anthropic API key` は、同じ端末を開き、保存済みのキーを別のキーに置き換えます。
- `Remove Anthropic API key` は、確認のあと、このコンピューターからキーのファイルを削除し、進行中の翻訳を取り消し、表示済みの訳文を破棄します。キーそのものは、Claude Consoleで削除するまで有効です。

キーが保存されている間、メニューはMarkdownとPlain Textの文書に出ます。ほかの文書では、キーが拒否されたときなど、翻訳に失敗した範囲にだけ出ます。それ以外のときにキーを置き換えるか削除するには、MarkdownかPlain Textのファイルを開いてください。

サーバーが開くのは、macOSでは「ターミナル」、Windowsでは新しいコンソールのウィンドウです。Linuxでは、次の順に試し、最初に起動できたものを使います。`xdg-terminal-exec`、`x-terminal-emulator`、`gnome-terminal`、`ptyxis`、`konsole`、`xfce4-terminal`、`kitty`、`alacritty`、`foot`、`wezterm`、`xterm`。

`Save Anthropic API key` または `Replace Anthropic API key` のあとに出る通知には、コマンドが入っています。言語サーバーのパスに `anthropic-key` を続けたものです。ウィンドウが開かない場合は、このコマンドを自分で端末から実行してください。Linuxで上の端末がどれも起動しない場合と、リモート開発の場合がこれに当たります。リモート開発では、サーバーがリモート側で動き、手元にウィンドウを開かないため、リモート側の端末で実行します。PowerShellでは、引用符で囲まれたパスの前に `&` を付けてください。このコマンドは、実行した端末でキーを尋ね、端末がない場所では失敗します。パイプからキーを読むことはありません。

キーは、サーバーの認証情報のディレクトリにあるファイル `anthropic-api-key` に保存します。サーバーは、キーが要るたびに、ディレクトリとこのファイルを検査します。ファイルの更新時刻、大きさ、macOSとLinuxではinodeのいずれかが変わっていれば、ファイルを読み直します。ファイルにはキーだけを入れます。キーの前後の空白と改行は無視します。自分で作ったり書き換えたりしたファイルも、同じように読みます。

| OS | キーのファイル |
| --- | --- |
| Windows | `%LOCALAPPDATA%\translation-lsp\anthropic-api-key` |
| macOS | `~/Library/Application Support/translation-lsp/anthropic-api-key` |
| Linux | `$XDG_CONFIG_HOME/translation-lsp/anthropic-api-key`。未設定の場合は `~/.config/translation-lsp/anthropic-api-key` |

macOSとLinuxでは、ディレクトリのモードが `0700`、ファイルのモードが `0600` にちょうど一致している必要があります。どちらもシンボリックリンクにはできません。Windowsでは、どちらもシンボリックリンクにはできません。誰がアクセスできるかは、サーバーは確かめません。コマンドは、ディレクトリとファイルをこの形で作ります。別の権限ですでにあるディレクトリは変更しません。その場合は何も保存せず、ディレクトリに求める条件を表示します。

キーは、端末からファイルへ渡ります。Zedの設定には書かず、環境変数も、Zedとサーバーの間のメッセージも通りません。サーバーは `ANTHROPIC_API_KEY` も読みません。Claude Codeはこの名前の環境変数をサブスクリプションのログインより優先するため、シェル全体に設定すると、ふだんのClaude Codeの利用がAPIの課金に切り替わるからです。

使えるキーがなくてもサーバーは起動し、翻訳を依頼すると、APIへ接続せずに設定エラーを返します。エラーは、キーが保存されていないのか、ファイルやディレクトリの権限が緩いのか、ファイルが空かキー以外のものを含むのかを示します。

## 費用と使用量

APIの利用は、キーを発行したClaude Consoleの組織に課金されます。Claudeのサブスクリプションに含まれる利用枠とは別です。`provider.document` を設定すると、タブ翻訳にはそこで指定したモデルの料金がかかります。

コメントのホバーとMarkdownの選択による自動翻訳は、[使い方](../usage.md)のとおり、確認なしに要求を送ります。

`Anthropic API key…` のメニューは、キーの保存、置き換え、削除を行います。使用量を開くコマンドはありません。使用量はClaude Consoleで確認できます。

## 送信する内容

翻訳のために送る文章は、[送信する内容](../usage.md#送信する内容)で説明します。

1回の要求には、システムプロンプトとして翻訳の指示文を、ユーザーメッセージ1件として翻訳する文章を `<source_text>` タグで囲んで含めます。単語を辞書で引く場合は、システムプロンプトを辞書の指示文にし、メッセージには語を `<term>` タグで、語のある行を `<line>` タグで囲んで含めます。会話履歴は送りません。出力は最大64,000トークンを要求し、`translation_limits.output_kib` をバイト数に換算した値（1 KiBあたり1,024）がそれより小さい場合は、その値に合わせます。ホバーで読む翻訳では、さらに16,384トークンに原文（辞書で引く場合は語のある行）1バイトあたり4トークンを足した値までに抑えます。コメントの中の文章によって、モデルに長い出力を書かせないためです。思考のトークンもこの上限に含まれます。上限に達した訳文は、途中までの内容を表示せずにエラーとして報告します。

## エラーと停止

請求のエラー、拒否されたキー、ワークスペースを1つに絞っていないキー、存在しないモデル、権限のエラーでは、自動翻訳を停止し、HTTPの状態コード、APIのエラー種別、要求IDを報告します。Claude Consoleで請求の問題を解消するか、`provider.model` または `provider.document.model` を直したあと、選択範囲または文書の翻訳を明示的に実行してください。それが成功すると自動翻訳を再開します。拒否されたキー、またはワークスペースを1つに絞っていないキーによる停止は、`Anthropic API key…` から別のキーを保存すると終わります。停止は、それが起きたキーに限られるからです。

キーの保存、置き換え、削除に、言語サーバーの再起動は要りません。あるキーで得た訳文と失敗の記録は、別のキーでは表示しません。キーが替わったときに進行中だった翻訳は、完了しても破棄します。設定を変更した場合は、言語サーバーの再起動が必要です。

レート制限と過負荷は、[自動の再試行](../behavior.md#自動の再試行)のとおり一度だけ再試行します。モデルが文章の翻訳を断った場合は、報告された分類を添えて失敗とし、再試行せず、自動翻訳も停止しません。

どのプロバイダーにも共通する規則は、[停止](../behavior.md#停止)と[自動の再試行](../behavior.md#自動の再試行)で説明します。

## 保存するデータ

キーは、[認証](#認証)で説明したファイル `anthropic-api-key` に、平文で入ります。ChatGPTプロバイダーが認証情報を保存するのと同じディレクトリです。ファイルを書くのは、メニューが端末で動かす `anthropic-key` コマンドです。ディレクトリがなければ、このコマンドが作ります。`Remove Anthropic API key` は、このファイルだけを削除します。それ以外では、言語サーバーはファイルを読むだけです。

macOSで「ターミナル」を開くとき、サーバーは一時ディレクトリにスクリプトを書きます。スクリプトに入るのは言語サーバーのパスだけで、キーは入りません。「ターミナル」が実行すると、スクリプトは自分を削除します。

サーバーがキーをログ、エラー文、通知、ディスクのほかの場所に書くことはありません。
