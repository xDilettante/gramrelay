# Bounded MTProto history (M2)

Status: implemented with mock verification. Extends SPEC-mtproto-common.md and
the owned Client path. Live authenticated compatibility remains unclaimed.

`Account::history(peer, limit, before)` reads one newest-to-oldest page with
messages.getHistory. Limit is 1..100. Both this method and Telegram dialogs are
user-only; a bot facade gets UnsupportedOperation/NotSent without network work.
The independently expected self is verified inside the same owned shared-auth
job before accessing history. No implicit retry, GetState/difference or update
acknowledgement is performed. Source entity routing metadata is validated and
durably cached without advancing provisional/acknowledged update state.

`HistoryCursor` is cloneable, opaque, non-serializable and redacted. It holds a
reference bound to originating client/account/authorization epoch, target peer
and a strictly positive exclusive before-ID. Both peer and cursor are checked
before admission and after acquiring the job's authorization guard. Foreign,
wrong-peer and obsolete cursors fail locally as InvalidCursor/NotSent. Cursors
are navigation metadata, not an agent grant or a persistent update checkpoint.
Reopening requires a new scan. Concurrent message changes are not a snapshot.

Requests set offset_id to the before-ID (zero initially), offset_date/add_offset/
max_id/min_id/hash to zero, and the exact requested limit. Every returned record
has a positive unique strictly descending ID, below the prior exclusive offset.
Nonempty regular/service peers must match the requested peer; Empty without a
peer inherits only this request's already verified target. Present sender/peer
IDs and ordinary/service dates are checked. At most limit messages are accepted.
Regular text is bounded to 64 KiB; response bytes share Client's existing cap.
Encoded bounds do not claim generated decoded-heap/stack limits (release Q4).

`HistoryMessage` exposes kind (regular/service/empty), peer, message ID, optional
date/sender/text/outgoing status and media presence. No constructor is silently
dropped. Its Debug redacts content and incidental metadata. It retains the full
pinned TL Message privately and exposes `raw()` only as an explicit trusted Rust
escape hatch for advanced fields. Raw media may contain file access hashes and
must not be logged or dumped into agent tools; M3 will emit a fixed safe shape.
Regular message text is best-effort zeroized on wrapper drop. No broad upstream,
compiler or OS memory-wipe claim is made. Download capabilities remain M2 files.

Full messages.Messages is terminal. Slice/ChannelMessages carries advisory total
count and an explicit inexact flag; count/position/pts must be nonnegative and
an exact total cannot be smaller than this page. ChannelMessages is accepted
only for a channel target. A short nonempty slice is NOT terminal: the last ID
becomes the next cursor while it is above 1. Empty exact continuation pages are
terminal (the total covers the whole history, not remaining records). An empty
inexact page, or an initial empty slice with positive count, returns explicit
IncompletePage/Unknown. NotModified with hash=0 and malformed/nonprogressing
records return Decode/Unknown, never panic or guessed empty success. A page
exposes is_inexact and total_count; neither is an update ACK.

Verification uses pinned TL serialization through the production test RPC
runner: multi-page/short-page/empty continuation, all constructors, channel and
inexact flags, negative/count/limit/ID/peer/order anomalies, local cursor binding,
post-guard epoch changes, identity before data, timeout ownership/no replay,
redaction and unchanged provisional state. No live Telegram calls.

Sources: pinned grammers-client 0.12.0 client/messages.rs fill_buffer/GetHistory,
vendor/grammers-tl-types/tl/api.tl and official
https://core.telegram.org/api/offsets ,
https://core.telegram.org/method/messages.getHistory ,
https://core.telegram.org/constructor/messages.messagesSlice ,
https://core.telegram.org/constructor/messages.channelMessages .
