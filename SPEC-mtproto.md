# Separate MTProto client (M1–M3)

## Objective and sequence

Deliver the separate tg-mtproto Client promised in docs/ARCHITECTURE.md. Bot API
and user/bot MTProto accounts retain different APIs and authorization models.
Do not implement cryptography or MTProto from scratch. No live Telegram login,
API invocation or use of existing private sessions is authorized by this work.

M1 first compares two real backend prototypes: pinned grammers 0.12.0 Rust APIs
and the official TDLib JSON C ABI from a pinned source revision. Compile/run
offline probes, record build prerequisites, retry behavior, session lifecycle,
update recovery and distribution costs. Write a reviewed backend decision and
the session/secret contract before marking M1 complete. Absence of credentials
does not block construction or local protocol/serialization probes.

M2 adds a separate production crate with auth/session lifecycle, typed common
actions, raw access, history pagination and recovery. M3 integrates distinct
account profiles and current agent policy, approval, quotas and journal rules.
Finish those later modules and release gates before completing the full goal.

## M1 experiment layout and commands

experiments/mtproto is an isolated, unpublished Cargo workspace. Its dependency
experiments must not pull C++ tools or alternate SQLite implementations into
the Bot API workspace. Production stays unsafe-free; experimental C ABI probes
are clearly isolated, never used as a production transport.

- cargo run --manifest-path experiments/mtproto/Cargo.toml --locked --bin grammers_probe
- python experiments/mtproto/prepare_tdlib.py
- cmake --build target/mtproto-spike/tdlib-build --target tdjson --parallel 2
- python experiments/mtproto/tdlib_probe.py target/mtproto-spike/tdlib-build/libtdjson.dll
- cargo test --manifest-path experiments/mtproto/Cargo.toml --locked

The TDLib preparation script downloads pinned sources and hash-verified native
dependencies into the repository's ignored target/mtproto-spike only. It never
installs global software or deletes existing project/user data. The C ABI probe
must demonstrate synchronous JSON execution, request/response correlation,
authorization-state discovery before credentials, and explicit close. No
setTdlibParameters/sign-in request is sent by a probe.

The grammers probe must compile the actual typed raw API, test NoRetries on
flood-wait and I/O errors, import a synthetic key/update state through Session,
create/drop a lazy pool without a network runner, and demonstrate why an
unbounded backend queue needs admission in the wrapper. A loopback-only error
probe may run the pool with synthetic endpoints to observe connection/retry
behavior. Do not direct the pool at default public datacenters.

## Session and failure invariants to resolve in M1

- Separate application credentials, interactive login secrets and reusable
  account session material. No Debug/Display/source chain or tool JSON may
  disclose API hashes, codes, passwords, auth keys or reusable login tokens.
- Persist identity, datacenter keys, peer cache and update checkpoints with
  bounded storage and an explicit owner. Specify protection at rest, key
  provisioning, atomic commit, cancellation, concurrent-open and close/restart.
  A backend's plaintext default session is not the product's secret contract.
- Verify the authorized account before execution and after session restore.
  A bot-profile grant cannot authorize an arbitrary user account session.
- Default RPC retries and flood sleeps must be disabled or explicitly bounded
  by documented effect/replay policy. Transport-level protocol correction is
  different from starting a new application operation.
- Cancellation/timeout after dispatch is Unknown. Dropping a future may leave
  backend work running. Bound and join backend shutdown, retain the operation
  key and never blindly reissue an uncertain write.
- Updates need a durable receive/processing checkpoint contract. Backend
  auto-catch-up or update persistence is not evidence that our handler committed
  an event. Preserve per-account/peer order and explicit gaps/overflow.
- History pages use bounded limits/cursors; raw access is typed and explicit
  for Rust callers, and never an authority bypass for an agent.

## Evidence and decision

Record exact versions/revisions, platform/toolchain, successful commands,
source references and failed experiments that affect the choice. A compile or
offline C ABI probe proves that boundary only; it does not prove successful
Telegram authentication, server compatibility or cross-platform operation.
Use fresh-context review for the session/failure boundary before choosing a
backend. Requirements above remain active even if a prototype is inconvenient.
