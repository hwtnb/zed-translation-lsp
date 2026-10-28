# 開発

[English](../development.md)

このページでは、必要なRustの版、実行する検証、モックのプロバイダー、CI、サイズの上限の基準、依存関係の方針、依存ライブラリのライセンス表示を説明します。

## 検証

各クレートはRust Edition 2024です。ネイティブ言語サーバーと `xtask` にはRust 1.97以降が必要です。WASM拡張は `rust-version` を宣言しません。Cargoは、ワークスペースのメンバーのうち最も低い `rust-version` に合う依存のバージョンを、ワークスペース全体に選びます。そのため、Zedの登録先が使う古いコンパイラーを拡張に宣言すると、ネイティブ言語サーバーの依存まで古いバージョンになります。拡張がそのコンパイラーでビルドできることは、CIで確かめます。古い最低対応Rustの維持より依存更新を優先し、必要に応じて対象クレートの要件を引き上げます。仮想ワークスペースには依存関係リゾルバーのバージョン3を使います。`Cargo.lock` をバージョン管理し、検証とリリースビルドは `--locked` で実行してください。

```sh
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## モックのプロバイダー

設定で `"provider": { "name": "mock" }` にすると、サーバーはモックのプロバイダーを使います。文章を外部へ送らず、同梱サンプルの決まった日本語訳を返すため、アカウントやAPIキーなしでZedでの流れを確かめられます。

[samples/comment.rs](../../samples/comment.rs) のコメント、または [samples/comment.cs](../../samples/comment.cs) の通常コメント・XMLドキュメントコメントにマウスポインターを重ねてください。C/C++は [samples/comment.c](../../samples/comment.c) と [samples/comment.cpp](../../samples/comment.cpp) でDoxygenコメントも試せます。

Pythonのコメントとドキュメンテーション文字列は [samples/comment.py](../../samples/comment.py) で試せます。Go、Java、Kotlin、Swift、Ruby、PHP、SQLのコメントは [samples/comment.go](../../samples/comment.go)、[samples/comment.java](../../samples/comment.java)、[samples/comment.kt](../../samples/comment.kt)、[samples/comment.swift](../../samples/comment.swift)、[samples/comment.rb](../../samples/comment.rb)、[samples/comment.php](../../samples/comment.php)、[samples/comment.sql](../../samples/comment.sql) で試せます。[samples/comment.html](../../samples/comment.html) にはHTMLのコメントと、スクリプトとスタイルシートの中のコメントがあり、[samples/comment.css](../../samples/comment.css) にはCSSのコメントがあります。JSON、YAML、TOMLのコメントは [samples/comment.jsonc](../../samples/comment.jsonc)、[samples/comment.yaml](../../samples/comment.yaml)、[samples/comment.toml](../../samples/comment.toml) で試せます。シェルスクリプト、PowerShell、Dockerfile、Docker Composeのコメントは [samples/comment.sh](../../samples/comment.sh)、[samples/comment.ps1](../../samples/comment.ps1)、[samples/Dockerfile](../../samples/Dockerfile)、[samples/compose.yaml](../../samples/compose.yaml) で試せます。[samples/paragraph.md](../../samples/paragraph.md) では段落を選択し、コードアクションの `Translate selection to Japanese (ja)` を実行してから、選択範囲にマウスポインターを重ねます。辞書の項目を見るには、同じサンプルで `retry_count` だけを選択して、同じ操作を実行します。モックのプロバイダーでは、Markdownは手動実行が必要です。

モックのプロバイダーはこれらのサンプル、その改行の違い、語 `retry_count` だけに対応します。ほかの本文や日本語以外の訳先ではエラーを返します。ホバーの見出しには `Mock translation (fixed sample)` と表示します。

stdioのテストは、実行ファイルを `--mock-delay-ms <ミリ秒>`、`--mock-fail`、`--mock-timeout-ms <ミリ秒>` 付きで起動し、モックの翻訳を遅らせる、失敗させる、または期限を指定の値にします。これらの引数はモックのプロバイダーにだけ効きます。

## CIとリリース前の確認

[CIワークフロー](../../.github/workflows/ci.yml)はWindows・macOS・Linuxそれぞれでワークスペース全体のテスト、fmt、Clippy、ネイティブとWASMのリリースビルドを実行します。1 OSでの成功だけを他OSの成功とは扱いません。リリース前には3 OSのCI結果に加え、各OSのZedで拡張導入・ネイティブ起動・認証・コメント翻訳・文書翻訳の表示と保存を確認してください。HTTP/OIDCのテスト用環境は合成の認証情報を使うため、ChatGPTアカウントもAnthropicのAPIキーも不要です。テストは実際のAPIを呼ばず、利用者の認証情報のディレクトリも読みません。言語サーバーをプロセスとして起動するテストはモックのプロバイダーだけを起動するか、プロバイダーなしで起動し、ChatGPTとAnthropicのプロバイダーのテストは、テスト用のディレクトリを使います。同じ実行ファイルを `anthropic-key` 付きで実行すると、エディターとは通信せず、端末でAnthropicのAPIキーを尋ねて保存します。この形で実行するテストは、テスト用のホームディレクトリと擬似端末を与え、macOSとLinuxでだけ動きます。Windowsは、認証情報のディレクトリを環境変数ではなくOSから得るためです。`unsafe` なコードは、ワークスペースのlint `unsafe_code` がすべてのクレートで拒否します。例外はこのテストの補助コードだけで、必要な関数ごとに、理由を付けて許可しています。

CIでは、テストを1つずつ実行します（`RUST_TEST_THREADS` が `1`）。時間を測るテストが、ほかのテストとランナーのCPUを取り合わないようにするためです。

Zedの登録先は、拡張を独自のコンパイラー（現在はRust 1.90）でビルドします。CIの別のジョブが、WASM拡張をそのコンパイラーでビルドします。拡張の依存が新しいコンパイラーを必要とするようになったら、登録先との互換性をどう保つかを改めて決めてください。

開発専用の `xtask` クレートは、ネイティブ言語サーバーを梱包し、リリースの元になる情報を検査します。同じジョブが次の2つのコマンドを実行します。

```sh
cargo run -p xtask --locked -- check-source
cargo run -p xtask --locked -- check-notices
```

`check-source` は、バージョン、ライセンス、配布対象が互いに合っていることを検査します。`check-notices` は、[依存ライブラリのライセンス表示](#依存ライブラリのライセンス表示)で説明する文書を生成します。梱包用のコマンドは `cargo run -p xtask --locked -- --help` で一覧できます。

拡張は、自分と同じバージョンのGitHubリリースからサーバーを取得します。[配布物を用意するワークフロー](../../.github/workflows/release-assets.yml)は、配布対象ごとに梱包用のコマンドを実行し、`draft-release` を有効にして起動すると、6本のアーカイブと `SHA256SUMS` を付けた下書きのリリースを作ります。そのため、そのリリースを公開してから、そのバージョンをZedに登録してください。

## サイズの上限の基準

[サイズの上限](configuration.md#サイズの上限)にある `translation_limits` の基準は、設定上の最大負荷が使うメモリを抑えるためのものです。入力の合計容量で受付済みの原文を制限します。出力の上限は入力より小さくしています。訳文を受け取る間は、エスケープされたSSEイベントと、訳文の複数のスナップショットも保持するためです。入力の個別上限は、大きな文書を許容しつつ、ホバーと選択範囲の入力を小さく保ちます。件数とキャッシュ項目数の上限は、小さな文章が大量にある場合のスケジューラとキャッシュの走査も制限します。通信側の制限は、JSONのエスケープ・訳文の重複送信・SSEイベントの付加情報を考慮して `output_kib` に連動します。訳文のサイズとは別に通信処理量を制限し、最大量のメモリを先に確保することはありません。

## 依存関係

依存関係のバージョンは `major.minor` のCargo互換範囲で指定します。特定のバージョンに固定する場合は、互換性上の理由を記録してください。更新時は `cargo update` によるロックファイルの差分を確認し、上記の検証とネイティブ・WASM両方のリリースビルドを実行してください。Oxcの関連クレートは同じリリース系列に揃えます。`zed_extension_api` の更新時は、対象ZedのAPI対応も確認してください。

新しいコード言語のコメント抽出では、Tree-sitterの文法とサーバー自身の読み取りを比べます。実際のファイルと生成したテキストで、それぞれが見つけるコメントをその言語自身の字句解析器と比べ、時間も比べます。RubyとPHPは、サーバー自身の読み取りを使います。この比較で、文法が言語の字句解析器と食い違う箇所でも一致し、時間は10分の1以下でした。Java、Kotlin、Swiftも同じ理由で、サーバー自身の読み取りを使います。実際のファイルで、KotlinとSwiftの文法が食い違う箇所でも、javac、Kotlinの字句解析器、Swiftのコンパイラーと一致し、時間は約50分の1でした。Go、HTML、CSSも、コメントが字句解析の少数の規則から決まるため、サーバー自身の読み取りを使います。SQLには、方言に共通する字句解析器がありません。SQLの読み取りはZedのSQL文法のトークンの規則に従い、関数本体の中のコメントを見つけるため、`$$` で囲んだ部分をSQLとして読みます。既存のRust用字句解析器とJavaScript／TypeScript用Oxcは継続して使用します。

読み取りの基準は、その言語自身の字句解析器です。実際のファイルやよく使われる書き方で字句解析器と食い違う場合、パニックする場合、かかる時間がテキストの大きさより速く増える場合は、読み取りを直します。そのために作ったテキストでだけ食い違い、その結果がコメントを対象から外すか、リテラルの一部をコメントとして扱うだけの場合は、読み取りに手を加えず、[対応言語](languages.md)のその言語の節に書きます。

Go、Java、Kotlin、Swift、Ruby、PHP、HTML、CSS、SQL、JSON、TOML、シェルスクリプト、PowerShell、Dockerfileのコメントは、サーバー自身の読み取りで読みます。YAMLは `granit-parser` のスキャナーで読みます。HTML文書のスクリプトはOxcで読みます。文書のコメントを読む間、要求を処理するスレッドは待つため、内容にかかわらず、読み取りにかかる時間はテキストの大きさに比例する必要があります。読み取りは、256 MiBのスタックを持つ専用のスレッドで動きます。Oxcは入れ子を再帰で読み、深さの上限を持たないためです（[oxc#21953](https://github.com/oxc-project/oxc/issues/21953)）。スタックは使った部分だけがメモリーを使います。約10万段を超える入れ子では、それでもスタックがあふれます。PowerShellとTOMLの読み取りは、`pwsh-parser` と `toml_parser` の字句解析器を置き換えたものです。これらは、テキストによっては大きさの2乗に比例する時間がかかりました。サーバー自身の読み取りは、各部分を1回だけ読み、前の位置から読み直さず、再帰も使いません。シェル、Dockerfile、Kotlin、Swift、Ruby、PHPの読み取りは、開いているものを自前のスタックに積むため、入れ子の深さを制限するのはメモリーだけです。Rubyの読み取りは、語がローカル変数かどうかを、スコープの深さにかかわらず1つの集合で調べます。テスト `readers_finish_repeated_pieces_in_seconds` と `readers_finish_texts_of_several_repeated_parts_in_seconds` が、短い文字列を繰り返したテキストで、YAMLを含む15の読み取りの時間を確かめます。読み取りを追加するときは、両方のテストに加えてください。これらのテストがすべてのテキストに届くわけではないので、読み取りを変えるときは、現在の文字より前や先を読む箇所を一つずつ確かめてください。

現在のバージョン固定の例外は `unicode-ident = "=1.0.24"` です。Rustの字句解析器とUnicodeテーブルの版を揃えるために固定しています。字句解析器と関連するUnicodeテーブルが同じ版に対応した時点で、固定を外してまとめて更新してください。

## 依存ライブラリのライセンス表示

サーバーの各リリースアーカイブには `THIRD_PARTY_LICENSES.txt` が入っています。内容は、その配布対象でサーバーが依存するライブラリのライセンスと、それぞれのソースの取得先です。`xtask` は梱包のときにこのファイルを生成し、リポジトリには置きません。生成には、[cargo-about](https://github.com/EmbarkStudios/cargo-about) 0.9.2を、`about.toml` とテンプレート `xtask/notices.hbs` で実行します。このバージョンは次のコマンドで導入します。

```sh
cargo install --locked --features cli cargo-about@0.9.2
```

`about.toml` には、受け入れるライセンスを列挙しています。そのどれも選べない依存があると、`check-notices` と梱包が失敗します。ライセンスを確認してから、一覧に追加してください。

`THIRD_PARTY_LICENSES.txt` には、各ライセンスを、クレートの作者が書いた文面で、著作権表示とともに載せます。cargo-aboutがクレートのライセンスファイルを自分で見つけられない場合は、`about.toml` にそのファイルとSHA-256チェックサムを書いています。ライセンスファイルを同梱していないクレートについては、GitHub上のそのクレートのリポジトリにあるファイルを指定しています。cargo-aboutは、生成のたびに、クレートが公開された時点のコミットからそのファイルを取得します。そのため、生成にはネットワーク接続が必要です。

チェックサムが合わないときと取得に失敗したとき、cargo-aboutは著作権者の名前がない標準の文面を使い、成功として終了します。この場合、`xtask` は該当するクレートの名前を挙げて失敗します。取得に失敗したときは、その前にcargo-aboutを合計3回まで実行し直します。標準の文面のままにしてよいクレートは、理由とともに `xtask/src/notices.rs` の `STANDARD_TEXT` に列挙しており、`THIRD_PARTY_LICENSES.txt` ではその文面の前にその旨を書きます。

cargo-aboutは、`about.toml` にライセンスを書いたクレートについて、クレートが宣言するライセンスの代わりにその記述を使います。チェックサムが確かめるのは、そこに指定したファイルだけです。そのため `xtask` は、クレートの宣言が `about.toml` の記述と異なるときにも失敗します。意図して宣言と異なる記述にしているクレートは、その記述を書いたときの宣言とともに `xtask/src/notices.rs` の `DECLARED` に列挙しています。

依存を更新したあとに `check-notices` が失敗したら、`about.toml` を変える前に、名前が挙がったクレートのライセンスファイルを読んでください。ファイルが変わっていた場合は、変更の内容を確認してから、チェックサムを新しいファイルのSHA-256チェックサムに更新します。`about.toml` にそのクレートがない場合は、ライセンスファイルを指定する項目を追加します。クレートの宣言が変わっていた場合は、クレートに追加されたライセンスファイルがないかも確認してから、`about.toml` の項目を更新します。意図して宣言と異なる記述にしている項目では、`DECLARED` も更新します。クレートのファイルは、Cargoのレジストリのソースディレクトリにあります。リポジトリのファイルは `https://raw.githubusercontent.com/<owner>/<repository>/<commit>/<path>` にあり、コミットはクレートの `.cargo_vcs_info.json` に記録されています。`ra-ap-rustc_lexer` はそこにコミットを記録していないので、クレートの説明文に書かれたコミットを `override-git-commit` に写してください。

cargo-aboutのバージョンを変えるときは、`xtask/src/notices.rs` と2つのワークフローの両方を変えてください。
