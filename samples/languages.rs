/// Retries the request when the remote service reports a temporary failure.
fn english() {}

/// リモートサービスが一時的な障害を報告したときに、要求を再試行する。
fn japanese() {}

/// `retry_count` が `max_retries` を超えたら、`Error::Timeout` を返す。
/// 詳しくは https://example.com/docs/retries を参照。
fn japanese_with_code() {}

/// 接続が切れたときは、一度だけ再試行する。
/// Otherwise, return the response as it is.
fn half_and_half() {}

/// 再試行回数上限超過時即時終了。
fn kanji() {}

/// 当远程服务报告临时故障时，重试该请求。
fn chinese() {}

/// 要求を送る前に、接続が開いているかを確かめる。
/// 接続が閉じていれば、新しく開く。
/// 開けなかったときは、一秒待ってから、もう一度だけ試す。
/// The second attempt uses a longer timeout.
/// 二回とも失敗したら、最後のエラーを返す。
/// 成功したら、使った接続をプールに戻す。
fn many_sentences() {}

/// Поново шаље захтев када удаљени сервис пријави привремени квар.
fn serbian_cyrillic() {}
