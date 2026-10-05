# Zed Code Translation

[English](README.md)

ZedでRust、JavaScript／JSX、TypeScript／TSXのコメントや、選択したMarkdownなどの文章を翻訳し、Hoverに訳文を表示します。原文はエディタに残ります。コードのコメントは初回からHoverで翻訳します。ChatGPTが利用可能な状態では、Markdownの文章を選択して安定させると背景で翻訳し、その範囲へのHoverで読めます。

ChatGPTを使う場合は、本人のChatGPTアカウントで接続し、ChatGPTプランの利用を許可します。設定なしでは、同梱サンプルだけを日本語に訳す模擬モードで動作し、文章を外部へ送りません。

## 導入

Rust 1.96以降のCargoとZedが必要です。checkoutのルートでネイティブLSPと拡張のWASMをビルドします。

```sh
cargo build -p zed-code-translation-lsp --release --locked
rustup target add wasm32-wasip2
cargo build --target wasm32-wasip2 --release --locked
```

Zedの拡張画面で `Install Dev Extension` を選び、checkout内の **`crates/zed-extension`** を指定します。[Zedの開発用拡張の導入手順](https://zed.dev/docs/extensions/developing-extensions)も参照してください。

## 設定

既存のZed設定の `lsp` に次の項目を追加し、`binary.path` をビルドした実行ファイルの絶対パスへ置き換えます。モデルは、接続するアカウントで使えるものを指定してください。下記のモデルと推論強度は設定例です。

```json
{
  "lsp": {
    "zed-code-translation": {
      "binary": {
        "path": "/absolute/path/to/checkout/target/release/zed-code-translation-lsp",
        "arguments": []
      },
      "initialization_options": {
        "target_language": "ja",
        "source_language": "auto",
        "hover_timeout_ms": 20000,
        "selection_timeout_ms": 30000,
        "markdown_auto_translate": true,
        "selection_debounce_ms": 450,
        "provider": {
          "name": "chatgpt",
          "model": "gpt-6.1-sol",
          "reasoning_effort": "low"
        }
      }
    }
  }
}
```

UIの表示は `target_language` にかかわらず英語です。`target_language` の既定値は `ja`、`source_language` は `auto` です。`auto` では、翻訳する同じ要求の中で原語を判定します。`provider.model` はChatGPTを使う場合に必須です。`reasoning_effort` は省略でき、指定する場合は `none`、`minimal`、`low`、`medium`、`high`、`xhigh`、`max` のいずれかです。モデルや推論強度がアカウントで使えない場合はエラーを返します。設定を変更したら言語サーバーを再起動してください。設定変更は、実行中のサーバーには反映されません。[Zedの言語サーバー設定](https://zed.dev/docs/configuring-languages#configuring-language-servers)に従います。

翻訳のタイミング設定は省略できます。

| 設定 | 既定値 | 指定できる値 | 動作 |
| --- | --- | --- | --- |
| `hover_timeout_ms` | `20000` | 整数 `1000`〜`120000`、両端を含む | コメントや翻訳待ちの選択範囲へのHover要求ごとの期限。 |
| `selection_timeout_ms` | `30000` | 整数 `1000`〜`120000`、両端を含む | 手動選択要求とMarkdown背景翻訳それぞれの期限。 |
| `markdown_auto_translate` | `true` | 真偽値 | 安定したMarkdownの文章選択から背景翻訳を始める。`false` でも手動選択翻訳とコメントHoverを使える。 |
| `selection_debounce_ms` | `450` | 整数 `100`〜`2000`、両端を含む | nativeサーバーが新しい選択を観測してから待つ時間。Zed側の観測待ちは別に加わる。 |

未知の項目、型の違う値、範囲外の数値は、英語の設定エラーを返して言語サーバーの起動を止めます。項目を省略すると既定値を使います。

`binary.path` を省略すると、作業ツリーのPATHにある `zed-code-translation-lsp` を探します。対応言語の `language_servers` を明示している場合は、既存の配列へ `"zed-code-translation"` を追加してください。Zedでは `.jsx` も `JavaScript`、`.tsx` は `TSX` の設定を使います。

## 翻訳モデルを変更する

1. Zedのコマンドパレットで `zed: open settings file` を実行します。
2. `lsp.zed-code-translation.initialization_options.provider.model` を、使いたいモデルのslugに変更します。
3. 必要に応じて `lsp.zed-code-translation.initialization_options.provider.reasoning_effort` を指定します。APIの既定値を使う場合は省略し、指定する場合は上記の値から選んでください。明示した `none` は省略とは異なります。
4. 設定を保存します。この拡張を使う文書で、コマンドパレットから `editor: restart language server` を実行します。

拡張は設定したモデルのslugをそのまま翻訳APIへ送り、別のモデルへ自動で切り替えません。そのモデルと推論強度をアカウントで使えるかどうかは、APIが判断します。

モデルには、文字列パターンと配列の要素数を制約できるStructured Outputsへの対応が必要です。翻訳の応答形式はAPIに指定します。非対応のモデルやスキーマはエラーとし、別モデルへの切り替えやMarkdown保護の緩和は行いません。

## ChatGPTへ接続する

対応する文書でCode Actions（macOSでは `⌘.`）を開き、`ChatGPT account…` から `Continue with ChatGPT / Choose account` を実行します。アカウントの選択肢から新規追加または保存済みの登録を選び、ブラウザで認証と利用許可を完了してください。設定の読込、アカウント管理メニューを開く操作、Hoverだけでは認証を開始しません。利用可能になると接続項目は `Change ChatGPT account` に変わり、利用許可の項目は表示されません。サインアウトはサインイン中だけ表示されます。管理操作中は重複操作の候補を表示しません。接続状態を読み取れない場合は `Check ChatGPT connection…` を実行し、表示された案内を確認してください。

- `Enable ChatGPT plan use` は、サインイン済みでプラン利用が許可されていない場合に使います。
- `ChatGPT Usage` は、[ChatGPTの使用量設定](https://chatgpt.com/settings/usage)を開きます。利用枠やcreditsの許可はこの設定に従います。
- `Sign out of ChatGPT` は、現在の登録のセッション終了を試み、その登録の認証情報を削除します。別の登録は残ります。通信のため終了を確認できなかった場合は案内に従ってChatGPT設定からアプリを切断できます。

認証情報は拡張専用のローカル保存先に保持します。macOSでは `~/Library/Application Support/zed-code-translation/` です。同じメールアドレスでも登録ごとのUUIDで区別し、再認証には元の登録を使います。APIキーの設定や、別の課金経路への自動切替はありません。接続方式は[公式のSign in with ChatGPT](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)に基づきます。

## 翻訳を表示する

対応するコードのコメントへマウスを重ねるか、カーソルを置いてZedの `editor: hover` を実行します。同じ種類・インデントの隣接した行コメントはまとめて訳し、空行やコードを挟むと分けます。ブロックコメントは一つずつ扱います。コメント記号と直後のASCIIスペース一つを除いた本文を翻訳します。

異なる文書の翻訳は最大2件まで同時に実行し、待機分を含めて最大4件を受け付けます。一つの文書で進められるのは一つの範囲だけです。同じ範囲への要求は一つの翻訳を共有します。

翻訳の待機・実行中は、ステータスバー左側にスピナーと文字を直接表示します。`Queued: …` は待機中の件数、`Translating: …, queued: …` は実行中と待機中の件数です。同じ範囲への複数の要求は一件と数えます。この欄はほかの言語サーバーと共有するため、別の活動が優先されることがあります。

スピナーは取消や時間切れを含め、作業が終わると消えます。結果は原文へのHoverで確認してください。状態表示に対応するクライアントでは、翻訳失敗・時間切れの警告が残り、再試行・編集・文書の閉鎖で消えます。これらの表示に対応しない場合も、Hoverと明示操作の通知を使えます。

ChatGPTが利用可能で `markdown_auto_translate` が `true` の場合、Markdownの文章を選択して安定させると背景で翻訳します。既定ではZedから選択が届いた後に450ms待ち、Zed側の観測待ちも別に加わります。その範囲でHoverを開いて訳文を読みます。背景翻訳の完了通知は表示しません。コードやリンク先だけ・空白だけの選択では開始しません。コードの囲みなどを途中で切り、選択部分だけでは構文の解釈が変わる範囲も自動対象外です。囲みを含めて選び直すか、手動翻訳を使ってください。選択した文章はコピー目的の場合も送信され得ます。

すぐに実行したい場合や背景翻訳が失敗した場合は、Code Actions（macOSでは `⌘.`）の `Translate selection to …` を使います。明示実行では完了通知 `Translation ready. Hover over the selected text to view it.` が出ます。コード言語やMarkdown以外でも範囲選択による翻訳を使えます。未接続またはプラン利用が無効の場合は、`ChatGPT account…` から接続・利用許可を実行してください。選択が無いと翻訳項目は表示されません。

明示した翻訳や管理操作が失敗した場合は、エラー通知を確認してください。認証更新で `invalid_grant` などが返された場合は、`ChatGPT account…` → `Continue with ChatGPT / Choose account` から保存済みの登録を選び、再接続します。

翻訳エラーでは、応答形式の不正、訳文の個数違い、改行の混入、Markdown構造の変化を区別します。検証に通らない訳文は表示しません。サーバー状態表示に対応するクライアントでは、背景翻訳の失敗にも同じ理由を表示します。時間切れは別のエラーです。選択範囲を短くするか、タイムアウト設定を変更して言語サーバーを再起動してください。応答内容の不正は、同じ要求を繰り返しても解消するとは限りません。

ChatGPTへ送る文章は、保護部分を含む抽出したコメント本文または選択範囲だけです。文書全体や会話履歴は送りません。送信する入力の中で認識されたコード、リンク先、Markdownの構文と改行は、訳文でも元のまま残します。

文書ごとに最後に訳した範囲をメモリに保持します。編集・文書の閉鎖・アカウントの変更・再認証で結果を破棄します。本文や訳文はディスクに保存しません。失敗、途中切断、取消、期限切れでは部分訳を表示しません。

## 模擬モード

`provider` を省略するか `{"name":"mock"}` にすると、接続せずに同梱サンプルを試せます。[samples/comment.rs](samples/comment.rs) のコメントへHoverしてください。[samples/paragraph.md](samples/paragraph.md) では段落を選択し、Code Actionsの `Translate selection to ja` を実行してから、選択範囲へHoverします。模擬モードのMarkdownは手動実行が必要です。

模擬モードはこれらのサンプルと改行の違いだけに対応します。ほかの本文や日本語以外の訳先ではエラーを返します。Hoverには `Mock translation (fixed sample)` と表示します。

## 制約

- 自動HoverはRust、JavaScript／JSX、TypeScript／TSXのコメントが対象です。Markdownは文章の選択後に背景翻訳し、段落へのHoverだけでは開始しません。Pythonのコメントには対応していません。
- 選択変更・解除はZedから新しい範囲要求が届いた時点で観測します。それまでは背景要求を即時取消できません。一つの言語サーバーでは最後に観測した選択を扱うため、複数のエディタの選択を独立には扱えません。背景翻訳の失敗を自動で再試行しません。
- 未閉鎖のブロックコメントは、閉じるまで自動翻訳を待ちます。JS／TSの解析が致命的な構文エラーで停止した場合、その後のコメントは自動で取得できません。範囲選択による翻訳を使うか、構文を修正してください。
- 原文範囲は16 KiBまで、翻訳結果は64 KiBまでです。既定では範囲選択と背景翻訳は翻訳受付から最大30秒、Hover要求はそれぞれ最大20秒で期限切れになります。workerの待ち時間、API呼出し、認証更新、streamingを含みます。選択の安定待ちや、受付前の空き枠・管理操作の待ち時間は含みません。同じ範囲への要求が合流しても、それぞれの期限を保持します。OAuth、接続、管理操作には別の内部制限もあります。混雑時は完了後に再実行してください。
- 利用枠の上限やアカウントの利用制限では新しい翻訳を停止し、エラーを表示します。使用量はChatGPTの設定で確認してください。通信障害では認証情報を保持し、少し待ってから再実行できます。
- Hoverの位置と幅はZedが決めます。上側への固定や、本文を完全に隠さない配置は保証しません。

## 開発

両crateはRust Edition 2024、最低対応Rustは1.96です。virtual workspaceにはresolver 3を使います。`Cargo.lock` をバージョン管理し、検証とreleaseビルドは `--locked` で実行してください。

```sh
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

依存は通常のCargo互換範囲を `major.minor` で指定します（`serde` と `serde_json` も `1.0`）。`cargo update` は意図した更新時に実行し、lockfileの変更を確認して、上記の検証とnative/WASMのreleaseビルドを繰り返してください。Oxcのparser・allocator・spanは同じリリース系列に揃えます。`zed_extension_api` を更新する場合は、WASMビルドに加えて対象ZedのAPI対応を確認してください。HTTP/OIDCテストはローカルのfixtureと合成の認証情報を使います。FixtureごとにOS乱数からEd25519署名鍵を生成し、メモリで保持して、JWT署名とJWKSに同じ鍵を使います。

Rust lexerはUnicodeテーブルの版が一致している必要があります。lockfileでは `unicode-properties` 0.1.4に合わせ、Unicode 17の `unicode-ident` 1.0.24を使います。1.0.25・1.0.26はUnicode 18で、lexerのコンパイル時検査に失敗します。更新でこのpackageが変わった場合は `cargo update -p unicode-ident --precise 1.0.24` で互換lockへ戻すか、lexerと両テーブルcrateが同じUnicode版に対応していることを確認してください。
