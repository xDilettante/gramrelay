# Durable authorized admission and safe receipts

Status: accepted for the A1 send_message slice; remaining actions are incomplete.

SQLite schema v4 adds budget ledgers and operation scopes without changing the
reviewed v1-v3 tables. Migration validates every prior DDL object and commits
atomically. A scope binds the immutable bot/action/chat/file/reference intent to
the J1 row at prepare. Existing unscoped manual rows fail closed at the executor
boundary. A partial unique index claims a callback hash only at begin, so aliases
cannot obtain a second admitted operation after Unknown or confirmed NotSent.

Budget identity is host configuration, independent of grant revision. Under the
write lock the executor checks current policy and charges invocations. New write
admission checks current policy again, atomically spends write/upload/cost units
and commits InFlight. Each NotSent retry spends a new attempt; cached results
spend only invocation quota. No ambiguous failure is refunded. Fixed-window
advancement retains a monotonic high-water bucket, cannot reopen old windows
after clock rollback and rejects duration changes for an existing identity.
Ledger capacity rejects new identities instead of evicting old counters.

The executor polls one lazy SDK attempt only after begin. InFlight exposes its
actual persisted lease deadline so operation lifetime also caps the RPC. Receipt
confirmation waits fairly for bounded native-worker admission and has its own
deadline. Timeout/cancellation or uncertain persistence after dispatch remains
Unknown. An admitted native job may commit after its async caller stops waiting;
recovery always uses the same permanent key. Existing-operation read failures
also retain Unknown because the earlier action may already have happened.

Revocation cannot undo an admitted remote effect. Complete the durable minimal
receipt, then recheck current authority before disclosure. Cache lookup validates
the persisted scope and projects allowlisted message/failure fields. Migration
chat IDs remain canonical strings; remote descriptions, message text and unknown
response fields are excluded. Starting a new attempt clears the previous
failure_json so an InFlight attempt cannot carry obsolete NotSent advice.

Verification includes competing SQLite connections, quota rollback/overflow,
window movement, restart, callback alias fencing, current-policy revocation
while waiting for SQLite and during RPC, cancellation, wrong target/identity,
blocked receipt commit and deterministic closure after prepare -> Existing.
Fresh adversarial review found the stale failure, Existing error outcome and
numeric migration-ID projection issues; each has a RED-to-GREEN regression.
