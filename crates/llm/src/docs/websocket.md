# WebSocket transport for the Responses API

`OpenAiProvider` and `CodexProvider` stream every turn over a persistent
WebSocket connection to the Responses resource instead of opening an HTTP/SSE
request per turn. This is unconditional — there is no flag to flip and no
configuration surface; the providers' public API, the
[`StreamingModelProvider`](crate::StreamingModelProvider) trait, and the
`LlmResponse` event sequence are unchanged, so consumers cannot tell the
transport apart. Bedrock Mantle keeps using HTTP/SSE.

## Why a persistent connection

Every agent turn re-sends the full context window over HTTP, so long tool-call
chains pay full serialization plus server-side reprocessing latency on each of
hundreds of round trips. WebSocket mode keeps one connection open and serves
continuation turns from a connection-local response cache: each turn sends
only the new input items plus `previous_response_id`.

## Frame protocol

Client → server frames are `response.create` envelopes: the regular Responses
create body minus transport-only fields (`stream`, `background`), plus an
optional `stream_id` naming the lane and `previous_response_id` when
continuing from an earlier response on that lane. `store` remains `false`, so
the flow is zero-data-retention compatible.

Server → client frames are envelopes too:

- `{"type": "message", "stream_id": …, "message": …}` wraps one regular
  Responses event (`response.created`, `response.output_text.delta`, …,
  `response.completed`), which is decoded by the same event pipeline as SSE.
- `{"type": "error", "status": …, "stream_id": …, "error": …}` carries a
  request-scoped failure, surfaced as a
  [`ProviderError`](crate::ProviderError) with the same classification
  (`rate_limit_exceeded` → rate limit, `server_error`/5xx → server,
  `invalid_stream_id`/`websocket_stream_limit_reached` → API, transport
  failures → stream interrupted).

## Lanes and incremental input

Lanes keep concurrent agents from interleaving state on one connection. A
lane is named by the context's `session_affinity_key`, sent as the protocol
`stream_id`; agents without a key share the implicit default lane. Keys
outside the protocol's `[A-Za-z0-9_.-]{1,256}` alphabet fall back to the
default lane rather than sending something the server would reject.

A turn sends incremental input only when, on that lane:

1. the previous turn completed (its response id is cached),
2. the non-input request parameters are unchanged (model, instructions,
   tools, reasoning settings), and
3. the new input extends the previously sent input as a prefix.

Otherwise — first turn, compaction, model or parameter changes, any earlier
failure — the turn resends the full input window with no
`previous_response_id`. An unchanged input window also resends fully rather
than emitting an empty continuation.

## Fallback and recovery

- **`previous_response_not_found`**: the lane's cached response is gone.
  The client retries once on the same connection with the full input window
  and never surfaces the miss if the retry succeeds.
- **Transport failure / `websocket_connection_limit_reached`**: the server
  cache is connection-local, so reconnecting invalidates every lane's
  `previous_response_id`. The client reconnects and retries the turn once as
  a full resend; the surfaced sequence stays a valid
  `Start → … → Done` stream.
- **401/403 handshakes** surface as authentication errors; Codex drops its
  cached OAuth token so the next turn re-authenticates from storage.

## Limits

The service allows 16 in-flight responses per connection (excess requests
queue FIFO per lane; same-lane turns never overlap), 32 distinct named lanes
per connection (the default lane is exempt), and connections live at most 60
minutes. Expiry looks like a transport close: reconnect plus full resend, not
a fatal error.

## Connection pooling

Connections are pooled per `(provider, endpoint, credentials, model)` and
shared across provider instances. Locking is per pool key only — a momentary
map lock guards lookups and never spans an `.await`, and each entry carries
its own connect lock, so one slow handshake never stalls unrelated providers,
models, or credentials.

## Codex status

The Codex backend (reverse-proxied Responses resource) exposes the same
WebSocket endpoint shape as the `OpenAI` API. The handshake carries the same
credentials as the HTTP path: `Authorization: Bearer`, `chatgpt-account-id`,
`originator: codex_cli_rs`, and `version`.
