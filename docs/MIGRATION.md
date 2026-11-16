# Migration notes

The crate is unpublished and pre-release. These changes replace the initial prototype surface before the first public release.

## Complete wire model

Root Message, Update, User, Chat, Document, CallbackQuery and WebhookInfo now re-export api::types. Mixing convenience methods and bot.api() no longer requires DTO conversion.

- Message.id becomes Message.message_id, still the strong root MessageId scalar. User.id and Chat.id keep their names.
- Unknown fields are read through unknown_fields(), not a mutable public map. They are retained by serialization.
- Optional nested objects are Option<Box<T>>. Read access normally uses as_deref(); generated setters take T and allocate the box.
- CallbackQuery.message is Option<MaybeInaccessibleMessage>. Use accessible() before reading message content, or chat()/message_id() for either variant.
- Update models every pinned event. Malformed known events now fail decoding, and multiple known events are rejected by decoding and Router::dispatch. Unknown future events still round-trip.
- Document.file_size and WebhookInfo.pending_update_count follow the wire's signed Integer representation. Validate non-negative sizes before using them as local byte counts.
- Root scalar MessageId differs from the Bot API response object api::types::MessageId. Namespaces retain this distinction deliberately.

Polling subscriptions do not change implicitly. Add kinds explicitly with PollingOptions::allowed_updates. Unscoped events serialize their batch; a typed chat scope now allows the existing per-chat ordering for other subscribed kinds.

## Rich text

String inputs still send plain text. send_message, MessageContext.reply and SendDocument.caption accept Into<RichText>. Use EntityKind and byte ranges, styled fragments or try_from_entities; errors report safe field names. Explicit ParseMode delegates markup parsing and rendered length validation to Telegram, and cannot be combined with explicit entities.

## Scoped replies

MessageContext.event records provenance. Route other message variants explicitly with on_message_event(MessageEvent, handler) and opt into their polling subscriptions. Replies now preserve direct-message topics and ephemeral recipient/parent IDs in addition to chat, thread and business connection. Missing recipients and expired ephemeral parents fail locally; the expiry preflight uses the host UTC clock.

CallbackContext.reply returns Result because inline and inaccessible callbacks do not provide readable message content. ephemeral_reply uses the callback ID without inventing a chat; answer() remains a separate required action. Guest contexts require answer_guest with a typed InlineQueryResult. Numeric guest chat IDs are not permission to send an ordinary bot message.

Text and document replies share delivery validation. reply_document/ephemeral_reply_document preserve the same scopes; the remaining ephemeral window also caps preparation/upload/response time. max_attachments is now configurable with a default of 50.

## Explicit retry policy

Requests still make one attempt by default. Generated Call accepts RetryPolicy for reviewed reads, with bounded elapsed time, attempt count and backoff/jitter. Confirmed ordinary JSON 429 refusals require retry_rejected_writes(true); Unknown writes, credential/session operations, migrations and temporal references remain ineligible. Multipart replay is rejected before dispatch. Downloads do not retry into a potentially partially written destination.

## Agent journal v5

Opening an agent journal upgrades reviewed v1/v2/v3/v4 databases to v5 in one
transaction, including storage-only builds. Existing operation keys, receipts,
update positions and source/consumer ownership are retained. New tables persist
executor scopes, fixed-window budget counters and immutable file metadata;
older library versions reject
this schema. Back up the protected SQLite directory before upgrading a deployed
host. Do not replace a live database with a pre-send backup to retry an uncertain
operation: doing so erases the deduplication boundary.

Manual J1 rows remain usable through Journal. The authorized executor refuses
to attach scope to an already-existing manual row; it cannot infer authority from
an arbitrary result. Retain those keys and choose a distinct executor namespace
only as an explicit host migration decision, never as automatic retry recovery.
Budget identities persist across grant revisions. Changing a fixed window needs
a new host-chosen budget identity; revocation/restart alone does not reset usage.

File bytes are resident snapshots, not SQLite content. New uploads after restart
require explicit host restoration of identical bytes for the retained handle;
cached/get_operation receipts require current file scope but no resident bytes.
Never renew an old handle by overwriting metadata. Register a new file intent
explicitly when content/expiry changes, retaining old operation keys and receipts.
