# MTProto documents (M2)

Status: uploads/sending implemented with local mock verification; downloads/DC
authorization and observable updates remain open. Authenticated live
compatibility remains unclaimed. Extends common actions and owned Client rules.

## Upload intent and source

`SendDocument` is a host-only non-serializable intent with redacted Debug. It
owns bytes or an absolute host-selected PathBuf, explicit basename/MIME type,
caption, immutable nonzero random_id and a distinct temporary upload file_id.
Constructors do no file/network I/O. Default local size bound is 64 MiB, host
configurable up to 4 GiB; this is a resource ceiling, not a claim about Telegram
account entitlements. Byte length AND capacity must fit it. Reject empty input,
non-regular opened files, invalid MIME, controls/unsafe basename separators and
invalid/oversized captions before upload. MIME components use RFC 6838 restricted
names (initial alphanumeric, <=127 bytes each), without parameters. No tool
accepts an arbitrary file path.
M3 must reuse approved immutable file handles and check their expected SHA-256.

The source pipeline is owned blocking work via Context.compute, not an external
spawn_blocking/abort wrapper. Each native chunk checks the action deadline/stop.
Session heartbeat stays independent. Native OS file I/O itself can block; early
timeout does not imply joined close, which must await owned I/O. Paths are trusted
host input in protected storage; no defense against a hostile local filesystem
or a kernel/network-filesystem hang is claimed. Errors retain static kinds only,
never path/IO source chains, content, MIME or filename in diagnostics.

Before upload, a bounded sequential pass calculates SHA-256 and (for <=10 MiB)
the required legacy MD5 checksum. Optional independently expected SHA-256 is
checked before any part RPC. The upload pass reads the same opened file handle
or frozen owned byte buffer, hashing the actual parts again. Truncation/growth/
different bytes abort final message dispatch. No temporary plaintext file or
unbounded in-memory spool is introduced. A source change can leave temporary
Telegram parts, but cannot knowingly publish a mixed/unverified document.
MD5 is protocol interoperability only; integrity uses maintained SHA-256.
Use existing RustCrypto SHA-2 and pinned RustCrypto md-5 0.11.0, no custom hash.

## Network sending

`Account::send_document(peer, intent)` uses the same owned verified-account job,
shared authorization guard and opaque peer pre/post-guard checks as text sends.
The absolute Client deadline includes preparation, chunks and final SendMedia.
Part size is the largest power of two <=512 KiB fitting the encoded request
budget (>=1 KiB). The chosen size divides 512 KiB; only the last part may be
short. <=10 MiB uses saveFilePart+inputFile/MD5; larger files use saveBigFilePart+
inputFileBig with the same immutable total. Every Bool result must be true.

At most four part calls are active by default, configurable 1..8. A bounded
FuturesUnordered is polled inside this action, spawning no independent async
tasks. It admits no more work after the first observed error and drains all
pre-admitted part futures before completing an ordinary error.
It must not drop pending part futures and free parent admission while the SDK
still owns requests. On deadline/shutdown, the existing whole-driver barrier
precedes abort/join of client jobs and drop of their part futures. Caller Drop never frees
the action's source, buffers or active-part budget early. No chunk, final send,
flood wait, migration or Unknown is automatically replayed.

Final SendMedia bytes are validated/encoded before part dispatch; media uses
force_file, exact MIME and one filename attribute, no paid/scheduled/send-as or
implicit extra effects. Caption is plain 0..1024 UTF-16 units, no NUL. Final
dispatch occurs only after all parts are confirmed and second SHA-256 matches.
Receipts require existing exact random-ID/peer/new-message evidence (or valid
short-sent context), outgoing/date/counter/caption checks, and a nonempty returned
Document with matching size/MIME/filename. No guessed text-only/media receipt.
Return intentional message metadata, byte count and explicit SHA-256 accessor,
not upload IDs, access hashes, file references, filename/content or raw TL Debug.
Explicit message/filename/MIME/checksum text and file byte buffers use best-effort
zeroizing guards; paths, copies in maintained SDK/hash code, the compiler and OS
remain outside that guarantee.

## Remaining download increment

Downloads must share owned admission/deadlines, verify account and an opaque
source capability, bound chunk count/size/destination ownership and never infer
EOF from a premature short response against a known size. Use getFile with
cdn_supported=false; unexpected CDN redirects are structured errors, not panic
or home-grown CDN cryptography. File references/DC metadata remain private.
Pinned grammers files.rs shows explicit AUTH_KEY_UNREGISTERED auth export/import
and FILE_MIGRATE handling; the wrapper must bound confirmed redirects, validate
the imported self identity, serialize/fence uncertain auth transitions and join
all work. SenderPool alone negotiates DC keys/config but does not copy user auth.
No generic Context.invoke_in_dc or download API is implemented by these notes.

## Verification and sources

Native source tests: owned bytes/no copy, actual file round trip, small/big
threshold, SHA-256/MD5, part boundaries, limits/capacity, append/truncate/change,
expected-hash refusal, cooperative stop and private diagnostics. Real pinned
test RPC: typed small/big chunks, bounded concurrency/drain, exact final media,
no final send after source/chunk errors, false/timeout/flood/Unknown no replay,
receipt evidence, peer/account/epoch guards and unchanged update state.
No live Telegram calls or private files are authorized.

Sources: pinned grammers-client 0.12.0 files.rs/net.rs, pinned TL api.tl;
https://core.telegram.org/api/files ,
https://core.telegram.org/method/upload.saveFilePart ,
https://core.telegram.org/method/upload.saveBigFilePart ,
https://core.telegram.org/method/upload.getFile and
https://docs.rs/md-5/0.11.0/md5/ and
https://www.rfc-editor.org/rfc/rfc6838.html#section-4.2 .
