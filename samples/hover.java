class Hover {
    /**
     * Sends the request again after a temporary failure.
     *
     * <p>The wait doubles after each failure. See
     * https://example.com/docs/retries for the list of temporary failures.
     *
     * @param retryCount how many times to try again; {@code 0} turns retries off
     * @return the number of attempts that were made
     * @throws IllegalStateException if the client is closed
     */
    int send(int retryCount) {
        /*
         * Steps:
         * - Wait 200 ms before the first retry.
         * - Double the wait after each failure.
         */
        return retryCount;
    }
}
