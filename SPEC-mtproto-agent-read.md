# Authorized MTProto event reads (M3)

Extends SPEC-mtproto-agent.md and SPEC-mtproto-agent-updates.md. Build the shared
durable event-read provider before adding remote actions and CLI/MCP dispatch.
No schema change, live Telegram call or additional authorization source.

## Provider contract

`mtproto::updates::Reader` binds one immutable Feed to the shared PolicyStore.
Host-only ReadOptions bound concurrent read executions and stored bytes inspected per
call (defaults 16 and 1 MiB; maxima 1024 and 64 MiB). It accepts the existing
immutable MTProto Request, Principal and optional host Approval. Only
mtproto_read_updates is supported here. Another profile, protocol family, account
kind/ID or unsupported action fails before reading content or charging a quota.
Plain storage access remains trusted and never issues an agent permit.

Dry-run parses and checks cursor position and current policy, but consumes no
quota and loads no event payload. Approval issuance uses the same provider-bound
preflight; it cannot turn an unrelated action/profile into a permitted read.
Foreign, malformed, future and expired cursors retain their explicit errors.

Execution checks policy/cursor before charging one invocation and configured
action cost through the existing durable budget transaction. Recheck current
authority inside that transaction. No quota refund on cancellation, storage
failure or post-charge revocation. A charged invocation is not a successful
disclosure. Restart keeps usage and cannot reset a budget window by clock rollback.

Read at most 100 source records within the configured byte bound, filter against
one current grant snapshot, and stop after the requested number of visible
events. Only Event::peer() decides scope; sender/actor IDs are untrusted metadata.
Events without a reviewed primary peer require explicit unscoped_updates. Never
reclassify control, unknown or inline records as scoped using nested metadata.
Denied records may advance the cursor; stop at the last considered record when
the visible limit is reached. Empty filtered pages can still have_more. Cursor
positions/has_more reveal bounded journal progress, as in Bot API read_updates.

Cursor validation, committed quota charging, bounded record decoding and filtering
run in one owned native job. It retains the read permit after caller cancellation
until the native work finishes; no detached Tokio task or additional worker is
created. Metadata-only dry-run/approval use the journal's existing native limits.

Recheck the grant and approval after the storage await and immediately before
return, rejecting any grant revision change during filtering. A concurrent
replacement/revocation cannot disclose a page assembled under stale authority.
The final synchronous policy check is the disclosure decision; later revocation
cannot recall already returned data.

The serializable page contains only safe Event projection, typed primary scope,
cursor, receipt time, conservative checkpoint confirmation and untrusted=true.
It contains neither callback query ID/provenance nor executable references. Query
registration remains the subsequent M3 provider. The reader performs no remote
RPC and does not assert current login, callback freshness or a new source ACK.
Debug/errors remain static and redacted. The future action executor/CLI/MCP must
reuse this provider, not create a weaker alternate read path.

## Acceptance

Prove exact profile/account/protocol/action refusal, allowlisted primary-peer
filtering, explicit unscoped controls/unknown/callbacks, pagination and cursor
errors, bounded bytes/concurrency, approvals/expiry/revocation, cancellation and
persistent invocation/cost usage. Exercise policy changes while native work is
queued and after records are loaded. End-to-end proof uses real journal workers
and encrypted M2 intake fixtures; unit boundary tests may inject private projections.
Run locked workspace tests, doctests, fmt/Clippy, isolated builds and existing
schema/audit gates.

## Implemented evidence

The Reader and shared native quota/read helpers are implemented. Ten unit cases
cover exact profile/protocol/account/action checks, primary peers and unscoped
controls/callbacks, 100-record scanning, cursor metadata/expiry, byte/concurrency
bounds, approvals and expiry, persistent invocation/cost limits, queued native
revocation/cancellation and post-load/pre-disclosure revision checks. The real
encrypted M2 intake integration also reads through this provider: sender scope
does not disclose messages, the primary peer does, baseline remains unscoped and
no additional identity/update RPC occurs.

Fresh adversarial review found that a cancelled caller released its concurrency
permit while native work continued. The deterministic single-slot regression
failed before the fix and passes with the permit moved into the one owned native
job. A caller-side recheck still protects pages already buffered in the reply
channel. Re-review has no unresolved findings. External cross-model review was
skipped in this autonomous continuation.

Windows/Rust 1.98.0: 565 all-feature / 365 default workspace tests and 16 doctests,
clean Clippy/fmt, isolated storage/MTProto builds, 25 Python checks, deterministic
protocol/agent schemas, four SDK verifiers and audit of 363 dependencies pass.
No live Telegram, callback execution, remote MTProto action admission or CLI/MCP
integration is claimed. M3 and Q1–Q4 remain open.
