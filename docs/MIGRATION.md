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
