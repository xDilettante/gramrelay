# Migration notes

The crate is unpublished and pre-release. These changes replace the initial prototype surface before the first public release.

## MT host configuration v2

Bot host v1 and both checked tool-schema inventories retain their wire meaning.
With `host,mtproto`, v2 requires a nonempty `mtproto.profiles` section alongside
the existing Bot `profiles` array. See [MT host setup](AGENT-MTPROTO.md) for exact
account/session/environment/grant fields, file restoration and explicit callback
aliases. Tool requests remain version 1. Config cannot select an RPC fixture.

`Session::open_existing` rejects missing/uninitialized stores instead of creating
them. Normal Session::open remains the provisioning API. `Host::close` now borrows
the Host so its wait can resume after cancellation; existing `host.close().await`
call syntax continues to work. Keep the Host alive and drain adapter calls first.
MT Executor/all Clients finish before Journal, including on startup failure.
The journal schema remains v9; key/receipt/scope/byte-expiry meaning is unchanged.
Live observation aliases must remain assigned to their original events when a
trusted host restores them across registry lifetimes; this adds no alias-history
ledger. Cross-lifetime callbacks retain the explicit wall-clock trust choice.

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

## Agent journal v9

Opening reviewed v1–v8 journals upgrades through the previous migrations and
rebuilds executor_files in one immediate transaction. The immutable columns and
owner index stay the same; profile now references account_profiles so MT bot/user
and Bot API handles share the bounded permanent registry. Copy/drop/rename/index
recreation preserves all metadata, scoped receipts, keys and budgets, including
rows beyond newly configured capacity. Old schemas and original FKs are verified
before copying; old Bot file bindings must match their actual Bot feed/account.
Orphans or contradictory kinds/IDs cannot be silently reclassified as MT data.
Final exact DDL and FK verification precede commit; failures retain the original
version/schema/rows. Older libraries reject v9. Protect/back up the SQLite
directory before deployment and join Executor → Client → Journal before shutdown.

The MT Rust executor now supports all eight typed actions. New document attempts
need owner-bound restored bytes plus current file/peer/action authority and any
exact approval. Retained get_operation/cache uses original grants without bytes
or a live file expiry. Terminal Unknown/Rejected keys never resend. CLI/MCP MT
integration and release gates remain open.
