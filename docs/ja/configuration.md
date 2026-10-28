# 設定

[English](../configuration.md)

このページでは、`lsp.translation-lsp` の設定を説明します。プロバイダーごとの項目は、[ChatGPT](providers/chatgpt.md)、[Anthropic API](providers/anthropic.md)、[OpenAI API](providers/openai-api.md)、[OpenAI互換API](providers/openai-compatible.md) のページにあります。

## 設定の全体例

拡張ID・言語サーバーの設定キー・ネイティブ実行ファイル名は `translation-lsp` です。

既存のZed設定の `lsp` に次の項目を追加します。モデルは、接続するアカウントで使えるものを指定してください。下記のモデルと推論強度は設定例です。

```json
{
  "lsp": {
    "translation-lsp": {
      "initialization_options": {
        "target_language": "ja",
        "source_language": "auto",
        "hover_timeout_ms": 20000,
        "selection_timeout_ms": 30000,
        "document_timeout_ms": 180000,
        "selection_debounce_ms": 450,
        "comment_parse_timeout_ms": 1000,
        "features": {
          "comment_hover": true,
          "automatic_selection": true,
          "selection_action": true,
          "document_action": true
        },
        "translation_limits": {
          "input_kib": {
            "hover": 16,
            "document": 1024
          },
          "output_kib": 4096
        },
        "provider": {
          "name": "chatgpt",
          "model": "gpt-6.1-sol",
          "effort": "low"
        }
      }
    }
  }
}
```

## サーバーのパス

### 探す順序

拡張は、次の順で最初に見つかったサーバーを起動します。

1. `binary.path` に指定した実行ファイル。
2. 作業ツリーのPATHにある `translation-lsp`。
3. 拡張が取得したサーバー。

`binary.arguments` と `binary.env` は、どの方法で見つかったサーバーにも適用します。

### 自動取得とキャッシュ

1と2で見つからない場合、拡張はこのリポジトリの[GitHub Releases](https://github.com/hwtnb/zed-translation-lsp/releases)からサーバーを取得し、拡張の作業ディレクトリに保存します。取得するのは拡張と同じバージョンのサーバーで、最新のリリースとは限りません。2回目以降の起動では保存済みのサーバーを使い、ネットワーク接続は要りません。拡張を更新すると、新しいバージョンのサーバーを取得し、ほかのバージョンのサーバーを削除します。

完了しなかった取得は破棄し、次の起動でもう一度取得します。取得の途中のサーバーを起動することはありません。取得にはZedのHTTPSダウンローダーを使います。拡張は、アーカイブをリリースの `SHA256SUMS` と照合しません。

PATHにあるサーバーは、取得したサーバーより先に使われます。そのため、拡張を更新しても置き換わりません。

### 手動での導入

GitHubに接続できない場合、お使いの環境向けのサーバーが配布されていない場合、自分でビルドしたサーバーを使う場合は、`binary.path` を指定します。サーバーは、[サーバーをソースからビルドする](../../README.ja.md#サーバーをソースからビルドする)の手順でビルドするか、拡張と同じバージョンのリリースからお使いの環境向けのアーカイブを取得して展開します。取得したアーカイブを確かめるには、そのSHA-256を、リリースの `SHA256SUMS` にある同じアーカイブの行と比べます。Linuxでは `sha256sum`、macOSでは `shasum -a 256`、PowerShellでは `Get-FileHash -Algorithm SHA256` を使います。アーカイブに入っているライセンスファイルは、実行ファイルと同じ場所に残してください。

`binary.path` は、言語サーバーを動かすOSの絶対パスにします。Windowsでは `.exe` まで指定してください。

| OS | `binary.path` の例 |
| --- | --- |
| Windows | `C:/Users/you/src/translation-lsp/target/release/translation-lsp.exe` |
| macOS | `/Users/you/src/translation-lsp/target/release/translation-lsp` |
| Linux | `/home/you/src/translation-lsp/target/release/translation-lsp` |

WindowsのパスもJSON内では `/` で区切れます。バックスラッシュを使う場合は1文字ごとに `\\` と記述してください。空白を含むパスでも、値の中にシェル用の引用符を追加しません。

### 言語サーバーの一覧

対応言語の `language_servers` を明示している場合は、既存の配列へ `"translation-lsp"` を追加してください。言語ごとの設定は[言語サーバーの設定](languages.md#言語サーバーの設定)にあります。

特定の言語でこの拡張を止めるには、その言語の `language_servers` に `"!translation-lsp"` を書き、続けて `"..."` を書いてほかのサーバーを残します。

```json
{
  "languages": {
    "YAML": {
      "language_servers": ["!translation-lsp", "..."]
    }
  }
}
```

その言語では、ホバーの翻訳もコードアクションも出なくなります。一部の機能だけを止める場合は、[`features`](#機能の切り替え) を使います。

## 翻訳する言語

画面上の表示は `target_language` にかかわらず英語です。`target_language` の既定値は `ja`、`source_language` は `auto` です。`auto` では、翻訳する同じ要求の中で原語を判定します。

ISO 639の言語コードの値は、単独でも `zh-Hans` や `pt-BR` のように続きがあっても、`Japanese (ja)` のように英語の言語名を付けて、コードアクションの表示とプロバイダーへの指示の両方で使います。`Japanese` のようなそれ以外の値は、書いたとおりに使います。

## 機能の切り替え

`features` は、翻訳を始める機能を、すべての言語または言語ごとに切り替えます。値はすべて真偽値です。

| `features` 内の設定 | 切り替える機能 | 言語ID |
| --- | --- | --- |
| `comment_hover` | コメントのホバーで翻訳を始める。 | `rust`、`javascript`、`typescript`、`typescriptreact`、`csharp`、`c`、`cpp`、`python`、`go`、`java`、`kotlin`、`swift`、`ruby`、`php`、`html`、`css`、`sql`、`json`、`jsonc`、`yaml`、`toml`、`shellscript`、`powershell`、`dockerfile`、`dockercompose` |
| `automatic_selection` | 安定したMarkdownの選択から翻訳を始める。 | `markdown` |
| `selection_action` | コードアクションの `Translate selection to …`。 | 下の表のすべての言語ID |
| `document_action` | コードアクションの `Translate selection to … and open in tab` と `Translate document to … and open in tab`。 | `markdown`、`plaintext` |

`features`、その中の各設定、`languages`、`languages` の中の各言語、言語の中の各設定は、どれも省略できます。ある言語の機能には、`languages.<言語ID>` の値を使います。その値がなければ `features` の直下の値を使い、それもなければ有効です。

次の例は、タブに開く翻訳をすべての言語で止め、コメントのホバーをYAMLとTOMLで止めます。

```json
"features": {
  "comment_hover": true,
  "automatic_selection": true,
  "selection_action": true,
  "document_action": false,
  "languages": {
    "yaml": { "comment_hover": false },
    "toml": { "comment_hover": false }
  }
}
```

`comment_hover` を止めた言語では、コメントにホバーしても翻訳を始めません。`Translate selection to …` で始めた翻訳は、ホバーに表示します。`selection_action` または `document_action` を止めた言語では、そのコードアクションを一覧に出しません。取り消し、再試行、アカウントメニューのコードアクションは変わりません。

`languages` のキーは次の言語IDで、小文字で書きます。

| Zedの言語 | 言語ID |
| --- | --- |
| Rust | `rust` |
| JavaScript（`.jsx` のファイルを含む） | `javascript` |
| TypeScript | `typescript` |
| TSX（`.tsx` のファイル） | `typescriptreact` |
| CSharp | `csharp` |
| C | `c` |
| C++ | `cpp` |
| Python | `python` |
| Go | `go` |
| Java | `java` |
| Kotlin | `kotlin` |
| Swift | `swift` |
| Ruby | `ruby` |
| PHP | `php` |
| HTML | `html` |
| CSS | `css` |
| SQL | `sql` |
| JSON | `json` |
| JSONC | `jsonc` |
| YAML | `yaml` |
| TOML | `toml` |
| Shell Script | `shellscript` |
| PowerShell | `powershell` |
| Dockerfile | `dockerfile` |
| Docker Compose | `dockercompose` |
| Markdown | `markdown` |
| Plain Text | `plaintext` |

`javascriptreact` など、これ以外のキーは設定エラーです。`languages.rust.document_action` のように、機能をその言語IDにない言語へ書いた場合も設定エラーです。

## タイミング

翻訳のタイミング設定は省略できます。

| 設定 | 既定値 | 指定できる値 | 動作 |
| --- | --- | --- | --- |
| `hover_timeout_ms` | `20000` | 整数 `1000`〜`120000`、両端を含む | ホバー要求ごとの受信時からの応答期限。キュー待ちを含む。 |
| `selection_timeout_ms` | `30000` | 整数 `1000`〜`120000`、両端を含む | 保持した手動選択・Markdownバックグラウンド翻訳の実行期限。ワーカー（翻訳処理を実行するタスク）へ渡した時点から計る。 |
| `document_timeout_ms` | `180000` | 整数 `1000`〜`600000`、両端を含む | 別タブに開く文書・選択範囲の実行期限。ワーカーへ渡した時点から計り、キュー待ちは別。 |
| `selection_debounce_ms` | `450` | 整数 `100`〜`2000`、両端を含む | ネイティブサーバーが新しい選択を観測してから待つ時間。Zed側の観測待ちは別に加わる。 |
| `comment_parse_timeout_ms` | `1000` | 整数 `100`〜`60000`、両端を含む | C、C++、C#、Pythonの文書からコメントを読み取る時間の上限。読み取りを始めた時点から計る。[コメントを読み取れなかった文書](languages.md#コメントを読み取れなかった文書)を参照。 |

## サイズの上限

翻訳量の上限は `initialization_options.translation_limits` にまとめます。値は正の整数で、単位はKiB（1 KiB = UTF-8の1024バイト）です。文字数やトークン数ではありません。各階層のオブジェクトや項目を省略すると、その項目の既定値を補います。

| `translation_limits` 内の設定 | 既定値 | 対象 |
| --- | --- | --- |
| `input_kib.hover` | `16` | 新たに開始するコメントのホバー、Markdownバックグラウンド翻訳、ホバーに表示する手動選択の翻訳。 |
| `input_kib.document` | `1024` | 別タブに開く全文・選択範囲の翻訳。 |
| `output_kib` | `4096` | 表示先によらず、完成した訳文。 |

例えば `"translation_limits": {"input_kib": {"document": 2048}}` とすると、文書入力の上限だけを2 MiBに変更します。ゼロ・負数・小数・null・未知の項目・数値以外は設定エラーになります。サーバーの実行環境で表現できない大きさも拒否します。変更後は言語サーバーを再起動してください。

明示実行で入力上限を超えると、実際のサイズ・上限・設定名を表示します。上限を超える自動選択は翻訳を開始しません。既に翻訳済みのキャッシュは、新しい翻訳要求を送らずに閲覧できます。

## 受付とキャッシュの上限

受付とキャッシュの上限も `translation_limits` の中で調整できます。

```json
"translation_limits": {
  "work": { "admitted": 8, "concurrent": 2, "documents": 1, "automatic": 3, "hover_waiters": 16 },
  "cache": { "entries": 64, "capacity_kib": 16384 }
}
```

`admitted` は実行・待機の合計で、取り消しを指示したワーカーも終了通知までは数えます。`concurrent` は全翻訳の同時実行数、`documents` はタブ翻訳として実行枠を割り当てる同時処理数、`automatic` は手動・タブ翻訳の依頼がなく、コメントのホバーや自動選択によって保持する処理数、`hover_waiters` は個々のホバー応答待ちの上限です。件数は正の整数とし、`documents <= concurrent <= admitted` と `automatic < admitted` を満たす必要があります。明示依頼の受付余地を残す制約であり、実行枠を常に空けておくものではありません。同時実行数を増やすと翻訳プロバイダー（翻訳サービスやモック実装）への同時要求も増えます。

`cache.entries` は成功・失敗・タブ用を合わせたすべての文書の項目数です。`cache.capacity_kib` は保存する文章の合計容量で、プロセス全体のメモリ上限ではありません。1件の最大結果を格納できるよう、実効容量はこの値と `output_kib` の大きい方になります。メタデータ、原文、処理中の入力、一時ファイルはこの文章容量に含めません。既定値は連続して読む用途の出発点であり、翻訳プロバイダーや機器の測定上限ではありません。省略した入れ子の項目は既定値を使い、変更後は言語サーバーを再起動してください。

## 設定できる最大値

次の値が**設定できる最大値**です。上記の既定値とは別であり、超過する設定は黙って切り下げず、起動時に拒否します。

| `translation_limits` 内の設定 | 最大値 |
| --- | --- |
| `input_kib.hover` | `1024`（1 MiB） |
| `input_kib.document`、`output_kib` | 各 `16384`（16 MiB） |
| `work.admitted` | `64` |
| `work.concurrent`、`work.documents` | 各 `8` |
| `work.automatic` | `63`、かつ `admitted` 未満 |
| `work.hover_waiters` | `256` |
| `cache.entries` | `4096` |
| `cache.capacity_kib` | `262144`（256 MiB） |

組み合わせにも制限があります。`max(input_kib.hover, input_kib.document) × work.admitted` は `262144` KiB（256 MiB）以下、`output_kib × work.concurrent` は `65536` KiB（64 MiB）以下にします。例えば文書入力16 MiB・受付64件は拒否され、この入力上限では受付16件までです。出力16 MiBなら同時実行は4件までです。現在キューが空でも、設定上の最大負荷を検証します。

これらはサーバー側の資源保護の基準であり、機器の実測限界や翻訳プロバイダーが処理できる量を表すものではありません。開いている原文、パーサー、メモリ管理、通信バッファ、一時ファイルなどの費用もあるため、**プロセス全体のメモリ保証値ではありません**。これを超える上限にはサーバーの変更が必要で、Zedの設定では引き上げられません。基準を決めた理由は[サイズの上限の基準](development.md#サイズの上限の基準)で説明します。

設定値を増やしても、モデルの入力・出力上限や翻訳期限は増えません。長文も一つの要求で送信し、自動分割は行いません。失敗・未完了・上限超過の結果を完成訳として表示・保存しません。

## 設定エラーと再起動

未知の項目、型の違う値、範囲外の数値は、英語の設定エラーを返して言語サーバーの起動を止めます。項目を省略すると既定値を使います。`provider` がないと、言語サーバーは何も翻訳せず、[プロバイダーの設定](../../README.ja.md#プロバイダーの設定)のとおりプロバイダーを選ぶよう求めます。

設定を変更したら言語サーバーを再起動してください。設定変更は、実行中のサーバーには反映されません。[Zedの言語サーバー設定](https://zed.dev/docs/configuring-languages#configuring-language-servers)に従います。

## 翻訳モデルを変更する

1. Zedのコマンドパレットで `zed: open settings file` を実行します。
2. `lsp.translation-lsp.initialization_options.provider.model` を、使いたいモデルの識別名に変更します。
3. 必要に応じて、プロバイダーのほかの項目を指定します。項目は [ChatGPT](providers/chatgpt.md#設定項目)、[Anthropic API](providers/anthropic.md#設定項目)、[OpenAI API](providers/openai-api.md#設定項目)、[OpenAI互換API](providers/openai-compatible.md#設定項目) のページにあります。
4. 設定を保存します。この拡張を使う文書で、コマンドパレットから `editor: restart language server` を実行します。

拡張は設定したモデルの識別名をそのまま翻訳APIへ送り、別のモデルへ自動で切り替えません。そのモデル・推論強度・速度をアカウントで使えるかどうかは、APIが判断します。

翻訳には通常のテキスト出力を使います。ChatGPTアカウント、APIキー、またはサービスで利用可能なモデルを選んでください。要求が失敗しても、拡張が別モデルへ切り替えることはありません。

### タブ翻訳だけ別のモデルを使う

タブに開く翻訳だけ別のモデルを使うには、`provider.document` を追加します。`Translate selection to … and open in tab` と `Translate document to … and open in tab` は、そこに書いたモデルと設定で送ります。コメントのホバー、自動選択、手動の選択翻訳、再試行は、外側の `provider` の値を使い続けます。

`provider.document.model` は必須です。`provider.document` のほかの値は省略でき、外側の設定を引き継ぎません。省略した値は、外側に何を書いていても、その項目の既定値になります。

`provider.document` で別のプロバイダーを選ぶことはできません。`provider.document` を設定すると、[受付と実行枠](behavior.md#受付と実行枠)のとおり、タブ翻訳と範囲翻訳を別の処理として扱います。

`provider.document` の中に書ける値と設定例は、各プロバイダーのページにあります。[ChatGPT](providers/chatgpt.md#タブ翻訳だけ別のモデルを使う)、[Anthropic API](providers/anthropic.md#タブ翻訳だけ別のモデルを使う)、[OpenAI API](providers/openai-api.md#タブ翻訳だけ別のモデルを使う)、[OpenAI互換API](providers/openai-compatible.md#タブ翻訳だけ別のモデルを使う) を参照してください。
