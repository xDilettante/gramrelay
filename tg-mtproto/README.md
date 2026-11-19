# tg-mtproto

[Руководство MTProto с примерами](../docs/MTPROTO.md)

`Session::open_existing` открывает только существующее инициализированное
зашифрованное хранилище: отсутствующий или пустой файл не создаётся/не
инициализируется. Применяется при загрузке готовых профилей CLI/MCP;
`Session::open` остаётся API создания сессии перед доверенным входом.

Separate MTProto package with the M2 client foundation implemented. Encrypted local session
storage and the owned network Client with typed raw TL calls are implemented.
Bot/user login, 2FA, confirmed login migration, live identity verification,
opaque peer lookup/cache references, text send/edit, bounded paginated history
and Main/Archive dialogs are available. Document upload/sending from owned bytes
or protected host files is implemented. Document downloads support verified DC
authorization and protected destinations. Observable batch updates have durable
ACK, bounded catch-up and explicit scoped recovery. Release gates remain open.
This package does not route MTProto operations through the Bot API.

Bot accounts can answer callbacks with `CallbackAnswer::new(query_id)` and
`Account::answer_callback` / `answer_callback_until`. Optional plain text is bounded
to 200 UTF-16 units; alert is explicit, URLs and caching are absent. All opaque
i64 query bits, including zero, are preserved. Only Bool true confirms success.
The absolute cap includes live identity waits; cancellation retains owned work
and never retries an ambiguous effect. This trusted Rust intent is not an agent
permission: `tg-agent::mtproto::Executor` supplies scoped opaque references.

```rust,no_run
use tg_mtproto::{Session, SessionBinding, SessionKey, SessionOptions};

async fn open_session(
    protected_absolute_path: &std::path::Path,
    host_key: SessionKey,
) -> Result<Session, tg_mtproto::SessionError> {
    Session::open(
        protected_absolute_path,
        SessionBinding::new("personal", 12345)?,
        host_key,
        SessionOptions::default(),
    ).await
}
```

Provision a cryptographically random 32-byte key through a host secret store.
Keep it outside this database, tool arguments and logs. The host protects the
database directory and backups. Opening performs no network request; remembered
account metadata is not proof of current authorization. Common actions verify
the expected identity before accessing account data, within the same owned job.

Trusted Rust code can invoke the pinned TL layer directly:

```rust,no_run
use tg_mtproto::{Client, ClientError, ClientOptions, Session, tl};

async fn probe(session: Session) -> Result<tl::enums::Pong, ClientError> {
    let client = Client::new(session, ClientOptions::default())?;
    let response = client.invoke(&tl::functions::Ping { ping_id: 42 }).await;
    client.close().await?;
    response
}
```

Construction is lazy and claims a single client per session. The first polled
call starts the driver; Tokio timers are required. Defaults allow 16 owned jobs,
a 30-second absolute deadline, a 1 MiB encoded request and an 8 MiB response.
Admission precedes serialization. Dropping a caller does not release capacity
while its owned work remains active. A timeout or session fence closes the whole
generation; `close().await` joins the driver tree before draining storage.
`Client::request_budget` exposes the configured request deadline budget. Hosts
with their own durable lease can use `Account::send_text_until` and
`edit_text_until` with an absolute `std::time::Instant`; that cap also covers
queue/identity waits and never extends the Client budget. An expired cap refuses
dispatch; a timed-out dispatched action remains Unknown, without automatic replay.
Public `Session::close` refuses while attached: close through the Client.

Raw calls do not retry. An ambiguous remote effect returns `Outcome::Unknown`;
a confirmed RPC refusal is `Rejected`, and a locally unsent call is `NotSent`.
Diagnostics retain static kinds/codes and allowlisted numeric flood waits only.
Raw TL is a trusted host capability, never a generic agent tool. Encoded response,
gzip expansion and envelope nesting limits apply before SDK control/update
decoding. The generated TL decoders also bound collection work, allocations and
recursive depth. The SDK tests exercise these limits with seeded structural input.
Host-provided Rust deserializers control their own resource use.

Build this development package from the repository workspace.
The hardened grammers graph uses direct relative SDK paths throughout the full
source tree. External path/pinned-Git consumers need no patch table; preserve
the complete vendor tree. See [source installation](../docs/INSTALL-SOURCE.md)
and [local private package preparation](../docs/PRIVATE-DISTRIBUTION.md).

`verify_account(expected)` queries the current self user and checks its ID/kind
against the host expectation and remembered account. It commits matching identity
and peer metadata together before returning. `sign_in_bot(credentials, token,
expected)` accepts host-only `AppCredentials` and `BotToken`, checks the app
binding/token prefix and validates the returned self identity. Credential Debug
is redacted; no plaintext getter or serialization is provided. Authorization
transitions exclude raw/identity jobs until their final outcome is classified.
An unknown transition fences the generation before waking waiting jobs. The
interactive flow uses opaque RAM-only challenges; see SPEC-mtproto-auth.md.
Agent tools cannot initiate login.

## Verify an existing session

This probe connects to Telegram and checks the identity of a previously
authorized session. The host supplies the expected account and the session key.

```rust,no_run
use std::path::Path;
use tg_mtproto::{
    AccountIdentity, Client, ClientOptions, Session, SessionBinding, SessionKey, SessionOptions,
};

async fn verify_existing(
    protected_absolute_path: &Path,
    binding: SessionBinding,
    host_key: SessionKey,
    expected: AccountIdentity,
) -> Result<AccountIdentity, Box<dyn std::error::Error>> {
    let session = Session::open_existing(
        protected_absolute_path,
        binding,
        host_key,
        SessionOptions::default(),
    )
    .await?;
    let client = Client::new(session, ClientOptions::default())?;
    let identity = client.verify_account(expected).await;
    client.close().await?;
    Ok(identity?)
}
```

## Use an account facade

Use a host account facade and an opaque peer reference for common actions:

```rust,no_run
use tg_mtproto::{AccountIdentity, Client, ClientError, MessageReceipt, SendText};

async fn greet(
    client: &Client,
    independently_expected: AccountIdentity,
) -> Result<MessageReceipt, ClientError> {
    let account = client.account(independently_expected);
    let peer = account.resolve_username("example").await?;
    let sent = account.send_text(&peer, SendText::new("Hello")?).await?;
    account.edit_text(&peer, sent.message_id(), "Hello again").await
}
```

The facade is not an agent grant. Each action verifies the current self account.
`Peer` exposes only its typed ID; access hashes stay in encrypted session metadata.
References cannot move between clients/accounts or survive an auth transition;
after reopening, use `cached_peer(id)` to obtain a fresh reference. Missing full
hashes return `None` from the cache, or `PeerUnavailable` from username lookup.
Minimal/empty objects do not invent an access hash.

`SendText` generates one immutable random ID, redacts Debug and limits plain text
to 1..4096 UTF-16 units. Silent sends are explicit; previews, paid sends and
scheduling are disabled. `with_random_id` lets a trusted host restore a journaled
intent; it does not make retrying `Unknown` safe. Send/edit receipts require
matching response evidence; unrelated, duplicate and malformed updates cannot
produce guessed success. These actions do not advance update acknowledgement.
See SPEC-mtproto-common.md for the complete current contract.

User accounts can read one explicit history page at a time. This example stops
after at most three pages; it does not start an unbounded background scan:

```rust,no_run
use tg_mtproto::{Account, ClientError, Peer};

async fn inspect_history(account: &Account, peer: &Peer) -> Result<(), ClientError> {
    let mut cursor = None;
    for _ in 0..3 {
        let page = account.history(peer, 50, cursor.as_ref()).await?;
        if page.is_inexact() {
            println!("Telegram marked this page as inexact");
        }
        for message in page.messages() {
            println!("{} {:?}", message.id(), message.kind());
        }
        cursor = page.next_cursor().cloned();
        if cursor.is_none() { break; }
    }
    Ok(())
}
```

A short page may have a next cursor. Empty and service records stay visible;
message content is available through explicit getters and Debug is redacted.
Cursors are RAM-only positions bound to account/client/auth epoch and target;
they cannot be used after reopening or as update acknowledgements. Inexact
empty pages fail explicitly instead of claiming complete history. The optional
count describes the whole history, not the remaining number of records.
`HistoryMessage::raw()` is a trusted Rust escape hatch for full pinned fields;
raw media may contain file access hashes and must not be dumped into tools/logs.
See SPEC-mtproto-history.md. History is user-only; bot facades fail before RPC.

Dialogs include a separate pinned phase, followed by ordinary chronological
pages. Match the explicit continuation status; an incomplete page contains
useful entries but cannot prove the scan finished:

```rust,no_run
use tg_mtproto::{Account, ClientError, DialogContinuation, DialogFolder};

async fn inspect_dialogs(account: &Account) -> Result<(), ClientError> {
    let mut cursor = None;
    for _ in 0..3 {
        let page = account.dialogs(DialogFolder::Main, 50, cursor.as_ref()).await?;
        for dialog in page.dialogs() {
            println!("{:?}", dialog.id());
        }
        match page.continuation() {
            DialogContinuation::More(next) => cursor = Some(next.clone()),
            DialogContinuation::Complete => break,
            DialogContinuation::Incomplete(issue) => {
                println!("Dialog scan incomplete: {issue:?}");
                break;
            }
        }
    }
    Ok(())
}
```

Folder/community entries are explicit containers; `peer()` returns a sendable
reference only for a conversation with full cached authority. Cleared dialogs
remain visible. Display names and last-message content require explicit access;
Debug is redacted. Pinned pages share one bounded RAM snapshot and still verify
the current self account on every page. Concurrent pin/unpin/move can repeat
entries; this is navigation, not a transactional snapshot or update checkpoint.
See SPEC-mtproto-dialogs.md. User-only dialogs support Main and Archive API peer
folders, distinct from custom chat filters.

## Send and download documents

Upload and send one document intent, with an independently known content hash:

`SendDocument::owned_bytes(name, mime, owner)` retains an immutable host owner
without a second full file copy. The owner implements `AsRef<[u8]> + Send + 'static`;
the host accounts storage beyond its exposed slice. The ordinary `bytes(Vec)`
entry point still checks retained Vec capacity and wipes its allocation on Drop.
`send_document_until` caps identity, native preparation, parts and final send with
one absolute deadline. `cached_peer_until` also caps the preceding verified cache
lookup when a host supplies an expiring capability.

Accepted calls retain their source from queue admission through joined cleanup,
including caller cancellation, failed reads and final media confirmation. Owner
destruction runs on bounded native work. If a host destructor panics, the client
is fenced and session cleanup continues; its panic payload is intentionally
retained because dropping that payload could panic again. Before admission,
ordinary Rust caller ownership applies. A failed enqueue disposes its rejected
call natively; its permit remains held if that caller cancels the wait.

```rust,no_run
use tg_mtproto::{Account, ClientError, DocumentReceipt, Peer, SendDocument};

async fn send_report(
    account: &Account,
    peer: &Peer,
    protected_absolute_path: &std::path::Path,
    expected_sha256: [u8; 32],
) -> Result<DocumentReceipt, ClientError> {
    let intent = SendDocument::path(
        protected_absolute_path, "report.pdf", "application/pdf",
    )?
        .caption("Report")?
        .expected_sha256(expected_sha256)
        .max_bytes(64 * 1024 * 1024)
        .parallelism(4);
    account.send_document(peer, intent).await
}
```

`SendDocument::bytes(name, mime, Vec<u8>)` transfers an owned buffer without a
full source copy. Constructors perform no file/network I/O. Source reads and
hashing are owned blocking work; preparation, upload and final SendMedia share
one Client deadline and live-account guard. Defaults bound a document to 64 MiB
and four active part calls; host options allow at most 4 GiB/eight calls. Server
account limits still apply. Configure the Client deadline for large transfers.

All parts must be confirmed, and the upload pass must match the prepared
SHA-256 before the final message is dispatched. Small-file MD5 is only the
legacy Telegram checksum. An ordinary error drains pre-admitted part futures
before releasing action capacity. Caller cancellation leaves admitted work
owned and can still result in a message; never replay an Unknown result.
Receipts expose message metadata, size and an explicit SHA-256 accessor. Debug
and static errors exclude content, paths and incidental metadata. Protected
paths are trusted host input; OS I/O itself can block and joined close awaits
owned reads. No agent tool accepts such a path; see SPEC-mtproto-files.md.

Download a document from an exact peer/message, without exposing reusable file keys:

```rust,no_run
use tg_mtproto::{Account, ClientError, DownloadOptions, DownloadReceipt, Peer};

async fn save_document(
    account: &Account,
    peer: &Peer,
    message_id: i32,
    new_protected_absolute_path: &std::path::Path,
    expected_sha256: [u8; 32],
) -> Result<Option<DownloadReceipt>, ClientError> {
    let Some(document) = account.document(peer, message_id).await? else {
        return Ok(None);
    };
    let options = DownloadOptions::default()
        .max_bytes(64 * 1024 * 1024)
        .parallelism(4)
        .expected_sha256(expected_sha256);
    account.download_document_to(&document, new_protected_absolute_path, options)
        .await.map(Some)
}
```

`RemoteDocument` is bound to the originating client, account and authorization
epoch. Its filename/MIME are untrusted display metadata, never output paths.
An expired reference returns `FileReferenceExpired`; explicitly look up the
message again and inspect the new document before starting a new action.
`download_document` returns a zeroizing buffer (at most 64 MiB); `into_bytes`
transfers wiping responsibility to the caller. File output supports up to
4 GiB with host options, using one absolute deadline and at most eight part
calls. Defaults are 64 MiB/four calls. Parts are consumed in order; exact sizes
and optional SHA-256 are checked before the result becomes available. An
aligned size needs an extra empty EOF probe; unexpected extra/short data fails.

File output uses a temporary in the same protected directory and no-clobber
publication; an existing entry, including one created during the transfer,
is never replaced. Failed validation leaves no final file. Native I/O remains
owned after caller cancellation; a deadline during OS publication can return
Unknown while leaving a complete validated destination. Inspect the destination
instead of blindly repeating. Process termination can leave a temporary file;
power-loss durability of the directory is not promised. Protected directory
selection and OS access control belong to the trusted host.

A separate DC is checked against the live expected self. Only exact
AUTH_KEY_UNREGISTERED allows serialized export/import; uncertain import fences
the client before waiters resume. One confirmed FILE_MIGRATE may redirect the
download without changing the home DC. Flood/5xx/ambiguous outcomes are never
automatically retried; unexpected CDN replies are explicit unsupported errors.
No update state is acknowledged by source lookup/download. See
SPEC-mtproto-download.md.

For a user account, the trusted host supplies UI callbacks and an independent
expected ID. `NewAccount` is an explicit alternative for first login; it refuses
remembered accounts and does not grant an agent profile any authority.

```rust,no_run
use std::ops::AsyncFnOnce;
use tg_mtproto::{AccountId, AppCredentials, Client, ClientError};
use tg_mtproto::auth::{AuthStep, LoginCode, Password, PhoneNumber, UserLoginTarget};

async fn login(
    client: &Client,
    app: AppCredentials,
    phone: PhoneNumber,
    expected: AccountId,
    ask_code: impl AsyncFnOnce() -> LoginCode,
    ask_password: impl AsyncFnOnce() -> Password,
) -> Result<AuthStep, ClientError> {
    let step = client.begin_user_login(app, phone, UserLoginTarget::Expected(expected)).await?;
    let step = if let AuthStep::CodeRequired(ref challenge) = step {
        client.submit_login_code(challenge, ask_code().await).await?
    } else { step };
    if let AuthStep::PasswordRequired(ref challenge) = step {
        client.submit_password(challenge, ask_password().await).await
    } else { Ok(step) }
}
```

This example prompts once per stage. Retain a code challenge when handling a
confirmed invalid code if the host wants an explicit retry. Expired/replaced or
cross-generation capabilities refuse locally. Password refusals return a fresh
`PasswordRequired` step with a static reason, without replaying the password
check. Signup/payment/email/unsupported delivery or algorithms return an explicit
`Unsupported` step. Password hints are untrusted text, exposed only by an explicit
accessor; Debug stays redacted. Pass owned password strings/bytes to `Password::new`
to move their allocation into zeroizing storage; host-retained copies remain the
host's responsibility. Keep credentials out of command-line/tool arguments.

SRP uses maintained grammers-crypto with checked parameters and OS randomness.
Its owned blocking work keeps admission until completion/join, independently of
the Session heartbeat. One allowlisted confirmed 303 migration per login job can
change home DC and resend the same bounded request under the original deadline.
Unknown, same/missing DC, unrecognized and repeated redirects never loop. Close
the Client when the host finishes using the authenticated session.

`sign_out(expected)` verifies the live account, durably quarantines the session
before LogOut and ends the generation. Success waits for joined network jobs,
all admitted storage writes and final credential clearing. Unknown/refused or
timed-out logout leaves durable quarantine. `Session::is_quarantined` reports
that local policy state; ordinary Client construction/admission then refuses.
After closing/reopening, a trusted host may explicitly call
`forget_quarantined_authorization` to discard local keys and checkpoints. This
does not revoke unresolved remote authorizations and never replays logout.
See SPEC-mtproto-logout.md. Snapshot payload v3 requires quarantine and update
journal state; old v1/v2 payloads migrate strictly while the encrypted
envelope/SQLite format stays v1.

Snapshots use full-round XChaCha20Poly1305 with a fresh OS nonce and authenticated
profile, application ID, storage ID and revision. A native SQLite worker commits
before publishing the cache. Its bounded jobs retain their slot after caller
cancellation. A heartbeat maintains ownership; expired or failed owners fence
all clones. `close().await` stops admission, drains admitted work and closes the
connection. Dropping the last handle initiates the same drain without blocking
the runtime. Crash recovery waits for the persisted lease to expire.

Backend working update state and application acknowledgements are separate.
Login, peer caching and reads cannot commit a provisional checkpoint. Whole-file
backup rollback and copied credentials cannot be prevented by a per-file owner
lease. Zeroization covers owned key/plaintext buffers on a best-effort basis;
the runtime, operating system and upstream copies are outside that guarantee.

Claim one lazy receiver with `account.updates(UpdateMode::Resume)?`. With no
acknowledged baseline, Resume returns a durable MissingBaseline gap. An initial
`UpdateMode::StartNow` explicitly chooses the current server state; that empty
baseline batch must still be acknowledged. Existing saved checkpoints always
resume, even with StartNow. Neither mode silently drops a pending batch.

Keep the receiver alive and process whole batches, committing the application
store (deduplicated by `batch.id()`) before ACK:

```rust,no_run
use std::time::Duration;
use tg_mtproto::{ClientError, ResyncRequired, UpdateBatch, UpdateItem, UpdateReceiver};

trait DurableSink {
    // Atomically persist the complete batch and its ID before returning success.
    async fn commit(&mut self, batch: &UpdateBatch) -> Result<(), ClientError>;
}

async fn receive_step(
    receiver: &mut UpdateReceiver,
    sink: &mut impl DurableSink,
) -> Result<Option<ResyncRequired>, ClientError> {
    match receiver.next_batch(Duration::from_secs(1)).await? {
        Some(UpdateItem::Batch(batch)) => {
            sink.commit(&batch).await?;
            receiver.acknowledge(&batch).await?;
            Ok(None)
        }
        Some(UpdateItem::Gap(required)) => Ok(Some(required)),
        None => Ok(None),
    }
}
```

One encrypted pending batch survives caller cancellation/restart with the same
ID/content. Checkpoints advance only after exact whole-batch ACK. SDK provisional
state, per-item iteration and peer metadata reads never acknowledge events. Raw
update/user/chat getters are trusted Rust access and must not become agent JSON
authority. Batch/gap capabilities bind the originating client/account/auth epoch;
their deduplication IDs are intentionally non-authorizing public metadata.

Gaps preserve old checkpoints. After host reconstruction or explicitly accepting
skipped history, `receiver.resync_from_current(&required).await?` stages an empty
decision batch; ACK it to commit. Account gaps refresh common counters while
preserving channel checkpoints; channel gaps refresh only that channel through
GetFullChannel. Missing hashes need host peer resolution first. An explicit
`retire_channel` decision can remove only the inaccessible channel named by a
ChannelAccessLost gap; later channel events require a new explicit baseline.
New loss invalidates stale decisions. On queue overflow, `restart_required()` is
true: close/join and reopen the Client/Session before choosing recovery; the
current driver's sticky signal cannot be cleared by ACK.

Validated routing metadata from updates and difference replies is durably cached
before exposing either a batch or a gap. This preserves a new private channel's
access hash for explicit recovery; peer caching itself never advances ACK.

Reception uses the same owned admission/auth/deadline/NoRetries pipeline as other
actions. Polling may return None before `wait` to bound internal work (64 steps)
and retain deadline margin. Applied packets and retained SDK backlog are bounded
to 4096 updates, 1024 total entities and 1 MiB encoded data, with at most 10000
channel checkpoints. The sender queue remains bounded at 100 envelopes; encoded
limits do not establish decoded heap/stack or performance targets. No secret-chat
decryption or exactly-once external side effect is claimed. See
SPEC-mtproto-updates.md; M3 agent integration is implemented through ToolSession
and Host v2. Release gates remain open.

Run `cargo test --locked -p tg-mtproto --all-targets` from the workspace root.
The optional `test-kit` feature adds `test_kit::client` for synthetic socket-free
RPC scripts through the real TL/owned runner path; production `Client::new` never
selects it implicitly. `Session::storage_fingerprint` and receiver/batch/gap
source/account/ordinal getters are stable metadata, not ACK or login authority.
They support the separate agent journal without exposing keys or raw storage IDs.
Tests use synthetic keys, actual encrypted SQLite files, a killed child process,
the SDK-backed RPC fixture and an encrypted loopback connection. No Telegram
account or live server is contacted. The full contract is
in SPEC-mtproto-client.md and SPEC-mtproto-session.md at the repository root.
