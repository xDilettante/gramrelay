# Authenticated webhook intake (R4)

## Objective and boundary

Implement the bot-runtime webhook module from docs/ARCHITECTURE.md. Both ordinary bots without SQLite and durable tg-agent feeds must be usable. Share authentication, parsing, admission, HTTP responses and shutdown; separate their acknowledgement/recovery semantics. In-memory queue admission alone never produces a successful response. Do not register, remove or replace the remote Telegram webhook implicitly.

Use an optional `webhook` feature in tg (implies bot), with Axum 0.8.9 and the existing maintained Hyper/Tokio stack. The HTTP adapter may be mounted in a host Axum router; a managed HTTP/1 server uses Hyper's parser, bounded connection tasks, header/connection deadlines and explicit shutdown. TLS terminates at the host's existing server or trusted reverse proxy. The managed listener defaults to loopback only; non-loopback plaintext requires an explicit trusted-proxy option. No custom HTTP, TLS or cryptographic implementation.

## Public contracts

- WebhookSecret validates Telegram's 1..256 ASCII A-Z/a-z/0-9/_/- rule. No Display/Serialize, redacted Debug, explicit expose method for authorized setWebhook setup. Compare with subtle's constant-time equality; reject missing, malformed or repeated headers before reading the body. No query-string/token-path authentication fallback.
- WebhookIngress and a small WebhookSink interface provide the framework-independent boundary. This low-level adapter does not verify bot identity or claim source ownership; its host owns those responsibilities. Managed Bot and UpdateFeed entry points verify getMe, share the SDK source guard and coordinate the durable source lease where applicable. Storage handles and callback values are not permission grants.
- Accept POST application/json (optional UTF-8 charset), reject content encodings other than absent/identity. Authenticate before buffering; reject oversized declared or streamed bodies. Decode the real generated Update, preserve unknown fields and reject negative IDs/multiple known event kinds. Return fixed safe HTTP bodies/statuses, never arbitrary Bot API methods in the webhook response.
- Defaults: path /telegram; event 1 MiB; 16 admitted requests with a 16 MiB aggregate byte reservation (reserve one event limit before reading); one 30-second deadline from admission through body, queue and backend completion; 30-second shutdown. No unbounded request queue or per-update spawn. Max managed connections 64, header deadline 5 seconds, connection lifetime 60 seconds. Validate all bounds/overflow before intake; larger configured limits remain explicit and finite.
- HTTP statuses: committed/processed or matching successful duplicate 200; invalid payload/framing headers 400; missing/wrong/repeated secret 401; payload too large 413; content type/encoding 415; payload conflict 409; overload/closed/storage/lease loss 503; admission deadline 504; handler failure/panic 500. Retryable refusals include a fixed Retry-After. Never return 2xx after only queuing memory work or after a failed/uncertain commit.
- Stop closes admission, drains accepted work within its deadline and then releases ownership. Deadline expiry aborts owned connection/handler tasks and reports an explicit error. Dropping the whole future aborts its owned tasks; a native DB job already admitted may still commit. Cooperative async scheduling and the impossibility of undoing external effects remain explicit limitations.

## In-memory mode

Bot-managed webhook validates/binds the Router and bot identity. A bounded queue groups ready updates for the existing dispatch_batch implementation: per-chat admission order, parallel unrelated chats and a global barrier for unknown scopes. Queue entries/bytes, batch size and chat groups are bounded independently of HTTP connections. Admission order means arrival at this queue, not Telegram's original order before a reordered network delivery.

Deduplicate IDs using a canonical DTO fingerprint. Matching in-flight delivery returns retryable refusal rather than executing again; successful duplicates return 200; conflicting content returns 409. Cache defaults: 100,000 IDs, 24-hour TTL, no eviction of unexpired entries to make room. Pending work cannot expire out from under an active handler. Handler failure/timeouts leave the update unacknowledged and permit later redelivery. HTTP cancellation does not detach a task per update; the bounded dispatcher owns accepted work until completion/deadline. Process restart loses this cache. External handler effects are at-least-once and need stable operation IDs.

## Durable mode

The optional tg-agent webhook feature reuses UpdateFeed::append_webhook; 200 follows its successful commit, including an identical stored duplicate. Check verified bot identity before claiming SourceMode::Webhook; exclude polling in this process and database. A supervised heartbeat renews the lease during idle time and active HTTP work, with renewal bounded by its known expiry. Lease loss closes intake; it never silently switches mode/reclaims another worker's source. Shutdown drains HTTP/DB work, waits behind cancelled native jobs and releases the DB lease before dropping the SDK source guard. Processing remains an independent durable consumer.

## Implementation slices and verification

1. src/webhook/{types,ingress,http}.rs: secret/options/sink contract, bounded authentication/body intake and Axum method router. Unit/integration fixtures cover repeated/wrong/missing secrets before body polling, malformed/unknown JSON, exact byte and request limits, slow/chunked bodies, fixed private diagnostics and deadline/cancellation accounting.
2. src/webhook/{server,memory}.rs: bounded managed HTTP server, ordinary Bot runtime, successful/in-flight/conflicting duplicates, per-chat ordering, overload, error/panic/timeouts and graceful/forced shutdown. Real localhost HTTP tests; no live Telegram.
3. tg-agent/src/webhook.rs: durable adapter and source heartbeat/cleanup. Localhost commit-before-ack, duplicate/conflict/quota, cross-instance ownership, blocked renewal, cancellation and restart tests reuse J2 crash evidence and add HTTP boundary evidence.
4. Optional tracing/metrics contain only fixed categories and durations; no token, secret, path, bot/chat/update IDs, payload or raw errors. Document task-based use and an offline localhost example, feature matrix, dependency audit and regressions. Mark R4 complete only after both managed modes and the transport boundary are integrated and checked.

Commands: cargo test --locked --workspace --all-targets --all-features; cargo test --locked --workspace --doc --all-features; cargo fmt --all -- --check; cargo clippy --locked --workspace --all-targets --all-features -- -D warnings; cargo audit. Verify client/storage-only builds separately. Existing schema generator output must remain unchanged. No live Telegram calls, remote registration, publication or push are part of this work.
