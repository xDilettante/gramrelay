# Full completion tasks

See tasks/goal.md. The active goal includes every unchecked module below.

## Bot runtime

- [x] R1: typed get_me/get_updates/get_webhook_info and preservation of nested unknown event fields. Verified: 3 update tests, all 26 unit/integration tests, 2 doctests and Clippy pass.
- [x] R2: Router<State>, typed contexts, exact commands/mentions and context-preserving replies. Verified: 4 routing tests, Clippy and client-only build pass; complete regression suite passed.
- [x] R3: one process-local poller, bounded per-chat batches, safe offset barrier, cancellation and handler failures. Verified: 11 polling tests, all 42 unit/integration tests, 2 doctests, client-only tests, fmt/Clippy/rustdoc and audit pass. Unknown event scopes serialize their batch; durable recovery is still J2.
- [ ] R4: authenticated webhook ingestion and transport adapter, limits, overload and shutdown. Depends J2 for durable mode. Verify: localhost webhook/recovery tests.

## Protocol and client

- [x] P1: pinned 10.4 structural schema (193 methods / 425 types), standard-library importer and reproducible Rust inventory catalog. Verified: 13 Python tests, complete reference/inventory checks, byte-identical re-import/regeneration, Rust catalog test and full regressions. Adversarial review corrected a conditional numeric fact and added fail-closed markup/union regressions. Full callable Rust types/methods remain P2.
- [ ] P2: all pinned Bot API methods/types, fixtures, effect/replay metadata and compatibility cases. Types implemented: all 425 definitions, strict literal/union decoding, read-only unknown fields, bounded Update/enum storage; 425 structural fixtures and 5 compatibility tests pass. Methods remain open. Depends P1. Verify: generated API tests and coverage manifest.
- [ ] C1: rich text/entities, complete contextual replies/files, download limits and opt-in safe retry policy. Depends P2. Verify: Unicode, streaming and failure tests.
- [ ] C2: optional tracing/metrics and narrow integration features. Verify: privacy and feature-matrix checks.

## Durable state and agent tools

- [ ] J1: SQLite operation journal, atomic claims, payload/owner validation and Unknown recovery. Verify: concurrent/crash tests.
- [ ] J2: durable update ingestion, acknowledgements, retention and executor ownership. Verify: crash/replay/TTL tests.
- [ ] A1: shared authorized executor with grants, bot/chat scopes, quotas and file handles. Depends J1/P2. Verify: policy bypass and cached-result access tests.
- [ ] A2: CLI JSON with versioned input/output schemas. Depends A1. Verify: executable-level offline tests.
- [ ] A3: MCP stdio adapter over the same executor, stable tool listing and correct annotations. Depends A2. Verify: protocol integration tests.

## MTProto

- [ ] M1: backend spike/comparison and recorded decision; session secret/lifecycle contract.
- [ ] M2: separate client, authentication/session lifecycle, common actions, raw access, history and recovery. Depends M1. Verify: mock backend and restart/flood-wait cases.
- [ ] M3: integrate agent profiles/policies and shared operational semantics. Depends M2/A3.

## Release gates

- [ ] Q1: task-based docs, migration guide, examples, agent index and 30-task evaluation.
- [ ] Q2: reproducible comparable benchmarks and memory/concurrency bounds; achieve measured acceptance targets.
- [ ] Q3: platform/MSRV matrix, feature checks, dependency audit and local release packaging.
- [ ] Q4: final correctness/security/simplicity review; all scope implemented and verification evidence recorded.

## Completed prototype

- [x] Bootstrap crate and local architecture. Verify: Cargo manifest and source document.
- [x] Message and failure semantics. Verified: exact JSON and typed response; one request on timeout; local validation sends nothing. cargo test --test messages: 3 passed.
- [x] Callback slice. Verified: inline keyboard JSON, Update decoding and answer callback. cargo test --test callbacks: 2 passed; message regressions passed; Clippy clean.
- [x] File slice. Verified: bytes/path streaming and file ID; upload bounds before dispatch. cargo test --test documents: 4 passed; all 9 integration tests and Clippy passed.
- [x] Review and handoff. Verified on Windows / Rust 1.98.0: 23 unit/integration tests, 2 doctests, all 4 examples compile, offline mock example runs; Clippy and rustdoc pass; cargo audit finds 0 vulnerabilities and no warnings. README records limits and deferred features.
