# MTProto document downloads (M2)

Status: implemented and verified locally on Windows / Rust 1.98.0. Extends
SPEC-mtproto-files.md and the owned Client path.

## DC routing and authorization

Private Context routing sends a bounded typed RPC to a known cached positive
DC without changing home_dc. It shares request/response checks, conservative
failure normalization, NoRetries, absolute deadline and joined driver ownership.
No raw DC/action field is exposed to agent tools. Home self verification remains
inside Account.run's shared authorization guard before file work.

For another DC, serialize its live self check/copy with a client-owned async
mutex. A full matching self returns success; exact 401 AUTH_KEY_UNREGISTERED
allows one export(home)/import(target) sequence. Do not overwrite a foreign
already-authorized identity. Both user and bot copying are supported by the API.
Export ID must match expected account, bytes be nonempty/<=16 KiB. Export/import
buffers and future_auth_token use zeroizing guards; diagnostics never retain
them. Imported authorization must contain a non-minimal, nondeleted full self
with expected ID/kind. No main auth epoch/home DC or update ACK changes.

Before Import dispatch, transfer the owned DC mutex guard into CallState and
mark a pending auth transition. Unknown import must fence the whole generation
before waiters wake. Confirming import is a protected substep: under the same
completion mutex used by supervisor sealing, require Pending, live Session,
open generation and unexpired deadline before clearing the transition/taking
the DC guard. Release it outside that lock. A sealed/late/error import keeps
the guard through final classification/cleanup. Main auth read guard stays
held throughout the file action, preventing login/logout races. Successful
copy need not serialize the remaining data transfer or invalidate peer handles.
No generic or ambiguous auth replay; malformed/foreign import fails closed.

## Source capability and transfer

Mint an opaque RAM-only RemoteDocument via a live verified exact peer/message
lookup. Existing HistoryMessage contains no client origin, so it cannot by itself
mint download authority. Use messages.getMessages for user/basic-chat IDs and
channels.getMessages for channels; validate exact message peer/ID before exposing
any file metadata. Missing/text/service messages return None, not a forged file.
Private file ID/hash/reference/DC fields remain bound to client/account/auth
epoch/peer/message and never Debug/Serialize. Names are explicit untrusted
display metadata, never an automatic output path.

Downloads share owned admission/deadline, verify source binding before admission
and after auth guard, and use getFile(cdn_supported=false) with bounded offsets/
chunks and known size. Premature short/empty responses are incomplete, never EOF
success. An aligned size requires one additional empty probe at the advertised
end (including zero-sized files), so an unreported aligned tail cannot succeed.
A confirmed allowlisted FILE_MIGRATE may redirect once to a different
known DC under the same deadline, with its own self/auth check. Flood/5xx/Unknown
are not replayed; unexpected CDN redirects are structured errors with secret
buffers wiped, not panic or new cryptography. Expired file references require
explicit source refresh; never silently replace an edited document.

Bytes output is bounded; large output needs an owned protected destination with
no clobber and no partial file masquerading as completed. Native I/O remains
joined and OS-blocking caveats from uploads apply. Final size and optional
expected SHA-256 are checked before publishing a completed result. No existing
destination may be overwritten. Defaults: 64 MiB/four concurrent parts; host
maximum 4 GiB/eight parts; memory result hard maximum 64 MiB. Part size is a
power of two in 4..512 KiB chosen from the response budget, dividing 1 MiB.
FuturesOrdered bounds active results and consumes them in order, without spawned
part tasks; ordinary errors drain already-admitted futures before action exit.

Native Writer creates a tempfile in the protected absolute destination's parent,
writes/hashes owned chunks, checks exact size/optional SHA-256, syncs the file,
then persists without clobber. Existing entries are refused before reads and a
racing creator is preserved by persist_noclobber. Validation errors remove the
temporary. Deadline checks surround native operations, but cannot cancel an OS
syscall: publication finishing late may leave a complete validated destination
with an Unknown outcome. No blind replay or deletion of that result. A killed
process may leave a temporary; directory crash durability and adversarial
same-user filesystem replacement are outside the protected-host contract.

## Verification / sources

Local real-TL fixtures: cross-DC routing with unchanged home, user/bot self
checks, one copy for concurrent requests, exact unregistered refusal, bounded
secrets, wrong/late/5xx import, no wake-before-fence and timeout/close join.
Source/size/chunk/migration/CDN/destination/cancellation coverage is implemented.
The confirmation-deadline and aligned-tail findings have RED-to-GREEN tests;
focused fresh-context follow-ups report no remaining issue. No live
Telegram requests, private credentials or private files are authorized.

Evidence: 17 new Rust tests (12 download, five DC) cover the cases above;
476 all-feature / 312 default workspace tests and 11 doctests pass. Clippy,
format, 25 Python schema/SDK checks, deterministic agent schemas and cargo audit
(363 dependencies) pass. No live-server, cross-platform or performance claim.

Pinned grammers-client 0.12.0 files.rs/net.rs, sender_pool.rs and TL api.tl;
https://core.telegram.org/method/auth.exportAuthorization ,
https://core.telegram.org/method/auth.importAuthorization ,
https://core.telegram.org/method/upload.getFile and
https://core.telegram.org/api/files .
