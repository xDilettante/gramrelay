# 0016: grammers with an explicit safety adapter for tg-mtproto

Status: accepted for implementation in M2, 2026-11-16. Production integration is
not complete. Required downstream fixes below are acceptance gates, not claims
about the unmodified dependency.

## Decision

Use pinned grammers 0.12.0 for the separate Rust tg-mtproto Client, including its
maintained TL/MTProto/cryptography crates. Do not ship its defaults unchanged.
Disable default session SQLite storage, use NoRetries, implement the encrypted
Session contract, and provide an observable update/lifecycle boundary. Bot API
remains a separate Client/transport/API.

Both alternatives were exercised locally. grammers exposes the typed TL raw
surface directly and fits the Rust API and Cargo distribution. TDLib is a viable
native alternative with broad client functionality and managed state, but adds
a substantial native artifact/build and a JSON/C boundary. This project needs
direct typed MTProto access and explicit operation policy; it will own a small
reviewed safety adapter rather than a second C++ runtime. Reopen this decision
if the required changes cannot remain narrow and reviewable. No cryptographic
primitive or MTProto encryption implementation is replaced by the adapter.

## What was actually verified

Windows x86_64, Rust 1.98.0. The isolated experiment workspace and its lockfile
are in experiments/mtproto; it does not change production dependencies.

| Boundary | grammers 0.12.0 | TDLib 1.8.70, commit 03220f6b3043c6fd7785fbf0901d63f19ad19974 |
|---|---|---|
| Executed probe | Actual Rust typed raw API, synthetic DC key/checkpoint import and serialization | Actual locally built C ABI via Python ctypes: UTF-16 entities, @extra correlation, wait-for-parameters state and close |
| Retry control | Closing loopback socket: NoRetries makes one connection; default makes two after I/O failure | Fine-grained in-flight retry/cancellation policy not exercised |
| Caller cancellation | Queue work begins connecting even after its caller future was dropped | Credential-free client closes; active authenticated operation cancellation not exercised |
| Session storage | Session trait supports a custom store; default key-bearing types are unsuitable as public diagnostics; MemorySession does not resolve the self-user sentinel | Native encrypted storage is an upstream capability; no private database was opened by the probe |
| Update recovery | Source inspection found hidden gaps, backend-written checkpoints and shutdown/overflow risks; explicit adapter required | Native state consistency is provided upstream; application acknowledgement remains a separate integration problem, not proven by this probe |
| Raw access | tl::RemoteCall compiles against the real generated types | Public TDLib JSON API; arbitrary raw MTProto RPC access was not demonstrated |
| Distribution evidence | Cargo builds and Clippy pass; audit of the 167 experiment dependencies passes | CMake 4.4.1/GCC 16.1.0, hash-verified OpenSSL/zlib/gperf inputs; native library built and loaded successfully |

The native build used two jobs and -O1, taking 1516 seconds in this one local
run. Its unstripped libtdjson.dll is 101924427 bytes and statically includes the
selected OpenSSL/zlib libraries. This is build/distribution evidence only:
caches, concurrent work and optimization differ. It is not a performance,
startup, runtime-memory or Rust-vs-C++ benchmark and does not satisfy Q2.
ctypes exercises the C ABI; it is not a completed Rust TDLib backend.

## Required safety work before M2 is complete

1. A bounded owned job retains capacity/buffers after caller cancellation.
   Configure NoRetries and classify ambiguous effects explicitly. Never create
   a new operation or random_id to recover an uncertain write.
2. A narrow sender lifecycle patch must stop connection setup cooperatively,
   abort and join every child on forced shutdown, and prove that no old task
   survives before a successor driver is admitted. Awaiting an aborted outer
   JoinHandle alone does not prove this.
3. A nonblocking observable update-overflow signal must preserve RPC progress.
   Waiting on a full updates queue in the same sender that serves GetDifference
   can deadlock; rate-limited logs are not a loss signal.
4. Use an observable receiver below the native UpdatesReceiver, reusing public
   maintained MessageBoxes. Detect irreversible differences/missing peer hashes
   before applying them. Preserve an existing channel pts when processing
   updateChannelTooLong. Commit durable Gap/ResyncRequired rather than treating
   missing history as a successful empty batch.
5. Backend provisional state and application-acknowledged state are distinct.
   Login/dialogs/GetState do not acknowledge events. New application receivers
   start from the acknowledged state; snapshots for keys/peers cannot advance
   it. Explicitly select a first baseline, append whole batches durably before
   checkpointing, and deduplicate redelivery.
6. Implement the bounded encrypted session and owner fencing described in
   SPEC-mtproto-session.md, including expected account/kind verification,
   self-user lookup, redacted secrets, key provisioning and close/restart tests.

Keep any downstream patch pinned, licensed, reproducible and independently
reviewed; document its diff from upstream and verify it during dependency
updates. The spike does not yet contain or claim the production patch.

## Sources and limits

- Exact crates.io source: grammers-client/session/mtsender 0.12.0, especially
  configuration.rs, sender_pool.rs, updates_receiver.rs, client/auth.rs,
  client/dialogs.rs, Session and MessageBoxes. https://docs.rs/grammers-client/0.12.0/grammers_client/
- Official TDLib source pinned above, td_json_client.h and td_api.tl:
  https://github.com/tdlib/td/tree/03220f6b3043c6fd7785fbf0901d63f19ad19974
- https://core.telegram.org/tdlib/getting-started
- https://core.telegram.org/constructor/updateChannelTooLong
- https://core.telegram.org/api/updates
- https://docs.rs/chacha20poly1305/0.11.0/chacha20poly1305/

Only synthetic material and localhost sockets were used. No sign-in,
setTdlibParameters, real account session, live Telegram RPC, cross-platform test
or successful application update recovery was exercised. Those implementation
and verification obligations remain M2/M3 and the release gates.
