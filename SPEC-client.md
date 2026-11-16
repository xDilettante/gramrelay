# Complete client capabilities

Module client, C1/C2. Depends on the pinned generated wire surface (SPEC-protocol.md). This file records acceptance gates; completing one slice does not complete C1.

## C1a: named multipart attachments

Every generated Call may bind named root InputFile values to typed wire references. A binding is lazy: no file opens or dispatch before await. Recognize attach:// references only in Telegram file fields, including nested media, never in arbitrary message text/URLs/unknown variants. Pin reviewed String file fields in schema/file-policy.json against the canonical schema hash; InputFile schema expressions are already file fields. A generated typed traversal collects references.

Reject invalid, duplicate, missing, unreferenced or top-level-parameter-colliding attachment names before dispatch. Names contain ASCII letters, digits and underscores, start with a letter, and have at most 64 bytes. Up to 32 bindings per request. FileId belongs directly in the wire parameter and is not a new multipart part.

Serialize non-file multipart parameters as Telegram expects: strings directly, other JSON values as JSON text. Reuse existing streamed paths/bytes preparation with a total upload-byte budget. Preparation, upload and response reading share the existing semaphore/deadline. Locally invalid/missing/oversized files are NotSent; post-dispatch timeouts remain Unknown with no automatic replay. File growth remains bounded by metadata length. Request cancellation drops open streams; it does not undo Telegram effects.

Verify bytes/path/nested multiple-file requests, ordinary attach:// message text, exact multipart fields, all binding failures, combined size limits and timeout/cancellation semantics using localhost mocks.

## C1b: entity-based rich text

Root RichText is an owned text/entities helper, distinct from the Bot API 10.3 rich-message tree in api::types::RichText. String inputs remain plain. RichText::entity accepts UTF-8 byte ranges on scalar boundaries, then converts to UTF-16 offsets. styled and append support composition without manual offsets. try_from_entities validates received/raw UTF-16 spans, including surrogate boundaries, known kinds and required/exclusive metadata. The helper exposes read-only text/entities and into_parts for generated methods.

Validate positive, in-bounds lengths, complete containment on overlap, no duplicate same-kind nesting, no nested quotes, and no nested continuous entities (links/mentions/etc). Styles may surround or occur inside other entities except code/pre/date_time. Quotes may contain continuous entities and code/pre/date_time, following TDLib's explicit exception; they cannot occur inside continuous entities. Equal-span ordering puts block entities before inline entities. Validation uses sorting and a bounded kind stack, without a quadratic pair scan. Do not silently drop, trim, split or repair user entities.

EntityKind covers every pinned MessageEntity kind; link/user/custom-emoji/date-time metadata is typed. Validate date-time format grammar r|w?[dD]?[tT]? and nonempty IDs/URLs. Telegram still decides eligibility (such as custom emoji permissions) and semantic parsing. Diagnostics contain field names, not text or metadata values.

send_message, MessageContext.reply and send_document.caption accept Into<RichText>, omit empty entity arrays, and retain existing code-point limits for plain/entity text. Explicit ParseMode supports HTML, MarkdownV2 and legacy Markdown; reject parse_mode plus explicit entities before dispatch. Telegram validates rendered markup length: the raw markup source length cannot substitute for it. No implicit markup parsing or interpolation. Test exact JSON/multipart, UTF-16 conversion, composition, overlap rules, metadata, explicit parse modes and length failures with synthetic data.

## Remaining C1 gates

Contextual documents share the same delivery descriptor and validation as text replies: chat/thread/business/direct-message topic, ephemeral receiver, parent ID and expiry. Expose MessageContext.reply_document, CallbackContext.reply_document and ephemeral_reply_document; inaccessible/guest/unsupported scopes never fall back to another send. The remaining ephemeral reply window caps preparation/upload/response time as well as being checked before dispatch. No file opens before await, including a request rejected by scope validation.

- Rich text/entity helpers use UTF-16 offsets and validate boundaries, nesting and Telegram limits; plain text stays the default.
- Runtime uses the complete generated Update/Message model without erasing unknown fields. Contextual replies preserve chat, thread, business and applicable direct-message/ephemeral scope.
- Downloads validate Telegram file paths and enforce streaming/time/byte limits. Destination ownership and partial-output behavior are explicit.
- An opt-in bounded retry policy permits reviewed reads and explicit server refusals only; Unknown writes never replay. Cancellation and rate limits have tests.

## C2 gates

Optional tracing/metrics report stable operation/method/outcome/duration categories, with no tokens, request bodies, file contents or raw remote descriptions. Optional integrations and feature combinations have compile checks. Performance targets remain subject to measured Q2 evidence.
