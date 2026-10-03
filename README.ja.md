# Zed Code Translation

[English](README.md)

ZedでRust、JavaScript／JSX、TypeScript／TSXのコメントや、選択したMarkdownなどの文章を翻訳し、Hoverに訳文を表示します。原文はエディタに残ります。コードのコメントは初回からHoverで翻訳します。ChatGPTが利用可能な状態では、Markdownの文章を選択して約0.7秒保つと背景で翻訳し、その範囲へのHoverで読めます。

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

`target_language` の既定値は `ja`、`source_language` は `auto` です。`auto` では、翻訳する同じ要求の中で原語を判定します。`provider.model` はChatGPTを使う場合に必須です。`reasoning_effort` は省略でき、指定する場合は `none`、`minimal`、`low`、`medium`、`high`、`xhigh`、`max` のいずれかです。モデルや推論強度がアカウントで使えない場合はエラーを返します。設定を変更したら言語サーバーを再起動してください。モデルや推論強度の変更は、実行中のサーバーには反映されません。[Zedの言語サーバー設定](https://zed.dev/docs/configuring-languages#configuring-language-servers)に従います。

`binary.path` を省略すると、作業ツリーのPATHにある `zed-code-translation-lsp` を探します。対応言語の `language_servers` を明示している場合は、既存の配列へ `"zed-code-translation"` を追加してください。Zedでは `.jsx` も `JavaScript`、`.tsx` は `TSX` の設定を使います。

## ChatGPTへ接続する

対応する文書でCode Actions（macOSでは `⌘.`）を開き、`接続・設定…` から `Continue with ChatGPT / Choose account` を実行します。アカウントの選択肢から新規追加または保存済みの登録を選び、ブラウザで認証と利用許可を完了してください。設定の読込、候補一覧の表示、Hoverだけでは認証を開始しません。利用可能になると接続項目は `Change ChatGPT account` に変わり、利用許可の項目は表示されません。モデル一覧は利用可能なとき、サインアウトはサインイン中だけ表示されます。管理操作中は重複操作の候補を表示しません。接続状態を読み取れない場合は `Check ChatGPT connection…` を実行し、表示された案内を確認してください。

- `Enable ChatGPT plan use` は、サインイン済みでプラン利用が許可されていない場合に使います。
- `ChatGPT models` は、選択したアカウントのモデル一覧を表示します。この一覧は、対応するモデルの完全な一覧ではありません。一覧にあるslugも無いslugも `provider.model` に指定でき、利用可否は翻訳APIの応答で確認します。一覧の表示ではモデルを選択・適用しません。設定を編集し、言語サーバーを再起動してください。
- `ChatGPT Usage` は、[ChatGPTの使用量設定](https://chatgpt.com/settings/usage)を開きます。利用枠やcreditsの許可はこの設定に従います。
- `Sign out of ChatGPT` は、現在の登録のセッション終了を試み、その登録の認証情報を削除します。別の登録は残ります。通信のため終了を確認できなかった場合は案内に従ってChatGPT設定からアプリを切断できます。

認証情報は拡張専用のローカル保存先に保持します。macOSでは `~/Library/Application Support/zed-code-translation/` です。同じメールアドレスでも登録ごとのUUIDで区別し、再認証には元の登録を使います。APIキーの設定や、別の課金経路への自動切替はありません。接続方式は[公式のSign in with ChatGPT](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)に基づきます。

## 翻訳を表示する

対応するコードのコメントへマウスを重ねるか、カーソルを置いてZedの `editor: hover` を実行します。同じ種類・インデントの隣接した行コメントはまとめて訳し、空行やコードを挟むと分けます。ブロックコメントは一つずつ扱います。コメント記号と直後のASCIIスペース一つを除いた本文を翻訳します。

ChatGPTが利用可能な状態では、Markdownの文章を選択して約0.7秒保つと背景で翻訳します。その範囲でHoverを開いて訳文を読みます。背景翻訳の完了通知は表示しません。コードやリンク先だけ・空白だけの選択では開始しません。コードの囲みなどを途中で切り、選択部分だけでは構文の解釈が変わる範囲も自動対象外です。囲みを含めて選び直すか、手動翻訳を使ってください。選択した文章はコピー目的の場合も送信され得ます。

すぐに実行したい場合や背景翻訳が失敗した場合は、Code Actions（macOSでは `⌘.`）の `選択範囲を翻訳（…）` を使います。明示実行では完了通知 `Translation ready. Hover over the selected text to view it.` が出ます。コード言語やMarkdown以外でも範囲選択による翻訳を使えます。未接続またはプラン利用が無効の場合は、`接続・設定…` から接続・利用許可を実行してください。選択が無いと翻訳項目は表示されません。

明示した翻訳や管理操作が失敗した場合は、エラー通知を確認してください。認証更新で `invalid_grant` などが返された場合は、`接続・設定…` → `Continue with ChatGPT / Choose account` から保存済みの登録を選び、再接続します。

ChatGPTへ送る文章は、保護部分を含む抽出したコメント本文または選択範囲だけです。文書全体や会話履歴は送りません。送信する入力の中で認識されたコード、リンク先、Markdownの構文と改行は、訳文でも元のまま残します。

文書ごとに最後に訳した範囲をメモリに保持します。編集・文書の閉鎖・アカウントの変更・再認証で結果を破棄します。本文や訳文はディスクに保存しません。失敗、途中切断、取消、期限切れでは部分訳を表示しません。

## 模擬モード

`provider` を省略するか `{"name":"mock"}` にすると、接続せずに同梱サンプルを試せます。[samples/comment.rs](samples/comment.rs) のコメントへHoverしてください。[samples/paragraph.md](samples/paragraph.md) では段落を選択し、Code Actionsの `Translate selection to Japanese (mock preview / 模擬翻訳)` を実行してから、選択範囲へHoverします。模擬モードのMarkdownは手動実行が必要です。

模擬モードはこれらのサンプルと改行の違いだけに対応します。ほかの本文や日本語以外の訳先ではエラーを返します。Hoverには模擬訳と分かる表示が付きます。

## 制約

- 自動HoverはRust、JavaScript／JSX、TypeScript／TSXのコメントが対象です。Markdownは文章の選択後に背景翻訳し、段落へのHoverだけでは開始しません。Pythonのコメントには対応していません。
- 選択変更・解除はZedから新しい範囲要求が届いた時点で観測します。それまでは背景要求を即時取消できません。一つの言語サーバーでは最後に観測した選択を扱うため、複数のエディタの選択を独立には扱えません。背景翻訳の失敗を自動で再試行しません。
- 未閉鎖のブロックコメントは、閉じるまで自動翻訳を待ちます。JS／TSの解析が致命的な構文エラーで停止した場合、その後のコメントは自動で取得できません。範囲選択による翻訳を使うか、構文を修正してください。
- 原文範囲は16 KiBまで、翻訳結果は64 KiBまでです。範囲選択と背景翻訳は受付から最大30秒、Hoverの要求は最大10秒で期限切れになります。待機時間も含み、同じ範囲への要求が合流した場合も、それぞれの受付から数えます。混雑時は完了後に再実行してください。
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
