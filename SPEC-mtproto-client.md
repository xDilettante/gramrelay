# MTProto implementation (M2)

Implement the separate tg-mtproto package against SPEC-mtproto.md,
SPEC-mtproto-session.md and ADR 0016. The complete M2 remains open until every
capability below has a real backend and meaningful mock/loopback coverage.

## Implementation order and boundaries

1. Encrypted SQLite Session: private backend adapter, explicit key/binding,
   bounded owned native jobs, single-owner lease, durable fencing, separate
   provisional/acknowledged state, redaction, close and restart.
2. Pinned backend safety changes: whole-tree joined shutdown and nonblocking
   sticky overflow observation. Preserve the maintained protocol/crypto code;
   record and verify any vendored diff. Client jobs retain admission/buffers
   after caller cancellation and stop an expired generation before replacement.
3. Lazy Client and auth: validated app credentials, stored-account verification,
   phone/code/2FA flow, bot login, explicit sign-out/quarantine and recovery.
   Tokens/challenges remain private host capabilities, never agent tool input.
4. Common typed actions: identity, peer lookup/dialogs, send/edit text, document
   upload/download and bounded paginated history. Raw typed TL access remains
   available to trusted Rust callers, with conservative failure/replay defaults.
5. Observable updates: maintained MessageBoxes behind checked control/difference
   handling, explicit gaps/resync, durable batch-before-checkpoint interface,
   bounded buffering and cancellation. Never hide lost history behind an empty
   successful batch or persist provisional state as acknowledgement.

Agent grants/approvals, tool schema/CLI/MCP integration and durable agent action
admission are M3, sharing this client and the existing operational semantics.
Do not substitute mock-only execution for the production backend.

## Client ownership increment

Client construction validates host app credentials/options and claims one client
per Session across clones. It starts no network activity. The first polled call
starts a private supervisor and lazy grammers pool with NoRetries. App API hash,
bot tokens, phone/code/password and challenge capabilities have redacted Debug;
no challenge can be serialized or reused across client generations.
Claim/close admission must be one atomic state transition. While attached to a
Client, public Session::close refuses with an explicit owned/attached error;
Client::close performs the private storage close only after the driver barrier.
Dropping an unstarted Client may release its claim; a started unjoined generation
must never release it merely because a waiting future or supervisor was dropped.

A bounded command/active-job budget precedes the upstream unbounded channel.
Once admitted, a job and its input remain owned until completion or joined
shutdown even if the caller drops its future. Each job carries an absolute
deadline covering identity verification and backend work. On deadline, storage
fencing, shutdown or backend panic, close admission for the whole generation,
return conservative outcomes, call quit, await the whole driver tree, then
abort/join residual client jobs and drain Session. No successor may start before
that barrier. A timeout can return Unknown while cleanup still owns capacity;
close().await confirms completion. Dropping a handle never reports joined close.
Local serialization/validation and command-queue expiry before an owned backend
job starts return NotSent without restarting an otherwise healthy generation.
An owned job's deadline seals its generation, including an unsent job: tracked
dispatch state distinguishes NotSent from Unknown. Completed RPCs are excluded
from deadline scans even while their result-delivery task is still unwinding.
An unsupported Tokio timer runtime is rejected before transferring the claim;
unstarted close requires no timer. Normal cleanup never externally aborts the
runner in place of awaiting its child barrier.

Common actions verify expected/stored account identity before account data or
write effects. Login and logout serialize authorization transitions. A logout
intent durably quarantines the session BEFORE dispatch; only confirmed remote
completion plus local commit clears credentials. An uncertain outcome preserves
quarantine for explicit host recovery, even across restart.

The high-level SDK fs feature stays disabled: its concurrent downloader spawns
workers and an unbounded chunk channel. Use owned bounded file iteration/writes
instead. Raw typed TL access remains trusted Rust-only, shares admission,
deadlines and conservative write classification, and is never a generic agent
tool. Known RPC refusals are Rejected; ambiguous transport/decode/late completion
and server failures remain Unknown. Only allowlisted flood-wait numbers survive
error normalization; remote descriptions and raw source errors do not.

Test RPC injection is a downstream opt-in test feature only. It replaces the
network dispatch with owned runner children while preserving SDK request
serialization, typed decoding, NoRetries, update-loss signaling and joined quit.
It must never be enabled implicitly by the production Client, accept tool JSON,
or use live Telegram credentials. Synthetic callbacks run concurrently and are
cancelled/joined through the same runner child tree.

## Session API and storage increment

Session::open takes an absolute protected path, SessionBinding(profile, app ID),
SessionKey and SessionOptions. It performs no network I/O. SessionKey is an
opaque non-serializable 32-byte host capability with redacted diagnostics.
Session/option errors retain static kinds/codes, never raw SQLite/crypto causes.
Successful close affects every clone, drains admitted work and releases the
owner. Drop closes admission without blocking the Tokio worker.

The public Session does not implement the upstream Session trait: a private
adapter exposes backend key material only internally. Public account/status
access reveals intentional metadata, not reusable credentials. Mutations are
committed by one native thread; cache publication occurs after durable commit,
so cancelling a waiting future cannot leave disk and memory divergent.

Version 1 uses one encrypted row per database with a persistent random storage
ID, revision, nonce/ciphertext and owner lease. AAD authenticates storage ID,
revision, domain/version, host profile and app ID. Header/encoded length limits
are checked before taking owned buffers; codec/data limits apply on decrypt.
Unsupported schema or wrong key/context never resets the old row. Snapshot
mutations and ownership checks are atomic. No plaintext key or peer hash is
placed in SQL parameters, database/WAL/journal, diagnostic strings or tracing.

Initial defaults: 8 MiB encoded snapshot, 10000 peers/channels, 32 datacenters,
16 pending jobs, 60-second owner lease. Limits are explicit and validated;
unknown/oversized/invalid state fails closed. The acknowledged checkpoint is
optional until an explicit first baseline. Backend state changes do not create
one implicitly. Heartbeat and every publication enforce the owner/deadline;
lease failure fences the session and its backend generation.

## Verification

- cargo test --locked -p tg-mtproto --all-targets
- cargo test --locked --workspace --all-targets --all-features
- cargo test --locked --doc
- cargo fmt --all -- --check
- cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
- cargo audit

Use synthetic credentials/state and localhost only. Storage tests cover actual
encrypted files and corrupt/wrong-context reopen, competing owners/processes,
cancelled native jobs, fencing/expiry, close/drop, bounded capacity and restart.
Backend tests prove shutdown of children and connection setup, overflow while
RPC progress continues, NoRetries, late completion and no ambiguous replay.
Auth/common/raw/history tests share production serialization/decoding where
possible. Updates tests cover gaps/TooLong/channel hints, empty batches,
provisional state, bootstrap, crash/redelivery and application acknowledgements.
Do not infer authenticated Telegram compatibility or platform/MSRV coverage
from mocks. The broader goal and release gates remain active.

Storage increment evidence (Windows, Rust 1.98.0, 16 November 2026): 18 session
checks plus the Bot API journal schema regression pass; full workspace has 361
all-feature and 197 default tests, four doctests, clean Clippy/fmt, 21 Python
schema checks, deterministic protocol/agent artifacts and clean audit of 320
dependencies. One ignored subprocess fixture is invoked by the crash test.
This evidence does not complete M2 or claim authenticated network compatibility.

Initial backend patch increment: the pinned grammers-mtsender archive was
vendored with sender_pool.rs/Cargo.toml changes plus local tests/provenance. Eight
downstream regressions and the upstream parser unit pass. Quit/setup/child join,
panic cleanup, post-quit refusal, sticky update loss, NoRetries I/O and flood
refusal are covered. tools/verify_grammers.py checks archive SHA-256, unchanged
source bytes and the exact recorded patch; its three tests bring Python checks
to 24. Whole-workspace all-feature tests, doctests and Clippy remain passing.
The owning Client must still provide admission, deadlines, joined replacement,
authorization and observable application acknowledgement; the backend patch
does not supply those by itself.

Client ownership/raw increment: lazy unique attachment, admission before bounded
serialization, owned jobs after caller cancellation, absolute deadlines, session
fencing and whole-driver joined close are implemented. The network backend uses
NoRetries. Ten client lifecycle regressions include late unsent jobs, completed
jobs pending collection, cancelled result destructors, unavailable Tokio timers,
late session fencing, cancelled close waiters and capacity held through child
cleanup. A public attachment test and a production Client encrypted-loopback
test cover the ordinary constructor; I/O loss opens exactly one connection and
returns Unknown. These join the eighteen session checks for thirty package tests.

Pinned incoming parser increments separately vendor grammers-mtproto and
grammers-tl-types 0.12.0. Response bounds precede SDK setup/control/error/update
decoding. Exact TL consumption, canonical byte lengths/zero padding, cumulative
gzip expansion, all gzip members and envelope depth are checked. Rejected
containers restore receive state and queued results/acknowledgements without
replenishing the expansion budget. Malformed own-updates/control failures remain
observable. Source verification checks all three archives, unchanged files and
recorded diffs; crypto/key derivation, schemas and generators are untouched.
There are 59 core, three TL primitive and 12 sender unit tests (16 with its
development fixture), plus 24 upstream TL doctests. Focused independent reviews
found and fixed lifecycle, early parsing and rollback gaps with regressions.

Whole-workspace evidence on Windows/Rust 1.98.0, 17 November 2026: 459 all-feature
and 295 default tests, ten doctests, clean Clippy, 25 Python checks and audit of
363 dependencies. Follow-up auth/code/2FA/logout, peer/text, history, dialogs and
document upload/sending are
implemented in SPEC-mtproto-auth.md, SPEC-mtproto-interactive.md,
SPEC-mtproto-logout.md, SPEC-mtproto-common.md, SPEC-mtproto-history.md and
SPEC-mtproto-dialogs.md and SPEC-mtproto-files.md,
using this owned pipeline.
No authenticated Telegram compatibility, generated decoded-heap/stack bound,
external-consumer packaging or cross-platform claim follows from this increment.
Downloads/DC authorization, observable updates, M3 and release gates remain open.
