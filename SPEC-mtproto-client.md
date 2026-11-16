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

Backend patch increment: the pinned grammers-mtsender archive is vendored with
only sender_pool.rs/Cargo.toml changes plus local tests/provenance. Eight
downstream regressions and the upstream parser unit pass. Quit/setup/child join,
panic cleanup, post-quit refusal, sticky update loss, NoRetries I/O and flood
refusal are covered. tools/verify_grammers.py checks archive SHA-256, unchanged
source bytes and the exact recorded patch; its three tests bring Python checks
to 24. Whole-workspace all-feature tests, doctests and Clippy remain passing.
The owning Client must still provide admission, deadlines, joined replacement,
authorization and observable application acknowledgement; the backend patch
does not supply those by itself.
