# Translation LSP

[English](README.md)

ZedでRust、JavaScript／JSX、TypeScript／TSX、C#、C、C++、Pythonのコメント、Pythonのdocstring、選択したMarkdownなどの文章を翻訳し、Hoverに訳文を表示します。原文はエディタに残ります。コードのコメントは初回からHoverで翻訳します。ChatGPTが利用可能な状態では、Markdownの文章を選択して安定させると背景で翻訳し、その範囲へのHoverで読めます。

ChatGPTを使う場合は、本人のChatGPTアカウントで接続し、ChatGPTプランの利用を許可します。設定なしでは、同梱サンプルだけを日本語に訳す模擬モードで動作し、文章を外部へ送りません。

## 導入

開発・利用の対象は **Windows・macOS・Linux** です。Zedと、[rustup経由のRust](https://www.rust-lang.org/tools/install)（1.97以降）を導入してください。ネイティブ依存のビルドにはC/C++の開発環境も必要です。

| OS | ビルドの前提 | ネイティブ実行ファイル |
| --- | --- | --- |
| Windows | MSVC版Rust、Visual Studio Build Toolsの「C++によるデスクトップ開発」とWindows SDK。x86-64ではNASMも導入し、Visual Studioの開発者用PowerShellまたはコマンドプロンプトを使います。 | `target/release/translation-lsp.exe` |
| macOS | Xcode Command Line Tools（`xcode-select --install`）。 | `target/release/translation-lsp` |
| Linux | GCCまたはClangとC/C++開発ツール。Debian/Ubuntuでは `build-essential` と `ca-certificates` を導入します。 | `target/release/translation-lsp` |

Tree-sitterとTLS Providerはネイティブコードをビルドします。別のツールチェーン構成はTLS Providerの[Windows向け要件](https://aws.github.io/aws-lc-rs/requirements/windows.html)と[Linux向け要件](https://aws.github.io/aws-lc-rs/requirements/linux.html)を参照してください。

checkoutのルートで次のコマンドを実行します。PowerShell・コマンドプロンプト・Bash・Zshで共通です。

```sh
cargo build -p translation-lsp --release --locked
rustup target add wasm32-wasip2
cargo build -p translation-lsp-extension --target wasm32-wasip2 --release --locked
```

Zedの拡張画面で `Install Dev Extension` を選び、checkout内の **`crates/zed-extension`** を指定します。[Zedの開発用拡張の導入手順](https://zed.dev/docs/extensions/developing-extensions)も参照してください。

C#ファイルでは、Zedの[C#言語拡張](https://github.com/zed-extensions/csharp)を導入してください。設定上の言語名は `CSharp` です。`languages.CSharp.language_servers` を明示している場合は、既存のC#サーバーに `"translation-lsp"` を追加します。RoslynやOmniSharpを置き換える必要はありません。

CとC++はZedに組み込まれています。言語サーバーを明示設定している場合は、`languages.C.language_servers` と `languages."C++".language_servers` の既存clangdに `"translation-lsp"` を追加します。

PythonもZedに組み込まれています。`languages.Python.language_servers` を明示設定している場合は、既存のPythonサーバーに `"translation-lsp"` を追加します。

## 設定

拡張ID・言語サーバーの設定キー・ネイティブ実行ファイル名は `translation-lsp` です。

既存のZed設定の `lsp` に次の項目を追加し、`binary.path` をビルドした実行ファイルの絶対パスへ置き換えます。モデルは、接続するアカウントで使えるものを指定してください。下記のモデルと推論強度は設定例です。

```json
{
  "lsp": {
    "translation-lsp": {
      "binary": {
        "path": "REPLACE_WITH_ABSOLUTE_SERVER_PATH",
        "arguments": []
      },
      "initialization_options": {
        "target_language": "ja",
        "source_language": "auto",
        "hover_timeout_ms": 20000,
        "selection_timeout_ms": 30000,
        "document_timeout_ms": 180000,
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

`binary.path` は言語サーバーを動かすOSの絶対パスにします。Windowsでは `.exe` まで指定してください。

| OS | `binary.path` の例 |
| --- | --- |
| Windows | `C:/Users/you/src/translation-lsp/target/release/translation-lsp.exe` |
| macOS | `/Users/you/src/translation-lsp/target/release/translation-lsp` |
| Linux | `/home/you/src/translation-lsp/target/release/translation-lsp` |

WindowsのパスもJSON内では `/` で区切れます。バックスラッシュを使う場合は1文字ごとに `\\` と記述してください。空白を含むパスでも、値の中にシェル用の引用符を追加しません。

UIの表示は `target_language` にかかわらず英語です。`target_language` の既定値は `ja`、`source_language` は `auto` です。`auto` では、翻訳する同じ要求の中で原語を判定します。`provider.model` はChatGPTを使う場合に必須です。`reasoning_effort` は省略でき、指定する場合は `none`、`minimal`、`low`、`medium`、`high`、`xhigh`、`max` のいずれかです。モデルや推論強度がアカウントで使えない場合はエラーを返します。設定を変更したら言語サーバーを再起動してください。設定変更は、実行中のサーバーには反映されません。[Zedの言語サーバー設定](https://zed.dev/docs/configuring-languages#configuring-language-servers)に従います。

翻訳のタイミング設定は省略できます。

| 設定 | 既定値 | 指定できる値 | 動作 |
| --- | --- | --- | --- |
| `hover_timeout_ms` | `20000` | 整数 `1000`〜`120000`、両端を含む | コメントや翻訳待ちの選択範囲へのHover要求ごとの期限。 |
| `selection_timeout_ms` | `30000` | 整数 `1000`〜`120000`、両端を含む | 手動選択のHoverプレビュー要求とMarkdown背景翻訳それぞれの期限。 |
| `document_timeout_ms` | `180000` | 整数 `1000`〜`600000`、両端を含む | 別タブに開く文書全体・選択範囲の翻訳期限。キューでの待ち時間を含む。 |
| `markdown_auto_translate` | `true` | 真偽値 | 安定したMarkdownの文章選択から背景翻訳を始める。`false` でも手動選択翻訳とコメントHoverを使える。 |
| `selection_debounce_ms` | `450` | 整数 `100`〜`2000`、両端を含む | nativeサーバーが新しい選択を観測してから待つ時間。Zed側の観測待ちは別に加わる。 |

別タブに開く翻訳は、コマンドを受け付けた後も処理を続け、完了時に結果を開きます。全文・選択範囲とも既定の期限は3分です。Hoverや背景翻訳の期限とは独立しており、文字数による自動延長はしません。処理中のキャンセルは進捗表示から行えます。元の文書を編集・閉じた場合も結果を開きません。

未知の項目、型の違う値、範囲外の数値は、英語の設定エラーを返して言語サーバーの起動を止めます。項目を省略すると既定値を使います。

`binary.path` を省略すると、作業ツリーのPATHにある `translation-lsp` を探します。対応言語の `language_servers` を明示している場合は、既存の配列へ `"translation-lsp"` を追加してください。Zedでは `.jsx` も `JavaScript`、`.tsx` は `TSX` の設定を使います。

## 翻訳モデルを変更する

1. Zedのコマンドパレットで `zed: open settings file` を実行します。
2. `lsp.translation-lsp.initialization_options.provider.model` を、使いたいモデルのslugに変更します。
3. 必要に応じて `lsp.translation-lsp.initialization_options.provider.reasoning_effort` を指定します。APIの既定値を使う場合は省略し、指定する場合は上記の値から選んでください。明示した `none` は省略とは異なります。
4. 設定を保存します。この拡張を使う文書で、コマンドパレットから `editor: restart language server` を実行します。

拡張は設定したモデルのslugをそのまま翻訳APIへ送り、別のモデルへ自動で切り替えません。そのモデルと推論強度をアカウントで使えるかどうかは、APIが判断します。

翻訳には通常のテキスト出力を使います。ChatGPTアカウントで利用可能なモデルを選んでください。要求が失敗しても、拡張が別モデルへ切り替えることはありません。

## ChatGPTへ接続する

対応する文書でZedのコマンドパレットから `editor: toggle code actions` を実行し、`ChatGPT account…` から `Continue with ChatGPT / Choose account` を実行します。アカウントの選択肢から新規追加または保存済みの登録を選び、ブラウザで認証と利用許可を完了してください。設定の読込、アカウント管理メニューを開く操作、Hoverだけでは認証を開始しません。利用可能になると接続項目は `Change ChatGPT account` に変わり、利用許可の項目は表示されません。サインアウトはサインイン中だけ表示されます。管理操作中は重複操作の候補を表示しません。接続状態を読み取れない場合は `Check ChatGPT connection…` を実行し、表示された案内を確認してください。

- `Enable ChatGPT plan use` は、サインイン済みでプラン利用が許可されていない場合に使います。
- `ChatGPT Usage` は、[ChatGPTの使用量設定](https://chatgpt.com/settings/usage)を開きます。利用枠やcreditsの許可はこの設定に従います。
- `Sign out of ChatGPT` は、現在の登録のセッション終了を試み、その登録の認証情報を削除します。別の登録は残ります。通信のため終了を確認できなかった場合は案内に従ってChatGPT設定からアプリを切断できます。

認証情報は、OSごとに次のローカル設定ディレクトリへ保存します。

| OS | 認証情報の保存先 |
| --- | --- |
| Windows | `%LOCALAPPDATA%\translation-lsp\` |
| macOS | `~/Library/Application Support/translation-lsp/` |
| Linux | `$XDG_CONFIG_HOME/translation-lsp/`。未設定の場合は `~/.config/translation-lsp/` |

保存先は `dirs::config_local_dir()` を通じてOSから取得します。同じメールアドレスでも登録ごとのUUIDで区別し、再認証には元の登録を使います。APIキーの設定や、別の課金経路への自動切替はありません。接続方式は[公式のSign in with ChatGPT](https://developers.openai.com/siwc/token-sharing-open-source/sign-in)に基づきます。

## 翻訳を表示する

対応するコードのコメントへマウスを重ねるか、カーソルを置いてZedの `editor: hover` を実行します。同じ種類・インデントの隣接した行コメントはまとめて訳し、空行やコードを挟むと分けます。ブロックコメントは一つずつ扱います。コメント記号と直後のASCIIスペース一つを除いた本文を翻訳します。

異なる文書の翻訳は最大2件まで同時に実行し、待機分を含めて最大4件を受け付けます。一つの文書で進められるのは一つの範囲だけです。同じ範囲への要求は一つの翻訳を共有します。

翻訳の待機・実行中は、ステータスバー左側にスピナーと文字を直接表示します。`Queued: …` は待機中の件数、`Translating: …, queued: …` は実行中と待機中の件数です。同じ範囲への複数の要求は一件と数えます。この欄はほかの言語サーバーと共有するため、別の活動が優先されることがあります。

スピナーは取消や時間切れを含め、作業が終わると消えます。結果は原文へのHoverで確認してください。状態表示に対応するクライアントでは、翻訳失敗・時間切れの警告が残り、再試行・編集・文書の閉鎖で消えます。これらの表示に対応しない場合も、Hoverと明示操作の通知を使えます。

ChatGPTが利用可能で `markdown_auto_translate` が `true` の場合、Markdownの文章を選択して安定させると背景で翻訳します。既定ではZedから選択が届いた後に450ms待ち、Zed側の観測待ちも別に加わります。その範囲でHoverを開いて訳文を読みます。背景翻訳の完了通知は表示しません。コードやリンク先だけ・空白だけの選択では開始しません。コードの囲みなどを途中で切り、選択部分だけでは構文の解釈が変わる範囲も自動対象外です。囲みを含めて選び直すか、手動翻訳を使ってください。選択した文章はコピー目的の場合も送信され得ます。

すぐに実行したい場合や背景翻訳が失敗した場合は、Code Actionsの `Translate selection to …` を使います。明示実行では完了通知 `Translation ready. Hover over the selected text to view it.` が出ます。コード言語やMarkdown以外でも範囲選択による翻訳を使えます。未接続またはプラン利用が無効の場合は、`ChatGPT account…` から接続・利用許可を実行してください。選択が無いとHover用の翻訳項目は表示されません。

MarkdownまたはPlain Textでは、`Translate selection to … and open in tab` で訳文を別タブに開けます。選択が無い場合は `Translate document to … and open in tab` となり、文書全体を翻訳します。入力上限は選択翻訳と同じ16 KiBです。クライアントには `window/showDocument` 対応が必要です。

Markdownの訳文は一時的な `.md` ファイルとして開きます。コマンドパレットの `markdown: open preview` で整形された表示にし、必要に応じてソースタブへ戻れます。Plain Textの訳文は `.txt` で開きます。通常のMarkdownの開き方は変更しません。開いた訳文はその時点の内容で、原文の編集には追従しません。必要なときに再び翻訳操作を実行してください。

残したい訳文は、言語サーバーの再起動やZedの終了前に「別名で保存」してください。一時ファイルはサーバーの正常終了時に削除します。異常終了した場合はシステムの一時フォルダーに残ることがあります。別名保存したファイルは削除しません。

相対パスのリンクや画像は一時ファイルの位置を基準に解決します。元の相対パスを使う場合は、原文と同じフォルダーへ別名保存してください。

明示した翻訳や管理操作が失敗した場合は、エラー通知を確認してください。認証更新で `invalid_grant` などが返された場合は、`ChatGPT account…` → `Continue with ChatGPT / Choose account` から保存済みの登録を選び、再接続します。

完成した訳文をテキストとしてそのまま表示します。選択した文章をまとまりとして渡し、意味、コード、識別子、URLを保って訳文だけを返すようモデルに指示します。訳文をJSONの断片へ分けたり、Markdownの書式や改行の違いを理由に拒否したりしません。この指示によって翻訳の正確さを保証するものではありません。

ChatGPTへ送る文章は、抽出したコメント本文、選択範囲、または文書翻訳を明示した場合の文書全体です。会話履歴は送りません。訳文を原文へ書き戻すこともありません。

文書ごとに最後に訳した範囲をメモリに保持します。編集・文書の閉鎖・アカウントの変更・再認証で結果を破棄します。Hoverでは本文や訳文をディスクに保存しません。訳文を別タブで開く操作では、上記のとおり完成した訳文を一時ファイルへ書き込みます。失敗、途中切断、取消、期限切れでは部分訳を表示しません。

## 模擬モード

`provider` を省略するか `{"name":"mock"}` にすると、接続せずに同梱サンプルを試せます。[samples/comment.rs](samples/comment.rs) のコメント、または [samples/comment.cs](samples/comment.cs) の通常コメント・XMLドキュメントコメントへHoverしてください。C/C++は [samples/comment.c](samples/comment.c) と [samples/comment.cpp](samples/comment.cpp) でDoxygenコメントも試せます。

Pythonのコメントとdocstringは [samples/comment.py](samples/comment.py) で試せます。[samples/paragraph.md](samples/paragraph.md) では段落を選択し、Code Actionsの `Translate selection to ja` を実行してから、選択範囲へHoverします。模擬モードのMarkdownは手動実行が必要です。

模擬モードはこれらのサンプルと改行の違いだけに対応します。ほかの本文や日本語以外の訳先ではエラーを返します。Hoverには `Mock translation (fixed sample)` と表示します。

## 制約

- 自動HoverはRust、JavaScript／JSX、TypeScript／TSX、C#、C、C++、Pythonのコメントとdocstringが対象です。Markdownは文章の選択後に背景翻訳し、段落へのHoverだけでは開始しません。
- 選択変更・解除はZedから新しい範囲要求が届いた時点で観測します。それまでは背景要求を即時取消できません。一つの言語サーバーでは最後に観測した選択を扱うため、複数のエディタの選択を独立には扱えません。背景翻訳の失敗から新しい処理は開始しません。一時的なサービスエラーが明示された場合は、現在の処理内で一度だけ再試行します。
- C#は `//`、`/* ... */`、`///`、`/** ... */` に対応します。同じ種類・字下げで連続する行コメントはまとめて翻訳します。XMLドキュメントコメントはタグ付きの訳文をHoverに表示し、タグ・属性の保持をモデルへ指示します。XML専用の整形表示や構造検証は行いません。構文エラーで判定が曖昧な範囲のコメントは自動Hoverの対象から外れますが、手動の範囲選択翻訳は使えます。
- C/C++は通常コメントとDoxygen形式（`///`、`//!`、`/**`、`/*!`、末尾の説明用の `<` 付き形式）に対応します。`@param`などは本文に残し、コマンドと引数名の保持をモデルへ指示します。構文エラーからの復旧で未閉鎖コメント内をコードと誤認する場合があるため、最初の構文エラー以降は自動抽出を止めます。その範囲は手動選択で翻訳できます。
- Pythonは `#` コメントと、モジュール・クラス・関数の先頭のdocstringに対応します。通常・raw・Unicode形式、括弧付き、暗黙に連結した文字列を扱います。代入された文字列、f-string、bytesは自動翻訳しません。docstringの引用符と接頭辞を除き、エスケープや字下げはPythonとして評価せず原文のまま送ります。shebangは除外します。最初の構文エラー以降は自動抽出を止めますが、手動選択は使えます。
- 未閉鎖のブロックコメントは、閉じるまで自動翻訳を待ちます。JS／TSの解析が致命的な構文エラーで停止した場合、その後のコメントは自動で取得できません。範囲選択による翻訳を使うか、構文を修正してください。
- 原文範囲は16 KiBまで、翻訳結果は64 KiBまでです。翻訳受付からの期限は、既定で別タブに開く翻訳が180秒、選択範囲のHoverプレビューと背景翻訳が30秒、個々のHover要求が20秒です。workerの待ち時間、API呼出し、認証更新、streamingを含みます。選択の安定待ちや、受付前の空き枠・管理操作の待ち時間は含みません。同じ範囲への要求が合流しても、それぞれの期限を保持します。OAuth、接続、管理操作には別の内部制限もあります。混雑時は完了後に再実行してください。
- HTTP 200の後に通知される過負荷を含め、明示された一時的なサービスエラーは元の期限内で最大一度だけ再試行します。既定の待機は2秒です。数値の `Retry-After` が120秒以内ならその秒数を使い、それ以外の値では手動再試行に委ねます。取消・文書の変更・アカウントの変更で再試行待ちも停止します。接続失敗や途中切断は自動で再送しません。
- 利用枠の上限やアカウントの制限では新しい翻訳を停止します。ChatGPTの使用量設定を確認してください。認証更新で無効な認可と判明した場合を除き、失敗しても認証情報を保持します。
- Hoverの位置と幅はZedが決めます。上側への固定や、本文を完全に隠さない配置は保証しません。

## 開発

両crateはRust Edition 2024です。native言語サーバーにはRust 1.97以降、WASM拡張にはRust 1.96以降が必要です。古い最低対応Rustの維持より依存更新を優先し、必要に応じて対象crateの要件を引き上げます。virtual workspaceにはresolver 3を使います。`Cargo.lock` をバージョン管理し、検証とreleaseビルドは `--locked` で実行してください。

```sh
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

[CIワークフロー](.github/workflows/ci.yml)はWindows・macOS・Linuxそれぞれでworkspaceテスト、fmt、Clippy、nativeとWASMのreleaseビルドを実行します。1 OSでの成功だけを他OSの成功とは扱いません。リリース前には3 OSのCI結果に加え、各OSのZedで拡張導入・native起動・認証・コメント翻訳・文書翻訳の表示と保存を確認してください。HTTP/OIDCのfixtureは合成の認証情報を使うため、ChatGPTアカウントは不要です。

依存は通常のCargo互換範囲を `major.minor` で指定します（`serde` と `serde_json` も `1.0`）。`cargo update` は意図した更新時に実行し、lockfileの変更を確認して、上記の検証とnative/WASMのreleaseビルドを繰り返してください。Oxcのparser・allocator・spanは同じリリース系列に揃えます。`zed_extension_api` を更新する場合は、WASMビルドに加えて対象ZedのAPI対応を確認してください。HTTP/OIDCテストはローカルのfixtureと合成の認証情報を使います。FixtureごとにOS乱数からEd25519署名鍵を生成し、メモリで保持して、JWT署名とJWKSに同じ鍵を使います。

Rust lexer（`ra-ap-rustc_lexer` 0.176）はUnicodeテーブルの版が一致している必要があります。nativeサーバーのmanifestで `unicode-ident` を `=1.0.24`（Unicode 17）に固定し、`unicode-properties` 0.1.4と揃えています。1.0.25・1.0.26はUnicode 18で、lexerのコンパイル時検査に失敗します。lexerと両テーブルcrateが同じUnicode版に対応した時点で、この互換性のための固定を外し、まとめて更新してください。

OIDC SDKは引き続き `ed25519-dalek` 2.2と `rand_core` 0.6に依存します。Fixtureの鍵生成には `ed25519-dalek` 3.0と `getrandom` 0.4を使い、SDKを変更せずに両系列を併用します。SDKが使う旧暗号ライブラリ系列は、現在Cargoが `crypto-common` 0.1.6と `generic-array` 0.14.9へ解決します。[0.1.7の変更](https://github.com/RustCrypto/traits/pull/2088)は、非推奨警告を抑えるために `generic-array` を0.14.7へ固定するものだけです。本プロジェクトではそのダウングレードを強制しません。

C#のコメント抽出にはTree-sitter 0.27と `tree-sitter-c-sharp` 0.23を使います。新しいコード言語を追加する場合も、Tree-sitterの文法を第一候補として検証します。既存のRust用lexer、JavaScript／TypeScript用Oxcは維持しています。C#の特殊なUnicode改行は、解析入力だけをbyte位置が変わらない形で調整し、コメント本文は変更していない原文から取得します。

C/C++の抽出は `tree-sitter-c` 0.24と `tree-sitter-cpp` 0.23を使い、Tree-sitter本体を共用します。raw文字列やバックスラッシュで継続する行コメントは、それぞれの文法で解析します。

Pythonの抽出は `tree-sitter-python` 0.25を使います。三重引用符だけを目印にせず、構文木上の位置からdocstringを判定します。
