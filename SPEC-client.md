# Complete client capabilities

Module client, C1/C2. Depends on the pinned generated wire surface (SPEC-protocol.md). This file records acceptance gates; completing one slice does not complete C1.

## C1a: named multipart attachments

Every generated Call may bind named root InputFile values to typed wire references. A binding is lazy: no file opens or dispatch before await. Recognize attach:// references only in Telegram file fields, including nested media, never in arbitrary message text/URLs/unknown variants. Pin reviewed String file fields in schema/file-policy.json against the canonical schema hash; InputFile schema expressions are already file fields. A generated typed traversal collects references.

Reject invalid, duplicate, missing, unreferenced or top-level-parameter-colliding attachment names before dispatch. Names contain ASCII letters, digits and underscores, start with a letter, and have at most 64 bytes. Up to 32 bindings per request. FileId belongs directly in the wire parameter and is not a new multipart part.

Serialize non-file multipart parameters as Telegram expects: strings directly, other JSON values as JSON text. Reuse existing streamed paths/bytes preparation with a total upload-byte budget. Preparation, upload and response reading share the existing semaphore/deadline. Locally invalid/missing/oversized files are NotSent; post-dispatch timeouts remain Unknown with no automatic replay. File growth remains bounded by metadata length. Request cancellation drops open streams; it does not undo Telegram effects.

Verify bytes/path/nested multiple-file requests, ordinary attach:// message text, exact multipart fields, all binding failures, combined size limits and timeout/cancellation semantics using localhost mocks.

## Remaining C1 gates

- Rich text/entity helpers use UTF-16 offsets and validate boundaries, nesting and Telegram limits; plain text stays the default.
- Runtime uses the complete generated Update/Message model without erasing unknown fields. Contextual replies preserve chat, thread, business and applicable direct-message/ephemeral scope.
- Downloads validate Telegram file paths and enforce streaming/time/byte limits. Destination ownership and partial-output behavior are explicit.
- An opt-in bounded retry policy permits reviewed reads and explicit server refusals only; Unknown writes never replay. Cancellation and rate limits have tests.

## C2 gates

Optional tracing/metrics report stable operation/method/outcome/duration categories, with no tokens, request bodies, file contents or raw remote descriptions. Optional integrations and feature combinations have compile checks. Performance targets remain subject to measured Q2 evidence.
