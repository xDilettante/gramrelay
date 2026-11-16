# Durable intake and consumer positions

Status: accepted for J2, 2026-11-16.

Telegram acknowledges earlier updates as soon as getUpdates receives a greater offset. A handler's successful return and a received HTTP response are insufficient evidence that the application can recover those events after a crash. Polling therefore commits the entire validated batch, deduplication fingerprints, capacity counters and next offset in one SQLite transaction. Every subsequent request reloads that checkpoint. Empty responses do not refresh its age; after 24 hours without a nonempty commit, polling omits the old offset. Switching source mode invalidates it. Raw API callers remain responsible for their own acknowledgement discipline.

Each feed has a local monotonically increasing sequence independent of Telegram IDs. Each named consumer has its own committed position and renewable worker/generation lease. Reads return a private acknowledgement proof and do not advance that position. Successful bounded batch dispatch permits confirmation by compare-and-swap against the proof's starting position. Handler errors, panics, timeouts and lease loss leave it unchanged. Already performed external effects can replay after a crash before confirmation; stable operation IDs and the operation journal address that boundary without claiming exactly-once delivery.

Leases and in-process proofs bind their issuing journal instance in addition to feed/consumer/generation. SDK-managed sources share a process-local guard based on the normalized bot ID, including rotated tokens. A database source lease coordinates instances using the same database. Startup verifies getMe identity and refuses an active webhook. These controls cannot revoke an external RPC already dispatched before a process suspension or coordinate arbitrary raw consumers in other databases.

Retention expiry is explicit. Consumer positions and sequence numbers never rewind; a lagging consumer stops with CursorExpired until the host explicitly skips the expired prefix. Consumer IDs are bounded and retained after release. Cursors, chat ordering hints, journal handles and leases are not authorization grants. A1 must check present permissions before reading or disclosing events.

Fresh adversarial review and failing regressions identified three runtime boundaries:

- A cancelled async DB caller does not cancel its native job or free its permit. Release and consumer confirmation wait fairly for a queue slot, then execute behind admitted work; ordinary admission remains fail-fast. Closing the journal wakes waiters and releases only this worker's leases.
- Reading can consume most of a lease while waiting for SQLite. Dispatch renews after reading and checks a remaining lifetime margin before admitting handlers. Heartbeat renewal has a deadline before the known expiry; a queued renewal alone does not prove ownership.
- Handler completion can race an already admitted renewal. Confirmation waits behind that job, preventing an artificial Capacity failure and unnecessary replay. Shutdown deadlines also cover pending renewal futures. Cooperative async handlers must yield; no runtime can forcibly undo their already started external effects or independently spawned tasks.

Schema v3 adds consumer positions to reviewed v2 update storage and v1 operation storage. Migration validates each source schema and commits its complete new schema atomically. Tests terminate real child processes after receive, intake commit, synthetic handler effect and consumer confirmation, and race separate source-owning processes. Offline runtime mocks verify that a full journal cannot send a premature next offset and that restart uses the committed offset.
