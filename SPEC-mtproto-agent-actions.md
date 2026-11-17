# MTProto durable action execution (M3)

Extends SPEC-mtproto-agent.md, SPEC-journal.md and SPEC-executor.md. Implement the
shared durable write path with real M2 text actions first. This increment supports
get_me, get_peer, read_updates, send_message, edit_message_text and get_operation.
Callback/document actions keep explicit UnsupportedAction until their capability
providers are implemented; their existing typed requests/policy never become
execution permits by themselves. No live Telegram or external publishing.

## Exact durable scopes

Strict journal v8 migration verifies/preserves v1–v7 schemas, operations, update
feeds, profile bindings, files and budgets. Add separate MTProto operation-scope
rows to the same operation key namespace and native worker. Persist exact MT bot
or user account kind/ID, MT action, tagged primary peer, optional immutable file
metadata and optional canonical callback hash. Validate exact durable account at
prepare, admission and disclosure; never derive scope from result JSON or tool
claims. Keep the Bot scope table, ID meanings and admission behavior unchanged.

Scope rows reference operations and immutable profiles. Peer kind/ID, file tuple
and callback tuple must be complete and action-consistent. Only MT bots can own
callback scopes. A partial unique index permanently binds an admitted canonical
callback to one operation across principals, including after NotSent/Unknown.
No callback capability registration is added by merely storing this metadata.

Prepare scope and operation atomically. Existing keys require exact owner, action,
request fingerprint and scope, or refuse. A generic old operation without an MT
scope is not an authorized receipt. Durable begin rechecks current policy and
exact account, marks callback admission and charges write/upload/cost quotas in
the same transaction. Failed admission rolls back every change. Invocation quota
is charged separately; replay consumes an invocation but no new write/cost quota.
Reuse J1 fences, terminal retention, unknown recovery and fair confirmation.

## Executor and lifecycle

Host registers an immutable Feed and already configured M2 Client; no session/key,
login/raw RPC/path/username configuration comes from tool JSON. Expected identity
comes only from Feed. Reject duplicate profile/account aliases and invalid network
budget plus receipt-margin/lease combinations before RPC. Profile count, active
jobs and source records/bytes are bounded. GetPeer is a verified cache availability
lookup, not arbitrary username resolution or a claim of a live remote peer.

One current shared PolicyStore controls exact account/actions/peers/files,
approvals, expiry and budgets. Dry-run/approval resolve retained operation scope
through owner-checked metadata only, without state refresh, quota or RPC. A
get_operation grant must also retain the original write action/resources; original
write approval is not requested again to read a receipt. Current get_operation
approval still applies. read_updates reuses the reviewed Reader provider.

Each execute invocation is an owned tracked Tokio job retaining its concurrency
permit through SDK work and journal confirmation after caller cancellation.
Bound and reap completed handles. close stops admission and joins all accepted
jobs, including after a cancelled close wait; the host then joins Client before
closing Journal. Executor does not close independent host resources. Dropping the
last executor aborts wrapper jobs; already accepted M2 work can finish, so recovery
stays conservative. Drop does not claim a durable receipt or joined shutdown.

Before a new text write, validate intent and resolve a cached opaque peer with
live identity verification, recheck authority, then commit durable begin. M2
send/edit actions accept an absolute monotonic deadline cap inside the owned
Client job, bounded by the operation lease minus receipt margin. Verification or
policy waits cannot move the deadline forward. Expired cap refuses dispatch;
M2 NoRetries, identity/epoch checks, exact receipts and conservative outcomes remain
authoritative. The random ID is a per-attempt implementation detail, not an agent
key or permission to retry Unknown. Only confirmed NotSent can execute again.

Wait for owned native confirmation rather than detaching its completion. The SDK
deadline bounds RPC work; native queue/SQLite limits govern receipt persistence.
Persist success/failure before disclosure and recheck current scope afterward.
Storage/receipt ambiguity after begin is Unknown, never safe-to-replay NotSent.
After receiving an owned job's result, the caller synchronously checks current
account/owner/original scope/approval and the result's authorization revision
immediately before returning, with no later await. A receipt/page buffered while
the caller is descheduled cannot bypass revocation; Reader snapshot revision must
be preserved through this extra job boundary rather than resampled afterward.
Cached/get_operation reads do not perform an identity/target RPC or revive an old
operation. Revocation can hide results but cannot undo an admitted remote effect.

## Safe output and acceptance

Allowlisted results: verified account kind/decimal ID, tagged peer plus cached
availability, safe Reader page, or operation state/availability with exact minimal
peer/message receipt and bounded static failure advice. No message text, raw TL,
access hashes, session data, remote descriptions or arbitrary cached JSON in action
receipts/diagnostics. Event content retains Reader's explicit untrusted marker.

Prove strict v8 migration/rollback/storage-only verification; account/peer/file/
callback scope constraints and ownership; quota rollback/contention/restart;
same-key success/Rejected/Unknown/NotSent behavior; deadline expiry while identity
waits; confirmed send/edit wire receipts and malformed/mismatched results; scope
revocation/approvals and foreign/manual rows; caller Drop, joined close and actual
process termination around begin/dispatch/confirmation. Use synthetic encrypted
sessions, actual M2 TL codecs and socket-free fixtures. Run locked workspace/default
tests, doctests, fmt/Clippy, Python/schema/SDK verification and audit. M3 remains
open until callback/files and all eight actions reach shared CLI/MCP execution.

## Implemented evidence

The v8 scope ledger and six-action Rust Executor are implemented. Twenty-three
additional tests comprise three migration cases, five scope/admission cases,
thirteen executor integration cases, one lease/policy-wait case and one M2
send/edit cap case. The integration suite includes actual child-process termination
before begin, after begin and after confirmation; recovery resumes only the known
unstarted attempt and never reissues the uncertain write. It also covers caller
Drop with retained capacity, cancelled/resumed close, buffered reply revocation,
approved receipt scopes/owners, quota restart, no-RPC cache reads with a closed
backend and explicit host preparation after a known missing-peer NotSent.

An initial migration regression failed at version 7 and passes at version 8.
The real-TL first-send/edit/cache path and socket-free mtproto_authorized_text
example pass. Contract review required the final caller-side guard; the executor
preserves Reader's filtering revision across the job reply. Storage, lifecycle/
disclosure and write/deadline reviews have no unresolved findings. Fresh intent
randomness is generated only after prepare identifies an executable attempt.
Cross-model review was skipped in this autonomous continuation.

Windows/Rust 1.98.0: 588 all-feature / 369 default workspace tests, 16 doctests,
Clippy/fmt, isolated builds, deterministic protocol/agent schemas, 25 Python
checks and four pinned SDK verifiers pass. Fresh cargo audit scans the unchanged
363 dependencies and reports one new informational unmaintained warning for
chrono 0.4.45 (RUSTSEC-2026-0543), transitively used by rmcp 3.10.0. No advisory
ignore was added; removal is an explicit Q3 release requirement. This is not a
clean-audit or release-completion claim. Callback/file providers and shared
CLI/MCP execution remain M3; all release gates remain open.
