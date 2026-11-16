# 0012: Immutable files for authorized document uploads

Status: accepted for A1 file registration and send_document.

## Decision

Trusted hosts register owned byte snapshots and receive random opaque handles.
Agents supply only a handle and plain caption; no local paths, remote URLs or raw
Telegram file IDs cross the tool boundary. Registration creates no grants. The
host grants the exact handle separately. A restored snapshot must match retained
owner/profile, size and SHA-256. Metadata has no overwrite or deletion API.

Schema v5 adds executor_files and its owner index; prior DDL stays unchanged.
Migration validates existing DDL and preserves operation scopes, receipts, update
positions and quota counters. A bounded permanent handle table rejects capacity
instead of reusing keys. Metadata and filenames are private, redacted diagnostics
and protected by the journal directory's host-managed permissions.

Resident and pending snapshots reserve count and actual Vec capacity before any
await or native hash job. Hash work moves both data and permits to a blocking
worker, so cancellation cannot release uncompleted work's memory charge. Multipart
uploads share the snapshot through Bytes::from_owner, retaining its memory/count
permits and a separate active-upload byte permit until body ownership ends. No
second full-file buffer is created. Discard/prune removes a map reference; an
outstanding upload retains the snapshot and its permits.

The existing write pipeline binds metadata to canonical intent and persists file
scope. Atomic begin charges full immutable bytes with the write/cost admission.
New uploads require live matching bytes before identity and begin; expiry is
rechecked inside the shared identity initializer, native begin and before SDK
dispatch. Identity/RPC budgets are capped by remaining expiry and operation lease.
Already retained receipts remain readable with current file/chat/action authority
after expiry, discard or restart. A new post-restart upload requires explicit
matching restoration. Unknown never replays; confirmed NotSent retries the same
key and spends another upload admission.

## Evidence and limits

Focused tests cover metadata ownership/nonreuse/shared capacity, filename and
retained-allocation limits, exact multipart bytes, restoration/restart, expiry,
current cached-result scope, exact approval, quota atomicity, SDK NotSent and
uncertain/wrong-target receipts. Two review findings have observed RED→GREEN
regressions: an uncharged pending restore and late identity initialization after
file expiry. Owner-lifetime tests cover registry discard and unpolled/consumed SDK
bodies. Migration tests cover v4→v5 preservation and malformed-v4 rollback.

File/grant expiry uses host UTC correctness across restarts. The host's own
preexisting input allocations are outside executor ownership until accepted.
Registration cancellation can leave immutable durable metadata without bytes;
safe new uploads require matching restoration. No live Telegram call is needed.

Sources: [Bytes ownership](https://docs.rs/bytes/1.12.1/bytes/struct.Bytes.html#method.from_owner)
and [known-length multipart bodies](https://docs.rs/reqwest/0.13.5/reqwest/multipart/struct.Part.html#method.stream_with_length).
