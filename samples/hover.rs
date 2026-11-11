/// Sends the request again after a temporary failure.
///
/// # Arguments
///
/// - `retry_count`: how many times to try again. Use `0` to turn retries off.
/// - `--verbose` on the command line prints each attempt.
///
/// # Examples
///
/// ```
/// let response = send(3);
/// ```
///
/// See https://example.com/docs/retries for the list of temporary failures.
/// The Python client names the same setting `__init__(retry_count=3)`.
fn send(retry_count: u32) -> u32 {
    // 1. Wait 200 ms before the first retry.
    // 2. Double the wait after each failure.
    // 3. Give up when `retry_count` reaches 0.
    //
    // | Attempt | Wait   |
    // | ------- | ------ |
    // | 1       | 200 ms |
    // | 2       | 400 ms |
    retry_count
}

/// Prints what the upload would send. Pass `--dry-run` to skip the upload.
///
/// It's safe to call twice: the second call prints "nothing to do". Lines
/// longer than 80 columns are cut and end with three dots...
fn report() {
    println!("nothing to do");
}

fn main() {
    report();
    //   - An indented item of a list.
    //   - Another item, with *emphasis* and ~~text struck out~~.
    println!("{}", send(3));
}
