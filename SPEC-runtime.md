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
- If any event has no identifiable chat scope (including inline callbacks without a message), the batch is serialized conservatively; it must not race a potentially related known chat. Future typed event support can restore more concurrency.
- The next offset is requested only after the entire prior batch succeeds. Handler errors/panics/timeouts stop the poller without acknowledging that batch. Completed side effects can therefore be repeated on application restart; persistent idempotency arrives with journal integration.
- Shutdown stops intake and drains already accepted handlers within a configured deadline. The API supports an explicit cancellation token for tests/embedding.
- Unknown writes are never automatically retried. The first runtime version returns transport errors to its supervisor; read retry policy is a later explicit addition.

## Tests

Local mocks prove exact getUpdates JSON and typed decoding, context replies, bot-mention filtering, state and callbacks, per-chat order, bounded concurrency, offset barriers and gaps, no ack after failure, single ownership, webhook conflict and graceful cancellation. Malformed known events must still fail decoding; unknown nested fields survive.

## Complete wire-model integration (C1)

Root Message/Update/User/Chat/Document/CallbackQuery/WebhookInfo re-export the canonical generated DTOs. Remove the prototype projections. Preserve strong IDs; callers use message_id and read-only unknown_fields(). Optional nested objects are boxed. CallbackQuery.message distinguishes accessible and inaccessible messages. Document this pre-release source change and migrate every example/test.

All known Update event fields participate in ambiguity checks, including typed Update values supplied directly to Router::dispatch. Malformed known events fail decoding instead of falling into unknown JSON. Unknown events remain lossless. known_event_count() is generated from the pinned event fields.

Update::chat_id returns a reliable chat scope for typed message, callback (including inaccessible messages), reaction, membership, join, boost, widget and message-generation events. Account/global/inline events without a single chat scope return None; the existing global batch barrier remains. The managed poller's default subscriptions remain explicit message/callback; callers opt into additional kinds through allowed_updates. Full types must not silently change intake subscriptions or acknowledgement rules.

## Complete reply context (C1)

on_message_event(MessageEvent, handler) explicitly routes message variants, preserving their provenance in MessageContext.event. Commands/on_message retain their existing primary-message meaning. Replies copy chat, thread, business connection and direct-message topic; ephemeral parents use ReplyParameters.ephemeral_message_id instead of a placeholder message_id. The full ReplyParameters type is available to explicit callers.

Replies to ephemeral parents must themselves remain ephemeral, retaining receiver_user_id; missing receiver information fails locally. At await, reject parent dates outside the documented 15-second reply window using the host UTC clock (including future dates). This prevents obvious stale-ID reuse; Telegram still decides delivery/expiry and there is no exactly-once or clock-synchronization guarantee. Never convert a failed ephemeral reply into a normal message.

CallbackContext.reply returns an error for inline or inaccessible messages instead of guessing a chat. CallbackContext.ephemeral_reply uses its callback ID and no invented chat; known channel direct-message chats are rejected locally. Bot::reply_ephemeral exposes the same explicit callback-based operation. Both remain lazy and use the shared transport.

Guest messages require answerGuestQuery, not an ordinary chat send. A guest MessageContext.reply fails locally; answer_guest accepts a typed InlineQueryResult and binds the guest query ID. No context failure may silently fall back to another chat or ordinary send.

## Deferred integration

Durable storage, webhook endpoint authentication and retry/backoff follow in subsequent tasks. They must be integrated before full-goal completion. This module does not claim exactly-once processing.

## Commands

cargo test --locked --test updates
cargo test --locked --test routing
cargo test --locked --test polling
cargo test --locked --all-targets
cargo test --locked --doc
cargo clippy --locked --all-targets -- -D warnings
