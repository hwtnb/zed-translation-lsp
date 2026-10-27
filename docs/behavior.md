# Behavior

[日本語](ja/behavior.md)

This page describes the rules the language server follows: how much work it accepts, what keeps accepted work running, how it tracks automatic selections, what Hover returns, what it caches, and how deadlines, retries, and pauses apply. The operations themselves are described under [Usage](usage.md).

## Admission and execution slots

By default, up to two translations run at once and eight are accepted across all open documents. Multiple ranges in the same document can be accepted. Exact duplicate targets share one operation. At most one tab translation starts at a time, leaving capacity for range translations. Accepted work keeps its queue order, except that range work may bypass a tab waiting for the tab limit.

A tab request for a range already accepted shares that operation and does not acquire another slot. That sharing applies without `provider.document`. With the Anthropic API, OpenAI API, and OpenAI-compatible providers, a range request asks for fewer output tokens than a tab request (see What is sent on the [Anthropic API](providers/anthropic.md#what-is-sent), [OpenAI API](providers/openai-api.md#what-is-sent), and [OpenAI-compatible API](providers/openai-compatible.md#what-is-sent) pages), so a tab shares a range operation only until its request is sent, and the shared request then asks for the tab's limit; a tab requested after that is a separate operation. With `provider.document`, a tab translation and a range translation of the same range are separate operations: neither joins the other, each counts toward the accepted limit, and each takes its own slot.

Configure these limits below `translation_limits.work`.

## What keeps accepted work running

Selection and tab commands acknowledge acceptance immediately. Changing or clearing a selection, switching tabs, or canceling that command's LSP response does not cancel accepted work. A Hover response can be canceled or time out independently, without canceling an accepted comment translation. Moving from comment A to B keeps A running or queued; return to either comment to read its completed result. Merely passing over a comment can therefore consume provider usage once Zed sends a Hover request.

Editing anywhere in the source document, closing it, changing authorization, or shutting down the server invalidates affected work and cached results. Token refresh does not invalidate them. With the providers that take an API key, saving, replacing, or removing the key changes authorization. Opening and dismissing the account or key menu of a provider leaves translations running.

## Automatic selection

A stable automatic selection that overlaps the immediately preceding selection replaces only that previous operation's automatic ownership. Empty or ineligible selections do not break this adjustment chain. Observing another document or a non-overlapping eligible selection does. Explicit requests and other Hover waiters keep shared work alive.

Comment Hover and automatic selection share the default limit of three automatic operations. A comment Hover rejected at capacity is not retained; return to it when capacity is available or run a manual selection translation. For automatic selection, when capacity is full, only the latest unaccepted candidate remains: selecting D then E may leave D untranslated. The candidate is labeled `selection not yet accepted` and is not included in the accepted queue count. An explicit request rejected for capacity reports the file name and line range and states that it was not accepted.

Selection changes are observed through Zed range requests. One server tracks the latest candidate and adjustment chain, not independent selections in separate editors. Previously accepted distinct ranges remain. Copying selected prose can still trigger automatic translation. A failed background operation is not automatically submitted again.

Code Action requests restricted to unrelated kinds do not change automatic selection tracking.

## What Hover returns

Hover first returns the newest completed translation covering the position. A newer pending or failed request never hides a readable completed result. Without a completed result, Hover joins a pending range, or displays its recorded failure with a manual retry instruction. It can find previously requested ranges after deselection, including manual selections in code files.

## Cache

The server caches multiple ranges in memory, bounded across the server by `translation_limits.cache`. The most recently requested successful range covering the Hover position wins, even if requests finish out of order. A new successful result removes older results that it fully contains. Other entries are evicted in least-recently-used order when count or capacity is exceeded.

Tab results are separate from Hover results, with at most one successful tab result per source document, and share the global cache budget. An explicit translation request without a cached result for its output can reuse the other output's completed translation of exactly the same range, subject to the requested input limit. Opening a tab alone does not make its result visible in Hover; explicitly translate that range for Hover to reuse it. With `provider.document`, neither output reuses the other's result: each request is translated with its own settings.

Retry actions always request a new translation or join one already in progress. Reopening the same cached tab result reuses its temporary file unless it was deleted or its saved contents changed.

After eviction, Markdown Hover does not automatically translate the range again; select it or run the manual action. Comment Hover can start a new translation. Editing or closing the source document, changing accounts, or reconnecting removes affected entries. Hover does not store text on disk; opening a translation tab writes the completed result to a temporary file. Failed, interrupted, canceled, and expired work never publishes a partial translation. A retained range failure is shown as an error, not as translated text.

## Deadlines

Each Hover response waits up to 20 seconds from receipt, including queue time. Manual and automatic selection work has a 30-second execution budget and tab work has 180 seconds, starting when dispatched to a worker. Retained comment translations use `hover_timeout_ms`, with a default 20-second execution cap starting at dispatch, independently of the Hover response deadline. Repeated joins never restart execution time; adding a longer-lived owner can increase the budget from the original start. API calls, credential refresh, retries, and streaming use that budget. Queue time is additional for retained work, and admission and concurrency limits bound the number of operations ahead of it. OAuth and management operations have separate internal limits.

## Automatic retries

An explicit temporary service error, including an overload reported after HTTP 200, is retried at most once within the original deadline. The server waits two seconds by default, or uses a numeric `Retry-After` of up to 120 seconds. Other `Retry-After` values leave the error for manual retry. Cancellation, document changes, and account changes also stop a pending retry. After an API key is saved, replaced, or removed, a pending retry is not sent. Connection failures and interrupted streams are not automatically replayed. A retry sends the same model, `effort`, and `speed`; no failure makes the server send a request again without `speed`.

## Pauses

Usage limits, account restrictions, and configuration errors pause automatic translation and return an error. After resolving the cause, explicitly run a selection or document translation to retry; restarting or reconnecting is unnecessary. Automatic translation stays paused until an explicit retry succeeds. Jobs accepted before the stop do not send new HTTP requests; repeated stop notifications are suppressed and affected retained ranges keep their failure state. The providers that take an API key follow the same rules for billing, key, model, and permission errors; see Errors and pauses on the [Anthropic API](providers/anthropic.md#errors-and-pauses), [OpenAI API](providers/openai-api.md#errors-and-pauses), and [OpenAI-compatible API](providers/openai-compatible.md#errors-and-pauses) pages.
