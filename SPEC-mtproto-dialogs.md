# Bounded MTProto dialogs (M2)

Status: implemented with local mock verification. Extends common/history specs
and the owned Client path. Authenticated live compatibility remains unclaimed.

`Account::dialogs(folder, limit, cursor)` is user-only, limit 1..100, and checks
live self identity in the same owned shared-auth job. Folder is Main(0) or
Archive(1), distinct from custom chat filters. All pages/cursors use existing
admission, absolute deadlines, no replay and joined shutdown. No returned pts,
updates.State, draft or channel state becomes provisional/durable update ACK.

First load fetches messages.getPinnedDialogs for that folder. A bounded immutable
RAM snapshot (<=1024 entries/messages and <=1024 source peer entities) is shared
by pinned pages/cursors; page slices retain the same allocation, not a copied
unbounded queue. Every pinned snapshot page still verifies current self identity.
After pinned pages, ordinary pages use messages.getDialogs with exclude_pinned
true, explicit folder_id and hash=0, so old pinned timestamps cannot skip newer
ordinary dialogs. If no pinned entries exist, the first call loads an ordinary
page directly. There is no automatic background/recursive scan. Changes during
a scan are not a snapshot: pin/unpin/move may legitimately repeat entries.

`DialogCursor` is opaque, cloneable, non-serializable and redacted, bound to
client/account/auth epoch and folder. It contains no strong Client. Check it
before admission and after the authorization guard; wrong bindings return
InvalidCursor/NotSent. Source hashes stay in encrypted Session metadata; returned
conversation references are opaque and optional when full authority is missing.

Entries have explicit `DialogId::Peer`, `Folder` or `Community`. Folder and
community containers never become guessed send targets. Each entry exposes its
ID, explicit display name, pinned/unread metadata, top message ID and optional
exact last message (shared HistoryMessage wrapper, not a peer-only guess).
Debug excludes names/content/incidental source metadata. Source names <=4096
bytes; response bytes use Client's cap, with decoded heap/stack still release Q4.
Every message constructor is validated; a missing peer on Empty is inferred
only from a unique matching top-message target. Duplicates, invalid IDs/dates,
negative counters, foreign-folder conversations and mismatched/missing positive
top-message records are malformed. top_message=0 is a cleared dialog and stays
visible without inventing a date. Source full/min/empty peers use the existing
checked reactive cache; metadata is committed before publishing the page.

`DialogPage::continuation()` is an explicit enum: More(cursor), Complete, or
Incomplete(issue). Never infer completion merely from a short page or missing
last-message/authority. Full messages.Dialogs and an empty ordinary result are
terminal; a nonempty Slice needs a last conversation with a dated message and a
usable cached reference, or returns Incomplete(MissingAnchor/MissingPeer).
Container-only/unpageable slices retain their entries with Incomplete. Ordinary
anchor rank is (message date, top message ID, Bot API dialog ID), descending,
per pinned TDLib DialogDate; dated conversation records and continuation anchors
must strictly progress from the previous rank. Other unanchorable records are
visible but do not invent ranks. NotModified with hash=0 and malformed responses
return Decode/Unknown, never panic. Advisory slice count is nonnegative and no
smaller than that response's entry count; it is not a global pinned+ordinary
snapshot total and is not used as the end condition.

Verification: real pinned serialization via local test runner; pinned snapshots
across small pages with no repeated pinned RPC, Main/Archive/container handling,
consistent last-message triples, short ordinary Slice, strict rank progression,
cleared/missing-hash incomplete tails, malformed/duplicate/counter/top errors,
cursor origin/folder/epoch checks, identity before even cached pages, bounded
snapshot, no strong Client retention, no ACK/provisional mutation, timeouts and
joined cancellation. No live Telegram requests are authorized.

Sources: pinned grammers-client 0.12 dialogs.rs; pinned official TDLib
03220f6b3043c6fd7785fbf0901d63f19ad19974 MessagesManager.cpp GetDialogListQuery,
on_get_dialogs/load_folder_dialog_list and DialogDate.h; pinned TL api.tl;
https://core.telegram.org/method/messages.getDialogs ,
https://core.telegram.org/method/messages.getPinnedDialogs ,
https://core.telegram.org/api/folders and https://core.telegram.org/api/offsets .
