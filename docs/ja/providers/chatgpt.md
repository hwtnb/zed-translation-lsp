# ChatGPT

[English](../../providers/chatgpt.md)

ChatGPTを使う場合は、本人のChatGPTアカウントで接続し、ChatGPTプランの利用を許可します。

## 最小の設定

```json
{
  "lsp": {
    "translation-lsp": {
      "initialization_options": {
        "provider": { "name": "chatgpt", "model": "gpt-6.1-sol", "reasoning_effort": "low" }
      }
    }
  }
}
```

[設定](../configuration.md)で説明するほかの `initialization_options` は、同じように使えます。

## 設定項目

| 設定 | 既定値 | 規則 |
| --- | --- | --- |
| `provider.name` | なし | `"chatgpt"` でこのプロバイダーを選びます。 |
| `provider.model` | なし | 必須です。空の値、空白や制御文字を含む値、256バイトを超える値は設定エラーです。指定した値をそのまま送り、別のモデルへ置き換えません。 |
| `provider.reasoning_effort` | なし | 省略できます。`none`、`minimal`、`low`、`medium`、`high`、`xhigh`、`max` のいずれかです。省略した場合は、APIの既定値を使います。明示した `none` は省略とは異なります。 |
| `provider.document` | なし | 省略できます。タブに開く翻訳で使うモデルと設定を入れるオブジェクトです。中の値は `provider.model`、`provider.reasoning_effort` を引き継ぎません。中で省略した値は、下の行の既定値になります。中の未知の項目は設定エラーです。 |
| `provider.document.model` | なし | `provider.document` を書く場合は必須です。規則は `provider.model` と同じです。 |
| `provider.document.reasoning_effort` | なし | 省略できます。選べる値は `provider.reasoning_effort` と同じです。 |

モデルや推論強度がアカウントで使えない場合はエラーを返します。未知の項目は拒否します。設定を変更したら言語サーバーを再起動してください。

### タブ翻訳だけ別のモデルを使う

```json
"provider": {
  "name": "chatgpt",
  "model": "gpt-6.1-sol",
  "reasoning_effort": "low",
  "document": { "model": "gpt-6.1-sol", "reasoning_effort": "medium" }
}
```

`provider.document` の外側に書いた `reasoning_effort` は、タブ翻訳には適用されません。`provider.document` の中で省略すると、タブ翻訳の要求は推論強度を含まず、モデル側の既定値が使われます。タブ翻訳の推論強度を固定したい場合は、`provider.document` の中にも書いてください。上のモデルと推論強度は例です。

`provider.document` を設定したときにどのプロバイダーでも変わる点は、[タブ翻訳だけ別のモデルを使う](../configuration.md#タブ翻訳だけ別のモデルを使う)で説明します。

## 認証

対応する文書でZedのコマンドパレットから `editor: toggle code actions` を実行し、`ChatGPT account…` から `Continue with ChatGPT / Choose account` を実行します。アカウントの選択肢から新規追加または保存済みの登録を選び、ブラウザで認証と利用許可を完了してください。設定の読み込み、アカウント管理メニューを開く操作、ホバーだけでは認証を開始しません。

利用可能になると接続項目は `Change ChatGPT account` に変わり、利用許可の項目は表示されません。サインアウトはサインイン中だけ表示されます。管理操作中は重複操作の候補を表示しません。接続状態を読み取れない場合は `Check ChatGPT connection…` を実行し、表示された案内を確認してください。

- `Enable ChatGPT plan use` は、サインイン済みでプラン利用が許可されていない場合に使います。
- `ChatGPT Usage` は、[ChatGPTの使用量設定](https://chatgpt.com/settings/usage)を開きます。利用枠やクレジットの許可はこの設定に従います。
- `Sign out of ChatGPT` は、現在の登録のセッション終了を試み、その登録の認証情報を削除します。別の登録は残ります。通信のため終了を確認できなかった場合は案内に従ってChatGPT設定からアプリを切断できます。

認証更新で `invalid_grant` などが返された場合は、`ChatGPT account…` → `Continue with ChatGPT / Choose account` から保存済みの登録を選び、再接続します。

接続方式は[公式のSign in with ChatGPT](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)に基づきます。

## 費用と使用量

ChatGPTプロバイダーではAPIキーを使えません。拡張が別の課金経路へ自動で切り替えることもありません。

利用枠はChatGPTの使用量設定で確認してください。[認証](#認証)の `ChatGPT Usage` から、その設定を開けます。

## 送信する内容

翻訳のために送る文章は、[送信する内容](../usage.md#送信する内容)で説明します。

## エラーと停止

認証更新で無効な認可と判明した場合を除き、失敗しても認証情報を保持します。

どのプロバイダーにも共通する規則は、[停止](../behavior.md#停止)と[自動の再試行](../behavior.md#自動の再試行)で説明します。

## 保存するデータ

認証情報は、OSごとに次のローカル設定ディレクトリへ保存します。

| OS | 認証情報の保存先 |
| --- | --- |
| Windows | `%LOCALAPPDATA%\translation-lsp\` |
| macOS | `~/Library/Application Support/translation-lsp/` |
| Linux | `$XDG_CONFIG_HOME/translation-lsp/`。未設定の場合は `~/.config/translation-lsp/` |

保存先は `dirs::config_local_dir()` を通じてOSから取得します。同じメールアドレスでも登録ごとのUUIDで区別し、再認証には元の登録を使います。

認証データは `auth.json` のバージョン2で保存します。バージョン1は初回アクセス時に認証ストアをロックし、原子的に保存して移行します。認証情報のバックアップは作りません。トークン・登録・選択中のアカウントは保持します。登録ごとに認可IDを追加し、トークン更新では保持、再認証では更新します。トークン更新や一時的なストアのロックでキャッシュ済みの訳は破棄せず、表示前に認可を再確認します。

旧版のバイナリはバージョン2を読めません。旧版へ戻す場合は、その時点の認証データをバージョン1へ変換するか、再認証してください。
