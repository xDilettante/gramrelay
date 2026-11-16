# Bot runtime

Module ID: bot-runtime. Depends on the existing client/protocol slice.

## Public contract

- Lazy get_me, get_updates and get_webhook_info requests use the shared HTTP path and typed results.
- get_updates exposes explicit offset, limit, server timeout and allowed_updates. Negative offsets are rejected by the managed poller; the raw API's negative-offset behavior is intentionally not exposed by this helper because it discards queued events.
- Router<State> registers ordinary async command/message/callback/update handlers. MessageContext and CallbackContext expose the correct event and explicit shared state. Reply preserves chat, message/thread and business context.
- Command matching is exact; bot mentions are checked against the bot username. The managed runtime obtains identity with getMe.
- One library-managed poller per bot ID in this process, including separate Bot instances and clones. Raw manual get_updates calls and other processes require caller coordination.
- Polling validates its configuration and refuses to start when an active webhook is found; it never deletes the webhook implicitly.
- A batch has at most 100 events and is bounded by the client's response-byte cap. Groups preserve admission order within chat; separate chats run concurrently up to a configured bound. Group state is discarded after the batch.
- The next offset is requested only after the entire prior batch succeeds. Handler errors/panics/timeouts stop the poller without acknowledging that batch. Completed side effects can therefore be repeated on application restart; persistent idempotency arrives with journal integration.
- Shutdown stops intake and drains already accepted handlers within a configured deadline. The API supports an explicit cancellation token for tests/embedding.
- Unknown writes are never automatically retried. The first runtime version returns transport errors to its supervisor; read retry policy is a later explicit addition.

## Tests

Local mocks prove exact getUpdates JSON and typed decoding, context replies, bot-mention filtering, state and callbacks, per-chat order, bounded concurrency, offset barriers and gaps, no ack after failure, single ownership, webhook conflict and graceful cancellation. Malformed known events must still fail decoding; unknown nested fields survive.

## Deferred integration

Durable storage, webhook endpoint authentication and retry/backoff follow in subsequent tasks. They must be integrated before full-goal completion. This module does not claim exactly-once processing.

## Commands

cargo test --locked --test updates
cargo test --locked --test routing
cargo test --locked --test polling
cargo test --locked --all-targets
cargo test --locked --doc
cargo clippy --locked --all-targets -- -D warnings
