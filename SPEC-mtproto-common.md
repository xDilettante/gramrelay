# MTProto common actions (M2)

Status: peer references and plain-text send/edit implemented. Bounded history is
implemented in SPEC-mtproto-history.md and dialogs in SPEC-mtproto-dialogs.md;
files and remaining common capabilities stay open in M2. This extends
SPEC-mtproto-client.md.

## Account and peer references

`Client::account(expected)` is a lazy host facade, not an agent grant or proof
of authorization. Each action runs live identity verification inside the same
owned job and shared authorization guard as its data/action RPC. Existing
admission, absolute deadline, shutdown and conservative outcomes apply.

`PeerId` contains only validated peer kind and bare ID. `Peer` is an opaque,
cloneable RAM reference bound to the originating client, expected account and
authorization epoch. It exposes only its ID; Debug and serialization must not
expose access hashes. Another client/account or an authorization transition
invalidates the reference. Both pre-admission and post-guard checks are required.
Reopening a session requires obtaining a fresh reference from its encrypted cache
or a fresh Telegram lookup. Host raw TL remains a separate trusted escape hatch.

`resolve_username` accepts an optional single leading @ and 1..32 ASCII letters,
digits or underscores; Telegram decides whether the name exists/is available.
It sends exactly one contacts.resolveUsername after verifying identity; a remote
refusal remains a sanitized Rejected error. Returned IDs must be valid, repeated
IDs are rejected, and self records must match the independently expected account.
At most 1024 users+chats are processed per response. Full hashes are persisted
reactively; min/empty constructors never replace a full hash or grant a new one.
Peer metadata is committed before a handle is returned, without changing either
provisional or acknowledged update state. Missing full authority is explicit
PeerUnavailable, never an implicit zero access hash. Basic chats need no hash;
the expected self user uses inputPeerSelf. A cached full hash may complete a min
response for the same peer. Session capacity/lease failures fail closed.

`cached_peer(id)` verifies live identity and returns an optional fresh reference
from encrypted metadata. It never contacts the target or guesses a hash. Neither
API returns raw User/Chat objects or incidental phones/names/access hashes.

## Text messages

Text send/edit share the same verified job and opaque peer binding. `SendText`
owns a Zeroizing text buffer (1..4096 UTF-16 units, rejecting NUL) and one
immutable nonzero OS-random random_id. A host may explicitly restore that ID for
its durable intent; this does not make replay of Unknown safe. Debug redacts
text. `edit_text` takes a positive message ID plus equally bounded text.
No ambiguous write is automatically replayed. A confirmed send receipt needs an
exactly-once matching updateMessageID plus one matching outgoing new-message
update (peer, ID, text), or a valid documented outgoing short-sent reply. Edit
needs one matching edit update (peer, ID, text). Duplicate or missing/conflicting
matching evidence, TooLong and unrelated updates return Decode/Unknown, never
guessed success. Receipts expose intentional peer/message IDs and date only.
Matching normal updates require nonnegative pts/pts_count; conflicting Service
or Empty constructors for the target cannot be ignored. Response peer entities
are checked/cached before a receipt is returned. Paid
sends, scheduling, extra send-as identities and link previews are disabled.
Only explicit `silent` may be set on the text intent. Raw TL supports advanced
host use until additional typed fields are specified.
Timeout/caller cancellation retains job ownership; Unknown does not authorize
retry. Returned update state must not be mistaken for durable acknowledgement.

## Verification

Local test RPC fixtures exercise the real pinned serialization/decoding path:
live identity before lookup/cache access; full, min, empty and missing hashes;
wrong/duplicate/self/malformed peer IDs; account/client/epoch binding; durable
cache across reopen; no update ACK/provisional mutation; request validation and
capacity; timeout/close with no replay. No live Telegram calls are authorized.

Sources: pinned grammers-session 0.12.0 src/peer.rs, grammers-client 0.12.0
src/client/chats.rs and vendor/grammers-tl-types/tl/api.tl. Official semantics:
https://core.telegram.org/api/peers and
https://core.telegram.org/method/contacts.resolveUsername . Current online TL
constructors are not substituted for the pinned layer.
