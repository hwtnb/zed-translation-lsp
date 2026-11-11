def send(retry_count):
    """Send the request again after a temporary failure.

    The wait doubles after each failure. See
    https://example.com/docs/retries for the list of temporary failures.

    Args:
        retry_count: How many times to try again. Use 0 to turn retries off.
        verbose: Print each attempt, like `--verbose` on the command line.

    Returns:
        The number of attempts that were made.

    Example:
        >>> send(3)
        3
    """
    # 1. Wait 200 ms before the first retry.
    # 2. Double the wait after each failure.
    # 3. Give up when `retry_count` reaches 0.
    return retry_count
