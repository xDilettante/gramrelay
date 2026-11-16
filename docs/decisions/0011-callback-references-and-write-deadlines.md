# 0011: Callback references and absolute write deadlines

Status: accepted for A1 increment 3. Files/send_document and full A1 remain open.

## Context

A callback ID supplied by an agent does not establish its source, chat scope or
freshness. A different operation key must not bypass an ambiguous prior answer.
Wall time alone cannot prove freshness after clock rollback and restart.

## Decision

Authorized update reads or explicit trusted-host observation register bounded
references from the same journal instance/profile. Only a single unambiguous
callback event can issue a reference. Current read or answer scope is necessary;
observation never supplies approval. References bind principal/profile, actual
chat/global scope and original receipt time. TTL defaults to ten seconds, at most
sixty; it is local policy rather than Telegram's acceptance guarantee.

The default JournalMonotonic mode requires private ingestion provenance captured
before the append job and published only after a fresh commit. A bounded cache
never refreshes duplicates. Restart or another journal instance lacks this proof
and fails closed. A host can explicitly select TrustedWallClock for historical
or cross-process observations, accepting responsibility for UTC correctness.
Both modes retain original wall expiry and a per-reference monotonic deadline.
No platform clock dependency, unsafe code or durable schema change is needed.

The durable callback identity is the profile plus SHA-256 of a domain separator
and exact callback ID, independent of principal, registry or timestamps. The v4
unique admission index permanently binds it to one operation after begin, even
after NotSent, expiry or receipt cleanup. A new alias already claimed elsewhere
fails before identity RPC. Concurrent claims still resolve atomically at begin.
Confirmed NotSent may retry the same key, spending a new admission; Unknown never
replays. get_operation can disclose a currently authorized retained receipt
without a live callback reference. Answer receipts contain acknowledged=true.

Send, normal-target edit and callback answer share one journal write path and
one bounded SDK attempt. Edit success must match both requested chat and message;
boolean-only success is ambiguous. RPC deadlines anchor to the persisted lease
before policy waits and are capped by remaining callback time. Receipt persistence
has its own bounded margin within that lease. Cached callback disclosure checks
current reference lifetime as well as current policy; expiry uses get_operation
for recovery. Revocation after dispatch cannot undo the remote effect.

## Evidence and consequences

Focused mocks cover twelve callback and three edit cases: source/principal/scope,
global permission, registry capacity, restart/clock trust, duplicate ingestion,
approval expiry, durable aliases, uncertain effects, confirmed NotSent and target
validation. Receipt-commit expiry and policy-lock delay regressions were observed
failing before fixes and passing afterward. Two cache unit tests cover bounded
provenance and reclamation. No live Telegram calls are used.

The default mode deliberately cannot answer an old callback after journal restart;
safe receipt recovery remains available. Registry exhaustion preserves readable
untrusted events while omitting action metadata. Full Telegram payloads stay
untrusted content rather than authorization or tool definitions.
