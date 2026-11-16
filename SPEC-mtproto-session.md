# MTProto session, secret and lifetime contract

Status: accepted M1 contract after adversarial review; production implementation
and its proofs belong to M2. See docs/decisions/0016-mtproto-backend.md.

## Ownership and secrets

The tg-mtproto Client owns a distinct account session. It is not a Bot API Bot
with a replacement transport. Application API ID/hash, phone login code, 2FA
password/challenges and reusable authorization keys have separate types and
lifetimes. Only a trusted Rust host supplies credentials or conducts login.
Agent tools cannot log in, import sessions, supply credentials or grant consent.

Own credential/session/challenge types have redacted Debug/Display and no
Serialize implementation that reveals reusable material. Error sources do not
retain backend errors, request bodies, phone numbers, hashes or key bytes. Raw
backend session types are not the public persistent-session API; upstream
DcOption derives Debug with auth_key, so copying that API would disclose keys.
Backend diagnostic output must be filtered before enabling verbose logging.
The custom Session must resolve PeerId::self_user from the verified self record;
the upstream MemorySession used in probes only does exact map lookup.

API credentials are validated before any network task. Own password and
temporary plaintext buffers use zeroize. This does not promise elimination of
all copies made by backend code, the allocator or the compiler. Session keys
remain in memory while a client or an admitted native job needs them.

## Persistent storage

Use an encrypted, bounded session snapshot with a dedicated SQLite database
and the same maintained rusqlite version family already used in the project.
Disable grammers-session default sqlite-storage; it introduces a different
SQLite stack and does not provide this encryption/ownership contract.

Encrypt with maintained RustCrypto chacha20poly1305 0.11.0 XChaCha20Poly1305,
using a host-provisioned 32-byte master key, a fresh 24-byte OS-random nonce per
commit and an authenticated versioned context. Use its full-round construction,
alloc/zeroize support and getrandom; no custom cryptographic primitives or KDF.
The master key never resides in the database, tool JSON, examples with real
accounts or command-line arguments. The host provisions it from protected
storage; the SDK does not discover the user's existing sessions or credentials.
Losing the key means losing local access, rather than silently resetting state.

The version-1 context binds a domain separator, storage/session identity,
operator profile identifier and application API ID. Account kind and verified
account ID are authenticated inside the snapshot. An agent registers a profile
only after comparing that identity with its independently configured expected
account and kind. Restoring bytes never authorizes a different account.

Store home DC, all permanent DC keys, peer identity/access-hash cache, account
identity and acknowledged update state. Keep provisional backend state distinct:
login, dialogs and backend GetState can call Session::set_update_state without
any application acknowledgement. Those calls never change the persisted
acknowledged position, including in snapshots triggered by unrelated key/peer
mutations. A recovering application receiver initializes from the acknowledged
state explicitly, not whichever provisional state a backend helper last set.
Temporary login tokens/codes/passwords are
not persisted. A failed/incomplete login after restart can request a new
challenge while retaining already generated encrypted session keys.

Nonce, ciphertext, snapshot revision and lease fencing must commit atomically.
Use WAL/FULL; database/WAL/journal files contain ciphertext rather than plaintext
auth material. Decode only after authentication, with an 8 MiB default snapshot
limit and explicit peer/channel caps. Reject tampering, wrong keys/context,
unsupported versions and capacity overflow without replacing the old snapshot.
Metadata/header limits are checked before allocation/decryption. Do not silently
evict the self-user or required channel hashes to satisfy a limit.

One database/profile has one mutable client owner with a renewable lease and
monotonic local deadline. Snapshot writes carry owner plus revision fencing.
Bound the worker queue; cancelled callers leave buffer/permit ownership with
admitted native work. Report committed results only after durable commit. Drain
or fail pending jobs and release ownership before successful explicit close.
Do not erase or overwrite keys merely because a lease or process disappeared.

Encryption does not make the database resistant to restoration of an entire
older valid backup. The operator must restore session and application journals
consistently. Ownership is per store: copying a valid session to another store
cannot be prevented by a local lease. Session blobs remain private credentials.

## Runtime and cancellation

Configure grammers NoRetries. The observed default retries I/O and sleeps on
flood waits; neither behavior may silently control an authorized write. Expose
allowlisted numeric flood-wait advice as a confirmed refusal. Any later retry
is explicit and bounded by effect/replay policy and the original operation.
MTProto transport correction for a rejected protocol envelope is not a new
application operation; do not claim that NoRetries disables that correction.

Bound calls before they enter the backend's unbounded request channel. A job
owns its capacity and input buffers until backend completion or joined driver
shutdown, even when its caller future is cancelled. Backend caller cancellation
does not remove queued work. At an absolute deadline, close/abort and join the
affected driver before constructing a successor; do not leave a background
queue delivering writes after a new generation is admitted. Other calls on that
driver can become Unknown; report that explicitly. Shutdown/reconnect never
replays application writes. An already dispatched remote effect is not revoked
by local cancellation, policy expiry or loss of the ownership lease.

The upstream runner owns private child sender tasks. Aborting and awaiting only
the outer runner is insufficient: Tokio JoinSet Drop aborts children without
joining them. The adapter must own a confirmed shutdown boundary for the entire
tree. The selected integration requires a narrow reviewed backend lifecycle
patch with cooperative stop during connection setup, explicit abort_all plus
join_next draining on forced stop, and joined normal shutdown. Source pinning,
patch verification and child-task termination tests are M2 acceptance gates;
do not claim them from an ordinary quit probe with no active children.

Classify local validation/capacity failures before dispatch as NotSent, confirmed
RPC refusals as Rejected, and ambiguous I/O, timeout or cancellation after
dispatch as Unknown. Preserve a durable agent operation key. Any deduplication
random_id used by a journaled write belongs to its immutable intent; generating
a different key or random_id after an uncertain attempt is not recovery.

Explicit sign-out gates other calls, awaits the remote response, then commits
the cleared/revoked local state. An uncertain sign-out quarantines the session
until explicit recovery; it does not report that Telegram invalidated the key.
Local reset and remote revocation are separate operator actions.

## Updates and recovery

Enable catch_up explicitly; grammers defaults to false. Persist peer hashes
needed for channel difference requests. The network update queue is bounded
but upstream may drop updates on overflow; surface gaps/recovery rather than
claiming that bounded buffering alone prevents loss.

Application update acknowledgement is separate from backend reception. The
unmodified native UpdatesReceiver is not the application boundary: it hides
DifferenceTooLong/ChannelDifferenceTooLong and missing peer hashes behind empty
batches or state changes. Use an observable receiver over the public maintained
MessageBoxes plus raw sender updates. Inspect difference results before applying
them; durably expose Gap/ResyncRequired with an account/channel/reason and stop
acknowledgement until the recovery decision is committed. Never turn an
irrecoverable gap into an ordinary empty acknowledged batch.
Inspect raw control updates before MessageBoxes too: updateChannelTooLong must
preserve an existing channel pts, rather than replacing it with the server's
hint. Use the hint only when no local baseline exists and make that initial
baseline/resync decision explicit. Pin this regression to the official Telegram
constructor contract; do not inherit upstream's unconditional pts replacement.

The selected integration also requires a narrow reviewed sender patch replacing
silent update-channel drops with a nonblocking observable overflow. A full
updates queue must not stop RPC progress: awaiting send inside the same sender
that services GetDifference replies creates a deadlock. On overflow, publish a
bounded sticky gap signal outside that full queue so a later quiet connection
cannot hide the loss. Shutdown remains interruptible; an application must not
commit a checkpoint across an unacknowledged overflow. Merely increasing
capacity or watching rate-limited logs does not meet this contract. These
downstream changes do not alter MTProto cryptography.

Consume whole batches from that observable boundary, preserve order, durably
append all events and required peer data, and only then commit their immutable
checkpoint to the session. A crash between append and checkpoint may redeliver,
so durable deduplication is required. Never synchronize a high-level UpdateStream
after only one item when it still buffers the rest of an applied batch. Normal
SDK login, dialogs reads and reconnect do not acknowledge application events.

Initial GetState/baseline establishment, empty batches, channel gaps, overflow,
checkpoint failure, consumer failure and close are explicit cases to cover in
M2/M3 tests. No authenticated live session is needed to exercise a mock backend.
Only committed state can be restored as the application's acknowledged offset.
An account with no acknowledged baseline must explicitly choose StartNow or a
resync workflow; Resume does not silently substitute the server's current state.
